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
use crate::provider::target::Target;
use crate::store::state::RunState;

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

/// The command that prints the whole of the job's output so far.
///
/// `type` on Windows and `cat` on Linux, both of which read a file the job's own
/// shell still holds open for writing. A job worth watching is one that runs for
/// tens of minutes with nothing else to say for itself, which is what a release
/// build is; the e2e suite finishes in under a minute and its log is printed
/// once at the end.
pub fn output_log_command(target: Target) -> String {
    match target {
        Target::Windows => format!(
            r"if exist {0}\results\output.log type {0}\results\output.log",
            crate::provider::GUEST_ROOT_WINDOWS
        ),
        Target::Linux => format!(
            "cat {}/results/output.log 2>/dev/null || true",
            crate::provider::GUEST_ROOT_LINUX
        ),
    }
}

/// How much of the job's log has been printed already.
///
/// The guest is asked for the whole log every poll and this decides what of it
/// is new, because there is no `tail -c +N` that works the same in `cmd.exe`.
/// Tracked in characters rather than lines so that a line the guest is still
/// writing is not printed twice.
#[derive(Debug, Clone, Default)]
pub struct OutputTail {
    printed: usize,
}

impl OutputTail {
    pub fn new() -> Self {
        Self::default()
    }

    /// The part of `log` that has not been printed yet, and nothing when there
    /// is none.
    ///
    /// A log that is shorter than what was already printed is a guest that
    /// started the job again, which the runner does by removing the results
    /// directory first. That reads as a fresh log rather than as nothing new.
    ///
    /// The log arrives decoded from whatever bytes the guest had written when
    /// the poll read the file, so a multi-byte character caught half-written
    /// comes back as one three-byte replacement character and turns into itself
    /// a poll later. That moves every byte after it, which is why an index into
    /// the previous decode is not trusted here: what the last poll ended in is
    /// held back rather than printed, and clamping to a character boundary keeps
    /// a stale index from landing inside a character and panicking.
    pub fn absorb<'a>(&mut self, log: &'a str) -> Option<&'a str> {
        if log.len() < self.printed {
            self.printed = 0;
        }
        while self.printed > 0 && !log.is_char_boundary(self.printed) {
            self.printed -= 1;
        }
        let fresh = &log[self.printed..];
        // A trailing replacement character is a character the guest is still
        // writing, so it waits for the poll that has the whole of it.
        let fresh = fresh.trim_end_matches(char::REPLACEMENT_CHARACTER);
        if fresh.is_empty() {
            return None;
        }
        self.printed += fresh.len();
        Some(fresh)
    }
}

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
        // A probe that did not answer has two causes: a session still coming
        // up, or a guest that died on the way. Only the provider can tell them
        // apart, and the second has no session coming, so waiting it out would
        // be dead time with the reason sitting unread in the VM's log.
        if let Some(reason) = provider.defunct(state) {
            return Err(format!(
                "the guest stopped before its desktop session appeared: {reason}"
            ));
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
    run_watching(
        provider,
        state,
        target,
        script,
        local_scratch,
        timeout,
        None,
    )
}

/// The same, printing the job's output as it arrives when a tail is given.
#[allow(clippy::too_many_arguments)]
pub fn run_watching(
    provider: &dyn Provider,
    state: &RunState,
    target: Target,
    script: &str,
    local_scratch: &Path,
    timeout: Duration,
    tail: Option<&mut OutputTail>,
) -> Result<i32, String> {
    std::fs::create_dir_all(local_scratch)
        .map_err(|e| format!("cannot create {}: {e}", local_scratch.display()))?;
    let local = local_scratch.join(job_file(target));
    // Written verbatim. The line endings belong to whoever built the script,
    // because they are part of what the guest's shell will accept: `cmd.exe`
    // wants CRLF and `sh` wants LF, and rewriting them here would silently
    // undo a choice made where the difference is understood.
    std::fs::write(&local, script).map_err(|e| format!("cannot write the job script: {e}"))?;

    provider.copy_in(state, &local, &job_destination(target))?;

    let out = provider.exec(state, launch_command(target))?;
    if !out.success() {
        return Err(format!(
            "the guest would not start the job: exit {:?}: {}",
            out.code,
            out.stderr.trim()
        ));
    }

    wait_for_exit_code(provider, state, target, timeout, tail)
}

/// Print what the job has written since it was last asked.
///
/// A log that could not be read is not an error: the job is what this is
/// watching, and one unanswered read of its log is the connection rather than
/// the run.
fn print_fresh_output(
    provider: &dyn Provider,
    state: &RunState,
    log_command: &str,
    tail: Option<&mut OutputTail>,
) {
    if let Some(tail) = tail
        && let Ok(out) = provider.exec(state, log_command)
        && let Some(fresh) = tail.absorb(&out.stdout)
    {
        print!("{fresh}");
    }
}

/// Poll until `exit_code.txt` appears, printing what the job wrote since the
/// last poll when a tail is given.
pub fn wait_for_exit_code(
    provider: &dyn Provider,
    state: &RunState,
    target: Target,
    timeout: Duration,
    mut tail: Option<&mut OutputTail>,
) -> Result<i32, String> {
    let command = exit_code_command(target);
    let log_command = output_log_command(target);
    let start = Instant::now();
    loop {
        print_fresh_output(provider, state, &log_command, tail.as_deref_mut());
        match provider.exec(state, &command) {
            Ok(out) => {
                if let Some(code) = parse_exit_code(&out.stdout)? {
                    // The read above and this answer are two round trips, so a
                    // job that finished between them wrote its last lines after
                    // the log was read. One more read has them, and the runner
                    // writes the exit code once the job's output is closed, so
                    // there is nothing after these.
                    print_fresh_output(provider, state, &log_command, tail.as_deref_mut());
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
        // No exit code yet also has two causes: a job still running, or a
        // guest that died and took the job with it. The second has no exit
        // code coming, so the rest of the timeout would be dead time.
        if let Some(reason) = provider.defunct(state) {
            return Err(format!(
                "the guest stopped while the job was running: {reason}"
            ));
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
        assert_eq!(job_destination(Target::Linux), "/var/lib/sunlit-e2e/job.sh");
    }

    #[test]
    fn the_exit_code_probe_is_quiet_when_the_file_is_not_there_yet() {
        let windows = exit_code_command(Target::Windows);
        assert!(windows.starts_with("if exist "), "{windows}");
        let linux = exit_code_command(Target::Linux);
        assert!(linux.contains("2>/dev/null"), "{linux}");
        assert!(linux.contains("|| true"), "{linux}");
    }

    /// The guest is asked for its whole log every poll, so what makes the
    /// output readable is this deciding what is new. Printing a line twice is
    /// the visible failure; printing nothing at all is the silent one.
    #[test]
    fn a_tail_prints_each_line_once_and_survives_a_restarted_job() {
        let mut tail = OutputTail::new();
        assert_eq!(tail.absorb(""), None);
        assert_eq!(tail.absorb("Compiling wgpu\n"), Some("Compiling wgpu\n"));
        assert_eq!(tail.absorb("Compiling wgpu\n"), None);
        assert_eq!(
            tail.absorb("Compiling wgpu\nCompiling slint\n"),
            Some("Compiling slint\n")
        );
        // The runner clears the results directory when a job starts, so a log
        // that got shorter is a new job rather than nothing to say.
        assert_eq!(tail.absorb("Compiling se\n"), Some("Compiling se\n"));
        assert_eq!(tail.absorb("Compiling se\n"), None);
    }

    /// The guest's log is decoded from the bytes that were there when the poll
    /// read it, so a character the job was in the middle of writing arrives as
    /// one replacement character and becomes itself on the next poll. Both
    /// lengths change under a byte index kept from the poll before: a four-byte
    /// character grows the log by a byte, which used to put the index inside a
    /// character and panic seven minutes into a build, and a two- or three-byte
    /// one leaves it the same length or shorter, which used to drop the rest of
    /// the line in silence.
    #[test]
    fn a_character_caught_half_written_neither_panics_nor_swallows_the_line() {
        for (half, whole) in [
            ("Compiling \u{fffd}", "Compiling \u{1f389}"),
            ("Compiling \u{fffd}", "Compiling \u{20ac}b"),
            ("Compiling \u{fffd}", "Compiling \u{e9}b"),
        ] {
            let mut tail = OutputTail::new();
            let mut printed = String::new();
            if let Some(fresh) = tail.absorb(half) {
                printed.push_str(fresh);
            }
            if let Some(fresh) = tail.absorb(whole) {
                printed.push_str(fresh);
            }
            assert_eq!(printed, whole, "{whole:?}");
            assert_eq!(tail.absorb(whole), None, "{whole:?}");
        }
    }

    #[test]
    fn the_log_probe_is_quiet_when_there_is_no_log_yet() {
        let windows = output_log_command(Target::Windows);
        assert!(windows.starts_with("if exist "), "{windows}");
        assert!(windows.contains("type "), "{windows}");
        let linux = output_log_command(Target::Linux);
        assert!(linux.contains("cat "), "{linux}");
        assert!(linux.contains("|| true"), "{linux}");
        for target in Target::ALL {
            assert!(
                output_log_command(target).contains("output.log"),
                "{target}"
            );
        }
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

    use crate::provider::target::ProviderKind;
    use crate::runner::CommandOutput;
    use crate::runner::fake::FakeRunner;
    use crate::store::state::StartReason;

    /// A provider whose guest answers whatever the runner's table says and
    /// whose death verdict is fixed. Only what the waits consult is real;
    /// everything a wait has no business calling is unreachable.
    struct FakeProvider<'a> {
        runner: &'a FakeRunner,
        defunct: Option<String>,
    }

    impl crate::provider::Provider for FakeProvider<'_> {
        fn kind(&self) -> ProviderKind {
            ProviderKind::Qemu
        }
        fn create_from_golden(
            &self,
            _: crate::provider::target::Image,
            _: StartReason,
        ) -> Result<RunState, String> {
            unreachable!("the waits create nothing")
        }
        fn start(&self, _: &mut RunState) -> Result<(), String> {
            unreachable!("the waits start nothing")
        }
        fn destroy(&self, _: &RunState) -> Result<crate::provider::Stopped, String> {
            unreachable!("the waits destroy nothing")
        }
        fn stop(&self, _: &RunState) -> Result<crate::provider::Stopped, String> {
            unreachable!("the waits stop nothing")
        }
        fn is_running(&self, _: &RunState) -> bool {
            self.defunct.is_none()
        }
        fn defunct(&self, _: &RunState) -> Option<String> {
            self.defunct.clone()
        }
        fn view(&self, _: &RunState) -> Result<String, String> {
            unreachable!("the waits view nothing")
        }
        fn ssh_target(&self, state: &RunState) -> crate::guest::ssh::SshTarget {
            crate::guest::ssh::SshTarget::from_state(state, "/srv/vm/ssh/id_ed25519")
        }
        fn runner(&self) -> &dyn crate::runner::Runner {
            self.runner
        }
    }

    fn state() -> RunState {
        let mut state = RunState::new(
            crate::provider::target::Image::Linux,
            ProviderKind::Qemu,
            std::path::PathBuf::from("/srv/vm/run/linux/overlay.qcow2"),
            StartReason::Run,
            0,
        );
        state.ssh_host = "127.0.0.1".to_owned();
        state.ssh_port = 2222;
        state.ssh_user = "tester".to_owned();
        state
    }

    #[test]
    fn a_dead_guest_ends_the_session_wait_with_the_providers_reason() {
        // Refused, which is what a dead guest's forwarded port answers with.
        // The timeout is short only so that a regression fails in seconds
        // instead of hanging; the death verdict must win long before it.
        let runner = FakeRunner::new().on(
            SESSION_READY_MARKER,
            CommandOutput::failed(255, "Connection refused"),
        );
        let provider = FakeProvider {
            runner: &runner,
            defunct: Some("qemu has exited: port taken".to_owned()),
        };
        let err = wait_for_session(
            &provider,
            &state(),
            Target::Linux,
            Duration::from_millis(200),
        )
        .unwrap_err();
        assert!(err.contains("stopped before its desktop session"), "{err}");
        assert!(err.contains("qemu has exited: port taken"), "{err}");
    }

    #[test]
    fn a_session_that_answers_outranks_a_stale_death_verdict() {
        // The probe settles it: a guest that answered is alive whatever the
        // process table said a poll ago.
        let runner = FakeRunner::new().on(
            SESSION_READY_MARKER,
            CommandOutput::ok("SUNLIT_SESSION_READY\n"),
        );
        let provider = FakeProvider {
            runner: &runner,
            defunct: Some("a stale verdict".to_owned()),
        };
        wait_for_session(
            &provider,
            &state(),
            Target::Linux,
            Duration::from_millis(200),
        )
        .expect("the marker answers");
    }

    #[test]
    fn a_dead_guest_ends_the_job_wait_rather_than_the_timeout() {
        // An empty exit-code file reads as "still running", which is exactly
        // the answer a dead guest would leave in place forever.
        let runner = FakeRunner::new().on("exit_code.txt", CommandOutput::ok(""));
        let provider = FakeProvider {
            runner: &runner,
            defunct: Some("qemu has exited".to_owned()),
        };
        let err = wait_for_exit_code(
            &provider,
            &state(),
            Target::Linux,
            Duration::from_millis(200),
            None,
        )
        .unwrap_err();
        assert!(err.contains("stopped while the job was running"), "{err}");
        assert!(err.contains("qemu has exited"), "{err}");
    }

    #[test]
    fn an_exit_code_outranks_a_stale_death_verdict() {
        let runner = FakeRunner::new().on("exit_code.txt", CommandOutput::ok("0\n"));
        let provider = FakeProvider {
            runner: &runner,
            defunct: Some("a stale verdict".to_owned()),
        };
        assert_eq!(
            wait_for_exit_code(
                &provider,
                &state(),
                Target::Linux,
                Duration::from_millis(200),
                None
            ),
            Ok(0)
        );
    }

    /// A poll reads the log and then asks for the exit code, and those are two
    /// round trips: a job that ends between them wrote its last lines after the
    /// read. Without a read after the answer, that is the tail of a
    /// forty-minute build lost from the console, which is where the docs send a
    /// person to watch one.
    #[test]
    fn the_last_of_the_output_survives_a_job_that_ends_mid_poll() {
        let runner = FakeRunner::new()
            .on_each(
                "output.log",
                [
                    CommandOutput::ok("cache: packing the build directory\n"),
                    CommandOutput::ok("cache: packing the build directory\ncache: packed in 63s\n"),
                ],
            )
            .on("exit_code.txt", CommandOutput::ok("0\n"));
        let provider = FakeProvider {
            runner: &runner,
            defunct: None,
        };
        let mut tail = OutputTail::new();
        assert_eq!(
            wait_for_exit_code(
                &provider,
                &state(),
                Target::Linux,
                Duration::from_millis(200),
                Some(&mut tail),
            ),
            Ok(0)
        );
        // Nothing of the whole log is still owed, and nothing was printed
        // twice: the tail counts what it has printed against what it is given.
        assert_eq!(
            tail.absorb("cache: packing the build directory\ncache: packed in 63s\n"),
            None
        );
    }
}
