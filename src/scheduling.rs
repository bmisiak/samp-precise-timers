
use std::{cell::{BorrowMutError, RefCell}, cmp::Reverse, time::{Duration, Instant}};

use fnv::FnvBuildHasher;
use priority_queue::PriorityQueue;
use samp::{amx::Amx, consts::AmxExecIdx, error::AmxError};
use slab::Slab;
use snafu::{ensure, OptionExt, ResultExt, Snafu};

use crate::timer::Timer;


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

/// A callback which MUST be executed.
/// Its args are already on the AMX stack.
#[must_use]
pub(crate) struct StackedCallbackData((&'static Amx, AmxExecIdx));

#[derive(Copy, Clone)]
pub(crate) enum Repeatability {
    Repeating(Duration),
    NotRepeating,
}

/// A struct defining when a timer gets triggered
#[derive(Clone)]
pub(crate) struct TimerScheduling {
    /// If Some, it's a repeating timer.
    /// If None, it will be gone after the next trigger.
    pub repeat: Repeatability,
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
        self.next_trigger.partial_cmp(&other.next_trigger)
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

pub(crate) fn remove_timers_from_amx(amx: &Amx) {
    let mut removed_timers = Vec::new();
    TIMERS.with_borrow_mut(|t| {
        t.retain(|key, timer| {
            if timer.was_scheduled_by_amx(amx) {
                removed_timers.push(key);
                false
            } else {
                true
            }
        });
    });
    QUEUE.with_borrow_mut(|q| {
        for key in removed_timers {
            q.remove(&key);
        }
    });
}

pub(crate) fn deschedule_timer(key: usize) -> Result<(), TriggeringError> {
    QUEUE.with_borrow_mut(|q| q.remove(&key).map(|_| ()).context(DeschedulingSnafu))
}

pub(crate) fn deschedule_next_timer_ensuring_key(key: usize) -> Result<(), TriggeringError> {
    let (popped_key, _) = QUEUE.with_borrow_mut(|q| q.pop().context(DeschedulingSnafu))?;
    ensure!(popped_key == key, InconsistencySnafu);
    Ok(())
}

pub(crate) fn reschedule_timer(key: usize, next_trigger: Instant) -> Result<(), TriggeringError> {
    QUEUE.with(|q| {
        q.try_borrow_mut()
            .context(ReschedulingBorrowSnafu)?
            .change_priority_by(&key, |Reverse(ref mut schedule)| {
                (*schedule).next_trigger = next_trigger
            })
            .then_some(())
            .ok_or(TriggeringError::Rescheduling)
    })
}

pub(crate) fn put_timer_on_amx_stack(timer: &Timer) -> Result<StackedCallbackData, TriggeringError> {
    let amx: &'static Amx = samp::amx::get(timer.amx_identifier).context(AmxGoneSnafu)?;
    timer
        .passed_arguments
        .push_onto_amx_stack(amx)
        .context(StackPushSnafu)?;
    Ok(StackedCallbackData((amx, timer.amx_callback_index)))
}


/// 1. Reschedules (or deschedules) the timer
/// 2. prepares the AMX for execution of the callback
///    by pushing arguments onto its stack
/// 3. Frees TIMERS and QUEUE
/// 4. Returns the callback.
pub(crate) fn start_triggering(
    timer_key: usize,
    repeat: Repeatability,
    now: Instant,
) -> Result<impl FnOnce() -> Result<i32, TriggeringError>, TriggeringError> {
    let StackedCallbackData((amx, callback_index)) = if let Repeatability::Repeating(interval) = repeat {
        start_triggering_repeating_timer(timer_key, interval, now)
    } else {
        start_triggering_singular_timer(timer_key)
    }?;
    Ok(move || {
        amx.exec(callback_index)
            .context(CallbackSnafu { timer_key })
    })
}

pub(crate) fn start_triggering_singular_timer(key: usize) -> Result<StackedCallbackData, TriggeringError> {
    deschedule_next_timer_ensuring_key(key)?;
    let timer = TIMERS.with_borrow_mut(|t| t.remove(key));
    put_timer_on_amx_stack(&timer)
}

pub(crate) fn start_triggering_repeating_timer(
    key: usize,
    interval: Duration,
    now: Instant,
) -> Result<StackedCallbackData, TriggeringError> {
    let next_trigger = now + interval;
    reschedule_timer(key, next_trigger)?;

    TIMERS.with_borrow_mut(|t| {
        let timer = t.get_mut(key).context(ExpectedInSlabSnafu)?;
        put_timer_on_amx_stack(timer)
    })
}

pub(crate) fn next_timer_due_for_triggering(now: Instant) -> Option<(usize, Repeatability)> {
    QUEUE.with_borrow(|q| match q.peek() {
        Some((
            &key,
            &Reverse(TimerScheduling {
                next_trigger,
                repeat,
            }),
        )) if next_trigger <= now => Some((key, repeat)),
        _ => None,
    })
}