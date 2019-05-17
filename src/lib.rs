#![feature(drain_filter)]

use samp::amx::Amx;
use samp::cell::{AmxCell, AmxString, Ref};//, UnsizedBuffer, Buffer};
use samp::error::{AmxResult,AmxError};
use samp::plugin::SampPlugin;
use samp::{initialize_plugin, native};

use std::time::{Instant,Duration};

use log::{info, error};

static mut EXECUTING_CALLBACK: bool = false;

/// These are the types of arguments the plugin supports for passing on to the callback.
#[derive(Debug, Clone)]
enum PassedArgument {
    Int(i32),
    Float(f32),
    Str(Vec<u8>)
}

/// Internal struct representing a single scheduled timer
#[derive(Debug, Clone)]
struct Timer {
    next_trigger: Instant,
    interval: Option<Duration>,
    passed_arguments: Vec<PassedArgument>,
    amx_identifier: samp::amx::AmxIdent,
    amx_callback_index: samp::consts::AmxExecIdx,
    scheduled_for_removal: bool
}

impl Timer {
    /// This function executes the callback provided to the `SetPreciseTimer` native.
    pub fn trigger(&mut self) -> AmxResult<()> {

        /* Get the AMX which scheduled the timer */
        let amx = samp::amx::get(self.amx_identifier).ok_or(samp::error::AmxError::NotFound)?;
        let allocator = amx.allocator();

        /* Push the timer's arguments onto the AMX stack, in first-in-last-out order, i.e. reversed */
        for param in self.passed_arguments.iter().rev() {
            match param {
                PassedArgument::Int(int_value) => amx.push(int_value)?,
                PassedArgument::Float(float_value) => amx.push(float_value)?,
                PassedArgument::Str(bytes) => {
                    let buffer = allocator.allot_buffer(bytes.len() + 1)?;
                    let amx_str = unsafe { AmxString::new(buffer, bytes) };
                    amx.push(amx_str)?
                }
            }
        }
        
        /* Execute the callback (after pushing its arguments onto the stack) */
        amx.exec(self.amx_callback_index)?;

        /* Return Result::Ok() with an empty value ("unit" ()) to indicate success. */
        Ok(())
    }
}

/// The plugin and its data: a list of scheduled timers
struct PreciseTimers {
    timers: Vec<Timer>,
    timers_added_from_pawn_callbacks_during_iteration: Vec<Timer>
}

impl PreciseTimers {
    /// This function is called from PAWN via the C foreign function interface.
    /// It returns the timer identifier or 0 in case of failure.
    ///  ```
    /// native SetPreciseTimer(const callback_name[], const interval, const bool:repeat, const types_of_arguments[]="", ...);
    /// ```
    #[native(raw, name="SetPreciseTimer")]
    pub fn create(&mut self, amx: &Amx, mut args: samp::args::Args) -> AmxResult<i32> {
        
        /* Get the basic, mandatory timer parameters */
        let callback_name = args.next::<AmxString>().ok_or(AmxError::Params)?;
        let interval = args.next::<i32>().ok_or(AmxError::Params)?;
        let repeat = args.next::<bool>().ok_or(AmxError::Params)?;
        let argument_type_lettters = args.next::<AmxString>().ok_or(AmxError::Params)?.to_bytes(); //iterator on AmxString would be more efficient if it was implemented

        /* Make sure they're sane */
        if argument_type_lettters.len() != args.count() - 4 {
            error!("The amount of arguments passed does not match the list of types.");
            return Err(AmxError::Params);
        }

        if interval < 0 {
            error!("Invalid interval");
            return Err(AmxError::Params);
        }

        let interval = Duration::from_millis(interval as u64);

        /* Get the arguments to pass to the callback */
        let mut argument_types_str_iterator = argument_type_lettters.iter();
        let mut passed_arguments: Vec<PassedArgument> = Vec::with_capacity(argument_type_lettters.len());

        while let Some(arg) = args.next::<Ref<i32>>() {
            match argument_types_str_iterator.next() { //if samp::args::Args implemented Iterator we could .zip args with letters
                Some(b'd') | Some(b'i') => {
                    passed_arguments.push(PassedArgument::Int(arg.as_cell()));
                }
                Some(b'f') => {
                    passed_arguments.push(PassedArgument::Float(arg.as_cell() as f32));
                }
                Some(b's') => {
                    /*let buffer = UnsizedBuffer {
                        inner: arg
                    };*/
                    let amx_str = samp::cell::AmxString::from_raw(amx,arg.as_cell())?;
                    passed_arguments.push(PassedArgument::Str(amx_str.to_bytes()));
                }
                None => {
                    error!("Not enough argument types provided");
                }
                _ => {
                    error!("Unsupported argument type provided");
                }
            }
        }

        /* Find the callback by name and save its index */
        let callback_index = amx.find_public(&callback_name.to_string())?;
        
        /* Add the timer to the list. Won't this mess up the iterator in case of a reallocation? */
        let timer = Timer {
            next_trigger: Instant::now() + interval,
            interval: if repeat { Some(interval) } else { None },
            passed_arguments: passed_arguments,
            amx_identifier: samp::amx::AmxIdent::from(amx.amx().as_ptr()),
            amx_callback_index: callback_index,
            scheduled_for_removal: false
        };
        
        /* 
        💀⚠ If our process_tick is in progress, it means one of the timers' callbacks called SetPreciseTimer
        (this very function). If we add the timer to self.timers at this point, we could invalidate the
        drain_filter() iterator. Instead, we add the timer to a separate list.
        Then, in process_tick, we merge the list into self.timers after iteration.
        */
        if unsafe { EXECUTING_CALLBACK } {
            self.timers_added_from_pawn_callbacks_during_iteration.push(timer);
        } else {
            self.timers.push(timer);
        }

        /* Return the timer's slot in Vec<> incresed by 1, so that 0 signifies an invalid timer in PAWN */
        Ok(self.timers.len() as i32)
    }

    /// This function is called from PAWN via the C foreign function interface.
    /// Returns 0 if the timer does not exist.
    ///  ```
    /// native DeletePreciseTimer(timer_number)
    /// ```
    #[native(name = "DeletePreciseTimer")]
    pub fn delete(&mut self, _: &Amx, timer_number: usize) -> AmxResult<i32> {
        /* Subtract 1 from the passed timer_number to get the actual Vec<> slot */
        match self.timers.get_mut(timer_number - 1) {
            Some(timer) => {
                /* We defer the removal so that we don't mess up the process_tick()->drain_filter() iterator. */
                timer.scheduled_for_removal = true;
                Ok(1)
            },
            None => Ok(0)
        }
    }
}

impl SampPlugin for PreciseTimers {
    fn on_load(&mut self) {
        info!("samp-precise-imers by Brian Misiak loaded correctly.");
    }

    #[inline(always)]
    fn process_tick(&mut self) {
        // Rust's Instant is monotonic and nondecreasing. 💖 Works even during NTP time adjustment.
        let now = Instant::now();

        // 💀⚠ Because of FFI with C, Rust can't notice the simultaenous mutation of self.timers, but the iterator can get messed up in case of
        // drain_filter() -> Timer::trigger() -> PAWN callback/ffi -> PreciseTimers::create/delete -> self.timers.push (when over capacity)/remove. 
        // That's why the DeletePreciseTimer() schedules timers for deletion instead of doing it right away, and SetPreciseTimer() adds them to a separate list.
        self.timers.append(&mut self.timers_added_from_pawn_callbacks_during_iteration);

        self.timers.drain_filter( |timer: &mut Timer| {
            if timer.next_trigger >= now {
                if timer.scheduled_for_removal {
                    //If scheduled for deletion, delete and don't execute the callback.
                    return true;
                } else {
                    unsafe { EXECUTING_CALLBACK = true; }
                    
                    // Execute the callback:
                    if let Err(err) = timer.trigger() {
                        error!("Error executing callback: {}",err);
                    }

                    unsafe { EXECUTING_CALLBACK = false; }
                    
                    if let Some(interval) = timer.interval {
                        timer.next_trigger = now + interval;
                        //Keep the timer, because it repeats
                        return false;
                    } else {
                        //REMOVE the timer. It got triggered and does not repeat
                        return true;
                    }
                }
            } else {
                //Keep the timer because it has yet to be triggered
                return false;
            }
        });
    }
}

initialize_plugin!(
    natives: [
        PreciseTimers::delete,
        PreciseTimers::create,
    ],
    {
        samp::plugin::enable_process_tick();

        // get the default samp logger (uses samp logprintf).
        let samp_logger = samp::plugin::logger()
            .level(log::LevelFilter::Info); // logging info, warn and error messages

        let _ = fern::Dispatch::new()
            .format(|callback, message, record| {
                callback.finish(format_args!("samp-precise-timers {}: {}", record.level().to_string().to_lowercase(), message))
            })
            .chain(samp_logger)
            .apply();
        
        return PreciseTimers {
            timers: Vec::with_capacity(1000),
            timers_added_from_pawn_callbacks_during_iteration: Vec::with_capacity(10)
        };
    }
);