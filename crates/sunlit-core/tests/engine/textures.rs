//! Switching the resolution setting, what the mailbox does about the overlay
//! decodes it outruns, and what the packs of the real faces hold.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use sunlit_core::assets::cube_layout::{CubeTextures, FaceSet};
use sunlit_core::assets::mailbox::DecodedTextureMessage;
use sunlit_core::assets::mailbox::TextureMailbox;
use sunlit_core::assets::texture_loader::{Channels, DecodedImage, Pixels};
use sunlit_core::assets::tiles::{self, PackKind, TileKey};
use sunlit_core::engine::EngineCommand;
use sunlit_core::engine::EngineConfig;
use sunlit_core::params::SceneParams;
use sunlit_core::renderer::SlotLayout;

use crate::groups::{SURFACE_RESOLUTION, surface};
use crate::harness::{Harness, TIMEOUT, gpu, has_lit_pixels, test_params};
use crate::memory::expected_widths;
use crate::sinks::screen;
use crate::test_support::ScratchDir;
use crate::tiles::settled;

/// A panorama file for the cases that have an engine of their own.
///
/// Small and bright rather than realistic: the Milky Way is the one slot the
/// resolution setting still reloads, and these cases are about that reload.
struct PanoramaFixture {
    dir: ScratchDir,
}

impl PanoramaFixture {
    /// A panorama at a chosen width. Not one of the widths the combo box
    /// offers, because the renderer takes any width as a cap and the three on
    /// offer are the config's business.
    fn with_width(name: &str, width: u32) -> Self {
        let dir = ScratchDir::new(name);
        let mut img = image::RgbaImage::new(width, width / 2);
        for (x, y, px) in img.enumerate_pixels_mut() {
            #[allow(clippy::cast_possible_truncation)]
            let v = 160 + ((x + y) % 96) as u8;
            *px = image::Rgba([v, v, v, 255]);
        }
        img.save(dir.join("panorama.png"))
            .expect("write the panorama fixture");
        Self { dir }
    }

    /// The paths as `texture_paths` wants them: no Moon, and the panorama.
    fn paths(&self) -> Vec<Option<std::path::PathBuf>> {
        vec![None, Some(self.dir.join("panorama.png"))]
    }

    /// An engine of its own at `width`, with the panorama wanted and the
    /// injected mailbox, if any.
    fn start(&self, width: u32, mailbox: Option<TextureMailbox>) -> Harness {
        let paths = self.paths();
        Harness::start(move |config| {
            config.texture_paths = paths;
            config.texture_resolution = width;
            config.mailbox = mailbox;
            config.params = panorama_wanted();
        })
    }
}

/// The grid, with the Milky Way switched on, since an overlay's texture is
/// loaded when it is wanted and not before.
fn panorama_wanted() -> SceneParams {
    SceneParams {
        milky_way_intensity: 1.0,
        ..test_params()
    }
}

/// The Milky Way's slot in production's layout: the grid, the Moon, then it.
const MILKY_WAY_SLOT: usize = 2;

/// Block until the Milky Way's texture is `width` wide in the memory report.
fn wait_for_panorama_width(harness: &Harness, width: u32, what: &str) {
    let deadline = std::time::Instant::now() + TIMEOUT;
    loop {
        let report = harness.engine.memory_report().expect("a report");
        if expected_widths(&report, "milky_way_texture") == [width] {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{what}: the panorama did not reach {width} within {TIMEOUT:?}:\n{report}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The day and night surfaces blended, the mode that wants every cube and
/// both halves of the page table.
pub(crate) fn blend_params() -> SceneParams {
    SceneParams {
        texture_index: 3,
        ..test_params()
    }
}

/// The switch is idempotent, so a client that re-sends the current setting
/// (the reset and load-defaults callbacks both do) costs nothing.
#[test]
fn a_switch_to_the_current_resolution_does_nothing() {
    let gpu = gpu();
    let harness = surface(&gpu);
    harness.settle_at(&blend_params());
    settled(harness, "the tiles", |_| true);
    harness.settle();

    harness.set_texture_resolution(SURFACE_RESOLUTION);
    assert!(
        harness.drained_frame(Duration::from_millis(500)).is_none(),
        "a switch to the setting already in force must not re-render"
    );
}

/// Three switches in a row end on the last one: the tiles of every one are
/// purged, and what the last allows, no tile at all, is what is in force.
#[test]
fn switches_in_quick_succession_end_on_the_last_one() {
    let gpu = gpu();
    let harness = surface(&gpu);
    harness.settle_at(&blend_params());
    let before = settled(harness, "the widest setting's tiles", |r| {
        !r.wanted.is_empty()
    });

    for width in [4096, SURFACE_RESOLUTION, 2048] {
        harness.set_texture_resolution(width);
    }

    let after = settled(harness, "after three switches in a row", |r| {
        r.epoch == before.epoch + 3
    });
    assert!(
        after.wanted.is_empty() && after.resident.is_empty(),
        "the last switch, which allows no tile, must be the one in force: {after:#?}"
    );
    let rgba = harness.export(256, 128);
    assert!(has_lit_pixels(&rgba), "the floors still draw the globe");
}

/// One decoded texture for a slot, as a background loader would post it.
fn decoded(slot_index: usize, width: u32, generation: u64, value: u8) -> DecodedTextureMessage {
    let height = width / 2;
    DecodedTextureMessage {
        slot_index,
        result: Ok(DecodedImage {
            pixels: Pixels::new(vec![value; (width as usize) * (height as usize) * 4]),
            width,
            height,
            channels: Channels::Rgba,
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
/// The Milky Way is switched off before the switch, so the purge is followed
/// by no reload and nothing the engine does can supply a texture afterwards:
/// the slot can only hold the new width if the fresh post survived, which is
/// what makes this fail when the guard is removed. An engine of its own for the
/// mailbox, which is read once when the engine is built.
#[test]
fn a_stale_decode_must_not_replace_the_texture_that_superseded_it() {
    const WIDE: u32 = 256;

    let _gpu = gpu();
    let fixture = PanoramaFixture::with_width("engine_resolution_stale", WIDE);
    let mailbox = TextureMailbox::new(SlotLayout::new(fixture.paths().len()).count());
    let harness = fixture.start(WIDE, Some(mailbox.clone()));
    wait_for_panorama_width(&harness, WIDE, "at startup");

    harness.settle_at(&test_params());
    harness.set_texture_resolution(WIDE / 2);
    harness.settle();
    let report = harness.engine.memory_report().expect("a report");
    assert!(
        expected_widths(&report, "milky_way_texture").is_empty(),
        "the switch purges the panorama:\n{report}"
    );

    // The replacement, parked first, and then the superseded decode of the old
    // width arriving late. Nothing pokes the engine in between, so the drain
    // that follows sees whatever the mailbox kept.
    mailbox.post(decoded(MILKY_WAY_SLOT, WIDE / 2, 1, 255));
    mailbox.post(decoded(MILKY_WAY_SLOT, WIDE, 0, 0));
    harness.engine.send(EngineCommand::Poke);

    wait_for_panorama_width(&harness, WIDE / 2, "after the stale arrival");
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
    // Two file-backed paths need five slots: the grid, both of them, the
    // clouds and their background.
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
/// dirtied would show the black panorama it carries.
#[test]
fn a_stale_arrival_produces_no_frame_at_all() {
    const WIDE: u32 = 256;

    let _gpu = gpu();
    let fixture = PanoramaFixture::with_width("engine_resolution_stale_alone", WIDE);
    let mailbox = TextureMailbox::new(SlotLayout::new(fixture.paths().len()).count());
    let harness = fixture.start(WIDE, Some(mailbox.clone()));
    wait_for_panorama_width(&harness, WIDE, "at startup");
    harness.set_texture_resolution(WIDE / 2);
    wait_for_panorama_width(&harness, WIDE / 2, "after the switch");
    harness.drained_frame(Duration::from_millis(300));

    mailbox.post(decoded(MILKY_WAY_SLOT, WIDE, 0, 0));
    harness.engine.send(EngineCommand::Poke);
    assert!(
        harness.drained_frame(Duration::from_millis(500)).is_none(),
        "a discarded arrival must not reach the GPU, and so must not produce a frame"
    );
}

/// A resolution change while the first load is still running converges.
///
/// The first frame is the render that spawns the panorama's decode, and a
/// panorama this wide takes that decode well past the switch sent right after
/// it, so the purge happens with a decode in flight. Its post is discarded when
/// it arrives; what must still happen is the reload, and the only evidence that
/// it did is the slot reaching the new width at all. An engine of its own,
/// because the load it interrupts is the one the engine starts with.
#[test]
fn a_switch_while_the_first_load_is_running_still_converges() {
    const WIDE: u32 = 2048;

    let _gpu = gpu();
    let fixture = PanoramaFixture::with_width("engine_resolution_midload", WIDE);
    let harness = fixture.start(WIDE, None);
    harness.next_frame();
    harness.set_texture_resolution(WIDE / 4);

    wait_for_panorama_width(&harness, WIDE / 4, "after a switch mid-load");
}

/// The halved copies earlier versions kept under the cache directory are
/// removed when an engine starts, and nothing else under it is.
#[test]
fn the_engine_removes_the_retired_downscales_at_startup() {
    let _gpu = gpu();
    let dir = ScratchDir::new("engine_retired_downscales");
    let cache = dir.join("cache");
    let retired = cache.join("texture_cache");
    std::fs::create_dir_all(&retired).expect("create the retired directory");
    std::fs::write(retired.join("day_4096.png"), b"a halved copy").expect("write a copy");
    let kept = cache.join("tile_cache");
    std::fs::create_dir_all(&kept).expect("create the tile cache");
    std::fs::write(kept.join("pack.bin"), b"a pack").expect("write a pack");

    let harness = Harness::start(move |config| config.cache_dir = Some(cache));
    let _ = harness.engine.memory_report();

    assert!(!retired.exists(), "the retired directory is still there");
    assert!(kept.join("pack.bin").exists(), "the tile cache was touched");
}

/// A wallpaper update asked for during a reload waits for the reload.
///
/// The switch purges the tiles, and "change the resolution, then click Set as
/// Wallpaper" is a natural sequence, so without the hold-back the floors alone
/// are what lands on the desktop. The publish waits for the tiles its screen
/// wants at the new setting, so it is the picture a second publish makes once
/// every tile has long been resident, and one publish goes out, not a
/// fallback and then the same wallpaper again.
#[test]
fn a_wallpaper_update_during_a_reload_waits_for_the_textures() {
    let gpu = gpu();
    let group = surface(&gpu);
    group
        .sink
        .set_monitors(vec![screen("only", 0, 1920, 1080, true)]);
    group.settle_at(&blend_params());
    settled(group, "the widest setting's tiles", |r| {
        !r.wanted.is_empty()
    });
    group.settle();
    assert!(
        group.sink.publications().is_empty(),
        "nothing has asked for a wallpaper yet"
    );

    // Both commands are handled before the engine ticks, so the purge has
    // already emptied the tile array when the publish is asked for.
    group.set_texture_resolution(4096);
    group.engine.send(EngineCommand::RenderWallpaperNow);
    assert!(
        group.wait_for_publish().is_ok(),
        "the publish should have succeeded"
    );
    settled(group, "the new setting's tiles", |_| true);
    group.settle();
    assert_eq!(
        group.sink.publications().len(),
        1,
        "exactly one frame should be published"
    );
    assert!(group.publish().is_ok());
    let publications = group.sink.publications();
    let pixels = |index: usize| {
        &publications[index].frames[0]
            .as_ref()
            .expect("the one screen is painted")
            .pixels
    };
    assert!(
        pixels(0) == pixels(1),
        "the wallpaper asked for during the reload is not the one its tiles make, \
         so it was published from the floors alone"
    );
}

#[allow(clippy::cast_precision_loss)]
pub(crate) fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// An asset in `textures/`, or the reason it is not usable.
///
/// `textures/**` is Git LFS, so a checkout without the objects holds pointer
/// files of a couple of hundred bytes, which exist as far as anything that only
/// asks about existence is concerned. Size is what tells the two apart, the same
/// check the guest staging in the xtask makes.
pub(crate) fn real_asset(name: &str) -> Result<std::path::PathBuf, String> {
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

/// The repository's cube faces, if this checkout has every one of them.
fn real_cube() -> Result<CubeTextures, String> {
    let dir = sunlit_core::assets::texture_loader::resolve_textures_dir(None)
        .ok_or_else(|| "there is no textures directory".to_owned())?;
    let cube = CubeTextures::resolve(&dir);
    if cube.is_complete() {
        Ok(cube)
    } else {
        Err(format!(
            "{} of the {} cube faces are there, the rest are Git LFS pointers or missing",
            cube.found(),
            CubeTextures::total()
        ))
    }
}

/// The repository's cube faces and a cache that outlives the run with every
/// pack of the year built from them, once for every case that reads it, or
/// why there are none: a few minutes the first time, and a positional read of
/// each index after it. A case takes the GPU before it reads this, so the
/// build, which holds every core for those minutes on a cold cache, is
/// serialized against the engine cases and their one minute waits like any
/// other case; taken again inside, the lock would never be had.
static REAL_PACKS: LazyLock<Result<(CubeTextures, PathBuf), String>> = LazyLock::new(|| {
    let cube = real_cube()?;
    let cache = crate::test_support::scratch_root().join("engine_resolution_memory_cache");
    let cancel = std::sync::atomic::AtomicBool::new(false);
    sunlit_core::assets::texture_loader::register_jxl_hook();
    for kind in PackKind::all() {
        tiles::ensure_pack(&cache, kind, &cube, &tiles::GEOMETRY, &cancel)
            .unwrap_or_else(|e| panic!("build the {kind:?} pack: {e:?}"));
    }
    Ok((cube, cache))
});

/// The fill the cube bake flattens open water to.
const FILL: [u8; 3] = [10, 30, 60];

/// How far the codec moves the flattened fill: every tile of open water on the
/// faces decodes within this of it on every channel.
const FILL_SPREAD: u8 = 5;

/// The luminance, Rec. 709 on the 8-bit values, at and above which the shader
/// starts taking the glint off a texel over open water: ice, or the part of a
/// texel ice covers.
const ICE_FOOT: f64 = 52.0;

/// One face of the real cube, decoded.
fn decoded_face(path: &Path) -> image::DynamicImage {
    let mut reader =
        image::ImageReader::open(path).unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
    reader.no_limits();
    reader
        .decode()
        .unwrap_or_else(|e| panic!("decode {}: {e}", path.display()))
}

/// RGBA8 texels of `size` a side, each the rounded mean of the four beneath it.
fn halved(texels: &[u8], size: usize) -> Vec<u8> {
    let half = size / 2;
    let mut out = Vec::with_capacity(half * half * 4);
    for row in 0..half {
        for col in 0..half {
            for channel in 0..4 {
                let at = |r: usize, c: usize| u32::from(texels[(r * size + c) * 4 + channel]);
                let sum = at(2 * row, 2 * col)
                    + at(2 * row, 2 * col + 1)
                    + at(2 * row + 1, 2 * col)
                    + at(2 * row + 1, 2 * col + 1);
                out.push(u8::try_from((sum + 2) / 4).expect("a mean of bytes"));
            }
        }
    }
    out
}

/// What the tiles of one day pack hold, against what its index says of them.
#[derive(Debug, Default)]
struct DayTiles {
    /// Tiles with a texel of ice over open water.
    with_ice: usize,
    /// Of those, the ones the pack flags constant ocean.
    ice_flagged: usize,
    /// Tiles whose window lies inside their face and is open water within
    /// [`FILL_SPREAD`] of the fill.
    open_water: usize,
    /// Of those, the ones the pack stores.
    open_water_stored: usize,
    /// Tiles whose window lies inside their face and is open water below the
    /// ice ramp's foot with a texel farther from the fill: ice too thin to
    /// reach the foot, at the edge of the pack.
    faint_ice: usize,
    /// Of those, the ones the pack stores.
    faint_ice_stored: usize,
}

/// Every tile of `pack`, a day pack cut from `faces` with `GEOMETRY`, held to
/// the texels it was cut from, and `mask`, the six open water faces at the
/// finest level, `true` for open water.
fn day_tiles(pack: &tiles::Pack, faces: &FaceSet, mask: &[Vec<bool>]) -> DayTiles {
    let geometry = tiles::GEOMETRY;
    let flagged: HashSet<TileKey> = pack
        .entries()
        .iter()
        .filter(|entry| entry.ocean)
        .map(|entry| entry.key)
        .collect();
    let (tile, gutter, layer) = (
        geometry.tile as usize,
        geometry.gutter as usize,
        geometry.layer() as usize,
    );
    let mut found = DayTiles::default();
    for (face, path) in faces.iter().enumerate() {
        let path = path.as_ref().expect("a complete cube");
        let mut texels = decoded_face(path).into_rgba8().into_raw();
        let mut water = mask[face].clone();
        let mut size = geometry.face as usize;
        for _ in 0..geometry.levels {
            let luma = |at: usize| {
                0.2126 * f64::from(texels[at * 4])
                    + 0.7152 * f64::from(texels[at * 4 + 1])
                    + 0.0722 * f64::from(texels[at * 4 + 2])
            };
            let bright: Vec<bool> = (0..size * size).map(|at| luma(at) >= ICE_FOOT).collect();
            for row in 0..size / tile {
                for col in 0..size / tile {
                    let key = TileKey {
                        level: tiles::Geometry::level_of(u32::try_from(size).expect("a face")),
                        face: u8::try_from(face).expect("a face index"),
                        row: u16::try_from(row).expect("a tile row"),
                        col: u16::try_from(col).expect("a tile column"),
                    };
                    let texels_of = |top: usize, left: usize, side: usize| {
                        (top..top + side)
                            .flat_map(move |r| (left..left + side).map(move |c| r * size + c))
                    };
                    if texels_of(row * tile, col * tile, tile).any(|at| water[at] && bright[at]) {
                        found.with_ice += 1;
                        found.ice_flagged += usize::from(flagged.contains(&key));
                    }
                    let inside = row * tile >= gutter
                        && col * tile >= gutter
                        && row * tile - gutter + layer <= size
                        && col * tile - gutter + layer <= size;
                    if !inside {
                        continue;
                    }
                    let window = || texels_of(row * tile - gutter, col * tile - gutter, layer);
                    if !window().all(|at| water[at] && !bright[at]) {
                        continue;
                    }
                    let stored = usize::from(!flagged.contains(&key));
                    if window().all(|at| {
                        (0..3).all(|c| texels[at * 4 + c].abs_diff(FILL[c]) <= FILL_SPREAD)
                    }) {
                        found.open_water += 1;
                        found.open_water_stored += stored;
                    } else {
                        found.faint_ice += 1;
                        found.faint_ice_stored += stored;
                    }
                }
            }
            texels = halved(&texels, size);
            water = (0..size / 2)
                .flat_map(|r| (0..size / 2).map(move |c| (r, c)))
                .map(|(r, c)| {
                    water[2 * r * size + 2 * c]
                        && water[2 * r * size + 2 * c + 1]
                        && water[(2 * r + 1) * size + 2 * c]
                        && water[(2 * r + 1) * size + 2 * c + 1]
                })
                .collect();
            size /= 2;
        }
    }
    found
}

/// The day packs of the real faces store every tile with sea ice in it and
/// flag the tiles of flattened open water constant ocean, and the ocean color
/// they draw those in stays on the fill, in every month. Ice is what the
/// shader takes the glint off, a texel over open water at or above the ramp's
/// foot, and flattened open water is open water within the codec's spread of
/// the fill. The tiles between the two, open water below the foot with a texel
/// off the fill, are ice too thin to reach the foot; they are counted and
/// printed, not held to either. A tile whose window crosses a face edge is left
/// out of the open water counts, since its gutter is read from the
/// neighboring face.
#[test]
fn the_real_day_packs_store_the_sea_ice_and_flag_the_open_water() {
    let _gpu = gpu();
    let (cube, cache) = match &*REAL_PACKS {
        Ok(found) => found.clone(),
        Err(why) => {
            println!("skipped: {why}; `git lfs pull` fetches the assets");
            return;
        }
    };
    let mask: Vec<Vec<bool>> = cube
        .mask
        .iter()
        .map(|path| {
            let path = path.as_ref().expect("a complete cube");
            decoded_face(path)
                .into_luma8()
                .into_raw()
                .into_iter()
                .map(|m| m == u8::MAX)
                .collect()
        })
        .collect();
    let months: Vec<(DayTiles, [u8; 4])> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..12)
            .map(|month| {
                let (faces, mask, cache) = (&cube.day[month], &mask, &cache);
                scope.spawn(move || {
                    let path = tiles::pack_path(cache, PackKind::Day(month));
                    let pack = tiles::Pack::open(&path).expect("a built pack");
                    (day_tiles(&pack, faces, mask), pack.ocean())
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a month's tally"))
            .collect()
    });
    for (month, (found, ocean)) in months.iter().enumerate() {
        println!("month {}: ocean {ocean:?}, {found:?}", month + 1);
    }
    for (month, (found, ocean)) in months.iter().enumerate() {
        let month = month + 1;
        assert!(
            ocean.iter().zip(FILL).all(|(&o, f)| o.abs_diff(f) <= 2),
            "month {month}'s ocean color {ocean:?} is more than 2 from the fill {FILL:?}"
        );
        assert!(
            found.with_ice > 0 && found.open_water > 0,
            "month {month} has no tile of ice or none of open water to hold: {found:?}"
        );
        assert_eq!(
            found.ice_flagged, 0,
            "month {month} flags tiles with sea ice constant ocean: {found:?}"
        );
        assert_eq!(
            found.open_water_stored, 0,
            "month {month} stores tiles of open water: {found:?}"
        );
    }
}

/// Going down a resolution has to give the memory back, which is the whole
/// point of the setting: on the real faces, at the widest setting, the tile
/// array holds the layers a 4K export at the zoom of the CPU adapter's peak
/// made resident, and at the narrowest it is gone, from the renderer's own
/// table and from wgpu's count of the texture memory it holds.
///
/// Process memory is not what this asserts on: WARP keeps the pages of a
/// destroyed texture for the next one, and five rounds of making 455 layers
/// resident and purging them leave the private bytes within 10 MiB of where
/// they started (`docs/testing.md`). The figure is printed for the record.
///
/// An engine of its own, over a cache that outlives the run: every pack of the
/// year is built before the engine starts, which takes a few minutes the first
/// time and a positional read of each index after it, so no build runs in the
/// background while the engine sits idle between the two reports.
#[test]
fn lowering_the_resolution_releases_the_tile_array() {
    const WIDE: u32 = 8192;
    const NARROW: u32 = 2048;

    let _gpu = gpu();
    let (cube, cache) = match &*REAL_PACKS {
        Ok(found) => found.clone(),
        Err(why) => {
            println!("skipped: {why}; `git lfs pull` fetches the assets");
            return;
        }
    };

    let harness = Harness::start(|config| {
        config.cube_textures = cube.clone();
        config.cache_dir = Some(cache.clone());
        config.texture_resolution = WIDE;
        // Asia at nine radii through the narrowest lens, where a CPU
        // adapter's 4K frame wants the most tiles (research section 24).
        config.params = SceneParams {
            texture_index: 3,
            camera: sunlit_core::scene::camera::CameraParams {
                fov_deg: 10.0,
                ..crate::tiles::day_over(100.0, 30.0, 0.45).camera
            },
            ..test_params()
        };
    });
    harness.wait_for_textures("at 8192");
    harness.export(3840, 2160);
    let loaded = settled(&harness, "at 8192", |r| !r.resident.is_empty());
    let wide = harness.engine.memory_report().expect("a report");
    let wide_private = sunlit_core::memory::snapshot().map(|s| s.private_bytes);
    println!(
        "at {WIDE}, {} tiles resident:\n{wide}",
        loaded.resident.len()
    );

    harness
        .engine
        .send(EngineCommand::SetTextureResolution(NARROW));
    settled(&harness, "at 2048", |r| {
        r.epoch > loaded.epoch && r.wanted.is_empty()
    });
    let rgba = harness.export(256, 128);
    assert!(has_lit_pixels(&rgba), "the floors should render");
    let narrow = harness.engine.memory_report().expect("a report");
    let narrow_private = sunlit_core::memory::snapshot().map(|s| s.private_bytes);
    println!("at {NARROW}:\n{narrow}");
    if let (Some(wide), Some(narrow)) = (wide_private, narrow_private) {
        println!(
            "private bytes: {:.1} MiB at {WIDE}, {:.1} MiB at {NARROW}",
            mib(wide),
            mib(narrow)
        );
    }

    let array: u64 = wide
        .expected
        .iter()
        .filter(|texture| texture.label == "tile_array")
        .map(sunlit_core::memory_report::ExpectedTexture::bytes)
        .sum();
    assert!(array > 0, "the array is resident at {WIDE}:\n{wide}");
    assert!(
        expected_widths(&narrow, "tile_array").is_empty(),
        "the array survived the switch:\n{narrow}"
    );
    assert!(
        crate::memory::but_the_day_floors(&narrow) + array
            <= crate::memory::but_the_day_floors(&wide),
        "the computed total should fall by the array's {:.1} MiB",
        mib(array)
    );
    let days = expected_widths(&narrow, "day_floor");
    assert!(
        !days.is_empty() && days.iter().all(|&width| width == tiles::GEOMETRY.floor),
        "the floors stay: {days:?}"
    );

    let (Ok(wide_measured), Ok(narrow_measured)) = (
        u64::try_from(wide.counters.texture_bytes),
        u64::try_from(narrow.counters.texture_bytes),
    ) else {
        panic!("wgpu reported negative texture memory");
    };
    if wide_measured == 0 {
        println!("skipping the counter check: this backend maintains no texture counter");
        return;
    }
    assert!(
        narrow_measured + array / 2 <= wide_measured,
        "wgpu holds {:.1} MiB of textures at {NARROW} against {:.1} MiB at {WIDE}, so the \
         array's {:.1} MiB was not released",
        mib(narrow_measured),
        mib(wide_measured),
        mib(array)
    );
}
