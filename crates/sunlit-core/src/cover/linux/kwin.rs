//! Plasma on Wayland: a one-shot `KWin` script that reports every window's full
//! screen state and geometry back over D-Bus.
//!
//! `KWin` implements neither the wlr nor the ext foreign toplevel protocol, and
//! its own window management protocol is reserved to clients a `.desktop` file
//! declares before Plasma 6.8. Its scripting interface has no such check: the
//! probe writes a script with a token of its own, has `KWin` load and run it,
//! and the script calls back to an object the probe exports on its session bus
//! connection, by the connection's unique name.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crossbeam_channel::Sender;
use zbus::blocking::Connection;

use super::probe::PROBE_DEADLINE;
use crate::cover::{Candidate, Coverage, Rect, Screen, coverage_of};

const KWIN: &str = "org.kde.KWin";
const OBJECT: &str = "/org/sunlitearth/CoverProbe";
const INTERFACE: &str = "org.sunlitearth.CoverProbe";

/// The probe waiting for a report, by its token, and where to hand it.
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(0);

type Waiting = Arc<Mutex<Option<(String, Sender<String>)>>>;

pub(crate) struct KWinRoute {
    exported: OnceLock<Result<(), String>>,
    waiting: Waiting,
}

struct Reports {
    waiting: Waiting,
}

#[zbus::interface(name = "org.sunlitearth.CoverProbe")]
impl Reports {
    fn report(&self, token: &str, windows: &str) {
        let waiting = self
            .waiting
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((expected, reply)) = waiting.as_ref()
            && expected == token
        {
            let _ = reply.try_send(windows.to_owned());
        }
    }
}

impl KWinRoute {
    pub(crate) fn new() -> Self {
        Self {
            exported: OnceLock::new(),
            waiting: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn probe(&self, bus: &Connection) -> Coverage {
        match self.query(bus) {
            Ok(coverage) => coverage,
            Err(reason) => Coverage::Unknown(reason),
        }
    }

    fn query(&self, bus: &Connection) -> Result<Coverage, String> {
        self.exported
            .get_or_init(|| {
                bus.object_server()
                    .at(
                        OBJECT,
                        Reports {
                            waiting: Arc::clone(&self.waiting),
                        },
                    )
                    .map(|_| ())
                    .map_err(|e| format!("the report object could not be exported: {e}"))
            })
            .clone()?;
        let unique = bus
            .unique_name()
            .ok_or("the session bus connection has no unique name")?
            .to_string();

        let token = format!(
            "{}-{}",
            std::process::id(),
            NEXT_TOKEN.fetch_add(1, Ordering::Relaxed)
        );
        let name = format!("sunlit-earth-cover-{token}");
        let dir = crate::user_state::cache_dir().unwrap_or_else(std::env::temp_dir);
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = dir.join(format!("{name}.js"));
        std::fs::write(&path, script(&unique, &token))
            .map_err(|e| format!("the script could not be written to {}: {e}", path.display()))?;

        // A channel of one, read below by this query until its deadline, and
        // filled by the report object on zbus's executor thread when KWin
        // calls back with this token.
        let (tx, rx) = crossbeam_channel::bounded(1);
        *self
            .waiting
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((token, tx));
        let report = run(bus, &path, &name).and_then(|()| {
            rx.recv_timeout(PROBE_DEADLINE).map_err(|_| {
                format!(
                    "the script did not report within {} ms",
                    PROBE_DEADLINE.as_millis()
                )
            })
        });
        *self
            .waiting
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        let _ = bus.call_method(
            Some(KWIN),
            "/Scripting",
            Some("org.kde.kwin.Scripting"),
            "unloadScript",
            &(name.as_str(),),
        );
        let _ = std::fs::remove_file(&path);
        let (candidates, screens) = parse(&report?, std::process::id());
        Ok(coverage_of(&candidates, &screens))
    }
}

/// Load and start the script at `path` under `name`.
fn run(bus: &Connection, path: &std::path::Path, name: &str) -> Result<(), String> {
    let failed = |e: zbus::Error| format!("KWin refused the script: {e}");
    let id: i32 = bus
        .call_method(
            Some(KWIN),
            "/Scripting",
            Some("org.kde.kwin.Scripting"),
            "loadScript",
            &(path.to_string_lossy().as_ref(), name),
        )
        .map_err(failed)?
        .body()
        .deserialize()
        .map_err(failed)?;
    if id < 0 {
        return Err("KWin could not load the script".to_owned());
    }
    bus.call_method(
        Some(KWIN),
        format!("/Scripting/Script{id}").as_str(),
        Some("org.kde.kwin.Script"),
        "run",
        &(),
    )
    .map_err(failed)?;
    Ok(())
}

/// The script: one line per normal, visible window on the current desktop,
/// tab separated, sent to `unique` with `token`.
fn script(unique: &str, token: &str) -> String {
    format!(
        r#"(function () {{
const current = workspace.currentDesktop;
const lines = [];
for (const w of workspace.windowList()) {{
    if (!w.normalWindow || w.minimized || w.deleted) continue;
    if (!(w.onAllDesktops || w.desktops.some(d => d.id === current.id))) continue;
    const g = w.frameGeometry;
    const o = w.output;
    const og = o ? o.geometry : {{x: 0, y: 0, width: 0, height: 0}};
    lines.push([w.pid, w.fullScreen ? 1 : 0, g.x, g.y, g.width, g.height,
                o ? o.name : "", og.x, og.y, og.width, og.height,
                w.resourceClass].join("\t"));
}}
lines.reverse();
callDBus("{unique}", "{OBJECT}", "{INTERFACE}", "Report", "{token}", lines.join("\n"));
}})();
"#
    )
}

/// The windows and outputs a report names, the windows of `own_pid` left
/// out. `KWin`'s geometry is in logical pixels and fractional under scaling, so
/// a window's edges are rounded to the nearest pixel before they are compared
/// with its output's.
fn parse(report: &str, own_pid: u32) -> (Vec<Candidate>, Vec<Screen>) {
    let mut candidates = Vec::new();
    let mut screens: Vec<Screen> = Vec::new();
    for line in report.lines() {
        let fields: Vec<&str> = line.splitn(12, '\t').collect();
        let [pid, full_screen, x, y, w, h, output, ox, oy, ow, oh, class] = fields[..] else {
            continue;
        };
        let number = |field: &str| field.parse::<f64>().ok();
        let (Some(x), Some(y), Some(w), Some(h)) = (number(x), number(y), number(w), number(h))
        else {
            continue;
        };
        let pid = pid.parse::<u32>().ok();
        if pid == Some(own_pid) {
            continue;
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "window coordinates are far inside i64"
        )]
        let round = |value: f64| value.round() as i64;
        if !output.is_empty()
            && !screens
                .iter()
                .any(|screen| screen.name.as_deref() == Some(output))
            && let (Some(ox), Some(oy), Some(ow), Some(oh)) =
                (number(ox), number(oy), number(ow), number(oh))
        {
            screens.push(Screen {
                name: Some(output.to_owned()),
                rect: Rect {
                    left: round(ox),
                    top: round(oy),
                    right: round(ox + ow),
                    bottom: round(oy + oh),
                },
            });
        }
        candidates.push(Candidate {
            application: Some(class.to_owned()).filter(|class| !class.is_empty()),
            pid,
            rect: Rect {
                left: round(x),
                top: round(y),
                right: round(x + w),
                bottom: round(y + h),
            },
            full_screen: full_screen == "1",
        });
    }
    (candidates, screens)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cover::Signal;

    #[test]
    fn a_fractional_window_as_wide_as_its_output_covers_it() {
        let report = "1523\t0\t0\t0\t1828.5714285714284\t1028.5714285714284\tVirtual-1\t0\t0\t1829\t1029\torg.example.Player";
        let (candidates, screens) = parse(report, 1);
        let coverage = coverage_of(&candidates, &screens);
        let Coverage::Covered(cover) = coverage else {
            panic!("covered");
        };
        assert_eq!(cover.monitor.as_deref(), Some("Virtual-1"));
        assert_eq!(cover.pid, Some(1523));
        assert_eq!(cover.signal, Signal::Geometry);
    }

    #[test]
    fn this_process_and_malformed_lines_are_left_out() {
        let report = "1\t1\t0\t0\t10\t10\tDP-1\t0\t0\t1920\t1080\tself\nnot a line";
        let (candidates, _) = parse(report, 1);
        assert_eq!(candidates.len(), 0);
    }

    #[test]
    fn the_script_names_its_caller_and_token() {
        let script = script(":1.42", "7-3");
        assert!(script.contains(r#"callDBus(":1.42", "/org/sunlitearth/CoverProbe""#));
        assert!(script.contains(r#""Report", "7-3""#));
    }
}
