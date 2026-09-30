//! Which tiles the frame wants, and the finest level the page table may name
//! at each cell, computed on the CPU from the cameras that draw the frame.
//!
//! Each face's quadtree is descended from the coarsest tiled level, once per
//! output of the display plan. A tile is dropped where it lies wholly beyond
//! the horizon or outside the frame and its margin, and it is wanted where
//! the level below it, the floor below the coarsest, projects a texel to more
//! than the threshold. A wanted tile whose children in view are all wanted
//! gives way to them, so the set holds the level the rule asks for at each
//! point and not the ancestors above it.
//!
//! The projected texel is measured where the tile is nearest the camera and
//! along the texel's longest side, which is the one the anisotropic sampler's
//! level of detail follows: foreshortening near the limb shortens only the
//! other side, so the rule never counts it, and errs toward the finer level
//! wherever the sampler's anisotropy runs out, at the very limb. Every bound
//! it takes (the tile's nearest depth, its widest angle off the lens axis, the
//! warp's largest stretch over the tile) errs the same way, so the rule wants
//! at least what each pixel needs and sometimes a level more.
//!
//! Pure and deterministic: the same request gives the same set, in the same
//! order, whatever computed it before.

use std::cmp::Ordering;

use glam::{DMat4, DVec3, DVec4, Vec3};

use crate::assets::tiles::{Geometry, PackKind, TileKey};
use crate::geometry::cube::{self, FACES};
use crate::scene::camera::{CameraParams, OrbitalCamera};

use super::tiles::{CellLevels, TileId};

/// A texel of the level a cell draws may cover this many pixels before the
/// next finer level is wanted there.
pub const THRESHOLD_PX: f64 = 1.0;

/// The threshold while a drag is in progress, which halves what the frame
/// asks for at the same zoom until the drag stops.
pub const DRAG_THRESHOLD_PX: f64 = 2.0;

/// How far past the edge of the frame the margin reaches, in tiles of the
/// tile's own level.
pub const MARGIN_TILES: f64 = 1.0;

/// How far ahead of a drag its lead reaches, in seconds of the drag's rate.
pub const DRAG_LEAD_SECONDS: f32 = 0.15;

/// Cameras along the lead, evenly spaced up to its end, so a lead longer
/// than the frame leaves no gap.
pub const DRAG_LEAD_STEPS: u32 = 3;

/// One image the frame is drawn into.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Output {
    /// The camera with the output's framing applied, as the render takes it.
    pub camera: CameraParams,
    pub width: u32,
    pub height: u32,
    /// A pending wallpaper export: what it shows comes before everything else.
    pub export: bool,
}

/// Which surfaces the globe draws.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Surfaces {
    Day,
    Night,
    /// Both, weighed across the terminator as `blend_fragment` weighs them:
    /// the day where the sun's elevation term `n . l` exceeds
    /// `-terminator_width`, the night where it is below `terminator_width`.
    Blend {
        sun: Vec3,
        terminator_width: f32,
    },
}

/// A drag in progress: how fast it turns the camera.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Drag {
    /// Degrees of the camera's longitude a second.
    pub longitude_rate: f32,
    /// Degrees of the camera's latitude a second.
    pub latitude_rate: f32,
}

/// Everything one computation reads.
pub struct Request<'a> {
    pub outputs: &'a [Output],
    /// The month in force, January 0, whose pack the day's tiles come from.
    pub month: usize,
    pub surfaces: Surfaces,
    /// The finest level the resolution setting allows, [`finest_level`]; one
    /// at or below the floor's wants no tiles at all.
    pub finest: u8,
    pub drag: Option<Drag>,
    /// Whether a tile has a blob in its pack. A tile flagged constant ocean
    /// has none and is never wanted, since the page table draws it from the
    /// pack's index.
    pub stored: &'a dyn Fn(TileId) -> bool,
}

/// One tile of the wanted set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WantedTile {
    pub id: TileId,
    /// Levels between the floor and the tile's own: how much finer than what
    /// is drawn there before it arrives the tile makes its footprint.
    pub deficit: u8,
    /// Wanted for the margin or a drag's lead rather than for what is in view.
    pub margin: bool,
}

/// What one computation wants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    /// Most wanted first: every tile of a pending export in view, then every
    /// other tile in view, then the margin; within each, the largest level
    /// deficit first, then the nearest the middle of the frame. Each tile
    /// once, whichever outputs want it.
    pub tiles: Vec<WantedTile>,
    /// The finest level wanted at each cell for what is in view, at the
    /// 1 px threshold even while a drag is in progress: a tile of that level
    /// is sampled within its two levels, so a resident one serves the frame
    /// however little of the set the drag asks for.
    pub cap: CellLevels,
}

impl Wanted {
    /// The tiles the frame needs resident to be complete: the set without its
    /// margin.
    pub fn in_view(&self) -> impl Iterator<Item = TileId> + '_ {
        self.tiles
            .iter()
            .filter(|tile| !tile.margin)
            .map(|tile| tile.id)
    }
}

/// The finest level the resolution setting's label allows: 8192 the 2048
/// level, 4096 the 1024 level, 2048 the floor's alone. The label is the width
/// of the equirectangular map a face of that level matches, four faces round
/// the equator.
#[must_use]
pub fn finest_level(texture_resolution: u32) -> u8 {
    Geometry::level_of((texture_resolution / 4).max(1))
}

/// The tiles of a geometry, measured once.
pub struct Residency {
    geometry: Geometry,
    floor: u8,
    /// The tiled levels, coarse first.
    levels: Vec<Tiling>,
}

struct Tiling {
    level: u8,
    /// Tiles along each side of a face.
    side: u32,
    /// The width of a texel of the next coarser level, the floor's below the
    /// coarsest, in warped coordinates.
    coarser_texel: f64,
    /// Face after face, row after row.
    shapes: Vec<Shape>,
}

impl Tiling {
    fn index(&self, face: usize, row: u32, col: u32) -> usize {
        let side = self.side as usize;
        (face * side + row as usize) * side + col as usize
    }

    fn key(&self, index: usize) -> TileKey {
        let side = self.side as usize;
        let narrow = |value: usize| u16::try_from(value).expect("a tile index");
        TileKey {
            level: self.level,
            face: u8::try_from(index / (side * side)).expect("a face"),
            row: narrow(index / side % side),
            col: narrow(index % side),
        }
    }
}

/// Where a tile is on the sphere.
#[derive(Debug, Clone, Copy)]
struct Shape {
    center: DVec3,
    /// The largest angle from the center to a corner, gutter excluded.
    cos_radius: f64,
    sin_radius: f64,
    /// The warp's largest stretch over the tile: the longest arc a unit of
    /// warped coordinate spans there, in any direction, in radians.
    stretch: f64,
    /// The corners in order round the tile.
    corners: [DVec3; 4],
}

impl Shape {
    fn new(face: usize, s: [f64; 2], t: [f64; 2]) -> Self {
        let at = |s: f64, t: f64| cube::direction(face, s, t).normalize();
        let center = at(f64::midpoint(s[0], s[1]), f64::midpoint(t[0], t[1]));
        let corners = [
            at(s[0], t[0]),
            at(s[1], t[0]),
            at(s[1], t[1]),
            at(s[0], t[1]),
        ];
        let cos_radius = corners
            .iter()
            .map(|corner| center.dot(*corner))
            .fold(1.0, f64::min)
            .clamp(-1.0, 1.0);
        let along = |range: [f64; 2], k: i32| range[0] + (range[1] - range[0]) * f64::from(k) / 2.0;
        let mut stretch: f64 = 0.0;
        for i in 0..3 {
            for j in 0..3 {
                stretch = stretch.max(warp_stretch(face, along(s, i), along(t, j)));
            }
        }
        Self {
            center,
            cos_radius,
            sin_radius: (1.0 - cos_radius * cos_radius).sqrt(),
            stretch,
            corners,
        }
    }

    /// The cosine of the smallest angle between `dir`, a unit vector, and the
    /// tile. The tile's edges are great circle arcs, so the nearest point is
    /// inside, on an edge, or a corner.
    fn nearest_cos(&self, dir: DVec3) -> f64 {
        let mut inside = true;
        let mut best = f64::NEG_INFINITY;
        for i in 0..4 {
            let (a, b) = (self.corners[i], self.corners[(i + 1) % 4]);
            let normal = a.cross(b);
            let side = normal.dot(dir);
            if side * normal.dot(self.center) < 0.0 {
                inside = false;
            }
            best = best.max(a.dot(dir));
            let unit = normal.normalize();
            let foot = dir - unit * unit.dot(dir);
            if foot.length_squared() > 1e-24 {
                let foot = foot.normalize();
                if a.cross(foot).dot(normal) >= 0.0 && foot.cross(b).dot(normal) >= 0.0 {
                    best = best.max(foot.dot(dir));
                }
            }
        }
        if inside { 1.0 } else { best }
    }
}

/// The largest singular value of the warp's Jacobian at `(s, t)`: how far a
/// unit step of warped coordinate moves the unit direction, in the direction
/// it moves it most.
fn warp_stretch(face: usize, s: f64, t: f64) -> f64 {
    const H: f64 = 1e-6;
    let at = |s: f64, t: f64| cube::direction(face, s, t).normalize();
    let ds = (at(s + H, t) - at(s - H, t)) / (2.0 * H);
    let dt = (at(s, t + H) - at(s, t - H)) / (2.0 * H);
    let (ss, st, tt) = (ds.dot(ds), ds.dot(dt), dt.dot(dt));
    let half = f64::midpoint(ss, tt);
    (half + (0.25 * (ss - tt) * (ss - tt) + st * st).sqrt()).sqrt()
}

/// Where a tile stands against one camera.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Class {
    /// At least partly in the frame.
    View,
    /// Outside the frame, within the margin.
    Margin,
}

/// One camera, set up for the tests the descent makes.
struct Lens {
    eye_dir: DVec3,
    cos_horizon: f64,
    sin_horizon: f64,
    /// Left, right, bottom, top, pointing inward, normalized.
    planes: [DVec4; 4],
    view: DMat4,
    mvp: DMat4,
    aspect: f64,
    /// Pixels per unit of tangent space at the lens axis.
    focal: f64,
    /// The widest angle off the lens axis the frame reaches, as a tangent.
    frame_tan: f64,
    /// The nearest depth any point of the sphere in the frame can have.
    min_depth: f64,
    /// How far past the frame the margin reaches, in tile widths.
    margin: f64,
}

impl Lens {
    fn new(camera: &CameraParams, width: u32, height: u32, margin: f64) -> Option<Self> {
        if width == 0 || height == 0 {
            return None;
        }
        let orbital = OrbitalCamera::from_params(camera);
        let aspect = aspect_ratio(width, height);
        let view = orbital.view_matrix().as_dmat4();
        let mvp = orbital.mvp_matrix(aspect).as_dmat4();
        let eye = orbital.eye_position().as_dvec3();
        let distance = eye.length();
        if distance.is_nan() || distance <= 1.0 {
            return None;
        }
        let aspect = f64::from(aspect);
        let tan_v = (f64::from(orbital.fov_deg).to_radians() * 0.5).tan();
        let frame_tan = ((1.0 + f64::from(camera.offset_x).abs()) * tan_v * aspect)
            .hypot((1.0 + f64::from(camera.offset_y).abs()) * tan_v);
        let plane = |p: DVec4| p / p.truncate().length();
        let (x, y, w) = (mvp.row(0), mvp.row(1), mvp.row(3));
        let cos_horizon = 1.0 / distance;
        Some(Self {
            eye_dir: eye / distance,
            cos_horizon,
            sin_horizon: (1.0 - cos_horizon * cos_horizon).sqrt(),
            planes: [plane(w + x), plane(w - x), plane(w + y), plane(w - y)],
            view,
            mvp,
            aspect,
            focal: f64::from(height) / (2.0 * tan_v),
            frame_tan,
            min_depth: (distance - 1.0) / frame_tan.hypot(1.0),
            margin,
        })
    }

    /// Where `shape` stands: `None` beyond the horizon or past the margin.
    fn classify(&self, shape: &Shape) -> Option<Class> {
        let cos_angle = shape.center.dot(self.eye_dir);
        if cos_angle < self.cos_horizon {
            let reach = self.cos_horizon * shape.cos_radius - self.sin_horizon * shape.sin_radius;
            if cos_angle < reach || shape.nearest_cos(self.eye_dir) < self.cos_horizon {
                return None;
            }
        }
        let ball = shape.center * shape.cos_radius;
        let radius = shape.sin_radius;
        let reach = radius * 2.0f64.mul_add(self.margin, 1.0);
        let mut class = Class::View;
        for plane in &self.planes {
            let distance = plane.truncate().dot(ball) + plane.w;
            if distance < -reach {
                return None;
            }
            if distance < -radius {
                class = Class::Margin;
            }
        }
        Some(class)
    }

    /// The most pixels a texel `texel` wide in warped coordinates covers
    /// anywhere on `shape` that the frame can show.
    fn texel_px(&self, shape: &Shape, texel: f64) -> f64 {
        let ball = self.view.transform_point3(shape.center * shape.cos_radius);
        let depth = (-ball.z - shape.sin_radius).max(self.min_depth);
        let off_axis = ((ball.x.hypot(ball.y) + shape.sin_radius) / depth).min(self.frame_tan);
        self.focal * off_axis.hypot(1.0) / depth * shape.stretch * texel
    }

    /// How far from the middle of the frame `dir` on the sphere projects, in
    /// half frame heights.
    fn distance_from_middle(&self, dir: DVec3) -> f64 {
        let clip = self.mvp * dir.extend(1.0);
        if clip.w <= 0.0 {
            return f64::INFINITY;
        }
        (clip.x / clip.w * self.aspect).hypot(clip.y / clip.w)
    }
}

/// The aspect ratio `write_uniforms` gives the projection for this size.
#[expect(
    clippy::cast_precision_loss,
    reason = "an output extent is far inside the f32 mantissa"
)]
fn aspect_ratio(width: u32, height: u32) -> f32 {
    width as f32 / height as f32
}

/// What the descents found out about a tile.
const IN_VIEW: u8 = 1;
const IN_MARGIN: u8 = 2;
const FOR_EXPORT: u8 = 4;
const CAPPED: u8 = 8;

/// One descent of every face's quadtree against one lens.
struct Walk<'a> {
    levels: &'a [Tiling],
    lens: &'a Lens,
    threshold: f64,
    /// Marks set on a tile wanted in view, and on one wanted in the margin.
    view_marks: u8,
    margin_marks: u8,
    marks: &'a mut [Vec<u8>],
}

impl Walk<'_> {
    fn run(&mut self) {
        let root = &self.levels[0];
        for face in 0..FACES {
            for row in 0..root.side {
                for col in 0..root.side {
                    let shape = &root.shapes[root.index(face, row, col)];
                    if let Some(class) = self.lens.classify(shape) {
                        self.visit(0, face, row, col, class);
                    }
                }
            }
        }
    }

    /// Whether the rule wants anything at this tile, and if it does, mark it
    /// or the children that replace it.
    fn visit(&mut self, depth: usize, face: usize, row: u32, col: u32, class: Class) -> bool {
        let level = &self.levels[depth];
        let index = level.index(face, row, col);
        if self
            .lens
            .texel_px(&level.shapes[index], level.coarser_texel)
            <= self.threshold
        {
            return false;
        }
        let mut uncovered = None;
        if let Some(finer) = self.levels.get(depth + 1) {
            for (down, right) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                let (row, col) = (row * 2 + down, col * 2 + right);
                let shape = &finer.shapes[finer.index(face, row, col)];
                let Some(child) = self.lens.classify(shape) else {
                    continue;
                };
                if !self.visit(depth + 1, face, row, col, child) {
                    uncovered = Some(uncovered.map_or(child, |other: Class| other.min(child)));
                }
            }
        } else {
            uncovered = Some(class);
        }
        if let Some(class) = uncovered {
            self.marks[depth][index] |= match class {
                Class::View => self.view_marks,
                Class::Margin => self.margin_marks,
            };
        }
        true
    }
}

impl Residency {
    /// Measure every tile `geometry` cuts.
    ///
    /// # Panics
    ///
    /// If `geometry` cannot be cut, which a pack built under it would already
    /// have refused.
    #[must_use]
    pub fn new(geometry: Geometry) -> Self {
        geometry
            .check()
            .unwrap_or_else(|e| panic!("the tile geometry: {e}"));
        let mut coarser = geometry.floor;
        let levels = geometry
            .level_sizes()
            .map(|size| {
                let side = size / geometry.tile;
                let edge = |i: u32| f64::from(i * geometry.tile) / f64::from(size) * 2.0 - 1.0;
                let mut shapes = Vec::with_capacity(FACES * (side * side) as usize);
                for face in 0..FACES {
                    for row in 0..side {
                        for col in 0..side {
                            shapes.push(Shape::new(
                                face,
                                [edge(col), edge(col + 1)],
                                [edge(row), edge(row + 1)],
                            ));
                        }
                    }
                }
                let level = Tiling {
                    level: Geometry::level_of(size),
                    side,
                    coarser_texel: 2.0 / f64::from(coarser),
                    shapes,
                };
                coarser = size;
                level
            })
            .collect();
        Self {
            geometry,
            floor: Geometry::level_of(geometry.floor),
            levels,
        }
    }

    #[must_use]
    pub fn geometry(&self) -> Geometry {
        self.geometry
    }

    /// The wanted set for `request`, and the cap the page table takes from it.
    #[must_use]
    pub fn wanted(&self, request: &Request<'_>) -> Wanted {
        let depth = self
            .levels
            .iter()
            .take_while(|level| level.level <= request.finest)
            .count();
        let levels = &self.levels[..depth];
        let lenses: Vec<(Lens, bool)> = request
            .outputs
            .iter()
            .filter_map(|output| {
                Lens::new(&output.camera, output.width, output.height, MARGIN_TILES)
                    .map(|lens| (lens, output.export))
            })
            .collect();
        if levels.is_empty() || lenses.is_empty() {
            return Wanted {
                tiles: Vec::new(),
                cap: CellLevels::uniform(&self.geometry, self.floor),
            };
        }
        let marks = mark(levels, &lenses, request);
        self.rank(levels, &marks, &lenses, request)
    }

    /// The set in its order, and the cap, from what the descents marked.
    fn rank(
        &self,
        levels: &[Tiling],
        marks: &[Vec<u8>],
        lenses: &[(Lens, bool)],
        request: &Request<'_>,
    ) -> Wanted {
        let mut cap = CellLevels::uniform(&self.geometry, self.floor);
        let mut ranked = Vec::new();
        for (level, marks) in levels.iter().zip(marks) {
            for (index, &mark) in marks.iter().enumerate() {
                let key = level.key(index);
                if mark & CAPPED != 0 {
                    cap.raise(key);
                }
                if mark & (IN_VIEW | IN_MARGIN) == 0 {
                    continue;
                }
                let shape = &level.shapes[index];
                let distance = lenses
                    .iter()
                    .map(|(lens, _)| lens.distance_from_middle(shape.center))
                    .fold(f64::INFINITY, f64::min);
                for pack in drawn(request, shape) {
                    let id = TileId { pack, key };
                    if !(request.stored)(id) {
                        continue;
                    }
                    ranked.push(Ranked {
                        tile: WantedTile {
                            id,
                            deficit: key.level - self.floor,
                            margin: mark & IN_VIEW == 0,
                        },
                        export: mark & FOR_EXPORT != 0,
                        distance,
                    });
                }
            }
        }
        ranked.sort_by(Ranked::order);
        Wanted {
            tiles: ranked.into_iter().map(|ranked| ranked.tile).collect(),
            cap,
        }
    }
}

/// Descend every output's lens, and during a drag the cameras along its lead,
/// and mark what each wants.
///
/// Without a drag one descent per output at the 1 px threshold marks both the
/// set and the cap. During one, the set is taken at the drag's threshold, the
/// cap in a second descent at 1 px, and the lead's cameras add to the margin.
fn mark(levels: &[Tiling], lenses: &[(Lens, bool)], request: &Request<'_>) -> Vec<Vec<u8>> {
    let mut marks: Vec<Vec<u8>> = levels
        .iter()
        .map(|level| vec![0; level.shapes.len()])
        .collect();
    let (threshold, capped) = match request.drag {
        Some(_) => (DRAG_THRESHOLD_PX, 0),
        None => (THRESHOLD_PX, CAPPED),
    };
    for (lens, export) in lenses {
        let export = if *export { FOR_EXPORT } else { 0 };
        Walk {
            levels,
            lens,
            threshold,
            view_marks: IN_VIEW | export | capped,
            margin_marks: IN_MARGIN,
            marks: &mut marks,
        }
        .run();
        if request.drag.is_some() {
            Walk {
                levels,
                lens,
                threshold: THRESHOLD_PX,
                view_marks: CAPPED,
                margin_marks: 0,
                marks: &mut marks,
            }
            .run();
        }
    }
    let Some(drag) = request.drag else {
        return marks;
    };
    for output in request.outputs {
        for step in 1..=DRAG_LEAD_STEPS {
            #[expect(clippy::cast_precision_loss, reason = "a handful of steps")]
            let ahead = DRAG_LEAD_SECONDS * step as f32 / DRAG_LEAD_STEPS as f32;
            let mut camera = output.camera;
            camera.longitude += drag.longitude_rate * ahead;
            camera.latitude += drag.latitude_rate * ahead;
            let Some(lens) = Lens::new(&camera, output.width, output.height, 0.0) else {
                continue;
            };
            Walk {
                levels,
                lens: &lens,
                threshold,
                view_marks: IN_MARGIN,
                margin_marks: IN_MARGIN,
                marks: &mut marks,
            }
            .run();
        }
    }
    marks
}

/// The packs whose tiles the globe draws at `shape`.
fn drawn(request: &Request<'_>, shape: &Shape) -> impl Iterator<Item = PackKind> + use<> {
    let day = PackKind::Day(request.month);
    let (day_drawn, night_drawn) = match request.surfaces {
        Surfaces::Day => (true, false),
        Surfaces::Night => (false, true),
        Surfaces::Blend {
            sun,
            terminator_width,
        } => {
            let sun = sun.as_dvec3().normalize_or_zero();
            let width = f64::from(terminator_width);
            let cos = shape.center.dot(sun).clamp(-1.0, 1.0);
            let sin = (1.0 - cos * cos).sqrt();
            let (c, s) = (shape.cos_radius, shape.sin_radius);
            let highest = if cos >= c { 1.0 } else { cos * c + sin * s };
            let lowest = if cos <= -c { -1.0 } else { cos * c - sin * s };
            (highest > -width, lowest < width)
        }
    };
    [
        day_drawn.then_some(day),
        night_drawn.then_some(PackKind::Night),
    ]
    .into_iter()
    .flatten()
}

struct Ranked {
    tile: WantedTile,
    export: bool,
    distance: f64,
}

impl Ranked {
    fn order(a: &Self, b: &Self) -> Ordering {
        let pack = |ranked: &Self| matches!(ranked.tile.id.pack, PackKind::Night);
        a.tile
            .margin
            .cmp(&b.tile.margin)
            .then(b.export.cmp(&a.export))
            .then(b.tile.deficit.cmp(&a.tile.deficit))
            .then(a.distance.total_cmp(&b.distance))
            .then(pack(a).cmp(&pack(b)))
            .then(a.tile.id.key.cmp(&b.tile.id.key))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::sync::LazyLock;

    use glam::DVec2;

    use super::*;
    use crate::assets::tiles::{FIXTURE, GEOMETRY};
    use crate::scene::camera::distance_to_zoom;

    static SHIPPED: LazyLock<Residency> = LazyLock::new(|| Residency::new(GEOMETRY));

    const UHD: (u32, u32) = (3840, 2160);

    fn finest() -> u8 {
        Geometry::level_of(GEOMETRY.face)
    }

    fn floor() -> u8 {
        Geometry::level_of(GEOMETRY.floor)
    }

    fn camera(longitude: f32, latitude: f32, distance: f32) -> CameraParams {
        CameraParams {
            longitude,
            latitude,
            zoom: distance_to_zoom(distance),
            ..CameraParams::default()
        }
    }

    fn uhd(camera: CameraParams) -> Output {
        Output {
            camera,
            width: UHD.0,
            height: UHD.1,
            export: false,
        }
    }

    fn everything(_: TileId) -> bool {
        true
    }

    fn request<'a>(outputs: &'a [Output], stored: &'a dyn Fn(TileId) -> bool) -> Request<'a> {
        Request {
            outputs,
            month: 4,
            surfaces: Surfaces::Day,
            finest: finest(),
            drag: None,
            stored,
        }
    }

    fn want(outputs: &[Output]) -> Wanted {
        SHIPPED.wanted(&request(outputs, &everything))
    }

    fn in_view(wanted: &Wanted) -> HashSet<TileId> {
        wanted.in_view().collect()
    }

    /// The tile of `level` that holds the point `dir` of the sphere.
    fn covering(dir: DVec3, level: u8) -> TileKey {
        let (face, s, t) = cube::locate(dir);
        let side = (GEOMETRY.face >> (finest() - level)) / GEOMETRY.tile;
        let index = |w: f64| {
            let at = (f64::midpoint(w, 1.0) * f64::from(side)).floor();
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "clamped to a face's tiles"
            )]
            let at = at.clamp(0.0, f64::from(side - 1)) as u16;
            at
        };
        TileKey {
            level,
            face: u8::try_from(face).unwrap(),
            row: index(t),
            col: index(s),
        }
    }

    /// The cell of the finest level that holds `dir`.
    fn cell(dir: DVec3) -> (u8, u32, u32) {
        let key = covering(dir, finest());
        (key.face, u32::from(key.row), u32::from(key.col))
    }

    /// Points of a tile on a grid of `per_side` steps a side, corners
    /// included, column by column.
    fn samples(key: TileKey, per_side: u32) -> impl Iterator<Item = DVec3> {
        let size = GEOMETRY.face >> (finest() - key.level);
        let edge = |i: u16| f64::from(u32::from(i) * GEOMETRY.tile) / f64::from(size) * 2.0 - 1.0;
        let (s0, s1) = (edge(key.col), edge(key.col + 1));
        let (t0, t1) = (edge(key.row), edge(key.row + 1));
        let face = usize::from(key.face);
        (0..=per_side).flat_map(move |i| {
            (0..=per_side).map(move |j| {
                let along =
                    |a: f64, b: f64, k: u32| a + (b - a) * f64::from(k) / f64::from(per_side);
                cube::direction(face, along(s0, s1, i), along(t0, t1, j)).normalize()
            })
        })
    }

    fn center_of(key: TileKey) -> DVec3 {
        samples(key, 2).nth(4).expect("nine samples")
    }

    /// The frame an output draws, ray cast on its own rather than through the
    /// descent's bounds.
    struct Frame {
        mvp: DMat4,
        inverse: DMat4,
        eye: DVec3,
        width: f64,
        height: f64,
    }

    impl Frame {
        fn new(output: &Output) -> Self {
            let camera = OrbitalCamera::from_params(&output.camera);
            let mvp = camera
                .mvp_matrix(aspect_ratio(output.width, output.height))
                .as_dmat4();
            Self {
                mvp,
                inverse: mvp.inverse(),
                eye: camera.eye_position().as_dvec3(),
                width: f64::from(output.width),
                height: f64::from(output.height),
            }
        }

        /// The point of the sphere at pixel `(x, y)`, x right and y down.
        fn hit(&self, x: f64, y: f64) -> Option<DVec3> {
            let ndc = DVec2::new(2.0 * x / self.width - 1.0, 1.0 - 2.0 * y / self.height);
            let near = self.inverse.project_point3(ndc.extend(0.0));
            let far = self.inverse.project_point3(ndc.extend(1.0));
            let dir = (far - near).normalize();
            let b = near.dot(dir);
            let disc = b * b - (near.length_squared() - 1.0);
            (disc >= 0.0).then(|| near + dir * (-b - disc.sqrt()))
        }

        fn ndc(&self, p: DVec3) -> Option<DVec2> {
            let clip = self.mvp * p.extend(1.0);
            (clip.w > 0.0).then(|| DVec2::new(clip.x / clip.w, clip.y / clip.w))
        }

        /// The angle from the point under the eye to the horizon.
        fn horizon(&self) -> f64 {
            (1.0 / self.eye.length()).acos()
        }

        /// The level the rule asks for at pixel `(x, y)`, measured from the
        /// pixel's own footprint on the face: the coarsest level, the floor
        /// included, whose texel covers at most `threshold` pixels along its
        /// longest side. `None` off the globe and where a neighbor pixel lands
        /// on another face.
        fn needed(&self, x: f64, y: f64, threshold: f64) -> Option<(DVec3, u8)> {
            let point = self.hit(x, y)?;
            let warped = |p: DVec3| {
                let (face, s, t) = cube::locate(p);
                (face, DVec2::new(s, t))
            };
            let (face, at) = warped(point);
            let (right_face, right) = warped(self.hit(x + 1.0, y)?);
            let (down_face, down) = warped(self.hit(x, y + 1.0)?);
            if right_face != face || down_face != face {
                return None;
            }
            let (step_x, step_y) = (right - at, down - at);
            let (xx, xy, yy) = (
                step_x.length_squared(),
                step_x.dot(step_y),
                step_y.length_squared(),
            );
            let root = (0.25 * (xx - yy) * (xx - yy) + xy * xy).sqrt();
            let shortest = (f64::midpoint(xx, yy) - root).max(0.0).sqrt();
            let mut level = floor();
            let mut size = GEOMETRY.floor;
            for finer in GEOMETRY.level_sizes() {
                if 2.0 / (f64::from(size) * shortest) <= threshold {
                    break;
                }
                level = Geometry::level_of(finer);
                size = finer;
            }
            Some((point, level))
        }
    }

    /// Cameras over a face center, the pole, a cube corner and elsewhere, a
    /// panned, tilted and turned one, at both ends of the zoom and between,
    /// through three lenses, and one along the limb.
    fn cameras() -> Vec<CameraParams> {
        let mut cameras = Vec::new();
        for distance in [1.5, 2.2, 3.5, 5.76, 8.0, 11.5, 20.0, 80.0] {
            for (longitude, latitude) in [(0.0, 0.0), (10.0, 89.9), (45.0, 35.26), (-100.0, -50.0)]
            {
                cameras.push(camera(longitude, latitude, distance));
            }
            let mut turned = camera(120.0, 20.0, distance);
            turned.offset_x = 0.3;
            turned.offset_y = -0.2;
            turned.tilt_deg = 35.0;
            turned.yaw_deg = 20.0;
            turned.pitch_deg = -15.0;
            cameras.push(turned);
            for fov_deg in [10.0, 60.0] {
                cameras.push(CameraParams {
                    fov_deg,
                    ..camera(30.0, -20.0, distance)
                });
            }
        }
        let mut limb = camera(0.0, 0.0, 1.5);
        limb.yaw_deg = 90.0;
        cameras.push(limb);
        cameras
    }

    /// Every pixel is drawn from a tile at least as fine as its own footprint
    /// asks for: the cap at its cell reaches that level and the set holds the
    /// tile of the cap's level there, in view. The descent's bounds are the
    /// only thing between the two, so this is what says they err the right
    /// way.
    #[test]
    fn every_pixel_gets_the_level_its_footprint_asks_for() {
        for camera in cameras() {
            let output = uhd(camera);
            let wanted = want(&[output]);
            let tiles = in_view(&wanted);
            let frame = Frame::new(&output);
            let stride = 24.0;
            let mut y = 0.5;
            while y < frame.height {
                let mut x = 0.5;
                while x < frame.width {
                    if let Some((p, needed)) = frame.needed(x, y, THRESHOLD_PX) {
                        let (face, row, col) = cell(p);
                        let capped = wanted.cap.at(face, row, col);
                        assert!(
                            capped >= needed,
                            "{camera:?}: pixel ({x}, {y}) needs level {needed}, \
                             its cell is capped at {capped}"
                        );
                        if capped > floor() {
                            let id = TileId {
                                pack: PackKind::Day(4),
                                key: covering(p, capped),
                            };
                            assert!(
                                tiles.contains(&id),
                                "{camera:?}: pixel ({x}, {y}) has no {id:?} in view"
                            );
                        }
                    }
                    x += stride;
                }
                y += stride;
            }
        }
    }

    /// Every wanted tile reaches in front of the horizon, and lies partly in
    /// the frame, or for the margin within its reach of it.
    #[test]
    fn nothing_out_of_reach_is_wanted() {
        for camera in cameras() {
            let output = uhd(camera);
            let frame = Frame::new(&output);
            let eye = frame.eye.normalize();
            for tile in &want(&[output]).tiles {
                let points: Vec<DVec3> = samples(tile.id.key, 16).collect();
                let spacing = points[0].angle_between(points[1]);
                let near_side: Vec<DVec3> = points
                    .iter()
                    .copied()
                    .filter(|p| p.angle_between(eye) <= frame.horizon() + spacing)
                    .collect();
                assert!(
                    !near_side.is_empty(),
                    "{camera:?}: {:?} lies wholly beyond the horizon",
                    tile.id
                );
                let corners: Vec<DVec2> = samples(tile.id.key, 1)
                    .filter_map(|p| frame.ndc(p))
                    .collect();
                let extent = corners
                    .iter()
                    .flat_map(|a| corners.iter().map(move |b| a.distance(*b)))
                    .fold(0.0, f64::max);
                let reach = if tile.margin {
                    extent * 2.0f64.mul_add(MARGIN_TILES, 2.0)
                } else {
                    extent
                };
                let near_frame = near_side
                    .iter()
                    .filter_map(|&p| frame.ndc(p))
                    .any(|ndc| ndc.x.abs() <= 1.0 + reach && ndc.y.abs() <= 1.0 + reach);
                assert!(
                    near_frame,
                    "{camera:?}: {:?} (margin {}) is more than {reach} from the frame",
                    tile.id, tile.margin
                );
            }
        }
    }

    #[test]
    fn the_set_is_the_same_whoever_computes_it() {
        let outputs = [uhd(camera(45.0, 35.26, 5.0)), uhd(camera(-60.0, 10.0, 9.0))];
        let first = want(&outputs);
        assert_eq!(want(&outputs), first);
        assert_eq!(
            Residency::new(GEOMETRY).wanted(&request(&outputs, &everything)),
            first
        );
    }

    /// A camera over the center of +Z close in sees only +Z, all of it at the
    /// finest level, and orders it from the middle of the frame out.
    #[test]
    fn a_face_on_view_wants_its_face_from_the_middle_out() {
        let output = uhd(camera(0.0, 0.0, 1.5));
        let wanted = want(&[output]);
        assert!(!wanted.tiles.is_empty());
        for tile in &wanted.tiles {
            assert_eq!(tile.id.key.face, 4, "{:?}", tile.id);
            assert_eq!(tile.id.key.level, finest(), "{:?}", tile.id);
            assert_eq!(tile.id.pack, PackKind::Day(4));
        }
        let middle = TileId {
            pack: PackKind::Day(4),
            key: covering(DVec3::Z, finest()),
        };
        assert!(in_view(&wanted).contains(&middle));

        let frame = Frame::new(&output);
        let distances: Vec<f64> = wanted
            .tiles
            .iter()
            .filter(|tile| !tile.margin)
            .map(|tile| {
                let ndc = frame.ndc(center_of(tile.id.key)).expect("in front");
                (ndc.x * frame.width / frame.height).hypot(ndc.y)
            })
            .collect();
        assert!(
            distances.windows(2).all(|pair| pair[0] <= pair[1] + 1e-6),
            "center outward: {distances:?}"
        );
        let first_margin = wanted
            .tiles
            .iter()
            .position(|tile| tile.margin)
            .expect("a ring around the frame");
        assert!(
            wanted.tiles[first_margin..].iter().all(|tile| tile.margin),
            "the margin comes last"
        );
    }

    /// Over the north pole the pole is the middle of +Y, where four tiles
    /// meet, and nothing sets it apart from any other point.
    #[test]
    fn over_the_pole_the_four_tiles_that_meet_there_are_wanted() {
        let wanted = want(&[uhd(camera(0.0, 89.9, 1.5))]);
        let tiles = in_view(&wanted);
        let half = u16::try_from(GEOMETRY.face / GEOMETRY.tile / 2).unwrap();
        for (row, col) in [
            (half - 1, half - 1),
            (half - 1, half),
            (half, half - 1),
            (half, half),
        ] {
            let id = TileId {
                pack: PackKind::Day(4),
                key: TileKey {
                    level: finest(),
                    face: 2,
                    row,
                    col,
                },
            };
            assert!(tiles.contains(&id), "{id:?}");
        }
        assert!(wanted.tiles.iter().all(|tile| tile.id.key.face == 2));
    }

    /// Looking along the limb close in, the tiles reach out to the horizon on
    /// the near side and stop there.
    #[test]
    fn a_limb_on_view_reaches_the_horizon_and_stops() {
        let mut limb = camera(0.0, 0.0, 1.5);
        limb.yaw_deg = 90.0;
        let output = uhd(limb);
        let frame = Frame::new(&output);
        let eye = frame.eye.normalize();
        let right = frame.hit(frame.width - 1.0, frame.height / 2.0);
        assert!(right.is_none(), "the right edge of the frame is sky");
        let straddles = |key: TileKey| {
            let (nearest, farthest) = samples(key, 16)
                .map(|p| p.angle_between(eye))
                .fold((f64::INFINITY, 0.0_f64), |(lo, hi), angle| {
                    (lo.min(angle), hi.max(angle))
                });
            nearest <= frame.horizon() && frame.horizon() <= farthest
        };
        let wanted = want(&[output]);
        let reaching = wanted.in_view().filter(|id| straddles(id.key)).count();
        assert!(reaching > 0, "some tile reaches the horizon");
    }

    /// At the near end of the zoom the frame covers a small patch at the
    /// finest level; at the far end the floor serves the whole disk.
    #[test]
    fn the_two_ends_of_the_zoom() {
        let near = want(&[uhd(camera(20.0, 10.0, 1.5))]);
        assert!(!near.tiles.is_empty());
        assert!(near.tiles.iter().all(|tile| tile.id.key.level == finest()));
        let disk = want(&[uhd(camera(20.0, 10.0, 5.0))]);
        assert!(
            in_view(&near).len() < in_view(&disk).len(),
            "the full disk wants more than a patch of it"
        );

        let far = want(&[uhd(camera(20.0, 10.0, 80.0))]);
        assert!(far.tiles.is_empty(), "{} tiles", far.tiles.len());
        assert_eq!(far.cap, CellLevels::uniform(&GEOMETRY, floor()));
    }

    /// Two outputs want each tile either wants, once, and each cell at the
    /// finer of the two levels.
    #[test]
    fn two_outputs_want_the_union_once() {
        let a = uhd(camera(0.0, 0.0, 4.0));
        let b = Output {
            width: 1920,
            height: 1080,
            ..uhd(camera(70.0, 20.0, 7.0))
        };
        let (alone_a, alone_b, both) = (want(&[a]), want(&[b]), want(&[a, b]));
        let ids: Vec<TileId> = both.tiles.iter().map(|tile| tile.id).collect();
        let unique: HashSet<TileId> = ids.iter().copied().collect();
        assert_eq!(unique.len(), ids.len(), "no tile twice");
        let union: HashSet<TileId> = in_view(&alone_a)
            .union(&in_view(&alone_b))
            .copied()
            .collect();
        assert_eq!(in_view(&both), union);
        for face in 0..6 {
            for row in 0..both.cap.cells() {
                for col in 0..both.cap.cells() {
                    assert_eq!(
                        both.cap.at(face, row, col),
                        alone_a
                            .cap
                            .at(face, row, col)
                            .max(alone_b.cap.at(face, row, col))
                    );
                }
            }
        }
    }

    /// A pending export's tiles in view come before every other output's.
    #[test]
    fn an_export_comes_first() {
        let preview = uhd(camera(0.0, 0.0, 3.0));
        let export = Output {
            export: true,
            ..uhd(camera(180.0, 0.0, 3.0))
        };
        let wanted = want(&[preview, export]);
        let behind = |tile: &WantedTile| center_of(tile.id.key).z < 0.0;
        let exported = wanted.tiles.iter().take_while(|tile| behind(tile)).count();
        assert!(exported > 0);
        assert!(
            wanted.tiles[exported..]
                .iter()
                .all(|tile| !behind(tile) || tile.margin),
            "the export's tiles in view lead"
        );
    }

    /// Where the view mixes the two levels, a drag's coarser threshold asks
    /// for far fewer tiles and keeps the cap where the 1 px rule has it, and a
    /// drag's rate adds a lead ahead of it.
    #[test]
    fn a_drag_asks_for_less_and_leads_ahead() {
        let outputs = [uhd(camera(0.0, 0.0, 7.0))];
        let still = want(&outputs);
        let held = SHIPPED.wanted(&Request {
            drag: Some(Drag {
                longitude_rate: 0.0,
                latitude_rate: 0.0,
            }),
            ..request(&outputs, &everything)
        });
        let (still_count, held_count) = (in_view(&still).len(), in_view(&held).len());
        assert!(
            held_count * 3 <= still_count * 2,
            "a drag wants {held_count} in view against {still_count}"
        );
        assert_eq!(held.cap, still.cap, "the cap stays at 1 px");

        let east = SHIPPED.wanted(&Request {
            drag: Some(Drag {
                longitude_rate: 100.0,
                latitude_rate: 0.0,
            }),
            ..request(&outputs, &everything)
        });
        let before: HashSet<TileId> = held.tiles.iter().map(|tile| tile.id).collect();
        let lead: Vec<&WantedTile> = east
            .tiles
            .iter()
            .filter(|tile| !before.contains(&tile.id))
            .collect();
        assert!(!lead.is_empty(), "the lead adds tiles");
        for tile in lead {
            assert!(tile.margin, "{:?} is lead, not view", tile.id);
            assert!(
                center_of(tile.id.key).x > 0.0,
                "{:?} lies east, ahead of the drag",
                tile.id
            );
        }
    }

    #[test]
    fn the_resolution_labels_name_their_levels() {
        assert_eq!(finest_level(8192), 11);
        assert_eq!(finest_level(4096), 10);
        assert_eq!(finest_level(2048), 9);
    }

    /// Each resolution setting stops the descent at its level, and the one at
    /// the floor's asks for nothing.
    #[test]
    fn each_resolution_stops_at_its_level() {
        let outputs = [uhd(camera(-30.0, 40.0, 3.0))];
        for finest in [finest(), finest() - 1, floor()] {
            let wanted = SHIPPED.wanted(&Request {
                finest,
                ..request(&outputs, &everything)
            });
            if finest == floor() {
                assert!(wanted.tiles.is_empty());
                assert_eq!(wanted.cap, CellLevels::uniform(&GEOMETRY, floor()));
                continue;
            }
            assert!(
                wanted.tiles.iter().all(|tile| tile.id.key.level == finest),
                "level {finest}: every point of this view wants the finest allowed"
            );
            assert!(!wanted.tiles.is_empty());
        }
    }

    /// A tile with no blob is never wanted, and the cap at its cells stays
    /// what the rule gives, since the page table draws it from the index.
    #[test]
    fn a_tile_without_a_blob_is_never_wanted() {
        let outputs = [uhd(camera(10.0, 5.0, 5.0))];
        let all = want(&outputs);
        let ocean: HashSet<TileKey> = all
            .tiles
            .iter()
            .map(|tile| tile.id.key)
            .filter(|key| (key.row + key.col) % 3 == 0)
            .collect();
        let stored = |id: TileId| !ocean.contains(&id.key);
        let some = SHIPPED.wanted(&request(&outputs, &stored));
        assert!(!ocean.is_empty());
        assert!(some.tiles.iter().all(|tile| !ocean.contains(&tile.id.key)));
        assert_eq!(some.tiles.len() + ocean.len(), all.tiles.len());
        assert_eq!(some.cap, all.cap);
    }

    /// In blend mode the night is wanted where the terminator lets it show and
    /// the day where the sun reaches, so a view across the terminator wants
    /// both, and noon and midnight one each.
    #[test]
    fn the_blend_wants_each_surface_where_it_shows() {
        let width = 0.1;
        let blend = |longitude: f32| {
            let outputs = [uhd(camera(longitude, 0.0, 3.0))];
            SHIPPED.wanted(&Request {
                surfaces: Surfaces::Blend {
                    sun: Vec3::X,
                    terminator_width: width,
                },
                ..request(&outputs, &everything)
            })
        };
        let count = |wanted: &Wanted, pack: PackKind| {
            wanted
                .tiles
                .iter()
                .filter(|tile| tile.id.pack == pack)
                .count()
        };
        let noon = blend(90.0);
        assert!(count(&noon, PackKind::Day(4)) > 0);
        assert_eq!(count(&noon, PackKind::Night), 0);
        let midnight = blend(-90.0);
        assert_eq!(count(&midnight, PackKind::Day(4)), 0);
        assert!(count(&midnight, PackKind::Night) > 0);
        let dusk = blend(0.0);
        assert!(count(&dusk, PackKind::Day(4)) > 0 && count(&dusk, PackKind::Night) > 0);
        let width = f64::from(width);
        for tile in &dusk.tiles {
            let (lowest, highest) = samples(tile.id.key, 8)
                .map(|p| p.x)
                .fold((1.0_f64, -1.0_f64), |(lo, hi), x| (lo.min(x), hi.max(x)));
            if tile.id.pack == PackKind::Night {
                assert!(lowest < width + 1e-6, "{:?}", tile.id);
            } else {
                assert!(highest > -width - 1e-6, "{:?}", tile.id);
            }
        }

        let night_alone = SHIPPED.wanted(&Request {
            surfaces: Surfaces::Night,
            ..request(&[uhd(camera(90.0, 0.0, 3.0))], &everything)
        });
        assert!(!night_alone.tiles.is_empty());
        assert!(
            night_alone
                .tiles
                .iter()
                .all(|tile| tile.id.pack == PackKind::Night)
        );
    }

    /// The small fixture geometry is measured from its own sizes: a camera
    /// that sees its coarse texels large gets its finest level everywhere.
    #[test]
    fn a_small_geometry_is_measured_from_its_own_sizes() {
        let fixture = Residency::new(FIXTURE);
        let outputs = [uhd(camera(0.0, 0.0, 3.0))];
        let wanted = fixture.wanted(&Request {
            finest: Geometry::level_of(FIXTURE.face),
            ..request(&outputs, &everything)
        });
        let levels: HashMap<u8, usize> =
            wanted
                .tiles
                .iter()
                .fold(HashMap::new(), |mut counts, tile| {
                    *counts.entry(tile.id.key.level).or_default() += 1;
                    counts
                });
        assert_eq!(
            levels.keys().copied().collect::<Vec<_>>(),
            [Geometry::level_of(FIXTURE.face)]
        );
        assert_eq!(wanted.cap.cells(), FIXTURE.face / FIXTURE.tile);
    }
}
