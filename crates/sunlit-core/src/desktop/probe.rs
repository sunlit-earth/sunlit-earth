//! The live answers to [`Session`]'s questions, asked of this machine.
//!
//! Each answer is looked up the first time a rung asks for it and kept for the
//! rest of one choice, which is as long as one of these lives.

use std::cell::OnceCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::Session;

/// This process's own session.
pub(crate) struct LiveSession {
    /// The names of this user's processes, read once from `/proc`.
    processes: OnceCell<Vec<String>>,
    /// The session bus, or `None` where it could not be reached.
    bus: OnceCell<Option<zbus::blocking::Connection>>,
    bus_names: std::cell::RefCell<HashMap<String, bool>>,
}

impl LiveSession {
    pub(crate) fn new() -> Self {
        Self {
            processes: OnceCell::new(),
            bus: OnceCell::new(),
            bus_names: std::cell::RefCell::new(HashMap::new()),
        }
    }

    fn bus(&self) -> Option<&zbus::blocking::Connection> {
        self.bus
            .get_or_init(|| match zbus::blocking::Connection::session() {
                Ok(bus) => Some(bus),
                Err(e) => {
                    tracing::debug!("the session bus could not be reached: {e}");
                    None
                }
            })
            .as_ref()
    }
}

impl Session for LiveSession {
    fn var(&self, name: &str) -> Option<String> {
        crate::env_override(name)
    }

    fn on_path(&self, program: &str) -> bool {
        which(program).is_some()
    }

    fn process_running(&self, name: &str) -> bool {
        self.processes
            .get_or_init(own_process_names)
            .iter()
            .any(|process| process == name)
    }

    fn bus_name_owned(&self, name: &str) -> bool {
        if let Some(&owned) = self.bus_names.borrow().get(name) {
            return owned;
        }
        let owned = self.bus().is_some_and(|bus| name_has_owner(bus, name));
        self.bus_names.borrow_mut().insert(name.to_owned(), owned);
        owned
    }

    fn path_exists(&self, path: &Path) -> bool {
        path.exists()
    }
}

fn name_has_owner(bus: &zbus::blocking::Connection, name: &str) -> bool {
    let Ok(name) = zbus::names::BusName::try_from(name) else {
        return false;
    };
    zbus::blocking::fdo::DBusProxy::new(bus)
        .ok()
        .and_then(|proxy| proxy.name_has_owner(name).ok())
        .unwrap_or(false)
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

/// The `comm` of every process this user owns.
///
/// Another user's shell does not draw this session's wallpaper, so a process
/// counts only when its `/proc` entry belongs to the same user as this one.
fn own_process_names() -> Vec<String> {
    use std::os::unix::fs::MetadataExt;

    let Ok(me) = std::fs::metadata("/proc/self").map(|m| m.uid()) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.bytes().all(|b| b.is_ascii_digit()))
        })
        .filter(|entry| entry.metadata().is_ok_and(|m| m.uid() == me))
        .filter_map(|entry| std::fs::read_to_string(entry.path().join("comm")).ok())
        .map(|comm| comm.trim_end().to_owned())
        .collect()
}
