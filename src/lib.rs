#![warn(clippy::pedantic)]
use amx_arguments::VariadicAmxArguments;
use log::{error, info};
use priority_queue::PriorityQueue;
use samp::amx::{Amx, AmxIdent};
use samp::cell::AmxString;
use samp::error::{AmxError, AmxResult};
use samp::plugin::SampPlugin;
use slab::Slab;
use std::cell::RefCell;
use std::cmp::Reverse;
use std::convert::TryFrom;
use std::time::{Duration, Instant};
use timer::Timer;
mod amx_arguments;
mod timer;

thread_local! {
    /// A slotmap of timers. Stable keys.
    static TIMERS: RefCell<Slab<Timer>> = RefCell::new(Slab::with_capacity(1000));
    /// Always sorted queue of timers. Easy O(1) peeking and popping of the next scheduled timer.
    static QUEUE: RefCell<PriorityQueue<usize, Reverse<TimerScheduling>, fnv::FnvBuildHasher>> = RefCell::new(PriorityQueue::with_capacity_and_default_hasher(1000));
}

/// The plugin
struct PreciseTimers;

/// A struct defining when a timer gets triggered
#[derive(Clone)]
struct TimerScheduling {
    /// If Some, it's a repeating timer.
    /// If None, it will be gone after the next trigger.
    interval: Option<Duration>,
    /// If true, the timer is marked for deletion
    execution_forbidden: bool,
    /// The timer will be executed after this instant passes
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
        let key: usize = TIMERS.with_borrow_mut(|t| t.insert(timer));
        QUEUE.with_borrow_mut(|q| {
            q.push(
                key,
                Reverse(TimerScheduling {
                    next_trigger,
                    interval: if repeat { Some(interval) } else { None },
                    execution_forbidden: false,
                }),
            )
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
    #[allow(clippy::unnecessary_wraps)]
    #[samp::native(name = "DeletePreciseTimer")]
    pub fn delete(&mut self, _: &Amx, timer_number: usize) -> AmxResult<i32> {
        let key = timer_number - 1;
        if QUEUE
            .try_with(|q| {
                q.borrow_mut().change_priority_by(&key, |scheduling| {
                    scheduling.0.execution_forbidden = true;
                })
            })
            .map_err(|_| AmxError::MemoryAccess)?
        {
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

        if QUEUE
            .try_with(|q| {
                q.borrow_mut().change_priority(
                    &key,
                    Reverse(TimerScheduling {
                        next_trigger: Instant::now() + interval,
                        interval: if repeat { Some(interval) } else { None },
                        execution_forbidden: false,
                    }),
                )
            })
            .map_err(|_| AmxError::MemoryAccess)?
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
        info!("samp-precise-timers 3 by Brian Misiak loaded correctly.");
    }

    #[allow(clippy::inline_always)]
    #[inline(always)]
    fn process_tick(&mut self) {
        // Rust's Instant is monotonic and nondecreasing, even during NTP time adjustment.
        let now = Instant::now();

        while let Some((key, interval, execution_forbidden)) = next_triggered_timer(now) {
            if let (Some(interval), false) = (interval, execution_forbidden) {
                let next_trigger = now + interval;
                QUEUE.with_borrow_mut(|q| {
                    q.change_priority(
                        &key,
                        Reverse(TimerScheduling {
                            next_trigger,
                            execution_forbidden,
                            interval: Some(interval),
                        }),
                    )
                    .expect("failed to reschedule repeating timer");
                });

                // Only holding onto TIMERS until we need to push the arguments
                let (amx,idx) = TIMERS.with_borrow_mut(|t| {
                    let timer = t.get_mut(key).expect("slab should contain repeating timer");
                    let amx = samp::amx::get(timer.amx_identifier).expect("missing amx");
                    timer.passed_arguments.push_onto_amx_stack(amx, &amx.allocator()).expect("failed to push args to amx");
                    (amx, timer.amx_callback_index)
                });
                // Now that we let go of TIMERS and QUEUE,
                // we can execute the callback. It can add new timers etc
                if let Err(err) = amx.exec(idx) {
                    error!("error executing repeating timer: {}", err);
                }
            } else {
                // Must pop before the timer is executed, so that
                // the callback can't schedule anything as the very next timer before
                // we have a chance to pop from the queue.
                let (popped_key, _) = QUEUE.with_borrow_mut(|q| q.pop().expect("peeked timer gone from queue"));
                assert_eq!(popped_key, key);
                // Remove from slab
                let timer = TIMERS.with_borrow_mut(|t| t.remove(key));
                // Only execute if not marked for deletion
                if !execution_forbidden {
                    let amx = samp::amx::get(timer.amx_identifier).expect("missing amx");
                    timer.passed_arguments.push_onto_amx_stack(amx, &amx.allocator()).expect("failed to push args to amx");
                    if let Err(err) = amx.exec(timer.amx_callback_index) {
                        error!("Error executing non-repeating timer callback: {}", err);
                    }
                }
            }
        }
    }

    fn on_amx_unload(&mut self, unloaded_amx: &Amx) {
        let mut removed_timers = Vec::new();
        TIMERS.with_borrow_mut(|t| {
            t.retain(|key, timer| {
                if timer.was_scheduled_by_amx(unloaded_amx) {
                    removed_timers.push(key);
                    false
                } else {
                    true
                }
            });
        });
        QUEUE.with_borrow_mut(|q| {
            for key in removed_timers {
                q.remove(&key);
            }
        });
    }
}

fn next_triggered_timer(now: Instant) -> Option<(usize, Option<Duration>, bool)> {
    QUEUE.with_borrow(|q| match q.peek() {
        Some((
            &key,
            &Reverse(TimerScheduling {
                next_trigger,
                interval,
                execution_forbidden,
            }),
        )) if next_trigger <= now => Some((key, interval, execution_forbidden)),
        _ => None,
    })
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
