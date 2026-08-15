//! End-to-end render export tests for the Sunlit Earth binary.
//!
//! All tests are marked `#[ignore]` because they require a desktop environment
//! and GPU. Run with `cargo test --test e2e -- --ignored`.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex, atomic::{AtomicU64, AtomicUsize, Ordering}};
use std::time::{Duration, Instant};

use image::GenericImageView;
use interprocess::local_socket::{GenericNamespaced, ToNsName};
use interprocess::local_socket::traits::Stream as StreamExt;
use serial_test::serial;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The compiled binary path, resolved by Cargo at build time.
const BINARY: &str = env!("CARGO_BIN_EXE_sunlit-earth");

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
        if let Some(mut child) = self.child.take() {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
                let _ = child.wait();
            }
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

/// A throwaway config path, passed to every spawned binary via
/// `SUNLIT_EARTH_CONFIG`.
///
/// Without it the tests read the developer's real settings from
/// `%LOCALAPPDATA%\SunlitEarth` and, when auto-refresh is enabled there,
/// replace the desktop wallpaper of the machine running the tests.
fn isolated_config_path() -> PathBuf {
    let dir = std::env::temp_dir().join("sunlit_earth_e2e_config");
    let _ = fs::create_dir_all(&dir);
    dir.join(format!(
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

        entries.push(MemoryEntry { context, rss_mb, peak_rss_mb });
    }
    entries
}

/// Extract the (R, G, B) channels of a pixel at the given coordinates.
fn rgb_at(img: &image::RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let p = img.get_pixel(x, y).0;
    [p[0], p[1], p[2]]
}

/// Assert that a pixel is approximately black (background/space).
/// Threshold accounts for slight nightglow/atmosphere bleed at edges.
fn assert_black(rgb: [u8; 3], label: &str) {
    let sum = u32::from(rgb[0]) + u32::from(rgb[1]) + u32::from(rgb[2]);
    assert!(sum < 40, "{label}: expected black, got {rgb:?} (sum={sum})");
}

/// Assert that a pixel is dark ocean on the night side (nearly black).
fn assert_night_ocean(rgb: [u8; 3], label: &str) {
    let sum = u32::from(rgb[0]) + u32::from(rgb[1]) + u32::from(rgb[2]);
    assert!(sum < 40, "{label}: expected dark ocean, got {rgb:?} (sum={sum})");
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
    format!(
        "sunlit-earth-test-{}-{}",
        std::process::id(),
        counter
    )
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
                            lines_clone.lock().expect("stderr watcher lock poisoned").push(l);
                        }
                        Err(_) => break,
                    }
                }
            })
            .expect("failed to spawn stderr watcher thread");

        Self { lines, _thread: thread }
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
        self.lines.lock().expect("stderr watcher lock poisoned").clone()
    }

    /// Number of stderr lines collected so far, usable as a cursor into
    /// a later `lines()` snapshot.
    fn line_count(&self) -> usize {
        self.lines.lock().expect("stderr watcher lock poisoned").len()
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
                            lines_clone.lock().expect("stdout watcher lock poisoned").push(l);
                        }
                        Err(_) => break,
                    }
                }
            })
            .expect("failed to spawn stdout watcher thread");

        Self { lines, _thread: thread }
    }

    /// Block until a `SIGNAL:<name>` line appears in stdout, or panic
    /// after `timeout` elapses.
    fn wait_for_signal(&self, name: &str, timeout: Duration) {
        self.wait_for_signal_line_from(name, 0, timeout);
    }

    /// Number of stdout lines collected so far. Used as a cursor so repeated
    /// queries do not match the reply to an earlier request.
    fn line_count(&self) -> usize {
        self.lines.lock().expect("stdout watcher lock poisoned").len()
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
fn serve_cloud_request(mut stream: TcpStream, jpeg: &[u8], state: &StubState) {
    const INM: &str = "if-none-match:";

    let Ok(peek) = stream.try_clone() else { return };
    let mut reader = BufReader::new(peek);

    let mut request_line = String::new();
    if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
        return;
    }
    let method = request_line.split_whitespace().next().unwrap_or_default().to_owned();

    let mut if_none_match: Option<String> = None;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 {
            break;
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
        format!("HTTP/1.1 304 Not Modified\r\nETag: {etag}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
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
    let path = Path::new(BINARY);
    assert!(
        path.exists(),
        "binary not found at {BINARY} — was the project built?"
    );
}

#[test]
#[ignore = "requires desktop environment and GPU"]
#[serial]
fn test_render_and_exit() {
    // 1. Create a temp directory and output path.
    let temp_dir = create_temp_dir();
    let output_path = temp_dir.join("render.png");
    let config_path = format!("{}/tests/fixtures/e2e_config.toml", env!("CARGO_MANIFEST_DIR"));

    // 2. Spawn the binary with the render subcommand.
    let child = Command::new(BINARY)
        .env("SUNLIT_EARTH_NO_CLOUDS", "1")
        .env("SUNLIT_EARTH_CONFIG", isolated_config_path())
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
            &config_path,
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
    assert!(
        file_size > 0,
        "render output file is empty (0 bytes)"
    );

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

    // Corners should be black (space/background)
    assert_black(rgb_at(&rgba, 0, 0), "top-left corner");
    assert_black(rgb_at(&rgba, 799, 0), "top-right corner");
    assert_black(rgb_at(&rgba, 0, 799), "bottom-left corner");
    assert_black(rgb_at(&rgba, 799, 799), "bottom-right corner");

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
        assert!(
            !line.contains(" ERROR "),
            "found ERROR in stderr:\n{line}"
        );
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
            entry.rss_mb, entry.context
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
            entry.rss_mb, peak
        );
    }

    // 11. Clean up.
    cleanup_temp_dir(&temp_dir);
}

#[test]
#[ignore = "requires desktop environment and GPU"]
#[serial]
fn test_tray_mode_ipc_lifecycle() {
    let socket_name = unique_socket_name();

    // 1. Spawn the binary in tray mode with window hidden and IPC enabled.
    let mut guard = ChildGuard::new(
        Command::new(BINARY)
            .env("SUNLIT_EARTH_NO_CLOUDS", "1")
            .env("SUNLIT_EARTH_CONFIG", isolated_config_path())
            .args([
                "--log-level", "debug",
                "--tray-start", "hidden",
                "--ipc-socket", &socket_name,
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
        assert!(
            !line.contains(" ERROR "),
            "found ERROR in stderr:\n{line}"
        );
    }
}

#[test]
#[ignore = "requires desktop environment and GPU"]
#[serial]
fn test_windowed_mode_graceful_shutdown() {
    let socket_name = unique_socket_name();

    // 1. Spawn in windowed mode with IPC enabled.
    let mut guard = ChildGuard::new(
        Command::new(BINARY)
            .env("SUNLIT_EARTH_NO_CLOUDS", "1")
            .env("SUNLIT_EARTH_CONFIG", isolated_config_path())
            .args([
                "--mode", "window",
                "--log-level", "debug",
                "--ipc-socket", &socket_name,
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
        assert!(
            !line.contains(" ERROR "),
            "found ERROR in stderr:\n{line}"
        );
    }
}

#[test]
#[ignore = "requires desktop environment and GPU"]
#[serial]
fn test_single_instance_second_exits() {
    let socket_name = unique_socket_name();

    // 1. Spawn instance A in tray mode with IPC (acquires the single-instance mutex).
    let mut guard_a = ChildGuard::new(
        Command::new(BINARY)
            .env("SUNLIT_EARTH_NO_CLOUDS", "1")
            .env("SUNLIT_EARTH_CONFIG", isolated_config_path())
            .args([
                "--log-level", "debug",
                "--ipc-socket", &socket_name,
            ])
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
    let instance_b = Command::new(BINARY)
        .env("SUNLIT_EARTH_NO_CLOUDS", "1")
        .env("SUNLIT_EARTH_CONFIG", isolated_config_path())
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
        Command::new(BINARY)
            .env("SUNLIT_EARTH_NO_CLOUDS", "1")
            .env("SUNLIT_EARTH_CONFIG", isolated_config_path())
            .args([
                "--log-level", "debug",
                "--tray-start", "visible",
                "--ipc-socket", &socket_name,
            ])
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
        assert!(
            !line.contains(" ERROR "),
            "found ERROR in stderr:\n{line}"
        );
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
        Command::new(BINARY)
            .env("SUNLIT_EARTH_NO_CLOUDS", "1")
            .env("SUNLIT_EARTH_CONFIG", isolated_config_path())
            .args([
                "--log-level", "debug",
                "--tray-start", "visible",
                "--ipc-socket", &socket_name,
            ])
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
        assert!(
            !line.contains(" ERROR "),
            "found ERROR in stderr:\n{line}"
        );
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
    let temp_dir = create_temp_dir();
    let cache_dir = temp_dir.join("cache");
    let textures_dir = temp_dir.join("textures");
    fs::create_dir_all(&cache_dir).expect("failed to create stub cache dir");
    fs::create_dir_all(&textures_dir).expect("failed to create empty textures dir");

    let (port, stub) = spawn_cloud_stub(cloud_fixture_jpeg(FIXTURE_WIDTH, FIXTURE_HEIGHT));

    // 1. Spawn in windowed mode: no tray icon and no single-instance mutex,
    //    while hiding over IPC still reproduces the exact leak condition.
    //    The empty textures directory leaves the JXL slots unloaded, so cloud
    //    frames are the only large allocations in flight.
    let mut guard = ChildGuard::new(
        Command::new(BINARY)
            .env("SUNLIT_EARTH_CONFIG", isolated_config_path())
            .env("SUNLIT_EARTH_SYNC_LOG", "1")
            .env("SUNLIT_EARTH_CLOUD_URL", format!("http://127.0.0.1:{port}/clouds.jpg"))
            .env("SUNLIT_EARTH_CLOUD_POLL_SECS", "1")
            .env("SUNLIT_EARTH_CACHE_DIR", &cache_dir)
            .args([
                "--mode", "window",
                "--log-level", "debug",
                "--ipc-socket", &socket_name,
                "--textures-dir", textures_dir.to_str().expect("non-UTF-8 temp path"),
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
    let ready_timeout = Duration::from_secs(60);
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

    // 8. Close the log window covering the hidden phase, then show again and
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

    cleanup_temp_dir(&temp_dir);
}
