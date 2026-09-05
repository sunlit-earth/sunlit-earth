//! Engine integration tests.
//!
//! These run the real engine against the real GPU pipeline, headlessly. They
//! assert behavioral invariants (a frame arrives, an unchanged scene does not
//! re-render, a changed one does) rather than pixel values, so they survive
//! adapter differences.
//!
//! An engine start costs about 1.3 seconds, almost all of it the wgpu device
//! and the seven pipelines compiled from `sphere.wgsl`, and none of it depends
//! on anything a case varies. So the cases are grouped by the configuration
//! they need, each group shares one engine for the whole run, and everything a
//! case does differ in reaches that engine as a command. What cannot be a
//! command is what a group is: the texture files behind the slots, the cloud
//! source, the sink and the clock are all read once at startup, and a case that
//! needs its own reads a `harness_of_its_own`.
//!
//! Every case takes `gpu()` first and holds it to the end, so exactly one
//! engine is rendering at any moment. The shared engines stay alive between
//! cases, which is the point of them; `Harness::reset` is what puts one back
//! the way its group expects to find it.

use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use sunlit_core::assets::mailbox::{DecodedTextureMessage, TextureMailbox};
use sunlit_core::assets::stars;
use sunlit_core::assets::texture_loader::DecodedImage;
use sunlit_core::config::QualityTier;
use sunlit_core::display::Monitor;
use sunlit_core::display::layout::DisplayMode;
use sunlit_core::engine::clock::MockClock;
use sunlit_core::engine::wallpaper_sink::{Frame, JobImages, WallpaperJob, WallpaperSink};
use sunlit_core::engine::{DISPLAY_SETTLE, EngineCommand, EngineConfig, EngineEvent, EngineHandle};
use sunlit_core::params::SceneParams;

mod support;

#[path = "../src/test_support.rs"]
mod test_support;

use test_support::ScratchDir;

static GPU_SERIAL: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Proof that this thread is the one allowed to drive an engine.
///
/// Every case holds one for its whole body. The shared engines below are
/// reachable only through a reference to it, so nothing can render while
/// another case is rendering, and a case that starts an engine of its own is
/// serialized against the shared ones too.
struct Gpu(#[allow(dead_code)] MutexGuard<'static, ()>);

/// Take the GPU, recovering it even if a previous case panicked holding it.
fn gpu() -> Gpu {
    Gpu(GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner))
}

/// How long to wait for the engine to produce something before giving up.
const TIMEOUT: Duration = Duration::from_mins(1);

/// Deterministic test parameters: the procedural grid texture, no MSAA, a
/// fixed date so the sun does not move between runs.
///
/// The four overlays are switched off rather than left at their defaults,
/// because a shared engine has fixtures behind slots an individual case may
/// know nothing about. Off here is the same frame the case would have got from
/// an engine with an empty slot, and a case about one of them switches its own
/// back on.
fn test_params() -> SceneParams {
    let mut params = SceneParams {
        texture_index: 0,
        sample_count: 1,
        // Camera mode is off for every case here, the way the goldens pin it:
        // the spikes and the ghosts reach past the glare's own cone, so a
        // default-on flare would put pixels in frames that are counting what
        // the Sun itself paints.
        sun_flare: 0.0,
        moon_brightness: 0.0,
        milky_way_intensity: 0.0,
        cloud_opacity: 0.0,
        cloud_opacity_night: 0.0,
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
    /// What `reset` puts the preview back to.
    preview_size: (u32, u32),
    /// The width the slots load at when no case has asked for another, and
    /// what the last case to ask left behind.
    texture_resolution: AtomicU32,
    default_resolution: u32,
}

impl Harness {
    fn start(configure: impl FnOnce(&mut EngineConfig)) -> Self {
        let (tx, events): (Sender<EngineEvent>, Receiver<EngineEvent>) =
            crossbeam_channel::unbounded();
        let mut config = EngineConfig::headless((512, 288));
        config.params = test_params();
        config.on_event = Arc::new(move |event| {
            let _ = tx.send(event);
        });
        configure(&mut config);
        let preview_size = config.preview_size;
        let default_resolution = config.texture_resolution;
        Self {
            engine: sunlit_core::engine::start(config)
                .expect("the harness needs a working adapter"),
            events,
            preview_size,
            texture_resolution: AtomicU32::new(default_resolution),
            default_resolution,
        }
    }

    /// Wait for the engine to finish a whole iteration, then throw away every
    /// event it has produced so far.
    ///
    /// A reply is sent while the loop is draining commands and the render comes
    /// after that drain, so two replies bracket one complete iteration:
    /// whatever the engine still owed when the first came back has been emitted
    /// by the time the second does, and the drain that follows takes all of it.
    fn settle(&self) {
        for _ in 0..2 {
            let _ = self.engine.memory_report();
        }
        while self.events.try_recv().is_ok() {}
    }

    /// Put a shared engine back the way its group's cases expect to find it.
    ///
    /// The scene is deliberately not part of this: every case sets its own
    /// before it looks at anything, and re-rendering a baseline nobody reads
    /// would cost a render per case. Neither is the texture width, because how
    /// to wait for a reload depends on what is behind the slots, which is the
    /// group's business rather than the harness's.
    fn reset(&self) {
        self.engine.send(EngineCommand::SetPreviewEnabled(true));
        self.engine.send(EngineCommand::SetPreviewSize(
            self.preview_size.0,
            self.preview_size.1,
        ));
        self.settle();
    }

    /// Put the slots back at the group's own width, and answer whether that
    /// was a change the caller has to wait out.
    fn restore_resolution(&self) -> bool {
        if self
            .texture_resolution
            .swap(self.default_resolution, Ordering::SeqCst)
            == self.default_resolution
        {
            return false;
        }
        self.engine
            .send(EngineCommand::SetTextureResolution(self.default_resolution));
        true
    }

    /// Reload the slots at `width`, remembering it so `reset` puts it back.
    fn set_texture_resolution(&self, width: u32) {
        self.texture_resolution.store(width, Ordering::SeqCst);
        self.engine.send(EngineCommand::SetTextureResolution(width));
    }

    /// Set the scene and hand back the picture it makes, rendered now.
    ///
    /// Synchronous, through the export path: the reply cannot arrive before the
    /// parameters have been applied, so there is no frame from the case before
    /// this one to mistake for this one's.
    fn picture(&self, params: &SceneParams, (width, height): (u32, u32)) -> Vec<u8> {
        self.engine
            .send(EngineCommand::UpdateParams(Box::new(*params)));
        self.export(width, height)
    }

    /// The picture at the current scene, rendered now.
    fn export(&self, width: u32, height: u32) -> Vec<u8> {
        self.engine
            .export_pixels(width, height)
            .expect("the engine should be able to export")
    }

    /// Apply the scene and wait until the engine has finished with it.
    fn settle_at(&self, params: &SceneParams) {
        self.engine
            .send(EngineCommand::UpdateParams(Box::new(*params)));
        self.settle();
    }

    /// Set the scene and hand back the preview frame of it.
    ///
    /// The engine emits a frame only when something changed, and on a shared
    /// engine the case before this one may have left this very scene behind. So
    /// the scene is applied and allowed to settle, and then the preview is
    /// switched off and on again: what that debt puts on the wire is the frame
    /// the engine is holding, which is this scene's whether it had to be drawn
    /// again or not.
    fn frame_for(&self, params: &SceneParams) -> (Vec<u8>, u32, u32) {
        self.settle_at(params);
        self.engine.send(EngineCommand::SetPreviewEnabled(false));
        self.engine.send(EngineCommand::SetPreviewEnabled(true));
        self.next_frame()
    }

    /// Set the scene and block until the frame the change itself produces.
    ///
    /// For the cases that are about whether a change produces a frame at all,
    /// which means the caller has to have left the engine on another scene.
    fn frame_after_change(&self, params: &SceneParams) -> (Vec<u8>, u32, u32) {
        self.engine
            .send(EngineCommand::UpdateParams(Box::new(*params)));
        self.next_frame()
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

    /// Ask for a wallpaper and block until the engine has finished trying.
    fn publish(&self) -> Result<String, String> {
        self.engine.send(EngineCommand::RenderWallpaperNow);
        self.wait_for_publish()
    }

    /// Block until the engine has finished a publish attempt.
    fn wait_for_publish(&self) -> Result<String, String> {
        let deadline = std::time::Instant::now() + TIMEOUT;
        while let Ok(event) = self.events.recv_deadline(deadline) {
            if let EngineEvent::WallpaperSet(result) = event {
                return result;
            }
        }
        panic!("no publish finished within {TIMEOUT:?}");
    }

    /// Send a display-change hint and wait until the engine has taken it.
    ///
    /// The round trip matters rather than the report: commands are handled in
    /// the order they were sent, so an answer to a later one is proof that the
    /// hint was read at the clock reading the case meant it to be read at.
    fn hint(&self) {
        self.engine.send(EngineCommand::DisplaysChanged);
        let _ = self.engine.memory_report();
    }

    /// Move the clock and ask the engine to look at its schedule now.
    fn advance(&self, clock: &MockClock, by: Duration) {
        clock.advance(by);
        self.engine.send(EngineCommand::Poke);
    }

    /// The next layout the engine announces, or `None` if it announces none.
    fn next_layout(&self, within: Duration) -> Option<Vec<Monitor>> {
        let deadline = std::time::Instant::now() + within;
        while let Ok(event) = self.events.recv_deadline(deadline) {
            if let EngineEvent::MonitorsChanged(monitors) = event {
                return Some(monitors);
            }
        }
        None
    }
}

/// A frame is "lit" when at least one pixel is clearly brighter than the
/// clear color (which is near-black).
fn has_lit_pixels(rgba: &[u8]) -> bool {
    rgba.chunks_exact(4)
        .any(|px| px[0] > 40 || px[1] > 40 || px[2] > 40)
}

// ---------------------------------------------------------------------------
// The shared engines
// ---------------------------------------------------------------------------

/// The size the preview quantizes to for every group here, and the size the
/// cases that read pixels export at.
const FRAME: (u32, u32) = (512, 256);

/// `test_params` with the Moon and the Milky Way switched back on.
///
/// The renderer loads an overlay's texture when the overlay is wanted and not
/// before, so an engine whose slots have to be full starts from this.
fn overlays_wanted() -> SceneParams {
    SceneParams {
        moon_brightness: SceneParams::default().moon_brightness,
        milky_way_intensity: 1.0,
        ..test_params()
    }
}

/// The fixture files the shared engines read.
///
/// One directory for the whole run, because a shared engine outlives every
/// case and the files behind its slots have to outlive it. This is the only
/// scratch directory in the file that is not dropped when a case ends: a
/// `static` is never dropped, so the tree survives the process. Everything a
/// case owns for itself is a `ScratchDir` of its own and goes away with it.
static FIXTURES: LazyLock<ScratchDir> = LazyLock::new(|| ScratchDir::new("engine_shared"));

/// The engine for every case that needs no texture file of its own.
///
/// The sink and the clock are the two things a case cannot replace on a running
/// engine, so both are here and both are steerable: the monitor list moves, the
/// refusal switches on and off, and the clock only ever goes forward, which is
/// all any case asks of it.
struct Plain {
    harness: Harness,
    sink: Arc<RecordingSink>,
    clock: Arc<MockClock>,
}

impl std::ops::Deref for Plain {
    type Target = Harness;
    fn deref(&self) -> &Harness {
        &self.harness
    }
}

static PLAIN: LazyLock<Plain> = LazyLock::new(|| {
    let sink = Arc::new(RecordingSink::new(two_screens()));
    let clock = Arc::new(MockClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let (sink_for_config, clock_for_config) = (Arc::clone(&sink), Arc::clone(&clock));
    let harness = Harness::start(move |config| {
        config.wallpaper = sink_for_config;
        config.clock = clock_for_config;
    });
    Plain {
        harness,
        sink,
        clock,
    }
});

fn plain(_gpu: &Gpu) -> &'static Plain {
    let group = &*PLAIN;
    group.harness.reset();
    group.sink.reset(two_screens());
    group.harness.engine.send(EngineCommand::SetDisplayPlan {
        mode: DisplayMode::default(),
        anchor: None,
    });
    group
}

/// The engine the display-change cases share, with no publish behind it.
///
/// Separate from [`PLAIN`] for one reason: the engine remembers whether it has
/// ever put a wallpaper on the desk, and three of these cases are about what a
/// layout change does when it has not. Nothing here ever publishes
/// successfully, so that stays true however the cases are ordered.
struct Watching {
    harness: Harness,
    sink: Arc<RecordingSink>,
    clock: Arc<MockClock>,
}

impl std::ops::Deref for Watching {
    type Target = Harness;
    fn deref(&self) -> &Harness {
        &self.harness
    }
}

static WATCHING: LazyLock<Watching> = LazyLock::new(|| {
    let sink = Arc::new(RecordingSink::new(two_screens()));
    let clock = Arc::new(MockClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let (sink_for_config, clock_for_config) = (Arc::clone(&sink), Arc::clone(&clock));
    let harness = Harness::start(move |config| {
        config.wallpaper = sink_for_config;
        config.clock = clock_for_config;
    });
    Watching {
        harness,
        sink,
        clock,
    }
});

fn watching(_gpu: &Gpu) -> &'static Watching {
    let group = &*WATCHING;
    group.harness.reset();
    group.sink.reset(two_screens());
    group
}

/// The engine with the fixture surface behind the day and night slots and a
/// cloud source behind the overlay.
///
/// The resolution cases and the cloud cases share it: both want file-backed
/// slots, neither disturbs the other, and the two together are eleven engine
/// starts in one.
struct Surface {
    harness: Harness,
    sink: Arc<RecordingSink>,
    clouds: Arc<support::FixtureClouds>,
}

impl std::ops::Deref for Surface {
    type Target = Harness;
    fn deref(&self) -> &Harness {
        &self.harness
    }
}

/// The width the fixture surface is written at, and the width the slots hold
/// when no case has asked for another.
const SURFACE_WIDTH: u32 = support::SURFACE_FIXTURE_WIDTH;

static SURFACE: LazyLock<Surface> = LazyLock::new(|| {
    let dir = FIXTURES.join("surface");
    let paths = support::write_surface_fixtures(&dir).paths();
    let sink = Arc::new(RecordingSink::new(two_screens()));
    let clouds = Arc::new(support::FixtureClouds::bands());
    let (sink_for_config, clouds_for_config, cache) =
        (Arc::clone(&sink), Arc::clone(&clouds), dir.clone());
    let harness = Harness::start(move |config| {
        config.texture_paths = paths;
        config.texture_resolution = SURFACE_WIDTH;
        config.cache_dir = Some(cache);
        config.wallpaper = sink_for_config;
        config.cloud = Some(clouds_for_config);
        // Short, because the cloud cases swap the served map and the poll is
        // what carries the new one to the slot. Every poll the swap does not
        // follow answers "unchanged" from a string compare.
        config.cloud_poll_interval = Duration::from_millis(50);
    });
    harness.wait_for_textures("the fixture surface at startup");
    Surface {
        harness,
        sink,
        clouds,
    }
});

fn surface(_gpu: &Gpu) -> &'static Surface {
    let group = &*SURFACE;
    if group.harness.restore_resolution() {
        group.harness.wait_for_textures("putting the width back");
    }
    let bands = group.clouds.serve_bands();
    wait_for_cloud_size(&group.harness, bands, "putting the cloud map back");
    group.harness.reset();
    group.sink.reset(two_screens());
    group
}

/// The engine with the Moon and a banded panorama behind their slots.
///
/// Two overlays in one engine because neither case group can see the other's:
/// the Moon cases run with `milky_way_intensity` at zero and the panorama cases
/// with `moon_brightness` at zero, both of which `test_params` already sets.
struct Sky {
    harness: Harness,
}

impl std::ops::Deref for Sky {
    type Target = Harness;
    fn deref(&self) -> &Harness {
        &self.harness
    }
}

static SKY: LazyLock<Sky> = LazyLock::new(|| {
    let dir = FIXTURES.join("sky");
    let moon = support::write_moon_fixture(&dir);
    let panorama = support::write_panorama_bands_fixture(&dir);
    let cache = dir.clone();
    let harness = Harness::start(move |config| {
        config.preview_size = FRAME;
        config.texture_paths = vec![None, None, Some(moon), Some(panorama)];
        config.cache_dir = Some(cache);
        // An overlay's texture is loaded when the overlay is wanted and not
        // before, so the engine has to start with both switched on or neither
        // slot would ever fill. Every case sets its own scene afterwards, and
        // a loaded slot stays loaded.
        config.params = overlays_wanted();
    });
    harness.wait_for_slot_texture("moon_texture");
    harness.wait_for_slot_texture("milky_way_texture");
    Sky { harness }
});

fn sky(_gpu: &Gpu) -> &'static Sky {
    let group = &*SKY;
    group.harness.reset();
    group
}

/// The engine whose panorama is one disc at Sirius and black everywhere else.
///
/// Separate from [`SKY`] because the two fixtures answer opposite questions: a
/// landmark is exactly the one-pixel-wide feature the seam case is looking for,
/// so it cannot be in the map that case reads.
struct Landmark {
    harness: Harness,
}

impl std::ops::Deref for Landmark {
    type Target = Harness;
    fn deref(&self) -> &Harness {
        &self.harness
    }
}

/// Sirius, right ascension and declination in degrees at J2000, and the
/// angular radius the fixture paints it at.
const LANDMARK: (f32, f32) = (101.287, -16.716);
const LANDMARK_RADIUS_DEGREES: f32 = 5.0;

static LANDMARK_SKY: LazyLock<Landmark> = LazyLock::new(|| {
    let dir = FIXTURES.join("landmark");
    let path = support::write_panorama_landmark_fixture(
        &dir,
        "landmark.png",
        LANDMARK.0,
        LANDMARK.1,
        LANDMARK_RADIUS_DEGREES,
    );
    let cache = dir.clone();
    let harness = Harness::start(move |config| {
        config.preview_size = FRAME;
        config.texture_paths = vec![None, None, None, Some(path)];
        config.cache_dir = Some(cache);
        config.params = overlays_wanted();
    });
    harness.wait_for_slot_texture("milky_way_texture");
    Landmark { harness }
});

fn landmark_sky(_gpu: &Gpu) -> &'static Landmark {
    let group = &*LANDMARK_SKY;
    group.harness.reset();
    group
}

#[test]
fn zero_star_intensity_leaves_catalog_pixels_at_the_clear_color() {
    const CLEAR: [u8; 4] = [1, 1, 3, 255];

    // With the atmosphere off, the only thing outside the globe is stars, so
    // every pixel the two frames disagree about is one a star painted. That is
    // what lets this assert what its name says rather than the much weaker "one
    // pixel changed": at intensity zero each of those pixels is still exactly
    // the clear color, not a dimmed star.
    let gpu = gpu();
    let harness = plain(&gpu);
    let mut params = test_params();
    params.atmo_enabled = false;
    params.star_intensity = 0.0;
    let stars_off = harness.picture(&params, FRAME);
    params.star_intensity = 1.5;
    let stars_on = harness.picture(&params, FRAME);

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
    const CLEAR: [u8; 4] = [1, 1, 3, 255];

    let gpu = gpu();
    let harness = plain(&gpu);
    let mut params = test_params();
    params.star_intensity = 2.0;
    params.star_size = 0.5;
    params.star_glow_strength = 0.0;
    params.star_mag_limit = 4.0;
    let small_stars = harness.picture(&params, FRAME);
    params.star_size = 3.0;
    let large_stars = harness.picture(&params, FRAME);

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
    const CLEAR: [u8; 4] = [1, 1, 3, 255];

    let gpu = gpu();
    let harness = plain(&gpu);
    let mut params = test_params();
    params.sky_fov = 60.0;
    let narrow_sky = harness.picture(&params, FRAME);
    params.sky_fov = 140.0;
    let wide_sky = harness.picture(&params, FRAME);

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
/// longitude picks: 160 stands a whole horizon zone and its adaptation reach
/// clear of the band, 168 puts the disk's lower edge exactly at the top of the
/// zone, 170.5 grazes the atmosphere band, 172 leaves the disk inside the band
/// with a hundredth of its light, and 176 puts it well inside the painted disc.
/// The two cases about a draw's own cull take the same family further round,
/// to 68 and 105, where the Sun is past a frame's corner rather than near the
/// limb. The atmosphere is off so that the only thing these cases can be
/// measuring is the Sun.
///
/// Those five numbers are for `FRAME`, the size these cases export at: at
/// another aspect ratio the painted silhouette is a different size and all five
/// move.
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
fn sun_off_and_on(harness: &Harness, longitude: f32) -> (Vec<u8>, Vec<u8>) {
    sun_off_and_on_framed(harness, sun_params(longitude))
}

/// The same for a framing the caller has already adjusted.
fn sun_off_and_on_framed(harness: &Harness, params: SceneParams) -> (Vec<u8>, Vec<u8>) {
    sun_off_and_on_at(harness, params, 1.5)
}

/// The same again at a glare strength of the caller's choosing, for a case
/// that needs the disk clipped white while the glare around it is not.
fn sun_off_and_on_at(harness: &Harness, params: SceneParams, glow: f32) -> (Vec<u8>, Vec<u8>) {
    let mut off = params;
    off.sun_glow = 0.0;
    let off = harness.picture(&off, FRAME);
    let mut on = params;
    on.sun_glow = glow;
    let on = harness.picture(&on, FRAME);
    (off, on)
}

/// Refraction is what could have taken this case away, and does not.
///
/// The lift is at most 0.46 of a horizon zone, which is 1.47 pixels here, and
/// it is spent long before this framing: the lifted disk's center sits 78.6
/// pixels from the globe's own center with a radius of 1.6 against a painted
/// limb at 81.35, so its whole image is inside. What the squash then does runs
/// the same way, since the flattened disk is compared against a limb moved out
/// by the same factor, which at the saturated squash here is 98.7 pixels.
#[test]
fn a_sun_behind_the_painted_globe_paints_nothing() {
    let gpu = gpu();
    let (off, on) = sun_off_and_on(plain(&gpu), 176.0);
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
    let gpu = gpu();
    let (off, on) = sun_off_and_on(plain(&gpu), 160.0);
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
    let gpu = gpu();
    let harness = plain(&gpu);
    // What each longitude adds to its own sun-off frame, summed per channel.
    // Comparing that against the other longitude's would compare two different
    // Earths; comparing each against its own leaves only the Sun.
    let warmth = |longitude: f32| {
        let (off, on) = sun_off_and_on(harness, longitude);
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

/// Pixels the Sun added more than four levels to, and the whole of the light
/// it added.
///
/// The total rather than the brightest pixel, which is what the amendment's
/// criterion 4 asks for and cannot have: the disk is drawn clipped white
/// wherever it is drawn at all, so a framing that carries any of it at all has
/// a brightest gain of 254 whatever the exposure does, and the two framings
/// below would compare equal at every setting. The gain multiplies the glare's
/// amplitude, so what it moves is how much light there is.
fn sun_light(harness: &Harness, params: SceneParams) -> (usize, u64) {
    let (off, on) = sun_off_and_on_framed(harness, params);
    let mut painted = 0;
    let mut total = 0;
    for (dark, lit) in off.chunks_exact(4).zip(on.chunks_exact(4)) {
        let gained = [0, 1, 2].map(|c| lit[c].saturating_sub(dark[c]));
        if gained.iter().any(|&value| value > 4) {
            painted += 1;
        }
        total += gained.into_iter().map(u64::from).sum::<u64>();
    }
    (painted, total)
}

/// A disk the band has taken almost all of still glares.
///
/// At longitude 172 the disk is three quarters clear of the painted limb and
/// carries 1.1 percent of its light, which is what the compressive response is
/// for: the tenth of that the square root leaves is a glare a person sees, and
/// a linear one would be a frame with a red dot in it. Measured, it paints
/// 3527 pixels, and 558 of them with the exposure gain taken out.
#[test]
fn a_sliver_of_sun_over_the_limb_still_glares() {
    let gpu = gpu();
    let (painted, _) = sun_light(plain(&gpu), sun_params(172.0));
    assert!(
        painted > 2000,
        "a sliver of Sun in the band painted only {painted} pixels"
    );
}

/// The glare peaks as the disk stands clear of the horizon zone.
///
/// Both framings carry the whole disk and all of its light, so the flux, the
/// tint and the squash are the same at each and the exposure gain is the only
/// thing between them: 3 at 168, where the disk's lower edge is at the top of
/// the zone, and 1 at 160, where it is past the adaptation reach. Measured, the
/// first adds 2.74 times what the second does rather than the gain's own 3,
/// because the brightest of the glare is clipped at both.
#[test]
fn the_glare_peaks_as_the_disk_clears_the_horizon() {
    let gpu = gpu();
    let harness = plain(&gpu);
    let (_, at_the_zone) = sun_light(harness, sun_params(168.0));
    let (_, well_clear) = sun_light(harness, sun_params(160.0));
    assert!(
        at_the_zone > well_clear * 2,
        "the glare at the top of the zone added {at_the_zone} against \
         {well_clear} for a Sun well clear of it, which is no peak at all"
    );
}

/// At `sun_horizon_boost` 1 there is no peak, which is the physical answer.
///
/// The same two framings, so what is left when the gain is 1 at both is the
/// difference between two positions on the frame: the glare falls on a
/// different part of the globe's own brightness at each, and where it is
/// clipped is where the two cannot agree exactly. Measured, that is 0.971
/// against the 2.74 the case above gets at the default.
#[test]
fn the_physical_exposure_has_no_peak() {
    let gpu = gpu();
    let harness = plain(&gpu);
    let flat = |longitude: f32| {
        let mut params = sun_params(longitude);
        params.sun_horizon_boost = 1.0;
        sun_light(harness, params).1
    };
    let at_the_zone = flat(168.0);
    let well_clear = flat(160.0);
    #[allow(clippy::cast_precision_loss)]
    let ratio = at_the_zone as f64 / well_clear as f64;
    assert!(
        (0.9..1.1).contains(&ratio),
        "with the boost at 1 the two framings should glare alike, but the one \
         at the top of the zone added {at_the_zone} against {well_clear}, a \
         ratio of {ratio:.3}"
    );
}

/// The user's own framing: 3.7 Earth radii, 140 degrees of sky, the latitude
/// and the framing the golden suite's `horizon_camera` uses.
///
/// The atmosphere is on, unlike the sun cases above, because what these
/// measure is the band the shell draws. Three longitudes matter here, and all
/// three are geometry rather than taste: at 163.35 the true Sun stands on the
/// true limb, 15.68 degrees off the view axis, while its image through the sky
/// lens is 52 pixels from the frame's center and the painted limb is 204, so
/// there is no Sun to see anywhere near the limb; at 117.66 the image sits one
/// horizon zone inside the painted limb, which takes a true Sun 57.1 degrees
/// off the axis; and at 115.93 the disk's lower edge stands on the limb.
fn horizon_params(longitude: f32) -> SceneParams {
    let mut params = test_params();
    params.datetime.custom_day_of_year = 172;
    params.camera.longitude = longitude;
    params.camera.latitude = -23.44;
    params.camera.zoom = 0.227_047_34;
    params.sky_fov = 140.0;
    params
}

/// What the Sun and its band add to a frame, as red minus blue.
///
/// Against the same frame with both switched off, so the globe's own texture
/// and the shell's own blue cancel and what is left is the light this
/// amendment moved. Red minus blue because the band and the transmitted disk
/// are red by construction and everything else the frame holds is not.
fn sunrise_excess(harness: &Harness, longitude: f32) -> i64 {
    let params = horizon_params(longitude);
    let mut dark = params;
    dark.sun_glow = 0.0;
    dark.atmo_sunrise_glow = 0.0;
    let dark = harness.picture(&dark, FRAME);
    let lit = harness.picture(&params, FRAME);
    lit.chunks_exact(4)
        .zip(dark.chunks_exact(4))
        .map(|(lit, dark)| {
            i64::from(lit[0].saturating_sub(dark[0])) - i64::from(lit[2].saturating_sub(dark[2]))
        })
        .sum()
}

/// The glow arrives with the Sun's image rather than with the true Sun.
///
/// The lobe on the Rayleigh shell is measured through the sky lens, so it
/// fires where the Sun is drawn. Measured at 512 by 256 with the lobe in the
/// true scattering angle instead, the framing with no Sun to see adds 1287
/// against the 12222 of the framing whose image stands on the limb, a ninth of
/// it: a glow that arrives well before the Sun, which is what the user saw.
/// Through the sky lens the same two are 381 and 31221, an eighty-second.
#[test]
fn the_sunrise_band_arrives_with_the_suns_image() {
    let gpu = gpu();
    let harness = plain(&gpu);
    let no_sun_to_see = sunrise_excess(harness, 163.353_15);
    let image_at_the_limb = sunrise_excess(harness, 117.658_22);
    let disk_emerged = sunrise_excess(harness, 115.931_64);
    assert!(
        no_sun_to_see * 20 < image_at_the_limb,
        "a framing whose Sun is nowhere near the painted limb still reddened it \
         by {no_sun_to_see} against the {image_at_the_limb} of one whose image \
         stands on it"
    );
    assert!(
        disk_emerged > image_at_the_limb,
        "the emerged disk added {disk_emerged}, less than the {image_at_the_limb} \
         of the framing that has no disk in it at all"
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
    fn added(harness: &Harness, params: SceneParams) -> (usize, u8) {
        let (off, on) = sun_off_and_on_framed(harness, params);
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

    let gpu = gpu();
    let harness = plain(&gpu);
    let mut framed = sun_params(68.0);
    assert_eq!(
        added(harness, framed),
        (0, 0),
        "no point of an unpanned frame is within the glare's cone here, so \
         the Sun may not touch a pixel of it"
    );

    framed.camera.offset_x = -0.9;
    let (painted, brightest) = added(harness, framed);
    assert!(
        painted > 4000 && brightest > 8,
        "the pan brings the frame's nearest pixel to 10.2 degrees from the \
         Sun, but only {painted} pixels gained more than four levels and the \
         brightest gained {brightest}"
    );
}

/// A disk the size slider has enlarged is drawn where its crescent reaches
/// the corner, which the true half degree alone would leave outside.
///
/// The disk draw culls itself on a cone of its own angular radius, and
/// `sun_size` multiplies that radius, so the cone the cull measures has to
/// carry the size the draw does. At longitude 105 the Sun stands 76.62
/// degrees off the view axis against a 76.11 degree corner: the true half
/// degree puts the whole disk outside the frame and eight times it puts a
/// crescent of it inside the top left one. The glare is the same either way,
/// and a quarter of the glow is where the core is already clipped white
/// (`SUN_CORE_GAIN` is 4) while the bloom around it is not, so a gain of more
/// than 200 levels is the disk and nothing else: measured, the crescent is 32
/// pixels and the glare without it gains at most 52.
#[test]
fn an_enlarged_disk_reaching_the_frame_corner_is_drawn() {
    let gpu = gpu();
    let mut framed = sun_params(105.0);
    framed.sun_size = 8.0;
    framed.sun_rays = 0.0;
    let (off, on) = sun_off_and_on_at(plain(&gpu), framed, 0.25);
    let clipped = off
        .chunks_exact(4)
        .zip(on.chunks_exact(4))
        .filter(|(dark, lit)| {
            [0, 1, 2]
                .iter()
                .any(|&c| lit[c].saturating_sub(dark[c]) > 200)
        })
        .count();
    assert!(
        clipped > 12,
        "the enlarged disk's crescent reaches inside the frame's corner, but \
         only {clipped} pixels there gained more than 200 levels"
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
    let gpu = gpu();
    let harness = plain(&gpu);
    let (width, height) = FRAME;
    let centered = harness.picture(&panned_params(0.0), FRAME);
    let panned = harness.picture(&panned_params(PAN), FRAME);

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
/// For the cases that have an engine of their own: the shared surface group
/// reads `support::write_surface_fixtures` instead. Small and bright rather
/// than realistic, and deletable, which is what the mailbox cases need.
struct TextureFixtures {
    dir: ScratchDir,
}

impl TextureFixtures {
    /// Fixtures at a chosen width. Not one of the widths the combo box offers,
    /// because the renderer takes any width as a cap and the three on offer are
    /// the config's business.
    fn with_width(name: &str, width: u32) -> Self {
        let dir = ScratchDir::new(name);
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

    fn path(&self) -> &Path {
        self.dir.path()
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
        harness.drained_frame(Duration::from_millis(400)).is_none(),
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
        harness.drained_frame(Duration::from_millis(400)).is_none(),
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

/// One fabricated monitor.
fn screen(id: &str, x: i32, width: u32, height: u32, primary: bool) -> Monitor {
    Monitor {
        id: id.to_owned(),
        label: id.to_owned(),
        x,
        y: 0,
        width,
        height,
        primary,
    }
}

/// A sink that reports a fabricated layout and keeps what it was handed.
///
/// This is what proves the engine asks for the right images in each mode with
/// no display anywhere, so it runs on Windows and macOS as well as Linux. It
/// also stands in for the sink that cannot publish at all, which is what
/// `SystemWallpaper` is on a Linux desktop the table does not know: `refuse`
/// switches that on and `reset` switches it back off.
struct RecordingSink {
    /// Behind a lock because a display-change case moves the layout under a
    /// running engine, which is the whole thing those cases are about.
    monitors: Mutex<Vec<Monitor>>,
    /// How often the engine has asked. A recheck that changes nothing leaves no
    /// other trace, so this is what says it happened at all.
    queries: std::sync::atomic::AtomicUsize,
    /// What `check_supported` answers; `None` accepts.
    refusal: Mutex<Option<String>>,
    published: Mutex<Vec<Publication>>,
}

/// What one publish came out as, in the terms the assertions are written in.
struct Publication {
    mode: DisplayMode,
    anchor: usize,
    /// One entry per monitor: its size, or `None` where the mode left it alone.
    images: Vec<Option<(u32, u32)>>,
    /// The canvas, where the publish spanned.
    canvas: Option<(u32, u32)>,
    /// Each screen's finished picture, cut where the publish spanned.
    frames: Vec<Option<Arc<Frame>>>,
    /// How many distinct pixel buffers the publish actually rendered.
    renders: usize,
}

impl RecordingSink {
    /// The reason a refusing recording sink gives.
    const REFUSED: &str = "this recording sink refuses on purpose";

    fn new(monitors: Vec<Monitor>) -> Self {
        Self {
            monitors: Mutex::new(monitors),
            queries: std::sync::atomic::AtomicUsize::new(0),
            refusal: Mutex::new(None),
            published: Mutex::new(Vec::new()),
        }
    }

    /// Forget everything the previous case did to this sink.
    fn reset(&self, monitors: Vec<Monitor>) {
        self.set_monitors(monitors);
        *self.refusal.lock().expect("the recording is poisoned") = None;
        self.publications().clear();
        self.queries.store(0, std::sync::atomic::Ordering::SeqCst);
    }

    /// Answer every later `check_supported` with a refusal.
    fn refuse(&self) {
        *self.refusal.lock().expect("the recording is poisoned") = Some(Self::REFUSED.to_owned());
    }

    /// Move the layout under the running engine.
    fn set_monitors(&self, monitors: Vec<Monitor>) {
        *self.monitors.lock().expect("the recording is poisoned") = monitors;
    }

    fn queries(&self) -> usize {
        self.queries.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn publications(&self) -> std::sync::MutexGuard<'_, Vec<Publication>> {
        self.published.lock().expect("the recording is poisoned")
    }
}

impl WallpaperSink for RecordingSink {
    fn check_supported(&self) -> Result<(), String> {
        match &*self.refusal.lock().expect("the recording is poisoned") {
            Some(reason) => Err(reason.clone()),
            None => Ok(()),
        }
    }

    fn monitors(&self) -> Result<Vec<Monitor>, String> {
        self.queries
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(self
            .monitors
            .lock()
            .expect("the recording is poisoned")
            .clone())
    }

    fn publish(&self, job: &WallpaperJob) -> Result<String, String> {
        let mut images = Vec::new();
        let mut frames = Vec::new();
        let mut distinct: Vec<*const u8> = Vec::new();
        for index in 0..job.monitors.len() {
            let image = job.image_for(index)?;
            images.push(image.as_ref().map(|frame| {
                assert!(frame.is_well_formed(), "a malformed frame reached the sink");
                (frame.width, frame.height)
            }));
            frames.push(image);
        }
        // Identity rather than equality: two screens of one size are meant to
        // share the buffer, and two equal buffers would not prove they did.
        if let Some(canvas) = job.canvas() {
            distinct.push(canvas.pixels.as_ptr());
        } else if let JobImages::PerMonitor(frames) = &job.images {
            for frame in frames.iter().flatten() {
                let ptr = frame.pixels.as_ptr();
                if !distinct.contains(&ptr) {
                    distinct.push(ptr);
                }
            }
        }
        self.publications().push(Publication {
            mode: job.mode,
            anchor: job.anchor,
            images,
            canvas: job.canvas().map(|frame| (frame.width, frame.height)),
            frames,
            renders: distinct.len(),
        });
        Ok(String::new())
    }
}

/// Publish once with a fabricated layout and a mode, and answer with what the
/// sink was handed.
fn publish_plan(
    group: &Plain,
    monitors: Vec<Monitor>,
    mode: DisplayMode,
    anchor: Option<&str>,
) -> Publication {
    publish_plan_with(group, monitors, mode, anchor, test_params())
}

/// The same, with the scene said out loud, for the cases that compare pixels.
fn publish_plan_with(
    group: &Plain,
    monitors: Vec<Monitor>,
    mode: DisplayMode,
    anchor: Option<&str>,
    params: SceneParams,
) -> Publication {
    group.sink.set_monitors(monitors);
    group.engine.send(EngineCommand::SetDisplayPlan {
        mode,
        anchor: anchor.map(ToOwned::to_owned),
    });
    group
        .engine
        .send(EngineCommand::UpdateParams(Box::new(params)));
    publish_once(group)
}

/// Ask for a wallpaper, wait for it, and take the one publish it made.
fn publish_once(group: &Plain) -> Publication {
    group.sink.publications().clear();
    group
        .publish()
        .expect("publishing to a recording sink cannot fail");
    let mut published = group.sink.publications();
    assert_eq!(published.len(), 1, "exactly one publish was asked for");
    published.pop().expect("the one publish")
}

/// Two screens side by side, the left one primary.
fn two_screens() -> Vec<Monitor> {
    vec![
        screen("A", 0, 320, 192, true),
        screen("B", 320, 320, 192, false),
    ]
}

/// The single-monitor identity: what every existing config describes, and what
/// must come out of this feature unchanged.
///
/// Byte for byte rather than by size, because a size is the one thing the
/// framing this feature derives cannot move. `ExportPixels` is the path this
/// feature replaced, unaltered: `prepare_export` and then `export_image` with
/// the scene's own parameters, which is what `render_wallpaper_pixels` was. So
/// what the comparison holds the publish against is the wallpaper the build
/// before this one would have written for the same screen.
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
/// is on purpose: departure 9 in the plan.
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
fn picture(published: &Publication, index: usize) -> &Frame {
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
fn compare(a: &Frame, b: &Frame) -> (f64, f64) {
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
fn globe(frame: &Frame) -> (f64, f64, f64) {
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
    let gpu = gpu();
    let group = plain(&gpu);
    group.sink.set_monitors(two_screens());
    group.engine.send(EngineCommand::SetDisplayPlan {
        mode: DisplayMode::OneScreen,
        anchor: Some("a-screen-that-went-away".to_owned()),
    });

    let note = group.publish().expect("falling back is not a failure");
    assert!(note.contains('A'), "{note}");
    assert_eq!(group.sink.publications()[0].anchor, 0);
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
/// ask for one. Before the engine resolved it against the adapter, that reached
/// `create_render_textures` and killed the engine thread with a wgpu validation
/// error: the window came up, IPC answered, and no frame ever arrived. The
/// startup half is why this has an engine of its own: the count has to be in
/// the configuration the renderer is built from.
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

    harness.drained_frame(Duration::from_millis(200));
    let (rgba, _, _) = harness.frame_after_change(&SceneParams {
        sample_count: 64,
        // Change something visible too, so the frame is not suppressed by the
        // dirty check once the count resolves back to what it was.
        cloud_opacity: 0.1,
        ..test_params()
    });
    assert!(has_lit_pixels(&rgba));
}

/// The day and night maps blended, which is the mode that needs both
/// file-backed slots and the composite bind group built from them.
fn blend_params() -> SceneParams {
    SceneParams {
        texture_index: 3,
        ..test_params()
    }
}

#[test]
fn a_resolution_switch_reloads_the_textures_in_both_directions() {
    let gpu = gpu();
    let harness = surface(&gpu);
    let (rgba, _, _) = harness.frame_for(&blend_params());
    assert!(
        has_lit_pixels(&rgba),
        "the globe should be visible at first"
    );

    // Down: the textures in memory are destroyed and the halved ones loaded.
    harness.set_texture_resolution(SURFACE_WIDTH / 4);
    harness.wait_for_textures("after switching down");
    let (rgba, _, _) = harness.next_frame();
    assert!(
        has_lit_pixels(&rgba),
        "a frame after the switch must come from the new textures, not from nothing"
    );

    // Up again: the same path in reverse, which is the one that would break if
    // the purge left a destroyed texture behind in a bind group.
    harness.set_texture_resolution(SURFACE_WIDTH);
    harness.wait_for_textures("after switching back up");
    let (rgba, _, _) = harness.next_frame();
    assert!(has_lit_pixels(&rgba));
}

/// The switch is idempotent, so a client that re-sends the current width (the
/// reset and load-defaults callbacks both do) costs nothing.
#[test]
fn a_switch_to_the_current_resolution_does_nothing() {
    let gpu = gpu();
    let harness = surface(&gpu);
    harness.settle_at(&blend_params());

    harness.set_texture_resolution(SURFACE_WIDTH);
    assert!(
        harness.drained_frame(Duration::from_millis(400)).is_none(),
        "a switch to the width already in force must not re-render"
    );
}

/// Three purges with no reload in between, ending on the last width.
///
/// The run loop drains every queued command before it ticks, and a reload is
/// only spawned from inside `render`, so all three purges here happen before
/// the first spawn and no decode is ever in flight during them. That makes this
/// a test of the purge being repeatable and of the last command winning, not of
/// the stale-arrival ordering; `a_stale_decode_must_not_replace_the_texture_that_superseded_it`
/// is that one.
#[test]
fn switches_in_quick_succession_end_on_the_last_one() {
    let gpu = gpu();
    let harness = surface(&gpu);
    harness.settle_at(&blend_params());

    for width in [SURFACE_WIDTH / 2, SURFACE_WIDTH, SURFACE_WIDTH / 4] {
        harness.set_texture_resolution(width);
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
/// when the guard is removed. An engine of its own for the mailbox, which is
/// read once when the engine is built.
#[test]
fn a_stale_decode_must_not_replace_the_texture_that_superseded_it() {
    const WIDE: u32 = 256;
    const DAY_SLOT: usize = 1;

    let _gpu = gpu();
    let fixtures = TextureFixtures::with_width("engine_resolution_stale", WIDE);
    let mailbox = TextureMailbox::new(fixtures.paths().len() + 2);
    let injected = mailbox.clone();
    let harness = Harness::start(|config| {
        config.texture_paths = fixtures.paths();
        config.texture_resolution = WIDE;
        config.cache_dir = Some(fixtures.path().to_path_buf());
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
    harness.set_texture_resolution(WIDE / 2);
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
/// session. The assertion runs on the engine thread before it reports an
/// adapter, so the caller gets an error instead of a handle rather than a
/// thread that quietly died, and no device is ever created.
#[test]
fn a_mailbox_that_does_not_match_the_slot_count_is_refused() {
    let mut config = EngineConfig::headless((64, 64));
    // Three file-backed paths need five slots: the grid, all three of them,
    // the clouds.
    config.mailbox = Some(TextureMailbox::new(3));
    let Err(error) = sunlit_core::engine::start(config) else {
        panic!("a mailbox with the wrong slot count must not produce a handle");
    };
    assert!(
        error.contains("before it could report its adapter"),
        "unexpected error: {error}"
    );
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

    let _gpu = gpu();
    let fixtures = TextureFixtures::with_width("engine_resolution_stale_alone", WIDE);
    let mailbox = TextureMailbox::new(fixtures.paths().len() + 2);
    let injected = mailbox.clone();
    let harness = Harness::start(|config| {
        config.texture_paths = fixtures.paths();
        config.texture_resolution = WIDE;
        config.cache_dir = Some(fixtures.path().to_path_buf());
        config.mailbox = Some(injected);
        config.params = SceneParams {
            texture_index: 1,
            atmo_enabled: false,
            ..test_params()
        };
    });
    harness.wait_for_textures("at startup");
    harness.set_texture_resolution(WIDE / 2);
    harness.wait_for_textures("after the switch");
    harness.drained_frame(Duration::from_millis(200));

    mailbox.post(decoded(DAY_SLOT, WIDE, 0, 0));
    harness.engine.send(EngineCommand::Poke);
    assert!(
        harness.drained_frame(Duration::from_millis(400)).is_none(),
        "a discarded arrival must not reach the GPU, and so must not produce a frame"
    );
}

/// A resolution change while the first load is still running converges.
///
/// The purge here happens with a decode genuinely in flight, which the
/// quick-succession case above cannot reach. The in-flight decode's post is
/// discarded when it arrives; what must still happen is the reload, and the only
/// evidence that it did is the slot becoming ready at all. An engine of its own,
/// because the load it interrupts is the one the engine starts with.
#[test]
fn a_switch_while_the_first_load_is_running_still_converges() {
    const WIDE: u32 = 1024;

    let _gpu = gpu();
    let fixtures = TextureFixtures::with_width("engine_resolution_midload", WIDE);
    let harness = Harness::start(|config| {
        config.texture_paths = fixtures.paths();
        config.texture_resolution = WIDE;
        config.cache_dir = Some(fixtures.path().to_path_buf());
        config.params = SceneParams {
            texture_index: 1,
            atmo_enabled: false,
            ..test_params()
        };
    });
    harness.wait_for_status(|text| !text.is_empty(), "the first load should start");
    harness.set_texture_resolution(WIDE / 4);

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
    let gpu = gpu();
    let group = surface(&gpu);
    group
        .sink
        .set_monitors(vec![screen("only", 0, 64, 32, true)]);
    group.settle_at(&blend_params());
    assert!(
        group.sink.publications().is_empty(),
        "nothing has asked for a wallpaper yet"
    );

    // Both commands are handled before the engine ticks, so the purge has
    // already emptied the slots when the publish is asked for.
    group.set_texture_resolution(SURFACE_WIDTH / 4);
    group.engine.send(EngineCommand::RenderWallpaperNow);

    let deadline = std::time::Instant::now() + TIMEOUT;
    let mut ready_first = None;
    while let Ok(event) = group.events.recv_deadline(deadline) {
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
                assert_eq!(
                    group.sink.publications().len(),
                    1,
                    "exactly one frame should be published"
                );
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

/// An asset in `textures/`, or the reason it is not usable.
///
/// `textures/**` is Git LFS, so a checkout without the objects holds pointer
/// files of a couple of hundred bytes, which exist as far as anything that only
/// asks about existence is concerned. Size is what tells the two apart, the same
/// check the guest staging in the xtask makes.
fn real_asset(name: &str) -> Result<std::path::PathBuf, String> {
    /// Smaller than any of the assets and far larger than an LFS pointer.
    const MIN_BYTES: u64 = 64 * 1024;

    let dir = sunlit_core::assets::texture_loader::resolve_textures_dir(None)
        .ok_or_else(|| "there is no textures directory".to_owned())?;
    let path = dir.join(name);
    match std::fs::metadata(&path) {
        Ok(meta) if meta.len() >= MIN_BYTES => Ok(path),
        Ok(meta) => Err(format!(
            "{name} is {} bytes, which is a Git LFS pointer rather than the asset",
            meta.len()
        )),
        Err(e) => Err(format!("{name} is not readable: {e}")),
    }
}

/// The repository's real surface and Moon assets, if this checkout has them.
fn real_textures() -> Result<Vec<Option<std::path::PathBuf>>, String> {
    [
        "world.topo.200405.jxl",
        "BlackMarble_2016.jxl",
        "lroc_color_poles_1k.jxl",
    ]
    .into_iter()
    .map(|name| real_asset(name).map(Some))
    .collect()
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
///
/// An engine of its own, and the slowest case in the file: the two 8K sources
/// are decoded once each, which is about six seconds apiece, and no shared
/// engine may carry them because every case that shares it would pay for them.
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

    let _gpu = gpu();
    let cache = ScratchDir::new("engine_resolution_memory");

    // Build the narrow copies before measuring anything. Otherwise the switch
    // decodes both 8K sources one last time to make them, and those two 128 MiB
    // buffers are freed but possibly still held by the allocator when the second
    // snapshot is taken, which would hide the very thing being measured.
    sunlit_core::assets::texture_loader::register_jxl_hook();
    for path in paths.iter().flatten() {
        sunlit_core::assets::texture_cache::load_at_resolution(path, NARROW, Some(cache.path()))
            .expect("build the narrow copy");
    }

    let cache_dir = cache.path().to_path_buf();
    let harness = Harness::start(|config| {
        config.texture_paths = paths.clone();
        config.texture_resolution = WIDE;
        config.cache_dir = Some(cache_dir);
        config.params = blend_params();
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

    fn set_offline(&self, offline: bool) {
        self.offline
            .store(offline, std::sync::atomic::Ordering::SeqCst);
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

/// The width the variant cases start and end at, and the one they switch to.
const VARIANT_WIDE: u32 = 8192;
const VARIANT_NARROW: u32 = 2048;

/// The engine whose cloud source serves a different image per variant.
struct Variant {
    harness: Harness,
    source: Arc<VariantCloud>,
}

impl std::ops::Deref for Variant {
    type Target = Harness;
    fn deref(&self) -> &Harness {
        &self.harness
    }
}

static VARIANT: LazyLock<Variant> = LazyLock::new(|| {
    let dir = FIXTURES.join("variant");
    std::fs::create_dir_all(&dir).expect("create the variant cache directory");
    let source = Arc::new(VariantCloud::new(VARIANT_WIDE));
    let cloud = Arc::clone(&source) as Arc<dyn sunlit_core::assets::cloud_source::CloudSource>;
    let harness = Harness::start(move |config| {
        config.texture_resolution = VARIANT_WIDE;
        config.cloud = Some(cloud);
        config.cache_dir = Some(dir);
    });
    wait_for_cloud_size(
        &harness,
        VariantCloud::image_size(VARIANT_WIDE),
        "at startup",
    );
    Variant { harness, source }
});

fn variant(_gpu: &Gpu) -> &'static Variant {
    let group = &*VARIANT;
    group.source.set_offline(false);
    if group.harness.restore_resolution() {
        wait_for_cloud_size(
            &group.harness,
            VariantCloud::image_size(VARIANT_WIDE),
            "putting the variant back",
        );
    }
    group.harness.reset();
    group
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
    let gpu = gpu();
    let group = variant(&gpu);
    let before = group.engine.memory_report().expect("a report");
    let fetches = group.source.fetches();
    assert_eq!(
        group.source.retarget_widths()[0],
        VARIANT_WIDE,
        "construction points the source at the configured variant"
    );

    group.set_texture_resolution(VARIANT_NARROW);
    wait_for_cloud_size(
        group,
        VariantCloud::image_size(VARIANT_NARROW),
        "after the switch",
    );
    let after = group.engine.memory_report().expect("a report");

    assert_eq!(
        *group
            .source
            .retarget_widths()
            .last()
            .expect("the switch retargeted the source"),
        VARIANT_NARROW
    );

    // One cloud texture, at the new size: the replacement went through the
    // slot rather than beside it.
    let sizes: Vec<(u32, u32)> = after
        .expected
        .iter()
        .filter(|texture| texture.label == "cloud_texture")
        .map(|texture| (texture.width, texture.height))
        .collect();
    assert_eq!(sizes, [VariantCloud::image_size(VARIANT_NARROW)]);

    // And the old one was actually freed, not merely forgotten. Skipped where
    // the backend keeps no texture counter; D3D12 and Vulkan both do.
    let (Ok(measured_before), Ok(measured_after)) = (
        u64::try_from(before.counters.texture_bytes),
        u64::try_from(after.counters.texture_bytes),
    ) else {
        panic!("wgpu reported negative texture memory");
    };
    println!(
        "wgpu texture bytes: {:.2} MiB before, {:.2} MiB after, {} fetches before the switch",
        mib(measured_before),
        mib(measured_after),
        fetches
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
/// current, and nothing in production calls `post_cached` after startup.
#[test]
fn a_switch_back_to_a_cached_variant_shows_it_again() {
    let gpu = gpu();
    let group = variant(&gpu);

    group.set_texture_resolution(VARIANT_NARROW);
    wait_for_cloud_size(
        group,
        VariantCloud::image_size(VARIANT_NARROW),
        "after switching down",
    );
    let downloaded = group.source.fetches();

    // Both entries are now on disk and neither has gone stale, so the switch
    // back is answered with a 304 and has to fall back to what it cached.
    group.set_texture_resolution(VARIANT_WIDE);
    wait_for_cloud_size(
        group,
        VariantCloud::image_size(VARIANT_WIDE),
        "after switching back up",
    );
    assert_eq!(
        group.source.fetches(),
        downloaded,
        "the switch back is served from disk, not downloaded again"
    );
}

/// A switch must not empty the cloud slot: a cloudless globe while a download
/// runs is a worse picture than one at the previous variant, and offline the
/// gap would never close.
#[test]
fn a_switch_keeps_the_old_cloud_texture_until_the_new_one_lands() {
    /// A width no earlier case has cached, so nothing on disk can answer for it
    /// while the source is offline.
    const UNCACHED: u32 = 4096;

    let gpu = gpu();
    let group = variant(&gpu);
    let showing = VariantCloud::image_size(VARIANT_WIDE);

    group.source.set_offline(true);
    group.set_texture_resolution(UNCACHED);

    // The retarget is a command, so a reply to a later one proves it was taken;
    // what follows has to be given a few of the engine's own ticks, because the
    // assertion is that nothing happens.
    let deadline = std::time::Instant::now() + CHANGE_TIMEOUT;
    while group.source.retarget_widths().last() != Some(&UNCACHED) {
        assert!(
            std::time::Instant::now() < deadline,
            "the switch should still have reached the cloud pipeline: {:?}",
            group.source.retarget_widths()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(NOTHING_HAPPENS_IN);

    let report = group.engine.memory_report().expect("a report");
    let sizes: Vec<(u32, u32)> = report
        .expected
        .iter()
        .filter(|texture| texture.label == "cloud_texture")
        .map(|texture| (texture.width, texture.height))
        .collect();
    assert_eq!(
        sizes,
        [showing],
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
    let gpu = gpu();
    let harness = surface(&gpu);
    harness.settle_at(&blend_params());

    let report = harness
        .engine
        .memory_report()
        .expect("the engine should answer with a report");
    println!("{report}");

    assert_eq!(expected_widths(&report, "day_texture"), [SURFACE_WIDTH]);
    assert_eq!(expected_widths(&report, "night_texture"), [SURFACE_WIDTH]);
    assert_eq!(
        expected_widths(&report, "grid_texture").len(),
        1,
        "the procedural grid is always resident"
    );
    // The preview target, at the size the harness asked for.
    assert_eq!(expected_widths(&report, "render_texture"), [FRAME.0]);
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
    let gpu = gpu();
    let harness = surface(&gpu);
    harness.settle_at(&blend_params());

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
    const NARROW: u32 = SURFACE_WIDTH / 8;

    let gpu = gpu();
    let harness = surface(&gpu);
    harness.settle_at(&blend_params());

    let before = harness.engine.memory_report().expect("a report");
    assert_eq!(expected_widths(&before, "day_texture"), [SURFACE_WIDTH]);

    harness.set_texture_resolution(NARROW);
    harness.wait_for_textures("after switching down");
    harness.settle();

    let after = harness.engine.memory_report().expect("a report");
    println!("{after}");
    assert_eq!(expected_widths(&after, "day_texture"), [NARROW]);
    assert_eq!(expected_widths(&after, "night_texture"), [NARROW]);
    assert!(
        !after
            .expected
            .iter()
            .any(|texture| texture.label.ends_with("_texture")
                && texture.width == SURFACE_WIDTH
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
    let gpu = gpu();
    let harness = plain(&gpu);
    harness.settle_at(&test_params());
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
    // Back on, since `test_params` switches every overlay off.
    params.moon_brightness = SceneParams::default().moon_brightness;
    // Earthshine well above the clear color, so the unlit face is part of what
    // "only adds light" is measured over rather than a wash against the sky.
    params.moon_earthshine = 0.2;
    params
}

/// The Moon adds light to a dark sky and takes none away.
///
/// With nothing else drawn, every pixel the Moon touches can only get brighter,
/// which is the shape `sun_off_and_on` uses: one engine, one `UpdateParams`, an
/// off frame against an on frame.
#[test]
fn a_moon_on_the_night_sky_only_adds_light() {
    let gpu = gpu();
    let harness = sky(&gpu);
    let params = moon_params();
    let on = harness.picture(&params, FRAME);

    let mut without = params;
    without.moon_brightness = 0.0;
    let off = harness.picture(&without, FRAME);

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
}

/// A Moon switched off and a Moon with no texture behind it are the same
/// picture, which is what makes the missing asset a non-event.
#[test]
fn a_switched_off_moon_and_a_missing_texture_draw_the_same_frame() {
    let gpu = gpu();
    let params = moon_params();

    // The one case that holds two engines at once, and it needs them because
    // the difference it is about is a configuration rather than a parameter:
    // the shared sky engine has a Moon behind its slot and the plain one has
    // nothing behind any of them.
    let mut off = params;
    off.moon_brightness = 0.0;
    let with_texture = sky(&gpu).picture(&off, FRAME);
    let without_texture = plain(&gpu).picture(&params, FRAME);

    let differing = with_texture
        .chunks_exact(4)
        .zip(without_texture.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(
        differing, 0,
        "{differing} pixels differ between a switched-off Moon and a missing one"
    );
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
    let (width, height) = FRAME;

    let gpu = gpu();
    let harness = sky(&gpu);
    let mut params = moon_params();
    let direction = sky_for(&params).moon_position.normalize();
    params.camera.latitude = direction.y.asin().to_degrees();
    params.camera.longitude = direction.x.atan2(direction.z).to_degrees();

    #[allow(clippy::cast_precision_loss)]
    let viewport = glam::Vec2::new(width as f32, height as f32);
    assert_eq!(
        moon_placement(&params, viewport).disc,
        None,
        "this framing is the one where the disc is empty, or it measures nothing"
    );

    let behind = harness.picture(&params, FRAME);
    let mut without = params;
    without.moon_brightness = 0.0;
    let off = harness.picture(&without, FRAME);

    let differing = behind
        .chunks_exact(4)
        .zip(off.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(
        differing, 0,
        "{differing} pixels differ between a Moon behind the camera and no Moon at all"
    );
}

/// The Moon's texture lands in the slot the layout reserves for it, at the
/// width of the file behind it.
#[test]
fn the_moon_texture_lands_in_its_own_slot() {
    let gpu = gpu();
    let harness = sky(&gpu);
    let report = harness
        .engine
        .memory_report()
        .expect("the engine should answer with a report");
    assert_eq!(
        expected_widths(&report, "moon_texture"),
        [support::MOON_FIXTURE_WIDTH]
    );
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

    let gpu = gpu();
    let harness = sky(&gpu);
    let mut params = moon_params();
    // No earthshine, so the unlit face is black and the threshold is a
    // question about sunlight rather than about the floor.
    params.moon_earthshine = 0.0;

    #[allow(clippy::cast_precision_loss)]
    let viewport = glam::Vec2::new(WIDTH as f32, HEIGHT as f32);
    for (doy, hour) in INSTANTS {
        let mut at = params;
        at.datetime.custom_day_of_year = doy;
        at.datetime.custom_hour = f32::from(u16::try_from(hour).expect("a small hour"));
        let pixels = harness.picture(&at, (WIDTH, HEIGHT));

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

    let gpu = gpu();
    let harness = sky(&gpu);
    let mut params = moon_params();
    params.moon_earthshine = 0.0;
    params.datetime.custom_day_of_year = 199;
    params.datetime.custom_hour = 16.0;
    let pixels = harness.picture(&params, (WIDTH, HEIGHT));

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

    let gpu = gpu();
    let harness = sky(&gpu);
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

    let eclipsed = harness.picture(&params, (WIDTH, HEIGHT));
    let mut without = params;
    without.moon_brightness = 0.0;
    let burning = harness.picture(&without, (WIDTH, HEIGHT));

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
}

/// Earthshine lifts the unlit face and nothing else.
///
/// The golden cannot see this: the floor at its default of 0.05 changes the
/// window it compares by a mean of 0.64 against a tolerance of 2.00. So it is
/// pinned here, where a count of pixels needs no tolerance: the frames with and
/// without it differ only inside the disk, and only upward.
#[test]
fn earthshine_lifts_the_unlit_face_only() {
    let gpu = gpu();
    let harness = sky(&gpu);
    let mut params = moon_params();
    params.moon_earthshine = 0.0;
    let dark = harness.picture(&params, FRAME);

    let mut lifted = params;
    lifted.moon_earthshine = 0.3;
    let shone = harness.picture(&lifted, FRAME);

    #[allow(clippy::cast_precision_loss)]
    let disc = moon_disc(&params, glam::Vec2::new(FRAME.0 as f32, FRAME.1 as f32));
    let mut raised = 0;
    let width = FRAME.0 as usize;
    for (index, (before, after)) in dark.chunks_exact(4).zip(shone.chunks_exact(4)).enumerate() {
        if before == after {
            continue;
        }
        #[allow(clippy::cast_precision_loss)]
        let position = glam::Vec2::new((index % width) as f32, (index / width) as f32);
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
///
/// Coarse then fine: four degrees over the whole sphere, then half a degree
/// over the eight degree neighborhood the coarse pass won. The clearance being
/// maximized varies over degrees rather than over half of one, so the answer is
/// the flat scan's for a fiftieth of its two hundred thousand cameras. A coarse
/// pass that landed somewhere wrong would not pass silently: every case that
/// calls this asserts that the direction it asked for really is on screen and
/// really is clear of the painted globe.
fn camera_showing(
    eqj: glam::Vec3,
    sky: &sunlit_core::scene::sky::SkyState,
    params: &SceneParams,
    viewport: glam::Vec2,
) -> (f32, f32) {
    let world = sky.world_from_eqj * eqj;
    let score = |longitude: f32, latitude: f32| -> Option<f32> {
        let camera = sunlit_core::scene::camera::OrbitalCamera::new(
            longitude,
            latitude,
            sunlit_core::scene::camera::zoom_to_distance(params.camera.zoom),
        );
        let view_direction = (camera.view_matrix() * world.extend(0.0)).truncate();
        let circle = sunlit_core::scene::sun_occlusion::sky_lens_disc(
            view_direction,
            0.0,
            params.sky_fov,
            glam::Vec2::ZERO,
            viewport,
        )?;
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
        Some(inset.min(circle.center.distance(globe.center) - globe.radius))
    };

    let sweep = |bounds: (f32, f32, f32, f32), step: f32, best: &mut (f32, (f32, f32))| {
        let (from_longitude, to_longitude, from_latitude, to_latitude) = bounds;
        let mut longitude = from_longitude;
        while longitude < to_longitude {
            let mut latitude = from_latitude;
            while latitude < to_latitude {
                if let Some(score) = score(longitude, latitude)
                    && score > best.0
                {
                    *best = (score, (longitude, latitude));
                }
                latitude += step;
            }
            longitude += step;
        }
    };

    // The coarse pass keeps several candidates rather than one, because the
    // score has plateaus: two framings eight degrees apart can differ by a
    // tenth of a pixel, and refining only the coarse winner would settle on
    // whichever side of the plateau the four degree grid happened to sample.
    let mut coarse: Vec<(f32, (f32, f32))> = Vec::new();
    let mut longitude = -180.0_f32;
    while longitude < 180.0 {
        let mut latitude = -85.0_f32;
        while latitude < 85.0 {
            if let Some(score) = score(longitude, latitude) {
                coarse.push((score, (longitude, latitude)));
            }
            latitude += 4.0;
        }
        longitude += 4.0;
    }
    coarse.sort_by(|a, b| b.0.total_cmp(&a.0));

    let mut best = (f32::MIN, (0.0_f32, 0.0_f32));
    for &(_, (longitude, latitude)) in coarse.iter().take(CANDIDATES) {
        sweep(
            (
                longitude - 8.0,
                longitude + 8.0,
                (latitude - 8.0).max(-85.0),
                (latitude + 8.0).min(85.0),
            ),
            0.5,
            &mut best,
        );
    }
    best.1
}

/// Coarse candidates refined at half a degree. Eight covers the plateau every
/// direction these cases ask about has, measured against the exhaustive scan
/// this replaced.
const CANDIDATES: usize = 8;

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

/// The frame the layer paints, and the same framing with it switched off,
/// which is what isolates the layer from the globe and the clear color.
fn panorama_pair(harness: &Harness, params: &SceneParams) -> (Vec<u8>, Vec<u8>) {
    let on = harness.picture(params, FRAME);
    let off = harness.picture(
        &SceneParams {
            milky_way_intensity: 0.0,
            ..*params
        },
        FRAME,
    );
    (on, off)
}

/// `FRAME` as the placement helpers want it.
fn viewport() -> glam::Vec2 {
    #[allow(clippy::cast_precision_loss)]
    glam::Vec2::new(FRAME.0 as f32, FRAME.1 as f32)
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
        cloud_opacity_night: 0.0,
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
    let gpu = gpu();
    let params = panorama_params();
    let (on, off) = panorama_pair(sky(&gpu), &params);
    let differences = channel_differences(&on, &off);
    let changed = differences.iter().filter(|&&d| d > 0).count();
    let total = differences.len();
    let circle = globe_circle(&params, viewport());
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
    let gpu = gpu();
    let harness = landmark_sky(&gpu);
    let mut params = panorama_params();
    let direction = eqj_direction(LANDMARK.0, LANDMARK.1);
    let sky = sky_for(&params);
    let viewport = viewport();
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

    let (with_landmark, without) = panorama_pair(harness, &params);
    let (landmark, lit) =
        difference_centroid(&channel_differences(&with_landmark, &without), FRAME.0, 300);

    let starry = SceneParams {
        milky_way_intensity: 0.0,
        star_intensity: 4.0,
        star_mag_limit: -1.0,
        ..params
    };
    let with_star = harness.picture(&starry, FRAME);
    let (sprite, sprite_pixels) =
        difference_centroid(&channel_differences(&with_star, &without), FRAME.0, 60);

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
/// `atan2`'s branch cut is a curve two pixels wide, a derivative being a
/// property of the fragment quad, which is a fraction of a percent of a golden
/// frame: inside its outlier allowance and absent from its mean, so a golden
/// passes with the seam in it and a second difference over the sky pixels is
/// what can see it. The fixture does not depend on right ascension at all, so a
/// pixel differing from its neighbors cannot be content, and its coarsest mip is
/// one texel holding the bands' own mean, which is far from the sky at most
/// declinations.
///
/// The painted globe is excluded, because its grid lines are features of exactly
/// the shape being measured.
#[test]
fn the_wrap_column_is_not_a_band_of_the_coarsest_mip() {
    /// The branch cut is the half plane where a direction's y is zero and its x
    /// is negative, which is right ascension 180 at every declination.
    const CUT_RIGHT_ASCENSION: f32 = 180.0;
    /// How far a sky pixel may sit from the mean of its neighbors.
    ///
    /// The clean frame reaches 4 on warp and 7 on lavapipe, which is the two
    /// rasterizers disagreeing about filtering and rounding rather than anything
    /// about the sky. The fault reaches 102 on warp and 18 on lavapipe, so the
    /// margin is wide on one adapter and narrow on the other and this sits
    /// between the two pairs; what the second adapter buys is that the case is
    /// not assumed to behave the same on both, which is the whole reason it runs
    /// on both.
    const SECOND_DIFFERENCE_TOLERANCE: i32 = 12;

    let gpu = gpu();
    let harness = sky(&gpu);
    let mut params = panorama_params();
    let state = sky_for(&params);
    let viewport = viewport();
    let (longitude, latitude) = camera_showing(
        eqj_direction(CUT_RIGHT_ASCENSION, 0.0),
        &state,
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

    let pixels = harness.picture(&params, FRAME);
    let circle = globe_circle(&params, viewport);

    let width = FRAME.0 as usize;
    let height = FRAME.1 as usize;
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
            if curvature > SECOND_DIFFERENCE_TOLERANCE {
                anomalies += 1;
            }
            if curvature > largest {
                largest = curvature;
                worst_at = (x, y);
            }
        }
    }
    println!(
        "the largest second difference among the sky pixels is {largest} at {worst_at:?}, \
         and {anomalies} of them are over {SECOND_DIFFERENCE_TOLERANCE}"
    );
    assert!(
        largest <= SECOND_DIFFERENCE_TOLERANCE,
        "the sky pixel at {worst_at:?} sits {largest} away from the mean of its neighbors, \
         and {anomalies} of them do: this panorama does not depend on right ascension and \
         its bands are tens of pixels wide, so nothing in it can turn over in one"
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
    let gpu = gpu();
    let harness = landmark_sky(&gpu);
    let mut params = panorama_params();
    // Further out than the other cases, so a landmark magnified by the narrow
    // end of the slider still has room beside the globe.
    params.camera.zoom = 0.8;
    let state = sky_for(&params);
    let viewport = viewport();
    // Chosen at the narrow end, where the layer is magnified most and the
    // landmark is hardest to keep in frame.
    let (longitude, latitude) = camera_showing(
        eqj_direction(LANDMARK.0, LANDMARK.1),
        &state,
        &SceneParams {
            sky_fov: 60.0,
            ..params
        },
        viewport,
    );
    params.camera.longitude = longitude;
    params.camera.latitude = latitude;

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
            "at {sky_fov} degrees of sky a landmark {:.1} px across sits {:.1} px from a \
             globe {:.1} px across",
            disc.radius * 2.0,
            disc.center.distance(circle.center),
            circle.radius * 2.0
        );
        let (on, off) = panorama_pair(harness, &framing);
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

/// The engine whose panorama slot holds the shipped asset, where the checkout
/// has it.
///
/// `None` where `textures/**` is still Git LFS pointers, which is what makes
/// the two cases that need it skip rather than fail.
struct RealSky {
    harness: Harness,
    cache: std::path::PathBuf,
}

impl std::ops::Deref for RealSky {
    type Target = Harness;
    fn deref(&self) -> &Harness {
        &self.harness
    }
}

/// The width the real sky loads at: above the asset's own 4096, so it loads as
/// it is and nothing is cached until a case asks for less.
const REAL_SKY_WIDTH: u32 = 8192;

static REAL_SKY: LazyLock<Option<RealSky>> = LazyLock::new(|| {
    let path = real_asset("milkyway_2020_4k.jxl").ok()?;
    let dir = FIXTURES.join("real_sky");
    std::fs::create_dir_all(&dir).expect("create the real sky cache directory");
    let cache = dir.clone();
    let harness = Harness::start(move |config| {
        config.preview_size = FRAME;
        config.params = panorama_params();
        config.texture_paths = vec![None, None, None, Some(path)];
        config.cache_dir = Some(dir);
        config.texture_resolution = REAL_SKY_WIDTH;
    });
    harness.wait_for_slot_texture("milky_way_texture");
    Some(RealSky { harness, cache })
});

/// The shipped panorama's engine, or `None` with a printed reason.
fn real_sky(_gpu: &Gpu) -> Option<&'static RealSky> {
    if let Err(why) = real_asset("milkyway_2020_4k.jxl") {
        println!("skipping: {why}; `git lfs pull` fetches the assets");
        return None;
    }
    let group = REAL_SKY.as_ref()?;
    if group.harness.restore_resolution() {
        wait_for_panorama_width(group, 4096);
    }
    group.harness.reset();
    Some(group)
}

/// Block until the panorama in the report is `width` wide, and answer with its
/// size.
fn wait_for_panorama_width(harness: &Harness, width: u32) -> (u32, u32) {
    let deadline = std::time::Instant::now() + TIMEOUT;
    loop {
        let report = harness
            .engine
            .memory_report()
            .expect("the engine should answer with a report");
        if let Some(texture) = report
            .expected
            .iter()
            .find(|texture| texture.label == "milky_way_texture")
            && texture.width == width
        {
            return (texture.width, texture.height);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no {width} wide panorama within {TIMEOUT:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
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
    let gpu = gpu();
    let Some(harness) = real_sky(&gpu) else {
        return;
    };

    let base = panorama_params();
    let state = sky_for(&base);
    let viewport = viewport();
    let sample = |name: &str, right_ascension: f32, declination: f32| {
        let direction = eqj_direction(right_ascension, declination);
        let (longitude, latitude) = camera_showing(direction, &state, &base, viewport);
        let mut params = base;
        params.camera.longitude = longitude;
        params.camera.latitude = latitude;
        let position = eqj_screen_position(direction, &params, viewport)
            .expect("in front of the lens at this framing");
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (x, y) = (position.x.round() as u32, position.y.round() as u32);
        assert!(
            (5..FRAME.0 - 5).contains(&x) && (5..FRAME.1 - 5).contains(&y),
            "{name} is at ({x}, {y}), which is not a window inside the frame"
        );
        assert!(
            position.distance(globe_circle(&params, viewport).center)
                > globe_circle(&params, viewport).radius + 10.0,
            "{name} lands on the painted globe"
        );
        let pixels = harness.picture(&params, FRAME);
        // A window rather than a pixel, because the sky is Gaia photon noise
        // and one texel of it is not what is being compared.
        let mut total = 0_u32;
        let mut count = 0_u32;
        for wy in y.saturating_sub(4)..(y + 5).min(FRAME.1) {
            for wx in x.saturating_sub(4)..(x + 5).min(FRAME.0) {
                let index = ((wy * FRAME.0 + wx) * 4) as usize;
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
}

/// No star the sprite pipeline draws is baked into the real panorama.
///
/// The `milkyway_2020` layer is the SVS map with the Hipparcos and Tycho stars
/// taken out, which is what keeps a bright star from being drawn twice: once as
/// phase A's sprite and once as a blob under it. That is a property of the file
/// that shipped rather than of the description it came with, and it is a
/// property a re-bake from the source could lose without anything else moving.
///
/// The measure is the one the asset was checked with by hand: the mean of a 3x3
/// texel window at the star's own position against the mean of the 41x41 window
/// around it, on the file as it sits on disk. Every catalog record the star draw
/// submits inside `MAGNITUDE_LIMIT` is measured, rather than a hand-picked list,
/// so the set is the one the sprites come from.
///
/// The bound is what separates the two answers, and the numbers on both sides
/// of it are measured. Across the 21 records inside the limit the ratio runs
/// from 0.73 to 1.26, which is bright stars sitting in bright parts of the Milky
/// Way and nothing more; the brightest is Antares at 1.26. A star baked into the
/// layer saturates the texels it covers, so a core at 765 of 765 reads 2.32
/// against the brightest surround in the set and 5 or more against a typical
/// one, and the wrong SVS layer would do that to most of the 21 at once. Two
/// sits between the two, with the clean maximum well clear of it.
///
/// What this window cannot see is a star confined to a single texel in the
/// brightest part of the plane: raising one texel of the nine to 765 where the
/// sky already reads 363, which is the brightest core in the set, takes the core
/// to 408 and the ratio to 1.26, inside the bound. The peak texel of the core
/// rather than its mean does not fix that and was measured: the map's own grain
/// already puts single texels at 2.08 times the local mean, so a peak metric has
/// no separation left to spend.
///
/// Skips with a printed reason where `textures/**` is still Git LFS pointers.
#[test]
fn no_bright_star_is_baked_into_the_real_panorama() {
    /// Bright enough to be a sprite nothing could hide under.
    const MAGNITUDE_LIMIT: f32 = 1.3;
    /// Half width of the core window, which is 3x3 texels.
    const CORE: i32 = 1;
    /// Half width of the surrounding window, which is 41x41.
    const SURROUND: i32 = 20;
    /// The core may be this much brighter than what surrounds it.
    const RATIO_BOUND: f64 = 2.0;

    let path = match real_asset("milkyway_2020_4k.jxl") {
        Ok(path) => path,
        Err(why) => {
            println!("skipping: {why}; `git lfs pull` fetches the assets");
            return;
        }
    };
    sunlit_core::assets::texture_loader::register_jxl_hook();
    let panorama = image::open(&path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()))
        .to_rgba8();
    let width = i32::try_from(panorama.width()).expect("a panorama of a sane width");
    let height = i32::try_from(panorama.height()).expect("a panorama of a sane height");

    // Wrapped in u and clamped in v, which is what the map itself does at its
    // seam and at its poles.
    let window_mean = |cx: i32, cy: i32, half: i32| -> f64 {
        let mut total = 0_u32;
        let mut count = 0_u32;
        for dy in -half..=half {
            let y = (cy + dy).clamp(0, height - 1);
            for dx in -half..=half {
                let x = (cx + dx).rem_euclid(width);
                let texel = panorama.get_pixel(
                    u32::try_from(x).expect("wrapped into the map"),
                    u32::try_from(y).expect("clamped into the map"),
                );
                total += u32::from(texel[0]) + u32::from(texel[1]) + u32::from(texel[2]);
                count += 1;
            }
        }
        f64::from(total) / f64::from(count.max(1))
    };

    let catalog = sunlit_core::assets::stars::embedded_catalog();
    let visible = usize::try_from(catalog.visible_count(MAGNITUDE_LIMIT)).expect("a small prefix");
    assert!(
        visible >= 15,
        "only {visible} catalog records are inside magnitude {MAGNITUDE_LIMIT}, \
         which is not the brightest sky"
    );
    let records = catalog.instance_bytes();

    let mut worst = (0.0_f64, 0.0_f32, 0.0_f32);
    for index in 0..visible {
        let record = &records[index * stars::RECORD_SIZE..(index + 1) * stars::RECORD_SIZE];
        let component = |offset: usize| {
            f32::from_le_bytes(
                record[offset..offset + 4]
                    .try_into()
                    .expect("four bytes of a direction"),
            )
        };
        let direction = glam::Vec3::new(component(0), component(4), component(8)).normalize();
        let right_ascension = direction
            .y
            .atan2(direction.x)
            .to_degrees()
            .rem_euclid(360.0);
        let declination = direction.z.clamp(-1.0, 1.0).asin().to_degrees();
        let magnitude = f32::from(record[15]) / 255.0 * 10.0 - 2.0;

        let (u, v) = support::panorama_texel(
            right_ascension,
            declination,
            panorama.width(),
            panorama.height(),
        );
        #[allow(clippy::cast_possible_truncation)]
        let (cx, cy) = (u.floor() as i32, v.floor() as i32);
        let core = window_mean(cx, cy, CORE);
        let surround = window_mean(cx, cy, SURROUND);
        let ratio = core / surround.max(1.0);
        println!(
            "  magnitude {magnitude:.2} at ra {right_ascension:.2} dec {declination:.2}: \
             core {core:.1} of 765, surround {surround:.1}, ratio {ratio:.2}"
        );
        if ratio > worst.0 {
            worst = (ratio, right_ascension, declination);
        }
    }

    println!(
        "the brightest core against its surroundings over {visible} stars is {:.2}, \
         at ra {:.2} dec {:.2}",
        worst.0, worst.1, worst.2
    );
    assert!(
        worst.0 < RATIO_BOUND,
        "a star at ra {:.2} dec {:.2} reads {:.2} times its surroundings, over {RATIO_BOUND}: \
         this panorama has bright stars in it and the sprites are drawing them again",
        worst.1,
        worst.2,
        worst.0
    );
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
    let gpu = gpu();
    let Some(group) = real_sky(&gpu) else {
        return;
    };

    let cached = group
        .cache
        .join("texture_cache")
        .join("milkyway_2020_4k.2048.png");
    let _ = std::fs::remove_file(&cached);

    // The engine starts at 8192, above the asset's own width.
    let wide = wait_for_panorama_width(group, 4096);
    println!(
        "at the {REAL_SKY_WIDTH} setting the panorama loads at {}x{}",
        wide.0, wide.1
    );
    assert_eq!(
        wide,
        (4096, 2048),
        "the widest setting is a cap, and the file is 4096 wide"
    );

    group.set_texture_resolution(2048);
    let narrow = wait_for_panorama_width(group, 2048);
    println!(
        "at the 2048 setting the panorama loads at {}x{}",
        narrow.0, narrow.1
    );
    assert_eq!(narrow, (2048, 1024));
    assert!(
        cached.exists(),
        "the halving was not written to {}",
        cached.display()
    );

    // Away and back, so the second load at the narrow width reads what the
    // first one wrote rather than halving the source again. The width alone
    // cannot show that; the file having to be there for it to succeed can.
    group.set_texture_resolution(REAL_SKY_WIDTH);
    wait_for_panorama_width(group, 4096);
    group.set_texture_resolution(2048);
    assert_eq!(wait_for_panorama_width(group, 2048), narrow);
}

// ---------------------------------------------------------------------------
// Clouds on the night side
// ---------------------------------------------------------------------------

/// The size the cloud cases export at. Small on purpose: what they measure is
/// the mean of one window, not a picture.
const CLOUD_CASE_SIZE: (u32, u32) = (256, 128);

/// Half-width of that window, in pixels. The frame's center is the point the
/// camera sits over, and the night map's city is thirty degrees away from it,
/// which is well outside this at every zoom that fills the frame.
const CLOUD_WINDOW: u32 = 6;

/// Mean channel value over a square window at the center of an exported frame.
#[allow(clippy::cast_precision_loss)]
fn center_window_mean(pixels: &[u8], size: (u32, u32)) -> f64 {
    let (width, height) = size;
    let mut total = 0u64;
    let mut count = 0u64;
    for y in height / 2 - CLOUD_WINDOW..height / 2 + CLOUD_WINDOW {
        for x in width / 2 - CLOUD_WINDOW..width / 2 + CLOUD_WINDOW {
            let px = ((y * width + x) * 4) as usize;
            total += u64::from(pixels[px]) + u64::from(pixels[px + 1]) + u64::from(pixels[px + 2]);
            count += 3;
        }
    }
    total as f64 / count as f64
}

/// Parameters the cloud cases share: the fixture surface, the camera over the
/// point the case is about, and nothing else in the window.
///
/// The atmosphere, the stars and the Sun are all off so that the window holds
/// the globe and the layer over it and nothing else, and `hour` is what moves
/// the sun: the camera stays where it is, so the surface under the window is the
/// same texels whichever side of the terminator the case asks for.
fn cloud_case_params(texture_index: i32, longitude: f32, hour: f32) -> SceneParams {
    let mut params = SceneParams {
        texture_index,
        sample_count: 1,
        atmo_enabled: false,
        star_intensity: 0.0,
        sun_glow: 0.0,
        camera: sunlit_core::scene::camera::CameraParams {
            longitude,
            latitude: 0.0,
            zoom: 0.26,
            ..sunlit_core::scene::camera::CameraParams::default()
        },
        ..SceneParams::default()
    };
    params.datetime.use_custom = true;
    params.datetime.custom_hour = hour;
    params.datetime.custom_day_of_year = 80;
    params.datetime.custom_year = 2026;
    params
}

/// Noon UTC, where the frame center of a camera at longitude 180 is as deep into
/// the night as the globe goes, and midnight, where the same pixels are lit.
const NIGHT_HOUR: f32 = 12.0;
const DAY_HOUR: f32 = 0.0;

/// The defect this change is about, in one assertion: a cloud on the night side
/// has to be brighter than the ground it covers.
///
/// The fixture's unlit base is what `BlackMarble_2016.jxl` reads over unlit
/// land, and at the old hardcoded 0.05 the deck comes out darker than it, 13.0
/// against 42.0 in the units this prints, so this fails on the code before this
/// change rather than merely measuring something. The deck reads its own value
/// almost exactly, because the fixture's cloud is 255 or nothing and the night
/// opacity covers the ground completely at any density of one.
#[test]
fn a_night_side_cloud_is_brighter_than_the_land_under_it() {
    let gpu = gpu();
    let harness = surface(&gpu);
    let params = cloud_case_params(3, 180.0, NIGHT_HOUR);

    let covered = harness.picture(&params, CLOUD_CASE_SIZE);
    let bare = harness.picture(
        &SceneParams {
            cloud_opacity: 0.0,
            cloud_opacity_night: 0.0,
            ..params
        },
        CLOUD_CASE_SIZE,
    );

    let (covered, bare) = (
        center_window_mean(&covered, CLOUD_CASE_SIZE),
        center_window_mean(&bare, CLOUD_CASE_SIZE),
    );
    println!("night side: {covered:.1} under the deck, {bare:.1} with the land bare");
    assert!(
        covered > bare + 8.0,
        "a night-side cloud reads {covered:.1} over ground that reads {bare:.1}: \
         the layer is darkening the night side instead of lighting it"
    );
}

/// The cloud layer is shaded by the sun in every texture mode, not only in the
/// one whose terminator uniform is real.
///
/// `write_uniforms` puts -1.0 in `terminator_width` outside blend mode, as the
/// sentinel that tells `fs_sphere` to ignore the sun, and `fs_cloud` used to
/// read the same uniform: its ramp became `smoothstep(1.0, -1.0, n_dot_l)`,
/// which the specification calls indeterminate and which the standard formula
/// inverts, so clouds were bright at local midnight and dark at noon. The three
/// single-texture modes are the ones that carry it.
///
/// The camera does not move between the two readings and the mode ignores the
/// sun, so the ground under the window is the same texels in both: the whole
/// difference is the layer's own shading.
#[test]
fn a_dayside_cloud_is_brighter_than_a_night_side_one_in_every_mode() {
    const MODES: [(i32, &str); 3] = [(0, "grid"), (1, "day"), (2, "night")];

    let gpu = gpu();
    let harness = surface(&gpu);

    for (mode, name) in MODES {
        let mut means = Vec::new();
        for hour in [DAY_HOUR, NIGHT_HOUR] {
            let pixels = harness.picture(&cloud_case_params(mode, 180.0, hour), CLOUD_CASE_SIZE);
            means.push(center_window_mean(&pixels, CLOUD_CASE_SIZE));
        }
        let (lit, unlit) = (means[0], means[1]);
        println!("{name} mode: {lit:.1} at noon, {unlit:.1} at midnight");
        assert!(
            lit > unlit + 40.0,
            "in {name} mode the deck reads {lit:.1} at noon and {unlit:.1} at midnight: \
             the cloud ramp is reading the sentinel rather than its own width"
        );
    }
}

/// Either opacity at zero switches off its own hemisphere and not the layer.
///
/// The banded fixture is 255 or nothing, so the deck over the frame center has a
/// density of one, and the night map's city is under it: the ground reads near
/// display white and the deck reads `cloud_night`, so which of the two the frame
/// holds is one number. At a night opacity of zero the night side has to show
/// the ground even though the day slider is up, and with only the night slider
/// up the layer still has to draw, which is what `draws_clouds` is for.
///
/// A density of one at a night opacity of zero is also `pow(0, 0)` before
/// `fs_cloud` holds the base off zero, so this is the shape that reaches it.
#[test]
fn an_opacity_at_zero_switches_off_only_its_own_hemisphere() {
    let gpu = gpu();
    let harness = surface(&gpu);
    let base = cloud_case_params(3, support::NIGHT_FIXTURE_CITY.0, NIGHT_HOUR);
    let read = |day: f32, night: f32| {
        let pixels = harness.picture(
            &SceneParams {
                cloud_opacity: day,
                cloud_opacity_night: night,
                ..base
            },
            CLOUD_CASE_SIZE,
        );
        center_window_mean(&pixels, CLOUD_CASE_SIZE)
    };

    let day_only = read(base.cloud_opacity, 0.0);
    let night_only = read(0.0, base.cloud_opacity_night);
    let deck = f64::from(base.cloud_night) * 255.0;
    println!("day slider alone reads {day_only:.1}, night slider alone reads {night_only:.1}");

    assert!(
        day_only > deck + 40.0,
        "with the night opacity at zero the night side reads {day_only:.1}, which is the deck          at {deck:.1} rather than the lit ground under it"
    );
    assert!(
        (night_only - deck).abs() < 2.0,
        "with only the night opacity up the night side reads {night_only:.1} rather than the          deck's {deck:.1}, so the layer is not drawing when the day slider is zero"
    );
}

/// The night opacity covers the ground at the top of its range, and lets it
/// through below.
///
/// The camera sits over the night map's city with the deck at one mid density
/// over all of it, which is the framing the defect was reported in: the ground
/// under the deck is display white and the deck itself is `cloud_night`, so what
/// the blend does with the two is visible in one number. The old straight
/// multiply could not reach the deck's own value from a density of 0.45 however
/// far the slider went, which is what "one hundred percent still lets it
/// through" meant.
///
/// The three readings also have to be ordered, or a mapping that covered the
/// ground by ignoring the slider would pass the first assertion alone.
#[test]
fn the_night_opacity_reaches_full_cover() {
    let gpu = gpu();
    let harness = surface(&gpu);
    let base = cloud_case_params(3, support::NIGHT_FIXTURE_CITY.0, NIGHT_HOUR);

    // The banded map is 255 or nothing, so any nonzero night opacity covers the
    // ground completely and there is no covering left to measure. This is the
    // one case that needs the partial map, and the size is how it knows the
    // swap has reached the slot.
    let uniform = harness.clouds.serve_uniform(support::CLOUD_FIXTURE_PARTIAL);
    wait_for_cloud_size(harness, uniform, "the partial cloud map");

    let read = |night_opacity: f32| {
        let pixels = harness.picture(
            &SceneParams {
                cloud_opacity_night: night_opacity,
                ..base
            },
            CLOUD_CASE_SIZE,
        );
        center_window_mean(&pixels, CLOUD_CASE_SIZE)
    };

    let uncovered = read(0.0);
    let partly = read(SceneParams::default().cloud_opacity_night);
    let covered = read(1.0);
    let deck = f64::from(base.cloud_night) * 255.0;
    println!(
        "night opacity 0 reads {uncovered:.1}, default reads {partly:.1}, 1 reads {covered:.1},          the deck alone would read {deck:.1}"
    );

    assert!(
        (covered - deck).abs() < 2.0,
        "at full night opacity the window reads {covered:.1} where the deck alone is {deck:.1},          so the ground under it is still showing through"
    );
    assert!(
        uncovered > covered + 40.0,
        "the window reads {uncovered:.1} with the deck switched off and {covered:.1} with it          covering, which is not the lit ground this case needs under the deck"
    );
    assert!(
        partly > covered + 5.0 && partly < uncovered - 5.0,
        "the default night opacity reads {partly:.1}, which is not between the {covered:.1} of          full cover and the {uncovered:.1} of none, so the slider is not doing the covering"
    );
}

// ---------------------------------------------------------------------------
// Reacting to a display layout that changed
// ---------------------------------------------------------------------------

/// How long a case waits for something it expects to happen.
const CHANGE_TIMEOUT: Duration = Duration::from_secs(20);

/// How long a case waits before concluding that nothing is going to happen.
///
/// The engine loop wakes every 50 ms whatever else is going on, so this is
/// several of its ticks and not a guess at how fast the machine is.
const NOTHING_HAPPENS_IN: Duration = Duration::from_millis(400);

/// Block until the sink has been asked for its monitors `target` times.
fn wait_for_queries(sink: &RecordingSink, target: usize) {
    let deadline = std::time::Instant::now() + CHANGE_TIMEOUT;
    while sink.queries() < target {
        assert!(
            std::time::Instant::now() < deadline,
            "the engine asked for the monitors {} times, not {target}",
            sink.queries()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A one-screen layout no case has used before, and none will use again.
///
/// The engine remembers the layout it last settled on for as long as it runs,
/// and every case here is about the difference between a layout that changed
/// and one that did not. A shared engine outlives the case that gave it its
/// last layout, so a case that reused a size would be asking about a change
/// that had already happened.
fn an_unfamiliar_layout(id: &str) -> Vec<Monitor> {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let width = 256 + NEXT.fetch_add(1, Ordering::Relaxed) * 8;
    vec![screen(id, 0, width, 160, true)]
}

/// Give the engine the settle and a hint's worth of a baseline to compare
/// against, so the cases below start from a layout the engine has seen.
fn establish_baseline(group: &Watching) {
    let layout = an_unfamiliar_layout("baseline");
    group.sink.set_monitors(layout.clone());
    group.hint();
    group.advance(&group.clock, DISPLAY_SETTLE + Duration::from_secs(1));
    let announced = group
        .next_layout(CHANGE_TIMEOUT)
        .expect("a layout the engine has not seen before is one it announces");
    assert_eq!(announced, layout);
}

/// A settled burst is one query and, where nothing moved, nothing else.
///
/// The hints this design pays for and discards are exactly this case: a resume
/// from sleep, a scaling change, a color depth change. Each costs one
/// enumeration and an equal comparison, and nothing on the desk moves. A burst
/// of them costs the same one enumeration, because a change is a burst on both
/// platforms.
#[test]
fn a_settled_burst_of_hints_is_one_query_whatever_it_was_made_of() {
    let gpu = gpu();
    let group = watching(&gpu);
    establish_baseline(group);

    let before = group.sink.queries();
    group.hint();
    group.advance(&group.clock, DISPLAY_SETTLE + Duration::from_secs(1));
    wait_for_queries(&group.sink, before + 1);
    assert!(
        group.next_layout(NOTHING_HAPPENS_IN).is_none(),
        "the layout is the one the engine already knows, so there is nothing to announce"
    );
    assert_eq!(
        group.sink.queries(),
        before + 1,
        "one settled burst is one query"
    );
    assert!(group.sink.publications().is_empty());

    for _ in 0..5 {
        group.hint();
    }
    group.advance(&group.clock, DISPLAY_SETTLE + Duration::from_secs(1));
    wait_for_queries(&group.sink, before + 2);
    std::thread::sleep(NOTHING_HAPPENS_IN);
    assert_eq!(
        group.sink.queries(),
        before + 2,
        "five hints inside the settle window are one layout change"
    );
}

/// A hint that lands while one is pending pushes the deadline out again.
///
/// Trailing rather than leading: a docking station brings its screens up one at
/// a time, and settling on the first of them costs a full render and a visible
/// swap that the second one immediately invalidates.
#[test]
fn a_hint_during_the_settle_moves_the_deadline_it_found() {
    let gpu = gpu();
    let group = watching(&gpu);
    establish_baseline(group);

    let before = group.sink.queries();
    group.hint();
    group.advance(&group.clock, Duration::from_millis(1500));
    group.hint();

    // The first hint's deadline has passed; the second one's has not.
    group.advance(&group.clock, Duration::from_secs(1));
    std::thread::sleep(NOTHING_HAPPENS_IN);
    assert_eq!(
        group.sink.queries(),
        before,
        "the second hint should have moved the deadline the first one set"
    );

    group.advance(&group.clock, Duration::from_secs(1));
    wait_for_queries(&group.sink, before + 1);
}

/// The rule, in the case it was written for: the desk holds a picture this
/// process made for a layout that is gone, so it gets one for the layout that
/// is here, at the sizes that layout has.
#[test]
fn a_layout_that_changed_under_a_published_wallpaper_is_published_again() {
    let gpu = gpu();
    let group = plain(&gpu);
    group.sink.set_monitors(an_unfamiliar_layout("docked"));

    group
        .publish()
        .expect("publishing to a recording sink cannot fail");
    assert_eq!(group.sink.publications().len(), 1);

    // The notebook was undocked: one screen left, and a different one.
    let alone = an_unfamiliar_layout("internal");
    let size = (alone[0].width, alone[0].height);
    group.sink.set_monitors(alone.clone());
    group.hint();
    group.advance(&group.clock, DISPLAY_SETTLE + Duration::from_secs(1));

    let announced = group
        .next_layout(CHANGE_TIMEOUT)
        .expect("a layout that changed is announced");
    assert_eq!(announced, alone);

    group
        .wait_for_publish()
        .expect("publishing to a recording sink cannot fail");
    let published = group.sink.publications();
    assert_eq!(published.len(), 2, "the change published once more");
    assert_eq!(
        published[1].images,
        vec![Some(size)],
        "the second publish is for the screen that is there now, at its own size"
    );
}

/// Somebody who opened the settings window to look and never asked for a
/// wallpaper does not get one because they moved a screen.
#[test]
fn a_layout_that_changed_with_nothing_on_the_desk_is_announced_and_not_published() {
    let gpu = gpu();
    let group = watching(&gpu);
    establish_baseline(group);

    let alone = an_unfamiliar_layout("internal");
    group.sink.set_monitors(alone.clone());
    group.hint();
    group.advance(&group.clock, DISPLAY_SETTLE + Duration::from_secs(1));

    assert_eq!(
        group.next_layout(CHANGE_TIMEOUT),
        Some(alone),
        "the window still has to be told, so its diagram is not a layout from ten minutes ago"
    );
    std::thread::sleep(NOTHING_HAPPENS_IN);
    assert!(
        group.sink.publications().is_empty(),
        "no wallpaper was ever asked for, so a moved screen does not produce one"
    );
}

/// A sink that refused is never asked unprompted, so a layout change cannot put
/// an error in the status line out of nowhere.
#[test]
fn a_layout_that_changed_after_a_refusal_is_announced_and_not_published() {
    let gpu = gpu();
    let group = watching(&gpu);
    group.sink.refuse();

    let refusal = group.publish().expect_err("a refusing sink cannot publish");
    assert_eq!(refusal, RecordingSink::REFUSED);

    establish_baseline(group);
    let alone = an_unfamiliar_layout("internal");
    group.sink.set_monitors(alone.clone());
    group.hint();
    group.advance(&group.clock, DISPLAY_SETTLE + Duration::from_secs(1));

    assert_eq!(group.next_layout(CHANGE_TIMEOUT), Some(alone));
    std::thread::sleep(NOTHING_HAPPENS_IN);
    assert!(group.sink.publications().is_empty());
    assert!(
        group.next_layout(NOTHING_HAPPENS_IN).is_none(),
        "one layout change is one announcement"
    );
}
