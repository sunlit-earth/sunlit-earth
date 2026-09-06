//! End-to-end render export tests for the Sunlit Earth binary.
//!
//! All tests are marked `#[ignore]` because they require a desktop environment
//! and GPU. Run with `cargo test --test e2e -- --ignored`.

use std::fs;
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use image::GenericImageView;
use serial_test::serial;
use sunlit_earth::ipc::DisplaysSignal;

mod common;

use common::cloud_stub::{cloud_fixture_jpeg, spawn_cloud_stub, wait_for_downloads};
use common::pixels::{
    assert_blue, assert_greenish, assert_ice, assert_night_land, assert_night_ocean,
    assert_space_corner, assert_yellowish, rgb_at,
};
use common::process::{
    ChildGuard, PUBLISH, READY, REPORT_SECTIONS, Ready, SHUTDOWN, SIGNAL_REPLY, Spawn,
    StderrWatcher, StdoutWatcher, TempDirGuard, WALLPAPER_OPT_IN, WALLPAPER_PLATFORM,
    assert_no_error_lines, binary, fixture, isolated_config_path, isolated_state_dir,
    memory_report, mib, parse_memory_entries, query_memory, quit_and_expect_clean_exit,
    send_ipc_command, skip_case, tray_supported, unique_socket_name, wait_with_timeout,
    wallpaper_supported,
};

#[cfg(target_os = "linux")]
use common::desktop_linux::assert_the_desktop_holds_the_wallpaper;
use common::desktop_linux::{
    a_layout_change_this_session_can_make, plasmashell_answers, plasmashell_pid, xrandr,
};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Every image a publish wrote is one this session has somewhere to put.
///
/// The count is the desktop's business and differs per backend, but the sizes
/// are not: a wallpaper is either one screen's own resolution or the bounding
/// box of all of them, and anything else is an image the desktop is going to
/// scale for itself. This is the assertion that a two-screen session got two
/// screens' worth of pixels rather than the primary's twice.
fn assert_the_files_match_the_layout(files: &[std::path::PathBuf]) {
    let monitors = sunlit_core::display::monitors().unwrap_or_default();
    let canvas = sunlit_core::display::layout::bounds_of(&monitors);
    let mut sizes: Vec<(u32, u32)> = monitors.iter().map(|m| (m.width, m.height)).collect();
    if let Some(canvas) = canvas {
        sizes.push((canvas.width, canvas.height));
    }
    for file in files {
        let image =
            image::open(file).unwrap_or_else(|e| panic!("{} is not a PNG: {e}", file.display()));
        let size = (image.width(), image.height());
        if sizes.is_empty() {
            // A session with no display to ask renders at the documented
            // default size, which is what this run cannot check against a
            // layout it does not have.
            println!("no monitors to check {} ({size:?}) against", file.display());
            continue;
        }
        assert!(
            sizes.contains(&size),
            "{} is {size:?}, which is neither a screen of this session nor the \
             box around them ({sizes:?})",
            file.display()
        );
    }
    println!(
        "the publish wrote {} image(s) for {} screen(s)",
        files.len(),
        monitors.len()
    );
}

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
    /// A cold-cache 800x800 render, surface texture decode included, on the
    /// software adapter of a guest.
    const RENDER: Duration = Duration::from_mins(1);

    let temp_dir = TempDirGuard::new();
    let output_path = temp_dir.path().join("render.png");
    let config_path = fixture("e2e_config.toml");
    // A cache directory of its own, so this case pays the texture decode rather
    // than inheriting a warm cache from whichever case ran first. The memory
    // profile asserted on below is the profile of a run that decodes.
    let cache_dir = temp_dir.path().join("cache");

    // The render subcommand is the one start with no socket to answer on, so it
    // is spawned by hand. The watchers are not optional even so: a minute of
    // `--log-level debug` into a pipe nobody reads is how a child blocks on a
    // write and a case fails as a timeout naming nothing.
    let mut guard = ChildGuard::new(
        Command::new(binary())
            .env("SUNLIT_EARTH_NO_CLOUDS", "1")
            .env("SUNLIT_EARTH_CONFIG", isolated_config_path())
            .env("SUNLIT_EARTH_METRICS_DIR", isolated_state_dir())
            .env("SUNLIT_EARTH_CACHE_DIR", &cache_dir)
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
            .expect("failed to spawn sunlit-earth binary"),
    );
    let child = guard.child_mut();
    let _stdout_watcher = StdoutWatcher::new(child);
    let stderr_watcher = StderrWatcher::new(child);

    let output = wait_with_timeout(guard.take(), RENDER);
    let stderr_lines = stderr_watcher.into_lines();
    let stderr_text = stderr_lines.join("\n");

    assert!(
        output.status.success(),
        "process exited with non-zero status: {:?}\nstderr:\n{stderr_text}",
        output.status
    );

    assert!(
        output_path.exists(),
        "render output file was not created at {}",
        output_path.display()
    );
    let file_size = fs::metadata(&output_path)
        .expect("failed to read render output metadata")
        .len();
    assert!(file_size > 0, "render output file is empty (0 bytes)");

    let img = image::open(&output_path).expect("failed to decode render output PNG");

    let (width, height) = img.dimensions();
    assert_eq!(width, 800, "render output width mismatch");
    assert_eq!(height, 800, "render output height mismatch");

    // The fixture config places the camera at lon=11, lat=48 (Central Europe)
    // at 17:00 UTC on the summer solstice, so the terminator runs through
    // eastern Europe with India and Tibet in night, which is what the sample
    // coordinates below mean.
    let rgba = img.to_rgba8();

    assert_space_corner(&rgba, false, false, "top-left corner");
    assert_space_corner(&rgba, true, false, "top-right corner");
    assert_space_corner(&rgba, false, true, "bottom-left corner");
    assert_space_corner(&rgba, true, true, "bottom-right corner");

    assert_greenish(rgb_at(&rgba, 400, 400), "center (Central Europe)");
    assert_yellowish(rgb_at(&rgba, 420, 560), "Sahara");
    assert_blue(rgb_at(&rgba, 250, 400), "Atlantic Ocean");
    assert_ice(rgb_at(&rgba, 300, 175), "Greenland");
    assert_night_ocean(rgb_at(&rgba, 730, 500), "Indian Ocean (night)");
    assert_night_land(rgb_at(&rgba, 730, 300), "Tibet (night)");

    assert_no_error_lines(&stderr_lines);

    assert!(
        stderr_text.contains("first frame rendered"),
        "stderr does not contain 'first frame rendered':\n{stderr_text}"
    );

    let mem = parse_memory_entries(&stderr_text);
    assert!(
        !mem.is_empty(),
        "no memory usage entries found in stderr (is log level debug?)"
    );

    if let Some(entry) = mem.iter().find(|e| e.context == "after window creation") {
        assert!(
            entry.rss_mb < 300.0,
            "early memory too high: {:.0} MB at '{}' (expected < 300 MB)",
            entry.rss_mb,
            entry.context
        );
    }

    let peak = mem.iter().map(|e| e.peak_rss_mb).fold(0.0f64, f64::max);
    assert!(
        peak < 3000.0,
        "peak RSS too high: {peak:.0} MB (expected < 3000 MB)"
    );

    if let Some(entry) = mem.iter().rev().find(|e| e.context == "before exit") {
        assert!(
            entry.rss_mb < 1000.0,
            "exit memory too high: {:.0} MB (expected < 1000 MB)",
            entry.rss_mb
        );
        // Only where there was something to settle. `peak_rss_mb` is this
        // process's own high-water mark, so `rss <= peak` holds by
        // construction and the comparison asks whether the decode's memory came
        // back before the last sample. With nothing decoded the profile is flat
        // and the last sample is the high-water mark itself, so the comparison
        // is a number against itself and the numbers are printed instead.
        if mem.iter().any(|e| e.context == "after texture decode") {
            assert!(
                entry.rss_mb < peak,
                "memory did not settle: exit RSS {:.0} MB >= peak {:.0} MB",
                entry.rss_mb,
                peak
            );
        } else {
            println!(
                "no surface texture was decoded, so there was nothing to settle: \
                 {:.0} MB at exit against a {peak:.0} MB peak",
                entry.rss_mb
            );
        }
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

    // Waiting for the deferred hide is what `Ready::HiddenWindow` is: it fires
    // via `Timer::single_shot(ZERO)` once the event loop starts, and a
    // show-window sent before it races the timer that hides the window.
    let (mut guard, stdout_watcher, stderr_watcher) = Spawn::new(&socket_name)
        .args(["--tray-start", "hidden"])
        .ready(Ready::HiddenWindow)
        .start();

    // A hidden window fires no Slint rendering callback, so the window has to
    // be shown before a frame can be waited for.
    send_ipc_command(&socket_name, "show-window");
    stdout_watcher.wait_for_signal("window_shown", SIGNAL_REPLY);
    stdout_watcher.wait_for_signal("first_frame_rendered", READY);

    send_ipc_command(&socket_name, "hide-window");
    stdout_watcher.wait_for_signal("window_hidden", SIGNAL_REPLY);

    quit_and_expect_clean_exit(&socket_name, &mut guard, &stderr_watcher);

    let stderr = stderr_watcher.lines().join("\n");
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

    /// How long the listener window has to appear once the app is up.
    ///
    /// Not an IPC reply and so not `SIGNAL_REPLY`: `find_session_listener`
    /// polls `FindWindowW` for a window `session_end` creates on a thread of
    /// its own, which the app signals nothing about.
    const LISTENER_WINDOW: Duration = Duration::from_secs(30);

    let socket_name = unique_socket_name();
    let (mut guard, _stdout_watcher, stderr_watcher) = Spawn::new(&socket_name)
        .args(["--tray-start", "hidden"])
        .ready(Ready::HiddenWindow)
        .start();

    let hwnd = find_session_listener(guard.pid(), LISTENER_WINDOW);

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

    // Its own tail rather than `quit_and_expect_clean_exit`: no `quit` is sent
    // here, and what the case is measuring is how long the exit Windows asked
    // for takes.
    let output = wait_with_timeout(guard.take(), SHUTDOWN);
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

    let stderr = stderr_watcher.lines().join("\n");
    assert!(
        stderr.contains("windows is ending the session"),
        "stderr missing the session-end log line:\n{stderr}"
    );
    assert_no_error_lines(&stderr_watcher.lines());
}

#[test]
#[ignore = "requires desktop environment and GPU"]
#[serial]
fn test_windowed_mode_graceful_shutdown() {
    let socket_name = unique_socket_name();

    let (mut guard, _stdout_watcher, stderr_watcher) =
        Spawn::new(&socket_name).args(["--mode", "window"]).start();

    quit_and_expect_clean_exit(&socket_name, &mut guard, &stderr_watcher);

    let stderr = stderr_watcher.lines().join("\n");
    assert!(
        stderr.contains("startup mode: windowed"),
        "stderr missing 'startup mode: windowed':\n{stderr}"
    );
    assert!(
        stderr.contains("quit_event_loop"),
        "stderr missing 'quit_event_loop':\n{stderr}"
    );
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

    // A holds the mutex, so it only has to be answering; nothing here looks at
    // what it drew.
    let (mut guard_a, _stdout_a, stderr_a) =
        Spawn::new(&socket_name).ready(Ready::Listener).start();

    // The same socket name as A, so B takes the mutex scoped to that name
    // rather than the default one, which a real running instance may hold. B
    // is spawned by hand because it is expected to exit on its own, which is
    // what the case is about.
    let instance_b = Command::new(binary())
        .env("SUNLIT_EARTH_NO_CLOUDS", "1")
        .env("SUNLIT_EARTH_CONFIG", isolated_config_path())
        .env("SUNLIT_EARTH_METRICS_DIR", isolated_state_dir())
        .args(["--log-level", "debug", "--ipc-socket", &socket_name])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn instance B");

    let output_b = wait_with_timeout(instance_b, SHUTDOWN);

    assert!(
        output_b.status.success(),
        "instance B exited with non-zero status: {:?}",
        output_b.status
    );

    let stderr_b = String::from_utf8_lossy(&output_b.stderr);
    assert!(
        stderr_b.contains("another instance is already running"),
        "instance B stderr missing single-instance message:\n{stderr_b}"
    );

    quit_and_expect_clean_exit(&socket_name, &mut guard_a, &stderr_a);
}

#[test]
#[ignore = "requires desktop environment and GPU"]
#[serial]
fn test_tray_hide_show_cycle() {
    let socket_name = unique_socket_name();

    let (mut guard, stdout_watcher, stderr_watcher) = Spawn::new(&socket_name).start();

    send_ipc_command(&socket_name, "hide-window");
    stdout_watcher.wait_for_signal("window_hidden", SIGNAL_REPLY);

    send_ipc_command(&socket_name, "show-window");
    stdout_watcher.wait_for_signal("window_shown", SIGNAL_REPLY);

    quit_and_expect_clean_exit(&socket_name, &mut guard, &stderr_watcher);
}

/// A wallpaper export must still succeed on a hidden window.
///
/// Automatic refresh in tray mode is only viable without a show-render-hide
/// cycle if the GPU resources outlive `window.hide()`.
#[test]
#[ignore = "requires desktop environment and GPU"]
#[serial]
fn test_gpu_persistence_after_hide() {
    let socket_name = unique_socket_name();

    let (mut guard, stdout_watcher, stderr_watcher) = Spawn::new(&socket_name).start();

    send_ipc_command(&socket_name, "hide-window");
    stdout_watcher.wait_for_signal("window_hidden", SIGNAL_REPLY);

    // A `RenderingTeardown` on hide would fail this with "GPU not initialized".
    send_ipc_command(&socket_name, "export-test");
    stdout_watcher.wait_for_signal("export_test_ok", SIGNAL_REPLY);

    quit_and_expect_clean_exit(&socket_name, &mut guard, &stderr_watcher);
}

/// Cloud updates arriving while the window is hidden must not grow process
/// memory without bound.
///
/// Decoded cloud frames reach the GPU through a channel drained from
/// `BeforeRendering`, which stops firing once the window is hidden. The case
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
    /// One image served by the local stub, which is polled once a second.
    const DOWNLOAD: Duration = Duration::from_secs(15);

    let socket_name = unique_socket_name();
    let temp_dir = TempDirGuard::new();
    let cache_dir = temp_dir.path().join("cache");
    let textures_dir = temp_dir.path().join("textures");
    fs::create_dir_all(&cache_dir).expect("failed to create stub cache dir");
    fs::create_dir_all(&textures_dir).expect("failed to create empty textures dir");

    let (port, stub) = spawn_cloud_stub(cloud_fixture_jpeg(FIXTURE_WIDTH, FIXTURE_HEIGHT));

    // Windowed, so there is no tray icon and no single-instance mutex, while
    // hiding over IPC still reaches the same state. The empty textures
    // directory leaves the JXL slots unloaded, so cloud frames are the only
    // large allocations in flight, and this is the one case that lets the cloud
    // fetcher run at all.
    let (mut guard, stdout_watcher, stderr_watcher) = Spawn::new(&socket_name)
        .args([
            "--mode",
            "window",
            "--textures-dir",
            textures_dir.to_str().expect("non-UTF-8 temp path"),
        ])
        .with_clouds()
        .env("SUNLIT_EARTH_SYNC_LOG", "1")
        .env(
            "SUNLIT_EARTH_CLOUD_URL",
            format!("http://127.0.0.1:{port}/clouds.jpg"),
        )
        .env("SUNLIT_EARTH_CLOUD_POLL_SECS", "1")
        .env("SUNLIT_EARTH_CACHE_DIR", &cache_dir)
        .start();

    wait_for_downloads(&stub, 1, READY);
    stderr_watcher.wait_for_log("GPU texture created", READY);

    // From here on `BeforeRendering` no longer fires.
    send_ipc_command(&socket_name, "hide-window");
    stdout_watcher.wait_for_signal("window_hidden", SIGNAL_REPLY);

    std::thread::sleep(SETTLE);
    let baseline = query_memory(&socket_name, &stdout_watcher);
    let stderr_cursor = stderr_watcher.line_count();

    for _ in 0..UPDATES {
        let target = stub.gets.load(Ordering::SeqCst) + 1;
        stub.version.fetch_add(1, Ordering::SeqCst);
        wait_for_downloads(&stub, target, DOWNLOAD);
    }

    std::thread::sleep(SETTLE);
    let end = query_memory(&socket_name, &stdout_watcher);

    send_ipc_command(&socket_name, "export-test");
    stdout_watcher.wait_for_signal("export_test_ok", SIGNAL_REPLY);

    // Its own tail rather than `quit_and_expect_clean_exit`: shutting down
    // before asserting is what makes a failing run produce a complete log
    // instead of a killed process. Showing the window drains everything that
    // was parked, so the cursor must be taken before it.
    let stderr_cursor_end = stderr_watcher.line_count();
    send_ipc_command(&socket_name, "show-window");
    stdout_watcher.wait_for_signal("window_shown", SIGNAL_REPLY);
    send_ipc_command(&socket_name, "quit");
    let output = wait_with_timeout(guard.take(), SHUTDOWN);

    let private_growth = end.private_bytes.saturating_sub(baseline.private_bytes);
    let rss_growth = end.rss_bytes.saturating_sub(baseline.rss_bytes);
    let created_while_hidden = stderr_watcher.lines()[stderr_cursor..stderr_cursor_end]
        .iter()
        .filter(|line| line.contains("GPU texture created"))
        .count();

    println!(
        "baseline: rss={:.1} MiB private={:.1} MiB",
        mib(baseline.rss_bytes),
        mib(baseline.private_bytes)
    );
    println!(
        "after {UPDATES} hidden cloud updates: rss={:.1} MiB private={:.1} MiB peak_rss={:.1} MiB",
        mib(end.rss_bytes),
        mib(end.private_bytes),
        mib(end.peak_rss_bytes)
    );
    println!(
        "growth: private={:.1} MiB rss={:.1} MiB (limit {:.0} MiB), \
         GPU textures created while hidden: {created_while_hidden}",
        mib(private_growth),
        mib(rss_growth),
        mib(GROWTH_LIMIT_BYTES)
    );

    // Private bytes (commit charge) rather than RSS, because working-set
    // trimming can hide heap growth from RSS.
    assert!(
        private_growth < GROWTH_LIMIT_BYTES,
        "private bytes grew by {:.1} MiB across {UPDATES} cloud updates while hidden \
         (limit {:.0} MiB, RSS grew by {:.1} MiB)",
        mib(private_growth),
        mib(GROWTH_LIMIT_BYTES),
        mib(rss_growth)
    );

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
    assert_no_error_lines(&stderr_watcher.lines());
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
    let (mut guard, stdout_watcher, stderr_watcher) = Spawn::new(&socket_name).start();

    let report = memory_report(&socket_name, &stdout_watcher);

    println!("--- memory report ---");
    for line in &report {
        println!("{line}");
    }
    println!("--- end of memory report ---");

    quit_and_expect_clean_exit(&socket_name, &mut guard, &stderr_watcher);

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
}

/// Verify that the app renders and publishes a real desktop wallpaper.
///
/// The one case that changes something outside the process, so it is opt-in
/// through `WALLPAPER_OPT_IN` rather than platform-gated.
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
    let (mut guard, stdout_watcher, stderr_watcher) = Spawn::new(&socket_name).start();

    // Twice, because the second publish is its own case: a desktop keys the
    // wallpaper it is showing on the path it was handed, so a frame written to
    // the path already in that setting is one nothing reloads. A setter that
    // succeeded on a first publish and changed nothing on a second is exactly
    // what a single pass here cannot tell apart from working.
    let mut published: Vec<Vec<std::path::PathBuf>> = Vec::new();
    #[cfg(target_os = "linux")]
    let mut held: Vec<Option<String>> = Vec::new();
    for pass in 1..=2 {
        let from = stdout_watcher.line_count();
        send_ipc_command(&socket_name, "set-wallpaper");
        let line = stdout_watcher.wait_for_signal_line_from("wallpaper_", from, PUBLISH);
        assert!(
            line.contains("wallpaper_set"),
            "publish {pass}: the engine reported a failure instead: {line}"
        );
        let files =
            sunlit_core::wallpaper::published_wallpaper_files().expect("a local data directory");
        assert!(
            !files.is_empty(),
            "publish {pass}: the engine reported a wallpaper and wrote no file"
        );
        assert_the_files_match_the_layout(&files);

        // Everything above is the app's own account of what it did. This is the
        // desktop's.
        #[cfg(target_os = "linux")]
        held.push(assert_the_desktop_holds_the_wallpaper(&files));
        published.push(files);
    }
    // Every path, not only one of them: the alternation has to hold per screen,
    // or a second monitor sits on a picture the desktop has no reason to reload.
    assert_eq!(
        published[0].len(),
        published[1].len(),
        "the two publishes wrote a different number of images: {published:?}"
    );
    for (first, second) in published[0].iter().zip(&published[1]) {
        assert_ne!(
            first, second,
            "the second publish wrote a path the desktop was already showing, \
             which is a wallpaper that does not visibly change"
        );
    }
    // And the desktop stored the new one, where its setter has a store to ask.
    #[cfg(target_os = "linux")]
    if let [Some(first), Some(second)] = held.as_slice() {
        assert_ne!(
            first, second,
            "the desktop reports the same wallpaper after both publishes"
        );
    }

    quit_and_expect_clean_exit(&socket_name, &mut guard, &stderr_watcher);

    let stderr = stderr_watcher.lines().join("\n");
    assert!(
        stderr.contains("wallpaper updated"),
        "stderr missing 'wallpaper updated':\n{stderr}"
    );
}

/// What the running app says this session's screens are.
///
/// The Linux half of the platform seam, and the half a unit test cannot reach:
/// every layout case in `display::layout` runs against a fabricated list, and
/// this is the one place a real session fills that list in. It publishes
/// nothing, so unlike the wallpaper cases it needs no opt-in and runs wherever
/// the suite runs.
#[test]
#[ignore = "requires desktop environment and GPU"]
#[serial]
fn test_displays_reports_the_session_layout() {
    let socket_name = unique_socket_name();
    let (mut guard, stdout_watcher, stderr_watcher) = Spawn::new(&socket_name).start();

    let from = stdout_watcher.line_count();
    send_ipc_command(&socket_name, "displays");
    let line = stdout_watcher.wait_for_signal_line_from("displays ", from, SIGNAL_REPLY);
    println!("the session reports: {line}");
    let plan = DisplaysSignal::parse(&line).unwrap_or_else(|| {
        panic!(
            "missing '{}' in the displays line: {line}",
            DisplaysSignal::missing_field(&line).unwrap_or("a field")
        )
    });

    assert!(
        plan.monitors >= 1,
        "a session the app is running in has at least one screen: {line}"
    );
    assert!(
        plan.anchor >= 0 && plan.anchor < i32::try_from(plan.monitors).unwrap_or(i32::MAX),
        "the anchor is a position in the list of {}: {line}",
        plan.monitors
    );
    assert!(
        !plan.images.is_empty(),
        "a plan with no export in it is a wallpaper that never appears: {line}"
    );

    // The test process asks the same platform the same question. Where it gets
    // an answer, the two have to agree exactly: this is the assertion that the
    // rectangles the layout math is fed are the rectangles the session has.
    let monitors = sunlit_core::display::monitors().unwrap_or_default();
    if monitors.is_empty() {
        println!(
            "this platform has no monitor query, so {:?} stands unchecked",
            plan.rects
        );
    } else {
        let expected: Vec<String> = monitors
            .iter()
            .map(|m| format!("{},{},{},{}", m.x, m.y, m.width, m.height))
            .collect();
        assert_eq!(
            plan.rects, expected,
            "the app and this test disagree about the session's own screens"
        );
        assert_eq!(plan.monitors, monitors.len());
    }

    quit_and_expect_clean_exit(&socket_name, &mut guard, &stderr_watcher);
}

/// One view across every screen, and the files that come out of it.
///
/// What a publish writes in this mode is not one shape: a desktop that spans
/// takes the canvas whole, one that addresses monitors takes a cut piece each,
/// and one that holds a single image takes the anchor's piece and nothing else.
/// So the case asserts the shape this session's own desktop can hold, which is
/// the same rule the sink used to decide, read back out of the files.
#[test]
#[ignore = "requires desktop environment and GPU"]
#[serial]
fn test_across_screens_writes_what_this_desktop_can_hold() {
    if !wallpaper_supported() {
        skip_case(
            "test_across_screens_writes_what_this_desktop_can_hold",
            "the wallpaper sink reports no setter for this session",
        );
        return;
    }
    if std::env::var_os(WALLPAPER_OPT_IN).is_none() {
        skip_case(
            "test_across_screens_writes_what_this_desktop_can_hold",
            "this case replaces the desktop wallpaper, so it runs only where \
             that is harmless; the VM job sets SUNLIT_EARTH_E2E_WALLPAPER",
        );
        return;
    }

    // The mode is a stored setting and this is the one case that needs it set,
    // so the config is written rather than driven through the window.
    let config = isolated_config_path();
    fs::write(
        &config,
        "[sunlit.earth]\ndisplay_mode = \"across-screens\"\n",
    )
    .expect("write the throwaway config");

    let socket_name = unique_socket_name();
    let (mut guard, stdout_watcher, stderr_watcher) =
        Spawn::new(&socket_name).config(config).start();

    let from = stdout_watcher.line_count();
    send_ipc_command(&socket_name, "set-wallpaper");
    let line = stdout_watcher.wait_for_signal_line_from("wallpaper_", from, PUBLISH);
    assert!(
        line.contains("wallpaper_set"),
        "the engine reported a failure instead: {line}"
    );

    let files =
        sunlit_core::wallpaper::published_wallpaper_files().expect("a local data directory");
    assert!(!files.is_empty(), "a publish that wrote no file");
    assert_the_files_match_the_layout(&files);

    let monitors = sunlit_core::display::monitors().unwrap_or_default();
    let bounds = sunlit_core::display::layout::bounds_of(&monitors);
    let sizes: Vec<(u32, u32)> = files
        .iter()
        .map(|file| {
            let image = image::open(file).expect("a PNG");
            (image.width(), image.height())
        })
        .collect();
    println!(
        "across-screens wrote {sizes:?} for {} screen(s)",
        monitors.len()
    );

    if let Some(bounds) = bounds {
        let canvas = (bounds.width, bounds.height);
        #[cfg(target_os = "linux")]
        {
            use sunlit_core::desktop::Reach;
            let backend = sunlit_core::desktop::detect_current()
                .expect("a session with no backend could not have published");
            match backend.reach() {
                Reach::Spanned => assert_eq!(
                    sizes,
                    vec![canvas],
                    "{} spans one image over the whole desktop, so the canvas is \
                     what it should have been handed",
                    backend.desktop
                ),
                Reach::PerMonitor => {
                    let expected: Vec<(u32, u32)> =
                        monitors.iter().map(|m| (m.width, m.height)).collect();
                    assert_eq!(
                        sizes, expected,
                        "{} addresses its monitors, so each one gets its own piece \
                         of the canvas at its own size",
                        backend.desktop
                    );
                }
                Reach::OneImage => {
                    let anchor = sunlit_core::display::layout::resolve_anchor(&monitors, None)
                        .expect("a session with a screen");
                    let screen = &monitors[anchor.index];
                    assert_eq!(
                        sizes,
                        vec![(screen.width, screen.height)],
                        "{} holds one wallpaper for every screen, so it gets the \
                         anchor's piece rather than a canvas it would zoom",
                        backend.desktop
                    );
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            // Windows spans through the shell rather than by being cut, so the
            // one file is the canvas whatever the screen count.
            assert_eq!(sizes, vec![canvas], "the span writes one canvas");
        }
    }

    // The desktop's own account of what it is holding, for the mode where the
    // image is not the shape any single screen is.
    #[cfg(target_os = "linux")]
    assert_the_desktop_holds_the_wallpaper(&files);

    quit_and_expect_clean_exit(&socket_name, &mut guard, &stderr_watcher);
}

/// A layout that changed, against a real X server, end to end.
///
/// What it proves is the whole path: that a `RandR` change wakes the watcher,
/// that the settle collapses the burst into one query, that the engine sees the
/// new list, and that the desktop was handed images for it. Every layout
/// question below the watcher is decided against fabricated monitor lists in
/// `display::layout` and `tests/engine.rs`; this is the one place a real
/// session's own screens move.
///
/// Gated like the other wallpaper cases, and additionally on this session
/// having a layout it can move at all: two outputs to switch one off, or a
/// second mode to switch to.
#[test]
#[ignore = "requires desktop environment and GPU"]
#[serial]
fn test_a_layout_change_republishes_the_wallpaper() {
    const CASE: &str = "test_a_layout_change_republishes_the_wallpaper";

    if !wallpaper_supported() {
        skip_case(
            CASE,
            "the wallpaper sink reports no setter for this session",
        );
        return;
    }
    if std::env::var_os(WALLPAPER_OPT_IN).is_none() {
        skip_case(
            CASE,
            "this case replaces the desktop wallpaper, so it runs only where \
             that is harmless; the VM job sets SUNLIT_EARTH_E2E_WALLPAPER",
        );
        return;
    }
    let Some(mut change) = a_layout_change_this_session_can_make() else {
        skip_case(
            CASE,
            "this session offers no layout change this case can make: it needs \
             two outputs to switch one off or a second mode to switch to, and \
             `display::outputs` is a Linux query, so off Linux it always lands here",
        );
        return;
    };
    println!("{CASE}: the change is {}", change.what);

    let socket_name = unique_socket_name();
    let (mut guard, stdout_watcher, stderr_watcher) = Spawn::new(&socket_name).start();

    // A wallpaper on the desk first: the engine only renders again for a layout
    // change where it is already holding one.
    let from = stdout_watcher.line_count();
    send_ipc_command(&socket_name, "set-wallpaper");
    let line = stdout_watcher.wait_for_signal_line_from("wallpaper_", from, PUBLISH);
    assert!(
        line.contains("wallpaper_set"),
        "the first publish failed: {line}"
    );
    let before = sunlit_core::wallpaper::published_wallpaper_files().expect("a data directory");
    assert!(!before.is_empty(), "the first publish wrote no file");
    let was = sunlit_core::display::outputs().unwrap_or_default();

    let from = stdout_watcher.line_count();
    assert!(
        xrandr(&change.apply),
        "the case cannot change a layout xrandr will not move"
    );
    if sunlit_core::display::outputs().unwrap_or_default() == was {
        skip_case(
            CASE,
            "this session put its own layout straight back, so there is nothing \
             here for the app to have noticed",
        );
        return;
    }

    let line = stdout_watcher.wait_for_signal_line_from("displays_changed ", from, PUBLISH);
    assert!(
        line.contains(&format!("monitors={}", change.monitors_after)),
        "the app was told about a layout that is not the one this case made: {line}"
    );
    let line = stdout_watcher.wait_for_signal_line_from("wallpaper_", from, PUBLISH);
    assert!(
        line.contains("wallpaper_set"),
        "the republish after the layout change failed: {line}"
    );

    let after = sunlit_core::wallpaper::published_wallpaper_files().expect("a data directory");
    assert!(!after.is_empty(), "the republish wrote no file");
    assert_the_files_match_the_layout(&after);
    for path in &after {
        assert!(
            !before.contains(path),
            "{} is a path the desktop was already showing, which is a wallpaper \
             that does not visibly change",
            path.display()
        );
    }

    let from = stdout_watcher.line_count();
    assert!(change.guard.restore(), "the layout could not be put back");
    let line = stdout_watcher.wait_for_signal_line_from("displays_changed ", from, PUBLISH);
    println!("{CASE}: the layout came back as {line}");
    let line = stdout_watcher.wait_for_signal_line_from("wallpaper_", from, PUBLISH);
    assert!(
        line.contains("wallpaper_set"),
        "the republish after the layout came back failed: {line}"
    );
    assert_the_files_match_the_layout(
        &sunlit_core::wallpaper::published_wallpaper_files().expect("a data directory"),
    );

    quit_and_expect_clean_exit(&socket_name, &mut guard, &stderr_watcher);
}

/// A burst of re-publishes does not crash plasmashell.
///
/// Plasma's `MediaProxy` keeps a `KDirWatch` on the current wallpaper file, so a
/// file rewritten in its live path can be decoded half-written, and a file
/// deleted and recreated can trip an assertion in the plugin, which on a distro
/// that ships it with assertions live takes the shell down. The other guest
/// desktops read the wallpaper once at set time and never watch it, so only a
/// Plasma session exercises this, and only a rapid re-publish makes the watch and
/// the rewrite race.
///
/// So this publishes several times back to back with no pause and then asks
/// whether plasmashell is still the same process answering D-Bus. A shell that
/// crashed would answer under a new pid, or not answer at all. It is the cheap
/// standing proxy for "it did not crash"; the definitive check stays the hand
/// check on a two-screen Plasma machine.
#[test]
#[ignore = "requires desktop environment and GPU"]
#[serial]
fn test_plasmashell_survives_rapid_republishing() {
    const CASE: &str = "test_plasmashell_survives_rapid_republishing";
    const BURST: usize = 5;

    if !wallpaper_supported() {
        skip_case(
            CASE,
            "the wallpaper sink reports no setter for this session",
        );
        return;
    }
    if std::env::var_os(WALLPAPER_OPT_IN).is_none() {
        skip_case(
            CASE,
            "this case replaces the desktop wallpaper, so it runs only where \
             that is harmless; the VM job sets SUNLIT_EARTH_E2E_WALLPAPER",
        );
        return;
    }
    let is_plasma = sunlit_core::desktop::detect_current()
        .is_some_and(|backend| backend.desktop == "KDE Plasma");
    if !is_plasma {
        skip_case(
            CASE,
            "the MediaProxy file watch this exercises is Plasma's; this session \
             is not KDE, so the crash cannot happen here",
        );
        return;
    }
    let Some(before) = plasmashell_pid() else {
        skip_case(
            CASE,
            "plasmashell is not answering the session bus, so it cannot be the \
             survival proxy",
        );
        return;
    };

    let socket_name = unique_socket_name();
    let (mut guard, stdout_watcher, stderr_watcher) = Spawn::new(&socket_name).start();

    // Several full renders, readbacks, encodes and sets in a row, with nothing
    // between them, which is what makes the watch and the rewrite race.
    for pass in 1..=BURST {
        let from = stdout_watcher.line_count();
        send_ipc_command(&socket_name, "set-wallpaper");
        let line = stdout_watcher.wait_for_signal_line_from("wallpaper_", from, PUBLISH);
        assert!(
            line.contains("wallpaper_set"),
            "publish {pass} of {BURST} failed: {line}"
        );
    }

    // Still the same process, which is what "did not crash" means here.
    let after = plasmashell_pid();
    assert_eq!(
        after.as_deref(),
        Some(before.as_str()),
        "plasmashell is no longer answering as pid {before} after {BURST} rapid \
         publishes, which is the crash this fix removes (now {after:?})"
    );
    assert!(
        plasmashell_answers(),
        "plasmashell stopped answering evaluateScript after {BURST} rapid publishes"
    );
    println!("{CASE}: plasmashell survived {BURST} rapid publishes as pid {before}");

    quit_and_expect_clean_exit(&socket_name, &mut guard, &stderr_watcher);
}
