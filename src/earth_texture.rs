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
/// The image is flipped horizontally after decoding because standard
/// equirectangular maps have east-to-the-right, but our sphere UV winding
/// goes in the opposite direction.
pub fn load(path: &Path) -> Result<DecodedImage, String> {
    let img = image::open(path)
        .map_err(|e| format!("Failed to load {}: {e}", path.display()))?
        .fliph()
        .into_rgba8();

    let (width, height) = img.dimensions();
    Ok(DecodedImage {
        pixels: img.into_raw(),
        width,
        height,
    })
}

/// Resolve the textures directory using the fallback chain:
/// 1. `--textures-dir` CLI flag
/// 2. `SUNLIT_EARTH_TEXTURES` environment variable
/// 3. `textures/` relative to the executable
/// 4. `textures/` relative to the current working directory
pub fn resolve_textures_dir(cli_override: Option<&Path>) -> Option<PathBuf> {
    let candidates: Vec<PathBuf> = [
        cli_override.map(Path::to_path_buf),
        std::env::var_os("SUNLIT_EARTH_TEXTURES").map(PathBuf::from),
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("textures"))),
        Some(PathBuf::from("textures")),
    ]
    .into_iter()
    .flatten()
    .collect();

    candidates.into_iter().find(|p| p.is_dir())
}
