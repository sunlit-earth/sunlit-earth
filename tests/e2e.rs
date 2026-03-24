//! End-to-end screenshot tests for the Sunlit Earth binary.
//!
//! All tests are marked `#[ignore]` because they require a desktop environment
//! and GPU. Run with `cargo test --test e2e -- --ignored`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use image::GenericImageView;

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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires desktop environment and GPU"]
fn test_binary_exists() {
    let path = Path::new(BINARY);
    assert!(
        path.exists(),
        "binary not found at {BINARY} — was the project built?"
    );
}

#[test]
#[ignore = "requires desktop environment and GPU"]
fn test_screenshot_and_exit() {
    // 1. Create a temp directory and screenshot path.
    let temp_dir = create_temp_dir();
    let screenshot_path = temp_dir.join("screenshot.png");

    // 2. Spawn the binary with --screenshot and --log-level info.
    let child = Command::new(BINARY)
        .args([
            "--screenshot",
            screenshot_path.to_str().expect("non-UTF-8 temp path"),
            "--log-level",
            "info",
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

    // 5. Assert the screenshot file exists and is non-empty.
    assert!(
        screenshot_path.exists(),
        "screenshot file was not created at {}",
        screenshot_path.display()
    );
    let file_size = fs::metadata(&screenshot_path)
        .expect("failed to read screenshot metadata")
        .len();
    assert!(
        file_size > 0,
        "screenshot file is empty (0 bytes)"
    );

    // 6. Decode the PNG with the image crate.
    let img = image::open(&screenshot_path).expect("failed to decode screenshot PNG");

    // 7. Assert image dimensions are non-zero.
    let (width, height) = img.dimensions();
    assert!(width > 0 && height > 0, "image has zero dimensions: {width}x{height}");

    // 8. Sample the center pixel — assert it is not pure black.
    let center_pixel = img.to_rgba8().get_pixel(width / 2, height / 2).0;
    let rgb_sum: u32 =
        u32::from(center_pixel[0]) + u32::from(center_pixel[1]) + u32::from(center_pixel[2]);
    assert!(
        rgb_sum > 0,
        "center pixel is pure black ({center_pixel:?}) — globe may not be visible"
    );

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

    // 11. Clean up.
    cleanup_temp_dir(&temp_dir);
}
