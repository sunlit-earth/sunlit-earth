//! The sea ice atlas: drawn once it is resident, waited for while it is on
//! its way, and the globe as it was without it.

use std::path::{Path, PathBuf};

use sunlit_core::assets::cube_layout::CubeTextures;
use sunlit_core::assets::tiles;
use sunlit_core::engine::EngineCommand;
use sunlit_core::params::SceneParams;
use sunlit_core::scene::camera::CameraParams;

use crate::harness::{Harness, TIMEOUT, gpu, test_params};
use crate::test_support::{self, ScratchDir};

/// The Earth fixture's cut, as `cube.rs` and the golden suite have it.
const EARTH: tiles::Geometry = tiles::Geometry {
    face: 256,
    levels: 2,
    tile: 32,
    gutter: 4,
    floor: 64,
    mask: 128,
};

const FRAME: (u32, u32) = (256, 128);

/// The Arctic from above in mid-March, the day floor alone, with the ice on.
fn arctic() -> SceneParams {
    let mut params = SceneParams {
        texture_index: 1,
        camera: CameraParams {
            longitude: -30.0,
            latitude: 75.0,
            ..test_params().camera
        },
        atmo_enabled: false,
        star_intensity: 0.0,
        sea_ice: true,
        ..test_params()
    };
    params.datetime.use_custom = true;
    params.datetime.custom_day_of_year = 74;
    params.datetime.custom_hour = 12.0;
    params.datetime.custom_year = 2026;
    params
}

/// An engine over the Earth fixture's cube with `atlas` in the sea ice slot,
/// started at `params`.
fn engine_with(name: &str, atlas: Option<PathBuf>, params: SceneParams) -> (ScratchDir, Harness) {
    let dir = ScratchDir::new(name);
    test_support::write_earth_fixture(&dir.join("textures"));
    let harness = Harness::start(|config| {
        config.cube_textures = CubeTextures::resolve(&dir.join("textures"));
        config.tile_geometry = EARTH;
        config.cache_dir = Some(dir.join("cache"));
        config.texture_paths = vec![None, None, atlas];
        config.params = params;
    });
    harness.wait_for_textures("the day floor");
    (dir, harness)
}

/// How many bytes two pictures of the same size differ in.
fn differing(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).filter(|(x, y)| x != y).count()
}

/// Without an atlas the globe is drawn as it was before the ice: the setting
/// changes nothing, and nothing waits for a file that is not there.
#[test]
fn without_an_atlas_the_globe_draws_no_ice() {
    let _gpu = gpu();
    let (_dir, harness) = engine_with("engine_sea_ice_missing", None, arctic());
    let on = harness.picture(&arctic(), FRAME);
    let off = harness.picture(
        &SceneParams {
            sea_ice: false,
            ..arctic()
        },
        FRAME,
    );
    assert_eq!(
        differing(&on, &off),
        0,
        "the setting moved a globe with no atlas"
    );
}

/// The atlas is not decoded while the setting is off. Switched on, the very
/// next picture has the ice, because an export waits for a wanted atlas as it
/// does for its tiles, and switching it off takes the ice away again.
#[test]
fn the_ice_is_drawn_once_the_atlas_is_resident() {
    let _gpu = gpu();
    let atlas = test_support::earth_fixture_dir().join("sea_ice.jxl");
    let without = SceneParams {
        sea_ice: false,
        ..arctic()
    };
    let (_dir, harness) = engine_with("engine_sea_ice_fixture", Some(atlas), without);
    let off = harness.picture(&without, FRAME);
    let on = harness.picture(&arctic(), FRAME);
    assert!(
        differing(&on, &off) > on.len() / 50,
        "the Arctic in March shows hardly any ice: {} bytes differ",
        differing(&on, &off)
    );
    assert_eq!(
        harness.picture(&without, FRAME),
        off,
        "switched off, the ice stays in the picture"
    );
}

/// The shipped atlas decodes to 24 caps of 512 texels, 6 MiB of `R8Unorm`.
/// Skips with a reason in a checkout whose Git LFS objects are missing.
#[test]
fn the_shipped_atlas_is_resident_as_24_caps() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../textures/sea_ice_2010_2020.jxl");
    let size = std::fs::metadata(&path).map_or(0, |meta| meta.len());
    if size < 64 * 1024 {
        println!(
            "skipped: {} is {size} bytes, a Git LFS pointer or missing",
            path.display()
        );
        return;
    }
    let _gpu = gpu();
    let (_dir, harness) = engine_with("engine_sea_ice_shipped", Some(path), arctic());
    harness.wait_for_slot_texture("sea_ice_texture");
    let report = harness
        .engine
        .memory_report()
        .expect("the engine should answer with a report");
    let caps = report
        .expected
        .iter()
        .find(|texture| texture.label == "sea_ice_texture")
        .expect("the atlas is resident");
    assert_eq!(
        (caps.width, caps.height, caps.layers, caps.mip_levels),
        (512, 512, 24, 1)
    );
    assert_eq!(caps.format, wgpu::TextureFormat::R8Unorm);
}

/// A resolution switch while the atlas is being decoded leaves it alone: the
/// setting does not govern the atlas, so the decode in flight is the one that
/// lands, the export waiting for it gets the ice, and the textures become
/// ready again.
#[test]
fn a_resolution_switch_during_the_atlas_decode_still_draws_the_ice() {
    let _gpu = gpu();
    let atlas = test_support::earth_fixture_dir().join("sea_ice.jxl");
    let without = SceneParams {
        sea_ice: false,
        ..arctic()
    };
    let (_dir, harness) = engine_with("engine_sea_ice_switch", Some(atlas), without);
    harness.settle();

    // One batch: the export draws and so starts the decode, and the switch
    // that follows it is handled before any tick could take the decode in.
    let (reply, replies) = crossbeam_channel::bounded(1);
    harness
        .engine
        .send(EngineCommand::UpdateParams(Box::new(arctic())));
    harness.engine.send(EngineCommand::ExportPixels {
        width: FRAME.0,
        height: FRAME.1,
        reply,
    });
    harness.set_texture_resolution(2048);

    replies
        .recv_timeout(TIMEOUT)
        .expect("the export should be answered")
        .expect("the engine should be able to export");
    harness.wait_for_textures("the atlas after the switch");
    let off = harness.picture(&without, FRAME);
    let on = harness.picture(&arctic(), FRAME);
    assert!(
        differing(&on, &off) > on.len() / 50,
        "the Arctic in March shows hardly any ice after the switch: {} bytes differ",
        differing(&on, &off)
    );
}
