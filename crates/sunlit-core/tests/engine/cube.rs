//! The globe drawn from the cube surface: the transcoder's first packs, the
//! readiness they make, the month the date names, and the publish that waits
//! for them.
//!
//! Every case has an engine of its own, because the cube faces, the cache
//! directory and the clock are read once at startup. The faces are the texture
//! pipeline's fixture bake, cut to the fixture's geometry, so a whole pack
//! builds in milliseconds.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sunlit_core::assets::cube_layout::CubeTextures;
use sunlit_core::assets::tiles::{self, PackKind};
use sunlit_core::display::Monitor;
use sunlit_core::engine::clock::MockClock;
use sunlit_core::engine::wallpaper_sink::{WallpaperJob, WallpaperSink};
use sunlit_core::engine::{EngineCommand, EngineConfig, EngineEvent};
use sunlit_core::params::SceneParams;
use sunlit_core::scene::camera::CameraParams;

use crate::groups::FRAME;
use crate::harness::{Harness, TIMEOUT, gpu, has_lit_pixels, test_params};
use crate::sinks::{RecordingSink, screen};
use crate::test_support::{self, ScratchDir};
use crate::tiles::settled;

/// A textures directory holding the fixture bake, and a cache directory for
/// the packs cut from it.
struct CubeFixture {
    dir: ScratchDir,
}

impl CubeFixture {
    fn new(name: &str) -> Self {
        let dir = ScratchDir::new(name);
        test_support::write_cube_fixture(&dir.join("textures"));
        Self { dir }
    }

    fn cache(&self) -> PathBuf {
        self.dir.join("cache")
    }

    /// Point an engine at the fixture.
    fn configure(&self, config: &mut EngineConfig) {
        config.cube_textures = CubeTextures::resolve(&self.dir.join("textures"));
        config.tile_geometry = tiles::FIXTURE;
        config.cache_dir = Some(self.cache());
    }

    fn pack_exists(&self, kind: PackKind) -> bool {
        tiles::pack_path(&self.cache(), kind).exists()
    }
}

/// The day and night surfaces blended, on `day_of_year` of 2026 at noon.
fn blend_on(day_of_year: u16) -> SceneParams {
    let mut params = SceneParams {
        texture_index: 3,
        ..test_params()
    };
    params.datetime.custom_day_of_year = day_of_year;
    params
}

/// March 10th, the first half of March, which the fixture draws from
/// January's faces.
const MARCH_10: u16 = 69;
/// September 5th, the first half of September, from July's faces. September
/// is the last month the transcoder builds while March is in force.
const SEPTEMBER_5: u16 = 248;

/// The rows of the memory report under `label`.
fn rows<'a>(
    report: &'a sunlit_core::memory_report::MemoryReport,
    label: &str,
) -> Vec<&'a sunlit_core::memory_report::ExpectedTexture> {
    report
        .expected
        .iter()
        .filter(|texture| texture.label == label)
        .collect()
}

/// The packs the first frame needs are built, their cubes made resident, and
/// the engine says the textures are ready. The page table is resident beside
/// the cubes, and the tile array is not: the narrowest resolution setting
/// allows no tile.
#[test]
fn the_first_frame_packs_make_the_textures_ready() {
    let _gpu = gpu();
    let fixture = CubeFixture::new("engine_cube_ready");
    let harness = Harness::start(|config| {
        fixture.configure(config);
        config.texture_resolution = 2048;
        config.params = blend_on(MARCH_10);
    });
    harness.wait_for_textures("the first-frame packs");

    for kind in [PackKind::Mask, PackKind::Day(2), PackKind::Night] {
        assert!(fixture.pack_exists(kind), "no {kind:?} pack was built");
    }
    let report = harness.engine.memory_report().expect("a report");
    for (label, width) in [
        ("day_floor", tiles::FIXTURE.floor),
        ("night_floor", tiles::FIXTURE.floor),
        ("water_mask", tiles::FIXTURE.mask),
    ] {
        let found = rows(&report, label);
        // The other months' day floors join March's as their packs land.
        let resident = if label == "day_floor" { 1..=12 } else { 1..=1 };
        assert!(
            resident.contains(&found.len()),
            "{label} is resident {} times:\n{report}",
            found.len()
        );
        assert!(
            found
                .iter()
                .all(|texture| (texture.width, texture.layers) == (width, 6)),
            "{label} is a cube of the fixture's size"
        );
    }
    let pages = rows(&report, "page_table");
    assert_eq!(pages.len(), 1, "one page table:\n{report}");
    let cells = tiles::FIXTURE.face / tiles::FIXTURE.tile;
    assert_eq!(
        (pages[0].width, pages[0].height, pages[0].layers),
        (cells, cells, 6),
        "a cell per finest tile of each face"
    );
    let tiles = settled(&harness, "the loader", |_| true);
    assert!(
        tiles.wanted.is_empty() && rows(&report, "tile_array").is_empty(),
        "the narrowest resolution setting allows no tile, so the array is never \
         created:\n{report}"
    );
    assert!(has_lit_pixels(&harness.export(FRAME.0, FRAME.1)));
}

/// A date in another month makes that month's floor the one drawn, and a
/// wallpaper asked for in the same breath waits for it.
///
/// The clock never moves, so the engine stays busy from its first frame on and
/// the pause gate keeps the rest of the year unbuilt: September's pack does not
/// exist when the date moves to it, and the publish cannot go out before it is
/// built. The two halves of the fixture's year are different faces, so the
/// picture changes with the floor.
#[test]
fn a_new_month_is_drawn_from_its_own_floor_and_a_publish_waits_for_it() {
    let _gpu = gpu();
    let fixture = CubeFixture::new("engine_cube_month");
    let sink = Arc::new(RecordingSink::new(vec![screen("only", 0, 64, 32, true)]));
    let clock = Arc::new(MockClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let (sink_for_config, clock_for_config) = (Arc::clone(&sink), Arc::clone(&clock));
    let harness = Harness::start(|config| {
        fixture.configure(config);
        config.wallpaper = sink_for_config;
        config.clock = clock_for_config;
        config.params = blend_on(MARCH_10);
    });
    harness.wait_for_textures("March");
    let march = harness.export(FRAME.0, FRAME.1);
    assert!(
        !fixture.pack_exists(PackKind::Day(8)),
        "the pause gate should have kept September unbuilt while the engine was busy"
    );

    harness
        .engine
        .send(EngineCommand::UpdateParams(Box::new(blend_on(SEPTEMBER_5))));
    harness.engine.send(EngineCommand::RenderWallpaperNow);

    let deadline = std::time::Instant::now() + TIMEOUT;
    let mut ready_first = false;
    let mut published = false;
    while let Ok(event) = harness.events.recv_deadline(deadline) {
        match event {
            EngineEvent::TexturesReady => ready_first = true,
            EngineEvent::WallpaperSet(result) => {
                assert!(result.is_ok(), "the publish should have succeeded");
                assert!(
                    ready_first,
                    "the wallpaper went out before September's floor was resident"
                );
                published = true;
                break;
            }
            _ => {}
        }
    }
    assert!(published, "no publish within {TIMEOUT:?}");
    assert!(fixture.pack_exists(PackKind::Day(8)));
    assert_eq!(sink.publications().len(), 1, "one publish");
    assert_ne!(
        harness.export(FRAME.0, FRAME.1),
        march,
        "September is drawn from July's faces, not January's"
    );
}

/// A publish requested before the first-frame packs land waits for them, and
/// goes out from the cube rather than from the grid.
#[test]
fn a_publish_at_startup_waits_for_the_first_frame_packs() {
    let _gpu = gpu();
    let fixture = CubeFixture::new("engine_cube_startup");
    let sink = Arc::new(RecordingSink::new(vec![screen("only", 0, 64, 32, true)]));
    let sink_for_config = Arc::clone(&sink);
    let harness = Harness::start(|config| {
        fixture.configure(config);
        config.wallpaper = sink_for_config;
        config.params = blend_on(MARCH_10);
    });
    harness.engine.send(EngineCommand::RenderWallpaperNow);

    let deadline = std::time::Instant::now() + TIMEOUT;
    let mut ready_first = false;
    while let Ok(event) = harness.events.recv_deadline(deadline) {
        match event {
            EngineEvent::TexturesReady => ready_first = true,
            EngineEvent::WallpaperSet(result) => {
                assert!(result.is_ok(), "the publish should have succeeded");
                assert!(
                    ready_first,
                    "the wallpaper went out before the packs landed"
                );
                let report = harness.engine.memory_report().expect("a report");
                assert!(!rows(&report, "day_floor").is_empty(), "{report}");
                assert_eq!(sink.publications().len(), 1);
                return;
            }
            _ => {}
        }
    }
    panic!("no publish within {TIMEOUT:?}");
}

/// The live clock crossing into the next month between two renders holds a
/// publish until that month's floor is resident, even though nothing else has
/// made the engine draw since.
///
/// The clock starts a second before the middle of October and moves one
/// second, inside the two seconds the pause gate stays closed after the first
/// frame, so November's pack is unbuilt when the publish is asked for.
#[test]
fn a_publish_after_the_month_turned_waits_for_the_new_floor() {
    let _gpu = gpu();
    let fixture = CubeFixture::new("engine_cube_turn");
    let sink = Arc::new(RecordingSink::new(vec![screen("only", 0, 64, 32, true)]));
    let before_noon = time::Date::from_calendar_date(2026, time::Month::October, 16)
        .and_then(|date| date.with_hms(11, 59, 59))
        .expect("a valid date")
        .assume_utc();
    let clock = Arc::new(MockClock::new(before_noon));
    let (sink_for_config, clock_for_config) = (Arc::clone(&sink), Arc::clone(&clock));
    let harness = Harness::start(|config| {
        fixture.configure(config);
        config.wallpaper = sink_for_config;
        config.clock = clock_for_config;
        config.params = SceneParams {
            texture_index: 3,
            ..test_params()
        };
        config.params.datetime.use_custom = false;
    });
    harness.wait_for_textures("October");
    harness.settle();
    assert!(
        !fixture.pack_exists(PackKind::Day(10)),
        "the pause gate should have kept November unbuilt"
    );

    clock.advance(std::time::Duration::from_secs(1));
    harness.engine.send(EngineCommand::RenderWallpaperNow);

    let deadline = std::time::Instant::now() + TIMEOUT;
    let mut ready_first = false;
    while let Ok(event) = harness.events.recv_deadline(deadline) {
        match event {
            EngineEvent::TexturesReady => ready_first = true,
            EngineEvent::WallpaperSet(result) => {
                assert!(result.is_ok(), "the publish should have succeeded");
                assert!(
                    ready_first,
                    "the wallpaper went out before November's floor was resident"
                );
                assert!(fixture.pack_exists(PackKind::Day(10)));
                return;
            }
            _ => {}
        }
    }
    panic!("no publish within {TIMEOUT:?}");
}

/// A sink whose monitor query moves the clock on, so that the time a publish
/// draws at is later than the time the tick that began it read.
struct ClockMovingSink {
    clock: Arc<MockClock>,
    by: Duration,
}

impl WallpaperSink for ClockMovingSink {
    fn check_supported(&self) -> Result<(), String> {
        Ok(())
    }

    fn monitors(&self) -> Result<Vec<Monitor>, String> {
        self.clock.advance(self.by);
        Ok(vec![screen("only", 0, 64, 32, true)])
    }

    fn publish(&self, _job: &WallpaperJob) -> Result<String, String> {
        Ok(String::new())
    }
}

/// An export closes the pause gate however long the engine was idle before
/// it, so the rest of the year does not build through a wallpaper.
///
/// The clock moves a minute, far past the busy span of the first frames,
/// inside the publish: after the tick that began it read the time and before
/// the export draws. Only the export's own closing of the gate is left to keep
/// April unbuilt on the ticks that follow, and once the clock moves past the
/// span the export began, the gate opens and April is built.
#[test]
fn an_export_closes_the_pause_gate_however_long_the_engine_was_idle() {
    let _gpu = gpu();
    let fixture = CubeFixture::new("engine_cube_export_gate");
    let clock = Arc::new(MockClock::new(time::OffsetDateTime::UNIX_EPOCH));
    let clock_for_config = Arc::clone(&clock);
    let sink = Arc::new(ClockMovingSink {
        clock: Arc::clone(&clock),
        by: Duration::from_mins(1),
    });
    let harness = Harness::start(|config| {
        fixture.configure(config);
        config.wallpaper = sink;
        config.clock = clock_for_config;
        config.params = blend_on(MARCH_10);
    });
    harness.wait_for_textures("March");
    harness.settle();
    let april = PackKind::Day(3);
    assert!(
        !fixture.pack_exists(april),
        "the pause gate should have kept April unbuilt while the engine was busy"
    );

    assert!(
        harness.publish().is_ok(),
        "the publish should have succeeded"
    );
    harness.settle();
    // A build of the fixture's April takes milliseconds once the gate opens.
    let window = Instant::now() + Duration::from_secs(2);
    while Instant::now() < window {
        assert!(
            !fixture.pack_exists(april),
            "April was built after an export, so the export left the gate open"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    harness.advance(&clock, Duration::from_mins(1));
    let deadline = Instant::now() + TIMEOUT;
    while !fixture.pack_exists(april) {
        assert!(
            Instant::now() < deadline,
            "April was not built within {TIMEOUT:?} of the gate opening"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The Earth fixture cut as the golden suite cuts it (`EARTH_GEOMETRY` in
/// `tests/golden.rs`), into packs that flag its open water constant ocean.
const EARTH: tiles::Geometry = tiles::Geometry {
    face: 256,
    levels: 2,
    tile: 32,
    gutter: 4,
    floor: 64,
    mask: 128,
};

/// The night drawn alone reads the night half of the page table: a cell the
/// night pack flags constant ocean draws the night's ocean color.
///
/// The day and the night drawn alone go through the same path of the shader,
/// and with the atmosphere and the stars off nothing else is drawn over the
/// globe, so a cell that read the day half at night would come out exactly as
/// the day frame has it. The flat ocean is the day frame's commonest color over
/// the Pacific, and the night frame must not show it.
#[test]
fn night_mode_reads_the_night_half_of_the_page_table() {
    let _gpu = gpu();
    let dir = ScratchDir::new("engine_cube_night_alone");
    test_support::write_earth_fixture(&dir.join("textures"));
    let base = SceneParams {
        texture_index: 3,
        camera: CameraParams {
            longitude: -150.0,
            latitude: 0.0,
            ..test_params().camera
        },
        atmo_enabled: false,
        star_intensity: 0.0,
        ..test_params()
    };
    let harness = Harness::start(|config| {
        config.cube_textures = CubeTextures::resolve(&dir.join("textures"));
        config.tile_geometry = EARTH;
        config.cache_dir = Some(dir.join("cache"));
        config.params = base;
    });
    harness.wait_for_textures("the day, the night and the mask");

    let day = harness.picture(
        &SceneParams {
            texture_index: 1,
            ..base
        },
        FRAME,
    );
    let night = harness.picture(
        &SceneParams {
            texture_index: 2,
            ..base
        },
        FRAME,
    );
    let pixels = |frame: &[u8]| -> Vec<[u8; 4]> {
        frame
            .chunks_exact(4)
            .map(|px| px.try_into().expect("four bytes"))
            .collect()
    };
    let (day, night) = (pixels(&day), pixels(&night));
    let background = day[0];
    let mut counts = std::collections::HashMap::new();
    for px in day.iter().filter(|&&px| px != background) {
        *counts.entry(*px).or_insert(0_usize) += 1;
    }
    let (ocean, in_day) = counts
        .into_iter()
        .max_by_key(|&(_, count)| count)
        .expect("the globe is in the frame");
    let in_night = night.iter().filter(|&&px| px == ocean).count();
    assert!(
        in_day > day.len() / 20,
        "the day's flat ocean {ocean:?} covers only {in_day} pixels"
    );
    assert!(
        in_night * 100 < in_day,
        "the night frame shows the day's ocean {ocean:?} at {in_night} pixels \
         of the day frame's {in_day}, so the night read the day half"
    );
}

/// A pack that cannot be built is not waited for: the textures never become
/// ready, and a wallpaper goes out from what there is, the grid, instead of
/// being held.
#[test]
fn a_pack_that_fails_does_not_hold_a_publish() {
    let _gpu = gpu();
    let fixture = CubeFixture::new("engine_cube_failed");
    std::fs::create_dir_all(fixture.cache()).expect("create the cache directory");
    // A file where the packs' directory goes, so every build fails.
    std::fs::write(fixture.cache().join(tiles::CACHE_SUBDIR), b"in the way")
        .expect("block the packs' directory");
    let sink = Arc::new(RecordingSink::new(vec![screen("only", 0, 64, 32, true)]));
    let sink_for_config = Arc::clone(&sink);
    let harness = Harness::start(|config| {
        fixture.configure(config);
        config.wallpaper = sink_for_config;
        config.params = blend_on(MARCH_10);
    });
    assert!(
        harness.publish().is_ok(),
        "a publish with nothing left to wait for goes out"
    );
    assert_eq!(sink.publications().len(), 1);
    let report = harness.engine.memory_report().expect("a report");
    assert!(
        ["day_floor", "night_floor", "water_mask"]
            .iter()
            .all(|label| rows(&report, label).is_empty()),
        "nothing can be resident from packs that were never built:\n{report}"
    );
}

/// Dropping the engine while its packs build stops the transcoder and returns,
/// which is the notify callback's contract: it wakes the engine through a
/// channel that never blocks, so the join in the transcoder's drop cannot wait
/// on the engine thread that is doing the dropping.
#[test]
fn an_engine_stops_promptly_while_its_packs_build() {
    let _gpu = gpu();
    let fixture = CubeFixture::new("engine_cube_shutdown");
    let mut config = EngineConfig::headless(FRAME);
    fixture.configure(&mut config);
    config.params = blend_on(MARCH_10);
    let engine = sunlit_core::engine::start(config).expect("an engine");

    let (done_tx, done_rx) = crossbeam_channel::bounded(1);
    std::thread::spawn(move || {
        engine.shutdown();
        let _ = done_tx.send(());
    });
    assert!(
        done_rx.recv_timeout(TIMEOUT).is_ok(),
        "the engine did not stop within {TIMEOUT:?}"
    );
}
