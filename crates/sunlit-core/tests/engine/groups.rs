use std::sync::Arc;
use std::sync::LazyLock;
use std::time::Duration;

use sunlit_core::display::layout::DisplayMode;
use sunlit_core::engine::EngineCommand;
use sunlit_core::engine::clock::MockClock;
use sunlit_core::params::SceneParams;

use crate::clouds_variant::wait_for_cloud_size;
use crate::harness::{Gpu, Harness, TIMEOUT, test_params};
use crate::memory::expected_widths;
use crate::sinks::{RecordingSink, two_screens};
use crate::support;
use crate::test_support::ScratchDir;
use crate::textures::blend_params;

// ---------------------------------------------------------------------------
// The shared engines
// ---------------------------------------------------------------------------

/// The size the preview quantizes to for every group here, and the size the
/// cases that read pixels export at.
pub(crate) const FRAME: (u32, u32) = (512, 256);

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
pub(crate) static FIXTURES: LazyLock<ScratchDir> = LazyLock::new(|| {
    sweep_abandoned_fixture_roots();
    ScratchDir::new("engine_shared")
});

/// Remove the fixture roots earlier runs left behind.
///
/// `FIXTURES` is not dropped, so each run leaves its tree in place. An hour is
/// far longer than this target has ever taken, so anything older than that
/// belongs to a run that is over, and a run still going keeps its own.
fn sweep_abandoned_fixture_roots() {
    const ABANDONED_AFTER: Duration = Duration::from_hours(1);

    let Ok(entries) = std::fs::read_dir(crate::test_support::scratch_root()) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with("sunlit_earth_engine_shared_")
        {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .is_ok_and(|at| at.elapsed().is_ok_and(|age| age > ABANDONED_AFTER));
        if old {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// The engine for every case that needs no texture file of its own.
///
/// The sink and the clock are the two things a case cannot replace on a running
/// engine, so both are here and both are steerable: the monitor list moves, the
/// refusal switches on and off, and the clock only ever goes forward, which is
/// all any case asks of it.
pub(crate) struct Plain {
    harness: Harness,
    pub(crate) sink: Arc<RecordingSink>,
    pub(crate) clock: Arc<MockClock>,
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

pub(crate) fn plain(_gpu: &Gpu) -> &'static Plain {
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
pub(crate) struct Watching {
    harness: Harness,
    pub(crate) sink: Arc<RecordingSink>,
    pub(crate) clock: Arc<MockClock>,
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

pub(crate) fn watching(_gpu: &Gpu) -> &'static Watching {
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
pub(crate) struct Surface {
    harness: Harness,
    pub(crate) sink: Arc<RecordingSink>,
    pub(crate) clouds: Arc<support::FixtureClouds>,
}

impl std::ops::Deref for Surface {
    type Target = Harness;
    fn deref(&self) -> &Harness {
        &self.harness
    }
}

/// The width the fixture surface is written at, and the width the slots hold
/// when no case has asked for another.
pub(crate) const SURFACE_WIDTH: u32 = support::SURFACE_FIXTURE_WIDTH;

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
        // Blend mode, because a file-backed slot is loaded only while the mode
        // wants it. At `test_params`'s grid the procedural texture alone is
        // what `TexturesReady` answers for, and both file-backed slots would
        // still be empty when the first case read them.
        config.params = blend_params();
        // Short, because the cloud cases swap the served map and the poll is
        // what carries the new one to the slot. Every poll the swap does not
        // follow answers "unchanged" from a string compare.
        config.cloud_poll_interval = Duration::from_millis(50);
    });
    harness.wait_for_textures("the fixture surface at startup");
    wait_for_surface_slots(&harness, SURFACE_WIDTH);
    Surface {
        harness,
        sink,
        clouds,
    }
});

pub(crate) fn surface(_gpu: &Gpu) -> &'static Surface {
    let group = &*SURFACE;
    // Blend mode before the width, and the width before the wait. A slot is
    // loaded only while the mode wants it, so a case that left a single-texture
    // mode behind would hand the next one an empty day or night slot, and a
    // reload at the restored width would fetch only what that mode asked for.
    group
        .harness
        .engine
        .send(EngineCommand::UpdateParams(Box::new(blend_params())));
    if group.harness.restore_resolution() {
        group.harness.wait_for_textures("putting the width back");
    }
    wait_for_surface_slots(&group.harness, SURFACE_WIDTH);
    let bands = group.clouds.serve_bands();
    wait_for_cloud_size(&group.harness, bands, "putting the cloud map back");
    group.harness.reset();
    group.sink.reset(two_screens());
    group
}

/// Block until the day and night slots hold a texture `width` wide.
///
/// `TexturesReady` answers for the mode the engine is in, and a case may leave
/// behind a mode that wants neither file-backed slot, so a group whose cases
/// read those slots asks about them by name rather than taking readiness for
/// the answer.
fn wait_for_surface_slots(harness: &Harness, width: u32) {
    let deadline = std::time::Instant::now() + TIMEOUT;
    loop {
        let report = harness.engine.memory_report().expect("a report");
        if expected_widths(&report, "day_texture") == [width]
            && expected_widths(&report, "night_texture") == [width]
        {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the fixture surface did not reach {width} within {TIMEOUT:?}:\n{report}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The engine with the Moon and a banded panorama behind their slots.
///
/// Two overlays in one engine because neither case group can see the other's:
/// the Moon cases run with `milky_way_intensity` at zero and the panorama cases
/// with `moon_brightness` at zero, both of which `test_params` already sets.
pub(crate) struct Sky {
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

pub(crate) fn sky(_gpu: &Gpu) -> &'static Sky {
    let group = &*SKY;
    group.harness.reset();
    group
}

/// The engine whose panorama is one disc at Sirius and black everywhere else.
///
/// Separate from [`SKY`] because the two fixtures answer opposite questions: a
/// landmark is exactly the one-pixel-wide feature the seam case is looking for,
/// so it cannot be in the map that case reads.
pub(crate) struct Landmark {
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
pub(crate) const LANDMARK: (f32, f32) = (101.287, -16.716);
pub(crate) const LANDMARK_RADIUS_DEGREES: f32 = 5.0;

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

pub(crate) fn landmark_sky(_gpu: &Gpu) -> &'static Landmark {
    let group = &*LANDMARK_SKY;
    group.harness.reset();
    group
}
