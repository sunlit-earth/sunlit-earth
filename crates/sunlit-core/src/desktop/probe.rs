//! The live answers to [`Session`]'s questions, asked of this machine.
//!
//! Each answer is looked up the first time a rung asks for it and kept for the
//! rest of one choice, which is as long as one of these lives.

use std::cell::OnceCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::{PortalFacts, Session, X11Facts};

/// This process's own session.
pub(crate) struct LiveSession {
    /// The names of this user's processes, read once from `/proc`.
    processes: OnceCell<Vec<String>>,
    /// The session bus, or `None` where it could not be reached.
    bus: OnceCell<Option<zbus::blocking::Connection>>,
    bus_names: std::cell::RefCell<HashMap<String, bool>>,
    x11: OnceCell<Option<X11Facts>>,
    wayland: OnceCell<Option<Vec<String>>>,
    portal: OnceCell<Option<PortalFacts>>,
}

impl LiveSession {
    pub(crate) fn new() -> Self {
        Self {
            portal: OnceCell::new(),
            x11: OnceCell::new(),
            wayland: OnceCell::new(),
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
            .get_or_init(|| {
                own_processes()
                    .into_iter()
                    .map(|process| process.comm)
                    .collect()
            })
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

    fn x11(&self) -> Option<X11Facts> {
        self.x11.get_or_init(x11_facts).clone()
    }

    fn wayland_globals(&self) -> Option<Vec<String>> {
        self.wayland.get_or_init(wayland_globals).clone()
    }

    fn processes_named(&self, name: &str) -> Vec<Vec<String>> {
        own_processes()
            .into_iter()
            .filter(|process| process.comm == name)
            .map(|process| process.cmdline)
            .collect()
    }

    fn wallpaper_dir(&self) -> Option<PathBuf> {
        crate::wallpaper::wallpaper_dir().ok()
    }

    fn portal(&self) -> Option<PortalFacts> {
        self.portal
            .get_or_init(|| self.bus().map(portal_facts))
            .clone()
    }
}

const PORTAL_BUS_NAME: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const BACKEND_PREFIX: &str = "org.freedesktop.impl.portal.desktop.";

/// The backends known to implement the wallpaper portal, for one that is
/// installed and not yet running, which cannot be asked without starting it.
const KNOWN_WALLPAPER_BACKENDS: [&str; 4] = ["gnome", "gtk", "kde", "xapp"];

/// The introspection XML of one object, or nothing where it did not answer.
fn introspect(bus: &zbus::blocking::Connection, destination: &str) -> String {
    bus.call_method(
        Some(destination),
        PORTAL_PATH,
        Some("org.freedesktop.DBus.Introspectable"),
        "Introspect",
        &(),
    )
    .ok()
    .and_then(|reply| reply.body().deserialize::<String>().ok())
    .unwrap_or_default()
}

fn declares(xml: &str, interface: &str) -> bool {
    xml.contains(&format!("\"{interface}\""))
}

/// What the portal frontend exports, and which backends on the bus implement
/// the wallpaper portal behind it.
fn portal_facts(bus: &zbus::blocking::Connection) -> PortalFacts {
    let frontend = introspect(bus, PORTAL_BUS_NAME);
    let proxy = zbus::blocking::fdo::DBusProxy::new(bus).ok();
    let owned: Vec<String> = proxy
        .as_ref()
        .and_then(|p| p.list_names().ok())
        .unwrap_or_default()
        .into_iter()
        .map(|name| name.to_string())
        .collect();
    let activatable: Vec<String> = proxy
        .as_ref()
        .and_then(|p| p.list_activatable_names().ok())
        .unwrap_or_default()
        .into_iter()
        .map(|name| name.to_string())
        .collect();
    let mut wallpaper_backends: Vec<String> = Vec::new();
    for name in owned.iter().chain(&activatable) {
        let Some(suffix) = name.strip_prefix(BACKEND_PREFIX) else {
            continue;
        };
        if wallpaper_backends.iter().any(|known| known == suffix) {
            continue;
        }
        let implements = if owned.contains(name) {
            declares(
                &introspect(bus, name),
                "org.freedesktop.impl.portal.Wallpaper",
            )
        } else {
            KNOWN_WALLPAPER_BACKENDS.contains(&suffix)
        };
        if implements {
            wallpaper_backends.push(suffix.to_owned());
        }
    }
    PortalFacts {
        wallpaper: declares(&frontend, "org.freedesktop.portal.Wallpaper"),
        registry: declares(&frontend, "org.freedesktop.host.portal.Registry"),
        wallpaper_backends,
    }
}

/// The interface names the compositor advertises, from one registry round
/// trip over a connection dropped straight after.
fn wayland_globals() -> Option<Vec<String>> {
    use wayland_client::protocol::wl_registry;
    use wayland_client::{Connection, Dispatch, QueueHandle};

    struct Globals(Vec<String>);

    impl Dispatch<wl_registry::WlRegistry, ()> for Globals {
        fn event(
            state: &mut Self,
            _: &wl_registry::WlRegistry,
            event: wl_registry::Event,
            (): &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let wl_registry::Event::Global { interface, .. } = event {
                state.0.push(interface);
            }
        }
    }

    let conn = match Connection::connect_to_env() {
        Ok(conn) => conn,
        Err(e) => {
            tracing::debug!("no Wayland display to ask what it offers: {e}");
            return None;
        }
    };
    let mut queue = conn.new_event_queue();
    conn.display().get_registry(&queue.handle(), ());
    let mut globals = Globals(Vec::new());
    queue.roundtrip(&mut globals).ok()?;
    Some(globals.0)
}

/// Ask the X server for the desktop windows and the window manager's name,
/// over one connection that is closed again straight after.
fn x11_facts() -> Option<X11Facts> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, Window};

    let (conn, screen_num) = match x11rb::connect(None) {
        Ok(connected) => connected,
        Err(e) => {
            tracing::debug!("no X display to ask about the desktop: {e}");
            return None;
        }
    };
    let root = conn.setup().roots.get(screen_num)?.root;
    let atom = |name: &str| -> Option<u32> {
        Some(
            conn.intern_atom(false, name.as_bytes())
                .ok()?
                .reply()
                .ok()?
                .atom,
        )
    };
    let property = |window: Window, name: u32, kind: u32, length: u32| {
        conn.get_property(false, window, name, kind, 0, length)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
    };

    let client_list = atom("_NET_CLIENT_LIST")?;
    let window_type = atom("_NET_WM_WINDOW_TYPE")?;
    let desktop_type = atom("_NET_WM_WINDOW_TYPE_DESKTOP")?;
    let windows: Vec<Window> = property(root, client_list, AtomEnum::WINDOW.into(), u32::MAX)
        .and_then(|reply| reply.value32().map(Iterator::collect))
        .unwrap_or_default();
    let desktop_windows = windows
        .into_iter()
        .filter(|&window| {
            property(window, window_type, AtomEnum::ATOM.into(), 64)
                .and_then(|reply| {
                    reply
                        .value32()
                        .map(|mut types| types.any(|t| t == desktop_type))
                })
                .unwrap_or(false)
        })
        .map(|window| {
            property(
                window,
                AtomEnum::WM_CLASS.into(),
                AtomEnum::STRING.into(),
                256,
            )
            .map(|reply| {
                reply
                    .value
                    .split(|&b| b == 0)
                    .filter(|part| !part.is_empty())
                    .map(|part| String::from_utf8_lossy(part).into_owned())
                    .collect()
            })
            .unwrap_or_default()
        })
        .collect();

    let wm_name = (|| {
        let check = atom("_NET_SUPPORTING_WM_CHECK")?;
        let window = property(root, check, AtomEnum::WINDOW.into(), 1)?
            .value32()?
            .next()?;
        let name = atom("_NET_WM_NAME")?;
        let utf8 = atom("UTF8_STRING")?;
        let reply = property(window, name, utf8, 256)?;
        Some(String::from_utf8_lossy(&reply.value).into_owned())
    })()
    .filter(|name| !name.is_empty());

    Some(X11Facts {
        desktop_windows,
        wm_name,
    })
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

/// One process of this user's, as `/proc` describes it.
pub(crate) struct Process {
    pub(crate) pid: u32,
    pub(crate) comm: String,
    pub(crate) cmdline: Vec<String>,
}

/// Every process this user owns.
///
/// Another user's shell does not draw this session's wallpaper, so a process
/// counts only when its `/proc` entry belongs to the same user as this one.
pub(crate) fn own_processes() -> Vec<Process> {
    use std::os::unix::fs::MetadataExt;

    let Ok(me) = std::fs::metadata("/proc/self").map(|m| m.uid()) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
            if !entry.metadata().is_ok_and(|m| m.uid() == me) {
                return None;
            }
            let comm = std::fs::read_to_string(entry.path().join("comm")).ok()?;
            let cmdline = std::fs::read(entry.path().join("cmdline")).unwrap_or_default();
            Some(Process {
                pid,
                comm: comm.trim_end().to_owned(),
                cmdline: cmdline
                    .split(|&b| b == 0)
                    .filter(|arg| !arg.is_empty())
                    .map(|arg| String::from_utf8_lossy(arg).into_owned())
                    .collect(),
            })
        })
        .collect()
}
