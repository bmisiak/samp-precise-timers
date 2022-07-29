use std::time::{Instant, Duration};
use samp::{amx::AmxIdent, prelude::{AmxResult, AmxString}, error::AmxError};
use crate::amx_arguments::PassedArgument;

/// The Timer struct represents a single scheduled timer
#[derive(Debug, Clone)]
pub struct Timer {
    pub next_trigger: Instant,
    pub interval: Option<Duration>,
    pub passed_arguments: Vec<PassedArgument>,
    pub amx_identifier: AmxIdent,
    pub amx_callback_index: samp::consts::AmxExecIdx,
    pub scheduled_for_removal: bool,
}

impl Timer {
    /// This function executes the callback provided to the `SetPreciseTimer` native.
    pub fn trigger(&mut self) -> AmxResult<()> {
        // Get the AMX which scheduled the timer
        let amx = samp::amx::get(self.amx_identifier).ok_or(AmxError::NotFound)?;
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
                }
                PassedArgument::Array(array_cells) => {
                    let amx_buffer = allocator.allot_array(array_cells.as_slice())?;
                    amx.push(array_cells.len())?;
                    amx.push(amx_buffer)?;
                }
            }
        }

        // Execute the callback (after pushing its arguments onto the stack)
        // Amx::exec should actually be marked unsafe in the samp-rs crate
        amx.exec(self.amx_callback_index)?;

        Ok(())
    }

    pub fn was_scheduled_by_amx(&self, amx: &samp::amx::Amx) -> bool {
        self.amx_identifier == AmxIdent::from(amx.amx().as_ptr())
    }
}