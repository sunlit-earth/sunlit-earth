//! Which Linux desktop this is, and how to tell it to change its wallpaper.
//!
//! Setting a wallpaper on Linux is a desktop-shell operation, not a display-server
//! one: it goes to GNOME's settings store, or to plasmashell, or to xfconf. So
//! there is no single call, there is a table, and what selects a row is
//! `XDG_CURRENT_DESKTOP`. The same row works in that desktop's X11 session and in
//! its Wayland one, because neither the shell nor the setting knows or cares which
//! is running.
//!
//! Not the XDG desktop portal, which is the other candidate and the one an
//! application would normally reach for. It targets sandboxed applications and
//! puts a confirmation dialog in front of every set; this wallpaper refreshes on a
//! schedule, and a dialog per refresh is not something to ship.
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
    /// `plasma-apply-wallpaperimage` is Plasma's own tool for this and writes
    /// every containment, so it can neither give two screens two pictures nor
    /// leave one alone. The scripting API behind it can do both:
    /// `org.kde.PlasmaShell.evaluateScript` runs JavaScript in the shell, where
    /// `desktops()` lists the containments, each carries the `screen` it is on,
    /// and `screenGeometry` says where that screen sits. Plasma has no span
    /// mode, which costs nothing here, because a view across the screens is
    /// already cut into one image per screen before it arrives.
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
    /// The program that has to exist for this backend to work at all. Probed
    /// before anything is rendered, because a wallpaper frame is the most
    /// expensive thing the engine does.
    pub program: &'static str,
    kind: Kind,
}

/// The XFCE backdrop property that holds an image path.
///
/// `last-image` is the one xfdesktop reads. The listing also contains
/// `image-style`, `color-style` and per-workspace colours, so the suffix is what
/// picks the right ones out.
const XFCE_IMAGE_PROPERTY: &str = "last-image";

/// The one beside it that says how to fit the image, and the value that fills.
///
/// xfdesktop's enum, in which 0 is None and shows no image at all.
const XFCE_STYLE_PROPERTY: &str = "image-style";
const XFCE_ZOOMED: &str = "5";

/// The channel those properties live in.
const XFCE_CHANNEL: &str = "xfce4-desktop";

/// The workspace whose backdrop the properties below belong to.
///
/// xfdesktop can give every workspace its own background, and by default does
/// not: `/backdrop/single-workspace-mode` is true out of the box and
/// `/backdrop/single-workspace-number` is 0, so workspace 0 is the one backdrop
/// the whole session shows. A session that turned that off and is sitting on
/// another workspace is the case this does not cover, and it is also a session
/// whose own listing already has the per-workspace properties, which the write
/// below covers because it writes every one of them.
const XFCE_WORKSPACE: &str = "workspace0";

/// The property xfdesktop reads for one monitor, by the monitor's own name.
///
/// This has to be built rather than only read out of the listing, and that is
/// the whole reason it exists. xfdesktop creates these lazily: a session where
/// nobody has ever changed the wallpaper has none of them, and the ones its
/// channel file does ship (`/backdrop/screen0/monitor0/...`, from a much older
/// xfdesktop) it does not read. Writing only what the listing offers is
/// therefore a set that reports success and changes nothing, which is what the
/// XFCE guest did: `last-image` held the right path under `monitor0` and the
/// desktop went on showing xfdesktop's built-in default.
fn xfce_live_property(monitor: &str, suffix: &str) -> String {
    format!("/backdrop/screen0/monitor{monitor}/{XFCE_WORKSPACE}/{suffix}")
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
            Kind::Gsettings { .. } | Kind::Kde | Kind::Lxqt => None,
        }
    }

    /// How far this desktop's setter reaches into a multi-monitor session.
    pub fn reach(&self) -> Reach {
        match self.kind {
            // XFCE names a backdrop property after each monitor and Plasma gives
            // each screen its own containment, so both hold one path per screen
            // and both can be told to leave a screen alone.
            Kind::Xfce | Kind::Kde => Reach::PerMonitor,
            // No per-monitor wallpaper in any of these schemas, but
            // `picture-options` has a `spanned` value that stretches one image
            // over the whole virtual desktop.
            Kind::Gsettings { .. } => Reach::Spanned,
            Kind::Lxqt => Reach::OneImage,
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
            Kind::Kde => vec![plasma_command(placement)],
            Kind::Xfce => {
                let write = |property: String, create: Option<&str>, value: String| {
                    let mut args = vec![
                        "-c".to_owned(),
                        XFCE_CHANNEL.to_owned(),
                        "-p".to_owned(),
                        property,
                    ];
                    // `-n` creates a property and fails on one that exists, so
                    // the two cases cannot share one command line.
                    if let Some(kind) = create {
                        args.push("-n".to_owned());
                        args.push("-t".to_owned());
                        args.push(kind.to_owned());
                    }
                    args.push("-s".to_owned());
                    args.push(value);
                    Invocation::new("xfconf-query", args)
                };
                // Every property of each kind that has to end up carrying a
                // value: the ones the session already has, plus the one
                // xfdesktop reads for each connected monitor, which a session
                // that has never had its wallpaper changed does not have yet.
                let plan = |suffix: &str, create_kind: &'static str| {
                    // A property naming a screen this publish left alone is not
                    // written at all, which is the only way to leave it alone.
                    let listed: Vec<String> = xfce_properties(discovered, suffix)
                        .filter(|property| {
                            xfce_monitor_of(property).is_none_or(|monitor| {
                                !placement.untouched.iter().any(|name| name == monitor)
                            })
                        })
                        .collect();
                    let missing: Vec<String> = placement
                        .per_monitor
                        .iter()
                        .map(|(monitor, _)| xfce_live_property(monitor, suffix))
                        .filter(|property| !listed.contains(property))
                        .collect();
                    listed
                        .into_iter()
                        .map(|property| (property, None))
                        .chain(
                            missing
                                .into_iter()
                                .map(move |property| (property, Some(create_kind))),
                        )
                        .collect::<Vec<_>>()
                };
                let images = plan(XFCE_IMAGE_PROPERTY, "string");
                // A set with nowhere to put the image would report a wallpaper
                // nothing is showing, so it is a refusal rather than a success.
                // Reachable only where the session lists no image property and
                // named no monitor, which is a session with no display to ask.
                if images.is_empty() {
                    return Vec::new();
                }
                // The style first, so the write that makes xfdesktop repaint is
                // the one carrying the new picture.
                plan(XFCE_STYLE_PROPERTY, "int")
                    .into_iter()
                    .map(|(property, create)| write(property, create, XFCE_ZOOMED.to_owned()))
                    .chain(images.into_iter().map(|(property, create)| {
                        // A property carries the picture of the monitor it is
                        // named after; one that names no monitor this session
                        // has takes the single image, which is what it held
                        // before there was more than one.
                        let image = xfce_monitor_of(&property)
                            .map_or(placement.single.as_path(), |monitor| {
                                placement.for_monitor(monitor)
                            })
                            .to_string_lossy()
                            .into_owned();
                        write(property, create, image)
                    }))
                    .collect()
            }
            Kind::Lxqt => vec![Invocation::new(
                "pcmanfm-qt",
                ["--set-wallpaper".to_owned(), single],
            )],
        }
    }

    /// Why this backend produced nothing to run, in the desktop's own terms.
    ///
    /// Only reachable for XFCE, and worth its own sentence rather than a generic
    /// failure: it means the session listed no backdrop property and named no
    /// monitor either, so there was nowhere to put the image and nowhere to
    /// create one, which is a different problem from a setter that ran and did
    /// not work.
    pub(crate) fn nothing_to_run(&self) -> String {
        format!(
            "{} has no wallpaper property to set: `xfconf-query -c {XFCE_CHANNEL} -l` \
             listed nothing ending in `{XFCE_IMAGE_PROPERTY}` and no connected \
             monitor was named, so there is no backdrop to write to",
            self.desktop
        )
    }
}

/// The monitor a backdrop property is named after, where it names one.
///
/// `/backdrop/screen0/monitorDP-1/workspace0/last-image` is `DP-1`. A property
/// from a much older xfdesktop names a screen index instead
/// (`/backdrop/screen0/monitor0/...`), which matches no connected monitor and so
/// falls back to the single image, which is what it held before this.
fn xfce_monitor_of(property: &str) -> Option<&str> {
    property
        .split('/')
        .find_map(|segment| segment.strip_prefix("monitor"))
        .filter(|monitor| !monitor.is_empty())
}

/// The backdrop properties in `xfconf-query -c xfce4-desktop -l` with one suffix.
fn xfce_properties<'a>(
    listing: &'a str,
    suffix: &'a str,
) -> impl Iterator<Item = String> + use<'a> {
    listing
        .lines()
        .map(str::trim)
        .filter(move |line| line.starts_with("/backdrop/") && line.ends_with(suffix))
        .map(ToOwned::to_owned)
}

/// The one call that puts this publish on Plasma's screens.
fn plasma_command(placement: &Placement) -> Invocation {
    Invocation::new(
        "dbus-send",
        [
            "--session".to_owned(),
            "--dest=org.kde.plasmashell".to_owned(),
            "--type=method_call".to_owned(),
            "/PlasmaShell".to_owned(),
            "org.kde.PlasmaShell.evaluateScript".to_owned(),
            format!("string:{}", plasma_script(placement)),
        ],
    )
}

/// The JavaScript `evaluateScript` runs to put one image on each screen.
///
/// Plasma orders its containments however it likes and renumbers them when the
/// layout changes, so the script sorts them by where their screens sit and
/// matches that against `by_position`, which is sorted the same way. A
/// containment on no screen (`screen` is -1) is not a desktop anyone can see and
/// is dropped before the sort; a screen whose entry is null is one this publish
/// left alone and is stepped over, which is what keeps the rest aligned.
///
/// `wallpaperPlugin` is written every time rather than only when it differs,
/// because a screen left on a colour or a slideshow would otherwise take the
/// image into a plugin that does not read it and show nothing.
fn plasma_script(placement: &Placement) -> String {
    let images = placement
        .by_position
        .iter()
        .map(|slot| slot.as_deref().map_or_else(|| "null".to_owned(), js_string))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "var images=[{images}];\
         var screens=desktops().filter(function(d){{return d.screen!=-1;}});\
         screens.sort(function(a,b){{\
         var x=screenGeometry(a.screen),y=screenGeometry(b.screen);\
         return x.left-y.left||x.top-y.top;}});\
         for(var i=0;i<screens.length&&i<images.length;i++){{\
         if(images[i]===null){{continue;}}\
         var d=screens[i];\
         d.wallpaperPlugin=\"org.kde.image\";\
         d.currentConfigGroup=[\"Wallpaper\",\"org.kde.image\",\"General\"];\
         d.writeConfig(\"Image\",images[i]);}}"
    )
}

/// One path as a JavaScript string literal, as a `file:` URI.
///
/// Plasma stores the key as a URL and hands back what it was given, so a bare
/// path round-trips through the config and then fails to load. The escaping is
/// belt and braces over `file_uri`, which already percent-encodes everything
/// outside an unreserved set and so can produce neither a quote nor a backslash.
fn js_string(path: &Path) -> String {
    let escaped = file_uri(path).replace('\\', r"\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
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

/// Every backend, with the names each desktop identifies itself by.
///
/// The names are matched case-insensitively against the tokens of
/// [`DESKTOP_ENV`], which is how one row covers the several spellings a desktop
/// has had: `X-Cinnamon` and `Cinnamon`, `KDE` and `plasma`.
///
/// Four of these are verified live, one boot per desktop in the Linux test guest
/// (phase 5 decision 9): KDE, GNOME, XFCE and Cinnamon. MATE, `LXQt` and Budgie
/// are in the table because their setters are documented and their rows cost one
/// line each, and they ship reviewed rather than exercised. `docs/roadmap.md`
/// says so rather than the docs claiming support that never ran.
const BACKENDS: &[(&[&str], Backend)] = &[
    (
        &["kde", "plasma"],
        Backend {
            desktop: "KDE Plasma",
            program: "dbus-send",
            kind: Kind::Kde,
        },
    ),
    (
        &["xfce"],
        Backend {
            desktop: "XFCE",
            program: "xfconf-query",
            kind: Kind::Xfce,
        },
    ),
    (
        &["x-cinnamon", "cinnamon"],
        Backend {
            desktop: "Cinnamon",
            program: "gsettings",
            kind: Kind::Gsettings {
                schema: "org.cinnamon.desktop.background",
                keys: &["picture-uri"],
                uri: true,
                fill: Some("picture-options"),
            },
        },
    ),
    (
        &["mate"],
        Backend {
            desktop: "MATE",
            program: "gsettings",
            kind: Kind::Gsettings {
                schema: "org.mate.background",
                // A path, not a URI: the key is named for what it holds.
                keys: &["picture-filename"],
                uri: false,
                fill: Some("picture-options"),
            },
        },
    ),
    (
        &["lxqt"],
        Backend {
            desktop: "LXQt",
            program: "pcmanfm-qt",
            kind: Kind::Lxqt,
        },
    ),
    (
        // Budgie is a GNOME shell replacement and keeps GNOME's settings store,
        // so it takes the same row's mechanism under its own name. It has to be
        // its own row rather than falling through to GNOME's, because the
        // refusal messages name what was detected.
        &["budgie"],
        Backend {
            desktop: "Budgie",
            program: "gsettings",
            kind: GNOME_BACKGROUND,
        },
    ),
    (
        &["gnome", "unity", "gnome-classic", "gnome-flashback"],
        Backend {
            desktop: "GNOME",
            program: "gsettings",
            kind: GNOME_BACKGROUND,
        },
    ),
];

/// Both keys, because GNOME picks between them by the current colour scheme and
/// setting only one leaves the other theme showing the previous wallpaper.
const GNOME_BACKGROUND: Kind = Kind::Gsettings {
    schema: "org.gnome.desktop.background",
    keys: &["picture-uri", "picture-uri-dark"],
    uri: true,
    fill: Some("picture-options"),
};

/// The backend for a desktop, from the value of [`DESKTOP_ENV`].
///
/// The list is walked in the order the desktop wrote it, and the first token
/// that names a backend wins, so `Budgie:GNOME` is Budgie and `ubuntu:GNOME` is
/// GNOME. Matching the table in its own order instead would make the first row
/// win regardless of what the session said it was.
fn detect(current_desktop: &str) -> Option<Backend> {
    current_desktop
        .split(':')
        .map(|token| token.trim().to_ascii_lowercase())
        .filter(|token| !token.is_empty())
        .find_map(|token| {
            BACKENDS
                .iter()
                .find(|(names, _)| names.contains(&token.as_str()))
                .map(|(_, backend)| *backend)
        })
}

/// The backend for this process's session.
pub fn detect_current() -> Option<Backend> {
    detect(&crate::env_override(DESKTOP_ENV).unwrap_or_default())
}

/// What to say when there is no backend for this session.
///
/// Names what was detected rather than only refusing, because the two causes
/// look identical from the outside: a desktop with no row in the table, and no
/// desktop at all.
pub(crate) fn no_backend_message(current_desktop: &str) -> String {
    let known: Vec<&str> = BACKENDS.iter().map(|(_, b)| b.desktop).collect();
    if current_desktop.trim().is_empty() {
        format!(
            "no desktop session to set a wallpaper in: {DESKTOP_ENV} is not set. \
             The desktops with a setter here are {}.",
            known.join(", ")
        )
    } else {
        format!(
            "setting the wallpaper is not supported on {current_desktop}. \
             The desktops with a setter here are {}.",
            known.join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commands(desktop: &str, path: &str, discovered: &str) -> Vec<Invocation> {
        commands_on(desktop, path, discovered, &[])
    }

    /// One image, shown on every monitor named, which is what a single-screen
    /// session and every reach short of per-monitor come down to.
    fn commands_on(
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
    fn two_screens() -> Placement {
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
    }

    #[test]
    fn the_dark_key_is_set_too_because_gnome_chooses_between_them() {
        // Setting only `picture-uri` leaves the dark theme showing the previous
        // wallpaper, and which one is showing depends on a setting this app has
        // no business reading.
        let keys: Vec<String> = commands("GNOME", "/w.png", "")
            .into_iter()
            .map(|c| c.args[2].clone())
            .collect();
        assert!(keys.contains(&"picture-uri-dark".to_owned()), "{keys:?}");
        // Cinnamon's schema has only the one key, so this is per row rather than
        // a rule.
        let cinnamon: Vec<String> = commands("X-Cinnamon", "/w.png", "")
            .into_iter()
            .map(|c| c.args[2].clone())
            .collect();
        assert!(
            !cinnamon.contains(&"picture-uri-dark".to_owned()),
            "{cinnamon:?}"
        );
    }

    /// The frame is rendered at the display's exact resolution, so a desktop set
    /// to centre or tile it shows it at the wrong size on a background of its
    /// own. The Windows sink writes `WallpaperStyle=10` for the same reason.
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

        let refusal = no_backend_message("Enlightenment");
        assert!(refusal.contains("Enlightenment"), "{refusal}");
        assert!(refusal.contains("KDE Plasma"), "{refusal}");
        // No session at all is a different sentence, because the cause is.
        let unset = no_backend_message("");
        assert!(unset.contains(DESKTOP_ENV), "{unset}");
        assert!(!unset.contains("not supported on"), "{unset}");
    }

    /// The script is one `dbus-send` call whatever the screen count, and the
    /// path reaches Plasma as a URI because that is what it stores.
    #[test]
    fn kde_runs_one_plasmashell_script() {
        let cmds = commands("KDE", "/home/tester/w.png", "");
        let [only] = cmds.as_slice() else {
            panic!("one command, not {cmds:?}")
        };
        assert_eq!(only.program, "dbus-send");
        assert_eq!(only.args[3], "/PlasmaShell");
        assert_eq!(only.args[4], "org.kde.PlasmaShell.evaluateScript");
        let script = &only.args[5];
        assert!(script.starts_with("string:"), "{script}");
        assert!(
            script.contains(r#"["file:///home/tester/w.png"]"#),
            "{script}"
        );
        assert!(
            script.contains(r#"d.writeConfig("Image",images[i])"#),
            "{script}"
        );
    }

    /// Two screens, two pictures, in the order a person sees them rather than
    /// the order the query answered in.
    #[test]
    fn kde_gives_each_screen_its_own_picture() {
        let cmds = detect("KDE")
            .expect("a backend")
            .commands(&two_screens(), "");
        let script = &cmds[0].args[5];
        assert!(
            script.contains(r#"["file:///w-0.png","file:///w-1.png"]"#),
            "{script}"
        );
        // Plasma renumbers its containments when the layout changes, so the
        // script has to put them in order itself rather than trust the index.
        assert!(script.contains("screens.sort("), "{script}");
        assert!(script.contains("d.screen!=-1"), "{script}");
    }

    /// One-screen mode on a two-screen session: the other screen keeps what it
    /// had, and the hole is what keeps the painted one addressed correctly.
    #[test]
    fn kde_steps_over_a_screen_the_publish_left_alone() {
        let placement = Placement {
            per_monitor: vec![("Virtual-2".to_owned(), PathBuf::from("/w-1.png"))],
            untouched: vec!["Virtual-1".to_owned()],
            by_position: vec![None, Some(PathBuf::from("/w-1.png"))],
            single: PathBuf::from("/w-1.png"),
            spanned: false,
        };
        let cmds = detect("KDE").expect("a backend").commands(&placement, "");
        let script = &cmds[0].args[5];
        assert!(script.contains(r#"[null,"file:///w-1.png"]"#), "{script}");
        assert!(
            script.contains("if(images[i]===null){continue;}"),
            "{script}"
        );
    }

    /// Plasma has no span mode, so a view across the screens arrives already cut
    /// and goes out the same way every other per-monitor publish does.
    #[test]
    fn kde_reaches_every_monitor_and_lxqt_does_not() {
        assert_eq!(detect("KDE").expect("a backend").reach(), Reach::PerMonitor);
        assert_eq!(detect("LXQt").expect("a backend").reach(), Reach::OneImage);
        for mode in [
            DisplayMode::OneScreen,
            DisplayMode::EveryScreen,
            DisplayMode::AcrossScreens,
        ] {
            assert_eq!(
                detect("KDE").expect("a backend").degradation(mode, 2),
                None,
                "{mode:?}"
            );
        }
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
    fn xfce_asks_which_backdrops_exist_before_setting_any() {
        let backend = detect("XFCE").expect("a backend");
        let discovery = backend.discovery().expect("XFCE asks first");
        assert_eq!(discovery.program, "xfconf-query");
        assert_eq!(discovery.args, vec!["-c", "xfce4-desktop", "-l"]);
        // Nothing else asks anything.
        for desktop in ["GNOME", "KDE", "X-Cinnamon", "MATE", "LXQt", "Budgie"] {
            assert!(
                detect(desktop).expect(desktop).discovery().is_none(),
                "{desktop}"
            );
        }
    }

    #[test]
    fn xfce_sets_every_backdrop_that_holds_an_image_and_nothing_else() {
        // Real `xfconf-query -c xfce4-desktop -l` output: two workspaces on one
        // monitor, and the properties that are not images.
        let listing = "\
/backdrop/screen0/monitorVirtual-1/workspace0/color-style
/backdrop/screen0/monitorVirtual-1/workspace0/image-style
/backdrop/screen0/monitorVirtual-1/workspace0/last-image
/backdrop/screen0/monitorVirtual-1/workspace1/last-image
/backdrop/single-workspace-mode
";
        let cmds = commands("XFCE", "/home/tester/w.png", listing);
        // One style property in the listing, two image ones, and nothing for the
        // colours or the workspace-mode flag.
        assert_eq!(cmds.len(), 3, "{cmds:?}");
        for cmd in &cmds {
            assert_eq!(cmd.program, "xfconf-query");
            assert!(cmd.args[3].starts_with("/backdrop/"), "{cmd:?}");
        }
        // The style comes first, so the write carrying the image is the one that
        // makes xfdesktop repaint.
        assert!(cmds[0].args[3].ends_with("image-style"), "{cmds:?}");
        assert_eq!(cmds[0].args[5], "5", "zoomed, which is what fills");
        assert_eq!(
            cmds[1].args[3],
            "/backdrop/screen0/monitorVirtual-1/workspace0/last-image"
        );
        assert_eq!(
            cmds[2].args[3],
            "/backdrop/screen0/monitorVirtual-1/workspace1/last-image"
        );
        for cmd in &cmds[1..] {
            assert_eq!(cmd.args[5], "/home/tester/w.png");
        }
    }

    /// The XFCE guest set its wallpaper, reported success, and went on showing
    /// xfdesktop's own default. Its listing had `last-image` only under
    /// `/backdrop/screen0/monitor0`, which is a much older xfdesktop's property
    /// and one this one does not read; the property it does read is named after
    /// the connected monitor and does not exist until something creates it.
    #[test]
    fn xfce_creates_the_property_its_own_monitor_is_named_after() {
        let listing = "\
/backdrop/screen0/monitor0/image-path
/backdrop/screen0/monitor0/last-image
/backdrop/single-workspace-mode
";
        let cmds = commands_on("XFCE", "/home/tester/w.png", listing, &["Virtual-1"]);
        let line = |cmd: &Invocation| cmd.args.join(" ");
        let all: Vec<String> = cmds.iter().map(line).collect();
        // The live image property is created, with a type, because a property
        // that does not exist cannot be set.
        assert!(
            all.contains(
                &"-c xfce4-desktop -p /backdrop/screen0/monitorVirtual-1/workspace0/last-image \
                  -n -t string -s /home/tester/w.png"
                    .replace("  ", " ")
            ),
            "{all:?}"
        );
        // So is its fill mode, since the session has none for that monitor.
        assert!(
            all.contains(
                &"-c xfce4-desktop -p /backdrop/screen0/monitorVirtual-1/workspace0/image-style \
                  -n -t int -s 5"
                    .replace("  ", " ")
            ),
            "{all:?}"
        );
        // The legacy property is still written, since a session that reads it is
        // a session this would otherwise stop working on.
        assert!(
            all.contains(
                &"-c xfce4-desktop -p /backdrop/screen0/monitor0/last-image -s /home/tester/w.png"
                    .to_owned()
            ),
            "{all:?}"
        );
        // Fill mode before image, still.
        assert!(cmds[0].args[3].ends_with(XFCE_STYLE_PROPERTY), "{all:?}");
        assert!(
            cmds.last().expect("a command").args[3].ends_with(XFCE_IMAGE_PROPERTY),
            "{all:?}"
        );
    }

    /// A property the session already has must not be created again: `-n` fails
    /// on one that exists, and xfconf-query's failure would fail the publish.
    #[test]
    fn xfce_does_not_create_a_property_the_session_already_has() {
        let listing = "\
/backdrop/screen0/monitorVirtual-1/workspace0/image-style
/backdrop/screen0/monitorVirtual-1/workspace0/last-image
";
        let cmds = commands_on("XFCE", "/w.png", listing, &["Virtual-1"]);
        assert_eq!(cmds.len(), 2, "{cmds:?}");
        for cmd in &cmds {
            assert!(!cmd.args.contains(&"-n".to_owned()), "{cmd:?}");
        }
    }

    #[test]
    fn an_xfce_session_with_a_style_but_no_image_property_is_still_a_refusal() {
        // Setting a fill mode on a desktop with nowhere to put the image would
        // report a wallpaper nothing is showing.
        let backend = detect("XFCE").expect("a backend");
        let listing = "/backdrop/screen0/monitor0/workspace0/image-style\n";
        assert!(
            backend
                .commands(&Placement::single(PathBuf::from("/w.png")), listing)
                .is_empty()
        );
    }

    #[test]
    fn an_xfce_session_with_no_backdrop_property_is_a_refusal_not_a_success() {
        // Nothing to run must never read as a wallpaper that was set.
        let backend = detect("XFCE").expect("a backend");
        let placement = Placement::single(PathBuf::from("/w.png"));
        assert!(
            backend
                .commands(&placement, "/backdrop/single-workspace-mode\n")
                .is_empty()
        );
        assert!(backend.commands(&placement, "").is_empty());
        let message = backend.nothing_to_run();
        assert!(message.contains("XFCE"), "{message}");
        assert!(message.contains("last-image"), "{message}");
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
        for (names, backend) in BACKENDS {
            let desktop = names[0];
            let discovered = "/backdrop/screen0/monitor0/workspace0/image-style\n\
                              /backdrop/screen0/monitor0/workspace0/last-image";
            let cmds = backend.commands(&Placement::single(PathBuf::from("/w.png")), discovered);
            assert!(!cmds.is_empty(), "{desktop} produced no command");
            for cmd in &cmds {
                assert_eq!(cmd.program, backend.program, "{desktop}");
            }
            if let Some(discovery) = backend.discovery() {
                assert_eq!(discovery.program, backend.program, "{desktop}");
            }
        }
    }

    /// The one row that can address a screen, doing it.
    #[test]
    fn xfce_gives_each_monitor_the_picture_that_monitor_is_named_after() {
        let listing = "\
/backdrop/screen0/monitorVirtual-1/workspace0/last-image
/backdrop/screen0/monitorVirtual-2/workspace0/last-image
";
        let cmds = detect("XFCE")
            .expect("a backend")
            .commands(&two_screens(), listing);
        let images: Vec<&String> = cmds
            .iter()
            .filter(|c| c.args[3].ends_with(XFCE_IMAGE_PROPERTY))
            .map(|c| &c.args[5])
            .collect();
        assert_eq!(images, vec!["/w-0.png", "/w-1.png"], "{cmds:?}");
    }

    /// A property naming a monitor this session does not have is the legacy
    /// `monitor0` shape, and it keeps holding the one image it always held.
    #[test]
    fn an_xfce_property_naming_no_connected_monitor_takes_the_single_image() {
        let listing = "/backdrop/screen0/monitor0/workspace0/last-image\n";
        let cmds = detect("XFCE")
            .expect("a backend")
            .commands(&two_screens(), listing);
        let legacy = cmds
            .iter()
            .find(|c| c.args[3] == "/backdrop/screen0/monitor0/workspace0/last-image")
            .expect("the legacy property is still written");
        assert_eq!(legacy.args[5], "/w-0.png");
        assert_eq!(
            xfce_monitor_of("/backdrop/screen0/monitorDP-1/w0/x"),
            Some("DP-1")
        );
        assert_eq!(xfce_monitor_of("/backdrop/single-workspace-mode"), None);
    }

    /// A screen the mode does not paint keeps whatever it was showing, which is
    /// what makes one-screen mode an escape hatch rather than a narrower version
    /// of the same thing.
    #[test]
    fn xfce_does_not_write_the_property_of_a_screen_the_publish_left_alone() {
        let listing = "\
/backdrop/screen0/monitorVirtual-1/workspace0/last-image
/backdrop/screen0/monitorVirtual-2/workspace0/last-image
";
        let placement = Placement {
            per_monitor: vec![("Virtual-1".to_owned(), PathBuf::from("/w-0.png"))],
            untouched: vec!["Virtual-2".to_owned()],
            by_position: vec![Some(PathBuf::from("/w-0.png")), None],
            single: PathBuf::from("/w-0.png"),
            spanned: false,
        };
        let cmds = detect("XFCE")
            .expect("a backend")
            .commands(&placement, listing);
        assert!(
            cmds.iter().all(|c| !c.args[3].contains("Virtual-2")),
            "{cmds:?}"
        );
        assert!(
            cmds.iter().any(|c| c.args[3].contains("Virtual-1")),
            "{cmds:?}"
        );
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
    }

    /// A mode a desktop cannot reach is not a failure, and the sentence that
    /// says what happened instead names the desktop.
    #[test]
    fn a_desktop_that_cannot_reach_the_mode_says_what_it_did_instead() {
        // One screen has nothing to explain: all three modes mean the same.
        for (_, backend) in BACKENDS {
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
        let mut seen: Vec<&str> = BACKENDS
            .iter()
            .flat_map(|(names, _)| *names)
            .copied()
            .collect();
        let before = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), before, "a desktop name appears in two rows");
        // And every name is already lower case, since that is what it is
        // compared against.
        for name in seen {
            assert_eq!(name, name.to_ascii_lowercase(), "{name}");
        }
    }
}
