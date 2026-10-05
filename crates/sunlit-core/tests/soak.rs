//! Mock-clock soak test.
//!
//! Two simulated days of cloud updates and unattended wallpaper exports,
//! compressed into a few seconds by advancing an injected clock instead of
//! waiting. This is the permanent guard against a background producer whose
//! consumer only runs under some condition.
//!
//! The globe is drawn from the cube surface of the Earth fixture, so the
//! transcoder, the floors, the tile loader and a change of month at the
//! middle of January all run under the same clock as the clouds and the
//! exports.
//!
//! The leak checks are exact rather than statistical: after every simulated
//! hour no decoded pixel buffer and no download is alive, and wgpu's own
//! counters read the same after every hour once the warm-up is over. Private
//! bytes are a coarse backstop for what none of those sees.
//!
//! Nothing here touches the network, the desktop, or the real clock.

#[path = "../src/test_support.rs"]
mod test_support;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use sunlit_core::assets::cloud_source::{CloudImage, CloudSource, Download, downloads};
use sunlit_core::assets::cube_layout::CubeTextures;
use sunlit_core::assets::texture_loader::decoded_pixels;
use sunlit_core::assets::tiles;
use sunlit_core::engine::clock::MockClock;
use sunlit_core::engine::wallpaper_sink::CountingSink;
use sunlit_core::engine::{EngineCommand, EngineConfig};
use sunlit_core::memory_report::CounterSection;
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
const STEP: Duration = Duration::from_hours(1);
/// Two simulated days at one step per hour.
///
/// What the run has to hold is the schedule: an export every hour, a
/// publication every three, the change of month at hour 12, and enough of
/// both after the warm-up for the counters to be compared across many
/// publications and across midnight, which the second day gives. The leak
/// checks need no length of their own, since each one is exact after every
/// step.
const STEPS: u64 = 2 * 24;
/// The upstream cloud service publishes every three hours.
const STEPS_PER_CLOUD_UPDATE: u64 = 3;
/// Fixture cloud image size. The checks count frames rather than bytes, so the
/// size buys nothing but time, and a small image keeps the per-update cost of
/// the decode, the mips and the first draw that samples them out of the run.
const CLOUD_WIDTH: u32 = 512;
const CLOUD_HEIGHT: u32 = 256;
/// Wallpaper export size.
///
/// Small on purpose, and the test is about the schedule and the memory rather
/// than the picture: what the export has to do here is go through the publish
/// path once per simulated hour, which it does at any size.
const EXPORT_SIZE: (u32, u32) = (160, 96);

/// How long to wait for the engine to catch up with one simulated step.
const STEP_TIMEOUT: Duration = Duration::from_secs(30);

/// The Earth fixture cut as `tests/golden.rs` cuts it.
const EARTH: tiles::Geometry = tiles::Geometry {
    face: 256,
    levels: 2,
    tile: 32,
    gutter: 4,
    floor: 64,
    mask: 128,
};

/// Days from the Unix epoch to the start of the run, January 16th, half a day
/// before January hands over to February at its middle, so the change of month
/// and the floor of the month ahead land inside the warm-up.
const START_DAY: i64 = 15;

/// Steps before the counters and private bytes are compared: through the
/// change of month at step 12 and two publications after it. The rest of the
/// year's packs, the floors and tiles made resident, and the allocator pools
/// are a one-off cost, and the leak this test guards against is per update.
const WARMUP_STEPS: u64 = 18;

/// Growth of private bytes allowed from the end of the warm-up to the end of
/// the run.
///
/// A backstop rather than the test: the exact checks are the decoded frames,
/// the downloads and wgpu's counters. This one is there for host memory
/// outside the counted buffers and GPU memory on a backend that keeps no byte
/// counters, and at this fixture it reaches only so far: the hours it compares,
/// 18 and 48, are ten publications and thirty exports apart, so it fails a leak
/// of more than about 6.4 MiB a publication or 2.1 MiB an export, and not one
/// of the 384 KiB a color decode of the fixture goes through. It is generous
/// because single readings move: lavapipe's allocator steps by 8 to 12 MiB at a
/// time, and on `macos-latest` about one reading in twenty is 8 to 11 MiB high.
const PRIVATE_GROWTH_LIMIT: u64 = 64 * 1024 * 1024;

/// A cloud source that serves a fixed JPEG under an `ETag` the test controls.
struct FixtureCloud {
    jpeg: Vec<u8>,
    version: AtomicU64,
    fetches: AtomicU64,
}

impl FixtureCloud {
    fn new(width: u32, height: u32) -> Self {
        // A gradient rather than a flat value, so the JPEG is a realistic size
        // and the decode does real work. Three components, so the decode goes
        // through the color arm rather than the gray one.
        let mut img = image::RgbImage::new(width, height);
        for (x, y, px) in img.enumerate_pixels_mut() {
            #[allow(clippy::cast_possible_truncation)]
            let v = ((x + y) % 256) as u8;
            *px = image::Rgb([v, v, v]);
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
            bytes: Download::new(self.jpeg.clone()),
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
/// the first of those may skip the private-bytes check.
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

/// Wait until every frame the downloads so far were decoded into has been
/// made and dropped again.
///
/// `made` reaching `made_by_now` is what tells "uploaded" from "not decoded
/// yet": a live count of zero alone would also hold in the moment between a
/// download and its decode. A frame that is still alive at the deadline is a
/// decoded pixel buffer parked somewhere, and the failure says so.
fn wait_for_decoded_frames_to_go(step: u64, made_by_now: u64) {
    let deadline = Instant::now() + STEP_TIMEOUT;
    loop {
        let count = decoded_pixels();
        if count.made >= made_by_now && count.frames == 0 && count.bytes == 0 {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "simulated hour {step}: {} decoded frames ({:.2} MiB) were still alive {STEP_TIMEOUT:?} \
             after the hour, with {} of the {made_by_now} frames its downloads need made; a frame \
             that outlives its upload is a decoded pixel buffer parked in a queue, a cache or a \
             long-lived struct",
            count.frames,
            mib(count.bytes),
            count.made
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Wait until every download so far has been made and dropped again.
///
/// The download outlives its decode by the rest of the poll that fetched it,
/// writing it to the cache and posting the frame, so it may still be alive for
/// a moment after its frame has gone. One still alive at the deadline is
/// parked somewhere, and the failure says so.
fn wait_for_downloads_to_go(step: u64, made_by_now: u64) {
    let deadline = Instant::now() + STEP_TIMEOUT;
    loop {
        let count = downloads();
        if count.made >= made_by_now && count.buffers == 0 && count.bytes == 0 {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "simulated hour {step}: {} downloads ({} bytes) were still alive {STEP_TIMEOUT:?} \
             after the hour, with {} of the {made_by_now} downloads so far made; a download \
             that outlives its poll is parked in a queue, a cache or a long-lived struct",
            count.buffers,
            count.bytes,
            count.made
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// wgpu's counters, by name, as the memory report reads them.
fn named_counters(counters: CounterSection) -> [(&'static str, i64); 5] {
    [
        ("texture bytes", counters.texture_bytes),
        ("buffer bytes", counters.buffer_bytes),
        ("textures", counters.textures),
        ("buffers", counters.buffers),
        ("allocations", counters.allocations),
    ]
}

/// The counters the backend under each software adapter the suite runs on
/// keeps in wgpu 28 (research section 28): D3D12 under WARP the bytes and both
/// object counts, Vulkan under lavapipe the bytes and the buffer count.
///
/// On these a counter that reads zero or below is a check gone missing rather
/// than one the backend never kept, so it fails the test. Any other adapter
/// has its counters inferred from the readings alone.
fn kept_counters(adapter: &str) -> &'static [&'static str] {
    match adapter {
        "warp" => &["texture bytes", "buffer bytes", "textures", "buffers"],
        "lavapipe" => &["texture bytes", "buffer bytes", "buffers"],
        _ => &[],
    }
}

/// Hold every counter wgpu maintains on this backend to the value it had at
/// the first reading, and say which ones it does not maintain.
///
/// The readings are every hour's after the warm-up, from settled reports: the
/// staging buffers of a write or a draw after the hour's export would
/// otherwise be counted until the next export retires them, which a tile that
/// lands after the export makes a matter of timing.
///
/// A counter that reads zero there is one the backend does not keep, since a
/// kept one counts at least the textures and buffers every frame uses, and one
/// that reads below zero is one it keeps half of, counting down on destroy and
/// never up on create, as Vulkan does with textures in wgpu 28. Neither is
/// asserted on, except where [`kept_counters`] says the backend keeps it.
fn check_counters(adapter: &str, readings: &[(u64, CounterSection)]) {
    let (_, first) = readings[0];
    for (index, (name, baseline)) in named_counters(first).into_iter().enumerate() {
        assert!(
            baseline > 0 || !kept_counters(adapter).contains(&name),
            "wgpu {name} reads {baseline} on {adapter}, whose backend keeps it: the check on it \
             is lost, through wgpu's `counters` feature left off or a backend that stopped \
             counting it"
        );
        if baseline == 0 {
            println!("wgpu {name}: reads zero on {adapter}, which does not keep it; not checked");
            continue;
        }
        if baseline < 0 {
            println!(
                "wgpu {name}: reads {baseline} on {adapter}, which counts it down and not up; \
                 not checked"
            );
            continue;
        }
        let moved: Vec<String> = readings
            .iter()
            .filter(|(_, counters)| named_counters(*counters)[index].1 != baseline)
            .map(|(step, counters)| {
                format!("{} at hour {step}", named_counters(*counters)[index].1)
            })
            .collect();
        assert!(
            moved.is_empty(),
            "wgpu {name} read {baseline} after the warm-up and then {} on {adapter}: something \
             the engine makes per update or per day is not freed",
            moved.join(", ")
        );
        println!(
            "wgpu {name}: {baseline} at every one of {} hours after the warm-up",
            readings.len()
        );
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn two_days_of_simulated_clouds_and_exports_leave_nothing_behind() {
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
    // Live time, so every simulated hour really does rotate the Earth and the
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

    // The decoded image is parked in the mailbox and reaches the GPU on a later
    // tick, so the run starts once the first cloud texture exists.
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
    // Every download is one decoded frame; the ones before this point are in
    // `made` already or about to be.
    let made_before = decoded_pixels().made.saturating_sub(cloud.fetches());
    let downloaded_before = downloads().made.saturating_sub(cloud.fetches());

    let started = Instant::now();

    // The first fetch above consumed version 1; every publication after that
    // must be downloaded too.
    let mut expected_fetches = 1;
    let mut private: Vec<Option<u64>> = Vec::new();
    let mut counters: Vec<(u64, CounterSection)> = Vec::new();
    let mut adapter = String::new();

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
        wait_for_decoded_frames_to_go(step, made_before + cloud.fetches());
        wait_for_downloads_to_go(step, downloaded_before + cloud.fetches());

        private.push(private_bytes());
        if step > WARMUP_STEPS {
            let report = engine.settled_memory_report().expect("a memory report");
            counters.push((step, report.counters));
            adapter = report.adapter;
        }
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

    // The run was drawn from the cube, not the grid, and crossed into
    // February, whose floor was made resident on the way.
    for label in ["day_floor", "night_floor", "water_mask", "tile_array"] {
        assert!(
            report.expected.iter().any(|texture| texture.label == label),
            "no {label} at the end of the run:
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
         (of {expected_publications} publications), no decoded frame or download alive after \
         any hour",
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
    // The point is compression, not a benchmark: the bound is there to catch a
    // change that makes a step cost an order of magnitude more rather than to
    // measure the machine. The measurements are in docs/testing.md. If a slower
    // runner trips this, shrink the render sizes; do not raise the bound.
    assert!(
        elapsed < Duration::from_secs(30),
        "{simulated_days} simulated days took {:.1}s, which defeats the purpose",
        elapsed.as_secs_f64()
    );

    check_counters(&adapter, &counters);

    for (row, chunk) in private.chunks(12).enumerate() {
        let values: Vec<String> = chunk
            .iter()
            .map(|bytes| bytes.map_or_else(|| "-".to_owned(), |b| format!("{:.1}", mib(b))))
            .collect();
        println!(
            "  hours {:>2} to {:>2}, private MiB: {}",
            row * 12 + 1,
            row * 12 + chunk.len(),
            values.join(" ")
        );
    }
    let warm = usize::try_from(WARMUP_STEPS).expect("steps fit in usize") - 1;
    let (Some(baseline), Some(end)) = (private[warm], private[private.len() - 1]) else {
        // Independent reads feed this. On a platform `memory::snapshot`
        // implements, a missing sample is a broken counter and not a reason to
        // stop testing: it fails here, the way the software-adapter test fails
        // when the adapter it queried for should have been there.
        assert!(
            !memory_counters_supported(),
            "memory counters are implemented on this platform but a sample was missing \
             (after the warm-up {:?}, at the end {:?})",
            private[warm],
            private[private.len() - 1]
        );
        eprintln!("no memory counters on this platform, skipping the private-bytes backstop");
        return;
    };
    let growth = end.saturating_sub(baseline);
    println!(
        "private bytes: {:.1} MiB after the warm-up, {:.1} MiB at the end (+{:.1}, limit {:.0})",
        mib(baseline),
        mib(end),
        mib(growth),
        mib(PRIVATE_GROWTH_LIMIT)
    );
    assert!(
        growth < PRIVATE_GROWTH_LIMIT,
        "private bytes grew by {:.1} MiB over the {} simulated hours after the warm-up \
         (limit {:.0} MiB)",
        mib(growth),
        STEPS - WARMUP_STEPS,
        mib(PRIVATE_GROWTH_LIMIT)
    );
}
