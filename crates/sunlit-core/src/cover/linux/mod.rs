//! Linux: which routes this session offers, and how their answers combine.
//!
//! No single interface tells an ordinary client about other applications'
//! windows on every Linux desktop, so the probe asks every route that applies
//! and combines them: any covered wins, clear needs every route clear, and
//! anything else is unknown.
//!
//! The choice of routes and the combination are pure functions over
//! [`SessionFacts`], compiled and tested on every platform.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::Receiver;

use super::Coverage;

#[cfg(target_os = "linux")]
mod kwin;
#[cfg(target_os = "linux")]
mod probe;
#[cfg(target_os = "linux")]
mod session;
#[cfg(target_os = "linux")]
mod wlr;
#[cfg(target_os = "linux")]
mod x11;

#[cfg(target_os = "linux")]
pub(crate) use probe::LinuxProbe;

/// One way of asking about other applications' windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Route {
    /// EWMH and `RandR` over the X server, which sees X11 and `XWayland` windows.
    X11,
    /// `zwlr_foreign_toplevel_manager_v1`, on wlroots compositors.
    Wlr,
    /// A `KWin` script, on Plasma's Wayland session.
    KWin,
    /// `org.gnome.SessionManager`'s idle inhibition, where nothing else sees
    /// native Wayland windows.
    SessionManager,
}

impl Route {
    pub fn name(self) -> &'static str {
        match self {
            Self::X11 => "X11",
            Self::Wlr => "wlr foreign toplevel",
            Self::KWin => "KWin script",
            Self::SessionManager => "session manager",
        }
    }
}

/// What the session offers, as far as the route choice needs to know.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one fact per thing the session may or may not offer"
)]
pub struct SessionFacts {
    /// `DISPLAY` is set.
    pub x11: bool,
    /// A Wayland display answered.
    pub wayland: bool,
    /// The Wayland globals include the wlr foreign toplevel manager.
    pub wlr: bool,
    /// `org.kde.KWin` is on the session bus.
    pub kwin: bool,
    /// `org.gnome.SessionManager` is on the session bus.
    pub gnome_session: bool,
}

/// The routes to ask in a session with `facts`, in the order their answers
/// are preferred.
pub fn routes(facts: SessionFacts) -> Vec<Route> {
    let mut routes = Vec::new();
    if facts.x11 {
        routes.push(Route::X11);
    }
    if facts.wayland {
        if facts.wlr {
            routes.push(Route::Wlr);
        } else if facts.kwin {
            routes.push(Route::KWin);
        } else if facts.gnome_session {
            routes.push(Route::SessionManager);
        }
    }
    routes
}

/// Whether a session with `facts` has native Wayland windows that none of
/// `routes` sees.
pub fn wayland_unseen(facts: SessionFacts, routes: &[Route]) -> bool {
    facts.wayland
        && !routes
            .iter()
            .any(|route| matches!(route, Route::Wlr | Route::KWin | Route::SessionManager))
}

/// What the routes' answers come to: the first covered wins, clear needs every
/// route clear, and anything else is unknown with every reason given.
pub fn combine(answers: Vec<(Route, Coverage)>, facts: SessionFacts) -> Coverage {
    let asked: Vec<Route> = answers.iter().map(|(route, _)| *route).collect();
    let mut reasons = Vec::new();
    if answers.is_empty() && !facts.wayland {
        reasons.push("this session has neither an X display nor a Wayland display".to_owned());
    }
    if wayland_unseen(facts, &asked) {
        reasons.push(
            "nothing this session offers reports native Wayland windows (no wlr foreign \
             toplevel manager, no KWin, no GNOME session manager)"
                .to_owned(),
        );
    }
    for (route, answer) in answers {
        match answer {
            Coverage::Covered(cover) => return Coverage::Covered(cover),
            Coverage::Unknown(reason) => reasons.push(format!("{}: {reason}", route.name())),
            Coverage::Clear => {}
        }
    }
    if reasons.is_empty() {
        Coverage::Clear
    } else {
        Coverage::Unknown(reasons.join("; "))
    }
}

/// Run `query` on a thread of its own and hand back where its answer will
/// arrive, unless the last query under `busy` is still running.
///
/// The answer crosses in a channel of one, read by the probe that started the
/// query, or by nobody once that probe's deadline has passed, in which case
/// the query finishes, clears `busy` and its answer is dropped with the
/// channel. `busy` is cleared however the query ends, a panic included.
pub(crate) fn on_a_thread<T: Send + 'static>(
    busy: &Arc<AtomicBool>,
    query: impl FnOnce() -> T + Send + 'static,
) -> Option<Receiver<T>> {
    struct Running(Arc<AtomicBool>);
    impl Drop for Running {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }

    if busy.swap(true, Ordering::AcqRel) {
        return None;
    }
    let (tx, rx) = crossbeam_channel::bounded(1);
    let running = Running(Arc::clone(busy));
    let spawned = std::thread::Builder::new()
        .name("cover probe".to_owned())
        .spawn(move || {
            let answer = {
                let _running = running;
                query()
            };
            let _ = tx.send(answer);
        });
    if spawned.is_err() {
        busy.store(false, Ordering::Release);
        return None;
    }
    Some(rx)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::cover::{Cover, Signal};

    fn covered(application: &str) -> Coverage {
        Coverage::Covered(Cover {
            application: Some(application.to_owned()),
            pid: None,
            monitor: None,
            signal: Signal::FullScreenState,
        })
    }

    #[test]
    fn an_x11_session_asks_x11_alone() {
        let facts = SessionFacts {
            x11: true,
            gnome_session: true,
            ..SessionFacts::default()
        };
        assert_eq!(routes(facts), [Route::X11]);
    }

    #[test]
    fn sway_with_xwayland_asks_x11_and_wlr() {
        let facts = SessionFacts {
            x11: true,
            wayland: true,
            wlr: true,
            ..SessionFacts::default()
        };
        assert_eq!(routes(facts), [Route::X11, Route::Wlr]);
    }

    #[test]
    fn plasma_wayland_asks_x11_and_kwin() {
        let facts = SessionFacts {
            x11: true,
            wayland: true,
            kwin: true,
            ..SessionFacts::default()
        };
        assert_eq!(routes(facts), [Route::X11, Route::KWin]);
    }

    #[test]
    fn gnome_wayland_asks_x11_and_the_session_manager() {
        let facts = SessionFacts {
            x11: true,
            wayland: true,
            gnome_session: true,
            ..SessionFacts::default()
        };
        assert_eq!(routes(facts), [Route::X11, Route::SessionManager]);
    }

    #[test]
    fn a_wayland_compositor_with_nothing_is_unknown_unless_covered() {
        let facts = SessionFacts {
            x11: true,
            wayland: true,
            ..SessionFacts::default()
        };
        assert_eq!(routes(facts), [Route::X11]);
        assert!(matches!(
            combine(vec![(Route::X11, Coverage::Clear)], facts),
            Coverage::Unknown(_)
        ));
        assert_eq!(
            combine(vec![(Route::X11, covered("wine"))], facts),
            covered("wine")
        );
        let bare = SessionFacts {
            wayland: true,
            ..SessionFacts::default()
        };
        assert_eq!(routes(bare), []);
        assert!(matches!(combine(Vec::new(), bare), Coverage::Unknown(_)));
    }

    #[test]
    fn no_display_at_all_is_unknown() {
        assert!(matches!(
            combine(Vec::new(), SessionFacts::default()),
            Coverage::Unknown(_)
        ));
    }

    #[test]
    fn covered_wins_over_unknown() {
        let facts = SessionFacts {
            x11: true,
            wayland: true,
            kwin: true,
            ..SessionFacts::default()
        };
        let answer = combine(
            vec![
                (Route::X11, Coverage::Unknown("timed out".to_owned())),
                (Route::KWin, covered("vlc")),
            ],
            facts,
        );
        assert_eq!(answer, covered("vlc"));
    }

    #[test]
    fn a_query_still_running_is_not_started_again() {
        let busy = Arc::new(AtomicBool::new(false));
        let (release, wait) = crossbeam_channel::bounded::<()>(0);
        let first = on_a_thread(&busy, move || wait.recv().is_ok()).expect("started");
        assert!(
            on_a_thread(&busy, || true).is_none(),
            "started on top of itself"
        );
        release.send(()).expect("the query waits");
        assert_eq!(first.recv_timeout(Duration::from_secs(10)), Ok(true));
        let again = on_a_thread(&busy, || true).expect("started once it returned");
        assert_eq!(again.recv_timeout(Duration::from_secs(10)), Ok(true));
    }

    #[test]
    fn a_query_that_panics_frees_its_route() {
        let busy = Arc::new(AtomicBool::new(false));
        let answer =
            on_a_thread(&busy, || -> bool { panic!("the route failed") }).expect("started");
        assert!(
            answer.recv_timeout(Duration::from_secs(10)).is_err(),
            "a panicking query answers nothing"
        );
        let again = on_a_thread(&busy, || true).expect("the route was freed");
        assert_eq!(again.recv_timeout(Duration::from_secs(10)), Ok(true));
    }

    #[test]
    fn clear_needs_every_route_clear() {
        let facts = SessionFacts {
            x11: true,
            wayland: true,
            wlr: true,
            ..SessionFacts::default()
        };
        assert_eq!(
            combine(
                vec![(Route::X11, Coverage::Clear), (Route::Wlr, Coverage::Clear)],
                facts
            ),
            Coverage::Clear
        );
        let Coverage::Unknown(reason) = combine(
            vec![
                (Route::X11, Coverage::Clear),
                (Route::Wlr, Coverage::Unknown("no answer".to_owned())),
            ],
            facts,
        ) else {
            panic!("one route unknown is unknown");
        };
        assert!(reason.contains("no answer"), "{reason}");
    }
}
