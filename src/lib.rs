#![warn(clippy::pedantic)]
use amx_arguments::VariadicAmxArguments;
use log::info;
use samp::amx::{Amx, AmxIdent};
use samp::cell::AmxString;
use samp::error::{AmxError, AmxResult};
use samp::plugin::SampPlugin;
use slab::Slab;
use std::convert::TryFrom;
use std::time::{Duration, Instant};
use timer::{Timer, TimerStaus};

mod amx_arguments;
mod timer;

/// The plugin and its data: a list of scheduled timers
struct PreciseTimers {
    timers: Slab<Timer>,
    sorted_timers: Vec<usize>,
    triggered_timers: Vec<Timer>,
}

/*#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SortedTimerRef(usize);

impl Into<&Timer> for SortedTimerRef {
    fn into(self) -> &'a Timer {

    }
}*/

impl PreciseTimers {
    /// This function is called from PAWN via C FFI.
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
        let timer = Timer {
            next_trigger,
            interval: if repeat { Some(interval) } else { None },
            passed_arguments,
            amx_identifier: AmxIdent::from(amx.amx().as_ptr()),
            amx_callback_index,
            scheduled_for_removal: false,
        };

        // Add the timer to the list. This is safe for Slab::retain() even if SetPreciseTimer was called from a timer's callback.
        let key = {
            let slab_entry = self.timers.vacant_entry();
            let slab_key = slab_entry.key();
            slab_entry.insert(timer);

            let sorted_slot = self.find_slot_by_trigger_time(next_trigger);
            self.sorted_timers.insert(sorted_slot, slab_key);

            slab_key
        };

        // The timer's slot in Slab<> incresed by 1, so that 0 signifies an invalid timer in PAWN
        let timer_number = key
            .checked_add(1)
            .and_then(|number| i32::try_from(number).ok())
            .ok_or(AmxError::Bounds)?;
        Ok(timer_number)
    }

    fn find_slot_by_trigger_time(&self, trigger_time: Instant) -> usize {
        let sorted_slot = self.sorted_timers.partition_point(|&probed_timer_key| {
            let probed_timer = self.timers
                .get(probed_timer_key)
                .expect("a matching timer should exist in the slab when its key is present in sorted_timers");
            probed_timer.next_trigger > trigger_time
        });
        sorted_slot
    }

    fn find_sort_slot_from_trigger_time_and_key(&self, timer: &Timer) -> usize {
        // We sort sorted_timers by trigger time, so we can only binary-search by trigger time.
        // First, find a slot that matches next trigger:
        let around_slot = self
            .sorted_timers
            .binary_search_by(|&probed_timer_key| {
                let probed_timer = self.timers.get(probed_timer_key).unwrap();
                probed_timer.next_trigger.cmp(&timer.next_trigger)
            })
            .expect("Failed to find a matching timer when binary searching by trigger time");
        // Now, there should only ever be one timer with a particular Instant.
        // What if by some reason there are more than one, and the one we
        // found via binary search is not the one we wanted? (we could keep the key in the Timer struct)
        // Should we check to the left and right of around_slot?
        // For now, assume they can't duplicate:
        around_slot
    }

    /// This function is called from PAWN via C FFI.
    /// Returns 0 if the timer does not exist.
    ///  ```
    /// native DeletePreciseTimer(timer_number)
    /// ```
    #[allow(clippy::unnecessary_wraps)]
    #[samp::native(name = "DeletePreciseTimer")]
    pub fn delete(&mut self, _: &Amx, timer_number: usize) -> AmxResult<i32> {
        // Subtract 1 from the passed timer_number (where 0=invalid) to get the actual Slab<> slot
        if let Some(timer) = self.timers.get_mut(timer_number - 1) {
            // We defer the removal so that we don't mess up the process_tick()->retain() iterator.
            // so we don't have to bother deleting it from the sorted slot rn? maybe we should tho
            //let x = self.find_timer_sorted_slot(timer);

            timer.scheduled_for_removal = true;
            Ok(1)
        } else {
            Ok(0)
        }
    }

    /// This function is called from PAWN via C FFI.
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
        if let Some(timer) = self.timers.get_mut(timer_number - 1) {
            let interval = u64::try_from(interval)
                .map(Duration::from_millis)
                .or(Err(AmxError::Params))?;

            timer.next_trigger = Instant::now() + interval;
            // what if a timer gets triggered and resets itself while executing?
            timer.interval = if repeat { Some(interval) } else { None };
            Ok(1)
        } else {
            Ok(0)
        }
    }
}

impl SampPlugin for PreciseTimers {
    fn on_load(&mut self) {
        info!("net4game.com/samp-precise-timers by Brian Misiak loaded correctly.");
    }

    #[allow(clippy::inline_always)]
    #[inline(always)]
    fn process_tick(&mut self) {
        // Rust's Instant is monotonic and nondecreasing, even during NTP time adjustment.
        let now = Instant::now();

        while let Some(_) = self
            .sorted_timers
            .last()
            .map(|&timer_key| self.timers.get(timer_key).unwrap().next_trigger)
            .filter(|&next_trigger| next_trigger <= now)
        {
            // what if I got rid of scheduled_for_removal
            // could I run the timer here, before popping? what if it:
            // * deleted itself - .pop() below would yield None
            // * rescheduled itself - I would .pop() the wrong timer, possibly one thats not due yet
            // * inserted a new timer as the next one to be triggered - cool
            let timer_key = self.sorted_timers.pop().unwrap();

            let keep = {
                let timer = self.timers.get_mut(timer_key).unwrap();

                if timer.scheduled_for_removal {
                    false
                } else {
                    // its gone from sorted_timers, present in slab, and we're 
                    // doing C FFI here in this kinda inconsistent state.
                    // if they try to delete by key, we would be unable to find the
                    // sorted slot corresponding to slab key.
                    if let Err(err) = timer.trigger() {
                        log::error!("Error executing timer callback: {}", err);
                    }
                    if let Some(interval) = timer.interval {
                        // reinsert into sorted_timers
                        let next_trigger = now + interval;
                        let new_slot = self.find_slot_by_trigger_time(next_trigger);
                        self.sorted_timers.insert(new_slot, timer_key);
                        true
                    } else {
                        false
                    }
                }
            };
            if !keep {
                self.timers.remove(timer_key); // will panic if timer_key not found
            }
        }
    }

    fn on_amx_unload(&mut self, unloaded_amx: &Amx) {
        self.timers
            .retain(|_, timer| !timer.was_scheduled_by_amx(unloaded_amx));
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

        PreciseTimers {
            timers: Slab::with_capacity(1000),
            sorted_timers: Vec::with_capacity(1000),
            triggered_timers: Vec::with_capacity(100),
        }
    }
);
