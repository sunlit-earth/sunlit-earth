use std::path::Path;
use std::time::Duration;

use sunlit_core::assets::mailbox::DecodedTextureMessage;
use sunlit_core::assets::mailbox::TextureMailbox;
use sunlit_core::assets::texture_loader::DecodedImage;
use sunlit_core::engine::EngineCommand;
use sunlit_core::engine::EngineConfig;
use sunlit_core::engine::EngineEvent;
use sunlit_core::params::SceneParams;

use crate::groups::{FRAME, SURFACE_WIDTH, surface};
use crate::harness::{Harness, TIMEOUT, gpu, has_lit_pixels, test_params};
use crate::memory::expected_widths;
use crate::sinks::screen;
use crate::test_support::ScratchDir;

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

/// The day and night maps blended, which is the mode that needs both
/// file-backed slots and the composite bind group built from them.
pub(crate) fn blend_params() -> SceneParams {
    SceneParams {
        texture_index: 3,
        ..test_params()
    }
}

#[test]
fn a_resolution_switch_reloads_the_textures_in_both_directions() {
    let gpu = gpu();
    let harness = surface(&gpu);
    let rgba = harness.picture(&blend_params(), FRAME);
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
pub(crate) fn decoded(
    slot_index: usize,
    width: u32,
    generation: u64,
    value: u8,
) -> DecodedTextureMessage {
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
pub(crate) fn lowering_the_resolution_lowers_the_process_footprint() {
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
    // Not a scratch directory: this is the downscale cache, and what it holds
    // is two halved copies of the 8K assets that cost about six seconds each to
    // build. It is keyed on the source's size and modification time, so a run
    // that finds it warm is reading exactly what it would have written.
    let cache = crate::test_support::scratch_root().join("engine_resolution_memory_cache");
    std::fs::create_dir_all(&cache).expect("create the downscale cache");

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
