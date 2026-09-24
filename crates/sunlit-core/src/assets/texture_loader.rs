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

/// Load and decode an image file into pixels the sphere can sample.
///
/// The format comes from the file extension, not from the content, because
/// that is what `ImageReader::open` reads it from. JXL is one of them once the
/// decoding hook has been registered through [`register_jxl_hook`].
#[tracing::instrument(skip_all, fields(path = %path.display()))]
pub fn load(path: &Path) -> Result<DecodedImage, String> {
    let mut img = decode(path)?;
    orient(&mut img);
    Ok(img)
}

/// Decode an image file to RGBA8 exactly as the file stores it.
///
/// This is the half of [`load`] the texture cache needs: a cached downscale is
/// written in the source's own orientation, so that reading one back through
/// `load` is correct and a human opening the file sees the map the right way
/// round.
pub(crate) fn decode(path: &Path) -> Result<DecodedImage, String> {
    let mut reader = image::ImageReader::open(path)
        .map_err(|e| format!("Failed to open {}: {e}", path.display()))?;
    reader.no_limits();
    let img = reader
        .decode()
        .map_err(|e| format!("Failed to decode {}: {e}", path.display()))?
        .into_rgba8();

    let (width, height) = img.dimensions();
    Ok(DecodedImage {
        pixels: img.into_raw(),
        width,
        height,
    })
}

/// Turn a standard equirectangular map into the sphere's UV layout.
///
/// Two transformations, both in place:
/// - Horizontal flip: standard equirectangular maps have east-to-the-right,
///   but our sphere UV winding goes in the opposite direction.
/// - Horizontal shift left by 1/4 width: aligns the prime meridian with u=0
///   in our sphere's UV mapping.
pub(crate) fn orient(img: &mut DecodedImage) {
    flip_horizontal(&mut img.pixels, img.width, img.height);
    shift_horizontal(&mut img.pixels, img.width, img.height);
}

/// Mirror every row, so east ends up where the sphere's winding expects it.
fn flip_horizontal(pixels: &mut [u8], width: u32, height: u32) {
    let w = width as usize;
    let row_bytes = w * 4;
    for y in 0..height as usize {
        let row = &mut pixels[y * row_bytes..(y + 1) * row_bytes];
        for x in 0..w / 2 {
            let (left, right) = (x * 4, (w - 1 - x) * 4);
            for c in 0..4 {
                row.swap(left + c, right + c);
            }
        }
    }
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

/// Box-filter downsample: average each 2x2 block of RGBA pixels.
///
/// Used for both mip generation in the renderer and the on-disk downscales the
/// texture cache writes, which is why it lives with the pixel handling rather
/// than with either caller.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the mean of four bytes is a byte, and a pixel count indexes a buffer that already holds those pixels"
)]
pub(crate) fn downsample_2x(src: &[u8], src_w: u32, src_h: u32) -> Vec<u8> {
    let dst_w = (src_w / 2).max(1) as usize;
    let dst_h = (src_h / 2).max(1) as usize;
    let sw = src_w as usize;
    let sh = src_h as usize;
    let mut dst = vec![0u8; dst_w * dst_h * 4];

    for y in 0..dst_h {
        for x in 0..dst_w {
            let sx = x * 2;
            let sy = y * 2;
            let sx1 = (sx + 1).min(sw - 1);
            let sy1 = (sy + 1).min(sh - 1);
            for c in 0..4 {
                let tl = u16::from(src[(sy * sw + sx) * 4 + c]);
                let tr = u16::from(src[(sy * sw + sx1) * 4 + c]);
                let bl = u16::from(src[(sy1 * sw + sx) * 4 + c]);
                let br = u16::from(src[(sy1 * sw + sx1) * 4 + c]);
                dst[(y * dst_w + x) * 4 + c] = ((tl + tr + bl + br + 2) / 4) as u8;
            }
        }
    }

    dst
}

/// Resolve the textures directory using the fallback chain:
/// 1. `--textures-dir` CLI flag
/// 2. `SUNLIT_EARTH_TEXTURES` environment variable
/// 3. `textures/` relative to the current working directory
/// 4. [`textures_for_exe`], walking up from the executable
pub fn resolve_textures_dir(cli_override: Option<&Path>) -> Option<PathBuf> {
    let explicit: [Option<PathBuf>; 3] = [
        cli_override.map(Path::to_path_buf),
        std::env::var_os("SUNLIT_EARTH_TEXTURES").map(PathBuf::from),
        Some(PathBuf::from("textures")),
    ];
    if let Some(dir) = explicit.into_iter().flatten().find(|p| p.is_dir()) {
        return Some(dir);
    }

    textures_for_exe(&std::env::current_exe().ok()?)
}

/// [`textures_near`] the executable as it was started, then its resolved path.
///
/// A package manager puts a symlink on `PATH`, and `current_exe()` can report
/// the link rather than its target, from where the walk up never reaches the
/// install directory. The resolved path comes second because on Windows it
/// turns Scoop's `current` junction into a versioned `\\?\` path, while the
/// path as started already works there.
pub fn textures_for_exe(exe: &Path) -> Option<PathBuf> {
    textures_near(exe).or_else(|| textures_near(&std::fs::canonicalize(exe).ok()?))
}

/// The textures directory an executable at this path can reach, or `None`.
///
/// Two candidates per ancestor: `textures/`, which every bundle but the macOS
/// one has, and `Resources/textures`, which is the `.app`'s, reached from
/// `Contents/MacOS/sunlit-earth` at the `Contents/` ancestor.
pub fn textures_near(exe: &Path) -> Option<PathBuf> {
    let mut dir = exe.to_path_buf();
    while dir.pop() {
        for candidate in [dir.join("textures"), dir.join("Resources").join("textures")] {
            if candidate.is_dir() {
                return Some(candidate);
            }
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

    /// A row rotates right by three quarters of its width, whatever the width,
    /// and a row of one colour comes back unchanged because a rotation moves
    /// pixels without altering them.
    #[test]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the fixture's channels are small by construction"
    )]
    fn a_row_rotates_right_by_three_quarters_of_its_width() {
        for width in [4usize, 8, 12] {
            let px: Vec<[u8; 4]> = (0..width)
                .map(|i| [(i * 10) as u8, (i * 10 + 1) as u8, (i * 10 + 2) as u8, 255])
                .collect();
            let mut buf = pixels_from(&px);
            shift_horizontal(&mut buf, width as u32, 1);

            let shift = width * 3 / 4;
            for (i, pixel) in px.iter().enumerate() {
                assert_eq!(
                    pixel_at(&buf, (i + shift) % width),
                    *pixel,
                    "pixel {i} of {width}"
                );
            }
        }

        let mut uniform = [128, 64, 32, 255].repeat(4);
        let original = uniform.clone();
        shift_horizontal(&mut uniform, 4, 1);
        assert_eq!(uniform, original, "a row of one colour cannot rotate");
    }

    #[test]
    fn shift_4px_two_rows_independent() {
        let row1 = [
            [1, 2, 3, 4],
            [5, 6, 7, 8],
            [9, 10, 11, 12],
            [13, 14, 15, 16],
        ];
        let row2 = [
            [17, 18, 19, 20],
            [21, 22, 23, 24],
            [25, 26, 27, 28],
            [29, 30, 31, 32],
        ];
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

    // -----------------------------------------------------------------------
    // flip_horizontal
    // -----------------------------------------------------------------------

    /// The flip replaced `DynamicImage::fliph`, which is what every golden
    /// reference was generated with, so it has to mean exactly the same thing.
    #[test]
    fn flip_matches_the_image_crates_own() {
        for (w, h) in [(1u32, 1u32), (2, 3), (5, 4), (8, 8), (7, 1)] {
            let pixels: Vec<u8> = (0..w * h * 4).map(|i| (i % 251) as u8).collect();
            let expected = image::imageops::flip_horizontal(
                &image::RgbaImage::from_raw(w, h, pixels.clone()).expect("buffer fits"),
            )
            .into_raw();

            let mut ours = pixels;
            flip_horizontal(&mut ours, w, h);
            assert_eq!(ours, expected, "differed at {w}x{h}");
        }
    }

    // -----------------------------------------------------------------------
    // downsample_2x
    // -----------------------------------------------------------------------

    #[test]
    fn downsample_2x2_checkerboard() {
        // tl=[0,0,0,255], tr=[100,0,0,255], bl=[0,100,0,255], br=[0,0,100,255]
        #[rustfmt::skip]
        let src = vec![
            0, 0, 0, 255,    100, 0, 0, 255,
            0, 100, 0, 255,  0, 0, 100, 255,
        ];
        let dst = downsample_2x(&src, 2, 2);
        assert_eq!(dst, [25, 25, 25, 255]);
    }

    /// Averaging four identical pixels cannot move the colour, whatever the
    /// size of the image they came from.
    #[test]
    fn a_uniform_image_survives_being_downsampled() {
        for (w, h, colour) in [
            (2u32, 2u32, [255u8, 0, 0, 255]),
            (4, 4, [255, 255, 255, 255]),
        ] {
            let src = colour.repeat((w * h) as usize);
            let dst = downsample_2x(&src, w, h);
            assert_eq!(dst.len(), ((w / 2) * (h / 2) * 4) as usize);
            for chunk in dst.chunks(4) {
                assert_eq!(chunk, colour, "{w}x{h}");
            }
        }
    }

    #[test]
    fn downsample_output_length() {
        for (w, h) in [(2, 2), (4, 4), (8, 6), (16, 2), (2, 16)] {
            let src = vec![128u8; (w * h * 4) as usize];
            let dst = downsample_2x(&src, w, h);
            let expected_len = ((w / 2).max(1) * (h / 2).max(1) * 4) as usize;
            assert_eq!(dst.len(), expected_len, "failed for ({w}, {h})");
        }
    }

    /// An image of `width` by `height` filled from `seed`.
    ///
    /// Derived from the dimensions rather than generated beside them, so no
    /// case is thrown away for having the wrong length.
    #[expect(clippy::cast_possible_truncation, reason = "the modulo leaves a byte")]
    fn fixture(width: u32, height: u32, seed: u8) -> Vec<u8> {
        (0..width as usize * height as usize * 4)
            .map(|i| (i.wrapping_mul(31).wrapping_add(seed as usize) % 251) as u8)
            .collect()
    }

    proptest::proptest! {
        #[test]
        fn flip_twice_is_identity_for_any_size(
            width in 1u32..=64,
            height in 1u32..=16,
            seed in proptest::num::u8::ANY,
        ) {
            let original = fixture(width, height, seed);
            let mut buf = original.clone();

            flip_horizontal(&mut buf, width, height);
            flip_horizontal(&mut buf, width, height);
            proptest::prop_assert_eq!(buf, original);
        }
    }

    proptest::proptest! {
        #[test]
        fn shift_four_times_is_identity(
            width in 1u32..=64,
            height in 1u32..=16,
            seed in proptest::num::u8::ANY,
        ) {
            let original = fixture(width, height, seed);
            let mut buf = original.clone();

            // Four shifts of 3/4 width = 3 full rotations = identity
            for _ in 0..4 {
                shift_horizontal(&mut buf, width, height);
            }
            proptest::prop_assert_eq!(buf, original);
        }
    }

    /// The `.app`'s `Contents/Resources/textures`, which the plain candidate
    /// never reaches on the way up, and the sibling layout beside it.
    #[test]
    fn the_walk_up_finds_an_app_bundles_resources_and_a_plain_layout_alike() {
        let scratch = crate::test_support::ScratchDir::new("texture_loader_layouts");

        let app = scratch.join("Sunlit Earth.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).expect("the executable directory");
        std::fs::create_dir_all(app.join("Contents/Resources/textures")).expect("the resources");
        assert_eq!(
            textures_near(&app.join("Contents/MacOS/sunlit-earth")),
            Some(app.join("Contents/Resources/textures"))
        );

        let plain = scratch.join("sunlit-earth-0.1.0-linux");
        std::fs::create_dir_all(plain.join("textures")).expect("the textures");
        assert_eq!(
            textures_near(&plain.join("sunlit-earth")),
            Some(plain.join("textures"))
        );

        // Where an ancestor has both, the plain one answers.
        let both = scratch.join("both");
        std::fs::create_dir_all(both.join("textures")).expect("the textures");
        std::fs::create_dir_all(both.join("Resources/textures")).expect("the resources");
        assert_eq!(
            textures_near(&both.join("sunlit-earth")),
            Some(both.join("textures"))
        );
    }

    /// A symlink in another directory, as Homebrew's `bin/` holds, leads to the
    /// textures beside its target.
    #[cfg(unix)]
    #[test]
    fn a_symlink_to_the_executable_finds_the_textures_beside_its_target() {
        let scratch = crate::test_support::ScratchDir::new("texture_loader_symlink");

        let install = scratch.join("libexec");
        std::fs::create_dir_all(install.join("textures")).expect("the textures");
        std::fs::write(install.join("sunlit-earth"), b"").expect("the executable");
        let bin = scratch.join("bin");
        std::fs::create_dir_all(&bin).expect("the bin directory");
        std::os::unix::fs::symlink(install.join("sunlit-earth"), bin.join("sunlit-earth"))
            .expect("the symlink");

        assert_eq!(textures_near(&bin.join("sunlit-earth")), None);
        let found = textures_for_exe(&bin.join("sunlit-earth")).expect("the textures");
        assert_eq!(
            std::fs::canonicalize(found).expect("the found directory"),
            std::fs::canonicalize(install.join("textures")).expect("the textures directory")
        );
    }
}
