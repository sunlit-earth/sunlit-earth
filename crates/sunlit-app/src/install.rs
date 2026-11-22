//! How this copy of the app was installed, and what the update window tells
//! its user to do.
//!
//! Read from where the executable is, never from a record of the install,
//! since none of the four ways of installing leaves one the app could trust.
//! A wrong answer costs little: a command where a button would have fit, or
//! the other way round, and the release notes link to the archives either way.

use std::path::Path;

use crate::autostart;

/// How this copy got where it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallKind {
    Scoop,
    /// A Homebrew formula, which is how Homebrew installs it on Linux.
    HomebrewFormula,
    /// A Homebrew cask, which is how Homebrew installs it on macOS.
    HomebrewCask,
    /// Unpacked from a release archive, or built from source.
    Archive,
}

/// The two places Homebrew keeps its casks: Apple silicon, then Intel.
const CASKROOMS: [&str; 2] = [
    "/opt/homebrew/Caskroom/sunlit-earth",
    "/usr/local/Caskroom/sunlit-earth",
];

/// How the executable at `exe` on `os` was installed, with `exists` answering
/// whether a path is there.
///
/// - Scoop keeps an app at `<root>/apps/<name>/<version>/` with its
///   `install.json` and `manifest.json` beside the executable, and `current`
///   is a junction to the newest version folder. The root can be anywhere, so
///   the word `scoop` is not looked for.
/// - A formula runs from `Cellar`, which [`autostart::linux::homebrew_alias`]
///   already recognizes.
/// - A cask puts the `.app` in Applications and keeps its own record in a
///   `Caskroom` folder.
pub fn kind_of(os: &str, exe: &Path, exists: impl Fn(&Path) -> bool) -> InstallKind {
    match os {
        "windows" if is_scoop(exe, &exists) => InstallKind::Scoop,
        "linux" if autostart::linux::homebrew_alias(exe).is_some() => InstallKind::HomebrewFormula,
        "macos"
            if autostart::macos::bundle_path(exe).is_some()
                && CASKROOMS.iter().any(|root| exists(Path::new(root))) =>
        {
            InstallKind::HomebrewCask
        }
        _ => InstallKind::Archive,
    }
}

fn is_scoop(exe: &Path, exists: &impl Fn(&Path) -> bool) -> bool {
    let Some(version) = exe.parent() else {
        return false;
    };
    let named = |path: Option<&Path>, name: &str| {
        path.and_then(Path::file_name)
            .is_some_and(|found| found.eq_ignore_ascii_case(name))
    };
    let app = version.parent();
    exists(&version.join("install.json"))
        && exists(&version.join("manifest.json"))
        && named(app, "sunlit-earth")
        && named(app.and_then(Path::parent), "apps")
}

/// How the running copy was installed.
pub fn kind() -> InstallKind {
    match std::env::current_exe() {
        Ok(exe) => kind_of(std::env::consts::OS, &exe, Path::exists),
        Err(error) => {
            tracing::warn!("could not tell where the app runs from: {error}");
            InstallKind::Archive
        }
    }
}

/// Whether the running copy is inside a macOS `.app`.
pub fn bundled() -> bool {
    std::env::current_exe().is_ok_and(|exe| autostart::macos::bundle_path(&exe).is_some())
}

/// What the update window says about getting the new version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateHelp {
    /// Quit, then run `command`; `hint` is for a package manager whose own
    /// list is older than the release.
    Command {
        command: &'static str,
        hint: &'static str,
    },
    /// Download the archive, then follow `steps`.
    Archive { steps: [&'static str; 2] },
}

const COPY_OVER: &str = "Quit Sunlit Earth, unpack the download, and copy the files in its folder over those in the folder Sunlit Earth runs from.";

/// The update window's instructions for a copy installed as `kind` on `os`,
/// `bundled` saying whether a macOS copy is the `.app`.
pub fn update_help(kind: InstallKind, os: &str, bundled: bool) -> UpdateHelp {
    match kind {
        InstallKind::Scoop => UpdateHelp::Command {
            command: "scoop update sunlit-earth",
            hint: "If Scoop reports the old version as current, run \"scoop update\" first.",
        },
        InstallKind::HomebrewFormula => UpdateHelp::Command {
            command: "brew upgrade sunlit-earth",
            hint: "If Homebrew reports the old version as current, run \"brew update\" first.",
        },
        InstallKind::HomebrewCask => UpdateHelp::Command {
            command: "brew upgrade --cask sunlit-earth",
            hint: "If Homebrew reports the old version as current, run \"brew update\" first.",
        },
        InstallKind::Archive => UpdateHelp::Archive {
            steps: match os {
                "windows" => [
                    COPY_OVER,
                    "Your settings are kept. Windows may ask once more before it starts the new version.",
                ],
                "macos" if bundled => [
                    "Quit Sunlit Earth, unzip the download, and move the new Sunlit Earth.app to Applications in place of the old one.",
                    "Your settings are kept. macOS may ask once more before it opens the new version, as it did the first time.",
                ],
                _ => [COPY_OVER, "Your settings are kept."],
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn tree(paths: &[&str]) -> impl Fn(&Path) -> bool {
        let present: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        move |path| present.iter().any(|found| found == path)
    }

    const SCOOP_DIR: &str = "D:/tools/scoop/apps/sunlit-earth/current";

    fn scoop_tree() -> impl Fn(&Path) -> bool {
        tree(&[
            "D:/tools/scoop/apps/sunlit-earth/current/install.json",
            "D:/tools/scoop/apps/sunlit-earth/current/manifest.json",
        ])
    }

    #[test]
    fn a_scoop_tree_is_scoop_wherever_its_root_is() {
        let exe = Path::new(SCOOP_DIR).join("sunlit-earth.exe");
        assert_eq!(kind_of("windows", &exe, scoop_tree()), InstallKind::Scoop);
    }

    #[test]
    fn a_scoop_tree_without_its_two_files_is_an_archive() {
        let exe = Path::new(SCOOP_DIR).join("sunlit-earth.exe");
        assert_eq!(kind_of("windows", &exe, tree(&[])), InstallKind::Archive);
        let one = tree(&["D:/tools/scoop/apps/sunlit-earth/current/install.json"]);
        assert_eq!(kind_of("windows", &exe, one), InstallKind::Archive);
    }

    #[test]
    fn the_two_files_outside_scoops_layout_are_an_archive() {
        let exe = Path::new("D:/tools/sunlit-earth/current/sunlit-earth.exe");
        let present = tree(&[
            "D:/tools/sunlit-earth/current/install.json",
            "D:/tools/sunlit-earth/current/manifest.json",
        ]);
        assert_eq!(kind_of("windows", &exe, present), InstallKind::Archive);
    }

    #[test]
    fn a_cellar_path_is_a_formula() {
        let exe =
            Path::new("/home/linuxbrew/.linuxbrew/Cellar/sunlit-earth/0.6.0/bin/sunlit-earth");
        assert_eq!(
            kind_of("linux", exe, tree(&[])),
            InstallKind::HomebrewFormula
        );
    }

    #[test]
    fn a_bundle_with_a_caskroom_folder_is_a_cask() {
        let exe = Path::new("/Applications/Sunlit Earth.app/Contents/MacOS/sunlit-earth");
        for root in CASKROOMS {
            assert_eq!(
                kind_of("macos", exe, tree(&[root])),
                InstallKind::HomebrewCask
            );
        }
    }

    #[test]
    fn a_bundle_without_a_caskroom_folder_is_an_archive() {
        let exe = Path::new("/Applications/Sunlit Earth.app/Contents/MacOS/sunlit-earth");
        assert_eq!(kind_of("macos", exe, tree(&[])), InstallKind::Archive);
    }

    #[test]
    fn a_bare_binary_beside_a_caskroom_folder_is_an_archive() {
        let exe = Path::new("/Users/me/sunlit-earth-0.6.0-macos-aarch64-terminal/sunlit-earth");
        assert_eq!(
            kind_of("macos", exe, tree(&[CASKROOMS[0]])),
            InstallKind::Archive
        );
    }

    #[test]
    fn a_plain_folder_is_an_archive_everywhere() {
        let exe = Path::new("/home/me/sunlit-earth-0.6.0-linux-x86_64/sunlit-earth");
        for os in ["windows", "linux", "macos"] {
            assert_eq!(kind_of(os, exe, tree(&[])), InstallKind::Archive);
        }
    }

    #[test]
    fn each_package_manager_names_its_own_command_and_hint() {
        for (kind, tool, command) in [
            (InstallKind::Scoop, "scoop", "scoop update sunlit-earth"),
            (
                InstallKind::HomebrewFormula,
                "brew",
                "brew upgrade sunlit-earth",
            ),
            (
                InstallKind::HomebrewCask,
                "brew",
                "brew upgrade --cask sunlit-earth",
            ),
        ] {
            let UpdateHelp::Command {
                command: given,
                hint,
            } = update_help(kind, "any", false)
            else {
                panic!("{kind:?} has no command");
            };
            assert_eq!(given, command);
            assert!(hint.contains(&format!("\"{tool} update\"")), "{hint}");
        }
    }

    #[test]
    fn every_archive_says_to_quit_first_and_that_the_settings_are_kept() {
        for (os, bundled) in [
            ("windows", false),
            ("linux", false),
            ("macos", true),
            ("macos", false),
        ] {
            let UpdateHelp::Archive { steps } = update_help(InstallKind::Archive, os, bundled)
            else {
                panic!("an archive has steps");
            };
            assert!(steps[0].starts_with("Quit Sunlit Earth"), "{os}");
            assert!(steps[1].starts_with("Your settings are kept."), "{os}");
        }
        let UpdateHelp::Archive { steps } = update_help(InstallKind::Archive, "macos", true) else {
            unreachable!()
        };
        assert!(steps[0].contains("Sunlit Earth.app"));
    }
}
