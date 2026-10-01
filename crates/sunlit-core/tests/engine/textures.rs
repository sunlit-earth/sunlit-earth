//! Switching the resolution setting, and what the mailbox does about the
//! overlay decodes it outruns.

use std::time::Duration;

use sunlit_core::assets::cube_layout::CubeTextures;
use sunlit_core::assets::mailbox::DecodedTextureMessage;
use sunlit_core::assets::mailbox::TextureMailbox;
use sunlit_core::assets::texture_loader::DecodedImage;
use sunlit_core::assets::tiles::{self, PackKind};
use sunlit_core::engine::EngineCommand;
use sunlit_core::engine::EngineConfig;
use sunlit_core::params::SceneParams;

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
    let mailbox = TextureMailbox::new(fixture.paths().len() + 2);
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
    // Two file-backed paths need four slots: the grid, both of them, the
    // clouds.
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
    let mailbox = TextureMailbox::new(fixture.paths().len() + 2);
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
/// year is built before the engine starts, which takes about a minute the first
/// time and a positional read of each index after it, so no build runs in the
/// background while the engine sits idle between the two reports.
#[test]
fn lowering_the_resolution_releases_the_tile_array() {
    const WIDE: u32 = 8192;
    const NARROW: u32 = 2048;

    let cube = match real_cube() {
        Ok(cube) => cube,
        Err(why) => {
            println!("skipped: {why}; `git lfs pull` fetches the assets");
            return;
        }
    };

    let _gpu = gpu();
    let cache = crate::test_support::scratch_root().join("engine_resolution_memory_cache");
    let cancel = std::sync::atomic::AtomicBool::new(false);
    sunlit_core::assets::texture_loader::register_jxl_hook();
    for kind in PackKind::all() {
        tiles::ensure_pack(&cache, kind, &cube, &tiles::GEOMETRY, &cancel)
            .unwrap_or_else(|e| panic!("build the {kind:?} pack: {e:?}"));
    }

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
        narrow.expected_bytes() + array <= wide.expected_bytes(),
        "the computed total should fall by the array's {:.1} MiB",
        mib(array)
    );
    assert_eq!(
        expected_widths(&narrow, "day_floor"),
        [tiles::GEOMETRY.floor],
        "the floors stay"
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
