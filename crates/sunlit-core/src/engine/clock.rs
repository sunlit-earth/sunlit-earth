//! Injectable time.
//!
//! The engine never sleeps on wall time to decide what is due. It asks the
//! clock how much time has passed and compares that against its schedule, so a
//! test can advance fourteen days in a few milliseconds and still exercise the
//! exact scheduling code production runs.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use time::OffsetDateTime;

/// Source of time for the engine.
pub trait Clock: Send + Sync {
    /// Time since this clock was created. Monotonic and never decreasing.
    fn elapsed(&self) -> Duration;

    /// Current UTC wall time, used for the sun position.
    fn now_utc(&self) -> OffsetDateTime;
}

/// The real clock: `Instant` for scheduling, the system clock for the sun.
pub struct SystemClock {
    start: Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    fn now_utc(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

/// A clock that only moves when a test tells it to.
///
/// UTC time moves with it, so simulated days really do rotate the Earth.
#[doc(hidden)]
pub struct MockClock {
    elapsed: Mutex<Duration>,
    base_utc: OffsetDateTime,
}

impl MockClock {
    /// Start at zero elapsed, with `base_utc` as the wall time.
    pub fn new(base_utc: OffsetDateTime) -> Self {
        Self {
            elapsed: Mutex::new(Duration::ZERO),
            base_utc,
        }
    }

    /// Move time forward.
    pub fn advance(&self, delta: Duration) {
        let mut elapsed = self.elapsed.lock().expect("mock clock lock poisoned");
        *elapsed += delta;
    }
}

impl Clock for MockClock {
    fn elapsed(&self) -> Duration {
        *self.elapsed.lock().expect("mock clock lock poisoned")
    }

    fn now_utc(&self) -> OffsetDateTime {
        self.base_utc + self.elapsed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_clock_starts_at_zero() {
        let clock = MockClock::new(OffsetDateTime::UNIX_EPOCH);
        assert_eq!(clock.elapsed(), Duration::ZERO);
        assert_eq!(clock.now_utc(), OffsetDateTime::UNIX_EPOCH);
    }

    /// Both measures move together, and they accumulate: the soak test walks
    /// simulated weeks one advance at a time.
    #[test]
    fn mock_clock_advances_both_measures_together_and_they_accumulate() {
        let clock = MockClock::new(OffsetDateTime::UNIX_EPOCH);
        clock.advance(Duration::from_hours(1));
        assert_eq!(clock.elapsed(), Duration::from_hours(1));
        assert_eq!(
            clock.now_utc(),
            OffsetDateTime::UNIX_EPOCH + Duration::from_hours(1)
        );

        for _ in 0..14 {
            clock.advance(Duration::from_hours(24));
        }
        assert_eq!(clock.elapsed(), Duration::from_hours(1 + 14 * 24));
        assert_eq!(
            clock.now_utc(),
            OffsetDateTime::UNIX_EPOCH + Duration::from_hours(1 + 14 * 24)
        );
    }
}
