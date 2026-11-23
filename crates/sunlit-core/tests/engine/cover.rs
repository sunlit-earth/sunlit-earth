//! Holding the automatic publishes while another application covers the
//! desktop.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use sunlit_core::cover::{Cover, CoverProbe, Coverage, Signal};
use sunlit_core::engine::clock::MockClock;
use sunlit_core::engine::{COVER_RECHECK, DISPLAY_SETTLE, EngineCommand};

use crate::display_change::{CHANGE_TIMEOUT, NOTHING_HAPPENS_IN};
use crate::harness::{Gpu, Harness, gpu};
use crate::sinks::{RecordingSink, screen, two_screens};

const INTERVAL: Duration = Duration::from_mins(1);

/// A probe that answers what the case tells it to, and counts the questions.
pub(crate) struct ScriptedProbe {
    answer: Mutex<Coverage>,
    asked: AtomicUsize,
}

impl ScriptedProbe {
    pub(crate) fn new() -> Self {
        Self {
            answer: Mutex::new(Coverage::Clear),
            asked: AtomicUsize::new(0),
        }
    }

    pub(crate) fn answer(&self, coverage: Coverage) {
        *self.answer.lock().expect("the probe is poisoned") = coverage;
    }

    pub(crate) fn cover(&self) {
        self.answer(Coverage::Covered(Cover {
            application: Some("player".to_owned()),
            pid: Some(4711),
            monitor: Some("DP-1".to_owned()),
            signal: Signal::FullScreenState,
        }));
    }

    pub(crate) fn asked(&self) -> usize {
        self.asked.load(Ordering::SeqCst)
    }
}

impl CoverProbe for ScriptedProbe {
    fn probe(&self) -> Coverage {
        self.asked.fetch_add(1, Ordering::SeqCst);
        self.answer.lock().expect("the probe is poisoned").clone()
    }
}

struct Covering {
    harness: Harness,
    sink: Arc<RecordingSink>,
    clock: Arc<MockClock>,
    probe: Arc<ScriptedProbe>,
}

impl std::ops::Deref for Covering {
    type Target = Harness;
    fn deref(&self) -> &Harness {
        &self.harness
    }
}

static COVERING: LazyLock<Covering> = LazyLock::new(start_covering);

fn start_covering() -> Covering {
    let sink = Arc::new(RecordingSink::new(two_screens()));
    let clock = Arc::new(MockClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let probe = Arc::new(ScriptedProbe::new());
    let (sink_for_config, clock_for_config, probe_for_config) =
        (Arc::clone(&sink), Arc::clone(&clock), Arc::clone(&probe));
    let harness = Harness::start(move |config| {
        config.wallpaper = sink_for_config;
        config.clock = clock_for_config;
        config.cover = Some(probe_for_config);
        config.auto_refresh = Some(INTERVAL);
    });
    Covering {
        harness,
        sink,
        clock,
        probe,
    }
}

/// The engine with a scripted probe and a one minute auto-refresh, with no
/// hold standing, the desk holding one of its pictures, the countdown
/// starting now, and nothing recorded.
fn covering(_gpu: &Gpu) -> &'static Covering {
    let group = &*COVERING;
    group.harness.reset();
    group.probe.answer(Coverage::Clear);
    group.sink.reset(two_screens());
    assert!(group.harness.publish().is_ok());
    group.harness.engine.send(EngineCommand::SetAutoRefresh {
        enabled: true,
        interval: INTERVAL,
    });
    group.harness.settle();
    group.sink.reset(two_screens());
    group.probe.asked.store(0, Ordering::SeqCst);
    group
}

/// Move the clock and wait until the engine has run a whole iteration at the
/// new reading, keeping every event.
fn advance(group: &Covering, by: Duration) {
    group.advance(&group.clock, by);
    for _ in 0..2 {
        let _ = group.engine.memory_report();
    }
}

fn published(group: &Covering) -> usize {
    group.sink.publications().len()
}

#[test]
fn a_cover_holds_the_auto_refresh_until_it_is_clear_and_then_publishes_once() {
    let gpu = gpu();
    let group = covering(&gpu);
    group.probe.cover();

    advance(group, INTERVAL);
    assert_eq!(group.probe.asked(), 1, "the refresh that fell due asked");
    assert_eq!(published(group), 0, "a covered desktop holds the refresh");

    advance(group, COVER_RECHECK.saturating_sub(Duration::from_secs(1)));
    assert_eq!(group.probe.asked(), 1, "asked again before the recheck");

    advance(group, Duration::from_secs(1));
    assert_eq!(group.probe.asked(), 2, "asked again at the recheck");
    assert_eq!(published(group), 0);

    group.probe.answer(Coverage::Clear);
    advance(group, COVER_RECHECK);
    assert!(group.wait_for_publish().is_ok());
    assert_eq!(group.probe.asked(), 3);
    std::thread::sleep(NOTHING_HAPPENS_IN);
    assert_eq!(published(group), 1, "one held refresh is one publish");
}

#[test]
fn after_a_held_publish_the_next_refresh_is_a_whole_interval_later() {
    let gpu = gpu();
    let group = covering(&gpu);
    group.probe.cover();
    advance(group, INTERVAL);
    group.probe.answer(Coverage::Clear);
    advance(group, COVER_RECHECK);
    assert!(group.wait_for_publish().is_ok());

    // The countdown the refresh started before the hold would fall due here.
    advance(group, INTERVAL.saturating_sub(Duration::from_secs(1)));
    assert_eq!(published(group), 1, "a refresh followed the held one early");
    advance(group, Duration::from_secs(1));
    assert!(group.wait_for_publish().is_ok());
    assert_eq!(published(group), 2);
}

#[test]
fn a_probe_that_cannot_tell_publishes_as_usual() {
    let gpu = gpu();
    let group = covering(&gpu);
    group.probe.answer(Coverage::Unknown("no route".to_owned()));
    advance(group, INTERVAL);
    assert!(group.wait_for_publish().is_ok());
    assert_eq!(group.probe.asked(), 1);
    assert_eq!(published(group), 1);
}

#[test]
fn a_publish_asked_for_while_held_goes_out_and_ends_the_hold() {
    let gpu = gpu();
    let group = covering(&gpu);
    group.probe.cover();
    advance(group, INTERVAL);
    assert_eq!(published(group), 0);

    assert!(group.publish().is_ok());
    assert_eq!(published(group), 1, "what the user asks for is not held");
    let asked = group.probe.asked();
    advance(group, COVER_RECHECK);
    advance(group, COVER_RECHECK);
    assert_eq!(
        group.probe.asked(),
        asked,
        "a hold that ended is not rechecked"
    );
    assert_eq!(published(group), 1);
}

#[test]
fn a_layout_change_republish_is_held_the_same_way() {
    let gpu = gpu();
    let group = covering(&gpu);
    group.probe.cover();
    let docked = vec![screen("docked", 0, 1600, 900, true)];
    group.sink.set_monitors(docked.clone());
    group.hint();
    advance(group, DISPLAY_SETTLE + Duration::from_secs(1));
    assert_eq!(group.next_layout(CHANGE_TIMEOUT), Some(docked));
    assert_eq!(group.probe.asked(), 1);
    std::thread::sleep(NOTHING_HAPPENS_IN);
    assert_eq!(published(group), 0, "a covered desktop holds the republish");

    group.probe.answer(Coverage::Clear);
    advance(group, COVER_RECHECK);
    assert!(group.wait_for_publish().is_ok());
    assert_eq!(published(group), 1);
}

/// Quitting publishes only what is owed or asked for, and a refresh held by a
/// cover is neither.
#[test]
fn quitting_while_held_publishes_nothing() {
    let _gpu = gpu();
    let Covering {
        harness,
        sink,
        clock,
        probe,
    } = start_covering();
    harness.settle();
    probe.cover();
    harness.advance(&clock, INTERVAL);
    for _ in 0..2 {
        let _ = harness.engine.memory_report();
    }
    assert_eq!(probe.asked(), 1);
    drop(harness);
    assert!(sink.publications().is_empty());
}

/// Quit with a refresh parked behind the scene hold and the probe answering
/// `coverage` at exit, and return how many publishes went out and how many
/// times the probe was asked.
fn quit_with_a_parked_refresh(coverage: Coverage) -> (usize, usize) {
    let _gpu = gpu();
    let Covering {
        harness,
        sink,
        clock,
        probe,
    } = start_covering();
    harness.settle();
    harness.engine.send(EngineCommand::Hold(true));
    harness.settle();
    harness.advance(&clock, INTERVAL);
    for _ in 0..2 {
        let _ = harness.engine.memory_report();
    }
    assert_eq!(probe.asked(), 0, "asked while the scene was held");
    probe.answer(coverage);
    drop(harness);
    (sink.publications().len(), probe.asked())
}

#[test]
fn a_refresh_parked_behind_the_scene_hold_goes_out_at_exit_when_clear() {
    assert_eq!(quit_with_a_parked_refresh(Coverage::Clear), (1, 1));
}

#[test]
fn a_refresh_parked_behind_the_scene_hold_stays_in_at_exit_when_covered() {
    let covered = Coverage::Covered(Cover {
        application: Some("player".to_owned()),
        pid: Some(4711),
        monitor: Some("DP-1".to_owned()),
        signal: Signal::FullScreenState,
    });
    assert_eq!(quit_with_a_parked_refresh(covered), (0, 1));
}

fn scene_hold(group: &Covering, hold: bool) {
    group.engine.send(EngineCommand::Hold(hold));
    group.settle();
}

/// A refresh that falls due while the scene is held is an automatic publish
/// still, and asks the probe at the release rather than going out as one the
/// user asked for.
#[test]
fn a_refresh_parked_behind_the_scene_hold_asks_the_probe_at_the_release() {
    let gpu = gpu();
    let group = covering(&gpu);
    scene_hold(group, true);
    advance(group, INTERVAL);
    assert_eq!(group.probe.asked(), 0, "asked while the scene was held");

    group.probe.cover();
    scene_hold(group, false);
    advance(group, Duration::from_secs(1));
    assert_eq!(group.probe.asked(), 1, "the release did not ask");
    std::thread::sleep(NOTHING_HAPPENS_IN);
    assert_eq!(published(group), 0, "a covered desktop holds the refresh");

    group.probe.answer(Coverage::Clear);
    advance(group, COVER_RECHECK);
    assert!(group.wait_for_publish().is_ok());
    std::thread::sleep(NOTHING_HAPPENS_IN);
    assert_eq!(published(group), 1, "one held refresh is one publish");
}

/// A refresh held for a cover whose recheck fell due while the scene was
/// held, and a refresh parked behind that hold, are one publish at the release.
#[test]
fn a_cover_released_with_a_refresh_parked_behind_the_scene_hold_publishes_once() {
    let gpu = gpu();
    let group = covering(&gpu);
    group.probe.cover();
    advance(group, INTERVAL);
    assert_eq!(group.probe.asked(), 1);

    scene_hold(group, true);
    group.probe.answer(Coverage::Clear);
    advance(group, INTERVAL);
    assert_eq!(group.probe.asked(), 1, "rechecked while the scene was held");

    group.engine.send(EngineCommand::Hold(false));
    assert!(group.wait_for_publish().is_ok());
    advance(group, Duration::from_secs(1));
    std::thread::sleep(NOTHING_HAPPENS_IN);
    assert_eq!(published(group), 1, "the release and the parked refresh");
}

/// A publish asked for while the scene was held, whose release lands on the
/// tick the cover's recheck does, is the same wallpaper and goes out once.
#[test]
fn a_cover_released_with_a_publish_asked_behind_the_scene_hold_publishes_once() {
    let gpu = gpu();
    let group = covering(&gpu);
    group.probe.cover();
    advance(group, INTERVAL);
    assert_eq!(group.probe.asked(), 1);

    scene_hold(group, true);
    group.probe.answer(Coverage::Clear);
    advance(group, COVER_RECHECK);
    group.engine.send(EngineCommand::RenderWallpaperNow);
    let _ = group.engine.memory_report();
    assert_eq!(published(group), 0, "a publish reached past the scene hold");

    group.engine.send(EngineCommand::Hold(false));
    assert!(group.wait_for_publish().is_ok());
    advance(group, Duration::from_secs(1));
    std::thread::sleep(NOTHING_HAPPENS_IN);
    assert_eq!(published(group), 1, "the release and the asked publish");
}

/// Switching the auto-refresh off drops a refresh held for a cover: nothing
/// goes out when the cover lifts, and the probe is not asked again.
#[test]
fn a_refresh_held_for_a_cover_is_dropped_when_the_auto_refresh_goes_off() {
    let gpu = gpu();
    let group = covering(&gpu);
    group.probe.cover();
    advance(group, INTERVAL);
    assert_eq!(group.probe.asked(), 1);

    group.engine.send(EngineCommand::SetAutoRefresh {
        enabled: false,
        interval: INTERVAL,
    });
    group.settle();
    group.probe.answer(Coverage::Clear);
    advance(group, COVER_RECHECK);
    advance(group, COVER_RECHECK);
    std::thread::sleep(NOTHING_HAPPENS_IN);
    assert_eq!(group.probe.asked(), 1, "a dropped hold was rechecked");
    assert_eq!(published(group), 0, "a dropped refresh went out");
}

/// A layout change republish held with the refresh stays held when the
/// auto-refresh goes off, and goes out once the cover lifts.
#[test]
fn a_layout_change_held_with_the_refresh_survives_the_auto_refresh_going_off() {
    let gpu = gpu();
    let group = covering(&gpu);
    group.probe.cover();
    advance(group, INTERVAL);
    let docked = vec![screen("docked", 0, 1600, 900, true)];
    group.sink.set_monitors(docked.clone());
    group.hint();
    advance(group, DISPLAY_SETTLE + Duration::from_secs(1));
    assert_eq!(group.next_layout(CHANGE_TIMEOUT), Some(docked));

    group.engine.send(EngineCommand::SetAutoRefresh {
        enabled: false,
        interval: INTERVAL,
    });
    group.settle();
    std::thread::sleep(NOTHING_HAPPENS_IN);
    assert_eq!(published(group), 0);

    group.probe.answer(Coverage::Clear);
    advance(group, COVER_RECHECK);
    assert!(group.wait_for_publish().is_ok());
    std::thread::sleep(NOTHING_HAPPENS_IN);
    assert_eq!(published(group), 1, "the held republish went out once");
}
