#![warn(clippy::pedantic)]
use amx_arguments::VariadicAmxArguments;
use log::{error, info};
use priority_queue::PriorityQueue;
use samp::amx::{Amx, AmxIdent};
use samp::cell::AmxString;
use samp::error::{AmxError, AmxResult};
use samp::plugin::SampPlugin;
use slab::Slab;
use std::cmp::Reverse;
use std::convert::TryFrom;
use std::time::{Duration, Instant};
use timer::Timer;
mod amx_arguments;
mod timer;

/// The plugin and its data: a list of scheduled timers
struct PreciseTimers {
    timers: Slab<Timer>,
    queue: PriorityQueue<usize, Reverse<TimerScheduling>, fnv::FnvBuildHasher>,
}

#[derive(Clone)]
struct TimerScheduling {
    interval: Option<Duration>,
    execution_forbidden: bool,
    next_trigger: Instant,
}

impl PartialEq for TimerScheduling {
    fn eq(&self, other: &Self) -> bool {
        self.next_trigger == other.next_trigger
    }
}
impl Eq for TimerScheduling {}

impl PartialOrd for TimerScheduling {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.next_trigger.partial_cmp(&other.next_trigger)
    }
}
impl Ord for TimerScheduling {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.next_trigger.cmp(&other.next_trigger)
    }
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
            passed_arguments,
            amx_identifier: AmxIdent::from(amx.amx().as_ptr()),
            amx_callback_index,
        };

        // Add the timer to the list. This is safe for Slab::retain() even if SetPreciseTimer was called from a timer's callback.
        let key: usize = self.timers.insert(timer);
        self.queue.push(
            key,
            Reverse(TimerScheduling {
                next_trigger,
                interval: if repeat { Some(interval) } else { None },
                execution_forbidden: false,
            }),
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
        if self.queue.change_priority_by(&key, |scheduling| {
            scheduling.0.execution_forbidden = true;
        }) {
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

        if self
            .queue
            .change_priority(
                &key,
                Reverse(TimerScheduling {
                    next_trigger: Instant::now() + interval,
                    interval: if repeat { Some(interval) } else { None },
                    execution_forbidden: false,
                }),
            )
            .is_some()
        {
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

        loop {
            let (key, interval, execution_forbidden) = {
                let Some((&key, &Reverse(ref scheduling))) = self.queue.peek() else {
                    break;
                };
                if scheduling.next_trigger > now {
                    break;
                }
                (
                    key.clone(),
                    scheduling.interval.clone(),
                    scheduling.execution_forbidden.clone(),
                )
            };

            if let (Some(interval), false) = (interval, execution_forbidden) {
                let next_trigger = now + interval;
                self.queue
                    .change_priority(
                        &key,
                        Reverse(TimerScheduling {
                            next_trigger,
                            execution_forbidden,
                            interval: Some(interval),
                        }),
                    )
                    .expect("failed to update scheduling of a repeating timer");

                let timer = self
                    .timers
                    .get_mut(key)
                    .expect("slab should contain repeating timer");
                if let Err(err) = timer.execute_pawn_callback() {
                    error!("Error executing repeating timer callback: {}", err);
                }
            } else {
                // Must pop before the timer is executed, so that
                // it can't schedule anything as the very next timer before
                // we have a chance to pop from the queue.
                let (popped_key, _) = self
                    .queue
                    .pop()
                    .expect("priority queue should have at least the timer we peeked");
                assert_eq!(
                    popped_key, key,
                    "timer popped from priority queue must match the peeked one"
                );
                let mut removed_timer = self.timers.remove(key);

                if !execution_forbidden {
                    if let Err(err) = removed_timer.execute_pawn_callback() {
                        error!("Error executing non-repeating timer callback: {}", err);
                    }
                }
            }
        }
    }

    fn on_amx_unload(&mut self, unloaded_amx: &Amx) {
        let mut removed_timers = Vec::new();
        self.timers.retain(|key, timer| {
            if timer.was_scheduled_by_amx(unloaded_amx) {
                removed_timers.push(key);
                false
            } else {
                true
            }
        });
        for key in removed_timers {
            self.queue.remove(&key);
        }
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
            queue: PriorityQueue::with_capacity_and_default_hasher(1000),
        }
    }
);
