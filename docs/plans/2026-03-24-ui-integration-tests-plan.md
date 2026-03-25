# Plan: UI Integration Tests and E2E Screenshot Test (2026-03-24)

## Summary

Add two new test suites to Sunlit Earth: (1) a headless Slint testing backend test file (`tests/slint_ui.rs`) that exercises UI property bindings, callback wiring, conditional visibility, preset application, and default restoration without any GPU or display; and (2) an E2E screenshot test (`tests/e2e.rs`) that launches the full application binary with `--screenshot <path>`, waits for textures to load, renders the final scene with the real GPU, saves a screenshot as PNG, and exits. The test validates basic invariants (exit code, file exists, image dimensions, globe visible, no ERROR/WARN in logs). Supporting changes include a `--screenshot` CLI flag in `main.rs`, a texture-readiness signal in the renderer, a "first frame rendered" tracing event, and a new `i-slint-backend-testing` dev-dependency.

## Stakes Classification

**Level**: Medium
**Rationale**: Multiple files are affected (Cargo.toml, main.rs, renderer/mod.rs, two new test files), but changes are additive (new test infrastructure, new CLI flag) with minimal risk to existing functionality. The Slint testing backend is a dev-dependency only. The CLI flag is a no-op during normal use. Rollback is straightforward: revert the commit.

## Context

**Research**:

- `docs/plans/2026-03-22-testability-observability-ui-testing.md` -- Slint testing backend, wgpu headless rendering, visual regression strategy
- `docs/plans/2026-03-24-ui-testing-codebase.md` -- comprehensive reference of all UI properties, callbacks, slider identifiers, and data bindings
- `docs/plans/2026-03-24-slint-testing-research.md` -- practical depth on ElementHandle API, property access, event simulation, version compatibility, integration test patterns
- `docs/plans/2026-03-24-e2e-testing-practices.md` -- subprocess lifecycle, screenshot validation, log-based assertions, CI considerations
- `docs/plans/2026-03-24-windows-uia-research.md` -- Windows UI Automation research (deferred, not used in this plan)

**Affected Areas**: `Cargo.toml`, `src/main.rs`, `src/renderer/mod.rs`, `tests/slint_ui.rs` (new), `tests/e2e.rs` (new)

## Success Criteria

- [ ] `cargo test --test slint_ui` passes on both developer machines and CI (no GPU or display needed)
- [ ] `cargo test --test e2e` passes on machines with a desktop environment (gated behind `#[ignore]` so `cargo test` skips it by default; run with `cargo test --test e2e -- --ignored`)
- [ ] Slint UI tests cover: property round-trips, preset callback wiring, load-defaults restoration, advanced section visibility toggle, atmosphere conditional visibility, datetime conditional visibility
- [ ] E2E test validates: exit code 0, screenshot PNG file exists, image is non-zero dimensions, image center pixel is not black (globe visible), no ERROR or WARN lines in captured stderr
- [ ] The screenshot shows the rendered globe with loaded textures (not just the grid placeholder)
- [ ] `cargo test` (without extra flags) continues to pass unchanged -- no regressions in existing tests
- [ ] `cargo clippy` passes with no new warnings

## Implementation Steps

### Phase 1: Dependencies and Plumbing

#### Step 1.1: Add `i-slint-backend-testing` dev-dependency

- **Files**: `Cargo.toml`
- **Action**: Add `i-slint-backend-testing = "=1.15.1"` under `[dev-dependencies]`. Also add `image = { version = "0.25.8", default-features = false, features = ["png"] }` under `[dev-dependencies]` (for e2e screenshot validation; the main `[dependencies]` already has `image` but dev-dependencies are separate).
- **Verify**: `cargo check --tests` succeeds. The `Cargo.lock` resolves `i-slint-backend-testing` to exactly 1.15.1.
- **Complexity**: Small

#### Step 1.2: Add `--screenshot <PATH>` CLI flag

- **Files**: `src/main.rs` (the `Cli` struct)
- **Action**: Add one new clap field to the `Cli` struct:
  - `--screenshot <PATH>`: `Option<PathBuf>`, path to write a screenshot PNG. When provided, the app waits for textures to load, saves a screenshot of the rendered scene, and exits.
- **Test cases** (manual, verified in Step 1.5):
  - `cargo run -- --help` shows the new `--screenshot` flag in output
  - `cargo run` (no flag) behaves identically to before (no regression)
- **Verify**: `cargo build` succeeds. `cargo run -- --help` shows the new flag.
- **Complexity**: Small

#### Step 1.3: Add texture-readiness signal in the renderer

- **Files**: `src/renderer/mod.rs`
- **Action**: Add an `Arc<AtomicBool>` parameter to `setup_rendering_notifier()` (or create it inside and return it -- whichever is cleaner). This flag is set to `true` by the renderer after a frame is rendered where all required texture slots for the current mode are loaded (i.e., `!loading && bind_group.is_some()` for each slot needed by the active texture index). Cloud textures are excluded from the readiness check since they require an internet fetch.

  The check happens in the `BeforeRendering` branch, after `execute_render_pass` and `win.set_rendered_image(image)`. The readiness condition:
  - For single-texture modes (grid=0, day=1, night=2): the requested slot's `bind_group.is_some()` and `!loading`
  - For blend mode (index=3): both `DAY_SLOT` and `NIGHT_SLOT` have `bind_group.is_some()` and `!loading`, and `composite_bind_group.is_some()`
  - Grid texture (index=0) is always ready immediately (procedural, no async load)

  Once the flag is set to `true`, it stays `true`. The renderer sets it only once after the first fully-textured frame.

- **Verify**: `cargo build` succeeds, `cargo clippy` passes.
- **Complexity**: Medium

#### Step 1.4: Implement screenshot-and-exit logic in main.rs

- **Files**: `src/main.rs`
- **Action**: When `cli.screenshot` is `Some(path)`, register a Slint timer before `window.run()` that polls the texture-readiness flag every ~200ms. When the flag becomes `true`:
  1. Call `export_wallpaper_image(width, height)` to re-render the scene and get raw RGBA8 pixels. Use the current window dimensions (or a reasonable default like the quantized viewport size).
  2. Encode the pixels as PNG using the `image` crate.
  3. Write the PNG to the screenshot path.
  4. Call `slint::quit_event_loop()` to exit.

  `export_wallpaper_image()` already exists in `renderer/mod.rs` and handles the full pipeline: creates temporary GPU textures with `COPY_SRC`, renders using the last frame's state and bind groups, reads back pixels via a staging buffer. It runs on the UI thread via the `GPU_RESOURCES` thread-local, which is the same thread the Slint timer fires on.

  When `--screenshot` is not provided, no timer is registered and the app runs normally.

  ```rust
  if let Some(screenshot_path) = cli.screenshot {
      let window_weak = window.as_weak();
      let textures_ready = textures_ready_flag.clone(); // Arc<AtomicBool> from renderer
      let screenshot_timer = slint::Timer::default();
      screenshot_timer.start(TimerMode::Repeated, Duration::from_millis(200), move || {
          if textures_ready.load(Ordering::Relaxed) {
              if let Some(win) = window_weak.upgrade() {
                  // Use current viewport dimensions for the screenshot
                  let (w, h) = get_viewport_dimensions(&win);
                  match renderer::export_wallpaper_image(w, h) {
                      Ok(pixels) => {
                          save_png(&screenshot_path, w, h, &pixels);
                          info!("screenshot saved to {}", screenshot_path.display());
                      }
                      Err(e) => error!("screenshot failed: {e}"),
                  }
              }
              slint::quit_event_loop().unwrap();
          }
      });
  }
  ```

- **Test cases** (manual, verified in Step 1.6):
  - `cargo run -- --screenshot test_output.png` renders, waits for textures, saves PNG, exits
  - The PNG shows the rendered globe with loaded textures (not just the grid)
  - `cargo run` without the flag runs normally (the timer is never registered)
- **Verify**: `cargo build` succeeds, `cargo clippy` passes.
- **Complexity**: Medium

#### Step 1.5: Add "first frame rendered" tracing event

- **Files**: `src/renderer/mod.rs`
- **Action**: After `execute_render_pass` and `win.set_rendered_image(image)`, add a `tracing::info!("first frame rendered")` event that fires only on the first render. Use `res.last_state.is_none()` (checked before `res.last_state = Some(current_state)`) to detect the first frame. This event is useful for e2e log validation and general observability.
- **Test cases** (manual):
  - `cargo run -- --log-level info 2>&1 | grep "first frame rendered"` shows exactly one matching line
- **Verify**: Running the app with `--log-level info` shows "first frame rendered" in stderr exactly once.
- **Complexity**: Small

#### Step 1.6: Manual verification of --screenshot

- **Files**: N/A (manual verification)
- **Action**: Run all manual test cases from Steps 1.2, 1.4, and 1.5.
- **Manual test cases**:
  - `cargo run -- --screenshot /tmp/sunlit_test.png` saves a valid PNG and exits
  - Open the PNG and verify it shows a rendered globe with loaded textures (not just the grid placeholder)
  - `cargo run -- --screenshot /tmp/sunlit_test.png --log-level info` shows "first frame rendered" in stderr
  - `cargo run` (no new flags) works normally as before
  - `cargo test` (existing tests) still passes
  - `cargo clippy` passes
- **Verify**: All manual checks pass.
- **Complexity**: Small

### Phase 2: Slint UI Tests

#### Step 2.1: Create `tests/slint_ui.rs` with initialization boilerplate

- **Files**: `tests/slint_ui.rs` (new file)
- **Action**: Create the test file with:
  - `use sunlit_earth::MainWindow;` (re-exported from `lib.rs` via `slint::include_modules!()`)
  - `use slint::ComponentHandle;`
  - `use i_slint_backend_testing::ElementHandle;`
  - An `init()` function using `OnceLock` to call `i_slint_backend_testing::init_no_event_loop()` exactly once per process
  - A helper function `create_window() -> MainWindow` that calls `init()` then `MainWindow::new().unwrap()`
  - One trivial test (`test_window_creates_successfully`) that calls `create_window()` and asserts the window exists
- **Test cases**:
  - `test_window_creates_successfully`: `create_window()` does not panic
- **Verify**: `cargo test --test slint_ui` passes with 1 test.
- **Complexity**: Small

#### Step 2.2: Property round-trip tests

- **Files**: `tests/slint_ui.rs`
- **Action**: Add tests that set properties on the window and read them back, verifying the Slint binding layer works correctly.
- **Test cases**:
  - `test_camera_longitude_roundtrip`: Set `camera-longitude` to 42.5, read back, assert equal
  - `test_camera_latitude_roundtrip`: Set `camera-latitude` to -30.0, read back, assert equal
  - `test_camera_zoom_roundtrip`: Set `camera-zoom` to 0.75, read back, assert equal
  - `test_bool_property_roundtrip`: Set `diffuse-shading` to false, read back, assert false; set to true, assert true
  - `test_int_property_roundtrip`: Set `texture-index` to 2, read back, assert 2
- **Verify**: `cargo test --test slint_ui` passes with all property tests.
- **Complexity**: Small

#### Step 2.3: Preset callback wiring tests

- **Files**: `tests/slint_ui.rs`
- **Action**: Add tests that invoke `apply-preset` and verify camera properties change. Since the Slint testing backend does not execute `on_apply_preset` callbacks registered in `main.rs` (those are Rust closures attached at runtime), the test must register its own callback using `window.on_apply_preset(...)`. The test registers a callback that mimics the production behavior (reads from `PRESETS` array and sets properties), then invokes it.

  Alternatively, since the presets are `Button` elements with `clicked => { root.apply-preset(N); }` in the `.slint` file, clicking the button via `invoke_accessible_default_action()` will fire the Slint-side `apply-preset(N)` callback. The test must register an `on_apply_preset` handler that applies the preset values. This tests the wiring from button click through to callback invocation.
- **Test cases**:
  - `test_preset_europe_fires_callback`: Find "Europe" button via `find_by_accessible_label`, invoke default action, verify the `on_apply_preset` callback received index 0
  - `test_preset_earthrise_fires_callback`: Find "Earthrise" button, invoke, verify index 8
  - `test_preset_changes_camera_properties`: Register a callback that applies `PRESETS[0]` values, invoke "Europe" button, verify `camera-longitude` and `camera-latitude` changed from their initial values to the preset values
- **Verify**: `cargo test --test slint_ui` passes with preset tests.
- **Complexity**: Medium

#### Step 2.4: Load-defaults callback test

- **Files**: `tests/slint_ui.rs`
- **Action**: Test that clicking "Load Defaults" fires the `load-defaults` callback. Register an `on_load_defaults` handler that applies `AppConfig::default()` values to the window (mimicking `main.rs` behavior). Set non-default values, click the button, verify properties reset.
- **Test cases**:
  - `test_load_defaults_fires_callback`: Set `camera-longitude` to 123.0, register `on_load_defaults` handler that applies defaults, find "Load Defaults" button via `find_by_accessible_label`, invoke it, verify `camera-longitude` changed to the default value
  - `test_load_defaults_resets_multiple_properties`: Set several properties to non-default values (longitude, zoom, diffuse-shading), invoke load-defaults, verify all reset
- **Verify**: `cargo test --test slint_ui` passes.
- **Complexity**: Medium

#### Step 2.5: Advanced section visibility toggle test

- **Files**: `tests/slint_ui.rs`
- **Action**: Test the collapsible advanced section. The advanced section is controlled by the `advanced-open` property. When false, the slider elements inside the `if root.advanced-open:` block should not be in the element tree (or should have zero size). When true, they should appear.
- **Test cases**:
  - `test_advanced_section_starts_closed`: Create window, assert `get_advanced_open()` is false, query for `"MainWindow::longitude-slider"` via `find_by_element_id`, assert the iterator is empty (element not in tree because guarded by `if`)
  - `test_advanced_section_opens`: Create window, set `advanced-open` to true, query for `"MainWindow::longitude-slider"`, assert the iterator is non-empty (element now in tree)
  - `test_advanced_section_closes`: Set `advanced-open` to true then back to false, query for `"MainWindow::longitude-slider"`, assert empty again
- **Verify**: `cargo test --test slint_ui` passes. This test empirically determines whether `if`-guarded elements are absent from the tree or present with zero size (research noted this was an open question).
- **Complexity**: Small

#### Step 2.6: Conditional visibility tests for datetime and atmosphere

- **Files**: `tests/slint_ui.rs`
- **Action**: Test that sub-sections controlled by `use-custom-datetime` and `atmo-enabled` conditionally appear/disappear. Both are inside the advanced section, so `advanced-open` must be true first. The datetime sliders (Hour, Day) are not named with `:=` identifiers in the `.slint` file, so they cannot be found by element ID. Instead, test the property state: set `use-custom-datetime` to true, verify the property reads back as true. For atmosphere, the `rayleigh-intensity-slider` has a named identifier and is inside `if root.atmo-enabled:`, so it can be tested via element presence.
- **Test cases**:
  - `test_datetime_controls_hidden_when_disabled`: Set `advanced-open` to true, leave `use-custom-datetime` as false. Verify that `custom-hour` property exists but the datetime section is logically hidden (we can test this indirectly: the `use-custom-datetime` property is false).
  - `test_atmosphere_sliders_hidden_when_disabled`: Set `advanced-open` to true, set `atmo-enabled` to false. Query for `"MainWindow::rayleigh-intensity-slider"`, assert the iterator is empty (inside `if root.atmo-enabled:` guard).
  - `test_atmosphere_sliders_visible_when_enabled`: Set `advanced-open` to true, set `atmo-enabled` to true. Query for `"MainWindow::rayleigh-intensity-slider"`, assert non-empty.
- **Verify**: `cargo test --test slint_ui` passes.
- **Complexity**: Small

#### Step 2.7: Run full Slint UI test suite and verify

- **Files**: N/A (verification step)
- **Action**: Run `cargo test --test slint_ui` and `cargo clippy` to verify all Slint UI tests pass cleanly.
- **Manual test cases**:
  - `cargo test --test slint_ui` shows all tests passing
  - `cargo test --test slint_ui -- --nocapture` shows no unexpected output or warnings
  - `cargo clippy` passes with no new warnings
  - `cargo test` (all tests) still passes including existing `render_pipeline` and `shading` tests
- **Verify**: All commands succeed with no failures or warnings.
- **Complexity**: Small

### Phase 3: E2E Screenshot Test

#### Step 3.1: Create `tests/e2e.rs` with `#[ignore]` gate and helpers

- **Files**: `tests/e2e.rs` (new file)
- **Action**: Create the e2e test file with shared helper infrastructure:
  - All test functions are marked `#[ignore]` so they do not run during normal `cargo test`. They run only with `cargo test --test e2e -- --ignored`.
  - `env!("CARGO_BIN_EXE_sunlit-earth")` locates the compiled binary.
  - Helper: `wait_with_timeout(child, timeout) -> Output` that polls `child.try_wait()` in a loop with `Duration` timeout, calling `child.kill()` if the process doesn't exit in time. Returns the captured stdout, stderr, and exit status.
  - Helper: `create_temp_dir() -> PathBuf` that creates a unique temp directory for test artifacts.
  - One trivial test (`test_binary_exists`) that verifies `env!("CARGO_BIN_EXE_sunlit-earth")` points to an existing file.
- **Test cases**:
  - `test_binary_exists`: Asserts the binary path from `CARGO_BIN_EXE_sunlit-earth` exists on disk.
- **Verify**: `cargo test --test e2e` compiles (tests are skipped due to `#[ignore]`). `cargo test --test e2e -- --ignored` runs the smoke test.
- **Complexity**: Small

#### Step 3.2: Implement the main E2E screenshot test

- **Files**: `tests/e2e.rs`
- **Action**: Add the main e2e test that launches the binary as a subprocess with `--screenshot`, enforces a timeout from the test side, captures output, and validates invariants.

  The test function:
  1. Creates a temp directory and screenshot path
  2. Spawns the binary with `--screenshot <path> --log-level info`, capturing stderr via `Stdio::piped()`
  3. Calls `wait_with_timeout(child, Duration::from_secs(60))` -- if the app doesn't exit within 60 seconds, `child.kill()` terminates it and the test panics with a clear message
  4. Asserts exit code is 0
  5. Asserts the screenshot file exists and is non-empty
  6. Decodes the PNG with the `image` crate
  7. Asserts image dimensions are non-zero
  8. Samples the center pixel and asserts it is not pure black (R+G+B > 0), meaning the globe is visible
  9. Parses captured stderr line by line, asserts no line contains " ERROR " or " WARN " (the tracing format includes the level in the line)
  10. Asserts stderr contains "first frame rendered"
  11. Cleans up the temp directory

  The timeout is enforced entirely by the test process. The app itself has no timeout -- it simply waits for textures, saves, and exits. If anything goes wrong (GPU stall, texture load failure, broken pipeline), the test kills the process after 60 seconds.

- **Test cases**:
  - `test_screenshot_and_exit`: The main e2e test described above. Validates: exit code 0, screenshot file exists, PNG is decodable, image dimensions > 0, center pixel is not black, no ERROR/WARN in logs, "first frame rendered" appears in logs.
- **Verify**: `cargo test --test e2e -- --ignored` passes on a machine with a desktop environment and a GPU.
- **Complexity**: Medium

#### Step 3.3: Manual verification of E2E tests

- **Files**: N/A
- **Action**: Run the full e2e test suite and verify results.
- **Manual test cases**:
  - `cargo test --test e2e -- --ignored` passes on the development machine
  - `cargo test --test e2e -- --ignored --nocapture` shows subprocess stderr in the output (for debugging)
  - `cargo test` (without `--ignored`) compiles `e2e.rs` but skips the ignored tests (exit code 0, no failures)
  - Verify the temp directory is cleaned up after each test
  - `cargo clippy` passes
  - `cargo test` (all tests) passes
- **Verify**: All commands succeed.
- **Complexity**: Small

### Phase 4: Final Verification

#### Step 4.1: Run complete test suite and clippy

- **Files**: N/A
- **Action**: Run the full test suite and linter to verify no regressions.
- **Manual test cases**:
  - `cargo test` passes (includes slint_ui tests; skips e2e due to `#[ignore]`)
  - `cargo test --test e2e -- --ignored` passes (e2e tests)
  - `cargo clippy` passes with no warnings
  - `cargo build --release` succeeds (release build not broken by new dev-dependencies)
- **Verify**: All commands succeed.
- **Complexity**: Small

## Test Strategy

### Automated Tests

| Test Case | Type | File | Expected Outcome |
| --- | --- | --- | --- |
| Window creates successfully | Slint UI | `tests/slint_ui.rs` | `MainWindow::new()` does not panic |
| Camera longitude round-trip | Slint UI | `tests/slint_ui.rs` | Set 42.5, read back 42.5 |
| Camera latitude round-trip | Slint UI | `tests/slint_ui.rs` | Set -30.0, read back -30.0 |
| Camera zoom round-trip | Slint UI | `tests/slint_ui.rs` | Set 0.75, read back 0.75 |
| Bool property round-trip | Slint UI | `tests/slint_ui.rs` | Set false, read false; set true, read true |
| Int property round-trip | Slint UI | `tests/slint_ui.rs` | Set 2, read 2 |
| Preset Europe fires callback | Slint UI | `tests/slint_ui.rs` | `on_apply_preset` receives index 0 |
| Preset Earthrise fires callback | Slint UI | `tests/slint_ui.rs` | `on_apply_preset` receives index 8 |
| Preset changes camera properties | Slint UI | `tests/slint_ui.rs` | Longitude/latitude match PRESETS[0] values |
| Load defaults fires callback | Slint UI | `tests/slint_ui.rs` | Camera longitude resets to default |
| Load defaults resets multiple properties | Slint UI | `tests/slint_ui.rs` | Multiple properties match AppConfig::default() |
| Advanced section starts closed | Slint UI | `tests/slint_ui.rs` | `advanced-open` is false, longitude slider not in tree |
| Advanced section opens | Slint UI | `tests/slint_ui.rs` | Set `advanced-open` true, longitude slider in tree |
| Advanced section closes | Slint UI | `tests/slint_ui.rs` | Toggle back to false, longitude slider gone |
| Atmosphere sliders hidden | Slint UI | `tests/slint_ui.rs` | `atmo-enabled` false, rayleigh slider not in tree |
| Atmosphere sliders visible | Slint UI | `tests/slint_ui.rs` | `atmo-enabled` true, rayleigh slider in tree |
| Screenshot and exit e2e | E2E | `tests/e2e.rs` | Exit 0, PNG valid, globe visible with textures, no errors |

### Manual Verification

- [ ] `cargo run -- --screenshot /tmp/test.png` renders with loaded textures, produces a valid PNG, and exits
- [ ] `cargo run` (no new flags) works normally -- no behavioral regression
- [ ] `cargo run -- --help` shows the new `--screenshot` flag
- [ ] `cargo test` passes (slint_ui included, e2e skipped)
- [ ] `cargo test --test e2e -- --ignored` passes on the development machine
- [ ] `cargo clippy` passes with no new warnings

## Risks and Mitigations

| Risk | Impact | Mitigation |
| --- | --- | --- |
| `i-slint-backend-testing` version mismatch with `slint` | Slint UI tests fail to compile with cryptic errors | Pin to `"=1.15.1"` exactly matching the resolved version in `Cargo.lock`; document the version coupling |
| `init_no_event_loop()` panics if called twice | Only first test passes, rest panic | Use `OnceLock` guard pattern (documented in research) |
| `if`-guarded elements behave differently than expected | Visibility tests may need adjustment | Research noted this is an open question; tests will empirically determine behavior and adapt assertions |
| Textures never finish loading (broken path, missing file) | App hangs waiting for readiness signal | The app itself does not enforce a timeout; the e2e test kills the process after 60 seconds via `child.kill()`. This keeps the app simple and lets the test control failure modes |
| Shutdown race between screenshot timer and rendering notifier | Crash or hang on exit | Timer polls `AtomicBool` flag, calls `export_wallpaper_image` + `quit_event_loop()` outside the render callback. Decoupled from the render cycle |
| Slint testing backend and wgpu cannot coexist | Tests crash if both initialized | Already mitigated by architecture: `slint_ui.rs` and `e2e.rs` are separate test binaries; `slint_ui.rs` never initializes wgpu |
| E2E test flaky on CI (no desktop / no GPU) | Test fails on GitHub Actions | `#[ignore]` gate ensures e2e tests do not run in CI by default; can be added to CI later with `--ignored` when confirmed stable |
| Screenshot path fails on CI or different OS | File write error | Use `std::env::temp_dir()` for cross-platform temp paths |
| `MainWindow` not importable from test binary | Slint types not available | `lib.rs` calls `slint::include_modules!()` which makes `MainWindow` available via `use sunlit_earth::MainWindow` |
| `std::process::exit(0)` in `main.rs` interferes with screenshot save | Screenshot not written before exit | `quit_event_loop()` returns from `window.run()`, allowing normal shutdown flow; screenshot is saved before `quit_event_loop()` is called |
| Cloud texture fetch delays readiness | App waits for internet fetch before taking screenshot | Cloud textures are excluded from the readiness check; only file-based textures (day, night) gate readiness |

## Rollback Strategy

All changes are additive: new files (`tests/slint_ui.rs`, `tests/e2e.rs`), new dev-dependency, and the new `--screenshot` CLI flag. Rollback is a single `git revert` of the commit. The new CLI flag is inert when not passed, so even a partial rollback (removing only the test files) leaves the codebase functional.

## Key Design Decisions

**Why `--screenshot` waits for textures instead of `--exit-after-frame`?** The app defers texture loading -- the first frame renders with a grid placeholder, not the actual Earth textures. A naive "exit after first frame" would capture the placeholder, not the final image. The `--screenshot` flag waits for the texture-readiness signal before saving, ensuring the screenshot reflects the fully-rendered scene. This is the only new CLI flag; there is no separate `--exit-after-frame`.

**Why is the timeout in the test, not the app?** The app's `--screenshot` path simply waits for textures, saves, and exits. If anything goes wrong (GPU stall, texture load failure, broken pipeline), the test kills the process after 60 seconds via `child.kill()` (`TerminateProcess` on Windows). This is simpler than putting timeout logic in the app, and it works even if the app is hung (unresponsive event loop, GPU stall). The OS reclaims all GPU and process resources after termination.

**Why `export_wallpaper_image()` for pixel readback?** The `rendered-image` Slint property is a `slint::Image` that doesn't expose raw pixel data. `export_wallpaper_image()` already exists and does exactly what we need: creates temporary GPU textures, re-renders the scene using the last frame's state and bind groups, reads back RGBA8 pixels via a staging buffer. It runs on the UI thread via the `GPU_RESOURCES` thread-local, which is the same thread the Slint timer fires on.

**Why `#[ignore]` instead of a feature flag for e2e tests?** The `#[ignore]` attribute is simpler to use (`--ignored` flag) and does not require adding a cargo feature. Feature flags affect compilation of the main binary, which is undesirable for test-only functionality. `#[ignore]` keeps the gate purely at the test runner level.

**Why register callbacks in tests instead of importing from main.rs?** The callback registration code lives in `main.rs` (the binary crate), which cannot be imported by integration tests. The test file registers its own simplified callback handlers that mimic the production behavior. This tests the Slint wiring (button click triggers callback) separately from the Rust business logic (which is already tested elsewhere).

**Why `Arc<AtomicBool>` for texture-readiness signaling?** The rendering callback runs on the UI thread inside a `thread_local!` block. It cannot directly call `quit_event_loop()` from within the `BeforeRendering` callback (this would interfere with Slint's render cycle). Instead, it sets a flag that a Slint timer reads, and the timer calls `export_wallpaper_image()` + `quit_event_loop()`. This decouples the render cycle from the shutdown sequence.

**Why not `--software-rendering` in the e2e test?** The e2e test should use the real GPU if available, testing the actual user experience path. CI runners can add `--software-rendering` if needed when the e2e test is eventually enabled there.

## Status

- [ ] Plan approved
- [ ] Implementation started
- [ ] Implementation complete
