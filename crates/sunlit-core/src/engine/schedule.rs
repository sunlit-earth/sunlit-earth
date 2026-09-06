//! Deadline bookkeeping for the engine loop's periodic jobs.
//!
//! Its own file because the loop's own schedule is the contract this crate
//! documents most carefully: the loop never sleeps on wall time, it asks this
//! what is due against the injected clock.

use std::time::Duration;

/// Deadline bookkeeping for one periodic job.
pub(super) struct Schedule {
    pub(super) interval: Duration,
    pub(super) next: Duration,
}

impl Schedule {
    pub(super) fn new(interval: Duration, now: Duration) -> Self {
        Self {
            interval,
            next: now + interval,
        }
    }

    /// Whether the job is due, rolling the deadline forward if so.
    ///
    /// The deadline is recomputed from `now` rather than accumulated, so a long
    /// stall (a sleeping laptop, a mock clock jumping a day) produces one run
    /// rather than a burst of catch-up runs.
    pub(super) fn due(&mut self, now: Duration) -> bool {
        if now >= self.next {
            self.next = now + self.interval;
            true
        } else {
            false
        }
    }

    pub(super) fn set_interval(&mut self, interval: Duration, now: Duration) {
        self.interval = interval;
        self.next = now + interval;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_fires_at_its_interval_and_not_before() {
        let mut s = Schedule::new(Duration::from_secs(10), Duration::ZERO);
        assert!(!s.due(Duration::from_secs(9)));
        assert!(s.due(Duration::from_secs(10)));
    }

    #[test]
    fn schedule_does_not_burst_after_a_long_stall() {
        let mut s = Schedule::new(Duration::from_secs(10), Duration::ZERO);
        // One jump of a simulated day must produce one run, not 8640.
        assert!(s.due(Duration::from_hours(24)));
        assert!(!s.due(Duration::from_hours(24)));
        assert!(s.due(Duration::from_secs(86_410)));
    }

    #[test]
    fn schedule_interval_change_restarts_the_countdown() {
        let mut s = Schedule::new(Duration::from_mins(10), Duration::ZERO);
        s.set_interval(Duration::from_mins(1), Duration::from_secs(30));
        assert!(!s.due(Duration::from_secs(89)));
        assert!(s.due(Duration::from_secs(90)));
    }
}
