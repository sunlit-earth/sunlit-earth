//! The XFCE arm: xfconf properties, and the ones xfdesktop has not created yet.
//!
//! The only row that asks the session a question before it sets anything, and
//! the only one that addresses a monitor by its own name. The rest of the
//! table is in the parent module.

use super::{Invocation, Placement};

/// The XFCE backdrop property that holds an image path.
///
/// `last-image` is the one xfdesktop reads. The listing also contains
/// `image-style`, `color-style` and per-workspace colours, so the suffix is what
/// picks the right ones out.
pub(super) const XFCE_IMAGE_PROPERTY: &str = "last-image";

/// The one beside it that says how to fit the image, and the value that fills.
///
/// xfdesktop's enum, in which 0 is None and shows no image at all.
const XFCE_STYLE_PROPERTY: &str = "image-style";
const XFCE_ZOOMED: &str = "5";

/// The channel those properties live in.
pub(super) const XFCE_CHANNEL: &str = "xfce4-desktop";

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
/// therefore a set that reports success and changes nothing.
fn xfce_live_property(monitor: &str, suffix: &str) -> String {
    format!("/backdrop/screen0/monitor{monitor}/{XFCE_WORKSPACE}/{suffix}")
}

/// Everything to run to make `placement` XFCE's wallpaper, given the listing
/// `discovery` asked for.
pub(super) fn commands(placement: &Placement, discovered: &str) -> Vec<Invocation> {
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
                xfce_monitor_of(property)
                    .is_none_or(|monitor| !placement.untouched.iter().any(|name| name == monitor))
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::desktop::detect;
    use crate::desktop::tests::{commands, commands_on, two_screens};

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
        // Written out rather than read from XFCE_ZOOMED: the enumerant is
        // xfdesktop's, not ours, so this literal is the only thing in the tree
        // recording what the outside world expects. 3 is stretched, 4 is
        // scaled, 5 is zoomed, and only 5 fills without letterboxing.
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

    /// A listing with nowhere to put the image is a refusal, whatever else is
    /// in it. Setting a fill mode on such a desktop, or running nothing at all,
    /// would report a wallpaper nothing is showing.
    #[test]
    fn an_xfce_listing_with_no_image_property_is_a_refusal_not_a_success() {
        let backend = detect("XFCE").expect("a backend");
        let placement = Placement::single(PathBuf::from("/w.png"));
        for listing in [
            "/backdrop/screen0/monitor0/workspace0/image-style\n",
            "/backdrop/single-workspace-mode\n",
            "",
        ] {
            assert!(
                backend.commands(&placement, listing).is_empty(),
                "{listing:?}"
            );
        }
        let message = backend.nothing_to_run();
        assert!(message.contains("XFCE"), "{message}");
        assert!(message.contains("last-image"), "{message}");
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
}
