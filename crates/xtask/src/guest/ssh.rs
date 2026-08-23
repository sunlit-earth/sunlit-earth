//! Reaching a guest: the OpenSSH client, driven as a process.
//!
//! Both providers use the same client and the same options, so the argument
//! lists live here rather than in either of them. They are built by pure
//! functions, which is the only way to check them without a running VM.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::runner::{Cmd, CommandOutput, Runner};
use crate::store::state::RunState;

/// How often to retry while waiting.
pub const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Where a guest is and how to authenticate to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshTarget {
    pub user: String,
    pub host: String,
    pub port: u16,
    pub key: PathBuf,
}

impl SshTarget {
    pub fn from_state(state: &RunState, key: impl Into<PathBuf>) -> Self {
        Self {
            user: state.ssh_user.clone(),
            host: state.ssh_host.clone(),
            port: state.ssh_port,
            key: key.into(),
        }
    }

    pub fn destination(&self) -> String {
        format!("{}@{}", self.user, self.host)
    }
}

/// Where the host keys these connections collect are kept: beside the key pair
/// the guests trust, inside the image store.
///
/// Not the null device, which cannot be named portably from here. `NUL` is the
/// null device to Windows' own OpenSSH client and an ordinary file name to the
/// MSYS2 build that ships with Git for Windows, which is what `ssh` resolves to
/// on a Windows host as often as not; that client took
/// `UserKnownHostsFile=NUL` literally and left a file called `NUL` in whatever
/// directory the xtask was run from, which is the repository root, and one that
/// `cmd` cannot delete without the `\\?\` prefix. `/dev/null` has the mirror
/// problem on the other client.
///
/// A real file in the store is a path both clients understand, and it keeps the
/// property the option is there for: the developer's own `~/.ssh/known_hosts` is
/// never written to.
///
/// `ssh` does read it, which is worth being clear about.
/// `StrictHostKeyChecking=no` accepts a key it has never seen without asking,
/// and says nothing about a key it has seen *change*: that still prints the
/// remote-host-identification-changed warning, on every connection, while
/// public-key authentication carries on working. A guest's host keys are
/// generated during the image build and live on the golden disk, so every
/// throwaway overlay of one image answers with the same key and the entries here
/// stay right for the life of that image. A rebuild is what changes them, and
/// `build_image::forget_host_keys` deletes this file at the end of one for
/// exactly that reason.
pub fn known_hosts(key: &Path) -> PathBuf {
    key.with_file_name("known_hosts")
}

/// The known-hosts path as the option value `ssh` parses.
///
/// `UserKnownHostsFile` takes a whitespace-separated list of files, so a store
/// under a directory with a space in its name would otherwise arrive as two
/// paths, neither of them real. `ssh` accepts a double-quoted argument for
/// exactly this, and the quotes are added only when they are needed so that the
/// ordinary case is the plain path it looks like.
pub fn known_hosts_option(key: &Path) -> String {
    let path = known_hosts(key);
    let path = path.to_string_lossy();
    if path.contains(' ') {
        format!("UserKnownHostsFile=\"{path}\"")
    } else {
        format!("UserKnownHostsFile={path}")
    }
}

/// The options every connection uses.
///
/// Host-key checking is off and the known-hosts file is one of the xtask's own
/// because every run boots a fresh overlay whose host keys are generated on
/// first boot: the same address legitimately has a different key every time, so
/// a known-hosts entry would be a guaranteed false alarm rather than a
/// protection. The guest is reachable only from this host's loopback.
pub fn common_options(key: &Path) -> Vec<String> {
    vec![
        "-o".to_owned(),
        "StrictHostKeyChecking=no".to_owned(),
        "-o".to_owned(),
        known_hosts_option(key),
        // Never fall back to a password or a passphrase prompt: a hung
        // orchestrator waiting on invisible input is the worst failure here.
        "-o".to_owned(),
        "BatchMode=yes".to_owned(),
        "-o".to_owned(),
        "IdentitiesOnly=yes".to_owned(),
        "-o".to_owned(),
        "ConnectTimeout=10".to_owned(),
        "-o".to_owned(),
        "LogLevel=ERROR".to_owned(),
        "-i".to_owned(),
        key.to_string_lossy().into_owned(),
    ]
}

/// `ssh [options] -p port user@host [command]`.
pub fn ssh_command(target: &SshTarget, remote: Option<&str>) -> Cmd {
    let mut args = common_options(&target.key);
    args.push("-p".to_owned());
    args.push(target.port.to_string());
    args.push(target.destination());
    if let Some(command) = remote {
        args.push(command.to_owned());
    }
    Cmd::new("ssh").args(args)
}

/// Normalize a remote path for `scp`.
///
/// The remote half of an `scp` argument is handled by a shell on the far side,
/// and on a Windows guest that shell is `cmd.exe`. Backslashes survive some
/// paths through it and not others. Windows accepts forward slashes in a path
/// everywhere it accepts backslashes, and a Linux guest path has no
/// backslashes to convert, so this is safe in both directions and removes the
/// question. Paths that go into a command the guest runs keep their native
/// separators; only the `scp` argument is converted.
pub fn scp_remote_path(remote: &str) -> String {
    remote.replace('\\', "/")
}

/// `scp [options] -P port <local> user@host:<remote>`.
///
/// `scp` spells the port `-P` where `ssh` spells it `-p`, which is the kind of
/// detail a unit test is for.
pub fn scp_to_command(target: &SshTarget, local: &Path, remote: &str) -> Cmd {
    let mut args = common_options(&target.key);
    args.push("-P".to_owned());
    args.push(target.port.to_string());
    args.push("-r".to_owned());
    args.push(local.to_string_lossy().into_owned());
    args.push(format!(
        "{}:{}",
        target.destination(),
        scp_remote_path(remote)
    ));
    Cmd::new("scp").args(args)
}

/// `scp [options] -P port user@host:<remote> <local>`.
pub fn scp_from_command(target: &SshTarget, remote: &str, local: &Path) -> Cmd {
    let mut args = common_options(&target.key);
    args.push("-P".to_owned());
    args.push(target.port.to_string());
    args.push("-r".to_owned());
    args.push(format!(
        "{}:{}",
        target.destination(),
        scp_remote_path(remote)
    ));
    args.push(local.to_string_lossy().into_owned());
    Cmd::new("scp").args(args)
}

/// Run one command in the guest.
pub fn exec(
    runner: &dyn Runner,
    target: &SshTarget,
    command: &str,
) -> Result<CommandOutput, String> {
    runner
        .capture(&ssh_command(target, Some(command)))
        .map_err(|e| format!("cannot run ssh: {e}"))
}

/// What the readiness probe asks the guest to say back.
pub const READY_MARKER: &str = "sunlit-e2e-ssh-ready";

/// One readiness probe: can the guest run a command right now?
///
/// A real command rather than a port check: an SSH server that accepts
/// connections before the account is usable is a real state, and a successful
/// `echo` is the first moment the guest can actually do anything.
///
/// The error carries the reason so a caller that gives up can say what the last
/// attempt looked like, which is the difference between "refused" and "timed
/// out" and between either of those and a rejected key.
pub fn probe_ready(runner: &dyn Runner, target: &SshTarget) -> Result<(), String> {
    match exec(runner, target, &format!("echo {READY_MARKER}")) {
        Ok(out) if out.stdout.contains(READY_MARKER) => Ok(()),
        Ok(out) => Err(format!("exit {:?}: {}", out.code, out.stderr.trim())),
        Err(e) => Err(e),
    }
}

/// Block until the guest answers, or give up.
///
/// `dead` is consulted after every failed probe, and a `Some` from it ends the
/// wait at once: a guest whose process has already exited turns the rest of
/// the timeout into dead time, and its reason is better told the moment it is
/// known. Asked only after a failure, so a guest that answers is never
/// second-guessed by a stale process table.
pub fn wait_ready(
    runner: &dyn Runner,
    target: &SshTarget,
    timeout: Duration,
    poll: Duration,
    dead: &dyn Fn() -> Option<String>,
) -> Result<Duration, String> {
    let start = Instant::now();
    let mut last;
    loop {
        match probe_ready(runner, target) {
            Ok(()) => return Ok(start.elapsed()),
            Err(e) => last = e,
        }
        if let Some(reason) = dead() {
            return Err(format!("the guest is not coming up: {reason}"));
        }
        if start.elapsed() >= timeout {
            return Err(format!(
                "the guest did not answer on {}:{} within {:.0}s; last attempt: {last}",
                target.host,
                target.port,
                timeout.as_secs_f64()
            ));
        }
        std::thread::sleep(poll);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::CommandOutput;
    use crate::runner::fake::FakeRunner;

    fn target() -> SshTarget {
        SshTarget {
            user: "tester".to_owned(),
            host: "127.0.0.1".to_owned(),
            port: 2222,
            key: PathBuf::from("/srv/vm/ssh/id_ed25519"),
        }
    }

    #[test]
    fn the_known_hosts_file_sits_beside_the_key_and_never_in_the_home_directory() {
        // `NUL` is what this used to be on a Windows host, and the MSYS2 ssh
        // that Git for Windows ships took it literally: the run left a file
        // called NUL in the directory the xtask was started from. A path in the
        // store is one both clients understand.
        // Joined rather than spelled out, because `with_file_name` uses the
        // host's own separator and this test runs on all three.
        let dir = Path::new("/srv/vm/ssh");
        let expected = dir.join("known_hosts");
        assert_eq!(known_hosts(&dir.join("id_ed25519")), expected);
        let options = common_options(&dir.join("id_ed25519"));
        assert!(
            options.contains(&format!("UserKnownHostsFile={}", expected.display())),
            "{options:?}"
        );
        for option in &options {
            assert!(
                !option.contains("NUL") && !option.contains("/dev/null"),
                "{option} names a null device only one of the two clients understands"
            );
        }
    }

    #[test]
    fn a_store_under_a_directory_with_a_space_is_quoted_for_ssh() {
        // UserKnownHostsFile takes a list of files separated by whitespace, so
        // an unquoted path with a space in it is two paths, neither of them
        // real. %LOCALAPPDATA% carries the account name, which can have one.
        let spaced = Path::new("/home/Ada Byron/.local/SunlitEarth/vm/ssh").join("id_ed25519");
        let option = known_hosts_option(&spaced);
        assert!(option.starts_with("UserKnownHostsFile=\""), "{option}");
        assert!(option.ends_with("known_hosts\""), "{option}");
        // The ordinary case stays the plain path it looks like.
        let plain = Path::new("/srv/vm/ssh").join("id_ed25519");
        assert_eq!(
            known_hosts_option(&plain),
            format!(
                "UserKnownHostsFile={}",
                Path::new("/srv/vm/ssh").join("known_hosts").display()
            )
        );
    }

    #[test]
    fn the_known_hosts_file_is_inside_the_image_store() {
        // Which is what makes it the xtask's own junk rather than the
        // developer's, and what a `vm purge` of the whole store would take with
        // it.
        let store = crate::store::Store::new("/srv/vm");
        assert!(store.contains(&known_hosts(&store.ssh_key())));
    }

    #[test]
    fn every_connection_refuses_to_prompt_for_anything() {
        let options = common_options(Path::new("/k"));
        assert!(options.contains(&"BatchMode=yes".to_owned()));
        assert!(options.contains(&"IdentitiesOnly=yes".to_owned()));
        assert!(options.contains(&"StrictHostKeyChecking=no".to_owned()));
    }

    #[test]
    fn the_ssh_command_puts_the_remote_command_last() {
        let cmd = ssh_command(&target(), Some("whoami"));
        assert_eq!(cmd.program, "ssh");
        assert_eq!(cmd.args.last().map(String::as_str), Some("whoami"));
        let destination = cmd.args.len() - 2;
        assert_eq!(cmd.args[destination], "tester@127.0.0.1");
        assert!(cmd.args.contains(&"-p".to_owned()));
        assert!(cmd.args.contains(&"2222".to_owned()));
    }

    #[test]
    fn an_interactive_session_passes_no_remote_command() {
        let cmd = ssh_command(&target(), None);
        assert_eq!(
            cmd.args.last().map(String::as_str),
            Some("tester@127.0.0.1")
        );
    }

    #[test]
    fn scp_spells_the_port_with_a_capital_p_in_both_directions() {
        let to = scp_to_command(&target(), Path::new("/tmp/app"), "/var/lib/sunlit-e2e/bin/");
        assert_eq!(to.program, "scp");
        assert!(to.args.contains(&"-P".to_owned()), "{:?}", to.args);
        assert!(!to.args.contains(&"-p".to_owned()), "{:?}", to.args);
        assert_eq!(
            to.args.last().map(String::as_str),
            Some("tester@127.0.0.1:/var/lib/sunlit-e2e/bin/")
        );

        let from = scp_from_command(
            &target(),
            "/var/lib/sunlit-e2e/results",
            Path::new("/tmp/out"),
        );
        assert!(from.args.contains(&"-P".to_owned()));
        assert_eq!(from.args.last().map(String::as_str), Some("/tmp/out"));
        let remote = from.args.len() - 2;
        assert_eq!(
            from.args[remote],
            "tester@127.0.0.1:/var/lib/sunlit-e2e/results"
        );
    }

    #[test]
    fn a_windows_guest_path_reaches_scp_with_forward_slashes() {
        // The remote half of an scp argument goes through cmd.exe on a Windows
        // guest, which does not handle backslashes there reliably.
        assert_eq!(scp_remote_path(r"C:\sunlit-e2e\bin"), "C:/sunlit-e2e/bin");
        assert_eq!(
            scp_remote_path("/var/lib/sunlit-e2e/results"),
            "/var/lib/sunlit-e2e/results"
        );

        let cmd = scp_to_command(&target(), Path::new("/tmp/app"), r"C:\sunlit-e2e\bin");
        assert_eq!(
            cmd.args.last().map(String::as_str),
            Some("tester@127.0.0.1:C:/sunlit-e2e/bin")
        );
    }

    #[test]
    fn copies_are_recursive_so_a_results_directory_comes_back_whole() {
        let from = scp_from_command(&target(), "r", Path::new("/tmp/out"));
        assert!(from.args.contains(&"-r".to_owned()));
    }

    #[test]
    fn waiting_succeeds_as_soon_as_the_guest_answers() {
        let runner = FakeRunner::new().on(
            "sunlit-e2e-ssh-ready",
            CommandOutput::ok("sunlit-e2e-ssh-ready\n"),
        );
        let elapsed = wait_ready(
            &runner,
            &target(),
            Duration::from_millis(50),
            Duration::from_millis(1),
            &|| None,
        )
        .expect("ready");
        assert!(elapsed < Duration::from_secs(1));
    }

    #[test]
    fn waiting_gives_up_with_the_last_error_rather_than_a_bare_timeout() {
        let runner = FakeRunner::new().on(
            "sunlit-e2e-ssh-ready",
            CommandOutput::failed(255, "Connection refused"),
        );
        let err = wait_ready(
            &runner,
            &target(),
            Duration::from_millis(5),
            Duration::from_millis(1),
            &|| None,
        )
        .unwrap_err();
        assert!(err.contains("127.0.0.1:2222"), "{err}");
        assert!(err.contains("Connection refused"), "{err}");
    }

    #[test]
    fn a_guest_reported_dead_ends_the_wait_with_the_reason() {
        let runner = FakeRunner::new().on(
            "sunlit-e2e-ssh-ready",
            CommandOutput::failed(255, "Connection refused"),
        );
        // An hour of timeout and an hour of poll: reaching the assertions at
        // all proves the wait ended on the verdict rather than on either.
        let err = wait_ready(
            &runner,
            &target(),
            Duration::from_secs(3600),
            Duration::from_secs(3600),
            &|| Some("qemu has exited".to_owned()),
        )
        .unwrap_err();
        assert!(err.contains("not coming up"), "{err}");
        assert!(err.contains("qemu has exited"), "{err}");
    }

    #[test]
    fn a_guest_that_answers_is_never_asked_whether_it_died() {
        // The probe settles it: a guest that answered is alive whatever a
        // stale process table would have said.
        let runner = FakeRunner::new().on(
            "sunlit-e2e-ssh-ready",
            CommandOutput::ok("sunlit-e2e-ssh-ready\n"),
        );
        wait_ready(
            &runner,
            &target(),
            Duration::from_millis(50),
            Duration::from_millis(1),
            &|| unreachable!("a successful probe settles it"),
        )
        .expect("ready");
    }
}
