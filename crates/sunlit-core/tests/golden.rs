//! Golden-image tests and the contact sheet.
//!
//! Each case renders a fixed `SceneParams` at a fixed size with the procedural
//! grid texture, so nothing depends on external assets, the clock, or the
//! network. The references are compared with a perceptual tolerance rather
//! than exactly: the existing behavioral-invariant convention exists because
//! float and filtering differences across adapters are real.
//!
//! All of these force the software adapter where the platform has one, so a
//! developer machine with a discrete GPU and a CI runner with WARP compare
//! against the same references.
//!
//! References live one directory per adapter, named by
//! `sunlit_core::wgpu_init::adapter_key`: `tests/golden/warp/`,
//! `tests/golden/lavapipe/`, `tests/golden/metal/`. That is about preserving
//! the tolerance for what it is meant to catch rather than spending it on the
//! difference between two correct rasterizers; the measurement and the argument
//! are on `adapter_key`.
//!
//! An adapter this repository has never generated references for skips rather
//! than fails, which is what lets a new platform land before its reference set
//! exists. Which adapters those are is not inferred from the filesystem but
//! listed in `GENERATED_ADAPTERS`, so a known adapter whose directory has gone
//! missing fails instead of quietly testing nothing. A directory that exists
//! but is missing one case fails too, every time it runs: the render is written
//! somewhere untracked for review and never into the tracked tree, so blessing
//! a new case stays a deliberate act.
//!
//! Regenerate with `SUNLIT_EARTH_UPDATE_GOLDEN=1 cargo test -p sunlit-core
//! --test golden`, on a machine using the adapter you are generating for.
//! Review the diff by eye before committing it: that is the whole point of a
//! golden test.

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard};

use sunlit_core::engine::{EngineCommand, EngineConfig, EngineHandle};
use sunlit_core::params::SceneParams;
use sunlit_core::scene::camera::{CameraParams, PRESETS};

mod support;

/// The texture mode that blends the day and night maps, as `texture_index`
/// spells it.
const BLEND_MODE: i32 = 3;

/// Golden images are small on purpose: they live in git.
const WIDTH: u32 = 512;
const HEIGHT: u32 = 256;

/// Mean absolute per-channel difference, in 0-255 units, that still counts as
/// a match.
const MEAN_TOLERANCE: f64 = 2.0;
/// Fraction of pixels allowed to differ by more than `OUTLIER_THRESHOLD`.
const OUTLIER_FRACTION: f64 = 0.01;
/// Per-channel difference that makes a pixel an outlier.
const OUTLIER_THRESHOLD: u8 = 24;

/// Adapter keys this repository ships reference sets for.
///
/// This list is what separates "no references have ever been generated for this
/// adapter" from "the references went missing". Without it both look identical
/// from inside the test, and the second one passes: that already happened
/// during Phase 2, when the macOS probe's golden leg was green while comparing
/// nothing. Any drift in `adapter_key` output (a Mesa driver that renames
/// itself, a different macOS device name, a renamed backend) would otherwise
/// delete the golden suite on that platform without a single red test.
///
/// Adding a reference directory means adding its key here.
const GENERATED_ADAPTERS: &[&str] = &["warp", "lavapipe", "metal"];

/// Marker line naming the adapter this run compared against.
///
/// Printed unconditionally by every case, because a skip is invisible in a
/// passing run otherwise: libtest captures the output of tests that pass, which
/// is why CI runs the suite with `--show-output`. `golden.yml` also reads this
/// line to learn which reference directory it just regenerated.
fn announce_adapter(adapter_key: &str) {
    println!("golden adapter key: {adapter_key}");
}

/// Whether an adapter with no reference directory is allowed to skip.
fn assert_directory_may_be_absent(adapter_key: &str, dir: &Path) {
    assert!(
        !GENERATED_ADAPTERS.contains(&adapter_key),
        "adapter {adapter_key} is one this repository ships golden references for, \
         but {} does not exist. Either the references were lost, or `adapter_key` \
         now reports something different for this adapter. Do not silence this by \
         regenerating blindly: work out which of the two happened first.",
        dir.display()
    );
}

/// One engine, shared by every case: it owns a wgpu device, and creating
/// several concurrently crashes on Windows.
static ENGINE: LazyLock<Mutex<EngineHandle>> = LazyLock::new(|| {
    let mut config = EngineConfig::headless((WIDTH, HEIGHT));
    // Force WARP so the references are adapter-independent in practice. This
    // matches the `headless` default but is stated explicitly, because the
    // correctness of the checked-in references depends on it.
    config.force_software = true;
    config.preview_enabled = false;
    config.params = base_params();
    // The Moon's slot points at a generated fixture rather than at the 1024
    // pixel asset, which is Git LFS and may not be there. Every case therefore
    // renders with a Moon wherever the sky puts one at the pinned instant,
    // which is what makes the default-on Moon visible to this suite at all
    // instead of quietly absent from it.
    //
    // The day and night maps are fixtures too, and they are here for the cloud
    // cases: the layer is shaded against the sun, so pinning it wants a mode
    // that is, and blend mode is the only one. Every other case renders the
    // grid, which reads neither slot.
    let surface = support::write_surface_fixtures(Path::new(env!("CARGO_TARGET_TMPDIR")));
    config.texture_paths = vec![
        Some(surface.day),
        Some(surface.night),
        Some(support::write_moon_fixture(Path::new(env!(
            "CARGO_TARGET_TMPDIR"
        )))),
        Some(support::write_panorama_bands_fixture(Path::new(env!(
            "CARGO_TARGET_TMPDIR"
        )))),
    ];
    // The cloud slot comes from the fetcher rather than from a path, so without
    // a source no case here could draw a cloud pixel at all; `base_params`
    // turns the layer off for every case that is not about it.
    config.cloud = Some(std::sync::Arc::new(support::FixtureClouds::bands()));
    Mutex::new(
        sunlit_core::engine::start(config).expect("the golden suite needs a working adapter"),
    )
});

fn engine() -> MutexGuard<'static, EngineHandle> {
    ENGINE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Parameters shared by every case: the grid texture, no multisampling, and a
/// fixed date and time so the terminator never moves.
fn base_params() -> SceneParams {
    let mut params = SceneParams {
        texture_index: 0,
        sample_count: 1,
        // Fill the frame. At the default zoom the globe is small enough that
        // limb effects land on a handful of pixels and no tolerance can tell
        // them apart from noise.
        camera: CameraParams {
            zoom: 0.26,
            ..CameraParams::default()
        },
        // Off for every case but the two that are about it. The layer covers
        // the whole frame by construction, so leaving it on would move all
        // eleven other references and bury what each of them is for under one
        // background; the two panorama cases switch it on, and its own engine
        // cases pin what a golden cannot see anyway.
        milky_way_intensity: 0.0,
        // Off for the same reason, and it has to be said explicitly now that
        // the engine has a cloud source: the layer covers half the frame. Both
        // hemispheres, because either one alone still draws the layer.
        cloud_opacity: 0.0,
        cloud_opacity_night: 0.0,
        // Camera mode is pinned off rather than left at its default for the
        // same reason: the aperture spikes and the ghosts reach outside the
        // glare's cone and would put streaks and blobs in every reference with
        // a Sun anywhere near the frame.
        sun_flare: 0.0,
        ..SceneParams::default()
    };
    params.datetime.use_custom = true;
    params.datetime.custom_hour = 12.0;
    params.datetime.custom_day_of_year = 172;
    params.datetime.custom_year = 2026;
    params
}

/// The part of a rendered frame a case is compared over.
///
/// Most cases compare the whole frame, and three do not. Each of the three is
/// arithmetic rather than taste, and all three are the same arithmetic: a thing
/// a few dozen pixels across cannot move a 512 by 256 frame past a tolerance
/// meant for a whole picture. Removing the Moon entirely comes to a mean
/// channel difference of 0.22 against a tolerance of 2.00, so a full-frame
/// reference would go on passing with the feature deleted, which is precisely
/// the failure phase B's goldens taught; over the window it is 3.12 with 1.71
/// percent of pixels outliers. `sunrise_band` and `sun_rising_through_the_band`
/// carry their own measurements above their windows. What each window leaves
/// out is the globe, which nine other cases pin.
#[derive(Clone, Copy)]
struct Window {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

const FULL_FRAME: Window = Window {
    x: 0,
    y: 0,
    width: WIDTH,
    height: HEIGHT,
};

/// Cut `window` out of a full-frame RGBA8 render.
fn crop(pixels: &[u8], window: Window) -> Vec<u8> {
    if window.x == 0 && window.y == 0 && window.width == WIDTH && window.height == HEIGHT {
        return pixels.to_vec();
    }
    assert!(
        window.x + window.width <= WIDTH && window.y + window.height <= HEIGHT,
        "the window has to be inside the frame"
    );
    let mut out = Vec::with_capacity((window.width * window.height * 4) as usize);
    for row in window.y..window.y + window.height {
        let start = ((row * WIDTH + window.x) * 4) as usize;
        out.extend_from_slice(&pixels[start..start + (window.width * 4) as usize]);
    }
    out
}

/// Reference directory for the adapter this run is using.
fn golden_dir(adapter_key: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(adapter_key)
}

fn updating() -> bool {
    std::env::var("SUNLIT_EARTH_UPDATE_GOLDEN").is_ok()
}

/// Block until an overlay's fixture texture has reached the GPU, by its GPU
/// label.
///
/// The Moon and the Milky Way are overlays, so nothing in the engine waits for
/// them and `TexturesReady` excludes both. A case would otherwise race a decode
/// that takes a few tens of milliseconds: the first case would render without
/// the texture and the rest with it, which is a reference that depends on test
/// order. The memory report is what says whether the renderer owns it.
fn wait_for_slot_texture(engine: &EngineHandle, label: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while std::time::Instant::now() < deadline {
        let report = engine
            .memory_report()
            .expect("the engine should answer with a report");
        if report.expected.iter().any(|texture| texture.label == label) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("the {label} fixture did not reach the GPU within a minute");
}

/// Render `params` and compare against `tests/golden/<adapter>/<name>.png`.
fn check_golden(name: &str, params: &SceneParams) {
    check_golden_in(name, params, FULL_FRAME);
}

/// The same, over one window of the frame.
fn check_golden_in(name: &str, params: &SceneParams, window: Window) {
    let engine = engine();
    let adapter_key = engine.adapter_key().to_owned();
    engine.send(EngineCommand::UpdateParams(Box::new(*params)));
    if params.moon_brightness > 0.0 {
        wait_for_slot_texture(&engine, "moon_texture");
    }
    if params.milky_way_intensity > 0.0 {
        wait_for_slot_texture(&engine, "milky_way_texture");
    }
    if params.draws_clouds() {
        wait_for_slot_texture(&engine, "cloud_texture");
    }
    // Blend mode is the one that reads the two surface slots, and nothing
    // spawns their decodes until a case asks for the mode: the first blend case
    // to run would otherwise export the frame the fallback draws, which is the
    // grid. That is what happened to the first pair of cloud references.
    if params.texture_index == BLEND_MODE {
        wait_for_slot_texture(&engine, "day_texture");
        wait_for_slot_texture(&engine, "night_texture");
    }
    let pixels = crop(
        &engine
            .export_pixels(WIDTH, HEIGHT)
            .expect("the engine should be able to export"),
        window,
    );
    drop(engine);

    announce_adapter(&adapter_key);
    let dir = golden_dir(&adapter_key);
    let path = dir.join(format!("{name}.png"));

    if updating() {
        std::fs::create_dir_all(&dir).expect("create golden directory");
        sunlit_core::engine::save_png(&path, window.width, window.height, &pixels)
            .expect("write golden");
        return;
    }

    // A missing directory means this adapter has no reference set yet, which is
    // a known state rather than a failure, but only for an adapter that is not
    // on the list; see `GENERATED_ADAPTERS`.
    if !dir.exists() {
        assert_directory_may_be_absent(&adapter_key, &dir);
        eprintln!(
            "no golden references for adapter {adapter_key} yet ({}); \
             generate them with SUNLIT_EARTH_UPDATE_GOLDEN=1, skipping",
            dir.display()
        );
        return;
    }

    if !path.exists() {
        // Write the render where a human can look at it, and deliberately not
        // into the tracked tree. A file written to `path` here would be
        // compared against itself on the next run and pass, so a missing case
        // would fail exactly once and then bless itself. Under
        // CARGO_TARGET_TMPDIR it stays a diagnostic, and this case keeps
        // failing until someone regenerates on purpose.
        let review = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("golden-missing")
            .join(&adapter_key);
        std::fs::create_dir_all(&review).expect("create the review directory");
        let review_path = review.join(format!("{name}.png"));
        sunlit_core::engine::save_png(&review_path, window.width, window.height, &pixels)
            .expect("write the review image");
        panic!(
            "golden reference {name} is missing at {}. This run's render is at {} \
             for review; bless it with SUNLIT_EARTH_UPDATE_GOLDEN=1 once you have \
             looked at it.",
            path.display(),
            review_path.display()
        );
    }

    let reference = image::open(&path)
        .unwrap_or_else(|e| panic!("could not read golden {}: {e}", path.display()))
        .to_rgba8();
    assert_eq!(
        (reference.width(), reference.height()),
        (window.width, window.height),
        "golden {name} has the wrong size"
    );

    let (mean, outliers) = compare(reference.as_raw(), &pixels);
    assert!(
        mean <= MEAN_TOLERANCE,
        "golden {name}: mean channel difference {mean:.2} exceeds {MEAN_TOLERANCE:.2}. \
         Re-run with SUNLIT_EARTH_UPDATE_GOLDEN=1 if the change is intended."
    );
    assert!(
        outliers <= OUTLIER_FRACTION,
        "golden {name}: {:.2}% of pixels differ by more than {OUTLIER_THRESHOLD} \
         (limit {:.2}%). Re-run with SUNLIT_EARTH_UPDATE_GOLDEN=1 if intended.",
        outliers * 100.0,
        OUTLIER_FRACTION * 100.0
    );
}

/// Mean absolute channel difference and the fraction of outlier pixels.
#[allow(clippy::cast_precision_loss)]
fn compare(reference: &[u8], actual: &[u8]) -> (f64, f64) {
    assert_eq!(reference.len(), actual.len(), "image sizes differ");
    let mut total = 0u64;
    let mut outliers = 0u64;
    for (r, a) in reference.chunks_exact(4).zip(actual.chunks_exact(4)) {
        let mut worst = 0u8;
        for c in 0..3 {
            let diff = r[c].abs_diff(a[c]);
            total += u64::from(diff);
            worst = worst.max(diff);
        }
        if worst > OUTLIER_THRESHOLD {
            outliers += 1;
        }
    }
    let channels = (reference.len() / 4 * 3) as f64;
    let pixels = (reference.len() / 4) as f64;
    (total as f64 / channels, outliers as f64 / pixels)
}

#[test]
fn golden_default_view() {
    check_golden("default", &base_params());
}

#[test]
fn golden_nightglow() {
    // Midnight over the prime meridian with the airglow shells turned up.
    //
    // The grid texture renders in single-texture mode, where the shader takes
    // the `terminator_width < 0` path and ignores the sun entirely, so the
    // only sun-dependent thing left in a texture-free golden is the atmosphere.
    // Cranking it is what makes this case differ from `default` at all.
    let mut params = base_params();
    params.datetime.custom_hour = 0.0;
    params.rayleigh_intensity = 3.0;
    params.nightglow_intensity = 2.0;
    params.nightglow_falloff = 4.0;
    check_golden("nightglow", &params);
}

#[test]
fn golden_rayleigh() {
    // Strong day-side scattering: the blue limb and the orange terminator
    // band, at noon over the prime meridian.
    let params = SceneParams {
        rayleigh_intensity: 4.0,
        rayleigh_sharpness: 8.0,
        rayleigh_haze: 1.0,
        nightglow_intensity: 0.0,
        ..base_params()
    };
    check_golden("rayleigh", &params);
}

#[test]
fn golden_close_up() {
    let base = base_params();
    let params = SceneParams {
        camera: CameraParams {
            longitude: 11.0,
            latitude: 48.0,
            zoom: 0.04,
            tilt_deg: 20.0,
            ..base.camera
        },
        ..base
    };
    check_golden("close_up", &params);
}

#[test]
fn golden_night_side_with_stars() {
    let base = base_params();
    let params = SceneParams {
        camera: CameraParams {
            longitude: 160.0,
            latitude: 0.0,
            zoom: 0.45,
            ..base.camera
        },
        atmo_enabled: false,
        star_intensity: 1.0,
        star_mag_limit: 6.0,
        ..base
    };
    check_golden("night_side_with_stars", &params);
}

#[test]
fn golden_large_crisp_stars_without_glow() {
    let base = base_params();
    let params = SceneParams {
        camera: CameraParams {
            longitude: 160.0,
            latitude: 0.0,
            zoom: 0.45,
            ..base.camera
        },
        atmo_enabled: false,
        star_intensity: 3.0,
        star_size: 3.0,
        star_glow_strength: 0.0,
        star_contrast: 0.0,
        star_mag_limit: 6.0,
        ..base
    };
    check_golden("large_crisp_stars", &params);
}

#[test]
fn golden_bright_star_halos() {
    // Every slider that feeds the halo at its maximum, which is the corner the
    // sprite quad's own edge used to draw in: a plain Gaussian still carries
    // 4.4% of its peak where the quad ends, and this is the setting that makes
    // that residual visible as a square. `large_crisp_stars` is the other end
    // of the same axis and has no halo at all, so neither case covers this one.
    let base = base_params();
    let params = SceneParams {
        camera: CameraParams {
            longitude: 160.0,
            latitude: 0.0,
            zoom: 0.45,
            ..base.camera
        },
        atmo_enabled: false,
        star_intensity: 5.0,
        star_glow_strength: 3.0,
        star_glow_radius: 30.0,
        star_mag_limit: 6.0,
        ..base
    };
    check_golden("bright_star_halos", &params);
}

/// The camera the two sun cases share, up to the longitude and the zoom.
///
/// The eye sits at the latitude of the subsolar point and swings round in
/// longitude, which puts the Sun beside the globe rather than above it. Above
/// it is where the aspect ratio would have put it, and NDC y carries twice the
/// angle NDC x does on a 512 by 256 frame, so a Sun clear of a limb this size
/// would have been off the top.
fn sun_camera(longitude: f32, zoom: f32) -> CameraParams {
    CameraParams {
        longitude,
        latitude: -23.44,
        zoom,
        ..CameraParams::default()
    }
}

/// Both sun cases run at the narrow end of the sky lens rather than at its
/// 140 degree default, and that is what gives them teeth.
///
/// The field of view decides how many pixels a degree is worth: 512 of them
/// across 140 degrees is three, so the whole Spencer composition lands inside
/// forty pixels and a reference that lost the Sun entirely would still pass at
/// a mean of 1.11 and half a percent of outliers. At 60 degrees a degree is
/// eight pixels and the glare is most of the frame, which is the picture these
/// cases are supposed to be about. What the Sun does at the default is pinned
/// by the engine cases instead, where a count of painted pixels needs no
/// tolerance at all.
const SUN_CASE_SKY_FOV: f32 = 60.0;

#[test]
fn golden_sun_over_the_night_side() {
    // Well clear of the painted limb, so nothing fades the glare: the clipped
    // core, the corona needles, the halo ring and the veil are all at full
    // strength against the sky and over the atmosphere shells.
    let base = base_params();
    let params = SceneParams {
        camera: sun_camera(161.8, 0.45),
        sky_fov: SUN_CASE_SKY_FOV,
        ..base
    };
    check_golden("sun_over_the_night_side", &params);
}

#[test]
fn golden_sun_grazing_the_limb() {
    // Closer in and one degree further round, where the disk straddles the
    // band between the painted silhouette and the atmosphere shell's: about
    // 97 percent of it still visible and 57 percent of it looking through the
    // lower atmosphere, which is what turns the glare warm and dims it. The
    // closer zoom is what makes that band wide enough to hold most of a disk;
    // at the other case's zoom it is one pixel across. Nothing else in the
    // suite reaches that branch of the occlusion function.
    //
    // The glare is turned up because the tint is what this case is for and the
    // tolerance has to be able to see it: at the default strength, losing the
    // warm shift entirely comes to a mean of 2.33 against a tolerance of 2.00
    // and 1.04 percent outliers against a limit of 1.00, which is a test that
    // passes or fails on rounding. At 1.6 it is a test.
    let base = base_params();
    let params = SceneParams {
        camera: sun_camera(160.75, 0.30),
        sky_fov: SUN_CASE_SKY_FOV,
        sun_glow: 1.6,
        ..base
    };
    check_golden("sun_grazing_the_limb", &params);
}

/// The camera the two horizon cases share, at the zoom where the painted
/// annulus and the disk are the same few pixels.
///
/// The eye sits at the anti-subsolar latitude and swings round in longitude,
/// which is the one family of framings where the Sun climbs the painted limb
/// sideways rather than off the top: the vertical half of a 512 by 256 frame
/// carries twice the angle the horizontal one does, so a Sun placed above the
/// globe is off screen before it has cleared anything. At zoom 0.26 the
/// silhouette's radius is 177 pixels, the annulus 2.7 and the disk 4.6, so the
/// horizon zone is the disk's own diameter and the whole gradient is inside it.
fn horizon_camera(longitude: f32) -> CameraParams {
    CameraParams {
        longitude,
        latitude: -23.44,
        zoom: 0.26,
        ..CameraParams::default()
    }
}

/// The window the disk and the near half of its glare land in at the framing
/// below.
///
/// The refraction is what decides the window. Deleting the tint moves the whole
/// frame by a mean of 10.04 with 16.80 percent of pixels outliers and deleting
/// the exposure gain by 7.27 with 11.00, both well past the tolerance, but
/// deleting the lift and the squash moves it by 0.91 with 0.22 percent, which
/// is a reference that passes with the effect gone. The disk is what refraction
/// moves and the glare is most of the frame, so comparing where the disk is
/// puts the three at 55.33 with 83.09 percent, 56.03 with 98.58, and 6.19 with
/// 4.58.
const RISING_SUN_WINDOW: Window = Window {
    x: 40,
    y: 76,
    width: 80,
    height: 80,
};

#[test]
fn golden_sun_rising_through_the_band() {
    // The disk crossing the painted limb inside the zone, with the atmosphere
    // off so that what the reference holds is the disk's own gradient and the
    // glare the flux model gives it, and nothing the shell draws.
    //
    // Four times the size for the reason `moon_crescent` is eight times its
    // own: at its true half degree the disk is 4.6 pixels across in a 512 by
    // 256 frame, the horizon zone is the same 4.6 pixels, and the part of the
    // band that colors anything is the lowest thirty kilometers of it, which
    // comes to a pixel and a half. `sun_size` scales the disk and the zone
    // together, so the whole of the geometry is the shipped one at four times
    // the pixels, and the gradient the case is named for exists to be compared.
    let base = base_params();
    let params = SceneParams {
        camera: horizon_camera(157.3),
        sky_fov: SUN_CASE_SKY_FOV,
        atmo_enabled: false,
        sun_size: 4.0,
        ..base
    };
    check_golden_in("sun_rising_through_the_band", &params, RISING_SUN_WINDOW);
}

/// The strip of frame the band runs down, with the limb in the middle of it.
///
/// The band is a thread along the limb and the frame is mostly the globe's
/// bright grid, so a full-frame reference has the same weakness the Moon's had:
/// deleting the lobe entirely comes to a mean of 0.21 over the whole frame with
/// 0.40 percent of pixels outliers, which passes on both counts. Over this
/// strip it is 1.52 and 2.83 percent, which fails on the second. What the strip
/// leaves out is the glare, and the case beside this one is about that.
const SUNRISE_BAND_WINDOW: Window = Window {
    x: 72,
    y: 0,
    width: 72,
    height: HEIGHT,
};

#[test]
fn golden_sunrise_band() {
    // The same framing a little further round, with the Sun behind the limb so
    // that what is left in the frame is the band the atmosphere takes around
    // it. The band is turned up for the reason `sun_grazing_the_limb` turns the
    // glare up: at the default it is a thread a few levels deep along a limb
    // the grid texture already paints bright, and a reference that lost it
    // entirely would still pass.
    let base = base_params();
    let params = SceneParams {
        camera: horizon_camera(157.7),
        sky_fov: SUN_CASE_SKY_FOV,
        atmo_sunrise_glow: 3.0,
        ..base
    };
    check_golden_in("sunrise_band", &params, SUNRISE_BAND_WINDOW);
}

/// The user's own camera: 3.7 Earth radii, and the sky at the width it ships
/// at rather than the 60 degrees the four cases above are framed in.
///
/// This is where the two lenses disagree most. The globe subtends 15.68
/// degrees from here, so the painted limb is 204 pixels out, while the sky lens
/// puts a direction there only when it is 57 degrees off the view axis. A lobe
/// in the true scattering angle therefore peaks with the Sun's image still 152
/// pixels inside the painted disc, and is a quarter of its peak by the time the
/// image reaches the limb.
fn close_camera(longitude: f32) -> CameraParams {
    CameraParams {
        longitude,
        latitude: -23.44,
        zoom: 0.227_047_34,
        ..CameraParams::default()
    }
}

/// The strip the band runs down at the framing below, with the limb inside it.
///
/// The left limb crosses the frame's top and bottom edges at x 97 and reaches
/// x 52 at half height, so 72 pixels from x 24 hold all of it but the two rows
/// at each edge, where it passes outside the window's own right edge at x 96,
/// and the annulus outside it. The band's own light is what has to be inside
/// the window; the globe is nine other cases' business.
const SUNRISE_BAND_CLOSE_WINDOW: Window = Window {
    x: 24,
    y: 0,
    width: 72,
    height: HEIGHT,
};

#[test]
fn golden_sunrise_band_close() {
    // The Sun's image one horizon zone inside the painted limb: near enough
    // that the lobe is at its peak under it, far enough that no disk is drawn
    // and what the reference holds is the band alone.
    let base = base_params();
    let params = SceneParams {
        camera: close_camera(117.658_22),
        sky_fov: 140.0,
        ..base
    };
    check_golden_in("sunrise_band_close", &params, SUNRISE_BAND_CLOSE_WINDOW);
}

/// The window the Moon lands in at the framing below, with room around it for
/// a Moon that moved to be visible rather than merely absent.
const MOON_WINDOW: Window = Window {
    x: 90,
    y: 73,
    width: 96,
    height: 96,
};

/// The Moon as a fat crescent, clear of the painted limb.
///
/// The instant is chosen so that the Moon sits beside the globe rather than
/// behind it, and so that the camera's own displacement barely changes the
/// phase: the eye is nine Earth radii from the geocenter and the Moon is sixty
/// away, so a framing where that displacement is nearly perpendicular to the
/// Sun's direction is one whose phase an ephemeris can be asked about. At 60
/// degrees of sky and eight times the size the disk is 31 pixels across, which
/// is what makes the crescent's orientation something a person can see.
#[test]
fn golden_moon_crescent() {
    let base = base_params();
    let mut params = SceneParams {
        camera: CameraParams {
            longitude: 160.0,
            latitude: 0.0,
            zoom: 0.45,
            ..base.camera
        },
        atmo_enabled: false,
        star_intensity: 0.0,
        sun_glow: 0.0,
        sky_fov: 60.0,
        moon_size: 8.0,
        ..base
    };
    params.datetime.custom_day_of_year = 199;
    params.datetime.custom_hour = 16.0;
    check_golden_in("moon_crescent", &params, MOON_WINDOW);
}

/// The panorama behind the stars at the default field of view.
///
/// The fixture is bands of declination, so what the reference shows is where
/// the celestial sphere's parallels lie in this framing as well as that the
/// layer is drawn at all: a reconstruction that had the sky rotated would bend
/// the bands somewhere else. What it cannot show is the real asset's own
/// layout, which is Git LFS and deliberately not what any reference here rests
/// on; `the_real_panorama_has_the_galactic_plane_where_the_plane_is` in
/// `tests/engine.rs` is where that lives.
#[test]
fn golden_panorama_behind_the_stars() {
    let base = base_params();
    let params = SceneParams {
        camera: CameraParams {
            longitude: 160.0,
            latitude: 0.0,
            zoom: 0.45,
            ..base.camera
        },
        atmo_enabled: false,
        star_intensity: 1.0,
        star_mag_limit: 6.0,
        sun_glow: 0.0,
        milky_way_intensity: 1.0,
        ..base
    };
    check_golden("panorama_behind_the_stars", &params);
}

/// The same sky at the narrow end of the slider, where the layer is magnified
/// about two and a half times more.
///
/// Two references at two fields of view are what makes the pair distinguishable
/// for the reason decision 5 cares about: the bands are wider apart here and
/// the globe is exactly the size it is in the other one.
#[test]
fn golden_panorama_at_a_narrow_sky() {
    let base = base_params();
    let params = SceneParams {
        camera: CameraParams {
            longitude: 160.0,
            latitude: 0.0,
            zoom: 0.45,
            ..base.camera
        },
        atmo_enabled: false,
        star_intensity: 1.0,
        star_mag_limit: 6.0,
        sun_glow: 0.0,
        sky_fov: 60.0,
        milky_way_intensity: 1.0,
        ..base
    };
    check_golden("panorama_at_a_narrow_sky", &params);
}

/// The same sky at a width only a spanned canvas derives.
///
/// The slider stops at 180; `display::layout::SKY_FOV_MAX` is 330, and a canvas
/// six screens wide asks for about 300. Nothing above 180 had a reference before
/// this case, and three things behave differently out there: star sprites carry
/// the conformal `(1 + r * r)` factor and magnify hard toward the corners, the
/// Milky Way is sampled through the lens inverse and is stretched severely at a
/// corner 165 degrees off the view axis, and the Sun's glare composition is
/// angular and was only ever exercised to 180. This is what turns those from
/// claims into something a change has to preserve.
#[test]
fn golden_panorama_at_a_wide_sky() {
    let base = base_params();
    let params = SceneParams {
        camera: CameraParams {
            longitude: 160.0,
            latitude: 0.0,
            zoom: 0.45,
            ..base.camera
        },
        atmo_enabled: false,
        star_intensity: 1.0,
        star_mag_limit: 6.0,
        sun_glow: 0.0,
        sky_fov: 300.0,
        milky_way_intensity: 1.0,
        ..base
    };
    check_golden("panorama_at_a_wide_sky", &params);
}

/// The framing the two cloud cases share: the terminator down the middle of the
/// frame, at the instant every case here renders.
///
/// One hemisphere alone would pass with either half of this change reverted, so
/// the frame has to hold both: the floor is what the night half shows, the ramp
/// and its nightward shift are what the middle shows, and the day half is what
/// says nothing about the lit side moved.
fn cloud_params() -> SceneParams {
    let base = base_params();
    SceneParams {
        texture_index: BLEND_MODE,
        camera: CameraParams {
            longitude: 90.0,
            latitude: 0.0,
            zoom: 0.26,
            ..base.camera
        },
        // What `base_params` turned off, back at the values the product ships.
        cloud_opacity: SceneParams::default().cloud_opacity,
        cloud_opacity_night: SceneParams::default().cloud_opacity_night,
        ..base
    }
}

/// The cloud layer across the terminator at the default settings.
#[test]
fn golden_clouds_across_the_terminator() {
    check_golden("clouds_across_the_terminator", &cloud_params());
}

/// The two terminators side by side, close enough to see them apart.
///
/// The camera sits over the terminator at the latitude where the fixture's
/// equatorial band ends, so the frame holds four quadrants: lit ground, unlit
/// ground, lit deck, unlit deck. The ground's edge is the product's own
/// `terminator_width` and the deck's is `CLOUD_TERMINATOR_WIDTH` centered three
/// degrees further into the night, which at this zoom is tens of pixels rather
/// than the ten the whole globe would give. That is what makes this the case a
/// revert of either the shift or the width fails: the cloud edge moves against a
/// ground edge that did not.
#[test]
fn golden_cloud_terminator_close_up() {
    let base = cloud_params();
    let params = SceneParams {
        camera: CameraParams {
            latitude: support::CLOUD_FIXTURE_BAND_EDGE,
            zoom: 0.04,
            ..base.camera
        },
        ..base
    };
    check_golden("cloud_terminator_close_up", &params);
}

/// Render every camera preset into one image for human review.
///
/// This asserts almost nothing: it exists so CI can upload a single PNG that a
/// human can glance at when a shading change lands. The file goes to
/// `target/contact-sheet.png` unless `SUNLIT_EARTH_CONTACT_SHEET` says
/// otherwise.
#[test]
fn contact_sheet_of_every_preset() {
    const CELL_W: u32 = 256;
    const CELL_H: u32 = 128;
    const COLUMNS: u32 = 3;

    let rows = u32::try_from(PRESETS.len().div_ceil(COLUMNS as usize))
        .expect("preset row count fits in u32");
    let mut sheet = image::RgbaImage::new(CELL_W * COLUMNS, CELL_H * rows);

    for (index, preset) in PRESETS.iter().enumerate() {
        let params = SceneParams {
            camera: *preset,
            ..base_params()
        };
        let engine = engine();
        engine.send(EngineCommand::UpdateParams(Box::new(params)));
        let pixels = engine
            .export_pixels(CELL_W, CELL_H)
            .expect("the engine should be able to export");
        drop(engine);

        let cell = image::RgbaImage::from_raw(CELL_W, CELL_H, pixels)
            .expect("exported buffer should match the cell size");
        let index = u32::try_from(index).expect("preset count fits in u32");
        image::imageops::replace(
            &mut sheet,
            &cell,
            i64::from(index % COLUMNS) * i64::from(CELL_W),
            i64::from(index / COLUMNS) * i64::from(CELL_H),
        );
    }

    let path = std::env::var("SUNLIT_EARTH_CONTACT_SHEET").map_or_else(
        |_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/contact-sheet.png"),
        PathBuf::from,
    );
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    sheet.save(&path).expect("write contact sheet");
    println!("contact sheet written to {}", path.display());
}

/// The comparator has to be able to fail, and the cases have to be distinct.
///
/// Every pair of references must land outside the tolerance. If two of them
/// compare equal, either the tolerance is too loose to catch a regression or
/// one of the cases is not testing anything the others do not.
///
/// The names are spelled here rather than read off the directory, so that a
/// reference file that went missing fails this case as well as the one that
/// owns it. What the directory is read for is the other direction: a reference
/// this list does not name is a case silently outside the guard, which is what
/// happened when the two panorama references were added, and the closest pair
/// in the set is exactly the pair most likely to arrive that way.
#[test]
fn every_golden_case_is_distinguishable() {
    if updating() {
        eprintln!("references are being regenerated in parallel, skipping");
        return;
    }
    let adapter_key = engine().adapter_key().to_owned();
    announce_adapter(&adapter_key);
    let dir = golden_dir(&adapter_key);
    if !dir.exists() {
        assert_directory_may_be_absent(&adapter_key, &dir);
        eprintln!("golden references for {adapter_key} not generated yet, skipping");
        return;
    }

    let names = [
        "default",
        "nightglow",
        "rayleigh",
        "close_up",
        "night_side_with_stars",
        "large_crisp_stars",
        "bright_star_halos",
        "sun_over_the_night_side",
        "sun_grazing_the_limb",
        "sun_rising_through_the_band",
        "sunrise_band",
        "sunrise_band_close",
        "moon_crescent",
        "panorama_behind_the_stars",
        "panorama_at_a_narrow_sky",
        "panorama_at_a_wide_sky",
        "clouds_across_the_terminator",
        "cloud_terminator_close_up",
    ];

    let mut unnamed: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("read the golden directory") {
        let file_name = entry.expect("a directory entry").file_name();
        let file_name = file_name.to_string_lossy();
        if let Some(stem) = file_name.strip_suffix(".png")
            && !names.contains(&stem)
        {
            unnamed.push(file_name.into_owned());
        }
    }
    unnamed.sort();
    assert!(
        unnamed.is_empty(),
        "the {adapter_key} set holds {unnamed:?}, which this case does not name; \
         add them to `names` so the guard compares them too"
    );

    let mut images = Vec::new();
    for name in names {
        let path = dir.join(format!("{name}.png"));
        assert!(
            path.exists(),
            "golden reference {} is missing while the rest of the {adapter_key} set \
             is present; the case that owns it reports the same thing",
            path.display()
        );
        images.push(image::open(&path).expect("read golden").to_rgba8());
    }

    for (i, a) in images.iter().enumerate() {
        for (j, b) in images.iter().enumerate().skip(i + 1) {
            // Two references of different sizes are distinguishable by their
            // sizes, and `compare` has no meaning across them. The three
            // windowed cases are each a size of their own, so what this leaves
            // them with is the presence check above; see `Window`.
            if a.dimensions() != b.dimensions() {
                continue;
            }
            let (mean, outliers) = compare(a.as_raw(), b.as_raw());
            println!(
                "{} vs {}: mean {mean:.2}, outliers {:.2}%",
                names[i],
                names[j],
                outliers * 100.0
            );
            assert!(
                mean > MEAN_TOLERANCE || outliers > OUTLIER_FRACTION,
                "{} and {} compare equal (mean {mean:.2}, outliers {:.2}%): either the                  tolerance is too loose or the cases overlap",
                names[i],
                names[j],
                outliers * 100.0
            );
        }
    }
}
