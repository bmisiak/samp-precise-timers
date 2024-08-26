use std::time::Duration;
use std::time::Instant;

/// A struct defining when a timer gets triggered
#[derive(Copy, Clone)]
pub(crate) struct Schedule {
    pub repeat: Option<Duration>,
    pub next_trigger: Instant,
}

impl std::fmt::Debug for Schedule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(interval) = self.repeat {
            f.write_fmt(format_args!(
                "[next_trigger in {:?}, repeat {:?}]",
                self.next_trigger - Instant::now(),
                interval
            ))
        } else {
            f.write_fmt(format_args!(
                "[next_trigger in {:?}, no repeat]",
                self.next_trigger - Instant::now(),
            ))
        }
    }
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
        other.next_trigger.cmp(&self.next_trigger)
    }
}
