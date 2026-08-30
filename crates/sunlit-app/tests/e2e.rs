//! End-to-end render export tests for the Sunlit Earth binary.
//!
//! All tests are marked `#[ignore]` because they require a desktop environment
//! and GPU. Run with `cargo test --test e2e -- --ignored`.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

use image::GenericImageView;
use interprocess::local_socket::traits::Stream as StreamExt;
use interprocess::local_socket::{GenericNamespaced, ToNsName};
use serial_test::serial;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The binary under test.
///
/// `CARGO_BIN_EXE_sunlit-earth` is resolved by Cargo at build time, which makes
/// it a path on the machine that compiled the suite. That is the right answer
/// on a developer desktop and the wrong one inside a VM, where the test binary
/// was built on the host and copied in. `SUNLIT_EARTH_BIN` is what the VM
/// orchestrator sets; without it nothing changes.
fn binary() -> PathBuf {
    std::env::var_os("SUNLIT_EARTH_BIN").map_or_else(
        || PathBuf::from(env!("CARGO_BIN_EXE_sunlit-earth")),
        PathBuf::from,
    )
}

/// A file from `tests/fixtures/`.
///
/// `CARGO_MANIFEST_DIR` has the same problem as `CARGO_BIN_EXE`: it is a
/// compile-time path into a source tree the guest does not have.
fn fixture(name: &str) -> PathBuf {
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
fn tray_supported() -> bool {
    static ANSWER: OnceLock<bool> = OnceLock::new();
    *ANSWER.get_or_init(|| {
        if cfg!(target_os = "windows") {
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
fn wallpaper_supported() -> bool {
    use sunlit_core::engine::wallpaper_sink::{SystemWallpaper, WallpaperSink};
    SystemWallpaper.check_supported().is_ok()
}

/// Whether this platform has a wallpaper setter at all.
///
/// The half of the question that is still a compile-time fact, and the one worth
/// pinning: a platform on this list that refuses is a regression, and one off it
/// that succeeds is a setter nobody wrote.
const WALLPAPER_PLATFORM: bool = cfg!(any(target_os = "windows", target_os = "linux"));

/// Whether this run is allowed to replace the desktop wallpaper.
///
/// Off by default, and deliberately not tied to the platform: the case is
/// harmless in a throwaway VM and rude on a developer's desktop, and those are
/// the same Windows. The VM job sets this; nothing else does.
const WALLPAPER_OPT_IN: &str = "SUNLIT_EARTH_E2E_WALLPAPER";

/// Announce that a case is not running here.
///
/// The suite is `#[ignore]`d and has no skip mechanism of its own, so a case
/// that returns early passes. Printing why is what stops that from being
/// indistinguishable from passing for the right reason.
fn skip_case(case: &str, why: &str) {
    println!("skipping {case}: {why}");
}

/// RAII guard that kills a child process on drop if it hasn't exited yet.
///
/// This prevents orphaned application windows from leaking when a test panics
/// before it gets a chance to send the IPC quit command.
struct ChildGuard {
    child: Option<Child>,
}

impl ChildGuard {
    fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    /// Take ownership of the inner `Child`, disabling the kill-on-drop guard.
    /// Use this when handing the child to `wait_with_timeout`.
    fn take(&mut self) -> Child {
        self.child.take().expect("child already taken")
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
fn wait_with_timeout(mut child: Child, timeout: Duration) -> Output {
    let start = Instant::now();

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // Process exited — collect output.
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
                // Still running.
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    // Wait for the killed process to clean up.
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
struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new() -> Self {
        Self {
            path: create_temp_dir(),
        }
    }

    fn path(&self) -> &Path {
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
fn isolated_state_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("sunlit_earth_e2e_state");
    let _ = fs::create_dir_all(&dir);
    dir
}

/// A throwaway config path, passed to every spawned binary via
/// `SUNLIT_EARTH_CONFIG`.
fn isolated_config_path() -> PathBuf {
    isolated_state_dir().join(format!(
        "config_{}_{}.toml",
        std::process::id(),
        SOCKET_COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

/// A parsed memory usage log entry.
struct MemoryEntry {
    context: String,
    rss_mb: f64,
    peak_rss_mb: f64,
}

/// Parse all "memory usage" lines from stderr, stripping ANSI escape codes.
fn parse_memory_entries(stderr: &str) -> Vec<MemoryEntry> {
    // Strip ANSI escape sequences: ESC [ ... m
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

/// Extract the (R, G, B) channels of a pixel at the given coordinates.
fn rgb_at(img: &image::RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let p = img.get_pixel(x, y).0;
    [p[0], p[1], p[2]]
}

/// Assert that a corner of the render is empty space rather than globe.
///
/// A patch and not a pixel, because the sky is drawn there. A single sample
/// asks whether one point happens to be free of stars, and the answer moved to
/// within four pixels of the top-left corner the moment stars were turned on
/// by default: a nudge to a star default, the catalog, or the fixture's date
/// would eventually land one on the sample and report "expected black" about a
/// globe that is exactly where it should be. What the case actually means is
/// that the corner is mostly empty, which a patch can say and a pixel cannot.
/// Stars are small and sparse; a globe filling the corner is neither.
///
/// What a patch cannot survive is a layer that paints every pixel of the sky,
/// so the fixture config switches the Milky Way off for the same reason it
/// switches the clouds off: this case reads the picture by color, and both of
/// those put a color where it reads.
const CORNER_PATCH: u32 = 16;
const CORNER_BLACK_FRACTION: f64 = 0.5;

fn assert_space_corner(img: &image::RgbaImage, right: bool, bottom: bool, label: &str) {
    let x0 = if right { img.width() - CORNER_PATCH } else { 0 };
    let y0 = if bottom {
        img.height() - CORNER_PATCH
    } else {
        0
    };
    let mut black = 0_u32;
    let mut brightest = ([0_u8; 3], 0_u32, (0_u32, 0_u32));
    for y in y0..y0 + CORNER_PATCH {
        for x in x0..x0 + CORNER_PATCH {
            let rgb = rgb_at(img, x, y);
            let sum = u32::from(rgb[0]) + u32::from(rgb[1]) + u32::from(rgb[2]);
            if sum < 40 {
                black += 1;
            }
            if sum > brightest.1 {
                brightest = (rgb, sum, (x, y));
            }
        }
    }
    let fraction = f64::from(black) / f64::from(CORNER_PATCH * CORNER_PATCH);
    assert!(
        fraction >= CORNER_BLACK_FRACTION,
        "{label}: expected mostly empty space, only {:.0}% of the \
         {CORNER_PATCH}x{CORNER_PATCH} patch at ({x0}, {y0}) is black \
         (limit {:.0}%). Brightest pixel {:?} sum {} at {:?}.",
        fraction * 100.0,
        CORNER_BLACK_FRACTION * 100.0,
        brightest.0,
        brightest.1,
        brightest.2
    );
}

/// Assert that a pixel is dark ocean on the night side (nearly black).
fn assert_night_ocean(rgb: [u8; 3], label: &str) {
    let sum = u32::from(rgb[0]) + u32::from(rgb[1]) + u32::from(rgb[2]);
    assert!(
        sum < 40,
        "{label}: expected dark ocean, got {rgb:?} (sum={sum})"
    );
}

/// Assert that a pixel is night-side land (dim, bluish from nightglow).
fn assert_night_land(rgb: [u8; 3], label: &str) {
    let [r, g, b] = rgb;
    let sum = u32::from(r) + u32::from(g) + u32::from(b);
    assert!(
        sum > 40 && sum < 200 && b >= r && b >= g,
        "{label}: expected dim bluish night land, got {rgb:?} (sum={sum})"
    );
}

/// Assert that a pixel is greenish (vegetation).
/// Green must clearly dominate both red and blue.
fn assert_greenish(rgb: [u8; 3], label: &str) {
    let [r, g, b] = rgb;
    assert!(
        g > r + 10 && g > b && g > 40,
        "{label}: expected greenish (G dominant), got {rgb:?}"
    );
}

/// Assert that a pixel is yellowish/sandy (desert).
/// Red and green both strong, blue clearly lower.
fn assert_yellowish(rgb: [u8; 3], label: &str) {
    let [r, g, b] = rgb;
    let warm = u32::from(r) + u32::from(g);
    assert!(
        warm > 3 * u32::from(b) && r > 80 && g > 80,
        "{label}: expected yellowish/sandy (R+G >> B), got {rgb:?}"
    );
}

/// Assert that a pixel is blue (ocean).
/// Blue must exceed the sum of red and green.
fn assert_blue(rgb: [u8; 3], label: &str) {
    let [r, g, b] = rgb;
    assert!(
        u32::from(b) > u32::from(r) + u32::from(g) && b > 30,
        "{label}: expected blue (B > R+G), got {rgb:?}"
    );
}

/// Assert that a pixel is bright/white (ice/snow).
/// High overall brightness with all channels close together.
fn assert_ice(rgb: [u8; 3], label: &str) {
    let [r, g, b] = rgb;
    let sum = u32::from(r) + u32::from(g) + u32::from(b);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let spread = u32::from(max) - u32::from(min);
    assert!(
        sum > 500 && spread < 30,
        "{label}: expected bright white ice (sum>500, spread<30), got {rgb:?} (sum={sum}, spread={spread})"
    );
}

// ---------------------------------------------------------------------------
// IPC helpers
// ---------------------------------------------------------------------------

/// Monotonically increasing counter for unique socket names.
static SOCKET_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Generate a unique local socket name for a test.
fn unique_socket_name() -> String {
    let counter = SOCKET_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("sunlit-earth-test-{}-{}", std::process::id(), counter)
}

/// Send a single IPC command to the named local socket.
fn send_ipc_command(socket_name: &str, command: &str) {
    let name = socket_name
        .to_ns_name::<GenericNamespaced>()
        .expect("failed to convert socket name");
    let mut stream = interprocess::local_socket::Stream::connect(name)
        .unwrap_or_else(|e| panic!("failed to connect to IPC socket '{socket_name}': {e}"));
    stream
        .write_all(format!("{command}\n").as_bytes())
        .unwrap_or_else(|e| panic!("failed to write IPC command '{command}': {e}"));
}

/// Watches a child process's stderr in a background thread, collecting lines
/// as they arrive. Provides `wait_for_log()` to block until a specific
/// substring appears in stderr output.
struct StderrWatcher {
    lines: Arc<Mutex<Vec<String>>>,
    _thread: std::thread::JoinHandle<()>,
}

impl StderrWatcher {
    /// Create a new watcher that takes ownership of `child.stderr`.
    fn new(child: &mut Child) -> Self {
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

        Self {
            lines,
            _thread: thread,
        }
    }

    /// Block until a line containing `needle` appears in stderr, or panic
    /// after `timeout` elapses.
    fn wait_for_log(&self, needle: &str, timeout: Duration) {
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
    fn lines(&self) -> Vec<String> {
        self.lines
            .lock()
            .expect("stderr watcher lock poisoned")
            .clone()
    }

    /// Number of stderr lines collected so far, usable as a cursor into
    /// a later `lines()` snapshot.
    fn line_count(&self) -> usize {
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
struct StdoutWatcher {
    lines: Arc<Mutex<Vec<String>>>,
    _thread: std::thread::JoinHandle<()>,
}

impl StdoutWatcher {
    /// Create a new watcher that takes ownership of `child.stdout`.
    fn new(child: &mut Child) -> Self {
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
    fn wait_for_signal(&self, name: &str, timeout: Duration) {
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
    fn line_count(&self) -> usize {
        self.lines
            .lock()
            .expect("stdout watcher lock poisoned")
            .len()
    }

    /// Block until a `SIGNAL:<name>` line appears at or after line index
    /// `from`, returning the matched line. Panics after `timeout` elapses.
    fn wait_for_signal_line_from(&self, name: &str, from: usize, timeout: Duration) -> String {
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

/// Process memory counters reported by the `query-memory` IPC command.
#[derive(Clone, Copy)]
struct MemoryQuery {
    rss: u64,
    peak_rss: u64,
    private: u64,
}

/// Extract a `key=<u64>` field from a `SIGNAL:memory ...` line.
fn parse_memory_field(line: &str, key: &str) -> u64 {
    let prefix = format!("{key}=");
    line.split_whitespace()
        .find_map(|token| token.strip_prefix(&prefix))
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_else(|| panic!("missing '{key}' in memory signal line: {line}"))
}

/// Send `query-memory` over IPC and parse the reply signal line.
fn query_memory(socket_name: &str, watcher: &StdoutWatcher) -> MemoryQuery {
    let from = watcher.line_count();
    send_ipc_command(socket_name, "query-memory");
    let line = watcher.wait_for_signal_line_from("memory ", from, Duration::from_secs(15));
    MemoryQuery {
        rss: parse_memory_field(&line, "rss_bytes"),
        peak_rss: parse_memory_field(&line, "peak_rss_bytes"),
        private: parse_memory_field(&line, "private_bytes"),
    }
}

/// Convert a byte count to MiB for readable log output.
#[allow(clippy::cast_precision_loss)]
fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// The section headers `memory-report` promises. Only these are a contract;
/// the numbers on them and the rows beneath them are free to change.
const REPORT_SECTIONS: [&str; 4] = [
    "process:",
    "wgpu counters:",
    "gpu allocations:",
    "expected:",
];

/// Send `memory-report` over IPC and return the lines between the two markers.
fn memory_report(socket_name: &str, watcher: &StdoutWatcher) -> Vec<String> {
    let from = watcher.line_count();
    send_ipc_command(socket_name, "memory-report");
    watcher.wait_for_signal_line_from("memory_report_end", from, Duration::from_secs(30));
    watcher.lines_between(
        "SIGNAL:memory_report_begin",
        "SIGNAL:memory_report_end",
        from,
    )
}

// ---------------------------------------------------------------------------
// Cloud stub server
// ---------------------------------------------------------------------------

/// Shared state of the stub cloud server: the currently published image
/// version and the number of full downloads served.
struct StubState {
    version: AtomicU64,
    gets: AtomicU64,
}

/// Start a minimal HTTP/1.1 server on an ephemeral port that serves `jpeg`
/// as the cloud image.
///
/// `HEAD` answers `304 Not Modified` when the request's `If-None-Match` matches
/// the current version and `200` otherwise; `GET` always returns the body and
/// increments the download counter. Every response carries `Content-Length` and
/// `Connection: close` and the socket is closed afterwards, so `ureq` never
/// waits for a keep-alive continuation.
///
/// Returns the bound port and the shared state, which the test uses to publish
/// new versions and observe downloads.
fn spawn_cloud_stub(jpeg: Vec<u8>) -> (u16, Arc<StubState>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind cloud stub server");
    let port = listener
        .local_addr()
        .expect("cloud stub server has no local address")
        .port();
    let state = Arc::new(StubState {
        version: AtomicU64::new(1),
        gets: AtomicU64::new(0),
    });

    let server_state = Arc::clone(&state);
    std::thread::Builder::new()
        .name("cloud-stub".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                serve_cloud_request(stream, &jpeg, &server_state);
            }
        })
        .expect("failed to spawn cloud stub thread");

    (port, state)
}

/// Handle a single stub request, then close the connection.
///
/// Any connection that does not deliver a complete request within the read
/// timeout is abandoned rather than blocking the accept loop; a stray local
/// connection (a port scanner, say) would otherwise wedge the server and hang
/// the test.
fn serve_cloud_request(mut stream: TcpStream, jpeg: &[u8], state: &StubState) {
    const INM: &str = "if-none-match:";
    const READ_TIMEOUT: Duration = Duration::from_secs(5);

    if stream.set_read_timeout(Some(READ_TIMEOUT)).is_err() {
        return;
    }
    let Ok(peek) = stream.try_clone() else { return };
    let mut reader = BufReader::new(peek);

    let mut request_line = String::new();
    if !matches!(reader.read_line(&mut request_line), Ok(n) if n > 0) {
        return;
    }
    let method = request_line
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_owned();

    let mut if_none_match: Option<String> = None;
    loop {
        let mut header = String::new();
        match reader.read_line(&mut header) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => return,
        }
        if header.trim().is_empty() {
            break;
        }
        if header.to_ascii_lowercase().starts_with(INM) {
            if_none_match = Some(header[INM.len()..].trim().to_owned());
        }
    }

    let etag = format!("\"v{}\"", state.version.load(Ordering::SeqCst));
    let response = if method == "HEAD" && if_none_match.as_deref() == Some(etag.as_str()) {
        format!(
            "HTTP/1.1 304 Not Modified\r\nETag: {etag}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
    } else {
        format!(
            "HTTP/1.1 200 OK\r\nETag: {etag}\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            jpeg.len()
        )
    };

    if stream.write_all(response.as_bytes()).is_err() {
        return;
    }
    if method == "GET" {
        if stream.write_all(jpeg).is_err() {
            return;
        }
        if stream.flush().is_err() {
            return;
        }
        state.gets.fetch_add(1, Ordering::SeqCst);
    }
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Both);
}

/// Encode a JPEG the stub server can serve as the cloud image.
///
/// The gradient keeps the encoded file small while the decoded RGBA buffer is
/// `width * height * 4` bytes, which is what the leak used to park in memory.
fn cloud_fixture_jpeg(width: u32, height: u32) -> Vec<u8> {
    let mut img = image::RgbImage::new(width, height);
    for (x, y, pixel) in img.enumerate_pixels_mut() {
        let r = u8::try_from(x % 256).expect("modulo 256 fits in u8");
        let g = u8::try_from(y % 256).expect("modulo 256 fits in u8");
        *pixel = image::Rgb([r, g, 128]);
    }
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Jpeg)
        .expect("failed to encode cloud fixture JPEG");
    buf.into_inner()
}

/// Block until the stub server has served at least `target` downloads.
fn wait_for_downloads(stub: &StubState, target: u64, timeout: Duration) {
    let start = Instant::now();
    loop {
        let served = stub.gets.load(Ordering::SeqCst);
        if served >= target {
            return;
        }
        assert!(
            start.elapsed() <= timeout,
            "timed out after {:.0}s waiting for cloud download #{target} (served {served})",
            timeout.as_secs_f64()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires desktop environment and GPU"]
#[serial]
fn test_binary_exists() {
    let path = binary();
    assert!(
        path.exists(),
        "binary not found at {} — was the project built?",
        path.display()
    );
}

#[test]
#[ignore = "requires desktop environment and GPU"]
#[serial]
fn test_render_and_exit() {
    // 1. Create a temp directory and output path. RAII so a failing assertion
    //    below still cleans the directory up.
    let temp_dir = TempDirGuard::new();
    let output_path = temp_dir.path().join("render.png");
    let config_path = fixture("e2e_config.toml");

    // 2. Spawn the binary with the render subcommand.
    let child = Command::new(binary())
        .env("SUNLIT_EARTH_NO_CLOUDS", "1")
        .env("SUNLIT_EARTH_CONFIG", isolated_config_path())
        .env("SUNLIT_EARTH_METRICS_DIR", isolated_state_dir())
        .args([
            "--log-level",
            "debug",
            "render",
            "--output",
            output_path.to_str().expect("non-UTF-8 temp path"),
            "--width",
            "800",
            "--height",
            "800",
            "--config",
            config_path.to_str().expect("non-UTF-8 fixture path"),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn sunlit-earth binary");

    // 3. Wait for the process to exit (30s timeout).
    let output = wait_with_timeout(child, Duration::from_secs(30));

    // 4. Assert exit code is 0.
    assert!(
        output.status.success(),
        "process exited with non-zero status: {:?}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    // 5. Assert the render output file exists and is non-empty.
    assert!(
        output_path.exists(),
        "render output file was not created at {}",
        output_path.display()
    );
    let file_size = fs::metadata(&output_path)
        .expect("failed to read render output metadata")
        .len();
    assert!(file_size > 0, "render output file is empty (0 bytes)");

    // 6. Decode the PNG with the image crate.
    let img = image::open(&output_path).expect("failed to decode render output PNG");

    // 7. Assert image dimensions match the requested size.
    let (width, height) = img.dimensions();
    assert_eq!(width, 800, "render output width mismatch");
    assert_eq!(height, 800, "render output height mismatch");

    // 8. Sample pixels at known geographic locations to validate the
    //    rendered globe. The config places the camera at lon=11, lat=48
    //    (Central Europe) at 17:00 UTC on summer solstice, so the
    //    terminator runs through eastern Europe with India/Tibet in night.
    let rgba = img.to_rgba8();

    // Corners should be space: empty background, with whatever stars the sky
    // put there.
    assert_space_corner(&rgba, false, false, "top-left corner");
    assert_space_corner(&rgba, true, false, "top-right corner");
    assert_space_corner(&rgba, false, true, "bottom-left corner");
    assert_space_corner(&rgba, true, true, "bottom-right corner");

    // Center: Central Europe — should be greenish (vegetation)
    assert_greenish(rgb_at(&rgba, 400, 400), "center (Central Europe)");

    // Sahara: south of center — should be yellowish/sandy
    assert_yellowish(rgb_at(&rgba, 420, 560), "Sahara");

    // Atlantic Ocean: west/left of center — should be blue
    assert_blue(rgb_at(&rgba, 250, 400), "Atlantic Ocean");

    // Greenland: upper-left — should be bright white (ice/snow)
    assert_ice(rgb_at(&rgba, 300, 175), "Greenland");

    // Indian Ocean: night side, far east — should be very dark water
    assert_night_ocean(rgb_at(&rgba, 730, 500), "Indian Ocean (night)");

    // Tibet: night side land with nightglow — dim and bluish
    assert_night_land(rgb_at(&rgba, 730, 300), "Tibet (night)");

    // 9. Parse stderr — assert no line contains " ERROR ".
    let stderr_text = String::from_utf8_lossy(&output.stderr);
    for line in stderr_text.lines() {
        assert!(!line.contains(" ERROR "), "found ERROR in stderr:\n{line}");
    }

    // 10. Assert stderr contains "first frame rendered".
    assert!(
        stderr_text.contains("first frame rendered"),
        "stderr does not contain 'first frame rendered':\n{stderr_text}"
    );

    // 11. Validate memory usage profile from debug log entries.
    let mem = parse_memory_entries(&stderr_text);
    assert!(
        !mem.is_empty(),
        "no memory usage entries found in stderr (is log level debug?)"
    );

    // Phase A: Early memory should be low (before textures load).
    if let Some(entry) = mem.iter().find(|e| e.context == "after window creation") {
        assert!(
            entry.rss_mb < 300.0,
            "early memory too high: {:.0} MB at '{}' (expected < 300 MB)",
            entry.rss_mb,
            entry.context
        );
    }

    // Phase B: Peak RSS should stay within limits during rendering.
    let peak = mem.iter().map(|e| e.peak_rss_mb).fold(0.0f64, f64::max);
    assert!(
        peak < 3000.0,
        "peak RSS too high: {peak:.0} MB (expected < 3000 MB)"
    );

    // Phase C: Memory should settle down before exit.
    if let Some(entry) = mem.iter().rev().find(|e| e.context == "before exit") {
        assert!(
            entry.rss_mb < 1000.0,
            "exit memory too high: {:.0} MB (expected < 1000 MB)",
            entry.rss_mb
        );
        assert!(
            entry.rss_mb < peak,
            "memory did not settle: exit RSS {:.0} MB >= peak {:.0} MB",
            entry.rss_mb,
            peak
        );
    }
}

#[test]
#[ignore = "requires desktop environment and GPU"]
#[serial]
fn test_tray_mode_ipc_lifecycle() {
    if !tray_supported() {
        skip_case(
            "test_tray_mode_ipc_lifecycle",
            "--tray-start hidden needs tray mode, and this session hosts no tray",
        );
        return;
    }
    let socket_name = unique_socket_name();

    // 1. Spawn the binary in tray mode with window hidden and IPC enabled.
    let mut guard = ChildGuard::new(
        Command::new(binary())
            .env("SUNLIT_EARTH_NO_CLOUDS", "1")
            .env("SUNLIT_EARTH_CONFIG", isolated_config_path())
            .env("SUNLIT_EARTH_METRICS_DIR", isolated_state_dir())
            .args([
                "--log-level",
                "debug",
                "--tray-start",
                "hidden",
                "--ipc-socket",
                &socket_name,
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn sunlit-earth binary"),
    );
    let child = guard.child.as_mut().unwrap();

    let stdout_watcher = StdoutWatcher::new(child);
    let watcher = StderrWatcher::new(child);

    // 2. Wait for the IPC listener and the deferred hide to complete.
    //    The deferred hide fires via Timer::single_shot(ZERO) after the
    //    event loop starts. We must wait for it before sending show-window
    //    to avoid a race where show fires before the timer hides the window.
    let ready_timeout = Duration::from_secs(30);
    stdout_watcher.wait_for_signal("ipc_listener_ready", ready_timeout);
    stdout_watcher.wait_for_signal("window_hidden_deferred", ready_timeout);

    // 3. Show the window via IPC so the rendering notifier fires
    //    (hidden windows don't trigger Slint rendering callbacks).
    send_ipc_command(&socket_name, "show-window");
    stdout_watcher.wait_for_signal("window_shown", Duration::from_secs(10));
    stdout_watcher.wait_for_signal("first_frame_rendered", ready_timeout);

    // 4. Hide the window via IPC (actual window.hide()).
    send_ipc_command(&socket_name, "hide-window");
    stdout_watcher.wait_for_signal("window_hidden", Duration::from_secs(10));

    // 5. Send quit via IPC and wait for graceful exit.
    send_ipc_command(&socket_name, "quit");
    let output = wait_with_timeout(guard.take(), Duration::from_secs(10));

    // 6. Assert exit code 0.
    assert!(
        output.status.success(),
        "process exited with non-zero status: {:?}",
        output.status
    );

    // 7. Assert expected log messages are present.
    let stderr = watcher.lines().join("\n");
    assert!(
        stderr.contains("startup mode: tray"),
        "stderr missing 'startup mode: tray':\n{stderr}"
    );
    assert!(
        stderr.contains("ipc listener ready"),
        "stderr missing 'ipc listener ready':\n{stderr}"
    );
    assert!(
        stderr.contains("quit_event_loop"),
        "stderr missing 'quit_event_loop':\n{stderr}"
    );

    // 8. Assert no ERROR lines in stderr.
    for line in watcher.lines() {
        assert!(!line.contains(" ERROR "), "found ERROR in stderr:\n{line}");
    }
}

/// Find the app's session-end listener window and check it belongs to `pid`.
///
/// Unlike the rest of the suite this case is `cfg`-gated rather than skipped at
/// runtime: the messages Windows sends before a reboot are Win32 calls, so the
/// body does not compile anywhere else. The decision they drive is unit-tested
/// on every platform in `session_end`.
#[cfg(windows)]
fn find_session_listener(pid: u32, timeout: Duration) -> isize {
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowThreadProcessId};

    let class: Vec<u16> = sunlit_earth::session_end::CLASS_NAME
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let deadline = Instant::now() + timeout;
    loop {
        // SAFETY: a NUL-terminated class name that outlives the call, and a
        // null window name, which means "any title".
        #[allow(unsafe_code)]
        let hwnd = unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) };
        if !hwnd.is_null() {
            let mut owner = 0u32;
            // SAFETY: a window handle just returned by FindWindowW and a
            // writable u32 for the process id.
            #[allow(unsafe_code)]
            unsafe {
                GetWindowThreadProcessId(hwnd, &raw mut owner);
            }
            assert_eq!(owner, pid, "the listener window belongs to another process");
            return hwnd as isize;
        }
        assert!(
            Instant::now() < deadline,
            "no session-end listener window appeared"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A reboot, as Windows conducts it: ask, then tell.
#[cfg(windows)]
#[test]
#[ignore = "requires desktop environment and GPU"]
#[serial]
fn test_session_end_shuts_down_promptly() {
    use windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW;

    let socket_name = unique_socket_name();
    let mut guard = ChildGuard::new(
        Command::new(binary())
            .env("SUNLIT_EARTH_NO_CLOUDS", "1")
            .env("SUNLIT_EARTH_CONFIG", isolated_config_path())
            .env("SUNLIT_EARTH_METRICS_DIR", isolated_state_dir())
            .args([
                "--log-level",
                "debug",
                "--tray-start",
                "hidden",
                "--ipc-socket",
                &socket_name,
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn sunlit-earth binary"),
    );
    let child = guard.child.as_mut().unwrap();
    let pid = child.id();
    let stdout_watcher = StdoutWatcher::new(child);
    let watcher = StderrWatcher::new(child);

    let ready_timeout = Duration::from_secs(30);
    stdout_watcher.wait_for_signal("ipc_listener_ready", ready_timeout);
    stdout_watcher.wait_for_signal("window_hidden_deferred", ready_timeout);

    let hwnd = find_session_listener(pid, Duration::from_secs(10));

    // SAFETY: a window handle whose owning process this test just verified, and
    // two documented messages. `SendMessageW` returns once it has been handled.
    #[allow(unsafe_code)]
    let permitted = unsafe {
        SendMessageW(
            hwnd as _,
            sunlit_earth::session_end::WM_QUERYENDSESSION,
            1,
            0,
        )
    };
    assert_eq!(
        permitted, 1,
        "vetoing is what makes Windows name the app as blocking the reboot"
    );

    let started = Instant::now();
    // SAFETY: as above. wParam 1 means the session really is ending.
    #[allow(unsafe_code)]
    unsafe {
        SendMessageW(hwnd as _, sunlit_earth::session_end::WM_ENDSESSION, 1, 0);
    }

    let output = wait_with_timeout(guard.take(), Duration::from_secs(15));
    let elapsed = started.elapsed();
    assert!(
        output.status.success(),
        "process exited with non-zero status: {:?}",
        output.status
    );
    // The point of the case is that it is quick: Windows shows its shutdown
    // screen while it waits, and kills the process after about five seconds.
    assert!(
        elapsed < Duration::from_secs(5),
        "the app took {elapsed:?} to exit after the session ended"
    );

    let stderr = watcher.lines().join("\n");
    assert!(
        stderr.contains("windows is ending the session"),
        "stderr missing the session-end log line:\n{stderr}"
    );
    for line in watcher.lines() {
        assert!(!line.contains(" ERROR "), "found ERROR in stderr:\n{line}");
    }
}

#[test]
#[ignore = "requires desktop environment and GPU"]
#[serial]
fn test_windowed_mode_graceful_shutdown() {
    let socket_name = unique_socket_name();

    // 1. Spawn in windowed mode with IPC enabled.
    let mut guard = ChildGuard::new(
        Command::new(binary())
            .env("SUNLIT_EARTH_NO_CLOUDS", "1")
            .env("SUNLIT_EARTH_CONFIG", isolated_config_path())
            .env("SUNLIT_EARTH_METRICS_DIR", isolated_state_dir())
            .args([
                "--mode",
                "window",
                "--log-level",
                "debug",
                "--ipc-socket",
                &socket_name,
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn sunlit-earth binary"),
    );
    let child = guard.child.as_mut().unwrap();

    let stdout_watcher = StdoutWatcher::new(child);
    let watcher = StderrWatcher::new(child);

    // 2. Wait for readiness.
    let ready_timeout = Duration::from_secs(30);
    stdout_watcher.wait_for_signal("ipc_listener_ready", ready_timeout);
    watcher.wait_for_log("first frame rendered", ready_timeout);

    // 3. Send quit via IPC.
    send_ipc_command(&socket_name, "quit");
    let output = wait_with_timeout(guard.take(), Duration::from_secs(10));

    // 4. Assert exit code 0 and expected log messages.
    assert!(
        output.status.success(),
        "process exited with non-zero status: {:?}",
        output.status
    );

    let stderr = watcher.lines().join("\n");
    assert!(
        stderr.contains("startup mode: windowed"),
        "stderr missing 'startup mode: windowed':\n{stderr}"
    );
    assert!(
        stderr.contains("quit_event_loop"),
        "stderr missing 'quit_event_loop':\n{stderr}"
    );

    // 5. No errors in log.
    for line in watcher.lines() {
        assert!(!line.contains(" ERROR "), "found ERROR in stderr:\n{line}");
    }
}

#[test]
#[ignore = "requires desktop environment and GPU"]
#[serial]
fn test_single_instance_second_exits() {
    if !tray_supported() {
        skip_case(
            "test_single_instance_second_exits",
            "the app takes the single-instance mutex in tray mode only, so there \
             is nothing to enforce in a session that hosts no tray",
        );
        return;
    }
    let socket_name = unique_socket_name();

    // 1. Spawn instance A in tray mode with IPC (acquires the single-instance mutex).
    let mut guard_a = ChildGuard::new(
        Command::new(binary())
            .env("SUNLIT_EARTH_NO_CLOUDS", "1")
            .env("SUNLIT_EARTH_CONFIG", isolated_config_path())
            .env("SUNLIT_EARTH_METRICS_DIR", isolated_state_dir())
            .args(["--log-level", "debug", "--ipc-socket", &socket_name])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn instance A"),
    );
    let instance_a = guard_a.child.as_mut().unwrap();

    let stdout_watcher_a = StdoutWatcher::new(instance_a);
    let _watcher_a = StderrWatcher::new(instance_a);

    // 2. Wait for instance A to be ready.
    let ready_timeout = Duration::from_secs(30);
    stdout_watcher_a.wait_for_signal("ipc_listener_ready", ready_timeout);

    // 3. Spawn instance B with the SAME ipc-socket name so it uses the
    //    same scoped mutex as A (otherwise it checks the default mutex
    //    which may conflict with a real running instance).
    let instance_b = Command::new(binary())
        .env("SUNLIT_EARTH_NO_CLOUDS", "1")
        .env("SUNLIT_EARTH_CONFIG", isolated_config_path())
        .env("SUNLIT_EARTH_METRICS_DIR", isolated_state_dir())
        .args(["--log-level", "debug", "--ipc-socket", &socket_name])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn instance B");

    // 4. Wait for B with a 10-second timeout.
    let output_b = wait_with_timeout(instance_b, Duration::from_secs(10));

    // 5. B should have exited with code 0.
    assert!(
        output_b.status.success(),
        "instance B exited with non-zero status: {:?}",
        output_b.status
    );

    // 6. B's stderr should contain the single-instance detection message.
    let stderr_b = String::from_utf8_lossy(&output_b.stderr);
    assert!(
        stderr_b.contains("another instance is already running"),
        "instance B stderr missing single-instance message:\n{stderr_b}"
    );

    // 7. Clean up instance A via IPC quit.
    send_ipc_command(&socket_name, "quit");
    let output_a = wait_with_timeout(guard_a.take(), Duration::from_secs(10));

    // 8. Instance A should exit with code 0.
    assert!(
        output_a.status.success(),
        "instance A exited with non-zero status: {:?}",
        output_a.status
    );
}

/// Verify that the Slint event loop stays alive and the window can be
/// shown again after being hidden via tray close.
///
/// The critical assertion: after hiding, the `show-window` IPC command
/// is processed. If the event loop dies after hide, this times out.
#[test]
#[ignore = "requires desktop environment and GPU"]
#[serial]
fn test_tray_hide_show_cycle() {
    let socket_name = unique_socket_name();

    // 1. Spawn the binary in tray mode with IPC enabled.
    let mut guard = ChildGuard::new(
        Command::new(binary())
            .env("SUNLIT_EARTH_NO_CLOUDS", "1")
            .env("SUNLIT_EARTH_CONFIG", isolated_config_path())
            .env("SUNLIT_EARTH_METRICS_DIR", isolated_state_dir())
            .args(["--log-level", "debug"])
            .args(lifecycle_mode_args())
            .args(["--ipc-socket", &socket_name])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn sunlit-earth binary"),
    );
    let child = guard.child.as_mut().unwrap();

    let stdout_watcher = StdoutWatcher::new(child);
    let stderr_watcher = StderrWatcher::new(child);

    // 2. Wait for the IPC listener and first frame via stdout signals.
    let ready_timeout = Duration::from_secs(30);
    stdout_watcher.wait_for_signal("ipc_listener_ready", ready_timeout);
    stdout_watcher.wait_for_signal("first_frame_rendered", ready_timeout);

    // 3. Hide the window via IPC (actual window.hide()).
    send_ipc_command(&socket_name, "hide-window");
    stdout_watcher.wait_for_signal("window_hidden", Duration::from_secs(10));

    // 4. KEY TEST: Show again. If the event loop died after hide,
    //    this command will never be processed and the test times out.
    send_ipc_command(&socket_name, "show-window");
    stdout_watcher.wait_for_signal("window_shown", Duration::from_secs(10));

    // 5. Send quit via IPC and wait for graceful exit.
    send_ipc_command(&socket_name, "quit");
    let output = wait_with_timeout(guard.take(), Duration::from_secs(10));

    // 6. Assert exit code 0.
    assert!(
        output.status.success(),
        "process exited with non-zero status: {:?}",
        output.status
    );

    // 7. Assert no ERROR lines in stderr.
    for line in stderr_watcher.lines() {
        assert!(!line.contains(" ERROR "), "found ERROR in stderr:\n{line}");
    }
}

/// Verify that GPU resources persist after `window.hide()` and that
/// `export_wallpaper_image()` works on a hidden window.
///
/// This is a gate test for the wallpaper scheduler feature: if the GPU
/// export fails after hide, automatic wallpaper refresh in tray mode
/// is not viable without a show-render-hide cycle.
#[test]
#[ignore = "requires desktop environment and GPU"]
#[serial]
fn test_gpu_persistence_after_hide() {
    let socket_name = unique_socket_name();

    // 1. Spawn in tray mode with visible window and IPC enabled.
    let mut guard = ChildGuard::new(
        Command::new(binary())
            .env("SUNLIT_EARTH_NO_CLOUDS", "1")
            .env("SUNLIT_EARTH_CONFIG", isolated_config_path())
            .env("SUNLIT_EARTH_METRICS_DIR", isolated_state_dir())
            .args(["--log-level", "debug"])
            .args(lifecycle_mode_args())
            .args(["--ipc-socket", &socket_name])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn sunlit-earth binary"),
    );
    let child = guard.child.as_mut().unwrap();

    let stdout_watcher = StdoutWatcher::new(child);
    let stderr_watcher = StderrWatcher::new(child);

    // 2. Wait for GPU init and first frame.
    let ready_timeout = Duration::from_secs(30);
    stdout_watcher.wait_for_signal("ipc_listener_ready", ready_timeout);
    stdout_watcher.wait_for_signal("first_frame_rendered", ready_timeout);

    // 3. Hide the window — GPU resources should persist.
    send_ipc_command(&socket_name, "hide-window");
    stdout_watcher.wait_for_signal("window_hidden", Duration::from_secs(10));

    // 4. CRITICAL ASSERTION: GPU export must succeed on a hidden window.
    //    If RenderingTeardown fires on hide, this would fail with
    //    "GPU not initialized".
    send_ipc_command(&socket_name, "export-test");
    stdout_watcher.wait_for_signal("export_test_ok", Duration::from_secs(30));

    // 5. Clean exit.
    send_ipc_command(&socket_name, "quit");
    let output = wait_with_timeout(guard.take(), Duration::from_secs(10));

    assert!(
        output.status.success(),
        "process exited with non-zero status: {:?}",
        output.status
    );

    // 6. No errors in log.
    for line in stderr_watcher.lines() {
        assert!(!line.contains(" ERROR "), "found ERROR in stderr:\n{line}");
    }
}

/// Verify that cloud updates arriving while the window is hidden do not grow
/// process memory without bound.
///
/// This is the regression test for the tray-mode memory leak: decoded cloud
/// frames used to accumulate in an unbounded channel that was drained only from
/// `BeforeRendering`, which stops firing once the window is hidden. The test
/// points the fetcher at a local stub server, hides the window, publishes 15
/// updates, and asserts both that private bytes stay bounded and that the
/// updates still reach the GPU while hidden.
#[test]
#[ignore = "requires desktop environment and GPU"]
#[serial]
#[allow(clippy::too_many_lines)]
fn test_hidden_window_cloud_updates_do_not_grow_memory() {
    /// Decoded size is 2048 * 1024 * 4 = 8 MiB per frame.
    const FIXTURE_WIDTH: u32 = 2048;
    const FIXTURE_HEIGHT: u32 = 1024;
    const UPDATES: u64 = 15;
    const GROWTH_LIMIT_BYTES: u64 = 40 * 1024 * 1024;
    /// Long enough for at least one tick of the 5 s drain timer.
    const SETTLE: Duration = Duration::from_secs(8);

    let socket_name = unique_socket_name();
    // RAII so a failing assertion below still cleans the directory up.
    let temp_dir = TempDirGuard::new();
    let cache_dir = temp_dir.path().join("cache");
    let textures_dir = temp_dir.path().join("textures");
    fs::create_dir_all(&cache_dir).expect("failed to create stub cache dir");
    fs::create_dir_all(&textures_dir).expect("failed to create empty textures dir");

    let (port, stub) = spawn_cloud_stub(cloud_fixture_jpeg(FIXTURE_WIDTH, FIXTURE_HEIGHT));

    // 1. Spawn in windowed mode: no tray icon and no single-instance mutex,
    //    while hiding over IPC still reproduces the exact leak condition.
    //    The empty textures directory leaves the JXL slots unloaded, so cloud
    //    frames are the only large allocations in flight.
    let mut guard = ChildGuard::new(
        Command::new(binary())
            .env("SUNLIT_EARTH_CONFIG", isolated_config_path())
            .env("SUNLIT_EARTH_METRICS_DIR", isolated_state_dir())
            .env("SUNLIT_EARTH_SYNC_LOG", "1")
            .env(
                "SUNLIT_EARTH_CLOUD_URL",
                format!("http://127.0.0.1:{port}/clouds.jpg"),
            )
            .env("SUNLIT_EARTH_CLOUD_POLL_SECS", "1")
            .env("SUNLIT_EARTH_CACHE_DIR", &cache_dir)
            .args([
                "--mode",
                "window",
                "--log-level",
                "debug",
                "--ipc-socket",
                &socket_name,
                "--textures-dir",
                textures_dir.to_str().expect("non-UTF-8 temp path"),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn sunlit-earth binary"),
    );
    let child = guard.child.as_mut().unwrap();
    let stdout_watcher = StdoutWatcher::new(child);
    let stderr_watcher = StderrWatcher::new(child);

    // 2. Wait for startup and for the first cloud image to be downloaded and
    //    uploaded to the GPU while the window is still visible.
    let ready_timeout = Duration::from_mins(1);
    stdout_watcher.wait_for_signal("ipc_listener_ready", ready_timeout);
    stdout_watcher.wait_for_signal("first_frame_rendered", ready_timeout);
    wait_for_downloads(&stub, 1, ready_timeout);
    stderr_watcher.wait_for_log("GPU texture created", ready_timeout);

    // 3. Hide the window. From here on BeforeRendering no longer fires.
    send_ipc_command(&socket_name, "hide-window");
    stdout_watcher.wait_for_signal("window_hidden", Duration::from_secs(15));

    // 4. Let everything in flight settle, then take the baseline sample.
    std::thread::sleep(SETTLE);
    let baseline = query_memory(&socket_name, &stdout_watcher);
    let stderr_cursor = stderr_watcher.line_count();

    // 5. Publish new cloud images, waiting for each download to be served.
    for _ in 0..UPDATES {
        let target = stub.gets.load(Ordering::SeqCst) + 1;
        stub.version.fetch_add(1, Ordering::SeqCst);
        wait_for_downloads(&stub, target, Duration::from_secs(15));
    }

    // 6. Settle again so the last update is processed, then take the end sample.
    std::thread::sleep(SETTLE);
    let end = query_memory(&socket_name, &stdout_watcher);

    // 7. The GPU must still be usable while hidden.
    send_ipc_command(&socket_name, "export-test");
    stdout_watcher.wait_for_signal("export_test_ok", Duration::from_secs(30));

    // 8. Close the log range covering the hidden phase, then show again and
    //    shut down cleanly before asserting, so a failing run still produces a
    //    complete log instead of a killed process. Showing the window drains
    //    everything that was parked, so the cursor must be taken before it.
    let stderr_cursor_end = stderr_watcher.line_count();
    send_ipc_command(&socket_name, "show-window");
    stdout_watcher.wait_for_signal("window_shown", Duration::from_secs(15));
    send_ipc_command(&socket_name, "quit");
    let output = wait_with_timeout(guard.take(), Duration::from_secs(15));

    let private_growth = end.private.saturating_sub(baseline.private);
    let rss_growth = end.rss.saturating_sub(baseline.rss);
    let created_while_hidden = stderr_watcher.lines()[stderr_cursor..stderr_cursor_end]
        .iter()
        .filter(|line| line.contains("GPU texture created"))
        .count();

    println!(
        "baseline: rss={:.1} MiB private={:.1} MiB",
        mib(baseline.rss),
        mib(baseline.private)
    );
    println!(
        "after {UPDATES} hidden cloud updates: rss={:.1} MiB private={:.1} MiB peak_rss={:.1} MiB",
        mib(end.rss),
        mib(end.private),
        mib(end.peak_rss)
    );
    println!(
        "growth: private={:.1} MiB rss={:.1} MiB (limit {:.0} MiB), \
         GPU textures created while hidden: {created_while_hidden}",
        mib(private_growth),
        mib(rss_growth),
        mib(GROWTH_LIMIT_BYTES)
    );

    // 9. The leak assertion. Private bytes (commit charge) is used instead of
    //    RSS because working-set trimming can hide heap growth from RSS.
    assert!(
        private_growth < GROWTH_LIMIT_BYTES,
        "private bytes grew by {:.1} MiB across {UPDATES} cloud updates while hidden \
         (limit {:.0} MiB, RSS grew by {:.1} MiB)",
        mib(private_growth),
        mib(GROWTH_LIMIT_BYTES),
        mib(rss_growth)
    );

    // 10. Updates must be processed while hidden, not merely discarded.
    assert!(
        created_while_hidden > 0,
        "no 'GPU texture created' line after the window was hidden: \
         cloud updates are not reaching the GPU while hidden"
    );

    assert!(
        output.status.success(),
        "process exited with non-zero status: {:?}",
        output.status
    );

    for line in stderr_watcher.lines() {
        assert!(!line.contains(" ERROR "), "found ERROR in stderr:\n{line}");
    }
}

/// Verify that `memory-report` answers with the four sections, and print what
/// it said.
///
/// The printing is the point as much as the assertions are: this is the one
/// place a real report from a real GPU (a host desktop) or a real guest (the VM
/// job) is captured, and both run the suite with `--nocapture`. The assertions
/// are on the structure only, because the report promises stable section names
/// and nothing else.
///
/// Not gated on anything. The report degrades section by section on its own: a
/// backend with no allocator report says so on the line where the allocations
/// would be, which the prefix check below accepts.
#[test]
#[ignore = "requires desktop environment and GPU"]
#[serial]
fn test_memory_report() {
    let socket_name = unique_socket_name();
    let mut guard = ChildGuard::new(
        Command::new(binary())
            .env("SUNLIT_EARTH_NO_CLOUDS", "1")
            .env("SUNLIT_EARTH_CONFIG", isolated_config_path())
            .env("SUNLIT_EARTH_METRICS_DIR", isolated_state_dir())
            .args(["--log-level", "debug"])
            .args(lifecycle_mode_args())
            .args(["--ipc-socket", &socket_name])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn sunlit-earth binary"),
    );
    let child = guard.child.as_mut().unwrap();
    let stdout_watcher = StdoutWatcher::new(child);
    let stderr_watcher = StderrWatcher::new(child);

    let ready_timeout = Duration::from_mins(1);
    stdout_watcher.wait_for_signal("ipc_listener_ready", ready_timeout);
    stdout_watcher.wait_for_signal("first_frame_rendered", ready_timeout);

    let report = memory_report(&socket_name, &stdout_watcher);

    println!("--- memory report ---");
    for line in &report {
        println!("{line}");
    }
    println!("--- end of memory report ---");

    send_ipc_command(&socket_name, "quit");
    let output = wait_with_timeout(guard.take(), Duration::from_secs(15));

    // Section headers start at column zero; rows beneath them are indented.
    let headers: Vec<&String> = report
        .iter()
        .filter(|line| !line.starts_with(' ') && !line.starts_with("memory report"))
        .collect();
    assert_eq!(
        headers.len(),
        REPORT_SECTIONS.len(),
        "expected {} sections, got:\n{}",
        REPORT_SECTIONS.len(),
        report.join("\n")
    );
    for (header, expected) in headers.iter().zip(REPORT_SECTIONS) {
        assert!(
            header.starts_with(expected),
            "expected a '{expected}' section, got '{header}'"
        );
    }
    assert!(
        report.iter().any(|line| line.contains("render_texture")),
        "the expected table should list the preview render target:\n{}",
        report.join("\n")
    );

    assert!(
        output.status.success(),
        "process exited with non-zero status: {:?}",
        output.status
    );
    for line in stderr_watcher.lines() {
        assert!(!line.contains(" ERROR "), "found ERROR in stderr:\n{line}");
    }
}

/// Ask this desktop what its wallpaper is, and check the answer is ours.
///
/// Answers with the file name the desktop was found to be holding, so that a
/// caller which publishes twice can require the second answer to differ from the
/// first. `None` where this desktop's setter has no store to ask.
///
/// A setter that exited zero is a weaker claim than a wallpaper that changed,
/// and the difference is not theoretical: the XFCE backend passed this case
/// while the desktop went on showing xfdesktop's own default, because it wrote a
/// property no current xfdesktop reads. Only a screenshot caught that, and a
/// screenshot is not standing regression cover. Reading the setting back is the
/// part of it a test can do by itself.
///
/// The read-back is derived from the writes the sink performs rather than
/// written out again, so a row that sets the wrong key reads the wrong key back
/// and fails here instead of agreeing with itself. Two backends have no store to
/// ask, Plasma's tool and `LXQt`'s file manager, and those say so rather than
/// failing.
///
/// Derivation alone would not have caught the XFCE case, though, and that is
/// what the last assertion is for: the old row wrote the properties the session
/// listed, so reading those same properties back would have found the path in
/// them and agreed that all was well. What was missing is the property named
/// after the connected monitor, which is the only one xfdesktop reads. So a
/// desktop whose settings are named after its own monitors has to hold the image
/// in one of those, and nothing else counts.
#[cfg(target_os = "linux")]
fn assert_the_desktop_holds_the_wallpaper() -> Option<String> {
    let image = sunlit_core::wallpaper::published_wallpaper_file()
        .expect("a local data directory")
        .expect("the setter said it set a wallpaper, so one was written");
    // The file name rather than the whole path, because what a write carries is
    // the path in that desktop's own spelling: a `file://` URI for the gsettings
    // rows and a plain path for the rest.
    let name = image
        .file_name()
        .expect("the wallpaper is a file")
        .to_string_lossy()
        .into_owned();
    let backend = sunlit_core::desktop::detect_current()
        .expect("a session with no backend could not have got this far");

    let discovered = match backend.discovery() {
        Some(query) => read_setting(&query),
        None => String::new(),
    };
    let monitors: Vec<String> = sunlit_core::display::outputs()
        .unwrap_or_default()
        .into_iter()
        .map(|output| output.name)
        .collect();

    let mut holders: Vec<String> = Vec::new();
    for command in backend.commands(&image, &discovered, &monitors) {
        // The fill-mode writes carry a mode rather than a path, and the mode is
        // not what this is about.
        let Some(written) = command.args.iter().find(|arg| arg.contains(&name)) else {
            continue;
        };
        let written = written.clone();
        let Some(query) = readback_of(&command) else {
            skip_case(
                "test_set_wallpaper's read-back",
                &format!(
                    "{}'s setter is `{}`, which sets a wallpaper and has nothing \
                     to ask what one is",
                    backend.desktop, command.program
                ),
            );
            return None;
        };
        let value = read_setting(&query);
        assert!(
            value.contains(&written),
            "{} reports its wallpaper as {value:?} after the setter said it set \
             {written}: `{} {}`",
            backend.desktop,
            query.program,
            query.args.join(" ")
        );
        holders.push(query.args.join(" "));
    }
    assert!(
        !holders.is_empty(),
        "{} set a wallpaper without writing the image anywhere",
        backend.desktop
    );

    // The XFCE clause, expressed as what makes XFCE different rather than by
    // name: asking the session which properties it has is the same thing as
    // those properties being named after this session's own monitors.
    if backend.discovery().is_some() {
        assert!(
            !monitors.is_empty(),
            "{} names its settings after the monitors, and xrandr named none",
            backend.desktop
        );
        assert!(
            monitors.iter().any(|monitor| holders
                .iter()
                .any(|holder| holder.contains(&format!("monitor{monitor}")))),
            "{} holds the wallpaper in {holders:?}, none of which is named after \
             a connected monitor ({monitors:?}), which is the only kind its \
             desktop reads",
            backend.desktop
        );
    }

    println!(
        "{} reports the app's {name} as its wallpaper, read back from {} \
         setting(s): {holders:?}",
        backend.desktop,
        holders.len()
    );
    Some(name)
}

/// The command that reads back what one of the sink's writes set.
///
/// `None` where the desktop's setter is a one-way action: `plasma-apply-wallpaperimage`
/// and `pcmanfm-qt --set-wallpaper` both take an image and neither answers with
/// one.
#[cfg(target_os = "linux")]
fn readback_of(
    command: &sunlit_core::desktop::Invocation,
) -> Option<sunlit_core::desktop::Invocation> {
    let args: Vec<String> = match command.program {
        // `set <schema> <key> <value>` is read back by `get <schema> <key>`.
        "gsettings" => match command.args.as_slice() {
            [set, schema, key, _value] if set == "set" => {
                vec!["get".to_owned(), schema.clone(), key.clone()]
            }
            _ => return None,
        },
        // The channel and the property, which is everything before the write:
        // `-s` carries the value and `-n -t <type>` creates the property.
        "xfconf-query" => command
            .args
            .iter()
            .take_while(|arg| *arg != "-n" && *arg != "-s")
            .cloned()
            .collect(),
        _ => return None,
    };
    Some(sunlit_core::desktop::Invocation {
        program: command.program,
        args,
    })
}

/// Run one read-back and answer with its output, trimmed of the quoting
/// `gsettings` puts around a string.
#[cfg(target_os = "linux")]
fn read_setting(query: &sunlit_core::desktop::Invocation) -> String {
    let out = Command::new(query.program)
        .args(&query.args)
        .output()
        .unwrap_or_else(|e| panic!("cannot run {}: {e}", query.program));
    assert!(
        out.status.success(),
        "{} {} failed: {}",
        query.program,
        query.args.join(" "),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .trim_matches('\'')
        .to_owned()
}

/// Verify that the app renders and publishes a real desktop wallpaper.
///
/// This is the one case that changes something outside the process, which is
/// why it is opt-in rather than platform-gated: it is harmless in a throwaway
/// VM and rude on a developer's desktop, and those are the same Windows. The
/// VM job sets `SUNLIT_EARTH_E2E_WALLPAPER`; nothing else does, so a plain
/// `cargo e2e` skips it and says so.
///
/// What it exercises is the whole path the "Set as Wallpaper" button uses:
/// render at the sink's native resolution, read back, encode a PNG, and hand
/// it to the OS. The engine reports the outcome on its own channel, so the
/// signal waited for here is the completion rather than the request. It presses
/// the button twice, because a wallpaper that changes once and then stops is the
/// shape this feature fails in.
#[test]
#[ignore = "requires desktop environment and GPU"]
#[serial]
fn test_set_wallpaper() {
    // `check_supported` is the product's own answer, and what it can be checked
    // against differs by platform. Windows has one setter and every session has
    // it, so absence there is a regression. A platform with no setter at all
    // must not claim one. Linux has a setter per desktop, so the answer belongs
    // to the session rather than to the build, and the guarantee is the narrower
    // one below.
    if cfg!(target_os = "windows") {
        assert!(
            wallpaper_supported(),
            "the wallpaper sink refuses on the platform that ships it"
        );
    }
    if !WALLPAPER_PLATFORM {
        assert!(
            !wallpaper_supported(),
            "the wallpaper sink claims support on a platform with no setter"
        );
    }

    let opted_in = std::env::var_os(WALLPAPER_OPT_IN).is_some();
    // The opt-in is set by the guest jobs and by nothing else, and a guest job
    // runs inside a desktop session that was chosen for having a setter. So on
    // Linux this is where a broken backend has to fail rather than skip: without
    // it, a session whose setter went missing would report itself unsupported
    // and this case would quietly pass.
    if opted_in && cfg!(target_os = "linux") {
        assert!(
            wallpaper_supported(),
            "this run opted in to replacing the wallpaper, so its desktop is one \
             that can: {:?} reports no setter",
            std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default()
        );
    }
    if !wallpaper_supported() {
        skip_case(
            "test_set_wallpaper",
            "the wallpaper sink reports no setter for this session",
        );
        return;
    }
    if !opted_in {
        skip_case(
            "test_set_wallpaper",
            "this case replaces the desktop wallpaper, so it runs only where \
             that is harmless; the VM job sets SUNLIT_EARTH_E2E_WALLPAPER",
        );
        return;
    }

    let socket_name = unique_socket_name();
    let mut guard = ChildGuard::new(
        Command::new(binary())
            .env("SUNLIT_EARTH_NO_CLOUDS", "1")
            .env("SUNLIT_EARTH_CONFIG", isolated_config_path())
            .env("SUNLIT_EARTH_METRICS_DIR", isolated_state_dir())
            .args(["--log-level", "debug"])
            .args(lifecycle_mode_args())
            .args(["--ipc-socket", &socket_name])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn sunlit-earth binary"),
    );
    let child = guard.child.as_mut().unwrap();
    let stdout_watcher = StdoutWatcher::new(child);
    let stderr_watcher = StderrWatcher::new(child);

    let ready_timeout = Duration::from_secs(30);
    stdout_watcher.wait_for_signal("ipc_listener_ready", ready_timeout);
    stdout_watcher.wait_for_signal("first_frame_rendered", ready_timeout);

    // Twice, because the second publish is its own case: a desktop keys the
    // wallpaper it is showing on the path it was handed, so a frame written to
    // the path already in that setting is one nothing reloads. A setter that
    // succeeded on a first publish and changed nothing on a second is exactly
    // what a single pass here cannot tell apart from working.
    let mut published: Vec<std::path::PathBuf> = Vec::new();
    #[cfg(target_os = "linux")]
    let mut held: Vec<Option<String>> = Vec::new();
    for pass in 1..=2 {
        // The full-resolution render, readback, and PNG encode take longer than
        // a preview frame, and on a software adapter in a VM longer again.
        let from = stdout_watcher.line_count();
        send_ipc_command(&socket_name, "set-wallpaper");
        let line =
            stdout_watcher.wait_for_signal_line_from("wallpaper_", from, Duration::from_mins(2));
        assert!(
            line.contains("wallpaper_set"),
            "publish {pass}: the engine reported a failure instead: {line}"
        );
        published.push(
            sunlit_core::wallpaper::published_wallpaper_file()
                .expect("a local data directory")
                .expect("the engine reported a wallpaper, so one was written"),
        );

        // Everything above is the app's own account of what it did. This is the
        // desktop's.
        #[cfg(target_os = "linux")]
        held.push(assert_the_desktop_holds_the_wallpaper());
    }
    assert_ne!(
        published[0], published[1],
        "the second publish wrote the path the desktop was already showing, \
         which is a wallpaper that does not visibly change"
    );
    // And the desktop stored the new one, where its setter has a store to ask.
    #[cfg(target_os = "linux")]
    if let [Some(first), Some(second)] = held.as_slice() {
        assert_ne!(
            first, second,
            "the desktop reports the same wallpaper after both publishes"
        );
    }

    send_ipc_command(&socket_name, "quit");
    let output = wait_with_timeout(guard.take(), Duration::from_secs(15));
    assert!(
        output.status.success(),
        "process exited with non-zero status: {:?}",
        output.status
    );

    let stderr = stderr_watcher.lines().join("\n");
    assert!(
        stderr.contains("wallpaper updated"),
        "stderr missing 'wallpaper updated':\n{stderr}"
    );
    for line in stderr_watcher.lines() {
        assert!(!line.contains(" ERROR "), "found ERROR in stderr:\n{line}");
    }
}
