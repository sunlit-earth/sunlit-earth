//! X11: the window manager's clients by EWMH state and geometry, and the
//! root's override-redirect children by geometry, against the `RandR` monitors.
//!
//! Every request for every window is sent before any reply is read, so the
//! whole query is a few round trips to the server whatever the window count.

use x11rb::connection::{Connection, RequestConnection};
use x11rb::cookie::Cookie;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::{
    AtomEnum, ConnectionExt as _, GetGeometryReply, GetPropertyReply, GetWindowAttributesReply,
    MapState, TranslateCoordinatesReply, Window,
};

use crate::cover::{Coverage, Rect, Screen, X11Type, X11Window, x11_coverage};

pub(super) fn probe() -> Coverage {
    match query() {
        Ok(coverage) => coverage,
        Err(reason) => Coverage::Unknown(reason),
    }
}

struct Atoms {
    client_list_stacking: u32,
    current_desktop: u32,
    state: u32,
    state_full_screen: u32,
    state_hidden: u32,
    desktop: u32,
    window_type: u32,
    type_desktop: u32,
    type_dock: u32,
    pid: u32,
}

/// The cookies of everything asked about one window.
struct Asked<'c, C: RequestConnection> {
    state: Cookie<'c, C, GetPropertyReply>,
    desktop: Cookie<'c, C, GetPropertyReply>,
    kind: Cookie<'c, C, GetPropertyReply>,
    pid: Cookie<'c, C, GetPropertyReply>,
    class: Cookie<'c, C, GetPropertyReply>,
    attributes: Cookie<'c, C, GetWindowAttributesReply>,
    geometry: Cookie<'c, C, GetGeometryReply>,
    origin: Cookie<'c, C, TranslateCoordinatesReply>,
}

fn failed(e: &dyn std::fmt::Display) -> String {
    format!("the X server refused a query: {e}")
}

fn intern(conn: &impl Connection) -> Result<Atoms, x11rb::errors::ReplyError> {
    let names = [
        "_NET_CLIENT_LIST_STACKING",
        "_NET_CURRENT_DESKTOP",
        "_NET_WM_STATE",
        "_NET_WM_STATE_FULLSCREEN",
        "_NET_WM_STATE_HIDDEN",
        "_NET_WM_DESKTOP",
        "_NET_WM_WINDOW_TYPE",
        "_NET_WM_WINDOW_TYPE_DESKTOP",
        "_NET_WM_WINDOW_TYPE_DOCK",
        "_NET_WM_PID",
    ];
    let cookies = names
        .iter()
        .map(|name| conn.intern_atom(false, name.as_bytes()))
        .collect::<Result<Vec<_>, _>>()?;
    let mut atoms = Vec::with_capacity(names.len());
    for cookie in cookies {
        atoms.push(cookie.reply()?.atom);
    }
    Ok(Atoms {
        client_list_stacking: atoms[0],
        current_desktop: atoms[1],
        state: atoms[2],
        state_full_screen: atoms[3],
        state_hidden: atoms[4],
        desktop: atoms[5],
        window_type: atoms[6],
        type_desktop: atoms[7],
        type_dock: atoms[8],
        pid: atoms[9],
    })
}

fn query() -> Result<Coverage, String> {
    let (conn, screen_num) =
        x11rb::connect(None).map_err(|e| format!("the X display could not be opened: {e}"))?;
    let root = conn
        .setup()
        .roots
        .get(screen_num)
        .ok_or("the X display has no such screen")?
        .root;
    let atoms = intern(&conn).map_err(|e| failed(&e))?;

    let monitors = conn
        .randr_get_monitors(root, true)
        .map_err(|e| failed(&e))?;
    let stacking = conn
        .get_property(
            false,
            root,
            atoms.client_list_stacking,
            AtomEnum::WINDOW,
            0,
            u32::MAX,
        )
        .map_err(|e| failed(&e))?;
    let current = conn
        .get_property(false, root, atoms.current_desktop, AtomEnum::CARDINAL, 0, 1)
        .map_err(|e| failed(&e))?;
    let tree = conn.query_tree(root).map_err(|e| failed(&e))?;

    let monitors = monitors.reply().map_err(|e| failed(&e))?.monitors;
    let monitor_names = monitors
        .iter()
        .map(|monitor| conn.get_atom_name(monitor.name))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| failed(&e))?;
    let screens: Vec<Screen> = monitors
        .iter()
        .zip(monitor_names)
        .map(|(monitor, name)| Screen {
            name: name
                .reply()
                .ok()
                .map(|reply| String::from_utf8_lossy(&reply.name).into_owned()),
            rect: Rect::from_origin(
                i64::from(monitor.x),
                i64::from(monitor.y),
                i64::from(monitor.width),
                i64::from(monitor.height),
            ),
        })
        .collect();

    let mut clients: Vec<Window> = stacking
        .reply()
        .ok()
        .and_then(|reply| reply.value32().map(Iterator::collect))
        .unwrap_or_default();
    clients.reverse();
    let current_desktop = current
        .reply()
        .ok()
        .and_then(|reply| reply.value32().and_then(|mut values| values.next()));
    let children = tree.reply().map_err(|e| failed(&e))?.children;

    let asked_clients = clients
        .iter()
        .map(|&window| ask(&conn, root, window, &atoms))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| failed(&e))?;
    let asked_children = children
        .iter()
        .rev()
        .map(|&window| ask(&conn, root, window, &atoms))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| failed(&e))?;

    let clients: Vec<X11Window> = asked_clients
        .into_iter()
        .filter_map(|asked| answer(asked, &atoms).map(|(window, _)| window))
        .collect();
    let override_redirect: Vec<X11Window> = asked_children
        .into_iter()
        .filter_map(|asked| answer(asked, &atoms))
        .filter(|(_, override_redirect)| *override_redirect)
        .map(|(window, _)| window)
        .collect();

    Ok(x11_coverage(
        &clients,
        &override_redirect,
        &screens,
        current_desktop,
        std::process::id(),
    ))
}

fn ask<'c, C: Connection>(
    conn: &'c C,
    root: Window,
    window: Window,
    atoms: &Atoms,
) -> Result<Asked<'c, C>, x11rb::errors::ConnectionError> {
    let property = |name: u32, kind: AtomEnum, length: u32| {
        conn.get_property(false, window, name, kind, 0, length)
    };
    Ok(Asked {
        state: property(atoms.state, AtomEnum::ATOM, 32)?,
        desktop: property(atoms.desktop, AtomEnum::CARDINAL, 1)?,
        kind: property(atoms.window_type, AtomEnum::ATOM, 32)?,
        pid: property(atoms.pid, AtomEnum::CARDINAL, 1)?,
        class: property(AtomEnum::WM_CLASS.into(), AtomEnum::STRING, 64)?,
        attributes: conn.get_window_attributes(window)?,
        geometry: conn.get_geometry(window)?,
        origin: conn.translate_coordinates(window, root, 0, 0)?,
    })
}

/// What the replies say of one window, and whether it is override-redirect,
/// or `None` where it is gone.
fn answer<C: RequestConnection>(asked: Asked<'_, C>, atoms: &Atoms) -> Option<(X11Window, bool)> {
    let atoms_of = |reply: Option<GetPropertyReply>| -> Vec<u32> {
        reply
            .and_then(|reply| reply.value32().map(Iterator::collect))
            .unwrap_or_default()
    };
    let first = |reply: Option<GetPropertyReply>| -> Option<u32> {
        reply.and_then(|reply| reply.value32().and_then(|mut values| values.next()))
    };
    let state = atoms_of(asked.state.reply().ok());
    let kind = atoms_of(asked.kind.reply().ok());
    let desktop = first(asked.desktop.reply().ok());
    let pid = first(asked.pid.reply().ok());
    let class = asked
        .class
        .reply()
        .ok()
        .and_then(|reply| class_of(&reply.value));
    let attributes = asked.attributes.reply().ok()?;
    let geometry = asked.geometry.reply().ok()?;
    let origin = asked.origin.reply().ok()?;
    Some((
        X11Window {
            application: class,
            pid,
            rect: Rect::from_origin(
                i64::from(origin.dst_x),
                i64::from(origin.dst_y),
                i64::from(geometry.width),
                i64::from(geometry.height),
            ),
            viewable: attributes.map_state == MapState::VIEWABLE,
            hidden: state.contains(&atoms.state_hidden),
            full_screen: state.contains(&atoms.state_full_screen),
            desktop,
            kind: if kind.contains(&atoms.type_desktop) {
                X11Type::Desktop
            } else if kind.contains(&atoms.type_dock) {
                X11Type::Dock
            } else {
                X11Type::Other
            },
        },
        attributes.override_redirect,
    ))
}

/// The class half of `WM_CLASS`, else its instance half.
fn class_of(value: &[u8]) -> Option<String> {
    let parts: Vec<&[u8]> = value
        .split(|&b| b == 0)
        .filter(|part| !part.is_empty())
        .collect();
    parts
        .get(1)
        .or_else(|| parts.first())
        .map(|part| String::from_utf8_lossy(part).into_owned())
}
