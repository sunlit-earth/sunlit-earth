//! Which desktop the Linux guest logs into, chosen by the host per boot.
//!
//! The golden image carries four of them side by side, and nothing in the image
//! decides which one a boot uses: the host writes a session name into QEMU's
//! `fw_cfg` and a oneshot unit in the guest puts it in sddm's autologin
//! configuration before the display manager starts. So one image serves four
//! desktops and a switch costs a reboot rather than an hour (plan decision 3).
//!
//! `fw_cfg` rather than SMBIOS OEM strings, which were the other candidate: the
//! `fw_cfg` device is ACPI-enumerated, so `qemu_fw_cfg` loads itself early and the
//! value appears as a file under `/sys/firmware/qemu_fw_cfg/by_name`, while
//! mainline exports no per-string sysfs interface for SMBIOS type 11 and reading
//! those means parsing a binary table or shipping `dmidecode`.

use clap::ValueEnum;

/// The `fw_cfg` key the host writes and the guest reads.
///
/// The `opt/` prefix is required: QEMU refuses to add a `-fw_cfg` entry outside
/// that namespace, which is reserved for exactly this.
pub const FW_CFG_NAME: &str = "opt/sunlit/desktop";

/// A desktop the Linux golden image can log into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Desktop {
    /// KDE Plasma on X11. The image default, and what a boot with no `--desktop`
    /// gets without the host having to say anything.
    Kde,
    /// GNOME on Xorg.
    Gnome,
    Xfce,
    Cinnamon,
}

impl Desktop {
    /// All four, in the order the docs and the reports list them.
    pub const ALL: [Self; 4] = [Self::Kde, Self::Gnome, Self::Xfce, Self::Cinnamon];

    /// What `--desktop` is spelled as, and what the run record stores.
    pub fn flag(self) -> &'static str {
        match self {
            Self::Kde => "kde",
            Self::Gnome => "gnome",
            Self::Xfce => "xfce",
            Self::Cinnamon => "cinnamon",
        }
    }

    /// The `.desktop` basename in `/usr/share/xsessions` that sddm's
    /// `[Autologin] Session=` wants.
    ///
    /// Read out of the Debian 13 archive rather than assumed, because only one of
    /// the four is the desktop's own name and the packages that ship them are not
    /// the packages one would guess: `plasmax11.desktop` from `plasma-workspace`,
    /// `gnome-xorg.desktop` from `gnome-session-xsession`, `xfce.desktop` from
    /// `xfce4-session`, `cinnamon.desktop` from `cinnamon-common`. Each of those
    /// packages ships Wayland sessions too, under `/usr/share/wayland-sessions`,
    /// and none of those is usable here: the guest contract launches windowed
    /// apps over SSH through `DISPLAY`.
    ///
    /// `the_guest_accepts_exactly_the_sessions_the_host_can_ask_for` pins each
    /// one against the selector shipped in the image.
    pub fn session(self) -> &'static str {
        match self {
            Self::Kde => "plasmax11",
            Self::Gnome => "gnome-xorg",
            Self::Xfce => "xfce",
            Self::Cinnamon => "cinnamon",
        }
    }

    /// The name a report calls this by.
    pub fn label(self) -> &'static str {
        match self {
            Self::Kde => "KDE Plasma (X11)",
            Self::Gnome => "GNOME (Xorg)",
            Self::Xfce => "XFCE",
            Self::Cinnamon => "Cinnamon",
        }
    }

    /// Read one back out of a run record, which stores [`Self::flag`].
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        Self::ALL.into_iter().find(|d| d.flag() == value)
    }
}

impl std::fmt::Display for Desktop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.flag())
    }
}

/// The `-fw_cfg` argument that tells a guest which session to log into.
pub fn fw_cfg_args(desktop: Desktop) -> Vec<String> {
    vec![
        "-fw_cfg".to_owned(),
        format!("name={FW_CFG_NAME},string={}", desktop.session()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_desktop_has_a_flag_a_session_and_a_label_of_its_own() {
        for (index, desktop) in Desktop::ALL.into_iter().enumerate() {
            assert_eq!(Desktop::parse(desktop.flag()), Some(desktop));
            for other in &Desktop::ALL[index + 1..] {
                assert_ne!(desktop.flag(), other.flag());
                assert_ne!(desktop.session(), other.session());
                assert_ne!(desktop.label(), other.label());
            }
        }
        assert_eq!(Desktop::parse("KDE"), None, "the record stores the flag");
        assert_eq!(Desktop::parse(""), None);
        assert_eq!(Desktop::parse("mate"), None);
    }

    #[test]
    fn the_named_sessions_are_the_x11_ones() {
        // Each of these packages ships Wayland sessions as well, and the guest
        // contract launches windowed apps over SSH through `DISPLAY`, so a
        // Wayland session here costs every windowed case in the suite. Pinned
        // rather than merely commented, because the two names differ by a
        // suffix in both cases where they differ at all.
        assert_eq!(Desktop::Kde.session(), "plasmax11");
        assert_eq!(Desktop::Gnome.session(), "gnome-xorg");
        assert_eq!(Desktop::Xfce.session(), "xfce");
        assert_eq!(Desktop::Cinnamon.session(), "cinnamon");
    }

    /// The host names a session and the guest decides whether to accept it, and
    /// nothing but this connects the two lists. A name added here and not there
    /// is a boot that falls back to Plasma while `vm status` says otherwise; a
    /// name removed there and not here is the same thing with the blame in the
    /// other place. Both sides are read, not just one, so neither list can grow
    /// past the other.
    #[test]
    fn the_guest_accepts_exactly_the_sessions_the_host_can_ask_for() {
        let script = std::fs::read_to_string(
            crate::store::template_dir(crate::provider::target::Image::Linux)
                .join("scripts/desktop.sh"),
        )
        .expect("the Linux desktop script");

        // The allowlist the selector matches against, as one `case` pattern.
        let allowlist = script
            .lines()
            .find_map(|line| line.trim().strip_suffix(") session=\"${asked}\" ;;"))
            .expect("the selector's allowlist is one case arm");
        let accepted: Vec<&str> = allowlist.split('|').collect();
        let asked_for: Vec<&str> = Desktop::ALL.iter().map(|d| d.session()).collect();
        assert_eq!(accepted, asked_for, "the two lists have drifted apart");

        // And every one of them is a session the image installs, which the
        // build checks for itself and this states in the one place a reader of
        // the constant would look.
        for session in &asked_for {
            assert!(
                script.contains("/usr/share/xsessions/${session}.desktop"),
                "the selector does not check the session file exists"
            );
            assert!(
                script.contains(session),
                "{session} is not a session this image installs"
            );
        }

        // The key the host writes is the path the guest reads.
        assert!(
            script.contains(&format!("qemu_fw_cfg/by_name/{FW_CFG_NAME}/raw")),
            "the selector reads a different fw_cfg entry than the host writes"
        );
    }

    #[test]
    fn the_fw_cfg_entry_is_in_the_namespace_qemu_allows() {
        // Outside `opt/`, QEMU rejects the argument and the guest never starts.
        assert!(FW_CFG_NAME.starts_with("opt/"), "{FW_CFG_NAME}");
        let args = fw_cfg_args(Desktop::Xfce);
        assert_eq!(args[0], "-fw_cfg");
        assert_eq!(args[1], "name=opt/sunlit/desktop,string=xfce");
    }
}
