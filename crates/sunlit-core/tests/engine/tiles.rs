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
use sunlit_core::scene::camera::{CameraParams, OrbitalCamera};

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
        // A clock that stands still, so the move is a jump and never a drag
        // however soon after the first frame it comes.
        config.clock = std::sync::Arc::new(sunlit_core::engine::clock::MockClock::new(
            time::OffsetDateTime::UNIX_EPOCH,
        ));
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

/// The centers of two patches of the Earth fixture's March ice, by latitude
/// and longitude in degrees: the pack north of Franz Josef Land, and the
/// marginal ice of the Barents Sea, streaks of ice and open water.
const PACK: (f64, f64) = (85.0, 40.0);
const MARGINAL_ICE: (f64, f64) = (75.0, 44.0);

/// A patch is everything within this distance of its center: 200 km across.
const PATCH_RADIUS_KM: f64 = 100.0;

const EARTH_RADIUS_KM: f64 = 6371.0;

/// The side of the square frames the patches are rendered into.
const PATCH_FRAME: u32 = 512;

/// The fewest pixels a patch fills in the farthest of its two frames.
const PATCH_PIXELS: usize = 64;

/// The day surface alone at `zoom` over `center`, on the first of March, with
/// no atmosphere over it.
fn march_over(center: (f64, f64), zoom: f32) -> SceneParams {
    #[expect(clippy::cast_possible_truncation, reason = "degrees")]
    let mut params = day_over(center.1 as f32, center.0 as f32, zoom);
    params.datetime.custom_day_of_year = 60;
    params.atmo_enabled = false;
    params
}

/// The byte offsets of the pixels of a square frame drawn at `params` whose
/// surface lies within [`PATCH_RADIUS_KM`] of `center`.
fn patch_pixels(params: &SceneParams, center: (f64, f64)) -> Vec<usize> {
    let inverse = OrbitalCamera::from_params(&params.camera)
        .mvp_matrix(1.0)
        .as_dmat4()
        .inverse();
    let (lat, lon) = (center.0.to_radians(), center.1.to_radians());
    let toward = glam::DVec3::new(lat.cos() * lon.sin(), lat.sin(), lat.cos() * lon.cos());
    let size = f64::from(PATCH_FRAME);
    let mut pixels = Vec::new();
    for y in 0..PATCH_FRAME {
        for x in 0..PATCH_FRAME {
            let ndc_x = (f64::from(x) + 0.5) / size * 2.0 - 1.0;
            let ndc_y = 1.0 - (f64::from(y) + 0.5) / size * 2.0;
            let near = inverse.project_point3(glam::DVec3::new(ndc_x, ndc_y, 0.0));
            let far = inverse.project_point3(glam::DVec3::new(ndc_x, ndc_y, 1.0));
            let ray = (far - near).normalize();
            let half_b = near.dot(ray);
            let discriminant = half_b * half_b - (near.length_squared() - 1.0);
            if discriminant < 0.0 {
                continue;
            }
            let surface = (near + ray * (-half_b - discriminant.sqrt())).normalize();
            if surface.dot(toward).clamp(-1.0, 1.0).acos() * EARTH_RADIUS_KM <= PATCH_RADIUS_KM {
                pixels.push(((y * PATCH_FRAME + x) * 4) as usize);
            }
        }
    }
    pixels
}

/// The farthest zoom, in steps of a hundredth, at which the patch around
/// `center` fills more than [`PATCH_PIXELS`] pixels.
fn farthest_zoom_filling(center: (f64, f64)) -> f32 {
    (0..=100_u8)
        .rev()
        .map(|step| f32::from(step) / 100.0)
        .find(|&zoom| patch_pixels(&march_over(center, zoom), center).len() > PATCH_PIXELS)
        .expect("the patch fills the frame at the closest zoom")
}

/// The mean of each color channel over `pixels` of `frame`.
#[expect(clippy::cast_precision_loss, reason = "a frame's worth of pixels")]
fn patch_mean(frame: &[u8], pixels: &[usize]) -> [f64; 3] {
    let mut sum = [0.0; 3];
    for &at in pixels {
        for (channel, total) in sum.iter_mut().enumerate() {
            *total += f64::from(frame[at + channel]);
        }
    }
    sum.map(|total| total / pixels.len() as f64)
}

/// Rec. 709 luminance of the pixel at `at`, on the 8-bit values.
fn luminance(frame: &[u8], at: usize) -> f64 {
    0.2126 * f64::from(frame[at])
        + 0.7152 * f64::from(frame[at + 1])
        + 0.0722 * f64::from(frame[at + 2])
}

/// The luminance, on the 8-bit values, at and above which a pixel is ice, and
/// at and below which it is open water: the two ends of the shader's ramp.
const ICE_LUMA: f64 = 76.0;
const WATER_LUMA: f64 = 52.0;

/// The sea ice is part of the picture and not of the view: a 200 km patch of
/// the pack and one of marginal ice keep their mean color, within 2 levels a
/// channel, from the closest zoom, about 2 km a pixel, to the farthest at
/// which the patch still fills 64 pixels, about 21 km a pixel. The fixture's
/// finest level is about 39 km a texel, so both frames read it, magnified.
#[test]
fn a_patch_of_sea_ice_keeps_its_mean_color_from_the_closest_zoom_to_the_farthest() {
    let _gpu = gpu();
    let dir = ScratchDir::new("engine_tiles_sea_ice");
    test_support::write_earth_fixture(&dir.join("textures"));
    let harness = start(&dir, |config| config.params = march_over(PACK, 0.0));
    harness.wait_for_textures("March's floor");

    for (what, center) in [("pack", PACK), ("marginal ice", MARGINAL_ICE)] {
        let near = march_over(center, 0.0);
        let far = march_over(center, farthest_zoom_filling(center));
        let near_pixels = patch_pixels(&near, center);
        let near_frame = harness.picture(&near, (PATCH_FRAME, PATCH_FRAME));
        let far_frame = harness.picture(&far, (PATCH_FRAME, PATCH_FRAME));
        let near_mean = patch_mean(&near_frame, &near_pixels);
        let far_mean = patch_mean(&far_frame, &patch_pixels(&far, center));

        #[expect(clippy::cast_precision_loss, reason = "a frame's worth of pixels")]
        let share = |keep: fn(f64) -> bool| {
            let kept = near_pixels
                .iter()
                .filter(|&&at| keep(luminance(&near_frame, at)))
                .count();
            kept as f64 / near_pixels.len() as f64
        };
        let (ice, water) = (share(|l| l >= ICE_LUMA), share(|l| l <= WATER_LUMA));
        println!(
            "{what}: {near_mean:.2?} at zoom 0, {far_mean:.2?} at zoom {}; \
             {ice:.2} of the patch ice, {water:.2} open water",
            far.camera.zoom
        );
        if center == PACK {
            assert!(ice >= 1.0, "the pack patch is {ice:.2} ice");
        } else {
            assert!(
                ice > 0.25 && water > 0.25,
                "the marginal ice patch is {ice:.2} ice and {water:.2} open water"
            );
        }
        assert!(
            (0..3).all(|channel| (near_mean[channel] - far_mean[channel]).abs() <= 2.0),
            "the {what} patch's mean is {near_mean:.2?} near and {far_mean:.2?} far"
        );
    }
}
