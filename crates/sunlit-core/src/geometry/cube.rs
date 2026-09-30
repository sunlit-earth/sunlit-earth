//! The equi-angular cube the surface textures are baked on.
//!
//! The cube is the world frame with no rotation about the pole: +Y north, +Z
//! longitude 0, +X longitude 90 E. Faces follow the OpenGL and Direct3D cube map
//! table and are numbered in cube layer order, +X, -X, +Y, -Y, +Z, -Z. A face is
//! addressed by warped coordinates `s` along a row and `t` down a column, each
//! -1 to 1 across the face; the tangent warp `tan(s * pi / 4)` turns them into
//! the plain cube face coordinates the table combines into a direction, so
//! every texel spans the same angle along a face's axes.
//!
//! Warped coordinates beyond -1 to 1 keep their meaning up to 2, where the
//! angle from the face center reaches 90 degrees: the direction continues onto
//! the neighboring face, which is how a tile's gutter finds its texels there.

use std::f64::consts::FRAC_PI_4;

use glam::DVec3;

/// The faces of a cube.
pub const FACES: usize = 6;

/// The direction through warped coordinates `(s, t)` of `face`, not normalized.
///
/// # Panics
///
/// If `face` is not below [`FACES`].
#[must_use]
pub fn direction(face: usize, s: f64, t: f64) -> DVec3 {
    let u = (s * FRAC_PI_4).tan();
    let v = (t * FRAC_PI_4).tan();
    match face {
        0 => DVec3::new(1.0, -v, -u),
        1 => DVec3::new(-1.0, -v, u),
        2 => DVec3::new(u, 1.0, v),
        3 => DVec3::new(u, -1.0, -v),
        4 => DVec3::new(u, -v, 1.0),
        5 => DVec3::new(-u, -v, -1.0),
        _ => panic!("a cube has {FACES} faces, not {}", face + 1),
    }
}

/// The face a direction lands on, and its warped coordinates there.
///
/// A direction exactly on an edge between two faces takes the first of them in
/// the order X, Y, Z, as the table's selection does. `dir` must not be zero.
#[must_use]
pub fn locate(dir: DVec3) -> (usize, f64, f64) {
    let a = dir.abs();
    let (face, sc, tc, major) = if a.x >= a.y && a.x >= a.z {
        if dir.x > 0.0 {
            (0, -dir.z, -dir.y, a.x)
        } else {
            (1, dir.z, -dir.y, a.x)
        }
    } else if a.y >= a.z {
        if dir.y > 0.0 {
            (2, dir.x, dir.z, a.y)
        } else {
            (3, dir.x, -dir.z, a.y)
        }
    } else if dir.z > 0.0 {
        (4, dir.x, -dir.y, a.z)
    } else {
        (5, -dir.x, -dir.y, a.z)
    };
    (
        face,
        (sc / major).atan() / FRAC_PI_4,
        (tc / major).atan() / FRAC_PI_4,
    )
}

/// The warped coordinate of the center of texel `index` on a face `size`
/// texels wide. An index outside the face gives a coordinate beyond -1 to 1.
#[must_use]
pub fn texel_center(index: i64, size: u32) -> f64 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "a texel index is far inside the f64 mantissa"
    )]
    let index = index as f64;
    (index + 0.5) / f64::from(size) * 2.0 - 1.0
}

/// The continuous texel coordinate of warped coordinate `w` on a face `size`
/// texels wide, with texel centers at whole numbers: the inverse of
/// [`texel_center`].
#[must_use]
pub fn texel_coordinate(w: f64, size: u32) -> f64 {
    f64::midpoint(w, 1.0) * f64::from(size) - 0.5
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    /// Longitude and latitude in degrees, east and north positive.
    fn lon_lat(dir: DVec3) -> (f64, f64) {
        (
            dir.x.atan2(dir.z).to_degrees(),
            dir.y.atan2(dir.x.hypot(dir.z)).to_degrees(),
        )
    }

    #[test]
    fn every_face_center_points_along_its_axis() {
        let axes = [
            DVec3::X,
            DVec3::NEG_X,
            DVec3::Y,
            DVec3::NEG_Y,
            DVec3::Z,
            DVec3::NEG_Z,
        ];
        for (face, axis) in axes.into_iter().enumerate() {
            assert_eq!(direction(face, 0.0, 0.0), axis, "face {face}");
            assert_eq!(locate(axis), (face, 0.0, 0.0), "face {face}");
        }
    }

    /// The frame the bake uses: Africa upright on +Z, north up on the side
    /// faces, east to the right, and the Arctic on +Y with longitude 0 toward
    /// its bottom row.
    #[test]
    fn the_faces_sit_where_the_bake_put_them() {
        let (lon, lat) = lon_lat(direction(4, 0.0, -0.5));
        assert_relative_eq!(lon, 0.0);
        assert!(lat > 20.0, "row 0 of +Z is north: {lat}");
        let (lon, _) = lon_lat(direction(4, 0.5, 0.0));
        assert!(lon > 20.0, "the last column of +Z is east: {lon}");

        let (lon, lat) = lon_lat(direction(0, 0.0, 0.0));
        assert_relative_eq!(lon, 90.0);
        assert_relative_eq!(lat, 0.0);
        let (lon, _) = lon_lat(direction(1, 0.0, 0.0));
        assert_relative_eq!(lon, -90.0);
        let (lon, _) = lon_lat(direction(5, 0.0, 0.0));
        assert_relative_eq!(lon.abs(), 180.0);

        let (lon, lat) = lon_lat(direction(2, 0.0, 0.5));
        assert_relative_eq!(lon, 0.0);
        assert!(lat > 45.0 && lat < 90.0, "{lat}");
        let (_, lat) = lon_lat(direction(3, 0.0, 0.0));
        assert_relative_eq!(lat, -90.0);
    }

    #[test]
    fn locate_inverts_direction_inside_every_face() {
        for face in 0..FACES {
            for (s, t) in [(-0.9, 0.3), (0.0, 0.99), (0.5, -0.5), (-0.99, -0.99)] {
                let (found, fs, ft) = locate(direction(face, s, t) * 3.0);
                assert_eq!(found, face);
                assert_relative_eq!(fs, s, epsilon = 1e-12);
                assert_relative_eq!(ft, t, epsilon = 1e-12);
            }
        }
    }

    /// Past the edge of a face the direction continues onto its neighbor, at
    /// the angle the warped coordinate says: a step of one texel across the
    /// edge is one texel's angle on either side of it.
    #[test]
    fn a_coordinate_past_the_edge_lands_on_the_neighbor_at_the_same_spacing() {
        let size = 64;
        let inside = direction(4, texel_center(63, size), 0.0);
        let outside = direction(4, texel_center(64, size), 0.0);
        let (face, s, t) = locate(outside);
        assert_eq!(face, 0, "right of +Z is +X");
        assert_relative_eq!(texel_coordinate(s, size), 0.0, epsilon = 1e-9);
        assert_relative_eq!(t, 0.0);
        assert_relative_eq!(
            inside.angle_between(outside),
            std::f64::consts::FRAC_PI_2 / f64::from(size),
            epsilon = 1e-12
        );
    }

    #[test]
    fn texel_centers_and_coordinates_are_inverse() {
        for size in [1, 16, 2048] {
            for index in [-8, 0, 7, i64::from(size) + 3] {
                #[expect(clippy::cast_precision_loss, reason = "small test indices")]
                let expected = index as f64;
                assert_relative_eq!(
                    texel_coordinate(texel_center(index, size), size),
                    expected,
                    epsilon = 1e-9
                );
            }
        }
        assert_relative_eq!(texel_center(0, 2), -0.5);
        assert_relative_eq!(texel_center(1, 2), 0.5);
    }
}
