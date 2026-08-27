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

use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use sunlit_core::assets::mailbox::{DecodedTextureMessage, TextureMailbox};
use sunlit_core::assets::texture_loader::DecodedImage;
use sunlit_core::config::QualityTier;
use sunlit_core::engine::wallpaper_sink::CountingSink;
use sunlit_core::engine::{EngineCommand, EngineConfig, EngineEvent, EngineHandle};
use sunlit_core::params::SceneParams;

mod support;

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

    /// Block until an overlay's texture has reached the GPU, by its GPU label.
    ///
    /// `TexturesReady` deliberately excludes the overlays, because nothing in
    /// the engine waits for one. So a test that wants the Moon or the Milky Way
    /// in a frame asks the memory report whether the renderer owns the texture
    /// yet, which is the only thing that answers it.
    fn wait_for_slot_texture(&self, label: &str) {
        let deadline = std::time::Instant::now() + TIMEOUT;
        while std::time::Instant::now() < deadline {
            let report = self
                .engine
                .memory_report()
                .expect("the engine should answer with a report");
            if report.expected.iter().any(|texture| texture.label == label) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("the {label} did not arrive within {TIMEOUT:?}");
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

    // With the atmosphere off, the only thing outside the globe is stars, so
    // every pixel the two frames disagree about is one a star painted. That is
    // what lets this assert what its name says rather than the much weaker "one
    // pixel changed": at intensity zero each of those pixels is still exactly
    // the clear color, not a dimmed star.
    let harness = Harness::start(|config| {
        config.params.atmo_enabled = false;
        config.params.star_intensity = 0.0;
    });
    let (stars_off, _, _) = harness.next_frame();
    let mut stars_on_params = test_params();
    stars_on_params.atmo_enabled = false;
    stars_on_params.star_intensity = 1.5;
    harness
        .engine
        .send(EngineCommand::UpdateParams(Box::new(stars_on_params)));
    let (stars_on, _, _) = harness.next_frame();

    let mut star_pixels = 0_usize;
    for (index, (off, on)) in stars_off
        .chunks_exact(4)
        .zip(stars_on.chunks_exact(4))
        .enumerate()
    {
        if off != on {
            star_pixels += 1;
            assert_eq!(
                off, CLEAR,
                "pixel {index} carries {off:?} with stars disabled, not the clear color"
            );
        }
    }
    assert!(
        star_pixels > 0,
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

/// A night-side framing at a longitude chosen for where the Sun lands.
///
/// At noon on day 172 the subsolar point is near the prime meridian, so a
/// camera on the far side looks at the night side with the Sun somewhere
/// beyond the limb. Which side of the painted limb it lands on is what the
/// longitude picks: 160 clears it, 170.5 grazes the atmosphere band, and 176
/// puts it well inside the painted disc. The atmosphere is off so that the
/// only thing these cases can be measuring is the Sun.
///
/// Those three numbers are for the 512 by 256 the preview quantizes down to,
/// which `sun_off_and_on` asserts rather than assumes: at another aspect ratio
/// the painted silhouette is a different size and all three move.
fn sun_params(longitude: f32) -> SceneParams {
    let mut params = test_params();
    params.datetime.custom_day_of_year = 172;
    params.camera.longitude = longitude;
    params.camera.latitude = 0.0;
    params.camera.zoom = 0.45;
    params.atmo_enabled = false;
    params
}

/// Render `params` with the Sun off and then on, and return both frames.
fn sun_off_and_on(longitude: f32) -> (Vec<u8>, Vec<u8>) {
    sun_off_and_on_framed(sun_params(longitude))
}

/// The same for a framing the caller has already adjusted.
fn sun_off_and_on_framed(params: SceneParams) -> (Vec<u8>, Vec<u8>) {
    let mut off = params;
    off.sun_glow = 0.0;
    let harness = Harness::start(|config| config.params = off);
    let (off, width, height) = harness.next_frame();
    assert_eq!(
        (width, height),
        (512, 256),
        "the longitudes these cases pick are for one framing"
    );
    let mut on = params;
    on.sun_glow = 1.5;
    harness
        .engine
        .send(EngineCommand::UpdateParams(Box::new(on)));
    let (on, _, _) = harness.next_frame();
    (off, on)
}

#[test]
fn a_sun_behind_the_painted_globe_paints_nothing() {
    let (off, on) = sun_off_and_on(176.0);
    let differing = off
        .chunks_exact(4)
        .zip(on.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(
        differing, 0,
        "{differing} pixels changed with the Sun fully behind the globe"
    );
}

#[test]
fn zero_sun_glow_takes_the_sun_out_of_the_frame() {
    let (off, on) = sun_off_and_on(160.0);
    let mut painted = 0_usize;
    for (index, (dark, lit)) in off.chunks_exact(4).zip(on.chunks_exact(4)).enumerate() {
        if dark == lit {
            continue;
        }
        painted += 1;
        // The two sun draws are additive, so every pixel the Sun touches can
        // only have got brighter.
        assert!(
            (0..3).all(|c| lit[c] >= dark[c]),
            "pixel {index} went from {dark:?} to {lit:?}, which additive blending cannot do"
        );
    }
    assert!(
        painted > 100,
        "a Sun clear of the limb painted only {painted} pixels"
    );
}

#[test]
fn a_sun_grazing_the_limb_turns_the_glare_warm() {
    // What each longitude adds to its own sun-off frame, summed per channel.
    // Comparing that against the other longitude's would compare two different
    // Earths; comparing each against its own leaves only the Sun.
    let warmth = |longitude: f32| {
        let (off, on) = sun_off_and_on(longitude);
        let mut added = [0_u64; 3];
        for (dark, lit) in off.chunks_exact(4).zip(on.chunks_exact(4)) {
            for (channel, total) in added.iter_mut().enumerate() {
                *total += u64::from(lit[channel].saturating_sub(dark[channel]));
            }
        }
        #[allow(clippy::cast_precision_loss)]
        let ratio = added[0] as f64 / added[2].max(1) as f64;
        ratio
    };
    let clear = warmth(160.0);
    let grazing = warmth(170.5);
    assert!(
        clear < 1.15,
        "a Sun clear of the atmosphere should glare near-white, red over blue was {clear:.2}"
    );
    assert!(
        grazing > clear * 1.3,
        "a Sun in the transit band should glare warmer than a clear one, \
         but red over blue was {grazing:.2} against {clear:.2}"
    );
}

/// A Sun past the reach of an unpanned frame is drawn once a pan reaches it.
///
/// Both sun draws cull themselves against `sky_corner_angle`, the angle of the
/// frame's furthest corner widened by the pan. The glare's cone is 30 degrees
/// wide, so the widening decides anything only where the Sun is more than 30
/// degrees past an unpanned corner: nearer than that the glare draw clears its
/// own cull without the pan and paints the same pixels either way. At
/// longitude 68 the Sun sits 110.5 degrees off the view axis against a 76.1
/// degree corner, which is past both, and a pan of 0.9 brings the frame's
/// nearest pixel to 10.2 degrees from it.
#[test]
fn a_pan_past_the_frame_corner_still_draws_the_sun() {
    /// Pixels the Sun added more than a handful of levels to, and its most.
    fn added(params: SceneParams) -> (usize, u8) {
        let (off, on) = sun_off_and_on_framed(params);
        let mut painted = 0;
        let mut brightest = 0;
        for (dark, lit) in off.chunks_exact(4).zip(on.chunks_exact(4)) {
            let gained = [0, 1, 2].map(|c| lit[c].saturating_sub(dark[c]));
            if gained.iter().any(|&value| value > 4) {
                painted += 1;
            }
            brightest = brightest.max(gained.into_iter().max().unwrap_or(0));
        }
        (painted, brightest)
    }

    let mut framed = sun_params(68.0);
    assert_eq!(
        added(framed),
        (0, 0),
        "no point of an unpanned frame is within the glare's cone here, so \
         the Sun may not touch a pixel of it"
    );

    framed.camera.offset_x = -0.9;
    let (painted, brightest) = added(framed);
    assert!(
        painted > 4000 && brightest > 8,
        "the pan brings the frame's nearest pixel to 10.2 degrees from the \
         Sun, but only {painted} pixels gained more than four levels and the \
         brightest gained {brightest}"
    );
}

/// Panning the frame slides the whole composite across the framebuffer, so a
/// panned frame is the unpanned one moved by the pan and nothing else.
///
/// The pan reaches the Sun through four places that each carry a sign:
/// `scene::sun_occlusion::sky_lens_disc`, which is where the CPU decides what
/// the globe hides, and `sun_disc`, `sky_corner_angle` and `sky_lens_direction`
/// in the shader. Any one of them disagreeing with the pan the globe got leaves
/// the composite sheared rather than moved, which no other case in the suite
/// would see: every other frame in it is rendered with no pan at all.
#[test]
fn a_pan_slides_the_composite_without_shearing_it() {
    // An eighth of the frame's width, which is a whole number of pixels, so
    // the two frames compare without resampling either.
    const PAN: f32 = -0.25;
    const SHIFT: usize = 64;

    let panned_params = |offset_x: f32| {
        let mut params = sun_params(160.0);
        params.sun_glow = 1.5;
        params.camera.offset_x = offset_x;
        params
    };
    let harness = Harness::start(|config| config.params = panned_params(0.0));
    let (centered, width, height) = harness.next_frame();
    assert_eq!(
        (width, height),
        (512, 256),
        "the pan below is a pixel count for one framing"
    );
    harness
        .engine
        .send(EngineCommand::UpdateParams(Box::new(panned_params(PAN))));
    let (panned, _, _) = harness.next_frame();

    let row = width as usize * 4;
    let compared = height as usize * (width as usize - SHIFT) * 3;
    let mut worst = 0_u8;
    let mut worst_at = (0_usize, 0_usize);
    let mut total = 0_u64;
    let mut outliers = 0_u64;
    let mut lit = 0_u64;
    for y in 0..height as usize {
        for x in 0..width as usize - SHIFT {
            let from = y * row + x * 4;
            let to = y * row + (x + SHIFT) * 4;
            if centered[from..from + 3].iter().any(|&c| c > 8) {
                lit += 1;
            }
            for channel in 0..3 {
                let difference = centered[from + channel].abs_diff(panned[to + channel]);
                total += u64::from(difference);
                if difference > 1 {
                    outliers += 1;
                }
                if difference > worst {
                    worst = difference;
                    worst_at = (x, y);
                }
            }
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let mean = total as f64 / compared as f64;
    assert!(
        lit > 2000,
        "only {lit} pixels of the compared region carry anything, so this \
         would pass on an empty frame"
    );
    // A step of the 8-bit output is the budget, because the glare dithers from
    // the framebuffer position and that does not travel with the pan, and
    // because a center is a pan plus a projection rather than a projection
    // shifted by whole pixels, so an antialiased edge can round the other way.
    // The handful of channels allowed past it are where an interpolated value
    // is steep enough that the same last bit of the vertex moves it further:
    // 0 of 344,064 on warp and 5 on lavapipe, against tens of thousands for
    // any of the signs being wrong.
    assert!(
        mean < 0.15 && outliers <= 64,
        "the panned frame is not the unpanned one moved by {SHIFT} pixels: \
         mean {mean:.4}, {outliers} of {compared} channels off by more than \
         one, worst {worst} at {worst_at:?}"
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
    // Three file-backed paths need five slots: the grid, all three of them,
    // the clouds.
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

/// The repository's real assets, if this checkout has them.
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
    for name in [
        "world.topo.200405.jxl",
        "BlackMarble_2016.jxl",
        "lroc_color_poles_1k.jxl",
    ] {
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

// ---------------------------------------------------------------------------
// The Moon
// ---------------------------------------------------------------------------

/// A framing with the Moon in it, and nothing else that emits light.
///
/// The stars, the Sun and the atmosphere are all switched off, so the only
/// thing these cases can be measuring is the Moon; the camera looks at the
/// night side, where the sky lens has room to show it. The pinned instant is
/// the one the moon golden uses.
fn moon_params() -> SceneParams {
    let mut params = test_params();
    params.datetime.custom_day_of_year = 172;
    params.datetime.custom_hour = 21.0;
    params.camera.longitude = 160.0;
    params.camera.latitude = 0.0;
    params.camera.zoom = 0.45;
    params.atmo_enabled = false;
    params.star_intensity = 0.0;
    params.sun_glow = 0.0;
    params.sky_fov = 60.0;
    params.moon_size = 8.0;
    // Earthshine well above the clear color, so the unlit face is part of what
    // "only adds light" is measured over rather than a wash against the sky.
    params.moon_earthshine = 0.2;
    params
}

/// Texture paths for a configuration whose Moon slot points at `moon`.
fn moon_paths(moon: Option<std::path::PathBuf>) -> Vec<Option<std::path::PathBuf>> {
    vec![None, None, moon]
}

/// The Moon adds light to a dark sky and takes none away.
///
/// With nothing else drawn, every pixel the Moon touches can only get brighter,
/// which is the shape `sun_off_and_on` uses: one engine, one `UpdateParams`, an
/// off frame against an on frame.
#[test]
fn a_moon_on_the_night_sky_only_adds_light() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("engine_moon_adds_light");
    let fixture = support::write_moon_fixture(&dir);
    let params = moon_params();

    let harness = Harness::start(|config| {
        config.params = params;
        config.texture_paths = moon_paths(Some(fixture.clone()));
    });
    // The first frame is what spawns the load, and the texture arrives on a
    // later tick, so the frames come from the export path rather than from the
    // preview: an export renders now, with whatever the renderer holds.
    let (_, width, height) = harness.next_frame();
    assert_eq!(
        (width, height),
        (512, 256),
        "this framing is for one aspect ratio"
    );
    harness.wait_for_slot_texture("moon_texture");
    let on = harness
        .engine
        .export_pixels(512, 256)
        .expect("the engine should be able to export");

    let mut without = params;
    without.moon_brightness = 0.0;
    harness
        .engine
        .send(EngineCommand::UpdateParams(Box::new(without)));
    harness.next_frame();
    let off = harness
        .engine
        .export_pixels(512, 256)
        .expect("the engine should be able to export");

    let mut brighter = 0;
    for (before, after) in off.chunks_exact(4).zip(on.chunks_exact(4)) {
        let sum = |px: &[u8]| u32::from(px[0]) + u32::from(px[1]) + u32::from(px[2]);
        assert!(
            sum(after) >= sum(before),
            "a pixel went from {before:?} to {after:?} with nothing but the Moon drawn"
        );
        if sum(after) > sum(before) + 30 {
            brighter += 1;
        }
    }
    assert!(
        brighter > 500,
        "only {brighter} pixels got brighter with an eight times Moon in frame"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A Moon switched off and a Moon with no texture behind it are the same
/// picture, which is what makes the missing asset a non-event.
#[test]
fn a_switched_off_moon_and_a_missing_texture_draw_the_same_frame() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("engine_moon_off");
    let fixture = support::write_moon_fixture(&dir);
    let params = moon_params();

    let mut off = params;
    off.moon_brightness = 0.0;
    let with_texture = {
        let harness = Harness::start(|config| {
            config.params = off;
            config.texture_paths = moon_paths(Some(fixture.clone()));
        });
        harness.next_frame().0
    };
    let without_texture = {
        let harness = Harness::start(|config| {
            config.params = params;
            config.texture_paths = moon_paths(None);
        });
        harness.next_frame().0
    };
    let differing = with_texture
        .chunks_exact(4)
        .zip(without_texture.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(
        differing, 0,
        "{differing} pixels differ between a switched-off Moon and a missing one"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A Moon at the antipode of the view axis is not drawn at all.
///
/// The camera looks at the origin, so an eye on the line from the Earth to the
/// Moon, at any zoom short of the orbit, has the Moon exactly behind it. A cone
/// that reaches the lens's antipode has no finite image, which is what
/// `place_moon` reports by leaving the disc empty: the vertices would otherwise
/// land thousands of units out in every radial direction at once and the mesh's
/// triangles would sweep the frame. The globe drag reaches that camera, so the
/// frame it produces has to be the frame with no Moon in it.
#[test]
fn a_moon_at_the_view_antipode_draws_nothing() {
    const WIDTH: u32 = 512;
    const HEIGHT: u32 = 256;

    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("engine_moon_antipode");
    let fixture = support::write_moon_fixture(&dir);
    let mut params = moon_params();
    let direction = sky_for(&params).moon_position.normalize();
    params.camera.latitude = direction.y.asin().to_degrees();
    params.camera.longitude = direction.x.atan2(direction.z).to_degrees();

    #[allow(clippy::cast_precision_loss)]
    let viewport = glam::Vec2::new(WIDTH as f32, HEIGHT as f32);
    assert_eq!(
        moon_placement(&params, viewport).disc,
        None,
        "this framing is the one where the disc is empty, or it measures nothing"
    );

    let harness = Harness::start(|config| {
        config.params = params;
        config.texture_paths = moon_paths(Some(fixture.clone()));
    });
    harness.next_frame();
    harness.wait_for_slot_texture("moon_texture");
    let behind = harness
        .engine
        .export_pixels(WIDTH, HEIGHT)
        .expect("the engine should be able to export");

    let mut without = params;
    without.moon_brightness = 0.0;
    harness
        .engine
        .send(EngineCommand::UpdateParams(Box::new(without)));
    harness.next_frame();
    let off = harness
        .engine
        .export_pixels(WIDTH, HEIGHT)
        .expect("the engine should be able to export");

    let differing = behind
        .chunks_exact(4)
        .zip(off.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(
        differing, 0,
        "{differing} pixels differ between a Moon behind the camera and no Moon at all"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The Moon is an overlay: its texture is not what `TexturesReady` waits for,
/// and the slot it lands in is the one the layout reserves for it.
#[test]
fn the_moon_texture_lands_in_its_own_slot_without_delaying_readiness() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("engine_moon_slot");
    let fixture = support::write_moon_fixture(&dir);
    let harness = Harness::start(|config| {
        config.params = moon_params();
        config.texture_paths = moon_paths(Some(fixture.clone()));
    });
    // Readiness arrives with the grid alone, before the Moon has decoded.
    harness.wait_for_textures("at startup");
    harness.next_frame();
    harness.wait_for_slot_texture("moon_texture");

    let report = harness
        .engine
        .memory_report()
        .expect("the engine should answer with a report");
    assert_eq!(
        expected_widths(&report, "moon_texture"),
        [support::MOON_FIXTURE_WIDTH]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The instant `doy` and `hour` name, in 2026.
fn moon_time(doy: u16, hour: i32) -> astronomy_engine_bindings::astro_time_t {
    let (month, day) = sunlit_core::scene::datetime::day_of_year_to_month_day(doy, 2026);
    sunlit_core::scene::sun::make_time(2026, i32::from(month), i32::from(day), hour, 0, 0.0)
}

/// The fraction of the Moon's disc an ephemeris says is lit, seen from the
/// geocenter. The independent answer these cases are measured against.
#[allow(clippy::cast_possible_truncation)]
fn illuminated_fraction(doy: u16, hour: i32) -> f32 {
    // SAFETY: Astronomy_Illumination is a pure C function taking and returning
    // value types.
    #[allow(unsafe_code)]
    let illumination = unsafe {
        astronomy_engine_bindings::Astronomy_Illumination(
            astronomy_engine_bindings::astro_body_t_BODY_MOON,
            moon_time(doy, hour),
        )
    };
    assert_eq!(
        illumination.status,
        astronomy_engine_bindings::astro_status_t_ASTRO_SUCCESS,
        "Astronomy_Illumination failed"
    );
    illumination.phase_fraction as f32
}

/// The sky state the engine computes for `params`.
///
/// Every framing here pins its datetime, so the clock a live one would consult
/// does not enter the answer, and the year and the fractional hour come from
/// the parameters instead of being assumed.
fn sky_for(params: &SceneParams) -> sunlit_core::scene::sky::SkyState {
    assert!(
        params.datetime.use_custom,
        "the placement helpers only answer for a pinned datetime"
    );
    sunlit_core::scene::sky::compute_sky_state(&params.datetime)
}

/// The camera `params` describes, as far as the placement needs it.
///
/// `write_uniforms` applies the pan and the orientation and the helpers below
/// pass none of them, so a case that set one would measure a disc away from
/// where the Moon is drawn; refused rather than answered wrong.
fn camera_for(params: &SceneParams) -> sunlit_core::scene::camera::OrbitalCamera {
    let cam = &params.camera;
    assert!(
        [
            cam.offset_x,
            cam.offset_y,
            cam.tilt_deg,
            cam.yaw_deg,
            cam.pitch_deg
        ]
        .iter()
        .all(|value| *value == 0.0),
        "the placement helpers carry no pan, tilt, yaw or pitch"
    );
    sunlit_core::scene::camera::OrbitalCamera::new(
        cam.longitude,
        cam.latitude,
        sunlit_core::scene::camera::zoom_to_distance(cam.zoom),
    )
}

/// Where the Moon lands for `params` at `viewport`, disc and all.
fn moon_placement(
    params: &SceneParams,
    viewport: glam::Vec2,
) -> sunlit_core::scene::moon::MoonPlacement {
    let sky = sky_for(params);
    let camera = camera_for(params);
    sunlit_core::scene::moon::place_moon(&sunlit_core::scene::moon::MoonPlacementInputs {
        position: sky.moon_position,
        rotation: sky.moon_rotation,
        eye: camera.eye_position(),
        view: camera.view_matrix(),
        size: params.moon_size,
        sky_fov_deg: params.sky_fov,
        screen_offset: glam::Vec2::ZERO,
        viewport,
    })
}

/// Where the Moon's disc lands, and how large, for `params` at `viewport`.
fn moon_disc(
    params: &SceneParams,
    viewport: glam::Vec2,
) -> sunlit_core::scene::sun_occlusion::ScreenCircle {
    moon_placement(params, viewport)
        .disc
        .expect("the moon is on screen at these framings")
}

/// The Sun's position on the same screen, for the cases that need it.
fn sun_screen_position(params: &SceneParams, viewport: glam::Vec2) -> glam::Vec2 {
    let sky = sky_for(params);
    let camera = camera_for(params);
    let view_direction = (camera.view_matrix() * sky.sun_direction.extend(0.0))
        .truncate()
        .normalize();
    sunlit_core::scene::sun_occlusion::sky_lens_disc(
        view_direction,
        0.0,
        params.sky_fov,
        glam::Vec2::ZERO,
        viewport,
    )
    .expect("the sun has an image at these framings")
    .center
}

/// Every pixel within a tenth of a radius of the disk, as (position, red
/// channel), from a render.
///
/// A circle rather than a box, and only a tenth wider than the disk, because
/// the painted globe reaches inside the box at some of these framings and its
/// grid is bright. The tenth is what lets a disk drawn larger than the circle
/// the CPU placed show up as too much lit area rather than being cropped out of
/// the measurement.
fn disc_pixels(
    pixels: &[u8],
    width: u32,
    disc: sunlit_core::scene::sun_occlusion::ScreenCircle,
) -> Vec<(glam::Vec2, u8)> {
    let reach = disc.radius * 1.1;
    let mut out = Vec::new();
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    for y in (disc.center.y - reach).max(0.0) as u32..(disc.center.y + reach) as u32 {
        for x in (disc.center.x - reach).max(0.0) as u32..(disc.center.x + reach) as u32 {
            let position = glam::Vec2::new(x as f32, y as f32);
            if position.distance(disc.center) > reach {
                continue;
            }
            let index = ((y * width + x) * 4) as usize;
            out.push((position, pixels[index]));
        }
    }
    out
}

/// The lit fraction of the drawn disk against an ephemeris, at three phases.
///
/// The count comes from the GPU and the area from `scene::moon::place_moon`, so
/// a disk drawn at the wrong size misses this as surely as a phase computed the
/// wrong way round, and a terminator on the wrong side reports one minus the
/// answer. The tolerance covers three things: the camera's own parallax, which
/// is nine Earth radii against the Moon's sixty and moves the phase by up to
/// 0.02 at these framings; the disk's own edge, which is not antialiased and so
/// quantizes the area by about one part in the radius; and the terminator's
/// smoothstep, which is a band a pixel or so wide. Measured on warp at 48 pixels
/// of radius: 0.2089 against the ephemeris 0.2272, 0.4461 against 0.4439, and
/// 0.8619 against 0.8450, so the worst of the three is 0.018 against the 0.04
/// this allows.
#[test]
fn the_lit_fraction_tracks_the_ephemeris_at_three_phases() {
    const WIDTH: u32 = 1600;
    const HEIGHT: u32 = 800;
    /// Crescent, quarter and gibbous, all with the Moon clear of the painted
    /// globe and the camera's displacement nearly across the Sun's direction,
    /// which is what keeps the geocentric answer applicable.
    const INSTANTS: [(u16, i32); 3] = [(199, 16), (189, 8), (185, 4)];

    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("engine_moon_phase");
    let fixture = support::write_moon_fixture(&dir);
    let mut params = moon_params();
    // No earthshine, so the unlit face is black and the threshold is a
    // question about sunlight rather than about the floor.
    params.moon_earthshine = 0.0;
    params.datetime.custom_day_of_year = INSTANTS[0].0;
    #[allow(clippy::cast_precision_loss)]
    let hour = f32::from(u16::try_from(INSTANTS[0].1).expect("a small hour"));
    params.datetime.custom_hour = hour;

    let harness = Harness::start(|config| {
        config.params = params;
        config.texture_paths = moon_paths(Some(fixture.clone()));
    });
    harness.next_frame();
    harness.wait_for_slot_texture("moon_texture");

    #[allow(clippy::cast_precision_loss)]
    let viewport = glam::Vec2::new(WIDTH as f32, HEIGHT as f32);
    for (doy, hour) in INSTANTS {
        let mut at = params;
        at.datetime.custom_day_of_year = doy;
        at.datetime.custom_hour = f32::from(u16::try_from(hour).expect("a small hour"));
        harness
            .engine
            .send(EngineCommand::UpdateParams(Box::new(at)));
        harness.next_frame();
        let pixels = harness
            .engine
            .export_pixels(WIDTH, HEIGHT)
            .expect("the engine should be able to export");

        let disc = moon_disc(&at, viewport);
        let lit = disc_pixels(&pixels, WIDTH, disc)
            .into_iter()
            .filter(|(_, red)| *red > 40)
            .count();
        #[allow(clippy::cast_precision_loss)]
        let fraction = lit as f32 / (std::f32::consts::PI * disc.radius * disc.radius);
        let expected = illuminated_fraction(doy, hour);
        println!(
            "day {doy} hour {hour}: disk radius {:.1} px, {lit} lit pixels,              fraction {fraction:.4} against the ephemeris {expected:.4}",
            disc.radius
        );
        assert!(
            (fraction - expected).abs() < 0.04,
            "day {doy} hour {hour}: the drawn fraction {fraction:.4} is not the              ephemeris {expected:.4}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The lit limb faces the Sun.
///
/// The brightness centroid of the disk sits on the sunward side of its center,
/// and the direction from the center to the centroid is the direction of the
/// Sun on screen. That is the check a person makes by eye when they look at a
/// crescent, and it is the one thing the phase fraction cannot see: a
/// terminator rotated by ninety degrees leaves the fraction untouched.
#[test]
fn the_lit_limb_faces_the_sun() {
    const WIDTH: u32 = 1600;
    const HEIGHT: u32 = 800;

    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("engine_moon_limb");
    let fixture = support::write_moon_fixture(&dir);
    let mut params = moon_params();
    params.moon_earthshine = 0.0;
    params.datetime.custom_day_of_year = 199;
    params.datetime.custom_hour = 16.0;

    let harness = Harness::start(|config| {
        config.params = params;
        config.texture_paths = moon_paths(Some(fixture.clone()));
    });
    harness.next_frame();
    harness.wait_for_slot_texture("moon_texture");
    let pixels = harness
        .engine
        .export_pixels(WIDTH, HEIGHT)
        .expect("the engine should be able to export");

    #[allow(clippy::cast_precision_loss)]
    let viewport = glam::Vec2::new(WIDTH as f32, HEIGHT as f32);
    let disc = moon_disc(&params, viewport);
    let mut weight = 0.0_f32;
    let mut centroid = glam::Vec2::ZERO;
    for (position, red) in disc_pixels(&pixels, WIDTH, disc) {
        let value = f32::from(red);
        weight += value;
        centroid += position * value;
    }
    assert!(weight > 0.0, "the disk painted nothing");
    centroid /= weight;

    let toward_light = (sun_screen_position(&params, viewport) - disc.center).normalize();
    let toward_centroid = (centroid - disc.center).normalize_or_zero();
    let separation = toward_centroid.angle_to(toward_light).abs().to_degrees();
    println!(
        "the lit centroid is {:.1} px from the disk's center, {separation:.1} degrees off          the direction of the Sun",
        centroid.distance(disc.center)
    );
    assert!(
        separation < 10.0,
        "the lit side points {separation:.1} degrees away from the Sun"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The glare fades behind a Moon that covers the Sun.
///
/// The instant is the greatest eclipse of the 2024-04-08 total solar eclipse,
/// where `the_moon_covers_the_sun_at_the_2024_total_eclipse` measures the two
/// geocentric directions 0.347 degrees apart. The camera puts its view axis
/// seven degrees off the Sun, which is the window where two things are true at
/// once: the Sun's image clears the painted globe, so there is a glare to fade,
/// and the camera's own parallax leaves the Moon inside its own disc of the Sun.
/// The assertion is on pixels well outside the Moon's silhouette, because those
/// can only have changed through `sun_visible`: the Moon paints nothing there,
/// and dropping the Moon's disc on the way into `place_sun` leaves them
/// identical.
#[test]
fn a_moon_over_the_sun_fades_the_glare_around_it() {
    const WIDTH: u32 = 1024;
    const HEIGHT: u32 = 256;
    /// The view axis this far off the Sun, in degrees.
    const OFF_AXIS: f32 = 7.0;

    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("engine_moon_eclipse");
    let fixture = support::write_moon_fixture(&dir);
    let mut params = moon_params();
    params.sun_glow = 1.0;
    params.datetime.custom_year = 2024;
    params.datetime.custom_day_of_year = 99;
    params.datetime.custom_hour = 18.0 + 17.0 / 60.0;

    // The eye on the night side, swung `OFF_AXIS` out of the Earth-Sun line, so
    // the Sun sits that far from the view axis and the Moon almost with it.
    let sunward = sky_for(&params).sun_direction.normalize();
    let across = sunward.cross(glam::Vec3::Y).normalize();
    let radians = OFF_AXIS.to_radians();
    let eye = -sunward * radians.cos() + across * radians.sin();
    params.camera.latitude = eye.y.asin().to_degrees();
    params.camera.longitude = eye.x.atan2(eye.z).to_degrees();

    #[allow(clippy::cast_precision_loss)]
    let viewport = glam::Vec2::new(WIDTH as f32, HEIGHT as f32);
    let disc = moon_disc(&params, viewport);
    let sun = sun_screen_position(&params, viewport);
    println!(
        "the moon's disc is {:.1} px across at ({:.1}, {:.1}), the sun at ({:.1}, {:.1}), \
         {:.1} px apart",
        disc.radius * 2.0,
        disc.center.x,
        disc.center.y,
        sun.x,
        sun.y,
        disc.center.distance(sun)
    );
    assert!(
        disc.center.distance(sun) + 2.0 < disc.radius,
        "this framing is meant to put the Sun's disk inside the Moon's"
    );

    let harness = Harness::start(|config| {
        config.params = params;
        config.texture_paths = moon_paths(Some(fixture.clone()));
    });
    harness.next_frame();
    harness.wait_for_slot_texture("moon_texture");
    let eclipsed = harness
        .engine
        .export_pixels(WIDTH, HEIGHT)
        .expect("the engine should be able to export");

    let mut without = params;
    without.moon_brightness = 0.0;
    harness
        .engine
        .send(EngineCommand::UpdateParams(Box::new(without)));
    harness.next_frame();
    let burning = harness
        .engine
        .export_pixels(WIDTH, HEIGHT)
        .expect("the engine should be able to export");

    // Far enough out that the Moon's own mesh cannot reach, since the disc is
    // the image of the cone every one of its vertices is inside.
    let reach = disc.radius * 1.5;
    let mut dimmed = 0;
    for (index, (with, out)) in eclipsed
        .chunks_exact(4)
        .zip(burning.chunks_exact(4))
        .enumerate()
    {
        #[allow(clippy::cast_precision_loss)]
        let position = glam::Vec2::new(
            (index % WIDTH as usize) as f32,
            (index / WIDTH as usize) as f32,
        );
        if position.distance(disc.center) <= reach {
            continue;
        }
        let sum = |px: &[u8]| u32::from(px[0]) + u32::from(px[1]) + u32::from(px[2]);
        assert!(
            sum(with) <= sum(out),
            "a pixel {:.1} px from the Moon went from {out:?} to {with:?} with the Moon over \
             the Sun",
            position.distance(disc.center)
        );
        if sum(with) + 3 < sum(out) {
            dimmed += 1;
        }
    }
    println!("{dimmed} pixels outside the Moon's silhouette dimmed with the Sun covered");
    assert!(
        dimmed > 1000,
        "only {dimmed} pixels dimmed outside the Moon's silhouette"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Earthshine lifts the unlit face and nothing else.
///
/// The golden cannot see this: the floor at its default of 0.05 changes the
/// window it compares by a mean of 0.64 against a tolerance of 2.00. So it is
/// pinned here, where a count of pixels needs no tolerance: the frames with and
/// without it differ only inside the disk, and only upward.
#[test]
fn earthshine_lifts_the_unlit_face_only() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("engine_moon_earthshine");
    let fixture = support::write_moon_fixture(&dir);
    let mut params = moon_params();
    params.moon_earthshine = 0.0;

    let harness = Harness::start(|config| {
        config.params = params;
        config.texture_paths = moon_paths(Some(fixture.clone()));
    });
    harness.next_frame();
    harness.wait_for_slot_texture("moon_texture");
    let dark = harness
        .engine
        .export_pixels(512, 256)
        .expect("the engine should be able to export");

    let mut lifted = params;
    lifted.moon_earthshine = 0.3;
    harness
        .engine
        .send(EngineCommand::UpdateParams(Box::new(lifted)));
    harness.next_frame();
    let shone = harness
        .engine
        .export_pixels(512, 256)
        .expect("the engine should be able to export");

    let disc = moon_disc(&params, glam::Vec2::new(512.0, 256.0));
    let mut raised = 0;
    for (index, (before, after)) in dark.chunks_exact(4).zip(shone.chunks_exact(4)).enumerate() {
        if before == after {
            continue;
        }
        #[allow(clippy::cast_precision_loss)]
        let position = glam::Vec2::new((index % 512) as f32, (index / 512) as f32);
        assert!(
            position.distance(disc.center) <= disc.radius + 1.5,
            "earthshine changed a pixel {:.1} px from the disk's center, which is {:.1} across",
            position.distance(disc.center),
            disc.radius * 2.0
        );
        assert!(
            after[0] >= before[0] && after[1] >= before[1] && after[2] >= before[2],
            "earthshine darkened a pixel from {before:?} to {after:?}"
        );
        raised += 1;
    }
    println!("earthshine raised {raised} pixels of the disk");
    assert!(
        raised > 100,
        "only {raised} pixels changed with the earthshine floor at 0.3"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// The Milky Way
// ---------------------------------------------------------------------------

/// A camera that shows `eqj` in the sky, as far as it can be from both the
/// frame's edges and the painted globe, chosen by maximizing the smaller of
/// those two clearances over the camera's two angles.
///
/// By search, so nothing here has to know the world frame's own convention, and
/// through `sky_lens_disc`, which is the CPU's own spelling of the projection
/// rather than a new one. The frames these cases render have the Sun switched
/// off, so where it lands is not part of the choice.
fn camera_showing(
    eqj: glam::Vec3,
    sky: &sunlit_core::scene::sky::SkyState,
    params: &SceneParams,
    viewport: glam::Vec2,
) -> (f32, f32) {
    let world = sky.world_from_eqj * eqj;
    let mut best = (f32::MIN, (0.0, 0.0));
    let mut longitude = -180.0_f32;
    while longitude < 180.0 {
        let mut latitude = -85.0_f32;
        while latitude < 85.0 {
            let camera = sunlit_core::scene::camera::OrbitalCamera::new(
                longitude,
                latitude,
                sunlit_core::scene::camera::zoom_to_distance(params.camera.zoom),
            );
            let view_direction = (camera.view_matrix() * world.extend(0.0)).truncate();
            if let Some(circle) = sunlit_core::scene::sun_occlusion::sky_lens_disc(
                view_direction,
                0.0,
                params.sky_fov,
                glam::Vec2::ZERO,
                viewport,
            ) {
                let globe = sunlit_core::scene::sun_occlusion::globe_screen_circle(
                    camera.mvp_matrix(viewport.x / viewport.y),
                    camera.distance,
                    1.0,
                    camera.fov_deg,
                    viewport,
                );
                let inset = circle
                    .center
                    .x
                    .min(viewport.x - circle.center.x)
                    .min(circle.center.y)
                    .min(viewport.y - circle.center.y);
                let clearance = circle.center.distance(globe.center) - globe.radius;
                let score = inset.min(clearance);
                if score > best.0 {
                    best = (score, (longitude, latitude));
                }
            }
            latitude += 0.5;
        }
        longitude += 0.5;
    }
    best.1
}

/// A unit vector in equatorial J2000 coordinates from right ascension and
/// declination, both in degrees.
fn eqj_direction(right_ascension: f32, declination: f32) -> glam::Vec3 {
    let (ra, dec) = (right_ascension.to_radians(), declination.to_radians());
    glam::Vec3::new(dec.cos() * ra.cos(), dec.cos() * ra.sin(), dec.sin())
}

/// Where the sky lens puts an equatorial J2000 direction, in pixels.
///
/// `scene::sun_occlusion::sky_lens_disc` of a zero-width cone, which is the
/// CPU's own spelling of the projection the shader inverts rather than a new
/// one.
fn eqj_screen_position(
    eqj: glam::Vec3,
    params: &SceneParams,
    viewport: glam::Vec2,
) -> Option<glam::Vec2> {
    let sky = sky_for(params);
    let view = camera_for(params).view_matrix();
    let view_direction = (view * (sky.world_from_eqj * eqj).extend(0.0)).truncate();
    sunlit_core::scene::sun_occlusion::sky_lens_disc(
        view_direction,
        0.0,
        params.sky_fov,
        glam::Vec2::ZERO,
        viewport,
    )
    .map(|circle| circle.center)
}

/// The painted globe's own circle, for the cases that have to ignore it.
fn globe_circle(
    params: &SceneParams,
    viewport: glam::Vec2,
) -> sunlit_core::scene::sun_occlusion::ScreenCircle {
    let camera = camera_for(params);
    sunlit_core::scene::sun_occlusion::globe_screen_circle(
        camera.mvp_matrix(viewport.x / viewport.y),
        camera.distance,
        1.0,
        camera.fov_deg,
        viewport,
    )
}

/// A framing with a panorama fixture in the sky, at 512 by 256.
///
/// The datetime and camera are the caller's, through `params`; what this owns is
/// the fixture, the engine, and the wait for the texture to arrive, which
/// `TexturesReady` does not cover because the panorama is an overlay.
struct PanoramaHarness {
    harness: Harness,
    dir: std::path::PathBuf,
}

impl PanoramaHarness {
    const WIDTH: u32 = 512;
    const HEIGHT: u32 = 256;

    fn new(
        name: &str,
        params: SceneParams,
        fixture: impl FnOnce(&Path) -> std::path::PathBuf,
    ) -> Self {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
        let _ = std::fs::remove_dir_all(&dir);
        let path = fixture(&dir);
        let harness = Harness::start(|config| {
            config.preview_size = (Self::WIDTH, Self::HEIGHT);
            config.params = params;
            config.texture_paths = vec![None, None, None, Some(path)];
            config.cache_dir = Some(dir.clone());
        });
        harness.wait_for_slot_texture("milky_way_texture");
        Self { harness, dir }
    }

    fn export(&self, params: &SceneParams) -> Vec<u8> {
        self.harness
            .engine
            .send(EngineCommand::UpdateParams(Box::new(*params)));
        self.harness
            .engine
            .export_pixels(Self::WIDTH, Self::HEIGHT)
            .expect("the engine should be able to export")
    }

    /// The same framing with the layer switched off, which is what isolates
    /// what the layer drew from the globe and the clear color.
    fn export_pair(&self, params: &SceneParams) -> (Vec<u8>, Vec<u8>) {
        let on = self.export(params);
        let off = self.export(&SceneParams {
            milky_way_intensity: 0.0,
            ..*params
        });
        (on, off)
    }

    fn viewport() -> glam::Vec2 {
        #[allow(clippy::cast_precision_loss)]
        glam::Vec2::new(Self::WIDTH as f32, Self::HEIGHT as f32)
    }
}

impl Drop for PanoramaHarness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Parameters for the panorama cases: a pinned instant, a globe small enough to
/// leave sky around it, and everything else that puts light in the sky switched
/// off, so what is measured is the layer.
fn panorama_params() -> SceneParams {
    let mut params = SceneParams {
        texture_index: 0,
        sample_count: 1,
        star_intensity: 0.0,
        sun_glow: 0.0,
        moon_brightness: 0.0,
        cloud_opacity: 0.0,
        atmo_enabled: false,
        milky_way_intensity: 1.0,
        camera: sunlit_core::scene::camera::CameraParams {
            zoom: 0.6,
            ..Default::default()
        },
        ..SceneParams::default()
    };
    params.datetime.use_custom = true;
    params.datetime.custom_hour = 2.0;
    params.datetime.custom_day_of_year = 172;
    params.datetime.custom_year = 2026;
    params
}

/// How much each pixel changed between two frames, as a sum over the channels.
fn channel_differences(on: &[u8], off: &[u8]) -> Vec<u32> {
    on.chunks_exact(4)
        .zip(off.chunks_exact(4))
        .map(|(a, b)| {
            u32::from(a[0].abs_diff(b[0]))
                + u32::from(a[1].abs_diff(b[1]))
                + u32::from(a[2].abs_diff(b[2]))
        })
        .collect()
}

/// The difference-weighted centroid of everything over `floor`, and how many
/// pixels that was.
fn difference_centroid(differences: &[u32], width: u32, floor: u32) -> (glam::Vec2, u32) {
    let mut sum = glam::Vec2::ZERO;
    let mut weight = 0.0_f32;
    let mut count = 0;
    for (index, &value) in differences.iter().enumerate() {
        if value < floor {
            continue;
        }
        #[allow(clippy::cast_possible_truncation)]
        let index = index as u32;
        #[allow(clippy::cast_precision_loss)]
        let position = glam::Vec2::new((index % width) as f32, (index / width) as f32);
        #[allow(clippy::cast_precision_loss)]
        let w = value as f32;
        sum += position * w;
        weight += w;
        count += 1;
    }
    (sum / weight.max(1.0), count)
}

/// The layer draws when it is switched on, and switching it off is the whole
/// sky's difference rather than a corner's.
#[test]
fn a_panorama_fills_the_sky_and_zero_intensity_empties_it() {
    let params = panorama_params();
    let panorama = PanoramaHarness::new(
        "engine_panorama_switch",
        params,
        support::write_panorama_bands_fixture,
    );

    let (on, off) = panorama.export_pair(&params);
    let differences = channel_differences(&on, &off);
    let changed = differences.iter().filter(|&&d| d > 0).count();
    let total = differences.len();
    let circle = globe_circle(&params, PanoramaHarness::viewport());
    println!(
        "the panorama changes {changed} of {total} pixels, \
         with a globe {:.0} px across in the middle of them",
        circle.radius * 2.0
    );
    assert!(
        changed * 2 > total,
        "only {changed} of {total} pixels differ with the panorama on"
    );
}

/// The panorama's image of a sky position is where the star sprites put the
/// same position.
///
/// The strongest statement available about the direction-to-texel map, and the
/// one the round-trip probe cannot make: the probe holds the reconstruction to
/// the projection, and this holds the panorama's own texel layout to the
/// catalog path phase A checked against ephemerides. A landmark painted at
/// Sirius's coordinates has to land on Sirius's sprite.
///
/// Sirius because it is the only catalog entry brighter than magnitude -1, so a
/// limit there leaves one sprite in the whole sky. Both measurements are taken
/// against the same frame with the layer or the stars switched off, so the
/// painted globe is subtracted out rather than reasoned about.
#[test]
fn the_panorama_puts_a_landmark_where_the_star_path_puts_the_same_direction() {
    /// Sirius, right ascension and declination in degrees at J2000.
    const SIRIUS: (f32, f32) = (101.287, -16.716);

    let mut params = panorama_params();
    let direction = eqj_direction(SIRIUS.0, SIRIUS.1);
    let sky = sky_for(&params);
    let viewport = PanoramaHarness::viewport();
    let (longitude, latitude) = camera_showing(direction, &sky, &params, viewport);
    params.camera.longitude = longitude;
    params.camera.latitude = latitude;

    let expected = eqj_screen_position(direction, &params, viewport)
        .expect("Sirius is not at the view antipode in this framing");
    let circle = globe_circle(&params, viewport);
    assert!(
        expected.x > 0.0 && expected.x < viewport.x && expected.y > 0.0 && expected.y < viewport.y,
        "the framing puts Sirius at {expected:?}, which is off screen"
    );
    assert!(
        expected.distance(circle.center) > circle.radius + 30.0,
        "the framing puts Sirius {:.1} px from the center of a globe {:.1} px across",
        expected.distance(circle.center),
        circle.radius * 2.0
    );

    let panorama = PanoramaHarness::new("engine_panorama_landmark", params, |dir| {
        support::write_panorama_landmark_fixture(dir, "sirius.png", SIRIUS.0, SIRIUS.1, 4.0)
    });

    let (with_landmark, without) = panorama.export_pair(&params);
    let (landmark, lit) =
        difference_centroid(&channel_differences(&with_landmark, &without), 512, 300);

    let starry = SceneParams {
        milky_way_intensity: 0.0,
        star_intensity: 4.0,
        star_mag_limit: -1.0,
        ..params
    };
    let with_star = panorama.export(&starry);
    let (sprite, sprite_pixels) = difference_centroid(
        &channel_differences(&with_star, &without),
        PanoramaHarness::WIDTH,
        60,
    );

    println!(
        "the landmark's centroid is at {landmark:?} over {lit} pixels, \
         the sprite's at {sprite:?} over {sprite_pixels}, \
         and the lens puts the direction at {expected:?}"
    );
    assert!(lit > 50, "only {lit} pixels of the landmark are lit");
    assert!(
        (1..=400).contains(&sprite_pixels),
        "{sprite_pixels} pixels changed with one sprite in the sky"
    );
    assert!(
        landmark.distance(sprite) < 5.0,
        "the landmark is {:.1} px from the sprite for the same direction",
        landmark.distance(sprite)
    );
    assert!(
        landmark.distance(expected) < 5.0,
        "the landmark is {:.1} px from where the lens puts the direction",
        landmark.distance(expected)
    );
}

/// The wrap column is not a band of the coarsest mip.
///
/// `atan2`'s branch cut is one pixel column wide, which is 0.2 percent of a
/// golden frame: inside its outlier allowance and absent from its mean, so a
/// golden passes with the seam in it and a per-column count is what can see it.
/// The fixture does not depend on right ascension at all, so a column
/// differing from both its neighbors cannot be content, and its coarsest mip is
/// one texel holding a value far from the ramp at most declinations.
///
/// The painted globe is excluded, because its grid lines are one-pixel features
/// of exactly the shape being counted.
#[test]
fn the_wrap_column_is_not_a_band_of_the_coarsest_mip() {
    /// The branch cut is the half plane where a direction's y is zero and its x
    /// is negative, which is right ascension 180 at every declination.
    const CUT_RIGHT_ASCENSION: f32 = 180.0;

    let mut params = panorama_params();
    let sky = sky_for(&params);
    let viewport = PanoramaHarness::viewport();
    let (longitude, latitude) = camera_showing(
        eqj_direction(CUT_RIGHT_ASCENSION, 0.0),
        &sky,
        &params,
        viewport,
    );
    params.camera.longitude = longitude;
    params.camera.latitude = latitude;

    // Vacuity guard: the case says nothing unless the cut crosses the frame.
    let mut on_screen = 0;
    for declination in [-60.0_f32, -30.0, 0.0, 30.0, 60.0] {
        let Some(position) = eqj_screen_position(
            eqj_direction(CUT_RIGHT_ASCENSION, declination),
            &params,
            viewport,
        ) else {
            continue;
        };
        if position.x >= 0.0
            && position.x < viewport.x
            && position.y >= 0.0
            && position.y < viewport.y
        {
            on_screen += 1;
        }
    }
    assert!(
        on_screen >= 2,
        "the branch cut crosses the frame at only {on_screen} of the five declinations sampled"
    );

    let panorama = PanoramaHarness::new(
        "engine_panorama_seam",
        params,
        support::write_panorama_bands_fixture,
    );
    let pixels = panorama.export(&params);
    let circle = globe_circle(&params, viewport);

    let width = PanoramaHarness::WIDTH as usize;
    let height = PanoramaHarness::HEIGHT as usize;
    let value = |x: usize, y: usize| i32::from(pixels[(y * width + x) * 4]);
    let sky_pixel = |x: usize, y: usize| {
        #[allow(clippy::cast_precision_loss)]
        let position = glam::Vec2::new(x as f32, y as f32);
        position.distance(circle.center) > circle.radius + 2.0
    };
    let mut largest = 0;
    let mut worst_at = (0, 0);
    let mut anomalies = 0;
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let neighbors = [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)];
            if !sky_pixel(x, y) || !neighbors.iter().all(|&(nx, ny)| sky_pixel(nx, ny)) {
                continue;
            }
            // Second differences on both axes: the bands are smooth enough that
            // theirs is a fraction of a code value, while anything one or two
            // pixels wide has its own height in one of the two.
            let here = 2 * value(x, y);
            let across = (value(x - 1, y) + value(x + 1, y) - here).abs();
            let down = (value(x, y - 1) + value(x, y + 1) - here).abs();
            let curvature = across.max(down);
            if curvature > 6 {
                anomalies += 1;
            }
            if curvature > largest {
                largest = curvature;
                worst_at = (x, y);
            }
        }
    }
    println!(
        "the largest second difference among the sky pixels is {largest} at {worst_at:?},          and {anomalies} of them are over six"
    );
    assert!(
        largest <= 6,
        "the sky pixel at {worst_at:?} sits {largest} away from the mean of its neighbors,          and {anomalies} of them do: this panorama does not depend on right ascension and          its bands are tens of pixels wide, so nothing in it can turn over in one"
    );
}

/// The panorama rescales with the sky field of view and the painted globe does
/// not, which is what keeps the two lenses from drifting apart.
///
/// The landmark is a piece of sky of a fixed angular size, so halving the field
/// of view has to roughly quadruple its area; the globe has the camera's own 20
/// degree lens and cannot move at all.
#[test]
fn the_panorama_tracks_the_sky_field_of_view_and_the_globe_does_not() {
    const LANDMARK: (f32, f32) = (101.287, -16.716);
    /// The landmark's own angular radius in the fixture.
    const LANDMARK_RADIUS_DEGREES: f32 = 5.0;

    let mut params = panorama_params();
    // Further out than the other cases, so a landmark magnified by the narrow
    // end of the slider still has room beside the globe.
    params.camera.zoom = 0.8;
    let sky = sky_for(&params);
    let viewport = PanoramaHarness::viewport();
    // Chosen at the narrow end, where the layer is magnified most and the
    // landmark is hardest to keep in frame.
    let (longitude, latitude) = camera_showing(
        eqj_direction(LANDMARK.0, LANDMARK.1),
        &sky,
        &SceneParams {
            sky_fov: 60.0,
            ..params
        },
        viewport,
    );
    params.camera.longitude = longitude;
    params.camera.latitude = latitude;

    let panorama = PanoramaHarness::new("engine_panorama_fov", params, |dir| {
        support::write_panorama_landmark_fixture(
            dir,
            "landmark.png",
            LANDMARK.0,
            LANDMARK.1,
            LANDMARK_RADIUS_DEGREES,
        )
    });

    let measure = |sky_fov: f32| {
        let framing = SceneParams { sky_fov, ..params };
        let sky = sky_for(&framing);
        let view_direction = (camera_for(&framing).view_matrix()
            * (sky.world_from_eqj * eqj_direction(LANDMARK.0, LANDMARK.1)).extend(0.0))
        .truncate();
        let disc = sunlit_core::scene::sun_occlusion::sky_lens_disc(
            view_direction,
            LANDMARK_RADIUS_DEGREES.to_radians(),
            sky_fov,
            glam::Vec2::ZERO,
            viewport,
        )
        .expect("in front of the lens");
        let circle = globe_circle(&framing, viewport);
        assert!(
            disc.center.distance(circle.center) > circle.radius + disc.radius + 3.0,
            "at {sky_fov} degrees of sky a landmark {:.1} px across sits {:.1} px from a              globe {:.1} px across",
            disc.radius * 2.0,
            disc.center.distance(circle.center),
            circle.radius * 2.0
        );
        let (on, off) = panorama.export_pair(&framing);
        let landmark = channel_differences(&on, &off)
            .iter()
            .filter(|&&d| d > 300)
            .count();
        // On the frame with no panorama in it the sky is the clear color, whose
        // green channel is far below anything the grid texture paints.
        let globe = off.chunks_exact(4).filter(|px| px[1] >= 60).count();
        (landmark, globe)
    };

    let (wide_landmark, wide_globe) = measure(120.0);
    let (narrow_landmark, narrow_globe) = measure(60.0);
    #[allow(clippy::cast_precision_loss)]
    let landmark_ratio = narrow_landmark as f32 / wide_landmark as f32;
    #[allow(clippy::cast_precision_loss)]
    let globe_ratio = narrow_globe as f32 / wide_globe as f32;
    println!(
        "halving the sky field of view takes the landmark from {wide_landmark} px to \
         {narrow_landmark} ({landmark_ratio:.2}x) and the globe from {wide_globe} to \
         {narrow_globe} ({globe_ratio:.3}x)"
    );
    assert!(
        wide_landmark > 200,
        "the landmark is only {wide_landmark} px"
    );
    assert!(
        landmark_ratio > 3.0,
        "the landmark's area grew {landmark_ratio:.2}x, where halving the field of view \
         should be about four"
    );
    assert!(
        (globe_ratio - 1.0).abs() < 0.02,
        "the globe's area moved by {globe_ratio:.3}x, and the sky slider is not its lens"
    );
}

/// The real panorama has the galactic plane where the plane is.
///
/// The fixture cases pin the map from a direction to a texel, but the fixture
/// and the shader are written from one reading of the asset's own layout, so
/// neither can catch that reading being wrong. This can: it samples the
/// rendered sky at the galactic center, at both galactic poles, and at two
/// stretches of the plane far from the center, and the ordering it asserts is
/// the one a mirrored or transposed reading gets backwards.
///
/// One framing per sample, each with the sample 40 degrees off the view axis,
/// because the five directions span the whole sky and no single frame holds
/// them.
///
/// Skips with a printed reason where `textures/**` is still Git LFS pointers.
#[test]
fn the_real_panorama_has_the_galactic_plane_where_the_plane_is() {
    let Some(path) = real_panorama() else {
        return;
    };

    let base = panorama_params();
    let sky = sky_for(&base);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("engine_panorama_real");
    let _ = std::fs::remove_dir_all(&dir);
    let harness = Harness::start(|config| {
        config.preview_size = (PanoramaHarness::WIDTH, PanoramaHarness::HEIGHT);
        config.params = base;
        config.texture_paths = vec![None, None, None, Some(path)];
        config.cache_dir = Some(dir.clone());
    });
    harness.wait_for_slot_texture("milky_way_texture");

    let viewport = PanoramaHarness::viewport();
    let sample = |name: &str, right_ascension: f32, declination: f32| {
        let direction = eqj_direction(right_ascension, declination);
        let (longitude, latitude) = camera_showing(direction, &sky, &base, viewport);
        let mut params = base;
        params.camera.longitude = longitude;
        params.camera.latitude = latitude;
        let position = eqj_screen_position(direction, &params, viewport)
            .expect("in front of the lens at this framing");
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (x, y) = (position.x.round() as u32, position.y.round() as u32);
        assert!(
            (5..PanoramaHarness::WIDTH - 5).contains(&x)
                && (5..PanoramaHarness::HEIGHT - 5).contains(&y),
            "{name} is at ({x}, {y}), which is not a window inside the frame"
        );
        assert!(
            position.distance(globe_circle(&params, viewport).center)
                > globe_circle(&params, viewport).radius + 10.0,
            "{name} lands on the painted globe"
        );
        harness
            .engine
            .send(EngineCommand::UpdateParams(Box::new(params)));
        let pixels = harness
            .engine
            .export_pixels(PanoramaHarness::WIDTH, PanoramaHarness::HEIGHT)
            .expect("the engine should be able to export");
        // A window rather than a pixel, because the sky is Gaia photon noise
        // and one texel of it is not what is being compared.
        let mut total = 0_u32;
        let mut count = 0_u32;
        for wy in y.saturating_sub(4)..(y + 5).min(PanoramaHarness::HEIGHT) {
            for wx in x.saturating_sub(4)..(x + 5).min(PanoramaHarness::WIDTH) {
                let index = ((wy * PanoramaHarness::WIDTH + wx) * 4) as usize;
                total += u32::from(pixels[index])
                    + u32::from(pixels[index + 1])
                    + u32::from(pixels[index + 2]);
                count += 1;
            }
        }
        let mean = total / count.max(1);
        println!("  {name} at ({x}, {y}): mean {mean} of 765");
        mean
    };

    let bulge = sample("the galactic center", 266.42, -29.01);
    let north_pole = sample("the north galactic pole", 192.86, 27.13);
    let south_pole = sample("the south galactic pole", 12.86, -27.13);
    let cygnus = sample("the plane through Cygnus", 310.4, 45.3);
    let carina = sample("the plane through Carina", 160.0, -59.0);

    for (name, pole) in [
        ("the north galactic pole", north_pole),
        ("the south galactic pole", south_pole),
    ] {
        assert!(
            bulge > pole * 3,
            "the galactic center reads {bulge} and {name} {pole}, which is not this sky"
        );
        assert!(
            cygnus > pole,
            "the plane through Cygnus reads {cygnus} and {name} {pole}"
        );
        assert!(
            carina > pole,
            "the plane through Carina reads {carina} and {name} {pole}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The panorama in `textures/`, or `None` with a printed reason.
fn real_panorama() -> Option<std::path::PathBuf> {
    /// Far larger than a Git LFS pointer and far smaller than the asset.
    const MIN_BYTES: u64 = 64 * 1024;

    let Some(dir) = sunlit_core::assets::texture_loader::resolve_textures_dir(None) else {
        println!("skipping: there is no textures directory");
        return None;
    };
    let path = dir.join("milkyway_2020_4k.jxl");
    match std::fs::metadata(&path) {
        Ok(meta) if meta.len() >= MIN_BYTES => Some(path),
        Ok(meta) => {
            println!(
                "skipping: {} is {} bytes, which is a Git LFS pointer rather than the asset",
                path.display(),
                meta.len()
            );
            None
        }
        Err(e) => {
            println!("skipping: {} is not readable: {e}", path.display());
            None
        }
    }
}

/// The panorama follows the texture resolution cap, and the halving cache is
/// what serves the narrow end.
///
/// Its source is 4096 wide, which is between the widest and the narrowest of
/// the three widths the config offers, so it is the one texture where the
/// setting is a cap in both directions: 8192 loads it as it is and 2048 halves
/// it. That halving is what keeps the layer's 42.7 MiB from being the price of
/// choosing the low setting, so it is worth knowing the file is written and
/// read rather than assuming it.
///
/// Skips with a printed reason where `textures/**` is still Git LFS pointers.
#[test]
fn the_panorama_follows_the_texture_resolution_cap() {
    let Some(path) = real_panorama() else {
        return;
    };

    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("engine_panorama_cap");
    let _ = std::fs::remove_dir_all(&dir);
    let params = panorama_params();

    let width_of = |resolution: u32| {
        let harness = Harness::start(|config| {
            config.preview_size = (256, 128);
            config.params = params;
            config.texture_paths = vec![None, None, None, Some(path.clone())];
            config.cache_dir = Some(dir.clone());
            config.texture_resolution = resolution;
        });
        harness.wait_for_slot_texture("milky_way_texture");
        let report = harness
            .engine
            .memory_report()
            .expect("the engine should answer with a report");
        let texture = report
            .expected
            .iter()
            .find(|texture| texture.label == "milky_way_texture")
            .expect("the panorama is in the report once it has arrived");
        (texture.width, texture.height)
    };

    let cached = dir.join("texture_cache").join("milkyway_2020_4k.2048.png");
    assert!(
        !cached.exists(),
        "the cache directory starts empty, and {} is in it",
        cached.display()
    );

    let (wide, wide_height) = width_of(8192);
    println!("at the 8192 setting the panorama loads at {wide}x{wide_height}");
    assert_eq!(
        (wide, wide_height),
        (4096, 2048),
        "the widest setting is a cap, and the file is 4096 wide"
    );

    let (narrow, narrow_height) = width_of(2048);
    println!("at the 2048 setting the panorama loads at {narrow}x{narrow_height}");
    assert_eq!((narrow, narrow_height), (2048, 1024));
    assert!(
        cached.exists(),
        "the halving was not written to {}",
        cached.display()
    );

    // The second run at the same width reads what the first wrote, which is the
    // point of the cache and not something the width alone can show.
    let (again, _) = width_of(2048);
    assert_eq!(again, narrow);

    let _ = std::fs::remove_dir_all(&dir);
}
