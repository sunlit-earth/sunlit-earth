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

use crate::config::{DEFAULT_TEXTURE_RESOLUTION, TEXTURE_RESOLUTIONS};

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
/// selects the resolution variant, which is how the texture resolution setting
/// gets a cheaper download without any new asset work.
const CLOUD_URL_TEMPLATE: &str = "https://clouds.matteason.co.uk/images/{size}/clouds.jpg";
const POLL_INTERVAL: Duration = Duration::from_hours(1);
/// First delay after a failed poll. Doubles up to [`MAX_RETRY_DELAY`].
const INITIAL_RETRY_DELAY: Duration = Duration::from_secs(15);
/// Ceiling for the retry backoff, so a service outage does not turn into an
/// hourly poll that misses the recovery by 59 minutes.
const MAX_RETRY_DELAY: Duration = Duration::from_mins(5);

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

/// The cloud image variant that goes with a surface texture resolution.
///
/// The upstream service publishes exactly the three widths
/// [`TEXTURE_RESOLUTIONS`] offers, each at 2:1, so on those three the mapping
/// is one to one and the clouds are as detailed as the surface under them. The
/// renderer takes any width as a cap, though, so a width from outside the three
/// takes the widest variant that does not exceed it, and one below all of them
/// takes the smallest: sharper clouds than surface is the one combination worth
/// ruling out.
pub fn cloud_variant(resolution: u32) -> (u32, u32) {
    let width = TEXTURE_RESOLUTIONS
        .iter()
        .copied()
        .filter(|&offered| offered <= resolution)
        .max()
        .or_else(|| TEXTURE_RESOLUTIONS.iter().copied().min())
        .unwrap_or(DEFAULT_TEXTURE_RESOLUTION);
    (width, width / 2)
}

/// The upstream URL for a texture resolution's image variant.
fn variant_cloud_url(resolution: u32) -> String {
    let (w, h) = cloud_variant(resolution);
    CLOUD_URL_TEMPLATE.replace("{size}", &format!("{w}x{h}"))
}

/// Resolve the cloud image URL: the environment override wins over the variant.
fn resolve_cloud_url(raw: Option<&str>, resolution: u32) -> String {
    raw.map_or_else(|| variant_cloud_url(resolution), ToOwned::to_owned)
}

/// The cloud image URL for `resolution`, honoring `SUNLIT_EARTH_CLOUD_URL`.
pub fn cloud_url(resolution: u32) -> String {
    resolve_cloud_url(crate::env_override(ENV_CLOUD_URL).as_deref(), resolution)
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

/// Write the cached JPEG, reporting whether the entry now holds it.
///
/// A failure takes the partial file with it. `fs::write` truncates before it
/// writes, so a half-written JPEG can be left behind, and the caller must be
/// able to read the answer as "there is an image here" or "there is not"
/// without a third state: it decides whether to keep the freshness sidecar on
/// that answer alone.
fn save_cache_image(bytes: &[u8], path: &Path) -> bool {
    if let Some(parent) = path.parent()
        && let Err(e) = fs::create_dir_all(parent)
    {
        warn!(path = %parent.display(), error = %e, "could not create cloud cache directory");
        return false;
    }
    if let Err(e) = fs::write(path, bytes) {
        warn!(path = %path.display(), error = %e, "could not cache cloud image");
        let _ = fs::remove_file(path);
        return false;
    }
    true
}

/// Remove a cache entry's freshness sidecar, if it has one.
///
/// Called when the image beside it could not be written, so that what is left
/// on disk is an entry with nothing in it rather than an `ETag` claiming an
/// image that is not there.
fn discard_cache_meta(path: &Path) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            warn!(path = %path.display(), error = %e, "could not discard cloud cache meta");
        }
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
    cache_dir: Option<PathBuf>,
    /// Whether `SUNLIT_EARTH_CLOUD_URL` is in force, which decides what the
    /// cache entry is named after. Read once: the environment does not change
    /// under a running process.
    overridden: bool,
    /// Which image variant is being fetched, and which cache entry belongs to
    /// it. Follows the texture resolution through [`Self::set_resolution`].
    variant: (u32, u32),
    /// The URL the source is pointed at, which is what actually decides both
    /// the bytes and the cache entry. Kept because the variant alone cannot
    /// answer whether a switch changes anything: under the environment override
    /// every variant resolves to the same URL.
    url: String,
    image_path: Option<PathBuf>,
    meta_path: Option<PathBuf>,
    cached_meta: Option<CacheMeta>,
}

/// What a cache entry is named after.
///
/// The variant, so that a resolution switch can never be answered with the
/// previous variant's bytes and a switch back finds what it left behind. An
/// entry a run at another resolution wrote is dead weight this one never reads,
/// which is also what makes the change safe to roll back.
///
/// `SUNLIT_EARTH_CLOUD_URL` wins over the variant, and what it serves has no
/// variant at all, so its bytes get an entry of their own rather than a name
/// claiming a size nobody checked. Without that, a run with the override would
/// leave an image of any size in `clouds_cache_4096x2048.jpg`, and the next run
/// without the override would put that image on screen and keep it there until
/// its first poll returned.
fn cache_stem(variant: (u32, u32), overridden: bool) -> String {
    if overridden {
        return "clouds_cache_override".to_owned();
    }
    let (width, height) = variant;
    format!("clouds_cache_{width}x{height}")
}

/// Where a cache entry's JPEG and its metadata sidecar live.
fn cache_paths(cache_dir: Option<&Path>, stem: &str) -> (Option<PathBuf>, Option<PathBuf>) {
    (
        cache_dir.map(|dir| dir.join(format!("{stem}.jpg"))),
        cache_dir.map(|dir| dir.join(format!("{stem}_meta.toml"))),
    )
}

impl CloudUpdater {
    /// Build an updater against `source`, fetching the variant that goes with
    /// `resolution` and caching into `cache_dir` (skipped entirely when `None`).
    ///
    /// The source is pointed at the URL the cache entry is named after, so the
    /// name and the bytes behind it cannot disagree. In production that is the
    /// URL the caller already built from the same resolution; the point is that
    /// this function, not the caller, is what decides what the entry holds.
    pub fn new(
        source: Arc<dyn CloudSource>,
        mailbox: TextureMailbox,
        notify: NotifyFn,
        slot: usize,
        cache_dir: Option<PathBuf>,
        resolution: u32,
    ) -> Self {
        let variant = cloud_variant(resolution);
        let url = cloud_url(resolution);
        source.retarget(&url);
        let overridden = crate::env_override(ENV_CLOUD_URL).is_some();
        let (image_path, meta_path) =
            cache_paths(cache_dir.as_deref(), &cache_stem(variant, overridden));
        let cached_meta = meta_path.as_ref().and_then(|p| load_cache_meta(p));
        Self {
            source,
            mailbox,
            notify,
            slot,
            cache_dir,
            overridden,
            variant,
            url,
            image_path,
            meta_path,
            cached_meta,
        }
    }

    /// Follow a change of texture resolution to the cloud variant that goes
    /// with it.
    ///
    /// Retargets the source, moves to that variant's cache entry, picks up
    /// whatever freshness metadata was left there, and posts whatever image was
    /// left there. That last step is what makes the switch visible: the poll
    /// that follows sends the new entry's `ETag` to the new entry's URL, and a
    /// switch back inside the upstream refresh window is answered with a 304,
    /// which posts nothing. Without posting the bytes already on disk the globe
    /// would keep the previous variant's overlay until upstream published
    /// again, which can be hours.
    ///
    /// Nothing is thrown away, and a variant with nothing on disk posts nothing:
    /// the image already on the GPU stays until a poll delivers the new one, so
    /// a switch made offline keeps showing the old clouds rather than none, for
    /// as long as the network stays down.
    ///
    /// A resolution that maps to the variant already in force is a no-op, which
    /// is what lets the worker call this before every poll.
    pub fn set_resolution(&mut self, resolution: u32) {
        let variant = cloud_variant(resolution);
        if variant == self.variant {
            return;
        }
        self.variant = variant;

        let url = cloud_url(resolution);
        if url == self.url {
            // `SUNLIT_EARTH_CLOUD_URL` is in force, so the variant selected
            // nothing: the same image is served either way, and moving the
            // cache entry or dropping the ETag would only cost a re-download of
            // bytes already in hand.
            return;
        }
        self.url = url;
        info!(url = %self.url, resolution, "cloud variant follows the texture resolution");
        self.source.retarget(&self.url);

        let (image_path, meta_path) = cache_paths(
            self.cache_dir.as_deref(),
            &cache_stem(variant, self.overridden),
        );
        self.image_path = image_path;
        self.meta_path = meta_path;
        self.cached_meta = self.meta_path.as_ref().and_then(|p| load_cache_meta(p));
        self.post_cached();
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

    /// One freshness check, download, decode, cache, and post cycle.
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

        // Decoded before anything is persisted. The sidecar is a claim that a
        // decodable image for its ETag is on disk, and a body that does not
        // decode (a captive portal answering 200 with an error page and an
        // ETag, say) must not leave that claim anywhere: on disk it defeats
        // `post_cached` and every later switch, and in memory it turns the
        // retry the backoff exists for into a 304 with nothing to post.
        // Failing here keeps the previous entry and the previous ETag, so the
        // retry is a genuine re-download.
        let img = match decode_cloud_jpeg(&fetched.bytes) {
            Ok(img) => img,
            Err(e) => {
                warn!(error = %e, "cloud image decode failed");
                return PollOutcome::Failed;
            }
        };
        info!(
            width = img.width,
            height = img.height,
            "decoded cloud image"
        );
        crate::memory::log_memory_usage("after cloud decode");

        let cached = self
            .image_path
            .as_deref()
            .is_some_and(|path| save_cache_image(&fetched.bytes, path));

        let meta = CacheMeta {
            etag: fetched.etag,
            last_modified: fetched.last_modified,
        };
        if let Some(path) = self.meta_path.as_deref() {
            // The sidecar goes only where its image went. A present sidecar is
            // read as "the image for this ETag is on disk": `set_resolution`
            // adopts the entry and posts what it finds, and the poll that
            // follows sends that ETag and is answered with a 304, which posts
            // nothing. One without an image is therefore a switch that silently
            // does nothing, and on a fresh process, no clouds at all until
            // upstream publishes. An entry with neither costs one download.
            if cached {
                save_cache_meta(&meta, path);
            } else {
                discard_cache_meta(path);
            }
        }
        // Kept in memory whatever the disk did. The frame below is about to be
        // on the GPU, so a 304 on the next poll is the right answer either way,
        // and with no cache directory at all this is the only copy there is.
        self.cached_meta = Some(meta);

        self.post(img);
        PollOutcome::Updated
    }

    fn post(&self, img: DecodedImage) {
        self.mailbox.post(DecodedTextureMessage {
            slot_index: self.slot,
            result: Ok(img),
            // The texture resolution does govern which variant this is, but the
            // cloud slot is never purged, so there is nothing for a stamp to
            // protect. A fetch of the old variant landing after a switch is one
            // poll of exactly the picture the switch deliberately leaves up,
            // and the next poll replaces it.
            generation: None,
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
    fn every_offered_resolution_has_its_own_variant() {
        assert_eq!(cloud_variant(2048), (2048, 1024));
        assert_eq!(cloud_variant(4096), (4096, 2048));
        assert_eq!(cloud_variant(8192), (8192, 4096));
    }

    /// The mapping has to be injective on the three offered widths, or lowering
    /// the resolution would not lower the download.
    #[test]
    fn the_variant_grows_with_the_resolution() {
        let mut widths: Vec<u32> = TEXTURE_RESOLUTIONS
            .iter()
            .map(|&w| cloud_variant(w).0)
            .collect();
        widths.sort_unstable();
        widths.dedup();
        assert_eq!(widths.len(), TEXTURE_RESOLUTIONS.len());
    }

    /// Every variant is 2:1, which is what an equirectangular projection is.
    #[test]
    fn every_variant_is_two_to_one() {
        for &width in &TEXTURE_RESOLUTIONS {
            let (w, h) = cloud_variant(width);
            assert_eq!(w, h * 2);
        }
    }

    /// The renderer takes any width as a cap, so a width from outside the three
    /// still has to land on a variant that exists, and never on a sharper one
    /// than the surface it sits over.
    #[test]
    fn a_width_outside_the_offered_three_takes_the_widest_that_fits() {
        assert_eq!(cloud_variant(9000), (8192, 4096));
        assert_eq!(cloud_variant(8191), (4096, 2048));
        assert_eq!(cloud_variant(2049), (2048, 1024));
        assert_eq!(cloud_variant(128), (2048, 1024));
        assert_eq!(cloud_variant(0), (2048, 1024));
    }

    #[test]
    fn the_url_names_the_variant() {
        assert!(variant_cloud_url(2048).contains("2048x1024"));
        assert!(variant_cloud_url(4096).contains("4096x2048"));
        assert!(variant_cloud_url(8192).contains("8192x4096"));
    }

    #[test]
    fn resolve_cloud_url_without_override_uses_the_variant() {
        assert_eq!(resolve_cloud_url(None, 2048), variant_cloud_url(2048));
    }

    #[test]
    fn environment_override_wins_over_the_variant() {
        for resolution in TEXTURE_RESOLUTIONS {
            assert_eq!(
                resolve_cloud_url(Some("http://127.0.0.1:8080/clouds.jpg"), resolution),
                "http://127.0.0.1:8080/clouds.jpg"
            );
        }
    }

    /// Two variants must not share a cache entry, or a switch could be answered
    /// with the previous variant's bytes.
    #[test]
    fn each_variant_has_its_own_cache_entry() {
        let dir = PathBuf::from("C:/tmp/sunlit");
        let mut seen = std::collections::HashSet::new();
        for &width in &TEXTURE_RESOLUTIONS {
            let (image, meta) = cache_paths(Some(&dir), &cache_stem(cloud_variant(width), false));
            let image = image.expect("a cache dir was given");
            let meta = meta.expect("a cache dir was given");
            assert_ne!(image, meta);
            assert!(seen.insert(image), "two variants share a JPEG path");
            assert!(seen.insert(meta), "two variants share a meta path");
        }
    }

    /// What the override serves has no variant, so it must not be filed under a
    /// name that claims one: the next run without the override would read that
    /// entry and show whatever the override was pointed at.
    #[test]
    fn the_environment_override_gets_a_cache_entry_of_its_own() {
        let overridden = cache_stem(cloud_variant(4096), true);
        for &width in &TEXTURE_RESOLUTIONS {
            assert_ne!(cache_stem(cloud_variant(width), false), overridden);
        }
        // And one entry whatever the resolution, since the URL is the same.
        for &width in &TEXTURE_RESOLUTIONS {
            assert_eq!(cache_stem(cloud_variant(width), true), overridden);
        }
    }

    #[test]
    fn no_cache_directory_means_no_cache_paths() {
        let (image, meta) = cache_paths(None, &cache_stem(cloud_variant(4096), false));
        assert!(image.is_none());
        assert!(meta.is_none());
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
        /// Number of upcoming calls that serve a body that is not a JPEG.
        corrupt: std::sync::atomic::AtomicU64,
        /// Every URL this source has been pointed at, in order.
        retargets: std::sync::Mutex<Vec<String>>,
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
                corrupt: std::sync::atomic::AtomicU64::new(0),
                retargets: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn retargets(&self) -> Vec<String> {
            self.retargets.lock().expect("retarget log").clone()
        }

        /// Drop the retarget construction performed, so a test can talk about
        /// the ones a switch performs.
        fn forget_retargets(&self) {
            self.retargets.lock().expect("retarget log").clear();
        }

        /// Make the next `count` calls fail, as an offline service would.
        fn fail_next(&self, count: u64) {
            self.failures
                .store(count, std::sync::atomic::Ordering::SeqCst);
        }

        /// Make the next `count` calls serve a body that is not a JPEG, as a
        /// captive portal answering 200 with an error page would.
        fn corrupt_next(&self, count: u64) {
            self.corrupt
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
            let bytes = if self
                .corrupt
                .fetch_update(
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                    |n| (n > 0).then(|| n - 1),
                )
                .is_ok()
            {
                b"an error page, not a JPEG".to_vec()
            } else {
                self.jpeg.clone()
            };
            Ok(Some(CloudImage {
                bytes,
                etag: Some(current),
                last_modified: None,
            }))
        }

        fn describe(&self) -> String {
            "scripted".to_owned()
        }

        fn retarget(&self, url: &str) {
            self.retargets
                .lock()
                .expect("retarget log")
                .push(url.to_owned());
        }
    }

    use super::super::cloud_source::CloudImage;

    fn updater_for(source: Arc<ScriptedSource>, mailbox: &TextureMailbox) -> CloudUpdater {
        CloudUpdater::new(
            source,
            mailbox.clone(),
            no_notify(),
            3,
            None,
            DEFAULT_TEXTURE_RESOLUTION,
        )
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
        let mut updater =
            CloudUpdater::new(source, mailbox, notify, 3, None, DEFAULT_TEXTURE_RESOLUTION);

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

        let mut first = cached_updater(&source, &mailbox, &dir, DEFAULT_TEXTURE_RESOLUTION);
        first.poll_once();
        mailbox.take_all();

        // A fresh updater reads the ETag back and does not re-download.
        let mut second = cached_updater(&source, &mailbox, &dir, DEFAULT_TEXTURE_RESOLUTION);
        assert_eq!(second.poll_once(), PollOutcome::Unchanged);
        assert_eq!(source.fetches(), 1);
        assert!(second.post_cached(), "the cached JPEG should decode");
        assert_eq!(mailbox.take_all().len(), 1);

        let _ = fs::remove_dir_all(&dir);
    }

    /// An updater on a disk cache at a chosen resolution.
    fn cached_updater(
        source: &Arc<ScriptedSource>,
        mailbox: &TextureMailbox,
        dir: &Path,
        resolution: u32,
    ) -> CloudUpdater {
        CloudUpdater::new(
            Arc::clone(source) as Arc<dyn CloudSource>,
            mailbox.clone(),
            no_notify(),
            3,
            Some(dir.to_path_buf()),
            resolution,
        )
    }

    // -----------------------------------------------------------------------
    // Following the texture resolution
    // -----------------------------------------------------------------------

    /// Construction points the source at the URL the cache entry is named
    /// after, so the two cannot disagree whatever the caller built the source
    /// with.
    #[test]
    fn construction_points_the_source_at_the_variant_it_will_cache_under() {
        let source = Arc::new(ScriptedSource::new());
        let mailbox = TextureMailbox::new(4);
        let _updater = CloudUpdater::new(
            Arc::clone(&source) as Arc<dyn CloudSource>,
            mailbox,
            no_notify(),
            3,
            None,
            8192,
        );
        let retargets = source.retargets();
        assert_eq!(retargets.len(), 1, "expected one retarget: {retargets:?}");
        assert!(
            retargets[0].contains("8192x4096"),
            "unexpected URL: {}",
            retargets[0]
        );
    }

    #[test]
    fn set_resolution_points_the_source_at_the_new_variant() {
        let source = Arc::new(ScriptedSource::new());
        let mailbox = TextureMailbox::new(4);
        let mut updater = updater_for(Arc::clone(&source), &mailbox);
        source.forget_retargets();

        updater.set_resolution(2048);
        let retargets = source.retargets();
        assert_eq!(retargets.len(), 1, "expected one retarget: {retargets:?}");
        assert!(
            retargets[0].contains("2048x1024"),
            "unexpected URL: {}",
            retargets[0]
        );
    }

    /// The worker calls this before every poll, so the common case has to be
    /// free of both a retarget and a cache reload.
    #[test]
    fn set_resolution_to_the_variant_in_force_does_nothing() {
        let source = Arc::new(ScriptedSource::new());
        let mailbox = TextureMailbox::new(4);
        let mut updater = updater_for(Arc::clone(&source), &mailbox);
        source.forget_retargets();

        for _ in 0..3 {
            updater.set_resolution(DEFAULT_TEXTURE_RESOLUTION);
        }
        assert!(source.retargets().is_empty());
    }

    /// Two resolutions that share a variant share everything: the URL does not
    /// change, so neither should anything else.
    #[test]
    fn two_resolutions_with_one_variant_do_not_retarget() {
        let source = Arc::new(ScriptedSource::new());
        let mailbox = TextureMailbox::new(4);
        let mut updater = updater_for(Arc::clone(&source), &mailbox);

        assert_eq!(cloud_variant(2048), cloud_variant(2049));
        updater.set_resolution(2048);
        source.forget_retargets();
        updater.set_resolution(2049);
        assert!(source.retargets().is_empty());
    }

    /// A switch back to a variant whose entry is still fresh has to put those
    /// bytes on screen.
    ///
    /// The poll that follows the switch sends the entry's own `ETag` to the
    /// entry's own URL and is answered with a 304, which posts nothing at all.
    /// The slot is never purged either, so without the switch itself posting
    /// what is on disk the overlay would stay at the previous variant until
    /// upstream published again, which can be hours. Nothing in production
    /// calls `post_cached` after startup, so the switch is the only place this
    /// can happen.
    #[test]
    fn a_switch_back_to_a_fresh_cache_entry_puts_its_pixels_on_screen() {
        let dir = std::env::temp_dir().join("sunlit_earth_test_cloud_switch_back");
        let _ = fs::remove_dir_all(&dir);
        let source = Arc::new(ScriptedSource::new());
        let mailbox = TextureMailbox::new(4);

        let mut updater = cached_updater(&source, &mailbox, &dir, 8192);
        assert_eq!(updater.poll_once(), PollOutcome::Updated);
        mailbox.take_all();

        // Down to a variant with nothing on disk: the switch posts nothing,
        // which is what leaves the wide overlay up while the download runs.
        updater.set_resolution(2048);
        assert!(
            mailbox.take_all().is_empty(),
            "a variant with no cache entry has nothing to post"
        );
        assert_eq!(updater.poll_once(), PollOutcome::Updated);
        mailbox.take_all();

        // Back up, inside the upstream refresh window.
        updater.set_resolution(8192);
        assert_eq!(
            mailbox.take_all().len(),
            1,
            "the switch must post the cached wide image"
        );
        assert_eq!(
            updater.poll_once(),
            PollOutcome::Unchanged,
            "the entry is still fresh, so the poll posts nothing"
        );
        assert_eq!(source.fetches(), 2, "and costs no third download");

        let _ = fs::remove_dir_all(&dir);
    }

    /// An image that could not be cached must leave no freshness claim behind,
    /// including one an earlier poll left there.
    ///
    /// The switch path reads a present sidecar as "the image for this `ETag` is
    /// on disk": it adopts the entry, posts what it finds, and the poll that
    /// follows sends that `ETag` and is answered with a 304. A sidecar with no
    /// image behind it is therefore a switch that does nothing, and a fresh
    /// process with no clouds at all until upstream publishes.
    ///
    /// The write is made to fail by putting a directory where the JPEG goes,
    /// which fails on both platforms and, unlike an unwritable parent, leaves
    /// the sidecar beside it perfectly writable. That is what makes the two
    /// halves separable here.
    #[test]
    fn an_image_that_could_not_be_cached_leaves_no_freshness_claim() {
        let dir = std::env::temp_dir().join("sunlit_earth_test_cloud_failed_image_write");
        let _ = fs::remove_dir_all(&dir);
        let stem = cache_stem(cloud_variant(8192), false);
        let image = dir.join(format!("{stem}.jpg"));
        let meta = dir.join(format!("{stem}_meta.toml"));

        let source = Arc::new(ScriptedSource::new());
        let mailbox = TextureMailbox::new(4);
        let mut updater = cached_updater(&source, &mailbox, &dir, 8192);

        // A healthy poll first, so there is an entry for the failure to have to
        // clear rather than merely decline to create.
        assert_eq!(updater.poll_once(), PollOutcome::Updated);
        mailbox.take_all();
        assert!(image.is_file() && meta.is_file());

        fs::remove_file(&image).expect("remove the cached image");
        fs::create_dir(&image).expect("occupy the image path with a directory");
        source.publish();

        assert_eq!(
            updater.poll_once(),
            PollOutcome::Updated,
            "a cache that cannot be written must not stop the frame reaching the screen"
        );
        assert_eq!(mailbox.take_all().len(), 1);
        assert!(
            !meta.is_file(),
            "an ETag was recorded for an image that is not on disk"
        );

        // A restart is a fresh updater reading that entry: it has to download
        // rather than revalidate into a 304 with nothing to post.
        let mut restarted = cached_updater(&source, &mailbox, &dir, 8192);
        assert!(!restarted.post_cached(), "there is no image to post");
        assert_eq!(restarted.poll_once(), PollOutcome::Updated);
        assert_eq!(source.fetches(), 3);

        let _ = fs::remove_dir_all(&dir);
    }

    /// The same rule one door further in: a body that downloads but does not
    /// decode must leave no freshness claim behind, on disk or in memory.
    ///
    /// The in-memory half is what keeps the retry alive: recording the bad
    /// body's `ETag` would turn the backoff's next poll into a 304 with
    /// nothing to post, and the session would have no clouds until upstream
    /// published. The disk half keeps the last good entry intact, so a
    /// restart or a switch still shows the newest image that ever decoded.
    #[test]
    fn an_undecodable_body_leaves_no_freshness_claim() {
        let dir = std::env::temp_dir().join("sunlit_earth_test_cloud_undecodable_body");
        let _ = fs::remove_dir_all(&dir);
        let stem = cache_stem(cloud_variant(8192), false);
        let image = dir.join(format!("{stem}.jpg"));
        let meta = dir.join(format!("{stem}_meta.toml"));

        let source = Arc::new(ScriptedSource::new());
        let mailbox = TextureMailbox::new(4);
        let mut updater = cached_updater(&source, &mailbox, &dir, 8192);

        assert_eq!(updater.poll_once(), PollOutcome::Updated);
        mailbox.take_all();
        let good_image = fs::read(&image).expect("the healthy poll cached its image");
        let good_meta = fs::read(&meta).expect("and its sidecar");

        source.publish();
        source.corrupt_next(1);
        assert_eq!(
            updater.poll_once(),
            PollOutcome::Failed,
            "a body that does not decode is a failed poll, not an update"
        );
        assert!(mailbox.take_all().is_empty());
        assert_eq!(
            fs::read(&image).expect("the last good image stays"),
            good_image
        );
        assert_eq!(
            fs::read(&meta).expect("with the sidecar that matches it"),
            good_meta
        );

        assert_eq!(
            updater.poll_once(),
            PollOutcome::Updated,
            "the retry must re-download, not revalidate into a 304"
        );
        assert_eq!(mailbox.take_all().len(), 1);
        assert_eq!(source.fetches(), 3);

        let _ = fs::remove_dir_all(&dir);
    }

    /// The other half of the same rule: with no cache entry and no network,
    /// a switch posts nothing rather than blanking the overlay.
    #[test]
    fn a_switch_with_nothing_cached_and_no_network_posts_nothing() {
        let dir = std::env::temp_dir().join("sunlit_earth_test_cloud_switch_offline");
        let _ = fs::remove_dir_all(&dir);
        let source = Arc::new(ScriptedSource::new());
        let mailbox = TextureMailbox::new(4);

        let mut updater = cached_updater(&source, &mailbox, &dir, 8192);
        assert_eq!(updater.poll_once(), PollOutcome::Updated);
        mailbox.take_all();

        source.fail_next(1);
        updater.set_resolution(2048);
        assert_eq!(updater.poll_once(), PollOutcome::Failed);
        assert!(
            mailbox.take_all().is_empty(),
            "nothing to post means nothing posted, so the old overlay stays"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// A switch must not be answered out of the previous variant's cache, and a
    /// switch back must find what it left behind.
    #[test]
    fn each_variant_keeps_its_own_cached_image() {
        let dir = std::env::temp_dir().join("sunlit_earth_test_cloud_variant_cache");
        let _ = fs::remove_dir_all(&dir);
        let source = Arc::new(ScriptedSource::new());
        let mailbox = TextureMailbox::new(4);

        let mut updater = cached_updater(&source, &mailbox, &dir, 8192);
        assert_eq!(updater.poll_once(), PollOutcome::Updated);
        mailbox.take_all();

        // Down: the ETag that satisfied the wide variant must not satisfy the
        // narrow one, so the switch costs a real download.
        updater.set_resolution(2048);
        assert_eq!(
            updater.poll_once(),
            PollOutcome::Updated,
            "the new variant must be fetched, not served from the old entry"
        );
        assert_eq!(source.fetches(), 2);
        mailbox.take_all();

        // Up again: the wide entry is still on disk with its own ETag, so this
        // is a 304 rather than a third download. What reaches the screen in
        // that case is `a_switch_back_to_a_fresh_cache_entry_puts_its_pixels_on_screen`.
        updater.set_resolution(8192);
        assert_eq!(updater.poll_once(), PollOutcome::Unchanged);
        assert_eq!(source.fetches(), 2);

        let _ = fs::remove_dir_all(&dir);
    }
}
