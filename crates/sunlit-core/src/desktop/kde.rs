//! The KDE Plasma arm: one `dbus-send` call carrying a script for plasmashell.
//!
//! Plasma has no settings key a command line can write, so the wallpaper is
//! set by evaluating JavaScript inside the shell. The rest of the table is in
//! the parent module.

use std::path::Path;

use super::{Invocation, Placement, file_uri};

/// The one call that puts this publish on Plasma's screens.
pub(super) fn plasma_command(placement: &Placement) -> Invocation {
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
/// `wallpaperPlugin` is written every time because a screen left on a colour or
/// a slideshow would otherwise take the image into a plugin that does not read
/// it.
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::desktop::detect;
    use crate::desktop::tests::{commands, two_screens};

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
}
