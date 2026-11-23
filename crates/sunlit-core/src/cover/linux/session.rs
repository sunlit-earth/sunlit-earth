//! GNOME on Wayland: `org.gnome.SessionManager`'s idle inhibition, which
//! stands in for full screen where no window state of native Wayland clients
//! can be read. It also fires for a video in a window, and for an application
//! that keeps the screen awake for good.

use zbus::blocking::Connection;
use zbus::zvariant::OwnedObjectPath;

use crate::cover::{Cover, Coverage, Signal};

const SERVICE: &str = "org.gnome.SessionManager";
const PATH: &str = "/org/gnome/SessionManager";
const INHIBITOR: &str = "org.gnome.SessionManager.Inhibitor";

/// `GsmInhibitorFlag`'s idle bit.
const IDLE: u32 = 8;

pub(super) fn probe(bus: &Connection) -> Coverage {
    match query(bus) {
        Ok(coverage) => coverage,
        Err(e) => Coverage::Unknown(format!("the session manager did not answer: {e}")),
    }
}

fn query(bus: &Connection) -> zbus::Result<Coverage> {
    let inhibited: bool = bus
        .call_method(Some(SERVICE), PATH, Some(SERVICE), "IsInhibited", &(IDLE,))?
        .body()
        .deserialize()?;
    if !inhibited {
        return Ok(Coverage::Clear);
    }
    let inhibitors: Vec<OwnedObjectPath> = bus
        .call_method(Some(SERVICE), PATH, Some(SERVICE), "GetInhibitors", &())?
        .body()
        .deserialize()?;
    let application = inhibitors.iter().find_map(|path| {
        let flags: u32 = bus
            .call_method(
                Some(SERVICE),
                path.as_str(),
                Some(INHIBITOR),
                "GetFlags",
                &(),
            )
            .ok()?
            .body()
            .deserialize()
            .ok()?;
        if flags & IDLE == 0 {
            return None;
        }
        bus.call_method(
            Some(SERVICE),
            path.as_str(),
            Some(INHIBITOR),
            "GetAppId",
            &(),
        )
        .ok()?
        .body()
        .deserialize::<String>()
        .ok()
        .filter(|app| !app.is_empty())
    });
    Ok(Coverage::Covered(Cover {
        application,
        pid: None,
        monitor: None,
        signal: Signal::IdleInhibited,
    }))
}
