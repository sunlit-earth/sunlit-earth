//! End-to-end render export tests for the Sunlit Earth binary.
//!
//! All tests are marked `#[ignore]` because they require a desktop environment
//! and GPU. Run with `cargo test --test e2e -- --ignored`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use image::GenericImageView;
use serial_test::serial;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The compiled binary path, resolved by Cargo at build time.
const BINARY: &str = env!("CARGO_BIN_EXE_sunlit-earth");

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

    // 3. Wait for the process to exit (60s timeout).
    let output = wait_with_timeout(child, Duration::from_secs(60));

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

/// Spawn the binary, let it run for `run_duration`, then kill it and return
/// the collected stderr. Unlike `wait_with_timeout`, this does not panic on
/// timeout — the kill is the expected outcome for long-running modes.
fn spawn_run_and_kill(args: &[&str], run_duration: Duration) -> (bool, String) {
    let mut child = Command::new(BINARY)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn sunlit-earth binary");

    std::thread::sleep(run_duration);

    // The process should still be alive (tray/windowed mode).
    let was_alive = child.try_wait().expect("error polling child").is_none();
    let _ = child.kill();
    let output = child.wait_with_output().expect("failed to collect output");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (was_alive, stderr)
}

#[test]
#[ignore = "requires desktop environment and GPU"]
#[serial]
fn test_tray_mode_starts_and_can_be_killed() {
    // Default launch (no extra args) starts in tray mode.
    let (was_alive, stderr) = spawn_run_and_kill(
        &["--log-level", "debug"],
        Duration::from_secs(3),
    );

    // The process should have been alive when we killed it (tray keeps it running).
    assert!(was_alive, "process exited before kill — tray mode should keep it running");

    // Startup banner should be present.
    assert!(
        stderr.contains("sunlit earth v"),
        "stderr missing startup banner:\n{stderr}"
    );

    // No errors in log.
    for line in stderr.lines() {
        assert!(
            !line.contains(" ERROR "),
            "found ERROR in stderr:\n{line}"
        );
    }
}

#[test]
#[ignore = "requires desktop environment and GPU"]
#[serial]
fn test_windowed_mode_starts() {
    let (was_alive, stderr) = spawn_run_and_kill(
        &["--windowed", "--log-level", "debug"],
        Duration::from_secs(3),
    );

    // Windowed mode also stays alive (it just doesn't have a tray icon).
    assert!(was_alive, "process exited before kill — windowed mode should keep it running");

    // Startup banner should be present.
    assert!(
        stderr.contains("sunlit earth v"),
        "stderr missing startup banner:\n{stderr}"
    );

    // No errors in log.
    for line in stderr.lines() {
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
    // 1. Spawn instance A in tray mode (acquires the single-instance mutex).
    let mut instance_a = Command::new(BINARY)
        .args(["--log-level", "debug"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn instance A");

    // 2. Wait for A to initialize and acquire the mutex.
    std::thread::sleep(Duration::from_secs(3));

    // 3. Spawn instance B (should detect A and exit immediately).
    let instance_b = Command::new(BINARY)
        .args(["--log-level", "debug"])
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

    // 7. Clean up instance A.
    let _ = instance_a.kill();
    let _ = instance_a.wait();
}
