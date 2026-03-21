//! Load equirectangular texture images from disk.

use std::path::{Path, PathBuf};

/// Register the JPEG-XL decoding hook with the `image` crate.
///
/// This must be called before any `image::open()` call that might encounter a
/// JXL file. The call is idempotent and safe to invoke multiple times.
pub fn register_jxl_hook() {
    jxl_oxide::integration::register_image_decoding_hook();
}

/// Decoded RGBA8 image data.
pub struct DecodedImage {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Load and decode an image file to RGBA8 pixel data.
///
/// The format is auto-detected by the `image` crate (including JXL when the
/// decoding hook has been registered via [`register_jxl_hook`]).
///
/// Two transformations are applied after decoding:
/// - Horizontal flip: standard equirectangular maps have east-to-the-right,
///   but our sphere UV winding goes in the opposite direction.
/// - Horizontal shift left by 1/4 width: aligns the prime meridian with u=0
///   in our sphere's UV mapping.
pub fn load(path: &Path) -> Result<DecodedImage, String> {
    let mut reader = image::ImageReader::open(path)
        .map_err(|e| format!("Failed to open {}: {e}", path.display()))?;
    reader.no_limits();
    let img = reader
        .decode()
        .map_err(|e| format!("Failed to decode {}: {e}", path.display()))?
        .fliph()
        .into_rgba8();

    let (width, height) = img.dimensions();
    let mut pixels = img.into_raw();
    shift_horizontal(&mut pixels, width, height);

    Ok(DecodedImage {
        pixels,
        width,
        height,
    })
}

/// Shift all rows left by 1/4 width (wrapping), aligning the prime meridian
/// with the sphere's u=0.
pub(crate) fn shift_horizontal(pixels: &mut [u8], width: u32, height: u32) {
    let w = width as usize;
    let row_bytes = w * 4;
    let shift_bytes = w * 3; // 3/4 width in bytes (each pixel is 4 bytes)

    for y in 0..height as usize {
        let row = &mut pixels[y * row_bytes..(y + 1) * row_bytes];
        row.rotate_right(shift_bytes);
    }
}

/// Resolve the textures directory using the fallback chain:
/// 1. `--textures-dir` CLI flag
/// 2. `SUNLIT_EARTH_TEXTURES` environment variable
/// 3. `textures/` relative to the current working directory
/// 4. `textures/` relative to the executable, walking up ancestor directories
///    (finds the project root from `target/debug/` or `target/release/`)
pub fn resolve_textures_dir(cli_override: Option<&Path>) -> Option<PathBuf> {
    let explicit: [Option<PathBuf>; 3] = [
        cli_override.map(Path::to_path_buf),
        std::env::var_os("SUNLIT_EARTH_TEXTURES").map(PathBuf::from),
        Some(PathBuf::from("textures")),
    ];
    if let Some(dir) = explicit.into_iter().flatten().find(|p| p.is_dir()) {
        return Some(dir);
    }

    // Walk up from the executable's directory to find a `textures/` folder.
    let mut dir = std::env::current_exe().ok()?;
    while dir.pop() {
        let candidate = dir.join("textures");
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a pixel buffer from an array of RGBA tuples.
    fn pixels_from(px: &[[u8; 4]]) -> Vec<u8> {
        px.iter().flat_map(|p| p.iter().copied()).collect()
    }

    /// Helper: extract pixel at position `i` from an RGBA buffer.
    fn pixel_at(buf: &[u8], i: usize) -> [u8; 4] {
        let s = i * 4;
        [buf[s], buf[s + 1], buf[s + 2], buf[s + 3]]
    }

    #[test]
    fn shift_4px_single_row() {
        // 4 pixels: [A, B, C, D] -> rotate_right by 3 -> [B, C, D, A]
        let a = [10, 20, 30, 255];
        let b = [40, 50, 60, 255];
        let c = [70, 80, 90, 255];
        let d = [100, 110, 120, 255];
        let mut buf = pixels_from(&[a, b, c, d]);
        shift_horizontal(&mut buf, 4, 1);
        assert_eq!(pixel_at(&buf, 0), b);
        assert_eq!(pixel_at(&buf, 1), c);
        assert_eq!(pixel_at(&buf, 2), d);
        assert_eq!(pixel_at(&buf, 3), a);
    }

    #[test]
    fn shift_4px_two_rows_independent() {
        let row1 = [[1, 2, 3, 4], [5, 6, 7, 8], [9, 10, 11, 12], [13, 14, 15, 16]];
        let row2 = [[17, 18, 19, 20], [21, 22, 23, 24], [25, 26, 27, 28], [29, 30, 31, 32]];
        let mut buf = pixels_from(&[row1.as_slice(), row2.as_slice()].concat());
        shift_horizontal(&mut buf, 4, 2);
        // Row 0: rotate_right by 3 -> [B, C, D, A]
        assert_eq!(pixel_at(&buf, 0), row1[1]);
        assert_eq!(pixel_at(&buf, 1), row1[2]);
        assert_eq!(pixel_at(&buf, 2), row1[3]);
        assert_eq!(pixel_at(&buf, 3), row1[0]);
        // Row 1: same rotation
        assert_eq!(pixel_at(&buf, 4), row2[1]);
        assert_eq!(pixel_at(&buf, 5), row2[2]);
        assert_eq!(pixel_at(&buf, 6), row2[3]);
        assert_eq!(pixel_at(&buf, 7), row2[0]);
    }

    #[test]
    fn shift_uniform_row_is_identity() {
        let mut buf = [128, 64, 32, 255].repeat(4);
        let original = buf.clone();
        shift_horizontal(&mut buf, 4, 1);
        assert_eq!(buf, original);
    }

    #[test]
    fn shift_8px_single_row() {
        // 8 pixels, shift by 6 (3/4 of 8)
        let px: Vec<[u8; 4]> = (0..8).map(|i| [i * 10, i * 10 + 1, i * 10 + 2, 255]).collect();
        let mut buf = pixels_from(&px);
        shift_horizontal(&mut buf, 8, 1);
        // rotate_right by 6 means first 2 pixels move to end
        // Result: [px[2], px[3], px[4], px[5], px[6], px[7], px[0], px[1]]
        assert_eq!(pixel_at(&buf, 0), px[2]);
        assert_eq!(pixel_at(&buf, 1), px[3]);
        assert_eq!(pixel_at(&buf, 6), px[0]);
        assert_eq!(pixel_at(&buf, 7), px[1]);
    }

    proptest::proptest! {
        #[test]
        fn shift_four_times_is_identity(
            width in 1u32..=64,
            height in 1u32..=16,
            pixels in proptest::collection::vec(proptest::num::u8::ANY, 1..=64*16*4),
        ) {
            // Truncate or skip if the random vec doesn't match the expected size
            let expected_len = (width as usize) * (height as usize) * 4;
            proptest::prop_assume!(pixels.len() >= expected_len);
            let mut buf = pixels[..expected_len].to_vec();
            let original = buf.clone();

            // Four shifts of 3/4 width = 3 full rotations = identity
            for _ in 0..4 {
                shift_horizontal(&mut buf, width, height);
            }
            proptest::prop_assert_eq!(buf, original);
        }
    }
}
