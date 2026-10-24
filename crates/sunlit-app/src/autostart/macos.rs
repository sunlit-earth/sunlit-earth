//! macOS: a launch agent, `~/Library/LaunchAgents/earth.sunlit.SunlitEarth.plist`.
//!
//! The bundle is started through `/usr/bin/open`, so `LaunchServices` launches
//! it the way a double-click does; the bare terminal binary has no bundle and
//! is started directly. The agent takes effect at the next login, and the app
//! never calls `launchctl`: `bootstrap` would start a second copy at once, and
//! `bootout` of a directly started agent would end the running app. A
//! switch-off in System Settings' Login Items is not read.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use super::{ARGS, Autostart};

/// The agent's label, the bundle identifier.
pub const LABEL: &str = "earth.sunlit.SunlitEarth";

/// The agent's file name.
pub const FILE_NAME: &str = "earth.sunlit.SunlitEarth.plist";

/// The program that starts the bundle through `LaunchServices`.
const OPEN: &str = "/usr/bin/open";

/// The launch agents folder of this user.
#[must_use]
pub fn system_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(|home| PathBuf::from(home).join("Library").join("LaunchAgents"))
}

/// The `.app` an executable sits in, three levels up from
/// `<name>.app/Contents/MacOS/<name>`, or `None` for a bare binary.
#[must_use]
pub fn bundle_path(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    let is_bundle = macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("app"));
    is_bundle.then(|| bundle.to_path_buf())
}

/// What an agent written by this executable starts: the bundle, or the bare
/// binary.
#[must_use]
pub fn launched_path(exe: &Path) -> PathBuf {
    bundle_path(exe).unwrap_or_else(|| exe.to_path_buf())
}

/// The agent's `ProgramArguments` for `exe`.
///
/// :returns: the arguments, or why no agent can be written for this copy
pub fn program_arguments(exe: &Path) -> Result<Vec<String>, String> {
    if exe.to_string_lossy().contains("/AppTranslocation/") {
        return Err("Move Sunlit Earth to the Applications folder first.".to_owned());
    }
    let args = ARGS.iter().map(|arg| (*arg).to_owned());
    Ok(match bundle_path(exe) {
        Some(bundle) => [
            OPEN.to_owned(),
            "-a".to_owned(),
            bundle.to_string_lossy().into_owned(),
            "--args".to_owned(),
        ]
        .into_iter()
        .chain(args)
        .collect(),
        None => std::iter::once(exe.to_string_lossy().into_owned())
            .chain(args)
            .collect(),
    })
}

fn escape_xml(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(c),
        }
    }
    escaped
}

fn unescape_xml(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The agent's plist for these arguments.
#[must_use]
pub fn plist(arguments: &[String]) -> String {
    let mut text = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n",
    );
    let _ = writeln!(text, "\t<key>Label</key>\n\t<string>{LABEL}</string>");
    text.push_str("\t<key>ProgramArguments</key>\n\t<array>\n");
    for argument in arguments {
        let _ = writeln!(text, "\t\t<string>{}</string>", escape_xml(argument));
    }
    text.push_str("\t</array>\n");
    text.push_str("\t<key>RunAtLoad</key>\n\t<true/>\n");
    text.push_str("\t<key>LimitLoadToSessionType</key>\n\t<string>Aqua</string>\n");
    text.push_str("\t<key>ProcessType</key>\n\t<string>Interactive</string>\n");
    let _ = writeln!(
        text,
        "\t<key>AssociatedBundleIdentifiers</key>\n\t<array>\n\t\t<string>{LABEL}</string>\n\t</array>"
    );
    text.push_str("</dict>\n</plist>\n");
    text
}

/// The `ProgramArguments` strings of a plist, unescaped.
#[must_use]
pub fn parse_program_arguments(text: &str) -> Vec<String> {
    let Some(key) = text.find("<key>ProgramArguments</key>") else {
        return Vec::new();
    };
    let rest = &text[key..];
    let (Some(start), Some(end)) = (rest.find("<array>"), rest.find("</array>")) else {
        return Vec::new();
    };
    if end < start {
        return Vec::new();
    }
    let mut arguments = Vec::new();
    let mut array = &rest[start..end];
    while let Some(open) = array.find("<string>") {
        let after = &array[open + "<string>".len()..];
        let Some(close) = after.find("</string>") else {
            break;
        };
        arguments.push(unescape_xml(&after[..close]));
        array = &after[close..];
    }
    arguments
}

/// The state an agent's plist comes to: on, for the bundle `open` starts or
/// for the program started directly.
#[must_use]
pub fn plist_state(text: &str) -> Autostart {
    let arguments = parse_program_arguments(text);
    let exe = match arguments.as_slice() {
        [open, flag, bundle, ..] if open == OPEN && flag == "-a" => PathBuf::from(bundle),
        [program, ..] => PathBuf::from(program),
        [] => PathBuf::new(),
    };
    Autostart::On { exe }
}

/// Read the agent in `dir`.
#[must_use]
pub fn read(dir: &Path) -> Autostart {
    match std::fs::read_to_string(dir.join(FILE_NAME)) {
        Ok(text) => plist_state(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Autostart::Off,
        Err(e) => {
            tracing::warn!("the launch agent could not be read: {e}");
            Autostart::Off
        }
    }
}

/// Write the agent for `exe` into `dir`.
///
/// :returns: why it could not be written, as a sentence for the user
pub fn enable(dir: &Path, exe: &Path) -> Result<(), String> {
    let text = plist(&program_arguments(exe)?);
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("The folder {} could not be created: {e}", dir.display()))?;
    let path = dir.join(FILE_NAME);
    let partial = dir.join(format!("{FILE_NAME}~"));
    std::fs::write(&partial, text)
        .and_then(|()| std::fs::rename(&partial, &path))
        .map_err(|e| {
            let _ = std::fs::remove_file(&partial);
            format!(
                "The launch agent {} could not be written: {e}",
                path.display()
            )
        })
}

/// Remove the agent from `dir`.
///
/// :returns: why it could not be removed, as a sentence for the user
pub fn disable(dir: &Path) -> Result<(), String> {
    let path = dir.join(FILE_NAME);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!(
            "The launch agent {} could not be removed: {e}",
            path.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUNDLE_EXE: &str = "/Applications/Sunlit Earth.app/Contents/MacOS/sunlit-earth";

    #[test]
    fn a_bundle_is_started_through_open() {
        let arguments = program_arguments(Path::new(BUNDLE_EXE)).expect("an installed bundle");
        assert_eq!(
            arguments,
            [
                "/usr/bin/open",
                "-a",
                "/Applications/Sunlit Earth.app",
                "--args",
                "--tray-start",
                "hidden"
            ]
        );
        assert_eq!(
            plist_state(&plist(&arguments)),
            Autostart::On {
                exe: PathBuf::from("/Applications/Sunlit Earth.app")
            }
        );
        assert_eq!(
            launched_path(Path::new(BUNDLE_EXE)),
            PathBuf::from("/Applications/Sunlit Earth.app"),
            "what this copy writes is what the read compares against"
        );
    }

    #[test]
    fn a_bare_binary_is_started_directly() {
        let exe = Path::new("/opt/homebrew/bin/sunlit-earth");
        let arguments = program_arguments(exe).expect("a bare binary");
        assert_eq!(arguments, [exe.to_str().unwrap(), "--tray-start", "hidden"]);
        assert_eq!(
            plist_state(&plist(&arguments)),
            Autostart::On {
                exe: exe.to_path_buf()
            }
        );
        assert_eq!(bundle_path(exe), None);
    }

    #[test]
    fn a_translocated_bundle_is_refused() {
        let exe = Path::new(
            "/private/var/folders/xy/T/AppTranslocation/0A1B/d/Sunlit Earth.app/Contents/MacOS/sunlit-earth",
        );
        let refusal = program_arguments(exe).expect_err("a path that changes per launch");
        assert!(refusal.contains("Applications folder"), "{refusal}");
    }

    #[test]
    fn markup_in_a_path_is_escaped_and_reads_back() {
        let exe = Path::new(
            "/Users/me/Apps & <Tools> \"x\"/Sunlit Earth.app/Contents/MacOS/sunlit-earth",
        );
        let text = plist(&program_arguments(exe).expect("a bundle"));
        assert!(
            text.contains("/Users/me/Apps &amp; &lt;Tools&gt; &quot;x&quot;/Sunlit Earth.app"),
            "{text}"
        );
        assert!(!text.contains("& <"), "{text}");
        assert_eq!(
            plist_state(&text),
            Autostart::On {
                exe: PathBuf::from("/Users/me/Apps & <Tools> \"x\"/Sunlit Earth.app")
            }
        );
    }

    #[test]
    fn the_plist_carries_what_launchd_needs() {
        let text = plist(&program_arguments(Path::new(BUNDLE_EXE)).expect("a bundle"));
        for needed in [
            "<key>Label</key>\n\t<string>earth.sunlit.SunlitEarth</string>",
            "<key>RunAtLoad</key>\n\t<true/>",
            "<key>LimitLoadToSessionType</key>\n\t<string>Aqua</string>",
            "<key>ProcessType</key>\n\t<string>Interactive</string>",
            "<key>AssociatedBundleIdentifiers</key>",
        ] {
            assert!(text.contains(needed), "{needed} missing:\n{text}");
        }
    }

    #[test]
    fn the_agent_round_trips_in_a_folder() {
        let dir = crate::test_support::ScratchDir::new("autostart_macos");
        let agents = dir.join("LaunchAgents");
        assert_eq!(read(&agents), Autostart::Off);
        enable(&agents, Path::new(BUNDLE_EXE)).expect("the agent is written");
        assert_eq!(
            read(&agents),
            Autostart::On {
                exe: PathBuf::from("/Applications/Sunlit Earth.app")
            }
        );
        disable(&agents).expect("the agent is removed");
        assert_eq!(read(&agents), Autostart::Off);
        disable(&agents).expect("removing what is not there is not an error");
    }
}
