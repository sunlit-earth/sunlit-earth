//! The one boundary between this crate and the outside world.
//!
//! Plan decision 9 keeps the xtask thin: it drives `powershell.exe`, `qemu`,
//! `packer`, `ssh`, and `cargo` as processes rather than linking libraries for
//! any of them. Everything that touches a process goes through [`Runner`], so
//! the logic above it, which is where the decisions live, is testable against
//! fabricated command output instead of a hypervisor.

use std::ffi::OsStr;
use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// What a finished process produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    /// `None` when the process was terminated by a signal.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    /// Test constructors: production code only ever receives these from a
    /// process.
    #[cfg(test)]
    pub fn ok(stdout: impl Into<String>) -> Self {
        Self {
            code: Some(0),
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    #[cfg(test)]
    pub fn failed(code: i32, stderr: impl Into<String>) -> Self {
        Self {
            code: Some(code),
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }

    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// Trimmed stdout, which is what every parser in here wants.
    pub fn trimmed(&self) -> &str {
        self.stdout.trim()
    }
}

/// One process invocation.
#[derive(Debug, Clone, Default)]
pub struct Cmd {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub stdin: Option<String>,
    /// The readable form of a command whose arguments are encoded, kept for
    /// logs and for the test double to match on. Never sent to the process.
    pub script: Option<String>,
}

impl Cmd {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    #[must_use]
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    #[must_use]
    pub fn cwd(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    #[must_use]
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Data for the child's stdin, which the real runner pipes in.
    ///
    /// Only tests build one today. The field itself is production code: it is
    /// what carries a payload too large for a command line.
    #[cfg(test)]
    #[must_use]
    pub fn stdin(mut self, text: impl Into<String>) -> Self {
        self.stdin = Some(text.into());
        self
    }

    #[must_use]
    pub fn script(mut self, text: impl Into<String>) -> Self {
        self.script = Some(text.into());
        self
    }

    /// A one-line rendering used for logs, error messages, and as the key the
    /// test double matches on.
    pub fn display(&self) -> String {
        let mut out = self.program.clone();
        for arg in &self.args {
            let _ = write!(out, " {arg}");
        }
        out
    }
}

/// Everything the orchestrator does to processes.
pub trait Runner {
    /// Run to completion, capturing stdout and stderr.
    fn capture(&self, cmd: &Cmd) -> io::Result<CommandOutput>;

    /// Run to completion with the child's output going straight to the
    /// terminal, for long jobs whose progress the user wants to watch.
    fn stream(&self, cmd: &Cmd) -> io::Result<i32>;

    /// Start a process and leave it running, returning its process id.
    fn spawn(&self, cmd: &Cmd, log: Option<&Path>) -> io::Result<u32>;

    /// Locate an executable on `PATH`.
    fn which(&self, program: &str) -> Option<PathBuf>;

    /// What the process with this id is, or `None` if there is no such
    /// process. Identity, not just liveness: process ids are reused, and
    /// everything downstream of this is about to kill something.
    fn process_identity(&self, pid: u32) -> Option<ProcessIdentity>;

    /// Ask a process to end, forcefully if need be. An error here means the
    /// process may still be running.
    fn terminate(&self, pid: u32) -> Result<(), String>;
}

/// The real implementation: `std::process`.
pub struct RealRunner;

impl RealRunner {
    fn build(cmd: &Cmd) -> Command {
        let mut command = Command::new(&cmd.program);
        command.args(&cmd.args);
        if let Some(dir) = &cmd.cwd {
            command.current_dir(dir);
        }
        for (key, value) in &cmd.env {
            command.env(key, value);
        }
        command
    }
}

impl Runner for RealRunner {
    fn capture(&self, cmd: &Cmd) -> io::Result<CommandOutput> {
        let mut command = Self::build(cmd);
        command
            .stdin(if cmd.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command.spawn()?;
        if let Some(text) = &cmd.stdin {
            let mut pipe = child
                .stdin
                .take()
                .ok_or_else(|| io::Error::other("stdin was piped but is missing"))?;
            pipe.write_all(text.as_bytes())?;
            // Dropping the handle closes the pipe, which is what tells a reader
            // such as `powershell -Command -` that the script is complete.
            drop(pipe);
        }
        let output = child.wait_with_output()?;
        Ok(CommandOutput {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    fn stream(&self, cmd: &Cmd) -> io::Result<i32> {
        let mut command = Self::build(cmd);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        let status = command.status()?;
        Ok(status.code().unwrap_or(-1))
    }

    fn spawn(&self, cmd: &Cmd, log: Option<&Path>) -> io::Result<u32> {
        let mut command = Self::build(cmd);
        command.stdin(Stdio::null());
        match log {
            Some(path) => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let mut out = std::fs::File::create(path)?;
                // The first line of a VM log is the exact invocation that
                // produced it, which is the thing anyone debugging a guest that
                // will not boot asks for first.
                writeln!(out, "{}", cmd.display())?;
                let err = out.try_clone()?;
                command.stdout(Stdio::from(out)).stderr(Stdio::from(err));
            }
            None => {
                command.stdout(Stdio::null()).stderr(Stdio::null());
            }
        }
        Ok(command.spawn()?.id())
    }

    fn which(&self, program: &str) -> Option<PathBuf> {
        find_on_path(
            program,
            std::env::var("PATH").ok().as_deref(),
            std::env::var("PATHEXT").ok().as_deref(),
            cfg!(windows),
            &|p| p.is_file(),
        )
    }

    fn process_identity(&self, pid: u32) -> Option<ProcessIdentity> {
        if cfg!(target_os = "linux") {
            // Two file reads, no subprocess. `comm` is truncated to 15
            // characters, which `image_matches` accounts for; `cmdline` is
            // NUL-separated and carries the arguments the VM was named with.
            let image = std::fs::read_to_string(format!("/proc/{pid}/comm"))
                .ok()?
                .trim()
                .to_owned();
            let command_line = std::fs::read(format!("/proc/{pid}/cmdline"))
                .ok()
                .map(|raw| String::from_utf8_lossy(&raw).replace(char::from(0), " "));
            return Some(ProcessIdentity {
                image,
                command_line,
            });
        }
        if cfg!(windows) {
            let cmd = Cmd::new("tasklist").args([
                "/FI".to_owned(),
                format!("PID eq {pid}"),
                "/NH".to_owned(),
                "/FO".to_owned(),
                "CSV".to_owned(),
            ]);
            return self
                .capture(&cmd)
                .ok()
                .and_then(|out| parse_tasklist_csv(&out.stdout, pid));
        }
        None
    }

    fn terminate(&self, pid: u32) -> Result<(), String> {
        let cmd = if cfg!(windows) {
            Cmd::new("taskkill").args([
                "/PID".to_owned(),
                pid.to_string(),
                "/T".into(),
                "/F".into(),
            ])
        } else {
            Cmd::new("kill").args(["-TERM".to_owned(), pid.to_string()])
        };
        let out = self
            .capture(&cmd)
            .map_err(|e| format!("cannot run {}: {e}", cmd.program))?;
        if out.success() {
            return Ok(());
        }
        // A kill that failed is the case that matters: the caller is about to
        // delete the file the process still has open.
        Err(format!(
            "{} exited {:?}: {}",
            cmd.program,
            out.code,
            if out.stderr.trim().is_empty() {
                out.stdout.trim()
            } else {
                out.stderr.trim()
            }
        ))
    }
}

/// What a process is, for deciding whether it is ours.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessIdentity {
    /// The executable name. Truncated to 15 characters on Linux, and carrying
    /// the `.exe` suffix on Windows.
    pub image: String,
    /// The full command line where it is cheap to get, which is Linux only.
    pub command_line: Option<String>,
}

/// Whether a process image is the one expected.
///
/// Handles the two ways the name arrives differently from how it was spelled:
/// Windows appends `.exe`, and Linux's `comm` is truncated to 15 characters,
/// so `qemu-system-x86_64` arrives as `qemu-system-x86`.
pub fn image_matches(found: &str, expected: &str) -> bool {
    let found = found.trim();
    let found = found.strip_suffix(".exe").unwrap_or(found);
    if found.eq_ignore_ascii_case(expected) {
        return true;
    }
    // A truncated name has to be a prefix, and a short one is not evidence.
    found.len() >= 15 && expected.len() > found.len() && expected.starts_with(found)
}

/// Read the image name out of `tasklist /NH /FO CSV`, which quotes every
/// field: `"qemu-system-x86_64.exe","4242","Console","1","1,234 K"`.
///
/// The CSV form is used rather than the default table because a process whose
/// name is longer than the column truncates in the table and a memory figure
/// with thousands separators makes whitespace splitting unreliable.
pub fn parse_tasklist_csv(stdout: &str, pid: u32) -> Option<ProcessIdentity> {
    let wanted = pid.to_string();
    for line in stdout.lines() {
        let fields: Vec<&str> = line.split("\",\"").collect();
        if fields.len() < 2 {
            continue;
        }
        let image = fields[0].trim_start_matches('"').trim();
        let found_pid = fields[1].trim_matches('"').trim();
        if found_pid == wanted && !image.is_empty() {
            return Some(ProcessIdentity {
                image: image.to_owned(),
                command_line: None,
            });
        }
    }
    None
}

/// Resolve `program` against a `PATH` string, applying Windows' `PATHEXT`.
///
/// Written as a pure function over the environment rather than shelling out to
/// `where.exe` or `which`, both so it is testable and so the answer does not
/// depend on which shell happens to be hosting the xtask.
pub fn find_on_path(
    program: &str,
    path_var: Option<&str>,
    pathext: Option<&str>,
    windows: bool,
    exists: &dyn Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let separator = if windows { ';' } else { ':' };
    let candidate = Path::new(program);
    if candidate.components().count() > 1 {
        // An explicit path is used as given.
        return exists(candidate).then(|| candidate.to_path_buf());
    }

    let extensions: Vec<String> = if windows {
        let raw = pathext.unwrap_or(".COM;.EXE;.BAT;.CMD");
        std::iter::once(String::new())
            .chain(
                raw.split(';')
                    .filter(|e| !e.trim().is_empty())
                    .map(str::to_ascii_lowercase),
            )
            .collect()
    } else {
        vec![String::new()]
    };

    for dir in path_var.unwrap_or_default().split(separator) {
        let dir = dir.trim().trim_matches('"');
        if dir.is_empty() {
            continue;
        }
        for ext in &extensions {
            let mut name = String::from(program);
            name.push_str(ext);
            let full = Path::new(dir).join(name);
            if exists(&full) {
                return Some(full);
            }
        }
    }
    None
}

/// Build a `powershell.exe` invocation that reads its script from stdin.
///
/// `-EncodedCommand` rather than `-Command` or a temporary script file. It
/// sidesteps every quoting problem a multi-line script would otherwise hit on
/// the command line, and it is the only one of the three that both leaves the
/// filesystem alone and parses the script as a script.
///
/// The two alternatives were tried first and both failed silently, which is
/// what earns this a paragraph. `-Command -` reads stdin the way a console
/// reads typing: a multi-line script came back with no output, no error, and
/// exit code 0, because a script read that way does not set an exit code when
/// it dies. The console encoding line is wrapped in a `try` for a related
/// reason: it throws when the process has no console attached, which is exactly
/// the case when the xtask was started by a tool rather than from a terminal.
///
/// The plaintext is kept on the command for logging and for the test double to
/// match on, since the arguments themselves are base64 by then.
pub fn powershell(script: &str) -> Cmd {
    let prelude = "try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch { }\n\
                   $ErrorActionPreference = 'Stop'\n";
    let full = format!("{prelude}{script}");
    Cmd::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand",
            &encode_command(&full),
        ])
        .script(full)
}

/// Encode a script the way `-EncodedCommand` expects: UTF-16LE, then base64.
pub fn encode_command(script: &str) -> String {
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64(&utf16)
}

/// Standard base64 with padding. Four lines of table lookup, against pulling in
/// a dependency for it.
fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).copied().map_or(0, u32::from);
        let b2 = chunk.get(2).copied().map_or(0, u32::from);
        let triple = (b0 << 16) | (b1 << 8) | b2;
        for i in 0..4 {
            if i <= chunk.len() {
                let index = (triple >> (18 - 6 * i)) & 0x3f;
                out.push(char::from(TABLE[index as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Quote a value for interpolation into a single-quoted `PowerShell` string.
pub fn ps_quote(value: &(impl AsRef<OsStr> + ?Sized)) -> String {
    let text = Path::new(value).to_string_lossy().into_owned();
    format!("'{}'", text.replace('\'', "''"))
}

#[cfg(test)]
pub mod fake {
    //! A [`Runner`] that answers from a table instead of a machine.

    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::{Cmd, CommandOutput, ProcessIdentity, Runner};
    use std::io;
    use std::path::{Path, PathBuf};

    #[derive(Default)]
    pub struct FakeRunner {
        responses: Vec<(String, CommandOutput)>,
        tools: HashMap<String, PathBuf>,
        /// Behind a `RefCell` because `terminate` removes from it: a process
        /// that was killed and is still reported as running would make the
        /// teardown path untestable, which is the path that deletes disks.
        processes: RefCell<HashMap<u32, ProcessIdentity>>,
        pub calls: RefCell<Vec<String>>,
        pub spawned: RefCell<Vec<String>>,
        pub terminated: RefCell<Vec<u32>>,
    }

    impl FakeRunner {
        pub fn new() -> Self {
            Self::default()
        }

        /// Answer any command whose rendering contains `key`.
        #[must_use]
        pub fn on(mut self, key: &str, output: CommandOutput) -> Self {
            self.responses.push((key.to_owned(), output));
            self
        }

        /// Pretend `name` is on `PATH` at `path`.
        #[must_use]
        pub fn with_tool(mut self, name: &str, path: &str) -> Self {
            self.tools.insert(name.to_owned(), PathBuf::from(path));
            self
        }

        /// A running process with this id, image, and command line.
        #[must_use]
        pub fn with_process(self, pid: u32, image: &str, command_line: Option<&str>) -> Self {
            let identity = ProcessIdentity {
                image: image.to_owned(),
                command_line: command_line.map(str::to_owned),
            };
            self.processes.borrow_mut().insert(pid, identity);
            self
        }

        /// Every command the code under test issued, in order.
        pub fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
    }

    impl Runner for FakeRunner {
        fn capture(&self, cmd: &Cmd) -> io::Result<CommandOutput> {
            let rendered = cmd.display();
            // Matching covers the readable script too, because a `PowerShell`
            // command carries its script base64-encoded in an argument.
            let haystack = format!(
                "{rendered}\n{}\n{}",
                cmd.stdin.clone().unwrap_or_default(),
                cmd.script.clone().unwrap_or_default()
            );
            self.calls.borrow_mut().push(rendered.clone());
            for (key, output) in &self.responses {
                if haystack.contains(key.as_str()) {
                    return Ok(output.clone());
                }
            }
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("no fake response registered for: {rendered}"),
            ))
        }

        fn stream(&self, cmd: &Cmd) -> io::Result<i32> {
            self.capture(cmd).map(|out| out.code.unwrap_or(-1))
        }

        fn spawn(&self, cmd: &Cmd, _log: Option<&Path>) -> io::Result<u32> {
            self.spawned.borrow_mut().push(cmd.display());
            Ok(4242)
        }

        fn which(&self, program: &str) -> Option<PathBuf> {
            self.tools.get(program).cloned()
        }

        fn process_identity(&self, pid: u32) -> Option<ProcessIdentity> {
            self.processes.borrow().get(&pid).cloned()
        }

        fn terminate(&self, pid: u32) -> Result<(), String> {
            self.terminated.borrow_mut().push(pid);
            self.processes.borrow_mut().remove(&pid);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_rendering_is_the_key_tests_match_on() {
        let cmd = Cmd::new("qemu-system-x86_64").args(["-m", "4096"]);
        assert_eq!(cmd.display(), "qemu-system-x86_64 -m 4096");
    }

    #[test]
    fn stdin_is_data_rather_than_part_of_the_command() {
        // The distinction matters: a payload too large for a command line goes
        // here instead, and nothing about it is parsed as code.
        let cmd = Cmd::new("powershell.exe")
            .arg("-Command")
            .stdin("a payload");
        assert_eq!(cmd.stdin.as_deref(), Some("a payload"));
        assert!(!cmd.display().contains("a payload"), "{}", cmd.display());
    }

    #[test]
    fn a_process_identity_is_read_out_of_the_csv_form_of_tasklist() {
        let hit = "\"qemu-system-x86_64.exe\",\"4242\",\"Console\",\"1\",\"1,234,567 K\"";
        let identity = parse_tasklist_csv(hit, 4242).expect("found");
        assert_eq!(identity.image, "qemu-system-x86_64.exe");
        assert_eq!(identity.command_line, None);
        assert_eq!(parse_tasklist_csv(hit, 42), None);
    }

    #[test]
    fn a_tasklist_miss_is_not_a_process() {
        // A miss is not an error exit; it is a sentence on stdout, so the exit
        // code cannot be trusted and the pid has to be looked for.
        let miss = "INFO: No tasks are running which match the specified criteria.";
        assert_eq!(parse_tasklist_csv(miss, 4242), None);
        assert_eq!(parse_tasklist_csv("", 4242), None);
    }

    #[test]
    fn a_pid_that_only_appears_inside_another_number_is_not_a_match() {
        let other = "\"qemu-system-x86_64.exe\",\"42425\",\"Console\",\"1\",\"1,234 K\"";
        assert_eq!(parse_tasklist_csv(other, 4242), None);
    }

    #[test]
    fn an_image_name_matches_through_both_ways_it_is_spelled_differently() {
        assert!(image_matches("qemu-system-x86_64", "qemu-system-x86_64"));
        // Windows appends the extension.
        assert!(image_matches(
            "qemu-system-x86_64.exe",
            "qemu-system-x86_64"
        ));
        // Linux truncates /proc/<pid>/comm to 15 characters.
        assert!(image_matches("qemu-system-x86", "qemu-system-x86_64"));
        assert!(image_matches(
            " qemu-system-x86 
",
            "qemu-system-x86_64"
        ));

        assert!(!image_matches("bash", "qemu-system-x86_64"));
        assert!(!image_matches("qemu-img", "qemu-system-x86_64"));
        // A short prefix is not evidence of a truncated name.
        assert!(!image_matches("qemu", "qemu-system-x86_64"));
        assert!(!image_matches("", "qemu-system-x86_64"));
    }

    #[cfg(windows)]
    #[test]
    fn path_lookup_applies_pathext_on_windows() {
        let exists = |p: &Path| {
            p.to_string_lossy()
                .eq_ignore_ascii_case(r"C:\qemu\qemu-img.exe")
        };
        let found = find_on_path(
            "qemu-img",
            Some(r"C:\missing;C:\qemu"),
            Some(".COM;.EXE;.CMD"),
            true,
            &exists,
        );
        assert_eq!(found, Some(PathBuf::from(r"C:\qemu\qemu-img.exe")));
    }

    #[test]
    fn path_lookup_on_unix_takes_the_first_hit_and_no_extension() {
        let exists = |p: &Path| p == Path::new("/usr/bin/ssh");
        assert_eq!(
            find_on_path("ssh", Some("/opt/bin:/usr/bin"), None, false, &exists),
            Some(PathBuf::from("/usr/bin/ssh"))
        );
        assert_eq!(
            find_on_path("ssh", Some("/opt/bin"), None, false, &exists),
            None
        );
    }

    #[test]
    fn path_lookup_accepts_an_explicit_path_and_skips_the_search() {
        let exists = |p: &Path| p == Path::new("/opt/qemu/bin/qemu-img");
        assert_eq!(
            find_on_path(
                "/opt/qemu/bin/qemu-img",
                Some("/usr/bin"),
                None,
                false,
                &exists
            ),
            Some(PathBuf::from("/opt/qemu/bin/qemu-img"))
        );
        assert_eq!(
            find_on_path(
                "/opt/qemu/bin/missing",
                Some("/usr/bin"),
                None,
                false,
                &exists
            ),
            None
        );
    }

    // Windows path semantics: off Windows, `Path` treats a drive-qualified
    // path as a single component, and this code only ever runs on a
    // Windows host anyway.
    #[cfg(windows)]
    #[test]
    fn path_lookup_survives_empty_and_quoted_entries() {
        let exists = |p: &Path| p == Path::new(r"C:\Program Files\qemu\qemu-img.exe");
        let found = find_on_path(
            "qemu-img",
            Some(r#";"C:\Program Files\qemu";"#),
            Some(".EXE"),
            true,
            &exists,
        );
        assert_eq!(
            found,
            Some(PathBuf::from(r"C:\Program Files\qemu\qemu-img.exe"))
        );
    }

    #[test]
    fn powershell_scripts_travel_base64_encoded() {
        let cmd = powershell("Get-CimInstance Win32_ComputerSystem");
        assert_eq!(cmd.program, "powershell.exe");
        assert!(cmd.args.contains(&"-NonInteractive".to_owned()));
        assert!(cmd.args.contains(&"-EncodedCommand".to_owned()));
        // Nothing is expected on stdin, which is what stopped working.
        assert_eq!(cmd.stdin, None);
        let script = cmd.script.expect("the plaintext travels with the command");
        assert!(script.contains("Get-CimInstance"));
        assert!(script.contains("OutputEncoding"));
        assert!(
            script.starts_with("try {"),
            "the encoding line must not be able to kill the script"
        );
        let encoded = cmd.args.last().expect("the encoded script is last");
        assert_eq!(encode_command(&script), *encoded);
    }

    #[test]
    fn the_encoder_produces_utf16le_base64() {
        // What `powershell -EncodedCommand` expects, checked against a value
        // small enough to verify by hand: 'hi' is 68 00 69 00.
        assert_eq!(encode_command("hi"), "aABpAA==");
        assert_eq!(encode_command(""), "");
        assert_eq!(encode_command("a"), "YQA=");
        assert_eq!(encode_command("abc"), "YQBiAGMA");
    }

    #[test]
    fn powershell_quoting_doubles_embedded_quotes() {
        assert_eq!(ps_quote("plain"), "'plain'");
        assert_eq!(ps_quote("it's"), "'it''s'");
        assert_eq!(ps_quote(r"C:\path with space"), r"'C:\path with space'");
    }

    #[test]
    fn the_fake_runner_matches_on_a_fragment_and_records_calls() {
        use super::fake::FakeRunner;
        let runner = FakeRunner::new().on("Win32_ComputerSystem", CommandOutput::ok("{}"));
        let out = runner
            .capture(&powershell("Get-CimInstance Win32_ComputerSystem"))
            .expect("registered response");
        assert_eq!(out.stdout, "{}");
        assert_eq!(runner.calls().len(), 1);
        assert!(
            runner
                .capture(&Cmd::new("packer").arg("version"))
                .is_err_and(|e| e.kind() == io::ErrorKind::NotFound)
        );
    }
}

#[cfg(all(test, windows))]
mod windows_boundary_tests {
    //! The one place the real runner is exercised against a real process.
    //!
    //! Every Windows host fact arrives through `powershell -Command -`, and the
    //! failure mode this pins is silent: a script that dies on its first
    //! statement produces no output and exit code 0, which reads as an empty
    //! document rather than as an error.

    use super::{RealRunner, Runner, powershell};

    #[test]
    fn a_powershell_script_on_stdin_produces_output_here() {
        let out = RealRunner
            .capture(&powershell("Write-Output 'xtask-probe-ok'"))
            .expect("powershell.exe runs");
        assert!(
            out.stdout.contains("xtask-probe-ok"),
            "code={:?} stdout={:?} stderr={:?}",
            out.code,
            out.stdout,
            out.stderr
        );
    }
}
