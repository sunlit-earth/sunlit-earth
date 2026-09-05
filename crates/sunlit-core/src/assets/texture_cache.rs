//! Downscaled copies of the local surface textures, kept on disk.
//!
//! The two surface assets are 8192 wide JPEG XL files. Loading one at 4096 or
//! 2048 means decoding the 8K source and halving it, which is the most
//! expensive thing the app does at startup, so the result is written back as a
//! PNG next to the cloud cache and every later run decodes that instead. PNG
//! because the `image` crate encodes it losslessly and decodes it in a fraction
//! of the time JPEG XL takes; the file is disposable either way.

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

    // Stamped before the decode rather than after it, so a source replaced
    // during it is not recorded as where the old pixels came from.
    let before = SourceStamp::of(source);
    let (mut decoded, halved) = load_and_halve(source, target_width)?;
    // Written before the orientation fixes, so what lands on disk is a plain
    // downscale of the source. Caching a file that was not halved would be a
    // second copy of the source in a different container, which costs disk and
    // saves only the difference between the two decoders.
    if halved {
        write_cache(&image_path, &meta_path, before, source, &decoded);
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

/// Write the downscale and its sidecar, temp-then-rename so that a crash, or
/// another writer of the same entry, never leaves a half-written PNG that looks
/// valid. The temporary names carry the process and a counter, so two writers
/// have their own and neither truncates the other's.
///
/// `stamped_before` is what the source looked like when the decode that produced
/// `img` started. It has to still look that way, or these pixels are not what
/// the sidecar would be claiming they came from.
///
/// Every failure here is a warning and nothing more: the cache is an
/// optimization, and a run that cannot write it still has its pixels.
fn write_cache(
    image_path: &Path,
    meta_path: &Path,
    stamped_before: Option<SourceStamp>,
    source: &Path,
    img: &DecodedImage,
) {
    let (Some(before), Some(after)) = (stamped_before, SourceStamp::of(source)) else {
        warn!(path = %source.display(), "no source metadata, not caching the downscale");
        return;
    };
    if before != after {
        warn!(
            path = %source.display(),
            "the source changed while it was being decoded, not caching the downscale"
        );
        return;
    }
    let stamp = after;

    if let Some(parent) = image_path.parent()
        && let Err(e) = fs::create_dir_all(parent)
    {
        warn!(path = %parent.display(), error = %e, "could not create the texture cache directory");
        return;
    }

    let tmp_image = unfinished(image_path);
    if let Err(e) = save_png(&tmp_image, img) {
        warn!(path = %tmp_image.display(), error = %e, "could not write the texture downscale");
        let _ = fs::remove_file(&tmp_image);
        return;
    }

    // Both temporary files are written first, then the PNG is put in place and
    // the sidecar last. The only orderings a reader can observe are therefore
    // "no sidecar", which it rebuilds, and "both", which it trusts.
    let tmp_meta = unfinished(meta_path);
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
    sweep_unfinished(image_path);
    sweep_unfinished(meta_path);
}

/// Delete unfinished versions of `target` that some earlier writer left behind.
///
/// A unique temporary name per writer is what stops two of them truncating each
/// other, and the cost of it is that nothing reuses the name: a process killed
/// mid-write leaves a file that would otherwise sit in the cache directory
/// forever, several megabytes at a time. Sweeping after a successful write bounds
/// that to whatever accumulates between two writes of the same entry.
///
/// A writer of this same entry that is still working loses its temporary file
/// here, and its own rename then fails with a warning; the entry it was building
/// is the one that just landed, so the next run reads that rather than rebuilding
/// anything. Orphans that never go away is the worse of the two.
fn sweep_unfinished(target: &Path) {
    let (Some(dir), Some(name)) = (target.parent(), target.file_name()) else {
        return;
    };
    let prefix = format!("{}.", name.to_string_lossy());
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let found = entry.file_name().to_string_lossy().into_owned();
        if found.starts_with(&prefix) && found.ends_with('~') {
            let path = entry.path();
            match fs::remove_file(&path) {
                Ok(()) => debug!(path = %path.display(), "removed an unfinished cache file"),
                Err(e) => {
                    debug!(path = %path.display(), error = %e, "could not remove an unfinished cache file");
                }
            }
        }
    }
}

/// A name for the not-yet-finished version of `path`, unique to this writer.
///
/// The `~` suffix is the config save's convention for a file that is not
/// finished yet. The process id and counter are what the config save does not
/// need and this does: two runs of the app, or two loader threads in one run
/// switching resolutions back and forth, can be building the same cache entry
/// at the same time, and a shared temporary name means the second `File::create`
/// truncates the first writer's PNG mid-write.
fn unfinished(path: &Path) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nonce = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{}.{nonce}~", std::process::id()));
    PathBuf::from(name)
}

/// Encode `img` as a PNG at `path`.
///
/// The encoder is named rather than inferred from the file extension, because
/// the file this writes to is the unfinished one and its extension is not
/// `png`. Fast compression rather than the default: the file is a cache entry whose
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
    use crate::test_support::ScratchDir;

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

    fn temp_dir(name: &str) -> ScratchDir {
        ScratchDir::new(&format!("texture_cache_{name}"))
    }

    // --- halvings_to ---

    /// The setting is a cap, so a source at or under the target is left alone
    /// and one above it is halved until it fits. A target of zero has no width
    /// to reach, so the halving stops at one pixel instead of looping.
    #[test]
    fn a_source_is_halved_until_it_fits_the_target() {
        for (source, target, halvings) in [
            (8192, 8192, 0),
            (8192, 4096, 1),
            (8192, 2048, 2),
            (4096, 2048, 1),
            (2048, 8192, 0),
            (1, 8192, 0),
            (8, 0, 3),
        ] {
            assert_eq!(
                halvings_to(source, target),
                halvings,
                "{source} down to {target}"
            );
        }
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

        let img = load_at_resolution(&source, 8, Some(dir.path())).expect("load");
        assert_eq!((img.width, img.height), (8, 4));
        assert_eq!(img.pixels.len(), 8 * 4 * 4);

        let (image_path, meta_path) = cache_paths(dir.path(), &source, 8);
        assert!(image_path.exists(), "the downscale should be cached");
        assert!(meta_path.exists(), "the sidecar should be beside it");

        let leftovers: Vec<_> = fs::read_dir(dir.join(CACHE_SUBDIR))
            .expect("the cache directory exists")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with('~'))
            .collect();
        assert!(
            leftovers.is_empty(),
            "no unfinished file should be left behind: {leftovers:?}"
        );
    }

    /// The next successful write of an entry clears the temporary files an
    /// earlier writer of it was killed holding.
    #[test]
    fn a_write_sweeps_unfinished_files_an_earlier_one_left() {
        let dir = temp_dir("sweep");
        let source = dir.join("day.png");
        write_source(&source, 32, 16, 10);
        let (image_path, meta_path) = cache_paths(dir.path(), &source, 8);

        fs::create_dir_all(image_path.parent().expect("a parent")).expect("cache directory");
        let orphans = [
            unfinished(&image_path),
            unfinished(&image_path),
            unfinished(&meta_path),
        ];
        for orphan in &orphans {
            fs::write(orphan, b"half a png").expect("write an orphan");
        }
        // Something that is not ours, to prove the sweep is not a wildcard.
        let bystander = dir.join(CACHE_SUBDIR).join("someone.else.2048.png");
        fs::write(&bystander, b"not ours").expect("write the bystander");
        // An orphan of the same source at another width: unfinished, but not
        // this target's, so only the prefix check keeps it alive.
        let other_width = unfinished(&cache_paths(dir.path(), &source, 4).0);
        fs::write(&other_width, b"half a png").expect("write the other width");

        load_at_resolution(&source, 8, Some(dir.path())).expect("load");

        for orphan in &orphans {
            assert!(!orphan.exists(), "{} should be swept", orphan.display());
        }
        assert!(image_path.exists() && meta_path.exists());
        assert!(bystander.exists(), "only unfinished files may be swept");
        assert!(
            other_width.exists(),
            "another width's orphan is not this write's to sweep"
        );
    }

    /// Two writers of the same entry must not share a temporary name, or the
    /// second `File::create` truncates the first one's PNG mid-write.
    #[test]
    fn each_unfinished_name_is_the_writers_own() {
        let target = Path::new("C:/data/texture_cache/day.2048.png");
        let first = unfinished(target);
        let second = unfinished(target);

        assert_ne!(first, second);
        for name in [&first, &second] {
            let name = name.to_string_lossy();
            assert!(name.starts_with(&*target.to_string_lossy()));
            assert!(name.ends_with('~'), "{name}");
            assert!(name.contains(&std::process::id().to_string()), "{name}");
        }
    }

    /// Proof that the second load reads the cache rather than the source: the
    /// cached file is replaced with different pixels of the same size, and the
    /// sidecar is left alone.
    #[test]
    fn a_second_load_reads_the_cached_file() {
        let dir = temp_dir("cache_hit");
        let source = dir.join("day.png");
        write_source(&source, 32, 16, 10);

        let first = load_at_resolution(&source, 8, Some(dir.path())).expect("first load");
        let (image_path, _) = cache_paths(dir.path(), &source, 8);
        write_source(&image_path, 8, 4, 200);

        let second = load_at_resolution(&source, 8, Some(dir.path())).expect("second load");
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
        load_at_resolution(&source, 8, Some(dir.path())).expect("first load");

        // A wider source is a different file by size as well as by content.
        write_source(&source, 64, 32, 20);
        let after =
            load_at_resolution(&source, 8, Some(dir.path())).expect("load after replacement");
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

        let img = load_at_resolution(&source, 32, Some(dir.path())).expect("load");
        assert_eq!((img.width, img.height), (32, 16));
        assert!(
            !cache_paths(dir.path(), &source, 32).0.exists(),
            "there is nothing to cache when nothing was halved"
        );
    }

    /// A cached downscale holds the source's own orientation, so loading one
    /// has to produce the same pixels as loading the source directly.
    #[test]
    fn the_cached_path_and_the_direct_path_agree() {
        let dir = temp_dir("orientation");
        let source = dir.join("day.png");
        write_source(&source, 32, 16, 10);

        let uncached = load_at_resolution(&source, 8, None).expect("uncached");
        let written = load_at_resolution(&source, 8, Some(dir.path())).expect("cache miss");
        let read_back = load_at_resolution(&source, 8, Some(dir.path())).expect("cache hit");

        assert_eq!(uncached.pixels, written.pixels);
        assert_eq!(uncached.pixels, read_back.pixels);
    }

    /// A source replaced while it was being decoded must not be recorded as
    /// where the old pixels came from.
    #[test]
    fn a_source_that_changed_during_the_decode_is_not_cached() {
        let dir = temp_dir("changed_mid_decode");
        let source = dir.join("day.png");
        write_source(&source, 32, 16, 10);

        // What a decode that started before the replacement would have carried.
        let stale_stamp = Some(SourceStamp {
            bytes: 1,
            modified_ms: Some(0),
        });
        let img = load_at_resolution(&source, 8, None).expect("load");
        let (image_path, meta_path) = cache_paths(dir.path(), &source, 8);

        write_cache(&image_path, &meta_path, stale_stamp, &source, &img);
        assert!(
            !image_path.exists() && !meta_path.exists(),
            "pixels from a source that has since changed must not be cached"
        );

        // The same call with the stamp the file actually has does write it, so
        // the check above is the reason nothing was written.
        let live_stamp = SourceStamp::of(&source);
        write_cache(&image_path, &meta_path, live_stamp, &source, &img);
        assert!(image_path.exists() && meta_path.exists());

        // The other arm of the same guard: a source that vanished has no
        // metadata to stamp, so nothing may be cached either.
        fs::remove_file(&image_path).expect("clear the cached image");
        fs::remove_file(&meta_path).expect("clear the sidecar");
        fs::remove_file(&source).expect("remove the source");
        write_cache(&image_path, &meta_path, live_stamp, &source, &img);
        assert!(
            !image_path.exists() && !meta_path.exists(),
            "a source with no metadata cannot be stamped"
        );
    }

    #[test]
    fn a_missing_source_is_an_error_rather_than_a_panic() {
        let dir = temp_dir("missing");
        let Err(err) = load_at_resolution(&dir.join("absent.png"), 8, Some(dir.path())) else {
            panic!("a source that is not there must not load");
        };
        assert!(err.contains("absent.png"), "unexpected error: {err}");
    }
}
