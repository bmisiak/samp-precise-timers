use std::{
    cell::{Cell, RefCell},
    fmt::{self, Formatter},
    rc::Rc,
    time::{Duration, Instant},
};

use durr::now;
use slab::Slab;

use crate::timer::Timer;

pub(crate) struct TimerState {
    pub(crate) timer: Timer,
    repeat: Cell<Option<Duration>>,
    next_trigger: Cell<Instant>,
    key: usize,
}

impl fmt::Debug for TimerState {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if let Some(interval) = self.repeat.get() {
            f.write_fmt(format_args!(
                "[next_trigger in {:?}, repeat {:?}]",
                self.next_trigger.get() - now(),
                interval
            ))
        } else {
            f.write_fmt(format_args!(
                "[next_trigger in {:?}, no repeat]",
                self.next_trigger.get() - now(),
            ))
        }
    }
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

struct State {
    /// Allocates the stable keys handed to PAWN and tracks which are alive.
    /// Every entry is also in `queue` and vice versa.
    timers: Slab<Rc<TimerState>>,
    /// Queue of timers sorted by descending `next_trigger`, so the next due
    /// timer is peekable and poppable at the back in O(1) every tick.
    /// Timers with equal triggers are in reverse scheduling order: the
    /// earliest-scheduled sits closest to the back and fires first (FIFO).
    queue: Vec<Rc<TimerState>>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
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

#[cfg(test)]
mod test {
    use std::fmt::Debug;

    use durr::{now, Durr};

    use crate::scheduling::{State, STATE};
    use crate::Timer;
    use crate::{amx_arguments::VariadicAmxArguments, scheduling::next_due_timer};

    use super::{insert_and_schedule_timer, TimerState};

    fn timer() -> Timer {
        Timer {
            passed_arguments: VariadicAmxArguments::empty(),
            amx_callback_index: samp::consts::AmxExecIdx::Continue,
            amx: samp::amx::AmxHandle::dangling(),
        }
    }

    fn timer_keys(q: impl IntoIterator<Item: AsRef<TimerState>> + Debug) -> Vec<usize> {
        q.into_iter().map(|s| s.as_ref().key).collect()
    }

    #[test]
    fn hello() {
        let start = now();
        assert!(next_due_timer(start).is_none());
        let timers = [
            insert_and_schedule_timer(timer(), start + 1.seconds(), Some(5.seconds())),
            insert_and_schedule_timer(timer(), start + 2.seconds(), None),
            insert_and_schedule_timer(timer(), start + 3.seconds(), Some(1.seconds())),
            insert_and_schedule_timer(timer(), start + 4.seconds(), Some(1.seconds())),
        ];
        STATE.with_borrow_mut(|&mut State { ref mut queue, .. }| {
            assert_eq!(
                timer_keys(queue),
                [timers[3], timers[2], timers[1], timers[0]]
            );
        });
        // Timer 0 fires at t=1.2s and is rescheduled to t=6.2s, the latest
        // trigger in the queue, which puts it at the front (soonest-last).
        assert!(next_due_timer(start + 1200.milliseconds()).is_some());
        STATE.with_borrow_mut(|state| {
            assert_eq!(
                timer_keys(&state.queue),
                [timers[0], timers[3], timers[2], timers[1]]
            )
        });
        assert!(next_due_timer(start + 1200.milliseconds()).is_none());
        // Timer 1 is a one-shot: firing it at t=2.1s removes it entirely.
        assert!(next_due_timer(start + 2100.milliseconds()).is_some());
        STATE.with_borrow_mut(|state| {
            assert_eq!(timer_keys(&state.queue), [timers[0], timers[3], timers[2]])
        });
    }

    #[test]
    fn equal_triggers_fire_in_scheduling_order() {
        let start = now();
        let trigger = start + 1.seconds();
        let timers = [
            insert_and_schedule_timer(timer(), trigger, None),
            insert_and_schedule_timer(timer(), trigger, None),
            insert_and_schedule_timer(timer(), trigger, None),
        ];
        for expected_key in timers {
            let due = next_due_timer(start + 1100.milliseconds()).expect("timer should be due");
            assert_eq!(due.key, expected_key);
        }
    }

    #[test]
    fn rescheduling_keeps_the_queue_sorted() {
        let start = now();
        let timers = [
            insert_and_schedule_timer(timer(), start + 10.seconds(), None),
            insert_and_schedule_timer(timer(), start + 8.seconds(), None),
            insert_and_schedule_timer(timer(), start + 6.seconds(), None),
            insert_and_schedule_timer(timer(), start + 4.seconds(), None),
        ];

        // Move timer 1 later (8s -> 5s): between timers 2 (6s) and 3 (4s).
        super::reschedule_timer(timers[1], start + 5.seconds(), None).unwrap();
        STATE.with_borrow_mut(|state| {
            assert_eq!(
                timer_keys(&state.queue),
                [timers[0], timers[2], timers[1], timers[3]]
            );
        });

        // Move timer 2 earlier (6s -> 9s): between timers 0 (10s) and 1 (5s).
        super::reschedule_timer(timers[2], start + 9.seconds(), None).unwrap();
        STATE.with_borrow_mut(|state| {
            assert_eq!(
                timer_keys(&state.queue),
                [timers[0], timers[2], timers[1], timers[3]]
            );
        });

        // Reschedule without changing relative order (9s -> 9.5s).
        super::reschedule_timer(timers[2], start + 9500.milliseconds(), None).unwrap();
        STATE.with_borrow_mut(|state| {
            assert_eq!(
                timer_keys(&state.queue),
                [timers[0], timers[2], timers[1], timers[3]]
            );
        });
    }

    #[test]
    fn missing_keys_are_errors_not_panics() {
        let start = now();
        let key = insert_and_schedule_timer(timer(), start + 1.seconds(), None);
        assert!(super::reschedule_timer(key + 1, start, None).is_none());
        assert!(super::delete_timer(key + 1).is_none());
        assert!(super::delete_timer(key).is_some());
        assert!(super::reschedule_timer(key, start, None).is_none());
    }
}
