use crate::amx_arguments::{StackedCallback, VariadicAmxArguments};

use samp::{
    amx::{self, Amx},
    consts::AmxExecIdx,
    error::AmxError,
};

/// The Timer struct represents a single scheduled timer
#[derive(Debug, Clone)]
pub(crate) struct Timer {
    pub passed_arguments: VariadicAmxArguments,
    pub amx: Amx,
    pub amx_callback_index: AmxExecIdx,
}

impl Timer {
    pub fn was_scheduled_by_amx(&self, amx: &amx::Amx) -> bool {
        self.amx.amx() == amx.amx()
    }

    pub fn stack_callback_on_amx(&self) -> Result<StackedCallback, AmxError> {
        self.passed_arguments
            .push_onto_amx_stack(self.amx.clone(), self.amx_callback_index)
    }
}
