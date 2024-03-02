use std::{
    cell::{BorrowMutError, RefCell},
    time::Instant,
};

use fnv::FnvHashSet;
use samp::error::AmxError;
use slab::Slab;
use snafu::{OptionExt, ResultExt, Snafu};

use crate::{
    schedule::{Repeat, Schedule},
    timer::Timer,
};

thread_local! {
    /// A slotmap of timers. Stable keys.
    static TIMERS: RefCell<Slab<Timer>> = RefCell::new(Slab::with_capacity(1000));
    /// Always sorted queue of timers. Easy O(1) peeking and popping of the next scheduled timer.
    static QUEUE: RefCell<Vec<Schedule>> = RefCell::new(Vec::with_capacity(1000));
}

#[derive(Debug, Snafu)]
pub(crate) enum TriggeringError {
    #[snafu(display("Error detected while executing callback for timer number {}", timer_key + 1))]
    Callback { source: AmxError, timer_key: usize },
    #[snafu(display("Unable to find timer in priority queue"))]
    TimerNotInQueue,
    #[snafu(display("Failed to get access to priority queue"))]
    QueueBorrowed { source: BorrowMutError },
    #[snafu(display("Inserting timer failed, unable to access store"))]
    Inserting { source: BorrowMutError },
    #[snafu(display("Popped timer is different from the expected due timer"))]
    ExpectedInSlab,
    #[snafu(display("Unable to push arguments onto AMX stack"))]
    StackPush { source: AmxError },
}

pub(crate) fn insert_and_schedule_timer(
    timer: Timer,
    schedule_getter: impl FnOnce(usize) -> Schedule,
) -> Result<usize, TriggeringError> {
    let key: usize = TIMERS
        .with(|t| t.try_borrow_mut().map(|mut t| t.insert(timer)))
        .context(QueueBorrowedSnafu)?;
    let schedule = schedule_getter(key);
    QUEUE
        .with(|q| {
            q.try_borrow_mut().map(|mut q| {
                let new_position = q.partition_point(|s| s < &schedule);
                q.insert(new_position, schedule)
            })
        })
        .context(QueueBorrowedSnafu)?;
    Ok(key)
}

pub(crate) fn delete_timer(timer_key: usize) -> Result<(), TriggeringError> {
    TIMERS
        .with(|t| {
            t.try_borrow_mut().map(|mut t| {
                if t.get(timer_key).is_some() {
                    t.remove(timer_key);
                }
            })
        })
        .context(QueueBorrowedSnafu)?;
    QUEUE
        .with(|q| {
            q.try_borrow_mut()
                .map(|mut q| q.retain(|s| s.key != timer_key))
        })
        .context(QueueBorrowedSnafu)?;
    Ok(())
}

pub(crate) fn reschedule_timer(key: usize, new_schedule: Schedule) -> Result<(), TriggeringError> {
    QUEUE.with(|q| {
        let mut q = q.try_borrow_mut().context(QueueBorrowedSnafu)?;
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
            let new_position =
                q[..old_position].partition_point(|s| s.next_trigger >= next_trigger);
            q[old_position].next_trigger = next_trigger;
            if new_position < old_position {
                q[new_position..].rotate_right(1);
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

    use crate::schedule::Repeat;
    use crate::Timer;
    use crate::{amx_arguments::VariadicAmxArguments, scheduling::trigger_next_due_and_then};

    use super::{insert_and_schedule_timer, Schedule};

    fn mock_no_arg_timer() -> Timer {
        let amx_pointer: *mut AMX = null_mut();
        Timer {
            passed_arguments: VariadicAmxArguments::empty(),
            amx_callback_index: samp::consts::AmxExecIdx::Continue,
            amx_identifier: amx_pointer.into(),
        }
    }

    #[test]
    fn hello() {
        insert_and_schedule_timer(
            mock_no_arg_timer(),
            Schedule {
                next_trigger: std::time::Instant::now(),
                repeat: Repeat::DontRepeat,
            },
        )
        .unwrap();
        let callback = trigger_next_due_and_then(std::time::Instant::now(), |_timer| ());
        assert!(callback.is_some());
    }
}
