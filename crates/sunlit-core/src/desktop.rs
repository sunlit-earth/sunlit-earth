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

use std::path::Path;

/// The variable that says which desktop this is.
///
/// A colon-separated list, most specific first, which is what makes the order
/// meaningful: Budgie sets `Budgie:GNOME` and Ubuntu sets `ubuntu:GNOME`, so
/// walking the list in order picks the desktop that claimed to be itself before
/// the one it is built on.
pub const DESKTOP_ENV: &str = "XDG_CURRENT_DESKTOP";

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
    },
    /// `plasma-apply-wallpaperimage <path>`, which is Plasma's own tool for
    /// exactly this and does the plasmashell scripting itself.
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

/// The XFCE backdrop properties that hold an image path.
///
/// `last-image` is the one xfdesktop reads. The list also contains
/// `image-style`, `color-style` and per-workspace colours, so the suffix is what
/// picks the right ones out.
const XFCE_IMAGE_PROPERTY: &str = "last-image";

/// The channel those properties live in.
const XFCE_CHANNEL: &str = "xfce4-desktop";

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

    /// Everything to run, in order, to make `image` this desktop's wallpaper.
    ///
    /// Empty means the desktop was asked something and answered with nothing
    /// usable, which is a refusal rather than a success: the caller must not
    /// report a wallpaper it did not set.
    pub fn commands(&self, image: &Path, discovered: &str) -> Vec<Invocation> {
        let path = image.to_string_lossy().into_owned();
        match self.kind {
            Kind::Gsettings { schema, keys, uri } => {
                let value = if uri { file_uri(image) } else { path.clone() };
                keys.iter()
                    .map(|key| {
                        Invocation::new(
                            "gsettings",
                            [
                                "set".to_owned(),
                                schema.to_owned(),
                                (*key).to_owned(),
                                value.clone(),
                            ],
                        )
                    })
                    .collect()
            }
            Kind::Kde => vec![Invocation::new("plasma-apply-wallpaperimage", [path])],
            Kind::Xfce => xfce_backdrop_properties(discovered)
                .map(|property| {
                    Invocation::new(
                        "xfconf-query",
                        [
                            "-c".to_owned(),
                            XFCE_CHANNEL.to_owned(),
                            "-p".to_owned(),
                            property,
                            "-s".to_owned(),
                            path.clone(),
                        ],
                    )
                })
                .collect(),
            Kind::Lxqt => vec![Invocation::new(
                "pcmanfm-qt",
                ["--set-wallpaper".to_owned(), path],
            )],
        }
    }

    /// Why this backend produced nothing to run, in the desktop's own terms.
    ///
    /// Only reachable for XFCE, and worth its own sentence rather than a generic
    /// failure: a session whose backdrop channel is empty has never had a
    /// wallpaper set by anything, which is a different problem from a setter that
    /// ran and did not work.
    pub fn nothing_to_run(&self) -> String {
        format!(
            "{} has no wallpaper property to set: `xfconf-query -c {XFCE_CHANNEL} -l` \
             listed nothing ending in `{XFCE_IMAGE_PROPERTY}`, which means this \
             session's desktop has never had a background of its own",
            self.desktop
        )
    }
}

/// The properties out of `xfconf-query -c xfce4-desktop -l` that hold an image.
fn xfce_backdrop_properties(listing: &str) -> impl Iterator<Item = String> + '_ {
    listing
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("/backdrop/") && line.ends_with(XFCE_IMAGE_PROPERTY))
        .map(ToOwned::to_owned)
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
            program: "plasma-apply-wallpaperimage",
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
};

/// The backend for a desktop, from the value of [`DESKTOP_ENV`].
///
/// The list is walked in the order the desktop wrote it, and the first token
/// that names a backend wins, so `Budgie:GNOME` is Budgie and `ubuntu:GNOME` is
/// GNOME. Matching the table in its own order instead would make the first row
/// win regardless of what the session said it was.
pub fn detect(current_desktop: &str) -> Option<Backend> {
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
pub fn no_backend_message(current_desktop: &str) -> String {
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
        detect(desktop)
            .unwrap_or_else(|| panic!("{desktop} has a backend"))
            .commands(Path::new(path), discovered)
    }

    #[test]
    fn gnome_sets_both_keys_as_uris() {
        let cmds = commands("GNOME", "/home/tester/w.png", "");
        assert_eq!(cmds.len(), 2, "{cmds:?}");
        for (cmd, key) in cmds.iter().zip(["picture-uri", "picture-uri-dark"]) {
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

    #[test]
    fn kde_hands_the_path_to_plasmas_own_tool() {
        let cmds = commands("KDE", "/home/tester/w.png", "");
        assert_eq!(
            cmds,
            vec![Invocation {
                program: "plasma-apply-wallpaperimage",
                args: vec!["/home/tester/w.png".to_owned()],
            }]
        );
    }

    #[test]
    fn mate_takes_a_path_where_the_others_take_a_uri() {
        // The key is `picture-filename`, and a URI in it leaves the desktop with
        // no wallpaper and no complaint.
        let cmds = commands("MATE", "/home/tester/w.png", "");
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].args[3], "/home/tester/w.png");
        assert!(!cmds[0].args[3].contains("file://"));
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
        assert_eq!(cmds.len(), 2, "{cmds:?}");
        for cmd in &cmds {
            assert_eq!(cmd.program, "xfconf-query");
            assert!(cmd.args[3].ends_with("last-image"), "{cmd:?}");
            assert_eq!(cmd.args[5], "/home/tester/w.png");
        }
        assert_eq!(
            cmds[0].args[3],
            "/backdrop/screen0/monitorVirtual-1/workspace0/last-image"
        );
        assert_eq!(
            cmds[1].args[3],
            "/backdrop/screen0/monitorVirtual-1/workspace1/last-image"
        );
    }

    #[test]
    fn an_xfce_session_with_no_backdrop_property_is_a_refusal_not_a_success() {
        // Nothing to run must never read as a wallpaper that was set.
        let backend = detect("XFCE").expect("a backend");
        assert!(
            backend
                .commands(Path::new("/w.png"), "/backdrop/single-workspace-mode\n")
                .is_empty()
        );
        assert!(backend.commands(Path::new("/w.png"), "").is_empty());
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
            let discovered = "/backdrop/screen0/monitor0/workspace0/last-image";
            let cmds = backend.commands(Path::new("/w.png"), discovered);
            assert!(!cmds.is_empty(), "{desktop} produced no command");
            for cmd in &cmds {
                assert_eq!(cmd.program, backend.program, "{desktop}");
            }
            if let Some(discovery) = backend.discovery() {
                assert_eq!(discovery.program, backend.program, "{desktop}");
            }
        }
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
