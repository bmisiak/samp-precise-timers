#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
use amx_arguments::{ArgError, VariadicAmxArguments};

use durr::{now, Durr};
use log::{error, info};
use samp::amx::{Amx, AmxExt};
use samp::cell::AmxString;
use samp::error::AmxError;
use snafu::{OptionExt, ResultExt, Snafu};

use std::convert::TryFrom;
use std::time::Duration;
use timer::Timer;
mod amx_arguments;
mod scheduling;
mod timer;
use scheduling::{
    delete_timer, insert_and_schedule_timer, next_due_timer, remove_timers, reschedule_timer,
};

/// Everything that can go wrong in our natives. The generated wrappers log
/// the `display` messages in the server log and return 0 to PAWN.
#[derive(Debug, Snafu)]
#[snafu(context(suffix(false)))]
enum TimerError {
    #[snafu(display("SetPreciseTimer expects a callback name as its first argument"))]
    MissingCallbackName,
    #[snafu(display("SetPreciseTimer expects an interval in milliseconds as its second argument"))]
    MissingInterval,
    #[snafu(display("the interval must be a non-negative number of milliseconds"))]
    InvalidInterval,
    #[snafu(display("SetPreciseTimer expects a bool:repeat as its third argument"))]
    MissingRepeat,
    #[snafu(display("repeating timers must have an interval of at least 1ms"))]
    ZeroIntervalRepeat,
    #[snafu(display("\"{name}\" is not a public function of the scheduling script"))]
    CallbackNotFound { name: String, source: AmxError },
    #[snafu(transparent)]
    InvalidArguments { source: ArgError },
    #[snafu(display("timer number {number} does not exist"))]
    NoSuchTimer { number: usize },
    #[snafu(display("the timer number space is exhausted"))]
    TooManyTimers,
}

/// A repeating timer with a 0ms interval would be due again immediately
/// after firing, making `process_tick`'s drain loop spin forever.
fn validated_repeat(repeat: bool, interval: Duration) -> Result<Option<Duration>, TimerError> {
    match (repeat, interval) {
        (true, Duration::ZERO) => ZeroIntervalRepeat.fail(),
        (true, interval) => Ok(Some(interval)),
        (false, _) => Ok(None),
    }
}

/// This function is called from PAWN via the C foreign function interface.
/// It returns the timer identifier or 0 in case of failure.
/// ```
/// native SetPreciseTimer(const callback_name[], const interval, const bool:repeat, const types_of_arguments[]="", {Float,_}:...);
/// ```
#[samp::native(raw, name = "SetPreciseTimer")]
fn create(amx: Amx, mut args: samp::args::Args) -> Result<i32, TimerError> {
    // Get the basic, mandatory timer parameters
    let callback_name: AmxString = args.next_arg().context(MissingCallbackName)?;
    let interval = args
        .next_arg::<i32>()
        .context(MissingInterval)
        .and_then(|ms| u64::try_from(ms).ok().context(InvalidInterval))?
        .milliseconds();
    let repeat = validated_repeat(args.next_arg().context(MissingRepeat)?, interval)?;
    let passed_arguments = VariadicAmxArguments::from_amx_args(args)?;

    let timer = Timer {
        passed_arguments,
        amx: amx.handle(),
        amx_callback_index: amx.find_public(&callback_name.to_cstring()).context(
            CallbackNotFound {
                name: callback_name.to_string_lossy(),
            },
        )?,
    };
    let key = insert_and_schedule_timer(timer, now() + interval, repeat);
    // The timer's slot in Slab<> incresed by 1, so that 0 signifies an invalid timer in PAWN
    let timer_number = key
        .checked_add(1)
        .and_then(|number| i32::try_from(number).ok())
        .context(TooManyTimers)?;
    Ok(timer_number)
}

/// This function is called from PAWN via the C foreign function interface.
/// Returns 0 if the timer does not exist.
///  ```
/// native DeletePreciseTimer(timer_number)
/// ```
#[samp::native(name = "DeletePreciseTimer")]
fn delete(_amx: Amx, number: usize) -> Result<i32, TimerError> {
    let key = number.checked_sub(1).context(NoSuchTimer { number })?;
    delete_timer(key).ok().context(NoSuchTimer { number })?;
    Ok(1)
}

/// This function is called from PAWN via the C foreign function interface.
/// Returns 0 if the timer does not exist, 1 if removed.
///  ```
/// native ResetPreciseTimer(timer_number, const interval, const bool:repeat)
/// ```
#[samp::native(name = "ResetPreciseTimer")]
fn reset(_amx: Amx, timer_number: usize, interval: i32, repeat: bool) -> Result<i32, TimerError> {
    let no_such_timer = NoSuchTimer {
        number: timer_number,
    };
    let key = timer_number.checked_sub(1).context(no_such_timer)?;
    let interval = u64::try_from(interval)
        .ok()
        .context(InvalidInterval)?
        .milliseconds();
    let repeat = validated_repeat(repeat, interval)?;

    reschedule_timer(key, now() + interval, repeat)
        .ok()
        .context(no_such_timer)?;
    Ok(1)
}

fn on_amx_unload(unloaded_amx: Amx) {
    let unloaded = unloaded_amx.handle();
    remove_timers(|timer| timer.amx == unloaded);
}

fn process_tick() {
    let now = now();

    // `next_due_timer` releases the borrow of the scheduling state before
    // returning, so the callback may freely reenter our natives.
    while let Some(due_timer) = next_due_timer(now) {
        if let Err(exec_err) = due_timer.timer.fire() {
            error!("Error while executing timer: {exec_err}");
        }
    }
}

#[cfg(test)]
mod test {
    use super::validated_repeat;
    use durr::Durr;

    #[test]
    fn zero_interval_is_only_valid_without_repeat() {
        assert!(validated_repeat(true, 0.milliseconds()).is_err());
        assert!(matches!(validated_repeat(false, 0.milliseconds()), Ok(None)));
        assert_eq!(
            validated_repeat(true, 5.milliseconds()).unwrap(),
            Some(5.milliseconds())
        );
    }
}

samp::initialize_plugin!(
    natives: [create, delete, reset],
    on_amx_unload: on_amx_unload,
    process_tick: process_tick,
    {
        let samp_logprintf = samp::plugin::logger().level(log::LevelFilter::Info);

        let _ = fern::Dispatch::new()
            .format(|out, message, record| {
                let level = record.level();
                out.finish(format_args!("samp-precise-timers {level}: {message}"));
            })
            .chain(samp_logprintf)
            .apply();

        // Panics abort the server (see Cargo.toml); the hook runs first and
        // puts the reason in the server log, which stderr may not reach.
        std::panic::set_hook(Box::new(|panic_info| {
            error!("fatal: {panic_info}");
        }));

        info!("samp-precise-timers v3 (c) Brian Misiak loaded correctly.");
    }
);
