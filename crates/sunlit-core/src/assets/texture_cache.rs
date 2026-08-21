//! Downscaled copies of the local surface textures, kept on disk.
//!
//! The two surface assets are 8192 wide JPEG XL files. Loading one at 4096 or
//! 2048 means decoding the 8K source and halving it, which is the most
//! expensive thing the app does at startup, so the result is written back as a
//! PNG next to the cloud cache and every later run decodes that instead. PNG
//! because the `image` crate encodes it losslessly and decodes it in a fraction
//! of the time JPEG XL takes; the file is disposable either way.
//!
//! A cached file is a plain downscale of its source, in the source's own
//! orientation, so reading one back goes through the same
//! [`texture_loader::load`] a source does. Whether it still matches the source
//! is decided by a sidecar recording the source's size and modification time.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use super::texture_loader::{self, DecodedImage};

/// Directory under the cache directory that holds the downscales.
const CACHE_SUBDIR: &str = "texture_cache";

/// What the cached file was made from.
///
/// Size and modification time rather than a hash of the contents: the sources
/// are over a hundred megabytes each, and hashing one costs as much as the
/// decode the cache exists to avoid. Both change when an asset is replaced,
/// which is the only way these files ever change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct SourceStamp {
    bytes: u64,
    /// Milliseconds since the Unix epoch. `None` when the platform or the
    /// filesystem does not report a modification time, which leaves size as the
    /// only check rather than invalidating every run.
    modified_ms: Option<u64>,
}

impl SourceStamp {
    /// Read the stamp of `source`, or `None` when its metadata is unreadable.
    fn of(source: &Path) -> Option<Self> {
        let meta = fs::metadata(source).ok()?;
        Some(Self {
            bytes: meta.len(),
            modified_ms: meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|d| u64::try_from(d.as_millis()).ok()),
        })
    }
}

/// Where the downscale of `source` at `width` and its sidecar live.
fn cache_paths(cache_dir: &Path, source: &Path, width: u32) -> (PathBuf, PathBuf) {
    let stem = source.file_stem().map_or_else(
        || "texture".to_owned(),
        |s| s.to_string_lossy().into_owned(),
    );
    let dir = cache_dir.join(CACHE_SUBDIR);
    (
        dir.join(format!("{stem}.{width}.png")),
        dir.join(format!("{stem}.{width}.toml")),
    )
}

/// Whether the sidecar at `meta_path` says the cached file was made from a
/// source in exactly this state.
fn stamp_matches(meta_path: &Path, current: SourceStamp) -> bool {
    let Ok(contents) = fs::read_to_string(meta_path) else {
        return false;
    };
    match toml::from_str::<SourceStamp>(&contents) {
        Ok(recorded) => recorded == current,
        Err(e) => {
            warn!(path = %meta_path.display(), error = %e, "unreadable texture cache sidecar");
            false
        }
    }
}

/// How many halvings take `width` down to at most `target`.
///
/// Zero when the source is already small enough, which is what makes the
/// setting a cap rather than a resize: a source narrower than the chosen width
/// is loaded as it is.
fn halvings_to(width: u32, target: u32) -> u32 {
    let mut steps = 0;
    let mut w = width;
    while w > target && w >= 2 {
        w /= 2;
        steps += 1;
    }
    steps
}

/// Load `source` at a width of at most `target_width`.
///
/// At or above the source's own width this is [`texture_loader::load`] and
/// nothing is cached. Below it, a valid cached downscale is read if there is
/// one; otherwise the source is decoded, halved, and written to the cache
/// before being returned.
///
/// `cache_dir` is the app's data directory (the one the cloud cache uses).
/// `None` disables caching entirely: the source is decoded and halved on every
/// run, which is what a machine with no writable data directory gets.
pub fn load_at_resolution(
    source: &Path,
    target_width: u32,
    cache_dir: Option<&Path>,
) -> Result<DecodedImage, String> {
    let Some(cache_dir) = cache_dir else {
        let (mut img, _) = load_and_halve(source, target_width)?;
        texture_loader::orient(&mut img);
        return Ok(img);
    };

    let (image_path, meta_path) = cache_paths(cache_dir, source, target_width);
    if let Some(stamp) = SourceStamp::of(source)
        && stamp_matches(&meta_path, stamp)
    {
        match texture_loader::load(&image_path) {
            Ok(img) if img.width <= target_width => {
                info!(
                    path = %image_path.display(),
                    width = img.width,
                    height = img.height,
                    "loaded a cached texture downscale"
                );
                return Ok(img);
            }
            Ok(img) => warn!(
                path = %image_path.display(),
                width = img.width,
                "cached downscale is wider than it claims to be, rebuilding"
            ),
            Err(e) => {
                debug!(path = %image_path.display(), error = %e, "no usable cached downscale");
            }
        }
    }

    let (mut decoded, halved) = load_and_halve(source, target_width)?;
    // Written before the orientation fixes, so what lands on disk is a plain
    // downscale of the source. Caching a file that was not halved would be a
    // second copy of the source in a different container, which costs disk and
    // saves only the difference between the two decoders.
    if halved {
        write_cache(&image_path, &meta_path, source, &decoded);
    }
    texture_loader::orient(&mut decoded);
    Ok(decoded)
}

/// Decode `source` and halve it until it is at most `target_width` wide.
///
/// Returns whether anything was halved, which is what decides if the result is
/// worth caching. The halving runs before the orientation fixes, so the pixels
/// it produces are what a cached copy holds.
fn load_and_halve(source: &Path, target_width: u32) -> Result<(DecodedImage, bool), String> {
    let start = std::time::Instant::now();
    let mut img = texture_loader::decode(source)?;
    let steps = halvings_to(img.width, target_width);
    info!(
        path = %source.display(),
        width = img.width,
        height = img.height,
        halvings = steps,
        elapsed_secs = format_args!("{:.2}", start.elapsed().as_secs_f64()),
        "decoded a texture source"
    );

    for _ in 0..steps {
        img.pixels = texture_loader::downsample_2x(&img.pixels, img.width, img.height);
        img.width = (img.width / 2).max(1);
        img.height = (img.height / 2).max(1);
    }
    Ok((img, steps > 0))
}

/// Write the downscale and its sidecar, temp-then-rename so a crash or a second
/// process never leaves a half-written PNG that looks valid.
///
/// Every failure here is a warning and nothing more: the cache is an
/// optimization, and a run that cannot write it still has its pixels.
fn write_cache(image_path: &Path, meta_path: &Path, source: &Path, img: &DecodedImage) {
    let Some(stamp) = SourceStamp::of(source) else {
        warn!(path = %source.display(), "no source metadata, not caching the downscale");
        return;
    };

    if let Some(parent) = image_path.parent()
        && let Err(e) = fs::create_dir_all(parent)
    {
        warn!(path = %parent.display(), error = %e, "could not create the texture cache directory");
        return;
    }

    let tmp_image = with_tilde(image_path);
    if let Err(e) = save_png(&tmp_image, img) {
        warn!(path = %tmp_image.display(), error = %e, "could not write the texture downscale");
        let _ = fs::remove_file(&tmp_image);
        return;
    }

    // The sidecar goes first and is removed on failure, so the only ordering a
    // reader can observe is "no sidecar" (rebuild) or "both files".
    let tmp_meta = with_tilde(meta_path);
    let toml_str = match toml::to_string_pretty(&stamp) {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "could not serialize the texture cache sidecar");
            let _ = fs::remove_file(&tmp_image);
            return;
        }
    };
    if let Err(e) = fs::write(&tmp_meta, &toml_str) {
        warn!(path = %tmp_meta.display(), error = %e, "could not write the texture cache sidecar");
        let _ = fs::remove_file(&tmp_image);
        return;
    }

    if let Err(e) = fs::rename(&tmp_image, image_path) {
        warn!(path = %image_path.display(), error = %e, "could not put the downscale in place");
        let _ = fs::remove_file(&tmp_image);
        let _ = fs::remove_file(&tmp_meta);
        return;
    }
    if let Err(e) = fs::rename(&tmp_meta, meta_path) {
        warn!(path = %meta_path.display(), error = %e, "could not put the sidecar in place");
        let _ = fs::remove_file(&tmp_meta);
        // The PNG without a sidecar is invalid, which is the safe direction.
        return;
    }
    info!(
        path = %image_path.display(),
        width = img.width,
        height = img.height,
        "cached a texture downscale"
    );
}

/// The same path with a `~` appended, following the config save's convention
/// for a file that is not finished yet.
fn with_tilde(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push("~");
    PathBuf::from(name)
}

/// Encode `img` as a PNG at `path`.
///
/// The encoder is named rather than inferred from the file extension, because
/// the file this writes to is the unfinished one and its extension is `png~`.
/// Fast compression rather than the default: the file is a cache entry whose
/// whole point is to be cheaper than decoding the source again, and the encode
/// happens on the loader thread while the app is waiting for its first frame.
fn save_png(path: &Path, img: &DecodedImage) -> Result<(), String> {
    use image::ImageEncoder;
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};

    let file = fs::File::create(path).map_err(|e| e.to_string())?;
    PngEncoder::new_with_quality(
        std::io::BufWriter::new(file),
        CompressionType::Fast,
        FilterType::Adaptive,
    )
    .write_image(
        &img.pixels,
        img.width,
        img.height,
        image::ExtendedColorType::Rgba8,
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A distinguishable gradient, so a downscale is not confusable with the
    /// source and a replacement is not confusable with either.
    fn write_source(path: &Path, width: u32, height: u32, seed: u8) {
        let mut img = image::RgbaImage::new(width, height);
        for (x, y, px) in img.enumerate_pixels_mut() {
            #[allow(clippy::cast_possible_truncation)]
            let v = ((x * 7 + y * 13) % 256) as u8;
            *px = image::Rgba([v, seed, 255 - v, 255]);
        }
        img.save(path).expect("write the fixture source");
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sunlit_earth_texture_cache_{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create the test directory");
        dir
    }

    // --- halvings_to ---

    #[test]
    fn a_source_at_the_target_width_is_not_halved() {
        assert_eq!(halvings_to(8192, 8192), 0);
    }

    #[test]
    fn each_step_halves_until_the_target_is_reached() {
        assert_eq!(halvings_to(8192, 4096), 1);
        assert_eq!(halvings_to(8192, 2048), 2);
        assert_eq!(halvings_to(4096, 2048), 1);
    }

    /// The setting is a cap, so asking for more than the file holds is not an
    /// upscale.
    #[test]
    fn a_source_narrower_than_the_target_is_left_alone() {
        assert_eq!(halvings_to(2048, 8192), 0);
        assert_eq!(halvings_to(1, 8192), 0);
    }

    #[test]
    fn halving_a_target_of_zero_terminates_at_one_pixel() {
        assert_eq!(halvings_to(8, 0), 3);
    }

    // --- the cache key ---

    #[test]
    fn cache_paths_name_the_source_and_the_width() {
        let (image, meta) = cache_paths(
            Path::new("C:/data/SunlitEarth"),
            Path::new("C:/textures/world.topo.200405.jxl"),
            2048,
        );
        assert!(image.ends_with("texture_cache/world.topo.200405.2048.png"));
        assert!(meta.ends_with("texture_cache/world.topo.200405.2048.toml"));
    }

    #[test]
    fn each_width_gets_its_own_cache_file() {
        let dir = Path::new("C:/data");
        let source = Path::new("C:/textures/day.jxl");
        assert_ne!(
            cache_paths(dir, source, 4096).0,
            cache_paths(dir, source, 2048).0
        );
    }

    #[test]
    fn a_stamp_matches_only_itself() {
        let dir = temp_dir("stamp");
        let meta = dir.join("stamp.toml");
        let recorded = SourceStamp {
            bytes: 1234,
            modified_ms: Some(9_999),
        };
        fs::write(&meta, toml::to_string_pretty(&recorded).unwrap()).unwrap();

        assert!(stamp_matches(&meta, recorded));
        assert!(!stamp_matches(
            &meta,
            SourceStamp {
                bytes: 1235,
                ..recorded
            }
        ));
        assert!(!stamp_matches(
            &meta,
            SourceStamp {
                modified_ms: Some(10_000),
                ..recorded
            }
        ));
        assert!(!stamp_matches(
            &meta,
            SourceStamp {
                modified_ms: None,
                ..recorded
            }
        ));
    }

    #[test]
    fn a_missing_or_corrupt_sidecar_is_a_miss() {
        let dir = temp_dir("sidecar");
        let stamp = SourceStamp {
            bytes: 1,
            modified_ms: None,
        };
        assert!(!stamp_matches(&dir.join("absent.toml"), stamp));

        let corrupt = dir.join("corrupt.toml");
        fs::write(&corrupt, "{{{not toml").unwrap();
        assert!(!stamp_matches(&corrupt, stamp));
    }

    // --- the load path ---

    #[test]
    fn a_first_load_halves_the_source_and_writes_the_cache() {
        let dir = temp_dir("first_load");
        let source = dir.join("day.png");
        write_source(&source, 32, 16, 10);

        let img = load_at_resolution(&source, 8, Some(&dir)).expect("load");
        assert_eq!((img.width, img.height), (8, 4));
        assert_eq!(img.pixels.len(), 8 * 4 * 4);

        let (image_path, meta_path) = cache_paths(&dir, &source, 8);
        assert!(image_path.exists(), "the downscale should be cached");
        assert!(meta_path.exists(), "the sidecar should be beside it");
        assert!(
            !with_tilde(&image_path).exists(),
            "the temporary file should be gone"
        );
    }

    /// Proof that the second load reads the cache rather than the source: the
    /// cached file is replaced with different pixels of the same size, and the
    /// sidecar is left alone.
    #[test]
    fn a_second_load_reads_the_cached_file() {
        let dir = temp_dir("cache_hit");
        let source = dir.join("day.png");
        write_source(&source, 32, 16, 10);

        let first = load_at_resolution(&source, 8, Some(&dir)).expect("first load");
        let (image_path, _) = cache_paths(&dir, &source, 8);
        write_source(&image_path, 8, 4, 200);

        let second = load_at_resolution(&source, 8, Some(&dir)).expect("second load");
        assert_eq!((second.width, second.height), (8, 4));
        assert_ne!(
            second.pixels, first.pixels,
            "the second load must come from the cached file"
        );
    }

    /// The cache is keyed on the source, so replacing the asset must not leave
    /// the old downscale in use.
    #[test]
    fn a_changed_source_invalidates_the_cache() {
        let dir = temp_dir("invalidate");
        let source = dir.join("day.png");
        write_source(&source, 32, 16, 10);
        load_at_resolution(&source, 8, Some(&dir)).expect("first load");

        // A wider source is a different file by size as well as by content.
        write_source(&source, 64, 32, 20);
        let after = load_at_resolution(&source, 8, Some(&dir)).expect("load after replacement");
        assert_eq!(
            (after.width, after.height),
            (8, 4),
            "the new source is halved further to reach the same width"
        );

        let expected = load_at_resolution(&source, 8, None).expect("uncached load");
        assert_eq!(
            after.pixels, expected.pixels,
            "the replaced source must be what was loaded"
        );
    }

    #[test]
    fn a_target_at_the_source_width_caches_nothing() {
        let dir = temp_dir("no_downscale");
        let source = dir.join("day.png");
        write_source(&source, 32, 16, 10);

        let img = load_at_resolution(&source, 32, Some(&dir)).expect("load");
        assert_eq!((img.width, img.height), (32, 16));
        assert!(
            !cache_paths(&dir, &source, 32).0.exists(),
            "there is nothing to cache when nothing was halved"
        );
    }

    #[test]
    fn without_a_cache_directory_the_source_is_still_halved() {
        let dir = temp_dir("no_cache_dir");
        let source = dir.join("day.png");
        write_source(&source, 32, 16, 10);

        let img = load_at_resolution(&source, 8, None).expect("load");
        assert_eq!((img.width, img.height), (8, 4));
    }

    /// A cached downscale holds the source's own orientation, so loading one
    /// has to produce the same pixels as loading the source directly.
    #[test]
    fn the_cached_path_and_the_direct_path_agree() {
        let dir = temp_dir("orientation");
        let source = dir.join("day.png");
        write_source(&source, 32, 16, 10);

        let uncached = load_at_resolution(&source, 8, None).expect("uncached");
        let written = load_at_resolution(&source, 8, Some(&dir)).expect("cache miss");
        let read_back = load_at_resolution(&source, 8, Some(&dir)).expect("cache hit");

        assert_eq!(uncached.pixels, written.pixels);
        assert_eq!(uncached.pixels, read_back.pixels);
    }

    #[test]
    fn a_missing_source_is_an_error_rather_than_a_panic() {
        let dir = temp_dir("missing");
        let Err(err) = load_at_resolution(&dir.join("absent.png"), 8, Some(&dir)) else {
            panic!("a source that is not there must not load");
        };
        assert!(err.contains("absent.png"), "unexpected error: {err}");
    }
}
