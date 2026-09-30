//! Turning one set of JPEG XL faces into a pack on disk.

use std::fs::{self, File};
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rayon::prelude::*;
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use super::codec::{self, BlockFormat};
use super::cut::{Cube, Plane};
use super::pack::{self, Entry, Header, Pack, TileKey};
use super::{Geometry, PackKind, TILE_LEVELS, pack_path};
use crate::assets::cube_layout::{CubeTextures, FACES, FaceSet, YEAR};
use crate::assets::texture_loader;

/// The suffix of a pack that is not finished yet.
pub(super) const UNFINISHED_SUFFIX: &str = "~";

/// The mask value of open water.
const OPEN_WATER: u8 = 255;

/// How far a night texel over open water may lie from the night's ocean color
/// in a tile drawn as that color. The night's open water is the source's dark
/// noise, within 2 of its tile's mean in 98% of the tiles, and a light at sea
/// stands tens to hundreds above it, which keeps its tile stored.
const NIGHT_OCEAN_TOLERANCE: u8 = 4;

/// Why a pack was not built.
#[derive(Debug)]
pub enum BuildError {
    /// A face the pack is made from is missing or a Git LFS pointer.
    Incomplete(String),
    /// The cancel flag was raised; nothing was put in place.
    Cancelled,
    Failed(String),
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Incomplete(what) => write!(f, "the sources are incomplete: {what}"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::Failed(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for BuildError {}

impl From<io::Error> for BuildError {
    fn from(e: io::Error) -> Self {
        Self::Failed(e.to_string())
    }
}

/// What [`ensure_pack`] found or did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ensured {
    /// The pack on disk was built under the current key.
    Current,
    Built(BuildReport),
}

/// What a build made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildReport {
    /// Tiles in the index, constant ocean included.
    pub tiles: usize,
    /// Tiles flagged constant ocean, which store nothing.
    pub ocean: usize,
    /// The size of the pack file.
    pub bytes: u64,
    /// Decoding the JPEG XL faces.
    pub decode: Duration,
    /// Everything, the decode included.
    pub total: Duration,
}

/// The versions that go into a key beside the sources and the geometry.
#[derive(Debug, Clone, Copy)]
struct Versions {
    format: u32,
    dds: &'static str,
    preset: &'static str,
}

const CURRENT: Versions = Versions {
    format: pack::FORMAT_VERSION,
    dds: codec::DDS_VERSION,
    preset: codec::PRESET.1,
};

/// What a source file looked like: size and modification time, as the texture
/// downscales are stamped. Hashing 84 files on every start would cost more
/// than it could catch, since they only change when an install replaces them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    bytes: u64,
    modified_ms: Option<u64>,
}

impl Stamp {
    fn of(path: &Path) -> Option<Self> {
        let meta = fs::metadata(path).ok()?;
        Some(Self {
            bytes: meta.len(),
            modified_ms: meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|d| u64::try_from(d.as_millis()).ok()),
        })
    }
}

/// The face sets a pack is made from: the surface itself, and the mask where
/// the pack flags ocean or is the mask.
struct Sources<'a> {
    color: Option<[&'a Path; FACES.len()]>,
    mask: Option<[&'a Path; FACES.len()]>,
}

fn whole<'a>(set: &str, faces: &'a FaceSet) -> Result<[&'a Path; 6], BuildError> {
    let mut paths = [Path::new(""); 6];
    for (slot, (face, path)) in paths.iter_mut().zip(FACES.iter().zip(faces)) {
        *slot = path
            .as_deref()
            .ok_or_else(|| BuildError::Incomplete(format!("{set} {face}")))?;
    }
    Ok(paths)
}

fn sources(kind: PackKind, textures: &CubeTextures) -> Result<Sources<'_>, BuildError> {
    Ok(match kind {
        PackKind::Day(month) => Sources {
            color: Some(whole("day", &textures.day[month])?),
            mask: Some(whole("mask", &textures.mask)?),
        },
        PackKind::Night => Sources {
            color: Some(whole("night", &textures.night)?),
            mask: Some(whole("mask", &textures.mask)?),
        },
        PackKind::Mask => Sources {
            color: None,
            mask: Some(whole("mask", &textures.mask)?),
        },
    })
}

fn stamps(sources: &Sources<'_>) -> Result<Vec<(&'static str, &'static str, Stamp)>, BuildError> {
    let mut out = Vec::new();
    for (role, set) in [("color", sources.color), ("mask", sources.mask)] {
        for (face, path) in FACES.iter().zip(set.iter().flatten()) {
            let stamp = Stamp::of(path).ok_or_else(|| {
                BuildError::Incomplete(format!("no metadata for {}", path.display()))
            })?;
            out.push((role, *face, stamp));
        }
    }
    Ok(out)
}

fn key_text(
    kind: PackKind,
    geometry: &Geometry,
    versions: Versions,
    stamps: &[(&str, &str, Stamp)],
) -> String {
    use std::fmt::Write as _;
    let mut key = format!(
        "sunlit-earth tile pack\nformat {}\ndds {}\npreset {}\n",
        versions.format, versions.dds, versions.preset
    );
    let Geometry {
        face,
        levels,
        tile,
        gutter,
        floor,
        mask,
    } = *geometry;
    let _ = writeln!(
        key,
        "geometry face {face} levels {levels} tile {tile} gutter {gutter} floor {floor} mask {mask}"
    );
    let _ = match kind {
        PackKind::Day(month) => writeln!(key, "kind day {YEAR}{:02}", month + 1),
        PackKind::Night => writeln!(key, "kind night"),
        PackKind::Mask => writeln!(key, "kind mask"),
    };
    for (role, face, stamp) in stamps {
        let modified = stamp
            .modified_ms
            .map_or_else(|| "-".to_owned(), |ms| ms.to_string());
        let _ = writeln!(key, "{role} {face} {} {modified}", stamp.bytes);
    }
    key
}

/// The key a pack of `kind` built now from `textures` would carry.
pub fn expected_key(
    kind: PackKind,
    textures: &CubeTextures,
    geometry: &Geometry,
) -> Result<String, BuildError> {
    let sources = sources(kind, textures)?;
    Ok(key_text(kind, geometry, CURRENT, &stamps(&sources)?))
}

/// Make sure the pack of `kind` under `cache_dir` is the one the current
/// sources give, building it when it is missing, unreadable or keyed
/// otherwise.
///
/// Runs on the calling thread, with the block encode spread over the current
/// rayon pool. `cancel` is looked at between face decodes and between batches of
/// 64 tiles, each about a quarter of a second on one thread; raising it
/// abandons the build and leaves whatever pack was there before.
pub fn ensure_pack(
    cache_dir: &Path,
    kind: PackKind,
    textures: &CubeTextures,
    geometry: &Geometry,
    cancel: &AtomicBool,
) -> Result<Ensured, BuildError> {
    ensure_as(cache_dir, kind, textures, geometry, cancel, CURRENT)
}

fn ensure_as(
    cache_dir: &Path,
    kind: PackKind,
    textures: &CubeTextures,
    geometry: &Geometry,
    cancel: &AtomicBool,
    versions: Versions,
) -> Result<Ensured, BuildError> {
    geometry.check().map_err(BuildError::Failed)?;
    let sources = sources(kind, textures)?;
    let before = stamps(&sources)?;
    let key = key_text(kind, geometry, versions, &before);
    let path = pack_path(cache_dir, kind);
    match Pack::open(&path) {
        Ok(pack) if pack.key() == key => {
            drop(pack);
            crate::files::sweep_unfinished(&path, UNFINISHED_SUFFIX);
            return Ok(Ensured::Current);
        }
        Ok(_) => info!(path = %path.display(), "the tile pack's key is stale, rebuilding"),
        Err(pack::PackError::Io(e)) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => warn!(path = %path.display(), error = %e, "rebuilding an unusable tile pack"),
    }

    let report = build(&path, kind, &sources, &before, geometry, key, cancel)?;
    info!(
        path = %path.display(),
        tiles = report.tiles,
        ocean = report.ocean,
        bytes = report.bytes,
        decode_secs = format_args!("{:.2}", report.decode.as_secs_f64()),
        total_secs = format_args!("{:.2}", report.total.as_secs_f64()),
        "built a tile pack"
    );
    Ok(Ensured::Built(report))
}

/// One entry to write: what it is, and where its texels come from.
enum Job {
    /// A face and its full mip chain, finest first.
    Face { key: TileKey, chain: Vec<Plane> },
    /// A tile of `levels[level]`.
    Tile {
        key: TileKey,
        level: usize,
        ocean: bool,
    },
}

impl Job {
    fn key(&self) -> TileKey {
        match self {
            Self::Face { key, .. } | Self::Tile { key, .. } => *key,
        }
    }
}

fn decode_face(path: &Path, channels: usize, size: u32) -> Result<Plane, BuildError> {
    let fail = |e: String| BuildError::Failed(format!("{}: {e}", path.display()));
    let mut reader = image::ImageReader::open(path).map_err(|e| fail(e.to_string()))?;
    reader.no_limits();
    let image = reader.decode().map_err(|e| fail(e.to_string()))?;
    if (image.width(), image.height()) != (size, size) {
        return Err(fail(format!(
            "{} x {}, not a {size} px face",
            image.width(),
            image.height()
        )));
    }
    let texels = if channels == 4 {
        image.into_rgba8().into_raw()
    } else {
        image.into_luma8().into_raw()
    };
    Ok(Plane {
        size,
        channels,
        texels,
    })
}

fn decode_cube(
    paths: &[&Path; 6],
    channels: usize,
    size: u32,
    cancel: &AtomicBool,
) -> Result<Cube, BuildError> {
    let mut faces = Vec::with_capacity(6);
    for path in paths {
        check(cancel)?;
        faces.push(decode_face(path, channels, size)?);
    }
    Cube::new(faces).map_err(BuildError::Failed)
}

fn check(cancel: &AtomicBool) -> Result<(), BuildError> {
    if cancel.load(Ordering::Relaxed) {
        Err(BuildError::Cancelled)
    } else {
        Ok(())
    }
}

/// A face's chain from `plane` halved until it is `size` wide, down to one
/// texel.
fn chain(plane: &Plane, size: u32) -> Vec<Plane> {
    let mut top = plane.halved();
    while top.size > size {
        top = top.halved();
    }
    let mut chain = vec![top];
    while chain.last().is_some_and(|p| p.size > 1) {
        let next = chain.last().expect("a level").halved();
        chain.push(next);
    }
    chain
}

/// The mean color of the texels of `color` that `mask` calls open water.
fn ocean_color(color: &Cube, mask: &Cube) -> [u8; 4] {
    let mut sum = [0_u64; 4];
    let mut count = 0_u64;
    for face in 0..6 {
        let texels = color.face(face).texels.chunks_exact(4);
        for (texel, &water) in texels.zip(&mask.face(face).texels) {
            if water == OPEN_WATER {
                for (s, &c) in sum.iter_mut().zip(texel) {
                    *s += u64::from(c);
                }
                count += 1;
            }
        }
    }
    if count == 0 {
        return [0; 4];
    }
    sum.map(|s| u8::try_from((s + count / 2) / count).expect("a mean of bytes"))
}

/// What a pack of `kind` stores, before any of it is encoded: its block
/// format, its ocean color, the tiled levels and the jobs in index order.
fn prepare(
    kind: PackKind,
    color: Option<Cube>,
    mask: Option<Cube>,
    geometry: &Geometry,
) -> (BlockFormat, [u8; 4], Vec<Cube>, Vec<Job>) {
    match (color, mask) {
        (Some(color), mask) => {
            let ocean = mask.as_ref().map_or([0; 4], |m| ocean_color(&color, m));
            let flat = matches!(kind, PackKind::Night).then_some((ocean, NIGHT_OCEAN_TOLERANCE));
            let (levels, jobs) = surface_jobs(color, mask.as_ref(), flat, geometry);
            (BlockFormat::Bc7, ocean, levels, jobs)
        }
        (None, Some(mask)) => {
            let jobs = (0..6)
                .map(|face| Job::Face {
                    key: face_key(geometry.mask, face),
                    chain: chain(mask.face(face), geometry.mask),
                })
                .collect();
            (BlockFormat::Bc4, [0; 4], Vec::new(), jobs)
        }
        (None, None) => unreachable!("every pack has a surface or is the mask"),
    }
}

fn build(
    path: &Path,
    kind: PackKind,
    sources: &Sources<'_>,
    before: &[(&str, &str, Stamp)],
    geometry: &Geometry,
    key: String,
    cancel: &AtomicBool,
) -> Result<BuildReport, BuildError> {
    let started = Instant::now();
    texture_loader::register_jxl_hook();
    let color = match sources.color {
        Some(paths) => Some(decode_cube(&paths, 4, geometry.face, cancel)?),
        None => None,
    };
    let mask = match sources.mask {
        Some(paths) => Some(decode_cube(&paths, 1, geometry.face, cancel)?),
        None => None,
    };
    let decode = started.elapsed();

    let (format, ocean, levels, jobs) = prepare(kind, color, mask, geometry);

    let header = Header {
        kind,
        format,
        tile: geometry.tile,
        gutter: geometry.gutter,
        tile_levels: TILE_LEVELS,
        ocean,
        key,
    };
    let tmp = crate::files::unfinished(path, UNFINISHED_SUFFIX);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    // Checked before the rename, so a pack whose sources moved under the
    // decode is never put where a reader would open it.
    let written = write_pack(&tmp, &header, &jobs, &levels, geometry, cancel).and_then(|bytes| {
        if stamps(sources)? == before {
            Ok(bytes)
        } else {
            Err(BuildError::Failed(
                "a source changed while its pack was built".to_owned(),
            ))
        }
    });
    let bytes = match written {
        Ok(bytes) => bytes,
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
    };
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(BuildError::Failed(format!(
            "could not put {} in place: {e}",
            path.display()
        )));
    }
    crate::files::sweep_unfinished(path, UNFINISHED_SUFFIX);

    let tiles: Vec<_> = jobs
        .iter()
        .filter_map(|job| match job {
            Job::Tile { ocean, .. } => Some(*ocean),
            Job::Face { .. } => None,
        })
        .collect();
    Ok(BuildReport {
        tiles: tiles.len(),
        ocean: tiles.iter().filter(|&&o| o).count(),
        bytes,
        decode,
        total: started.elapsed(),
    })
}

fn face_key(size: u32, face: usize) -> TileKey {
    TileKey {
        level: Geometry::level_of(size),
        face: u8::try_from(face).expect("a face index"),
        row: 0,
        col: 0,
    }
}

/// The tiled levels, coarse first, and the jobs of a surface pack in index
/// order: the floor faces, then every tile of every level.
fn surface_jobs(
    color: Cube,
    mask: Option<&Cube>,
    flat: Option<([u8; 4], u8)>,
    geometry: &Geometry,
) -> (Vec<Cube>, Vec<Job>) {
    let mut levels = vec![color];
    for _ in 1..geometry.levels {
        let next = levels.last().expect("a level").halved();
        levels.push(next);
    }
    levels.reverse();

    let mut jobs: Vec<Job> = (0..6)
        .map(|face| Job::Face {
            key: face_key(geometry.floor, face),
            chain: chain(levels[0].face(face), geometry.floor),
        })
        .collect();
    let finest = geometry.face;
    for (level, size) in geometry.level_sizes().enumerate() {
        let per_side = size / geometry.tile;
        let scale = i64::from(finest / size);
        for face in 0..6 {
            for row in 0..per_side {
                for col in 0..per_side {
                    let ocean = mask.is_some_and(|mask| {
                        let (r, c) = tile_origin(geometry, row, col);
                        let window = mask.window(
                            face,
                            r * scale,
                            c * scale,
                            geometry.layer() * u32::try_from(scale).expect("a small scale"),
                        );
                        window.iter().all(|&m| m == OPEN_WATER)
                    }) && flat.is_none_or(|(ocean, tolerance)| {
                        let (r, c) = tile_origin(geometry, row, col);
                        let window = levels[level].window(face, r, c, geometry.layer());
                        window.chunks_exact(4).all(|texel| {
                            texel[..3]
                                .iter()
                                .zip(ocean)
                                .all(|(&t, o)| t.abs_diff(o) <= tolerance)
                        })
                    });
                    jobs.push(Job::Tile {
                        key: TileKey {
                            level: Geometry::level_of(size),
                            face: u8::try_from(face).expect("a face index"),
                            row: u16::try_from(row).expect("a tile row"),
                            col: u16::try_from(col).expect("a tile column"),
                        },
                        level,
                        ocean,
                    });
                }
            }
        }
    }
    (levels, jobs)
}

/// The face texel a tile's layer starts at, its gutter included.
fn tile_origin(geometry: &Geometry, row: u32, col: u32) -> (i64, i64) {
    let at = |i: u32| i64::from(i * geometry.tile) - i64::from(geometry.gutter);
    (at(row), at(col))
}

/// Jobs encoded side by side on the rayon pool before their blobs are written:
/// enough to keep every thread busy, few enough that the blobs waiting to be
/// written, about 26 KB a tile, stay near a megabyte and a half.
const BATCH: usize = 64;

/// One encoded blob and its checksums.
struct Blob {
    bytes: Vec<u8>,
    crc32: u32,
    hash: [u8; 32],
}

/// The layer of the tile at `key` cut from its level, and the mips below it.
fn tile_planes(key: TileKey, level: &Cube, geometry: &Geometry) -> Vec<Plane> {
    let (row, col) = tile_origin(geometry, u32::from(key.row), u32::from(key.col));
    let mut plane = Plane {
        size: geometry.layer(),
        channels: 4,
        texels: level.window(usize::from(key.face), row, col, geometry.layer()),
    };
    let mut planes = Vec::new();
    for _ in 1..TILE_LEVELS {
        let next = plane.halved();
        planes.push(std::mem::replace(&mut plane, next));
    }
    planes.push(plane);
    planes
}

/// Encode one job. A tile is small enough that splitting it across threads
/// costs more than it gains, so the tiles of a batch run side by side instead;
/// a whole face is split.
fn blob_of(
    job: &Job,
    format: BlockFormat,
    levels: &[Cube],
    geometry: &Geometry,
) -> Result<Blob, BuildError> {
    let mut bytes = Vec::new();
    match job {
        Job::Face { chain, .. } => {
            for plane in chain {
                codec::encode(format, &plane.texels, plane.size, true, &mut bytes)
                    .map_err(BuildError::Failed)?;
            }
        }
        Job::Tile { ocean: true, .. } => {
            return Ok(Blob {
                bytes,
                crc32: 0,
                hash: [0; 32],
            });
        }
        Job::Tile { key, level, .. } => {
            for plane in tile_planes(*key, &levels[*level], geometry) {
                codec::encode(format, &plane.texels, plane.size, false, &mut bytes)
                    .map_err(BuildError::Failed)?;
            }
        }
    }
    Ok(Blob {
        crc32: crc32fast::hash(&bytes),
        hash: Sha256::digest(&bytes).into(),
        bytes,
    })
}

/// Write the pack to `tmp` and return its size: the blobs streamed in index
/// order behind room for the header, then the header and index over it.
fn write_pack(
    tmp: &Path,
    header: &Header,
    jobs: &[Job],
    levels: &[Cube],
    geometry: &Geometry,
    cancel: &AtomicBool,
) -> Result<u64, BuildError> {
    let start = pack::header_len(&header.key, jobs.len()) as u64;
    let mut out = BufWriter::new(File::create(tmp)?);
    out.seek(SeekFrom::Start(start))?;
    #[cfg(test)]
    tests::after_create(tmp);
    let mut offset = start;
    let mut entries = Vec::with_capacity(jobs.len());
    for batch in jobs.chunks(BATCH) {
        check(cancel)?;
        let blobs = batch
            .par_iter()
            .map(|job| blob_of(job, header.format, levels, geometry))
            .collect::<Result<Vec<_>, _>>()?;
        for (job, blob) in batch.iter().zip(blobs) {
            let ocean = matches!(job, Job::Tile { ocean: true, .. });
            let length = u32::try_from(blob.bytes.len()).expect("a blob fits in 4 GiB");
            entries.push(Entry {
                key: job.key(),
                ocean,
                whole_face: matches!(job, Job::Face { .. }),
                offset: if ocean { 0 } else { offset },
                length,
                crc32: blob.crc32,
                hash: blob.hash,
            });
            out.write_all(&blob.bytes)?;
            offset += u64::from(length);
        }
    }
    let mut file = out
        .into_inner()
        .map_err(|e| BuildError::Failed(e.to_string()))?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&pack::encode_header(header, &entries))?;
    file.sync_all()?;
    Ok(offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::tiles::cut::tests::smooth_cube;
    use crate::assets::tiles::{CACHE_SUBDIR, FIXTURE, decode_bc7};
    use crate::test_support::{ScratchDir, write_cube_fixture};
    use std::cell::RefCell;
    use std::sync::Arc;

    type Hook = Box<dyn FnOnce(&Path)>;

    thread_local! {
        static AFTER_CREATE: RefCell<Option<Hook>> = const { RefCell::new(None) };
    }

    pub(super) fn after_create(tmp: &Path) {
        if let Some(hook) = AFTER_CREATE.with_borrow_mut(Option::take) {
            hook(tmp);
        }
    }

    struct Setup {
        dir: ScratchDir,
        textures: CubeTextures,
    }

    impl Setup {
        fn new(name: &str) -> Self {
            let dir = ScratchDir::new(&format!("tiles_build_{name}"));
            write_cube_fixture(&dir.join("textures"));
            let textures = CubeTextures::resolve(&dir.join("textures"));
            assert!(textures.is_complete(), "the fixture is a whole set");
            Self { dir, textures }
        }

        fn cache(&self) -> std::path::PathBuf {
            self.dir.join("cache")
        }

        fn ensure(&self, kind: PackKind) -> Ensured {
            self.ensure_as(kind, &FIXTURE, CURRENT)
        }

        fn ensure_as(&self, kind: PackKind, geometry: &Geometry, versions: Versions) -> Ensured {
            ensure_as(
                &self.cache(),
                kind,
                &self.textures,
                geometry,
                &AtomicBool::new(false),
                versions,
            )
            .expect("ensure")
        }

        fn open(&self, kind: PackKind) -> Pack {
            Pack::open(&pack_path(&self.cache(), kind)).expect("open")
        }

        fn face(&self, month: usize, face: usize) -> std::path::PathBuf {
            self.textures.day[month][face].clone().expect("a face")
        }
    }

    fn built(ensured: &Ensured) -> bool {
        matches!(ensured, Ensured::Built(_))
    }

    /// The largest channel difference between the core of a decoded tile layer
    /// and the texels of `face` it was cut from.
    fn core_error(layer: &[u8], face: &Plane, key: TileKey) -> (f64, u8) {
        let width = FIXTURE.layer() as usize;
        let mut worst = 0;
        let mut sum = 0_u32;
        for y in 0..FIXTURE.tile {
            for x in 0..FIXTURE.tile {
                let r = u32::from(key.row) * FIXTURE.tile + y;
                let c = u32::from(key.col) * FIXTURE.tile + x;
                let at =
                    (((y + FIXTURE.gutter) * FIXTURE.layer() + x + FIXTURE.gutter) * 4) as usize;
                let src = ((r * face.size + c) * 4) as usize;
                for k in 0..3 {
                    let d = layer[at + k].abs_diff(face.texels[src + k]);
                    worst = worst.max(d);
                    sum += u32::from(d);
                }
            }
        }
        debug_assert_eq!(layer.len(), width * width * 4);
        (
            f64::from(sum) / f64::from(FIXTURE.tile * FIXTURE.tile * 3),
            worst,
        )
    }

    #[test]
    fn a_month_round_trips_through_its_pack() {
        let setup = Setup::new("round_trip");
        let ensured = setup.ensure(PackKind::Day(2));
        let Ensured::Built(report) = &ensured else {
            panic!("the first ensure builds: {ensured:?}");
        };
        assert_eq!(report.tiles, 6 * (1 + 4));

        let pack = setup.open(PackKind::Day(2));
        assert_eq!(pack.kind(), PackKind::Day(2));
        assert_eq!(pack.format(), BlockFormat::Bc7);
        assert_eq!(pack.entries().len(), 6 + report.tiles);
        let levels: Vec<u8> = pack.entries().iter().map(|e| e.key.level).collect();
        assert!(
            levels.windows(2).all(|w| w[0] <= w[1]),
            "coarse levels first"
        );
        assert_eq!(levels[..6], [2; 6], "the floor faces come first");

        // The mean error against the tile diagonally across the face, which
        // shows the comparison can tell one tile's texels from another's.
        let (mut own, mut across) = (0.0, 0.0);
        for face in 0..6 {
            let finest = decode_face(&setup.face(2, face), 4, FIXTURE.face).expect("decode");
            let coarse = finest.halved();
            for entry in pack
                .entries()
                .iter()
                .filter(|e| usize::from(e.key.face) == face)
            {
                let blob = pack.read(entry).expect("read");
                if entry.ocean {
                    assert!(blob.is_empty());
                    continue;
                }
                assert_eq!(<[u8; 32]>::from(Sha256::digest(&blob)), entry.hash);
                let mips = pack.mips(entry);
                let top = &mips[0];
                let texels =
                    decode_bc7(&blob[top.offset..top.offset + top.len], top.size, top.size)
                        .expect("decode");
                if entry.whole_face {
                    assert_eq!(mips.len(), 3, "4, 2 and 1 texels");
                    continue;
                }
                let source = if entry.key.level == 4 {
                    &finest
                } else {
                    &coarse
                };
                let (mean, worst) = core_error(&texels, source, entry.key);
                assert!(
                    mean < 3.0 && worst <= 40,
                    "{:?} is off its own texels by {mean:.2} on average, {worst} at worst",
                    entry.key
                );
                if entry.key.level == 4 {
                    own += mean;
                    let opposite = TileKey {
                        row: 1 - entry.key.row,
                        col: 1 - entry.key.col,
                        ..entry.key
                    };
                    across += core_error(&texels, source, opposite).0;
                }
            }
        }
        assert!(across > own * 4.0, "own {own:.1}, across {across:.1}");

        assert_eq!(setup.ensure(PackKind::Day(2)), Ensured::Current);
    }

    #[test]
    fn the_night_and_the_mask_have_packs_of_their_own() {
        let setup = Setup::new("night_mask");
        assert!(built(&setup.ensure(PackKind::Night)));
        assert!(built(&setup.ensure(PackKind::Mask)));

        let night = setup.open(PackKind::Night);
        assert!(
            night.entries().iter().all(|e| !e.ocean),
            "the fixture's night is a gradient over the water too, so nothing there is flat"
        );
        let mask = setup.open(PackKind::Mask);
        assert_eq!(mask.format(), BlockFormat::Bc4);
        assert_eq!(mask.entries().len(), 6);
        let entry = &mask.entries()[4];
        assert!(entry.whole_face && entry.key.level == 3);
        let blob = mask.read(entry).expect("read");
        assert_eq!(blob.len(), 32 + 8 + 8 + 8);

        let mut top = vec![0_u8; 64];
        dds::decode(
            &mut &blob[..32],
            dds::ImageViewMut::new(
                &mut top,
                dds::Size::new(8, 8),
                dds::ColorFormat::GRAYSCALE_U8,
            )
            .expect("a view"),
            dds::Format::BC4_UNORM,
            &dds::DecodeOptions::default(),
        )
        .expect("decode BC4");
        let path = setup.textures.mask[4].clone().expect("a face");
        let expected = decode_face(&path, 1, FIXTURE.face)
            .expect("decode")
            .halved();
        let worst = top
            .iter()
            .zip(&expected.texels)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .expect("texels");
        assert!(
            worst <= 16,
            "the mask's top level is its halved face: {worst}"
        );
    }

    #[test]
    fn every_part_of_the_key_rebuilds_the_pack() {
        let setup = Setup::new("key");
        let kind = PackKind::Day(0);
        assert!(built(&setup.ensure(kind)));
        assert_eq!(setup.ensure(kind), Ensured::Current);

        let other = Geometry {
            tile: 4,
            gutter: 2,
            ..FIXTURE
        };
        let variants = [
            (
                "format",
                Versions {
                    format: CURRENT.format + 1,
                    ..CURRENT
                },
                FIXTURE,
            ),
            (
                "dds",
                Versions {
                    dds: "9.9.9",
                    ..CURRENT
                },
                FIXTURE,
            ),
            (
                "preset",
                Versions {
                    preset: "normal",
                    ..CURRENT
                },
                FIXTURE,
            ),
            ("geometry", CURRENT, other),
        ];
        for (what, versions, geometry) in variants {
            assert!(built(&setup.ensure_as(kind, &geometry, versions)), "{what}");
            assert_eq!(
                setup.ensure_as(kind, &geometry, versions),
                Ensured::Current,
                "{what}"
            );
        }
        assert!(built(&setup.ensure(kind)), "back to the current key");

        assert!(built(&setup.ensure(PackKind::Night)));
        assert!(built(&setup.ensure(PackKind::Mask)));
        fs::copy(setup.face(6, 1), setup.face(0, 1)).expect("replace a day face");
        assert!(built(&setup.ensure(kind)), "a day face");
        assert_eq!(setup.ensure(PackKind::Night), Ensured::Current);
        assert_eq!(setup.ensure(PackKind::Mask), Ensured::Current);

        let mask = setup.textures.mask[3].clone().expect("a face");
        let file = fs::OpenOptions::new()
            .write(true)
            .open(&mask)
            .expect("open a mask face");
        file.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(86_400))
            .expect("touch");
        drop(file);
        assert!(
            built(&setup.ensure(kind)),
            "a mask face flags the day's ocean"
        );
        assert!(
            built(&setup.ensure(PackKind::Night)),
            "a mask face flags the night's ocean too"
        );
        assert!(built(&setup.ensure(PackKind::Mask)));
        assert_eq!(setup.ensure(kind), Ensured::Current);
    }

    #[test]
    fn a_damaged_pack_is_detected_and_rebuilt() {
        let setup = Setup::new("damaged");
        let kind = PackKind::Day(7);
        assert!(built(&setup.ensure(kind)));
        let path = pack_path(&setup.cache(), kind);
        let pack = setup.open(kind);
        let entry = pack.entries()[8].clone();
        assert!(!entry.ocean);
        drop(pack);

        let mut bytes = fs::read(&path).expect("read the pack");
        bytes[usize::try_from(entry.offset).expect("an offset") + 3] ^= 0x10;
        fs::write(&path, &bytes).expect("damage a blob");
        let pack = setup.open(kind);
        assert!(
            matches!(pack.read(&entry), Err(pack::PackError::Corrupt(key)) if key == entry.key)
        );
        drop(pack);

        bytes[100] ^= 0x10;
        fs::write(&path, &bytes).expect("damage the index");
        assert!(Pack::open(&path).is_err());
        assert!(built(&setup.ensure(kind)), "an unreadable pack is rebuilt");
        assert!(setup.open(kind).read(&entry).is_ok());
    }

    /// A rebuild renames over a pack a reader may hold open, and the reader
    /// goes on reading the pack it opened.
    #[test]
    fn a_reader_keeps_the_pack_it_opened_while_it_is_rebuilt() {
        let setup = Setup::new("rename_over");
        let kind = PackKind::Day(0);
        assert!(built(&setup.ensure(kind)));
        let old = setup.open(kind);
        let entry = old.entries()[10].clone();
        let before = old.read(&entry).expect("read");

        let face = usize::from(entry.key.face);
        fs::copy(setup.face(6, face), setup.face(0, face)).expect("replace the face");
        assert!(built(&setup.ensure(kind)));

        assert_eq!(old.read(&entry).expect("the old pack still reads"), before);
        assert_ne!(setup.open(kind).key(), old.key());
    }

    #[test]
    fn a_current_pack_still_gets_its_leftovers_swept() {
        let setup = Setup::new("sweep_current");
        let kind = PackKind::Day(1);
        assert!(built(&setup.ensure(kind)));
        let target = pack_path(&setup.cache(), kind);
        let orphan = crate::files::unfinished(&target, UNFINISHED_SUFFIX);
        fs::write(&orphan, b"half a sweep").expect("write");

        assert_eq!(setup.ensure(kind), Ensured::Current);
        assert!(!orphan.exists(), "swept although nothing was rebuilt");
    }

    #[test]
    fn a_build_sweeps_what_an_earlier_one_left_unfinished() {
        let setup = Setup::new("sweep");
        let kind = PackKind::Day(1);
        let target = pack_path(&setup.cache(), kind);
        let dir = target.parent().expect("a parent").to_owned();
        fs::create_dir_all(&dir).expect("the cache directory");
        let orphans = [
            crate::files::unfinished(&target, UNFINISHED_SUFFIX),
            crate::files::unfinished(&target, UNFINISHED_SUFFIX),
        ];
        let other = crate::files::unfinished(
            &pack_path(&setup.cache(), PackKind::Night),
            UNFINISHED_SUFFIX,
        );
        let bystander = dir.join("notes.txt");
        for path in orphans.iter().chain([&other, &bystander]) {
            fs::write(path, b"half a pack").expect("write");
        }

        assert!(built(&setup.ensure(kind)));
        for orphan in &orphans {
            assert!(!orphan.exists(), "{} is swept", orphan.display());
        }
        assert!(
            other.exists(),
            "another pack's leftovers are not this build's"
        );
        assert!(bystander.exists());
        let leftovers = fs::read_dir(&dir)
            .expect("list")
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("day-200402.pack.")
            })
            .count();
        assert_eq!(leftovers, 0, "the build's own temporary file is gone");
    }

    /// A build is handed the stamps its key was made from; sources that no
    /// longer look that way when the pack is written mean the pack may hold
    /// pixels of either, and it is not put in place.
    #[test]
    fn a_pack_whose_sources_changed_during_the_build_is_not_put_in_place() {
        let setup = Setup::new("changed_mid_build");
        let kind = PackKind::Day(5);
        let sources = sources(kind, &setup.textures).expect("sources");
        let mut stale = stamps(&sources).expect("stamps");
        stale[2].2.bytes += 1;
        let path = pack_path(&setup.cache(), kind);
        let result = build(
            &path,
            kind,
            &sources,
            &stale,
            &FIXTURE,
            "key".to_owned(),
            &AtomicBool::new(false),
        );
        assert!(
            matches!(&result, Err(BuildError::Failed(why)) if why.contains("changed")),
            "{result:?}"
        );
        assert!(!path.exists());
        let left = fs::read_dir(path.parent().expect("a parent"))
            .expect("list")
            .count();
        assert_eq!(left, 0, "the unfinished pack is removed");
    }

    #[test]
    fn a_cancelled_build_leaves_the_previous_pack() {
        let setup = Setup::new("cancel");
        let kind = PackKind::Day(0);
        assert!(built(&setup.ensure(kind)));
        let key = setup.open(kind).key().to_owned();
        fs::copy(setup.face(6, 0), setup.face(0, 0)).expect("replace a face");

        let result = ensure_pack(
            &setup.cache(),
            kind,
            &setup.textures,
            &FIXTURE,
            &AtomicBool::new(true),
        );
        assert!(matches!(result, Err(BuildError::Cancelled)), "{result:?}");
        assert_eq!(setup.open(kind).key(), key);
        let names: Vec<_> = fs::read_dir(setup.cache().join(CACHE_SUBDIR))
            .expect("list")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["day-200401.pack"]);
    }

    #[test]
    fn a_cancel_while_the_pack_is_being_written_removes_the_unfinished_file() {
        let setup = Setup::new("cancel_writing");
        let kind = PackKind::Day(0);
        let cancel = Arc::new(AtomicBool::new(false));
        let raised = Arc::clone(&cancel);
        AFTER_CREATE.set(Some(Box::new(move |tmp: &Path| {
            assert!(tmp.exists(), "the unfinished file is there");
            raised.store(true, Ordering::Relaxed);
        })));

        let result = ensure_pack(&setup.cache(), kind, &setup.textures, &FIXTURE, &cancel);
        assert!(matches!(result, Err(BuildError::Cancelled)), "{result:?}");
        assert!(AFTER_CREATE.with_borrow(Option::is_none), "the hook ran");
        let names: Vec<_> = fs::read_dir(setup.cache().join(CACHE_SUBDIR))
            .expect("list")
            .flatten()
            .collect();
        assert!(names.is_empty(), "{names:?}");
    }

    #[test]
    fn a_missing_face_or_a_face_of_the_wrong_size_builds_nothing() {
        let mut setup = Setup::new("incomplete");
        let ensure = |setup: &Setup, kind, geometry: &Geometry| {
            ensure_pack(
                &setup.cache(),
                kind,
                &setup.textures,
                geometry,
                &AtomicBool::new(false),
            )
        };
        let wrong = Geometry {
            face: 32,
            floor: 8,
            mask: 16,
            ..FIXTURE
        };
        let result = ensure(&setup, PackKind::Night, &wrong);
        assert!(
            matches!(&result, Err(BuildError::Failed(why)) if why.contains("not a 32 px face")),
            "{result:?}"
        );
        assert!(!pack_path(&setup.cache(), PackKind::Night).exists());

        setup.textures.day[3][5] = None;
        let result = ensure(&setup, PackKind::Day(3), &FIXTURE);
        assert!(
            matches!(&result, Err(BuildError::Incomplete(what)) if what == "day nz"),
            "{result:?}"
        );
    }

    fn water_cube(size: u32, land: &[(usize, u32, u32)]) -> Cube {
        let faces = (0..6)
            .map(|face| {
                let mut texels = vec![OPEN_WATER; (size * size) as usize];
                for &(f, r, c) in land {
                    if f == face {
                        texels[(r * size + c) as usize] = 40;
                    }
                }
                Plane {
                    size,
                    channels: 1,
                    texels,
                }
            })
            .collect();
        Cube::new(faces).expect("a cube")
    }

    /// A tile is ocean only when its whole footprint on the finest mask, the
    /// gutter included, is open water: one land texel under a neighbor's
    /// gutter or across a face edge is enough to store it.
    #[test]
    fn a_tile_is_ocean_when_its_footprint_and_gutter_are_all_water() {
        let geometry = Geometry {
            face: 32,
            levels: 2,
            tile: 8,
            gutter: 4,
            floor: 8,
            mask: 16,
        };
        let key = |level, face, row, col| TileKey {
            level,
            face,
            row,
            col,
        };
        // One land texel in the core of +Z tile (1, 1) at 32 px, which the
        // gutters of +Z tiles (0, 0), (0, 1) and (1, 0) reach, and one on the
        // first column of +X, which the gutter of +Z tile (1, 3) reaches
        // across the edge. +Z tile (1, 2) starts two texels past the first.
        let mask = water_cube(32, &[(4, 10, 10), (0, 12, 0)]);
        let (_, jobs) = surface_jobs(smooth_cube(32), Some(&mask), None, &geometry);
        let (mut ocean, mut stored) = (Vec::new(), Vec::new());
        for job in &jobs {
            match job {
                Job::Tile {
                    key, ocean: true, ..
                } => ocean.push(*key),
                Job::Tile { key, .. } => stored.push(*key),
                Job::Face { .. } => {}
            }
        }

        for k in [
            key(5, 4, 1, 1),
            key(5, 4, 0, 0),
            key(5, 4, 0, 1),
            key(5, 4, 1, 0),
            key(5, 4, 1, 3),
            key(5, 0, 1, 0),
        ] {
            assert!(stored.contains(&k), "{k:?} has land in its footprint");
        }
        for k in [
            key(5, 4, 1, 2),
            key(5, 4, 3, 0),
            key(5, 4, 3, 3),
            key(5, 0, 3, 3),
        ] {
            assert!(ocean.contains(&k), "{k:?} has only water in its footprint");
        }
        for k in [key(4, 4, 0, 0), key(4, 4, 0, 1), key(4, 0, 0, 0)] {
            assert!(stored.contains(&k), "{k:?} at 16 px covers the land at 32");
        }
        assert!(ocean.contains(&key(4, 5, 1, 1)), "-Z has no land");
        assert_eq!(ocean.len() + stored.len(), 6 * (16 + 4));
    }

    /// Over open water the night is flat only where it has no lights: a tile
    /// is drawn as the night's ocean color when every texel of its layer lies
    /// within the tolerance of it, and a light at sea keeps its tile, and the
    /// tiles whose gutters reach it, stored.
    #[test]
    fn a_night_tile_over_water_is_ocean_only_without_lights() {
        let geometry = Geometry {
            face: 32,
            levels: 2,
            tile: 8,
            gutter: 4,
            floor: 8,
            mask: 16,
        };
        let dark = [5_u8, 5, 15, 255];
        let faces = (0..6)
            .map(|face| {
                let mut texels: Vec<u8> = (0..32 * 32)
                    .flat_map(|i: usize| {
                        let wobble = u8::try_from(i % 7).expect("a byte");
                        [dark[0] + wobble % 3, dark[1], dark[2] - wobble % 4, 255]
                    })
                    .collect();
                if face == 4 {
                    let at = (26 * 32 + 3) * 4;
                    texels[at..at + 3].copy_from_slice(&[250, 240, 200]);
                }
                Plane {
                    size: 32,
                    channels: 4,
                    texels,
                }
            })
            .collect();
        let night = Cube::new(faces).expect("a cube");
        let mask = water_cube(32, &[]);
        let (_, jobs) = surface_jobs(
            night,
            Some(&mask),
            Some((dark, NIGHT_OCEAN_TOLERANCE)),
            &geometry,
        );
        let stored: Vec<TileKey> = jobs
            .iter()
            .filter_map(|job| match job {
                Job::Tile {
                    key, ocean: false, ..
                } => Some(*key),
                _ => None,
            })
            .collect();
        let key = |level, face, row, col| TileKey {
            level,
            face,
            row,
            col,
        };
        // The light sits at row 26 and column 3 of +Z: in +Z tile (3, 0) at 32
        // px and (1, 0) at 16, in the gutter of +Z tile (2, 0) above it, of -X
        // tile (3, 3) across the left edge, and at 16 px, where a gutter reaches
        // twice as far, of -X tile (1, 1) and -Y tile (0, 0) across the bottom.
        assert_eq!(
            stored,
            [
                key(4, 1, 1, 1),
                key(4, 3, 0, 0),
                key(4, 4, 1, 0),
                key(5, 1, 3, 3),
                key(5, 4, 2, 0),
                key(5, 4, 3, 0),
            ]
        );
    }

    #[test]
    fn the_ocean_color_is_the_mean_of_open_water() {
        let color = smooth_cube(4);
        let mut sum = [0_u64; 4];
        for face in 0..6 {
            for texel in color.face(face).texels.chunks(4) {
                for (s, &c) in sum.iter_mut().zip(texel) {
                    *s += u64::from(c);
                }
            }
        }
        let expected = sum.map(|s| u8::try_from((s + 48) / 96).expect("a byte"));
        assert_eq!(ocean_color(&color, &water_cube(4, &[])), expected);

        let everywhere: Vec<_> = (0..6)
            .flat_map(|f| (0..16).map(move |i| (f, i / 4, i % 4)))
            .collect();
        assert_eq!(
            ocean_color(&color, &water_cube(4, &everywhere)),
            [0; 4],
            "no water, no color"
        );
    }

    /// A digest of everything the cutter hands the encoder for the fixture's
    /// kinds of pack and for a noisy night over open water: the ocean rules,
    /// the gutters, the halving, the floor chains, the mask chain and the
    /// ocean colors.
    fn cutter_digest() -> String {
        texture_loader::register_jxl_hook();
        let setup = Setup::new("cutter_digest");
        let cancel = AtomicBool::new(false);
        let mut cases = Vec::new();
        for kind in [
            PackKind::Day(0),
            PackKind::Day(6),
            PackKind::Night,
            PackKind::Mask,
        ] {
            let sources = sources(kind, &setup.textures).expect("sources");
            let cube = |paths: Option<[&Path; 6]>, channels| {
                paths.map(|paths| {
                    decode_cube(&paths, channels, FIXTURE.face, &cancel).expect("decode")
                })
            };
            cases.push((kind, cube(sources.color, 4), cube(sources.mask, 1)));
        }
        let noisy = (0..6)
            .map(|face| Plane {
                size: FIXTURE.face,
                channels: 4,
                texels: (0..FIXTURE.face as usize * FIXTURE.face as usize)
                    .flat_map(|i| {
                        [
                            20 + u8::try_from(i * 7 % (face + 4)).expect("a byte"),
                            30,
                            60,
                            255,
                        ]
                    })
                    .collect(),
            })
            .collect();
        cases.push((
            PackKind::Night,
            Some(Cube::new(noisy).expect("a cube")),
            Some(water_cube(FIXTURE.face, &[])),
        ));

        let mut digest = Sha256::new();
        for (kind, color, mask) in cases {
            let (format, ocean, levels, jobs) = prepare(kind, color, mask, &FIXTURE);
            digest.update([format.code()]);
            digest.update(ocean);
            for job in &jobs {
                let key = job.key();
                digest.update([key.level, key.face]);
                digest.update(key.row.to_le_bytes());
                digest.update(key.col.to_le_bytes());
                match job {
                    Job::Face { chain, .. } => {
                        for plane in chain {
                            digest.update(&plane.texels);
                        }
                    }
                    Job::Tile { ocean: true, .. } => digest.update([1]),
                    Job::Tile { level, .. } => {
                        for plane in tile_planes(key, &levels[*level], &FIXTURE) {
                            digest.update(&plane.texels);
                        }
                    }
                }
            }
        }
        digest.finalize().iter().fold(String::new(), |mut hex, b| {
            use std::fmt::Write as _;
            let _ = write!(hex, "{b:02x}");
            hex
        })
    }

    #[test]
    fn a_change_to_what_the_cutter_makes_comes_with_a_format_version_bump() {
        const PINNED: (u32, &str) = (
            1,
            "64ab0d3dcb4b2dffbf705a3157bc5449024f89c6568eb167e8a5923598249461",
        );
        let found = cutter_digest();
        assert_eq!(
            (pack::FORMAT_VERSION, found.as_str()),
            PINNED,
            "what the cutter hands the encoder changed, so packs built by the old cutter must be rebuilt: bump FORMAT_VERSION in pack.rs and set PINNED to ({}, \"{found}\") in this test",
            pack::FORMAT_VERSION + 1,
        );
    }

    #[test]
    fn the_dds_version_in_the_key_is_the_one_the_lockfile_holds() {
        let lock = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock"))
            .expect("read Cargo.lock");
        let entry = lock
            .split("[[package]]")
            .find(|block| block.contains("\nname = \"dds\"\n"))
            .expect("dds is locked");
        assert!(
            entry.contains(&format!("\nversion = \"{}\"\n", codec::DDS_VERSION)),
            "DDS_VERSION is {}, the lockfile says {entry}",
            codec::DDS_VERSION
        );
    }
}
