# E2E GUI Testing Practices for Desktop Apps (2026-03-24)

**Scope:** Research into end-to-end testing patterns for native desktop applications, with a focus
on lessons applicable to Sunlit Earth (Rust + Slint + wgpu). Complements the earlier
`2026-03-22-testability-observability-ui-testing.md` (which covers in-process Slint/wgpu testing)
by addressing the broader question of full-binary e2e tests: subprocess lifecycle management,
screenshot-based validation, accessibility + visual layering, CI infrastructure, and log-based
assertions.

---

## 1. E2E Testing Patterns for Desktop Apps

### The General Landscape

E2E tests for desktop applications sit at the top of the testing pyramid: they are expensive to run
and maintain, but they verify end-to-end behavior that unit and integration tests cannot. The
established industry approaches, arranged by the technology they assume:

**WebDriver-based automation (Electron/Tauri/web)**

The WebDriver protocol provides a standardized interface for driving GUIs via a test client that
sends commands (click, type, find element) to a driver process that in turn controls the app.
Electron has first-class support through Chrome DevTools Protocol (CDP), which Playwright uses for
its experimental `_electron.launch()` API. Tauri exposes a `tauri-driver` shim that proxies
WebDriver commands to the platform-native driver (EdgeDriver on Windows, WebKitWebDriver on Linux).

Applicability to Slint: Slint does not expose a WebDriver interface. This approach does not transfer
directly. Slint's winit backend exposes an AccessKit accessibility tree on Windows, which UIA clients
can read, but there is no WebDriver bridge.

**OS-level accessibility automation (Win32/Qt/GTK)**

Microsoft UI Automation (UIA) is the Win32 accessibility API that tools like WinAppDriver,
FlaUI (C#), and Squish use to drive Windows desktop applications. UIA exposes every interactive
element as a COM object with properties (Name, AutomationId, ControlType) and patterns (Invoke,
Toggle, SelectionItem, Value) that test code can manipulate.

Squish (the Qt/GTK/desktop testing tool) takes this further: it can drive applications through
accessibility trees, OCR, or image matching, and supports Python/JavaScript test scripts. The core
insight from Squish users is that timing is the dominant failure mode: scripts must wait for objects
to reach a correct state before acting on them, and UIA queries require elements to be actually
focusable and rendered before they are visible in the tree.

Applicability to Slint: Slint exposes accessible roles and labels through AccessKit on Windows,
which bridges to UIA. This means tools like FlaUI (C#) or UIA APIs from Python (`comtypes`) can
in principle query Slint's accessibility tree. The limitation is that Slint's AccessKit integration
is still maturing (known issues with text input widgets, list view focus updates, performance
overhead when the `accessibility` feature is enabled). For simple structural assertions (button
exists, label text, checkbox state), the UIA path is viable; for complex interactions it is fragile.

**Image/pixel-based automation (SikuliX, AskUI)**

Frameworks like SikuliX operate at the pixel level: they find UI elements by template-matching
reference images against the live screen, then simulate mouse clicks at matched coordinates. This
works for any application regardless of accessibility support. The cost is brittleness to any visual
change (DPI, theme, resolution) and the inability to make semantic assertions.

For Sunlit Earth, pixel-based automation is the most accessible path but also the most fragile.

**In-process testing (Slint testing backend, egui_kittest)**

The most robust approach for a Rust GUI app is to test logic and state within the same process,
using a headless/mock backend that does not require a display. This is what
`i-slint-backend-testing` provides for Slint and what `egui_kittest` provides for egui.
See `2026-03-24-slint-testing-research.md` for detailed coverage of this approach.

The key limitation: in-process tests cannot exercise the real rendering path (the wgpu pipeline),
the real window event loop, or behaviors that only emerge when the full binary runs. Those require
subprocess-based e2e tests.

**Lessons from the Qt/Squish ecosystem**

Squish users report consistent lessons: (1) Unique, stable object identifiers matter enormously —
applications without stable IDs or accessible names force tests to use fragile hierarchy or
occurrence-based selectors that break on any UI change. (2) Race conditions are the primary source
of flakiness — tests that press buttons before the state transition animation finishes fail
intermittently. (3) Separate test script from test data — parameterize tests over inputs rather than
hardcoding values. (4) Screenshot tests in Squish are a last resort, used only for purely visual
aspects that accessibility trees cannot capture (custom-painted widgets, charts, rendered scenes).

For Sunlit Earth, point (1) implies that adding meaningful `accessible-label` properties to Slint
elements and assigning element IDs to all interactable controls is prerequisite work for any UIA or
Slint testing backend tests.

---

## 2. Test Lifecycle Management: Subprocess Launch and Teardown

### Overview

When the goal is to test the full application binary (rather than testing the library in-process),
the standard pattern across all frameworks is:

1. Compile the binary under test (Cargo handles this via `CARGO_BIN_EXE_<name>`).
2. Spawn the binary as a child process with captured stdio.
3. Wait until the app signals readiness.
4. Execute test assertions (via UIA, screenshot capture, or log inspection).
5. Tear down cleanly, or kill with a timeout.

### Launching the Binary from Rust Tests

Cargo sets `CARGO_BIN_EXE_<name>` environment variables during test builds, where `<name>` is the
binary name from `Cargo.toml`. This gives the exact path to the compiled binary without
hardcoding:

```rust
// In tests/e2e_smoke.rs
let bin_path = env!("CARGO_BIN_EXE_sunlit-earth");
let mut child = std::process::Command::new(bin_path)
    .arg("--software-rendering")   // force deterministic GPU path
    .arg("--exit-after-frame")     // hypothetical: render one frame then exit
    .stderr(std::process::Stdio::piped())
    .stdout(std::process::Stdio::piped())
    .spawn()
    .expect("failed to spawn sunlit-earth");
```

The `--software-rendering` flag is already implemented in Sunlit Earth's `main.rs` (it calls
`set_rendering_backend` to force the wgpu software adapter). Any e2e test that renders GPU output
should pass this flag to ensure deterministic output on CI runners without hardware GPUs.

For Windows CI (GitHub Actions `windows-latest`), the runner runs as an interactive desktop session
(not a Windows service), which means windows created by the subprocess are visible to the session
and receive paint messages. This is sufficient for Slint window creation to succeed. The limitation
is that GitHub-hosted runners do not have hardware GPUs, so rendering must use the software adapter.

### Readiness Detection

The hardest problem in subprocess-based GUI testing is knowing when the app is ready to receive
test inputs. Several strategies exist, from robust to fragile:

**Strategy 1: Log-line sentinel (most reliable for instrumented apps)**

The application emits a structured log line when the first frame is rendered. The test harness reads
stderr line by line until the sentinel appears:

```rust
use std::io::{BufRead, BufReader};
use std::time::{Duration, Instant};

fn wait_for_ready(
    stderr: &mut BufReader<impl Read>,
    sentinel: &str,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    let mut line = String::new();
    loop {
        if Instant::now() > deadline {
            return Err(format!("timed out waiting for sentinel: {sentinel}"));
        }
        line.clear();
        match stderr.read_line(&mut line) {
            Ok(0) => return Err("subprocess exited before sentinel".to_string()),
            Ok(_) => {
                if line.contains(sentinel) {
                    return Ok(());
                }
            }
            Err(e) => return Err(format!("read error: {e}")),
        }
    }
}

// Usage:
let stderr = child.stderr.take().unwrap();
let mut reader = BufReader::new(stderr);
wait_for_ready(&mut reader, "frame rendered", Duration::from_secs(30))?;
```

This is the same pattern used by `testcontainers-rs`'s `WaitFor::StdOutMessage` strategy, which
waits for a specific string on stdout or stderr before the container is considered ready.

For Sunlit Earth, this requires adding a `tracing::info!("frame rendered")` (or similar) event
at the point where `BeforeRendering` completes its first successful render. The event should be at
`info` level so it appears even in release builds if `RUST_LOG=info` is set.

**Strategy 2: Fixed-duration sleep (fragile, not recommended)**

Simply sleeping for a fixed duration (e.g., 2 seconds) before proceeding. This is universally
condemned in the literature and fails under load, on slow CI machines, and when the app has a slow
startup path. It is the "static wait" anti-pattern that causes the majority of e2e test flakiness.

Use only as a last resort fallback if no better signal is available, and always pair it with a
maximum timeout.

**Strategy 3: Window enumeration polling (platform-specific)**

On Windows, the test harness can poll the Win32 `FindWindow` API to detect when the app's window
has been created and is visible. This is more reliable than a sleep but still subject to race
conditions between window creation and the first rendered frame.

```rust
// Windows-only: poll until the window appears
fn wait_for_window(title_fragment: &str, timeout: Duration) -> bool {
    use std::time::Instant;
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        // windows-sys FindWindowW could be called here
        // simplified: check process existence and retry
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}
```

**Strategy 4: Exit-code smoke test (simplest for CI)**

The simplest form of e2e test does not require the app to stay running. The binary is launched with
a CLI flag (e.g., `--exit-after-frame`) that causes it to render exactly one frame, write a
screenshot to a path, and exit with code 0 on success or non-zero on failure. The test asserts on
the exit code and optionally validates the screenshot:

```rust
let status = std::process::Command::new(bin_path)
    .args(["--software-rendering", "--exit-after-frame", "--screenshot", "/tmp/frame.png"])
    .status()
    .expect("failed to run sunlit-earth");
assert!(status.success(), "app exited with error: {status}");
```

This is how Bevy's CI works: the `bevy_ci_testing` crate adds a `--ci-testing-config <path>` flag
that reads a `.ron` config specifying how many frames to render and where to save screenshots before
exiting. Firefox uses a similar `MOZ_HEADLESS` + `-screenshot` mechanism for its rendering tests.

For Sunlit Earth, implementing `--exit-after-frame` and `--screenshot <path>` CLI flags in
`main.rs` would unlock the simplest possible e2e test with no subprocess synchronization problem.

### Teardown

**Graceful shutdown:** The preferred approach is to send a close signal to the app before killing
it. On Windows, this can be done by posting a `WM_CLOSE` message to the window, or by calling
`TerminateProcess`. With Slint, the event loop can be exited programmatically via
`slint::quit_event_loop()`, but this requires an IPC mechanism.

**Timeout-based kill:** The fallback is to call `child.kill()` after a timeout. Always wait for
the process to exit after killing it:

```rust
// Give the process a chance to exit cleanly
match child.wait_timeout(Duration::from_secs(5)) {
    Ok(Some(_)) => {} // exited on its own
    Ok(None) => {
        child.kill().ok();
        child.wait().ok();
    }
    Err(_) => {
        child.kill().ok();
    }
}
```

The `wait_timeout` method is available via the `wait-timeout` crate, or implementable with
`child.try_wait()` polling.

**Always capture stderr on failure:** Even if the test does not assert on log output, capturing
stderr and printing it on test failure dramatically accelerates debugging:

```rust
let output = child.wait_with_output().expect("failed to wait");
if !output.status.success() {
    eprintln!("--- stderr ---\n{}", String::from_utf8_lossy(&output.stderr));
    panic!("subprocess failed: {}", output.status);
}
```

---

## 3. Screenshot-Based Validation

### When to Use Pixel Comparison vs. Structural/Semantic Checks

The egui project's guidelines capture the practical rule well: prefer regular Rust assertions or
accessibility-tree assertions over image comparison tests. Image comparison tests are relatively
slow (rendering + comparison cost), brittle (unrelated visual changes cause failures), and generate
maintenance overhead (reference images must be regenerated). Use them only when the assertion
cannot be expressed structurally.

A decision tree:

- Can the assertion be expressed as "property X has value Y"? Use the Slint testing backend or
  in-process Rust assertions.
- Can the assertion be expressed as "element E is visible / has label L / has role R"? Use the
  accessibility tree (UIA query, Slint `ElementHandle`, or kittest).
- Is the assertion about rendered pixel output (the wgpu globe render, color blending, atmosphere
  effects)? Use invariant-based assertions (brightness monotonicity, color range bounds).
- Is the assertion about the exact visual appearance of a specific rendering configuration? Use
  perceptual screenshot comparison (MSSIM) against a reference image generated by the software
  adapter.

### Handling GPU Rendering Variance Across Hardware

GPU rendering introduces floating-point variance that makes exact pixel comparison across machines
unreliable. Sources of variance include:

- Different GPU architectures (NVIDIA, AMD, Intel) produce slightly different results for the same
  WGSL shader due to different floating-point rounding and shader compiler implementations.
- Different driver versions for the same GPU can produce different results.
- Software rasterizers (WARP on Windows, llvmpipe/lavapipe on Linux) produce consistent output
  within themselves but differ from each other and from hardware GPUs.
- MSAA sample patterns differ across hardware, making anti-aliased edges vary by 1-2 pixels.

**The practical solution for CI:** Pin reference images to the software adapter. The wgpu
software adapter (WARP on Windows, selectable via `force_fallback_adapter: true` or
`WGPU_ADAPTER_NAME`) produces deterministic output for a given wgpu version and OS. Generate
reference images on CI using the software adapter and compare against them in CI only.

The wgpu-py project documents this explicitly: they use `WGPUPY_WGPU_ADAPTER_NAME=llvmpipe` for
their screenshot CI tests on Linux, store CI-generated reference images in the repository, and
note that "pixel perfect results will differ from those on the CI due to discrepancies in hardware
and driver versions."

For Sunlit Earth, this means:

1. Reference images stored under `tests/snapshots/warp/` (for WARP-generated baselines).
2. Tests explicitly pass `--software-rendering` when running on CI.
3. CI never compares against hardware GPU output — only against WARP output.
4. Developer machines use hardware GPUs; visual differences from reference images are expected and
   do not fail local tests by default (or a `COMPARE_SNAPSHOTS` env var gates the comparison).

### Pixel Comparison vs. Perceptual Comparison

**Exact pixel comparison** fails across software adapter versions and is appropriate only when
comparing two renders from the exact same adapter on the same machine. The `image` crate can do
this with `image::DynamicImage::raw_pixels()`.

**MSSIM (Mean Structural Similarity Index)** from the `image-compare` crate compares structural
similarity using 8x8 pixel windows averaged across the image. It is robust to sub-pixel shifts and
minor floating-point differences while catching meaningful regressions. A score of 0.97-0.99 is
a reasonable CI threshold for GPU rendering. See `2026-03-22-testability-observability-ui-testing.md`
for a full survey of comparison crates.

**Perceptual hash** (`img_hash` crate: aHash, dHash, pHash) provides a fast cheap similarity
pre-check but is too coarse to detect subtle rendering regressions. Use as a quick filter before
full MSSIM comparison.

### Reference Image Management

**Storage:** Reference images should be committed to the repository alongside the tests that use
them (e.g., `tests/snapshots/warp/<test-name>.png`). This makes them version-controlled and
reproducible.

**Update workflow:** Provide an environment variable (`UPDATE_SNAPSHOTS=true` or similar) that
causes tests to write actual output as the new reference rather than comparing. This mirrors the
`SLINT_CREATE_SCREENSHOTS=1` environment variable used by Slint's own screenshot test suite, and
the `UPDATE_SNAPSHOTS=true` convention used by `egui_kittest`.

**Per-platform vs. shared references:** If the software adapter produces identical output across
platforms (which WARP does on Windows), a single set of reference images is sufficient for CI.
If tests are also run on Linux CI (using lavapipe), separate reference sets may be needed because
WARP and lavapipe do not produce pixel-identical output.

**Format:** PNG (lossless). Never use JPEG for reference images — JPEG compression introduces
lossy artifacts that corrupt pixel comparisons.

**Test fixture pattern in Rust:**

```rust
fn compare_or_update(actual: &image::RgbaImage, name: &str) {
    let path = format!("tests/snapshots/warp/{name}.png");
    if std::env::var("UPDATE_SNAPSHOTS").is_ok() {
        actual.save(&path).expect("failed to save reference");
        return;
    }
    let reference = image::open(&path)
        .unwrap_or_else(|_| panic!("reference not found: {path}; run with UPDATE_SNAPSHOTS=true"))
        .into_rgba8();
    let result = image_compare::rgba_hybrid_compare(&reference, actual)
        .expect("comparison failed");
    assert!(
        result.score >= 0.97,
        "visual regression in {name}: score {:.4} < 0.97",
        result.score
    );
}
```

### CLI Flag Patterns for Screenshot Capture

Several mature projects implement a `--exit-after-frame` / `--screenshot` pattern:

- **Bevy** (`bevy_ci_testing`): `--ci-testing-config <ron-file>` specifies how many frames to
  render and what screenshots to capture. The exit is triggered after the configured frame count.
- **wgpu examples**: `--no-gui` flag renders to an offscreen texture and saves a PNG, then exits.
- **Firefox**: `firefox --headless --screenshot output.png <url>` renders a URL and exits.

The recommended CLI extension for Sunlit Earth (added to `main.rs` via `clap`):

```
--exit-after-frame          Render one frame, save it if --screenshot is given, then exit 0
--screenshot <path>         Path for screenshot output (PNG); requires --exit-after-frame
```

These flags would be guarded by `#[cfg(test)]` or only compiled in debug builds, or conditionally
compiled as a `testing` cargo feature to avoid bloating the release binary.

---

## 4. Combining Accessibility Inspection with Visual Validation

### The Layered Testing Strategy

Mature test frameworks combine accessibility tree inspection with screenshot comparison in a
deliberate order, using each layer for what it is good at:

**Layer 1: Accessibility tree assertions (structural/semantic)**

Query the UIA tree (or Slint `ElementHandle`, or kittest's `get_by_label`) to verify:
- Expected UI elements exist with correct labels and roles.
- Interactive elements are in the correct state (checkbox checked, slider value, button enabled).
- Dynamic UI updates are reflected (loading indicator visible/hidden, status text updated).

This layer runs fast (milliseconds), is robust to pixel-level rendering changes, and is the
primary assertion layer for UI state correctness.

Playwright's "aria snapshot" feature is the most sophisticated implementation of this layer: it
captures a YAML representation of the accessibility tree and allows asserting against it with
partial matching, regex patterns, and structural constraints. The update workflow
(`--update-snapshots`) makes regression detection easy. Slint does not have this level of tooling,
but the `ElementHandle` API covers the basics.

**Layer 2: Behavioral visual assertions (invariant-based)**

Assert that the rendered output satisfies invariants that must hold for any correct render, without
comparing against a specific reference image. Examples for Sunlit Earth:

- The center of the frame contains a non-black pixel (the globe is visible).
- The lit hemisphere has higher average brightness than the dark hemisphere.
- No pixel has an alpha value below 255 (no transparency bleed-through).
- Cloud pixels in a region with opacity > 0 are brighter than the same region with opacity = 0.

This layer catches GPU pipeline regressions, shader bugs, and texture loading failures without
requiring per-platform reference images. The existing `tests/render_pipeline.rs` uses this approach.

**Layer 3: Perceptual screenshot comparison (snapshot regression)**

Compare a rendered frame against a reference image using MSSIM. This is the most expensive and
brittle layer. Use it for:
- Detecting accidental visual regressions in specific rendering configurations.
- Providing a visual record of expected output in the repository.
- Catching subtle color shifts that invariant tests might miss.

Run this layer only on CI with the software adapter, against references generated by the same
software adapter.

### Integration Points for Slint + UIA

Slint's AccessKit integration exposes elements through UIA on Windows. The `accessible-label`
property in `.slint` maps to the UIA `Name` property. The `accessible-role` property maps to
the UIA `ControlType`.

Example: to make the longitude slider queryable by UIA test tools:

```slint
longitude-slider := Slider {
    accessible-label: "Longitude";
    accessible-role: slider;
    ...
}
```

With these properties set, external test tools (FlaUI, Accessibility Insights, WinAppDriver) can
find the element by name. The `i-slint-backend-testing` crate's `find_by_accessible_label` can
also use these labels.

The Slint `author_id` / `accessible-id` property (exposed via AccessKit's `author_id` field) maps
to the UIA `AutomationId` property, which is the most stable identifier for test automation.
Setting unique IDs on all interactive elements reduces reliance on fragile name-based or
hierarchy-based queries.

---

## 5. CI Considerations for Desktop E2E Tests

### Windows GitHub Actions Runners and GUI Apps

GitHub-hosted `windows-latest` runners (currently Windows Server 2022, migrating to Windows Server
2025 in September 2025) run in an interactive desktop session, not as a Windows service. This is
critical: when the GitHub Actions runner runs as a service, UI applications cannot display windows
(their processes exist in task manager but have no screen). The GitHub-hosted runners avoid this
problem by running as an interactive process.

The practical implication for Sunlit Earth: Slint window creation should succeed on
`windows-latest` runners without any special session setup. The `--software-rendering` flag
ensures the GPU path uses WARP rather than requiring a hardware GPU.

This distinguishes GitHub-hosted runners from self-hosted runners configured as Windows services,
where GUI apps do not work without additional session management (RDP connection, `tscon` utility,
or starting the runner via `run.cmd` rather than as a service).

### Headless vs. Non-Headless on Windows CI

On Linux, Xvfb (X Virtual Framebuffer) provides a headless display for GUI tests. The
`setup-headless-display-action` GitHub marketplace action can configure Xvfb across platforms
including Windows (using Mesa3D).

On Windows, "headless" for a Slint/wgpu app means two different things:

1. **No window, no event loop:** Use the Slint testing backend (`i-slint-backend-testing`) or
   exercise the renderer library directly without creating a window. This is already the approach
   in `tests/render_pipeline.rs` and works fine on Windows CI runners.
2. **Window exists but is not shown:** The window is created and the event loop runs, but the
   window is minimized or the rendering is diverted to an offscreen texture. This is needed for
   full-binary e2e tests.

For scenario (2) on Windows CI, the runner's interactive session means a real window can be
created. The risk is that the window obscures other windows or triggers unexpected paint events,
but in practice this is not a problem on CI runners where no other UI activity occurs.

### Flakiness Mitigation

The primary causes of e2e test flakiness for desktop apps and their mitigations:

**Race conditions on startup:** The app window appears before the first frame is rendered. Tests
that interact immediately after `spawn()` fail intermittently. Mitigation: use a log-line sentinel
or `--exit-after-frame` pattern (see section 2).

**Non-deterministic rendering:** GPU floating-point variance causes pixel comparison failures.
Mitigation: use software adapter on CI, perceptual comparison (MSSIM) rather than exact match,
and invariant-based assertions where possible.

**Animation timing:** Dynamic elements (loading indicator, timer-driven redraws) change between
assertion calls. Mitigation: mock time (`init_integration_test_with_mock_time()` in the Slint
testing backend), or use `--exit-after-frame` to capture a specific deterministic frame.

**Resource loading races:** Textures load asynchronously via `mpsc` channels. A screenshot
captured before texture load completes will show the grid placeholder, not the earth texture.
Mitigation: use test textures (small synthetic images), or wait for a specific log sentinel
("texture loaded") before capturing.

**Parallel test interference:** Multiple e2e tests running in parallel may compete for the display
or GPU resources. The existing project pattern (`LazyLock<Mutex<GpuContext>>`) serializes GPU
access within a test binary. For subprocess-based e2e tests, using `serial_test` or reducing test
parallelism (`--test-threads=1`) avoids window and GPU conflicts.

**Timeout calibration:** App startup time varies with system load. Use generous timeouts for CI
(30-60 seconds for startup) but detect readiness actively (log sentinel) rather than sleeping.

### Test Isolation

Each e2e test should start from a clean config state to avoid inter-test dependencies:

```rust
// Write a clean config before spawning the app
let config_dir = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| "/tmp".to_string());
let config_path = format!("{config_dir}/SunlitEarth/config.toml");
std::fs::write(&config_path, "# test config\n").ok();
```

Alternatively, the app can accept a `--config-path` flag to redirect config to a test-specific
location, avoiding interference with the developer's real config.

### CI Workflow Considerations

For the current Sunlit Earth CI (`ci.yml`):

- The existing `cargo test --locked` command already runs `tests/render_pipeline.rs` which
  exercises the GPU rendering pipeline on the WARP software adapter. This is the correct approach
  for GPU rendering tests.
- Subprocess-based e2e tests (if added) should be in a separate test binary with `harness = false`
  or a dedicated `[[test]]` target to control parallelism independently.
- Screenshot artifacts (actual output from failing tests) should be uploaded as workflow artifacts
  for visual inspection. GitHub Actions supports this via `actions/upload-artifact`.

Example CI workflow addition:

```yaml
- name: Upload test artifacts on failure
  if: failure()
  uses: actions/upload-artifact@v4
  with:
    name: test-screenshots
    path: tests/snapshots/actual/
    retention-days: 7
```

---

## 6. Tauri and Rust GUI Frameworks: E2E Patterns

### Tauri's WebDriver Architecture

Tauri v2 implements e2e testing through the WebDriver protocol. The `tauri-driver` crate acts as
a proxy between standard WebDriver clients (Selenium, WebdriverIO) and the platform-native
WebDriver server (EdgeDriver on Windows, WebKitWebDriver on Linux).

The architecture:

```
Test script (JS/Python)
    → WebDriver client (Selenium/WebdriverIO)
        → tauri-driver (Rust, port 4444)
            → EdgeDriver (Windows) or WebKitWebDriver (Linux)
                → Tauri app (spawned as subprocess)
```

`tauri-driver` handles app launch automatically: it starts the Tauri binary and connects
EdgeDriver to it. Tests do not need to manage subprocess lifecycle manually.

Assertions are standard WebDriver operations: find element by CSS selector or XPath, get text,
click, take screenshot. Tauri-specific APIs are accessible through `driver.execute()` which
evaluates JavaScript in the Tauri webview.

**Platform support:** Linux and Windows only. macOS has no WKWebView WebDriver implementation
(Apple does not provide one). CrabNebula's `tauri-driver` fork adds macOS support.

**Applicability to Slint:** Slint does not embed a webview, so the WebDriver-over-webview approach
does not apply. The analogous Slint path would be UIA-based automation (see section 4), but there
is no tauri-driver equivalent for Slint.

### Lessons from the Tauri E2E Pattern for Slint

Despite the architectural difference, several Tauri patterns transfer directly:

**App launch via subprocess with CARGO_BIN_EXE:** Tauri's CI documentation shows exactly the same
`CARGO_BIN_EXE_<name>` pattern for locating the binary in Rust integration tests.

**Separate test binary with `harness = false`:** Tauri's WebdriverIO example uses a separate
Node.js test runner, but the Rust equivalent is a `[[test]]` target with `harness = false` in
`Cargo.toml`, giving full control over the test lifecycle.

**Environment variables for test mode:** Tauri recommends checking an environment variable (e.g.,
`TAURI_TESTING=1`) in the app code to enable test-specific behavior like disabling animations,
using deterministic seeds, or skipping wallpaper write operations.

**`tauri-plugin-e2e-tests` (CrabNebula):** CrabNebula provides a plugin that adds structured test
result reporting and screenshot capture to Tauri apps, integrated with the WebDriver session. The
Slint analog would be a `--testing` CLI flag that enables screenshot capture and structured log
output optimized for test harness parsing.

### egui_kittest: The Best Rust-Native E2E Pattern

`egui_kittest` (from the egui and Rerun projects) is the most mature Rust-native e2e test framework
for a GPU-rendered GUI app. Its design is instructive for Slint:

1. **`Harness` struct:** A central test controller that wraps the UI state, processes events, and
   manages the render loop. Tests call `harness.run()` to advance the event loop by one step.

2. **AccessKit-based queries:** Uses `kittest` (a generic AccessKit-powered testing library) to
   find elements by accessible label: `harness.get_by_label("Check me!")`. This is the same
   accessibility-first philosophy as Testing Library in the JavaScript world.

3. **Software rasterizer preference:** `egui_kittest` enumerates all wgpu adapters and prefers
   software rasterizers for snapshot tests, rather than using `force_fallback_adapter: true` (which
   fails on macOS). On Windows, this selects WARP.

4. **Snapshot update workflow:** `UPDATE_SNAPSHOTS=true cargo test` regenerates reference images.
   `SnapshotOptions::threshold` controls per-test tolerance.

5. **Philosophy:** The `egui_kittest` documentation explicitly recommends using accessibility
   assertions and state assertions before reaching for screenshot comparison.

For Slint, the `i-slint-backend-testing` crate is the equivalent of `egui_kittest`, with the
AccessKit queries available through `find_by_accessible_label`. The main gap is that
`i-slint-backend-testing` does not currently expose a `Harness::snapshot()` equivalent for
capturing pixel output; snapshot tests for the wgpu render must be written separately against the
rendered texture.

---

## 7. Log-Based Assertions

### In-Process Log Assertion: `tracing-test`

The `tracing-test` crate provides the `#[traced_test]` proc macro which captures all tracing events
emitted during a test into an in-memory buffer:

```rust
#[cfg(test)]
mod tests {
    use tracing_test::traced_test;

    #[traced_test]
    #[test]
    fn test_texture_load() {
        // trigger some code that emits tracing events
        load_texture("earth.jxl");
        // assert that specific events were emitted
        assert!(logs_contain("texture loaded"));
        // assert complex conditions
        logs_assert(|lines| {
            let loaded = lines.iter().filter(|l| l.contains("texture loaded")).count();
            if loaded == 1 { Ok(()) } else { Err(format!("expected 1 load, got {loaded}")) }
        });
    }
}
```

This is the correct tool for in-process tests that need to verify tracing output. It works for both
`#[test]` and `#[tokio::test]` (async) tests.

**Limitations:** `tracing-test` is single-process. It cannot capture log output from a subprocess.
It also installs a global subscriber, which conflicts if another subscriber is already installed.
The `no-env-filter` feature must be enabled for integration tests in `tests/` to capture logs
from all crates (not just the crate under test).

### Cross-Process Log Assertion: JSON/NDJSON on Stderr

For subprocess-based e2e tests, log assertion requires parsing the subprocess's stderr. The
approach depends on whether the app writes structured (JSON) or human-readable logs.

**Structured JSON logs from `tracing-subscriber`:**

Configure the app with `tracing_subscriber::fmt().json()` output on stderr, activated when a
`--json-logs` CLI flag or `RUST_LOG_FORMAT=json` environment variable is set. The `Json` format
from `tracing-subscriber` writes newline-delimited JSON (NDJSON), one event per line:

```json
{"timestamp":"2026-03-24T12:00:00.123Z","level":"INFO","fields":{"message":"frame rendered","duration_ms":42},"target":"sunlit_earth::renderer"}
```

The test harness reads stderr line by line and parses each JSON object with `serde_json`:

```rust
fn find_log_event(lines: &[&str], target: &str, message: &str) -> Option<serde_json::Value> {
    lines.iter().find_map(|line| {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        if v["target"].as_str()? == target && v["fields"]["message"].as_str()? == message {
            Some(v)
        } else {
            None
        }
    })
}

// In the test:
let log_lines: Vec<&str> = stderr_text.lines().collect();
let event = find_log_event(&log_lines, "sunlit_earth::renderer", "frame rendered")
    .expect("expected 'frame rendered' event in logs");
let duration_ms = event["fields"]["duration_ms"].as_u64().unwrap_or(0);
assert!(duration_ms < 5000, "frame render took too long: {duration_ms}ms");
```

**Human-readable log parsing:**

If JSON format is not available, the test harness can search for substrings in the captured stderr:

```rust
let stderr_text = String::from_utf8_lossy(&output.stderr);
assert!(
    stderr_text.contains("frame rendered"),
    "expected 'frame rendered' in logs;\nstderr:\n{stderr_text}"
);
```

This is less precise but sufficient for smoke tests. The tradeoff: human-readable log format is
subject to change without notice, while structured JSON is a stable contract.

**Log level considerations:** Sunlit Earth uses `max_level_debug` in debug builds and
`release_max_level_warn` in release builds. E2E tests should run against the debug binary (which
is what `CARGO_BIN_EXE_sunlit-earth` points to during `cargo test`) so that `INFO` and `DEBUG`
events are available for assertion.

### Recommended Log Events to Assert In E2E Tests

Given Sunlit Earth's existing tracing instrumentation, the following events would be useful for
e2e log assertions:

| Event | Level | Location | Assertion use |
|---|---|---|---|
| First frame rendered | INFO | `renderer/render_pass.rs` | Readiness sentinel |
| GPU adapter selected | INFO | `wgpu_init.rs` | Verify software adapter on CI |
| Texture loaded | INFO | `renderer/textures.rs` | Wait for texture before screenshot |
| Clouds fetched | INFO | `cloud_fetcher.rs` | Verify cloud pipeline on CI |
| Memory RSS | DEBUG | `memory.rs` | Catch memory regressions |
| Config saved | INFO | `main.rs` | Verify wallpaper save path |

These events do not all exist yet — some would need to be added. The key ones for e2e testing are
"first frame rendered" (readiness sentinel) and "GPU adapter selected" (CI invariant: must be WARP
on CI runners).

---

## 8. Synthesis: Recommended Testing Tiers for Sunlit Earth

Pulling together the research, a practical testing hierarchy for Sunlit Earth has four tiers:

### Tier 1: In-Process Unit Tests (already in place)

Pure function tests in `src/*` modules using `#[test]`. No GPU, no window, no subprocess.
Fast, deterministic, high coverage. Examples: `camera.rs`, `datetime.rs`, `grid_texture.rs`.

### Tier 2: In-Process GPU Integration Tests (already in place)

`tests/render_pipeline.rs` and `tests/shading.rs` exercise the real wgpu pipeline with the
software adapter. Invariant-based assertions. No window or event loop. This is the correct layer
for rendering correctness.

### Tier 3: Slint UI Logic Tests (partially planned)

Use `i-slint-backend-testing` to test UI callbacks, property bindings, and state transitions
without any GPU. Verify that slider callbacks invoke the correct Rust functions, preset buttons
change the expected properties, and the load-defaults/reset callbacks work. These tests run in
the same process as the test binary, with a mock Slint backend. Planned in
`2026-03-24-slint-testing-research.md`.

### Tier 4: Full-Binary Smoke Tests (not yet implemented)

Launch the binary with `--software-rendering --exit-after-frame --screenshot <path>`, verify
exit code 0, and optionally run MSSIM comparison against a WARP-generated reference. This is the
only tier that exercises the integration between `main.rs`, Slint's event loop, and the wgpu
rendering notifier.

For this tier to be practical, two things must be added to `main.rs`:
1. A `--exit-after-frame` flag that causes the app to exit after the first successful render.
2. A `--screenshot <path>` flag that saves the rendered frame as PNG before exiting.

Both flags are guarded by a `testing` feature or only included in debug builds.

The full-binary smoke test catches: startup crashes, window creation failures, wgpu initialization
errors, shader compile failures, and any regression that prevents the first frame from rendering.
It does not catch subtle visual regressions (Tier 2 covers those).

---

## Confidence Assessment

| Topic | Confidence | Basis |
|---|---|---|
| Tauri WebDriver architecture | High | Official Tauri v2 docs |
| `CARGO_BIN_EXE_<name>` for test binary path | High | Cargo reference docs |
| Log-line sentinel for readiness detection | High | testcontainers-rs WaitFor pattern, widely used |
| `--exit-after-frame` pattern | High | Bevy CI, wgpu examples, Firefox |
| WARP software adapter on Windows GitHub Actions | High | Existing project CI, wgpu docs |
| Slint AccessKit/UIA integration maturity | Medium | Several open issues; basic labels work |
| `tracing-test` for in-process log assertion | High | docs.rs API, well-maintained crate |
| NDJSON log parsing for cross-process assertion | High | `tracing-subscriber` JSON format documented |
| GitHub-hosted windows-latest runner has interactive desktop | High | Multiple community reports confirm |
| egui_kittest as reference for Rust-native e2e patterns | High | docs.rs, GitHub |
| FlaUI/WinAppDriver for UIA-based automation | Medium | Works in principle; Slint UIA gaps noted |

## Knowledge Gaps

- **`--exit-after-frame` feasibility in Slint:** Slint's event loop runs until
  `quit_event_loop()` is called. Whether the `BeforeRendering` callback can call
  `quit_event_loop()` without causing a crash or deadlock requires empirical testing.
- **Slint `accessible-id` / `author_id` availability:** The `accessible-id` property that maps
  to UIA `AutomationId` was proposed in Slint issue #3972. Whether it is available in Slint ~1.15
  was not confirmed.
- **WARP stability on Windows Server 2025:** As `windows-latest` migrates to Windows Server 2025,
  the WARP adapter behavior may change. The existing tests should continue to work but should be
  monitored after the migration.
- **Cross-process `tracing` log capture without JSON format:** If the app uses the default
  human-readable format, log parsing is fragile. Adding a `--json-logs` flag or `RUST_LOG_FORMAT`
  env var would make cross-process log assertions much more reliable.

---

## Sources

- [Tauri v2 Testing Docs](https://v2.tauri.app/develop/tests/)
- [Tauri WebDriver Overview](https://v2.tauri.app/develop/tests/webdriver/)
- [Tauri CI with WebDriver](https://v2.tauri.app/develop/tests/webdriver/ci/)
- [CrabNebula tauri-e2e-tests plugin](https://docs.crabnebula.dev/plugins/tauri-e2e-tests/)
- [Electron Automated Testing Guide](https://www.electronjs.org/docs/latest/tutorial/automated-testing)
- [Podman Desktop Playwright E2E Tests](https://github.com/podman-desktop/e2e)
- [egui_kittest on docs.rs](https://docs.rs/egui_kittest)
- [egui PR #5714: Image comparison test guidelines](https://github.com/emilk/egui/pull/5714)
- [egui PR #5506: Software rasterizer preference for tests](https://github.com/emilk/egui/pull/5506)
- [kittest: AccessKit-based UI testing for Rust](https://github.com/rerun-io/kittest)
- [i-slint-backend-testing on docs.rs](https://docs.rs/i-slint-backend-testing/latest/i_slint_backend_testing/)
- [Slint testing.md on GitHub](https://github.com/slint-ui/slint/blob/master/docs/testing.md)
- [Slint accessibility author_id issue #3972](https://github.com/slint-ui/slint/issues/3972)
- [tracing-test on docs.rs](https://docs.rs/tracing-test/latest/tracing_test/)
- [tracing-test on GitHub](https://github.com/dbrgn/tracing-test)
- [tracing-subscriber JSON format](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/fmt/format/struct.Json.html)
- [testcontainers-rs WaitFor strategies](https://rust.testcontainers.org/features/wait_strategies/)
- [Microsoft UIA for Testing](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-usefortesting)
- [Playwright aria snapshots](https://playwright.dev/docs/aria-snapshots)
- [Squish for Qt: lessons learned](https://witekio.com/blog/automated-testing-framework-try-squish-with-a-qt-gui/)
- [Screenshot testing with Rust (Tony Finn)](https://tonyfinn.com/blog/rust-screenshot-testing/)
- [Visual regression testing flakiness reduction](https://www.visual-regression-testing.dev/reduce-visual-testing-flakiness)
- [wgpu-py screenshot CI with llvmpipe](https://wgpu-py.readthedocs.io/)
- [GitHub Actions windows-latest desktop session discussion](https://github.com/orgs/community/discussions/67003)
- [winit-test harness](https://github.com/notgull/winit-test)
- [Microsoft github-actions-for-desktop-apps sample](https://github.com/microsoft/github-actions-for-desktop-apps)
- [NoriSte ui-testing-best-practices](https://github.com/NoriSte/ui-testing-best-practices)
- [Flaky test mitigation strategies (Rainforest QA)](https://www.rainforestqa.com/blog/flaky-tests)
- [Modern E2E test architecture patterns](https://www.thunders.ai/articles/modern-e2e-test-architecture-patterns-and-anti-patterns-for-a-maintainable-test-suite)
