//! What a guest needs before a person is handed the keys to it.
//!
//! `vm up` exists to be looked at, and `e2e --keep` leaves the same guest
//! behind. Two things stand between a staged guest and someone double-clicking
//! the app in it: the environment the app cannot discover for itself, and
//! somewhere obvious to click. This module produces both, as a launcher beside
//! the binaries and a pair of shortcuts on the console user's desktop.
//!
//! Per boot rather than baked into the golden image, for the same reason the
//! job script is per run: the launcher names the textures directory only when
//! the textures were staged, and whether they were is a fact about this boot
//! rather than about the image. It also means neither of these ever needs an
//! image rebuild to change.
//!
//! Both guests get the same two things, and almost none of the mechanism is
//! shared. A Windows guest needs a batch file that sets `SLINT_BACKEND` and a
//! pair of `.lnk` shortcuts written by `WScript.Shell`; a Linux guest needs
//! neither the override, because Mesa answers GL there, nor a COM object,
//! because its shortcuts are text files. What it does need is to be told where
//! its root is: `/var/lib/sunlit-e2e` is outside any home directory on purpose,
//! and until this existed the first person handed a KDE guest had nowhere to
//! look for the app.

use crate::commands::e2e::WINDOWS_SLINT_BACKEND;
use crate::guest::artifacts::{GuestPaths, shell_quote};
use crate::provider::Provider;
use crate::provider::target::{Image, ProviderKind, Target};
use crate::runner::{encode_command, encode_text};
use crate::store::Store;
use crate::store::state::RunState;

/// The launcher's name inside the guest, beside the staged binaries.
pub const LAUNCHER: &str = "run-app.cmd";

/// What the desktop shortcuts are called.
///
/// The app one is title case because it is a thing to launch; the folder keeps
/// the directory's own name, so that what is on the desktop and what is on the
/// disk read as the same place.
pub const APP_SHORTCUT: &str = "Sunlit Earth.lnk";
pub const FOLDER_SHORTCUT: &str = "sunlit-e2e.lnk";

/// What the app is called wherever a person is offered it.
///
/// One constant for both guests: a shortcut on a Windows desktop and a desktop
/// entry in a Linux menu are the same offer, and the closing text names it from
/// here rather than from either platform's file name.
pub const ENTRY_NAME: &str = "Sunlit Earth";

/// Where the launcher is written inside the guest.
pub fn launcher_path() -> String {
    format!(r"{}\{LAUNCHER}", crate::provider::GUEST_ROOT_WINDOWS)
}

/// The batch file that starts the app the way this guest has to start it.
///
/// `SLINT_BACKEND` is the whole point: the same value the generated job sets,
/// from the same constant, because a guest that cannot run the app by hand and
/// can run it under the harness is a guest nobody can debug in. The app is a
/// console-subsystem binary, so its log lands in the window the launcher was
/// started from; the exit code is captured before anything else can overwrite
/// it, and a failure holds the window open long enough to be read.
pub fn launcher_script(paths: &GuestPaths) -> String {
    let textures = paths.textures.as_ref().map_or_else(String::new, |dir| {
        format!("set SUNLIT_EARTH_TEXTURES={dir}\r\n")
    });
    format!(
        "@echo off\r\n\
         rem Written by `cargo xtask vm up`. This guest is a throwaway overlay,\r\n\
         rem so an edit here lasts until it is taken down.\r\n\
         setlocal\r\n\
         rem This guest's display adapter offers no OpenGL and Windows ships no\r\n\
         rem software implementation of it, so Slint's default renderer cannot\r\n\
         rem start and the app exits before its window appears.\r\n\
         set SLINT_BACKEND={backend}\r\n\
         {textures}\
         if not exist \"{app}\" goto :missing\r\n\
         \"{app}\" %*\r\n\
         set code=%ERRORLEVEL%\r\n\
         if not \"%code%\"==\"0\" (\r\n\
         \x20 echo.\r\n\
         \x20 echo the app exited with %code%\r\n\
         \x20 pause\r\n\
         )\r\n\
         exit /b %code%\r\n\
         \r\n\
         :missing\r\n\
         echo {app} is not here.\r\n\
         echo `cargo xtask vm up windows` on the host copies the binaries in.\r\n\
         pause\r\n\
         exit /b 1\r\n",
        backend = WINDOWS_SLINT_BACKEND,
        app = paths.app,
    )
}

/// The `PowerShell` that puts both shortcuts on the console user's desktop.
///
/// Run over SSH, which lands in the same account the console session belongs
/// to, so `GetFolderPath` names the desktop that is actually on screen. The app
/// shortcut points at the launcher rather than at the binary, since the binary
/// on its own is the thing that does not work, and takes its icon from the
/// binary anyway so that the desktop shows the app rather than a batch file.
pub fn shortcut_script(paths: &GuestPaths) -> String {
    use crate::runner::ps_quote;
    format!(
        "$ErrorActionPreference = 'Stop'\n\
         $desktop = [Environment]::GetFolderPath('Desktop')\n\
         $shell = New-Object -ComObject WScript.Shell\n\
         $app = $shell.CreateShortcut((Join-Path $desktop {app_link}))\n\
         $app.TargetPath = {launcher}\n\
         $app.WorkingDirectory = {root}\n\
         $app.IconLocation = {icon}\n\
         $app.Description = 'Start sunlit earth with the environment this guest needs'\n\
         $app.Save()\n\
         $folder = $shell.CreateShortcut((Join-Path $desktop {folder_link}))\n\
         $folder.TargetPath = {root}\n\
         $folder.Description = 'The binaries, fixtures and results this run staged'\n\
         $folder.Save()\n\
         Write-Output 'shortcuts written'\n",
        app_link = ps_quote(APP_SHORTCUT),
        launcher = ps_quote(&launcher_path()),
        root = ps_quote(crate::provider::GUEST_ROOT_WINDOWS),
        icon = ps_quote(&format!("{},0", paths.app)),
        folder_link = ps_quote(FOLDER_SHORTCUT),
    )
}

/// The command that runs a script in the guest without quoting it twice.
///
/// The guest's SSH shell is `cmd.exe` and the script has quotes, newlines and
/// `$` in it, none of which survive being passed through as a command line.
/// `-EncodedCommand` is one base64 token: nothing in it needs escaping, which
/// is the same reason the host side uses it.
pub fn powershell_command(script: &str) -> String {
    format!(
        "powershell.exe -NoProfile -NonInteractive -EncodedCommand {}",
        encode_command(script)
    )
}

/// The Linux launcher's name, and where it sits inside the guest.
pub const LINUX_LAUNCHER: &str = "run-app.sh";

/// Where the app's own output goes when nobody started it from a terminal.
pub const LINUX_LAUNCHER_LOG: &str = "run-app.log";

/// The desktop entries' file names.
///
/// Both are written twice, into the applications directory and onto the desktop,
/// under the same name in each: an entry that appears in the menu under one name
/// and on the desktop under another is two things as far as a reader is
/// concerned.
pub const APP_ENTRY: &str = "sunlit-earth.desktop";
pub const FOLDER_ENTRY: &str = "sunlit-e2e.desktop";

/// What the install script prints once it is through, for the same reason the
/// enhanced-session script has a marker: the interesting failures leave an exit
/// code that says nothing.
pub const LINUX_HANDOVER_READY: &str = "HANDOVER=ready";

/// Where the launcher is written inside the guest: beside the binaries, in the
/// root the closing text names, so that what a person is told to type and what
/// the desktop entry runs are the same path.
pub fn linux_launcher_path() -> String {
    format!("{}/{LINUX_LAUNCHER}", crate::provider::GUEST_ROOT_LINUX)
}

fn linux_launcher_log() -> String {
    format!("{}/{LINUX_LAUNCHER_LOG}", crate::provider::GUEST_ROOT_LINUX)
}

/// The shell script that starts the app the way this guest has to start it.
///
/// No `SLINT_BACKEND`: Mesa is a software GL implementation and llvmpipe answers
/// in here, which is the whole reason only the Windows launcher sets one. So what
/// is left is the textures directory, named exactly when this boot staged one,
/// and the two things a person clicking an icon cannot get for themselves: a
/// place for the app's output to go, and a sentence saying what is missing when
/// the binaries are not there.
pub fn linux_launcher_script(paths: &GuestPaths) -> String {
    let textures = paths.textures.as_ref().map_or_else(String::new, |dir| {
        format!("export SUNLIT_EARTH_TEXTURES={}\n", shell_quote(dir))
    });
    format!(
        "#!/usr/bin/env bash\n\
         # Written by `cargo xtask vm up`. This guest is a throwaway overlay,\n\
         # so an edit here lasts until it is taken down.\n\
         set -u\n\
         app={app}\n\
         # Started from a desktop entry there is no terminal for anything below\n\
         # to print to, so it all goes to the log instead. Started from a shell\n\
         # the terminal is the better place, and a log would hide it there.\n\
         if [ ! -t 1 ]; then\n\
         \x20 exec >>{log} 2>&1\n\
         fi\n\
         if [ ! -x \"${{app}}\" ]; then\n\
         \x20 echo \"${{app}} is not here.\"\n\
         \x20 echo 'cargo xtask vm up linux on the host copies the binaries in.'\n\
         \x20 exit 1\n\
         fi\n\
         {textures}\
         exec \"${{app}}\" \"$@\"\n",
        app = shell_quote(&paths.app),
        log = shell_quote(&linux_launcher_log()),
    )
}

/// The entry that starts the app, through the launcher.
///
/// `Exec` is absolute, because a desktop entry is run with whatever working
/// directory the shell that launched it had, and on GNOME that is the session's
/// own. Through the launcher rather than the binary for the same reason the
/// Windows shortcut is: the environment the app needs is the launcher's whole
/// purpose, and an entry that skipped it would be the one thing in the guest
/// that starts the app differently from everything else.
///
/// `Icon` is a stock name from the icon theme. The app ships no icon file, and
/// every desktop in the image has an icon theme, so a name that resolves in all
/// four is better than a missing file in each.
pub fn app_entry() -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Version=1.0\n\
         Name={ENTRY_NAME}\n\
         Comment=Start sunlit earth with what this boot staged in the guest\n\
         Exec={launcher}\n\
         Path={root}\n\
         Icon=applications-graphics\n\
         Terminal=false\n\
         Categories=Graphics;\n",
        launcher = linux_launcher_path(),
        root = crate::provider::GUEST_ROOT_LINUX,
    )
}

/// The entry that opens the guest's root in a file manager.
///
/// An application entry running `xdg-open` rather than a `Type=Link`, which
/// would be the more obvious spelling for a place: a menu shows `Type=Application`
/// entries and nothing else, and GNOME has no desktop icons at all, so a link
/// would be invisible on the one desktop where the menu is the whole hand-over.
/// `xdg-open` is what every desktop in the image routes to its own file manager,
/// so one entry covers all four.
pub fn folder_entry() -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Version=1.0\n\
         Name={name}\n\
         Comment=The binaries, fixtures and results this run staged\n\
         Exec=xdg-open {root}\n\
         Icon=folder\n\
         Terminal=false\n\
         Categories=Utility;\n",
        name = FOLDER_ENTRY.trim_end_matches(".desktop"),
        root = crate::provider::GUEST_ROOT_LINUX,
    )
}

/// The script that puts all three files where the desktop will find them.
///
/// Run over SSH as the account the console session belongs to, so `HOME` is the
/// home whose desktop is on screen and nothing here needs root; the guest's root
/// is owned by that account too, which is what lets the launcher be written
/// beside the binaries.
///
/// The desktop directory is asked for rather than assumed: it is localized, and
/// `xdg-user-dir` answers `$HOME` for a home with no user-dirs configuration at
/// all, which is not a desktop to write two launchers into. Both entries are
/// written into the applications directory as well, and that copy is the only
/// one GNOME will ever show, because GNOME draws no desktop icons.
pub fn linux_install_script(paths: &GuestPaths) -> String {
    format!(
        "#!/usr/bin/env bash\n\
         set -eu\n\
         # The blessing below is written by the session's own metadata daemon,\n\
         # over the session bus a process started from outside the session does\n\
         # not have. The guest writes that environment out at logon for exactly\n\
         # this class of process, and the job runner sources the same file.\n\
         if [ -f {session_env} ]; then\n\
         \x20 set -a\n\
         \x20 . {session_env}\n\
         \x20 set +a\n\
         fi\n\
         launcher={launcher}\n\
         apps=\"${{HOME}}/.local/share/applications\"\n\
         desktop=\"$(xdg-user-dir DESKTOP 2>/dev/null || true)\"\n\
         if [ -z \"${{desktop}}\" ] || [ \"${{desktop}}\" = \"${{HOME}}\" ]; then\n\
         \x20 desktop=\"${{HOME}}/Desktop\"\n\
         fi\n\
         mkdir -p \"${{apps}}\" \"${{desktop}}\"\n\
         \n\
         cat > \"${{launcher}}\" <<'SUNLIT_LAUNCHER_EOF'\n\
         {launcher_script}\
         SUNLIT_LAUNCHER_EOF\n\
         chmod 0755 \"${{launcher}}\"\n\
         \n\
         cat > \"${{apps}}/{app_entry_name}\" <<'SUNLIT_APP_EOF'\n\
         {app_entry}\
         SUNLIT_APP_EOF\n\
         \n\
         cat > \"${{apps}}/{folder_entry_name}\" <<'SUNLIT_FOLDER_EOF'\n\
         {folder_entry}\
         SUNLIT_FOLDER_EOF\n\
         \n\
         # The executable bit is the whole blessing for two of the three\n\
         # desktops that draw icons: measured in the guest, Plasma and Nemo both\n\
         # run an executable entry without asking. xfdesktop is the one that\n\
         # does not: it calls the desktop an insecure location whatever the mode\n\
         # bits say, and wants its own mark, which is the file's sha256 under the\n\
         # name below. That is exactly what its \"Mark As Secure And Launch\"\n\
         # button writes, so writing it here is that dialog answered in advance.\n\
         # Every other desktop ignores the attribute, and a guest with no\n\
         # metadata daemon to write it is no reason to fail the hand-over.\n\
         for entry in {app_entry_name} {folder_entry_name}; do\n\
         \x20 chmod 0755 \"${{apps}}/${{entry}}\"\n\
         \x20 cp -f \"${{apps}}/${{entry}}\" \"${{desktop}}/${{entry}}\"\n\
         \x20 chmod 0755 \"${{desktop}}/${{entry}}\"\n\
         \x20 gio set -t string \"${{desktop}}/${{entry}}\" \
         metadata::xfce-exe-checksum \
         \"$(sha256sum \"${{desktop}}/${{entry}}\" | cut -d' ' -f1)\" \
         >/dev/null 2>&1 || true\n\
         done\n\
         \n\
         update-desktop-database \"${{apps}}\" >/dev/null 2>&1 || true\n\
         printf '%s\\n' '{LINUX_HANDOVER_READY}'\n",
        session_env = shell_quote(&format!(
            "{}/session.env",
            crate::provider::GUEST_ROOT_LINUX
        )),
        launcher = shell_quote(&linux_launcher_path()),
        launcher_script = linux_launcher_script(paths),
        app_entry_name = APP_ENTRY,
        app_entry = app_entry(),
        folder_entry_name = FOLDER_ENTRY,
        folder_entry = folder_entry(),
    )
}

/// The command that runs a script in the guest without quoting it twice.
///
/// The Linux counterpart of [`powershell_command`], and there for the same
/// reason: the script has quotes, newlines, `$` and heredocs in it, and every
/// one of them would have to survive both the local command line and the guest's
/// login shell. Base64 has none of those characters in it.
pub fn bash_command(script: &str) -> String {
    format!("printf %s '{}' | base64 -d | bash -s", encode_text(script))
}

/// Put the launcher and the shortcuts in the guest.
///
/// The caller treats a failure as a warning: a guest that staged its binaries
/// is worth having even without somewhere convenient to click, and the reason
/// the shortcuts are here at all is convenience.
pub fn prepare(
    provider: &dyn Provider,
    state: &RunState,
    store: &Store,
    image: Image,
    paths: &GuestPaths,
) -> Result<(), String> {
    match image.target() {
        Target::Windows => prepare_windows(provider, state, store, image, paths),
        Target::Linux => prepare_linux(provider, state, paths),
    }
}

/// The Linux half: one script, and a marker to prove it ran all the way through.
fn prepare_linux(
    provider: &dyn Provider,
    state: &RunState,
    paths: &GuestPaths,
) -> Result<(), String> {
    let out = provider.exec(state, &bash_command(&linux_install_script(paths)))?;
    if !out.stdout.contains(LINUX_HANDOVER_READY) {
        return Err(format!(
            "the guest would not take the launcher and the desktop entries: {}{}",
            out.stdout.trim(),
            out.stderr.trim()
        ));
    }
    Ok(())
}

fn prepare_windows(
    provider: &dyn Provider,
    state: &RunState,
    store: &Store,
    image: Image,
    paths: &GuestPaths,
) -> Result<(), String> {
    let scratch = store.handover_scratch(image);
    std::fs::create_dir_all(&scratch)
        .map_err(|e| format!("cannot create {}: {e}", scratch.display()))?;
    let local = scratch.join(LAUNCHER);
    std::fs::write(&local, launcher_script(paths))
        .map_err(|e| format!("cannot write the launcher: {e}"))?;
    provider.copy_in(state, &local, &launcher_path())?;

    let out = provider.exec(state, &powershell_command(&shortcut_script(paths)))?;
    if !out.success() {
        return Err(format!(
            "the guest would not write the desktop shortcuts: {}",
            out.stderr.trim()
        ));
    }
    Ok(())
}

/// The marker the enhanced-session script prints when it got all the way
/// through, because `powershell.exe -EncodedCommand` will not carry that in an
/// exit code any more reliably here than it does on the host.
pub const ENHANCED_READY: &str = "ENHANCED=ready";

/// What a guest needs before `vmconnect` can open an enhanced session into it
/// without anyone typing a password.
///
/// An enhanced session is RDP over `VMBus`, and RDP wants credentials. The
/// three things in here are what turn that into a dismissable dialog: an
/// account with no password to type, the LSA policy that otherwise confines a
/// blank-password account to the physical console, and the service that answers
/// the connection at all. The guest is a throwaway with no secrets, reachable
/// from its own host and nowhere else, and its password was already in this
/// repository in plain text (unattend decision 6), so what this gives up is a
/// formality. What it buys is the thing a basic session cannot do at all: a
/// window that can be resized, with the guest's desktop resizing to match.
///
/// Not part of the boot, and not part of staging. A guest running a test suite
/// must not offer this: connecting takes the console session over, which is
/// where the windowed tests have their desktop. So it is done at the two points
/// where a guest is handed to a person and nothing of ours is running in it,
/// `vm up` and `e2e --keep`, and a guest in the middle of a run is left as it
/// was, offering the basic session that is safe to watch.
pub fn enhanced_session_script(user: &str) -> String {
    format!(
        "$ErrorActionPreference = 'Stop'\n\
         Set-LocalUser -Name {user} -Password (New-Object System.Security.SecureString)\n\
         New-ItemProperty -Path 'HKLM:\\SYSTEM\\CurrentControlSet\\Control\\Lsa' \
         -Name 'LimitBlankPasswordUse' -PropertyType DWord -Value 0 -Force | Out-Null\n\
         Set-Service -Name 'TermService' -StartupType Manual\n\
         Start-Service -Name 'TermService'\n\
         Write-Output '{ENHANCED_READY}'\n",
        user = crate::runner::ps_quote(user),
    )
}

/// Turn it on in a guest that is being handed over.
///
/// Answers whether the guest ends up offering an enhanced session, which is not
/// the same question as whether this succeeded: a target that has no such
/// session to offer answers `Ok(false)` without asking the guest anything. Only
/// Windows guests are looked at through `vmconnect`, and the record this answer
/// is written into says what was done to the guest rather than what was tried,
/// so a Linux guest reporting itself handed over would be a false claim about a
/// blank password and a `tester` account it does not have.
///
/// A failure is the caller's to report and not to fail on: what is lost is a
/// resizable window, and the basic session still shows a desktop that is
/// already signed in.
/// Whether a guest has an enhanced session to offer at all.
///
/// Two conditions, and both are about what the console is rather than about
/// this guest: it is `vmconnect`'s feature, so the hypervisor has to be
/// Hyper-V, and only a Windows guest has one. The same Windows image under QEMU
/// is looked at over VNC, which asks for nothing and cannot be resized, so
/// enabling RDP in it would change the guest for a console nobody on that host
/// can open.
pub fn offers_enhanced_session(kind: ProviderKind, target: Target) -> bool {
    target == Target::Windows && kind == ProviderKind::HyperV
}

pub fn enable_enhanced_session(
    provider: &dyn Provider,
    state: &RunState,
    target: Target,
) -> Result<bool, String> {
    if !offers_enhanced_session(provider.kind(), target) {
        return Ok(false);
    }
    let script = enhanced_session_script(crate::provider::hyperv::GUEST_USER);
    let out = provider.exec(state, &powershell_command(&script))?;
    if !out.stdout.contains(ENHANCED_READY) {
        return Err(format!(
            "the guest would not make an enhanced session possible: {}{}",
            out.stdout.trim(),
            out.stderr.trim()
        ));
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(textures: bool) -> GuestPaths {
        crate::guest::artifacts::guest_paths(
            Target::Windows,
            "sunlit-earth.exe",
            "e2e-1a2b.exe",
            textures,
        )
    }

    /// A Windows guest on a Linux host is reached over VNC, and running the
    /// enhanced-session script in it would blank an account's password and
    /// start Remote Desktop Services for a console that host cannot open.
    #[test]
    fn only_a_hyper_v_windows_guest_offers_an_enhanced_session() {
        assert!(offers_enhanced_session(
            ProviderKind::HyperV,
            Target::Windows
        ));
        assert!(!offers_enhanced_session(
            ProviderKind::Qemu,
            Target::Windows
        ));
        assert!(!offers_enhanced_session(
            ProviderKind::HyperV,
            Target::Linux
        ));
        assert!(!offers_enhanced_session(ProviderKind::Qemu, Target::Linux));
    }

    /// The launcher exists for this one line, and it has to be the same line the
    /// generated job sets: a guest where the harness works and a hand-started
    /// app does not is a guest that cannot be debugged in.
    #[test]
    fn the_launcher_sets_the_backend_the_job_sets() {
        let script = launcher_script(&paths(true));
        assert!(
            script.contains(&format!("set SLINT_BACKEND={WINDOWS_SLINT_BACKEND}")),
            "{script}"
        );
        assert!(
            script.contains(r"C:\sunlit-e2e\bin\sunlit-earth.exe"),
            "{script}"
        );
    }

    /// Same rule as the job script: name the textures directory when there is
    /// one, and say nothing when there is not, rather than pointing the app at
    /// a directory the guest does not have.
    #[test]
    fn the_launcher_names_the_textures_only_when_they_were_staged() {
        assert!(
            launcher_script(&paths(true))
                .contains(r"set SUNLIT_EARTH_TEXTURES=C:\sunlit-e2e\textures"),
            "with textures"
        );
        assert!(
            !launcher_script(&paths(false)).contains("SUNLIT_EARTH_TEXTURES"),
            "without textures"
        );
    }

    /// `cmd.exe` wants CRLF, and a batch file with bare newlines fails in ways
    /// that read as a broken command rather than a broken file.
    #[test]
    fn the_launcher_is_a_crlf_batch_file() {
        let script = launcher_script(&paths(true));
        assert!(script.starts_with("@echo off\r\n"), "{script}");
        assert!(!script.replace("\r\n", "").contains('\n'), "{script}");
    }

    /// The exit code is read into a variable before anything else runs, because
    /// every later statement would overwrite `%ERRORLEVEL%`.
    #[test]
    fn the_launcher_keeps_the_exit_code() {
        let script = launcher_script(&paths(true));
        let run = script.find("\" %*\r\n").expect("the app is started");
        let capture = script
            .find("set code=%ERRORLEVEL%")
            .expect("the code is captured");
        assert!(capture > run, "{script}");
        assert!(script.contains("exit /b %code%"), "{script}");
    }

    /// The app shortcut points at the launcher, since the binary on its own is
    /// exactly what does not work in here.
    #[test]
    fn the_app_shortcut_points_at_the_launcher_and_the_folder_at_the_root() {
        let script = shortcut_script(&paths(true));
        assert!(
            script.contains(&format!("$app.TargetPath = '{}'", launcher_path())),
            "{script}"
        );
        assert!(
            script.contains(&format!(
                "$folder.TargetPath = '{}'",
                crate::provider::GUEST_ROOT_WINDOWS
            )),
            "{script}"
        );
        assert!(script.contains(APP_SHORTCUT), "{script}");
        assert!(script.contains(FOLDER_SHORTCUT), "{script}");
    }

    /// One token, so nothing in the script has to survive `cmd.exe` twice.
    #[test]
    fn the_guest_command_carries_the_script_as_one_argument() {
        let command = powershell_command(&shortcut_script(&paths(true)));
        assert!(command.contains("-EncodedCommand "), "{command}");
        let encoded = command.rsplit(' ').next().unwrap_or_default();
        assert!(!encoded.is_empty());
        assert!(
            encoded
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '='),
            "{encoded}"
        );
    }

    /// The three things that turn the enhanced session's credential dialog into
    /// something to dismiss rather than fill in.
    #[test]
    fn the_enhanced_session_script_removes_the_password_the_dialog_would_want() {
        let script = enhanced_session_script("tester");
        // An account with nothing to type.
        assert!(
            script.contains("Set-LocalUser -Name 'tester' -Password"),
            "{script}"
        );
        assert!(script.contains("SecureString"), "{script}");
        // The policy that otherwise confines a blank password to the console.
        assert!(script.contains("LimitBlankPasswordUse"), "{script}");
        assert!(script.contains("-Value 0"), "{script}");
        // And the service that answers the connection.
        assert!(
            script.contains("Start-Service -Name 'TermService'"),
            "{script}"
        );
        assert!(script.contains(ENHANCED_READY), "{script}");
    }

    /// The account is the one the image creates, from the constant that creates
    /// it: a guest whose account was renamed would otherwise be left with a
    /// password nobody can type and no way in.
    #[test]
    fn the_enhanced_session_script_names_the_guest_account() {
        let script = enhanced_session_script(crate::provider::hyperv::GUEST_USER);
        assert!(
            script.contains(crate::provider::hyperv::GUEST_USER),
            "{script}"
        );
    }

    fn linux_paths(textures: bool) -> GuestPaths {
        crate::guest::artifacts::guest_paths(Target::Linux, "sunlit-earth", "e2e-1a2b", textures)
    }

    /// The Linux launcher exists for the opposite reason to the Windows one:
    /// there is no backend to set, because Mesa answers GL in the guest, and
    /// setting one anyway would be the app running differently there than it
    /// does under the suite.
    #[test]
    fn the_linux_launcher_sets_no_backend_and_execs_the_staged_binary() {
        let script = linux_launcher_script(&linux_paths(true));
        assert!(!script.contains("SLINT_BACKEND"), "{script}");
        assert!(
            script.contains("exec \"${app}\" \"$@\""),
            "the app is what it ends in: {script}"
        );
        assert!(
            script.contains("app='/var/lib/sunlit-e2e/bin/sunlit-earth'"),
            "{script}"
        );
        assert!(script.starts_with("#!/usr/bin/env bash\n"), "{script}");
        // LF, because a shell script with CRLF fails on its shebang.
        assert!(!script.contains('\r'), "{script}");
    }

    /// Same rule as the job script and the Windows launcher: name the textures
    /// directory when this boot staged one, and say nothing when it did not.
    #[test]
    fn the_linux_launcher_names_the_textures_only_when_they_were_staged() {
        assert!(
            linux_launcher_script(&linux_paths(true))
                .contains("export SUNLIT_EARTH_TEXTURES='/var/lib/sunlit-e2e/textures'"),
            "with textures"
        );
        assert!(
            !linux_launcher_script(&linux_paths(false)).contains("SUNLIT_EARTH_TEXTURES"),
            "without textures"
        );
    }

    /// Clicked from an icon there is no terminal, so a failure would be
    /// invisible; run from a shell a log file would hide what the terminal was
    /// about to show. The launcher decides which of the two it is in.
    #[test]
    fn the_linux_launcher_keeps_its_output_where_whoever_started_it_can_read_it() {
        let script = linux_launcher_script(&linux_paths(true));
        assert!(script.contains("if [ ! -t 1 ]; then"), "{script}");
        assert!(
            script.contains("exec >>'/var/lib/sunlit-e2e/run-app.log' 2>&1"),
            "{script}"
        );
        // And the redirect is decided before anything that could fail runs.
        let redirect = script.find("-t 1").expect("the terminal test");
        let missing = script.find("is not here").expect("the missing-app branch");
        assert!(redirect < missing, "{script}");
    }

    /// A desktop entry is run with whatever working directory the session had,
    /// so every path in one has to be absolute, and the app is reached through
    /// the launcher rather than directly for the same reason it is on Windows.
    #[test]
    fn both_desktop_entries_are_absolute_and_declare_themselves() {
        for entry in [app_entry(), folder_entry()] {
            assert!(entry.starts_with("[Desktop Entry]\n"), "{entry}");
            assert!(entry.contains("Type=Application"), "{entry}");
            assert!(entry.contains("Terminal=false"), "{entry}");
            let exec = entry
                .lines()
                .find_map(|line| line.strip_prefix("Exec="))
                .expect("an Exec line");
            assert!(
                exec.split_whitespace()
                    .last()
                    .is_some_and(|arg| arg.starts_with('/')),
                "{exec}"
            );
        }
        assert!(app_entry().contains(&format!("Exec={}", linux_launcher_path())));
        assert!(app_entry().contains(&format!("Name={ENTRY_NAME}")));
        // The same offer in both guests is the same name in both guests: what a
        // Windows desktop shows and what a Linux menu shows come from one
        // constant and one directory name, so a rename on either side is this
        // assertion rather than two guests that disagree.
        assert!(APP_SHORTCUT.starts_with(ENTRY_NAME), "{APP_SHORTCUT}");
        assert_eq!(
            FOLDER_SHORTCUT.trim_end_matches(".lnk"),
            FOLDER_ENTRY.trim_end_matches(".desktop")
        );
        // The folder entry is an application entry running `xdg-open` rather
        // than a `Type=Link`: a menu shows nothing but application entries, and
        // GNOME's menu is the whole hand-over there.
        assert!(
            folder_entry().contains(&format!(
                "Exec=xdg-open {}",
                crate::provider::GUEST_ROOT_LINUX
            )),
            "{}",
            folder_entry()
        );
    }

    /// Every one of the three files the script writes ends up somewhere the
    /// desktop looks, under a name that is the same in both places.
    #[test]
    fn the_install_script_writes_the_launcher_and_both_entries_where_each_is_found() {
        let script = linux_install_script(&linux_paths(true));
        assert!(script.contains("cat > \"${launcher}\""), "{script}");
        assert!(
            script.contains("apps=\"${HOME}/.local/share/applications\""),
            "{script}"
        );
        for entry in [APP_ENTRY, FOLDER_ENTRY] {
            assert!(
                script.contains(&format!("cat > \"${{apps}}/{entry}\"")),
                "{entry}"
            );
        }
        // Both names go through the one loop that copies onto the desktop, so
        // the menu copy and the desktop copy cannot end up named differently.
        assert!(
            script.contains(&format!("for entry in {APP_ENTRY} {FOLDER_ENTRY}; do")),
            "{script}"
        );
        assert!(
            script.contains("cp -f \"${apps}/${entry}\" \"${desktop}/${entry}\""),
            "{script}"
        );
        // The desktop directory is asked for, and the answer that means "no
        // user-dirs at all" is not treated as a desktop.
        assert!(script.contains("xdg-user-dir DESKTOP"), "{script}");
        assert!(
            script.contains("[ \"${desktop}\" = \"${HOME}\" ]"),
            "{script}"
        );
        // Nothing in it needs root: the account it runs as owns both the home
        // directory and the guest root.
        assert!(!script.contains("sudo"), "{script}");
    }

    /// Both blessings, and the fact that neither may fail the hand-over: a
    /// desktop that ignores one is the normal case rather than an error. The
    /// executable bit is what Plasma and Nemo ask for, and
    /// Both entries run the launcher as their `Exec`, so a launcher written
    /// and never made executable fails every activation with a permission
    /// error, and nothing else in the suite would notice: the harness never
    /// clicks an icon.
    #[test]
    fn the_install_script_makes_the_launcher_executable() {
        let script = linux_install_script(&linux_paths(true));
        assert!(script.contains("chmod 0755 \"${launcher}\""), "{script}");
        // After the write, not before it: `cat >` creates the file whose mode
        // is being set.
        let written = script.find("cat > \"${launcher}\"").expect("the write");
        let blessed = script
            .find("chmod 0755 \"${launcher}\"")
            .expect("the chmod");
        assert!(written < blessed, "{script}");
    }

    /// `metadata::xfce-exe-checksum`, the file's own sha256, is what xfdesktop
    /// asks for on top of it.
    ///
    /// The gio half needs the session bus, which a process started over SSH does
    /// not have: measured in the guest, `gio set` without one answers "Setting
    /// attribute ... not supported" and with one writes the attribute. So the
    /// script sources the environment the session wrote out at logon, and that
    /// file's name is pinned against the script that writes it, since nothing
    /// else connects the two.
    #[test]
    fn the_desktop_entries_are_blessed_the_way_each_desktop_asks_for() {
        let script = linux_install_script(&linux_paths(true));
        assert!(
            script.contains("chmod 0755 \"${desktop}/${entry}\""),
            "{script}"
        );
        // xfdesktop compares the mark against the file it is on, so the value
        // has to be computed from that file rather than written as a constant.
        assert!(
            script.contains("metadata::xfce-exe-checksum")
                && script.contains("sha256sum \"${desktop}/${entry}\""),
            "{script}"
        );
        let blessings: Vec<&str> = script
            .lines()
            .filter(|line| line.contains("gio set"))
            .collect();
        assert_eq!(blessings.len(), 1, "{blessings:?}");
        for line in blessings {
            assert!(
                line.trim_end().ends_with("|| true"),
                "a blessing that can fail the hand-over: {line}"
            );
        }

        let session_env = format!("{}/session.env", crate::provider::GUEST_ROOT_LINUX);
        assert!(script.contains(&format!(". '{session_env}'")), "{script}");
        let sourced = script.find(". '").expect("the source line");
        let blessing = script.find("gio set").expect("the gio line");
        assert!(sourced < blessing, "{script}");

        let contract = std::fs::read_to_string(
            crate::store::repo_root().join("vm/linux/scripts/guest-contract.sh"),
        )
        .expect("the Linux guest contract script");
        assert!(
            contract.contains("${root}/session.env"),
            "the guest no longer writes the environment this sources"
        );
        assert!(
            contract.contains("DBUS_SESSION_BUS_ADDRESS"),
            "the environment it writes no longer carries the session bus"
        );
    }

    /// One token again, and for the same reason the Windows side has one: the
    /// script has quotes, newlines and heredocs in it, and it has to survive
    /// both a local command line and the guest's login shell.
    #[test]
    fn the_linux_guest_command_carries_the_script_base64_encoded() {
        let command = bash_command(&linux_install_script(&linux_paths(true)));
        assert!(command.contains("base64 -d | bash -s"), "{command}");
        let encoded = command
            .split('\'')
            .nth(1)
            .expect("the payload is the quoted part");
        assert!(!encoded.is_empty());
        assert!(
            encoded
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '='),
            "{encoded}"
        );
    }

    /// A Linux guest is asked exactly once, and the answer is the marker rather
    /// than the exit code: the shell that runs this is the guest's login shell,
    /// and what a failed heredoc leaves behind is a zero exit and no files.
    #[test]
    fn a_linux_guest_is_asked_for_the_launcher_and_the_entries() {
        let store = Store::new(r"C:\vm store");
        let runner = crate::runner::fake::FakeRunner::default().on(
            "base64 -d",
            crate::runner::CommandOutput {
                code: Some(0),
                stdout: format!("{LINUX_HANDOVER_READY}\n"),
                stderr: String::new(),
            },
        );
        let provider = crate::provider::qemu::QemuProvider::new(
            &runner,
            &store,
            crate::provider::target::HostOs::Windows,
        );
        let state = crate::store::state::RunState::new(
            Image::Linux,
            crate::provider::target::ProviderKind::Qemu,
            std::path::PathBuf::from("/tmp/overlay.qcow2"),
            crate::store::state::StartReason::Up,
            0,
        );
        assert_eq!(
            prepare(&provider, &state, &store, Image::Linux, &linux_paths(true)),
            Ok(())
        );
        assert_eq!(runner.calls().len(), 1, "{:?}", runner.calls());

        // And a guest that answered without the marker is a failure to report,
        // not a hand-over to claim.
        let silent = crate::runner::fake::FakeRunner::default().on(
            "base64 -d",
            crate::runner::CommandOutput {
                code: Some(0),
                stdout: String::new(),
                stderr: "bash: xdg-user-dir: not found".to_owned(),
            },
        );
        let provider = crate::provider::qemu::QemuProvider::new(
            &silent,
            &store,
            crate::provider::target::HostOs::Windows,
        );
        let err = prepare(&provider, &state, &store, Image::Linux, &linux_paths(true))
            .expect_err("no marker is no hand-over");
        assert!(err.contains("xdg-user-dir"), "{err}");
    }

    /// A Linux guest is looked at through VNC, so there is no enhanced session
    /// to turn on and nothing to report as turned on. The answer is what
    /// `RunState::handed_over` records, and a guest recorded as handed over is
    /// promised a blank password for an account it does not have: under
    /// `SUNLIT_EARTH_VM_PROVIDER=hyperv` that promise reaches a reader.
    ///
    /// The fake runner has no response registered, so it also proves the guest
    /// was not asked: any command at all would come back as an error here.
    #[test]
    fn a_linux_guest_is_not_recorded_as_offering_a_session_it_has_no_way_to_offer() {
        let store = Store::new(r"C:\vm store");
        let runner = crate::runner::fake::FakeRunner::default();
        let provider = crate::provider::qemu::QemuProvider::new(
            &runner,
            &store,
            crate::provider::target::HostOs::Windows,
        );
        let state = crate::store::state::RunState::new(
            Image::Linux,
            crate::provider::target::ProviderKind::Qemu,
            std::path::PathBuf::from("/tmp/overlay.qcow2"),
            crate::store::state::StartReason::Up,
            0,
        );
        assert_eq!(
            enable_enhanced_session(&provider, &state, Target::Linux),
            Ok(false)
        );
        assert!(runner.calls().is_empty(), "{:?}", runner.calls());
    }
}
