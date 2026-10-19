//! Playback on the engine's clock: the preview leaves the time setting for an
//! excursion that runs at a rate or rests paused, the wallpaper keeps the
//! setting, and the excursion ends when the setting changes. A run carries the
//! month in force, the motion observer and a Sun-fixed camera along with it.

use std::sync::Arc;
use std::time::Duration;

use sunlit_core::assets::tiles::PackKind;
use sunlit_core::engine::clock::MockClock;
use sunlit_core::engine::excursion::{ExcursionCommand, ExcursionKind, ExcursionState};
use sunlit_core::engine::{EngineCommand, EngineEvent};
use sunlit_core::params::SceneParams;
use sunlit_core::scene::camera::{CameraParams, longitude_of};
use sunlit_core::scene::scene_time::SceneTime;
use sunlit_core::scene::sky;

use crate::groups::{Plain, plain};
use crate::harness::{Harness, TIMEOUT, gpu, test_params};
use crate::months::assert_month;
use crate::readiness::build_every_pack;
use crate::test_support::{self, ScratchDir};
use crate::tiles::{day_over, settled, start, tile_report};

/// The next preview frame's instant, and every excursion state announced on
/// the way to it.
fn next_instant(group: &Plain) -> (time::OffsetDateTime, Vec<Option<ExcursionState>>) {
    let deadline = std::time::Instant::now() + TIMEOUT;
    let mut states = Vec::new();
    while let Ok(event) = group.events.recv_deadline(deadline) {
        match event {
            EngineEvent::PreviewFrame { instant, .. } => return (instant, states),
            EngineEvent::Excursion(state) => states.push(state),
            _ => {}
        }
    }
    panic!("no preview frame within {TIMEOUT:?}");
}

/// The excursion states announced until the engine has settled.
fn announced(group: &Plain) -> Vec<Option<ExcursionState>> {
    let _ = group.engine.memory_report();
    let _ = group.engine.memory_report();
    let mut states = Vec::new();
    while let Ok(event) = group.events.try_recv() {
        if let EngineEvent::Excursion(state) = event {
            states.push(state);
        }
    }
    states
}

fn send(group: &Plain, command: ExcursionCommand) {
    group.engine.send(EngineCommand::Excursion(command));
}

fn setting(params: &sunlit_core::params::SceneParams) -> time::OffsetDateTime {
    params.time.instant(time::OffsetDateTime::UNIX_EPOCH)
}

/// An hour a second for one second of the engine's clock is an hour of the
/// sky: the frame says so, and the Sun has turned 15 degrees about the axis.
#[test]
fn a_run_at_3600_times_moves_the_preview_an_hour_a_second() {
    let gpu = gpu();
    let group = plain(&gpu);
    let params = test_params();
    group.settle_at(&params);
    let start = setting(&params);

    send(
        group,
        ExcursionCommand::Play {
            rate: 3600,
            kind: ExcursionKind::Clock,
        },
    );
    let states = announced(group);
    assert_eq!(
        states.last().copied().flatten(),
        Some(ExcursionState {
            running: true,
            rate: 3600,
            kind: ExcursionKind::Clock,
        })
    );
    group.advance(&group.clock, Duration::from_secs(1));
    let (instant, _) = next_instant(group);
    assert_eq!(instant, start + time::Duration::hours(1));

    let before = sky::compute_sky_state_for(start).sun_direction;
    let after = sky::compute_sky_state_for(instant).sun_direction;
    let turned = (after.x.atan2(after.z) - before.x.atan2(before.z)).to_degrees();
    let turned = (turned + 540.0).rem_euclid(360.0) - 180.0;
    approx::assert_relative_eq!(turned.abs(), 15.0, epsilon = 0.1);

    send(group, ExcursionCommand::End);
    assert_eq!(announced(group).last().copied(), Some(None));
}

/// A wallpaper made during a run is the one the setting draws, pixel for
/// pixel.
#[test]
fn a_wallpaper_during_a_run_is_drawn_at_the_time_setting() {
    let gpu = gpu();
    let group = plain(&gpu);
    let params = test_params();
    group.settle_at(&params);
    let pixels = |group: &Plain| {
        group.publish().expect("the recording sink accepts");
        let publications = group.sink.publications();
        let last = publications.last().expect("one publication");
        last.frames
            .iter()
            .flatten()
            .map(|frame| frame.pixels.clone())
            .collect::<Vec<_>>()
    };
    let at_rest = pixels(group);

    send(
        group,
        ExcursionCommand::Play {
            rate: 21_600,
            kind: ExcursionKind::Clock,
        },
    );
    // Handled before the clock moves, so the run starts from the setting.
    group.settle();
    group.advance(&group.clock, Duration::from_secs(1));
    let (instant, _) = next_instant(group);
    assert_ne!(instant, setting(&params), "the premise: the preview moved");
    assert!(
        pixels(group) == at_rest,
        "the wallpaper moved with the preview"
    );
    send(group, ExcursionCommand::End);
    group.settle();
}

/// A change of the setting ends the excursion; a push that carries the same
/// setting, as every camera drag does, leaves it alone.
#[test]
fn only_a_changed_setting_ends_the_excursion() {
    let gpu = gpu();
    let group = plain(&gpu);
    let params = test_params();
    group.settle_at(&params);
    send(
        group,
        ExcursionCommand::Skip(sunlit_core::engine::excursion::TimeStep {
            kind: ExcursionKind::Clock,
            forward: true,
            big: true,
        }),
    );
    assert!(
        matches!(announced(group).last(), Some(Some(state)) if !state.running),
        "a skip starts a paused excursion"
    );

    let mut dragged = params;
    dragged.camera.longitude += 5.0;
    let (instant, states) = {
        group
            .engine
            .send(EngineCommand::UpdateParams(Box::new(dragged)));
        next_instant(group)
    };
    assert!(states.is_empty(), "a camera move ended the excursion");
    assert_eq!(instant, setting(&params) + time::Duration::hours(1));

    let mut moved = dragged;
    moved.time = SceneTime::fixed(setting(&params) + time::Duration::days(3));
    group
        .engine
        .send(EngineCommand::UpdateParams(Box::new(moved)));
    let (instant, states) = next_instant(group);
    assert_eq!(states, [None], "a new setting ends the excursion");
    assert_eq!(instant, setting(&moved));
}

/// Hiding the preview pauses a running excursion, and showing it again shows
/// the instant it paused at, however long the clock ran meanwhile.
#[test]
fn hiding_the_preview_pauses_the_excursion() {
    let gpu = gpu();
    let group = plain(&gpu);
    let params = test_params();
    group.settle_at(&params);
    send(
        group,
        ExcursionCommand::Play {
            rate: 60,
            kind: ExcursionKind::Clock,
        },
    );
    group.settle();
    group.advance(&group.clock, Duration::from_secs(2));
    let (shown, _) = next_instant(group);

    group.engine.send(EngineCommand::SetPreviewEnabled(false));
    let states = announced(group);
    assert!(
        matches!(states.last(), Some(Some(state)) if !state.running),
        "hiding paused it: {states:?}"
    );
    group.advance(&group.clock, Duration::from_secs(30));
    group.engine.send(EngineCommand::SetPreviewEnabled(true));
    let (again, _) = next_instant(group);
    assert!(
        again >= shown && again <= shown + time::Duration::minutes(1),
        "{again} is not where it paused, after {shown}"
    );
    send(group, ExcursionCommand::End);
    group.settle();
}

/// An engine over the Earth fixture with every pack built, drawing `params`
/// on a clock of its own.
fn over_the_fixture(name: &str, params: &SceneParams) -> (ScratchDir, Harness, Arc<MockClock>) {
    let dir = ScratchDir::new(name);
    test_support::write_earth_fixture(&dir.join("textures"));
    build_every_pack(&dir);
    let clock = Arc::new(MockClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let for_config = Arc::clone(&clock);
    let harness = start(&dir, |config| {
        config.params = *params;
        config.clock = for_config;
    });
    harness.wait_for_textures("the floors and the preview's tiles");
    (dir, harness, clock)
}

fn command(harness: &Harness, command: ExcursionCommand) {
    harness.engine.send(EngineCommand::Excursion(command));
    harness.settle();
}

/// A month a second for one second takes the preview from the 21st of March,
/// where April is in force, past the middle of April, where May is.
#[test]
fn a_date_run_across_the_middle_of_a_month_changes_the_month_in_force() {
    let _gpu = gpu();
    let view = day_over(20.0, 0.0, 0.55);
    let (_dir, harness, clock) = over_the_fixture("engine_excursion_month", &view);
    let april = settled(&harness, "April's tiles", |r| {
        r.named.iter().any(|id| id.pack == PackKind::Day(3))
    });
    assert_month(&april, 3, None, "at the setting");

    command(
        &harness,
        ExcursionCommand::Play {
            rate: 30,
            kind: ExcursionKind::Date,
        },
    );
    harness.advance(&clock, Duration::from_secs(1));
    let may = settled(&harness, "May's tiles", |r| {
        r.named.iter().any(|id| id.pack == PackKind::Day(4))
    });
    assert_month(&may, 4, None, "a month into the run");
}

/// The Sun moving through a run is motion, though the camera holds still, and
/// the motion settles once the run is paused and the preview has rested.
#[test]
fn a_running_sun_is_motion_until_the_run_stops() {
    let _gpu = gpu();
    let view = day_over(20.0, 0.0, 0.55);
    let (_dir, harness, clock) = over_the_fixture("engine_excursion_motion", &view);
    let still = settled(&harness, "the tiles at rest", |r| !r.wanted.is_empty());
    assert!(still.drag.is_none(), "{still:#?}");

    command(
        &harness,
        ExcursionCommand::Play {
            rate: 3600,
            kind: ExcursionKind::Clock,
        },
    );
    for _ in 0..6 {
        harness.advance(&clock, Duration::from_millis(50));
        harness.settle();
        let drag = tile_report(&harness).drag;
        assert!(
            drag.is_some_and(|drag| drag.longitude_rate == 0.0 && drag.latitude_rate == 0.0),
            "the Sun moving with the camera still is not motion of the camera: {drag:?}"
        );
    }

    command(&harness, ExcursionCommand::Pause);
    harness.advance(&clock, Duration::from_millis(200));
    harness.settle();
    settled(&harness, "the tiles after the run", |r| r.drag.is_none());
}

/// A Clock run turns a camera fixed toward the Sun with the Earth: the motion
/// observer measures the turn of the resolved camera at the run's rate and in
/// its direction, and the tiles wanted where the run paused are those an
/// Earth-fixed camera over the longitude it reached wants.
#[test]
fn a_sun_fixed_clock_run_turns_the_camera_and_the_tiles_follow() {
    let _gpu = gpu();
    let sun_fixed = SceneParams {
        sun_fixed: true,
        solar_time: 12.0,
        ..day_over(0.0, 0.0, 0.55)
    };
    let (_dir, harness, clock) = over_the_fixture("engine_excursion_sun_fixed", &sun_fixed);
    let at_rest = settled(&harness, "the tiles at the setting", |r| {
        !r.wanted.is_empty()
    });
    let start = setting(&sun_fixed);
    let beneath = |at: time::OffsetDateTime| {
        longitude_of(
            sun_fixed.solar_time,
            sky::compute_sky_state_for(at).sun_direction,
        )
    };

    command(
        &harness,
        ExcursionCommand::Play {
            rate: 3600,
            kind: ExcursionKind::Clock,
        },
    );
    // The first move is measured from the camera the settings hold; the
    // second, 50 ms of the clock and three minutes of the run later, from the
    // first.
    harness.advance(&clock, Duration::from_millis(50));
    harness.settle();
    harness.advance(&clock, Duration::from_millis(50));
    harness.settle();
    let drag = tile_report(&harness).drag.expect("the turn is motion");
    let turned =
        beneath(start + time::Duration::minutes(6)) - beneath(start + time::Duration::minutes(3));
    assert!(turned < 0.0, "the premise: the camera turns west, {turned}");
    approx::assert_relative_eq!(drag.longitude_rate, turned / 0.05, max_relative = 1e-3);

    harness.advance(&clock, Duration::from_millis(900));
    harness.settle();
    command(&harness, ExcursionCommand::Pause);
    harness.advance(&clock, Duration::from_millis(200));
    harness.settle();
    let paused = settled(&harness, "the tiles where the run paused", |r| {
        r.drag.is_none() && r.wanted != at_rest.wanted
    });

    let reached = start + time::Duration::hours(1);
    let earth = SceneParams {
        sun_fixed: false,
        camera: CameraParams {
            longitude: beneath(reached),
            ..sun_fixed.camera
        },
        ..sun_fixed
    };
    harness.settle_at(&earth);
    let over = settled(&harness, "the Earth-fixed camera's tiles", |r| {
        r.drag.is_none()
    });
    assert_eq!(
        over.wanted, paused.wanted,
        "the tiles wanted are not the ones beneath the camera the run turned"
    );
}
