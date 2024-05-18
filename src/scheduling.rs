use std::{cell::RefCell, time::Instant};

use fnv::FnvHashSet;
use samp::error::AmxError;
use slab::Slab;
use snafu::{ensure, OptionExt, Snafu};

use crate::{
    schedule::{Repeat, Schedule},
    timer::Timer,
};

thread_local! {
    /// A slotmap of timers. Stable keys.
    static TIMERS: RefCell<Slab<Timer>> = RefCell::new(Slab::with_capacity(1000));
    /// Always sorted queue of timers. Easy O(1) peeking and popping of the next scheduled timer.
    pub static QUEUE: RefCell<Vec<Schedule>> = RefCell::new(Vec::with_capacity(1000));
}

#[derive(Debug, Snafu)]
pub(crate) enum TriggeringError {
    #[snafu(display("Error detected while executing callback for timer number {}", timer_key + 1))]
    Callback { source: AmxError, timer_key: usize },
    #[snafu(display("Unable to find timer in priority queue"))]
    TimerNotInQueue,
    #[snafu(display("Unable to push arguments onto AMX stack"))]
    StackPush { source: AmxError },
}

pub(crate) fn insert_and_schedule_timer(
    timer: Timer,
    schedule_getter: impl FnOnce(usize) -> Schedule,
) -> Result<usize, TriggeringError> {
    let key: usize = TIMERS.with_borrow_mut(|t| t.insert(timer));
    let schedule = schedule_getter(key);
    QUEUE.with_borrow_mut(|q| {
        let new_position = q.partition_point(|s| s < &schedule);
        q.insert(new_position, schedule);
    });
    Ok(key)
}

pub(crate) fn delete_timer(timer_key: usize) -> Result<(), TriggeringError> {
    TIMERS.with_borrow_mut(|t| {
        ensure!(t.contains(timer_key), TimerNotInQueueSnafu);
        t.remove(timer_key);
        Ok(())
    })?;
    QUEUE.with_borrow_mut(|q| q.retain(|s| s.key != timer_key));
    Ok(())
}

pub(crate) fn reschedule_timer(key: usize, new_schedule: Schedule) -> Result<(), TriggeringError> {
    QUEUE.with_borrow_mut(|q| {
        let current_index = q
            .iter()
            .position(|s| s.key == key)
            .context(TimerNotInQueueSnafu)?;
        let new_index = q.partition_point(|s| s < &new_schedule);
        q[current_index].next_trigger = new_schedule.next_trigger;
        q[current_index].repeat = new_schedule.repeat;
        if new_index < current_index {
            q[new_index..=current_index].rotate_right(1);
        } else if new_index > current_index {
            q[current_index..=new_index].rotate_left(1);
        }
        Ok(())
    })
}

pub(crate) fn remove_timers(predicate: impl Fn(&Timer) -> bool) {
    let mut deleted_timers = FnvHashSet::default();
    TIMERS.with_borrow_mut(|timers| {
        timers.retain(|key, timer| {
            if predicate(timer) {
                deleted_timers.insert(key);
                false
            } else {
                true
            }
        });
    });

    QUEUE.with_borrow_mut(|queue| {
        queue.retain(|schedule| !deleted_timers.contains(&schedule.key));
    });
}

/// 1. Reschedules (or deschedules) the timer
/// 2. While holding the timer, gives it to the closure
///    (which uses its data to push onto the amx stack)
/// 3. Frees TIMERS and QUEUE.
/// 4. Returns the result of the closure.
/// `prep` must not make additional references of stores.
#[inline]
pub(crate) fn trigger_next_due_and_then<T>(
    now: Instant,
    timer_manipulator: impl Fn(&Timer) -> T,
) -> Option<T> {
    QUEUE.with_borrow_mut(|q| {
        let Some(scheduled @ &Schedule { key, .. }) = q.last() else {
            return None;
        };
        if scheduled.next_trigger > now {
            return None;
        }

        if let Repeat::Every(interval) = scheduled.repeat {
            let next_trigger = now + interval;
            let old_position = q.len() - 1;
            let new_position = q.partition_point(|s| s.next_trigger >= next_trigger);
            q[old_position].next_trigger = next_trigger;
            if new_position < old_position {
                q[new_position..].rotate_right(1);
            } else {
                debug_assert_eq!(new_position, old_position);
            }

            TIMERS.with_borrow_mut(|t| {
                let timer = t.get_mut(key).expect("due timer should be in slab");
                Some(timer_manipulator(timer))
            })
        } else {
            let descheduled = q.pop().expect("due timer should be in queue");
            assert_eq!(descheduled.key, key);

            let timer = TIMERS.with_borrow_mut(|t| t.remove(key));
            Some(timer_manipulator(&timer))
        }
    })
}

#[cfg(test)]
mod test {
    use std::ptr::null_mut;

    use samp::raw::types::AMX;

    use crate::schedule::Repeat::{DontRepeat, Every};
    use crate::scheduling::QUEUE;
    use crate::Timer;
    use crate::{amx_arguments::VariadicAmxArguments, scheduling::trigger_next_due_and_then};
    use std::time::{Duration, Instant};

    use super::{insert_and_schedule_timer, Schedule};

    fn empty_timer() -> Timer {
        let amx_pointer: *mut AMX = null_mut();
        Timer {
            passed_arguments: VariadicAmxArguments::empty(),
            amx_callback_index: samp::consts::AmxExecIdx::Continue,
            amx_identifier: amx_pointer.into(),
        }
    }

    fn noop(_timer: &Timer) {}

    fn every_1s(key: usize) -> Schedule {
        Schedule {
            key,
            next_trigger: Instant::now() + Duration::from_secs(key as u64),
            repeat: Every(Duration::from_secs(1)),
        }
    }

    fn dont_repeat(key: usize) -> Schedule {
        Schedule {
            key,
            next_trigger: Instant::now() + Duration::from_secs(key as u64),
            repeat: DontRepeat,
        }
    }

    fn timer_keys(q: &Vec<Schedule>) -> Vec<usize> {
        dbg!(q);
        q.iter().map(|s| s.key).collect()
    }

    #[test]
    fn hello() {
        assert_eq!(trigger_next_due_and_then(Instant::now(), noop), None);
        let first = insert_and_schedule_timer(empty_timer(), every_1s).unwrap();
        let second = insert_and_schedule_timer(empty_timer(), every_1s).unwrap();
        let third = insert_and_schedule_timer(empty_timer(), every_1s).unwrap();
        let fourth = insert_and_schedule_timer(empty_timer(), dont_repeat).unwrap();
        QUEUE.with_borrow(|q| {
            assert_eq!(timer_keys(q), [fourth, third, second, first]);
        });
        assert!(trigger_next_due_and_then(Instant::now(), noop).is_some());
        QUEUE.with_borrow(|q| {
            assert_eq!(timer_keys(q), [fourth, third, first, second]);
        });
        assert_eq!(trigger_next_due_and_then(Instant::now(), noop), None);
    }
}
