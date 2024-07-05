use crate::amx_arguments::{StackedCallback, VariadicAmxArguments};

use samp::{
    amx::{self, Amx, AmxIdent},
    consts::AmxExecIdx,
    error::AmxError,
};

/// The Timer struct represents a single scheduled timer
#[derive(Debug, Clone)]
pub(crate) struct Timer {
    pub passed_arguments: VariadicAmxArguments,
    pub amx_identifier: AmxIdent,
    pub amx_callback_index: AmxExecIdx,
}

impl Timer {
    pub fn was_scheduled_by_amx(&self, amx: &amx::Amx) -> bool {
        self.amx_identifier == AmxIdent::from(amx.amx().as_ptr())
    }

    pub fn stack_callback_on_amx<'amx>(&self) -> Result<StackedCallback<'amx>, AmxError> {
        // amx::get returns an invalid amx lifetime
        let amx: &'amx Amx = amx::get(self.amx_identifier).ok_or(AmxError::NotFound)?;
        self.passed_arguments
            .push_onto_amx_stack(amx, self.amx_callback_index)
    }
}
