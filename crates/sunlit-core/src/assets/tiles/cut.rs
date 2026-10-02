//! Cube faces in memory, and the windows tiles are cut from.
//!
//! A window is a square of texels on one face's grid that may reach past the
//! face's edges. Inside the face it is the face's own texels; outside, each
//! texel is the neighboring face's color at the same point on the sphere,
//! sampled bilinearly there through the cube's direction mapping. That is what
//! a tile's gutter has to hold for the sampling across a face edge to be
//! continuous: the shader extends a face's coordinates past its edge exactly
//! as [`cube::texel_center`] does.

use crate::assets::texture_loader::downsample_2x;
use crate::geometry::cube::{self, FACES};

/// One face of one level: `size` texels square, `channels` bytes each, row 0
/// first in memory.
pub(crate) struct Plane {
    pub size: u32,
    pub channels: usize,
    pub texels: Vec<u8>,
}

impl Plane {
    fn texel(&self, row: usize, col: usize) -> &[u8] {
        let at = (row * self.size as usize + col) * self.channels;
        &self.texels[at..at + self.channels]
    }

    /// Half the size, by the box filter the mip chain uses.
    pub fn halved(&self) -> Self {
        let texels = if self.channels == 4 {
            downsample_2x(&self.texels, self.size, self.size)
        } else {
            halve_channels(&self.texels, self.size, self.channels)
        };
        Self {
            size: (self.size / 2).max(1),
            channels: self.channels,
            texels,
        }
    }
}

/// [`downsample_2x`] for any number of channels.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the mean of four bytes is a byte"
)]
fn halve_channels(src: &[u8], size: u32, channels: usize) -> Vec<u8> {
    let s = size as usize;
    let d = (s / 2).max(1);
    let mut dst = vec![0_u8; d * d * channels];
    for y in 0..d {
        for x in 0..d {
            let (x0, y0) = (x * 2, y * 2);
            let (x1, y1) = ((x0 + 1).min(s - 1), (y0 + 1).min(s - 1));
            for c in 0..channels {
                let at = |xx: usize, yy: usize| u16::from(src[(yy * s + xx) * channels + c]);
                let sum = at(x0, y0) + at(x1, y0) + at(x0, y1) + at(x1, y1);
                dst[(y * d + x) * channels + c] = ((sum + 2) / 4) as u8;
            }
        }
    }
    dst
}

/// Six planes of one size and channel count, in cube layer order.
pub(crate) struct Cube {
    faces: Vec<Plane>,
}

impl Cube {
    pub fn new(faces: Vec<Plane>) -> Result<Self, String> {
        let Some(first) = faces.first() else {
            return Err("a cube needs its faces".to_owned());
        };
        let (size, channels) = (first.size, first.channels);
        if faces.len() != FACES {
            return Err(format!("a cube has {FACES} faces, not {}", faces.len()));
        }
        for (index, face) in faces.iter().enumerate() {
            if face.size != size || face.channels != channels {
                return Err(format!(
                    "face {index} is {} px with {} channels, face 0 is {size} px with {channels}",
                    face.size, face.channels
                ));
            }
            if face.texels.len() != size as usize * size as usize * channels {
                return Err(format!("face {index} does not hold {size} x {size} texels"));
            }
        }
        Ok(Self { faces })
    }

    pub fn size(&self) -> u32 {
        self.faces[0].size
    }

    pub fn channels(&self) -> usize {
        self.faces[0].channels
    }

    pub fn face(&self, face: usize) -> &Plane {
        &self.faces[face]
    }

    pub fn halved(&self) -> Self {
        Self {
            faces: self.faces.iter().map(Plane::halved).collect(),
        }
    }

    /// The square window of `size` texels whose first texel is at `row` and
    /// `col` of `face`, either of which may lie outside the face.
    pub fn window(&self, face: usize, row: i64, col: i64, size: u32) -> Vec<u8> {
        let n = self.size() as usize;
        let channels = self.channels();
        let width = size as usize;
        let mut out = vec![0_u8; width * width * channels];
        let plane = self.face(face);
        let inside = |i: i64| usize::try_from(i).ok().filter(|&i| i < n);
        for (line, r) in out.chunks_exact_mut(width * channels).zip(row..) {
            for (texel, c) in line.chunks_exact_mut(channels).zip(col..) {
                match (inside(r), inside(c)) {
                    (Some(r), Some(c)) => texel.copy_from_slice(plane.texel(r, c)),
                    _ => self.sample_past_edge(face, r, c, texel),
                }
            }
        }
        out
    }

    /// The color at texel `(row, col)` of `face`'s grid extended past its
    /// edges, from whichever face the direction through it lands on.
    fn sample_past_edge(&self, face: usize, row: i64, col: i64, out: &mut [u8]) {
        let n = self.size();
        let dir = cube::direction(face, cube::texel_center(col, n), cube::texel_center(row, n));
        let (onto, s, t) = cube::locate(dir);
        self.bilinear(
            onto,
            cube::texel_coordinate(t, n),
            cube::texel_coordinate(s, n),
            out,
        );
    }

    /// Bilinear sample of `face` at continuous texel coordinates, clamped to
    /// the face.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the coordinates are clamped to the face, and a blend of bytes rounds to a byte"
    )]
    fn bilinear(&self, face: usize, row: f64, col: f64, out: &mut [u8]) {
        let last = f64::from(self.size() - 1);
        let (y, x) = (row.clamp(0.0, last), col.clamp(0.0, last));
        let (y0, x0) = (y.floor(), x.floor());
        let (fy, fx) = (y - y0, x - x0);
        let (y0, x0) = (y0 as usize, x0 as usize);
        let top = self.size() as usize - 1;
        let (y1, x1) = ((y0 + 1).min(top), (x0 + 1).min(top));
        let plane = self.face(face);
        let (above, below) = (
            [plane.texel(y0, x0), plane.texel(y0, x1)],
            [plane.texel(y1, x0), plane.texel(y1, x1)],
        );
        let blend = |pair: [&[u8]; 2], k: usize| {
            f64::from(pair[0][k]) * (1.0 - fx) + f64::from(pair[1][k]) * fx
        };
        for (k, value) in out.iter_mut().enumerate() {
            let (upper, lower) = (blend(above, k), blend(below, k));
            *value = (upper * (1.0 - fy) + lower * fy).round() as u8;
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use glam::DVec3;

    /// A cube whose texels are a smooth function of their direction, filled
    /// through a mapping written out here from the cube map table rather than
    /// through [`cube::direction`], so the two check each other.
    pub fn smooth_cube(size: u32) -> Cube {
        let faces = (0..FACES)
            .map(|face| {
                let mut texels = Vec::new();
                for r in 0..size {
                    for c in 0..size {
                        let dir = table_direction(
                            face,
                            cube::texel_center(i64::from(c), size),
                            cube::texel_center(i64::from(r), size),
                        );
                        texels.extend(smooth_color(dir));
                    }
                }
                Plane {
                    size,
                    channels: 4,
                    texels,
                }
            })
            .collect();
        Cube::new(faces).expect("a whole cube")
    }

    /// The cube map table: the face's major axis and how `sc` and `tc` run.
    fn table_direction(face: usize, s: f64, t: f64) -> DVec3 {
        let (sc, tc) = (
            (s * std::f64::consts::FRAC_PI_4).tan(),
            (t * std::f64::consts::FRAC_PI_4).tan(),
        );
        let (major, s_axis, t_axis) = [
            (DVec3::X, DVec3::NEG_Z, DVec3::NEG_Y),
            (DVec3::NEG_X, DVec3::Z, DVec3::NEG_Y),
            (DVec3::Y, DVec3::X, DVec3::Z),
            (DVec3::NEG_Y, DVec3::X, DVec3::NEG_Z),
            (DVec3::Z, DVec3::X, DVec3::NEG_Y),
            (DVec3::NEG_Z, DVec3::NEG_X, DVec3::NEG_Y),
        ][face];
        major + s_axis * sc + t_axis * tc
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "each channel is scaled into a byte"
    )]
    pub fn smooth_color(dir: DVec3) -> [u8; 4] {
        let n = dir.normalize();
        let byte = |v: f64| ((v * 0.5 + 0.5) * 255.0).round() as u8;
        [byte(n.x), byte(n.y), byte(n.z), 255]
    }

    /// The face grid row and column of texel `index` of a window `width`
    /// wide that starts `reach` texels before the face's first.
    fn grid_position(index: usize, width: u32, reach: u32) -> (i64, i64) {
        let at = |i: usize| i64::try_from(i).expect("a small index") - i64::from(reach);
        (at(index / width as usize), at(index % width as usize))
    }

    fn constant_faces(size: u32) -> Cube {
        let faces = (0..FACES)
            .map(|face| Plane {
                size,
                channels: 1,
                texels: vec![u8::try_from(face * 40 + 10).expect("a byte"); (size * size) as usize],
            })
            .collect();
        Cube::new(faces).expect("a whole cube")
    }

    #[test]
    fn a_window_inside_the_face_is_the_faces_own_texels() {
        let cube = smooth_cube(16);
        let window = cube.window(4, 3, 5, 6);
        for y in 0..6 {
            for x in 0..6 {
                let at = (y * 6 + x) * 4;
                assert_eq!(&window[at..at + 4], cube.face(4).texel(3 + y, 5 + x));
            }
        }
    }

    /// Faces of one value each: every texel past an edge or a corner holds
    /// the value of the face the table selects for its direction, exactly,
    /// since a blend of equal texels is that texel.
    #[test]
    fn past_an_edge_the_window_holds_the_neighbor_the_direction_lands_on() {
        let size = 8;
        let cube = constant_faces(size);
        let reach = 3;
        let width = size + 2 * reach;
        for face in 0..FACES {
            let window = cube.window(face, -i64::from(reach), -i64::from(reach), width);
            for (index, &value) in window.iter().enumerate() {
                let (r, c) = grid_position(index, width, reach);
                let dir = table_direction(
                    face,
                    cube::texel_center(c, size),
                    cube::texel_center(r, size),
                );
                let a = dir.abs();
                let expected = if a.x >= a.y && a.x >= a.z {
                    usize::from(dir.x < 0.0)
                } else if a.y >= a.z {
                    2 + usize::from(dir.y < 0.0)
                } else {
                    4 + usize::from(dir.z < 0.0)
                };
                assert_eq!(
                    value,
                    cube.face(expected).texel(0, 0)[0],
                    "face {face} texel ({r}, {c})"
                );
            }
        }
    }

    /// On a cube whose color follows the direction, a gutter texel holds the
    /// color of its own direction to within the blend's error, however far
    /// past the edge it is. Repeating the edge texel instead would be off by
    /// several levels a texel out and by tens at the gutter's end.
    #[test]
    fn a_gutter_texel_holds_the_color_of_its_direction() {
        let size = 64;
        let cube = smooth_cube(size);
        let reach = 8;
        let width = size + 2 * reach;
        let mut worst = 0;
        let mut clamped_worst = 0;
        for face in 0..FACES {
            let window = cube.window(face, -i64::from(reach), -i64::from(reach), width);
            for (index, texel) in window.chunks(4).enumerate() {
                let (r, c) = grid_position(index, width, reach);
                let expected = smooth_color(table_direction(
                    face,
                    cube::texel_center(c, size),
                    cube::texel_center(r, size),
                ));
                for k in 0..3 {
                    worst = worst.max(texel[k].abs_diff(expected[k]));
                }
                let edge = cube.face(face).texel(
                    usize::try_from(r.clamp(0, i64::from(size) - 1)).expect("clamped"),
                    usize::try_from(c.clamp(0, i64::from(size) - 1)).expect("clamped"),
                );
                for k in 0..3 {
                    clamped_worst = clamped_worst.max(edge[k].abs_diff(expected[k]));
                }
            }
        }
        assert!(worst <= 2, "worst channel error in the gutter {worst}");
        assert!(
            clamped_worst > 10,
            "the check has to be able to tell a repeated edge: {clamped_worst}"
        );
    }

    #[test]
    fn halving_one_channel_is_halving_rgba_one_channel_at_a_time() {
        let size = 6;
        let gray: Vec<u8> = (0..size * size)
            .map(|i| u8::try_from(i * 7 % 256).unwrap())
            .collect();
        let rgba: Vec<u8> = gray.iter().flat_map(|&g| [g, g, g, 255]).collect();
        let halved = halve_channels(&gray, size, 1);
        let expected: Vec<u8> = downsample_2x(&rgba, size, size)
            .chunks(4)
            .map(|t| t[0])
            .collect();
        assert_eq!(halved, expected);
    }

    #[test]
    fn a_cube_refuses_faces_that_do_not_match() {
        let face = |size: u32| Plane {
            size,
            channels: 1,
            texels: vec![0; (size * size) as usize],
        };
        assert!(Cube::new((0..5).map(|_| face(4)).collect()).is_err());
        let mut faces: Vec<_> = (0..6).map(|_| face(4)).collect();
        faces[3] = face(8);
        assert!(Cube::new(faces).is_err());
    }
}
