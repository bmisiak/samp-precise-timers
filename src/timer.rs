use crate::amx_arguments::VariadicAmxArguments;
use log::error;
use samp::{
    amx::AmxIdent,
    error::AmxError,
    prelude::AmxResult,
};
use std::time::{Duration, Instant};

#[derive(PartialEq)]
pub(crate) enum TimerStaus {
    MightTriggerInTheFuture,
    WillNeverTriggerAgain,
}

/// The Timer struct represents a single scheduled timer
#[derive(Debug, Clone)]
pub(crate) struct Timer {
    pub next_trigger: Instant,
    pub interval: Option<Duration>,
    pub passed_arguments: VariadicAmxArguments,
    pub amx_identifier: AmxIdent,
    pub amx_callback_index: samp::consts::AmxExecIdx,
    pub scheduled_for_removal: bool,
}

impl Timer {
    pub fn was_scheduled_by_amx(&self, amx: &samp::amx::Amx) -> bool {
        self.amx_identifier == AmxIdent::from(amx.amx().as_ptr())
    }

    /// This function executes the callback provided to the `SetPreciseTimer` native.
    pub fn trigger(&mut self) -> AmxResult<()> {
        // Get the AMX which scheduled the timer
        let amx = samp::amx::get(self.amx_identifier).ok_or(AmxError::NotFound)?;
        let allocator = amx.allocator();

        // Execute the callback (after pushing its arguments onto the stack)
        // Amx::exec should actually be marked unsafe in the samp-rs crate
        self.passed_arguments.push_onto_amx_stack(amx, &allocator)?;
        amx.exec(self.amx_callback_index)?;

        Ok(())
    }

    /// Checks if it's time to trigger the timer yet. If so, triggers it.
    /// Returns info about whether the timer is okay to remove now
    #[inline(always)]
    pub fn trigger_if_due(&mut self, now: Instant) -> TimerStaus {
        use TimerStaus::{MightTriggerInTheFuture, WillNeverTriggerAgain};

        if self.scheduled_for_removal {
            // Ordered removed. Do not execute the timer's callback.
            return WillNeverTriggerAgain;
        }
        if self.next_trigger > now {
            // Not the time to trigger it yet.
            return MightTriggerInTheFuture;
        }

        // Execute the callback:
        if let Err(err) = self.trigger() {
            error!("Error executing timer callback: {}", err);
        }

        if let Some(interval) = self.interval {
            self.next_trigger = now + interval;
            // It repeats. Keep it, unless removed from PAWN when it was triggered just now.
            // Hopfully LLVM doesn't elide this check, but it could, given that we checked
            // scheduled_for_removal earlier, .trigger() doesn't modify it, and Amx::exec
            // is wrongly marked safe despite its potential for aliased references.
            if self.scheduled_for_removal {
                WillNeverTriggerAgain
            } else {
                MightTriggerInTheFuture
            }
        } else {
            // Remove the timer. It got triggered and does not repeat
            WillNeverTriggerAgain
        }
    }
}
