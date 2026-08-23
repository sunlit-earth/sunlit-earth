//! `cargo xtask bake-icon`: the icon SVGs to the rasters the app ships.
//!
//! The mark is four SVGs under `assets/icon/`, a master and three variants that
//! shed detail rather than scale it, and everything the app shows an icon in
//! wants pixels: a Windows `.ico` for the exe resource, raw RGBA for the tray,
//! a PNG for the About window, and the freedesktop hicolor set for Linux
//! launchers. This is the one place that turns one into the other.
//!
//! The outputs are committed rather than baked during `cargo build`, so a build
//! gains no SVG rasterizer and nothing runs one on a user's machine; the bake
//! reruns when an SVG changes. [`bake`] returns the bytes instead of writing
//! them, which is what lets `the_committed_bake_matches_a_fresh_one` compare
//! the tree against a fresh render and fail when an SVG moved without it.

use std::path::{Path, PathBuf};

use resvg::tiny_skia;
use resvg::usvg;

use crate::store;
use crate::util;

/// The basename every output is filed under, and the `Icon=` key in the
/// desktop entry. The hicolor lookup is by name, so this is a contract with
/// `assets/linux/sunlit-earth.desktop` rather than a label.
pub const ICON_NAME: &str = "sunlit-earth";

/// Where the baked outputs live, relative to the repository root.
pub const BAKED_DIR: &str = "assets/icon/baked";

/// The sizes inside the Windows `.ico`.
///
/// The list is what Windows itself asks for across its shell surfaces: 16 in a
/// title bar, 20 and 24 in the tray and small list views, 32 on the desktop,
/// 40 and 48 in medium views and the taskbar at scaling, and 64 upward for the
/// large views and the file dialog preview. A size the file does not carry is
/// resampled by the shell from one that it does, which is exactly the muddy
/// result the per-size variants exist to avoid.
pub const ICO_SIZES: [u32; 9] = [16, 20, 24, 32, 40, 48, 64, 128, 256];

/// The sizes in the freedesktop hicolor set.
///
/// The theme spec's own list minus the ones no panel asks for; 20 and 40 are
/// Windows shell sizes and have no hicolor directory to live in.
pub const HICOLOR_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

/// The tray pixmap's edge, which is what `tray::create_icon` hands Slint.
pub const TRAY_SIZE: u32 = 32;

/// The About window's logo edge.
pub const ABOUT_SIZE: u32 = 256;

/// The sizes a taste check looks at: the ones a shell draws small enough that
/// the pixel grid decides whether the mark reads at all.
const REVIEW_SIZES: [u32; 6] = [16, 20, 24, 32, 40, 48];

/// The two fields the review sheet lays them on, a dark and a light shell
/// chrome. An icon is judged against what is behind it, and a mark tuned on
/// one of these can lose an edge on the other.
const REVIEW_BACKDROPS: [[u8; 3]; 2] = [[0x1C, 0x1D, 0x22], [0xF2, 0xF3, 0xF5]];

/// How far the sheet magnifies the second row. Nearest-neighbour, so it shows
/// the pixels the shell will actually draw rather than a smoothed idea of them.
const REVIEW_ZOOM: u32 = 6;

/// One baked file: where it goes under [`BAKED_DIR`], and what is in it.
pub struct Output {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
}

/// The SVG a raster of this size is drawn from.
///
/// Each variant is authored for a size range rather than for one size: what
/// cannot survive the raster is dropped and what stays grows in canvas units,
/// so the choice is the largest variant that is still at or below the target.
/// The master covers everything from 48 up, where the full detail budget
/// resolves.
pub fn source_for(size: u32) -> &'static str {
    match size {
        0..=16 => "sunlit-earth-16.svg",
        17..=24 => "sunlit-earth-24.svg",
        25..=40 => "sunlit-earth-32.svg",
        _ => "sunlit-earth.svg",
    }
}

/// Rasterize every output from the SVGs in `source_dir`, without writing
/// anything.
pub fn bake(source_dir: &Path) -> Result<Vec<Output>, String> {
    let mut outputs = Vec::new();

    let mut ico = ico::IconDir::new(ico::ResourceType::Icon);
    for size in ICO_SIZES {
        let image = ico::IconImage::from_rgba_data(size, size, render(source_dir, size)?);
        // BMP below 256 and PNG at 256, which is the layout every Windows icon
        // tool produces: a 32-bit BMP carries the alpha channel intact, and the
        // 256 entry is PNG because that is the only encoding the shell reads at
        // that size.
        let entry = if size == 256 {
            ico::IconDirEntry::encode_as_png(&image)
        } else {
            ico::IconDirEntry::encode_as_bmp(&image)
        };
        ico.add_entry(entry.map_err(|e| format!("encoding the {size} px icon entry: {e}"))?);
    }
    let mut ico_bytes = Vec::new();
    ico.write(&mut ico_bytes)
        .map_err(|e| format!("assembling {ICON_NAME}.ico: {e}"))?;
    outputs.push(Output {
        path: PathBuf::from(format!("{ICON_NAME}.ico")),
        bytes: ico_bytes,
    });

    for size in HICOLOR_SIZES {
        outputs.push(Output {
            path: hicolor_path(size),
            bytes: png(render(source_dir, size)?, size)?,
        });
    }

    // Raw RGBA rather than a PNG: the app crate wraps these bytes in a
    // `SharedPixelBuffer` directly, so shipping them decoded is what keeps an
    // image decoder out of the app's dependency list.
    outputs.push(Output {
        path: PathBuf::from(format!("tray-{TRAY_SIZE}.rgba")),
        bytes: render(source_dir, TRAY_SIZE)?,
    });

    outputs.push(Output {
        path: PathBuf::from(format!("about-{ABOUT_SIZE}.png")),
        bytes: png(render(source_dir, ABOUT_SIZE)?, ABOUT_SIZE)?,
    });

    Ok(outputs)
}

/// Where a hicolor raster goes, as the theme spec lays the directory out. The
/// scalable slot is the master SVG itself and is not baked; the install script
/// copies it from `assets/icon/`.
pub fn hicolor_path(size: u32) -> PathBuf {
    PathBuf::from("hicolor")
        .join(format!("{size}x{size}"))
        .join("apps")
        .join(format!("{ICON_NAME}.png"))
}

/// Render one SVG at one size into straight (non-premultiplied) RGBA8.
///
/// Straight alpha because both consumers want it that way: `slint::Image::
/// from_rgba8` and `ico::IconImage::from_rgba_data` each take unpremultiplied
/// bytes, and tiny-skia's pixmap is premultiplied, so the demultiply happens
/// here once rather than in two callers.
fn render(source_dir: &Path, size: u32) -> Result<Vec<u8>, String> {
    let source = source_dir.join(source_for(size));
    let data = std::fs::read(&source).map_err(|e| format!("reading {}: {e}", source.display()))?;
    let tree = usvg::Tree::from_data(&data, &usvg::Options::default())
        .map_err(|e| format!("parsing {}: {e}", source.display()))?;

    let mut pixmap = tiny_skia::Pixmap::new(size, size)
        .ok_or_else(|| format!("a {size}x{size} pixmap is not a valid size"))?;
    // Every variant shares the master's 64-unit viewBox, so one uniform scale
    // covers all four and the mark lands on the pixel grid the same way at
    // every size.
    #[allow(clippy::cast_precision_loss)]
    let scale = size as f32 / tree.size().width();
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );

    let mut rgba = Vec::with_capacity(pixmap.pixels().len() * 4);
    for pixel in pixmap.pixels() {
        let color = pixel.demultiply();
        rgba.extend_from_slice(&[color.red(), color.green(), color.blue(), color.alpha()]);
    }
    Ok(rgba)
}

/// Encode straight RGBA8 as a PNG.
fn png(rgba: Vec<u8>, size: u32) -> Result<Vec<u8>, String> {
    let image = image::RgbaImage::from_raw(size, size, rgba)
        .ok_or_else(|| format!("the {size} px raster is not {size}x{size} of RGBA"))?;
    let mut bytes = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .map_err(|e| format!("encoding the {size} px PNG: {e}"))?;
    Ok(bytes)
}

/// The contact sheet for the small-size taste check: every [`REVIEW_SIZES`]
/// raster at 1x and magnified, on both of [`REVIEW_BACKDROPS`].
///
/// The judgement this supports cannot be automated, and the sheet does not try
/// to make it: what it removes is the part that is only tedious, which is
/// getting the same rasters onto both fields at a scale where the grid is
/// visible, every time an SVG is nudged.
fn review_sheet(source_dir: &Path) -> Result<Vec<u8>, String> {
    const MARGIN: u32 = 16;
    const GAP: u32 = 16;

    let rasters = REVIEW_SIZES
        .iter()
        .map(|&size| render(source_dir, size).map(|rgba| (size, rgba)))
        .collect::<Result<Vec<_>, _>>()?;

    let tallest = REVIEW_SIZES.into_iter().max().unwrap_or(0);
    let (mut row, mut zoomed) = (0, 0);
    for size in REVIEW_SIZES {
        row += size + GAP;
        zoomed += size * REVIEW_ZOOM + GAP;
    }
    let content = row.max(zoomed) - GAP;
    let strip = MARGIN + tallest + GAP + tallest * REVIEW_ZOOM + MARGIN;
    let strips = u32::try_from(REVIEW_BACKDROPS.len()).expect("a handful of backdrops");

    let mut sheet = image::RgbaImage::new(MARGIN * 2 + content, strip * strips);
    let mut top = 0;
    for backdrop in REVIEW_BACKDROPS {
        for y in top..top + strip {
            for x in 0..sheet.width() {
                sheet.put_pixel(
                    x,
                    y,
                    image::Rgba([backdrop[0], backdrop[1], backdrop[2], 255]),
                );
            }
        }

        // Bottom-aligned in each row, the way a shell seats icons of different
        // sizes on one baseline.
        let plain_foot = top + MARGIN + tallest;
        let zoomed_foot = plain_foot + GAP + tallest * REVIEW_ZOOM;
        let (mut plain_x, mut zoomed_x) = (MARGIN, MARGIN);
        for (size, rgba) in &rasters {
            blit(&mut sheet, rgba, *size, 1, plain_x, plain_foot - size);
            blit(
                &mut sheet,
                rgba,
                *size,
                REVIEW_ZOOM,
                zoomed_x,
                zoomed_foot - size * REVIEW_ZOOM,
            );
            plain_x += size + GAP;
            zoomed_x += size * REVIEW_ZOOM + GAP;
        }
        top += strip;
    }

    let mut bytes = Vec::new();
    sheet
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .map_err(|e| format!("encoding the review sheet: {e}"))?;
    Ok(bytes)
}

/// Composite one raster over the sheet, magnified by whole pixels.
fn blit(sheet: &mut image::RgbaImage, rgba: &[u8], size: u32, zoom: u32, ox: u32, oy: u32) {
    for y in 0..size * zoom {
        for x in 0..size * zoom {
            let i = (((y / zoom) * size + (x / zoom)) * 4) as usize;
            let alpha = u32::from(rgba[i + 3]);
            let under = *sheet.get_pixel(ox + x, oy + y);
            let mut over = [0u8; 4];
            for c in 0..3 {
                let blended =
                    (u32::from(rgba[i + c]) * alpha + u32::from(under[c]) * (255 - alpha) + 127)
                        / 255;
                over[c] = u8::try_from(blended).unwrap_or(u8::MAX);
            }
            over[3] = 255;
            sheet.put_pixel(ox + x, oy + y, image::Rgba(over));
        }
    }
}

pub fn run(review: Option<PathBuf>) -> Result<u8, String> {
    let repo = store::repo_root();
    let source_dir = repo.join("assets").join("icon");
    let baked_dir = repo.join(BAKED_DIR);

    if let Some(dir) = review {
        std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
        for size in REVIEW_SIZES {
            let path = dir.join(format!("review-{size}.png"));
            std::fs::write(&path, png(render(&source_dir, size)?, size)?)
                .map_err(|e| format!("writing {}: {e}", path.display()))?;
        }
        let path = dir.join("review-sheet.png");
        std::fs::write(&path, review_sheet(&source_dir)?)
            .map_err(|e| format!("writing {}: {e}", path.display()))?;
        println!("review rasters and {} written", path.display());
        return Ok(0);
    }

    println!("baking from {}", source_dir.display());
    let outputs = bake(&source_dir)?;

    let mut unchanged = 0;
    for output in &outputs {
        let target = baked_dir.join(&output.path);
        let parent = target
            .parent()
            .ok_or_else(|| format!("{} has no parent directory", target.display()))?;
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("creating {}: {e}", parent.display()))?;

        let same = std::fs::read(&target).is_ok_and(|existing| existing == output.bytes);
        if same {
            unchanged += 1;
        } else {
            std::fs::write(&target, &output.bytes)
                .map_err(|e| format!("writing {}: {e}", target.display()))?;
        }
        println!(
            "  {:<40} {:>10}{}",
            output.path.display().to_string().replace('\\', "/"),
            util::format_bytes(output.bytes.len() as u64),
            if same { "" } else { "  (changed)" }
        );
    }

    println!(
        "\n{} into {}, {unchanged} already current",
        util::count(outputs.len(), "output"),
        baked_dir.display()
    );
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source_dir() -> PathBuf {
        store::repo_root().join("assets").join("icon")
    }

    #[test]
    fn every_size_is_drawn_from_a_variant_that_exists() {
        let sizes = ICO_SIZES
            .into_iter()
            .chain(HICOLOR_SIZES)
            .chain([TRAY_SIZE, ABOUT_SIZE]);
        for size in sizes {
            let source = source_dir().join(source_for(size));
            assert!(
                source.is_file(),
                "{size} px is drawn from {}, which is not in the tree",
                source.display()
            );
        }
    }

    #[test]
    fn a_variant_covers_its_own_size_and_nothing_larger() {
        // The mapping's whole point: a size gets the variant authored at or
        // below it, never one authored above, since a variant carries detail
        // its own size can resolve and no more.
        assert_eq!(source_for(16), "sunlit-earth-16.svg");
        assert_eq!(source_for(20), "sunlit-earth-24.svg");
        assert_eq!(source_for(24), "sunlit-earth-24.svg");
        assert_eq!(source_for(32), "sunlit-earth-32.svg");
        assert_eq!(source_for(40), "sunlit-earth-32.svg");
        assert_eq!(source_for(48), "sunlit-earth.svg");
        assert_eq!(source_for(256), "sunlit-earth.svg");
    }

    #[test]
    fn the_smallest_raster_is_still_a_disk_on_a_transparent_field() {
        let size = 16;
        let rgba = render(&source_dir(), size).expect("the 16 px raster");
        let at = |x: u32, y: u32| -> [u8; 4] {
            let i = ((y * size + x) * 4) as usize;
            [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
        };
        // Opaque in the middle and clear in the corner: a pixmap that came back
        // empty, or one the transform filled edge to edge, fails here rather
        // than shipping as an invisible or a square icon.
        assert_eq!(at(size / 2, size / 2)[3], 255, "the disk centre is opaque");
        assert_eq!(at(0, size - 1)[3], 0, "the far corner is clear");
    }

    #[test]
    fn the_desktop_entry_asks_for_the_icon_the_bake_writes() {
        // `Icon=` is a theme lookup by name, and nothing else connects the
        // entry to the files: a mismatch is a launcher showing a generic
        // placeholder, with no error raised anywhere along the way.
        let path = store::repo_root()
            .join("assets")
            .join("linux")
            .join("sunlit-earth.desktop");
        let entry = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        let icon = entry
            .lines()
            .find_map(|line| line.trim().strip_prefix("Icon="))
            .expect("the entry has an Icon= key");

        assert_eq!(icon, ICON_NAME);
        for size in HICOLOR_SIZES {
            let installed = hicolor_path(size);
            assert_eq!(
                installed.file_stem().and_then(std::ffi::OsStr::to_str),
                Some(icon),
                "{} is not what the entry asks for",
                installed.display()
            );
        }
    }

    #[test]
    fn the_committed_bake_matches_a_fresh_one() {
        // Criterion 1, and the reason the outputs can be trusted as committed
        // data: an SVG edited without rerunning the bake fails here, and so
        // does a rasterizer whose output moved under the pinned version.
        let baked_dir = store::repo_root().join(BAKED_DIR);
        for output in bake(&source_dir()).expect("the bake") {
            let target = baked_dir.join(&output.path);
            let committed = std::fs::read(&target)
                .unwrap_or_else(|e| panic!("reading {}: {e}", target.display()));
            assert_eq!(
                committed.len(),
                output.bytes.len(),
                "{} is {} bytes on disk and {} freshly baked; run `cargo xtask bake-icon`",
                output.path.display(),
                committed.len(),
                output.bytes.len()
            );
            assert!(
                committed == output.bytes,
                "{} differs from a fresh bake; run `cargo xtask bake-icon`",
                output.path.display()
            );
        }
    }
}
