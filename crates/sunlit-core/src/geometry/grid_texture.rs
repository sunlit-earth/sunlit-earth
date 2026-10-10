//! Generate the procedural grid the globe shows without a surface texture, as
//! RGBA8 cube faces.
//!
//! The grid is laid out on the equirectangular coordinates the sphere mesh
//! carries: longitude 0 to 360 degrees along them, colatitude 0 to 180 degrees
//! down them. Grid lines are drawn every `GRID_SPACING` degrees, with thicker
//! highlighted lines at the equator and at the mesh's longitude 0. Each cube
//! texel takes the color of the place its direction has on the mesh, so the
//! globe looks the way it did when the mesh's coordinates sampled a flat grid,
//! except at the poles, which are now ordinary points.

use std::f64::consts::{PI, TAU};

use super::cube;

const GRID_SPACING: f32 = 15.0;
const LINE_WIDTH: f32 = 0.5; // degrees
const MAJOR_LINE_WIDTH: f32 = 0.8;

// Colors (RGBA8, stored without gamma conversion via Rgba8Unorm format)
const OCEAN_BLUE: [u8; 3] = [60, 110, 200];
const LAND_GREEN: [u8; 3] = [80, 170, 110];
const GRID_WHITE: [u8; 3] = [204, 204, 204];
const MAJOR_YELLOW: [u8; 3] = [255, 230, 77];

/// The grid's color at mesh longitude `lon_deg` (0 to 360) and colatitude
/// `lat_deg` (0 at the north pole, 180 at the south), for a texel `texel_deg`
/// degrees of arc wide.
///
/// A line covers a texel in proportion to how far into it the line reaches,
/// over a ramp one texel wide. The lines cross the cube's texel grid at every
/// angle, so a texel that is either on a line or off it would draw them as
/// staircases where the flat grid's lines ran along its rows and columns.
fn color_at(lon_deg: f32, lat_deg: f32, texel_deg: f32) -> [u8; 3] {
    // A degree of longitude is sin(colatitude) degrees of arc.
    let lon_texel = (texel_deg / lat_deg.to_radians().sin().max(1e-3)).min(360.0);
    let covered = |distance: f32, half_width: f32, texel: f32| {
        ((half_width - distance) / texel + 0.5).clamp(0.0, 1.0)
    };

    let lon_dist = lon_deg % GRID_SPACING;
    let lon_line = lon_dist.min(GRID_SPACING - lon_dist);
    let lat_dist = lat_deg % GRID_SPACING;
    let lat_line = lat_dist.min(GRID_SPACING - lat_dist);
    let minor =
        covered(lon_line, LINE_WIDTH, lon_texel).max(covered(lat_line, LINE_WIDTH, texel_deg));

    let equator_dist = (lat_deg - 90.0).abs();
    let pm_dist = lon_deg.min((lon_deg - 360.0).abs());
    let major = covered(equator_dist, MAJOR_LINE_WIDTH, texel_deg).max(covered(
        pm_dist,
        MAJOR_LINE_WIDTH,
        lon_texel,
    ));

    // Between the lines, a blend of ocean and land by latitude, greener to
    // the north.
    let base = lerp_color(OCEAN_BLUE, LAND_GREEN, (1.0 - lat_deg / 180.0) * 0.5 + 0.25);
    lerp_color(lerp_color(base, GRID_WHITE, minor), MAJOR_YELLOW, major)
}

/// Face `face` of the grid, `size` texels wide, row 0 first, in cube layer
/// order and at the texel centers the surface faces use.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "angles in degrees are far inside the f32 range, and a face width inside its mantissa"
)]
pub(crate) fn generate_cube_face(face: usize, size: u32) -> Vec<u8> {
    // An equi-angular face spans 90 degrees along each axis in even steps.
    let texel_deg = 90.0 / size as f32;
    let mut pixels = Vec::with_capacity(size as usize * size as usize * 4);
    for row in 0..i64::from(size) {
        let t = cube::texel_center(row, size);
        for col in 0..i64::from(size) {
            let dir = cube::direction(face, cube::texel_center(col, size), t).normalize();
            // The sphere mesh's own coordinates: `u` turns from +X toward +Z,
            // `v` runs from the north pole down.
            let lon = dir.z.atan2(dir.x).rem_euclid(TAU) / TAU * 360.0;
            let lat = dir.y.clamp(-1.0, 1.0).acos() / PI * 180.0;
            let [r, g, b] = color_at(lon as f32, lat as f32, texel_deg);
            pixels.extend_from_slice(&[r, g, b, 255]);
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

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "an interpolation between two bytes stays between them"
)]
fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (f32::from(a) + (f32::from(b) - f32::from(a)) * t) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A texel far narrower than a line.
    const FINE: f32 = 0.01;

    /// The color of the texel at `(row, col)` of a face.
    fn texel(pixels: &[u8], size: u32, row: u32, col: u32) -> [u8; 4] {
        let at = ((row * size + col) * 4) as usize;
        pixels[at..at + 4].try_into().expect("four bytes")
    }

    #[test]
    fn a_face_is_square_and_opaque() {
        for face in 0..cube::FACES {
            let pixels = generate_cube_face(face, 16);
            assert_eq!(pixels.len(), 16 * 16 * 4);
            assert!(pixels.chunks(4).all(|texel| texel[3] == 255), "face {face}");
        }
    }

    /// The mesh's longitude 0 runs through +X, so the major meridian crosses
    /// the middle of +X and the equator crosses it too, at the width a face of
    /// the shipped size draws them.
    #[test]
    fn the_major_lines_cross_at_the_middle_of_the_x_face() {
        let size = 512;
        let pixels = generate_cube_face(0, size);
        let rgb = |row, col| texel(&pixels, size, row, col)[..3].to_vec();
        assert_eq!(rgb(size / 2, size / 2), MAJOR_YELLOW, "the crossing");
        assert_eq!(rgb(4, size / 2), MAJOR_YELLOW, "the meridian, north");
        assert_eq!(rgb(size / 2, 4), MAJOR_YELLOW, "the equator, west");
    }

    #[test]
    fn the_major_lines_are_yellow_and_the_minor_ones_white() {
        assert_eq!(color_at(0.0, 45.0, FINE), MAJOR_YELLOW, "the meridian");
        assert_eq!(color_at(180.0, 90.0, FINE), MAJOR_YELLOW, "the equator");
        assert_eq!(
            color_at(15.0, 60.0, FINE),
            GRID_WHITE,
            "a crossing of minor lines"
        );
        let between = color_at(100.0, 50.0, FINE);
        assert_ne!(between, MAJOR_YELLOW);
        assert_ne!(between, GRID_WHITE);
    }

    /// A texel astride a line's edge takes part of the line's color, which is
    /// what keeps an edge that crosses the texel grid at an angle smooth.
    #[test]
    fn a_texel_on_a_lines_edge_is_partly_the_line() {
        let texel = 0.2;
        let edge = color_at(100.0, 60.0 + LINE_WIDTH, texel);
        let inside = color_at(100.0, 60.0, texel);
        let outside = color_at(100.0, 60.0 + LINE_WIDTH + texel, texel);
        assert_eq!(inside, GRID_WHITE);
        assert_ne!(outside, GRID_WHITE);
        for channel in 0..3 {
            let (low, high) = (
                inside[channel].min(outside[channel]),
                inside[channel].max(outside[channel]),
            );
            assert!(
                edge[channel] > low && edge[channel] < high,
                "channel {channel}: the edge {edge:?} is not between {inside:?} and {outside:?}"
            );
        }
    }

    /// Between the lines the grid is greener to the north, which is what tells
    /// the poles apart at a glance.
    #[test]
    fn the_north_is_greener_than_the_south() {
        // Ten degrees from each pole, between two meridians.
        let (size, row, col) = (32, 19, 17);
        let north = texel(&generate_cube_face(2, size), size, row, col);
        let south = texel(&generate_cube_face(3, size), size, row, col);
        assert!(north[1] > south[1], "north {north:?}, south {south:?}");
        assert!(north[2] < south[2], "north {north:?}, south {south:?}");
    }
}
