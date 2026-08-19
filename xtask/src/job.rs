//! The job protocol, host side.
//!
//! Plan decision 6: the orchestrator drops a job into the guest, fires the
//! runner that the golden image already carries, and polls for `exit_code.txt`.
//! The file appearing is the end of the run; its contents are the result.
//!
//! Firing and waiting are separate on purpose. On Windows the job has to run in
//! the console session, which means a scheduled task rather than the SSH
//! session, so there is no process for SSH to wait on. Doing the same on Linux
//! keeps one protocol instead of two, and it means a dropped connection during
//! a twenty-minute suite costs nothing.

use std::path::Path;
use std::time::{Duration, Instant};

use crate::provider::Provider;
use crate::state::RunState;
use crate::target::Target;

/// How often to ask the guest whether the job has finished.
pub const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// The job script's name inside the guest.
pub fn job_file(target: Target) -> &'static str {
    match target {
        Target::Windows => "job.cmd",
        Target::Linux => "job.sh",
    }
}

/// Where the job script is copied to.
pub fn job_destination(target: Target) -> String {
    match target {
        Target::Windows => format!(r"{}\job.cmd", crate::provider::GUEST_ROOT_WINDOWS),
        Target::Linux => format!("{}/job.sh", crate::provider::GUEST_ROOT_LINUX),
    }
}

/// The command that starts the job and returns immediately.
///
/// On Windows this is the pre-registered scheduled task from the golden image,
/// which is the only way to reach the interactive desktop: a process started
/// over SSH lands in a non-interactive session where no window can appear. On
/// Linux the runner is detached from the SSH session so that closing the
/// connection does not take the job with it.
pub fn launch_command(target: Target) -> &'static str {
    match target {
        Target::Windows => r"schtasks /run /tn sunlit-e2e-job",
        Target::Linux => {
            "nohup setsid /usr/local/bin/sunlit-e2e-run-job </dev/null >/dev/null 2>&1 & echo started"
        }
    }
}

/// The command that reads the exit-code file, printing nothing if it is absent.
pub fn exit_code_command(target: Target) -> String {
    match target {
        Target::Windows => {
            // The Windows OpenSSH default shell is cmd.exe, and the golden
            // image leaves it that way.
            format!(
                r"if exist {0}\results\exit_code.txt type {0}\results\exit_code.txt",
                crate::provider::GUEST_ROOT_WINDOWS
            )
        }
        Target::Linux => format!(
            "cat {}/results/exit_code.txt 2>/dev/null || true",
            crate::provider::GUEST_ROOT_LINUX
        ),
    }
}

/// The command that reports whether the desktop session exists yet.
pub fn session_ready_command(target: Target) -> String {
    match target {
        Target::Windows => format!(
            r"if exist {}\ready echo SUNLIT_SESSION_READY",
            crate::provider::GUEST_ROOT_WINDOWS
        ),
        Target::Linux => format!(
            "test -f {}/ready && echo SUNLIT_SESSION_READY || true",
            crate::provider::GUEST_ROOT_LINUX
        ),
    }
}

/// The marker the session-ready probe looks for.
pub const SESSION_READY_MARKER: &str = "SUNLIT_SESSION_READY";

/// Read an exit-code file's contents.
///
/// Absent or still empty reads as "not finished"; anything else has to parse,
/// because a file with garbage in it is a broken guest rather than a pass.
pub fn parse_exit_code(text: &str) -> Result<Option<i32>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    trimmed
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .parse::<i32>()
        .map(Some)
        .map_err(|_| format!("the guest wrote an unreadable exit code: {trimmed:?}"))
}

/// Wait for the desktop session to exist.
pub fn wait_for_session(
    provider: &dyn Provider,
    state: &RunState,
    target: Target,
    timeout: Duration,
) -> Result<Duration, String> {
    let command = session_ready_command(target);
    let start = Instant::now();
    loop {
        if let Ok(out) = provider.exec(state, &command)
            && out.stdout.contains(SESSION_READY_MARKER)
        {
            return Ok(start.elapsed());
        }
        if start.elapsed() >= timeout {
            return Err(format!(
                "no desktop session in the guest after {:.0}s; the autologon did not \
                 complete. `cargo xtask vm view {target}` shows what it is doing.",
                timeout.as_secs_f64()
            ));
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Drop a job into the guest, start it, and wait for its exit code.
pub fn run(
    provider: &dyn Provider,
    state: &RunState,
    target: Target,
    script: &str,
    local_scratch: &Path,
    timeout: Duration,
) -> Result<i32, String> {
    std::fs::create_dir_all(local_scratch)
        .map_err(|e| format!("cannot create {}: {e}", local_scratch.display()))?;
    let local = local_scratch.join(job_file(target));
    // LF endings even for the Windows job: cmd.exe copes, and writing bytes
    // that depend on the host would make the guest's behavior depend on it too.
    std::fs::write(&local, script.replace("\r\n", "\n"))
        .map_err(|e| format!("cannot write the job script: {e}"))?;

    provider.copy_in(state, &local, &job_destination(target))?;

    let out = provider.exec(state, launch_command(target))?;
    if !out.success() {
        return Err(format!(
            "the guest would not start the job: exit {:?}: {}",
            out.code,
            out.stderr.trim()
        ));
    }

    wait_for_exit_code(provider, state, target, timeout)
}

/// Poll until `exit_code.txt` appears.
pub fn wait_for_exit_code(
    provider: &dyn Provider,
    state: &RunState,
    target: Target,
    timeout: Duration,
) -> Result<i32, String> {
    let command = exit_code_command(target);
    let start = Instant::now();
    loop {
        match provider.exec(state, &command) {
            Ok(out) => {
                if let Some(code) = parse_exit_code(&out.stdout)? {
                    return Ok(code);
                }
            }
            Err(e) => {
                // A single failed poll is not a failed run: the guest may be
                // busy enough to drop a connection.
                if start.elapsed() >= timeout {
                    return Err(e);
                }
            }
        }
        if start.elapsed() >= timeout {
            return Err(format!(
                "the job did not finish within {:.0}s. It may still be running: \
                 `cargo xtask vm ssh {target}` gets you in, and the results \
                 directory holds whatever it wrote.",
                timeout.as_secs_f64()
            ));
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_windows_job_goes_through_the_scheduled_task() {
        // A process started over SSH cannot touch the interactive desktop, so
        // this indirection is the whole reason the task exists.
        assert!(launch_command(Target::Windows).contains("schtasks /run"));
        assert!(launch_command(Target::Windows).contains("sunlit-e2e-job"));
    }

    #[test]
    fn the_linux_job_is_detached_from_the_ssh_session() {
        let command = launch_command(Target::Linux);
        assert!(command.contains("setsid"), "{command}");
        assert!(command.contains("</dev/null"), "{command}");
        assert!(command.trim_end().ends_with("echo started"), "{command}");
    }

    #[test]
    fn the_job_file_matches_the_guests_shell() {
        assert_eq!(job_file(Target::Windows), "job.cmd");
        assert_eq!(job_file(Target::Linux), "job.sh");
        assert_eq!(job_destination(Target::Windows), r"C:\sunlit-e2e\job.cmd");
        assert_eq!(job_destination(Target::Linux), "sunlit-e2e/job.sh");
    }

    #[test]
    fn the_exit_code_probe_is_quiet_when_the_file_is_not_there_yet() {
        let windows = exit_code_command(Target::Windows);
        assert!(windows.starts_with("if exist "), "{windows}");
        let linux = exit_code_command(Target::Linux);
        assert!(linux.contains("2>/dev/null"), "{linux}");
        assert!(linux.contains("|| true"), "{linux}");
    }

    #[test]
    fn an_absent_exit_code_reads_as_still_running() {
        assert_eq!(parse_exit_code(""), Ok(None));
        assert_eq!(parse_exit_code("   \n"), Ok(None));
    }

    #[test]
    fn an_exit_code_is_read_from_the_first_line() {
        assert_eq!(parse_exit_code("0\n"), Ok(Some(0)));
        assert_eq!(parse_exit_code("101\r\n"), Ok(Some(101)));
        assert_eq!(parse_exit_code(" 3 \nextra\n"), Ok(Some(3)));
    }

    #[test]
    fn a_broken_exit_code_is_an_error_rather_than_a_pass() {
        let err = parse_exit_code("Access is denied.").unwrap_err();
        assert!(err.contains("unreadable exit code"), "{err}");
        assert!(parse_exit_code("0x1").is_err());
    }

    #[test]
    fn the_session_probe_looks_for_the_marker_the_image_writes() {
        for target in Target::ALL {
            assert!(
                session_ready_command(target).contains(SESSION_READY_MARKER),
                "{target}"
            );
            assert!(session_ready_command(target).contains("ready"), "{target}");
        }
    }
}
