//! The layers underneath the cases in `e2e.rs`.
//!
//! An integration target may have submodules of its own, so the harness lives
//! here and the file beside this one is the fifteen cases and nothing else.

pub(crate) mod cloud_stub;
pub(crate) mod desktop_linux;
pub(crate) mod pixels;
pub(crate) mod process;
