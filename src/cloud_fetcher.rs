//! Background cloud texture fetcher.
//!
//! Downloads an 8K equirectangular cloud JPEG from the matteason/live-cloud-maps
//! service, caches it to disk, and polls for updates every 60 minutes using
//! HEAD + `ETag` freshness checks.
//!
//! Three values can be overridden through the environment so tests can point
//! the fetcher at a local stub server without touching the real cache:
//! `SUNLIT_EARTH_CLOUD_URL`, `SUNLIT_EARTH_CLOUD_POLL_SECS`, and
//! `SUNLIT_EARTH_CACHE_DIR`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use slint::ComponentHandle;
use tracing::{info, warn};

use crate::renderer::{DecodedTextureMessage, TextureMailbox};
use crate::texture_loader::{self, DecodedImage};
use crate::MainWindow;

const CLOUD_URL: &str = "https://clouds.matteason.co.uk/images/8192x4096/clouds.jpg";
const POLL_INTERVAL: Duration = Duration::from_secs(3600);
const INITIAL_RETRY_DELAY: Duration = Duration::from_secs(15);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(300);

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

/// Resolve the cloud image URL from an optional environment override.
fn resolve_cloud_url(raw: Option<&str>) -> String {
    raw.map_or_else(|| CLOUD_URL.to_owned(), ToOwned::to_owned)
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
            warn!(value, env = ENV_POLL_SECS, "invalid poll interval, using default");
            POLL_INTERVAL
        }
    }
}

/// Resolve the cloud cache directory from an optional environment override.
fn resolve_cache_dir(raw: Option<&str>) -> Option<PathBuf> {
    match raw {
        Some(dir) => Some(PathBuf::from(dir)),
        None => Some(dirs::data_local_dir()?.join("SunlitEarth")),
    }
}

fn cache_dir() -> Option<PathBuf> {
    resolve_cache_dir(crate::env_override(ENV_CACHE_DIR).as_deref())
}

fn cache_image_path() -> Option<PathBuf> {
    Some(cache_dir()?.join("clouds_cache.jpg"))
}

fn cache_meta_path() -> Option<PathBuf> {
    Some(cache_dir()?.join("clouds_cache_meta.toml"))
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

/// Check if the remote cloud image has changed using HEAD + `If-None-Match`.
///
/// Returns `Ok(true)` if unchanged (304), `Ok(false)` if new content is
/// available (200), or `Err` on transport/server errors.
#[tracing::instrument(skip(agent), fields(etag = %etag))]
fn check_freshness(agent: &ureq::Agent, url: &str, etag: &str) -> Result<bool, String> {
    let response = agent
        .head(url)
        .header("If-None-Match", etag)
        .call()
        .map_err(|e| format!("Cloud freshness check failed: {e}"))?;

    if response.status().as_u16() == 304 {
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Download the cloud image unconditionally.
///
/// Returns the raw JPEG bytes and extracted cache metadata (`ETag`, Last-Modified).
#[tracing::instrument(skip(agent), fields(url = %url))]
fn download_image(agent: &ureq::Agent, url: &str) -> Result<(Vec<u8>, CacheMeta), String> {
    let mut response = agent
        .get(url)
        .call()
        .map_err(|e| format!("Cloud download failed: {e}"))?;

    let etag = response
        .headers()
        .get("ETag")
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let last_modified = response
        .headers()
        .get("Last-Modified")
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    // 8K cloud JPEG can be ~15 MB, raise the default 10 MB limit
    let body = response
        .body_mut()
        .with_config()
        .limit(50 * 1024 * 1024)
        .read_to_vec()
        .map_err(|e| format!("Failed to read cloud image body: {e}"))?;

    Ok((
        body,
        CacheMeta {
            etag,
            last_modified,
        },
    ))
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

/// Spawn the background cloud fetcher thread.
///
/// On the calling thread: loads the cached JPEG (if it exists), decodes it,
/// and posts it to the texture mailbox so clouds appear on the first frame.
///
/// On the background thread: polls for updates every 60 minutes using HEAD +
/// `ETag`, downloading a fresh image only when the remote has changed.
#[allow(clippy::too_many_lines)]
pub fn spawn_cloud_fetcher(
    mailbox: TextureMailbox,
    window_weak: slint::Weak<MainWindow>,
    clouds_slot: usize,
) {
    let image_path = cache_image_path();
    let meta_path = cache_meta_path();
    let cloud_url = resolve_cloud_url(crate::env_override(ENV_CLOUD_URL).as_deref());
    let poll_interval = resolve_poll_interval(crate::env_override(ENV_POLL_SECS).as_deref());
    info!(
        url = %cloud_url,
        poll_secs = poll_interval.as_secs(),
        cache_dir = ?cache_dir(),
        "cloud fetcher configuration"
    );

    // Load cached image synchronously so clouds appear on the first frame
    let mut cached_meta = meta_path
        .as_ref()
        .and_then(|p| load_cache_meta(p));

    if let Some(ref path) = image_path
        && path.exists()
    {
        match fs::read(path) {
            Ok(bytes) => match decode_cloud_jpeg(&bytes) {
                Ok(img) => {
                    info!(
                        width = img.width,
                        height = img.height,
                        path = %path.display(),
                        "loaded cached cloud image"
                    );
                    mailbox.post(DecodedTextureMessage {
                        slot_index: clouds_slot,
                        result: Ok(img),
                    });
                    let ww = window_weak.clone();
                    let _ = ww.upgrade_in_event_loop(|win| {
                        win.window().request_redraw();
                    });
                }
                Err(e) => warn!(error = %e, "cached cloud image decode failed"),
            },
            Err(e) => warn!(error = %e, "could not read cached cloud image"),
        }
    }

    // Background thread for network I/O
    let mailbox_bg = mailbox;
    let window_weak_bg = window_weak;
    std::thread::spawn(move || {
        let user_agent = format!("sunlit.earth/{}", env!("CARGO_PKG_VERSION"));
        let agent = ureq::Agent::config_builder()
            .user_agent(&user_agent)
            .build()
            .new_agent();
        let mut retry_delay = INITIAL_RETRY_DELAY;

        loop {
            let should_download = match &cached_meta {
                Some(meta) if meta.etag.is_some() => {
                    let etag = meta.etag.as_ref().unwrap();
                    match check_freshness(&agent, &cloud_url, etag) {
                        Ok(true) => {
                            info!("cloud image unchanged (304 Not Modified)");
                            false
                        }
                        Ok(false) => {
                            info!("cloud image has changed, downloading");
                            true
                        }
                        Err(e) => {
                            warn!(error = %e, "cloud freshness check failed");
                            false
                        }
                    }
                }
                _ => {
                    info!("no cached cloud ETag, downloading");
                    true
                }
            };

            if should_download {
                let start = std::time::Instant::now();
                match download_image(&agent, &cloud_url) {
                    Ok((bytes, meta)) => {
                        let elapsed = start.elapsed().as_secs_f64();
                        info!(
                            bytes = bytes.len(),
                            elapsed_secs = format_args!("{elapsed:.1}"),
                            "downloaded cloud image"
                        );

                        // Save to cache
                        if let Some(ref path) = image_path {
                            if let Some(parent) = path.parent() {
                                let _ = fs::create_dir_all(parent);
                            }
                            if let Err(e) = fs::write(path, &bytes) {
                                warn!(error = %e, "could not cache cloud image");
                            }
                        }
                        if let Some(ref path) = meta_path {
                            save_cache_meta(&meta, path);
                        }
                        cached_meta = Some(meta);

                        // Decode and send
                        match decode_cloud_jpeg(&bytes) {
                            Ok(img) => {
                                info!(
                                    width = img.width,
                                    height = img.height,
                                    "decoded cloud image"
                                );
                                crate::memory::log_memory_usage("after cloud decode");
                                mailbox_bg.post(DecodedTextureMessage {
                                    slot_index: clouds_slot,
                                    result: Ok(img),
                                });
                                let ww = window_weak_bg.clone();
                                let _ = ww.upgrade_in_event_loop(|win| {
                                    win.window().request_redraw();
                                });
                            }
                            Err(e) => warn!(error = %e, "cloud image decode failed"),
                        }

                        // Reset backoff on success
                        retry_delay = INITIAL_RETRY_DELAY;
                    }
                    Err(e) => {
                        warn!(
                            error = %e,
                            retry_delay_secs = retry_delay.as_secs(),
                            "cloud download failed, retrying"
                        );
                        std::thread::sleep(retry_delay);
                        retry_delay = (retry_delay * 2).min(MAX_RETRY_DELAY);
                        continue;
                    }
                }
            }

            crate::memory::log_memory_usage("cloud fetcher idle");
            std::thread::sleep(poll_interval);
        }
    });
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
        let _ = fs::remove_dir_all(
            std::env::temp_dir().join("sunlit_earth_test_cloud_meta_mkdir"),
        );
        let path = dir.join("meta.toml");

        save_cache_meta(&CacheMeta::default(), &path);
        assert!(path.exists());

        let _ = fs::remove_dir_all(
            std::env::temp_dir().join("sunlit_earth_test_cloud_meta_mkdir"),
        );
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
    fn resolve_cloud_url_without_override_uses_constant() {
        assert_eq!(resolve_cloud_url(None), CLOUD_URL);
    }

    #[test]
    fn resolve_cloud_url_with_override() {
        assert_eq!(
            resolve_cloud_url(Some("http://127.0.0.1:8080/clouds.jpg")),
            "http://127.0.0.1:8080/clouds.jpg"
        );
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
            assert!(dir.ends_with("SunlitEarth"), "unexpected cache dir: {}", dir.display());
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
}
