//! Background cloud texture fetcher.
//!
//! Downloads an 8K equirectangular cloud JPEG from the matteason/live-cloud-maps
//! service, caches it to disk, and polls for updates every 60 minutes using
//! HEAD + `ETag` freshness checks.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use slint::ComponentHandle;

use crate::renderer::DecodedTextureMessage;
use crate::texture_loader::{self, DecodedImage};
use crate::MainWindow;

const CLOUD_URL: &str = "https://clouds.matteason.co.uk/images/8192x4096/clouds.jpg";
const POLL_INTERVAL: Duration = Duration::from_secs(3600);
const INITIAL_RETRY_DELAY: Duration = Duration::from_secs(15);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(300);

/// Metadata cached alongside the cloud JPEG for freshness checks.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CacheMeta {
    etag: Option<String>,
    last_modified: Option<String>,
}

fn cache_dir() -> Option<PathBuf> {
    Some(dirs::data_local_dir()?.join("SunlitEarth"))
}

pub(crate) fn cache_image_path() -> Option<PathBuf> {
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
            eprintln!("Warning: could not serialize cloud cache meta: {e}");
            return;
        }
    };

    if let Some(parent) = path.parent()
        && let Err(e) = fs::create_dir_all(parent)
    {
        eprintln!(
            "Warning: could not create cloud cache directory {}: {e}",
            parent.display()
        );
        return;
    }

    let tmp_path = path.with_extension("toml~");
    if let Err(e) = fs::write(&tmp_path, &toml_str) {
        eprintln!(
            "Warning: could not write cloud cache meta {}: {e}",
            tmp_path.display()
        );
        return;
    }

    if let Err(e) = fs::rename(&tmp_path, path) {
        eprintln!(
            "Warning: could not rename cloud cache meta to {}: {e}",
            path.display()
        );
    }
}

/// Check if the remote cloud image has changed using HEAD + `If-None-Match`.
///
/// Returns `Ok(true)` if unchanged (304), `Ok(false)` if new content is
/// available (200), or `Err` on transport/server errors.
fn check_freshness(agent: &ureq::Agent, etag: &str) -> Result<bool, String> {
    let response = agent
        .head(CLOUD_URL)
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
fn download_image(agent: &ureq::Agent) -> Result<(Vec<u8>, CacheMeta), String> {
    let mut response = agent
        .get(CLOUD_URL)
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
pub(crate) fn decode_cloud_jpeg(bytes: &[u8]) -> Result<DecodedImage, String> {
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

/// Run a synchronous cloud cache freshness check and download if stale.
///
/// Used by the headless renderer before wallpaper export. Not called from
/// the Slint rendering path (which uses the background cloud fetcher thread).
///
/// Called before each headless wallpaper render to ensure cloud data is
/// current. Reuses the existing `check_freshness()` and `download_image()`
/// logic. Returns `Ok(true)` if a fresh image was downloaded, `Ok(false)`
/// if the cache was already current, or `Err` on failure.
pub(crate) fn check_and_refresh_cloud_cache() -> Result<bool, String> {
    let image_path = cache_image_path().ok_or("Could not determine cloud cache path")?;
    let meta_path = cache_meta_path().ok_or("Could not determine cloud meta path")?;

    let user_agent = format!("sunlit.earth/{}", env!("CARGO_PKG_VERSION"));
    let agent = ureq::Agent::config_builder()
        .user_agent(&user_agent)
        .build()
        .new_agent();

    let cached_meta = load_cache_meta(&meta_path);

    let should_download = match &cached_meta {
        Some(meta) if meta.etag.is_some() => {
            let etag = meta.etag.as_ref().unwrap();
            match check_freshness(&agent, etag) {
                Ok(true) => {
                    eprintln!("Cloud image unchanged (304)");
                    false
                }
                Ok(false) => {
                    eprintln!("Cloud image has changed, downloading...");
                    true
                }
                Err(e) => {
                    eprintln!("Warning: {e}");
                    false
                }
            }
        }
        _ => {
            // No cache or no ETag: need to download
            if image_path.exists() {
                // Cache file exists but no meta — use cached file as-is
                eprintln!("No cloud cache metadata, using existing cache");
                false
            } else {
                eprintln!("No cached cloud image, downloading...");
                true
            }
        }
    };

    if should_download {
        let start = std::time::Instant::now();
        let (bytes, meta) = download_image(&agent)?;
        let elapsed = start.elapsed().as_secs_f64();
        eprintln!(
            "Downloaded cloud image ({} bytes) in {elapsed:.1}s",
            bytes.len()
        );

        // Save to cache
        if let Some(parent) = image_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::write(&image_path, &bytes)
            .map_err(|e| format!("Could not cache cloud image: {e}"))?;
        save_cache_meta(&meta, &meta_path);

        Ok(true)
    } else {
        Ok(false)
    }
}

/// Spawn the background cloud fetcher thread.
///
/// On the calling thread: loads the cached JPEG (if it exists), decodes it,
/// and sends it via the texture channel so clouds appear on the first frame.
///
/// On the background thread: polls for updates every 60 minutes using HEAD +
/// `ETag`, downloading a fresh image only when the remote has changed.
#[allow(clippy::too_many_lines)]
pub fn spawn_cloud_fetcher(
    tx: mpsc::Sender<DecodedTextureMessage>,
    window_weak: slint::Weak<MainWindow>,
    clouds_slot: usize,
) {
    let image_path = cache_image_path();
    let meta_path = cache_meta_path();

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
                    eprintln!(
                        "Loaded cached cloud image ({}\u{d7}{}) from {}",
                        img.width,
                        img.height,
                        path.display()
                    );
                    let _ = tx.send(DecodedTextureMessage {
                        slot_index: clouds_slot,
                        result: Ok(img),
                    });
                    let ww = window_weak.clone();
                    let _ = ww.upgrade_in_event_loop(|win| {
                        win.window().request_redraw();
                    });
                }
                Err(e) => eprintln!("Warning: cached cloud image decode failed: {e}"),
            },
            Err(e) => eprintln!("Warning: could not read cached cloud image: {e}"),
        }
    }

    // Background thread for network I/O
    let tx_bg = tx;
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
                    match check_freshness(&agent, etag) {
                        Ok(true) => {
                            eprintln!("Cloud image unchanged (304)");
                            false
                        }
                        Ok(false) => {
                            eprintln!("Cloud image has changed, downloading...");
                            true
                        }
                        Err(e) => {
                            eprintln!("Warning: {e}");
                            false
                        }
                    }
                }
                _ => {
                    eprintln!("No cached cloud ETag, downloading...");
                    true
                }
            };

            if should_download {
                let start = std::time::Instant::now();
                match download_image(&agent) {
                    Ok((bytes, meta)) => {
                        let elapsed = start.elapsed().as_secs_f64();
                        eprintln!(
                            "Downloaded cloud image ({} bytes) in {elapsed:.1}s",
                            bytes.len()
                        );

                        // Save to cache
                        if let Some(ref path) = image_path {
                            if let Some(parent) = path.parent() {
                                let _ = fs::create_dir_all(parent);
                            }
                            if let Err(e) = fs::write(path, &bytes) {
                                eprintln!("Warning: could not cache cloud image: {e}");
                            }
                        }
                        if let Some(ref path) = meta_path {
                            save_cache_meta(&meta, path);
                        }
                        cached_meta = Some(meta);

                        // Decode and send
                        match decode_cloud_jpeg(&bytes) {
                            Ok(img) => {
                                eprintln!(
                                    "Decoded cloud image ({}\u{d7}{})",
                                    img.width, img.height
                                );
                                let _ = tx_bg.send(DecodedTextureMessage {
                                    slot_index: clouds_slot,
                                    result: Ok(img),
                                });
                                let ww = window_weak_bg.clone();
                                let _ = ww.upgrade_in_event_loop(|win| {
                                    win.window().request_redraw();
                                });
                            }
                            Err(e) => eprintln!("Warning: cloud image decode failed: {e}"),
                        }

                        // Reset backoff on success
                        retry_delay = INITIAL_RETRY_DELAY;
                    }
                    Err(e) => {
                        eprintln!(
                            "Warning: {e} (retrying in {}s)",
                            retry_delay.as_secs()
                        );
                        std::thread::sleep(retry_delay);
                        retry_delay = (retry_delay * 2).min(MAX_RETRY_DELAY);
                        continue;
                    }
                }
            }

            std::thread::sleep(POLL_INTERVAL);
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
    fn cache_image_path_ends_with_expected_name() {
        let path = cache_image_path().expect("should resolve cloud cache path");
        assert!(
            path.ends_with("clouds_cache.jpg"),
            "path should end with clouds_cache.jpg, got: {path:?}"
        );
        // Verify it's inside the SunlitEarth directory
        let parent = path.parent().expect("path should have parent");
        assert!(
            parent.ends_with("SunlitEarth"),
            "parent should end with SunlitEarth, got: {parent:?}"
        );
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
