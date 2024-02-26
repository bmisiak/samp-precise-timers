use std::{
    cell::{BorrowMutError, RefCell},
    cmp::Reverse,
    time::{Duration, Instant},
};

use fnv::FnvBuildHasher;
use priority_queue::PriorityQueue;
use samp::error::AmxError;
use slab::Slab;
use snafu::{ensure, OptionExt, ResultExt, Snafu};

use crate::{amx_arguments::StackedCallback, timer::Timer};

thread_local! {
    /// A slotmap of timers. Stable keys.
    static TIMERS: RefCell<Slab<Timer>> = RefCell::new(Slab::with_capacity(1000));
    /// Always sorted queue of timers. Easy O(1) peeking and popping of the next scheduled timer.
    static QUEUE: RefCell<PriorityQueue<usize, Reverse<TimerScheduling>, FnvBuildHasher>> = RefCell::new(PriorityQueue::with_capacity_and_default_hasher(1000));
}

#[derive(Debug, Snafu)]
pub(crate) enum TriggeringError {
    #[snafu(display("Error detected while executing callback for timer number {}", timer_key + 1))]
    Callback { source: AmxError, timer_key: usize },
    #[snafu(display("Unable to reschedule"))]
    Rescheduling,
    #[snafu(display("Unable to reschedule 2"))]
    ReschedulingBorrow { source: BorrowMutError },
    #[snafu(display("Unable to deschedule"))]
    Descheduling,
    #[snafu(display("Timer was expected to be present in slab"))]
    Inconsistency,
    #[snafu(display("Peeked a timer, but popped a different one"))]
    ExpectedInSlab,
    #[snafu(display("The AMX which scheduled the timer disappeared"))]
    AmxGone,
    #[snafu(display("Unable to push arguments onto AMX stack"))]
    StackPush { source: AmxError },
}

#[derive(Copy, Clone)]
pub(crate) enum Repeat {
    Every(Duration),
    DontRepeat,
}

/// A struct defining when a timer gets triggered
#[derive(Clone)]
pub(crate) struct TimerScheduling {
    /// If Some, it's a repeating timer.
    /// If None, it will be gone after the next trigger.
    pub repeat: Repeat,
    /// The timer will be executed after this instant passes
    pub next_trigger: Instant,
}

impl PartialEq for TimerScheduling {
    fn eq(&self, other: &Self) -> bool {
        self.next_trigger == other.next_trigger
    }
}
impl Eq for TimerScheduling {}

impl PartialOrd for TimerScheduling {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for TimerScheduling {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.next_trigger.cmp(&other.next_trigger)
    }
}

pub(crate) fn insert_and_schedule_timer(timer: Timer, scheduling: TimerScheduling) -> usize {
    let key: usize = TIMERS.with_borrow_mut(|t| t.insert(timer));
    QUEUE.with_borrow_mut(|q| q.push(key, Reverse(scheduling)));
    key
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

fn deschedule_next_due(next_due: &NextDue) -> Result<(), TriggeringError> {
    let (popped_key, _) = QUEUE.with_borrow_mut(|q| q.pop().context(DeschedulingSnafu))?;
    ensure!(popped_key == next_due.key, InconsistencySnafu);
    Ok(())
}

fn change_next_trigger(key: usize, next_trigger: Instant) -> Result<(), TriggeringError> {
    QUEUE.with(|q| {
        q.try_borrow_mut()
            .context(ReschedulingBorrowSnafu)?
            .change_priority_by(&key, |Reverse(ref mut schedule)| {
                schedule.next_trigger = next_trigger;
            })
            .then_some(())
            .ok_or(TriggeringError::Rescheduling)
    })
}

pub(crate) fn reschedule_timer(
    key: usize,
    new_schedule: TimerScheduling,
) -> Result<(), TriggeringError> {
    QUEUE.with(|q| {
        q.try_borrow_mut()
            .context(ReschedulingBorrowSnafu)?
            .change_priority(&key, Reverse(new_schedule))
            .map(|_| ())
            .ok_or(TriggeringError::Rescheduling)
    })
}

#[derive(Copy, Clone)]
pub(crate) struct NextDue {
    pub key: usize,
    pub repeat: Repeat,
}

impl NextDue {
    /// 1. Reschedules (or deschedules) the timer
    /// 2. While holding the timer, executes the closure
    ///    which uses its data to push onto the amx stack
    /// 3. Frees TIMERS and QUEUE.
    /// 4. Returns the callback.
    pub(crate) fn bump_schedule_and(
        &self,
        now: Instant,
        prep: impl Fn(&Timer) -> Result<StackedCallback, AmxError>,
    ) -> Result<StackedCallback, TriggeringError> {
        if let Repeat::Every(interval) = self.repeat {
            let next_trigger = now + interval;
            change_next_trigger(self.key, next_trigger)?;

            TIMERS.with_borrow_mut(|t| {
                let timer: &mut Timer = t.get_mut(self.key).context(ExpectedInSlabSnafu)?;
                prep(timer).context(StackPushSnafu)
            })
        } else {
            deschedule_next_due(self)?;
            let timer = TIMERS.with_borrow_mut(|t| t.remove(self.key));
            prep(&timer).context(StackPushSnafu)
        }
    }
}

pub(crate) fn next_timer_due_for_triggering(now: Instant) -> Option<NextDue> {
    QUEUE.with_borrow(|q| match q.peek() {
        Some((
            &key,
            &Reverse(TimerScheduling {
                next_trigger,
                repeat,
            }),
        )) if next_trigger <= now => Some(NextDue { key, repeat }),
        _ => None,
    })
}

#[cfg(test)]
mod test {
    use std::ptr::null_mut;

    use samp::raw::types::AMX;

    use crate::Timer;
    use crate::{amx_arguments::VariadicAmxArguments, scheduling::next_timer_due_for_triggering};

    use super::{insert_and_schedule_timer, TimerScheduling};

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
            TimerScheduling {
                next_trigger: std::time::Instant::now(),
                repeat: super::Repeat::DontRepeat,
            },
        );
        let next_due = next_timer_due_for_triggering(std::time::Instant::now());
        
        assert!(next_due.is_some());
    }
}
