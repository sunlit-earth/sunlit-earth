//! Finding the UEFI firmware QEMU needs for a Windows guest.
//!
//! Windows 11 wants UEFI. The `Hyper-V` provider gets that for free from a
//! generation 2 VM; QEMU needs to be handed OVMF, which every distribution and
//! every QEMU build puts somewhere different, under a name of its own. There is
//! no API to ask, so this is a list of the places it is and the names it goes
//! by, checked in order.

use std::path::{Path, PathBuf};

use crate::provider::target::HostOs;

/// The two halves of an OVMF installation.
///
/// The code half is read-only and shared. The variables half is per-VM,
/// because the firmware writes to it while booting and two VMs sharing one
/// store corrupt each other's.
///
/// What it does not carry is Windows Setup's boot entry: the store Setup wrote
/// belongs to Packer's output directory and goes when that is cleaned up, so
/// every guest starts with blank NVRAM. Booting works anyway because the
/// image's finalize step puts a fallback loader at `\EFI\Boot\bootx64.efi`,
/// which is the path UEFI tries when no variable names one. `Hyper-V` depends
/// on the same fallback for the same reason, so one mechanism covers both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Firmware {
    pub code: PathBuf,
    pub vars: PathBuf,
}

/// The `(code, vars)` file names one installation goes by, most likely first.
///
/// The halves are paired by name rather than searched for separately because
/// they have to match: the 4 MB code half wants the 4 MB variables store, not
/// the 2 MB one lying next to it.
///
/// `_4M` leads because it is the build Debian and Ubuntu package today, and
/// their `ovmf` package carries nothing else: an Ubuntu 24.04 host with the
/// package installed has no `OVMF_CODE.fd` at all, which is why looking for
/// the plain names alone reported the firmware missing on a host that had it.
/// The rest are the other spellings of the same two files, `.4m.fd` and the
/// plain pair elsewhere, `edk2-` in QEMU's own builds.
pub const NAMES: [(&str, &str); 4] = [
    ("OVMF_CODE_4M.fd", "OVMF_VARS_4M.fd"),
    ("OVMF_CODE.4m.fd", "OVMF_VARS.4m.fd"),
    ("OVMF_CODE.fd", "OVMF_VARS.fd"),
    ("edk2-x86_64-code.fd", "edk2-i386-vars.fd"),
];

/// The directories a distribution installs it into.
pub const LINUX_DIRS: [&str; 5] = [
    "/usr/share/OVMF",
    "/usr/share/edk2/ovmf",
    "/usr/share/edk2/x64",
    "/usr/share/edk2-ovmf/x64",
    "/usr/share/qemu",
];

/// The directories to search on this host, most likely first.
///
/// On Windows the QEMU installation carries the firmware in its `share`
/// directory, so the search starts from the binary's own location rather than
/// from a guess. That directory comes first on any host that has one.
fn dirs(host: HostOs, qemu_binary: Option<&Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(dir) = qemu_binary.and_then(Path::parent) {
        out.push(dir.join("share"));
        out.push(dir.to_path_buf());
    }
    if host == HostOs::Linux {
        out.extend(LINUX_DIRS.iter().map(PathBuf::from));
    }
    out
}

/// Candidate `(code, vars)` pairs for this host, most likely first.
pub fn candidates(host: HostOs, qemu_binary: Option<&Path>) -> Vec<(PathBuf, PathBuf)> {
    dirs(host, qemu_binary)
        .iter()
        .flat_map(|dir| {
            NAMES
                .iter()
                .map(|(code, vars)| (dir.join(code), dir.join(vars)))
        })
        .collect()
}

/// The first candidate whose halves both exist.
pub fn find(candidates: &[(PathBuf, PathBuf)], exists: &dyn Fn(&Path) -> bool) -> Option<Firmware> {
    candidates
        .iter()
        .find(|(code, vars)| exists(code) && exists(vars))
        .map(|(code, vars)| Firmware {
            code: code.clone(),
            vars: vars.clone(),
        })
}

/// Locate the firmware on this host.
pub fn locate(host: HostOs, qemu_binary: Option<&Path>) -> Option<Firmware> {
    find(&candidates(host, qemu_binary), &|p| p.is_file())
}

/// Where the search looked, which is the actionable half of "not found": a
/// host may carry the firmware under a name nothing here knows, and there is
/// no way to tell that from "none found" alone. [`missing_message`] promises
/// the doctor prints this, so the two belong together.
pub fn searched(host: HostOs, qemu_binary: Option<&Path>) -> String {
    let names: Vec<&str> = NAMES.iter().map(|(code, _)| *code).collect();
    let dirs: Vec<String> = dirs(host, qemu_binary)
        .iter()
        .map(|dir| dir.display().to_string())
        .collect();
    format!(
        "{}, each beside its matching variables half, in {}",
        names.join(", "),
        dirs.join(", ")
    )
}

/// What to say when it is not there.
pub fn missing_message(host: HostOs) -> String {
    let hint = match host {
        HostOs::Windows => {
            "it ships with QEMU, in the `share` directory beside qemu-system-x86_64.exe"
        }
        _ => "install the ovmf package, which `cargo xtask vm setup` also does",
    };
    format!(
        "no UEFI firmware found, and the Windows guest needs it under QEMU: {hint}. \
         `cargo xtask vm doctor` lists where it looked."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // Windows path semantics: off Windows, `Path` treats a drive-qualified
    // path as a single component, and this code only ever runs on a
    // Windows host anyway.
    #[cfg(windows)]
    #[test]
    fn the_search_starts_beside_the_qemu_binary() {
        let candidates = candidates(
            HostOs::Windows,
            Some(Path::new(r"C:\Program Files\qemu\qemu-system-x86_64.exe")),
        );
        let share = Path::new(r"C:\Program Files\qemu\share");
        assert!(
            candidates
                .iter()
                .take(NAMES.len())
                .all(|(code, _)| code.parent() == Some(share)),
            "{candidates:?}"
        );
        assert!(
            candidates
                .iter()
                .any(|(code, vars)| *code == share.join("edk2-x86_64-code.fd")
                    && *vars == share.join("edk2-i386-vars.fd")),
            "{candidates:?}"
        );
    }

    #[test]
    fn a_linux_host_also_looks_in_the_distribution_locations() {
        let candidates = candidates(HostOs::Linux, None);
        assert!(
            candidates
                .iter()
                .any(|(code, _)| code == Path::new("/usr/share/OVMF/OVMF_CODE.fd")),
            "{candidates:?}"
        );
    }

    // Debian and Ubuntu have shipped only the 4 MB build for several releases,
    // so a host with the ovmf package and nothing else installed is found
    // through this name or not at all.
    #[test]
    fn the_debian_four_megabyte_pair_is_a_candidate() {
        let candidates = candidates(HostOs::Linux, None);
        assert!(
            candidates.contains(&(
                PathBuf::from("/usr/share/OVMF/OVMF_CODE_4M.fd"),
                PathBuf::from("/usr/share/OVMF/OVMF_VARS_4M.fd"),
            )),
            "{candidates:?}"
        );
    }

    // Mixing builds does not boot, so no candidate may pair one name's code
    // half with another's variables store.
    #[test]
    fn every_candidate_pairs_the_two_halves_of_one_build() {
        for (code, vars) in candidates(HostOs::Linux, None) {
            let file = |p: &Path| p.file_name().unwrap().to_string_lossy().into_owned();
            assert!(
                NAMES
                    .iter()
                    .any(|(c, v)| *c == file(&code) && *v == file(&vars)),
                "{code:?} paired with {vars:?}"
            );
            assert_eq!(code.parent(), vars.parent());
        }
    }

    #[test]
    fn a_windows_host_does_not_look_in_linux_locations() {
        let candidates = candidates(HostOs::Windows, None);
        assert!(candidates.is_empty());
    }

    #[test]
    fn both_halves_have_to_be_there() {
        let candidates = vec![
            (PathBuf::from("/a/code.fd"), PathBuf::from("/a/vars.fd")),
            (PathBuf::from("/b/code.fd"), PathBuf::from("/b/vars.fd")),
        ];
        // Only the code half of the first pair exists, so it is skipped: a
        // firmware without its variables store cannot boot an installed
        // Windows.
        let exists = |p: &Path| p != Path::new("/a/vars.fd");
        assert_eq!(
            find(&candidates, &exists),
            Some(Firmware {
                code: PathBuf::from("/b/code.fd"),
                vars: PathBuf::from("/b/vars.fd"),
            })
        );
        assert_eq!(find(&candidates, &|_| false), None);
    }

    #[test]
    fn the_missing_message_says_where_it_comes_from_on_this_host() {
        assert!(missing_message(HostOs::Windows).contains("ships with QEMU"));
        assert!(missing_message(HostOs::Linux).contains("ovmf"));
    }

    #[test]
    fn what_was_searched_names_every_directory_and_every_name() {
        let searched = searched(HostOs::Linux, None);
        for dir in LINUX_DIRS {
            assert!(searched.contains(dir), "{searched}");
        }
        for (code, _) in NAMES {
            assert!(searched.contains(code), "{searched}");
        }
    }
}
