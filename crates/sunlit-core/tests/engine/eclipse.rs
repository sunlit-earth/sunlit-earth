//! The Moon's shadow in the engine: eclipse mode on the engine's clock, and
//! what drawing the shadow costs a preview frame.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sunlit_core::assets::cube_layout::CubeTextures;
use sunlit_core::engine::EngineCommand;
use sunlit_core::engine::clock::MockClock;
use sunlit_core::engine::excursion::{ExcursionCommand, ExcursionKind};
use sunlit_core::params::SceneParams;
use sunlit_core::scene::camera::CameraParams;
use sunlit_core::scene::eclipse;
use sunlit_core::scene::scene_time::SceneTime;
use sunlit_core::scene::sky;

use crate::harness::{Harness, gpu};
use crate::readiness::build_every_pack;
use crate::support;
use crate::test_support::{self, ScratchDir};
use crate::tiles::{day_over, settled, start, tile_report};

fn command(harness: &Harness, command: ExcursionCommand) {
    harness.engine.send(EngineCommand::Excursion(command));
    harness.settle();
}

/// In eclipse mode a Clock run carries the camera along with the shadow: the
/// motion observer reads the camera the frames resolve, as it reads a
/// Sun-fixed camera's turn, at the rate the followed point moves, and where
/// the run paused the tiles wanted and the frame are those of an Earth-fixed
/// camera over the point the mode followed there.
/// At the setting, two moves of the time 30 ms apart, which is what the day
/// track does in the mode, are no drag: the held camera does not move.
#[test]
fn an_eclipse_mode_run_follows_the_shadow_and_a_time_write_is_no_drag() {
    let _gpu = gpu();
    let found =
        eclipse::nearest(time::macros::datetime!(2024-04-08 12:00 UTC)).expect("an eclipse");
    let begin = found.peak - time::Duration::minutes(90);
    let following = SceneParams {
        eclipse_follow: true,
        time: SceneTime::fixed(begin),
        ..day_over(0.0, 0.0, 0.2)
    };
    let dir = ScratchDir::new("engine_eclipse_follow");
    test_support::write_earth_fixture(&dir.join("textures"));
    build_every_pack(&dir);
    let clock = Arc::new(MockClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let for_config = Arc::clone(&clock);
    let harness = start(&dir, |config| {
        config.params = following;
        config.clock = for_config;
    });
    harness.wait_for_textures("the floors and the preview's tiles");
    let at_rest = settled(&harness, "the tiles at the setting", |r| {
        !r.wanted.is_empty()
    });
    let followed_at = |at: time::OffsetDateTime| eclipse::followed_point(&sky::shadow_at(at));

    command(
        &harness,
        ExcursionCommand::Play {
            rate: 3600,
            kind: ExcursionKind::Clock,
        },
    );
    harness.advance(&clock, Duration::from_millis(50));
    harness.settle();
    harness.advance(&clock, Duration::from_millis(50));
    harness.settle();
    let drag = tile_report(&harness).drag.expect("the following is motion");
    let (from, to) = (
        followed_at(begin + time::Duration::minutes(3)),
        followed_at(begin + time::Duration::minutes(6)),
    );
    assert!(
        (to.0 - from.0).abs() > 0.01 && (to.1 - from.1).abs() > 0.01,
        "the premise: the shadow moves, {from:?} to {to:?}"
    );
    approx::assert_relative_eq!(
        drag.latitude_rate,
        (to.0 - from.0) / 0.05,
        max_relative = 1e-3
    );
    approx::assert_relative_eq!(
        drag.longitude_rate,
        (to.1 - from.1) / 0.05,
        max_relative = 1e-3
    );

    harness.advance(&clock, Duration::from_millis(900));
    harness.settle();
    command(&harness, ExcursionCommand::Pause);
    harness.advance(&clock, Duration::from_millis(200));
    harness.settle();
    let paused = settled(&harness, "the tiles where the run paused", |r| {
        r.drag.is_none() && r.wanted != at_rest.wanted
    });

    let (latitude, longitude) = followed_at(begin + time::Duration::hours(1));
    let earth = SceneParams {
        eclipse_follow: false,
        camera: CameraParams {
            longitude,
            latitude,
            ..following.camera
        },
        ..following
    };
    harness.settle_at(&earth);
    let over = settled(&harness, "the Earth-fixed camera's tiles", |r| {
        r.drag.is_none()
    });
    assert_eq!(
        over.wanted, paused.wanted,
        "the tiles wanted are not the ones beneath the camera the run followed"
    );
    assert_the_frame_is_the_fixed_cameras(&harness, &following, &earth);

    command(&harness, ExcursionCommand::End);
    harness.settle_at(&following);
    harness.advance(&clock, Duration::from_millis(200));
    harness.settle_at(&following);
    for minutes in [1, 2] {
        harness.advance(&clock, Duration::from_millis(30));
        harness.settle_at(&SceneParams {
            time: SceneTime::fixed(begin + time::Duration::minutes(minutes)),
            ..following
        });
    }
    assert!(
        tile_report(&harness).drag.is_none(),
        "two moves of the time in eclipse mode were taken for a drag"
    );
}

/// The frame in eclipse mode is the frame of the Earth-fixed camera `earth`
/// at the same instant, much nearer it than the frame of a camera 0.05
/// degrees off.
fn assert_the_frame_is_the_fixed_cameras(
    harness: &Harness,
    following: &SceneParams,
    earth: &SceneParams,
) {
    let frame_of = |params: &SceneParams| {
        harness.settle();
        harness.frame_after_change(params).0
    };
    let mode = frame_of(following);
    let fixed = frame_of(earth);
    let off = frame_of(&SceneParams {
        camera: CameraParams {
            latitude: earth.camera.latitude + 0.05,
            ..earth.camera
        },
        ..*earth
    });
    // The followed point moves up to 0.04 degrees a second of the eclipse, so
    // a camera resolved at another second than the pause's comes near the
    // frame 0.05 degrees off.
    let (to_fixed, _) = crate::tiles::difference(&mode, &fixed, 24);
    let (to_off, _) = crate::tiles::difference(&mode, &off, 24);
    assert!(
        to_off > 0.0 && to_fixed * 10.0 <= to_off,
        "the paused frame is not the one over the followed point: {to_fixed}, {to_off} 0.05 degrees off"
    );
}

/// The preview sizes the Sun-fixed plan measured its frames at.
const PREVIEW_SIZES: [(u32, u32); 3] = [(704, 576), (1088, 896), (1920, 1088)];

/// A real overlay texture where the checkout has it, else the fixture.
fn overlay(name: &str, fixture: impl FnOnce() -> std::path::PathBuf) -> std::path::PathBuf {
    let real = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../textures")
        .join(name);
    let real_size = std::fs::metadata(&real).map_or(0, |meta| meta.len());
    if real_size > 100_000 { real } else { fixture() }
}

/// The time from an `UpdateParams` to the preview frame it produces, the frame
/// itself and its size. The engine is let finish whatever it was doing first,
/// so the frame is the change's own and not one already on its way.
fn frame_time(harness: &Harness, params: &SceneParams) -> (f64, Vec<u8>, (u32, u32)) {
    harness.settle();
    let sent = Instant::now();
    let (rgba, width, height) = harness.frame_after_change(params);
    (sent.elapsed().as_secs_f64() * 1e3, rgba, (width, height))
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

/// Pairs of frames at one instant of the eclipse, one with the shadow switched
/// off, which closes the gate, and one with it on, the instant moved by half a
/// minute each pair, 25 pairs after 5: the median of each side, the median of
/// the pairwise differences, and the size the frames came at.
fn alternated(
    harness: &Harness,
    params: &SceneParams,
    peak: time::OffsetDateTime,
) -> (f64, f64, f64, (u32, u32)) {
    let mut size = (0, 0);
    let pairs: Vec<(f64, f64)> = (0..30_i64)
        .map(|step| {
            let at = SceneTime::fixed(peak + time::Duration::seconds(30 * step));
            let (outside, plain, _) = frame_time(
                harness,
                &SceneParams {
                    time: at,
                    moon_shadow: 0.0,
                    ..*params
                },
            );
            let (inside, shadowed, drawn) = frame_time(
                harness,
                &SceneParams {
                    time: at,
                    moon_shadow: 1.0,
                    ..*params
                },
            );
            assert!(
                plain != shadowed,
                "the frame inside the eclipse drew no shadow"
            );
            size = drawn;
            (outside, inside)
        })
        .skip(5)
        .collect();
    (
        median(pairs.iter().map(|pair| pair.0).collect()),
        median(pairs.iter().map(|pair| pair.1).collect()),
        median(pairs.iter().map(|pair| pair.1 - pair.0).collect()),
        size,
    )
}

/// What drawing the Moon's shadow costs a preview frame: a frame inside the
/// 2024-04-08 eclipse against the same frame with the gate closed, on the software
/// adapter and on whatever adapter the machine prefers, at the preview sizes
/// the Sun-fixed plan measured. The engine's own time from the command to the
/// frame, so the sky state, the wanted set, the draw and the readback, in
/// blend mode over the Earth fixture's cube and its tiles with the cloud
/// fixture, the Moon and the Milky Way, at the High tier with the default
/// MSAA, with the camera
/// above the peak point at the default zoom.
///
/// A measurement rather than a check: run it by hand in a release build with
/// `cargo test --release -p sunlit-core --test engine -- --ignored --nocapture
/// measure_what_the_shadow_costs`.
#[test]
#[ignore = "a measurement, run by hand"]
fn measure_what_the_shadow_costs_a_preview_frame() {
    let _gpu = gpu();
    let eclipse =
        sunlit_core::scene::eclipse::nearest(time::macros::datetime!(2024-04-08 12:00 UTC))
            .expect("an eclipse");
    let params = SceneParams {
        texture_index: 3,
        camera: CameraParams {
            latitude: eclipse.greatest.latitude,
            longitude: eclipse.greatest.longitude,
            ..SceneParams::default().camera
        },
        ..SceneParams::default()
    };
    let dir = ScratchDir::new("engine_eclipse_cost");
    test_support::write_earth_fixture(&dir.join("textures"));
    let moon = overlay("lroc_color_poles_1k.jxl", || {
        support::write_moon_fixture(dir.path())
    });
    let panorama = overlay("milkyway_2020_4k.jxl", || {
        support::write_panorama_bands_fixture(dir.path())
    });
    println!("| Preview | Adapter | Outside | Inside | Inside less outside |");
    for software in [true, false] {
        let harness = Harness::start(|config| {
            config.force_software = software;
            config.quality = sunlit_core::config::QualityTier::High;
            config.preview_size = PREVIEW_SIZES[0];
            config.cube_textures = CubeTextures::resolve(&dir.join("textures"));
            config.tile_geometry = crate::tiles::EARTH;
            config.cache_dir = Some(dir.join("cache"));
            config.texture_paths = vec![Some(moon.clone()), Some(panorama.clone())];
            config.cloud = Some(std::sync::Arc::new(support::FixtureClouds::bands()));
            config.params = params;
        });
        harness.wait_for_textures("the Earth fixture's cube");
        harness.wait_for_slot_texture("moon_texture");
        harness.wait_for_slot_texture("milky_way_texture");
        harness.wait_for_slot_texture("cloud_texture");
        let adapter = harness.engine.adapter_info().to_owned();
        for (width, height) in PREVIEW_SIZES {
            harness
                .engine
                .send(EngineCommand::SetPreviewSize(width, height));
            harness.settle();
            let (outside, inside, difference, drawn) = alternated(&harness, &params, eclipse.peak);
            println!(
                "| {width} by {height}, drawn at {} by {} | {adapter} | {outside:.1} ms | \
                 {inside:.1} ms | {difference:+.1} ms |",
                drawn.0, drawn.1,
            );
        }
    }
}
