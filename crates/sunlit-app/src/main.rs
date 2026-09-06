#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::ExitCode;

/// Attach to the parent process's console so that stdout/stderr work when
/// invoked from a terminal.  In release builds the `windows_subsystem = "windows"`
/// attribute suppresses the console, which makes `--help` / `--version` silent.
/// `AttachConsole(ATTACH_PARENT_PROCESS)` re-establishes the connection without
/// creating a new console window when launched from Explorer.
#[cfg(windows)]
fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};

    // SAFETY: AttachConsole is a well-documented Win32 function with no
    // preconditions. It returns FALSE (harmlessly) when no parent console
    // exists, e.g. when launched from Explorer.
    #[allow(unsafe_code)]
    let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

fn main() -> ExitCode {
    #[cfg(windows)]
    attach_parent_console();

    sunlit_earth::run()
}
