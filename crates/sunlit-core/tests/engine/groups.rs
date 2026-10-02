//! The shared engines, one per configuration a group of cases needs.

use std::sync::Arc;
use std::sync::LazyLock;
use std::time::Duration;

use sunlit_core::assets::cube_layout::CubeTextures;
use sunlit_core::display::layout::DisplayMode;
use sunlit_core::engine::EngineCommand;
use sunlit_core::engine::clock::MockClock;
use sunlit_core::params::SceneParams;

use crate::clouds_variant::wait_for_cloud_size;
use crate::harness::{Gpu, Harness, test_params};
use crate::sinks::{RecordingSink, two_screens};
use crate::support;
use crate::test_support::{self, ScratchDir};
use crate::textures::blend_params;

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

/// The engine with the Earth fixture's cube behind the globe and a cloud
/// source behind the overlay.
///
/// The resolution cases and the cloud cases share it: both want the surface,
/// neither disturbs the other, and the two together are a dozen engine starts
/// in one.
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

/// The resolution setting the surface group runs at when no case has asked
/// for another: the widest, which allows both tile levels.
pub(crate) const SURFACE_RESOLUTION: u32 = 8192;

static SURFACE: LazyLock<Surface> = LazyLock::new(|| {
    let dir = FIXTURES.join("surface");
    test_support::write_earth_fixture(&dir.join("textures"));
    let sink = Arc::new(RecordingSink::new(two_screens()));
    let clouds = Arc::new(support::FixtureClouds::bands());
    let (sink_for_config, clouds_for_config) = (Arc::clone(&sink), Arc::clone(&clouds));
    let harness = Harness::start(move |config| {
        config.cube_textures = CubeTextures::resolve(&dir.join("textures"));
        config.tile_geometry = crate::tiles::EARTH;
        config.cache_dir = Some(dir.join("cache"));
        config.texture_resolution = SURFACE_RESOLUTION;
        config.wallpaper = sink_for_config;
        config.cloud = Some(clouds_for_config);
        // Blend mode, the one that wants every cube and both halves of the
        // page table.
        config.params = blend_params();
        // Short, because the cloud cases swap the served map and the poll is
        // what carries the new one to the slot. Every poll the swap does not
        // follow answers "unchanged" from a string compare.
        config.cloud_poll_interval = Duration::from_millis(50);
    });
    harness.wait_for_textures("the Earth fixture's cube at startup");
    Surface {
        harness,
        sink,
        clouds,
    }
});

pub(crate) fn surface(_gpu: &Gpu) -> &'static Surface {
    let group = &*SURFACE;
    // Blend mode before the setting, so a reload the restored setting starts
    // is waited for in the mode the next case begins in.
    group.harness.settle_at(&blend_params());
    if group.harness.restore_resolution() {
        group.harness.wait_for_textures("putting the setting back");
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
        config.texture_paths = vec![Some(moon), Some(panorama)];
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
        config.texture_paths = vec![None, Some(path)];
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
