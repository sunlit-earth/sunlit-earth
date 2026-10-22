//! The Sun-fixed camera on the engine's schedule: a shown preview is redrawn
//! as the Earth turns beneath the camera and a hidden one or an Earth-fixed
//! camera is not, and the tiles it wants follow the turn, which is never a
//! drag, while a hand on the time of day is one.

use std::sync::Arc;
use std::time::Duration;

use sunlit_core::engine::EngineCommand;
use sunlit_core::engine::clock::{Clock, MockClock};
use sunlit_core::params::SceneParams;
use sunlit_core::scene::camera::{CameraParams, longitude_of, zoom_to_distance};
use sunlit_core::scene::sky;

use crate::groups::plain;
use crate::harness::{gpu, test_params};
use crate::test_support::{self, ScratchDir};
use crate::tiles::{day_over, settled, start, tile_report};

/// How long a preview `height` pixels tall takes to move one pixel as the
/// Earth turns beneath `params`' camera, by the geometry: the point beneath
/// the camera moves one Earth radius a radian of the turn, at `distance - 1`
/// from the eye.
fn one_pixel(params: &SceneParams, height: u32) -> Duration {
    let distance = zoom_to_distance(params.camera.zoom);
    #[expect(clippy::cast_precision_loss, reason = "a preview's height")]
    let focal = height as f32 / (2.0 * (params.camera.fov_deg.to_radians() / 2.0).tan());
    let turn = std::f32::consts::TAU / 86_400.0;
    Duration::from_secs_f32((distance - 1.0) / (focal * turn))
}

/// `params` at the passing instant rather than the pinned one.
fn live(mut params: SceneParams) -> SceneParams {
    params.time = sunlit_core::scene::scene_time::SceneTime::Live;
    params
}

#[test]
fn a_shown_sun_fixed_preview_is_redrawn_once_a_pixel_of_the_turn() {
    let gpu = gpu();
    let group = plain(&gpu);
    let earth = live(SceneParams {
        camera: CameraParams {
            zoom: 0.0,
            ..test_params().camera
        },
        ..test_params()
    });
    group.settle_at(&earth);
    let sun_fixed = SceneParams {
        sun_fixed: true,
        solar_time: 12.0,
        ..earth
    };
    let (_, _, height) = group.frame_after_change(&sun_fixed);
    let pixel = one_pixel(&sun_fixed, height);
    assert!(
        pixel > Duration::from_secs(2) && pixel < Duration::from_mins(1),
        "the premise: a pixel takes {pixel:?}, well inside two minutes"
    );
    group.settle();
    group.advance(&group.clock, pixel.saturating_sub(Duration::from_secs(1)));
    assert!(
        group.drained_frame(Duration::from_millis(300)).is_none(),
        "redrawn before the turn had moved the preview a pixel"
    );
    group.advance(&group.clock, Duration::from_secs(2));
    let (turned, _, _) = group.next_frame();

    // Hidden, the schedule is the two minutes it always was, so the frame in
    // the texture when the preview is shown again is the one it was hidden on,
    // though a hundred seconds of the turn is ten pixels of this preview.
    group.engine.send(EngineCommand::SetPreviewEnabled(false));
    group.advance(&group.clock, Duration::from_secs(100));
    group.settle();
    group.engine.send(EngineCommand::SetPreviewEnabled(true));
    let (shown_again, _, _) = group.next_frame();
    assert!(
        shown_again == turned,
        "a hidden preview was redrawn on the Sun-fixed schedule"
    );

    // Earth-fixed, nothing turns beneath the camera, and the Sun and the
    // stars wait the two minutes they always did.
    let _ = group.frame_after_change(&earth);
    group.advance(&group.clock, Duration::from_secs(100));
    assert!(
        group.drained_frame(Duration::from_millis(300)).is_none(),
        "an Earth-fixed preview was redrawn before two minutes"
    );
    group.advance(&group.clock, Duration::from_secs(25));
    group.next_frame();

    // The group's own scene back, at its pinned instant, so the cases that
    // move its clock after this one move no sky of this one's.
    group.settle_at(&test_params());
}

#[test]
fn the_wanted_tiles_follow_the_turn_and_only_a_hand_on_the_time_is_a_drag() {
    let _gpu = gpu();
    let dir = ScratchDir::new("engine_sun_fixed_tiles");
    test_support::write_earth_fixture(&dir.join("textures"));
    let clock = Arc::new(MockClock::new(
        time::OffsetDateTime::UNIX_EPOCH + time::Duration::days(20_000),
    ));
    let sun_fixed = live(SceneParams {
        sun_fixed: true,
        solar_time: 12.0,
        ..day_over(0.0, 0.0, 0.55)
    });
    let for_config = Arc::clone(&clock);
    let harness = start(&dir, |config| {
        config.params = sun_fixed;
        config.clock = for_config;
    });
    harness.wait_for_textures("the floors and the preview's tiles");
    let noon = settled(&harness, "the tiles at the start", |r| !r.wanted.is_empty());

    // The schedule draws the turned globe and asks for what came into view,
    // with no hand on anything.
    harness.advance(&clock, Duration::from_hours(1));
    let later = settled(&harness, "the tiles an hour later", |r| {
        r.wanted != noon.wanted
    });
    assert!(
        later.drag.is_none(),
        "the Earth's turn was taken for a drag"
    );

    // What is wanted is what an Earth-fixed camera at the longitude the time
    // of day puts beneath the camera now wants, at the same instant.
    let sky = sky::compute_sky_state_at(&sun_fixed.time, clock.now_utc());
    let beneath = SceneParams {
        sun_fixed: false,
        camera: CameraParams {
            longitude: longitude_of(sun_fixed.solar_time, sky.sun_direction),
            ..sun_fixed.camera
        },
        ..sun_fixed
    };
    harness.settle_at(&beneath);
    let earth = settled(&harness, "the Earth-fixed camera's tiles", |_| true);
    assert_eq!(
        earth.wanted, later.wanted,
        "the tiles wanted are not the ones beneath the resolved camera"
    );

    // Back to the Sun-fixed camera, which a rest makes a jump rather than a
    // drag. Draws 50 ms apart from there move the resolved camera with the
    // turn and the held one not at all, and the held one is what a drag is
    // measured in. The sky is computed to the whole second, so the draws run
    // past one.
    harness.settle_at(&sun_fixed);
    harness.advance(&clock, Duration::from_millis(200));
    harness.settle_at(&sun_fixed);
    for _ in 0..24 {
        harness.advance(&clock, Duration::from_millis(50));
        harness.settle_at(&sun_fixed);
        assert!(
            tile_report(&harness).drag.is_none(),
            "the turn between draws 50 ms apart was taken for a drag"
        );
    }

    // Two moves of the time of day 30 ms apart are a hand on it.
    harness.advance(&clock, Duration::from_millis(50));
    harness.settle_at(&SceneParams {
        solar_time: 12.25,
        ..sun_fixed
    });
    harness.advance(&clock, Duration::from_millis(30));
    harness.settle_at(&SceneParams {
        solar_time: 12.5,
        ..sun_fixed
    });
    assert!(
        tile_report(&harness).drag.is_some(),
        "two moves of the time of day 30 ms apart are not a drag"
    );
}
