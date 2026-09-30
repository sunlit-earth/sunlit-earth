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

use sunlit_core::assets::cube_layout::CubeTextures;
use sunlit_core::assets::tiles::{self, PackKind};
use sunlit_core::engine::clock::MockClock;
use sunlit_core::engine::{EngineCommand, EngineConfig, EngineEvent};
use sunlit_core::params::SceneParams;

use crate::groups::FRAME;
use crate::harness::{Harness, TIMEOUT, gpu, has_lit_pixels, test_params};
use crate::sinks::{RecordingSink, screen};
use crate::test_support::{self, ScratchDir};

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

    /// Point an engine at the fixture, and at no flat surface.
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
/// the engine says the textures are ready, with nothing but the cube faces
/// behind it.
#[test]
fn the_first_frame_packs_make_the_textures_ready() {
    let _gpu = gpu();
    let fixture = CubeFixture::new("engine_cube_ready");
    let harness = Harness::start(|config| {
        fixture.configure(config);
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
        assert_eq!(found.len(), 1, "{label} is resident once:\n{report}");
        assert_eq!(
            (found[0].width, found[0].layers),
            (width, 6),
            "{label} is a cube of the fixture's size"
        );
    }
    assert!(
        rows(&report, "day_texture").is_empty() && rows(&report, "night_texture").is_empty(),
        "the flat maps are not loaded beside the cube:\n{report}"
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
    loop {
        match harness.events.recv_deadline(deadline) {
            Ok(EngineEvent::TexturesReady) => ready_first = true,
            Ok(EngineEvent::WallpaperSet(result)) => {
                assert!(result.is_ok(), "the publish should have succeeded");
                assert!(
                    ready_first,
                    "the wallpaper went out before September's floor was resident"
                );
                break;
            }
            Ok(_) => {}
            Err(_) => panic!("no publish within {TIMEOUT:?}"),
        }
    }
    assert!(fixture.pack_exists(PackKind::Day(8)));
    assert_eq!(sink.publications().len(), 1, "one publish");
    assert_ne!(
        harness.export(FRAME.0, FRAME.1),
        march,
        "September is drawn from July's faces, not January's"
    );
}

/// A pack that cannot be built is not waited for: the textures never become
/// ready, and a wallpaper goes out from what there is instead of being held.
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
