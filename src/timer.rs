use crate::amx_arguments::VariadicAmxArguments;

use samp::{amx::AmxHandle, cell::RawCell, consts::AmxExecIdx, error::AmxError};

/// The Timer struct represents a single scheduled callback
#[derive(Debug, Clone)]
pub(crate) struct Timer {
    pub passed_arguments: VariadicAmxArguments,
    pub amx: AmxHandle,
    pub amx_callback_index: AmxExecIdx,
}

impl Timer {
    /// Push the timer's arguments onto the AMX stack and execute the
    /// callback. The callback may reenter our natives (e.g.
    /// `DeletePreciseTimer`), so the caller must not hold any borrow of the
    /// scheduling state.
    ///
    /// # Panics
    /// When the scheduling script is no longer loaded — `on_amx_unload`
    /// removes its timers, so a scheduled timer without its script is a bug.
    pub fn fire(&self) -> Result<RawCell, AmxError> {
        samp::amx::with(self.amx, |amx| {
            amx.exec_with_args(self.amx_callback_index, |allocator| {
                self.passed_arguments.push_onto_amx_stack(allocator)
            })
        })
        .expect("scheduled timer must outlive its script")
    }
}
