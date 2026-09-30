//! Helpers shared by the library's own tests and by the integration targets.
//!
//! The library reaches it as a `#[cfg(test)]` module; the integration targets
//! reach the same file through a `#[path]` attribute in `tests/common/mod.rs`,
//! so both sides name and clean up their scratch directories the same way.
//! Nothing here may depend on crate internals, because in an integration target
//! it is compiled outside the crate.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A directory that exists for the life of one test.
///
/// The name carries the process id and a counter, so two `cargo test` runs over
/// the same checkout, and two tests that ask for the same name in one run, never
/// share a directory. Dropping it removes the tree, which a trailing
/// `remove_dir_all` at the end of a test body does not do when an assertion
/// panics before reaching it.
pub struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    /// Create a scratch directory under [`scratch_root`], named after `name`.
    pub fn new(name: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = scratch_root().join(format!(
            "sunlit_earth_{name}_{}_{nonce}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path)
            .unwrap_or_else(|e| panic!("create scratch directory {}: {e}", path.display()));
        Self { path }
    }

    /// The directory itself, which exists.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A path inside the directory. Nothing is created, so this is also how a
    /// test names a directory it wants the code under test to create.
    pub fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl AsRef<Path> for ScratchDir {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// The texture pipeline's committed fixture bake: cube faces of 16 texels,
/// lossless, for January and July, the night and the water mask, in the
/// layout the textures directory has.
///
/// No Rust JPEG XL encoder is in the dependency tree, so the tests read the
/// files the pipeline's own fixture test keeps equal to a fresh bake rather
/// than writing their own.
pub fn cube_fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/texture-pipeline/tests/fixtures/cube")
}

/// Lay the fixture bake out under `dir` as a complete cube texture set: the
/// first six months of the year from January's faces and the last six from
/// July's, the night and the mask as they are.
pub fn write_cube_fixture(dir: &Path) {
    let source = cube_fixture_dir();
    for month in 1..=12 {
        let from = if month <= 6 { "200401" } else { "200407" };
        copy_files(
            &source.join("day").join(from),
            &dir.join(format!("day/2004{month:02}")),
        );
    }
    copy_files(&source.join("night"), &dir.join("night"));
    copy_files(&source.join("mask"), &dir.join("mask"));
}

/// The shipped cube at an eighth of its size: July's day faces, the night and
/// the water mask, area-averaged to 256 texels, which the texture pipeline's
/// `test_earth_fixture.py` keeps equal to the shipped faces. Unlike the
/// fixture bake it shows the real continents, so a picture of it says which
/// way round a face is.
pub fn earth_fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/earth")
}

/// Lay the Earth fixture out under `dir` as a complete cube texture set, with
/// July's faces standing in for every month.
pub fn write_earth_fixture(dir: &Path) {
    let source = earth_fixture_dir();
    for month in 1..=12 {
        copy_files(
            &source.join("day/200407"),
            &dir.join(format!("day/2004{month:02}")),
        );
    }
    copy_files(&source.join("night"), &dir.join("night"));
    copy_files(&source.join("mask"), &dir.join("mask"));
}

fn copy_files(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap_or_else(|e| panic!("create {}: {e}", to.display()));
    let entries =
        std::fs::read_dir(from).unwrap_or_else(|e| panic!("read {}: {e}", from.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|e| panic!("list {}: {e}", from.display()));
        std::fs::copy(entry.path(), to.join(entry.file_name()))
            .unwrap_or_else(|e| panic!("copy {}: {e}", entry.path().display()));
    }
}

/// Where scratch directories are made.
///
/// Cargo sets `CARGO_TARGET_TMPDIR` when it compiles an integration target and
/// not when it compiles a library's own unit tests, so the integration targets
/// land under `target/` and the unit tests fall back to the system temporary
/// directory.
pub fn scratch_root() -> PathBuf {
    option_env!("CARGO_TARGET_TMPDIR").map_or_else(std::env::temp_dir, PathBuf::from)
}
