//! Start on login: the platform's own autostart entry, which is the setting.
//!
//! Nothing about it is kept in `config.toml`. The entry is read whenever the
//! app needs to show its state, so a change the user makes in Task Manager or
//! a desktop's settings is what the app shows next. Turning it on writes the
//! entry for this copy of the app and turning it off removes it; the app
//! never rewrites an entry on its own, so a development build cannot take
//! over an installed copy's entry.
//!
//! Each backend takes where its entry lives as a value, so the tests point it
//! at a scratch registry key or a temporary directory. The parts that need no
//! system call (the Windows command line, the desktop entry, the plist, the
//! Homebrew path mapping) compile and are tested on every platform.

use std::path::{Path, PathBuf};

#[cfg_attr(not(windows), allow(dead_code))]
pub mod windows;

/// The arguments every entry passes after the executable.
pub const ARGS: [&str; 2] = ["--tray-start", "hidden"];

/// What the platform's entry says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Autostart {
    /// No entry, or one the user's desktop treats as absent.
    Off,
    /// An entry that starts `exe` at login.
    On { exe: PathBuf },
    /// An entry for `exe` that the system's own settings switched off.
    TurnedOffBySystem { exe: PathBuf },
    /// This session has no way to start the app at login, and what the user
    /// can do instead.
    Unsupported { reason: String },
}

impl Autostart {
    /// The word the `autostart-status` signal uses for this state.
    #[must_use]
    pub fn state_name(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::On { .. } => "on",
            Self::TurnedOffBySystem { .. } => "off-by-system",
            Self::Unsupported { .. } => "unsupported",
        }
    }

    /// The executable the entry runs, where there is an entry.
    #[must_use]
    pub fn exe(&self) -> Option<&Path> {
        match self {
            Self::On { exe } | Self::TurnedOffBySystem { exe } => Some(exe),
            Self::Off | Self::Unsupported { .. } => None,
        }
    }

    /// Whether the switch shows on: an entry exists, whichever copy it runs.
    #[must_use]
    pub fn is_on(&self) -> bool {
        matches!(self, Self::On { .. })
    }

    /// The signal body `autostart-status` answers with, without `SIGNAL:`.
    #[must_use]
    pub fn signal_line(&self) -> String {
        format!(
            "autostart state={} exe={}",
            self.state_name(),
            crate::activation::percent_encode(
                &self
                    .exe()
                    .map(|exe| exe.to_string_lossy().into_owned())
                    .unwrap_or_default()
            )
        )
    }
}

/// Read the entry of this session.
#[must_use]
pub fn read() -> Autostart {
    platform::read()
}

/// Write the entry for this copy of the app.
///
/// :returns: why the entry could not be written, as a sentence for the user
pub fn enable() -> Result<(), String> {
    platform::enable()
}

/// Remove the entry.
///
/// :returns: why the entry could not be removed, as a sentence for the user
pub fn disable() -> Result<(), String> {
    platform::disable()
}

/// The executable an entry written by this copy would run.
///
/// :returns: the path, or why there is none
pub fn this_exe() -> Result<PathBuf, String> {
    platform::this_exe()
}

/// Whether two executable paths name the same program the way the platform
/// compares paths: without case on Windows and macOS, exactly on Linux.
#[must_use]
pub fn same_exe(a: &Path, b: &Path) -> bool {
    if cfg!(any(windows, target_os = "macos")) {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

/// Turn the entry on or off and read it back.
///
/// The state that comes back is what the switch shows, so a write that failed
/// leaves the switch where the system has it.
///
/// :param on: whether to write the entry or remove it
/// :returns: the state read after the write, and the write's error if it
///     failed
pub fn set(on: bool) -> (Autostart, Result<(), String>) {
    let written = if on { enable() } else { disable() };
    (read(), written)
}

#[cfg(windows)]
mod platform {
    use std::path::PathBuf;

    use super::Autostart;
    use super::windows::Location;

    pub(super) fn read() -> Autostart {
        Location::system().read()
    }

    pub(super) fn enable() -> Result<(), String> {
        Location::system().enable(&this_exe()?)
    }

    pub(super) fn disable() -> Result<(), String> {
        Location::system().disable()
    }

    pub(super) fn this_exe() -> Result<PathBuf, String> {
        std::env::current_exe().map_err(|e| format!("this program's path could not be read: {e}"))
    }
}

#[cfg(not(windows))]
mod platform {
    use std::path::PathBuf;

    use super::Autostart;

    const NOT_YET: &str = "Start on login is not available on this platform yet.";

    pub(super) fn read() -> Autostart {
        Autostart::Unsupported {
            reason: NOT_YET.to_owned(),
        }
    }

    pub(super) fn enable() -> Result<(), String> {
        Err(NOT_YET.to_owned())
    }

    pub(super) fn disable() -> Result<(), String> {
        Err(NOT_YET.to_owned())
    }

    pub(super) fn this_exe() -> Result<PathBuf, String> {
        std::env::current_exe().map_err(|e| format!("this program's path could not be read: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_line_names_the_state_and_the_executable_on_one_line() {
        let line = Autostart::On {
            exe: PathBuf::from("/opt/Sunlit Earth/sunlit-earth"),
        }
        .signal_line();
        assert_eq!(line.split_whitespace().count(), 3, "{line}");
        assert!(line.starts_with("autostart state=on exe="), "{line}");
        assert_eq!(
            Autostart::Off.signal_line(),
            "autostart state=off exe=",
            "no entry, no executable"
        );
        assert!(
            Autostart::TurnedOffBySystem {
                exe: PathBuf::from("x")
            }
            .signal_line()
            .contains("state=off-by-system")
        );
    }

    #[test]
    fn only_an_entry_that_runs_shows_the_switch_on() {
        let exe = PathBuf::from("x");
        assert!(Autostart::On { exe: exe.clone() }.is_on());
        assert!(!Autostart::TurnedOffBySystem { exe }.is_on());
        assert!(!Autostart::Off.is_on());
        assert!(
            !Autostart::Unsupported {
                reason: String::new()
            }
            .is_on()
        );
    }
}
