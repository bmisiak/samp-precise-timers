use std::time::Duration;
use std::time::Instant;

#[derive(Copy, Clone)]
pub(crate) enum Repeat {
    Every(Duration),
    DontRepeat,
}

/// A struct defining when a timer gets triggered
#[derive(Copy, Clone)]
pub(crate) struct Schedule {
    /// If Some, it's a repeating timer.
    /// If None, it will be gone after the next trigger.
    pub repeat: Repeat,
    /// The timer will be executed after this instant passes
    pub next_trigger: Instant,
}

impl PartialEq for Schedule {
    fn eq(&self, other: &Self) -> bool {
        self.next_trigger == other.next_trigger
    }
}

impl Eq for Schedule {}

impl PartialOrd for Schedule {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Schedule {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.next_trigger.cmp(&other.next_trigger)
    }
}
