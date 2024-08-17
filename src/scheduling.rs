use std::{cell::RefCell, rc::Rc, time::Instant};

use slab::Slab;
use snafu::{ensure, OptionExt, Snafu};

use crate::{
    schedule::{Repeat, Schedule},
    timer::Timer,
};

struct TimerState {
    timer: Timer,
    schedule: RefCell<Schedule>,
    key: usize,
}

struct State {
    /// A slotmap of timers. Stable keys.
    timers: Slab<Rc<TimerState>>,
    /// Always sorted queue of timers. Easy O(1) peeking and popping of the next scheduled timer.
    queue: Vec<Rc<TimerState>>,
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
        let entry = timers.vacant_entry();
        let key = entry.key();
        let schedule = get_schedule_based_on_key(key);
        let rc = Rc::new(TimerState {
            timer,
            schedule: RefCell::new(schedule),
            key,
        });
        entry.insert(Rc::clone(&rc));
        let new_position = queue.partition_point(|s| *s.schedule.borrow() < schedule);
        queue.insert(new_position, rc);
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
        let new_index = queue.partition_point(|s| *s.schedule.borrow() < new_schedule);
        queue[current_index].schedule.replace(new_schedule);
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
        queue.retain(|timer_state| {
            if predicate(&timer_state.timer) {
                timers.remove(timer_state.key);
                false
            } else {
                true
            }
        });
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
        let (key, repeat, timer) = {
            let Some(coming_up) = queue.last() else {
                return None;
            };
            let schedule = coming_up.schedule.borrow();
            let next_trigger = schedule.next_trigger;

            if next_trigger > now {
                return None;
            }
            (coming_up.key, schedule.repeat, &coming_up.timer)
        };
        if let Repeat::Every(interval) = repeat {
            let next_trigger = now + interval;
            let old_position = queue.len() - 1;
            let new_position =
                queue.partition_point(|s| s.schedule.borrow().next_trigger >= next_trigger);
            queue[old_position].schedule.borrow_mut().next_trigger = next_trigger;

            let result = timer_manipulator(timer);

            if new_position < old_position {
                queue[new_position..].rotate_right(1);
            } else {
                debug_assert_eq!(new_position, old_position);
            }

            Some(result)
        } else {
            let descheduled = queue.pop().expect("due timer should be in queue");
            debug_assert_eq!(key, descheduled.key);
            timers.remove(key);

            Some(timer_manipulator(&descheduled.timer))
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
