use std::{cell::RefCell, time::Instant};

use fnv::FnvHashSet;
use slab::Slab;
use snafu::{ensure, OptionExt, Snafu};

use crate::{
    schedule::{Repeat, Schedule},
    timer::Timer,
};

struct State {
    /// A slotmap of timers. Stable keys.
    timers: Slab<Timer>,
    /// Always sorted queue of timers. Easy O(1) peeking and popping of the next scheduled timer.
    queue: Vec<Schedule>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        timers: Slab::with_capacity(1000),
        queue: Vec::with_capacity(1000),
    })
}

#[derive(Debug, Snafu)]
#[snafu(context(suffix(false)))]
pub(crate) enum TriggeringError {
    #[snafu(display("Unable to find timer in priority queue"))]
    TimerNotInQueue,
}

pub(crate) fn insert_and_schedule_timer(
    timer: Timer,
    get_schedule_based_on_key: impl FnOnce(usize) -> Schedule,
) -> usize {
    STATE.with_borrow_mut(|State { timers, queue }| {
        let key = timers.insert(timer);
        let schedule = get_schedule_based_on_key(key);
        let new_position = queue.partition_point(|s| s < &schedule);
        queue.insert(new_position, schedule);
        key
    })
}

pub(crate) fn delete_timer(timer_key: usize) -> Result<(), TriggeringError> {
    STATE.with_borrow_mut(|State { timers, queue }| {
        ensure!(timers.contains(timer_key), TimerNotInQueue);
        timers.remove(timer_key);
        queue.retain(|s| s.key != timer_key);
        Ok(())
    })
}

pub(crate) fn reschedule_timer(key: usize, new_schedule: Schedule) -> Result<(), TriggeringError> {
    STATE.with_borrow_mut(|State { queue, .. }| {
        let current_index = queue
            .iter()
            .position(|s| s.key == key)
            .context(TimerNotInQueue)?;
        let new_index = queue.partition_point(|s| s < &new_schedule);
        queue[current_index].next_trigger = new_schedule.next_trigger;
        queue[current_index].repeat = new_schedule.repeat;
        if new_index < current_index {
            queue[new_index..=current_index].rotate_right(1);
        } else if new_index > current_index {
            queue[current_index..=new_index].rotate_left(1);
        }
        Ok(())
    })
}

pub(crate) fn remove_timers(predicate: impl Fn(&Timer) -> bool) {
    STATE.with_borrow_mut(|State { timers, queue }| {
        let mut removed_keys = FnvHashSet::default();
        queue.retain(|&Schedule { key, .. }| {
            if predicate(&timers[key]) {
                removed_keys.insert(key);
                false
            } else {
                true
            }
        });
        for key in removed_keys {
            timers.remove(key);
        }
    });
}

/// 1. Reschedules (or deschedules) the timer
/// 2. While holding the timer, gives it to the closure
///    (which uses its data to push onto the amx stack)
/// 3. Frees state.
/// 4. Returns the result of the closure.
/// `timer_manipulator` must not borrow state
#[inline]
pub(crate) fn reschedule_next_due_and_then<T>(
    now: Instant,
    timer_manipulator: impl FnOnce(&Timer) -> T,
) -> Option<T> {
    STATE.with_borrow_mut(|State { timers, queue }| {
        let Some(scheduled @ &Schedule { key, .. }) = queue.last() else {
            return None;
        };
        if scheduled.next_trigger > now {
            return None;
        }

        if let Repeat::Every(interval) = scheduled.repeat {
            let next_trigger = now + interval;
            let old_position = queue.len() - 1;
            let new_position = queue.partition_point(|s| s.next_trigger >= next_trigger);
            queue[old_position].next_trigger = next_trigger;
            if new_position < old_position {
                queue[new_position..].rotate_right(1);
            } else {
                debug_assert_eq!(new_position, old_position);
            }

            let timer = timers.get_mut(key).expect("due timer should be in slab");
            Some(timer_manipulator(timer))
        } else {
            let descheduled = queue.pop().expect("due timer should be in queue");
            assert_eq!(descheduled.key, key);

            let timer = timers.remove(key);
            Some(timer_manipulator(&timer))
        }
    })
}

#[cfg(test)]
mod test {
    use std::ptr::null_mut;

    use durr::{now, Durr};

    use crate::schedule::Repeat::{DontRepeat, Every};
    use crate::scheduling::{State, STATE};
    use crate::Timer;
    use crate::{amx_arguments::VariadicAmxArguments, scheduling::reschedule_next_due_and_then};

    use super::{insert_and_schedule_timer, Schedule};

    fn empty_timer() -> Timer {
        Timer {
            passed_arguments: VariadicAmxArguments::empty(),
            amx_callback_index: samp::consts::AmxExecIdx::Continue,
            amx: samp::amx::Amx::new(null_mut(), 0),
        }
    }

    fn noop(_timer: &Timer) {}

    fn every_1s(key: usize) -> Schedule {
        Schedule {
            key,
            next_trigger: now() + (key as u64).seconds(),
            repeat: Every(1.seconds()),
        }
    }

    fn dont_repeat(key: usize) -> Schedule {
        Schedule {
            key,
            next_trigger: now() + (key as u64).seconds(),
            repeat: DontRepeat,
        }
    }

    fn timer_keys(q: &Vec<Schedule>) -> Vec<usize> {
        dbg!(q);
        q.iter().map(|s| s.key).collect()
    }

    #[test]
    fn hello() {
        assert_eq!(reschedule_next_due_and_then(now(), noop), None);
        let first = insert_and_schedule_timer(empty_timer(), every_1s);
        let second = insert_and_schedule_timer(empty_timer(), every_1s);
        let third = insert_and_schedule_timer(empty_timer(), every_1s);
        let fourth = insert_and_schedule_timer(empty_timer(), dont_repeat);
        STATE.with_borrow_mut(|&mut State { ref mut queue, .. }| {
            assert_eq!(timer_keys(queue), [fourth, third, second, first]);
        });
        assert!(reschedule_next_due_and_then(now(), noop).is_some());
        STATE.with_borrow_mut(|&mut State { ref mut queue, .. }| {
            assert_eq!(timer_keys(queue), [fourth, third, first, second]);
        });
        assert_eq!(reschedule_next_due_and_then(now(), noop), None);
    }
}
