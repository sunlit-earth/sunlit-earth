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
    Mutex::new(sunlit_core::engine::start(config))
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
        ..SceneParams::default()
    };
    params.datetime.use_custom = true;
    params.datetime.custom_hour = 12.0;
    params.datetime.custom_day_of_year = 172;
    params.datetime.custom_year = 2026;
    params
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

/// Render `params` and compare against `tests/golden/<adapter>/<name>.png`.
fn check_golden(name: &str, params: &SceneParams) {
    let engine = engine();
    let adapter_key = engine.adapter_key().to_owned();
    engine.send(EngineCommand::UpdateParams(Box::new(*params)));
    let pixels = engine
        .export_pixels(WIDTH, HEIGHT)
        .expect("the engine should be able to export");
    drop(engine);

    announce_adapter(&adapter_key);
    let dir = golden_dir(&adapter_key);
    let path = dir.join(format!("{name}.png"));

    if updating() {
        std::fs::create_dir_all(&dir).expect("create golden directory");
        sunlit_core::engine::save_png(&path, WIDTH, HEIGHT, &pixels).expect("write golden");
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
        sunlit_core::engine::save_png(&review_path, WIDTH, HEIGHT, &pixels)
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
        (WIDTH, HEIGHT),
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
    ];
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
