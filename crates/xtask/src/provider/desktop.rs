//! Which desktop the Linux guest logs into, chosen by the host per boot.
//!
//! The golden image carries four of them side by side, and nothing in the image
//! decides which one a boot uses: the host writes a session name into QEMU's
//! `fw_cfg` and a oneshot unit in the guest puts it in sddm's autologin
//! configuration before the display manager starts. So one image serves four
//! desktops and a switch costs a reboot rather than an hour (plan decision 3).
//! KDE and GNOME also have a Wayland session in the same image, chosen the same
//! way: the guest only ever receives one session basename, so the session type
//! is a second axis on the host alone.
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
    /// KDE Plasma. On X11 the image default, and what a boot with no
    /// `--desktop` gets without the host having to say anything.
    Kde,
    Gnome,
    Xfce,
    Cinnamon,
    /// sway, a Wayland compositor with no desktop of its own, which is where
    /// the sway row and the owned `swaybg` are exercised.
    Sway,
    /// i3, a plain X11 window manager, which is where the root pixmap is.
    I3,
}

impl Desktop {
    /// All six, in the order the docs and the reports list them.
    pub const ALL: [Self; 6] = [
        Self::Kde,
        Self::Gnome,
        Self::Xfce,
        Self::Cinnamon,
        Self::Sway,
        Self::I3,
    ];

    /// What `--desktop` is spelled as, and what the run record stores.
    pub fn flag(self) -> &'static str {
        match self {
            Self::Kde => "kde",
            Self::Gnome => "gnome",
            Self::Xfce => "xfce",
            Self::Cinnamon => "cinnamon",
            Self::Sway => "sway",
            Self::I3 => "i3",
        }
    }

    /// The `.desktop` basename that sddm's `[Autologin] Session=` wants for
    /// this desktop on that display server, or `None` where the image has no
    /// such session.
    ///
    /// Read out of the Debian 13 archive rather than assumed, because only one of
    /// the X11 names is the desktop's own name and the packages that ship them are
    /// not the packages one would guess: `plasmax11.desktop` from
    /// `plasma-workspace`, `gnome-xorg.desktop` from `gnome-session-xsession`,
    /// `xfce.desktop` from `xfce4-session`, `cinnamon.desktop` from
    /// `cinnamon-common`. The two Wayland names are `plasma` from
    /// `plasma-workspace` and `gnome-wayland` from `gnome-session`. Not `gnome`,
    /// which is a basename in both session directories: sddm's autologin looks in
    /// the X11 one first (sddm issue #837), so `Session=gnome` starts GNOME on
    /// Xorg. XFCE's and Cinnamon's Wayland sessions are experimental in this
    /// release and not offered. sway runs on Wayland alone, as `sway` from
    /// `sway`, and i3 on X11 alone, as `i3` from `i3-wm`.
    ///
    /// `the_guest_accepts_exactly_the_sessions_the_host_can_ask_for` pins each
    /// one against the selector shipped in the image.
    pub fn session(self, session_type: SessionType) -> Option<&'static str> {
        match (self, session_type) {
            (Self::Kde, SessionType::X11) => Some("plasmax11"),
            (Self::Gnome, SessionType::X11) => Some("gnome-xorg"),
            (Self::Xfce, SessionType::X11) => Some("xfce"),
            (Self::Cinnamon, SessionType::X11) => Some("cinnamon"),
            (Self::Kde, SessionType::Wayland) => Some("plasma"),
            (Self::Gnome, SessionType::Wayland) => Some("gnome-wayland"),
            (Self::Sway, SessionType::Wayland) => Some("sway"),
            (Self::I3, SessionType::X11) => Some("i3"),
            (Self::Xfce | Self::Cinnamon | Self::I3, SessionType::Wayland)
            | (Self::Sway, SessionType::X11) => None,
        }
    }

    /// The session type a boot gets when `--session-type` names none: X11
    /// where the desktop has it, and otherwise the one it has.
    pub fn default_session_type(self) -> SessionType {
        if self.session(SessionType::X11).is_some() {
            SessionType::X11
        } else {
            SessionType::Wayland
        }
    }

    /// What `XDG_CURRENT_DESKTOP` names this desktop as inside its session,
    /// which is how the guest reports back which one came up.
    ///
    /// sway's and i3's come from the `DesktopNames` of their session files,
    /// `sway;wlroots` and `i3`, which sddm exports as the variable.
    pub fn current_desktop(self) -> &'static str {
        match self {
            Self::Kde => "KDE",
            Self::Gnome => "GNOME",
            Self::Xfce => "XFCE",
            Self::Cinnamon => "X-Cinnamon",
            Self::Sway => "sway",
            Self::I3 => "i3",
        }
    }

    /// The name a report calls this by.
    pub fn label(self) -> &'static str {
        match self {
            Self::Kde => "KDE Plasma (X11)",
            Self::Gnome => "GNOME (Xorg)",
            Self::Xfce => "XFCE",
            Self::Cinnamon => "Cinnamon",
            Self::Sway => "sway (Wayland)",
            Self::I3 => "i3 (X11)",
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

/// Which display server a session runs on, named as `XDG_SESSION_TYPE` names
/// it, since that is what the guest reports back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum SessionType {
    #[default]
    X11,
    Wayland,
}

impl SessionType {
    pub const ALL: [Self; 2] = [Self::X11, Self::Wayland];

    /// What `--session-type` is spelled as, what the run record stores, and
    /// what `XDG_SESSION_TYPE` holds inside such a session.
    pub fn flag(self) -> &'static str {
        match self {
            Self::X11 => "x11",
            Self::Wayland => "wayland",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        Self::ALL.into_iter().find(|t| t.flag() == value)
    }
}

impl std::fmt::Display for SessionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.flag())
    }
}

/// A desktop on a display server, which is what one boot logs into. Only a pair
/// the image has a session for can be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Login {
    desktop: Desktop,
    session_type: SessionType,
}

impl Login {
    /// What a Linux guest logs into when the host names nothing.
    pub const IMAGE_DEFAULT: Self = Self {
        desktop: Desktop::Kde,
        session_type: SessionType::X11,
    };

    /// The pair, or the refusal that names what would work instead.
    pub fn new(desktop: Desktop, session_type: SessionType) -> Result<Self, String> {
        if desktop.session(session_type).is_some() {
            return Ok(Self {
                desktop,
                session_type,
            });
        }
        let offered: Vec<String> = Desktop::ALL
            .into_iter()
            .filter(|d| d.session(session_type).is_some())
            .map(|d| format!("--desktop {d}"))
            .collect();
        let runs_on = desktop.default_session_type();
        let why = match desktop {
            Desktop::Xfce | Desktop::Cinnamon => {
                format!("{desktop}'s {session_type} session is experimental in Debian 13")
            }
            _ => format!("{desktop} has no {session_type} session"),
        };
        Err(format!(
            "--desktop {desktop} --session-type {session_type} asks for a session \
             this image does not offer: {why}. {} have one, and {desktop} runs with \
             --session-type {runs_on}",
            offered.join(", ")
        ))
    }

    pub fn desktop(self) -> Desktop {
        self.desktop
    }

    pub fn session_type(self) -> SessionType {
        self.session_type
    }

    /// The session basename the guest is handed.
    pub fn session(self) -> &'static str {
        self.desktop
            .session(self.session_type)
            .expect("a Login is only made for a pair the image has a session for")
    }

    /// The name a report calls this by.
    pub fn label(self) -> &'static str {
        match (self.desktop, self.session_type) {
            (desktop, SessionType::X11) => desktop.label(),
            (Desktop::Kde, SessionType::Wayland) => "KDE Plasma (Wayland)",
            (Desktop::Gnome, SessionType::Wayland) => "GNOME (Wayland)",
            (Desktop::Xfce, SessionType::Wayland) => "XFCE (Wayland)",
            (Desktop::Cinnamon, SessionType::Wayland) => "Cinnamon (Wayland)",
            (Desktop::Sway, SessionType::Wayland) => Desktop::Sway.label(),
            (Desktop::I3, SessionType::Wayland) => "i3 (Wayland)",
        }
    }

    /// Read one back out of a run record's two fields. A record with no
    /// session type is an X11 one, which is every record written before the
    /// field existed.
    pub fn from_record(desktop: Option<&str>, session_type: Option<&str>) -> Option<Self> {
        let desktop = Desktop::parse(desktop?)?;
        let session_type = match session_type {
            None => SessionType::X11,
            Some(value) => SessionType::parse(value)?,
        };
        Self::new(desktop, session_type).ok()
    }

    /// Whether the `session.env` a guest's session wrote describes this login,
    /// and if not, a refusal that names both.
    ///
    /// The likeliest failure of a session choice is a silent one: sddm resolves
    /// names by basename across two directories, and the selector falls back to
    /// Plasma on X11 for anything it does not accept, so without this a boot can
    /// come up in another session while every record names the one asked for.
    pub fn check_session_env(self, session_env: &str) -> Result<(), String> {
        let value = |key: &str| {
            session_env
                .lines()
                .find_map(|line| {
                    line.strip_prefix(key)
                        .and_then(|rest| rest.strip_prefix('='))
                })
                .map(str::trim)
                .unwrap_or_default()
        };
        let observed_type = value("XDG_SESSION_TYPE");
        let observed_desktop = value("XDG_CURRENT_DESKTOP");
        let desktop_matches = observed_desktop
            .split(':')
            .any(|name| name.eq_ignore_ascii_case(self.desktop.current_desktop()));
        if observed_type == self.session_type.flag() && desktop_matches {
            return Ok(());
        }
        let shown = |v: &str| {
            if v.is_empty() {
                "nothing".to_owned()
            } else {
                format!("{v:?}")
            }
        };
        Err(format!(
            "the guest was asked for {} (XDG_CURRENT_DESKTOP {:?}, XDG_SESSION_TYPE \
             {:?}) and its session reports XDG_CURRENT_DESKTOP {}, XDG_SESSION_TYPE \
             {}. The selector falls back to Plasma on X11 for a name it does not \
             accept, and sddm takes the X11 session when a name exists in both \
             session directories; `journalctl -b -u sunlit-e2e-desktop` in the \
             guest says which name it was handed",
            self.label(),
            self.desktop.current_desktop(),
            self.session_type.flag(),
            shown(observed_desktop),
            shown(observed_type),
        ))
    }
}

/// The `-fw_cfg` argument that tells a guest which session to log into.
pub fn fw_cfg_args(login: Login) -> Vec<String> {
    vec![
        "-fw_cfg".to_owned(),
        format!("name={FW_CFG_NAME},string={}", login.session()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accepted() -> Vec<Login> {
        SessionType::ALL
            .into_iter()
            .flat_map(|t| Desktop::ALL.map(|d| Login::new(d, t)))
            .filter_map(Result::ok)
            .collect()
    }

    /// Where sddm looks for a session of this type.
    fn session_dir(session_type: SessionType) -> &'static str {
        match session_type {
            SessionType::X11 => "/usr/share/xsessions",
            SessionType::Wayland => "/usr/share/wayland-sessions",
        }
    }

    fn names_of(session_type: SessionType) -> Vec<&'static str> {
        accepted()
            .into_iter()
            .filter(|login| login.session_type() == session_type)
            .map(Login::session)
            .collect()
    }

    #[test]
    fn every_desktop_has_a_flag_and_a_label_of_its_own() {
        for (index, desktop) in Desktop::ALL.into_iter().enumerate() {
            assert_eq!(Desktop::parse(desktop.flag()), Some(desktop));
            for other in &Desktop::ALL[index + 1..] {
                assert_ne!(desktop.flag(), other.flag());
                assert_ne!(desktop.label(), other.label());
                assert_ne!(desktop.current_desktop(), other.current_desktop());
            }
        }
        assert_eq!(Desktop::parse("KDE"), None, "the record stores the flag");
        assert_eq!(Desktop::parse(""), None);
        assert_eq!(Desktop::parse("mate"), None);
    }

    #[test]
    fn every_accepted_pair_has_a_session_and_a_label_of_its_own() {
        let logins = accepted();
        for (index, login) in logins.iter().enumerate() {
            for other in &logins[index + 1..] {
                assert_ne!(login.session(), other.session(), "{login:?} {other:?}");
                assert_ne!(login.label(), other.label(), "{login:?} {other:?}");
            }
        }
        assert!(
            !names_of(SessionType::Wayland).is_empty(),
            "no desktop has a Wayland session"
        );
    }

    #[test]
    fn the_x11_sessions_are_the_ones_every_desktop_had_before() {
        assert_eq!(Desktop::Kde.session(SessionType::X11), Some("plasmax11"));
        assert_eq!(Desktop::Gnome.session(SessionType::X11), Some("gnome-xorg"));
        assert_eq!(Desktop::Xfce.session(SessionType::X11), Some("xfce"));
        assert_eq!(
            Desktop::Cinnamon.session(SessionType::X11),
            Some("cinnamon")
        );
        assert_eq!(Login::IMAGE_DEFAULT.session(), "plasmax11");
    }

    #[test]
    fn wayland_gnome_is_not_the_basename_both_session_directories_have() {
        // sddm resolves the name in the X11 directory first, so `gnome` would
        // start GNOME on Xorg while every record said Wayland.
        assert_ne!(Desktop::Gnome.session(SessionType::Wayland), Some("gnome"));
    }

    #[test]
    fn a_desktop_without_a_wayland_session_is_refused_naming_the_ones_with_one() {
        for desktop in [Desktop::Xfce, Desktop::Cinnamon] {
            let refusal = Login::new(desktop, SessionType::Wayland)
                .expect_err("no Wayland session for this desktop");
            assert!(
                refusal.contains(&format!("--desktop {desktop} --session-type wayland")),
                "{refusal}"
            );
            assert!(refusal.contains("--desktop kde"), "{refusal}");
            assert!(refusal.contains("--desktop gnome"), "{refusal}");
            assert!(refusal.contains("--session-type x11"), "{refusal}");
        }
    }

    #[test]
    fn sway_and_i3_have_one_session_each_and_get_it_by_default() {
        assert_eq!(Desktop::Sway.default_session_type(), SessionType::Wayland);
        assert_eq!(Desktop::I3.default_session_type(), SessionType::X11);
        assert_eq!(Desktop::Kde.default_session_type(), SessionType::X11);

        let refusal = Login::new(Desktop::Sway, SessionType::X11).expect_err("Wayland only");
        assert!(refusal.contains("sway has no x11 session"), "{refusal}");
        assert!(refusal.contains("--session-type wayland"), "{refusal}");
        assert!(!refusal.contains("experimental"), "{refusal}");
        let refusal = Login::new(Desktop::I3, SessionType::Wayland).expect_err("X11 only");
        assert!(refusal.contains("--session-type x11"), "{refusal}");
        assert!(refusal.contains("--desktop sway"), "{refusal}");
    }

    #[test]
    fn a_record_reads_back_as_the_login_it_was_written_from() {
        for login in accepted() {
            assert_eq!(
                Login::from_record(
                    Some(login.desktop().flag()),
                    Some(login.session_type().flag())
                ),
                Some(login)
            );
        }
        assert_eq!(
            Login::from_record(Some("gnome"), None),
            Login::new(Desktop::Gnome, SessionType::X11).ok(),
            "a record from before the field is an X11 one"
        );
        assert_eq!(Login::from_record(Some("xfce"), Some("wayland")), None);
        assert_eq!(Login::from_record(None, Some("wayland")), None);
        assert_eq!(Login::from_record(Some("kde"), Some("mir")), None);
    }

    /// The host names a session and the guest decides whether to accept it, and
    /// nothing but this connects the two lists. A name added here and not there
    /// is a boot that falls back to Plasma; a name removed there and not here is
    /// the same thing with the blame in the other place. Both sides are read, so
    /// neither list can grow past the other, and each name is checked against
    /// the directory its session type belongs in, because sddm's autologin
    /// finds a name in whichever directory has it.
    #[test]
    fn the_guest_accepts_exactly_the_sessions_the_host_can_ask_for() {
        let script = std::fs::read_to_string(
            crate::store::template_dir(crate::provider::target::Image::Linux)
                .join("scripts/desktop.sh"),
        )
        .expect("the Linux desktop script");
        let arm = |suffix: &str| -> Vec<&str> {
            script
                .lines()
                .find_map(|line| line.trim().strip_suffix(suffix))
                .unwrap_or_else(|| panic!("no case arm ends {suffix:?}"))
                .split('|')
                .collect()
        };

        let allowlist = arm(") session=\"${asked}\" ;;");
        let asked_for: Vec<&str> = accepted().into_iter().map(Login::session).collect();
        assert_eq!(allowlist, asked_for, "the two lists have drifted apart");

        let wayland = names_of(SessionType::Wayland);
        assert_eq!(
            arm(&format!(
                ") sessions_dir={} ;;",
                session_dir(SessionType::Wayland)
            )),
            wayland,
            "the selector looks a Wayland name up somewhere else"
        );
        assert!(
            script.contains(&format!(
                "*) sessions_dir={} ;;",
                session_dir(SessionType::X11)
            )),
            "the selector does not look the X11 names up in the X11 directory"
        );
        assert!(
            script.contains("[ ! -f \"${sessions_dir}/${session}.desktop\" ]"),
            "the selector does not check the session file exists"
        );

        // The build asserts every name is installed where its type belongs, and
        // that no Wayland name is also an X11 one.
        for session_type in SessionType::ALL {
            let names = names_of(session_type).join(" ");
            let dir = session_dir(session_type);
            assert!(
                script.contains(&format!("for session in {names}; do")),
                "the build does not check the {session_type} names {names}"
            );
            assert!(
                script.contains(&format!("test -f \"{dir}/${{session}}.desktop\"")),
                "the build does not look for the {session_type} sessions in {dir}"
            );
        }
        assert!(
            script.contains(&format!(
                "[ -e \"{}/${{session}}.desktop\" ]",
                session_dir(SessionType::X11)
            )),
            "the build does not refuse a Wayland name the X11 directory also has"
        );

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
        let xfce = Login::new(Desktop::Xfce, SessionType::X11).unwrap();
        let args = fw_cfg_args(xfce);
        assert_eq!(args[0], "-fw_cfg");
        assert_eq!(args[1], "name=opt/sunlit/desktop,string=xfce");
        let plasma = Login::new(Desktop::Kde, SessionType::Wayland).unwrap();
        assert_eq!(
            fw_cfg_args(plasma)[1],
            "name=opt/sunlit/desktop,string=plasma"
        );
    }

    fn session_env(desktop: &str, session_type: &str) -> String {
        format!(
            "DISPLAY=:0\nXAUTHORITY=/run/user/1000/xauth\n\
             XDG_CURRENT_DESKTOP={desktop}\nXDG_SESSION_TYPE={session_type}\n\
             XDG_SESSION_DESKTOP={desktop}\n"
        )
    }

    #[test]
    fn a_session_that_came_up_as_asked_passes_the_check() {
        let gnome = Login::new(Desktop::Gnome, SessionType::Wayland).unwrap();
        assert_eq!(
            gnome.check_session_env(&session_env("GNOME", "wayland")),
            Ok(())
        );
        let cinnamon = Login::new(Desktop::Cinnamon, SessionType::X11).unwrap();
        assert_eq!(
            cinnamon.check_session_env(&session_env("X-Cinnamon", "x11")),
            Ok(())
        );
        assert_eq!(
            gnome.check_session_env(&session_env("ubuntu:GNOME", "wayland")),
            Ok(()),
            "a distribution's own name in front of the desktop's"
        );
    }

    #[test]
    fn a_session_that_fell_back_names_what_was_asked_for_and_what_came_up() {
        let plasma = Login::new(Desktop::Kde, SessionType::Wayland).unwrap();
        let refusal = plasma
            .check_session_env(&session_env("KDE", "x11"))
            .expect_err("an X11 session is not the Wayland one asked for");
        assert!(refusal.contains("KDE Plasma (Wayland)"), "{refusal}");
        assert!(refusal.contains("XDG_SESSION_TYPE \"x11\""), "{refusal}");

        let gnome = Login::new(Desktop::Gnome, SessionType::Wayland).unwrap();
        let refusal = gnome
            .check_session_env(&session_env("KDE", "wayland"))
            .expect_err("Plasma is not GNOME");
        assert!(refusal.contains("GNOME (Wayland)"), "{refusal}");
        assert!(refusal.contains("XDG_CURRENT_DESKTOP \"KDE\""), "{refusal}");

        let refusal = Login::IMAGE_DEFAULT
            .check_session_env("")
            .expect_err("an empty file describes no session");
        assert!(refusal.contains("nothing"), "{refusal}");
    }

    #[test]
    fn the_check_reads_the_variable_it_names_and_not_one_that_starts_with_it() {
        let env = "XDG_SESSION_TYPE_OLD=wayland\nXDG_SESSION_TYPE=x11\nXDG_CURRENT_DESKTOP=KDE\n";
        assert_eq!(Login::IMAGE_DEFAULT.check_session_env(env), Ok(()));
    }
}
