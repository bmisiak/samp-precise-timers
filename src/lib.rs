#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
use amx_arguments::{ArgError, VariadicAmxArguments};

use durr::{now, Durr};
use log::{error, info};
use samp::amx::{Amx, AmxExt};
use samp::args::VariadicArgs;
use samp::cell::AmxString;
use samp::error::AmxError;

use std::ffi::CString;
use std::num::NonZeroUsize;
use std::time::Duration;
use timer::Callback;
mod amx_arguments;
mod scheduling;
#[cfg(test)]
mod tests;
mod timer;
use scheduling::{
    delete_timer, insert_and_schedule_timer, next_due_timer, remove_timers, reschedule_timer,
};

custom_error::custom_error! {
    /// Everything that can go wrong in our natives. The generated wrappers log
    /// the display messages in the server log and return 0 to PAWN.
    TimerError
    ZeroIntervalRepeat = "repeating timers must have an interval of at least 1ms",
    CallbackNotFound { callback_name: CString, source: AmxError } =
        @{ format!("{callback_name:?} is not a public function of the scheduling script: {source}") },
    InvalidArguments { source: ArgError } = "{source}",
    NoSuchTimer { number: NonZeroUsize } = "timer number {number} does not exist",
    TooManyTimers = "the timer number space is exhausted",
}
use TimerError::{CallbackNotFound, NoSuchTimer, TooManyTimers, ZeroIntervalRepeat};

/// A repeating timer with a 0ms interval would be due again immediately
/// after firing, making `process_tick`'s drain loop spin forever.
fn validated_repeat(repeat: bool, interval: Duration) -> Result<Option<Duration>, TimerError> {
    match (repeat, interval) {
        (true, Duration::ZERO) => Err(ZeroIntervalRepeat),
        (true, interval) => Ok(Some(interval)),
        (false, _) => Ok(None),
    }
}

/// This function is called from PAWN via the C foreign function interface.
/// It returns the timer identifier or 0 in case of failure.
/// ```
/// native SetPreciseTimer(const callback_name[], const interval, const bool:repeat, const types_of_arguments[]="", {Float,_}:...);
/// ```
#[allow(clippy::needless_pass_by_value)]
fn create(
    amx: Amx,
    callback_name: AmxString,
    interval: u32,
    repeat: bool,
    args: VariadicArgs,
) -> Result<i32, TimerError> {
    let interval = u64::from(interval).milliseconds();
    let repeat = validated_repeat(repeat, interval)?;
    let passed_arguments = VariadicAmxArguments::from_amx_args(args)?;
    let callback_name = callback_name.to_cstring();

    let callback = Callback {
        passed_arguments,
        amx: amx.handle(),
        amx_callback_index: amx
            .find_public(&callback_name)
            .map_err(|source| CallbackNotFound { callback_name, source })?,
    };
    let key = insert_and_schedule_timer(callback, now() + interval, repeat);
    // The timer's slot in Slab<> incresed by 1, so that 0 signifies an invalid timer in PAWN
    let timer_number = key
        .checked_add(1)
        .and_then(|number| i32::try_from(number).ok())
        .ok_or(TooManyTimers)?;
    Ok(timer_number)
}

/// This function is called from PAWN via the C foreign function interface.
/// Returns 0 if the timer does not exist.
///  ```
/// native DeletePreciseTimer(timer_number)
/// ```
fn delete(number: NonZeroUsize) -> Result<i32, TimerError> {
    delete_timer(number.get() - 1).map_err(|_| NoSuchTimer { number })?;
    Ok(1)
}

/// This function is called from PAWN via the C foreign function interface.
/// Returns 0 if the timer does not exist, 1 if removed.
///  ```
/// native ResetPreciseTimer(timer_number, const interval, const bool:repeat)
/// ```
fn reset(number: NonZeroUsize, interval: u32, repeat: bool) -> Result<i32, TimerError> {
    let interval = u64::from(interval).milliseconds();
    let repeat = validated_repeat(repeat, interval)?;

    reschedule_timer(number.get() - 1, now() + interval, repeat)
        .map_err(|_| NoSuchTimer { number })?;
    Ok(1)
}

fn on_amx_unload(unloaded_amx: Amx) {
    remove_timers(|timer| timer.amx == unloaded_amx.handle());
}

fn process_tick() {
    let now = now();

    // `next_due_timer` releases the borrow of the scheduling state before
    // returning, so the callback may freely reenter our natives.
    while let Some(due_timer) = next_due_timer(now) {
        if let Err(exec_err) = due_timer.callback.fire() {
            error!("Error while executing timer: {exec_err}");
        }
    }
}

samp::plugin! {
    natives: [
        c"SetPreciseTimer" = create,
        c"DeletePreciseTimer" = delete,
        c"ResetPreciseTimer" = reset,
    ],
    on_amx_unload: on_amx_unload,
    process_tick: process_tick,
    load: {
        // Panics abort the server; the hook runs first and
        // puts the reason in the server log
        std::panic::set_hook(Box::new(|panic_info| {
            error!("fatal: {panic_info}");
        }));

        info!("samp-precise-timers v3 (c) Brian Misiak loaded correctly.");
    }
}
