//! Holding the automatic publishes back while another application covers
//! the desktop.
//!
//! An automatic publish that falls due asks the probe first. Covered holds
//! it, and the probe is asked again every [`COVER_RECHECK`] until the screens
//! are clear or it cannot tell, and the held publish goes out then. A probe
//! that cannot tell holds nothing.

use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::{debug, info};

use crate::cover::{CoverProbe, Coverage};

/// How often a held publish asks the probe again.
pub const COVER_RECHECK: Duration = Duration::from_secs(10);

/// An automatic publish, by what made it fall due.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Automatic {
    AutoRefresh,
    LayoutChange,
    Reexport,
}

impl fmt::Display for Automatic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::AutoRefresh => "auto-refresh",
            Self::LayoutChange => "the display layout change",
            Self::Reexport => "the tiles a wallpaper went out without",
        })
    }
}

/// Why a held publish went out at a recheck.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Release {
    /// The probe answered clear.
    Clear,
    /// The probe could no longer tell, which holds nothing.
    CannotTell,
}

/// A publish held back by a cover.
struct Held {
    since: Duration,
    recheck: Duration,
    /// Every automatic publish the hold stands for.
    holds: Vec<Automatic>,
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

    /// Whether the automatic publishes `what` falling due at `now` may go
    /// out. A publish already held stays held, and covers these too.
    pub(super) fn admit(&mut self, now: Duration, what: &[Automatic]) -> bool {
        if let Some(held) = &mut self.held {
            debug!(?what, "an automatic publish joins the one held by a cover");
            join(&mut held.holds, what);
            return false;
        }
        match self.ask() {
            Coverage::Covered(cover) => {
                info!(
                    %cover,
                    ?what,
                    recheck_secs = COVER_RECHECK.as_secs(),
                    "another application covers the desktop; the wallpaper update is held \
                     until it is clear"
                );
                let mut holds = Vec::new();
                join(&mut holds, what);
                self.held = Some(Held {
                    since: now,
                    recheck: now + COVER_RECHECK,
                    holds,
                });
                false
            }
            Coverage::Clear | Coverage::Unknown(_) => true,
        }
    }

    /// Whether the automatic publishes `what`, due as the engine stops, may
    /// go out. Nothing is held, since there is no tick left to release it in,
    /// and a publish already held stays in.
    pub(super) fn admit_at_exit(&mut self, what: &[Automatic]) -> bool {
        if self.held.is_some() {
            return false;
        }
        match self.ask() {
            Coverage::Covered(cover) => {
                info!(
                    %cover,
                    ?what,
                    "another application covers the desktop; the wallpaper update is dropped                      at exit"
                );
                false
            }
            Coverage::Clear | Coverage::Unknown(_) => true,
        }
    }

    /// Whether the publish held is to go out at `now`, and why: the recheck
    /// fell due and the probe no longer answers covered. Ends the hold if so.
    pub(super) fn released(&mut self, now: Duration) -> Option<Release> {
        let held = self.held.as_mut()?;
        if now < held.recheck {
            return None;
        }
        let release = match self.ask() {
            Coverage::Covered(cover) => {
                debug!(%cover, "the desktop is still covered");
                if let Some(held) = &mut self.held {
                    held.recheck = now + COVER_RECHECK;
                }
                return None;
            }
            Coverage::Clear => Release::Clear,
            Coverage::Unknown(_) => Release::CannotTell,
        };
        let why = match release {
            Release::Clear => "the desktop is clear",
            Release::CannotTell => {
                "it can no longer be told whether another application covers the desktop"
            }
        };
        self.release(now, why);
        Some(release)
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

    /// Stop holding `what`, which is no longer owed. A hold that stood for
    /// nothing else ends without a publish.
    pub(super) fn forget(&mut self, now: Duration, what: Automatic) {
        let Some(held) = &mut self.held else {
            return;
        };
        held.holds.retain(|held| *held != what);
        if held.holds.is_empty() {
            info!(
                held_secs = now.saturating_sub(held.since).as_secs(),
                "{what} is off; the wallpaper update held for it is dropped"
            );
            self.held = None;
        }
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

fn join(holds: &mut Vec<Automatic>, what: &[Automatic]) {
    for &what in what {
        if !holds.contains(&what) {
            holds.push(what);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::cover::{Cover, Signal};

    struct Scripted(Mutex<Coverage>);

    impl CoverProbe for Scripted {
        fn probe(&self) -> Coverage {
            self.0.lock().expect("unpoisoned").clone()
        }
    }

    fn covered() -> Coverage {
        Coverage::Covered(Cover {
            application: Some("player".to_owned()),
            pid: Some(4711),
            monitor: None,
            signal: Signal::FullScreenState,
        })
    }

    fn gate(answer: Coverage) -> (CoverGate, Arc<Scripted>) {
        let probe = Arc::new(Scripted(Mutex::new(answer)));
        let for_gate: Arc<dyn CoverProbe> = probe.clone();
        (CoverGate::new(for_gate), probe)
    }

    #[test]
    fn a_recheck_that_cannot_tell_releases_and_says_so() {
        let (mut gate, probe) = gate(covered());
        assert!(!gate.admit(Duration::ZERO, &[Automatic::AutoRefresh]));
        *probe.0.lock().expect("unpoisoned") = Coverage::Unknown("no route".to_owned());
        assert_eq!(gate.released(COVER_RECHECK), Some(Release::CannotTell));
        assert_eq!(gate.released(COVER_RECHECK * 2), None, "the hold ended");
    }

    #[test]
    fn a_clear_recheck_releases_as_clear() {
        let (mut gate, probe) = gate(covered());
        assert!(!gate.admit(Duration::ZERO, &[Automatic::AutoRefresh]));
        assert_eq!(gate.released(COVER_RECHECK), None, "still covered");
        *probe.0.lock().expect("unpoisoned") = Coverage::Clear;
        assert_eq!(gate.released(COVER_RECHECK * 2), Some(Release::Clear));
    }

    #[test]
    fn forgetting_the_only_thing_held_ends_the_hold() {
        let (mut gate, probe) = gate(covered());
        assert!(!gate.admit(Duration::ZERO, &[Automatic::AutoRefresh]));
        gate.forget(Duration::from_secs(1), Automatic::AutoRefresh);
        *probe.0.lock().expect("unpoisoned") = Coverage::Clear;
        assert_eq!(gate.released(COVER_RECHECK), None, "nothing is held");
        assert!(!gate.end(COVER_RECHECK, "asked"));
    }

    #[test]
    fn forgetting_one_of_two_keeps_the_hold_for_the_other() {
        let (mut gate, probe) = gate(covered());
        assert!(!gate.admit(Duration::ZERO, &[Automatic::AutoRefresh]));
        assert!(!gate.admit(Duration::from_secs(1), &[Automatic::LayoutChange]));
        gate.forget(Duration::from_secs(2), Automatic::AutoRefresh);
        *probe.0.lock().expect("unpoisoned") = Coverage::Clear;
        assert_eq!(gate.released(COVER_RECHECK), Some(Release::Clear));
    }
}
