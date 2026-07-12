use durr::{now, Durr};

use crate::amx_arguments::VariadicAmxArguments;
use crate::scheduling::{
    delete_timer, insert_and_schedule_timer, next_due_timer, reschedule_timer, State, TimerState,
    STATE,
};
use crate::{validated_repeat, Timer};

fn timer() -> Timer {
    Timer {
        passed_arguments: VariadicAmxArguments::empty(),
        amx_callback_index: samp::consts::AmxExecIdx::Continue,
        amx: samp::amx::AmxHandle::dangling(),
    }
}

fn timer_keys(queue: impl IntoIterator<Item: AsRef<TimerState>>) -> Vec<usize> {
    queue.into_iter().map(|state| state.as_ref().key).collect()
}

#[test]
fn zero_interval_is_only_valid_without_repeat() {
    assert!(validated_repeat(true, 0.milliseconds()).is_err());
    assert!(matches!(
        validated_repeat(false, 0.milliseconds()),
        Ok(None)
    ));
    assert_eq!(
        validated_repeat(true, 5.milliseconds()).unwrap(),
        Some(5.milliseconds())
    );
}

#[test]
fn due_timers_are_removed_or_rescheduled() {
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
        );
    });
    assert!(next_due_timer(start + 1200.milliseconds()).is_none());

    // Timer 1 is a one-shot: firing it at t=2.1s removes it entirely.
    assert!(next_due_timer(start + 2100.milliseconds()).is_some());
    STATE.with_borrow_mut(|state| {
        assert_eq!(timer_keys(&state.queue), [timers[0], timers[3], timers[2]]);
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
    reschedule_timer(timers[1], start + 5.seconds(), None).unwrap();
    STATE.with_borrow_mut(|state| {
        assert_eq!(
            timer_keys(&state.queue),
            [timers[0], timers[2], timers[1], timers[3]]
        );
    });

    // Move timer 2 earlier (6s -> 9s): between timers 0 (10s) and 1 (5s).
    reschedule_timer(timers[2], start + 9.seconds(), None).unwrap();
    STATE.with_borrow_mut(|state| {
        assert_eq!(
            timer_keys(&state.queue),
            [timers[0], timers[2], timers[1], timers[3]]
        );
    });

    // Reschedule without changing relative order (9s -> 9.5s).
    reschedule_timer(timers[2], start + 9500.milliseconds(), None).unwrap();
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
    assert!(reschedule_timer(key + 1, start, None).is_none());
    assert!(delete_timer(key + 1).is_none());
    assert!(delete_timer(key).is_some());
    assert!(reschedule_timer(key, start, None).is_none());
}
