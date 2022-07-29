use log::{error, info};
use samp::amx::{Amx, AmxIdent};
use samp::cell::AmxString;
use samp::error::{AmxError, AmxResult};
use samp::plugin::SampPlugin;
use slab::Slab;
use std::convert::TryFrom;
use std::time::{Duration, Instant};
use timer::Timer;
use amx_arguments::parse_variadic_arguments_passed_into_timer;

mod amx_arguments;
mod timer;

/// The plugin and its data: a list of scheduled timers
struct PreciseTimers {
    timers: Slab<Timer>,
}

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
        let interval = Duration::from_millis(
            args.next::<i32>()
                .and_then(|ms| u64::try_from(ms).ok())
                .ok_or(AmxError::Params)?,
        );
        let repeat = args.next::<bool>().ok_or(AmxError::Params)?;
        let passed_arguments =
            parse_variadic_arguments_passed_into_timer(args).ok_or(AmxError::Params)?;

        // Find the callback by name and save its index
        let amx_callback_index = amx.find_public(&callback_name.to_string())?;

        let timer = Timer {
            next_trigger: Instant::now() + interval,
            interval: if repeat { Some(interval) } else { None },
            passed_arguments,
            amx_identifier: AmxIdent::from(amx.amx().as_ptr()),
            amx_callback_index,
            scheduled_for_removal: false,
        };

        // Add the timer to the list. This is safe for Slab::retain() even if SetPreciseTimer was called from a timer's callback.
        let key: usize = self.timers.insert(timer);

        // Return the timer's slot in Slab<> incresed by 1, so that 0 signifies an invalid timer in PAWN
        Ok((key as i32) + 1)
    }

    /// This function is called from PAWN via the C foreign function interface.
    /// Returns 0 if the timer does not exist.
    ///  ```
    /// native DeletePreciseTimer(timer_number)
    /// ```
    #[samp::native(name = "DeletePreciseTimer")]
    pub fn delete(&mut self, _: &Amx, timer_number: usize) -> AmxResult<i32> {
        // Subtract 1 from the passed timer_number (where 0=invalid) to get the actual Slab<> slot
        match self.timers.get_mut(timer_number - 1) {
            Some(timer) => {
                // We defer the removal so that we don't mess up the process_tick()->retain() iterator.
                timer.scheduled_for_removal = true;
                Ok(1)
            }
            None => Ok(0),
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
        match self.timers.get_mut(timer_number - 1) {
            Some(timer) => {
                let interval = Duration::from_millis(interval as u64);
                timer.next_trigger = Instant::now() + interval;
                timer.interval = if repeat { Some(interval) } else { None };
                Ok(1)
            }
            None => Ok(0),
        }
    }
}

impl SampPlugin for PreciseTimers {
    fn on_load(&mut self) {
        info!("net4game.com/samp-precise-timers by Brian Misiak loaded correctly.");
    }

    #[inline(always)]
    fn process_tick(&mut self) {
        // Rust's Instant is monotonic and nondecreasing, even during NTP time adjustment.
        let now = Instant::now();

        // 💀⚠ Because of FFI with C, Rust can't notice the simultaneous mutation of self.timers, but the iterator could get messed up in case of
        // Slab::retain() -> Timer::trigger() -> PAWN callback/ffi which calls DeletePreciseTimer() -> Slab::remove.
        // That's why the DeletePreciseTimer() schedules timers for deletion instead of doing it right away.
        // Slab::retain() is, however, okay with inserting new timers during its execution, even in case of reallocation when over capacity.
        self.timers.retain(|_key: usize, timer| {
            if timer.next_trigger <= now {
                if timer.scheduled_for_removal {
                    // Remove timer and do not execute its callback.
                    false
                } else {
                    // Execute the callback:
                    if let Err(err) = timer.trigger() {
                        error!("Error executing timer callback: {}", err);
                    }

                    if let Some(interval) = timer.interval {
                        timer.next_trigger = now + interval;
                        // It repeats. Keep it, unless removed by PAWN when it was triggered just now
                        !timer.scheduled_for_removal
                    } else {
                        // Remove the timer. It got triggered and does not repeat
                        false
                    }
                }
            } else {
                // Keep the timer, it has yet to be triggered
                true
            }
        });
    }

    fn on_amx_unload(&mut self, unloaded_amx: &Amx) {
        self.timers
            .retain(|_, timer| !timer.was_scheduled_by_amx(unloaded_amx))
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
                callback.finish(format_args!("samp-precise-timers {}: {}", record.level().to_string().to_lowercase(), message))
            })
            .chain(samp_logger)
            .apply();

        PreciseTimers {
            timers: Slab::with_capacity(1000)
        }
    }
);
