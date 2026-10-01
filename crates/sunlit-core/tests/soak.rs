//! Mock-clock soak test.
//!
//! Seven simulated days of cloud updates and unattended wallpaper exports,
//! compressed into a few seconds by advancing an injected clock instead of
//! waiting. This is the permanent guard against a background producer whose
//! consumer only runs under some condition.
//!
//! The globe is drawn from the cube surface of the Earth fixture, so the
//! transcoder, the floors, the tile loader and a change of month at the
//! middle of January all run under the same clock as the clouds and the
//! exports.
//!
//! Nothing here touches the network, the desktop, or the real clock.

#[path = "../src/test_support.rs"]
mod test_support;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use sunlit_core::assets::cloud_source::{CloudImage, CloudSource};
use sunlit_core::assets::cube_layout::CubeTextures;
use sunlit_core::assets::tiles;
use sunlit_core::engine::clock::MockClock;
use sunlit_core::engine::wallpaper_sink::CountingSink;
use sunlit_core::engine::{EngineCommand, EngineConfig};
use sunlit_core::params::SceneParams;
use sunlit_core::scene::camera::CameraParams;

/// Keeps a single engine (and therefore a single wgpu device) alive at a time,
/// matching the convention in the other GPU test binaries: per-test device
/// creation crashes on Windows.
static GPU_SERIAL: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// One simulated step. Auto-refresh fires once per step.
///
/// One hour rather than 30 minutes: the per-step cost is the render and the
/// engine wake-up rather than the export, so what the step size buys is the
/// number of steps, and the number of steps is what the wall time is.
const STEP: Duration = Duration::from_hours(1);
/// Seven simulated days at one step per hour.
///
/// What the assertion needs is enough cloud updates behind it for a per-update
/// leak to be unmissable, and 56 of them at one decoded frame each would be
/// 450 MiB against `GROWTH_LIMIT`'s 8.
const STEPS: u64 = 7 * 24;
/// The upstream cloud service publishes every three hours.
const STEPS_PER_CLOUD_UPDATE: u64 = 3;
/// Fixture cloud image size: large enough that a leaked frame (8 MiB decoded)
/// would dominate the noise, small enough to decode hundreds of times.
const CLOUD_WIDTH: u32 = 2048;
const CLOUD_HEIGHT: u32 = 1024;
/// Wallpaper export size.
///
/// Small on purpose, and the test is about the schedule and the memory rather
/// than the picture: what the export has to do here is go through the publish
/// path once per simulated hour, which it does at any size. Most of a step is
/// the round trip rather than the render, so this buys less than it looks like
/// it should; the measurements are in docs/testing.md.
const EXPORT_SIZE: (u32, u32) = (160, 96);

/// How long to wait for the engine to catch up with one simulated step.
const STEP_TIMEOUT: Duration = Duration::from_secs(30);

/// Growth allowed after warm-up: one decoded 2048x1024 frame.
///
/// Sized with `STEPS` and `FLOOR_WINDOW` so the sensitivity per update is
/// fixed: at least 35 publications lie between the last step of the baseline
/// window and the first step of the end window, against 8 MiB.
const GROWTH_LIMIT: u64 = 8 * 1024 * 1024;
/// Steps each memory floor is the minimum over.
///
/// One reading is not a level. On `macos-latest` the footprint sits flat and
/// then reads 8.0 or 10.8 MiB high for a single sample, about one step in
/// twenty and on steps with no cloud update as often as on steps with one, and
/// a baseline or end taken from one reading passes or fails by whether it
/// lands on such a step. A leak raises every reading after it, the lowest
/// included, so the floor of a window still sees it. Seven publications wide.
const FLOOR_WINDOW: u64 = STEPS / 8;
/// Allocation allowed during warm-up, from the first cloud texture to the end
/// of the warm-up steps: the rest of the year's packs, the floors and tiles
/// made resident, and wgpu's allocator pools grown to hold them. About twice
/// the largest measured (`docs/testing.md`).
const WARMUP_LIMIT: u64 = 128 * 1024 * 1024;

/// The Earth fixture cut as `tests/golden.rs` cuts it.
const EARTH: tiles::Geometry = tiles::Geometry {
    face: 256,
    levels: 2,
    tile: 32,
    gutter: 4,
    floor: 64,
    mask: 128,
};

/// Days from the Unix epoch to the start of the simulated week, January 16th,
/// half a day before January hands over to February at its middle: the
/// month ahead is read and the month changes inside the warm-up, so both
/// windows the growth is measured between are February's, and a change of
/// month, which is a one-off rather than the per-publication growth this test
/// bounds, cannot land between them.
const START_DAY: i64 = 15;

/// Steps to run before taking the memory baseline.
///
/// The rest of the year's packs, the floors and tiles made resident, and
/// wgpu's allocator pools are a one-off cost (`WARMUP_LIMIT`). Measuring from
/// before that would be measuring startup, not growth; the leak this test
/// guards against is per-update, so the interesting window is everything after
/// warm-up.
const WARMUP_STEPS: u64 = STEPS / 8;

/// A cloud source that serves a fixed JPEG under an `ETag` the test controls.
struct FixtureCloud {
    jpeg: Vec<u8>,
    version: AtomicU64,
    fetches: AtomicU64,
}

impl FixtureCloud {
    fn new(width: u32, height: u32) -> Self {
        // A gradient rather than a flat color, so the JPEG is a realistic size
        // and the decode does real work.
        let mut img = image::RgbImage::new(width, height);
        for (x, y, px) in img.enumerate_pixels_mut() {
            #[allow(clippy::cast_possible_truncation)]
            let v = ((x + y) % 256) as u8;
            *px = image::Rgb([v, 255 - v, v / 2]);
        }
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Jpeg)
            .expect("encode fixture cloud JPEG");

        Self {
            jpeg: buf.into_inner(),
            version: AtomicU64::new(1),
            fetches: AtomicU64::new(0),
        }
    }

    /// Publish a new image version, as the upstream service does every three
    /// hours.
    fn publish(&self) {
        self.version.fetch_add(1, Ordering::SeqCst);
    }

    fn fetches(&self) -> u64 {
        self.fetches.load(Ordering::SeqCst)
    }
}

impl CloudSource for FixtureCloud {
    fn fetch_if_changed(&self, known_etag: Option<&str>) -> Result<Option<CloudImage>, String> {
        let current = format!("v{}", self.version.load(Ordering::SeqCst));
        if known_etag == Some(current.as_str()) {
            return Ok(None);
        }
        self.fetches.fetch_add(1, Ordering::SeqCst);
        Ok(Some(CloudImage {
            bytes: self.jpeg.clone(),
            etag: Some(current),
            last_modified: None,
        }))
    }

    fn describe(&self) -> String {
        "fixture".to_owned()
    }
}

/// Whether `memory::snapshot` has an implementation for this platform.
///
/// Mirrors the cfg on `memory::snapshot` itself. It is the difference between
/// "this platform cannot answer" and "this platform failed to answer", and only
/// the first of those may skip the growth assertion.
fn memory_counters_supported() -> bool {
    cfg!(any(windows, target_os = "linux", target_os = "macos"))
}

fn private_bytes() -> Option<u64> {
    sunlit_core::memory::snapshot().map(|s| s.private_bytes)
}

#[allow(clippy::cast_precision_loss)]
fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// Spin until `condition` holds, or panic after `STEP_TIMEOUT`.
fn wait_until(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + STEP_TIMEOUT;
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "timed out after {STEP_TIMEOUT:?} waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn a_week_of_simulated_clouds_and_exports_stays_bounded() {
    let _guard = gpu_lock();

    let dir = test_support::ScratchDir::new("soak");
    test_support::write_earth_fixture(&dir.join("textures"));
    let cloud = Arc::new(FixtureCloud::new(CLOUD_WIDTH, CLOUD_HEIGHT));
    let sink = Arc::new(CountingSink::new(EXPORT_SIZE.0, EXPORT_SIZE.1));
    let clock = Arc::new(MockClock::new(
        time::OffsetDateTime::UNIX_EPOCH + time::Duration::days(START_DAY),
    ));

    // Blended, over Africa at the zoom where the preview wants tiles of the
    // coarser level, so day tiles, night tiles and the mask are all read.
    let mut params = SceneParams {
        texture_index: 3,
        sample_count: 1,
        camera: CameraParams {
            longitude: 20.0,
            latitude: 0.0,
            zoom: 0.55,
            ..CameraParams::default()
        },
        ..SceneParams::default()
    };
    // Live time, so every simulated day really does rotate the Earth and the
    // sun-position schedule is exercised too.
    params.datetime.use_custom = false;

    let mut config = EngineConfig::headless((512, 288));
    config.params = params;
    // Nothing is looking at the preview: this is the hidden-window scenario.
    config.preview_enabled = false;
    config.clock = clock.clone();
    config.cloud = Some(cloud.clone());
    config.cloud_poll_interval = STEP;
    config.cache_dir = Some(dir.join("cache"));
    config.cube_textures = CubeTextures::resolve(&dir.join("textures"));
    config.tile_geometry = EARTH;
    config.texture_resolution = 8192;
    config.auto_refresh = Some(STEP);
    config.wallpaper = sink.clone();

    let engine = sunlit_core::engine::start(config).expect("the soak test needs a working adapter");

    // The fetch is not the thing to measure from: the decoded image is parked
    // in the mailbox and reaches the GPU on a later tick, so the baseline has to
    // be taken after the first cloud texture exists or the warm-up it is
    // compared against would include that upload.
    wait_until("the first cloud fetch", || cloud.fetches() >= 1);
    wait_until("the first cloud texture", || {
        engine.send(EngineCommand::Poke);
        engine.memory_report().is_ok_and(|report| {
            report
                .expected
                .iter()
                .any(|texture| texture.label == "cloud_texture")
        })
    });

    let startup = private_bytes();
    let started = Instant::now();

    // The first fetch above consumed version 1; every publication after that
    // must be downloaded too.
    let mut expected_fetches = 1;
    let mut readings: Vec<Option<u64>> = Vec::new();

    for step in 1..=STEPS {
        let published = step % STEPS_PER_CLOUD_UPDATE == 0;
        if published {
            cloud.publish();
            expected_fetches += 1;
        }
        clock.advance(STEP);
        engine.send(EngineCommand::Poke);

        // Auto-refresh runs inside the engine's tick, so the export count
        // reaching `step` proves the engine processed this simulated step.
        let target = usize::try_from(step).expect("step fits in usize");
        wait_until("the scheduled wallpaper export", || sink.count() >= target);

        if published {
            // Let the cloud worker actually consume this version before the
            // next one is published, so the run really does process every
            // update rather than coalescing them away.
            wait_until("the cloud download", || cloud.fetches() >= expected_fetches);
        }

        readings.push(private_bytes());
    }

    let elapsed = started.elapsed();
    let exports = sink.count();
    let fetches = cloud.fetches();
    let report = engine.memory_report().expect("a memory report");
    let surface = engine
        .tile_report()
        .expect("a tile report")
        .expect("the engine draws from the cube");
    engine.shutdown();

    // The week was drawn from the cube, not the grid, and crossed into
    // February, whose floor was made resident on the way.
    for label in ["day_floor", "night_floor", "water_mask", "tile_array"] {
        assert!(
            report.expected.iter().any(|texture| texture.label == label),
            "no {label} at the end of the week:
{report}"
        );
    }
    assert!(
        surface.floors_installed >= 2 && !surface.resident.is_empty(),
        "{surface:#?}"
    );
    assert!(
        surface
            .named
            .iter()
            .all(|id| id.pack != tiles::PackKind::Day(0)),
        "January's tiles are still named after the hand-over: {surface:#?}"
    );

    let expected_publications = STEPS / STEPS_PER_CLOUD_UPDATE;
    let expected_fetches = expected_publications + 1;
    let simulated_days = STEPS / 24;
    println!(
        "{simulated_days} simulated days in {:.1}s: {exports} exports, {fetches} cloud fetches \
         (of {expected_publications} publications)",
        elapsed.as_secs_f64()
    );

    assert_eq!(
        exports,
        usize::try_from(STEPS).expect("steps fit in usize"),
        "one wallpaper export per simulated hour"
    );
    assert!(
        fetches >= expected_fetches,
        "expected at least {expected_fetches} cloud downloads, got {fetches}"
    );
    // The point is compression, not a benchmark: seven days in two minutes is
    // still a ratio of about 8 000 to 1, and the bound is there to catch a
    // change that makes a step cost an order of magnitude more rather than to
    // measure the machine. The measurements are in docs/testing.md. If a slower
    // runner trips this, reduce STEPS (fewer simulated days, proportionally
    // fewer publications) or shrink the render sizes; do not raise the bound.
    assert!(
        elapsed < Duration::from_mins(2),
        "{simulated_days} simulated days took {:.1}s, which defeats the purpose",
        elapsed.as_secs_f64()
    );

    // Memory: the whole point. A hidden path that parked one decoded frame per
    // update would cost hundreds of megabytes over these updates; this
    // architecture should add nothing per update at all.
    for (step, bytes) in (1..).zip(&readings) {
        if step % FLOOR_WINDOW == 0
            && let Some(bytes) = bytes
        {
            println!("  step {step:>4}: private {:.1} MiB", mib(*bytes));
        }
    }

    let floor = |from: u64| -> Option<u64> {
        let from = usize::try_from(from).expect("step fits in usize");
        let window = usize::try_from(FLOOR_WINDOW).expect("window fits in usize");
        readings[from..from + window]
            .iter()
            .copied()
            .collect::<Option<Vec<u64>>>()?
            .into_iter()
            .min()
    };
    let baseline = floor(WARMUP_STEPS);
    let end = floor(STEPS - FLOOR_WINDOW);

    let (startup, baseline, end) = match (startup, baseline, end) {
        (Some(startup), Some(baseline), Some(end)) => (startup, baseline, end),
        // Three independent reads feed this. On a platform `memory::snapshot`
        // implements, a missing sample is a broken counter and not a reason to
        // stop testing: it fails here, the way the software-adapter test fails
        // when the adapter it queried for should have been there.
        (startup, baseline, end) => {
            assert!(
                !memory_counters_supported(),
                "memory counters are implemented on this platform but a sample was \
                 missing (startup {startup:?}, baseline {baseline:?}, end {end:?}); \
                 the growth assertion must not be skipped here"
            );
            eprintln!("no memory counters on this platform, skipping the growth assertion");
            return;
        }
    };
    let warmup = baseline.saturating_sub(startup);
    let growth = end.saturating_sub(baseline);
    let soaked_days = (STEPS - WARMUP_STEPS) / 24;
    println!(
        "private bytes: startup {:.1} MiB, floor after warm-up {:.1} MiB (+{:.1}), \
         floor at the end {:.1} MiB (+{:.1} over {soaked_days} simulated days)",
        mib(startup),
        mib(baseline),
        mib(warmup),
        mib(end),
        mib(growth),
    );

    assert!(
        growth < GROWTH_LIMIT,
        "private bytes grew by {:.1} MiB over the {soaked_days} simulated days after \
         warm-up (limit {:.0} MiB, two decoded frames)",
        mib(growth),
        mib(GROWTH_LIMIT)
    );

    // Warm-up is a one-off, but a gross regression in it should not slip
    // through.
    assert!(
        warmup < WARMUP_LIMIT,
        "warm-up allocated {:.1} MiB (limit {:.0} MiB)",
        mib(warmup),
        mib(WARMUP_LIMIT)
    );
}
