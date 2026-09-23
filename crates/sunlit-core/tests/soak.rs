//! Mock-clock soak test.
//!
//! Seven simulated days of cloud updates and unattended wallpaper exports,
//! compressed into a few seconds by advancing an injected clock instead of
//! waiting. This is the permanent guard against a background producer whose
//! consumer only runs under some condition.
//!
//! Nothing here touches the network, the desktop, or the real clock.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use sunlit_core::assets::cloud_source::{CloudImage, CloudSource};
use sunlit_core::engine::clock::MockClock;
use sunlit_core::engine::wallpaper_sink::CountingSink;
use sunlit_core::engine::{EngineCommand, EngineConfig};
use sunlit_core::params::SceneParams;

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
/// Sized with `STEPS` so the sensitivity per update is fixed, at 49
/// publications against 8 MiB. Measured growth on the development desktop is
/// 2.0 MiB with about 2.5 MiB of sample-to-sample noise, so the headroom is
/// fourfold.
const GROWTH_LIMIT: u64 = 8 * 1024 * 1024;
/// Allocation allowed during warm-up: the first cloud texture, its mip chain,
/// and wgpu's allocator pools.
const WARMUP_LIMIT: u64 = 192 * 1024 * 1024;

/// Steps to run before taking the memory baseline.
///
/// The first cloud texture, its mip chain, and wgpu's allocator pools are a
/// one-off cost of roughly 85 MiB. Measuring from before that would be
/// measuring startup, not growth; the leak this test guards against is
/// per-update, so the interesting window is everything after warm-up.
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

    let cloud = Arc::new(FixtureCloud::new(CLOUD_WIDTH, CLOUD_HEIGHT));
    let sink = Arc::new(CountingSink::new(EXPORT_SIZE.0, EXPORT_SIZE.1));
    let clock = Arc::new(MockClock::new(time::OffsetDateTime::UNIX_EPOCH));

    let mut params = SceneParams {
        texture_index: 0,
        sample_count: 1,
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
    config.cache_dir = None;
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
    let mut samples: Vec<(u64, u64)> = Vec::new();
    let mut baseline = startup;

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

        {
            let a = private_bytes().unwrap_or(0);
            let b = engine.memory_report().expect("report");
            std::thread::sleep(Duration::from_millis(50));
            engine.send(EngineCommand::Poke);
            let c = engine.memory_report().expect("report");
            let priv_of = |r: &sunlit_core::memory_report::MemoryReport| {
                r.process.as_ref().map_or(0, |p| p.private_bytes)
            };
            println!(
                "DIAG step {step:>3} pub={} fetches={} | A {:.2} | B {:.2} tex={} texmem={:.2} | C {:.2} tex={} texmem={:.2}",
                u8::from(published),
                cloud.fetches(),
                mib(a),
                mib(priv_of(&b)),
                b.counters.textures,
                mib(u64::try_from(b.counters.texture_bytes).unwrap_or(0)),
                mib(priv_of(&c)),
                c.counters.textures,
                mib(u64::try_from(c.counters.texture_bytes).unwrap_or(0)),
            );
        }
        if step % (STEPS / 8) == 0
            && let Some(bytes) = private_bytes()
        {
            samples.push((step, bytes));
        }
        if step == WARMUP_STEPS {
            baseline = private_bytes();
        }
    }

    let elapsed = started.elapsed();
    let end = private_bytes();
    let exports = sink.count();
    let fetches = cloud.fetches();
    engine.shutdown();

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
    for (step, bytes) in &samples {
        println!("  step {step:>4}: private {:.1} MiB", mib(*bytes));
    }

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
        "private bytes: startup {:.1} MiB, after warm-up {:.1} MiB (+{:.1}), \
         end {:.1} MiB (+{:.1} over {soaked_days} simulated days)",
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

    // Warm-up is a one-off (the first cloud texture, its mip chain, and wgpu's
    // allocator pools), but a gross regression in it should not slip through.
    assert!(
        warmup < WARMUP_LIMIT,
        "warm-up allocated {:.1} MiB (limit {:.0} MiB)",
        mib(warmup),
        mib(WARMUP_LIMIT)
    );
    panic!("diagnostic run: print the DIAG table");
}
