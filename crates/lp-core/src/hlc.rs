//! Hybrid logical clock: wall-clock milliseconds plus a counter, so an edit
//! made after seeing another edit always sorts after it, even with clock skew.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Remote timestamps further than this into the future don't advance our clock,
/// so one device with a broken clock can't drag every other device forward.
pub const MAX_FUTURE_DRIFT_MS: u64 = 24 * 60 * 60 * 1000;

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct Hlc {
    #[serde(rename = "w")]
    pub wall_ms: u64,
    #[serde(rename = "c")]
    pub counter: u32,
}

#[derive(Clone, Debug, Default)]
pub struct HlcClock {
    last: Hlc,
}

impl HlcClock {
    pub fn new(last: Hlc) -> Self {
        Self { last }
    }

    pub fn last(&self) -> Hlc {
        self.last
    }

    /// Timestamp for a new local event.
    pub fn tick(&mut self, now_ms: u64) -> Hlc {
        self.last = if now_ms > self.last.wall_ms {
            Hlc {
                wall_ms: now_ms,
                counter: 0,
            }
        } else {
            Hlc {
                wall_ms: self.last.wall_ms,
                counter: self.last.counter.saturating_add(1),
            }
        };
        self.last
    }

    /// Folds in a timestamp seen on another device's op.
    pub fn observe(&mut self, remote: Hlc, now_ms: u64) {
        if remote.wall_ms <= now_ms.saturating_add(MAX_FUTURE_DRIFT_MS) && remote > self.last {
            self.last = remote;
        }
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_is_strictly_monotonic_even_if_wall_clock_goes_back() {
        let mut c = HlcClock::default();
        let a = c.tick(1_000);
        let b = c.tick(1_000);
        let d = c.tick(500);
        let e = c.tick(2_000);
        assert!(a < b && b < d && d < e);
        assert_eq!(
            e,
            Hlc {
                wall_ms: 2_000,
                counter: 0
            }
        );
    }

    #[test]
    fn observe_moves_clock_past_remote_events() {
        let mut c = HlcClock::default();
        c.tick(1_000);
        let remote = Hlc {
            wall_ms: 5_000,
            counter: 3,
        };
        c.observe(remote, 1_000);
        assert!(c.tick(1_000) > remote);
    }

    #[test]
    fn observe_ignores_far_future() {
        let mut c = HlcClock::default();
        c.observe(
            Hlc {
                wall_ms: u64::MAX / 2,
                counter: 0,
            },
            1_000,
        );
        assert_eq!(
            c.tick(1_000),
            Hlc {
                wall_ms: 1_000,
                counter: 0
            }
        );
    }
}
