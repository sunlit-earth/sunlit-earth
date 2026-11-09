//! The hold the export sheet takes: the preview and every export stay at the
//! instant the preview showed when it began, wallpaper publishes wait for the
//! release, and readiness is announced afresh.

use std::sync::Arc;
use std::time::{Duration, Instant};

use sunlit_core::engine::clock::MockClock;
use sunlit_core::engine::excursion::{ExcursionCommand, ExcursionKind};
use sunlit_core::engine::{EngineCommand, EngineEvent};
use sunlit_core::scene::scene_time::SceneTime;

use crate::groups::{Plain, plain};
use crate::harness::{Harness, TIMEOUT, gpu, test_params};
use crate::sinks::{RecordingSink, two_screens};
use crate::test_support::{self, ScratchDir};
use crate::tiles::{day_over, settled, start};

const EXPORT: (u32, u32) = (256, 128);

/// The instant the preview is drawn at now, read from a frame drawn for
/// `params` after a frame of a slightly different zoom, so the engine is left
/// on `params` and there is a frame to read.
fn redrawn_instant(
    group: &Plain,
    params: &sunlit_core::params::SceneParams,
) -> time::OffsetDateTime {
    group.settle();
    let mut nudged = *params;
    nudged.camera.zoom += 0.05;
    next_instant(group, &nudged);
    next_instant(group, params)
}

fn next_instant(group: &Plain, params: &sunlit_core::params::SceneParams) -> time::OffsetDateTime {
    group
        .engine
        .send(EngineCommand::UpdateParams(Box::new(*params)));
    let deadline = Instant::now() + TIMEOUT;
    while let Ok(event) = group.events.recv_deadline(deadline) {
        if let EngineEvent::PreviewFrame { instant, .. } = event {
            return instant;
        }
    }
    panic!("no preview frame within {TIMEOUT:?}");
}

fn hold(group: &Harness, hold: bool) {
    group.engine.send(EngineCommand::Hold(hold));
    group.settle();
}

/// With a live setting, the clock moving under a hold moves neither the
/// preview nor an export, and after the release the setting follows the
/// clock again.
#[test]
fn a_hold_keeps_a_live_scene_where_it_was() {
    let gpu = gpu();
    let group = plain(&gpu);
    let mut params = test_params();
    params.time = SceneTime::Live;
    group.settle_at(&params);

    hold(group, true);
    let held = redrawn_instant(group, &params);
    let at_hold = group.export(EXPORT.0, EXPORT.1);

    group.advance(&group.clock, Duration::from_secs(6 * 3600));
    assert_eq!(redrawn_instant(group, &params), held, "the preview moved");
    assert!(
        group.export(EXPORT.0, EXPORT.1) == at_hold,
        "an export moved with the clock"
    );

    hold(group, false);
    let released = redrawn_instant(group, &params);
    assert_eq!(released, held + time::Duration::hours(6));
    assert!(
        group.export(EXPORT.0, EXPORT.1) != at_hold,
        "an export after the release still draws the held instant"
    );
}

/// A running excursion is paused by a hold and stays paused after it, and an
/// export under the hold draws the excursion's instant, not the setting's.
#[test]
fn a_hold_pauses_an_excursion_and_exports_its_instant() {
    let gpu = gpu();
    let group = plain(&gpu);
    let params = test_params();
    group.settle_at(&params);
    redrawn_instant(group, &params);
    let at_setting = group.export(EXPORT.0, EXPORT.1);

    group
        .engine
        .send(EngineCommand::Excursion(ExcursionCommand::Play {
            rate: 21_600,
            kind: ExcursionKind::Clock,
        }));
    group.settle();
    group.advance(&group.clock, Duration::from_secs(1));
    group.settle();

    group.engine.send(EngineCommand::Hold(true));
    let mut paused = None;
    let deadline = Instant::now() + TIMEOUT;
    while let Ok(event) = group.events.recv_deadline(deadline) {
        if let EngineEvent::Excursion(state) = event {
            paused = state;
            break;
        }
    }
    assert_eq!(paused.map(|state| state.running), Some(false), "not paused");
    let held = redrawn_instant(group, &params);
    assert_ne!(held, params.time.instant(time::OffsetDateTime::UNIX_EPOCH));
    let under_hold = group.export(EXPORT.0, EXPORT.1);
    assert!(
        under_hold != at_setting,
        "an export under the hold drew the setting"
    );

    group.advance(&group.clock, Duration::from_secs(1));
    assert_eq!(redrawn_instant(group, &params), held);
    assert!(group.export(EXPORT.0, EXPORT.1) == under_hold);

    hold(group, false);
    group.advance(&group.clock, Duration::from_secs(1));
    assert_eq!(
        redrawn_instant(group, &params),
        held,
        "the excursion ran again after the release"
    );

    group
        .engine
        .send(EngineCommand::Excursion(ExcursionCommand::End));
    group.settle();
}

/// A refresh that comes due during a hold is not made then, and is made once
/// at the release.
#[test]
fn a_refresh_due_during_a_hold_runs_once_at_the_release() {
    let gpu = gpu();
    let group = plain(&gpu);
    group.settle_at(&test_params());
    let before = group.sink.publications().len();

    group.engine.send(EngineCommand::SetAutoRefresh {
        enabled: true,
        interval: Duration::from_secs(60),
    });
    hold(group, true);
    group.advance(&group.clock, Duration::from_secs(61));
    group.settle();
    group.advance(&group.clock, Duration::from_secs(1));
    group.settle();
    assert_eq!(
        group.sink.publications().len(),
        before,
        "a wallpaper went out during the hold"
    );

    group.engine.send(EngineCommand::SetAutoRefresh {
        enabled: false,
        interval: Duration::from_secs(60),
    });
    group.engine.send(EngineCommand::Hold(false));
    group
        .wait_for_publish()
        .expect("the recording sink accepts");
    group.settle();
    assert_eq!(group.sink.publications().len(), before + 1);
}

/// Taking the hold with everything resident announces `TexturesReady` again.
#[test]
fn a_hold_announces_readiness_at_once_when_everything_is_resident() {
    let gpu = gpu();
    let group = plain(&gpu);
    group.settle_at(&test_params());
    group.engine.send(EngineCommand::Hold(true));
    let deadline = Instant::now() + TIMEOUT;
    let mut ready = false;
    while let Ok(event) = group.events.recv_deadline(deadline) {
        if matches!(event, EngineEvent::TexturesReady) {
            ready = true;
            break;
        }
    }
    hold(group, false);
    assert!(ready, "no TexturesReady after the hold");
}

/// Taking the hold while the view's tiles are on their way announces
/// `TexturesReady` once they have landed.
#[test]
fn a_hold_announces_readiness_when_the_tiles_land() {
    let _gpu = gpu();
    let dir = ScratchDir::new("engine_hold_tiles");
    test_support::write_earth_fixture(&dir.join("textures"));
    let harness = start(&dir, |config| {
        config.params = day_over(25.0, 0.0, 0.1);
        config.clock = std::sync::Arc::new(sunlit_core::engine::clock::MockClock::new(
            time::OffsetDateTime::UNIX_EPOCH,
        ));
    });
    harness.wait_for_textures("the first view");
    settled(&harness, "the first view's tiles", |_| true);
    harness.settle();

    harness
        .engine
        .send(EngineCommand::UpdateParams(Box::new(day_over(
            -120.0, 30.0, 0.6,
        ))));
    harness.engine.send(EngineCommand::Hold(true));
    harness.wait_for_textures("the held view");
    let report = crate::tiles::tile_report(&harness);
    assert!(
        report
            .wanted
            .iter()
            .all(|id| report.resident.contains(id) || report.failed.contains(id)),
        "TexturesReady before the tiles landed: {report:#?}"
    );
}

/// A wallpaper still owed when the engine stops under a hold is drawn at the
/// clock's instant, not the held one.
#[test]
fn a_publish_owed_at_exit_under_a_hold_draws_the_clock() {
    let _gpu = gpu();
    let sink = Arc::new(RecordingSink::new(two_screens()));
    let clock = Arc::new(MockClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let (sink_for_config, clock_for_config) = (Arc::clone(&sink), Arc::clone(&clock));
    let harness = Harness::start(move |config| {
        config.wallpaper = sink_for_config;
        config.clock = clock_for_config;
        config.params.time = SceneTime::Live;
    });
    harness.wait_for_textures("the first view");
    harness.settle();
    harness.engine.send(EngineCommand::RenderWallpaperNow);
    harness
        .wait_for_publish()
        .expect("the recording sink accepts");
    harness.settle();
    let at_hold = sink.publications()[0].frames.clone();

    hold(&harness, true);
    harness.advance(&clock, Duration::from_secs(6 * 3600));
    harness.settle();
    harness.engine.send(EngineCommand::RenderWallpaperNow);
    harness.settle();
    assert_eq!(
        sink.publications().len(),
        1,
        "the premise: the publish waits for the release"
    );

    harness.engine.shutdown();
    let publications = sink.publications();
    assert_eq!(publications.len(), 2, "the owed wallpaper did not go out");
    assert!(
        publications[1].frames != at_hold,
        "the wallpaper owed at exit was drawn at the held instant"
    );
}
