//! Cloud texture caching, decoding, and polling.
//!
//! A [`CloudUpdater`] owns the on-disk cache and turns a [`CloudSource`] into
//! decoded frames parked in the texture mailbox. The engine drives it from its
//! own worker thread on its own schedule.
//!
//! Three values can be overridden through the environment so tests can point
//! the fetcher at a local stub server without touching the real cache:
//! `SUNLIT_EARTH_CLOUD_URL`, `SUNLIT_EARTH_CLOUD_POLL_SECS`, and
//! `SUNLIT_EARTH_CACHE_DIR`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::config::QualityTier;

use super::cloud_source::CloudSource;
use super::mailbox::{DecodedTextureMessage, TextureMailbox};
use super::texture_loader::{self, DecodedImage};

/// Callback invoked after a new frame has been posted, so a client that only
/// works on demand knows there is something waiting. Headless callers that poll
/// on their own schedule pass a no-op.
pub type NotifyFn = Arc<dyn Fn() + Send + Sync>;

/// A notify callback that does nothing.
pub fn no_notify() -> NotifyFn {
    Arc::new(|| {})
}

/// Base of the upstream cloud service. The path segment before `clouds.jpg`
/// selects the resolution variant, which is how the quality tiers get a
/// cheaper download without any new asset work.
const CLOUD_URL_TEMPLATE: &str = "https://clouds.matteason.co.uk/images/{size}/clouds.jpg";
const POLL_INTERVAL: Duration = Duration::from_secs(3600);
/// First delay after a failed poll. Doubles up to [`MAX_RETRY_DELAY`].
const INITIAL_RETRY_DELAY: Duration = Duration::from_secs(15);
/// Ceiling for the retry backoff, so a service outage does not turn into an
/// hourly poll that misses the recovery by 59 minutes.
const MAX_RETRY_DELAY: Duration = Duration::from_secs(300);

/// Exponential backoff for failed cloud polls.
///
/// Split out from the worker loop so the schedule can be tested without
/// actually sleeping through it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryBackoff {
    delay: Duration,
}

impl RetryBackoff {
    pub fn new() -> Self {
        Self {
            delay: INITIAL_RETRY_DELAY,
        }
    }

    /// How long to wait before the next attempt.
    pub fn delay(&self) -> Duration {
        self.delay
    }

    /// Record a failure and return how long to wait before retrying.
    pub fn fail(&mut self) -> Duration {
        let current = self.delay;
        self.delay = (self.delay * 2).min(MAX_RETRY_DELAY);
        current
    }

    /// Record a success, so the next outage starts from the short delay again.
    pub fn reset(&mut self) {
        self.delay = INITIAL_RETRY_DELAY;
    }
}

impl Default for RetryBackoff {
    fn default() -> Self {
        Self::new()
    }
}

/// Environment variable overriding the cloud image URL.
const ENV_CLOUD_URL: &str = "SUNLIT_EARTH_CLOUD_URL";
/// Environment variable overriding the poll interval, in seconds.
const ENV_POLL_SECS: &str = "SUNLIT_EARTH_CLOUD_POLL_SECS";
/// Environment variable overriding the cloud cache directory.
const ENV_CACHE_DIR: &str = "SUNLIT_EARTH_CACHE_DIR";

/// Metadata cached alongside the cloud JPEG for freshness checks.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CacheMeta {
    etag: Option<String>,
    last_modified: Option<String>,
}

/// The upstream URL for a quality tier's image variant.
fn tier_cloud_url(tier: QualityTier) -> String {
    let (w, h) = tier.cloud_size();
    CLOUD_URL_TEMPLATE.replace("{size}", &format!("{w}x{h}"))
}

/// Resolve the cloud image URL: the environment override wins over the tier.
fn resolve_cloud_url(raw: Option<&str>, tier: QualityTier) -> String {
    raw.map_or_else(|| tier_cloud_url(tier), ToOwned::to_owned)
}

/// The configured cloud image URL for `tier`, honoring `SUNLIT_EARTH_CLOUD_URL`.
pub fn cloud_url(tier: QualityTier) -> String {
    resolve_cloud_url(crate::env_override(ENV_CLOUD_URL).as_deref(), tier)
}

/// Resolve the poll interval from an optional environment override.
///
/// Values that do not parse as a positive number of seconds are ignored
/// with a warning and the compiled-in interval is used instead.
fn resolve_poll_interval(raw: Option<&str>) -> Duration {
    let Some(value) = raw else {
        return POLL_INTERVAL;
    };
    match value.trim().parse::<u64>() {
        Ok(secs) if secs > 0 => Duration::from_secs(secs),
        _ => {
            warn!(
                value,
                env = ENV_POLL_SECS,
                "invalid poll interval, using default"
            );
            POLL_INTERVAL
        }
    }
}

/// The configured poll interval, honoring `SUNLIT_EARTH_CLOUD_POLL_SECS`.
pub fn poll_interval() -> Duration {
    resolve_poll_interval(crate::env_override(ENV_POLL_SECS).as_deref())
}

/// Resolve the cloud cache directory from an optional environment override.
fn resolve_cache_dir(raw: Option<&str>) -> Option<PathBuf> {
    match raw {
        Some(dir) => Some(PathBuf::from(dir)),
        None => Some(dirs::data_local_dir()?.join("SunlitEarth")),
    }
}

/// The configured cache directory, honoring `SUNLIT_EARTH_CACHE_DIR`.
pub fn cache_dir() -> Option<PathBuf> {
    resolve_cache_dir(crate::env_override(ENV_CACHE_DIR).as_deref())
}

fn load_cache_meta(path: &Path) -> Option<CacheMeta> {
    let contents = fs::read_to_string(path).ok()?;
    toml::from_str(&contents).ok()
}

fn save_cache_meta(meta: &CacheMeta, path: &Path) {
    let toml_str = match toml::to_string_pretty(meta) {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "could not serialize cloud cache meta");
            return;
        }
    };

    if let Some(parent) = path.parent()
        && let Err(e) = fs::create_dir_all(parent)
    {
        warn!(path = %parent.display(), error = %e, "could not create cloud cache directory");
        return;
    }

    let tmp_path = path.with_extension("toml~");
    if let Err(e) = fs::write(&tmp_path, &toml_str) {
        warn!(path = %tmp_path.display(), error = %e, "could not write cloud cache meta");
        return;
    }

    if let Err(e) = fs::rename(&tmp_path, path) {
        warn!(path = %path.display(), error = %e, "could not rename cloud cache meta");
    }
}

/// Decode a JPEG cloud image from raw bytes into RGBA8 pixel data.
///
/// Applies the same transforms as equirectangular texture loading:
/// horizontal flip and 1/4-width shift to align the prime meridian.
#[tracing::instrument(skip(bytes), fields(bytes_len = bytes.len()))]
fn decode_cloud_jpeg(bytes: &[u8]) -> Result<DecodedImage, String> {
    let img = image::load_from_memory(bytes)
        .map_err(|e| format!("Failed to decode cloud JPEG: {e}"))?
        .fliph()
        .into_rgba8();

    let width = img.width();
    let height = img.height();
    let mut pixels = img.into_raw();
    texture_loader::shift_horizontal(&mut pixels, width, height);

    Ok(DecodedImage {
        pixels,
        width,
        height,
    })
}

/// What one call to [`CloudUpdater::poll_once`] achieved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PollOutcome {
    /// A new frame was decoded and posted to the mailbox.
    Updated,
    /// The source reported no change.
    Unchanged,
    /// The fetch or the decode failed; the caller decides about backoff.
    Failed,
}

/// Turns a [`CloudSource`] into decoded frames in the texture mailbox, keeping
/// the on-disk cache in step.
pub struct CloudUpdater {
    source: Arc<dyn CloudSource>,
    mailbox: TextureMailbox,
    notify: NotifyFn,
    slot: usize,
    image_path: Option<PathBuf>,
    meta_path: Option<PathBuf>,
    cached_meta: Option<CacheMeta>,
}

impl CloudUpdater {
    /// Build an updater against `source`, caching into `cache_dir` (skipped
    /// entirely when `None`).
    pub fn new(
        source: Arc<dyn CloudSource>,
        mailbox: TextureMailbox,
        notify: NotifyFn,
        slot: usize,
        cache_dir: Option<PathBuf>,
    ) -> Self {
        let image_path = cache_dir.as_ref().map(|d| d.join("clouds_cache.jpg"));
        let meta_path = cache_dir.map(|d| d.join("clouds_cache_meta.toml"));
        let cached_meta = meta_path.as_ref().and_then(|p| load_cache_meta(p));
        Self {
            source,
            mailbox,
            notify,
            slot,
            image_path,
            meta_path,
            cached_meta,
        }
    }

    /// Decode the cached image, if any, and post it so clouds appear without
    /// waiting for the network.
    pub fn post_cached(&self) -> bool {
        let Some(path) = self.image_path.as_ref().filter(|p| p.exists()) else {
            return false;
        };
        match fs::read(path) {
            Ok(bytes) => match decode_cloud_jpeg(&bytes) {
                Ok(img) => {
                    info!(
                        width = img.width,
                        height = img.height,
                        path = %path.display(),
                        "loaded cached cloud image"
                    );
                    self.post(img);
                    true
                }
                Err(e) => {
                    warn!(error = %e, "cached cloud image decode failed");
                    false
                }
            },
            Err(e) => {
                warn!(error = %e, "could not read cached cloud image");
                false
            }
        }
    }

    /// One freshness check, download, decode, and post cycle.
    pub fn poll_once(&mut self) -> PollOutcome {
        let known_etag = self.cached_meta.as_ref().and_then(|m| m.etag.as_deref());
        let start = std::time::Instant::now();
        let fetched = match self.source.fetch_if_changed(known_etag) {
            Ok(Some(image)) => image,
            Ok(None) => return PollOutcome::Unchanged,
            Err(e) => {
                warn!(error = %e, "cloud fetch failed");
                return PollOutcome::Failed;
            }
        };

        info!(
            bytes = fetched.bytes.len(),
            elapsed_secs = format_args!("{:.1}", start.elapsed().as_secs_f64()),
            "downloaded cloud image"
        );

        if let Some(path) = &self.image_path {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Err(e) = fs::write(path, &fetched.bytes) {
                warn!(error = %e, "could not cache cloud image");
            }
        }
        let meta = CacheMeta {
            etag: fetched.etag,
            last_modified: fetched.last_modified,
        };
        if let Some(path) = &self.meta_path {
            save_cache_meta(&meta, path);
        }
        self.cached_meta = Some(meta);

        match decode_cloud_jpeg(&fetched.bytes) {
            Ok(img) => {
                info!(
                    width = img.width,
                    height = img.height,
                    "decoded cloud image"
                );
                crate::memory::log_memory_usage("after cloud decode");
                self.post(img);
                PollOutcome::Updated
            }
            Err(e) => {
                warn!(error = %e, "cloud image decode failed");
                PollOutcome::Failed
            }
        }
    }

    fn post(&self, img: DecodedImage) {
        self.mailbox.post(DecodedTextureMessage {
            slot_index: self.slot,
            result: Ok(img),
        });
        (self.notify)();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_cache_meta_nonexistent_returns_none() {
        let path = std::env::temp_dir()
            .join("sunlit_earth_test_cloud_meta_nonexistent")
            .join("meta.toml");
        let _ = fs::remove_file(&path);
        assert!(load_cache_meta(&path).is_none());
    }

    #[test]
    fn save_and_load_cache_meta_round_trip() {
        let dir = std::env::temp_dir().join("sunlit_earth_test_cloud_meta_roundtrip");
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("meta.toml");

        let meta = CacheMeta {
            etag: Some("\"abc123\"".to_owned()),
            last_modified: Some("Thu, 01 Jan 2026 00:00:00 GMT".to_owned()),
        };
        save_cache_meta(&meta, &path);
        let loaded = load_cache_meta(&path).expect("should load saved meta");
        assert_eq!(loaded.etag, meta.etag);
        assert_eq!(loaded.last_modified, meta.last_modified);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_cache_meta_creates_parent_directories() {
        let dir = std::env::temp_dir()
            .join("sunlit_earth_test_cloud_meta_mkdir")
            .join("nested");
        let _ = fs::remove_dir_all(std::env::temp_dir().join("sunlit_earth_test_cloud_meta_mkdir"));
        let path = dir.join("meta.toml");

        save_cache_meta(&CacheMeta::default(), &path);
        assert!(path.exists());

        let _ = fs::remove_dir_all(std::env::temp_dir().join("sunlit_earth_test_cloud_meta_mkdir"));
    }

    #[test]
    fn cache_meta_etag_only_round_trip() {
        let dir = std::env::temp_dir().join("sunlit_earth_test_cloud_meta_etag_only");
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("meta.toml");

        let meta = CacheMeta {
            etag: Some("\"etag-only\"".to_owned()),
            last_modified: None,
        };
        save_cache_meta(&meta, &path);
        let loaded = load_cache_meta(&path).expect("should load");
        assert_eq!(loaded.etag, meta.etag);
        assert!(loaded.last_modified.is_none());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_meta_both_none_round_trip() {
        let dir = std::env::temp_dir().join("sunlit_earth_test_cloud_meta_both_none");
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("meta.toml");

        let meta = CacheMeta {
            etag: None,
            last_modified: None,
        };
        save_cache_meta(&meta, &path);
        let loaded = load_cache_meta(&path).expect("should load");
        assert!(loaded.etag.is_none());
        assert!(loaded.last_modified.is_none());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cloud_url_follows_the_quality_tier() {
        assert!(tier_cloud_url(QualityTier::Low).contains("2048x1024"));
        assert!(tier_cloud_url(QualityTier::Medium).contains("4096x2048"));
        assert!(tier_cloud_url(QualityTier::High).contains("8192x4096"));
    }

    #[test]
    fn resolve_cloud_url_without_override_uses_the_tier() {
        assert_eq!(
            resolve_cloud_url(None, QualityTier::Low),
            tier_cloud_url(QualityTier::Low)
        );
    }

    #[test]
    fn environment_override_wins_over_the_tier() {
        for tier in [QualityTier::Low, QualityTier::Medium, QualityTier::High] {
            assert_eq!(
                resolve_cloud_url(Some("http://127.0.0.1:8080/clouds.jpg"), tier),
                "http://127.0.0.1:8080/clouds.jpg"
            );
        }
    }

    #[test]
    fn resolve_poll_interval_without_override_uses_constant() {
        assert_eq!(resolve_poll_interval(None), POLL_INTERVAL);
    }

    #[test]
    fn resolve_poll_interval_parses_seconds() {
        assert_eq!(resolve_poll_interval(Some("5")), Duration::from_secs(5));
        assert_eq!(resolve_poll_interval(Some(" 42 ")), Duration::from_secs(42));
    }

    #[test]
    fn resolve_poll_interval_rejects_invalid_values() {
        for value in ["0", "-1", "abc", "1.5", ""] {
            assert_eq!(
                resolve_poll_interval(Some(value)),
                POLL_INTERVAL,
                "expected fallback for {value:?}"
            );
        }
    }

    #[test]
    fn resolve_cache_dir_with_override() {
        let dir = resolve_cache_dir(Some("C:/tmp/sunlit")).expect("override should resolve");
        assert_eq!(dir, PathBuf::from("C:/tmp/sunlit"));
    }

    #[test]
    fn resolve_cache_dir_without_override_ends_in_app_folder() {
        if let Some(dir) = resolve_cache_dir(None) {
            assert!(
                dir.ends_with("SunlitEarth"),
                "unexpected cache dir: {}",
                dir.display()
            );
        }
    }

    #[test]
    fn decode_cloud_jpeg_invalid_bytes() {
        let result = decode_cloud_jpeg(&[0, 1, 2, 3]);
        assert!(result.is_err());
    }

    #[test]
    fn decode_cloud_jpeg_valid_minimal() {
        // Create a minimal 2x2 JPEG in memory using the image crate
        let mut buf = std::io::Cursor::new(Vec::new());
        let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([128, 64, 32, 255]));
        let rgb_img = image::DynamicImage::ImageRgba8(img).into_rgb8();
        rgb_img
            .write_to(&mut buf, image::ImageFormat::Jpeg)
            .expect("should encode minimal JPEG");

        let result = decode_cloud_jpeg(buf.get_ref());
        assert!(result.is_ok());
        let decoded = result.unwrap();
        assert_eq!(decoded.width, 2);
        assert_eq!(decoded.height, 2);
        assert_eq!(decoded.pixels.len(), 2 * 2 * 4);
    }

    // -----------------------------------------------------------------------
    // CloudUpdater against a scripted source
    // -----------------------------------------------------------------------

    /// A source that serves a fixed image and flips its `ETag` on demand, so the
    /// updater's freshness logic can be tested without a server.
    struct ScriptedSource {
        jpeg: Vec<u8>,
        version: std::sync::atomic::AtomicU64,
        fetches: std::sync::atomic::AtomicU64,
        /// Number of upcoming calls that report a transport error.
        failures: std::sync::atomic::AtomicU64,
    }

    impl ScriptedSource {
        fn new() -> Self {
            let mut buf = std::io::Cursor::new(Vec::new());
            let img = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                4,
                2,
                image::Rgba([200, 200, 200, 255]),
            ))
            .into_rgb8();
            img.write_to(&mut buf, image::ImageFormat::Jpeg)
                .expect("encode fixture");
            Self {
                jpeg: buf.into_inner(),
                version: std::sync::atomic::AtomicU64::new(1),
                fetches: std::sync::atomic::AtomicU64::new(0),
                failures: std::sync::atomic::AtomicU64::new(0),
            }
        }

        /// Make the next `count` calls fail, as an offline service would.
        fn fail_next(&self, count: u64) {
            self.failures
                .store(count, std::sync::atomic::Ordering::SeqCst);
        }

        fn publish(&self) {
            self.version
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }

        fn fetches(&self) -> u64 {
            self.fetches.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl CloudSource for ScriptedSource {
        fn fetch_if_changed(&self, known_etag: Option<&str>) -> Result<Option<CloudImage>, String> {
            if self
                .failures
                .fetch_update(
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                    |n| (n > 0).then(|| n - 1),
                )
                .is_ok()
            {
                return Err("scripted transport failure".to_owned());
            }
            let current = format!(
                "v{}",
                self.version.load(std::sync::atomic::Ordering::SeqCst)
            );
            if known_etag == Some(current.as_str()) {
                return Ok(None);
            }
            self.fetches
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Some(CloudImage {
                bytes: self.jpeg.clone(),
                etag: Some(current),
                last_modified: None,
            }))
        }

        fn describe(&self) -> String {
            "scripted".to_owned()
        }
    }

    use super::super::cloud_source::CloudImage;

    fn updater_for(source: Arc<ScriptedSource>, mailbox: &TextureMailbox) -> CloudUpdater {
        CloudUpdater::new(source, mailbox.clone(), no_notify(), 3, None)
    }

    // -----------------------------------------------------------------------
    // RetryBackoff
    // -----------------------------------------------------------------------

    #[test]
    fn backoff_starts_short_and_doubles() {
        let mut backoff = RetryBackoff::new();
        assert_eq!(backoff.fail(), INITIAL_RETRY_DELAY);
        assert_eq!(backoff.fail(), INITIAL_RETRY_DELAY * 2);
        assert_eq!(backoff.fail(), INITIAL_RETRY_DELAY * 4);
    }

    #[test]
    fn backoff_saturates_at_the_ceiling() {
        let mut backoff = RetryBackoff::new();
        for _ in 0..20 {
            backoff.fail();
        }
        assert_eq!(backoff.delay(), MAX_RETRY_DELAY);
        assert_eq!(backoff.fail(), MAX_RETRY_DELAY);
    }

    #[test]
    fn backoff_resets_after_a_success() {
        let mut backoff = RetryBackoff::new();
        backoff.fail();
        backoff.fail();
        assert_ne!(backoff.delay(), INITIAL_RETRY_DELAY);
        backoff.reset();
        assert_eq!(backoff.delay(), INITIAL_RETRY_DELAY);
    }

    // -----------------------------------------------------------------------
    // CloudUpdater
    // -----------------------------------------------------------------------

    #[test]
    fn updater_posts_a_frame_on_first_poll() {
        let source = Arc::new(ScriptedSource::new());
        let mailbox = TextureMailbox::new(4);
        let mut updater = updater_for(Arc::clone(&source), &mailbox);

        assert_eq!(updater.poll_once(), PollOutcome::Updated);
        let posted = mailbox.take_all();
        assert_eq!(posted.len(), 1);
        assert_eq!(posted[0].slot_index, 3);
    }

    #[test]
    fn updater_skips_unchanged_sources() {
        let source = Arc::new(ScriptedSource::new());
        let mailbox = TextureMailbox::new(4);
        let mut updater = updater_for(Arc::clone(&source), &mailbox);

        assert_eq!(updater.poll_once(), PollOutcome::Updated);
        assert_eq!(updater.poll_once(), PollOutcome::Unchanged);
        assert_eq!(updater.poll_once(), PollOutcome::Unchanged);
        assert_eq!(
            source.fetches(),
            1,
            "only the changed version is downloaded"
        );
    }

    #[test]
    fn updater_picks_up_a_new_publication() {
        let source = Arc::new(ScriptedSource::new());
        let mailbox = TextureMailbox::new(4);
        let mut updater = updater_for(Arc::clone(&source), &mailbox);

        updater.poll_once();
        source.publish();
        assert_eq!(updater.poll_once(), PollOutcome::Updated);
        assert_eq!(source.fetches(), 2);
    }

    #[test]
    fn updater_reports_failure_and_recovers() {
        let source = Arc::new(ScriptedSource::new());
        let mailbox = TextureMailbox::new(4);
        let mut updater = updater_for(Arc::clone(&source), &mailbox);
        let mut backoff = RetryBackoff::new();

        // Two failed attempts, each backing off further, then a success that
        // resets the schedule. This is the loop the cloud worker runs, minus
        // the sleeping.
        source.fail_next(2);
        assert_eq!(updater.poll_once(), PollOutcome::Failed);
        let first = backoff.fail();
        assert_eq!(updater.poll_once(), PollOutcome::Failed);
        let second = backoff.fail();
        assert!(second > first, "the second retry must wait longer");
        assert!(
            mailbox.take_all().is_empty(),
            "a failed poll must not post a frame"
        );

        assert_eq!(updater.poll_once(), PollOutcome::Updated);
        backoff.reset();
        assert_eq!(backoff.delay(), INITIAL_RETRY_DELAY);
        assert_eq!(mailbox.take_all().len(), 1, "the recovery posts a frame");
    }

    #[test]
    fn updater_failure_does_not_poison_the_cached_etag() {
        let source = Arc::new(ScriptedSource::new());
        let mailbox = TextureMailbox::new(4);
        let mut updater = updater_for(Arc::clone(&source), &mailbox);

        // Succeed once, then fail: the stored ETag must still be the one that
        // worked, so the next successful poll can still short-circuit on 304.
        assert_eq!(updater.poll_once(), PollOutcome::Updated);
        mailbox.take_all();
        source.fail_next(1);
        assert_eq!(updater.poll_once(), PollOutcome::Failed);
        assert_eq!(updater.poll_once(), PollOutcome::Unchanged);
    }

    #[test]
    fn updater_notifies_after_posting() {
        let source = Arc::new(ScriptedSource::new());
        let mailbox = TextureMailbox::new(4);
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let notify: NotifyFn = Arc::new(move || {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        let mut updater = CloudUpdater::new(source, mailbox, notify, 3, None);

        updater.poll_once();
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn updater_without_a_cache_dir_posts_nothing_from_disk() {
        let source = Arc::new(ScriptedSource::new());
        let mailbox = TextureMailbox::new(4);
        let updater = updater_for(source, &mailbox);
        assert!(!updater.post_cached());
        assert!(mailbox.take_all().is_empty());
    }

    #[test]
    fn updater_round_trips_through_the_disk_cache() {
        let dir = std::env::temp_dir().join("sunlit_earth_test_cloud_updater_cache");
        let _ = fs::remove_dir_all(&dir);
        let source = Arc::new(ScriptedSource::new());
        let mailbox = TextureMailbox::new(4);

        let mut first = CloudUpdater::new(
            Arc::clone(&source) as Arc<dyn CloudSource>,
            mailbox.clone(),
            no_notify(),
            3,
            Some(dir.clone()),
        );
        first.poll_once();
        mailbox.take_all();

        // A fresh updater reads the ETag back and does not re-download.
        let mut second = CloudUpdater::new(
            Arc::clone(&source) as Arc<dyn CloudSource>,
            mailbox.clone(),
            no_notify(),
            3,
            Some(dir.clone()),
        );
        assert_eq!(second.poll_once(), PollOutcome::Unchanged);
        assert_eq!(source.fetches(), 1);
        assert!(second.post_cached(), "the cached JPEG should decode");
        assert_eq!(mailbox.take_all().len(), 1);

        let _ = fs::remove_dir_all(&dir);
    }
}
