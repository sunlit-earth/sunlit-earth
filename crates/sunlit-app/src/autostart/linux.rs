//! Linux: an XDG autostart entry, `sunlit-earth.desktop` in
//! `$XDG_CONFIG_HOME/autostart`.
//!
//! GNOME, XFCE and Cinnamon's session managers start the entries in that
//! folder, and Plasma 6 does through `systemd-xdg-autostart-generator`. Plasma
//! and XFCE switch an entry off with `Hidden=true`, Cinnamon with
//! `X-GNOME-Autostart-enabled=false`, and GNOME Tweaks deletes the file, so
//! the entry is on when the file exists and neither key turns it off. The
//! app's own switch deletes the file, which every desktop understands. sway,
//! i3 and Hyprland start nothing from the folder.

use std::path::{Component, Path, PathBuf};

use super::{ARGS, Autostart};

/// The entry's file name, the basename of the app's own desktop entry.
pub const FILE_NAME: &str = "sunlit-earth.desktop";

/// The sessions that start nothing from the autostart folder, each with what
/// its config starts a program at login with.
const SESSIONS_WITHOUT_AUTOSTART: [(&str, &str); 3] = [
    ("sway", "exec"),
    ("i3", "exec"),
    ("Hyprland", "exec-once ="),
];

/// The autostart folder for a given environment.
///
/// :param `xdg_config_home`: `XDG_CONFIG_HOME`; blank or relative is unset,
///     as the base directory specification says
/// :param home: the home directory
/// :returns: the folder, or `None` with neither to go on
#[must_use]
pub fn autostart_dir(xdg_config_home: Option<&str>, home: Option<&Path>) -> Option<PathBuf> {
    match xdg_config_home.map(str::trim).map(Path::new) {
        Some(config) if config.is_absolute() => Some(config.join("autostart")),
        _ => home.map(|home| home.join(".config").join("autostart")),
    }
}

/// The autostart folder of this session.
#[must_use]
pub fn system_dir() -> Option<PathBuf> {
    let config = std::env::var("XDG_CONFIG_HOME").ok();
    let home = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from);
    autostart_dir(config.as_deref(), home.as_deref())
}

/// Why this session cannot be given an entry, with the line the user can add
/// instead, or `None` when it starts entries from the folder.
///
/// :param `current_desktop`: `XDG_CURRENT_DESKTOP`, a colon-separated list
/// :param exe: the executable the line would start
#[must_use]
pub fn unsupported_session(current_desktop: Option<&str>, exe: Option<&Path>) -> Option<String> {
    let desktops = current_desktop.unwrap_or_default();
    let keyword = desktops.split(':').find_map(|desktop| {
        SESSIONS_WITHOUT_AUTOSTART
            .iter()
            .find(|(name, _)| desktop.trim().eq_ignore_ascii_case(name))
            .map(|(_, keyword)| *keyword)
    })?;
    let exe = exe.map_or_else(
        || "sunlit-earth".to_owned(),
        |exe| {
            let exe = exe.to_string_lossy();
            if exe.contains(char::is_whitespace) {
                format!("\"{exe}\"")
            } else {
                exe.into_owned()
            }
        },
    );
    Some(format!(
        "This session does not start apps from the autostart folder. Add `{keyword} {exe} {}` to its config.",
        ARGS.join(" ")
    ))
}

/// The path an entry should start: the Homebrew `opt` alias when `exe` is a
/// versioned `Cellar` path the next `brew upgrade` removes and the alias
/// resolves to the same file, and `exe` itself otherwise.
#[must_use]
pub fn stable_path(exe: &Path) -> PathBuf {
    match homebrew_alias(exe) {
        Some(alias) if same_file(&alias, exe) => alias,
        _ => exe.to_path_buf(),
    }
}

/// `<prefix>/opt/sunlit-earth/bin/sunlit-earth` for a path under
/// `<prefix>/Cellar/sunlit-earth/<version>/`, whether or not it exists.
#[must_use]
pub fn homebrew_alias(exe: &Path) -> Option<PathBuf> {
    let components: Vec<Component> = exe.components().collect();
    let at = components.windows(3).position(|window| {
        window[0].as_os_str() == "Cellar"
            && window[1].as_os_str() == "sunlit-earth"
            && matches!(window[2], Component::Normal(_))
    })?;
    if at + 3 >= components.len() {
        return None;
    }
    let prefix: PathBuf = components[..at].iter().collect();
    Some(
        prefix
            .join("opt")
            .join("sunlit-earth")
            .join("bin")
            .join("sunlit-earth"),
    )
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// The characters the desktop entry specification reserves in an `Exec`
/// argument, which put it in quotes.
const RESERVED: &[char] = &[
    ' ', '\t', '\n', '"', '\'', '\\', '>', '<', '~', '|', '&', ';', '$', '*', '?', '#', '(', ')',
    '`',
];

/// One `Exec` argument: quoted with `"`, `` ` ``, `$` and `\` escaped when it
/// holds a reserved character, as it is before the string escapes.
fn exec_argument(arg: &str) -> String {
    if !arg.contains(RESERVED) {
        return arg.to_owned();
    }
    let mut quoted = String::from("\"");
    for c in arg.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    quoted
}

/// The string escapes of the key file format.
fn escape_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\t' => escaped.push_str("\\t"),
            '\r' => escaped.push_str("\\r"),
            _ => escaped.push(c),
        }
    }
    escaped
}

/// The `Exec` value that runs `exe` with [`ARGS`]: each argument quoted where
/// it needs to be, `%` doubled so no field code is read into it, and the
/// string escapes on top.
#[must_use]
pub fn exec_value(exe: &Path) -> String {
    let line = std::iter::once(exe.to_string_lossy().into_owned())
        .chain(ARGS.iter().map(|arg| (*arg).to_owned()))
        .map(|arg| exec_argument(&arg))
        .collect::<Vec<_>>()
        .join(" ")
        .replace('%', "%%");
    escape_string(&line)
}

/// The whole entry for `exe`.
#[must_use]
pub fn entry(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Sunlit Earth\n\
         Comment=Keep the Earth wallpaper up to date\n\
         Exec={}\n\
         TryExec={}\n\
         Icon=sunlit-earth\n\
         Terminal=false\n\
         StartupNotify=false\n\
         X-GNOME-Autostart-enabled=true\n",
        exec_value(exe),
        escape_string(&exe.to_string_lossy()),
    )
}

/// The keys of the `[Desktop Entry]` group the state is read from.
#[derive(Debug, Default)]
struct EntryKeys {
    exec: Option<String>,
    try_exec: Option<String>,
    hidden: Option<String>,
    gnome_enabled: Option<String>,
}

fn entry_keys(text: &str) -> EntryKeys {
    let mut keys = EntryKeys::default();
    let mut in_entry = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().to_owned();
        match key.trim() {
            "Exec" => keys.exec = Some(value),
            "TryExec" => keys.try_exec = Some(value),
            "Hidden" => keys.hidden = Some(value),
            "X-GNOME-Autostart-enabled" => keys.gnome_enabled = Some(value),
            _ => {}
        }
    }
    keys
}

/// The state an entry's text comes to.
#[must_use]
pub fn entry_state(text: &str) -> Autostart {
    let keys = entry_keys(text);
    let exe = keys
        .exec
        .as_deref()
        .and_then(|exec| parse_exec(exec).into_iter().next())
        .or_else(|| keys.try_exec.as_deref().map(unescape_string))
        .map(PathBuf::from)
        .unwrap_or_default();
    let is = |value: Option<&String>, word: &str| {
        value.is_some_and(|value| value.eq_ignore_ascii_case(word))
    };
    if is(keys.hidden.as_ref(), "true") || is(keys.gnome_enabled.as_ref(), "false") {
        Autostart::TurnedOffBySystem { exe }
    } else {
        Autostart::On { exe }
    }
}

/// The string unescapes of the key file format.
fn unescape_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// Split an `Exec` value into its arguments, as the specification reads it:
/// the string unescapes first, then arguments separated by spaces, a quoted
/// one running to its closing quote with `\"`, `` \` ``, `\$` and `\\` taken
/// literally, and `%%` a literal `%`.
#[must_use]
pub fn parse_exec(value: &str) -> Vec<String> {
    let line = unescape_string(value);
    let mut args = Vec::new();
    let mut chars = line.chars().peekable();
    loop {
        while chars.peek() == Some(&' ') {
            chars.next();
        }
        let Some(&first) = chars.peek() else {
            break;
        };
        let mut arg = String::new();
        if first == '"' {
            chars.next();
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' => {
                        if let Some(next) = chars.next() {
                            arg.push(next);
                        }
                    }
                    _ => arg.push(c),
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c == ' ' {
                    break;
                }
                arg.push(c);
                chars.next();
            }
        }
        args.push(arg.replace("%%", "%"));
    }
    args
}

/// Read the entry in `dir`.
#[must_use]
pub fn read(dir: &Path) -> Autostart {
    match std::fs::read_to_string(dir.join(FILE_NAME)) {
        Ok(text) => entry_state(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Autostart::Off,
        Err(e) => {
            tracing::warn!("the autostart entry could not be read: {e}");
            Autostart::Off
        }
    }
}

/// Write the entry for `exe` into `dir`, whole, so a `Hidden=true` left in an
/// old one goes with it.
///
/// :returns: why it could not be written, as a sentence for the user
pub fn enable(dir: &Path, exe: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| {
        format!(
            "The autostart folder {} could not be created: {e}",
            dir.display()
        )
    })?;
    let path = dir.join(FILE_NAME);
    let partial = dir.join(format!("{FILE_NAME}~"));
    std::fs::write(&partial, entry(exe))
        .and_then(|()| std::fs::rename(&partial, &path))
        .map_err(|e| {
            let _ = std::fs::remove_file(&partial);
            format!(
                "The autostart entry {} could not be written: {e}",
                path.display()
            )
        })
}

/// Remove the entry from `dir`.
///
/// :returns: why it could not be removed, as a sentence for the user
pub fn disable(dir: &Path) -> Result<(), String> {
    let path = dir.join(FILE_NAME);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!(
            "The autostart entry {} could not be removed: {e}",
            path.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_arguments_survive_the_specifications_reading() {
        for exe in [
            "/usr/bin/sunlit-earth",
            "/opt/Sunlit Earth/sunlit-earth",
            "/home/me/$HOME/sunlit-earth",
            "/home/me/`id`/sunlit-earth",
            "/home/me/\"quoted\"/sunlit-earth",
            "/home/me/back\\slash/sunlit-earth",
            "/home/me/100%/sunlit-earth",
            "/home/me/%f %u/sunlit earth",
        ] {
            let value = exec_value(Path::new(exe));
            let mut expected = vec![exe.to_owned()];
            expected.extend(ARGS.iter().map(|arg| (*arg).to_owned()));
            assert_eq!(parse_exec(&value), expected, "Exec={value}");
            let lone_percent = value.replace("%%", "");
            assert!(
                !lone_percent.contains('%'),
                "a % that is not doubled is a field code: Exec={value}"
            );
        }
    }

    #[test]
    fn a_plain_path_is_not_quoted() {
        assert_eq!(
            exec_value(Path::new("/usr/bin/sunlit-earth")),
            "/usr/bin/sunlit-earth --tray-start hidden"
        );
    }

    #[test]
    fn the_parser_reads_the_specifications_examples() {
        assert_eq!(parse_exec(r#""/a b/c" d"#), ["/a b/c", "d"]);
        assert_eq!(
            parse_exec(r#""a \\\\ b \\$ c \\` d \\" e""#),
            [r#"a \ b $ c ` d " e"#],
            "a literal backslash is four in the file"
        );
        assert_eq!(
            parse_exec("a\\sb c"),
            ["a", "b", "c"],
            "an escaped space is still a space between arguments"
        );
        assert_eq!(parse_exec("x 100%%"), ["x", "100%"]);
    }

    #[test]
    fn the_entry_written_reads_back_as_on_for_its_executable() {
        let exe = Path::new("/opt/Sunlit Earth/sunlit-earth");
        assert_eq!(
            entry_state(&entry(exe)),
            Autostart::On {
                exe: exe.to_path_buf()
            }
        );
    }

    #[test]
    fn the_desktops_switch_offs_read_as_off() {
        let base = entry(Path::new("/usr/bin/sunlit-earth"));
        let exe = PathBuf::from("/usr/bin/sunlit-earth");
        let on = Autostart::On { exe: exe.clone() };
        let off = Autostart::TurnedOffBySystem { exe };
        assert_eq!(entry_state(&format!("{base}Hidden=true\n")), off);
        assert_eq!(entry_state(&format!("{base}Hidden=false\n")), on);
        assert_eq!(
            entry_state(&base.replace(
                "X-GNOME-Autostart-enabled=true",
                "X-GNOME-Autostart-enabled=false"
            )),
            off
        );
        assert_eq!(
            entry_state(&format!(
                "{}Hidden=true\n",
                base.replace(
                    "X-GNOME-Autostart-enabled=true",
                    "X-GNOME-Autostart-enabled=false"
                )
            )),
            off
        );
    }

    #[test]
    fn an_entry_another_tool_wrote_is_read_from_its_own_group() {
        let text = "# written by a settings tool\n\
                    [Desktop Entry]\n\
                    Name=Sunlit Earth\n\
                    Name[de]=Sonnige Erde\n\
                    Exec=\"/opt/Other Copy/sunlit-earth\" --tray-start hidden\n\
                    X-KDE-AutostartScript=false\n\
                    \n\
                    [Desktop Action Hide]\n\
                    Hidden=true\n";
        assert_eq!(
            entry_state(text),
            Autostart::On {
                exe: PathBuf::from("/opt/Other Copy/sunlit-earth")
            }
        );
    }

    #[test]
    fn a_missing_file_is_off_and_the_switch_round_trips_in_a_folder() {
        let dir = crate::test_support::ScratchDir::new("autostart_linux");
        let autostart = dir.join("autostart");
        assert_eq!(read(&autostart), Autostart::Off);

        let exe = Path::new("/opt/sunlit earth/sunlit-earth");
        enable(&autostart, exe).expect("the entry is written");
        assert_eq!(
            read(&autostart),
            Autostart::On {
                exe: exe.to_path_buf()
            }
        );

        std::fs::write(
            autostart.join(FILE_NAME),
            format!("{}Hidden=true\n", entry(exe)),
        )
        .expect("the desktop's switch-off");
        enable(&autostart, exe).expect("the entry is written again");
        assert!(read(&autostart).is_on(), "a whole write drops Hidden=true");

        disable(&autostart).expect("the entry is removed");
        assert_eq!(read(&autostart), Autostart::Off);
        disable(&autostart).expect("removing what is not there is not an error");
    }

    #[test]
    fn the_folder_follows_xdg_config_home_when_it_is_usable() {
        let home = Path::new("/home/me");
        let config = std::env::temp_dir().join("me-config");
        assert_eq!(
            autostart_dir(config.to_str(), Some(home)),
            Some(config.join("autostart"))
        );
        for unset in [None, Some(""), Some("   "), Some("relative/config")] {
            assert_eq!(
                autostart_dir(unset, Some(home)),
                Some(PathBuf::from("/home/me/.config/autostart")),
                "{unset:?}"
            );
        }
        assert_eq!(autostart_dir(None, None), None);
    }

    #[test]
    fn the_sessions_that_start_nothing_from_the_folder_are_recognized() {
        let exe = Path::new("/usr/bin/sunlit-earth");
        for (desktop, line) in [
            ("sway", "`exec /usr/bin/sunlit-earth --tray-start hidden`"),
            ("SWAY", "`exec /usr/bin/sunlit-earth --tray-start hidden`"),
            (
                "wlroots:sway",
                "`exec /usr/bin/sunlit-earth --tray-start hidden`",
            ),
            ("i3", "`exec /usr/bin/sunlit-earth --tray-start hidden`"),
            (
                "Hyprland",
                "`exec-once = /usr/bin/sunlit-earth --tray-start hidden`",
            ),
            (
                "hyprland",
                "`exec-once = /usr/bin/sunlit-earth --tray-start hidden`",
            ),
        ] {
            let reason = unsupported_session(Some(desktop), Some(exe))
                .unwrap_or_else(|| panic!("{desktop} starts nothing from the folder"));
            assert!(reason.contains(line), "{desktop}: {reason}");
        }
        for desktop in [Some("ubuntu:GNOME"), Some("KDE"), Some(""), None] {
            assert_eq!(unsupported_session(desktop, Some(exe)), None, "{desktop:?}");
        }
    }

    #[test]
    fn a_path_outside_the_cellar_has_no_homebrew_alias() {
        assert_eq!(homebrew_alias(Path::new("/usr/bin/sunlit-earth")), None);
        assert_eq!(
            homebrew_alias(Path::new("/opt/Cellar/other/1.0/bin/other")),
            None
        );
        assert_eq!(
            homebrew_alias(Path::new(
                "/home/linuxbrew/.linuxbrew/Cellar/sunlit-earth/0.5.0/libexec/sunlit-earth"
            )),
            Some(PathBuf::from(
                "/home/linuxbrew/.linuxbrew/opt/sunlit-earth/bin/sunlit-earth"
            ))
        );
        let plain = Path::new("/usr/bin/sunlit-earth");
        assert_eq!(stable_path(plain), plain);
    }

    /// A fabricated prefix with a `Cellar` and an `opt` in a temporary folder.
    struct Prefix {
        dir: crate::test_support::ScratchDir,
        exe: PathBuf,
        alias: PathBuf,
    }

    impl Prefix {
        fn new(case: &str) -> Self {
            let dir = crate::test_support::ScratchDir::new(case);
            let root = dir.path();
            let keg = root.join("Cellar").join("sunlit-earth").join("0.5.0");
            std::fs::create_dir_all(keg.join("libexec")).expect("keg");
            let exe = keg.join("libexec").join("sunlit-earth");
            std::fs::write(&exe, b"binary").expect("exe");
            let alias = root
                .join("opt")
                .join("sunlit-earth")
                .join("bin")
                .join("sunlit-earth");
            Self { dir, exe, alias }
        }
    }

    #[test]
    fn the_cellar_path_stays_when_the_alias_is_missing_or_another_file() {
        let prefix = Prefix::new("homebrew_other");
        assert_eq!(stable_path(&prefix.exe), prefix.exe, "no opt at all");

        std::fs::create_dir_all(prefix.alias.parent().expect("bin")).expect("opt");
        std::fs::write(&prefix.alias, b"another binary").expect("another file");
        assert_eq!(stable_path(&prefix.exe), prefix.exe, "opt is not this file");
    }

    #[cfg(unix)]
    #[test]
    fn the_alias_is_used_when_it_resolves_to_the_same_file() {
        use std::os::unix::fs::symlink;

        let prefix = Prefix::new("homebrew_same");
        let keg = prefix.exe.parent().and_then(Path::parent).expect("keg");
        std::fs::create_dir_all(keg.join("bin")).expect("bin");
        symlink(
            "../libexec/sunlit-earth",
            keg.join("bin").join("sunlit-earth"),
        )
        .expect("bin link");
        std::fs::create_dir_all(prefix.dir.join("opt")).expect("opt");
        symlink(keg, prefix.dir.join("opt").join("sunlit-earth")).expect("opt link");

        assert_eq!(stable_path(&prefix.exe), prefix.alias);
    }
}
