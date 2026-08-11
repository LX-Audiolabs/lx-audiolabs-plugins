//! UI tick throttling (~30 Hz).

use std::time::{Duration, Instant};

/// Default host→UI poll interval used across all LX Audiolabs plugins.
pub const TICK_INTERVAL: Duration = Duration::from_millis(33);

/// Small state machine that throttles UI updates to [`TICK_INTERVAL`].
///
/// The first call after construction is always considered "due" so that a
/// freshly opened editor window is fully populated immediately.
#[derive(Debug, Clone)]
pub struct TickCache {
    last_tick: Instant,
    primed: bool,
}

impl TickCache {
    pub fn new() -> Self {
        Self {
            last_tick: Instant::now()
                .checked_sub(TICK_INTERVAL)
                .unwrap_or_else(Instant::now),
            primed: false,
        }
    }

    /// Returns `true` once per [`TICK_INTERVAL`], plus always on the very
    /// first call.
    pub fn due(&mut self) -> bool {
        let now = Instant::now();
        if !self.primed || now.duration_since(self.last_tick) >= TICK_INTERVAL {
            self.last_tick = now;
            self.primed = true;
            true
        } else {
            false
        }
    }
}

impl Default for TickCache {
    fn default() -> Self {
        Self::new()
    }
}
