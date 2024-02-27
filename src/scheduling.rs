use std::{
    cell::{BorrowMutError, RefCell},
    cmp::Reverse,
    time::Instant,
};

use fnv::FnvBuildHasher;
use priority_queue::PriorityQueue;
use samp::error::AmxError;
use slab::Slab;
use snafu::{ensure, OptionExt, ResultExt, Snafu};

use crate::{
    schedule::{Repeat, Schedule},
    timer::Timer,
};

thread_local! {
    /// A slotmap of timers. Stable keys.
    static TIMERS: RefCell<Slab<Timer>> = RefCell::new(Slab::with_capacity(1000));
    /// Always sorted queue of timers. Easy O(1) peeking and popping of the next scheduled timer.
    static QUEUE: RefCell<PriorityQueue<usize, Reverse<Schedule>, FnvBuildHasher>> = RefCell::new(PriorityQueue::with_capacity_and_default_hasher(1000));
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
    Inconsistency,
    #[snafu(display("Timer was expected to be present in slab"))]
    ExpectedInSlab,
    #[snafu(display("The AMX which scheduled the timer disappeared"))]
    AmxGone,
    #[snafu(display("Unable to push arguments onto AMX stack"))]
    StackPush { source: AmxError },
}

pub(crate) fn insert_and_schedule_timer(
    timer: Timer,
    scheduling: Schedule,
) -> Result<usize, TriggeringError> {
    let key: usize = TIMERS
        .with(|t| t.try_borrow_mut().map(|mut t| t.insert(timer)))
        .context(InsertingSnafu)?;
    QUEUE
        .with(|q| {
            q.try_borrow_mut()
                .map(|mut q| q.push(key, Reverse(scheduling)))
        })
        .context(InsertingSnafu)?;
    Ok(key)
}

pub(crate) fn delete_timer(timer_key: usize) -> Result<Option<Timer>, BorrowMutError> {
    let Some((removed_key, _)) =
        QUEUE.with(|q| q.try_borrow_mut().map(|mut q| q.remove(&timer_key)))?
    else {
        return Ok(None);
    };
    Ok(Some(TIMERS.with(|t| {
        t.try_borrow_mut().map(|mut t| t.remove(removed_key))
    })?))
}

pub(crate) fn reschedule_timer(key: usize, new_schedule: Schedule) -> Result<(), TriggeringError> {
    QUEUE.with(|q| {
        q.try_borrow_mut()
            .context(QueueBorrowedSnafu)?
            .change_priority(&key, Reverse(new_schedule))
            .map(|_| ())
            .context(TimerNotInQueueSnafu)
    })
}

pub(crate) fn remove_timers(predicate: impl Fn(&Timer) -> bool) {
    TIMERS.with_borrow_mut(|timers| {
        QUEUE.with_borrow_mut(|queue| {
            timers.retain(|key, timer| {
                if predicate(timer) {
                    queue.remove(&key);
                    false
                } else {
                    true
                }
            });
        });
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
) -> Result<Option<T>, TriggeringError> {
    QUEUE.with_borrow_mut(|q| {
        let Some((&key, &Reverse(scheduled))) = q.peek() else {
            return Ok(None);
        };
        if scheduled.next_trigger > now {
            return Ok(None);
        }

        if let Repeat::Every(interval) = scheduled.repeat {
            q.change_priority_by(&key, |&mut Reverse(ref mut schedule)| {
                schedule.next_trigger = now + interval;
            });
            TIMERS.with_borrow_mut(|t| {
                let timer = t.get_mut(key).context(ExpectedInSlabSnafu)?;
                Ok(Some(timer_manipulator(timer)))
            })
        } else {
            let (descheduled, _) = q.pop().context(TimerNotInQueueSnafu)?;
            ensure!(descheduled == key, InconsistencySnafu);

            let timer = TIMERS.with_borrow_mut(|t| t.remove(key));
            Ok(Some(timer_manipulator(&timer)))
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
        );
        let callback =
            trigger_next_due_and_then(std::time::Instant::now(), |_timer| Ok(())).unwrap();
        assert!(callback.is_some());
    }
}
