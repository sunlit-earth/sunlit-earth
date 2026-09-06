use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::Duration;

use sunlit_core::engine::EngineCommand;

use crate::display_change::{CHANGE_TIMEOUT, NOTHING_HAPPENS_IN};
use crate::groups::FIXTURES;
use crate::harness::{Gpu, Harness, TIMEOUT, gpu};
use crate::textures::mib;

// ---------------------------------------------------------------------------
// The cloud variant follows the texture resolution
// ---------------------------------------------------------------------------

/// The variant width in a cloud URL, from the one path segment shaped `WxH`.
fn variant_width_of(url: &str) -> Option<u32> {
    url.split('/').find_map(|segment| {
        let (width, height) = segment.split_once('x')?;
        let width: u32 = width.parse().ok()?;
        let _: u32 = height.parse().ok()?;
        Some(width)
    })
}

/// A cloud source that serves an image sized after the variant it was last
/// pointed at, so a test can see which variant reached the GPU rather than
/// only which URL was asked for.
///
/// The images are small next to the real ones and still far enough apart that
/// replacing one with the other moves wgpu's texture counter.
struct VariantCloud {
    width: Mutex<u32>,
    fetches: std::sync::atomic::AtomicU64,
    retargets: Mutex<Vec<String>>,
    /// Every fetch fails while this is set, as an outage would.
    offline: std::sync::atomic::AtomicBool,
}

impl VariantCloud {
    /// A JPEG this many times narrower than the variant it stands for.
    const SCALE: u32 = 8;

    fn new(width: u32) -> Self {
        Self {
            width: Mutex::new(width),
            fetches: std::sync::atomic::AtomicU64::new(0),
            retargets: Mutex::new(Vec::new()),
            offline: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn set_offline(&self, offline: bool) {
        self.offline
            .store(offline, std::sync::atomic::Ordering::SeqCst);
    }

    /// The image size this source serves for `variant_width`.
    fn image_size(variant_width: u32) -> (u32, u32) {
        (
            variant_width / Self::SCALE,
            variant_width / (Self::SCALE * 2),
        )
    }

    fn fetches(&self) -> u64 {
        self.fetches.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// The variant width of every URL this source has been pointed at, in
    /// order. The first is the one construction sets, which is how the updater
    /// makes the cache entry's name and its contents agree.
    fn retarget_widths(&self) -> Vec<u32> {
        self.retargets
            .lock()
            .expect("retarget log")
            .iter()
            .map(|url| variant_width_of(url).expect("a cloud URL names its variant"))
            .collect()
    }
}

impl sunlit_core::assets::cloud_source::CloudSource for VariantCloud {
    fn fetch_if_changed(
        &self,
        known_etag: Option<&str>,
    ) -> Result<Option<sunlit_core::assets::cloud_source::CloudImage>, String> {
        if self.offline.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("the fixture is offline".to_owned());
        }
        let width = *self.width.lock().expect("variant width");
        let etag = format!("v{width}");
        if known_etag == Some(etag.as_str()) {
            return Ok(None);
        }
        let (w, h) = Self::image_size(width);
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            w,
            h,
            image::Rgba([200, 200, 200, 255]),
        ))
        .into_rgb8()
        .write_to(&mut buf, image::ImageFormat::Jpeg)
        .map_err(|e| format!("encode: {e}"))?;
        self.fetches
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Some(sunlit_core::assets::cloud_source::CloudImage {
            bytes: buf.into_inner(),
            etag: Some(etag),
            last_modified: None,
        }))
    }

    fn describe(&self) -> String {
        "variant fixture".to_owned()
    }

    fn retarget(&self, url: &str) {
        self.retargets
            .lock()
            .expect("retarget log")
            .push(url.to_owned());
        if let Some(width) = variant_width_of(url) {
            *self.width.lock().expect("variant width") = width;
        }
    }
}

/// The width the variant cases start and end at, the one they switch to, and
/// the one no case leaves in the disk cache.
const VARIANT_WIDE: u32 = 8192;
const VARIANT_NARROW: u32 = 2048;
const VARIANT_UNCACHED: u32 = 4096;

/// The engine whose cloud source serves a different image per variant.
struct Variant {
    harness: Harness,
    source: Arc<VariantCloud>,
}

impl std::ops::Deref for Variant {
    type Target = Harness;
    fn deref(&self) -> &Harness {
        &self.harness
    }
}

static VARIANT: LazyLock<Variant> = LazyLock::new(|| {
    let dir = FIXTURES.join("variant");
    std::fs::create_dir_all(&dir).expect("create the variant cache directory");
    let source = Arc::new(VariantCloud::new(VARIANT_WIDE));
    let cloud = Arc::clone(&source) as Arc<dyn sunlit_core::assets::cloud_source::CloudSource>;
    let harness = Harness::start(move |config| {
        config.texture_resolution = VARIANT_WIDE;
        config.cloud = Some(cloud);
        config.cache_dir = Some(dir);
    });
    wait_for_cloud_size(
        &harness,
        VariantCloud::image_size(VARIANT_WIDE),
        "at startup",
    );
    Variant { harness, source }
});

fn variant(_gpu: &Gpu) -> &'static Variant {
    let group = &*VARIANT;
    if group.harness.restore_resolution() {
        wait_for_cloud_size(
            &group.harness,
            VariantCloud::image_size(VARIANT_WIDE),
            "putting the variant back",
        );
    }
    group.harness.reset();
    group
}

/// Block until the cloud texture in the report is `expected`, or panic.
///
/// Polling the report is also what keeps the engine ticking, and there is no
/// event for "the cloud overlay changed": it is an overlay, so it is
/// deliberately not part of `TexturesReady`.
pub(crate) fn wait_for_cloud_size(harness: &Harness, expected: (u32, u32), what: &str) {
    let deadline = std::time::Instant::now() + TIMEOUT;
    loop {
        let report = harness.engine.memory_report().expect("a report");
        if let Some(cloud) = report
            .expected
            .iter()
            .find(|texture| texture.label == "cloud_texture")
            && (cloud.width, cloud.height) == expected
        {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{what}: no {expected:?} cloud texture within {TIMEOUT:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The fetched cloud variant follows the resolution: a switch costs a refetch
/// of the new variant, and the replacement frees the texture it replaced.
#[test]
fn the_cloud_variant_follows_the_texture_resolution() {
    let gpu = gpu();
    let group = variant(&gpu);
    let before = group.engine.memory_report().expect("a report");
    let fetches = group.source.fetches();
    assert_eq!(
        group.source.retarget_widths()[0],
        VARIANT_WIDE,
        "construction points the source at the configured variant"
    );

    group.set_texture_resolution(VARIANT_UNCACHED);
    wait_for_cloud_size(
        group,
        VariantCloud::image_size(VARIANT_UNCACHED),
        "after the switch",
    );
    let after = group.engine.memory_report().expect("a report");

    assert_eq!(
        *group
            .source
            .retarget_widths()
            .last()
            .expect("the switch retargeted the source"),
        VARIANT_UNCACHED
    );
    assert_eq!(
        group.source.fetches(),
        fetches + 1,
        "a switch to a variant nothing has cached must cost a fetch of it"
    );

    // One cloud texture, at the new size: the replacement went through the
    // slot rather than beside it.
    let sizes: Vec<(u32, u32)> = after
        .expected
        .iter()
        .filter(|texture| texture.label == "cloud_texture")
        .map(|texture| (texture.width, texture.height))
        .collect();
    assert_eq!(sizes, [VariantCloud::image_size(VARIANT_UNCACHED)]);

    // And the old one was actually freed, not merely forgotten. Skipped where
    // the backend keeps no texture counter; D3D12 and Vulkan both do.
    let (Ok(measured_before), Ok(measured_after)) = (
        u64::try_from(before.counters.texture_bytes),
        u64::try_from(after.counters.texture_bytes),
    ) else {
        panic!("wgpu reported negative texture memory");
    };
    println!(
        "wgpu texture bytes: {:.2} MiB before, {:.2} MiB after",
        mib(measured_before),
        mib(measured_after)
    );
    if measured_before == 0 {
        println!("skipping the free check: this backend maintains no texture counter");
        return;
    }
    let freed = before.expected_bytes() - after.expected_bytes();
    assert!(
        measured_after + freed / 2 <= measured_before,
        "wgpu still holds {:.2} MiB against {:.2} MiB before, having dropped {:.2} MiB \
         of expected textures:\n{after}",
        mib(measured_after),
        mib(measured_before),
        mib(freed)
    );
}

/// A switch back to a variant whose cache entry is still fresh has to show
/// that variant again, through the engine rather than by hand.
///
/// The whole chain matters here and the unit tests cannot reach it: the command
/// retargets the worker, the worker calls `set_resolution` before its next
/// poll, that poll is answered with a 304 because the entry it just adopted is
/// current, and nothing in production calls `post_cached` after startup.
#[test]
fn a_switch_back_to_a_cached_variant_shows_it_again() {
    let gpu = gpu();
    let group = variant(&gpu);

    group.set_texture_resolution(VARIANT_NARROW);
    wait_for_cloud_size(
        group,
        VariantCloud::image_size(VARIANT_NARROW),
        "after switching down",
    );
    let downloaded = group.source.fetches();

    // Both entries are now on disk and neither has gone stale, so the switch
    // back is answered with a 304 and has to fall back to what it cached.
    group.set_texture_resolution(VARIANT_WIDE);
    wait_for_cloud_size(
        group,
        VariantCloud::image_size(VARIANT_WIDE),
        "after switching back up",
    );
    assert_eq!(
        group.source.fetches(),
        downloaded,
        "the switch back is served from disk, not downloaded again"
    );
}

/// A switch must not empty the cloud slot: a cloudless globe while a download
/// runs is a worse picture than one at the previous variant, and offline the
/// gap would never close.
#[test]
fn a_switch_keeps_the_old_cloud_texture_until_the_new_one_lands() {
    const WIDE: u32 = VARIANT_WIDE;
    const NARROW: u32 = VARIANT_NARROW;

    let _gpu = gpu();
    let source = Arc::new(VariantCloud::new(WIDE));
    let cloud = Arc::clone(&source) as Arc<dyn sunlit_core::assets::cloud_source::CloudSource>;
    // An engine of its own, and no cache directory. A failed poll puts the
    // fetcher into a fifteen second backoff, which the shared engine would then
    // hand to whichever case came next, and a cache would answer the switch
    // from disk rather than leaving it unanswered, which is the state this case
    // is about.
    let harness = Harness::start(move |config| {
        config.texture_resolution = WIDE;
        config.cloud = Some(cloud);
    });
    let showing = VariantCloud::image_size(WIDE);
    wait_for_cloud_size(&harness, showing, "at startup");

    source.set_offline(true);
    harness
        .engine
        .send(EngineCommand::SetTextureResolution(NARROW));

    // The retarget is a command, so a reply to a later one proves it was taken;
    // what follows has to be given a few of the engine's own ticks, because the
    // assertion is that nothing happens.
    let deadline = std::time::Instant::now() + CHANGE_TIMEOUT;
    while source.retarget_widths().last() != Some(&NARROW) {
        assert!(
            std::time::Instant::now() < deadline,
            "the switch should still have reached the cloud pipeline: {:?}",
            source.retarget_widths()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(NOTHING_HAPPENS_IN);

    let report = harness.engine.memory_report().expect("a report");
    let sizes: Vec<(u32, u32)> = report
        .expected
        .iter()
        .filter(|texture| texture.label == "cloud_texture")
        .map(|texture| (texture.width, texture.height))
        .collect();
    assert_eq!(
        sizes,
        [showing],
        "the old cloud texture must survive a switch the network cannot answer:\n{report}"
    );
}
