//! Generate an equirectangular grid texture as RGBA8 pixel data.
//!
//! The texture maps longitude (0–360°) along the X axis and latitude (0–180°)
//! along the Y axis. Grid lines are drawn every `spacing` degrees, with
//! thicker highlighted lines at the equator and prime meridian.

const GRID_SPACING: f32 = 15.0;
const LINE_WIDTH: f32 = 0.5; // degrees
const MAJOR_LINE_WIDTH: f32 = 0.8;

// Colors (RGBA8, stored without gamma conversion via Rgba8Unorm format)
const OCEAN_BLUE: [u8; 3] = [60, 110, 200];
const LAND_GREEN: [u8; 3] = [80, 170, 110];
const GRID_WHITE: [u8; 3] = [204, 204, 204];
const MAJOR_YELLOW: [u8; 3] = [255, 230, 77];

#[allow(clippy::cast_precision_loss)]
pub(crate) fn generate(width: u32, height: u32) -> Vec<u8> {
    let mut pixels = vec![0u8; (width * height * 4) as usize];

    for y in 0..height {
        let lat_deg = (y as f32 / height as f32) * 180.0;
        let latitude_factor = 1.0 - lat_deg / 180.0;

        for x in 0..width {
            let lon_deg = (x as f32 / width as f32) * 360.0;

            // Distance to nearest grid line
            let lon_dist = lon_deg % GRID_SPACING;
            let lon_line = lon_dist.min(GRID_SPACING - lon_dist);
            let lat_dist = lat_deg % GRID_SPACING;
            let lat_line = lat_dist.min(GRID_SPACING - lat_dist);

            let is_grid = lon_line < LINE_WIDTH || lat_line < LINE_WIDTH;

            // Major lines: equator (lat=90°) and prime meridian (lon=0°/360°)
            let equator_dist = (lat_deg - 90.0).abs();
            let pm_dist = lon_deg.min((lon_deg - 360.0).abs());
            let is_major = equator_dist < MAJOR_LINE_WIDTH || pm_dist < MAJOR_LINE_WIDTH;

            // Base color: blend between ocean and land by latitude
            let t = latitude_factor * 0.5 + 0.25;
            let base = lerp_color(OCEAN_BLUE, LAND_GREEN, t);

            let color = if is_major {
                MAJOR_YELLOW
            } else if is_grid {
                GRID_WHITE
            } else {
                base
            };

            let idx = ((y * width + x) * 4) as usize;
            pixels[idx..idx + 4].copy_from_slice(&[color[0], color[1], color[2], 255]);
        }
    }

    pixels
}

fn lerp_color(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    [
        lerp_u8(a[0], b[0], t),
        lerp_u8(a[1], b[1], t),
        lerp_u8(a[2], b[2], t),
    ]
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (f32::from(a) + (f32::from(b) - f32::from(a)) * t) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_has_correct_size() {
        let pixels = generate(64, 32);
        assert_eq!(pixels.len(), 64 * 32 * 4);
    }

    #[test]
    fn all_pixels_are_opaque() {
        let pixels = generate(64, 32);
        for chunk in pixels.chunks(4) {
            assert_eq!(chunk[3], 255, "alpha should be 255");
        }
    }

    #[test]
    fn contains_grid_lines() {
        // At lon=0° (x=0) we should see the prime meridian (major yellow)
        let width = 360;
        let height = 180;
        let pixels = generate(width, height);
        // Check pixel at (0, 90) — prime meridian at equator
        let idx = (90 * width * 4) as usize;
        assert_eq!(pixels[idx], MAJOR_YELLOW[0]);
        assert_eq!(pixels[idx + 1], MAJOR_YELLOW[1]);
        assert_eq!(pixels[idx + 2], MAJOR_YELLOW[2]);
    }

    /// Helper: get the RGB color of a pixel at (x, y) in a 360x180 grid.
    fn pixel_rgb(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 3] {
        let idx = ((y * width + x) * 4) as usize;
        [pixels[idx], pixels[idx + 1], pixels[idx + 2]]
    }

    #[test]
    fn equator_is_major_yellow() {
        // 360x180: equator is at lat_deg=90, so y=90
        let pixels = generate(360, 180);
        // Pick a pixel on the equator away from the prime meridian
        // x=180 -> lon_deg=180, which is not a major line
        let color = pixel_rgb(&pixels, 360, 180, 90);
        assert_eq!(color, MAJOR_YELLOW, "Equator should be MAJOR_YELLOW");
    }

    #[test]
    fn prime_meridian_is_major_yellow() {
        // x=0 -> lon_deg=0 (prime meridian)
        // y=45 -> away from equator
        let pixels = generate(360, 180);
        let color = pixel_rgb(&pixels, 360, 0, 45);
        assert_eq!(color, MAJOR_YELLOW, "Prime meridian should be MAJOR_YELLOW");
    }

    #[test]
    fn minor_grid_line_is_white() {
        // 15-degree minor line: lon_deg=15 -> x=15 in a 360-wide texture
        // At y=45 (lat_deg=45), which is also a 15-degree grid line
        // But lat_deg=45 is not 90 (equator), not 0 or 180 (pm), so it's minor.
        // Actually lat_deg=45 is a minor grid line too. We just need lon or lat
        // on a 15-degree multiple that isn't major.
        let pixels = generate(360, 180);
        // x=15 -> lon_deg=15 (minor), y=60 -> lat_deg=60 (minor)
        // This pixel is at a grid line intersection.
        let color = pixel_rgb(&pixels, 360, 15, 60);
        assert_eq!(
            color, GRID_WHITE,
            "15-degree grid line should be GRID_WHITE"
        );
    }

    #[test]
    fn pixel_between_grid_lines_is_base_color() {
        // Pick a pixel clearly between grid lines
        // x=100 -> lon_deg=100 (100%15=10, not near a line)
        // y=50 -> lat_deg=50 (50%15=5, not near a line)
        let pixels = generate(360, 180);
        let color = pixel_rgb(&pixels, 360, 100, 50);
        assert_ne!(color, MAJOR_YELLOW, "Should not be major yellow");
        assert_ne!(color, GRID_WHITE, "Should not be grid white");
        // Should be a blend of OCEAN_BLUE and LAND_GREEN — not a grid line color
        // Verify the pixel alpha is 255 (opaque)
        let idx = ((50 * 360 + 100) * 4) as usize;
        assert_eq!(pixels[idx + 3], 255);
    }
}
