//! How the preview is framed: for the anchor screen, on the short edge the
//! frame says it was framed on.

use std::sync::Arc;
use std::time::Duration;

use sunlit_core::display::layout::DisplayMode;
use sunlit_core::engine::clock::MockClock;
use sunlit_core::engine::{DISPLAY_SETTLE, EngineCommand};
use sunlit_core::params::SceneParams;

use crate::display_change::CHANGE_TIMEOUT;
use crate::harness::{Framed, Harness, gpu, test_params};
use crate::sinks::{RecordingSink, screen};

/// An engine with its own preview size, the monitors it starts knowing, and
/// a sink and a clock to move them with.
fn framing_rig(
    preview: (u32, u32),
    monitors: Vec<sunlit_core::display::Monitor>,
) -> (Harness, Arc<RecordingSink>, Arc<MockClock>) {
    let sink = Arc::new(RecordingSink::new(monitors.clone()));
    let clock = Arc::new(MockClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let (sink_for_config, clock_for_config) = (Arc::clone(&sink), Arc::clone(&clock));
    let harness = Harness::start(move |config| {
        config.preview_size = preview;
        config.monitors = monitors;
        config.wallpaper = sink_for_config;
        config.clock = clock_for_config;
    });
    (harness, sink, clock)
}

/// The next frame drawn after `command`, with whatever was queued before it
/// thrown away.
fn frame_after(harness: &Harness, command: EngineCommand) -> Framed {
    harness.settle();
    harness.engine.send(command);
    harness.next_framed()
}

/// The frame says which screen it was framed for and on what short edge, and
/// follows the anchor as the display plan and the layout move it: the
/// startup enumeration, a stored anchor, and a layout that changed under it.
#[test]
fn the_preview_frame_says_which_screen_it_was_framed_for() {
    let _gpu = gpu();
    let (harness, sink, clock) = framing_rig(
        (512, 256),
        vec![
            screen("wide", 0, 1920, 1080, true),
            screen("tall", 1920, 1080, 1920, false),
        ],
    );
    let mut moved = test_params();
    moved.camera.longitude += 5.0;
    let first = frame_after(&harness, EngineCommand::UpdateParams(Box::new(moved)));
    assert_eq!((first.width, first.height), (512, 256));
    assert_eq!(first.screen, Some((1920, 1080)));
    approx::assert_relative_eq!(first.short_edge, 256.0);

    // A portrait anchor: its rectangle is as tall as the frame, so the short
    // edge is its width there.
    let tall = frame_after(
        &harness,
        EngineCommand::SetDisplayPlan {
            mode: DisplayMode::OneScreen,
            anchor: Some("tall".to_owned()),
        },
    );
    assert_eq!(tall.screen, Some((1080, 1920)));
    approx::assert_relative_eq!(tall.short_edge, 256.0 * 1080.0 / 1920.0);

    // The anchor is unplugged: the plan falls back to the new primary, and
    // the preview follows it once the layout change is announced.
    harness.settle();
    sink.set_monitors(vec![screen("ultrawide", 0, 3440, 1440, true)]);
    harness.hint();
    harness.advance(&clock, DISPLAY_SETTLE + Duration::from_secs(1));
    assert!(harness.next_layout(CHANGE_TIMEOUT).is_some());
    let ultrawide = harness.next_framed();
    assert_eq!(ultrawide.screen, Some((3440, 1440)));
    approx::assert_relative_eq!(ultrawide.short_edge, 256.0);
}

#[test]
fn a_preview_with_no_screen_known_frames_on_its_own_height() {
    let _gpu = gpu();
    let (harness, _, _) = framing_rig((320, 480), Vec::new());
    let mut moved = test_params();
    moved.camera.longitude += 5.0;
    let frame = frame_after(&harness, EngineCommand::UpdateParams(Box::new(moved)));
    assert_eq!(frame.screen, None);
    approx::assert_relative_eq!(frame.short_edge, 480.0);
}

/// Where the globe's center is from the frame's center and how large it is,
/// both in units of `short_edge`, from the pixels the grid paints.
#[allow(clippy::cast_precision_loss)]
fn globe_in_short_edges(rgba: &[u8], width: u32, height: u32, short_edge: f32) -> [f32; 3] {
    let (mut count, mut sum_x, mut sum_y) = (0.0_f64, 0.0_f64, 0.0_f64);
    for (index, pixel) in rgba.chunks_exact(4).enumerate() {
        // The clear color's green is far below anything the grid paints.
        if pixel[1] >= 60 {
            count += 1.0;
            sum_x += (index % width as usize) as f64 + 0.5;
            sum_y += (index / width as usize) as f64 + 0.5;
        }
    }
    assert!(
        count > 200.0,
        "only {count} pixels of globe in a {width}x{height} frame"
    );
    let short_edge = f64::from(short_edge);
    #[allow(clippy::cast_possible_truncation)]
    let measured = [
        ((sum_x / count - f64::from(width) * 0.5) / short_edge) as f32,
        ((sum_y / count - f64::from(height) * 0.5) / short_edge) as f32,
        ((count / std::f64::consts::PI).sqrt() / short_edge) as f32,
    ];
    measured
}

/// A small globe panned off center, with nothing else lit, so that the
/// pixels the grid paints are the globe and all of it is in every frame.
fn small_globe() -> SceneParams {
    let mut params = test_params();
    params.camera.zoom = 0.75;
    params.camera.latitude = 0.0;
    params.camera.offset_x = 0.25;
    params.camera.offset_y = -0.1;
    params.atmo_enabled = false;
    params.star_intensity = 0.0;
    params.sun_glow = 0.0;
    params
}

/// Success criterion 3: the globe in the preview frame, measured in the frame's
/// short edge, is the globe of an export at 1600x900, a 16:9 screen's own
/// picture, in a panel wider than the screen, one narrower than it, and with
/// no screen known at all.
#[test]
fn the_preview_puts_the_globe_where_an_export_of_the_screen_does() {
    let _gpu = gpu();
    let params = small_globe();
    let mut cases = Vec::new();
    for (panel, monitors) in [
        ((768, 240), vec![screen("wide", 0, 1600, 900, true)]),
        ((300, 300), vec![screen("wide", 0, 1600, 900, true)]),
        ((480, 270), Vec::new()),
    ] {
        let (harness, _, _) = framing_rig(panel, monitors);
        harness.settle_at(&params);
        let export = harness.export(1600, 900);
        let expected = globe_in_short_edges(&export, 1600, 900, 900.0);
        let mut moved = params;
        moved.camera.longitude += 1.0;
        let frame = frame_after(&harness, EngineCommand::UpdateParams(Box::new(moved)));
        let drawn = globe_in_short_edges(&frame.rgba, frame.width, frame.height, frame.short_edge);
        cases.push((panel, expected, drawn));
    }
    for (panel, expected, drawn) in cases {
        println!("{panel:?}: export {expected:?}, preview {drawn:?}");
        for (axis, (expected, drawn)) in expected.iter().zip(&drawn).enumerate() {
            assert!(
                (expected - drawn).abs() < 0.002,
                "a {panel:?} preview puts the globe's {} at {drawn:.4} short edges, \
                 and the 1600x900 export at {expected:.4}",
                ["center x", "center y", "radius"][axis]
            );
        }
    }
}
