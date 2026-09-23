//! The commands of the rows written from documentation: sway, Hyprland, LXDE,
//! Deepin and Trinity. The rest of the table is in the parent module.

use super::{DEEPIN_BUS_NAMES, Invocation, Placement};

/// `pcmanfm`'s fill mode that zooms the image until it covers the screen.
pub(super) const LXDE_FILL_MODE: &str = "crop";

/// `KBackgroundSettings::ScaleAndCrop`, Trinity's zoom-to-cover mode.
pub(super) const TRINITY_FILL_MODE: &str = "8";

/// A path as one argument of a sway command.
///
/// `swaymsg` joins its arguments with spaces and sway parses the result again,
/// so a path with a space in it is two arguments unless it is quoted.
pub(super) fn sway_quoted(path: &str) -> String {
    format!("\"{}\"", path.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Load the image into hyprpaper, show it on every monitor, and have it drop
/// the images nothing shows any more.
pub(super) fn hyprpaper_commands(path: &str) -> Vec<Invocation> {
    let call = |args: &[&str]| {
        Invocation::new(
            "hyprctl",
            ["hyprpaper"]
                .iter()
                .chain(args)
                .map(|arg| (*arg).to_owned()),
        )
    };
    vec![
        call(&["preload", path]),
        call(&["wallpaper", &format!(",{path}")]),
        call(&["unload", "unused"]),
    ]
}

/// One `SetMonitorBackground` per monitor this publish painted.
pub(super) fn deepin_commands(placement: &Placement, legacy: bool) -> Vec<Invocation> {
    let name = DEEPIN_BUS_NAMES[usize::from(legacy)];
    let path = format!("/{}", name.replace('.', "/"));
    placement
        .per_monitor
        .iter()
        .map(|(monitor, image)| {
            Invocation::new(
                "dbus-send",
                [
                    "--session".to_owned(),
                    "--print-reply".to_owned(),
                    format!("--dest={name}"),
                    "--type=method_call".to_owned(),
                    path.clone(),
                    format!("{name}.SetMonitorBackground"),
                    format!("string:{monitor}"),
                    format!("string:{}", image.to_string_lossy()),
                ],
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::desktop::tests::{commands, two_screens};
    use crate::desktop::{Placement, detect};

    fn args(cmd: &Invocation) -> Vec<&str> {
        cmd.args.iter().map(String::as_str).collect()
    }

    #[test]
    fn sway_sets_every_output_through_its_own_background_command() {
        let cmds = commands("sway", "/home/a b/w.png", "");
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].program, "swaymsg");
        assert_eq!(
            args(&cmds[0]),
            ["output", "*", "bg", "\"/home/a b/w.png\"", "fill"]
        );
    }

    #[test]
    fn hyprpaper_loads_shows_and_then_lets_go_of_what_nothing_shows() {
        let cmds = commands("Hyprland", "/w.png", "");
        let calls: Vec<Vec<&str>> = cmds.iter().map(args).collect();
        assert_eq!(
            calls,
            [
                vec!["hyprpaper", "preload", "/w.png"],
                vec!["hyprpaper", "wallpaper", ",/w.png"],
                vec!["hyprpaper", "unload", "unused"],
            ]
        );
    }

    #[test]
    fn lxde_sets_the_image_zoomed_to_cover_the_screen() {
        let cmds = commands("LXDE", "/w.png", "");
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].program, "pcmanfm");
        assert_eq!(
            args(&cmds[0]),
            ["--set-wallpaper=/w.png", "--wallpaper-mode=crop"]
        );
    }

    #[test]
    fn trinity_asks_kdesktop_to_scale_and_crop() {
        let cmds = commands("TDE", "/w.png", "");
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].program, "dcop");
        assert_eq!(
            args(&cmds[0]),
            [
                "kdesktop",
                "KBackgroundIface",
                "setWallpaper",
                "/w.png",
                "8"
            ]
        );
    }

    #[test]
    fn deepin_gives_each_monitor_its_own_picture_by_name() {
        let backend = detect("Deepin").expect("a backend");
        let cmds = backend.commands(&two_screens(), "");
        assert_eq!(cmds.len(), 2, "{cmds:?}");
        for (cmd, (monitor, image)) in cmds
            .iter()
            .zip([("Virtual-1", "/w-0.png"), ("Virtual-2", "/w-1.png")])
        {
            let args = args(cmd);
            assert!(
                args.contains(&"--dest=org.deepin.dde.Appearance1"),
                "{args:?}"
            );
            assert!(args.contains(&"/org/deepin/dde/Appearance1"), "{args:?}");
            assert!(
                args.contains(&"org.deepin.dde.Appearance1.SetMonitorBackground"),
                "{args:?}"
            );
            assert_eq!(
                args[args.len() - 2..],
                [
                    format!("string:{monitor}").as_str(),
                    format!("string:{image}").as_str()
                ]
            );
        }

        let legacy = crate::desktop::DEEPIN_LEGACY.commands(&two_screens(), "");
        assert!(
            args(&legacy[0]).contains(&"com.deepin.daemon.Appearance.SetMonitorBackground"),
            "{legacy:?}"
        );

        let alone = Placement {
            per_monitor: vec![("Virtual-1".to_owned(), PathBuf::from("/w-0.png"))],
            untouched: vec!["Virtual-2".to_owned()],
            by_position: vec![Some(PathBuf::from("/w-0.png")), None],
            single: PathBuf::from("/w-0.png"),
            spanned: false,
        };
        assert_eq!(backend.commands(&alone, "").len(), 1);
    }
}
