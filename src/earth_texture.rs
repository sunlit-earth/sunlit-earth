//! Load the Earth texture from a JPG file on disk.

use std::path::{Path, PathBuf};

/// Decoded RGBA8 image data.
pub struct DecodedImage {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Load and decode a JPG file to RGBA8 pixel data.
///
/// Two transformations are applied after decoding:
/// - Horizontal flip: standard equirectangular maps have east-to-the-right,
///   but our sphere UV winding goes in the opposite direction.
/// - Horizontal shift left by 1/4 width: aligns the prime meridian with u=0
///   in our sphere's UV mapping.
pub fn load(path: &Path) -> Result<DecodedImage, String> {
    let img = image::open(path)
        .map_err(|e| format!("Failed to load {}: {e}", path.display()))?
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
fn shift_horizontal(pixels: &mut [u8], width: u32, height: u32) {
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
/// 3. `textures/` relative to the executable
/// 4. `textures/` relative to the current working directory
pub fn resolve_textures_dir(cli_override: Option<&Path>) -> Option<PathBuf> {
    [
        cli_override.map(Path::to_path_buf),
        std::env::var_os("SUNLIT_EARTH_TEXTURES").map(PathBuf::from),
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("textures"))),
        Some(PathBuf::from("textures")),
    ]
    .into_iter()
    .flatten()
    .find(|p| p.is_dir())
}
