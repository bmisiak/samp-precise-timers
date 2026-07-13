use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::{Duration, Instant},
};

use crate::timer::Callback;
use slab::Slab;

pub(crate) struct Timer {
    pub(crate) callback: Callback,
    repeat: Cell<Option<Duration>>,
    /// When this changes, the queue of timers must be reordered
    next_trigger: Cell<Instant>,
    pub(crate) key: usize,
}

pub(crate) struct State {
    /// Allocates the stable keys handed to PAWN and tracks which are alive.
    /// Every entry is also in `queue` and vice versa.
    pub(crate) timers: Slab<Rc<Timer>>,
    /// Queue of timers sorted by descending `next_trigger`, so the next due
    /// timer is peekable and poppable at the back in O(1) every tick.
    pub(crate) queue: Vec<Rc<Timer>>,
}

thread_local! {
    pub(crate) static STATE: RefCell<State> = RefCell::new(State {
        timers: Slab::with_capacity(1024),
        queue: Vec::with_capacity(1024),
    })
}

pub(crate) fn insert_and_schedule_timer(
    callback: Callback,
    next_trigger: Instant,
    repeat: Option<Duration>,
) -> usize {
    STATE.with_borrow_mut(|State { timers, queue }| {
        let slab_entry = timers.vacant_entry();
        let key = slab_entry.key();
        let rc = Rc::new(Timer {
            callback,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NoSuchTimer;

fn timer_position_in_queue(queue: &[Rc<Timer>], timer: &Rc<Timer>) -> usize {
    let trigger = timer.next_trigger.get();
    let binary_searched_idx = queue.partition_point(|state| state.next_trigger.get() > trigger);
    let offset_from_binary_search = queue[binary_searched_idx..]
        .iter()
        .take_while(|state| state.next_trigger.get() == trigger)
        .position(|state| Rc::ptr_eq(state, timer))
        .expect("timer in slab must also be in the schedule queue");
    binary_searched_idx + offset_from_binary_search
}

/// Changes a timer's trigger and restores the queue ordering by rotating only
/// the affected range. Strict `>` gives a rescheduled timer the same tie
/// behavior as a newly scheduled timer: timers already at that trigger fire
/// first.
fn reposition_timer(queue: &mut [Rc<Timer>], timer_index: usize, next_trigger: Instant) {
    let insertion_index = queue.partition_point(|state| state.next_trigger.get() > next_trigger);

    queue[timer_index].next_trigger.set(next_trigger);

    if insertion_index <= timer_index {
        queue[insertion_index..=timer_index].rotate_right(1);
    } else {
        // The partition includes the timer at its old position, so the
        // exclusive end compensates for removing it.
        queue[timer_index..insertion_index].rotate_left(1);
    }
}

pub(crate) fn delete_timer(timer_key: usize) -> Result<(), NoSuchTimer> {
    STATE.with_borrow_mut(|State { timers, queue }| {
        let timer = timers.try_remove(timer_key).ok_or(NoSuchTimer)?;
        let queue_index = timer_position_in_queue(queue, &timer);
        queue.remove(queue_index);
        Ok(())
    })
}

pub(crate) fn reschedule_timer(
    key: usize,
    next_trigger: Instant,
    repeat: Option<Duration>,
) -> Result<(), NoSuchTimer> {
    STATE.with_borrow_mut(|State { timers, queue }| {
        let timer = timers.get(key).ok_or(NoSuchTimer)?;
        timer.repeat.set(repeat);
        let current_index = timer_position_in_queue(queue, &timer);
        reposition_timer(queue, current_index, next_trigger);
        Ok(())
    })
}

pub(crate) fn remove_timers(predicate: impl Fn(&Callback) -> bool) {
    STATE.with_borrow_mut(|State { timers, queue }| {
        for removed in queue.extract_if(.., |timer| predicate(&timer.callback)) {
            timers.remove(removed.key);
        }
    });
}

/// Pops (for one-shot timers) or reschedules (for repeating ones) the next
/// due timer and returns it. Rescheduling happens *before* execution so that
/// reentrant calls into our natives see consistent state, and the returned
/// `Rc` keeps the timer's data alive even if the callback deletes the timer.
#[inline]
pub(crate) fn next_due_timer(now: Instant) -> Option<Rc<Timer>> {
    STATE.with_borrow_mut(|State { timers, queue }| {
        let next_up = queue.last()?;
        if next_up.next_trigger.get() > now {
            return None;
        }
        if let Some(interval) = next_up.repeat.get() {
            let due = Rc::clone(next_up);
            let next_trigger = now + interval;
            let current_index = queue.len() - 1;
            reposition_timer(queue, current_index, next_trigger);
            Some(due)
        } else {
            let unscheduled = queue.pop().expect("queue is empty after peeking");
            timers.remove(unscheduled.key);

            Some(unscheduled)
        }
    })
}
