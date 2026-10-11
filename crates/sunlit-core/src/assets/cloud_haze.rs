//! The clear-sky level of a cloud map, which `fs_cloud` subtracts as a local
//! floor.
//!
//! The upstream map is mostly an infrared temperature map, so cold clear
//! ground reads as a grey veil whose level depends on the region. The estimate
//! takes a low percentile of the map over a window of a few hundred kilometers
//! as that level: inside a window most pixels are taken to be clear sky, and
//! the cloud above them stands out. `docs/rendering.md` has the reasoning and
//! the plan behind it is `docs/plans/2026-10-10-cloud-haze-plan.md`.
//!
//! Every filter wraps across the antimeridian and clamps at the poles.

use super::texture_loader::{Channels, DecodedImage, Pixels};

/// Width of the grid the estimate is made on, about 78 km a pixel at the
/// equator.
pub const GRID_WIDTH: usize = 512;
/// Height of the grid, which is 2:1 like every map.
pub const GRID_HEIGHT: usize = 256;
/// Share of a window, in percent, assumed to be clear sky.
pub const PERCENTILE: usize = 15;
/// Side of the percentile window in grid pixels, about 700 km at the equator.
pub const WINDOW: usize = 9;
/// Standard deviation of the blur applied to the percentile, in grid pixels,
/// about 230 km.
pub const SMOOTH_SIGMA: f32 = 3.0;

/// How far the blur reaches, in standard deviations.
const SMOOTH_TRUNCATE: f32 = 4.0;
/// Levels the percentile is taken over: the map's own 8 bits.
const LEVELS: usize = 256;

/// The clear-sky background of a one-channel cloud map, as a
/// [`GRID_WIDTH`] x [`GRID_HEIGHT`] one-channel image in the map's own
/// orientation.
///
/// Any map size is accepted, since `SUNLIT_EARTH_CLOUD_URL` can serve one that
/// is not a published variant.
///
/// # Panics
///
/// When the map is not one channel or holds fewer bytes than its size says.
#[expect(clippy::cast_possible_truncation, reason = "the grid is 512 x 256")]
pub fn background(map: &DecodedImage) -> DecodedImage {
    assert_eq!(
        map.channels,
        Channels::Red,
        "the estimate reads a one-channel map"
    );
    let (width, height) = (map.width as usize, map.height as usize);
    assert!(map.pixels.len() >= width * height, "a short cloud map");

    let grid = reduce(&map.pixels, width, height);
    let floor = percentile(&grid);
    let smoothed = blur(&floor);
    let pixels = smoothed.iter().map(|&value| quantize(value)).collect();

    DecodedImage {
        pixels: Pixels::new(pixels),
        width: GRID_WIDTH as u32,
        height: GRID_HEIGHT as u32,
        channels: Channels::Red,
    }
}

/// A value from 0 to 1 as the byte it rounds to.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to 0 to 255 before the cast"
)]
fn quantize(value: f32) -> u8 {
    (value * 255.0).round().clamp(0.0, 255.0) as u8
}

/// What each output index of a box reduction from `from` to `to` samples: the
/// first input index and the weight of each input from there on, summing to 1.
///
/// An area average, so an input that does not divide evenly gives the inputs
/// at the edges of a cell the share of it they cover.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "indices of a map, which fit an f64 and come back non-negative"
)]
fn box_weights(from: usize, to: usize) -> Vec<(usize, Vec<f32>)> {
    let scale = from as f64 / to as f64;
    (0..to)
        .map(|index| {
            let low = index as f64 * scale;
            let high = (index + 1) as f64 * scale;
            let first = low.floor() as usize;
            let last = (high.ceil() as usize).min(from).max(first + 1);
            let weights = (first..last)
                .map(|input| {
                    let covered = high.min(input as f64 + 1.0) - low.max(input as f64);
                    (covered.max(0.0) / scale) as f32
                })
                .collect();
            (first, weights)
        })
        .collect()
}

/// Reduce the map to the grid by area averaging, as values from 0 to 1.
///
/// The rows first, into one accumulator row per grid row, which reads the map
/// once in memory order; then the columns of the grid's few rows.
fn reduce(pixels: &[u8], width: usize, height: usize) -> Vec<f32> {
    let rows = box_weights(height, GRID_HEIGHT);
    let columns = box_weights(width, GRID_WIDTH);
    let mut grid = vec![0.0f32; GRID_WIDTH * GRID_HEIGHT];
    let mut accumulated = vec![0.0f32; width];
    for (grid_row, (first, weights)) in grid.chunks_exact_mut(GRID_WIDTH).zip(&rows) {
        accumulated.fill(0.0);
        for (offset, &weight) in weights.iter().enumerate() {
            let start = (first + offset) * width;
            for (sum, &value) in accumulated.iter_mut().zip(&pixels[start..start + width]) {
                *sum += weight * f32::from(value);
            }
        }
        for (cell, (first, weights)) in grid_row.iter_mut().zip(&columns) {
            *cell = weights
                .iter()
                .zip(&accumulated[*first..])
                .map(|(weight, sum)| weight * sum)
                .sum::<f32>()
                / 255.0;
        }
    }
    grid
}

/// The [`PERCENTILE`] of every [`WINDOW`] x [`WINDOW`] window of the grid, on
/// the map's 8-bit levels.
///
/// A histogram slides along each row, and the level holding the wanted rank
/// moves with it, so a step costs the two columns that change and the few
/// levels the answer moves by rather than a sort of the whole window.
#[expect(
    clippy::cast_precision_loss,
    reason = "a level below 256, which an f32 holds exactly"
)]
fn percentile(grid: &[f32]) -> Vec<f32> {
    let levels: Vec<u8> = grid.iter().map(|&value| quantize(value)).collect();
    let levels = levels.as_slice();
    let half = WINDOW / 2;
    let count = WINDOW * WINDOW;
    // The rank scipy's percentile filter takes, 0-based.
    let rank = (PERCENTILE * count / 100).min(count - 1);
    let mut out = vec![0.0f32; GRID_WIDTH * GRID_HEIGHT];

    for (y, out_row) in out.chunks_exact_mut(GRID_WIDTH).enumerate() {
        let rows: Vec<usize> = (0..WINDOW)
            .map(|offset| (y + offset).saturating_sub(half).min(GRID_HEIGHT - 1))
            .collect();
        let column = |x: usize| rows.iter().map(move |&row| levels[row * GRID_WIDTH + x]);
        let wrap = |x: isize| x.rem_euclid(GRID_WIDTH.cast_signed()).cast_unsigned();

        let mut histogram = [0u32; LEVELS];
        for dx in -half.cast_signed()..=half.cast_signed() {
            for value in column(wrap(dx)) {
                histogram[usize::from(value)] += 1;
            }
        }
        // `below` counts the window's values under `level`, and the answer is
        // the level where the count first passes the rank.
        let mut level = 0usize;
        let mut below = 0u32;
        let rank = u32::try_from(rank).expect("a window fits a u32");

        for (x, cell) in out_row.iter_mut().enumerate() {
            if x > 0 {
                let x = x.cast_signed();
                for value in column(wrap(x - half.cast_signed() - 1)) {
                    histogram[usize::from(value)] -= 1;
                    below -= u32::from(usize::from(value) < level);
                }
                for value in column(wrap(x + half.cast_signed())) {
                    histogram[usize::from(value)] += 1;
                    below += u32::from(usize::from(value) < level);
                }
            }
            while below > rank {
                level -= 1;
                below -= histogram[level];
            }
            while below + histogram[level] <= rank {
                below += histogram[level];
                level += 1;
            }
            *cell = level as f32 / 255.0;
        }
    }
    out
}

/// A Gaussian blur of [`SMOOTH_SIGMA`], wrapping along the rows and clamping
/// along the columns.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    reason = "a kernel radius of a few pixels"
)]
fn blur(grid: &[f32]) -> Vec<f32> {
    let radius = (SMOOTH_TRUNCATE * SMOOTH_SIGMA + 0.5) as isize;
    let mut kernel: Vec<f32> = (-radius..=radius)
        .map(|offset| (-0.5 * (offset as f32 / SMOOTH_SIGMA).powi(2)).exp())
        .collect();
    let total: f32 = kernel.iter().sum();
    for weight in &mut kernel {
        *weight /= total;
    }

    let (width, height) = (GRID_WIDTH as isize, GRID_HEIGHT as isize);
    let mut across = vec![0.0f32; grid.len()];
    for (row, out) in grid
        .chunks_exact(GRID_WIDTH)
        .zip(across.chunks_exact_mut(GRID_WIDTH))
    {
        for (x, cell) in out.iter_mut().enumerate() {
            *cell = kernel
                .iter()
                .zip(-radius..)
                .map(|(weight, offset)| {
                    weight * row[(x as isize + offset).rem_euclid(width) as usize]
                })
                .sum();
        }
    }

    let mut down = vec![0.0f32; grid.len()];
    for (y, out) in down.chunks_exact_mut(GRID_WIDTH).enumerate() {
        out.fill(0.0);
        for (weight, offset) in kernel.iter().zip(-radius..) {
            let source = (y as isize + offset).clamp(0, height - 1) as usize;
            let row = &across[source * GRID_WIDTH..(source + 1) * GRID_WIDTH];
            for (cell, &value) in out.iter_mut().zip(row) {
                *cell += weight * value;
            }
        }
    }
    down
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TEXTURE_RESOLUTIONS;

    fn map(width: u32, height: u32, pixel: impl Fn(u32, u32) -> u8) -> DecodedImage {
        let pixels = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .map(|(x, y)| pixel(x, y))
            .collect();
        DecodedImage {
            pixels: Pixels::new(pixels),
            width,
            height,
            channels: Channels::Red,
        }
    }

    fn uniform(width: u32, height: u32, value: u8) -> DecodedImage {
        map(width, height, |_, _| value)
    }

    /// Bright ones included: the cap is the shader's, so the estimate keeps
    /// what polar ground reads above it.
    #[test]
    fn a_uniform_map_is_its_own_background() {
        for level in [100, 240, 255] {
            let estimate = background(&uniform(1024, 512, level));
            assert!(
                estimate.pixels.iter().all(|&value| value == level),
                "{level}"
            );
        }
    }

    /// Dense cloud over half the map, in patches smaller than the window, over
    /// a haze level: the estimate finds the haze.
    #[test]
    fn dense_patches_smaller_than_the_window_leave_the_haze_level() {
        let haze = 90;
        // Cells of 4 grid pixels at 2 map pixels a grid pixel.
        let patch = 8;
        let estimate = background(&map(1024, 512, |x, y| {
            if (x / patch + y / patch) % 2 == 0 {
                255
            } else {
                haze
            }
        }));
        for &value in estimate.pixels.iter() {
            assert!(
                value.abs_diff(haze) <= 1,
                "a background of {value} over a haze of {haze}"
            );
        }
    }

    /// Rotating the map by half its width rotates the background by half its
    /// width, which holds only when every filter wraps.
    #[test]
    fn the_estimate_wraps_across_the_antimeridian() {
        let mut state = 0x2545_f491_u32;
        let mut noise = || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        let (width, height) = (1024u32, 512u32);
        let source: Vec<u8> = (0..width * height)
            .map(|_| u8::try_from(noise() >> 24).expect("a byte"))
            .collect();
        let at = |x: u32, y: u32| source[(y * width + x) as usize];
        let original = background(&map(width, height, at));
        let rotated = background(&map(width, height, |x, y| at((x + width / 2) % width, y)));

        for y in 0..GRID_HEIGHT {
            for x in 0..GRID_WIDTH {
                let turned = (x + GRID_WIDTH / 2) % GRID_WIDTH;
                assert_eq!(
                    rotated.pixels[y * GRID_WIDTH + x],
                    original.pixels[y * GRID_WIDTH + turned],
                    "at {x}, {y}"
                );
            }
        }
    }

    /// The poles are clamped rather than wrapped: a band of haze along the top
    /// edge stays there and does not reach the bottom.
    #[test]
    fn the_estimate_clamps_at_the_poles() {
        let estimate = background(&map(1024, 512, |_, y| if y < 64 { 200 } else { 0 }));
        let row = |y: usize| estimate.pixels[y * GRID_WIDTH];
        assert!(row(0) > 150, "the top row is {}", row(0));
        assert_eq!(row(GRID_HEIGHT - 1), 0);
    }

    #[test]
    fn every_size_comes_to_the_grid() {
        let sizes = TEXTURE_RESOLUTIONS
            .iter()
            .map(|&width| (width, width / 2))
            .chain([(1000, 500), (300, 150)]);
        for (width, height) in sizes {
            let estimate = background(&uniform(width, height, 60));
            assert_eq!(
                (estimate.width as usize, estimate.height as usize),
                (GRID_WIDTH, GRID_HEIGHT),
                "{width} x {height}"
            );
            assert_eq!(estimate.pixels.len(), GRID_WIDTH * GRID_HEIGHT);
            assert!(
                estimate.pixels.iter().all(|&value| value == 60),
                "{width} x {height}: an area average of a uniform map is uniform"
            );
        }
    }

    #[test]
    fn box_weights_cover_each_cell_once() {
        for (from, to) in [(8192, 512), (1000, 512), (300, 512), (7, 3)] {
            let weights = box_weights(from, to);
            assert_eq!(weights.len(), to);
            for (index, (first, cell)) in weights.iter().enumerate() {
                let total: f32 = cell.iter().sum();
                approx::assert_relative_eq!(total, 1.0, epsilon = 1e-5);
                assert!(first + cell.len() <= from, "{from} to {to}: cell {index}");
            }
        }
    }
}
