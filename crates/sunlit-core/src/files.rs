//! Writing a file without leaving a half-written one behind.
//!
//! The file a reader can come back to is written under a temporary name and
//! then put in place: the config, the cloud cache sidecar, a cached texture
//! downscale and a wallpaper frame. What differs between them is the format and
//! the temporary suffix. The rest is here, so a process killed mid-write leaves
//! the previous file rather than a truncated one, and so there is one answer to
//! what a temporary name looks like.
//!
//! An exported render is the exception and writes in place: `engine::save_png`
//! is the end of a `render --output` or a `displays --out`, nothing in this
//! program reads the result back, and a half-written file there is one the user
//! asked for and can ask for again.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) use image::codecs::png::{CompressionType, FilterType};
use tracing::warn;

/// A name for the not-yet-finished version of `path`, unique to this writer.
///
/// The process id and the counter are what a fixed temporary name does not
/// carry and a concurrent writer needs: two runs of the app, or two threads in
/// one run, can be building the same file at the same time, and a shared
/// temporary name means the second `File::create` truncates the first writer's
/// file mid-write.
///
/// `suffix` is the caller's, because a sweep that removes the leftovers looks
/// for it: the texture cache uses `~`, the wallpaper directory `.tmp`.
pub(crate) fn unfinished(path: &Path, suffix: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nonce = NEXT.fetch_add(1, Ordering::Relaxed);
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{}.{nonce}{suffix}", std::process::id()));
    PathBuf::from(name)
}

/// Encode RGBA8 `pixels` as a PNG at `path`.
///
/// The encoder is named rather than inferred from the file extension, because
/// two of the three callers write to a temporary name whose extension is not
/// `png`. The settings stay with the caller rather than being decided here:
/// how much time is worth spending on the size of a file depends on how often
/// it is rewritten and who is waiting for it. They agree on compression today
/// and differ on the filter.
///
/// The buffer check comes before the filesystem does, so a mismatched buffer
/// creates nothing.
pub(crate) fn write_png(
    path: &Path,
    pixels: &[u8],
    width: u32,
    height: u32,
    compression: CompressionType,
    filter: FilterType,
) -> Result<(), String> {
    use image::ImageEncoder;
    use image::codecs::png::PngEncoder;

    let expected = usize::try_from(u64::from(width) * u64::from(height) * 4);
    if expected != Ok(pixels.len()) {
        return Err(format!(
            "pixel buffer size mismatch: {} bytes for {width}x{height}",
            pixels.len()
        ));
    }

    let file = std::fs::File::create(path).map_err(|e| format!("could not create the PNG: {e}"))?;
    PngEncoder::new_with_quality(std::io::BufWriter::new(file), compression, filter)
        .write_image(pixels, width, height, image::ExtendedColorType::Rgba8)
        .map_err(|e| format!("could not encode the PNG: {e}"))
}

/// Serialize `value` as TOML and put it in place of `path`.
///
/// Warns rather than fails: neither caller has anything useful to do about a
/// settings file it could not write, and losing the write costs the next run
/// the previous contents rather than anything it cannot rebuild. `what` names
/// the file in those warnings.
pub(crate) fn write_toml<T: serde::Serialize>(value: &T, path: &Path, what: &str) {
    let toml_str = match toml::to_string_pretty(value) {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "could not serialize the {what}");
            return;
        }
    };

    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        warn!(path = %parent.display(), error = %e, "could not create the directory for the {what}");
        return;
    }

    let tmp_path = path.with_extension("toml~");
    if let Err(e) = std::fs::write(&tmp_path, &toml_str) {
        warn!(path = %tmp_path.display(), error = %e, "could not write the {what}");
        return;
    }

    if let Err(e) = std::fs::rename(&tmp_path, path) {
        warn!(path = %path.display(), error = %e, "could not put the {what} in place");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ScratchDir;

    /// `texture_cache` owns the case for the shape of the name; this is the
    /// half of it the suffix parameter added.
    #[test]
    fn a_suffix_is_the_end_of_the_name_the_sweep_looks_for() {
        let name = unfinished(Path::new("/wallpapers/screen-0.png"), ".tmp");
        assert!(name.to_string_lossy().ends_with(".tmp"));
    }

    #[test]
    fn a_toml_write_leaves_no_temporary_file_behind() {
        let dir = ScratchDir::new("files_toml");
        let path = dir.path().join("thing.toml");
        let value = std::collections::BTreeMap::from([("count".to_owned(), 3)]);
        write_toml(&value, &path, "thing");

        assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), "count = 3");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name())
            .filter(|name| name.to_string_lossy().ends_with('~'))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }
}
