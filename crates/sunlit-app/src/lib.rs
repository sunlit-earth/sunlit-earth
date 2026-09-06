pub mod about;
pub mod displays;
pub mod engine_client;
pub mod ipc;
pub mod mouse_math;
pub mod session_end;
pub mod tray;
pub mod ui_callbacks;

mod app;
mod cli;
mod headless;
mod logging;
mod startup;

pub use app::run;

slint::include_modules!();

/// The scratch-directory helper, shared with `sunlit-core`'s own unit tests.
///
/// `sunlit_core::test_support` is `#[cfg(test)]`, so it is not on the library's
/// public surface; the file is taken by path the same way the integration
/// targets take it.
#[cfg(test)]
#[path = "../../sunlit-core/src/test_support.rs"]
mod test_support;
