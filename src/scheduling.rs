use std::{
    cell::{Cell, RefCell},
    fmt::{self, Formatter},
    rc::{Rc, Weak},
    time::{Duration, Instant},
};

use durr::now;
use slab::Slab;
use snafu::{ensure, Snafu};

use crate::timer::Timer;

struct TimerState {
    timer: Timer,
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
    /// A slotmap of timers. Stable keys.
    timers: Slab<Weak<TimerState>>,
    /// Always sorted queue of timers. Easy O(1) peeking and popping of the next scheduled timer.
    queue: Vec<Rc<TimerState>>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        timers: Slab::with_capacity(1024),
        queue: Vec::with_capacity(1024),
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
        slab_entry.insert(Rc::downgrade(&rc));

        let new_position = queue.partition_point(|s| s.next_trigger.get() >= next_trigger);
        dbg!(new_position);
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

pub(crate) fn reschedule_timer(
    key: usize,
    next_trigger: Instant,
    repeat: Option<Duration>,
) -> Result<(), TriggeringError> {
    STATE.with_borrow_mut(|State { queue, .. }| {
        let current_index = queue.iter().position(|ts| ts.key == key).unwrap();
        let new_index = queue.partition_point(|s| s.next_trigger.get() >= next_trigger);
        queue[current_index].next_trigger.replace(next_trigger);
        queue[current_index].repeat.replace(repeat);
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
/// `stack_callback` must not borrow state
#[inline]
pub(crate) fn with_next_due_timer<T>(
    now: Instant,
    timer_manipulator: impl FnOnce(&Timer) -> T,
) -> Option<T> {
    STATE.with_borrow_mut(|State { timers, queue }| {
        let next_up = queue.last()?;
        if next_up.next_trigger.get() > now {
            return None;
        }
        if let Some(interval) = next_up.repeat.get() {
            let stacked_callback = timer_manipulator(&next_up.timer);

            let next_trigger = now + interval;
            let new_position = queue.partition_point(|s| s.next_trigger.get() >= next_trigger);

            next_up.next_trigger.replace(next_trigger);
            queue[new_position..].rotate_right(1);
            Some(stacked_callback)
        } else {
            let unscheduled = queue.pop().expect("due timer should be in queue");
            timers.remove(unscheduled.key);

            Some(timer_manipulator(&unscheduled.timer))
        }
    })
}

#[cfg(test)]
mod test {
    use std::fmt::Debug;
    use std::ptr::null_mut;

    use durr::{now, Durr};

    use crate::scheduling::{State, STATE};
    use crate::Timer;
    use crate::{amx_arguments::VariadicAmxArguments, scheduling::with_next_due_timer};

    use super::{insert_and_schedule_timer, TimerState};

    fn timer() -> Timer {
        Timer {
            passed_arguments: VariadicAmxArguments::empty(),
            amx_callback_index: samp::consts::AmxExecIdx::Continue,
            amx: samp::amx::Amx::new(null_mut(), 0),
        }
    }

    fn noop(_timer: &Timer) {}

    fn timer_keys(q: impl IntoIterator<Item: AsRef<TimerState>> + Debug) -> Vec<usize> {
        dbg!(&q);
        q.into_iter().map(|s| s.as_ref().key).collect()
    }

    #[test]
    fn hello() {
        let start = now();
        assert_eq!(with_next_due_timer(start, noop), None);
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
        assert!(with_next_due_timer(start + 1200.milliseconds(), noop).is_some());
        STATE.with_borrow_mut(|state| {
            assert_eq!(
                timer_keys(&state.queue),
                [timers[3], timers[2], timers[0], timers[1]]
            )
        });
        assert_eq!(with_next_due_timer(start + 1200.milliseconds(), noop), None);
        assert!(with_next_due_timer(start + 2100.milliseconds(), noop).is_some());
        STATE.with_borrow_mut(|state| {
            assert_eq!(timer_keys(&state.queue), [timers[3], timers[2], timers[0]])
        });
    }
}
