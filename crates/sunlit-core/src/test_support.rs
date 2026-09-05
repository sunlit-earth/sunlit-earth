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

/// Where scratch directories are made.
///
/// Cargo sets `CARGO_TARGET_TMPDIR` when it compiles an integration target and
/// not when it compiles a library's own unit tests, so the integration targets
/// land under `target/` and the unit tests fall back to the system temporary
/// directory.
pub fn scratch_root() -> PathBuf {
    option_env!("CARGO_TARGET_TMPDIR").map_or_else(std::env::temp_dir, PathBuf::from)
}
