//! What each display mode publishes, and to which screen.

use std::sync::Arc;
use std::time::Duration;

use sunlit_core::config::QualityTier;
use sunlit_core::display::layout::DisplayMode;
use sunlit_core::engine::EngineCommand;
use sunlit_core::engine::wallpaper_sink::Frame;
use sunlit_core::params::SceneParams;

use crate::groups::plain;
use crate::harness::{Harness, gpu, has_lit_pixels, test_params};
use crate::sinks::{
    Publication, RecordingSink, publish_plan, publish_plan_with, screen, two_screens,
};

/// The single-monitor identity: what every existing config describes, and what
/// the multi-monitor path must leave unchanged.
///
/// Byte for byte rather than by size, because a size is the one thing a derived
/// framing cannot move. The publish is held against `ExportPixels`, which is
/// `prepare_export` and then `export_image` with the scene's own parameters.
#[test]
fn one_monitor_is_one_image_at_its_own_size_in_every_mode() {
    let gpu = gpu();
    let group = plain(&gpu);
    for mode in DisplayMode::ALL {
        let published = publish_plan(group, vec![screen("only", 0, 320, 192, true)], mode, None);
        assert_eq!(published.mode, mode);
        assert_eq!(published.anchor, 0);
        assert_eq!(published.images, vec![Some((320, 192))], "{mode:?}");
        assert_eq!(published.renders, 1, "{mode:?}");

        let before = group.export(320, 192);
        assert_eq!(
            picture(&published, 0).pixels,
            before,
            "{mode:?} moved a landscape screen's wallpaper"
        );
    }
}

/// The one thing a single monitor does *not* come out of this unchanged, and it
/// is on purpose.
///
/// A portrait screen is what the contain rule exists for, and containing is
/// exactly what byte-identity forbids. The rule wins, so the exception is pinned
/// here rather than left latent: the publish is not the pre-feature render, and
/// it is precisely the render at the contained lens.
#[test]
fn a_portrait_screen_takes_the_contained_lens_instead_of_the_old_one() {
    let gpu = gpu();
    let group = plain(&gpu);
    let published = publish_plan(
        group,
        vec![screen("tall", 0, 192, 320, true)],
        DisplayMode::EveryScreen,
        None,
    );
    assert_eq!(published.images, vec![Some((192, 320))]);

    let before = group.export(192, 320);
    assert_ne!(
        picture(&published, 0).pixels,
        before,
        "a portrait screen still renders what it did before the contain rule"
    );

    // And what it renders instead is the setting run through the rule, not
    // something that merely differs from it.
    let mut contained = test_params();
    contained.camera.fov_deg =
        sunlit_core::display::layout::contain_camera_fov(contained.camera.fov_deg, 192, 320);
    assert!(contained.camera.fov_deg > test_params().camera.fov_deg);
    let widened = group.picture(&contained, (192, 320));
    assert_eq!(
        picture(&published, 0).pixels,
        widened,
        "the portrait screen's wallpaper is not the contain rule's own framing"
    );
}

#[test]
fn every_screen_renders_one_image_per_distinct_size() {
    let gpu = gpu();
    let group = plain(&gpu);
    let published = publish_plan(group, two_screens(), DisplayMode::EveryScreen, None);
    assert_eq!(published.images, vec![Some((320, 192)), Some((320, 192))]);
    assert_eq!(
        published.renders, 1,
        "two screens of one size are one render shared by both"
    );
    assert!(published.canvas.is_none());

    // Different sizes are one render each, at each screen's own size.
    let published = publish_plan(
        group,
        vec![
            screen("A", 0, 320, 192, true),
            screen("B", 320, 256, 128, false),
        ],
        DisplayMode::EveryScreen,
        None,
    );
    assert_eq!(published.images, vec![Some((320, 192)), Some((256, 128))]);
    assert_eq!(published.renders, 2);
}

#[test]
fn one_screen_paints_the_anchor_and_leaves_the_others_alone() {
    let gpu = gpu();
    let group = plain(&gpu);
    let published = publish_plan(group, two_screens(), DisplayMode::OneScreen, None);
    assert_eq!(published.images, vec![Some((320, 192)), None]);
    assert_eq!(published.renders, 1);

    // And the anchor is the stored one where the session still has it.
    let published = publish_plan(group, two_screens(), DisplayMode::OneScreen, Some("B"));
    assert_eq!(published.anchor, 1);
    assert_eq!(published.images, vec![None, Some((320, 192))]);
}

#[test]
fn across_screens_renders_one_canvas_and_cuts_it() {
    let gpu = gpu();
    let published = publish_plan(plain(&gpu), two_screens(), DisplayMode::AcrossScreens, None);
    assert_eq!(
        published.canvas,
        Some((640, 192)),
        "the canvas is the bounding box of the layout"
    );
    assert_eq!(published.renders, 1, "one render for the whole desktop");
    assert_eq!(
        published.images,
        vec![Some((320, 192)), Some((320, 192))],
        "each screen's own piece, at its own size"
    );
}

/// The picture one screen of a publish was given.
pub(crate) fn picture(published: &Publication, index: usize) -> &Frame {
    published.frames[index]
        .as_deref()
        .unwrap_or_else(|| panic!("screen {index} was left alone by this publish"))
}

/// Mean absolute per-channel difference in 0-255 units, and the fraction of
/// pixels differing by more than 24.
///
/// The golden suite's comparator and the golden suite's numbers: two renders of
/// the same scene through the same shaders on the same adapter, which is exactly
/// what that tolerance was measured for.
pub(crate) fn compare(a: &Frame, b: &Frame) -> (f64, f64) {
    assert_eq!(
        (a.width, a.height),
        (b.width, b.height),
        "two frames of different sizes are not the same framing to begin with"
    );
    let mut total = 0u64;
    let mut outliers = 0usize;
    for (x, y) in a.pixels.iter().zip(&b.pixels) {
        let diff = u64::from(x.abs_diff(*y));
        total += diff;
        if diff > 24 {
            outliers += 1;
        }
    }
    #[allow(clippy::cast_precision_loss)]
    (
        total as f64 / a.pixels.len() as f64,
        outliers as f64 / a.pixels.len() as f64,
    )
}

/// Where the globe sits in a frame and how large it is, in pixels.
///
/// Measured from the pixels it lights up, so the scene it is measured in has to
/// be one where nothing else does: no stars, no Milky Way, no glare, no
/// atmosphere. The radius is the one a disc of that many pixels would have.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn globe(frame: &Frame) -> (f64, f64, f64) {
    let (mut sum_x, mut sum_y, mut count) = (0.0, 0.0, 0.0);
    for y in 0..frame.height {
        for x in 0..frame.width {
            let at = ((y * frame.width + x) * 4) as usize;
            let luminance = u32::from(frame.pixels[at])
                + u32::from(frame.pixels[at + 1])
                + u32::from(frame.pixels[at + 2]);
            if luminance > 24 {
                sum_x += f64::from(x);
                sum_y += f64::from(y);
                count += 1.0;
            }
        }
    }
    assert!(count > 100.0, "no globe in this frame to measure");
    (
        sum_x / count,
        sum_y / count,
        (count / std::f64::consts::PI).sqrt(),
    )
}

/// The span identity, all the way through the shaders: the anchor's crop out of
/// a canvas is the picture that screen would have got alone.
///
/// Two equal 16:9 screens side by side, which is the layout this mode is for and
/// the one the old 180 degree sky clamp could not hold: the canvas derives 218
/// degrees, and under the clamp the sky came out at a different scale on both
/// screens while the globe continued exactly. Nothing is contrived here, and
/// nothing about the scene is excluded: the sky, the stars, the Milky Way and
/// the Sun are all in the frame and all have to land in the same place.
#[test]
fn the_anchors_crop_of_a_span_is_the_picture_it_would_have_had_alone() {
    let monitors = vec![
        screen("A", 0, 640, 360, true),
        screen("B", 640, 640, 360, false),
    ];
    let gpu = gpu();
    let group = plain(&gpu);
    let spanned = publish_plan(group, monitors.clone(), DisplayMode::AcrossScreens, None);
    assert_eq!(spanned.canvas, Some((1280, 360)));
    let alone = publish_plan(group, monitors, DisplayMode::EveryScreen, None);

    let (mean, outliers) = compare(picture(&spanned, 0), picture(&alone, 0));
    assert!(
        mean < 2.0 && outliers < 0.01,
        "the anchor's crop and its standalone render are {mean:.2} apart on average, with \
         {:.2}% of pixels past the outlier threshold",
        outliers * 100.0
    );

    // And the other screen is the view continuing outward rather than a second
    // copy of it, which is the whole difference between this mode and the one
    // above it. Without this the case would still pass on a publish that put
    // the anchor's picture on every screen.
    let (mean, _) = compare(picture(&spanned, 1), picture(&alone, 1));
    assert!(
        mean > 2.0,
        "the second screen's crop is the picture it would have got alone ({mean:.2} apart), so \
         the canvas is not continuing the view across the seam"
    );
}

/// A canvas taller than the anchor still puts the globe where the anchor had it.
///
/// The weaker half of the identity, and the honest one: `sphere.wgsl` sizes star
/// sprites against the viewport, so a taller canvas does not draw the same stars
/// the anchor alone would have. The globe follows the tan-space scaling and does,
/// which is what this measures: the same disc, the same size, in the same place.
#[test]
fn a_taller_canvas_still_puts_the_globe_where_the_anchor_had_it() {
    let monitors = vec![
        screen("A", 0, 640, 360, true),
        screen("B", 640, 640, 480, false),
    ];
    // Nothing in the frame but the globe, so that what is being measured is the
    // globe rather than whatever else happens to be bright.
    let params = SceneParams {
        atmo_enabled: false,
        star_intensity: 0.0,
        milky_way_intensity: 0.0,
        sun_glow: 0.0,
        moon_brightness: 0.0,
        ..test_params()
    };
    let gpu = gpu();
    let group = plain(&gpu);
    let spanned = publish_plan_with(
        group,
        monitors.clone(),
        DisplayMode::AcrossScreens,
        None,
        params,
    );
    assert_eq!(spanned.canvas, Some((1280, 480)));
    let alone = publish_plan_with(group, monitors, DisplayMode::EveryScreen, None, params);

    let (cx, cy, radius) = globe(picture(&spanned, 0));
    let (alone_x, alone_y, alone_radius) = globe(picture(&alone, 0));
    assert!(
        (cx - alone_x).abs() < 1.5 && (cy - alone_y).abs() < 1.5,
        "the globe is at ({cx:.1}, {cy:.1}) in the crop and ({alone_x:.1}, {alone_y:.1}) alone"
    );
    assert!(
        (radius - alone_radius).abs() < 1.5,
        "the globe's radius is {radius:.1} pixels in the crop and {alone_radius:.1} alone"
    );
}

/// A stored anchor that is no longer connected must not silently draw somewhere
/// else: the plan falls back to the primary and the publish says it did.
#[test]
fn a_stored_anchor_that_is_gone_falls_back_and_reports_it() {
    let _gpu = gpu();
    let sink = Arc::new(RecordingSink::new(two_screens()));
    // The stored anchor is a configuration here rather than a command, which is
    // how it reaches a real session: the app reads it out of the config file and
    // hands it to `engine::start`. This is the one case that still does that, so
    // it is what would notice `display_mode` or `anchor_monitor` going unread at
    // startup.
    let sink_for_config = Arc::clone(&sink);
    let harness = Harness::start(move |config| {
        config.wallpaper = sink_for_config;
        config.display_mode = DisplayMode::OneScreen;
        config.anchor_monitor = Some("a-screen-that-went-away".to_owned());
    });

    let note = harness.publish().expect("falling back is not a failure");
    assert!(note.contains('A'), "{note}");
    let published = sink.publications();
    assert_eq!(published.len(), 1);
    assert_eq!(published[0].mode, DisplayMode::OneScreen);
    assert_eq!(published[0].anchor, 0);
}

/// The plan is re-read on every publish rather than cached at startup.
#[test]
fn a_new_display_plan_changes_the_next_publish() {
    let gpu = gpu();
    let group = plain(&gpu);
    group.sink.set_monitors(two_screens());
    group.engine.send(EngineCommand::SetDisplayPlan {
        mode: DisplayMode::EveryScreen,
        anchor: None,
    });

    group.publish().expect("a recording sink cannot fail");
    group.engine.send(EngineCommand::SetDisplayPlan {
        mode: DisplayMode::AcrossScreens,
        anchor: Some("B".to_owned()),
    });
    group.publish().expect("a recording sink cannot fail");

    let published = group.sink.publications();
    assert_eq!(published.len(), 2);
    assert_eq!(published[0].mode, DisplayMode::EveryScreen);
    assert!(published[0].canvas.is_none());
    assert_eq!(published[1].mode, DisplayMode::AcrossScreens);
    assert_eq!(published[1].canvas, Some((640, 192)));
    assert_eq!(published[1].anchor, 1);
}

/// A sink that refuses up front is never asked for its monitors.
///
/// What matters is not only that the export fails but that it fails before the
/// expensive part: the monitor list is the engine's first step toward a
/// native-resolution render and a readback of the whole image, so a query count
/// that did not move is the assertion that nothing was rendered.
#[test]
fn a_sink_that_cannot_publish_is_never_asked_to_render() {
    let gpu = gpu();
    let group = plain(&gpu);
    group.sink.refuse();
    let before = group.sink.queries();

    let reported = group.publish().expect_err("a refusing sink cannot succeed");
    assert_eq!(
        reported,
        RecordingSink::REFUSED,
        "the sink's own reason should reach the client unchanged"
    );
    assert_eq!(
        group.sink.queries(),
        before,
        "the engine asked a sink that had already refused for its monitors"
    );
    assert!(group.sink.publications().is_empty());
}

#[test]
fn switching_texture_mode_produces_a_new_frame() {
    let gpu = gpu();
    let harness = plain(&gpu);
    harness.settle_at(&test_params());

    // Slot 1 has no file behind it in this configuration, so the renderer
    // falls back to the grid. The frame still has to be re-rendered: the
    // selection is part of the dirty check, and a client that switched modes
    // is waiting for a picture either way.
    let swapped = SceneParams {
        texture_index: 1,
        ..test_params()
    };
    let (rgba, width, height) = harness.frame_after_change(&swapped);
    assert_eq!(rgba.len(), (width as usize) * (height as usize) * 4);
}

/// A sample count the adapter does not offer renders anyway, whether it is
/// there at startup or arrives later.
///
/// A saved config, or a combo box index built against a different adapter, can
/// ask for one, and a count that reached `create_render_textures` unresolved
/// would kill the engine thread with a wgpu validation error. The startup half
/// is why this has an engine of its own: the count has to be in the
/// configuration the renderer is built from.
#[test]
fn an_unsupported_sample_count_still_renders() {
    let _gpu = gpu();
    let harness = Harness::start(|config| {
        // The High tier deliberately does not cap the sample count, so the
        // adapter's own support list is the only thing between this config and
        // create_render_textures.
        config.quality = QualityTier::High;
        config.params = SceneParams {
            sample_count: 64,
            ..test_params()
        };
    });
    let (rgba, width, height) = harness.next_frame();
    assert_eq!(rgba.len(), (width as usize) * (height as usize) * 4);
    assert!(has_lit_pixels(&rgba));

    harness.drained_frame(Duration::from_millis(300));
    let (rgba, _, _) = harness.frame_after_change(&SceneParams {
        sample_count: 64,
        // Change something visible too, so the frame is not suppressed by the
        // dirty check once the count resolves back to what it was.
        cloud_opacity: 0.1,
        ..test_params()
    });
    assert!(has_lit_pixels(&rgba));
}
