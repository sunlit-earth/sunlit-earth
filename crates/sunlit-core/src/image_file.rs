//! Encoding a rendered image into a file, with progress and a way to stop.
//!
//! Every image this program writes goes through [`write_image`]: the exports
//! the user asks for, the `render` subcommand and the wallpaper frames. The
//! file is built under a temporary name beside the target and renamed onto it
//! at the end, so a reader never sees half an image and a cancel or a failure
//! leaves whatever was there before.

use std::cell::Cell;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// The rows of a PNG handed to the encoder at a time, in bytes: small enough
/// for a progress bar to move smoothly, large enough that the calls cost
/// nothing next to the compression.
const PNG_BAND_BYTES: usize = 1 << 20;

/// What to encode an image as, with the one setting each format has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    /// `compression` is a deflate level from 1 (fastest) to 9 (smallest).
    Png { compression: u8 },
    /// `quality` from 1 to 100.
    Jpeg { quality: u8 },
}

impl ImageFormat {
    /// The PNG every file the program writes for itself uses: the fastest
    /// level, since somebody is waiting for the wallpaper to change.
    pub const FAST_PNG: Self = Self::Png { compression: 1 };

    /// The file extension this format is saved under, without the dot.
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png { .. } => "png",
            Self::Jpeg { .. } => "jpg",
        }
    }
}

/// Why [`write_image`] wrote nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteError {
    /// The cancel flag was set before the file was complete.
    Cancelled,
    /// The buffer, the encoder or the filesystem failed; the text says which.
    Failed(String),
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("the export was cancelled"),
            Self::Failed(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for WriteError {}

impl From<WriteError> for String {
    fn from(error: WriteError) -> Self {
        error.to_string()
    }
}

/// Encode RGBA8 `pixels` of `width` by `height` as `format` into `path`.
///
/// `progress` is called with the fraction of the image encoded, rising from 0
/// to 1, from this thread. Setting `cancel` stops the encoder at its next
/// write. Either way out but success removes the temporary file and leaves
/// `path` as it was. The buffer is checked before the filesystem is touched,
/// so a buffer of the wrong size creates nothing.
///
/// :param path: the file to create or replace.
/// :param pixels: the image, four bytes a pixel, rows from the top.
/// :param progress: told how far the encoder has come.
/// :param cancel: read before every write the encoder makes.
/// :returns: `WriteError::Cancelled` when `cancel` stopped it.
pub fn write_image(
    path: &Path,
    pixels: &[u8],
    width: u32,
    height: u32,
    format: ImageFormat,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<(), WriteError> {
    let expected = usize::try_from(u64::from(width) * u64::from(height) * 4);
    if expected != Ok(pixels.len()) || width == 0 || height == 0 {
        return Err(WriteError::Failed(format!(
            "the pixel buffer holds {} bytes, which is not a {width}x{height} image",
            pixels.len()
        )));
    }

    let temp = crate::files::unfinished(path, ".tmp");
    let written = std::fs::File::create(&temp)
        .map_err(|e| WriteError::Failed(format!("could not create {}: {e}", temp.display())))
        .and_then(|file| {
            let mut sink = Cancellable {
                inner: BufWriter::new(file),
                cancel,
            };
            let encoded = match format {
                ImageFormat::Png { compression } => {
                    encode_png(&mut sink, pixels, width, height, compression, progress)
                }
                ImageFormat::Jpeg { quality } => {
                    encode_jpeg(&mut sink, pixels, width, height, quality, progress)
                }
            };
            encoded
                .and_then(|()| {
                    sink.inner
                        .into_inner()
                        .map(drop)
                        .map_err(|e| e.into_error().to_string())
                })
                .map_err(|reason| {
                    if cancel.load(Ordering::Relaxed) {
                        WriteError::Cancelled
                    } else {
                        WriteError::Failed(format!(
                            "could not write the {}: {reason}",
                            format.extension().to_uppercase()
                        ))
                    }
                })
        })
        .and_then(|()| {
            if cancel.load(Ordering::Relaxed) {
                return Err(WriteError::Cancelled);
            }
            std::fs::rename(&temp, path).map_err(|e| {
                WriteError::Failed(format!("could not put {} in place: {e}", path.display()))
            })
        });
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    } else {
        progress(1.0);
    }
    written
}

/// [`write_image`] as [`ImageFormat::FAST_PNG`], for a caller that neither
/// shows progress nor cancels: a wallpaper frame and a `render` subcommand.
pub fn write_png(path: &Path, pixels: &[u8], width: u32, height: u32) -> Result<(), WriteError> {
    write_image(
        path,
        pixels,
        width,
        height,
        ImageFormat::FAST_PNG,
        &|_| {},
        &AtomicBool::new(false),
    )
}

fn encode_png(
    sink: &mut impl Write,
    pixels: &[u8],
    width: u32,
    height: u32,
    compression: u8,
    progress: &dyn Fn(f32),
) -> Result<(), String> {
    let mut encoder = png::Encoder::new(sink, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_filter(png::Filter::Adaptive);
    encoder.set_deflate_compression(match compression.clamp(1, 9) {
        1 => png::DeflateCompression::FdeflateUltraFast,
        level => png::DeflateCompression::Level(level),
    });
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    {
        let mut stream = writer.stream_writer().map_err(|e| e.to_string())?;
        let row = width as usize * 4;
        let band = (PNG_BAND_BYTES / row).max(1) * row;
        let mut done = 0usize;
        progress(0.0);
        for chunk in pixels.chunks(band) {
            stream.write_all(chunk).map_err(|e| e.to_string())?;
            done += chunk.len();
            #[expect(clippy::cast_precision_loss, reason = "a fraction for a progress bar")]
            progress(done as f32 / pixels.len() as f32);
        }
        stream.finish().map_err(|e| e.to_string())?;
    }
    writer.finish().map_err(|e| e.to_string())
}

fn encode_jpeg(
    sink: &mut impl Write,
    pixels: &[u8],
    width: u32,
    height: u32,
    quality: u8,
    progress: &dyn Fn(f32),
) -> Result<(), String> {
    let view = RowReporting {
        pixels,
        width,
        height,
        row: Cell::new(0),
        progress,
    };
    progress(0.0);
    image::codecs::jpeg::JpegEncoder::new_with_quality(sink, quality.clamp(1, 100))
        .encode_image(&view)
        .map_err(|e| e.to_string())
}

/// An RGBA buffer seen as an image, which tells `progress` whenever the JPEG
/// encoder reaches a row further down than any before. The encoder reads every
/// pixel once, in rows of 8 by 8 blocks from the top.
struct RowReporting<'a> {
    pixels: &'a [u8],
    width: u32,
    height: u32,
    row: Cell<u32>,
    progress: &'a dyn Fn(f32),
}

impl image::GenericImageView for RowReporting<'_> {
    type Pixel = image::Rgba<u8>;

    fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn get_pixel(&self, x: u32, y: u32) -> image::Rgba<u8> {
        if y > self.row.get() {
            self.row.set(y);
            #[expect(clippy::cast_precision_loss, reason = "a fraction for a progress bar")]
            (self.progress)(y as f32 / self.height as f32);
        }
        let at = (y as usize * self.width as usize + x as usize) * 4;
        let mut pixel = [0; 4];
        pixel.copy_from_slice(&self.pixels[at..at + 4]);
        image::Rgba(pixel)
    }
}

/// A writer that fails once `cancel` is set, which is how an encoder that
/// knows nothing of cancelling is stopped between two of its writes.
struct Cancellable<'a, W> {
    inner: W,
    cancel: &'a AtomicBool,
}

impl<W: Write> Write for Cancellable<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(io::Error::other("cancelled"));
        }
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::test_support::ScratchDir;

    fn gradient(width: u32, height: u32) -> Vec<u8> {
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
        for y in 0..height {
            for x in 0..width {
                #[expect(clippy::cast_possible_truncation, reason = "a test pattern")]
                pixels.extend_from_slice(&[
                    (x * 255 / width) as u8,
                    (y * 255 / height) as u8,
                    ((x + y) % 256) as u8,
                    255,
                ]);
            }
        }
        pixels
    }

    fn entries(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect()
    }

    fn write_recording(
        path: &Path,
        pixels: &[u8],
        width: u32,
        height: u32,
        format: ImageFormat,
    ) -> (Result<(), WriteError>, Vec<f32>) {
        let seen = RefCell::new(Vec::new());
        let result = write_image(
            path,
            pixels,
            width,
            height,
            format,
            &|fraction| seen.borrow_mut().push(fraction),
            &AtomicBool::new(false),
        );
        (result, seen.into_inner())
    }

    fn assert_progress_runs_from_zero_to_one(seen: &[f32]) {
        assert!(!seen.is_empty());
        assert!(seen[0] >= 0.0, "{seen:?}");
        assert!(seen.windows(2).all(|pair| pair[0] <= pair[1]), "{seen:?}");
        approx::assert_relative_eq!(*seen.last().unwrap(), 1.0);
    }

    #[test]
    fn a_png_round_trips_pixel_for_pixel() {
        let dir = ScratchDir::new("image_file_png");
        let path = dir.path().join("out.png");
        let (width, height) = (300, 900);
        let pixels = gradient(width, height);

        let (result, seen) = write_recording(&path, &pixels, width, height, ImageFormat::FAST_PNG);

        result.unwrap();
        let decoded = image::open(&path).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (width, height));
        assert_eq!(decoded.into_raw(), pixels);
        assert_progress_runs_from_zero_to_one(&seen);
        assert!(seen.len() > 2, "a tall image is fed in bands: {seen:?}");
        assert_eq!(entries(dir.path()), ["out.png"]);
    }

    #[test]
    fn every_png_level_decodes_to_the_same_pixels() {
        let dir = ScratchDir::new("image_file_levels");
        let (width, height) = (64, 48);
        let pixels = gradient(width, height);
        for compression in 1..=9 {
            let path = dir.path().join(format!("level-{compression}.png"));
            let (result, _) = write_recording(
                &path,
                &pixels,
                width,
                height,
                ImageFormat::Png { compression },
            );
            result.unwrap();
            let decoded = image::open(&path).unwrap().to_rgba8();
            assert_eq!(decoded.into_raw(), pixels, "level {compression}");
        }
    }

    #[test]
    fn a_jpeg_decodes_close_to_its_source() {
        let dir = ScratchDir::new("image_file_jpeg");
        let path = dir.path().join("out.jpg");
        let (width, height) = (320, 200);
        let pixels = gradient(width, height);

        let (result, seen) = write_recording(
            &path,
            &pixels,
            width,
            height,
            ImageFormat::Jpeg { quality: 90 },
        );

        result.unwrap();
        let decoded = image::open(&path).unwrap().to_rgb8();
        assert_eq!(decoded.dimensions(), (width, height));
        let source = pixels.chunks_exact(4).flat_map(|pixel| &pixel[..3]);
        let total: u64 = decoded
            .as_raw()
            .iter()
            .zip(source)
            .map(|(a, b)| u64::from(a.abs_diff(*b)))
            .sum();
        #[expect(clippy::cast_precision_loss, reason = "a mean for a test")]
        let mean = total as f64 / decoded.as_raw().len() as f64;
        assert!(mean < 3.0, "mean channel difference {mean}");
        assert_progress_runs_from_zero_to_one(&seen);
        assert!(seen.len() > 2, "{seen:?}");
    }

    #[test]
    fn a_buffer_of_the_wrong_size_creates_nothing() {
        let dir = ScratchDir::new("image_file_short");
        let path = dir.path().join("out.png");
        let (result, _) = write_recording(&path, &[0; 8], 4, 4, ImageFormat::FAST_PNG);
        assert!(matches!(result, Err(WriteError::Failed(_))), "{result:?}");
        assert!(entries(dir.path()).is_empty());
    }

    #[test]
    fn a_cancel_halfway_leaves_neither_the_file_nor_a_temporary_one() {
        let (width, height) = (512, 2048);
        let pixels = gradient(width, height);
        for format in [ImageFormat::FAST_PNG, ImageFormat::Jpeg { quality: 90 }] {
            let dir = ScratchDir::new("image_file_cancel");
            let path = dir.path().join("out");
            let cancel = AtomicBool::new(false);
            let result = write_image(
                &path,
                &pixels,
                width,
                height,
                format,
                &|fraction| {
                    if fraction >= 0.5 {
                        cancel.store(true, Ordering::Relaxed);
                    }
                },
                &cancel,
            );
            assert_eq!(result, Err(WriteError::Cancelled), "{format:?}");
            assert!(entries(dir.path()).is_empty(), "{format:?}");
        }
    }

    #[test]
    fn a_cancel_leaves_an_existing_file_as_it_was() {
        let dir = ScratchDir::new("image_file_keep");
        let path = dir.path().join("out.png");
        std::fs::write(&path, b"before").unwrap();
        let result = write_image(
            &path,
            &gradient(8, 8),
            8,
            8,
            ImageFormat::FAST_PNG,
            &|_| {},
            &AtomicBool::new(true),
        );
        assert_eq!(result, Err(WriteError::Cancelled));
        assert_eq!(std::fs::read(&path).unwrap(), b"before");
        assert_eq!(entries(dir.path()), ["out.png"]);
    }

    #[test]
    fn a_missing_folder_is_an_error_that_leaves_nothing() {
        let dir = ScratchDir::new("image_file_folder");
        let path = dir.path().join("no-such-folder").join("out.png");
        let (result, _) = write_recording(&path, &gradient(4, 4), 4, 4, ImageFormat::FAST_PNG);
        assert!(matches!(result, Err(WriteError::Failed(_))), "{result:?}");
        assert!(entries(dir.path()).is_empty());
    }
}
