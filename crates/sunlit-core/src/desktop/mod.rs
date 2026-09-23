//! Which Linux desktop this is, and how to tell it to change its wallpaper.
//!
//! Setting a wallpaper on Linux is a desktop-shell operation, not a display-server
//! one: it goes to GNOME's settings store, or to plasmashell, or to xfconf. So
//! there is no single call, there is a table, and what selects a row is
//! `XDG_CURRENT_DESKTOP`. The same row works in that desktop's X11 session and in
//! its Wayland one, because neither the shell nor the setting knows or cares which
//! is running.
//!
//! Not the XDG desktop portal: it puts a confirmation dialog in front of every
//! set, and this wallpaper refreshes on a schedule.
//!
//! Everything here is a pure function over an environment and a path. Only the
//! sink runs the commands, so the table is tested on every platform against
//! fabricated sessions rather than only where a desktop exists.

// Every row here drives a Linux desktop, so on another platform the crate calls
// almost none of this. It is compiled and tested there anyway, which is what
// lets the table be checked without a Linux machine.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::path::{Path, PathBuf};

use crate::display::layout::DisplayMode;

mod choose;
mod kde;
#[cfg(target_os = "linux")]
mod probe;
mod setters;
mod xfce;

pub use choose::{Choice, Declined, FORCE_ENV, Refusal, Session, choose};
use setters::{
    LXDE_FILL_MODE, TRINITY_FILL_MODE, deepin_commands, hyprpaper_commands, sway_quoted,
};
use xfce::{XFCE_CHANNEL, XFCE_IMAGE_PROPERTY};

/// The variable that says which desktop this is.
///
/// A colon-separated list, most specific first, which is what makes the order
/// meaningful: Budgie sets `Budgie:GNOME` and Ubuntu sets `ubuntu:GNOME`, so
/// walking the list in order picks the desktop that claimed to be itself before
/// the one it is built on.
pub(crate) const DESKTOP_ENV: &str = "XDG_CURRENT_DESKTOP";

/// One command to run, as a program and its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: &'static str,
    pub args: Vec<String>,
}

impl Invocation {
    fn new(program: &'static str, args: impl IntoIterator<Item = String>) -> Self {
        Self {
            program,
            args: args.into_iter().collect(),
        }
    }
}

/// How one family of desktops takes a wallpaper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `gsettings set <schema> <key> <value>`, once per key.
    ///
    /// Two shapes of value, which is the whole reason this carries a flag: the
    /// GNOME and Cinnamon schemas take a `file://` URI and the MATE one takes a
    /// plain path, and a URI in the MATE key produces a desktop with no
    /// wallpaper and no error.
    Gsettings {
        schema: &'static str,
        keys: &'static [&'static str],
        uri: bool,
        /// The key that says how the image is fitted, where the schema has one.
        ///
        /// Set for the same reason the Windows sink writes `WallpaperStyle=10`
        /// before applying: the frame was rendered at this display's exact
        /// resolution, and a desktop set to centre or tile it would show it at
        /// the wrong size on a background of its own. It is set before the image
        /// rather than after, so that the write which makes the shell repaint is
        /// the one carrying the new picture.
        ///
        /// The value is `zoom` for one screen's image and `spanned` for a canvas
        /// over the whole virtual desktop, which is the only per-monitor reach
        /// these schemas have.
        fill: Option<&'static str>,
    },
    /// A plasmashell script over `dbus-send`, which is the only way to reach one
    /// screen at a time.
    ///
    /// `plasma-apply-wallpaperimage` writes every containment, so it can neither
    /// give two screens two pictures nor leave one alone; the scripting API
    /// behind it can do both. Plasma has no span mode, which costs nothing here,
    /// because a view across the screens is already cut into one image per
    /// screen before it arrives.
    Kde,
    /// `xfconf-query`, once per backdrop property that holds an image.
    ///
    /// The only member of the table that cannot be written down in advance:
    /// which properties exist depends on how many monitors and workspaces this
    /// session has, and their names are inside the property paths. So this row
    /// asks first.
    Xfce,
    /// `pcmanfm-qt --set-wallpaper <path>`, `LXQt`'s file manager doubling as its
    /// desktop.
    Lxqt,
    /// `pcmanfm --set-wallpaper`, LXDE's file manager doing the same job as
    /// `LXQt`'s under the older name.
    Lxde,
    /// `swaymsg output * bg`, which has sway start its own `swaybg`.
    Sway,
    /// `hyprctl hyprpaper`, three calls to a daemon that holds every image it
    /// was given until told to let go.
    Hyprpaper,
    /// Deepin's appearance daemon over `dbus-send`, one call per monitor.
    ///
    /// Two names for one interface: the daemon moved from
    /// `com.deepin.daemon.Appearance` to `org.deepin.dde.Appearance1`, and which
    /// one a session has is asked of its bus.
    Deepin { legacy: bool },
    /// Trinity's `kdesktop` over `dcop`, the KDE 3 way that Trinity kept.
    Trinity,
}

/// How far into a multi-monitor session one desktop's setter reaches.
///
/// A property of the desktop, not a guess the sink makes: which of the three
/// this is decides what the publish writes, and a row that claimed more than its
/// setter can do would produce files nothing reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// A wallpaper per monitor, addressed by the monitor's own name.
    PerMonitor,
    /// One image, with a fit mode that stretches it over the whole virtual
    /// desktop where the mode asks for that.
    Spanned,
    /// One image, which every screen shows. No fit mode reaches further.
    OneImage,
}

/// What one publish left on disk, in the terms a desktop's setter takes.
///
/// The sink cuts and writes; the table only says which path goes where. Keeping
/// the two apart is what lets the table stay a pure function over fabricated
/// sessions on a machine with one screen or none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    /// One image per monitor, by the monitor's own name, in layout order.
    ///
    /// Empty where the publish did not cut one image per screen, which is every
    /// reach but [`Reach::PerMonitor`].
    pub per_monitor: Vec<(String, PathBuf)>,
    /// Monitors this publish deliberately did not paint, by name.
    ///
    /// A desktop that addresses monitors individually leaves these alone, which
    /// is what one-screen mode means on a session with more than one screen.
    /// One that cannot address them gives them the single image, because the
    /// alternative is setting no wallpaper at all.
    pub untouched: Vec<String>,
    /// One entry per monitor, left to right and then top to bottom: the image
    /// that screen is to hold, or `None` for one this publish left alone.
    ///
    /// The same facts `per_monitor` and `untouched` carry, in the shape a setter
    /// that addresses screens by where they sit rather than by name needs. KDE
    /// is the only such setter, because Plasma's scripting API offers a
    /// containment's screen index and that index's geometry and no output name
    /// at all. The holes are what keep it usable: drop them and every screen
    /// after an unpainted one is addressed one place too early.
    pub by_position: Vec<Option<PathBuf>>,
    /// The one image a desktop that holds only one shows.
    pub single: PathBuf,
    /// Whether `single` covers the whole virtual desktop rather than one screen.
    pub spanned: bool,
}

impl Placement {
    /// One image for everything, which is what a single-monitor session is.
    pub fn single(path: PathBuf) -> Self {
        Self {
            per_monitor: Vec::new(),
            untouched: Vec::new(),
            by_position: vec![Some(path.clone())],
            single: path,
            spanned: false,
        }
    }

    /// The image for one monitor, or the single one where it has none of its own.
    fn for_monitor(&self, monitor: &str) -> &Path {
        self.per_monitor
            .iter()
            .find(|(name, _)| name == monitor)
            .map_or(self.single.as_path(), |(_, path)| path.as_path())
    }
}

/// A desktop's way of being told, and what it is called.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backend {
    /// What to call this in a message, which is read by whoever is looking at a
    /// refusal.
    pub desktop: &'static str,
    /// The name `SUNLIT_EARTH_WALLPAPER_SETTER` knows this setter by.
    pub setter: &'static str,
    /// The program that has to exist for this backend to work at all. Probed
    /// before anything is rendered, because a wallpaper frame is the most
    /// expensive thing the engine does. `None` for a setter that runs nothing.
    pub program: Option<&'static str>,
    kind: Kind,
}

impl Backend {
    /// The command whose output the commands need, if this backend asks
    /// anything first.
    ///
    /// Only XFCE does. Everything else builds its commands from the path alone
    /// and is handed an empty string.
    pub fn discovery(&self) -> Option<Invocation> {
        match self.kind {
            Kind::Xfce => Some(Invocation::new(
                "xfconf-query",
                ["-c".to_owned(), XFCE_CHANNEL.to_owned(), "-l".to_owned()],
            )),
            Kind::Gsettings { .. }
            | Kind::Kde
            | Kind::Lxqt
            | Kind::Lxde
            | Kind::Sway
            | Kind::Hyprpaper
            | Kind::Deepin { .. }
            | Kind::Trinity => None,
        }
    }

    /// How far this desktop's setter reaches into a multi-monitor session.
    pub fn reach(&self) -> Reach {
        match self.kind {
            // XFCE names a backdrop property after each monitor and Plasma gives
            // each screen its own containment, so both hold one path per screen
            // and both can be told to leave a screen alone. Deepin takes a
            // monitor's own name in every call.
            Kind::Xfce | Kind::Kde | Kind::Deepin { .. } => Reach::PerMonitor,
            // No per-monitor wallpaper in any of these schemas, but
            // `picture-options` has a `spanned` value that stretches one image
            // over the whole virtual desktop.
            Kind::Gsettings { .. } => Reach::Spanned,
            // sway and hyprpaper address outputs by the compositor's names,
            // which `xrandr` through Xwayland does not report, so both are
            // handed one image for every output.
            Kind::Lxqt | Kind::Lxde | Kind::Sway | Kind::Hyprpaper | Kind::Trinity => {
                Reach::OneImage
            }
        }
    }

    /// What to say when the mode asked for more than this desktop reaches.
    ///
    /// Not a failure: the sink does the nearest thing and this is the sentence
    /// that says which, in the same voice as the refusals. `None` where the
    /// desktop did exactly what was asked, which includes every single-monitor
    /// session, since all three modes mean the same thing on one screen.
    pub(crate) fn degradation(&self, mode: DisplayMode, screens: usize) -> Option<String> {
        if screens < 2 {
            return None;
        }
        let desktop = self.desktop;
        match (self.reach(), mode) {
            (Reach::PerMonitor, _) | (Reach::Spanned, DisplayMode::AcrossScreens) => None,
            (Reach::Spanned, _) => Some(format!(
                "{desktop} has one wallpaper for all monitors, so every screen got the same image"
            )),
            (Reach::OneImage, DisplayMode::AcrossScreens) => Some(format!(
                "{desktop} sets one wallpaper on every screen and has no view across them, so \
                 each screen got the chosen screen's own picture instead"
            )),
            (Reach::OneImage, _) => Some(format!(
                "{desktop} sets one wallpaper on every screen, so all {screens} got the same image"
            )),
        }
    }

    /// Everything to run, in order, to make `placement` this desktop's wallpaper.
    ///
    /// Empty means the desktop was asked something and answered with nothing
    /// usable, which is a refusal rather than a success: the caller must not
    /// report a wallpaper it did not set.
    ///
    /// Only XFCE reads `placement.per_monitor`, and it cannot do without the
    /// monitors' own names: see `xfce_live_property`. An empty list leaves it
    /// with whatever its listing offered, all of it holding the single image.
    pub fn commands(&self, placement: &Placement, discovered: &str) -> Vec<Invocation> {
        let single = placement.single.to_string_lossy().into_owned();
        match self.kind {
            Kind::Gsettings {
                schema,
                keys,
                uri,
                fill,
            } => {
                let value = if uri {
                    file_uri(&placement.single)
                } else {
                    single
                };
                let set = |key: &str, value: &str| {
                    Invocation::new(
                        "gsettings",
                        [
                            "set".to_owned(),
                            schema.to_owned(),
                            key.to_owned(),
                            value.to_owned(),
                        ],
                    )
                };
                let mode = if placement.spanned { "spanned" } else { "zoom" };
                fill.map(|key| set(key, mode))
                    .into_iter()
                    .chain(keys.iter().map(|key| set(key, &value)))
                    .collect()
            }
            Kind::Kde => vec![kde::plasma_command(placement)],
            Kind::Xfce => xfce::commands(placement, discovered),
            Kind::Lxqt => vec![Invocation::new(
                "pcmanfm-qt",
                ["--set-wallpaper".to_owned(), single],
            )],
            Kind::Lxde => vec![Invocation::new(
                "pcmanfm",
                [
                    format!("--set-wallpaper={single}"),
                    format!("--wallpaper-mode={LXDE_FILL_MODE}"),
                ],
            )],
            Kind::Sway => vec![Invocation::new(
                "swaymsg",
                [
                    "output".to_owned(),
                    "*".to_owned(),
                    "bg".to_owned(),
                    sway_quoted(&single),
                    "fill".to_owned(),
                ],
            )],
            Kind::Hyprpaper => hyprpaper_commands(&single),
            Kind::Deepin { legacy } => deepin_commands(placement, legacy),
            Kind::Trinity => vec![Invocation::new(
                "dcop",
                [
                    "kdesktop".to_owned(),
                    "KBackgroundIface".to_owned(),
                    "setWallpaper".to_owned(),
                    single,
                    TRINITY_FILL_MODE.to_owned(),
                ],
            )],
        }
    }

    /// Why this backend produced nothing to run, in the desktop's own terms.
    ///
    /// Only reachable for the setters that address monitors by name, and worth
    /// its own sentence rather than a generic failure. For XFCE it means the
    /// session listed no backdrop property and named no monitor either, so there
    /// was nowhere to put the image and nowhere to create one, which is a
    /// different problem from a setter that ran and did not work.
    pub(crate) fn nothing_to_run(&self) -> String {
        match self.kind {
            Kind::Deepin { .. } => format!(
                "{} sets a wallpaper per monitor by name, and xrandr named no \
                 connected monitor to set it on",
                self.desktop
            ),
            _ => format!(
                "{} has no wallpaper property to set: `xfconf-query -c {XFCE_CHANNEL} -l` \
                 listed nothing ending in `{XFCE_IMAGE_PROPERTY}` and no connected \
                 monitor was named, so there is no backdrop to write to",
                self.desktop
            ),
        }
    }
}

/// A `file://` URI for a local path, which is what the gsettings keys want.
///
/// Percent-encoding is limited to the characters that would otherwise change
/// what the URI means. The path this is called with is one the app wrote itself,
/// under a directory named by the OS, so the general case is not the case here;
/// a space in a home directory is, and that is the one that has to work.
fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                uri.push(char::from(byte));
            }
            b'\\' => uri.push('/'),
            other => {
                use std::fmt::Write as _;
                let _ = write!(uri, "%{other:02X}");
            }
        }
    }
    uri
}

/// What has to be true, besides its name and its program, for a row to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Gate {
    /// Nothing more.
    None,
    /// One of these processes runs: the one that draws the key the setter
    /// writes. `gsettings set` succeeds silently with nobody drawing, so the
    /// schema being installed says nothing on its own.
    Process(&'static [&'static str]),
    /// hyprpaper's socket exists, which is the difference between a Hyprland
    /// that takes `hyprctl hyprpaper` and one running some other daemon.
    HyprpaperSocket,
}

/// One desktop the session can name itself as.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Row {
    /// The names it goes by, lower case.
    pub(crate) names: &'static [&'static str],
    pub(crate) gate: Gate,
    pub(crate) backend: Backend,
}

/// Every desktop with a row, with the names each identifies itself by.
///
/// The names are matched case-insensitively against the tokens of
/// [`DESKTOP_ENV`], which is how one row covers the several spellings a desktop
/// has had: `X-Cinnamon` and `Cinnamon`, `KDE` and `plasma`.
///
/// Not every row has been run against its desktop; `docs/platforms.md` gives
/// each one's tier of evidence.
pub(crate) const ROWS: &[Row] = &[
    Row {
        names: &["kde", "plasma"],
        gate: Gate::None,
        backend: Backend {
            desktop: "KDE Plasma",
            setter: "kde",
            program: Some("dbus-send"),
            kind: Kind::Kde,
        },
    },
    Row {
        names: &["xfce"],
        gate: Gate::None,
        backend: Backend {
            desktop: "XFCE",
            setter: "xfce",
            program: Some("xfconf-query"),
            kind: Kind::Xfce,
        },
    },
    Row {
        names: &["x-cinnamon", "cinnamon"],
        gate: Gate::Process(&["cinnamon"]),
        backend: Backend {
            desktop: "Cinnamon",
            setter: "cinnamon",
            program: Some("gsettings"),
            kind: Kind::Gsettings {
                schema: "org.cinnamon.desktop.background",
                keys: &["picture-uri"],
                uri: true,
                fill: Some("picture-options"),
            },
        },
    },
    Row {
        names: &["mate"],
        gate: Gate::None,
        backend: Backend {
            desktop: "MATE",
            setter: "mate",
            program: Some("gsettings"),
            kind: Kind::Gsettings {
                schema: "org.mate.background",
                // A path, not a URI: the key is named for what it holds.
                keys: &["picture-filename"],
                uri: false,
                fill: Some("picture-options"),
            },
        },
    },
    Row {
        names: &["lxqt"],
        gate: Gate::None,
        backend: Backend {
            desktop: "LXQt",
            setter: "lxqt",
            program: Some("pcmanfm-qt"),
            kind: Kind::Lxqt,
        },
    },
    // Budgie is a GNOME shell replacement and keeps GNOME's settings store, so
    // it takes the same row's mechanism under its own name. It has to be its
    // own row rather than falling through to GNOME's, because the refusal
    // messages name what was detected.
    Row {
        names: &["budgie"],
        gate: Gate::None,
        backend: Backend {
            desktop: "Budgie",
            setter: "budgie",
            program: Some("gsettings"),
            kind: GNOME_BACKGROUND,
        },
    },
    Row {
        names: &["gnome", "gnome-classic", "gnome-flashback"],
        gate: Gate::Process(&["gnome-shell", "gnome-flashback"]),
        backend: Backend {
            desktop: "GNOME",
            setter: "gnome",
            program: Some("gsettings"),
            kind: GNOME_BACKGROUND,
        },
    },
    // Unity draws GNOME's key, and no process name for it was verified, so it
    // has GNOME's mechanism without GNOME's gate.
    Row {
        names: &["unity"],
        gate: Gate::None,
        backend: Backend {
            desktop: "Unity",
            setter: "unity",
            program: Some("gsettings"),
            kind: GNOME_BACKGROUND,
        },
    },
    Row {
        names: &["sway"],
        gate: Gate::None,
        backend: SWAY,
    },
    Row {
        names: &["hyprland"],
        gate: Gate::HyprpaperSocket,
        backend: Backend {
            desktop: "Hyprland",
            setter: "hyprland",
            program: Some("hyprctl"),
            kind: Kind::Hyprpaper,
        },
    },
    Row {
        names: &["lxde"],
        gate: Gate::None,
        backend: Backend {
            desktop: "LXDE",
            setter: "lxde",
            program: Some("pcmanfm"),
            kind: Kind::Lxde,
        },
    },
    Row {
        names: &["deepin"],
        gate: Gate::None,
        backend: Backend {
            desktop: "Deepin",
            setter: "deepin",
            program: Some("dbus-send"),
            kind: Kind::Deepin { legacy: false },
        },
    },
    Row {
        names: &["tde", "trinity"],
        gate: Gate::None,
        backend: Backend {
            desktop: "Trinity",
            setter: "trinity",
            program: Some("dcop"),
            kind: Kind::Trinity,
        },
    },
];

/// Both keys, because GNOME picks between them by the current colour scheme and
/// setting only one leaves the other theme showing the previous wallpaper.
const GNOME_BACKGROUND: Kind = Kind::Gsettings {
    schema: "org.gnome.desktop.background",
    keys: &["picture-uri", "picture-uri-dark"],
    uri: true,
    fill: Some("picture-options"),
};

/// sway's row, which a session can also reach through `SWAYSOCK` alone.
pub(crate) const SWAY: Backend = Backend {
    desktop: "sway",
    setter: "sway",
    program: Some("swaymsg"),
    kind: Kind::Sway,
};

/// Deepin's appearance daemon under the name releases before DDE 23 used.
pub(crate) const DEEPIN_LEGACY: Backend = Backend {
    desktop: "Deepin",
    setter: "deepin",
    program: Some("dbus-send"),
    kind: Kind::Deepin { legacy: true },
};

/// The bus names Deepin's appearance daemon has had, current first.
pub(crate) const DEEPIN_BUS_NAMES: [&str; 2] =
    ["org.deepin.dde.Appearance1", "com.deepin.daemon.Appearance"];

/// The setter for this process's session, asked of the session itself.
#[cfg(target_os = "linux")]
pub fn choose_current() -> Result<Choice, Refusal> {
    choose(&probe::LiveSession::new())
}

/// The backend for this process's session, where it has one.
#[cfg(target_os = "linux")]
pub fn detect_current() -> Option<Backend> {
    choose_current().ok().map(|choice| choice.backend)
}

/// The backend a session named `desktop` gets when everything it asks for is
/// there, which is the table's own answer with no probing in the way.
#[cfg(test)]
fn detect(desktop: &str) -> Option<Backend> {
    choose(&choose::fake::FakeSession::complete(desktop))
        .ok()
        .map(|choice| choice.backend)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn commands(desktop: &str, path: &str, discovered: &str) -> Vec<Invocation> {
        commands_on(desktop, path, discovered, &[])
    }

    /// One image, shown on every monitor named, which is what a single-screen
    /// session and every reach short of per-monitor come down to.
    pub(super) fn commands_on(
        desktop: &str,
        path: &str,
        discovered: &str,
        monitors: &[&str],
    ) -> Vec<Invocation> {
        let placement = Placement {
            per_monitor: monitors
                .iter()
                .map(|m| ((*m).to_owned(), PathBuf::from(path)))
                .collect(),
            untouched: Vec::new(),
            by_position: std::iter::repeat_n(Some(PathBuf::from(path)), monitors.len().max(1))
                .collect(),
            single: PathBuf::from(path),
            spanned: false,
        };
        detect(desktop)
            .unwrap_or_else(|| panic!("{desktop} has a backend"))
            .commands(&placement, discovered)
    }

    /// A two-monitor session where each screen has its own picture.
    pub(super) fn two_screens() -> Placement {
        Placement {
            per_monitor: vec![
                ("Virtual-1".to_owned(), PathBuf::from("/w-0.png")),
                ("Virtual-2".to_owned(), PathBuf::from("/w-1.png")),
            ],
            untouched: Vec::new(),
            by_position: vec![
                Some(PathBuf::from("/w-0.png")),
                Some(PathBuf::from("/w-1.png")),
            ],
            single: PathBuf::from("/w-0.png"),
            spanned: false,
        }
    }

    #[test]
    fn gnome_sets_both_keys_as_uris() {
        let cmds = commands("GNOME", "/home/tester/w.png", "");
        // The fill mode first, then the two image keys.
        assert_eq!(cmds.len(), 3, "{cmds:?}");
        for (cmd, key) in cmds[1..].iter().zip(["picture-uri", "picture-uri-dark"]) {
            assert_eq!(cmd.program, "gsettings");
            assert_eq!(
                cmd.args,
                vec![
                    "set".to_owned(),
                    "org.gnome.desktop.background".to_owned(),
                    key.to_owned(),
                    "file:///home/tester/w.png".to_owned(),
                ]
            );
        }

        let cinnamon: Vec<String> = commands("X-Cinnamon", "/w.png", "")
            .into_iter()
            .map(|c| c.args[2].clone())
            .collect();
        assert!(
            !cinnamon.contains(&"picture-uri-dark".to_owned()),
            "{cinnamon:?}"
        );
    }

    #[test]
    fn the_image_is_made_to_fill_the_screen_before_it_is_set() {
        for desktop in ["GNOME", "X-Cinnamon", "MATE", "Budgie"] {
            let cmds = commands(desktop, "/w.png", "");
            assert_eq!(cmds[0].args[2], "picture-options", "{desktop}: {cmds:?}");
            assert_eq!(cmds[0].args[3], "zoom", "{desktop}: {cmds:?}");
            // And the write that carries the picture comes after it, so the
            // repaint it triggers is the one showing the new image.
            assert!(cmds.len() > 1, "{desktop}: {cmds:?}");
            assert!(
                cmds.last().expect("a command").args[3].contains("/w.png"),
                "{desktop}: {cmds:?}"
            );
        }
        // Plasma's own tool decides its fill mode itself, and LXQt's row is one
        // that has never run, so neither gets an option invented for it.
        assert_eq!(commands("KDE", "/w.png", "").len(), 1);
        assert_eq!(commands("LXQt", "/w.png", "").len(), 1);
    }

    #[test]
    fn the_session_names_itself_before_what_it_is_built_on() {
        // Both of these end in GNOME and only one of them is GNOME.
        assert_eq!(detect("Budgie:GNOME").map(|b| b.desktop), Some("Budgie"));
        assert_eq!(detect("ubuntu:GNOME").map(|b| b.desktop), Some("GNOME"));
        // And the guest's four, as their sessions set the variable.
        assert_eq!(detect("KDE").map(|b| b.desktop), Some("KDE Plasma"));
        assert_eq!(detect("XFCE").map(|b| b.desktop), Some("XFCE"));
        assert_eq!(detect("X-Cinnamon").map(|b| b.desktop), Some("Cinnamon"));
        assert_eq!(detect("GNOME").map(|b| b.desktop), Some("GNOME"));
    }

    /// What each of the table's desktops is called by the sessions that run it,
    /// and the setter that name chose before there was anything but the table.
    #[test]
    fn the_desktops_in_the_table_choose_the_setter_they_always_chose() {
        for (session, setter) in [
            ("KDE", "kde"),
            ("plasma", "kde"),
            ("XFCE", "xfce"),
            ("X-Cinnamon", "cinnamon"),
            ("Cinnamon", "cinnamon"),
            ("MATE", "mate"),
            ("LXQt", "lxqt"),
            ("Budgie:GNOME", "budgie"),
            ("GNOME", "gnome"),
            ("ubuntu:GNOME", "gnome"),
            ("pop:GNOME", "gnome"),
            ("GNOME-Classic:GNOME", "gnome"),
            ("GNOME-Flashback:GNOME", "gnome"),
            ("Unity", "unity"),
            ("Unity:Unity7", "unity"),
        ] {
            assert_eq!(
                detect(session).map(|backend| backend.setter),
                Some(setter),
                "{session}"
            );
        }
    }

    #[test]
    fn the_match_is_case_insensitive_and_ignores_padding() {
        for spelling in ["kde", "KDE", "Kde", " KDE ", "plasma:KDE"] {
            assert_eq!(
                detect(spelling).map(|b| b.desktop),
                Some("KDE Plasma"),
                "{spelling}"
            );
        }
    }

    #[test]
    fn a_desktop_with_no_row_is_refused_by_name() {
        assert_eq!(detect("Enlightenment"), None);
        assert_eq!(detect("COSMIC"), None);
        assert_eq!(detect(""), None);
        assert_eq!(detect(":::"), None);
    }

    #[test]
    fn mate_takes_a_path_where_the_others_take_a_uri() {
        // The key is `picture-filename`, and a URI in it leaves the desktop with
        // no wallpaper and no complaint.
        let cmds = commands("MATE", "/home/tester/w.png", "");
        let image = cmds.last().expect("the image is set");
        assert_eq!(image.args[2], "picture-filename");
        assert_eq!(image.args[3], "/home/tester/w.png");
        assert!(!image.args[3].contains("file://"));
    }

    #[test]
    fn a_uri_escapes_what_would_change_its_meaning_and_nothing_more() {
        assert_eq!(
            file_uri(Path::new("/home/a b/w.png")),
            "file:///home/a%20b/w.png"
        );
        assert_eq!(
            file_uri(Path::new("/home/t/w-1_2.3~.png")),
            "file:///home/t/w-1_2.3~.png"
        );
        // A `#` would truncate the URI at the fragment, and a `?` at the query.
        assert_eq!(
            file_uri(Path::new("/t/a#b?c.png")),
            "file:///t/a%23b%3Fc.png"
        );
    }

    #[test]
    fn every_backend_names_the_program_its_commands_run() {
        // The probe happens before anything is rendered, so it has to look for
        // the program the commands will actually invoke.
        for Row { names, backend, .. } in ROWS {
            let desktop = names[0];
            let discovered = "/backdrop/screen0/monitor0/workspace0/image-style\n\
                              /backdrop/screen0/monitor0/workspace0/last-image";
            let placement = Placement {
                per_monitor: vec![("Virtual-1".to_owned(), PathBuf::from("/w.png"))],
                ..Placement::single(PathBuf::from("/w.png"))
            };
            let cmds = backend.commands(&placement, discovered);
            assert!(!cmds.is_empty(), "{desktop} produced no command");
            for cmd in &cmds {
                assert_eq!(Some(cmd.program), backend.program, "{desktop}");
            }
            if let Some(discovery) = backend.discovery() {
                assert_eq!(Some(discovery.program), backend.program, "{desktop}");
            }
        }
    }

    /// The gsettings schemas have one wallpaper and a fit mode, and `spanned` is
    /// the only thing in either that reaches past one screen.
    #[test]
    fn a_canvas_is_spanned_and_one_screens_image_is_zoomed() {
        for desktop in ["GNOME", "X-Cinnamon", "MATE", "Budgie"] {
            let backend = detect(desktop).expect(desktop);
            let spanned = Placement {
                per_monitor: Vec::new(),
                untouched: Vec::new(),
                by_position: vec![Some(PathBuf::from("/canvas.png"))],
                single: PathBuf::from("/canvas.png"),
                spanned: true,
            };
            let cmds = backend.commands(&spanned, "");
            assert_eq!(cmds[0].args[2], "picture-options", "{desktop}: {cmds:?}");
            assert_eq!(cmds[0].args[3], "spanned", "{desktop}: {cmds:?}");
            assert!(
                cmds.last().expect("a command").args[3].contains("canvas.png"),
                "{desktop}: {cmds:?}"
            );
        }
    }

    /// Every row says how far it reaches, and the reach is what decides what a
    /// publish writes rather than the sink guessing per desktop.
    ///
    /// Plasma has no span mode, so a view across the screens arrives already cut
    /// and goes out the same way every other per-monitor publish does, which is
    /// why it reaches every mode without degrading any of them.
    #[test]
    fn every_row_says_how_far_it_reaches() {
        assert_eq!(detect("XFCE").unwrap().reach(), Reach::PerMonitor);
        for desktop in ["GNOME", "X-Cinnamon", "MATE", "Budgie"] {
            assert_eq!(
                detect(desktop).unwrap().reach(),
                Reach::Spanned,
                "{desktop}"
            );
        }
        assert_eq!(detect("KDE").unwrap().reach(), Reach::PerMonitor);
        assert_eq!(detect("LXQt").unwrap().reach(), Reach::OneImage);
        for mode in DisplayMode::ALL {
            assert_eq!(
                detect("KDE").unwrap().degradation(mode, 2),
                None,
                "{mode:?}"
            );
        }
    }

    /// A mode a desktop cannot reach is not a failure, and the sentence that
    /// says what happened instead names the desktop.
    #[test]
    fn a_desktop_that_cannot_reach_the_mode_says_what_it_did_instead() {
        // One screen has nothing to explain: all three modes mean the same.
        for Row { backend, .. } in ROWS {
            for mode in DisplayMode::ALL {
                assert_eq!(backend.degradation(mode, 1), None, "{}", backend.desktop);
            }
        }
        // XFCE reaches every mode on any number of screens.
        let xfce = detect("XFCE").unwrap();
        for mode in DisplayMode::ALL {
            assert_eq!(xfce.degradation(mode, 2), None, "{mode:?}");
        }
        // GNOME can span and cannot do anything else per screen.
        let gnome = detect("GNOME").unwrap();
        assert_eq!(gnome.degradation(DisplayMode::AcrossScreens, 2), None);
        let note = gnome
            .degradation(DisplayMode::EveryScreen, 2)
            .expect("GNOME cannot give two screens two images");
        assert!(note.contains("GNOME"), "{note}");
        // LXQt sets one wallpaper and cannot span, so the span mode loses
        // something the other two do not and gets its own sentence.
        let lxqt = detect("LXQt").unwrap();
        let spanning = lxqt
            .degradation(DisplayMode::AcrossScreens, 2)
            .expect("LXQt has no span mode");
        assert!(spanning.contains("LXQt"), "{spanning}");
        assert_ne!(
            Some(spanning),
            lxqt.degradation(DisplayMode::EveryScreen, 3),
            "the span mode loses something the other modes do not"
        );
        assert!(
            lxqt.degradation(DisplayMode::OneScreen, 3)
                .expect("three screens all get the one image")
                .contains('3')
        );
    }

    #[test]
    fn no_two_backends_answer_to_the_same_name() {
        // One name in two rows would make the table's answer depend on its
        // order, which the detection deliberately does not use.
        let mut seen: Vec<&str> = ROWS.iter().flat_map(|row| row.names).copied().collect();
        let before = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), before, "a desktop name appears in two rows");
        // Nor two rows to the same setter name, which is what the forcing
        // variable looks them up by.
        let mut setters: Vec<&str> = ROWS.iter().map(|row| row.backend.setter).collect();
        let rows = setters.len();
        setters.sort_unstable();
        setters.dedup();
        assert_eq!(setters.len(), rows, "a setter name appears in two rows");
        // And every name is already lower case, since that is what it is
        // compared against.
        for name in seen {
            assert_eq!(name, name.to_ascii_lowercase(), "{name}");
        }
    }
}
