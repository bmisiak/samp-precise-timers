use crate::amx_arguments::VariadicAmxArguments;

use samp::{amx::{Amx, AmxIdent}, consts::AmxExecIdx};

/// The Timer struct represents a single scheduled timer
#[derive(Debug, Clone)]
pub(crate) struct Timer {
    pub passed_arguments: VariadicAmxArguments,
    pub amx_identifier: AmxIdent,
    pub amx_callback_index: AmxExecIdx,
}

impl Timer {
    pub fn was_scheduled_by_amx(&self, amx: &samp::amx::Amx) -> bool {
        self.amx_identifier == AmxIdent::from(amx.amx().as_ptr())
    }
}