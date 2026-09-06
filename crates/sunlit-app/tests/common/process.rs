//! Starting the binary, watching what it says, and taking it down again.
//!
//! The layer every case sits on: where the binary and its fixtures are, what
//! this session can be asked to do, the four timeouts, the guards that make a
//! failed case tidy up after itself, and the IPC round trips.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use interprocess::local_socket::traits::Stream as StreamExt;
use interprocess::local_socket::{GenericNamespaced, ToNsName};

use sunlit_earth::ipc::MemorySignal;

/// The binary under test.
///
/// `CARGO_BIN_EXE_sunlit-earth` is resolved by Cargo at build time, which makes
/// it a path on the machine that compiled the suite. That is the right answer
/// on a developer desktop and the wrong one inside a VM, where the test binary
/// was built on the host and copied in. `SUNLIT_EARTH_BIN` is what the VM
/// orchestrator sets; without it nothing changes.
pub(crate) fn binary() -> PathBuf {
    std::env::var_os("SUNLIT_EARTH_BIN").map_or_else(
        || PathBuf::from(env!("CARGO_BIN_EXE_sunlit-earth")),
        PathBuf::from,
    )
}

/// A file from `tests/fixtures/`.
///
/// `CARGO_MANIFEST_DIR` has the same problem as `CARGO_BIN_EXE`: it is a
/// compile-time path into a source tree the guest does not have.
pub(crate) fn fixture(name: &str) -> PathBuf {
    std::env::var_os("SUNLIT_EARTH_E2E_FIXTURES")
        .map_or_else(
            || {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("tests")
                    .join("fixtures")
            },
            PathBuf::from,
        )
        .join(name)
}

/// Whether this session gives the app a working tray icon.
///
/// A session and not a platform, which is the same shape the wallpaper answer
/// took and for a related reason. Windows has a tray in every session. Linux has
/// one when the desktop runs a `StatusNotifierItem` host: Slint registers its icon
/// over D-Bus through `ksni`, so what decides the answer is whether anything on
/// the session bus owns `org.kde.StatusNotifierWatcher`. Plasma does, and the
/// icon and its menu were both observed there; GNOME ships no host at all
/// without a shell extension. macOS has no tray in this app either way.
///
/// The probe is `gdbus`, which is a program rather than a dependency, and a
/// session where it is absent or answers anything but `true` is read as having
/// no tray. That is the safe direction: the cases this gates skip rather than
/// fail when the answer is wrong, and a Windows regression cannot hide behind it
/// because Windows never reaches the probe.
///
/// Cases below split three ways on this. Ones that exercise the tray itself
/// skip where there is none; ones that merely need a window and a hide-and-show
/// cycle run windowed instead, which tests the same thing minus the icon; the
/// rest do not care.
pub(crate) fn tray_supported() -> bool {
    static ANSWER: OnceLock<bool> = OnceLock::new();
    *ANSWER.get_or_init(|| {
        // macOS has a menu bar in every GUI session, so there is nothing to probe.
        if cfg!(any(target_os = "windows", target_os = "macos")) {
            return true;
        }
        if !cfg!(target_os = "linux") {
            return false;
        }
        status_notifier_watcher_present()
    })
}

/// Does anything on this session bus own the `StatusNotifier` watcher name?
fn status_notifier_watcher_present() -> bool {
    let output = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.freedesktop.DBus",
            "--object-path",
            "/org/freedesktop/DBus",
            "--method",
            "org.freedesktop.DBus.NameHasOwner",
            "org.kde.StatusNotifierWatcher",
        ])
        .output();
    match output {
        Ok(done) => String::from_utf8_lossy(&done.stdout).contains("true"),
        Err(_) => false,
    }
}

/// Startup arguments for a case that needs a window and an IPC-driven
/// hide-and-show cycle, but not the tray icon itself.
fn lifecycle_mode_args() -> [&'static str; 2] {
    if tray_supported() {
        ["--tray-start", "visible"]
    } else {
        ["--mode", "window"]
    }
}

/// Whether this session can set the desktop wallpaper.
///
/// Unlike the tray, this one answers for itself: `SystemWallpaper` reports
/// whether it has anywhere to publish to, which is the same query the engine
/// makes before it renders anything.
///
/// A session and not a platform, which is the change Linux support brought.
/// Windows has one setter and every session has it; Linux has one per desktop
/// and a process outside a desktop session has none, so the answer here is about
/// where this run is and not only about what it was compiled for.
pub(crate) fn wallpaper_supported() -> bool {
    use sunlit_core::engine::wallpaper_sink::{SystemWallpaper, WallpaperSink};
    SystemWallpaper.check_supported().is_ok()
}

/// Whether this platform has a wallpaper setter at all.
///
/// The half of the question that is still a compile-time fact, and the one worth
/// pinning: a platform on this list that refuses is a regression, and one off it
/// that succeeds is a setter nobody wrote.
pub(crate) const WALLPAPER_PLATFORM: bool = cfg!(any(
    target_os = "windows",
    target_os = "linux",
    target_os = "macos"
));

/// Whether this run is allowed to replace the desktop wallpaper.
///
/// Off by default, and deliberately not tied to the platform: the case is
/// harmless in a throwaway VM and rude on a developer's desktop, and those are
/// the same Windows. The VM job sets this; nothing else does.
pub(crate) const WALLPAPER_OPT_IN: &str = "SUNLIT_EARTH_E2E_WALLPAPER";

/// How long a case waits for the app to come up: the IPC listener, then the
/// first frame or the deferred hide.
///
/// Sized for the slowest start the suite sees, which is a cold texture cache on
/// a software adapter in a guest. The only thing a generous budget costs is how
/// long a genuinely hung process takes to be reported.
pub(crate) const READY: Duration = Duration::from_mins(1);

/// How long a case waits for the reply to one IPC command that answers without
/// rendering a wallpaper: a show, a hide, an export probe, a memory query or a
/// memory report.
pub(crate) const SIGNAL_REPLY: Duration = Duration::from_secs(30);

/// How long a case waits for a publish: a full-resolution render, a readback, a
/// PNG encode and a handoff to the desktop, all on that software adapter.
pub(crate) const PUBLISH: Duration = Duration::from_mins(2);

/// How long a case waits for the process to be gone after `quit`.
pub(crate) const SHUTDOWN: Duration = Duration::from_secs(15);

/// Announce that a case is not running here.
///
/// The suite is `#[ignore]`d and has no skip mechanism of its own, so a case
/// that returns early passes. Printing why is what stops that from being
/// indistinguishable from passing for the right reason.
pub(crate) fn skip_case(case: &str, why: &str) {
    println!("skipping {case}: {why}");
}

/// RAII guard that kills a child process on drop if it hasn't exited yet.
///
/// This prevents orphaned application windows from leaking when a test panics
/// before it gets a chance to send the IPC quit command.
pub(crate) struct ChildGuard {
    child: Option<Child>,
}

impl ChildGuard {
    pub(crate) fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    /// Take ownership of the inner `Child`, disabling the kill-on-drop guard.
    /// Use this when handing the child to `wait_with_timeout`.
    pub(crate) fn take(&mut self) -> Child {
        self.child.take().expect("child already taken")
    }

    /// The child, for the two things only it can answer: its streams and its
    /// process id.
    pub(crate) fn child_mut(&mut self) -> &mut Child {
        self.child.as_mut().expect("child already taken")
    }

    /// The child's process id.
    #[cfg(windows)]
    pub(crate) fn pid(&self) -> u32 {
        self.child.as_ref().expect("child already taken").id()
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take()
            && child.try_wait().ok().flatten().is_none()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Poll `child.try_wait()` until the process exits or `timeout` elapses.
///
/// If the process does not exit in time, it is killed via `child.kill()` and
/// the function panics with a descriptive message.
pub(crate) fn wait_with_timeout(mut child: Child, timeout: Duration) -> Output {
    let start = Instant::now();

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = child.stdout.take().map_or_else(Vec::new, |mut s| {
                    use std::io::Read;
                    let mut buf = Vec::new();
                    s.read_to_end(&mut buf).unwrap_or(0);
                    buf
                });
                let stderr = child.stderr.take().map_or_else(Vec::new, |mut s| {
                    use std::io::Read;
                    let mut buf = Vec::new();
                    s.read_to_end(&mut buf).unwrap_or(0);
                    buf
                });
                return Output {
                    status,
                    stdout,
                    stderr,
                };
            }
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!(
                        "child process did not exit within {:.0}s — killed",
                        timeout.as_secs_f64()
                    );
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => {
                panic!("error polling child process: {e}");
            }
        }
    }
}

/// Create a unique temporary directory for test artifacts.
///
/// The caller is responsible for removing it when done.
fn create_temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sunlit_earth_e2e_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));
    fs::create_dir_all(&dir).expect("failed to create temp dir");
    dir
}

/// Remove a directory and all its contents, ignoring errors.
fn cleanup_temp_dir(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

/// RAII guard that removes a temp directory on drop, including when the test
/// panics part-way through. Mirrors `ChildGuard`, for filesystem state.
pub(crate) struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    pub(crate) fn new() -> Self {
        Self {
            path: create_temp_dir(),
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        cleanup_temp_dir(&self.path);
    }
}

/// Scratch directory for state a spawned binary would otherwise write into the
/// developer's `%LOCALAPPDATA%\SunlitEarth`.
///
/// Without this redirection the tests read the real settings (and, when
/// auto-refresh is enabled there, replace the desktop wallpaper of the machine
/// running them) and append rows to the real memory metrics CSV, contaminating
/// the soak data that file exists to collect.
pub(crate) fn isolated_state_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("sunlit_earth_e2e_state");
    let _ = fs::create_dir_all(&dir);
    dir
}

/// A throwaway config path, passed to every spawned binary via
/// `SUNLIT_EARTH_CONFIG`.
pub(crate) fn isolated_config_path() -> PathBuf {
    isolated_state_dir().join(format!(
        "config_{}_{}.toml",
        std::process::id(),
        SOCKET_COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

/// A parsed memory usage log entry.
pub(crate) struct MemoryEntry {
    pub(crate) context: String,
    pub(crate) rss_mb: f64,
    pub(crate) peak_rss_mb: f64,
}

/// Parse all "memory usage" lines from stderr, stripping ANSI escape codes.
pub(crate) fn parse_memory_entries(stderr: &str) -> Vec<MemoryEntry> {
    let ansi_re = regex_lite::Regex::new(r"\x1b\[[0-9;]*m").unwrap();

    let mut entries = Vec::new();
    for line in stderr.lines() {
        if !line.contains("memory usage") {
            continue;
        }
        let clean = ansi_re.replace_all(line, "");

        let context = clean
            .split("context=")
            .nth(1)
            .and_then(|s| {
                // context="some text" — extract between quotes
                let s = s.trim_start_matches('"');
                s.split('"').next()
            })
            .unwrap_or("")
            .to_owned();

        let rss_mb = clean
            .split("rss_mb=")
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);

        let peak_rss_mb = clean
            .split("peak_rss_mb=")
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);

        entries.push(MemoryEntry {
            context,
            rss_mb,
            peak_rss_mb,
        });
    }
    entries
}

/// Monotonically increasing counter for names that have to be unique within
/// this process: socket names and throwaway config paths.
static SOCKET_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Generate a unique local socket name for a test.
pub(crate) fn unique_socket_name() -> String {
    let counter = SOCKET_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("sunlit-earth-test-{}-{}", std::process::id(), counter)
}

/// Send a single IPC command to the named local socket.
pub(crate) fn send_ipc_command(socket_name: &str, command: &str) {
    let name = socket_name
        .to_ns_name::<GenericNamespaced>()
        .expect("failed to convert socket name");
    let mut stream = interprocess::local_socket::Stream::connect(name)
        .unwrap_or_else(|e| panic!("failed to connect to IPC socket '{socket_name}': {e}"));
    stream
        .write_all(format!("{command}\n").as_bytes())
        .unwrap_or_else(|e| panic!("failed to write IPC command '{command}': {e}"));
}

/// What being ready means for one start of the app.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ready {
    /// The listener answers and a frame has been drawn.
    FirstFrame,
    /// The listener answers and the deferred hide has run, which is what a
    /// `--tray-start hidden` run reaches instead of a first frame.
    HiddenWindow,
    /// The listener answers, and that is all this case needs before it acts.
    Listener,
}

/// One start of the app under test.
///
/// Every case wants the same things: a config file and a metrics directory of
/// its own so that nothing touches the developer's, an IPC socket of its own,
/// the debug log, and both streams piped and drained from the moment the child
/// exists, which is what [`StdoutWatcher`]'s note on pipe buffer congestion
/// asks for. What differs between cases is the mode arguments, the environment
/// the cloud case adds, and what there is to wait for, so those are what this
/// carries.
pub(crate) struct Spawn<'a> {
    socket_name: &'a str,
    config: PathBuf,
    args: Vec<&'a str>,
    env: Vec<(&'a str, std::ffi::OsString)>,
    clouds: bool,
    ready: Ready,
}

impl<'a> Spawn<'a> {
    /// A start in whichever of tray and windowed mode this session can hold,
    /// with a throwaway config and no cloud fetcher.
    pub(crate) fn new(socket_name: &'a str) -> Self {
        Self {
            socket_name,
            config: isolated_config_path(),
            args: lifecycle_mode_args().to_vec(),
            env: Vec::new(),
            clouds: false,
            ready: Ready::FirstFrame,
        }
    }

    /// The arguments between the log level and the socket name, replacing the
    /// mode [`Spawn::new`] chose.
    pub(crate) fn args(mut self, args: impl IntoIterator<Item = &'a str>) -> Self {
        self.args = args.into_iter().collect();
        self
    }

    /// The config file this run reads and writes, for a case that wrote one.
    pub(crate) fn config(mut self, config: PathBuf) -> Self {
        self.config = config;
        self
    }

    /// One more environment variable for the child.
    pub(crate) fn env(mut self, key: &'a str, value: impl Into<std::ffi::OsString>) -> Self {
        self.env.push((key, value.into()));
        self
    }

    /// Let the cloud fetcher run, which only the case watching it wants: every
    /// other case keeps the network out of the picture.
    pub(crate) fn with_clouds(mut self) -> Self {
        self.clouds = true;
        self
    }

    /// What to wait for before handing the app back.
    pub(crate) fn ready(mut self, ready: Ready) -> Self {
        self.ready = ready;
        self
    }

    /// Spawn, attach both watchers, and wait until the app is answering.
    pub(crate) fn start(self) -> (ChildGuard, StdoutWatcher, StderrWatcher) {
        let mut command = Command::new(binary());
        command
            .env("SUNLIT_EARTH_CONFIG", &self.config)
            .env("SUNLIT_EARTH_METRICS_DIR", isolated_state_dir());
        if !self.clouds {
            command.env("SUNLIT_EARTH_NO_CLOUDS", "1");
        }
        for (key, value) in &self.env {
            command.env(key, value);
        }
        let mut guard = ChildGuard::new(
            command
                .args(["--log-level", "debug"])
                .args(&self.args)
                .args(["--ipc-socket", self.socket_name])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("failed to spawn sunlit-earth binary"),
        );
        let child = guard.child_mut();
        let stdout_watcher = StdoutWatcher::new(child);
        let stderr_watcher = StderrWatcher::new(child);

        stdout_watcher.wait_for_signal("ipc_listener_ready", READY);
        match self.ready {
            Ready::FirstFrame => stdout_watcher.wait_for_signal("first_frame_rendered", READY),
            Ready::HiddenWindow => stdout_watcher.wait_for_signal("window_hidden_deferred", READY),
            Ready::Listener => {}
        }
        (guard, stdout_watcher, stderr_watcher)
    }
}

/// Watches a child process's stderr in a background thread, collecting lines
/// as they arrive. Provides `wait_for_log()` to block until a specific
/// substring appears in stderr output.
pub(crate) struct StderrWatcher {
    lines: Arc<Mutex<Vec<String>>>,
    thread: std::thread::JoinHandle<()>,
}

impl StderrWatcher {
    /// Create a new watcher that takes ownership of `child.stderr`.
    pub(crate) fn new(child: &mut Child) -> Self {
        let stderr = child.stderr.take().expect("child stderr not piped");
        let lines = Arc::new(Mutex::new(Vec::new()));
        let lines_clone = Arc::clone(&lines);

        let thread = std::thread::Builder::new()
            .name("stderr-watcher".into())
            .spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines() {
                    match line {
                        Ok(l) => {
                            lines_clone
                                .lock()
                                .expect("stderr watcher lock poisoned")
                                .push(l);
                        }
                        Err(_) => break,
                    }
                }
            })
            .expect("failed to spawn stderr watcher thread");

        Self { lines, thread }
    }

    /// Block until a line containing `needle` appears in stderr, or panic
    /// after `timeout` elapses.
    pub(crate) fn wait_for_log(&self, needle: &str, timeout: Duration) {
        let start = Instant::now();
        let mut last_checked = 0;
        loop {
            {
                let lines = self.lines.lock().expect("stderr watcher lock poisoned");
                for line in &lines[last_checked..] {
                    if line.contains(needle) {
                        return;
                    }
                }
                last_checked = lines.len();
            }
            if start.elapsed() > timeout {
                let lines = self.lines.lock().expect("stderr watcher lock poisoned");
                panic!(
                    "timed out after {:.0}s waiting for '{needle}' in stderr.\n\
                     Collected {} lines:\n{}",
                    timeout.as_secs_f64(),
                    lines.len(),
                    lines.join("\n")
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Return all collected stderr lines.
    pub(crate) fn lines(&self) -> Vec<String> {
        self.lines
            .lock()
            .expect("stderr watcher lock poisoned")
            .clone()
    }

    /// Every line, once the stream has ended.
    ///
    /// Joins the reader thread, so what comes back is the whole of what the
    /// child wrote rather than whatever had arrived by the time it exited.
    pub(crate) fn into_lines(self) -> Vec<String> {
        let Self { lines, thread } = self;
        let _ = thread.join();
        Arc::try_unwrap(lines).map_or_else(
            |shared| shared.lock().expect("stderr watcher lock poisoned").clone(),
            |lock| lock.into_inner().expect("stderr watcher lock poisoned"),
        )
    }

    /// Number of stderr lines collected so far, usable as a cursor into
    /// a later `lines()` snapshot.
    pub(crate) fn line_count(&self) -> usize {
        self.lines
            .lock()
            .expect("stderr watcher lock poisoned")
            .len()
    }
}

/// Watches a child process's stdout in a background thread, collecting lines
/// as they arrive. Provides `wait_for_signal()` to block until a specific
/// `SIGNAL:<name>` line appears. Stdout is used as a dedicated signaling
/// channel, separate from the tracing stderr stream, to avoid pipe buffer
/// congestion and stderr lock contention.
pub(crate) struct StdoutWatcher {
    lines: Arc<Mutex<Vec<String>>>,
    _thread: std::thread::JoinHandle<()>,
}

impl StdoutWatcher {
    /// Create a new watcher that takes ownership of `child.stdout`.
    pub(crate) fn new(child: &mut Child) -> Self {
        let stdout = child.stdout.take().expect("child stdout not piped");
        let lines = Arc::new(Mutex::new(Vec::new()));
        let lines_clone = Arc::clone(&lines);

        let thread = std::thread::Builder::new()
            .name("stdout-watcher".into())
            .spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines() {
                    match line {
                        Ok(l) => {
                            lines_clone
                                .lock()
                                .expect("stdout watcher lock poisoned")
                                .push(l);
                        }
                        Err(_) => break,
                    }
                }
            })
            .expect("failed to spawn stdout watcher thread");

        Self {
            lines,
            _thread: thread,
        }
    }

    /// Block until a `SIGNAL:<name>` line appears in stdout, or panic
    /// after `timeout` elapses.
    pub(crate) fn wait_for_signal(&self, name: &str, timeout: Duration) {
        self.wait_for_signal_line_from(name, 0, timeout);
    }

    /// The lines strictly between the first `begin` and the first `end` at or
    /// after index `from`. Panics when either marker is missing.
    fn lines_between(&self, begin: &str, end: &str, from: usize) -> Vec<String> {
        let lines = self.lines.lock().expect("stdout watcher lock poisoned");
        let tail = &lines[from.min(lines.len())..];
        let start = tail
            .iter()
            .position(|line| line.contains(begin))
            .unwrap_or_else(|| panic!("no '{begin}' in stdout:\n{}", tail.join("\n")));
        let stop = tail[start..]
            .iter()
            .position(|line| line.contains(end))
            .unwrap_or_else(|| panic!("no '{end}' after '{begin}':\n{}", tail.join("\n")));
        tail[start + 1..start + stop].to_vec()
    }

    /// Number of stdout lines collected so far. Used as a cursor so repeated
    /// queries do not match the reply to an earlier request.
    pub(crate) fn line_count(&self) -> usize {
        self.lines
            .lock()
            .expect("stdout watcher lock poisoned")
            .len()
    }

    /// Block until a `SIGNAL:<name>` line appears at or after line index
    /// `from`, returning the matched line. Panics after `timeout` elapses.
    pub(crate) fn wait_for_signal_line_from(
        &self,
        name: &str,
        from: usize,
        timeout: Duration,
    ) -> String {
        let needle = format!("SIGNAL:{name}");
        let start = Instant::now();
        let mut last_checked = from;
        loop {
            {
                let lines = self.lines.lock().expect("stdout watcher lock poisoned");
                for line in &lines[last_checked.min(lines.len())..] {
                    if line.contains(&needle) {
                        return line.clone();
                    }
                }
                last_checked = lines.len();
            }
            if start.elapsed() > timeout {
                let lines = self.lines.lock().expect("stdout watcher lock poisoned");
                panic!(
                    "timed out after {:.0}s waiting for '{needle}' in stdout.\n\
                     Collected {} lines:\n{}",
                    timeout.as_secs_f64(),
                    lines.len(),
                    lines.join("\n")
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// Send `query-memory` over IPC and read the reply signal line back.
///
/// Through the app's own [`MemorySignal`], which is the type that wrote the
/// line, so a field renamed in the producer is a compile error here.
pub(crate) fn query_memory(socket_name: &str, watcher: &StdoutWatcher) -> MemorySignal {
    let from = watcher.line_count();
    send_ipc_command(socket_name, "query-memory");
    let line = watcher.wait_for_signal_line_from("memory ", from, SIGNAL_REPLY);
    MemorySignal::parse(&line).unwrap_or_else(|| {
        panic!(
            "missing '{}' in memory signal line: {line}",
            MemorySignal::missing_field(&line).unwrap_or("a field")
        )
    })
}

/// Convert a byte count to MiB for readable log output.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// The section headers `memory-report` promises. Only these are a contract;
/// the numbers on them and the rows beneath them are free to change.
pub(crate) const REPORT_SECTIONS: [&str; 4] = [
    "process:",
    "wgpu counters:",
    "gpu allocations:",
    "expected:",
];

/// Send `memory-report` over IPC and return the lines between the two markers.
pub(crate) fn memory_report(socket_name: &str, watcher: &StdoutWatcher) -> Vec<String> {
    let from = watcher.line_count();
    send_ipc_command(socket_name, "memory-report");
    watcher.wait_for_signal_line_from("memory_report_end", from, SIGNAL_REPLY);
    watcher.lines_between(
        "SIGNAL:memory_report_begin",
        "SIGNAL:memory_report_end",
        from,
    )
}

/// No line the app logged is an error.
pub(crate) fn assert_no_error_lines(lines: &[String]) {
    for line in lines {
        assert!(!line.contains(" ERROR "), "found ERROR in stderr:\n{line}");
    }
}

/// Quit over IPC, wait for the process to be gone, and assert it went cleanly.
///
/// The tail every case ends in but two: the cloud case quits before asserting
/// so that a failing run still yields a complete log, and the session-end case
/// never sends `quit` at all because what it is timing is the exit Windows asks
/// for.
pub(crate) fn quit_and_expect_clean_exit(
    socket_name: &str,
    guard: &mut ChildGuard,
    stderr: &StderrWatcher,
) {
    send_ipc_command(socket_name, "quit");
    let output = wait_with_timeout(guard.take(), SHUTDOWN);
    assert!(
        output.status.success(),
        "process exited with non-zero status: {:?}",
        output.status
    );
    assert_no_error_lines(&stderr.lines());
}
