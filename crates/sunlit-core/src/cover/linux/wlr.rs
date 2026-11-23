//! wlroots compositors: the foreign toplevel manager's state of every
//! toplevel, over a Wayland connection of the probe's own.

use std::collections::HashMap;

use wayland_client::backend::ObjectId;
use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, event_created_child};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1 as handle, zwlr_foreign_toplevel_manager_v1 as manager,
};

use crate::cover::{Coverage, WlrToplevel, wlr_coverage};

/// The global this route needs.
pub(super) const MANAGER: &str = "zwlr_foreign_toplevel_manager_v1";

/// The first version whose toplevel state carries full screen.
const FULL_SCREEN_SINCE: u32 = 2;

pub(super) fn probe() -> Coverage {
    match query() {
        Ok(toplevels) => wlr_coverage(&toplevels),
        Err(reason) => Coverage::Unknown(reason),
    }
}

#[derive(Default)]
struct State {
    manager: Option<(u32, u32)>,
    outputs: Vec<(u32, u32)>,
    output_names: HashMap<ObjectId, String>,
    toplevels: HashMap<ObjectId, WlrToplevel>,
    /// The outputs each toplevel is on, by object, named once every output
    /// has said its name.
    entered: HashMap<ObjectId, Vec<ObjectId>>,
}

fn query() -> Result<Vec<WlrToplevel>, String> {
    let conn = Connection::connect_to_env()
        .map_err(|e| format!("the Wayland display could not be reached: {e}"))?;
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    let registry = conn.display().get_registry(&qh, ());
    let mut state = State::default();
    let failed = |e: &dyn std::fmt::Display| format!("the compositor did not answer: {e}");
    queue.roundtrip(&mut state).map_err(|e| failed(&e))?;

    let (name, version) = state
        .manager
        .ok_or_else(|| format!("the compositor no longer offers {MANAGER}"))?;
    if version < FULL_SCREEN_SINCE {
        return Err(format!(
            "{MANAGER} is at version {version}, which has no full screen state"
        ));
    }
    let outputs: Vec<wl_output::WlOutput> = state
        .outputs
        .iter()
        .map(|&(name, version)| registry.bind(name, version.min(4), &qh, ()))
        .collect();
    let manager: manager::ZwlrForeignToplevelManagerV1 =
        registry.bind(name, version.min(3), &qh, ());
    queue.roundtrip(&mut state).map_err(|e| failed(&e))?;
    queue.roundtrip(&mut state).map_err(|e| failed(&e))?;
    manager.stop();
    let _ = conn.flush();

    for (index, output) in outputs.iter().enumerate() {
        state
            .output_names
            .entry(output.id())
            .or_insert_with(|| format!("output {index}"));
    }
    let State {
        output_names,
        mut toplevels,
        entered,
        ..
    } = state;
    for (toplevel, on) in entered {
        if let Some(found) = toplevels.get_mut(&toplevel) {
            found.outputs = on
                .iter()
                .filter_map(|output| output_names.get(output).cloned())
                .collect();
        }
    }
    Ok(toplevels.into_values().collect())
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            if interface == MANAGER {
                state.manager = Some((name, version));
            } else if interface == wl_output::WlOutput::interface().name {
                state.outputs.push((name, version));
            }
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for State {
    fn event(
        state: &mut Self,
        output: &wl_output::WlOutput,
        event: wl_output::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event {
            state.output_names.insert(output.id(), name);
        }
    }
}

impl Dispatch<manager::ZwlrForeignToplevelManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &manager::ZwlrForeignToplevelManagerV1,
        _: manager::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }

    event_created_child!(State, manager::ZwlrForeignToplevelManagerV1, [
        manager::EVT_TOPLEVEL_OPCODE => (handle::ZwlrForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<handle::ZwlrForeignToplevelHandleV1, ()> for State {
    fn event(
        state: &mut Self,
        toplevel: &handle::ZwlrForeignToplevelHandleV1,
        event: handle::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let id = toplevel.id();
        match event {
            handle::Event::AppId { app_id } => {
                state.toplevels.entry(id).or_default().app_id = Some(app_id);
            }
            handle::Event::State { state: flags } => {
                let full_screen = flags
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|chunk| u32::from_ne_bytes(*chunk))
                    .any(|flag| flag == handle::State::Fullscreen as u32);
                state.toplevels.entry(id).or_default().full_screen = full_screen;
            }
            handle::Event::OutputEnter { output } => {
                state.toplevels.entry(id.clone()).or_default();
                state.entered.entry(id).or_default().push(output.id());
            }
            handle::Event::OutputLeave { output } => {
                if let Some(on) = state.entered.get_mut(&id) {
                    on.retain(|entered| *entered != output.id());
                }
            }
            handle::Event::Closed => {
                state.toplevels.remove(&id);
                state.entered.remove(&id);
            }
            _ => {}
        }
    }
}
