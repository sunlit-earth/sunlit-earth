use std::time::Duration;

use sunlit_core::display::layout::DisplayMode;
use sunlit_core::engine::EngineCommand;
use sunlit_core::params::SceneParams;

use crate::groups::plain;
use crate::harness::{Harness, gpu, has_lit_pixels, test_params};
use crate::sinks::{publish_plan, screen};
use crate::test_support::ScratchDir;

#[test]
fn unchanged_parameters_do_not_produce_another_frame() {
    let gpu = gpu();
    let harness = plain(&gpu);
    let params = test_params();
    harness.settle_at(&params);

    // Resending the same parameters marks the engine dirty, but the renderer's
    // own dirty check must still recognize that nothing actually changed.
    harness
        .engine
        .send(EngineCommand::UpdateParams(Box::new(params)));
    assert!(
        harness.drained_frame(Duration::from_millis(500)).is_none(),
        "an identical scene must not be re-rendered"
    );
}

#[test]
fn changed_parameters_produce_a_new_frame() {
    let gpu = gpu();
    let harness = plain(&gpu);
    harness.settle_at(&test_params());

    let moved = SceneParams {
        camera: sunlit_core::scene::camera::CameraParams {
            longitude: test_params().camera.longitude + 45.0,
            ..test_params().camera
        },
        ..test_params()
    };
    let (rgba, width, height) = harness.frame_after_change(&moved);
    assert_eq!(rgba.len(), (width as usize) * (height as usize) * 4);
    assert!(has_lit_pixels(&rgba));
}

#[test]
fn preview_size_changes_are_quantized_and_applied() {
    let gpu = gpu();
    let harness = plain(&gpu);

    for (asked, quantized) in [((300, 200), (256, 192)), ((512, 288), (512, 256))] {
        harness
            .engine
            .send(EngineCommand::SetPreviewSize(asked.0, asked.1));
        let (rgba, width, height) = harness.next_frame();
        assert_eq!((width, height), quantized, "{asked:?}");
        assert_eq!(rgba.len(), (width as usize) * (height as usize) * 4);
    }
}

/// Three states of one debt: the preview owes a frame when it is switched on,
/// owes nothing while it is off, and pays the debt out of the texture that is
/// already there rather than out of a re-render.
///
/// An engine of its own, because the first state is a client that hides the
/// window before anything has been drawn, which is what `--tray-start hidden`
/// does, and no shared engine has never drawn anything.
#[test]
fn switching_the_preview_on_owes_a_frame_and_switching_it_off_stops_them() {
    let _gpu = gpu();
    let harness = Harness::start(|config| config.preview_enabled = false);

    // Nothing has been drawn yet, so the debt has to survive a tick that has
    // nothing to pay it with.
    harness.engine.send(EngineCommand::SetPreviewEnabled(true));
    let (first, width, height) = harness.next_frame();
    assert_eq!(first.len(), (width as usize) * (height as usize) * 4);
    assert!(has_lit_pixels(&first));

    harness.engine.send(EngineCommand::SetPreviewEnabled(false));
    let moved = SceneParams {
        cloud_opacity: 0.1,
        ..test_params()
    };
    harness
        .engine
        .send(EngineCommand::UpdateParams(Box::new(moved)));
    assert!(
        harness.drained_frame(Duration::from_millis(500)).is_none(),
        "no frames should be delivered while the preview is off"
    );

    // Back to the scene the first frame was drawn from, so the dirty check
    // would suppress a re-render and the only thing that can produce a frame
    // is the debt paying itself out of the existing texture.
    harness
        .engine
        .send(EngineCommand::UpdateParams(Box::new(test_params())));
    harness.settle();
    harness.engine.send(EngineCommand::SetPreviewEnabled(true));
    let (again, _, _) = harness.next_frame();
    assert_eq!(
        again, first,
        "re-enabling the preview sent something other than the frame that was already there"
    );
}

#[test]
fn render_to_file_writes_a_png_at_the_requested_size() {
    let gpu = gpu();
    let harness = plain(&gpu);
    let dir = ScratchDir::new("engine_render_to_file");
    harness
        .engine
        .send(EngineCommand::UpdateParams(Box::new(test_params())));

    let path = dir.join("out.png");
    harness
        .engine
        .render_to_file(path.clone(), 320, 192)
        .expect("render_to_file should succeed");
    let decoded = image::open(&path).expect("output should be a readable PNG");
    assert_eq!((decoded.width(), decoded.height()), (320, 192));
    assert!(
        has_lit_pixels(decoded.to_rgba8().as_raw()),
        "the exported image should contain the globe"
    );

    // A hidden window is the case the render subcommand runs in, and the export
    // has no business depending on whether anybody is watching.
    harness.engine.send(EngineCommand::SetPreviewEnabled(false));
    let headless = dir.join("headless.png");
    harness
        .engine
        .render_to_file(headless.clone(), 256, 144)
        .expect("a headless engine must still be able to export");
    let decoded = image::open(&headless).expect("output should be a readable PNG");
    assert_eq!((decoded.width(), decoded.height()), (256, 144));
    assert!(has_lit_pixels(decoded.to_rgba8().as_raw()));
}

#[test]
fn wallpaper_now_publishes_one_frame_at_the_sink_size() {
    let gpu = gpu();
    let group = plain(&gpu);
    let published = publish_plan(
        group,
        vec![screen("only", 0, 320, 192, true)],
        DisplayMode::default(),
        None,
    );
    assert_eq!(published.images, vec![Some((320, 192))]);
}
