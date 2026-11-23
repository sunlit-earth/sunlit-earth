//! The live probe: the session's facts, read once, and its routes asked in
//! parallel under one deadline.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError};

use super::{Route, SessionFacts, combine, on_a_thread, routes};
use crate::cover::{CoverProbe, Coverage};

/// How long a probe waits for the session's facts and every route's answer
/// together.
pub(crate) const PROBE_DEADLINE: Duration = Duration::from_millis(250);

pub(crate) struct LinuxProbe {
    facts: Arc<OnceLock<SessionFacts>>,
    bus: Arc<OnceLock<Option<zbus::blocking::Connection>>>,
    kwin: Arc<super::kwin::KWinRoute>,
    /// One flag per query that runs on a thread of its own, set while it
    /// runs, so a query that never returns is not started again on top of
    /// itself.
    busy: [Arc<AtomicBool>; 5],
}

const FACTS: usize = 4;

impl LinuxProbe {
    pub(crate) fn new() -> Self {
        Self {
            facts: Arc::new(OnceLock::new()),
            bus: Arc::new(OnceLock::new()),
            kwin: Arc::new(super::kwin::KWinRoute::new()),
            busy: std::array::from_fn(|_| Arc::new(AtomicBool::new(false))),
        }
    }

    fn busy(&self, route: Route) -> &Arc<AtomicBool> {
        &self.busy[match route {
            Route::X11 => 0,
            Route::Wlr => 1,
            Route::KWin => 2,
            Route::SessionManager => 3,
        }]
    }

    fn facts(&self, deadline: Instant) -> Result<SessionFacts, String> {
        if let Some(facts) = self.facts.get() {
            return Ok(*facts);
        }
        let (facts, bus) = (Arc::clone(&self.facts), Arc::clone(&self.bus));
        let answer = on_a_thread(&self.busy[FACTS], move || {
            let found = session_facts(&bus);
            let _ = facts.set(found);
            found
        })
        .ok_or_else(|| "the session's facts are still being read".to_owned())?;
        answer.recv_deadline(deadline).map_err(|_| {
            format!(
                "the session's facts were not read within {} ms",
                PROBE_DEADLINE.as_millis()
            )
        })
    }

    fn start(&self, route: Route) -> Option<Receiver<Coverage>> {
        let busy = self.busy(route);
        match route {
            Route::X11 => on_a_thread(busy, super::x11::probe),
            Route::Wlr => on_a_thread(busy, super::wlr::probe),
            Route::KWin => {
                let kwin = Arc::clone(&self.kwin);
                let bus = Arc::clone(&self.bus);
                on_a_thread(busy, move || match bus.get().and_then(Option::as_ref) {
                    Some(bus) => kwin.probe(bus),
                    None => Coverage::Unknown("the session bus could not be reached".to_owned()),
                })
            }
            Route::SessionManager => {
                let bus = Arc::clone(&self.bus);
                on_a_thread(busy, move || match bus.get().and_then(Option::as_ref) {
                    Some(bus) => super::session::probe(bus),
                    None => Coverage::Unknown("the session bus could not be reached".to_owned()),
                })
            }
        }
    }
}

impl CoverProbe for LinuxProbe {
    fn probe(&self) -> Coverage {
        let deadline = Instant::now() + PROBE_DEADLINE;
        let facts = match self.facts(deadline) {
            Ok(facts) => facts,
            Err(reason) => return Coverage::Unknown(reason),
        };
        let started: Vec<(Route, Option<Receiver<Coverage>>)> = routes(facts)
            .into_iter()
            .map(|route| (route, self.start(route)))
            .collect();
        let answers = started
            .into_iter()
            .map(|(route, answer)| {
                let coverage = match answer {
                    None => Coverage::Unknown("the previous query has not returned".to_owned()),
                    Some(answer) => answer.recv_deadline(deadline).unwrap_or_else(|e| {
                        Coverage::Unknown(match e {
                            RecvTimeoutError::Timeout => {
                                format!("no answer within {} ms", PROBE_DEADLINE.as_millis())
                            }
                            RecvTimeoutError::Disconnected => "the query failed".to_owned(),
                        })
                    }),
                };
                (route, coverage)
            })
            .collect();
        combine(answers, facts)
    }
}

fn session_facts(bus: &OnceLock<Option<zbus::blocking::Connection>>) -> SessionFacts {
    let set = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    let globals = if set("WAYLAND_DISPLAY") {
        crate::desktop::probe::wayland_globals()
    } else {
        None
    };
    let bus = bus.get_or_init(|| zbus::blocking::Connection::session().ok());
    let owned = |name: &str| {
        bus.as_ref()
            .is_some_and(|bus| crate::desktop::probe::name_has_owner(bus, name))
    };
    SessionFacts {
        x11: set("DISPLAY"),
        wayland: globals.is_some(),
        wlr: globals
            .as_ref()
            .is_some_and(|globals| globals.iter().any(|g| g == super::wlr::MANAGER)),
        kwin: owned("org.kde.KWin"),
        gnome_session: owned("org.gnome.SessionManager"),
    }
}
