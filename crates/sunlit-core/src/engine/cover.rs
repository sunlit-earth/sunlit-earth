//! Holding the automatic publishes back while another application covers
//! the desktop.
//!
//! An automatic publish that falls due asks the probe first. Covered holds
//! it, and the probe is asked again every [`COVER_RECHECK`] until the screens
//! are clear or it cannot tell, and the held publish goes out then. A probe
//! that cannot tell holds nothing.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::{debug, info};

use crate::cover::{CoverProbe, Coverage};

/// How often a held publish asks the probe again.
pub const COVER_RECHECK: Duration = Duration::from_secs(10);

/// A publish held back by a cover.
struct Held {
    since: Duration,
    recheck: Duration,
}

/// The probe and what it is holding.
pub(super) struct CoverGate {
    probe: Arc<dyn CoverProbe>,
    held: Option<Held>,
    /// The reasons an unknown answer has already been logged at info level.
    unknown_reasons: HashSet<String>,
}

impl CoverGate {
    pub(super) fn new(probe: Arc<dyn CoverProbe>) -> Self {
        Self {
            probe,
            held: None,
            unknown_reasons: HashSet::new(),
        }
    }

    /// Whether an automatic publish falling due at `now` may go out. A
    /// publish already held stays held, and covers this one too.
    pub(super) fn admit(&mut self, now: Duration, what: &str) -> bool {
        if self.held.is_some() {
            debug!(what, "an automatic publish joins the one held by a cover");
            return false;
        }
        match self.ask() {
            Coverage::Covered(cover) => {
                info!(
                    %cover,
                    what,
                    recheck_secs = COVER_RECHECK.as_secs(),
                    "another application covers the desktop; the wallpaper update is held \
                     until it is clear"
                );
                self.held = Some(Held {
                    since: now,
                    recheck: now + COVER_RECHECK,
                });
                false
            }
            Coverage::Clear | Coverage::Unknown(_) => true,
        }
    }

    /// Whether the publish held is to go out at `now`: the recheck fell due
    /// and the probe no longer answers covered. Ends the hold if so.
    pub(super) fn released(&mut self, now: Duration) -> bool {
        let Some(held) = &mut self.held else {
            return false;
        };
        if now < held.recheck {
            return false;
        }
        if let Coverage::Covered(cover) = self.ask() {
            debug!(%cover, "the desktop is still covered");
            if let Some(held) = &mut self.held {
                held.recheck = now + COVER_RECHECK;
            }
            return false;
        }
        self.release(now, "the desktop is clear");
        true
    }

    /// End a hold because a publish goes out anyway. Returns whether there
    /// was one.
    pub(super) fn end(&mut self, now: Duration, why: &str) -> bool {
        if self.held.is_none() {
            return false;
        }
        self.release(now, why);
        true
    }

    fn release(&mut self, now: Duration, why: &str) {
        if let Some(held) = self.held.take() {
            info!(
                held_secs = now.saturating_sub(held.since).as_secs(),
                "{why}; the held wallpaper update goes out"
            );
        }
    }

    fn ask(&mut self) -> Coverage {
        let started = Instant::now();
        let coverage = self.probe.probe();
        let took_ms = started.elapsed().as_secs_f64() * 1000.0;
        debug!(%coverage, took_ms, "asked whether another application covers the desktop");
        if let Coverage::Unknown(reason) = &coverage
            && self.unknown_reasons.insert(reason.clone())
        {
            info!(
                reason,
                "cannot tell whether another application covers the desktop; wallpaper \
                 updates go out as usual"
            );
        }
        coverage
    }
}
