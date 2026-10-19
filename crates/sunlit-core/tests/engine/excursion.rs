//! Playback on the engine's clock: the preview leaves the time setting for an
//! excursion that runs at a rate or rests paused, the wallpaper keeps the
//! setting, and the excursion ends when the setting changes.

use std::time::Duration;

use sunlit_core::engine::excursion::{ExcursionCommand, ExcursionKind, ExcursionState};
use sunlit_core::engine::{EngineCommand, EngineEvent};
use sunlit_core::scene::scene_time::SceneTime;
use sunlit_core::scene::sky;

use crate::groups::{Plain, plain};
use crate::harness::{TIMEOUT, gpu, test_params};

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
