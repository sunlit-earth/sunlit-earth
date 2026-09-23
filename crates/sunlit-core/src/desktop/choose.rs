//! Which setter this session gets, and on what evidence.
//!
//! A pure function over [`Session`], which is every question the choice asks of
//! the machine it runs on. The live answers are in `probe.rs`; the tests answer
//! from a table, which is what lets the whole choice be checked on a machine
//! with no Linux desktop at all.

use std::path::{Path, PathBuf};

use super::{
    APP_ID, AWWW, Backend, COMPOSITING_SHELLS, DEEPIN_BUS_NAMES, DEEPIN_LEGACY, DESKTOP_ENV,
    DESKTOP_WINDOW_OWNERS, GTK_PORTAL_BACKEND, Gate, Kind, LADDER, LAYER_SHELL, PORTAL,
    ROOT_PIXMAP, ROWS, Row, SWAY, SWAYBG, SWWW, WPAPERD, names_a_file_under,
};

/// The variables walked, in order, for the name a session gives itself.
///
/// The second and third only when the first is empty: a display manager that
/// starts a bare window manager often exports no `XDG_CURRENT_DESKTOP` and still
/// names the session it started in one of the others.
const NAME_SOURCES: [&str; 3] = [DESKTOP_ENV, "XDG_SESSION_DESKTOP", "DESKTOP_SESSION"];

/// The variable that forces a setter by name, past everything that would have
/// chosen one.
pub const FORCE_ENV: &str = "SUNLIT_EARTH_WALLPAPER_SETTER";

/// What the choice needs to know about the session it is choosing for.
///
/// Each question is asked only when a rung reaches it, so a session decided by
/// its first token asks for nothing else.
pub trait Session {
    /// An environment variable, blank being the same as unset.
    fn var(&self, name: &str) -> Option<String>;
    /// Whether a program of this name is on `PATH`.
    fn on_path(&self, program: &str) -> bool;
    /// Whether a process of this user with this name runs.
    fn process_running(&self, name: &str) -> bool;
    /// Whether this name has an owner on the session bus.
    fn bus_name_owned(&self, name: &str) -> bool;
    /// Whether something exists at this path: a socket, most often.
    fn path_exists(&self, path: &Path) -> bool;
    /// What the X server says about the desktop, or `None` where no X display
    /// could be opened.
    fn x11(&self) -> Option<X11Facts>;
    /// The interfaces the Wayland compositor advertises, or `None` where no
    /// Wayland display could be opened.
    fn wayland_globals(&self) -> Option<Vec<String>>;
    /// The command lines of this user's processes with this name.
    fn processes_named(&self, name: &str) -> Vec<Vec<String>>;
    /// The directory this app publishes its wallpapers into.
    fn wallpaper_dir(&self) -> Option<PathBuf>;
    /// What the XDG desktop portal offers, or `None` where the session bus
    /// could not be asked.
    fn portal(&self) -> Option<PortalFacts>;
}

/// What the portal rung asks of the session bus.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PortalFacts {
    /// Whether `org.freedesktop.portal.Wallpaper` is exported.
    pub wallpaper: bool,
    /// Whether `org.freedesktop.host.portal.Registry` is, which lets the app
    /// be known by its own id.
    pub registry: bool,
    /// The backends that implement `org.freedesktop.impl.portal.Wallpaper`,
    /// by the last part of their bus name: `gnome`, `gtk`, `kde`.
    pub wallpaper_backends: Vec<String>,
}

/// The two things an X11 session is asked before its root is painted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct X11Facts {
    /// The `WM_CLASS` of every window typed `_NET_WM_WINDOW_TYPE_DESKTOP`, both
    /// halves of each.
    pub desktop_windows: Vec<Vec<String>>,
    /// The window manager's own name, from `_NET_SUPPORTING_WM_CHECK`, where
    /// it sets one.
    pub wm_name: Option<String>,
}

/// The windows typed as a desktop, from the window manager's client list where
/// it names one, and otherwise from the root's own children.
///
/// A session whose window manager never ran has no `_NET_CLIENT_LIST`, and one
/// whose window manager died keeps the last list it wrote, which a desktop
/// window mapped since is not on. The tree is therefore walked whenever the
/// list yields no desktop window. A child without the type is also looked
/// through one level down, since a reparenting window manager puts the type on
/// the client inside its frame.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn desktop_windows<W: Copy>(
    client_list: Option<Vec<W>>,
    top_level: impl FnOnce() -> Vec<W>,
    children: impl Fn(W) -> Vec<W>,
    is_desktop: impl Fn(W) -> bool,
) -> Vec<W> {
    let listed: Vec<W> = client_list
        .unwrap_or_default()
        .into_iter()
        .filter(|&w| is_desktop(w))
        .collect();
    if !listed.is_empty() {
        return listed;
    }
    top_level()
        .into_iter()
        .flat_map(|window| {
            if is_desktop(window) {
                vec![window]
            } else {
                children(window)
                    .into_iter()
                    .filter(|&w| is_desktop(w))
                    .collect()
            }
        })
        .collect()
}

/// The setter a session gets, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub backend: Backend,
    /// The facts the choice rests on, in the order they were found.
    pub evidence: Vec<String>,
}

impl Choice {
    /// The evidence as one sentence, for a log line or a report.
    pub fn explanation(&self) -> String {
        format!("{}: {}", self.backend.desktop, self.evidence.join("; "))
    }
}

/// One way of setting the wallpaper that was considered and did not apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declined {
    /// What was tried: a desktop row or a rung of the ladder.
    pub rung: String,
    pub reason: String,
}

/// No setter for this session, with everything that was tried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub declined: Vec<Declined>,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "no way to set the wallpaper was found in this session")?;
        for declined in &self.declined {
            write!(f, "; {}: {}", declined.rung, declined.reason)?;
        }
        write!(
            f,
            ". {FORCE_ENV} can name one of: {}.",
            setter_names().join(", ")
        )
    }
}

/// The setter for `session`, or every reason there is none.
pub fn choose(session: &dyn Session) -> Result<Choice, Refusal> {
    if let Some(name) = session.var(FORCE_ENV) {
        return forced(session, name.trim());
    }
    let mut declined = Vec::new();
    if let Some(choice) = from_names(session, &mut declined) {
        return Ok(choice);
    }
    if let Some(choice) = from_sway_socket(session, &mut declined) {
        return Ok(choice);
    }
    if let Some(choice) = ladder(session, &mut declined) {
        return Ok(choice);
    }
    Err(Refusal { declined })
}

/// The rungs for a session no row claimed, first working one wins.
fn ladder(session: &dyn Session, declined: &mut Vec<Declined>) -> Option<Choice> {
    let mut decline = |rung: &str, reason: String| {
        declined.push(Declined {
            rung: rung.to_owned(),
            reason,
        });
    };
    let wayland = session.var("WAYLAND_DISPLAY");
    let display = session.var("DISPLAY");
    match (&wayland, &display) {
        (Some(wayland), _) => {
            match wayland_rungs(session, wayland) {
                Ok(choice) => return Some(choice),
                Err(reasons) => {
                    for (rung, reason) in reasons {
                        decline(rung, reason);
                    }
                }
            }
            let reason = format!("WAYLAND_DISPLAY={wayland}, so the X11 root is not what is shown");
            decline("desktop window owner", reason.clone());
            decline("root pixmap", reason);
        }
        (None, None) => {
            for rung in WAYLAND_RUNGS {
                decline(rung, "WAYLAND_DISPLAY is not set".to_owned());
            }
            decline(
                "desktop window owner",
                "neither WAYLAND_DISPLAY nor DISPLAY is set".to_owned(),
            );
            decline("root pixmap", "DISPLAY is not set".to_owned());
        }
        (None, Some(display)) => {
            for rung in WAYLAND_RUNGS {
                decline(rung, "WAYLAND_DISPLAY is not set".to_owned());
            }
            match x11_rungs(session, display) {
                Ok(choice) => return Some(choice),
                Err(reasons) => {
                    for (rung, reason) in reasons {
                        decline(rung, reason);
                    }
                }
            }
        }
    }
    let mut evidence = Vec::new();
    match portal_applies(session, &mut evidence) {
        Ok(()) => Some(Choice {
            backend: PORTAL,
            evidence,
        }),
        Err(reason) => {
            decline("portal", reason);
            None
        }
    }
}

/// Whether the portal can set a wallpaper that something draws.
///
/// The GTK backend writes GNOME's key, so where it is the only backend that
/// implements the wallpaper portal and no GNOME shell runs, a success would
/// change nothing on screen.
fn portal_applies(session: &dyn Session, evidence: &mut Vec<String>) -> Result<(), String> {
    let Some(portal) = session.portal() else {
        return Err("the session bus could not be asked for the portal".to_owned());
    };
    if !portal.wallpaper {
        return Err(
            "org.freedesktop.portal.Wallpaper is not exported, so no portal backend here \
             sets wallpapers"
                .to_owned(),
        );
    }
    evidence.push("org.freedesktop.portal.Wallpaper exported".to_owned());
    if portal.wallpaper_backends == [GTK_PORTAL_BACKEND] {
        if !session.process_running("gnome-shell") {
            return Err(
                "the only portal backend that sets wallpapers is xdg-desktop-portal-gtk, which \
                 writes GNOME's key, and no gnome-shell runs to draw it"
                    .to_owned(),
            );
        }
        evidence.push("the GTK backend, with gnome-shell running to draw its key".to_owned());
    } else if !portal.wallpaper_backends.is_empty() {
        evidence.push(format!(
            "wallpaper backends: {}",
            portal.wallpaper_backends.join(", ")
        ));
    }
    evidence.push(if portal.registry {
        format!("registered with the portal as {APP_ID}")
    } else {
        "no portal Registry, so the permission is shared with every other unidentified app"
            .to_owned()
    });
    Ok(())
}

/// The Wayland rungs, in the order they are tried.
const WAYLAND_RUNGS: [&str; 3] = ["awww", "wpaperd", "swaybg"];

/// The three Wayland rungs: a daemon the user already runs, then a `swaybg` of
/// this app's own.
///
/// A running daemon comes first so the app does not put a second background
/// surface under one the user already has.
fn wayland_rungs(
    session: &dyn Session,
    wayland: &str,
) -> Result<Choice, Vec<(&'static str, String)>> {
    let mut reasons = Vec::new();
    let session_line = format!("Wayland session on WAYLAND_DISPLAY={wayland}");
    let runtime = session.var("XDG_RUNTIME_DIR");

    let daemon = |backends: &[(Backend, PathBuf)]| -> Result<Choice, String> {
        let mut found = None;
        for (backend, socket) in backends {
            if !session.path_exists(socket) {
                continue;
            }
            let mut evidence = vec![
                session_line.clone(),
                format!("{} daemon socket at {}", backend.desktop, socket.display()),
            ];
            match present(session, backend, &mut evidence) {
                Ok(()) => {
                    return Ok(Choice {
                        backend: *backend,
                        evidence,
                    });
                }
                Err(reason) => found = Some(reason),
            }
        }
        Err(found.unwrap_or_else(|| {
            let sockets: Vec<String> = backends
                .iter()
                .map(|(_, socket)| socket.display().to_string())
                .collect();
            format!("no daemon listens: nothing at {}", sockets.join(" or "))
        }))
    };

    if let Some(runtime) = &runtime {
        let display = Path::new(wayland).file_name().map_or_else(
            || wayland.to_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
        let runtime = Path::new(runtime);
        let awww = [
            (AWWW, runtime.join(format!("{display}-awww-daemon.sock"))),
            (SWWW, runtime.join(format!("{display}-swww-daemon.sock"))),
        ];
        match daemon(&awww) {
            Ok(choice) => return Ok(choice),
            Err(reason) => reasons.push(("awww", reason)),
        }
        match daemon(&[(WPAPERD, runtime.join("wpaperd.sock"))]) {
            Ok(choice) => return Ok(choice),
            Err(reason) => reasons.push(("wpaperd", reason)),
        }
    } else {
        let reason = "XDG_RUNTIME_DIR is not set, so there is no daemon socket to look for";
        reasons.push(("awww", reason.to_owned()));
        reasons.push(("wpaperd", reason.to_owned()));
    }

    let Some(globals) = session.wayland_globals() else {
        reasons.push((
            "swaybg",
            format!("WAYLAND_DISPLAY={wayland} could not be opened to ask what it offers"),
        ));
        return Err(reasons);
    };
    if !globals.iter().any(|global| global == LAYER_SHELL) {
        reasons.push((
            "swaybg",
            format!("the compositor does not offer {LAYER_SHELL}, which swaybg draws with"),
        ));
        return Err(reasons);
    }
    let mut evidence = vec![session_line, format!("{LAYER_SHELL} advertised")];
    if let Err(reason) = present(session, &SWAYBG, &mut evidence) {
        reasons.push(("swaybg", reason));
        return Err(reasons);
    }
    let foreign = match session.wallpaper_dir() {
        Some(dir) => session
            .processes_named("swaybg")
            .iter()
            .filter(|cmdline| !names_a_file_under(cmdline, &dir))
            .count(),
        None => 0,
    };
    if foreign > 0 {
        evidence.push(format!(
            "{foreign} swaybg not started by this app already runs and is left alone"
        ));
    }
    Ok(Choice {
        backend: SWAYBG,
        evidence,
    })
}

/// The two X11 rungs: the program that owns a desktop window, or the root.
fn x11_rungs(session: &dyn Session, display: &str) -> Result<Choice, Vec<(&'static str, String)>> {
    let owner = "desktop window owner";
    let root = "root pixmap";
    let Some(facts) = session.x11() else {
        let reason = format!("DISPLAY={display} could not be opened");
        return Err(vec![(owner, reason.clone()), (root, reason)]);
    };
    let mut evidence = vec![format!("X11 session on DISPLAY={display}")];
    if let Some(classes) = facts.desktop_windows.first() {
        let class = classes.join(".");
        let row = DESKTOP_WINDOW_OWNERS
            .iter()
            .find(|(owner, _)| classes.iter().any(|c| c.eq_ignore_ascii_case(owner)))
            .and_then(|(_, setter)| ROWS.iter().find(|row| row.backend.setter == *setter));
        let covered = format!("a desktop window ({class}) covers the root");
        let Some(row) = row else {
            return Err(vec![
                (
                    owner,
                    format!("the desktop window's class {class} has no row here"),
                ),
                (root, covered),
            ]);
        };
        evidence.push(format!(
            "desktop window of class {class}, which is {}'s",
            row.backend.desktop
        ));
        let backend = resolve(session, row.backend, &mut evidence);
        return match present(session, &backend, &mut evidence) {
            Ok(()) => Ok(Choice { backend, evidence }),
            Err(reason) => Err(vec![(owner, reason), (root, covered)]),
        };
    }
    let no_window = "no desktop window".to_owned();
    let wm = facts.wm_name.as_deref();
    if let Some((name, shell)) = wm.and_then(|name| {
        COMPOSITING_SHELLS
            .iter()
            .find(|shell| name.contains(*shell))
            .map(|shell| (name, shell))
    }) {
        return Err(vec![
            (owner, no_window),
            (
                root,
                format!(
                    "the window manager is {name}, and {shell} draws its own background \
                     over the root"
                ),
            ),
        ]);
    }
    evidence.push(no_window);
    evidence.push(wm.map_or_else(
        || "the window manager names itself nowhere".to_owned(),
        |name| format!("window manager {name}"),
    ));
    Ok(Choice {
        backend: ROOT_PIXMAP,
        evidence,
    })
}

/// Every setter [`FORCE_ENV`] can name: the table's rows, then the ladder's.
fn forcible() -> impl Iterator<Item = &'static Backend> {
    ROWS.iter().map(|row| &row.backend).chain(LADDER)
}

/// Every name [`FORCE_ENV`] accepts, in the order the table has them.
fn setter_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = Vec::new();
    for backend in forcible() {
        if !names.contains(&backend.setter) {
            names.push(backend.setter);
        }
    }
    names
}

/// The setter [`FORCE_ENV`] names, if it is there to run.
///
/// Detection is skipped and presence is not: a forced setter whose program is
/// missing is still a refusal, and it says which variable asked for it.
fn forced(session: &dyn Session, name: &str) -> Result<Choice, Refusal> {
    let refuse = |reason: String| Refusal {
        declined: vec![Declined {
            rung: format!("{FORCE_ENV}={name}"),
            reason,
        }],
    };
    let Some(backend) = forcible().find(|backend| backend.setter.eq_ignore_ascii_case(name)) else {
        return Err(refuse("no setter goes by that name".to_owned()));
    };
    let mut evidence = vec![format!("{FORCE_ENV}={name}")];
    let backend = resolve(session, *backend, &mut evidence);
    present(session, &backend, &mut evidence).map_err(refuse)?;
    Ok(Choice { backend, evidence })
}

/// The first row of the table the session names itself as, whose setter is
/// there and whose gate holds.
///
/// A row that does not apply does not end the walk: the next token may name a
/// desktop that can do the job, and the refusal lists the row either way.
fn from_names(session: &dyn Session, declined: &mut Vec<Declined>) -> Option<Choice> {
    let mut named_any = false;
    for source in NAME_SOURCES {
        let Some(value) = session.var(source) else {
            continue;
        };
        for token in tokens(&value) {
            let Some(row) = row_for(&token) else {
                continue;
            };
            named_any = true;
            let evidence = vec![format!("{source}={value} names {}", row.backend.desktop)];
            match applies(session, row, evidence) {
                Ok(choice) => return Some(choice),
                Err(reason) => declined.push(Declined {
                    rung: row.backend.desktop.to_owned(),
                    reason,
                }),
            }
        }
        if source == DESKTOP_ENV {
            break;
        }
    }
    if !named_any {
        declined.push(Declined {
            rung: "desktop table".to_owned(),
            reason: names_nothing(session),
        });
    }
    None
}

/// sway by its socket, for a session that runs it without naming it.
fn from_sway_socket(session: &dyn Session, declined: &mut Vec<Declined>) -> Option<Choice> {
    if declined.iter().any(|d| d.rung == SWAY.desktop) {
        return None;
    }
    let socket = session.var("SWAYSOCK")?;
    let row = ROWS.iter().find(|row| row.backend == SWAY)?;
    match applies(session, row, vec![format!("SWAYSOCK={socket}")]) {
        Ok(choice) => Some(choice),
        Err(reason) => {
            declined.push(Declined {
                rung: SWAY.desktop.to_owned(),
                reason,
            });
            None
        }
    }
}

/// Whether a row the session named applies: its gate holds and its setter is
/// there.
fn applies(session: &dyn Session, row: &Row, mut evidence: Vec<String>) -> Result<Choice, String> {
    let desktop = row.backend.desktop;
    match row.gate {
        Gate::None => {}
        Gate::Process(names) => match names.iter().find(|name| session.process_running(name)) {
            Some(name) => evidence.push(format!("{name} running")),
            None => {
                return Err(format!(
                    "the session names {desktop}, and none of {} runs to draw its wallpaper",
                    names.join(", ")
                ));
            }
        },
        Gate::HyprpaperSocket => match hyprpaper_socket(session) {
            Some(socket) if session.path_exists(&socket) => {
                evidence.push(format!("hyprpaper socket at {}", socket.display()));
            }
            Some(socket) => {
                return Err(format!(
                    "no hyprpaper socket at {}, so hyprpaper is not running",
                    socket.display()
                ));
            }
            None => {
                return Err(
                    "XDG_RUNTIME_DIR or HYPRLAND_INSTANCE_SIGNATURE is not set, so there is \
                     no hyprpaper socket to look for"
                        .to_owned(),
                );
            }
        },
    }
    let backend = resolve(session, row.backend, &mut evidence);
    present(session, &backend, &mut evidence)?;
    Ok(Choice { backend, evidence })
}

/// The variant of a row this session has, where a row has more than one.
fn resolve(session: &dyn Session, backend: Backend, evidence: &mut Vec<String>) -> Backend {
    if backend == AWWW && !session.on_path("awww") && session.on_path("swww") {
        evidence.push("swww, awww's name before its rename, on PATH".to_owned());
        return SWWW;
    }
    if let Kind::Deepin { .. } = backend.kind {
        let [current, legacy] = DEEPIN_BUS_NAMES;
        if !session.bus_name_owned(current) && session.bus_name_owned(legacy) {
            evidence.push(format!("{legacy} on the session bus, {current} not"));
            return DEEPIN_LEGACY;
        }
    }
    backend
}

/// Whether the setter a choice settled on can run here.
fn present(
    session: &dyn Session,
    backend: &Backend,
    evidence: &mut Vec<String>,
) -> Result<(), String> {
    if let Some(program) = backend.program {
        if !session.on_path(program) {
            return Err(format!(
                "this is {desktop}, whose wallpaper is set with `{program}`, and that \
                 program is not on PATH",
                desktop = backend.desktop,
            ));
        }
        evidence.push(format!("{program} on PATH"));
    }
    if backend.kind == Kind::RootPixmap && session.x11().is_none() {
        return Err("no X display could be opened to paint the root of".to_owned());
    }
    if backend.kind == Kind::Portal && !session.portal().is_some_and(|p| p.wallpaper) {
        return Err(
            "org.freedesktop.portal.Wallpaper is not exported on the session bus".to_owned(),
        );
    }
    Ok(())
}

/// Where hyprpaper listens, for the Hyprland instance this session runs.
pub(crate) fn hyprpaper_socket(session: &dyn Session) -> Option<PathBuf> {
    let runtime = session.var("XDG_RUNTIME_DIR")?;
    let instance = session.var("HYPRLAND_INSTANCE_SIGNATURE")?;
    Some(
        Path::new(&runtime)
            .join("hypr")
            .join(instance)
            .join(".hyprpaper.sock"),
    )
}

/// Why the session's own name picked no row.
fn names_nothing(session: &dyn Session) -> String {
    let set: Vec<String> = NAME_SOURCES
        .iter()
        .filter_map(|name| session.var(name).map(|value| format!("{name}={value}")))
        .collect();
    if set.is_empty() {
        format!("{} are not set", NAME_SOURCES.join(", "))
    } else {
        format!("{} names no desktop with a row here", set.join(", "))
    }
}

/// The tokens of one variable, lower case, in the session's own order.
///
/// `DESKTOP_SESSION` is sometimes a path to the session file rather than its
/// name, so each token is reduced to its last path component.
fn tokens(value: &str) -> impl Iterator<Item = String> + '_ {
    value
        .split(':')
        .map(|token| {
            token
                .trim()
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase()
        })
        .filter(|token| !token.is_empty())
}

fn row_for(token: &str) -> Option<&'static Row> {
    ROWS.iter().find(|row| row.names.contains(&token))
}

#[cfg(test)]
pub(crate) mod fake {
    use std::collections::{HashMap, HashSet};
    use std::path::{Path, PathBuf};

    use super::{PortalFacts, Session, X11Facts};

    /// A session made of fabricated answers.
    #[derive(Default)]
    pub(crate) struct FakeSession {
        pub vars: HashMap<String, String>,
        pub programs: HashSet<String>,
        pub processes: HashSet<String>,
        pub bus_names: HashSet<String>,
        pub paths: HashSet<PathBuf>,
        pub x11: Option<X11Facts>,
        pub wayland_globals: Option<Vec<String>>,
        pub portal: Option<PortalFacts>,
        /// Each process's command line, its name first.
        pub cmdlines: Vec<Vec<String>>,
        /// Every program is on `PATH` and every process runs, whatever the
        /// sets hold.
        pub everything: bool,
    }

    impl FakeSession {
        pub(crate) fn named(desktop: &str) -> Self {
            Self::default().var(super::DESKTOP_ENV, desktop)
        }

        /// A session named `desktop` in which every program and process
        /// anything asks for is there, which is what the table's own tests
        /// assume.
        pub(crate) fn complete(desktop: &str) -> Self {
            Self {
                everything: true,
                x11: Some(X11Facts::default()),
                ..Self::named(desktop)
            }
            .var("XDG_RUNTIME_DIR", "/run/user/1000")
            .var("HYPRLAND_INSTANCE_SIGNATURE", "abc")
            .path("/run/user/1000/hypr/abc/.hyprpaper.sock")
        }

        pub(crate) fn var(mut self, name: &str, value: &str) -> Self {
            self.vars.insert(name.to_owned(), value.to_owned());
            self
        }

        pub(crate) fn program(mut self, name: &str) -> Self {
            self.programs.insert(name.to_owned());
            self
        }

        pub(crate) fn process(mut self, name: &str) -> Self {
            self.processes.insert(name.to_owned());
            self
        }

        pub(crate) fn bus_name(mut self, name: &str) -> Self {
            self.bus_names.insert(name.to_owned());
            self
        }

        pub(crate) fn path(mut self, path: &str) -> Self {
            self.paths.insert(PathBuf::from(path));
            self
        }

        /// An X11 session on `:0` whose server answers with `facts`.
        pub(crate) fn x11(mut self, facts: X11Facts) -> Self {
            self.x11 = Some(facts);
            self.var("DISPLAY", ":0")
        }

        /// A Wayland session on `wayland-1` whose compositor advertises
        /// `globals`.
        pub(crate) fn wayland(mut self, globals: &[&str]) -> Self {
            self.wayland_globals = Some(globals.iter().map(|g| (*g).to_owned()).collect());
            self.var("WAYLAND_DISPLAY", "wayland-1")
                .var("XDG_RUNTIME_DIR", "/run/user/1000")
        }

        /// A portal exporting the wallpaper interface, served by `backends`.
        pub(crate) fn portal(mut self, backends: &[&str], registry: bool) -> Self {
            self.portal = Some(PortalFacts {
                wallpaper: true,
                registry,
                wallpaper_backends: backends.iter().map(|b| (*b).to_owned()).collect(),
            });
            self
        }

        pub(crate) fn running(mut self, cmdline: &[&str]) -> Self {
            self.cmdlines
                .push(cmdline.iter().map(|a| (*a).to_owned()).collect());
            self
        }
    }

    impl Session for FakeSession {
        fn var(&self, name: &str) -> Option<String> {
            self.vars
                .get(name)
                .filter(|value| !value.trim().is_empty())
                .cloned()
        }

        fn on_path(&self, program: &str) -> bool {
            self.everything || self.programs.contains(program)
        }

        fn process_running(&self, name: &str) -> bool {
            self.everything || self.processes.contains(name)
        }

        fn bus_name_owned(&self, name: &str) -> bool {
            self.bus_names.contains(name)
        }

        fn path_exists(&self, path: &Path) -> bool {
            self.paths.contains(path)
        }

        fn x11(&self) -> Option<X11Facts> {
            self.x11.clone()
        }

        fn wayland_globals(&self) -> Option<Vec<String>> {
            self.wayland_globals.clone()
        }

        fn processes_named(&self, name: &str) -> Vec<Vec<String>> {
            self.cmdlines
                .iter()
                .filter(|cmdline| cmdline.first().is_some_and(|first| first.ends_with(name)))
                .cloned()
                .collect()
        }

        fn wallpaper_dir(&self) -> Option<PathBuf> {
            Some(PathBuf::from(WALLPAPER_DIR))
        }

        fn portal(&self) -> Option<PortalFacts> {
            self.portal.clone()
        }
    }

    /// Where the fabricated sessions' app publishes.
    pub(crate) const WALLPAPER_DIR: &str = "/home/t/.local/share/SunlitEarth/wallpaper";
}

#[cfg(test)]
mod tests {
    use super::fake::FakeSession;
    use super::*;

    fn chosen(session: &FakeSession) -> Option<&'static str> {
        choose(session).ok().map(|choice| choice.backend.desktop)
    }

    #[test]
    fn a_named_desktop_with_its_setter_is_chosen_with_the_evidence() {
        let session = FakeSession::named("ubuntu:GNOME")
            .program("gsettings")
            .process("gnome-shell");
        let choice = choose(&session).expect("GNOME with gsettings");
        assert_eq!(choice.backend.desktop, "GNOME");
        let explanation = choice.explanation();
        assert!(explanation.contains("ubuntu:GNOME"), "{explanation}");
        assert!(explanation.contains("gnome-shell running"), "{explanation}");
        assert!(explanation.contains("gsettings on PATH"), "{explanation}");
    }

    #[test]
    fn a_missing_setter_moves_on_to_the_next_token_and_is_listed_if_nothing_else_works() {
        let session = FakeSession::named("KDE:XFCE").program("xfconf-query");
        assert_eq!(chosen(&session), Some("XFCE"));

        let refusal = choose(&FakeSession::named("MATE")).expect_err("no gsettings");
        let text = refusal.to_string();
        assert!(text.contains("`gsettings`"), "{text}");
        assert!(text.contains("not on PATH"), "{text}");
    }

    #[test]
    fn the_secondary_variables_are_read_only_when_the_first_is_empty() {
        let session = FakeSession::default()
            .var("XDG_SESSION_DESKTOP", "xfce")
            .program("xfconf-query");
        assert_eq!(chosen(&session), Some("XFCE"));

        let path = FakeSession::default()
            .var("DESKTOP_SESSION", "/usr/share/xsessions/plasma")
            .program("dbus-send");
        assert_eq!(chosen(&path), Some("KDE Plasma"));

        let named = FakeSession::named("Enlightenment")
            .var("XDG_SESSION_DESKTOP", "xfce")
            .program("xfconf-query");
        assert_eq!(chosen(&named), None);
    }

    #[test]
    fn a_session_that_names_nothing_known_says_what_it_did_name() {
        let refusal = choose(&FakeSession::named("Enlightenment")).expect_err("no row");
        let text = refusal.to_string();
        assert!(text.contains("XDG_CURRENT_DESKTOP=Enlightenment"), "{text}");

        let unset = choose(&FakeSession::default())
            .expect_err("no name")
            .to_string();
        for name in NAME_SOURCES {
            assert!(unset.contains(name), "{unset}");
        }
    }

    #[test]
    fn trinity_calling_itself_kde_too_is_trinity() {
        assert_eq!(
            chosen(&FakeSession::complete("TDE:KDE")),
            Some("Trinity"),
            "the session names Trinity first"
        );
    }

    #[test]
    fn regolith_on_wayland_with_no_gnome_shell_is_sway() {
        let session = FakeSession::named("Regolith-Wayland:GNOME:sway")
            .program("gsettings")
            .program("swaymsg");
        assert_eq!(chosen(&session), Some("sway"));
    }

    #[test]
    fn gnome_with_nothing_drawing_its_key_is_not_gnome() {
        let session = FakeSession::named("GNOME").program("gsettings");
        let refusal = choose(&session).expect_err("nothing draws the key");
        let text = refusal.to_string();
        assert!(text.contains("gnome-shell"), "{text}");

        let flashback = FakeSession::named("GNOME-Flashback:GNOME")
            .program("gsettings")
            .process("gnome-flashback");
        assert_eq!(chosen(&flashback), Some("GNOME"));

        let cinnamon = FakeSession::named("X-Cinnamon").program("gsettings");
        assert_eq!(chosen(&cinnamon), None);
        assert_eq!(chosen(&cinnamon.process("cinnamon")), Some("Cinnamon"));
    }

    #[test]
    fn sway_is_found_by_its_socket_when_the_session_does_not_name_it() {
        let session = FakeSession::default()
            .var("SWAYSOCK", "/run/user/1000/sway-ipc.sock")
            .program("swaymsg");
        let choice = choose(&session).expect("sway by its socket");
        assert_eq!(choice.backend.desktop, "sway");
        assert!(choice.explanation().contains("SWAYSOCK"), "{choice:?}");
    }

    #[test]
    fn hyprland_takes_hyprpaper_only_where_hyprpaper_listens() {
        let with = FakeSession::complete("Hyprland");
        assert_eq!(chosen(&with), Some("Hyprland"));

        let without = FakeSession::named("Hyprland")
            .program("hyprctl")
            .var("XDG_RUNTIME_DIR", "/run/user/1000")
            .var("HYPRLAND_INSTANCE_SIGNATURE", "abc");
        let text = choose(&without).expect_err("no hyprpaper").to_string();
        assert!(text.contains(".hyprpaper.sock"), "{text}");
    }

    #[test]
    fn deepin_uses_the_older_name_only_where_it_is_the_only_one() {
        let current = FakeSession::complete("Deepin");
        assert_eq!(
            choose(&current).expect("Deepin").backend.kind,
            Kind::Deepin { legacy: false }
        );
        let legacy = FakeSession::complete("Deepin").bus_name("com.deepin.daemon.Appearance");
        assert_eq!(
            choose(&legacy).expect("Deepin").backend.kind,
            Kind::Deepin { legacy: true }
        );
        let both = legacy.bus_name("org.deepin.dde.Appearance1");
        assert_eq!(
            choose(&both).expect("Deepin").backend.kind,
            Kind::Deepin { legacy: false }
        );
    }

    #[test]
    fn the_forcing_variable_skips_detection_and_keeps_the_presence_check() {
        let session = FakeSession::named("KDE")
            .program("dbus-send")
            .program("swaymsg")
            .var(FORCE_ENV, "sway");
        let choice = choose(&session).expect("sway forced");
        assert_eq!(choice.backend.desktop, "sway");
        assert!(choice.explanation().contains(FORCE_ENV), "{choice:?}");

        // Forcing GNOME skips its gate: nothing here runs gnome-shell.
        let gnome = FakeSession::default()
            .program("gsettings")
            .var(FORCE_ENV, "GNOME");
        assert_eq!(chosen(&gnome), Some("GNOME"));

        let missing = FakeSession::named("KDE")
            .program("dbus-send")
            .var(FORCE_ENV, "sway");
        let text = choose(&missing).expect_err("no swaymsg").to_string();
        assert!(text.contains(FORCE_ENV), "{text}");
        assert!(text.contains("`swaymsg`"), "{text}");
    }

    #[test]
    fn an_unknown_forced_name_is_refused_with_the_names_there_are() {
        let session = FakeSession::complete("KDE").var(FORCE_ENV, "feh");
        let text = choose(&session).expect_err("no such setter").to_string();
        assert!(text.contains("feh"), "{text}");
        for name in setter_names() {
            assert!(text.contains(name), "{text}");
        }
    }

    fn window_manager(name: Option<&str>) -> X11Facts {
        X11Facts {
            desktop_windows: Vec::new(),
            wm_name: name.map(str::to_owned),
        }
    }

    #[test]
    fn a_plain_window_manager_on_x11_gets_the_root_pixmap() {
        for wm in [Some("i3"), Some("Openbox"), None] {
            let session = FakeSession::named("i3").x11(window_manager(wm));
            let choice = choose(&session).unwrap_or_else(|r| panic!("{wm:?}: {r}"));
            assert_eq!(choice.backend.setter, "root-pixmap", "{wm:?}");
            let explanation = choice.explanation();
            assert!(explanation.contains("no desktop window"), "{explanation}");
        }
        // An empty XDG_CURRENT_DESKTOP, as sddm leaves it for i3.
        let unnamed = FakeSession::default()
            .var("XDG_SESSION_DESKTOP", "i3")
            .x11(window_manager(Some("i3")));
        assert_eq!(
            choose(&unnamed).map(|c| c.backend.setter),
            Ok("root-pixmap")
        );
    }

    #[test]
    fn a_shell_that_draws_its_own_background_gets_no_root_pixmap() {
        for wm in ["GNOME Shell", "Mutter (Muffin)", "KWin"] {
            let session = FakeSession::named("i3").x11(window_manager(Some(wm)));
            let text = choose(&session).expect_err(wm).to_string();
            assert!(text.contains("root pixmap"), "{text}");
            assert!(text.contains(wm), "{text}");
        }
    }

    #[test]
    fn a_desktop_window_picks_its_owners_row_and_keeps_the_root_alone() {
        let xfdesktop = X11Facts {
            desktop_windows: vec![vec!["xfdesktop".to_owned(), "Xfdesktop".to_owned()]],
            wm_name: Some("Openbox".to_owned()),
        };
        let session = FakeSession::named("openbox")
            .x11(xfdesktop.clone())
            .program("xfconf-query");
        let choice = choose(&session).expect("xfdesktop owns the desktop");
        assert_eq!(choice.backend.setter, "xfce");
        assert!(choice.explanation().contains("xfdesktop"), "{choice:?}");

        let without = FakeSession::named("openbox").x11(xfdesktop);
        let text = choose(&without).expect_err("no xfconf-query").to_string();
        assert!(text.contains("covers the root"), "{text}");

        let unknown = FakeSession::named("openbox").x11(X11Facts {
            desktop_windows: vec![vec!["spacefm".to_owned(), "Spacefm".to_owned()]],
            wm_name: None,
        });
        let text = choose(&unknown)
            .expect_err("no row for spacefm")
            .to_string();
        assert!(text.contains("spacefm"), "{text}");
        assert!(text.contains("covers the root"), "{text}");
    }

    #[test]
    fn cinnamon_without_its_shell_is_found_by_nemos_desktop_window() {
        let facts = X11Facts {
            desktop_windows: vec![vec!["nemo-desktop".to_owned(), "Nemo-desktop".to_owned()]],
            wm_name: None,
        };
        let session = FakeSession::named("X-Cinnamon")
            .program("gsettings")
            .x11(facts);
        let choice = choose(&session).expect("nemo-desktop owns the desktop");
        assert_eq!(choice.backend.setter, "cinnamon");
        assert!(choice.explanation().contains("nemo-desktop"), "{choice:?}");
    }

    #[test]
    fn the_client_list_is_read_where_there_is_one_and_the_tree_where_there_is_not() {
        let desktops = [3, 21];
        let is_desktop = |w: u32| desktops.contains(&w);
        let frames = |w: u32| if w == 2 { vec![21] } else { Vec::new() };

        let listed = desktop_windows(Some(vec![1, 3]), || panic!("unused"), frames, is_desktop);
        assert_eq!(listed, vec![3]);

        let tree = || vec![1, 2, 3];
        assert_eq!(desktop_windows(None, tree, frames, is_desktop), vec![21, 3]);
        assert_eq!(
            desktop_windows(Some(Vec::new()), tree, frames, is_desktop),
            vec![21, 3]
        );
        assert_eq!(
            desktop_windows(Some(vec![1, 2]), tree, frames, is_desktop),
            vec![21, 3]
        );
        assert!(desktop_windows(None, Vec::new, frames, is_desktop).is_empty());
    }

    #[test]
    fn a_wayland_session_is_not_offered_the_root() {
        let session = FakeSession::named("niri")
            .var("WAYLAND_DISPLAY", "wayland-1")
            .x11(window_manager(None));
        let text = choose(&session).expect_err("no rung yet").to_string();
        assert!(text.contains("root pixmap: WAYLAND_DISPLAY"), "{text}");
    }

    #[test]
    fn the_root_pixmap_can_be_forced_only_where_there_is_an_x_display() {
        let session = FakeSession::named("KDE")
            .program("dbus-send")
            .var(FORCE_ENV, "root-pixmap")
            .x11(window_manager(Some("KWin")));
        assert_eq!(chosen(&session), Some(ROOT_PIXMAP.desktop));
        let no_display = FakeSession::named("KDE").var(FORCE_ENV, "root-pixmap");
        assert!(choose(&no_display).is_err());
    }

    #[test]
    fn a_running_daemon_comes_before_a_swaybg_of_our_own() {
        let everything = FakeSession::named("niri")
            .wayland(&[LAYER_SHELL])
            .program("awww")
            .program("wpaperctl")
            .program("swaybg")
            .path("/run/user/1000/wayland-1-awww-daemon.sock")
            .path("/run/user/1000/wpaperd.sock");
        assert_eq!(chosen(&everything), Some("awww"));

        let wpaperd = FakeSession::named("niri")
            .wayland(&[LAYER_SHELL])
            .program("awww")
            .program("wpaperctl")
            .program("swaybg")
            .path("/run/user/1000/wpaperd.sock");
        assert_eq!(chosen(&wpaperd), Some("wpaperd"));

        let bare = FakeSession::named("niri")
            .wayland(&[LAYER_SHELL])
            .program("awww")
            .program("wpaperctl")
            .program("swaybg");
        let choice = choose(&bare).expect("swaybg");
        assert_eq!(choice.backend.desktop, "swaybg");
        assert!(choice.explanation().contains(LAYER_SHELL), "{choice:?}");
    }

    #[test]
    fn an_older_swww_is_used_under_its_own_socket_name() {
        let session = FakeSession::named("river")
            .wayland(&[])
            .program("swww")
            .path("/run/user/1000/wayland-1-swww-daemon.sock");
        let choice = choose(&session).expect("swww");
        assert_eq!(choice.backend.desktop, "swww");
        assert_eq!(
            choice
                .backend
                .commands(&crate::desktop::Placement::single("/w.png".into()), "")[0]
                .program,
            "swww"
        );
    }

    #[test]
    fn swaybg_needs_layer_shell_and_says_so() {
        let session = FakeSession::named("GNOME")
            .wayland(&["wl_compositor", "xdg_wm_base"])
            .program("swaybg")
            .program("gsettings");
        let text = choose(&session)
            .expect_err("mutter has no layer shell")
            .to_string();
        assert!(
            text.contains(&format!("does not offer {LAYER_SHELL}")),
            "{text}"
        );
    }

    #[test]
    fn a_swaybg_that_is_not_ours_is_mentioned_and_left_alone() {
        let ours = format!("{}/gen-1/0.png", fake::WALLPAPER_DIR);
        let session = FakeSession::named("labwc:wlroots")
            .wayland(&[LAYER_SHELL])
            .program("swaybg")
            .running(&["swaybg", "-o", "*", "-i", &ours, "-m", "fill"])
            .running(&["/usr/bin/swaybg", "-i", "/home/t/Pictures/beach.png"]);
        let choice = choose(&session).expect("swaybg");
        let explanation = choice.explanation();
        assert!(
            explanation.contains("1 swaybg not started by this app"),
            "{explanation}"
        );
    }

    #[test]
    fn our_swaybg_is_told_by_the_file_it_was_given() {
        let dir = Path::new(fake::WALLPAPER_DIR);
        let words = |line: &[&str]| line.iter().map(|w| (*w).to_owned()).collect::<Vec<_>>();
        let ours = format!("{}/gen-7/0.png", fake::WALLPAPER_DIR);
        assert!(names_a_file_under(&words(&["swaybg", "-i", &ours]), dir));
        assert!(!names_a_file_under(
            &words(&["swaybg", "-i", "/home/t/Pictures/beach.png"]),
            dir
        ));
        // A sibling directory whose name merely starts the same is not ours.
        assert!(!names_a_file_under(
            &words(&[
                "swaybg",
                "-i",
                "/home/t/.local/share/SunlitEarth/wallpaper-old/0.png"
            ]),
            dir
        ));
        assert!(!names_a_file_under(&words(&[&ours]), dir), "the name alone");
    }

    #[test]
    fn the_portal_is_the_last_rung() {
        let session = FakeSession::named("COSMIC")
            .wayland(&["wl_compositor"])
            .portal(&["cosmic", "gtk"], true);
        let choice = choose(&session).expect("the portal");
        assert_eq!(choice.backend.setter, "portal");
        assert!(choice.explanation().contains(APP_ID), "{choice:?}");

        let unregistered = FakeSession::named("COSMIC")
            .wayland(&["wl_compositor"])
            .portal(&["kde"], false);
        let explanation = choose(&unregistered).expect("the portal").explanation();
        assert!(
            explanation.contains("shared with every other"),
            "{explanation}"
        );

        // Anything earlier wins: layer shell and swaybg come first.
        let swaybg = FakeSession::named("COSMIC")
            .wayland(&[LAYER_SHELL])
            .program("swaybg")
            .portal(&["kde"], true);
        assert_eq!(chosen(&swaybg), Some("swaybg"));
    }

    #[test]
    fn the_gtk_backend_alone_is_trusted_only_with_a_gnome_shell() {
        let bare = FakeSession::named("")
            .var("DISPLAY", ":0")
            .portal(&[GTK_PORTAL_BACKEND], true);
        let text = choose(&bare)
            .expect_err("gtk writes a key nobody draws")
            .to_string();
        assert!(text.contains("portal: the only portal backend"), "{text}");

        let with_shell = FakeSession::named("")
            .var("DISPLAY", ":0")
            .process("gnome-shell")
            .portal(&[GTK_PORTAL_BACKEND], true);
        assert_eq!(choose(&with_shell).map(|c| c.backend.setter), Ok("portal"));

        let beside_another = FakeSession::named("")
            .var("DISPLAY", ":0")
            .portal(&[GTK_PORTAL_BACKEND, "xapp"], true);
        assert_eq!(
            choose(&beside_another).map(|c| c.backend.setter),
            Ok("portal")
        );
    }

    #[test]
    fn a_session_nothing_accepts_is_refused_with_every_rung_and_its_reason() {
        let refusal = choose(&FakeSession::default()).expect_err("nothing here");
        let rungs: Vec<&str> = refusal.declined.iter().map(|d| d.rung.as_str()).collect();
        assert_eq!(
            rungs,
            [
                "desktop table",
                "awww",
                "wpaperd",
                "swaybg",
                "desktop window owner",
                "root pixmap",
                "portal"
            ]
        );
        for declined in &refusal.declined {
            assert!(!declined.reason.is_empty(), "{declined:?}");
        }
        let text = refusal.to_string();
        for declined in &refusal.declined {
            assert!(
                text.contains(&format!("{}: {}", declined.rung, declined.reason)),
                "{text}"
            );
        }
        assert!(text.contains(FORCE_ENV), "{text}");
    }

    #[test]
    fn every_setter_can_be_forced_by_its_name() {
        for name in setter_names() {
            let session = FakeSession::complete("")
                .portal(&["gnome"], true)
                .var(FORCE_ENV, name);
            let choice = choose(&session).unwrap_or_else(|r| panic!("{name}: {r}"));
            assert_eq!(choice.backend.setter, name);
        }
    }
}
