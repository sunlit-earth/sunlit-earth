//! Reaching a guest: the OpenSSH client, driven as a process.
//!
//! Both providers use the same client and the same options, so the argument
//! lists live here rather than in either of them. They are built by pure
//! functions, which is the only way to check them without a running VM.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::runner::{Cmd, CommandOutput, Runner};
use crate::state::RunState;

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

/// The null device, which is where the known-hosts file goes.
pub fn null_device(windows_host: bool) -> &'static str {
    if windows_host { "NUL" } else { "/dev/null" }
}

/// The options every connection uses.
///
/// Host-key checking is off and the known-hosts file is the null device
/// because every run boots a fresh overlay whose host keys are generated on
/// first boot: the same address legitimately has a different key every time, so
/// a known-hosts entry would be a guaranteed false alarm rather than a
/// protection. The guest is reachable only from this host's loopback.
pub fn common_options(key: &Path, windows_host: bool) -> Vec<String> {
    vec![
        "-o".to_owned(),
        "StrictHostKeyChecking=no".to_owned(),
        "-o".to_owned(),
        format!("UserKnownHostsFile={}", null_device(windows_host)),
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
pub fn ssh_command(target: &SshTarget, remote: Option<&str>, windows_host: bool) -> Cmd {
    let mut args = common_options(&target.key, windows_host);
    args.push("-p".to_owned());
    args.push(target.port.to_string());
    args.push(target.destination());
    if let Some(command) = remote {
        args.push(command.to_owned());
    }
    Cmd::new("ssh").args(args)
}

/// `scp [options] -P port <local> user@host:<remote>`.
///
/// `scp` spells the port `-P` where `ssh` spells it `-p`, which is the kind of
/// detail a unit test is for.
pub fn scp_to_command(target: &SshTarget, local: &Path, remote: &str, windows_host: bool) -> Cmd {
    let mut args = common_options(&target.key, windows_host);
    args.push("-P".to_owned());
    args.push(target.port.to_string());
    args.push("-r".to_owned());
    args.push(local.to_string_lossy().into_owned());
    args.push(format!("{}:{remote}", target.destination()));
    Cmd::new("scp").args(args)
}

/// `scp [options] -P port user@host:<remote> <local>`.
pub fn scp_from_command(target: &SshTarget, remote: &str, local: &Path, windows_host: bool) -> Cmd {
    let mut args = common_options(&target.key, windows_host);
    args.push("-P".to_owned());
    args.push(target.port.to_string());
    args.push("-r".to_owned());
    args.push(format!("{}:{remote}", target.destination()));
    args.push(local.to_string_lossy().into_owned());
    Cmd::new("scp").args(args)
}

/// Run one command in the guest.
pub fn exec(
    runner: &dyn Runner,
    target: &SshTarget,
    command: &str,
    windows_host: bool,
) -> Result<CommandOutput, String> {
    runner
        .capture(&ssh_command(target, Some(command), windows_host))
        .map_err(|e| format!("cannot run ssh: {e}"))
}

/// Block until the guest answers, or give up.
///
/// The probe is a real command rather than a port check: an SSH server that
/// accepts connections before the account is usable is a real state, and a
/// successful `echo` is the first moment the guest can actually do anything.
pub fn wait_ready(
    runner: &dyn Runner,
    target: &SshTarget,
    timeout: Duration,
    poll: Duration,
    windows_host: bool,
) -> Result<Duration, String> {
    let start = Instant::now();
    let mut last;
    loop {
        match exec(runner, target, "echo sunlit-e2e-ssh-ready", windows_host) {
            Ok(out) if out.stdout.contains("sunlit-e2e-ssh-ready") => return Ok(start.elapsed()),
            Ok(out) => last = format!("exit {:?}: {}", out.code, out.stderr.trim()),
            Err(e) => last = e,
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
    fn the_known_hosts_file_is_the_platform_null_device() {
        assert_eq!(null_device(true), "NUL");
        assert_eq!(null_device(false), "/dev/null");
        let options = common_options(Path::new("/k"), true);
        assert!(
            options.contains(&"UserKnownHostsFile=NUL".to_owned()),
            "{options:?}"
        );
    }

    #[test]
    fn every_connection_refuses_to_prompt_for_anything() {
        let options = common_options(Path::new("/k"), false);
        assert!(options.contains(&"BatchMode=yes".to_owned()));
        assert!(options.contains(&"IdentitiesOnly=yes".to_owned()));
        assert!(options.contains(&"StrictHostKeyChecking=no".to_owned()));
    }

    #[test]
    fn the_ssh_command_puts_the_remote_command_last() {
        let cmd = ssh_command(&target(), Some("whoami"), false);
        assert_eq!(cmd.program, "ssh");
        assert_eq!(cmd.args.last().map(String::as_str), Some("whoami"));
        let destination = cmd.args.len() - 2;
        assert_eq!(cmd.args[destination], "tester@127.0.0.1");
        assert!(cmd.args.contains(&"-p".to_owned()));
        assert!(cmd.args.contains(&"2222".to_owned()));
    }

    #[test]
    fn an_interactive_session_passes_no_remote_command() {
        let cmd = ssh_command(&target(), None, false);
        assert_eq!(
            cmd.args.last().map(String::as_str),
            Some("tester@127.0.0.1")
        );
    }

    #[test]
    fn scp_spells_the_port_with_a_capital_p_in_both_directions() {
        let to = scp_to_command(&target(), Path::new("/tmp/app"), "sunlit-e2e/bin/", false);
        assert_eq!(to.program, "scp");
        assert!(to.args.contains(&"-P".to_owned()), "{:?}", to.args);
        assert!(!to.args.contains(&"-p".to_owned()), "{:?}", to.args);
        assert_eq!(
            to.args.last().map(String::as_str),
            Some("tester@127.0.0.1:sunlit-e2e/bin/")
        );

        let from = scp_from_command(
            &target(),
            "sunlit-e2e/results",
            Path::new("/tmp/out"),
            false,
        );
        assert!(from.args.contains(&"-P".to_owned()));
        assert_eq!(from.args.last().map(String::as_str), Some("/tmp/out"));
        let remote = from.args.len() - 2;
        assert_eq!(from.args[remote], "tester@127.0.0.1:sunlit-e2e/results");
    }

    #[test]
    fn copies_are_recursive_so_a_results_directory_comes_back_whole() {
        let from = scp_from_command(&target(), "r", Path::new("/tmp/out"), false);
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
            false,
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
            false,
        )
        .unwrap_err();
        assert!(err.contains("127.0.0.1:2222"), "{err}");
        assert!(err.contains("Connection refused"), "{err}");
    }
}
