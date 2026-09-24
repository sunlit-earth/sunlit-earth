//! Asking a Linux desktop what it did with the wallpaper, and moving its
//! screens about.
//!
//! Every one of these shells out to the session: the desktop's own settings
//! store for what was published, and `xrandr` for a layout change.
//!
//! The module compiles everywhere and four of its items are `cfg`-gated, which
//! is the shape the suite already has: a case is `#[ignore]`d rather than
//! `cfg`-gated, so it compiles on all three platforms and says at run time why
//! it is skipping. The four that read a wallpaper back need a desktop that has
//! one and are the exception.

use std::process::Command;

#[cfg(target_os = "linux")]
use crate::common::process::skip_case;

/// The placement the app would have made for this session, rebuilt from what it
/// actually wrote: the file per monitor where this desktop takes one, and the
/// first file otherwise.
#[cfg(target_os = "linux")]
fn placement_of(
    backend: &sunlit_core::desktop::Backend,
    published: &[std::path::PathBuf],
    monitors: &[String],
) -> sunlit_core::desktop::Placement {
    if backend.reach() != sunlit_core::desktop::Reach::PerMonitor {
        return sunlit_core::desktop::Placement::single(published[0].clone());
    }
    sunlit_core::desktop::Placement {
        per_monitor: monitors
            .iter()
            .cloned()
            .zip(published.iter().cloned())
            .collect(),
        untouched: Vec::new(),
        by_position: published.iter().cloned().map(Some).collect(),
        single: published[0].clone(),
        spanned: false,
    }
}

/// Ask this desktop what its wallpaper is, and check the answer is ours.
///
/// Answers with the values the desktop was found to be holding, whole, so that
/// a caller which publishes twice can require the second answer to differ from
/// the first. Not the file names: every publish writes its own directory with
/// the same names in it, so two publishes the desktop told apart share them.
/// `None` where this desktop's setter has no store to ask, which is Plasma's
/// tool and `LXQt`'s file manager.
///
/// The read-back is derived from the writes the sink performs rather than
/// written out again, so a row that sets the wrong key reads the wrong key back
/// and fails here instead of agreeing with itself. A desktop whose settings are
/// named after its own monitors has to hold the image in one of those and
/// nothing else counts, which is what the last assertion is for.
#[cfg(target_os = "linux")]
pub(crate) fn assert_the_desktop_holds_the_wallpaper(
    published: &[std::path::PathBuf],
) -> Option<String> {
    use sunlit_core::desktop::Mechanism;

    let backend = sunlit_core::desktop::detect_current()
        .expect("a session with no backend could not have got this far");
    match backend.mechanism() {
        Mechanism::RootPixmap => Some(the_root_holds_a_new_pixmap()),
        Mechanism::Swaybg => Some(one_swaybg_of_ours_shows(published)),
        Mechanism::Portal => Some(the_portal_wrote_gnomes_key(published)),
        Mechanism::Commands => the_commands_writes_hold(&backend, published),
    }
}

/// The read-back for a setter that runs commands: each write is asked back of
/// the store it went to.
#[cfg(target_os = "linux")]
fn the_commands_writes_hold(
    backend: &sunlit_core::desktop::Backend,
    published: &[std::path::PathBuf],
) -> Option<String> {
    assert!(
        !published.is_empty(),
        "the setter said it set a wallpaper, so one was written"
    );
    // The file names rather than the whole paths, because what a write carries
    // is the path in that desktop's own spelling: a `file://` URI for the
    // gsettings rows and a plain path for the rest.
    let names: Vec<String> = published
        .iter()
        .map(|image| {
            image
                .file_name()
                .expect("the wallpaper is a file")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let name = names.join(", ");

    let discovered = match backend.discovery() {
        Some(query) => read_setting(&query),
        None => String::new(),
    };
    let monitors: Vec<String> = sunlit_core::display::monitors()
        .unwrap_or_default()
        .into_iter()
        .map(|monitor| monitor.id)
        .collect();

    let placement = placement_of(backend, published, &monitors);

    let mut holders: Vec<String> = Vec::new();
    let mut held: Vec<String> = Vec::new();
    for command in backend.commands(&placement, &discovered) {
        // The fill-mode writes carry a mode rather than a path, and the mode is
        // not what this is about.
        let Some(written) = command
            .args
            .iter()
            .find(|arg| names.iter().any(|name| arg.contains(name)))
        else {
            continue;
        };
        let written = written.clone();
        if command.program == "swaymsg" {
            holders.push("sway's swaybg".to_owned());
            held.push(sway_shows(&written));
            continue;
        }
        let Some(query) = readback_of(&command) else {
            skip_case(
                "test_set_wallpaper's read-back",
                &format!(
                    "{}'s setter is `{}`, which sets a wallpaper and has nothing \
                     to ask what one is",
                    backend.desktop, command.program
                ),
            );
            return None;
        };
        let value = read_setting(&query);
        assert!(
            value.contains(&written),
            "{} reports its wallpaper as {value:?} after the setter said it set \
             {written}: `{} {}`",
            backend.desktop,
            query.program,
            query.args.join(" ")
        );
        holders.push(query.args.join(" "));
        held.push(value);
    }
    assert!(
        !holders.is_empty(),
        "{} set a wallpaper without writing the image anywhere",
        backend.desktop
    );

    // Expressed as what makes such a desktop different rather than by name:
    // asking the session which properties it has is the same thing as those
    // properties being named after this session's own monitors.
    if backend.discovery().is_some() {
        assert!(
            !monitors.is_empty(),
            "{} names its settings after the monitors, and xrandr named none",
            backend.desktop
        );
        assert!(
            monitors.iter().any(|monitor| holders
                .iter()
                .any(|holder| holder.contains(&format!("monitor{monitor}")))),
            "{} holds the wallpaper in {holders:?}, none of which is named after \
             a connected monitor ({monitors:?}), which is the only kind its \
             desktop reads",
            backend.desktop
        );
    }

    println!(
        "{} reports the app's {name} as its wallpaper, read back from {} \
         setting(s): {holders:?}",
        backend.desktop,
        holders.len()
    );
    Some(held.join(", "))
}

/// What sway holds after `output * bg`: sway runs a swaybg of its own for it,
/// so the answer is that process's command line naming the file.
#[cfg(target_os = "linux")]
fn sway_shows(written: &str) -> String {
    let file = written.trim_matches('"').to_owned();
    let shown = wait_for_swaybgs(|lines| lines.iter().any(|line| line.contains(&file)));
    assert!(
        shown.iter().any(|line| line.contains(&file)),
        "sway was told to show {file} and no swaybg shows it: {shown:?}"
    );
    file
}

/// The command lines of this user's `swaybg` processes, polled for up to ten
/// seconds until `settled` holds, since the owned one's predecessor is ended a
/// second after the publish and sway starts its own asynchronously.
#[cfg(target_os = "linux")]
fn wait_for_swaybgs(settled: impl Fn(&[Vec<String>]) -> bool) -> Vec<Vec<String>> {
    use std::os::unix::fs::MetadataExt;

    let me = std::fs::metadata("/proc/self").map(|m| m.uid()).ok();
    let read = || -> Vec<Vec<String>> {
        std::fs::read_dir("/proc")
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|entry| entry.metadata().ok().map(|m| m.uid()) == me)
            .filter(|entry| {
                std::fs::read_to_string(entry.path().join("comm"))
                    .is_ok_and(|comm| comm.trim_end() == "swaybg")
            })
            .filter_map(|entry| std::fs::read(entry.path().join("cmdline")).ok())
            .map(|raw| {
                raw.split(|&b| b == 0)
                    .filter(|arg| !arg.is_empty())
                    .map(|arg| String::from_utf8_lossy(arg).into_owned())
                    .collect()
            })
            .collect()
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let lines = read();
        if settled(&lines) || std::time::Instant::now() >= deadline {
            return lines;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// Under the owned `swaybg`: exactly one `swaybg` names a file under the
/// wallpaper directory, and it is the file this publish wrote. More than one is
/// the leak the handover exists to prevent.
#[cfg(target_os = "linux")]
fn one_swaybg_of_ours_shows(published: &[std::path::PathBuf]) -> String {
    let newest = published
        .first()
        .expect("the swaybg setter writes the image it starts swaybg with")
        .to_string_lossy()
        .into_owned();
    let dir = sunlit_core::wallpaper::wallpaper_dir().expect("a wallpaper directory");
    let ours = |lines: &[Vec<String>]| -> Vec<Vec<String>> {
        lines
            .iter()
            .filter(|line| sunlit_core::desktop::names_a_file_under(line, &dir))
            .cloned()
            .collect()
    };
    let lines = wait_for_swaybgs(|lines| {
        let ours = ours(lines);
        ours.len() == 1 && ours[0].contains(&newest)
    });
    let ours = ours(&lines);
    assert_eq!(
        ours.len(),
        1,
        "exactly one swaybg of ours should be running after the handover: {ours:?}"
    );
    assert!(
        ours[0].contains(&newest),
        "the swaybg left running shows {:?}, not the newest image {newest}",
        ours[0]
    );
    println!("one swaybg of ours runs, showing {newest}; all swaybgs: {lines:?}");
    newest
}

/// Under the portal on GNOME: the backend wrote GNOME's key, and the file it
/// names holds the image this publish handed over. GNOME's backend copies the
/// image to `~/.config/background` and points the key there, the same path on
/// every publish, so the name says nothing and the bytes are what is compared;
/// the answer names the file they match, which is what differs between two.
#[cfg(target_os = "linux")]
fn the_portal_wrote_gnomes_key(published: &[std::path::PathBuf]) -> String {
    let handed = published
        .first()
        .expect("the portal setter writes the image it hands over");
    let expected =
        std::fs::read(handed).unwrap_or_else(|e| panic!("cannot read {}: {e}", handed.display()));
    let query = sunlit_core::desktop::Invocation {
        program: "gsettings",
        args: vec![
            "get".to_owned(),
            "org.gnome.desktop.background".to_owned(),
            "picture-uri".to_owned(),
        ],
    };
    let holds = |value: &str| {
        value
            .strip_prefix("file://")
            .and_then(|path| std::fs::read(path).ok())
            .is_some_and(|bytes| bytes == expected)
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut value = read_setting(&query);
    while !holds(&value) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(250));
        value = read_setting(&query);
    }
    assert!(
        holds(&value),
        "the portal said it set the wallpaper and GNOME's key holds {value:?}, which is not a copy of {}",
        handed.display()
    );
    let held = format!("{value}, a copy of {}", handed.display());
    println!("the portal left {held} in GNOME's picture-uri");
    held
}

/// Under the root pixmap: `_XROOTPMAP_ID` names a live pixmap, and the one the
/// previous publish left is gone, which is the leak check. Answers with the id,
/// so a caller that publishes twice requires the two to differ.
#[cfg(target_os = "linux")]
fn the_root_holds_a_new_pixmap() -> String {
    use std::sync::Mutex;
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};

    static PREVIOUS: Mutex<Option<u32>> = Mutex::new(None);

    let (conn, screen) = x11rb::connect(None).expect("an X display to read the root of");
    let root = conn.setup().roots[screen].root;
    let atom = conn
        .intern_atom(true, b"_XROOTPMAP_ID")
        .expect("intern")
        .reply()
        .expect("intern reply")
        .atom;
    assert_ne!(atom, 0, "nothing ever set _XROOTPMAP_ID on this display");
    let id = conn
        .get_property(false, root, atom, AtomEnum::PIXMAP, 0, 1)
        .expect("get_property")
        .reply()
        .expect("get_property reply")
        .value32()
        .and_then(|mut values| values.next())
        .expect("_XROOTPMAP_ID holds a pixmap");
    assert!(
        conn.get_geometry(id).expect("get_geometry").reply().is_ok(),
        "_XROOTPMAP_ID names pixmap {id:#x}, which does not exist"
    );
    let mut previous = PREVIOUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(old) = previous.filter(|old| *old != id) {
        assert!(
            conn.get_geometry(old)
                .expect("get_geometry")
                .reply()
                .is_err(),
            "the previous root pixmap {old:#x} is still alive after the next \
             publish replaced it with {id:#x}: every publish would leak one"
        );
        println!("the root pixmap moved from {old:#x} to {id:#x} and the old one is freed");
    }
    *previous = Some(id);
    format!("root pixmap {id:#x}")
}

/// The command that reads back what one of the sink's writes set.
///
/// `None` where the desktop's setter is a one-way action: `plasma-apply-wallpaperimage`
/// and `pcmanfm-qt --set-wallpaper` both take an image and neither answers with
/// one.
#[cfg(target_os = "linux")]
fn readback_of(
    command: &sunlit_core::desktop::Invocation,
) -> Option<sunlit_core::desktop::Invocation> {
    let args: Vec<String> = match command.program {
        // `set <schema> <key> <value>` is read back by `get <schema> <key>`.
        "gsettings" => match command.args.as_slice() {
            [set, schema, key, _value] if set == "set" => {
                vec!["get".to_owned(), schema.clone(), key.clone()]
            }
            _ => return None,
        },
        // The channel and the property, which is everything before the write:
        // `-s` carries the value and `-n -t <type>` creates the property.
        "xfconf-query" => command
            .args
            .iter()
            .take_while(|arg| *arg != "-n" && *arg != "-s")
            .cloned()
            .collect(),
        _ => return None,
    };
    Some(sunlit_core::desktop::Invocation {
        program: command.program,
        args,
    })
}

/// Run one read-back and answer with its output, trimmed of the quoting
/// `gsettings` puts around a string.
#[cfg(target_os = "linux")]
fn read_setting(query: &sunlit_core::desktop::Invocation) -> String {
    let out = Command::new(query.program)
        .args(&query.args)
        .output()
        .unwrap_or_else(|e| panic!("cannot run {}: {e}", query.program));
    assert!(
        out.status.success(),
        "{} {} failed: {}",
        query.program,
        query.args.join(" "),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .trim_matches('\'')
        .to_owned()
}

/// Run one xrandr command and say whether it worked.
pub(crate) fn xrandr(args: &[String]) -> bool {
    match Command::new("xrandr").args(args).output() {
        Ok(out) if out.status.success() => true,
        Ok(out) => {
            println!(
                "xrandr {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            );
            false
        }
        Err(e) => {
            println!("xrandr {} could not be run: {e}", args.join(" "));
            false
        }
    }
}

fn words(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

/// Puts the layout back the way the boot left it, whatever the case did.
///
/// A guard rather than a line at the end, so a failing assertion cannot leave
/// the guest one-screened, or at the wrong mode, for the cases after it.
pub(crate) struct LayoutGuard {
    restore: Vec<String>,
    restored: bool,
}

impl LayoutGuard {
    pub(crate) fn restore(&mut self) -> bool {
        self.restored = true;
        xrandr(&self.restore)
    }
}

impl Drop for LayoutGuard {
    fn drop(&mut self) {
        if !self.restored {
            self.restore();
        }
    }
}

/// A change this session can make to its own layout, and how to undo it.
pub(crate) struct LayoutChange {
    pub(crate) what: String,
    pub(crate) apply: Vec<String>,
    pub(crate) guard: LayoutGuard,
    pub(crate) monitors_after: usize,
}

/// A mode this output has that is not the one it is using.
///
/// The mode list is the indented block under the output's own line in
/// `xrandr --query`, which is the half `display::parse_outputs` skips because a
/// mode nothing is displaying at is not somewhere to put a window.
fn a_different_mode(output: &str, current: (u32, u32)) -> Option<String> {
    let text = Command::new("xrandr")
        .arg("--query")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())?;
    let mut under_output = false;
    for line in text.lines() {
        if !line.starts_with(char::is_whitespace) {
            under_output = line.split_whitespace().next() == Some(output);
            continue;
        }
        if !under_output {
            continue;
        }
        let Some(mode) = line.split_whitespace().next() else {
            continue;
        };
        let Some((width, height)) = mode.split_once('x') else {
            continue;
        };
        let (Ok(width), Ok(height)) = (width.parse::<u32>(), height.parse::<u32>()) else {
            continue;
        };
        if (width, height) != current && width >= 640 && height >= 480 {
            return Some(mode.to_owned());
        }
    }
    None
}

/// How this session's layout can be made to move.
///
/// Two screens is the case the feature was written for: switch the one that is
/// not primary off, and the app should be told and should render for what is
/// left. One screen can still change its own rectangle by changing its mode,
/// which is the same `RandR` event and the same comparison in the engine, minus
/// the screen count moving. Either is a real layout change made by a real
/// display server, which is what no unit test can produce.
pub(crate) fn a_layout_change_this_session_can_make() -> Option<LayoutChange> {
    // Every change below is an xrandr command, and macOS answers
    // `display::outputs` too.
    if !cfg!(target_os = "linux") {
        return None;
    }
    let outputs = sunlit_core::display::outputs().unwrap_or_default();
    let primary = sunlit_core::display::primary_of(&outputs)?.name.clone();
    if outputs.len() >= 2 {
        let second = outputs
            .iter()
            .find(|output| output.name != primary)?
            .name
            .clone();
        return Some(LayoutChange {
            what: format!("{second} switched off"),
            apply: words(&["--output", &second, "--off"]),
            guard: LayoutGuard {
                restore: words(&["--output", &second, "--auto", "--right-of", &primary]),
                restored: false,
            },
            monitors_after: outputs.len() - 1,
        });
    }
    let only = outputs.first()?;
    let mode = a_different_mode(&only.name, (only.width, only.height))?;
    Some(LayoutChange {
        what: format!("{} at {mode}", only.name),
        apply: words(&["--output", &only.name, "--mode", &mode]),
        guard: LayoutGuard {
            restore: words(&["--output", &only.name, "--auto"]),
            restored: false,
        },
        monitors_after: 1,
    })
}

/// plasmashell's process id over the session bus, or `None` when the shell is
/// not there to answer.
///
/// `GetConnectionUnixProcessID` for `org.kde.plasmashell` on the session bus. The
/// reply is a line ending in `uint32 <pid>`, and the pid is what tells a shell
/// that kept running apart from one that crashed and was restarted under the same
/// name.
pub(crate) fn plasmashell_pid() -> Option<String> {
    let out = Command::new("dbus-send")
        .args([
            "--session",
            "--print-reply",
            "--reply-timeout=5000",
            "--dest=org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus.GetConnectionUnixProcessID",
            "string:org.kde.plasmashell",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .last()
        .filter(|pid| pid.chars().all(|c| c.is_ascii_digit()))
        .map(str::to_owned)
}

/// Whether plasmashell answers an `evaluateScript`, the same interface the KDE
/// wallpaper setter drives.
pub(crate) fn plasmashell_answers() -> bool {
    Command::new("dbus-send")
        .args([
            "--session",
            "--print-reply",
            "--reply-timeout=5000",
            "--dest=org.kde.plasmashell",
            "--type=method_call",
            "/PlasmaShell",
            "org.kde.PlasmaShell.evaluateScript",
            "string:print(1);",
        ])
        .output()
        .is_ok_and(|out| out.status.success())
}
