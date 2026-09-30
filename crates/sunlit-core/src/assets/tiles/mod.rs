//! The tile cache the surfaces are drawn from.
//!
//! The textures directory ships each surface as six JPEG XL cube faces
//! ([`cube_layout`](super::cube_layout)). This module turns one set of them
//! into a pack on disk, once, and reads it back a blob at a time:
//!
//! - A day pack, one per month, holds the month's 128 px tiles at the 1024 and
//!   2048 levels, each with an 8 px gutter and one mip, in BC7, and the
//!   month's floor, a 512 px cube with its full mip chain, in BC7. A tile whose
//!   whole footprint on the 2048 mask is open water is flagged constant ocean
//!   and stores nothing.
//! - The night pack holds the same for the night. A night tile over open water
//!   is flagged only where its texels also lie within a few levels of the
//!   night's ocean color, so the lights at sea keep their tiles.
//! - The mask pack holds the water mask as a BC4 cube of 1024 px with its mip
//!   chain.
//!
//! A pack carries the key it was built under: the stamps of the faces it was
//! made from, the `dds` version, the preset, the geometry and the format
//! version. [`ensure_pack`] rebuilds a pack whose key is not the one the
//! current sources and this build would give it. Packs are written under a
//! temporary name and renamed into place, so a reader sees a whole pack or
//! none, and one that holds a pack open keeps reading the one it opened.

mod build;
mod codec;
mod cut;
mod pack;
mod transcoder;

use std::path::{Path, PathBuf};

pub use build::{BuildError, BuildReport, Ensured, ensure_pack, expected_key};
pub use codec::{
    BlockFormat, MipLevel, blob_bytes, decode_bc4, decode_bc7, full_chain, mip_levels,
};
pub use pack::{Entry, Pack, PackError, TileKey};
pub use transcoder::{
    PACKS, PackFailure, Phase, TranscodeNotify, TranscodeStatus, Transcoder, TranscoderConfig,
    default_threads,
};

use super::cube_layout::{MONTHS, YEAR};

/// Directory under the cache directory that holds the packs.
pub const CACHE_SUBDIR: &str = "tile_cache";

/// Levels in a tile's blob: the layer and the one mip below it.
pub const TILE_LEVELS: u32 = 2;

/// The sizes a pack is cut to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    /// The width of a shipped face, which is the finest tiled level.
    pub face: u32,
    /// Tiled levels, each half the width of the next finer one.
    pub levels: u32,
    /// The width of a tile without its gutter.
    pub tile: u32,
    /// The gutter on each side of a tile.
    pub gutter: u32,
    /// The width of a floor face.
    pub floor: u32,
    /// The width of a mask face.
    pub mask: u32,
}

/// What the app cuts the shipped faces to.
pub const GEOMETRY: Geometry = Geometry {
    face: 2048,
    levels: 2,
    tile: 128,
    gutter: 8,
    floor: 512,
    mask: 1024,
};

/// What the tests cut the texture pipeline's fixture bake to: its faces are 16
/// texels, so two levels of 8 px tiles with a 4 px gutter, a floor of 4 and a
/// mask of 8. Public for the integration targets, which hand it to the engine.
pub const FIXTURE: Geometry = Geometry {
    face: 16,
    levels: 2,
    tile: 8,
    gutter: 4,
    floor: 4,
    mask: 8,
};

impl Geometry {
    /// The width of a tile's layer, the tile and both gutters.
    #[must_use]
    pub const fn layer(&self) -> u32 {
        self.tile + 2 * self.gutter
    }

    /// The face widths of the tiled levels, coarse first.
    pub fn level_sizes(&self) -> impl Iterator<Item = u32> + use<> {
        let (face, levels) = (self.face, self.levels);
        (0..levels).rev().map(move |down| face >> down)
    }

    /// The level a face of `size` texels is: the base 2 logarithm of its width.
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the logarithm of a u32 is below 32"
    )]
    pub const fn level_of(size: u32) -> u8 {
        size.ilog2() as u8
    }

    pub(crate) fn check(&self) -> Result<(), String> {
        let most = crate::renderer::tiles::MAX_TILED_LEVELS;
        if self.levels == 0 || self.levels > most {
            return Err(format!(
                "{self:?}: a page table entry names one to {most} tiled levels"
            ));
        }
        let coarsest = self.face >> (self.levels - 1);
        let sizes = [self.face, self.tile, self.floor, self.mask];
        if sizes.iter().any(|s| !s.is_power_of_two()) {
            return Err(format!("{self:?}: the sizes must be powers of two"));
        }
        if coarsest < self.tile || self.floor >= coarsest || self.mask >= self.face {
            return Err(format!(
                "{self:?}: every tiled level must hold a tile, the floor lie below them and the mask below the face"
            ));
        }
        if !self.layer().is_multiple_of(4 << (TILE_LEVELS - 1)) || self.gutter * 2 > coarsest {
            return Err(format!(
                "{self:?}: every level of a layer must be whole blocks, and a gutter reach at most half a face"
            ));
        }
        Ok(())
    }
}

/// Which pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PackKind {
    /// A month of day tiles and its floor, January 0.
    Day(usize),
    /// The night tiles and the night floor.
    Night,
    /// The water mask.
    Mask,
}

impl PackKind {
    /// Every pack there is: the twelve months, the night and the mask.
    pub fn all() -> impl Iterator<Item = Self> {
        (0..MONTHS).map(Self::Day).chain([Self::Night, Self::Mask])
    }

    #[must_use]
    pub fn file_name(self) -> String {
        match self {
            Self::Day(month) => format!("day-{YEAR}{:02}.pack", month + 1),
            Self::Night => "night.pack".to_owned(),
            Self::Mask => "mask.pack".to_owned(),
        }
    }
}

/// Where the pack of `kind` lives under the app's cache directory, the one
/// the cloud cache and the texture downscales use.
#[must_use]
pub fn pack_path(cache_dir: &Path, kind: PackKind) -> PathBuf {
    cache_dir.join(CACHE_SUBDIR).join(kind.file_name())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_geometry_holds_together() {
        GEOMETRY.check().expect("the shipped geometry");
        assert_eq!(GEOMETRY.layer(), 144);
        assert_eq!(GEOMETRY.level_sizes().collect::<Vec<_>>(), [1024, 2048]);
        assert_eq!(Geometry::level_of(2048), 11);
    }

    #[test]
    fn a_geometry_that_cannot_be_cut_is_refused() {
        for broken in [
            Geometry {
                face: 3000,
                ..GEOMETRY
            },
            Geometry {
                levels: 5,
                ..GEOMETRY
            },
            Geometry {
                levels: 40,
                ..GEOMETRY
            },
            Geometry {
                floor: 1024,
                ..GEOMETRY
            },
            Geometry {
                mask: 4096,
                ..GEOMETRY
            },
            Geometry {
                mask: GEOMETRY.face,
                ..GEOMETRY
            },
            Geometry {
                gutter: 6,
                ..GEOMETRY
            },
        ] {
            assert!(broken.check().is_err(), "{broken:?}");
        }
    }

    #[test]
    fn every_pack_has_its_own_file() {
        let names: Vec<_> = PackKind::all().map(PackKind::file_name).collect();
        assert_eq!(names.len(), 14);
        assert_eq!(names[0], "day-200401.pack");
        assert_eq!(names[11], "day-200412.pack");
        let unique: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len());
        assert!(pack_path(Path::new("data"), PackKind::Night).ends_with("tile_cache/night.pack"));
    }
}
