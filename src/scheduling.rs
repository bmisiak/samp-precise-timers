use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
    time::Instant,
};

use slab::Slab;
use snafu::{ensure, Snafu};

use crate::{schedule::Schedule, timer::Timer};

#[derive(Debug)]
struct TimerState {
    timer: Timer,
    schedule: Cell<Schedule>,
    key: usize,
}

struct State {
    /// A slotmap of timers. Stable keys.
    timers: Slab<Weak<TimerState>>,
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
        let new_position = queue.partition_point(|s| s.schedule.get() < schedule);
        let schedule = Cell::new(schedule);
        let rc = Rc::new(TimerState { timer, schedule, key });
        entry.insert(Rc::downgrade(&rc));
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
    STATE.with_borrow_mut(|State { queue, timers }| {
        let old_state = timers[key].upgrade().unwrap();
        let old_index = queue
            .binary_search_by_key(&old_state.schedule.get(), |ts| ts.schedule.get())
            .unwrap();

        let new_index = queue.partition_point(|s| s.schedule.get() < new_schedule);
        queue[old_index].schedule.replace(new_schedule);
        if new_index < old_index {
            queue[new_index..=old_index].rotate_right(1);
        } else if new_index > old_index {
            queue[old_index..=new_index].rotate_left(1);
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
    stack_callback: impl FnOnce(&Timer) -> T,
) -> Option<T> {
    STATE.with_borrow_mut(|State { timers, queue }| {
        let next_up = queue.last()?;
        let Schedule { next_trigger, repeat } = next_up.schedule.get();
        if next_trigger > now {
            return None;
        }
        if let Some(interval) = repeat {
            let stacked_callback = stack_callback(&next_up.timer);

            let next_trigger = now + interval;
            let new_schedule = Schedule { next_trigger, repeat };
            let old_position = queue.len() - 1; // next timer is at the end of the queue
            let new_position = queue.partition_point(|s| s.schedule.get() >= new_schedule);

            next_up.schedule.replace(new_schedule);

            if new_position < old_position {
                queue[new_position..].rotate_right(1);
            } else {
                debug_assert_eq!(new_position, old_position);
            }
            Some(stacked_callback)
        } else {
            let unscheduled = queue.pop().expect("due timer should be in queue");
            timers.remove(unscheduled.key);

            Some(stack_callback(&unscheduled.timer))
        }
    })
}

#[cfg(test)]
mod test {
    use std::ptr::null_mut;

    use durr::{now, Durr};

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
            next_trigger: now() + (key as u64).seconds(),
            repeat: Some(1.seconds()),
        }
    }

    fn dont_repeat(key: usize) -> Schedule {
        Schedule {
            next_trigger: now() + (key as u64).seconds(),
            repeat: None,
        }
    }

    fn timer_keys(q: &Vec<std::rc::Rc<super::TimerState>>) -> Vec<usize> {
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
