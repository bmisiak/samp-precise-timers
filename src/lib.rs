use samp::amx::{Amx,AmxIdent};
use samp::cell::{AmxCell, AmxString, Ref};
use samp::error::{AmxResult,AmxError};
use samp::plugin::SampPlugin;

use std::time::{Instant,Duration};
use slab::Slab;
use log::{info, error};

/// These are the types of arguments the plugin supports for passing on to the callback.
#[derive(Debug, Clone)]
enum PassedArgument {
    PrimitiveCell(i32),
    Str(Vec<u8>),
    Array(Vec<i32>),
}

/// The Timer struct represents a single scheduled timer
#[derive(Debug, Clone)]
struct Timer {
    next_trigger: Instant,
    interval: Option<Duration>,
    passed_arguments: Vec<PassedArgument>,
    amx_identifier: AmxIdent,
    amx_callback_index: samp::consts::AmxExecIdx,
    scheduled_for_removal: bool
}

impl Timer {
    /// This function executes the callback provided to the `SetPreciseTimer` native.
    pub fn trigger(&mut self) -> AmxResult<()> {

        // Get the AMX which scheduled the timer
        let amx = samp::amx::get(self.amx_identifier).ok_or(samp::error::AmxError::NotFound)?;
        let allocator = amx.allocator();

        // Push the timer's arguments onto the AMX stack, in first-in-last-out order, i.e. reversed
        for param in self.passed_arguments.iter().rev() {
            match param {
                PassedArgument::PrimitiveCell(cell_value) => {
                    amx.push(cell_value)?;
                }
                PassedArgument::Str(bytes) => {
                    let buffer = allocator.allot_buffer(bytes.len() + 1)?;
                    let amx_str = unsafe { AmxString::new(buffer, bytes) };
                    amx.push(amx_str)?;
                },
                PassedArgument::Array(array_cells) => {
                    let amx_buffer = allocator.allot_array(array_cells.as_slice())?;
                    amx.push(array_cells.len())?; // Stacking the length first because it appears after the array.
                    amx.push(amx_buffer)?;
                }
            }
        }
        
        // Execute the callback (after pushing its arguments onto the stack)
        amx.exec(self.amx_callback_index)?;

        // Return Result::Ok() with an empty value ("unit" ()) to indicate success.
        Ok(())
    }
}

/// The plugin and its data: a list of scheduled timers
struct PreciseTimers {
    timers: Slab<Timer>,
}

impl PreciseTimers {
    /// This function is called from PAWN via the C foreign function interface.
    /// It returns the timer identifier or 0 in case of failure.
    ///  ```
    /// native SetPreciseTimer(const callback_name[], const interval, const bool:repeat, const types_of_arguments[]="", {Float,_}:...);
    /// ```
    #[samp::native(raw,name="SetPreciseTimer")]
    pub fn create(&mut self, amx: &Amx, mut args: samp::args::Args) -> AmxResult<i32> {
        
        // Get the basic, mandatory timer parameters
        let callback_name = args.next::<AmxString>().ok_or(AmxError::Params)?;
        let interval = args.next::<i32>().ok_or(AmxError::Params)?;
        let repeat = args.next::<bool>().ok_or(AmxError::Params)?;
        let argument_type_lettters = args.next::<AmxString>().ok_or(AmxError::Params)?.to_bytes(); //iterator on AmxString would be more efficient if it was implemented

        // Make sure they're sane
        if argument_type_lettters.len() != args.count() - 4 {
            error!("The amount of callback arguments passed ({}) does not match the length of the list of types ({}).",args.count() - 4, argument_type_lettters.len());
            return Err(AmxError::Params);
        }

        if interval < 0 {
            error!("Invalid interval");
            return Err(AmxError::Params);
        }

        let interval = Duration::from_millis(interval as u64);

        // Get the arguments to pass on to the callback
        let mut passed_arguments: Vec<PassedArgument> = Vec::with_capacity(argument_type_lettters.len());
        let mut type_iterator = argument_type_lettters.iter();
        
        while let Some(type_letter) = type_iterator.next() {
            match type_letter {
                b's' => {
                    let argument: Ref<i32> = args.next().ok_or(AmxError::Params)?;
                    let amx_str = AmxString::from_raw(amx,argument.address())?;
                    passed_arguments.push( PassedArgument::Str(amx_str.to_bytes()) );
                },
                b'a' => {
                    if let Some(b'i') | Some(b'A') = type_iterator.next() {
                        let array_argument: samp::cell::UnsizedBuffer = args.next().ok_or(AmxError::Params)?;
                        let length_argument: Ref<i32> = args.next().ok_or(AmxError::Params)?;
                        
                        if *length_argument < 0 {
                            error!("Array size cannot be negative.");
                            return Err(AmxError::Params);
                        }

                        let amx_buffer = array_argument.into_sized_buffer(*length_argument as usize);
                        passed_arguments.push( PassedArgument::Array( amx_buffer.as_slice().to_vec() ) );
                    } else {
                        error!("Array arguments (a) must be followed by an array length argument (i/A).");
                        return Err(AmxError::Params);
                    }
                }
                _ => {
                    let argument: Ref<i32> = args.next().ok_or(AmxError::Params)?;
                    passed_arguments.push( PassedArgument::PrimitiveCell( *argument ) );
                }
            }
        }

        // Find the callback by name and save its index
        let callback_index = amx.find_public(&callback_name.to_string())?;
        
        let timer = Timer {
            next_trigger: Instant::now() + interval,
            interval: if repeat { Some(interval) } else { None },
            passed_arguments: passed_arguments,
            amx_identifier: AmxIdent::from(amx.amx().as_ptr()),
            amx_callback_index: callback_index,
            scheduled_for_removal: false
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
            },
            None => Ok(0)
        }
    }

    /// This function is called from PAWN via the C foreign function interface.
    /// Returns 0 if the timer does not exist, otherwise 
    ///  ```
    /// native ResetPreciseTimer(timer_number, const interval, const bool:repeat)
    /// ```
    #[samp::native(name = "ResetPreciseTimer")]
    pub fn reset(&mut self, _: &Amx, timer_number: usize, interval: i32, repeat: bool) -> AmxResult<i32> {
        // Subtract 1 from the passed timer_number (where 0=invalid) to get the actual Slab<> slot
        match self.timers.get_mut(timer_number - 1) {
            Some(timer) => {
                // We defer the removal so that we don't mess up the process_tick()->retain() iterator.
                let interval = Duration::from_millis(interval as u64);
                timer.next_trigger = Instant::now() + interval;
                timer.interval = if repeat { Some(interval) } else { None };
                Ok(1)
            },
            None => Ok(0)
        }
    }
}

impl SampPlugin for PreciseTimers {
    fn on_load(&mut self) {
        info!("net4game.com/samp-precise-timers by Brian Misiak loaded correctly.");
    }

    #[inline(always)]
    fn process_tick(&mut self) {
        // Rust's Instant is monotonic and nondecreasing. 💖 Works even during NTP time adjustment.
        let now = Instant::now();

        // 💀⚠ Because of FFI with C, Rust can't notice the simultaenous mutation of self.timers, but the iterator could get messed up in case of
        // Slab::retain() -> Timer::trigger() -> PAWN callback/ffi -> DeletePreciseTimer()->Slab::remove.
        // That's why the DeletePreciseTimer() schedules timers for deletion instead of doing it right away.
        // Slab::retain() is, however, okay with inserting new timers during its execution, even in case of reallocation when over capacity.
        self.timers.retain( |_key: usize, timer: &mut Timer| {
            if timer.next_trigger <= now {
                if timer.scheduled_for_removal {
                    // REMOVE timer and don't execute its callback.
                    return false;
                } else {
                    // Execute the callback:
                    if let Err(err) = timer.trigger() {
                        error!("Error executing callback: {}",err);
                    }

                    if let Some(interval) = timer.interval {
                        timer.next_trigger = now + interval;
                        // Keep the timer, because it repeats
                        return true;
                    } else {
                        // REMOVE the timer. It got triggered and does not repeat
                        return false;
                    }
                }
            } else {
                // Keep the timer because it has yet to be triggered
                return true;
            }
        });
    }

    fn on_amx_unload(&mut self, unloaded_amx: &Amx) {
        // Retain only timers scheduled by an AMX other than the unloaded one.
        self.timers.retain( |_key: usize, timer: &mut Timer| {
            timer.amx_identifier != AmxIdent::from(unloaded_amx.amx().as_ptr())
        });
    }
}

samp::initialize_plugin!(
    natives: [
        PreciseTimers::delete,
        PreciseTimers::create,
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
        
        return PreciseTimers {
            timers: Slab::with_capacity(1000)
        };
    }
);
