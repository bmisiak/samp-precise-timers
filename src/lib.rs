#![warn(clippy::pedantic)]
use amx_arguments::VariadicAmxArguments;

use log::{error, info};

use samp::amx::{Amx, AmxIdent};
use samp::cell::AmxString;
use samp::error::{AmxError, AmxResult};
use samp::plugin::SampPlugin;



use std::convert::TryFrom;
use std::time::{Duration, Instant};
use timer::Timer;
mod amx_arguments;
mod timer;
mod scheduling;
use scheduling::{delete_timer, deschedule_timer, insert_and_schedule_timer, next_timer_due_for_triggering, remove_timers, reschedule_timer, start_triggering, Repeatability, StackedCallback, TimerScheduling, TriggeringError};
/// The plugin
struct PreciseTimers;

impl PreciseTimers {
    /// This function is called from PAWN via the C foreign function interface.
    /// It returns the timer identifier or 0 in case of failure.
    /// ```
    /// native SetPreciseTimer(const callback_name[], const interval, const bool:repeat, const types_of_arguments[]="", {Float,_}:...);
    /// ```
    #[samp::native(raw, name = "SetPreciseTimer")]
    pub fn create(&mut self, amx: &Amx, mut args: samp::args::Args) -> AmxResult<i32> {
        // Get the basic, mandatory timer parameters
        let callback_name = args.next::<AmxString>().ok_or(AmxError::Params)?;
        let interval = args
            .next::<i32>()
            .and_then(|ms| u64::try_from(ms).ok())
            .ok_or(AmxError::Params)
            .map(Duration::from_millis)?;
        let repeat = args.next::<bool>().ok_or(AmxError::Params)?;
        let passed_arguments = VariadicAmxArguments::from_amx_args(args, 3)?;

        // Find the callback by name and save its index
        let amx_callback_index = amx.find_public(&callback_name.to_string())?;
        let next_trigger = Instant::now() + interval;

        let key = insert_and_schedule_timer(
            Timer {
                passed_arguments,
                amx_identifier: AmxIdent::from(amx.amx().as_ptr()),
                amx_callback_index,
            },
            TimerScheduling {
                next_trigger,
                repeat: if repeat {
                    Repeatability::Repeating(interval)
                } else {
                    Repeatability::NotRepeating
                },
            },
        );
        // The timer's slot in Slab<> incresed by 1, so that 0 signifies an invalid timer in PAWN
        let timer_number = key
            .checked_add(1)
            .and_then(|number| i32::try_from(number).ok())
            .ok_or(AmxError::Bounds)?;
        Ok(timer_number)
    }

    /// This function is called from PAWN via the C foreign function interface.
    /// Returns 0 if the timer does not exist.
    ///  ```
    /// native DeletePreciseTimer(timer_number)
    /// ```
    #[allow(clippy::unnecessary_wraps)]
    #[samp::native(name = "DeletePreciseTimer")]
    pub fn delete(&mut self, _: &Amx, timer_number: usize) -> AmxResult<i32> {
        let key = timer_number - 1;
        if (delete_timer(key).map_err(|_| AmxError::MemoryAccess)?).is_some() {
            Ok(1)
        } else {
            Ok(0)
        }
    }

    /// This function is called from PAWN via the C foreign function interface.
    /// Returns 0 if the timer does not exist, 1 if removed.
    ///  ```
    /// native ResetPreciseTimer(timer_number, const interval, const bool:repeat)
    /// ```
    #[samp::native(name = "ResetPreciseTimer")]
    pub fn reset(
        &mut self,
        _: &Amx,
        timer_number: usize,
        interval: i32,
        repeat: bool,
    ) -> AmxResult<i32> {
        let key = timer_number - 1;
        let interval = u64::try_from(interval)
            .map(Duration::from_millis)
            .or(Err(AmxError::Params))?;

        let result = if repeat {
            reschedule_timer(key, Instant::now() + interval)
        } else {
            deschedule_timer(key)
        };

        if let Err(error) = result {
            error!("{}",error);
            Ok(0)
        } else {
            Ok(1)
        }
    }
}
use snafu::{ResultExt, Snafu};
#[derive(Debug, Snafu)]
#[snafu(display("Error while executing timer number {}", timer_key+1))]
struct TimerError {
    source: TriggeringError,
    timer_key: usize,
}


#[allow(clippy::inline_always)]
#[inline(always)]
pub(crate) fn trigger_due_timers() {
    let now = Instant::now();

    while let Some((timer_key, repeat)) = next_timer_due_for_triggering(now) {
        match start_triggering(timer_key, repeat, now, put_timer_on_amx_stack)
                    .context(TimerSnafu { timer_key }) {
            Ok(callback) => {
                if let Err(err) = callback.execute() {
                    error!("Error executing callback for timer {}: {}", timer_key+1, err);
                }
            }
            Err(err) => error!("Error triggering timer {}: {}", timer_key+1, err),
        } 
    }
}

pub(crate) fn put_timer_on_amx_stack(timer: &Timer) -> Result<StackedCallback, AmxError> {
    let amx: &'static Amx = samp::amx::get(timer.amx_identifier).ok_or(AmxError::NotFound)?;
    timer
        .passed_arguments
        .push_onto_amx_stack(amx)?;
    Ok(StackedCallback{ amx, callback_idx: timer.amx_callback_index})
}

impl SampPlugin for PreciseTimers {
    fn on_load(&mut self) {
        info!("samp-precise-timers v3 (c) Brian Misiak loaded correctly.");
    }

    #[allow(clippy::inline_always)]
    #[inline(always)]
    fn process_tick(&mut self) {
        trigger_due_timers();
    }

    fn on_amx_unload(&mut self, unloaded_amx: &Amx) {
        remove_timers(|timer| timer.was_scheduled_by_amx(unloaded_amx));
    }
}


samp::initialize_plugin!(
    natives: [
        PreciseTimers::delete,
        PreciseTimers::create,
        PreciseTimers::reset,
    ],
    {
        samp::plugin::enable_process_tick();

        // get the default samp logger (uses samp logprintf).
        let samp_logger = samp::plugin::logger().level(log::LevelFilter::Info); // logging info, warn and error messages

        let _ = fern::Dispatch::new()
            .format(|callback, message, record| {
                callback.finish(format_args!("samp-precise-timers {}: {}", record.level().to_string().to_lowercase(), message));
            })
            .chain(samp_logger)
            .apply();

        PreciseTimers
    }
);
