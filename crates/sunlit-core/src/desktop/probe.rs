//! The live answers to [`Session`]'s questions, asked of this machine.
//!
//! Each answer is looked up the first time a rung asks for it and kept for the
//! rest of one choice, which is as long as one of these lives.

use std::path::PathBuf;

use super::Session;

/// This process's own session.
pub(crate) struct LiveSession {
    _private: (),
}

impl LiveSession {
    pub(crate) fn new() -> Self {
        Self { _private: () }
    }
}

impl Session for LiveSession {
    fn var(&self, name: &str) -> Option<String> {
        crate::env_override(name)
    }

    fn on_path(&self, program: &str) -> bool {
        which(program).is_some()
    }
}

/// Whether a program is on `PATH`, and where.
///
/// Written out rather than shelling out to `which`, which is one more program
/// that has to be installed for the check to work.
pub(crate) fn which(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}
