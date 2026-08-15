//! Engine integration tests.
//!
//! These run the real engine against the real GPU pipeline, headlessly. They
//! assert behavioral invariants (a frame arrives, an unchanged scene does not
//! re-render, a changed one does) rather than pixel values, so they survive
//! adapter differences.
//!
//! Every test that starts an engine holds `GPU_SERIAL` for its whole lifetime.
//! The engine creates its own wgpu device, and creating several devices
//! concurrently crashes on Windows, so the lock keeps at most one engine alive
//! at a time.

use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use sunlit_core::engine::wallpaper_sink::CountingSink;
use sunlit_core::engine::{EngineCommand, EngineConfig, EngineEvent, EngineHandle};
use sunlit_core::params::SceneParams;

static GPU_SERIAL: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Hold the GPU lock even if a previous test panicked while holding it.
fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// How long to wait for the engine to produce something before giving up.
const TIMEOUT: Duration = Duration::from_secs(60);

/// Deterministic test parameters: the procedural grid texture, no MSAA, a
/// fixed date so the sun does not move between runs.
fn test_params() -> SceneParams {
    let mut params = SceneParams {
        texture_index: 0,
        sample_count: 1,
        ..SceneParams::default()
    };
    params.datetime.use_custom = true;
    params.datetime.custom_hour = 12.0;
    params.datetime.custom_day_of_year = 80;
    params.datetime.custom_year = 2026;
    params
}

struct Harness {
    engine: EngineHandle,
    events: Receiver<EngineEvent>,
    _guard: MutexGuard<'static, ()>,
}

impl Harness {
    fn start(configure: impl FnOnce(&mut EngineConfig)) -> Self {
        let guard = gpu_lock();
        let (tx, events): (Sender<EngineEvent>, Receiver<EngineEvent>) =
            crossbeam_channel::unbounded();
        let mut config = EngineConfig::headless((512, 288));
        config.params = test_params();
        config.wallpaper = Arc::new(CountingSink::new(320, 192));
        config.on_event = Arc::new(move |event| {
            let _ = tx.send(event);
        });
        configure(&mut config);
        Self {
            engine: sunlit_core::engine::start(config),
            events,
            _guard: guard,
        }
    }

    /// Block until the next preview frame, or panic on timeout.
    fn next_frame(&self) -> (Vec<u8>, u32, u32) {
        let deadline = std::time::Instant::now() + TIMEOUT;
        while let Ok(event) = self.events.recv_deadline(deadline) {
            if let EngineEvent::PreviewFrame { rgba, width, height } = event {
                return (rgba, width, height);
            }
        }
        panic!("no preview frame within {TIMEOUT:?}");
    }

    /// Drain events already queued and report whether any frame was among them.
    fn drained_frame(&self, settle: Duration) -> Option<(u32, u32)> {
        std::thread::sleep(settle);
        let mut found = None;
        while let Ok(event) = self.events.try_recv() {
            if let EngineEvent::PreviewFrame { width, height, .. } = event {
                found = Some((width, height));
            }
        }
        found
    }
}

/// A frame is "lit" when at least one pixel is clearly brighter than the
/// clear color (which is near-black).
fn has_lit_pixels(rgba: &[u8]) -> bool {
    rgba.chunks_exact(4).any(|px| px[0] > 40 || px[1] > 40 || px[2] > 40)
}

#[test]
fn engine_renders_a_first_preview_frame() {
    let harness = Harness::start(|_| {});
    let (rgba, width, height) = harness.next_frame();

    assert_eq!((width, height), (512, 256), "512x288 quantizes to 512x256");
    assert_eq!(rgba.len(), (width as usize) * (height as usize) * 4);
    assert!(has_lit_pixels(&rgba), "the globe should be visible on the first frame");
}

#[test]
fn unchanged_parameters_do_not_produce_another_frame() {
    let harness = Harness::start(|_| {});
    harness.next_frame();
    harness.drained_frame(Duration::from_millis(300));

    // Resending the same parameters marks the engine dirty, but the renderer's
    // own dirty check must still recognize that nothing actually changed.
    harness
        .engine
        .send(EngineCommand::UpdateParams(Box::new(test_params())));
    assert!(
        harness.drained_frame(Duration::from_millis(500)).is_none(),
        "an identical scene must not be re-rendered"
    );
}

#[test]
fn changed_parameters_produce_a_new_frame() {
    let harness = Harness::start(|_| {});
    harness.next_frame();
    harness.drained_frame(Duration::from_millis(300));

    let moved = SceneParams {
        camera: sunlit_core::scene::camera::CameraParams {
            longitude: test_params().camera.longitude + 45.0,
            ..test_params().camera
        },
        ..test_params()
    };
    harness.engine.send(EngineCommand::UpdateParams(Box::new(moved)));

    let (rgba, _, _) = harness.next_frame();
    assert!(has_lit_pixels(&rgba));
}

#[test]
fn preview_size_changes_are_quantized_and_applied() {
    let harness = Harness::start(|_| {});
    harness.next_frame();
    harness.drained_frame(Duration::from_millis(300));

    harness.engine.send(EngineCommand::SetPreviewSize(300, 200));
    let (rgba, width, height) = harness.next_frame();
    assert_eq!((width, height), (256, 192));
    assert_eq!(rgba.len(), (width as usize) * (height as usize) * 4);
}

#[test]
fn disabling_the_preview_stops_frames_without_stopping_the_engine() {
    let harness = Harness::start(|_| {});
    harness.next_frame();
    harness.drained_frame(Duration::from_millis(300));

    harness.engine.send(EngineCommand::SetPreviewEnabled(false));
    let moved = SceneParams {
        cloud_opacity: 0.1,
        ..test_params()
    };
    harness.engine.send(EngineCommand::UpdateParams(Box::new(moved)));
    assert!(
        harness.drained_frame(Duration::from_millis(500)).is_none(),
        "no frames should be delivered while the preview is off"
    );

    // The engine is still alive and resumes on demand.
    harness.engine.send(EngineCommand::SetPreviewEnabled(true));
    harness.next_frame();
}

#[test]
fn render_to_file_writes_a_png_at_the_requested_size() {
    let harness = Harness::start(|_| {});
    let dir = std::env::temp_dir().join("sunlit_earth_test_engine_render_to_file");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    let path = dir.join("out.png");

    harness
        .engine
        .render_to_file(path.clone(), 320, 192)
        .expect("render_to_file should succeed");

    let decoded = image::open(&path).expect("output should be a readable PNG");
    assert_eq!(decoded.width(), 320);
    assert_eq!(decoded.height(), 192);
    assert!(
        has_lit_pixels(decoded.to_rgba8().as_raw()),
        "the exported image should contain the globe"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn render_to_file_works_with_the_preview_disabled() {
    let harness = Harness::start(|config| config.preview_enabled = false);
    let dir = std::env::temp_dir().join("sunlit_earth_test_engine_headless_render");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    let path = dir.join("headless.png");

    harness
        .engine
        .render_to_file(path.clone(), 256, 144)
        .expect("a headless engine must still be able to export");

    let decoded = image::open(&path).expect("output should be a readable PNG");
    assert_eq!((decoded.width(), decoded.height()), (256, 144));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn wallpaper_now_publishes_one_frame_at_the_sink_size() {
    let sink = Arc::new(CountingSink::new(320, 192));
    let sink_for_config = Arc::clone(&sink);
    let harness = Harness::start(move |config| config.wallpaper = sink_for_config);
    harness.next_frame();

    harness.engine.send(EngineCommand::RenderWallpaperNow);

    let deadline = std::time::Instant::now() + TIMEOUT;
    let mut published = false;
    while let Ok(event) = harness.events.recv_deadline(deadline) {
        if let EngineEvent::WallpaperSet(result) = event {
            result.expect("publishing to a counting sink cannot fail");
            published = true;
            break;
        }
    }
    assert!(published, "no WallpaperSet event within {TIMEOUT:?}");
    assert_eq!(sink.count(), 1);
}

#[test]
fn switching_texture_mode_produces_a_new_frame() {
    let harness = Harness::start(|_| {});
    harness.next_frame();
    harness.drained_frame(Duration::from_millis(300));

    // Slot 1 has no file behind it in this configuration, so the renderer
    // falls back to the grid. The frame still has to be re-rendered: the
    // selection is part of the dirty check, and a client that switched modes
    // is waiting for a picture either way.
    let swapped = SceneParams {
        texture_index: 1,
        ..test_params()
    };
    harness.engine.send(EngineCommand::UpdateParams(Box::new(swapped)));
    let (rgba, width, height) = harness.next_frame();
    assert_eq!(rgba.len(), (width as usize) * (height as usize) * 4);
}

#[test]
fn textures_ready_fires_for_the_procedural_grid() {
    let harness = Harness::start(|_| {});
    let deadline = std::time::Instant::now() + TIMEOUT;
    let mut ready = false;
    while let Ok(event) = harness.events.recv_deadline(deadline) {
        if matches!(event, EngineEvent::TexturesReady) {
            ready = true;
            break;
        }
    }
    assert!(ready, "the grid texture is built up front and is always ready");
}
