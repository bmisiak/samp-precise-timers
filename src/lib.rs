#![warn(clippy::pedantic)]
use amx_arguments::VariadicAmxArguments;

use durr::{now, Durr};
use log::{error, info};
use samp::amx::Amx;
use samp::cell::AmxString;
use samp::error::{AmxError, AmxResult};
use samp::plugin::SampPlugin;
use scheduling::{reschedule_next_due_and_then, reschedule_timer};

use std::convert::TryFrom;
use timer::Timer;
mod amx_arguments;
mod schedule;
mod scheduling;
mod timer;
use schedule::Repeat::{DontRepeat, Every};
use schedule::Schedule;
use scheduling::{delete_timer, insert_and_schedule_timer, remove_timers, STATE};

/// The plugin
struct PreciseTimers;

#[allow(clippy::manual_let_else)]
#[allow(clippy::unused_self)]
impl PreciseTimers {
    /// This function is called from PAWN via the C foreign function interface.
    /// It returns the timer identifier or 0 in case of failure.
    /// ```
    /// native SetPreciseTimer(const callback_name[], const interval, const bool:repeat, const types_of_arguments[]="", {Float,_}:...);
    /// ```
    #[samp::native(raw, name = "SetPreciseTimer")]
    pub fn create(&self, amx: &Amx, mut args: samp::args::Args) -> AmxResult<i32> {
        // Get the basic, mandatory timer parameters
        let callback_name: AmxString = args.next().ok_or(AmxError::Params)?;
        let interval = args
            .next()
            .and_then(|ms: i32| u64::try_from(ms).ok())
            .ok_or(AmxError::Params)?
            .milliseconds();
        let repeat: bool = args.next().ok_or(AmxError::Params)?;
        let passed_arguments = VariadicAmxArguments::from_amx_args::<3>(args)?;

        let timer = Timer {
            passed_arguments,
            amx: amx.clone(),
            amx_callback_index: amx.find_public(&callback_name.to_string())?,
        };
        let key = insert_and_schedule_timer(timer, |key| Schedule {
            key,
            next_trigger: now() + interval,
            repeat: if repeat { Every(interval) } else { DontRepeat },
        });
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
    #[samp::native(name = "DeletePreciseTimer")]
    pub fn delete(&self, _: &Amx, timer_number: usize) -> AmxResult<i32> {
        let key = timer_number - 1;
        if let Err(err) = delete_timer(key) {
            error!("{err}");
            return Ok(0);
        }
        Ok(1)
    }

    /// This function is called from PAWN via the C foreign function interface.
    /// Returns 0 if the timer does not exist, 1 if removed.
    ///  ```
    /// native ResetPreciseTimer(timer_number, const interval, const bool:repeat)
    /// ```
    #[samp::native(name = "ResetPreciseTimer")]
    pub fn reset(
        &self,
        _: &Amx,
        timer_number: usize,
        interval: i32,
        repeat: bool,
    ) -> AmxResult<i32> {
        let key = timer_number - 1;
        let interval = u64::try_from(interval)
            .map_err(|_| AmxError::Params)?
            .milliseconds();

        let schedule = Schedule {
            key,
            next_trigger: now() + interval,
            repeat: if repeat { Every(interval) } else { DontRepeat },
        };
        if let Err(error) = reschedule_timer(key, schedule) {
            error!("{error}");
            return Ok(0);
        }
        Ok(1)
    }

    #[samp::native(name = "IsValidPreciseTimer")]
    pub fn is_valid(&self, _: &Amx, timer_number: i32) -> AmxResult<i32> {
        if timer_number <= 0 {
            return Ok(0);
        }
        
        let key = (timer_number as usize).saturating_sub(1);
        
        let is_valid = STATE.with_borrow(|state| {
            state.timers.contains(key)
        });
        
        Ok(if is_valid { 1 } else { 0 })
    }
}

impl SampPlugin for PreciseTimers {
    fn on_load(&self) {
        info!("samp-precise-timers v3 (c) Brian Misiak loaded correctly.");
    }

    fn on_amx_unload(&self, unloaded_amx: &Amx) {
        remove_timers(|timer| timer.was_scheduled_by_amx(unloaded_amx));
    }

    #[allow(clippy::inline_always)]
    #[inline(always)]
    fn process_tick(&self) {
        let now = now();

        while let Some(callback) = reschedule_next_due_and_then(now, Timer::stack_callback_on_amx) {
            match callback {
                Ok(stacked_callback) => {
                    // SAFETY: We are not holding any references to scheduling stores.
                    if let Err(exec_err) = stacked_callback.execute() {
                        error!("Error while executing timer: {exec_err}");
                    }
                }
                Err(stacking_err) => error!("Failed to stack callback: {stacking_err}"),
            }
        }
    }
}

samp::initialize_plugin!(
    natives: [
        PreciseTimers::delete,
        PreciseTimers::create,
        PreciseTimers::reset,
        PreciseTimers::is_valid,
    ],
    {
        samp::plugin::enable_process_tick();

        let samp_logprintf = samp::plugin::logger().level(log::LevelFilter::Info);

        let _ = fern::Dispatch::new()
            .format(|out, message, record| {
                let level = record.level();
                out.finish(format_args!("samp-precise-timers {level}: {message}"));
            })
            .chain(samp_logprintf)
            .apply();

        PreciseTimers
    }
);
