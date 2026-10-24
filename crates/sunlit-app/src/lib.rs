pub mod about;
pub mod activation;
pub mod autostart;
pub mod camera_controls;
pub mod displays;
pub mod engine_client;
pub mod ipc;
pub mod mouse_math;
pub mod session_end;
pub mod time_bar;
pub mod tray;
pub mod ui_callbacks;
pub mod window_state;

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
