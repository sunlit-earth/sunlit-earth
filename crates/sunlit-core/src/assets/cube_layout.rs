//! Where the cube-map textures live under the textures directory.
//!
//! Twelve months of day faces under `day/2004MM/`, and one set each for the
//! night and the water mask, six faces apiece in cube layer order. A file that
//! is missing, or is a Git LFS pointer standing in for one, resolves to `None`
//! and leaves its place in the set, the way the Moon's and the Milky Way's
//! paths behave.

use std::io::Read;
use std::path::{Path, PathBuf};

use super::cube_names;
pub use super::cube_names::{FACES, MONTHS, YEAR};

const LFS_POINTER_PREFIX: &[u8] = b"version https://git-lfs.github.com/spec/v1";

/// The six files of one cube, `None` where a face is not usable.
pub type FaceSet = [Option<PathBuf>; 6];

/// Every cube file the textures directory can hold.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CubeTextures {
    /// January first.
    pub day: [FaceSet; MONTHS],
    pub night: FaceSet,
    pub mask: FaceSet,
}

impl CubeTextures {
    /// Resolve the layout under `dir`.
    #[must_use]
    pub fn resolve(dir: &Path) -> Self {
        Self {
            day: std::array::from_fn(|month| resolve_set(&dir.join(cube_names::day_set(month)))),
            night: resolve_set(&dir.join(cube_names::NIGHT_SET)),
            mask: resolve_set(&dir.join(cube_names::MASK_SET)),
        }
    }

    fn sets(&self) -> impl Iterator<Item = &FaceSet> {
        self.day.iter().chain([&self.night, &self.mask])
    }

    /// How many of the `MONTHS * 6 + 12` files resolved.
    #[must_use]
    pub fn found(&self) -> usize {
        self.sets().flatten().flatten().count()
    }

    /// Whether every file resolved, which is what a renderer needs before it
    /// takes the cube path at all.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.found() == Self::total()
    }

    #[must_use]
    pub const fn total() -> usize {
        (MONTHS + 2) * FACES.len()
    }
}

fn resolve_set(dir: &Path) -> FaceSet {
    FACES.map(|face| {
        let path = dir.join(cube_names::face_file(face));
        is_asset(&path).then_some(path)
    })
}

/// Whether `path` is a file that holds something other than a Git LFS pointer.
///
/// Size cannot tell them apart here, since a lossless mask face can be smaller
/// than the threshold that separates the Moon and the Milky Way from a
/// pointer.
fn is_asset(path: &Path) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = [0_u8; LFS_POINTER_PREFIX.len()];
    let mut filled = 0;
    while filled < head.len() {
        match file.read(&mut head[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(_) => return false,
        }
    }
    filled > 0 && head[..filled] != *LFS_POINTER_PREFIX
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ScratchDir;

    fn write(dir: &Path, relative: &str, bytes: &[u8]) {
        let path = dir.join(relative);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("create the directory");
        std::fs::write(path, bytes).expect("write the stand-in");
    }

    fn write_all(dir: &Path) {
        for file in cube_names::all_files() {
            write(dir, &file, b"\xff\x0a");
        }
    }

    #[test]
    fn a_complete_layout_resolves_every_file_in_order() {
        let scratch = ScratchDir::new("cube_layout_complete");
        write_all(scratch.path());

        let cube = CubeTextures::resolve(scratch.path());
        assert!(cube.is_complete());
        assert_eq!(cube.found(), 84);
        let march = &cube.day[2];
        for (face, path) in FACES.iter().zip(march) {
            assert_eq!(
                path.as_deref(),
                Some(scratch.join(&format!("day/200403/{face}.jxl")).as_path())
            );
        }
        assert_eq!(
            cube.mask[5].as_deref(),
            Some(scratch.join("mask/nz.jxl").as_path())
        );

        // The list the xtask bundles, in its order, is what resolve reads.
        let resolved: Vec<_> = cube.sets().flatten().flatten().cloned().collect();
        let listed: Vec<_> = cube_names::all_files()
            .iter()
            .map(|file| scratch.join(file))
            .collect();
        assert_eq!(resolved, listed);
    }

    #[test]
    fn a_missing_face_leaves_its_place_empty() {
        let scratch = ScratchDir::new("cube_layout_missing");
        write_all(scratch.path());
        std::fs::remove_file(scratch.join("day/200407/py.jxl")).expect("remove a face");

        let cube = CubeTextures::resolve(scratch.path());
        assert!(!cube.is_complete());
        assert_eq!(cube.found(), 83);
        assert_eq!(cube.day[6][2], None);
        assert!(cube.day[6][1].is_some() && cube.day[6][3].is_some());
    }

    #[test]
    fn a_git_lfs_pointer_is_not_a_face() {
        let scratch = ScratchDir::new("cube_layout_pointer");
        write_all(scratch.path());
        write(
            scratch.path(),
            "night/nx.jxl",
            b"version https://git-lfs.github.com/spec/v1\noid sha256:0\nsize 180067\n",
        );
        write(scratch.path(), "mask/ny.jxl", b"");

        let cube = CubeTextures::resolve(scratch.path());
        assert_eq!(cube.night[1], None, "a pointer");
        assert_eq!(cube.mask[3], None, "an empty file");
        assert_eq!(cube.found(), 82);
    }

    #[test]
    fn a_directory_without_the_layout_resolves_nothing() {
        let scratch = ScratchDir::new("cube_layout_empty");
        let cube = CubeTextures::resolve(scratch.path());
        assert_eq!(cube, CubeTextures::default());
        assert_eq!(cube.found(), 0);
    }
}
