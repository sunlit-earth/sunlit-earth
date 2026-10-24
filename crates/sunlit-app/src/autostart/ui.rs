//! The "Start on login" switch in the settings window and in the tray menu.
//!
//! Both show one read of the entry and act through [`super::set`], which reads
//! again after every write, so the two always show what the system holds. The
//! entry is read at startup, every time the settings window is shown, and
//! after every toggle from either place.

use std::path::Path;

use slint::ComponentHandle;
use tracing::{debug, warn};

use super::Autostart;
use crate::MainWindow;

/// What the switch shows for one state of the entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    /// Whether the switch is on.
    pub on: bool,
    /// Whether the switch can be used at all.
    pub available: bool,
    /// The note under the switch, empty for none.
    pub note: String,
}

/// The note for an entry the system's own settings switched off.
const TURNED_OFF_BY_SYSTEM: &str = if cfg!(windows) {
    "Turned off in Windows' Startup apps settings."
} else {
    "Turned off in this desktop's autostart settings."
};

/// What the switch shows for `state`, read by a copy at `this_exe`.
///
/// :param state: what the entry says
/// :param `this_exe`: what an entry written by this copy would run, when known
#[must_use]
pub fn view(state: &Autostart, this_exe: Option<&Path>) -> View {
    match state {
        Autostart::Off => View {
            on: false,
            available: true,
            note: String::new(),
        },
        Autostart::On { exe } => View {
            on: true,
            available: true,
            note: if this_exe.is_some_and(|this| super::same_exe(this, exe)) {
                String::new()
            } else {
                format!(
                    "Starts {} at login. Turn this off and on again to start this copy instead.",
                    exe.display()
                )
            },
        },
        Autostart::TurnedOffBySystem { .. } => View {
            on: false,
            available: true,
            note: TURNED_OFF_BY_SYSTEM.to_owned(),
        },
        Autostart::Unsupported { reason } => View {
            on: false,
            available: false,
            note: reason.clone(),
        },
    }
}

/// Show `state` in the settings window.
///
/// :param window: the settings window
/// :param state: what the entry says
pub fn show(window: &MainWindow, state: &Autostart) {
    let view = view(state, super::this_exe().ok().as_deref());
    window.set_autostart_on(view.on);
    window.set_autostart_available(view.available);
    window.set_autostart_note(view.note.into());
}

/// Read the entry and show what it says.
///
/// :param window: the settings window
pub fn refresh(window: &MainWindow) {
    window.set_autostart_error(slint::SharedString::new());
    let state = super::read();
    debug!(state = state.state_name(), "read the autostart entry");
    show(window, &state);
}

/// Turn the entry on or off, read it back, and show the read.
///
/// :param window: the settings window
/// :param on: whether to write the entry or remove it
/// :returns: why the write failed, if it did
pub fn toggle(window: &MainWindow, on: bool) -> Result<(), String> {
    let (state, written) = super::set(on);
    show(window, &state);
    if let Err(e) = &written {
        warn!("the autostart entry could not be changed: {e}");
    }
    written
}

/// Wire the settings window's switch.
///
/// A failed write leaves the switch where the read after it found the entry,
/// and the line under it says why.
///
/// :param window: the settings window
pub fn register(window: &MainWindow) {
    let window_weak = window.as_weak();
    window.on_autostart_toggled(move |on| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let message = toggle(&win, on).err().unwrap_or_default();
        win.set_autostart_error(message.into());
    });
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn an_entry_for_this_copy_has_no_note() {
        let exe = PathBuf::from("/usr/bin/sunlit-earth");
        assert_eq!(
            view(&Autostart::On { exe: exe.clone() }, Some(&exe)),
            View {
                on: true,
                available: true,
                note: String::new()
            }
        );
    }

    #[test]
    fn an_entry_for_another_copy_names_it() {
        let view = view(
            &Autostart::On {
                exe: PathBuf::from("/opt/other/sunlit-earth"),
            },
            Some(Path::new("/usr/bin/sunlit-earth")),
        );
        assert!(view.on);
        assert!(view.note.contains("/opt/other/sunlit-earth"), "{view:?}");
    }

    #[test]
    fn the_systems_switch_off_reads_as_off_and_says_where() {
        let view = view(
            &Autostart::TurnedOffBySystem {
                exe: PathBuf::from("x"),
            },
            None,
        );
        assert!(!view.on);
        assert!(view.available);
        assert!(view.note.starts_with("Turned off in"), "{view:?}");
    }

    #[test]
    fn an_unsupported_session_disables_the_switch_with_its_reason() {
        let view = view(
            &Autostart::Unsupported {
                reason: "Add `exec x` to its config.".to_owned(),
            },
            None,
        );
        assert!(!view.on && !view.available);
        assert_eq!(view.note, "Add `exec x` to its config.");
    }
}
