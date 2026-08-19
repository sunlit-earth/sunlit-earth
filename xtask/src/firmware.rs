//! Finding the UEFI firmware QEMU needs for a Windows guest.
//!
//! Windows 11 wants UEFI. The `Hyper-V` provider gets that for free from a
//! generation 2 VM; QEMU needs to be handed OVMF, which every distribution and
//! every QEMU build puts somewhere different. There is no API to ask, so this
//! is a list of the places it is, checked in order.

use std::path::{Path, PathBuf};

use crate::target::HostOs;

/// The two halves of an OVMF installation.
///
/// The code half is read-only and shared. The variables half is per-VM: it is
/// where the firmware stores its boot entries, and Windows Setup writes one.
/// Booting with a shared or absent variables store is how an installed Windows
/// ends up with no boot entry to find.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Firmware {
    pub code: PathBuf,
    pub vars: PathBuf,
}

/// Candidate `(code, vars)` pairs for this host, most likely first.
///
/// On Windows the QEMU installation carries them in its `share` directory, so
/// the search starts from the binary's own location rather than from a guess.
pub fn candidates(host: HostOs, qemu_binary: Option<&Path>) -> Vec<(PathBuf, PathBuf)> {
    let mut out = Vec::new();
    if let Some(dir) = qemu_binary.and_then(Path::parent) {
        for share in [dir.join("share"), dir.to_path_buf()] {
            out.push((
                share.join("edk2-x86_64-code.fd"),
                share.join("edk2-i386-vars.fd"),
            ));
            out.push((share.join("OVMF_CODE.fd"), share.join("OVMF_VARS.fd")));
        }
    }
    if host == HostOs::Linux {
        for dir in [
            "/usr/share/OVMF",
            "/usr/share/edk2/ovmf",
            "/usr/share/edk2-ovmf/x64",
            "/usr/share/qemu",
        ] {
            let dir = Path::new(dir);
            out.push((dir.join("OVMF_CODE.fd"), dir.join("OVMF_VARS.fd")));
            out.push((
                dir.join("edk2-x86_64-code.fd"),
                dir.join("edk2-i386-vars.fd"),
            ));
        }
    }
    out
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

/// What to say when it is not there.
pub fn missing_message(host: HostOs) -> String {
    let hint = match host {
        HostOs::Windows => {
            "it ships with QEMU, in the `share` directory beside qemu-system-x86_64.exe"
        }
        _ => "install the ovmf package",
    };
    format!(
        "no UEFI firmware found, and the Windows guest needs it under QEMU: {hint}. \
         `cargo xtask vm doctor` lists where it looked."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_search_starts_beside_the_qemu_binary() {
        let candidates = candidates(
            HostOs::Windows,
            Some(Path::new(r"C:\Program Files\qemu\qemu-system-x86_64.exe")),
        );
        assert_eq!(
            candidates[0].0,
            PathBuf::from(r"C:\Program Files\qemu\share\edk2-x86_64-code.fd")
        );
        assert!(candidates[0].1.ends_with("edk2-i386-vars.fd"));
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
}
