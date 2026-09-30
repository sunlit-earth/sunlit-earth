//! The tiles above the floor: what the loader makes resident as the camera
//! moves and the resolution setting changes, what it does with a tile it
//! cannot read, and the frames the renderer draws from them.
//!
//! Every case has an engine of its own over the Earth fixture's cube, cut as
//! the golden suite cuts it, whose continents give the tiles something to
//! refine.

use std::time::Instant;

use sunlit_core::assets::cube_layout::CubeTextures;
use sunlit_core::assets::tiles::{self, PackKind};
use sunlit_core::engine::{EngineCommand, TileReport};
use sunlit_core::params::SceneParams;
use sunlit_core::scene::camera::CameraParams;

use crate::harness::{Harness, TIMEOUT, gpu, test_params};
use crate::test_support::{self, ScratchDir};

/// The Earth fixture cut as `tests/golden.rs` cuts it.
pub(crate) const EARTH: tiles::Geometry = tiles::Geometry {
    face: 256,
    levels: 2,
    tile: 32,
    gutter: 4,
    floor: 64,
    mask: 128,
};

/// The day surface alone, at `zoom` over `longitude` and
/// `latitude`.
pub(crate) fn day_over(longitude: f32, latitude: f32, zoom: f32) -> SceneParams {
    SceneParams {
        texture_index: 1,
        camera: CameraParams {
            longitude,
            latitude,
            zoom,
            ..test_params().camera
        },
        ..test_params()
    }
}

/// An engine over the Earth fixture `test_support::write_earth_fixture` wrote
/// into `dir`.
pub(crate) fn start(
    dir: &ScratchDir,
    configure: impl FnOnce(&mut sunlit_core::engine::EngineConfig),
) -> Harness {
    Harness::start(|config| {
        config.cube_textures = CubeTextures::resolve(&dir.join("textures"));
        config.tile_geometry = EARTH;
        config.cache_dir = Some(dir.join("cache"));
        config.texture_resolution = 8192;
        configure(config);
    })
}

pub(crate) fn tile_report(harness: &Harness) -> TileReport {
    *harness
        .engine
        .tile_report()
        .expect("the engine answers")
        .expect("the engine draws from the cube")
}

/// The report once every tile taken from the wanted set is resident or
/// failed, and `until` holds of it.
pub(crate) fn settled(
    harness: &Harness,
    what: &str,
    until: impl Fn(&TileReport) -> bool,
) -> TileReport {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let report = tile_report(harness);
        let done = report
            .wanted
            .iter()
            .all(|id| report.resident.contains(id) || report.failed.contains(id));
        if done && until(&report) {
            return report;
        }
        assert!(Instant::now() < deadline, "{what}: {report:#?}");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Every tile blob of every day pack in `cache` fails its checksum, while the
/// headers, the indexes and the floors stay whole.
pub(crate) fn damage_the_day_tiles(cache: &std::path::Path) {
    for month in 0..12 {
        let path = tiles::pack_path(cache, PackKind::Day(month));
        let pack = tiles::Pack::open(&path).expect("a built pack");
        let mut bytes = std::fs::read(&path).expect("read the pack");
        for entry in pack.entries() {
            if !entry.ocean && !entry.whole_face {
                bytes[usize::try_from(entry.offset).expect("an offset")] ^= 0xFF;
            }
        }
        drop(pack);
        std::fs::write(&path, bytes).expect("write the damaged pack");
    }
}

/// The mean absolute difference of the color channels of two frames, and how
/// many pixels differ by more than `by` in some channel.
pub(crate) fn difference(a: &[u8], b: &[u8], by: u8) -> (f64, usize) {
    assert_eq!(a.len(), b.len());
    let (mut sum, mut over) = (0_u64, 0);
    for (pa, pb) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        let worst = (0..3).map(|c| pa[c].abs_diff(pb[c])).max().expect("three");
        sum += (0..3)
            .map(|c| u64::from(pa[c].abs_diff(pb[c])))
            .sum::<u64>();
        over += usize::from(worst > by);
    }
    #[expect(clippy::cast_precision_loss, reason = "a frame's worth of channels")]
    let mean = sum as f64 / (a.len() / 4 * 3) as f64;
    (mean, over)
}

/// The last preview frame among the events queued so far.
pub(crate) fn last_frame(harness: &Harness) -> Option<Vec<u8>> {
    let mut last = None;
    while let Ok(event) = harness.events.try_recv() {
        if let sunlit_core::engine::EngineEvent::PreviewFrame { rgba, .. } = event {
            last = Some(rgba);
        }
    }
    last
}

/// The frame drawn from the floor alone at `view`, the engine being at the
/// resolution setting's lowest, which allows no tile, as a preview frame.
fn floor_frame(harness: &Harness, view: &SceneParams) -> Vec<u8> {
    harness.wait_for_textures("the floors");
    harness.settle();
    let (frame, _, _) = harness.frame_after_change(view);
    frame
}

/// Tiles that land are drawn into the preview frame that follows, and refine
/// the floor rather than replace it: the frame is close to the floor's, and
/// differs from it in detail.
///
/// Through `Renderer::upload_tiles`: without the upload's own redraw the
/// render after it is skipped as unchanged, and the last frame is the floor's;
/// without the bind groups rebuilt round the array its first tile creates, the
/// page table names layers of the one-layer stand-in, and the tiles draw
/// black.
#[test]
fn tiles_that_land_are_drawn_into_the_next_preview_frame() {
    let _gpu = gpu();
    let dir = ScratchDir::new("engine_tiles_frames");
    test_support::write_earth_fixture(&dir.join("textures"));
    let view = day_over(20.0, 0.0, 0.1);
    let harness = start(&dir, |config| {
        config.params = day_over(25.0, 0.0, 0.1);
        config.texture_resolution = 2048;
    });
    let floor = floor_frame(&harness, &view);

    harness
        .engine
        .send(EngineCommand::SetTextureResolution(8192));
    let report = settled(&harness, "the tiles", |r| {
        r.epoch == 1 && !r.wanted.is_empty()
    });
    let tiled = last_frame(&harness).expect("a preview frame after the tiles landed");

    let (mean, detail) = difference(&floor, &tiled, 16);
    let (_, far) = difference(&floor, &tiled, 64);
    assert!(
        detail > floor.len() / 4 / 100,
        "{} tiles are resident and only {detail} pixels differ from the floor by more than 16",
        report.resident.len()
    );
    assert!(
        mean < 8.0 && far < floor.len() / 4 / 100,
        "the tiles do not refine the floor: a mean difference of {mean:.2}, \
         {far} pixels off by more than 64"
    );
}

/// A camera that moves brings the tiles of its new view in and evicts, of the
/// old view, as many as the array needs room for and no more.
#[test]
fn a_camera_move_brings_its_tiles_in_and_evicts_what_it_needs_room_for() {
    const LAYERS: u32 = 120;
    let _gpu = gpu();
    let dir = ScratchDir::new("engine_tiles_move");
    test_support::write_earth_fixture(&dir.join("textures"));
    let (africa, pacific) = (day_over(20.0, 0.0, 0.1), day_over(-150.0, 0.0, 0.1));
    let harness = start(&dir, |config| {
        config.params = africa;
        config.tile_layers = Some(LAYERS);
    });
    harness.wait_for_textures("the floors");
    let before = settled(&harness, "the tiles over Africa", |r| !r.wanted.is_empty());
    assert_eq!(before.capacity, LAYERS);

    harness.settle_at(&pacific);
    let after = settled(&harness, "the tiles over the Pacific", |r| {
        r.wanted != before.wanted
    });
    let union: std::collections::HashSet<_> = before.wanted.iter().chain(&after.wanted).collect();
    assert!(
        before.wanted.len() <= LAYERS as usize
            && after.wanted.len() <= LAYERS as usize
            && union.len() > LAYERS as usize,
        "the premise: each view fits the array and the two together do not \
         ({} and {}, {} together, {LAYERS} layers)",
        before.wanted.len(),
        after.wanted.len(),
        union.len()
    );
    assert_eq!(after.beyond, 0);
    assert!(after.wanted.iter().all(|id| after.resident.contains(id)));
    assert_eq!(
        after.resident.len(),
        LAYERS as usize,
        "eviction frees only the layers the new tiles need"
    );
    let evicted = before
        .wanted
        .iter()
        .filter(|id| !after.resident.contains(id))
        .count();
    assert_eq!(evicted, union.len() - LAYERS as usize);
}

/// Tiles whose blobs fail their checksum are read once each, recorded as
/// failed, and drawn from what is resident above them, which for every tile
/// of a day pack is the floor: the frame is the floor's own, up to the
/// constant-ocean cells the page table draws from the pack's index.
#[test]
fn tiles_that_cannot_be_read_are_drawn_from_the_floor() {
    let _gpu = gpu();
    let dir = ScratchDir::new("engine_tiles_failed");
    test_support::write_earth_fixture(&dir.join("textures"));
    let textures = CubeTextures::resolve(&dir.join("textures"));
    let cache = dir.join("cache");
    let cancel = std::sync::atomic::AtomicBool::new(false);
    for kind in PackKind::all() {
        tiles::ensure_pack(&cache, kind, &textures, &EARTH, &cancel).expect("build the pack");
    }
    damage_the_day_tiles(&cache);
    let view = day_over(20.0, 0.0, 0.1);
    let harness = start(&dir, |config| {
        config.params = day_over(25.0, 0.0, 0.1);
        config.texture_resolution = 2048;
    });
    let floor = floor_frame(&harness, &view);

    harness
        .engine
        .send(EngineCommand::SetTextureResolution(8192));
    let report = settled(&harness, "the tiles", |r| {
        r.epoch == 1 && !r.wanted.is_empty()
    });
    let drawn = last_frame(&harness).expect("a preview frame after the switch");
    assert!(report.resident.is_empty(), "{report:#?}");
    let mut wanted = report.wanted.clone();
    wanted.sort_by_key(|id| id.key);
    assert_eq!(report.failed, wanted, "every wanted tile failed");
    assert_eq!(report.reads, wanted.len() as u64, "each was read once");
    let (_, off) = difference(&floor, &drawn, 3);
    assert_eq!(off, 0, "the frame is not the floor's");

    harness.settle_at(&SceneParams {
        star_intensity: 0.5,
        ..view
    });
    assert_eq!(
        tile_report(&harness).reads,
        report.reads,
        "a failed tile was read again"
    );
}

/// A change of the resolution setting purges the tile array, and what comes
/// back is what the new setting allows: one level fewer at 4096, nothing at
/// 2048, where the array itself goes, and both levels again at 8192.
#[test]
fn a_resolution_switch_purges_the_tiles_and_takes_the_new_cap() {
    let _gpu = gpu();
    let dir = ScratchDir::new("engine_tiles_resolution");
    test_support::write_earth_fixture(&dir.join("textures"));
    let harness = start(&dir, |config| config.params = day_over(20.0, 0.0, 0.1));
    harness.wait_for_textures("the floors");
    let finest = tiles::Geometry::level_of(EARTH.face);
    let levels = |r: &TileReport| {
        let mut levels: Vec<u8> = r.resident.iter().map(|id| id.key.level).collect();
        levels.sort_unstable();
        levels.dedup();
        levels
    };
    let has_array = || {
        let report = harness.engine.memory_report().expect("a report");
        report
            .expected
            .iter()
            .any(|texture| texture.label == "tile_array")
    };

    let full = settled(&harness, "8192", |r| !r.wanted.is_empty());
    assert_eq!(levels(&full), [finest - 1, finest]);
    assert!(has_array());

    harness
        .engine
        .send(EngineCommand::SetTextureResolution(4096));
    let half = settled(&harness, "4096", |r| {
        r.epoch == full.epoch + 1 && !r.wanted.is_empty()
    });
    assert_eq!(
        levels(&half),
        [finest - 1],
        "only the coarser level is resident"
    );

    harness
        .engine
        .send(EngineCommand::SetTextureResolution(2048));
    let none = settled(&harness, "2048", |r| r.epoch == full.epoch + 2);
    assert!(
        none.wanted.is_empty() && none.resident.is_empty(),
        "{none:#?}"
    );
    assert!(!has_array(), "the array is freed when no tile is allowed");

    harness
        .engine
        .send(EngineCommand::SetTextureResolution(8192));
    let back = settled(&harness, "8192 again", |r| {
        r.epoch == full.epoch + 3 && !r.wanted.is_empty()
    });
    assert_eq!(levels(&back), [finest - 1, finest]);
    assert_eq!(
        back.wanted, full.wanted,
        "the same view wants the same tiles"
    );
}
