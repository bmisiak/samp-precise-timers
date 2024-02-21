use crate::amx_arguments::VariadicAmxArguments;

use samp::{amx::AmxIdent, error::AmxError, prelude::AmxResult};

/// The Timer struct represents a single scheduled timer
#[derive(Debug, Clone)]
pub(crate) struct Timer {
    pub passed_arguments: VariadicAmxArguments,
    pub amx_identifier: AmxIdent,
    pub amx_callback_index: samp::consts::AmxExecIdx,
}

impl Timer {
    pub fn was_scheduled_by_amx(&self, amx: &samp::amx::Amx) -> bool {
        self.amx_identifier == AmxIdent::from(amx.amx().as_ptr())
    }

    /// This function executes the callback provided to the `SetPreciseTimer` native.
    pub fn execute_pawn_callback(&mut self) -> AmxResult<()> {
        // Get the AMX which scheduled the timer
        let amx = samp::amx::get(self.amx_identifier).ok_or(AmxError::NotFound)?;
        let allocator = amx.allocator();

        // Execute the callback (after pushing its arguments onto the stack)
        // Amx::exec should actually be marked unsafe in the samp-rs crate
        self.passed_arguments.push_onto_amx_stack(amx, &allocator)?;
        amx.exec(self.amx_callback_index)?;

        Ok(())
    }
}
