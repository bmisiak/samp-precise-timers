use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::{Duration, Instant},
};

use slab::Slab;

use crate::timer::Timer;

pub(crate) struct TimerState {
    pub(crate) timer: Timer,
    repeat: Cell<Option<Duration>>,
    next_trigger: Cell<Instant>,
    pub(crate) key: usize,
}

impl PartialEq for TimerState {
    fn eq(&self, other: &Self) -> bool {
        self.next_trigger == other.next_trigger
    }
}

impl Eq for TimerState {}

impl PartialOrd for TimerState {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TimerState {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.next_trigger.cmp(&self.next_trigger)
    }
}

pub(crate) struct State {
    /// Allocates the stable keys handed to PAWN and tracks which are alive.
    /// Every entry is also in `queue` and vice versa.
    pub(crate) timers: Slab<Rc<TimerState>>,
    /// Queue of timers sorted by descending `next_trigger`, so the next due
    /// timer is peekable and poppable at the back in O(1) every tick.
    pub(crate) queue: Vec<Rc<TimerState>>,
}

thread_local! {
    pub(crate) static STATE: RefCell<State> = RefCell::new(State {
        timers: Slab::with_capacity(1024),
        queue: Vec::with_capacity(1024),
    })
}

pub(crate) fn insert_and_schedule_timer(
    timer: Timer,
    next_trigger: Instant,
    repeat: Option<Duration>,
) -> usize {
    STATE.with_borrow_mut(|State { timers, queue }| {
        let slab_entry = timers.vacant_entry();
        let key = slab_entry.key();
        let rc = Rc::new(TimerState {
            timer,
            key,
            next_trigger: Cell::new(next_trigger),
            repeat: Cell::new(repeat),
        });
        slab_entry.insert(Rc::clone(&rc));

        // Strict `>` keeps ties FIFO: the new timer lands in front of
        // already-queued equals, so they reach the back and fire first.
        let new_position = queue.partition_point(|s| s.next_trigger.get() > next_trigger);
        queue.insert(new_position, rc);
        key
    })
}

/// `None` when no timer has the key; the caller owns the user-facing error.
pub(crate) fn delete_timer(timer_key: usize) -> Option<()> {
    STATE.with_borrow_mut(|State { timers, queue }| {
        timers.try_remove(timer_key)?;
        queue.retain(|s| s.key != timer_key);
        Some(())
    })
}

/// `None` when no timer has the key; the caller owns the user-facing error.
pub(crate) fn reschedule_timer(
    key: usize,
    next_trigger: Instant,
    repeat: Option<Duration>,
) -> Option<()> {
    STATE.with_borrow_mut(|State { queue, .. }| {
        let current_index = queue.iter().position(|ts| ts.key == key)?;
        // Remove before searching for the new position, so the stale entry
        // can't skew `partition_point` (the queue must stay sorted while
        // it's scanned).
        let timer_state = queue.remove(current_index);
        timer_state.next_trigger.replace(next_trigger);
        timer_state.repeat.replace(repeat);
        let new_index = queue.partition_point(|s| s.next_trigger.get() > next_trigger);
        queue.insert(new_index, timer_state);
        Some(())
    })
}

pub(crate) fn remove_timers(predicate: impl Fn(&Timer) -> bool) {
    STATE.with_borrow_mut(|State { timers, queue }| {
        for removed in queue.extract_if(.., |timer_state| predicate(&timer_state.timer)) {
            timers.remove(removed.key);
        }
    });
}

/// Pops (for one-shot timers) or reschedules (for repeating ones) the next
/// due timer and returns it. Rescheduling happens *before* execution so that
/// reentrant calls into our natives see consistent state, and the returned
/// `Rc` keeps the timer's data alive even if the callback deletes the timer.
#[inline]
pub(crate) fn next_due_timer(now: Instant) -> Option<Rc<TimerState>> {
    STATE.with_borrow_mut(|State { timers, queue }| {
        let next_up = queue.last()?;
        if next_up.next_trigger.get() > now {
            return None;
        }
        if let Some(interval) = next_up.repeat.get() {
            let due = Rc::clone(next_up);
            let next_trigger = now + interval;
            let new_position = queue.partition_point(|s| s.next_trigger.get() > next_trigger);

            due.next_trigger.replace(next_trigger);
            queue[new_position..].rotate_right(1);
            Some(due)
        } else {
            let unscheduled = queue.pop().expect("due timer should be in queue");
            timers.remove(unscheduled.key);

            Some(unscheduled)
        }
    })
}
