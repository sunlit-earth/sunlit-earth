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
//! Windows only. The Linux guest needs no backend override, because Mesa
//! answers there, and its root is not a place a desktop file would point at.

use crate::commands::e2e::WINDOWS_SLINT_BACKEND;
use crate::guest::artifacts::GuestPaths;
use crate::provider::Provider;
use crate::provider::target::Target;
use crate::runner::encode_command;
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

/// Put the launcher and the shortcuts in the guest.
///
/// The caller treats a failure as a warning: a guest that staged its binaries
/// is worth having even without somewhere convenient to click, and the reason
/// the shortcuts are here at all is convenience.
pub fn prepare(
    provider: &dyn Provider,
    state: &RunState,
    store: &Store,
    target: Target,
    paths: &GuestPaths,
) -> Result<(), String> {
    if target != Target::Windows {
        return Ok(());
    }
    let scratch = store.run_dir(target).join("handover");
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
pub fn enable_enhanced_session(
    provider: &dyn Provider,
    state: &RunState,
    target: Target,
) -> Result<bool, String> {
    if target != Target::Windows {
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

    /// Nothing is written into a Linux guest, which has neither the backend
    /// problem nor a desktop to put a shortcut on.
    #[test]
    fn a_linux_guest_gets_nothing() {
        let store = Store::new(r"C:\vm store");
        let runner = crate::runner::fake::FakeRunner::default();
        let provider = crate::provider::hyperv::HypervProvider::new(
            &runner,
            &store,
            crate::provider::target::HostOs::Windows,
        );
        let state = crate::store::state::RunState::new(
            Target::Linux,
            crate::provider::target::ProviderKind::Qemu,
            std::path::PathBuf::from("/tmp/overlay.qcow2"),
            crate::store::state::StartReason::Up,
            0,
        );
        assert_eq!(
            prepare(&provider, &state, &store, Target::Linux, &paths(true)),
            Ok(())
        );
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
            Target::Linux,
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
