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
use sunlit_core::assets::mailbox::{DecodedTextureMessage, TextureMailbox};
use sunlit_core::assets::texture_loader::DecodedImage;
use sunlit_core::config::QualityTier;
use sunlit_core::engine::wallpaper_sink::CountingSink;
use sunlit_core::engine::{EngineCommand, EngineConfig, EngineEvent, EngineHandle};
use sunlit_core::params::SceneParams;

static GPU_SERIAL: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Hold the GPU lock even if a previous test panicked while holding it.
fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// How long to wait for the engine to produce something before giving up.
const TIMEOUT: Duration = Duration::from_mins(1);

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
            if let EngineEvent::PreviewFrame {
                rgba,
                width,
                height,
            } = event
            {
                return (rgba, width, height);
            }
        }
        panic!("no preview frame within {TIMEOUT:?}");
    }

    /// Block until the textures the current mode needs are loaded, or panic on
    /// timeout. Preview frames arriving in the meantime are discarded.
    fn wait_for_textures(&self, what: &str) {
        let deadline = std::time::Instant::now() + TIMEOUT;
        while let Ok(event) = self.events.recv_deadline(deadline) {
            if matches!(event, EngineEvent::TexturesReady) {
                return;
            }
        }
        panic!("{what}: no TexturesReady within {TIMEOUT:?}");
    }

    /// Block until a status event whose text `matches`, or panic on timeout.
    ///
    /// The status is the loading indicator, so this is how a test observes that
    /// a background decode has started or finished without guessing at a sleep.
    fn wait_for_status(&self, matches: impl Fn(&str) -> bool, what: &str) {
        let deadline = std::time::Instant::now() + TIMEOUT;
        while let Ok(event) = self.events.recv_deadline(deadline) {
            if let EngineEvent::Status(text) = event
                && matches(&text)
            {
                return;
            }
        }
        panic!("{what}: no matching status within {TIMEOUT:?}");
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
    rgba.chunks_exact(4)
        .any(|px| px[0] > 40 || px[1] > 40 || px[2] > 40)
}

#[test]
fn zero_star_intensity_leaves_catalog_pixels_at_the_clear_color() {
    const CLEAR: [u8; 4] = [5, 5, 13, 255];

    let harness = Harness::start(|config| config.params.star_intensity = 0.0);
    let (stars_off, _, _) = harness.next_frame();
    let mut stars_on_params = test_params();
    stars_on_params.star_intensity = 1.5;
    harness
        .engine
        .send(EngineCommand::UpdateParams(Box::new(stars_on_params)));
    let (stars_on, _, _) = harness.next_frame();

    let revealed_star = stars_off
        .chunks_exact(4)
        .zip(stars_on.chunks_exact(4))
        .any(|(off, on)| off == CLEAR && on != CLEAR);
    assert!(
        revealed_star,
        "enabling stars should alter a clear background pixel"
    );
}

#[test]
fn larger_star_size_expands_crisp_cores_when_glow_is_disabled() {
    const CLEAR: [u8; 4] = [5, 5, 13, 255];

    let harness = Harness::start(|config| {
        config.params.star_intensity = 2.0;
        config.params.star_size = 0.5;
        config.params.star_glow_strength = 0.0;
        config.params.star_mag_limit = 4.0;
    });
    let (small_stars, _, _) = harness.next_frame();
    let mut large_params = test_params();
    large_params.star_intensity = 2.0;
    large_params.star_size = 3.0;
    large_params.star_glow_strength = 0.0;
    large_params.star_mag_limit = 4.0;
    harness
        .engine
        .send(EngineCommand::UpdateParams(Box::new(large_params)));
    let (large_stars, _, _) = harness.next_frame();

    let expanded_core = small_stars
        .chunks_exact(4)
        .zip(large_stars.chunks_exact(4))
        .any(|(small, large)| small == CLEAR && large != CLEAR);
    assert!(
        expanded_core,
        "increasing star size should expand a core without relying on glow"
    );
}

#[test]
fn wider_sky_fov_reveals_more_catalog_directions() {
    const CLEAR: [u8; 4] = [5, 5, 13, 255];

    let harness = Harness::start(|config| config.params.sky_fov = 60.0);
    let (narrow_sky, _, _) = harness.next_frame();
    let mut wide_params = test_params();
    wide_params.sky_fov = 140.0;
    harness
        .engine
        .send(EngineCommand::UpdateParams(Box::new(wide_params)));
    let (wide_sky, _, _) = harness.next_frame();

    let newly_visible_pixels = narrow_sky
        .chunks_exact(4)
        .zip(wide_sky.chunks_exact(4))
        .filter(|(narrow, wide)| *narrow == CLEAR && *wide != CLEAR)
        .count();
    assert!(
        newly_visible_pixels > 50,
        "wider sky FOV revealed only {newly_visible_pixels} background pixels"
    );
}

/// Two small texture files and a cache directory to go with them.
///
/// The resolution tests need file-backed slots, which the headless config
/// deliberately has none of. Small and bright rather than realistic: what is
/// being tested is the purge and the reload, and an 8K asset would make every
/// one of these tests a minute long.
struct TextureFixtures {
    dir: std::path::PathBuf,
}

impl TextureFixtures {
    /// The width both files are written at. A switch below this halves for real
    /// and exercises the on-disk cache; these are not one of the widths the
    /// combo box offers, because the renderer takes any width as a cap and the
    /// three on offer are the config's business.
    const WIDTH: u32 = 128;

    fn new(name: &str) -> Self {
        Self::with_width(name, Self::WIDTH)
    }

    /// Fixtures at a chosen width, for the one test that needs a decode slow
    /// enough to still be running a moment after it was spawned.
    fn with_width(name: &str, width: u32) -> Self {
        let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the fixture directory");
        for file in ["day.png", "night.png"] {
            let mut img = image::RgbaImage::new(width, width / 2);
            for (x, y, px) in img.enumerate_pixels_mut() {
                // Bright throughout, so a lit globe is lit whichever texture
                // and blend the mode picks.
                #[allow(clippy::cast_possible_truncation)]
                let v = 160 + ((x + y) % 96) as u8;
                *px = image::Rgba([v, v, v, 255]);
            }
            img.save(dir.join(file)).expect("write a fixture texture");
        }
        Self { dir }
    }

    /// Delete the files, so that a slot backed by one becomes terminal on its
    /// next load: the decode fails, and a failed decode clears the slot's path.
    fn remove_files(&self) {
        for file in ["day.png", "night.png"] {
            std::fs::remove_file(self.dir.join(file)).expect("remove a fixture texture");
        }
    }

    fn paths(&self) -> Vec<Option<std::path::PathBuf>> {
        vec![
            Some(self.dir.join("day.png")),
            Some(self.dir.join("night.png")),
        ]
    }
}

impl Drop for TextureFixtures {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn engine_renders_a_first_preview_frame() {
    let harness = Harness::start(|_| {});
    let (rgba, width, height) = harness.next_frame();

    assert_eq!((width, height), (512, 256), "512x288 quantizes to 512x256");
    assert_eq!(rgba.len(), (width as usize) * (height as usize) * 4);
    assert!(
        has_lit_pixels(&rgba),
        "the globe should be visible on the first frame"
    );
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
    harness
        .engine
        .send(EngineCommand::UpdateParams(Box::new(moved)));

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
    harness
        .engine
        .send(EngineCommand::UpdateParams(Box::new(moved)));
    assert!(
        harness.drained_frame(Duration::from_millis(500)).is_none(),
        "no frames should be delivered while the preview is off"
    );

    // The engine is still alive and resumes on demand.
    harness.engine.send(EngineCommand::SetPreviewEnabled(true));
    harness.next_frame();
}

#[test]
fn enabling_the_preview_before_any_frame_exists_still_delivers_one() {
    // The window can be hidden before the engine has drawn anything, which is
    // what `--tray-start hidden` does. Showing it later must produce a frame:
    // the owed-frame debt has to survive a tick where there is nothing to pay
    // it with yet.
    let harness = Harness::start(|config| config.preview_enabled = false);
    harness.engine.send(EngineCommand::SetPreviewEnabled(true));
    let (rgba, width, height) = harness.next_frame();
    assert_eq!(rgba.len(), (width as usize) * (height as usize) * 4);
    assert!(has_lit_pixels(&rgba));
}

#[test]
fn re_enabling_the_preview_resends_the_current_frame_unchanged() {
    // Hide, change nothing, show again. The dirty check would suppress a
    // re-render, so the frame has to come from the texture that is already
    // there or the window stays blank until the user touches a control.
    let harness = Harness::start(|_| {});
    harness.next_frame();
    harness.drained_frame(Duration::from_millis(300));

    harness.engine.send(EngineCommand::SetPreviewEnabled(false));
    assert!(
        harness.drained_frame(Duration::from_millis(300)).is_none(),
        "no frames while the preview is off"
    );

    harness.engine.send(EngineCommand::SetPreviewEnabled(true));
    let (rgba, _, _) = harness.next_frame();
    assert!(has_lit_pixels(&rgba));
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

/// A sink that refuses up front, and records anything asked of it afterwards.
///
/// This is the shape of `SystemWallpaper` off Windows, which cannot be
/// exercised directly on the machine this suite usually runs on. What matters
/// is not only that the export fails but that it fails before the expensive
/// part: `target_size` is the engine's first step towards a native-resolution
/// render and a readback of the whole image, so a count of zero there is the
/// assertion that nothing was rendered.
struct RefusingSink {
    size_queries: std::sync::atomic::AtomicUsize,
    publishes: std::sync::atomic::AtomicUsize,
}

impl RefusingSink {
    const REASON: &'static str = "this sink refuses on purpose";

    fn new() -> Self {
        Self {
            size_queries: std::sync::atomic::AtomicUsize::new(0),
            publishes: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn size_queries(&self) -> usize {
        self.size_queries.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn publishes(&self) -> usize {
        self.publishes.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl sunlit_core::engine::wallpaper_sink::WallpaperSink for RefusingSink {
    fn check_supported(&self) -> Result<(), String> {
        Err(Self::REASON.to_owned())
    }

    fn target_size(&self) -> Result<(u32, u32), String> {
        self.size_queries
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok((320, 192))
    }

    fn publish(&self, _pixels: &[u8], _width: u32, _height: u32) -> Result<(), String> {
        self.publishes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

#[test]
fn a_sink_that_cannot_publish_is_never_asked_to_render() {
    let sink = Arc::new(RefusingSink::new());
    let sink_for_config = Arc::clone(&sink);
    let harness = Harness::start(move |config| config.wallpaper = sink_for_config);
    harness.next_frame();

    harness.engine.send(EngineCommand::RenderWallpaperNow);

    let deadline = std::time::Instant::now() + TIMEOUT;
    let mut reported = None;
    while let Ok(event) = harness.events.recv_deadline(deadline) {
        if let EngineEvent::WallpaperSet(result) = event {
            reported = Some(result.expect_err("a refusing sink cannot succeed"));
            break;
        }
    }
    assert_eq!(
        reported.as_deref(),
        Some(RefusingSink::REASON),
        "the sink's own reason should reach the client unchanged"
    );
    assert_eq!(
        sink.size_queries(),
        0,
        "the engine asked for a render size from a sink that had already refused"
    );
    assert_eq!(sink.publishes(), 0);
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
    harness
        .engine
        .send(EngineCommand::UpdateParams(Box::new(swapped)));
    let (rgba, width, height) = harness.next_frame();
    assert_eq!(rgba.len(), (width as usize) * (height as usize) * 4);
}

#[test]
fn an_unsupported_sample_count_still_renders() {
    // A saved config, or a combo box index built against a different adapter,
    // can ask for a sample count this GPU does not offer. Before the engine
    // resolved it against the adapter, that reached create_render_textures and
    // killed the engine thread with a wgpu validation error: the window came
    // up, IPC answered, and no frame ever arrived.
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
}

#[test]
fn an_unsupported_sample_count_arriving_later_still_renders() {
    let harness = Harness::start(|config| config.quality = QualityTier::High);
    harness.next_frame();
    harness.drained_frame(Duration::from_millis(300));

    harness
        .engine
        .send(EngineCommand::UpdateParams(Box::new(SceneParams {
            sample_count: 64,
            // Change something visible too, so the frame is not suppressed by
            // the dirty check once the count resolves back to what it was.
            cloud_opacity: 0.1,
            ..test_params()
        })));
    let (rgba, _, _) = harness.next_frame();
    assert!(has_lit_pixels(&rgba));
}

/// Start an engine on the fixture textures in day/night blend mode, which is
/// the mode that needs both file-backed slots and the composite bind group
/// built from them.
fn blend_harness(fixtures: &TextureFixtures, width: u32) -> Harness {
    Harness::start(|config| {
        config.texture_paths = fixtures.paths();
        config.texture_resolution = width;
        config.cache_dir = Some(fixtures.dir.clone());
        config.params = SceneParams {
            texture_index: 3,
            ..test_params()
        };
    })
}

#[test]
fn a_resolution_switch_reloads_the_textures_in_both_directions() {
    let fixtures = TextureFixtures::new("engine_resolution_switch");
    let harness = blend_harness(&fixtures, TextureFixtures::WIDTH);
    harness.wait_for_textures("at startup");
    let (rgba, _, _) = harness.next_frame();
    assert!(
        has_lit_pixels(&rgba),
        "the globe should be visible at first"
    );

    // Down: the textures in memory are destroyed and the halved ones loaded.
    harness.engine.send(EngineCommand::SetTextureResolution(
        TextureFixtures::WIDTH / 2,
    ));
    harness.wait_for_textures("after switching down");
    let (rgba, _, _) = harness.next_frame();
    assert!(
        has_lit_pixels(&rgba),
        "a frame after the switch must come from the new textures, not from nothing"
    );

    // Up again: the same path in reverse, which is the one that would break if
    // the purge left a destroyed texture behind in a bind group.
    harness
        .engine
        .send(EngineCommand::SetTextureResolution(TextureFixtures::WIDTH));
    harness.wait_for_textures("after switching back up");
    let (rgba, _, _) = harness.next_frame();
    assert!(has_lit_pixels(&rgba));
}

/// The switch is idempotent, so a client that re-sends the current width (the
/// reset and load-defaults callbacks both do) costs nothing.
#[test]
fn a_switch_to_the_current_resolution_does_nothing() {
    let fixtures = TextureFixtures::new("engine_resolution_noop");
    let harness = blend_harness(&fixtures, TextureFixtures::WIDTH);
    harness.wait_for_textures("at startup");
    harness.next_frame();
    harness.drained_frame(Duration::from_millis(300));

    harness
        .engine
        .send(EngineCommand::SetTextureResolution(TextureFixtures::WIDTH));
    assert!(
        harness.drained_frame(Duration::from_millis(500)).is_none(),
        "a switch to the width already in force must not re-render"
    );
}

/// Three purges with no reload in between, ending on the last width.
///
/// The run loop drains every queued command before it ticks, and a reload is
/// only spawned from inside `render`, so all three purges here happen before
/// the first spawn and no decode is ever in flight during them. That makes this
/// a test of the purge being repeatable and of the last command winning, not of
/// the stale-arrival ordering; `a_stale_decode_arriving_after_a_switch_is_never_applied`
/// is that one.
#[test]
fn switches_in_quick_succession_end_on_the_last_one() {
    let fixtures = TextureFixtures::new("engine_resolution_races");
    let harness = blend_harness(&fixtures, TextureFixtures::WIDTH);
    harness.wait_for_textures("at startup");

    for width in [
        TextureFixtures::WIDTH / 2,
        TextureFixtures::WIDTH,
        TextureFixtures::WIDTH / 4,
    ] {
        harness
            .engine
            .send(EngineCommand::SetTextureResolution(width));
    }

    harness.wait_for_textures("after three switches in a row");
    let (rgba, _, _) = harness.next_frame();
    assert!(
        has_lit_pixels(&rgba),
        "the last switch must be the one that is showing"
    );
}

/// One decoded texture for a slot, as a background loader would post it.
fn decoded(slot_index: usize, width: u32, generation: u64, value: u8) -> DecodedTextureMessage {
    let height = width / 2;
    DecodedTextureMessage {
        slot_index,
        result: Ok(DecodedImage {
            pixels: vec![value; (width as usize) * (height as usize) * 4],
            width,
            height,
        }),
        generation: Some(generation),
    }
}

/// A stale decode must not destroy the fresh one that replaced it.
///
/// This is the ordering the mailbox guard exists for and the only one that can
/// lose a texture permanently: the reload's own post is already parked when the
/// superseded decode finishes, and the consumer discards a stale post on sight,
/// so overwriting the parked one would leave nothing for anybody. Reproducing it
/// needs a decode of the old width to finish after the new width's, which no
/// waiting can arrange, so both posts are made directly into the injected
/// mailbox.
///
/// The slot is made terminal first, by deleting the file behind it, so that
/// nothing the engine does can supply a texture afterwards. `TexturesReady` can
/// then only fire if the fresh post survived, which is what makes this test fail
/// when the guard is removed.
#[test]
fn a_stale_decode_must_not_replace_the_texture_that_superseded_it() {
    const WIDE: u32 = 256;
    const DAY_SLOT: usize = 1;

    let fixtures = TextureFixtures::with_width("engine_resolution_stale", WIDE);
    let mailbox = TextureMailbox::new(fixtures.paths().len() + 2);
    let injected = mailbox.clone();
    let harness = Harness::start(|config| {
        config.texture_paths = fixtures.paths();
        config.texture_resolution = WIDE;
        config.cache_dir = Some(fixtures.dir.clone());
        config.mailbox = Some(injected);
        config.params = SceneParams {
            // The day texture alone, and no atmosphere: then every lit pixel
            // comes from the texture under test and nothing else can stand in
            // for it.
            texture_index: 1,
            atmo_enabled: false,
            ..test_params()
        };
    });
    harness.wait_for_textures("at startup");
    let (rgba, _, _) = harness.next_frame();
    assert!(
        has_lit_pixels(&rgba),
        "the globe should be visible at first"
    );

    // Take the file away, then switch. The reload finds nothing to decode and
    // clears the slot's path, which is terminal: from here the only textures
    // this slot can ever get are the ones posted below.
    fixtures.remove_files();
    harness
        .engine
        .send(EngineCommand::SetTextureResolution(WIDE / 2));
    harness.wait_for_status(|text| !text.is_empty(), "the reload should start");
    harness.wait_for_status(str::is_empty, "the reload should fail and stop loading");

    // The reload's replacement, parked first, and then the superseded decode of
    // the old width arriving late. Nothing pokes the engine in between, so the
    // drain that follows sees whatever the mailbox kept.
    mailbox.post(decoded(DAY_SLOT, WIDE / 2, 1, 255));
    mailbox.post(decoded(DAY_SLOT, WIDE, 0, 0));
    harness.engine.send(EngineCommand::Poke);

    harness.wait_for_textures("after the stale arrival");
    let (rgba, _, _) = harness.next_frame();
    assert!(
        has_lit_pixels(&rgba),
        "the surviving texture is the white one, so the globe must be lit"
    );
}

/// A mailbox that disagrees with the engine's slot count fails at startup.
///
/// The seam is for tests, and both ways of getting it wrong are quiet: too few
/// slots and the posts for the high ones are dropped, leaving those slots
/// waiting for a load that was thrown away; too many and the consumer is handed
/// a slot index its own array does not have, which is a panic in the middle of a
/// session. The panic seen here is `start`'s, because the assertion runs on the
/// engine thread before it reports an adapter, so the caller learns about it
/// rather than a thread quietly dying.
#[test]
#[should_panic(expected = "engine thread died before reporting its adapter")]
fn a_mailbox_that_does_not_match_the_slot_count_is_refused() {
    let mut config = EngineConfig::headless((64, 64));
    // Two file-backed paths need four slots: the grid, both of them, the clouds.
    config.mailbox = Some(TextureMailbox::new(3));
    let _ = sunlit_core::engine::start(config);
}

/// A stale arrival that nothing is racing is discarded rather than drawn.
///
/// Separate from the ordering above because it asserts the other half: not that
/// the fresh post survives, but that the stale one is never applied. A discarded
/// message dirties nothing, so no frame follows it; were it applied, the frame it
/// dirtied would show the black globe it carries.
#[test]
fn a_stale_arrival_produces_no_frame_at_all() {
    const WIDE: u32 = 256;
    const DAY_SLOT: usize = 1;

    let fixtures = TextureFixtures::with_width("engine_resolution_stale_alone", WIDE);
    let mailbox = TextureMailbox::new(fixtures.paths().len() + 2);
    let injected = mailbox.clone();
    let harness = Harness::start(|config| {
        config.texture_paths = fixtures.paths();
        config.texture_resolution = WIDE;
        config.cache_dir = Some(fixtures.dir.clone());
        config.mailbox = Some(injected);
        config.params = SceneParams {
            texture_index: 1,
            atmo_enabled: false,
            ..test_params()
        };
    });
    harness.wait_for_textures("at startup");
    harness
        .engine
        .send(EngineCommand::SetTextureResolution(WIDE / 2));
    harness.wait_for_textures("after the switch");
    harness.drained_frame(Duration::from_millis(300));

    mailbox.post(decoded(DAY_SLOT, WIDE, 0, 0));
    harness.engine.send(EngineCommand::Poke);
    assert!(
        harness.drained_frame(Duration::from_millis(500)).is_none(),
        "a discarded arrival must not reach the GPU, and so must not produce a frame"
    );
}

/// A resolution change while the first load is still running converges.
///
/// The purge here happens with a decode genuinely in flight, which is the state
/// Step 3 promises to survive and the one the quick-succession test above cannot
/// reach. The in-flight decode's post is discarded when it arrives; what must
/// still happen is the reload, and the only evidence that it did is the slot
/// becoming ready at all.
#[test]
fn a_switch_while_the_first_load_is_running_still_converges() {
    const WIDE: u32 = 1024;

    let fixtures = TextureFixtures::with_width("engine_resolution_midload", WIDE);
    let harness = Harness::start(|config| {
        config.texture_paths = fixtures.paths();
        config.texture_resolution = WIDE;
        config.cache_dir = Some(fixtures.dir.clone());
        config.params = SceneParams {
            texture_index: 1,
            atmo_enabled: false,
            ..test_params()
        };
    });
    harness.wait_for_status(|text| !text.is_empty(), "the first load should start");
    harness
        .engine
        .send(EngineCommand::SetTextureResolution(WIDE / 4));

    harness.wait_for_textures("after a switch mid-load");
    let (rgba, _, _) = harness.next_frame();
    assert!(has_lit_pixels(&rgba));
}

/// A wallpaper update asked for during a reload waits for the reload.
///
/// The purge leaves the renderer on the procedural grid until the new textures
/// arrive, and "change the resolution, then click Set as Wallpaper" is a natural
/// sequence, so without the hold-back the grid is what lands on the desktop.
/// Asserted on the order of the engine's own events, which is the only place the
/// distinction shows: a publish from the grid would be reported before the
/// textures were ready rather than after.
#[test]
fn a_wallpaper_update_during_a_reload_waits_for_the_textures() {
    let fixtures = TextureFixtures::new("engine_resolution_wallpaper");
    let sink = Arc::new(CountingSink::new(64, 32));
    let published = Arc::clone(&sink);
    let harness = Harness::start(|config| {
        config.texture_paths = fixtures.paths();
        config.texture_resolution = TextureFixtures::WIDTH;
        config.cache_dir = Some(fixtures.dir.clone());
        config.wallpaper = published;
        config.params = SceneParams {
            texture_index: 3,
            ..test_params()
        };
    });
    harness.wait_for_textures("at startup");
    harness.drained_frame(Duration::from_millis(300));
    assert_eq!(sink.count(), 0, "nothing has asked for a wallpaper yet");

    // Both commands are handled before the engine ticks, so the purge has
    // already emptied the slots when the publish is asked for.
    harness.engine.send(EngineCommand::SetTextureResolution(
        TextureFixtures::WIDTH / 2,
    ));
    harness.engine.send(EngineCommand::RenderWallpaperNow);

    let deadline = std::time::Instant::now() + TIMEOUT;
    let mut ready_first = None;
    while let Ok(event) = harness.events.recv_deadline(deadline) {
        match event {
            EngineEvent::TexturesReady => {
                ready_first.get_or_insert(true);
            }
            EngineEvent::WallpaperSet(result) => {
                assert!(result.is_ok(), "the publish should have succeeded");
                assert_eq!(
                    ready_first,
                    Some(true),
                    "the wallpaper was published before the textures were loaded, \
                     which means it was published from the procedural grid"
                );
                assert_eq!(sink.count(), 1, "exactly one frame should be published");
                return;
            }
            _ => {}
        }
    }
    panic!("no wallpaper result within {TIMEOUT:?}");
}

/// Whether `memory::snapshot` has an implementation for this platform.
///
/// Mirrors the cfg on `memory::snapshot` itself, and the same helper in the
/// soak test. It is the difference between "this platform cannot answer" and
/// "this platform failed to answer", and only the first of those may skip.
fn memory_counters_supported() -> bool {
    cfg!(any(windows, target_os = "linux", target_os = "macos"))
}

fn private_bytes() -> Option<u64> {
    sunlit_core::memory::snapshot().map(|s| s.private_bytes)
}

#[allow(clippy::cast_precision_loss)]
fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// The repository's real 8K assets, if this checkout has them.
///
/// `textures/**` is Git LFS, so a checkout without the objects holds pointer
/// files of a couple of hundred bytes, which exist as far as anything that only
/// asks about existence is concerned. Size is what tells the two apart, the same
/// check the guest staging in the xtask makes.
fn real_textures() -> Result<Vec<Option<std::path::PathBuf>>, String> {
    /// Smaller than either asset and far larger than an LFS pointer.
    const MIN_BYTES: u64 = 64 * 1024;

    let dir = sunlit_core::assets::texture_loader::resolve_textures_dir(None)
        .ok_or_else(|| "there is no textures directory".to_owned())?;
    let mut paths = Vec::new();
    for name in ["world.topo.200405.jxl", "BlackMarble_2016.jxl"] {
        let path = dir.join(name);
        match std::fs::metadata(&path) {
            Ok(meta) if meta.len() >= MIN_BYTES => paths.push(Some(path)),
            Ok(meta) => {
                return Err(format!(
                    "{name} is {} bytes, which is a Git LFS pointer rather than the asset",
                    meta.len()
                ));
            }
            Err(e) => return Err(format!("{name} is not readable: {e}")),
        }
    }
    Ok(paths)
}

/// Going down a resolution has to give the memory back, which is the whole
/// point of the setting.
///
/// The widest and the narrowest of the offered widths, on the real assets: at
/// 8192 the two textures and their mip chains are about 341 MiB of pixels, at
/// 2048 about 21 MiB, and the purge is what decides whether the difference
/// comes back. A margin well below the expected drop, because what is being
/// asserted is that the old textures were released, not how promptly an
/// allocator returns pages to the OS.
///
/// Only the software adapter is measured here (the headless config forces it),
/// which is what puts the textures in process memory in the first place; on a
/// discrete GPU they would live in VRAM, where this counter cannot see them.
#[test]
fn lowering_the_resolution_lowers_the_process_footprint() {
    /// Drop the measurement must show, out of roughly 320 MiB expected.
    const MIN_DROP: u64 = 128 * 1024 * 1024;
    const WIDE: u32 = 8192;
    const NARROW: u32 = 2048;

    if !memory_counters_supported() {
        println!("skipped: no memory counters on this platform");
        return;
    }
    let paths = match real_textures() {
        Ok(paths) => paths,
        Err(why) => {
            println!("skipped: {why}; `git lfs pull` fetches the assets");
            return;
        }
    };

    let cache = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("resolution_memory_cache");
    std::fs::create_dir_all(&cache).expect("create the cache directory");

    // Build the narrow copies before measuring anything. Otherwise the switch
    // decodes both 8K sources one last time to make them, and those two 128 MiB
    // buffers are freed but possibly still held by the allocator when the second
    // snapshot is taken, which would hide the very thing being measured.
    sunlit_core::assets::texture_loader::register_jxl_hook();
    for path in paths.iter().flatten() {
        sunlit_core::assets::texture_cache::load_at_resolution(path, NARROW, Some(&cache))
            .expect("build the narrow copy");
    }

    let harness = Harness::start(|config| {
        config.texture_paths = paths.clone();
        config.texture_resolution = WIDE;
        config.cache_dir = Some(cache.clone());
        config.params = SceneParams {
            texture_index: 3,
            ..test_params()
        };
    });
    harness.wait_for_textures("at 8192");
    harness.next_frame();
    harness.drained_frame(Duration::from_millis(500));
    let wide = private_bytes().expect("this platform reports memory counters");
    let wide_report = harness.engine.memory_report().expect("a report");
    println!("at {WIDE}:\n{wide_report}");

    harness
        .engine
        .send(EngineCommand::SetTextureResolution(NARROW));
    harness.wait_for_textures("at 2048");
    let (rgba, _, _) = harness.next_frame();
    assert!(has_lit_pixels(&rgba), "the narrow textures should render");
    harness.drained_frame(Duration::from_millis(500));
    let narrow = private_bytes().expect("this platform reports memory counters");
    let narrow_report = harness.engine.memory_report().expect("a report");
    println!("at {NARROW}:\n{narrow_report}");

    println!(
        "private bytes: {:.1} MiB at {WIDE}, {:.1} MiB at {NARROW}, {:.1} MiB returned",
        mib(wide),
        mib(narrow),
        mib(wide.saturating_sub(narrow)),
    );
    assert!(
        wide.saturating_sub(narrow) >= MIN_DROP,
        "switching from {WIDE} to {NARROW} returned {:.1} MiB, expected at least {:.1} MiB",
        mib(wide.saturating_sub(narrow)),
        mib(MIN_DROP),
    );

    // The report has to agree with the measurement, on the real widths rather
    // than on the small fixtures the other resolution tests use.
    assert_eq!(expected_widths(&wide_report, "day_texture"), [WIDE]);
    assert_eq!(expected_widths(&narrow_report, "day_texture"), [NARROW]);
    assert_eq!(expected_widths(&narrow_report, "night_texture"), [NARROW]);
    assert!(
        !narrow_report
            .expected
            .iter()
            .any(|texture| texture.width == WIDE),
        "the report still lists a texture at {WIDE}:\n{narrow_report}"
    );
}

// ---------------------------------------------------------------------------
// The cloud variant follows the texture resolution
// ---------------------------------------------------------------------------

/// The variant width in a cloud URL, from the one path segment shaped `WxH`.
fn variant_width_of(url: &str) -> Option<u32> {
    url.split('/').find_map(|segment| {
        let (width, height) = segment.split_once('x')?;
        let width: u32 = width.parse().ok()?;
        let _: u32 = height.parse().ok()?;
        Some(width)
    })
}

/// A cloud source that serves an image sized after the variant it was last
/// pointed at, so a test can see which variant reached the GPU rather than
/// only which URL was asked for.
///
/// The images are small next to the real ones and still far enough apart that
/// replacing one with the other moves wgpu's texture counter.
struct VariantCloud {
    width: Mutex<u32>,
    fetches: std::sync::atomic::AtomicU64,
    retargets: Mutex<Vec<String>>,
    /// Every fetch fails while this is set, as an outage would.
    offline: std::sync::atomic::AtomicBool,
}

impl VariantCloud {
    /// A JPEG this many times narrower than the variant it stands for.
    const SCALE: u32 = 8;

    fn new(width: u32) -> Self {
        Self {
            width: Mutex::new(width),
            fetches: std::sync::atomic::AtomicU64::new(0),
            retargets: Mutex::new(Vec::new()),
            offline: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn go_offline(&self) {
        self.offline
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// The image size this source serves for `variant_width`.
    fn image_size(variant_width: u32) -> (u32, u32) {
        (
            variant_width / Self::SCALE,
            variant_width / (Self::SCALE * 2),
        )
    }

    fn fetches(&self) -> u64 {
        self.fetches.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// The variant width of every URL this source has been pointed at, in
    /// order. The first is the one construction sets, which is how the updater
    /// makes the cache entry's name and its contents agree.
    fn retarget_widths(&self) -> Vec<u32> {
        self.retargets
            .lock()
            .expect("retarget log")
            .iter()
            .map(|url| variant_width_of(url).expect("a cloud URL names its variant"))
            .collect()
    }
}

impl sunlit_core::assets::cloud_source::CloudSource for VariantCloud {
    fn fetch_if_changed(
        &self,
        known_etag: Option<&str>,
    ) -> Result<Option<sunlit_core::assets::cloud_source::CloudImage>, String> {
        if self.offline.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("the fixture is offline".to_owned());
        }
        let width = *self.width.lock().expect("variant width");
        let etag = format!("v{width}");
        if known_etag == Some(etag.as_str()) {
            return Ok(None);
        }
        let (w, h) = Self::image_size(width);
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            w,
            h,
            image::Rgba([200, 200, 200, 255]),
        ))
        .into_rgb8()
        .write_to(&mut buf, image::ImageFormat::Jpeg)
        .map_err(|e| format!("encode: {e}"))?;
        self.fetches
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Some(sunlit_core::assets::cloud_source::CloudImage {
            bytes: buf.into_inner(),
            etag: Some(etag),
            last_modified: None,
        }))
    }

    fn describe(&self) -> String {
        "variant fixture".to_owned()
    }

    fn retarget(&self, url: &str) {
        self.retargets
            .lock()
            .expect("retarget log")
            .push(url.to_owned());
        if let Some(width) = variant_width_of(url) {
            *self.width.lock().expect("variant width") = width;
        }
    }
}

/// Block until the cloud texture in the report is `expected`, or panic.
///
/// Polling the report is also what keeps the engine ticking, and there is no
/// event for "the cloud overlay changed": it is an overlay, so it is
/// deliberately not part of `TexturesReady`.
fn wait_for_cloud_size(harness: &Harness, expected: (u32, u32), what: &str) {
    let deadline = std::time::Instant::now() + TIMEOUT;
    loop {
        let report = harness.engine.memory_report().expect("a report");
        if let Some(cloud) = report
            .expected
            .iter()
            .find(|texture| texture.label == "cloud_texture")
            && (cloud.width, cloud.height) == expected
        {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{what}: no {expected:?} cloud texture within {TIMEOUT:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The fetched cloud variant follows the resolution: a switch costs a refetch
/// of the new variant, and the replacement frees the texture it replaced.
#[test]
fn the_cloud_variant_follows_the_texture_resolution() {
    const WIDE: u32 = 8192;
    const NARROW: u32 = 2048;

    let source = Arc::new(VariantCloud::new(WIDE));
    let cloud = Arc::clone(&source) as Arc<dyn sunlit_core::assets::cloud_source::CloudSource>;
    let harness = Harness::start(move |config| {
        config.texture_resolution = WIDE;
        config.cloud = Some(cloud);
    });

    wait_for_cloud_size(&harness, VariantCloud::image_size(WIDE), "at startup");
    harness.next_frame();
    let before = harness.engine.memory_report().expect("a report");
    assert_eq!(source.fetches(), 1);
    assert_eq!(
        source.retarget_widths(),
        [WIDE],
        "construction points the source at the configured variant"
    );

    harness
        .engine
        .send(EngineCommand::SetTextureResolution(NARROW));
    wait_for_cloud_size(
        &harness,
        VariantCloud::image_size(NARROW),
        "after the switch",
    );
    harness.next_frame();
    let after = harness.engine.memory_report().expect("a report");

    assert_eq!(
        source.fetches(),
        2,
        "the switch must cost a fetch of the new variant"
    );
    assert_eq!(source.retarget_widths(), [WIDE, NARROW]);

    // One cloud texture, at the new size: the replacement went through the
    // slot rather than beside it.
    let sizes: Vec<(u32, u32)> = after
        .expected
        .iter()
        .filter(|texture| texture.label == "cloud_texture")
        .map(|texture| (texture.width, texture.height))
        .collect();
    assert_eq!(sizes, [VariantCloud::image_size(NARROW)]);

    // And the old one was actually freed, not merely forgotten. Skipped where
    // the backend keeps no texture counter; D3D12 and Vulkan both do.
    let (Ok(measured_before), Ok(measured_after)) = (
        u64::try_from(before.counters.texture_bytes),
        u64::try_from(after.counters.texture_bytes),
    ) else {
        panic!("wgpu reported negative texture memory");
    };
    println!(
        "wgpu texture bytes: {:.2} MiB before, {:.2} MiB after",
        mib(measured_before),
        mib(measured_after)
    );
    if measured_before == 0 {
        println!("skipping the free check: this backend maintains no texture counter");
        return;
    }
    let freed = before.expected_bytes() - after.expected_bytes();
    assert!(
        measured_after + freed / 2 <= measured_before,
        "wgpu still holds {:.2} MiB against {:.2} MiB before, having dropped {:.2} MiB \
         of expected textures:\n{after}",
        mib(measured_after),
        mib(measured_before),
        mib(freed)
    );
}

/// A switch back to a variant whose cache entry is still fresh has to show
/// that variant again, through the engine rather than by hand.
///
/// The whole chain matters here and the unit tests cannot reach it: the command
/// retargets the worker, the worker calls `set_resolution` before its next
/// poll, that poll is answered with a 304 because the entry it just adopted is
/// current, and nothing in production calls `post_cached` after startup. This
/// is the only engine test with both a cache directory and a cloud source, and
/// it takes both to reach the cloud disk cache: the resolution tests have a
/// directory but no cloud, and the other cloud tests have a cloud but no
/// directory.
#[test]
fn a_switch_back_to_a_cached_variant_shows_it_again() {
    const WIDE: u32 = 8192;
    const NARROW: u32 = 2048;

    let cache = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("cloud_switch_back_cache");
    let _ = std::fs::remove_dir_all(&cache);
    std::fs::create_dir_all(&cache).expect("create the cache directory");

    let source = Arc::new(VariantCloud::new(WIDE));
    let cloud = Arc::clone(&source) as Arc<dyn sunlit_core::assets::cloud_source::CloudSource>;
    let cache_dir = cache.clone();
    let harness = Harness::start(move |config| {
        config.texture_resolution = WIDE;
        config.cloud = Some(cloud);
        config.cache_dir = Some(cache_dir);
    });

    wait_for_cloud_size(&harness, VariantCloud::image_size(WIDE), "at startup");
    harness
        .engine
        .send(EngineCommand::SetTextureResolution(NARROW));
    wait_for_cloud_size(
        &harness,
        VariantCloud::image_size(NARROW),
        "after switching down",
    );

    // Both entries are now on disk and neither has gone stale, so the switch
    // back is answered with a 304 and has to fall back to what it cached.
    harness
        .engine
        .send(EngineCommand::SetTextureResolution(WIDE));
    wait_for_cloud_size(
        &harness,
        VariantCloud::image_size(WIDE),
        "after switching back up",
    );
    assert_eq!(
        source.fetches(),
        2,
        "the switch back is served from disk, not downloaded again"
    );

    let _ = std::fs::remove_dir_all(&cache);
}

/// A switch must not empty the cloud slot: a cloudless globe while a download
/// runs is a worse picture than one at the previous variant, and offline the
/// gap would never close.
#[test]
fn a_switch_keeps_the_old_cloud_texture_until_the_new_one_lands() {
    const WIDE: u32 = 8192;
    const NARROW: u32 = 2048;
    /// Long enough for the retarget, the failed poll, and several ticks.
    const SETTLE: Duration = Duration::from_secs(2);

    let source = Arc::new(VariantCloud::new(WIDE));
    let cloud = Arc::clone(&source) as Arc<dyn sunlit_core::assets::cloud_source::CloudSource>;
    let harness = Harness::start(move |config| {
        config.texture_resolution = WIDE;
        config.cloud = Some(cloud);
    });

    wait_for_cloud_size(&harness, VariantCloud::image_size(WIDE), "at startup");

    source.go_offline();
    harness
        .engine
        .send(EngineCommand::SetTextureResolution(NARROW));
    std::thread::sleep(SETTLE);

    assert_eq!(
        source.retarget_widths(),
        [WIDE, NARROW],
        "the switch should still have reached the cloud pipeline"
    );
    let report = harness.engine.memory_report().expect("a report");
    let sizes: Vec<(u32, u32)> = report
        .expected
        .iter()
        .filter(|texture| texture.label == "cloud_texture")
        .map(|texture| (texture.width, texture.height))
        .collect();
    assert_eq!(
        sizes,
        [VariantCloud::image_size(WIDE)],
        "the old cloud texture must survive a switch the network cannot answer:\n{report}"
    );
}

// ---------------------------------------------------------------------------
// The memory report
// ---------------------------------------------------------------------------

/// The width of every texture the report says the renderer owns under `label`.
fn expected_widths(report: &sunlit_core::memory_report::MemoryReport, label: &str) -> Vec<u32> {
    report
        .expected
        .iter()
        .filter(|texture| texture.label == label)
        .map(|texture| texture.width)
        .collect()
}

#[test]
fn the_report_names_the_textures_the_renderer_owns() {
    let fixtures = TextureFixtures::new("engine_memory_report");
    let harness = blend_harness(&fixtures, TextureFixtures::WIDTH);
    harness.wait_for_textures("at startup");
    harness.next_frame();

    let report = harness
        .engine
        .memory_report()
        .expect("the engine should answer with a report");
    println!("{report}");

    assert_eq!(
        expected_widths(&report, "day_texture"),
        [TextureFixtures::WIDTH]
    );
    assert_eq!(
        expected_widths(&report, "night_texture"),
        [TextureFixtures::WIDTH]
    );
    assert_eq!(
        expected_widths(&report, "grid_texture").len(),
        1,
        "the procedural grid is always resident"
    );
    // The preview target, at the size the harness asked for.
    assert_eq!(expected_widths(&report, "render_texture"), [512]);
    assert!(report.expected_bytes() > 0);
}

/// The measured and computed columns are the point of the report, so they have
/// to agree.
///
/// The tolerance is loose in both directions on purpose. wgpu's counter is the
/// backend allocator's figure, which rounds every texture up to an alignment
/// and may cover objects the renderer does not know it owns, so it sits above
/// the computed total; a backend that attributes some of its textures
/// elsewhere would sit below it. What the band is tight enough to catch is the
/// thing worth catching: a surface texture that was never freed, which is an
/// order of magnitude, not a factor of two.
#[test]
fn the_measured_and_computed_texture_totals_agree() {
    let fixtures = TextureFixtures::new("engine_memory_report_totals");
    let harness = blend_harness(&fixtures, TextureFixtures::WIDTH);
    harness.wait_for_textures("at startup");
    harness.next_frame();

    let report = harness.engine.memory_report().expect("a report");
    let expected = report.expected_bytes();
    let Ok(measured) = u64::try_from(report.counters.texture_bytes) else {
        panic!("wgpu reported negative texture memory: {report}");
    };
    println!(
        "expected {:.1} MiB, wgpu counter {:.1} MiB",
        mib(expected),
        mib(measured)
    );
    if measured == 0 {
        println!("skipping the comparison: this backend maintains no texture counter");
        return;
    }
    assert!(
        measured >= expected / 2 && measured <= expected * 2 + 16 * 1024 * 1024,
        "wgpu says {:.1} MiB of textures, the renderer expects {:.1} MiB:\n{report}",
        mib(measured),
        mib(expected)
    );
}

/// After a switch down, nothing of the old width is left in the report and the
/// computed total has fallen.
///
/// The widths are the fixtures' rather than the 8192 and 2048 a user picks
/// between, for the reason every other resolution test uses small fixtures: the
/// property is about the purge and the reload, and real 8K assets would make
/// this a minute long. `lowering_the_resolution_lowers_the_process_footprint`
/// is the one that measures the real pair, where they are present.
#[test]
fn a_switch_down_leaves_no_texture_at_the_old_width() {
    const WIDE: u32 = 256;
    const NARROW: u32 = 32;

    let fixtures = TextureFixtures::with_width("engine_memory_report_switch", WIDE);
    let harness = blend_harness(&fixtures, WIDE);
    harness.wait_for_textures("at startup");
    harness.next_frame();

    let before = harness.engine.memory_report().expect("a report");
    assert_eq!(expected_widths(&before, "day_texture"), [WIDE]);

    harness
        .engine
        .send(EngineCommand::SetTextureResolution(NARROW));
    harness.wait_for_textures("after switching down");
    harness.next_frame();

    let after = harness.engine.memory_report().expect("a report");
    println!("{after}");
    assert_eq!(expected_widths(&after, "day_texture"), [NARROW]);
    assert_eq!(expected_widths(&after, "night_texture"), [NARROW]);
    assert!(
        !after
            .expected
            .iter()
            .any(|texture| texture.label.ends_with("_texture")
                && texture.width == WIDE
                && texture.mip_levels > 1),
        "a texture at the old width survived the switch:\n{after}"
    );
    assert!(
        after.expected_bytes() < before.expected_bytes(),
        "the computed total should fall with the width"
    );
}

/// Pool slack is what the allocator holds but nothing is using, and the report
/// shows it as the difference between the two totals it prints.
#[test]
fn the_allocator_section_reports_reserved_at_least_as_large_as_allocated() {
    let harness = Harness::start(|_| {});
    harness.next_frame();
    let report = harness.engine.memory_report().expect("a report");

    let Some(allocator) = &report.allocator else {
        println!(
            "skipping: {} has no allocator report, and the section says so",
            report.adapter
        );
        assert!(
            report.to_string().contains("no allocator report"),
            "a backend without a report must still print the section:\n{report}"
        );
        return;
    };
    assert!(
        allocator.total_reserved_bytes >= allocator.total_allocated_bytes,
        "reserved {} is below allocated {}",
        allocator.total_reserved_bytes,
        allocator.total_allocated_bytes
    );
    assert!(
        allocator.total_allocated_bytes > 0,
        "a live device holds something:\n{report}"
    );
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
    assert!(
        ready,
        "the grid texture is built up front and is always ready"
    );
}
