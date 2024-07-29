#![warn(clippy::pedantic)]
use amx_arguments::VariadicAmxArguments;

use log::{error, info};
use samp::amx::Amx;
use samp::args::Args;
use samp::cell::AmxString;
use samp::consts::Supports;
use samp::error::{AmxError, AmxResult};
use scheduling::{reschedule_next_due_and_then, reschedule_timer};
use std::io::Write;

use std::convert::TryFrom;
use std::ffi::CString;
use std::ptr::NonNull;
use std::time::{Duration, Instant};
use timer::Timer;
mod amx_arguments;
mod schedule;
mod scheduling;
mod timer;
use schedule::Repeat::{DontRepeat, Every};
use schedule::Schedule;
use scheduling::{delete_timer, insert_and_schedule_timer, remove_timers};

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
        let callback_name = args.next::<AmxString>().ok_or(AmxError::Params)?;
        let interval = args
            .next::<i32>()
            .and_then(|ms| u64::try_from(ms).ok())
            .ok_or(AmxError::Params)
            .map(Duration::from_millis)?;
        let repeat = args.next::<bool>().ok_or(AmxError::Params)?;
        let passed_arguments = VariadicAmxArguments::from_amx_args::<3>(args)?;

        let timer = Timer {
            passed_arguments,
            amx_identifier: amx.amx().as_ptr().into(),
            amx_callback_index: amx.find_public(&callback_name.to_string())?,
        };
        let key = insert_and_schedule_timer(timer, |key| Schedule {
            key,
            next_trigger: Instant::now() + interval,
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
}

/// This function is called from PAWN via the C foreign function interface.
/// Returns 0 if the timer does not exist, 1 if removed.
///  ```
/// native ResetPreciseTimer(timer_number, const interval, const bool:repeat)
/// ```
extern "C" fn reset_precise_timer(amx: *mut AMX, args: *mut i32) -> i32 {
    let mut args = get_args(amx, args);
    let (Some(timer_number), Some(interval), Some(repeat)) = (
        args.next::<usize>(),
        args.next::<i32>(),
        args.next::<bool>(),
    ) else {
        return 0;
    };
    let Ok(interval) = u64::try_from(interval).map(Duration::from_millis) else {
        return 0;
    };
    let key = timer_number - 1;
    let schedule = Schedule {
        key,
        next_trigger: Instant::now() + interval,
        repeat: if repeat { Every(interval) } else { DontRepeat },
    };
    if let Err(error) = reschedule_timer(key, schedule) {
        error!("{error}");
        return AmxError::Params as i32;
    }
    1
}

use samp::raw::types::{AMX, AMX_NATIVE_INFO};
type SampLogprintf = unsafe extern "C" fn(format: *const std::os::raw::c_char, ...);
type AmxCallPublic = unsafe extern "C" fn(szFunctionName: *const std::os::raw::c_char) -> i32;
#[repr(C)]
struct SampServerData {
    logprintf: *const SampLogprintf,         // Offset 0x00
    _padding1: [u8; 0x10 - 0x08],            // Padding to reach 0x10
    amx_exports: *const std::ffi::c_void,    // Offset 0x10
    amx_callpublic_fs: *const AmxCallPublic, // Offset 0x11
    amx_callpublic_gm: *const AmxCallPublic, // Offset 0x12
}

thread_local! {
    static SERVER_DATA: std::cell::OnceCell<NonNull<SampServerData>> = std::cell::OnceCell::new();
}

fn get_amx_exports() -> *const std::ffi::c_void {
    SERVER_DATA.with(|cell| {
        let data = cell.get().expect("SERVER_DATA should be set");
        unsafe { data.as_ref() }.amx_exports
    })
}

fn amx_from_ptr(amx_ptr: *mut AMX) -> Amx {
    Amx::new(amx_ptr, get_amx_exports() as usize)
}
fn get_args<'amx>(amx: *mut AMX, args: *mut i32) -> Args<'amx> {
    Args::new(&amx_from_ptr(amx), args)
}

#[no_mangle]
pub extern "system" fn Load(server_data: NonNull<SampServerData>) -> i32 {
    let samp_logprintf = unsafe { *server_data.as_ref().logprintf };
    SERVER_DATA
        .with(|cell| cell.set(server_data))
        .expect("Server data should be unset when the plugin Load()s.");

    fern::Dispatch::new()
        .chain(fern::Output::call(|record| {
            let level = record.level();
            let message = record.args();
            let mut msg = vec![];
            write!(&mut msg, "samp-precise-timers {level}: {message}\0");
            match CString::from_vec_with_nul(msg) {
                Ok(cstr) => samp_logprintf(cstr.as_ptr()),
                Err(_) => (),
            }
        }))
        .apply()
        .unwrap();
    info!("samp-precise-timers v3 (c) Brian Misiak loaded correctly.");
    return 1;
}

#[no_mangle]
pub extern "system" fn AmxLoad(amx_ptr: *mut AMX) {
    let amx = amx_from_ptr(amx_ptr);

    let natives = [
        AMX_NATIVE_INFO {
            name: c"ResetPreciseTimer".as_ptr(),
            func: reset_precise_timer,
        },
        AMX_NATIVE_INFO {
            name: c"ResetPreciseTimer".as_ptr(),
            func: reset_precise_timer,
        },
        AMX_NATIVE_INFO {
            name: c"ResetPreciseTimer".as_ptr(),
            func: reset_precise_timer,
        },
    ];

    amx.register(&natives).unwrap();
}

#[no_mangle]
pub extern "system" fn AmxUnload(unloaded_amx: NonNull<AMX>) {
    remove_timers(|timer| timer.was_scheduled_by_amx(unloaded_amx));
}

#[no_mangle]
pub extern "system" fn Supports() -> u32 {
    return (Supports::AMX_NATIVES | Supports::VERSION | Supports::PROCESS_TICK).bits();
}

#[no_mangle]
pub extern "system" fn ProcessTick() {
    let now = Instant::now();
    while let Some(callback) = reschedule_next_due_and_then(now, Timer::stack_callback_on_amx) {
        match callback {
            Ok(stacked_callback) => {
                // SAFETY: We are not holding any references to scheduling stores.
                if let Err(exec_err) = unsafe { stacked_callback.execute() } {
                    error!("Error while executing timer: {exec_err}");
                }
            }
            Err(stacking_err) => error!("Failed to stack callback: {stacking_err}"),
        }
    }
}
