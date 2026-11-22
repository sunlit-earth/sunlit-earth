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

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub mod linux;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod macos;
pub mod ui;
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
                    .map_or_default(|exe| exe.to_string_lossy().into_owned())
            )
        )
    }
}

/// What an `autostart` signal line says, read back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutostartSignal {
    /// `off`, `on`, `off-by-system` or `unsupported`.
    pub state: String,
    /// The executable the entry runs, empty where there is none.
    pub exe: String,
}

impl AutostartSignal {
    /// Read a `SIGNAL:autostart ...` line back, the other side of
    /// [`Autostart::signal_line`].
    #[must_use]
    pub fn parse(line: &str) -> Option<Self> {
        let field = |key: &str| {
            let prefix = format!("{key}=");
            line.split_whitespace()
                .find_map(|token| token.strip_prefix(prefix.as_str()))
        };
        Some(Self {
            state: field("state")?.to_owned(),
            exe: crate::activation::percent_decode(field("exe")?)?,
        })
    }
}

thread_local! {
    static STAND_IN: std::cell::RefCell<Option<Autostart>> = const { std::cell::RefCell::new(None) };
}

/// Read the entry of this session.
#[must_use]
pub fn read() -> Autostart {
    STAND_IN
        .with(|slot| slot.borrow().clone())
        .unwrap_or_else(platform::read)
}

/// Write the entry for this copy of the app.
///
/// :returns: why the entry could not be written, as a sentence for the user
pub fn enable() -> Result<(), String> {
    if STAND_IN.with(|slot| slot.borrow().is_some()) {
        let exe = this_exe()?;
        STAND_IN.with(|slot| *slot.borrow_mut() = Some(Autostart::On { exe }));
        return Ok(());
    }
    platform::enable()
}

/// Remove the entry.
///
/// :returns: why the entry could not be removed, as a sentence for the user
pub fn disable() -> Result<(), String> {
    if STAND_IN.with(|slot| slot.borrow().is_some()) {
        STAND_IN.with(|slot| *slot.borrow_mut() = Some(Autostart::Off));
        return Ok(());
    }
    platform::disable()
}

/// Answer [`read`], [`enable`] and [`disable`] on the calling thread from a
/// value that starts as `state`, instead of from the system's entry, for as
/// long as the guard lives.
///
/// For the UI tests, whose thread has not claimed user state and so may not
/// read the real entry.
///
/// :param state: what the entry says until the code under test changes it
#[doc(hidden)]
#[must_use = "the stand-in applies only while the guard lives"]
pub fn stand_in_for_this_thread(state: Autostart) -> StandInGuard {
    let previous = STAND_IN.with(|slot| slot.replace(Some(state)));
    StandInGuard {
        previous,
        _this_thread: std::marker::PhantomData,
    }
}

/// Holds a thread's stand-in entry; see [`stand_in_for_this_thread`].
#[doc(hidden)]
pub struct StandInGuard {
    previous: Option<Autostart>,
    _this_thread: std::marker::PhantomData<*const ()>,
}

impl Drop for StandInGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        let _ = STAND_IN.try_with(|slot| *slot.borrow_mut() = previous);
    }
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

#[cfg(target_os = "linux")]
mod platform {
    use std::path::PathBuf;

    use super::Autostart;
    use super::linux;

    pub(super) fn read() -> Autostart {
        if let Some(reason) = linux::unsupported_session(
            std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref(),
            this_exe().ok().as_deref(),
        ) {
            return Autostart::Unsupported { reason };
        }
        match linux::system_dir() {
            Some(dir) => linux::read(&dir),
            None => Autostart::Unsupported {
                reason: "This system has no home directory to keep an autostart entry in."
                    .to_owned(),
            },
        }
    }

    pub(super) fn enable() -> Result<(), String> {
        let dir = linux::system_dir()
            .ok_or("This system has no home directory to keep an autostart entry in.")?;
        linux::enable(&dir, &this_exe()?)
    }

    pub(super) fn disable() -> Result<(), String> {
        let dir = linux::system_dir()
            .ok_or("This system has no home directory to keep an autostart entry in.")?;
        linux::disable(&dir)
    }

    pub(super) fn this_exe() -> Result<PathBuf, String> {
        let exe = std::env::current_exe()
            .map_err(|e| format!("this program's path could not be read: {e}"))?;
        Ok(linux::stable_path(&exe))
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::path::PathBuf;

    use super::Autostart;
    use super::macos;

    pub(super) fn read() -> Autostart {
        match macos::system_dir() {
            Some(dir) => macos::read(&dir),
            None => Autostart::Unsupported {
                reason: "This system has no home directory to keep a launch agent in.".to_owned(),
            },
        }
    }

    pub(super) fn enable() -> Result<(), String> {
        let dir = macos::system_dir()
            .ok_or("This system has no home directory to keep a launch agent in.")?;
        let exe = std::env::current_exe()
            .map_err(|e| format!("this program's path could not be read: {e}"))?;
        macos::enable(&dir, &exe)
    }

    pub(super) fn disable() -> Result<(), String> {
        let dir = macos::system_dir()
            .ok_or("This system has no home directory to keep a launch agent in.")?;
        macos::disable(&dir)
    }

    pub(super) fn this_exe() -> Result<PathBuf, String> {
        let exe = std::env::current_exe()
            .map_err(|e| format!("this program's path could not be read: {e}"))?;
        Ok(macos::launched_path(&exe))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stand_in_answers_the_reads_and_keeps_the_writes() {
        let _outer = stand_in_for_this_thread(Autostart::Unsupported {
            reason: "outer".to_owned(),
        });
        {
            let _inner = stand_in_for_this_thread(Autostart::Off);
            assert_eq!(read(), Autostart::Off);
            let (state, written) = set(true);
            assert_eq!(written, Ok(()));
            assert_eq!(state.exe(), this_exe().ok().as_deref());
            assert!(read().is_on());
            assert_eq!(set(false), (Autostart::Off, Ok(())));
        }
        assert_eq!(read().state_name(), "unsupported");
    }

    #[test]
    #[cfg(any(windows, target_os = "macos"))]
    #[should_panic(expected = "has not claimed user state")]
    fn without_a_stand_in_a_test_may_not_read_the_entry() {
        let _ = read();
    }

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
    fn the_status_line_reads_back() {
        let exe = PathBuf::from(r"C:\Program Files\Sunlit Earth 100%\sunlit-earth.exe");
        let line = format!(
            "SIGNAL:{}",
            Autostart::On { exe: exe.clone() }.signal_line()
        );
        assert_eq!(
            AutostartSignal::parse(&line),
            Some(AutostartSignal {
                state: "on".to_owned(),
                exe: exe.to_string_lossy().into_owned(),
            })
        );
        assert_eq!(
            AutostartSignal::parse(&Autostart::Off.signal_line()),
            Some(AutostartSignal {
                state: "off".to_owned(),
                exe: String::new(),
            })
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
