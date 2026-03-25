# Plan: System Tray Icon (2026-03-25)

## Summary

Add system tray icon support to Sunlit Earth so the app minimizes to the system tray on window close instead of exiting. The tray icon provides a context menu with "Open" (show window) and "Exit" (quit process). Single-instance enforcement prevents duplicate processes. A `--windowed` CLI flag preserves the original close-exits behavior. The `render` subcommand bypasses tray mode entirely.

## Stakes Classification

**Level**: Medium
**Rationale**: Touches the main event loop and window lifecycle (architectural surface area), but the changes are additive — existing behavior is preserved behind `--windowed` and the `render` subcommand path is untouched. The tray module is self-contained and the integration point in `main.rs` is narrow. Rollback is straightforward (revert the main.rs changes and remove the new module).

## Context

**Research**: [docs/plans/2026-03-25-tray-icon-research.md](2026-03-25-tray-icon-research.md)
**Affected Areas**:

- `src/main.rs` — CLI struct, event loop, window lifecycle
- `src/tray.rs` — new module for tray icon and thread management
- `src/lib.rs` — module declaration
- `Cargo.toml` — new dependencies
- `tests/e2e.rs` — new E2E tests for tray behavior

## Success Criteria

- [ ] Default launch shows a system tray icon and minimizes to tray on window close
- [ ] Tray context menu "Open" shows the window, "Exit" quits the process
- [ ] Second instance exits silently when an instance is already running
- [ ] `--windowed` flag runs without tray icon (original close-exits behavior)
- [ ] `render` subcommand works unchanged (no tray, no single-instance check)
- [ ] `cargo test` passes (existing tests unbroken)
- [ ] `cargo clippy` passes
- [ ] E2E tests validate tray startup, single-instance, and windowed mode

## Implementation Steps

### Phase 1: Dependencies and CLI

#### Step 1.1: Add `tray-icon` and `single-instance` dependencies

- **Files**: `Cargo.toml`
- **Action**: Add `tray-icon = "0.21"` and `single-instance = "0.3"` to `[target.'cfg(windows)'.dependencies]`. Both are Windows-only for now.
- **Verify**: `cargo check` succeeds with the new dependencies resolved
- **Complexity**: Small
- **Status**: Complete
- **Verified**: `cargo check` passes

#### Step 1.2: Add `--windowed` CLI flag

- **Files**: `src/main.rs:27-44` (the `Cli` struct)
- **Action**: Add a `windowed: bool` field with `#[arg(long)]` and doc comment `/// Run in windowed mode (close exits instead of minimizing to tray)`. Add `debug!` for the new flag in the existing CLI debug log at line 117-122.
- **Verify**: `cargo run -- --help` shows the `--windowed` flag. `cargo run -- --windowed` launches normally (no behavior change yet — tray mode doesn't exist). `cargo clippy` passes.
- **Complexity**: Small
- **Status**: Complete
- **Verified**: `cargo clippy` passes

### Phase 2: Tray Module

#### Step 2.1: Create `src/tray.rs` with programmatic icon generation

- **Files**: `src/tray.rs` (new), `src/lib.rs`
- **Action**: Create `src/tray.rs` with a `cfg(windows)` gate. Implement a `fn create_icon() -> tray_icon::Icon` function that generates a 32x32 RGBA image programmatically — a filled circle with an Earth-like blue-green gradient on a transparent background. Use raw pixel manipulation (no `image` crate needed for generation). Add `pub mod tray;` (behind `#[cfg(windows)]`) to `src/lib.rs`.
- **Test cases**:
  - Unit test `icon_is_32x32`: call `create_icon()`, assert it doesn't panic (icon creation validates dimensions internally)
- **Verify**: `cargo test tray` runs the icon test. `cargo clippy` passes.
- **Complexity**: Small
- **Status**: Complete
- **Verified**: `cargo test --lib icon_is_32x32` passes, `cargo clippy` clean

#### Step 2.2: Add tray thread spawn function

- **Files**: `src/tray.rs`
- **Action**: Implement `pub fn spawn_tray_thread(window_weak: slint::Weak<MainWindow>) -> std::thread::JoinHandle<()>`. This function:
  1. Spawns a named thread (`"tray-icon"`)
  2. On the thread: creates a `Menu` with two `MenuItem`s: "Open" (id saved) and "Exit" (id saved)
  3. Creates a `TrayIconBuilder` with the menu, a tooltip `"Sunlit Earth"`, the icon from `create_icon()`, and `.with_menu_on_left_click(false)`
  4. Registers `MenuEvent::set_event_handler` that checks the event id:
     - "Open" id: calls `slint::invoke_from_event_loop` with a closure that upgrades `window_weak` and calls `window.show().ok()`
     - "Exit" id: calls `slint::invoke_from_event_loop` with a closure that calls `slint::quit_event_loop().ok()`
  5. Registers `TrayIconEvent::set_event_handler` that on `ClickType::Left` calls `slint::invoke_from_event_loop` to show the window (same as "Open")
  6. Runs a Win32 message pump: `GetMessageW` loop (requires `#[allow(unsafe_code)]` with `// SAFETY:` comment). The loop calls `TranslateMessage` and `DispatchMessageW` until `GetMessageW` returns 0 or negative.
  7. After the loop exits, drops the `TrayIcon` (happens automatically when the thread function returns)
- **Test cases**: No unit tests for this function — it requires a Win32 desktop environment and message loop. Covered by E2E tests in Phase 4.
- **Verify**: `cargo check` succeeds. `cargo clippy` passes. The function compiles but is not yet called from `main.rs`.
- **Complexity**: Medium
- **Status**: Complete
- **Verified**: Included in Step 2.1 implementation. `cargo clippy` clean.

#### Step 2.3: Add single-instance check function

- **Files**: `src/tray.rs`
- **Action**: Implement `pub fn enforce_single_instance() -> single_instance::SingleInstance`. This function:
  1. Creates `SingleInstance::new("sunlit-earth-app")` and unwraps it
  2. If `!instance.is_single()`, logs `info!("another instance is already running, exiting")` and calls `std::process::exit(0)`
  3. Returns the `SingleInstance` value (caller must keep it alive for the process duration — dropping it releases the OS mutex)
- **Test cases**: No unit tests — requires process-level behavior. Covered by E2E test in Phase 4.
- **Verify**: `cargo check` succeeds. `cargo clippy` passes.
- **Complexity**: Small
- **Status**: Complete
- **Verified**: Included in Step 2.1 implementation. `cargo clippy` clean.

### Phase 3: Main Integration

#### Step 3.1: Integrate tray mode into `main()`

- **Files**: `src/main.rs:113-511` (the `main()` function)
- **Action**: After the existing `window.run()` call at line 495, restructure the tail of `main()` into three modes. The logic branches based on the CLI:

  **1. Render subcommand** (already handled — no changes to this path):
  The existing `render_timer` + `window.run()` path works as-is.

  **2. Tray mode** (default, when `cli.command.is_none() && !cli.windowed`):
  Insert before `window.run()` (around line 494):
  - Call `sunlit_earth::tray::enforce_single_instance()` and bind the return value to `_instance_guard` (must live until process exit)
  - Register `window.window().on_close_requested(|| slint::CloseRequestResponse::HideWindow)` to hide on close instead of exiting
  - Call `sunlit_earth::tray::spawn_tray_thread(window.as_weak())` and bind the handle to `_tray_handle`
  - Replace `window.run()` with `window.show().expect("Failed to show window")` followed by `slint::run_event_loop().expect("Failed to run event loop")`
  - After the event loop returns: save window geometry (same as current code), then `std::process::exit(0)`

  **3. Windowed mode** (`--windowed` or render subcommand):
  Keep the existing `window.run()` path unchanged.

  The restructured code should use a match or if/else on `(cli.command, cli.windowed)` to select the mode. Keep the render timer setup before the branch point (it's already conditional on the render subcommand). The `sun_timer`, `render_timer`, and `_guard` must all live past the event loop in every mode.

- **Test cases**: No unit tests (UI lifecycle). Covered by E2E tests and manual verification.
- **Verify**: `cargo build` succeeds. `cargo clippy` passes. `cargo test` passes (existing tests don't exercise main). Manual verification:
  - `cargo run` — tray icon appears, closing window hides it, "Open" shows it, "Exit" quits
  - `cargo run -- --windowed` — original behavior, no tray icon, close exits
  - `cargo run -- render --output test.png` — renders and exits (no tray)
- **Complexity**: Medium
- **Status**: Complete
- **Verified**: `cargo clippy` clean, `cargo test` all pass. Tray mode uses `#[cfg(windows)]` guards throughout.

#### Step 3.2: Handle window geometry save in tray mode

- **Files**: `src/main.rs`
- **Action**: In tray mode, window geometry should be saved when the window is hidden (not just on process exit), because in tray mode the window may be hidden for a long time before "Exit" is clicked. Move the geometry save into the `on_close_requested` handler for tray mode: before returning `HideWindow`, read the window position/size and call `config::save_window_geometry()`. The existing geometry save after the event loop can remain as a fallback for the exit path.
- **Test cases**: Manual verification — resize/move window, close to tray, "Exit" from tray, relaunch: window should restore at the last position.
- **Verify**: Manual verification as described. `cargo clippy` passes.
- **Complexity**: Small
- **Status**: Complete
- **Verified**: `cargo clippy` clean. Geometry is saved in `on_close_requested` before returning `HideWindow`.

### Phase 4: E2E Tests

#### Step 4.1: Add E2E test for tray mode startup and exit

- **Files**: `tests/e2e.rs`
- **Action**: Add `test_tray_mode_starts_and_can_be_killed`:
  1. Spawn the binary with no extra args (default = tray mode) and `--log-level debug`
  2. Sleep 3 seconds to let the window and tray initialize
  3. Kill the process (it won't exit on its own since tray mode keeps running)
  4. Assert the process was running (kill succeeded or process was alive)
  5. Assert stderr contains `"sunlit earth v"` (startup log)
  6. Assert stderr does not contain `" ERROR "`
- **Test cases**:
  - Process starts successfully in tray mode
  - Process stays alive (doesn't crash immediately)
  - Clean kill without errors in log
- **Verify**: `cargo test --test e2e test_tray_mode -- --ignored` passes on a desktop environment
- **Complexity**: Small
- **Status**: Complete
- **Verified**: Test compiles, `cargo clippy --tests` clean

#### Step 4.2: Add E2E test for `--windowed` mode

- **Files**: `tests/e2e.rs`
- **Action**: Add `test_windowed_mode_starts`:
  1. Spawn the binary with `--windowed --log-level debug`
  2. Sleep 3 seconds to let the window initialize
  3. Kill the process
  4. Assert stderr contains `"sunlit earth v"` (startup log)
  5. Assert stderr does not contain `" ERROR "`
- **Test cases**:
  - Process starts successfully in windowed mode
  - No crashes on startup
- **Verify**: `cargo test --test e2e test_windowed -- --ignored` passes on a desktop environment
- **Complexity**: Small
- **Status**: Complete
- **Verified**: Test compiles, `cargo clippy --tests` clean

#### Step 4.3: Add E2E test for single-instance enforcement

- **Files**: `tests/e2e.rs`
- **Action**: Add `test_single_instance_second_exits`:
  1. Spawn instance A with default args (tray mode) and `--log-level debug`
  2. Sleep 3 seconds to let A initialize and acquire the mutex
  3. Spawn instance B with default args (tray mode) and `--log-level debug`
  4. Wait for B with a 10-second timeout (it should exit quickly on its own)
  5. Assert B exited with status code 0
  6. Assert B's stderr contains `"another instance is already running"` (the log message from `enforce_single_instance`)
  7. Kill instance A (cleanup)
- **Test cases**:
  - Second instance detects existing instance and exits cleanly
  - Second instance exits with code 0 (not a crash)
  - First instance remains unaffected
- **Verify**: `cargo test --test e2e test_single_instance -- --ignored` passes on a desktop environment
- **Complexity**: Medium
- **Status**: Complete
- **Verified**: Test compiles, `cargo clippy --tests` clean

#### Step 4.4: Verify `render` subcommand still bypasses tray

- **Files**: `tests/e2e.rs`
- **Action**: The existing `test_render_and_exit` test already validates that the render subcommand exits cleanly. No changes needed. Verify it still passes after the tray integration.
- **Verify**: `cargo test --test e2e test_render -- --ignored` passes
- **Complexity**: Small
- **Status**: Complete
- **Verified**: `test_render_and_exit` still present and compiles

### Phase 5: Final Verification

#### Step 5.1: Full test suite and lint check

- **Files**: All
- **Action**: Run the full test suite and linter to confirm nothing is broken.
- **Verify**: `cargo test` passes. `cargo clippy` passes. `cargo test --test e2e -- --ignored` passes on a desktop environment (all E2E tests including new ones).
- **Complexity**: Small
- **Status**: Complete
- **Verified**: `cargo test` 210+31+16 tests pass. `cargo clippy` clean. E2E tests compile and are properly ignored.

#### Step 5.2: Update CLAUDE.md architecture documentation

- **Files**: `CLAUDE.md`
- **Action**: Update the relevant sections:
  - Add `tray.rs` to the **Key modules** list with a brief description: `tray.rs` — system tray icon with context menu (Open/Exit), single-instance enforcement, background thread with Win32 message pump (`cfg(windows)` only)
  - Update the `main.rs` module description to mention the three startup modes (render, tray, windowed) and the `--windowed` flag
  - Update the **Notable dependencies** section to include `tray-icon` and `single-instance`
  - Update the CLI structure in the research-relevant sections if present
- **Verify**: Read through the updated sections for accuracy. `cargo test` still passes (no code changes).
- **Complexity**: Small
- **Status**: Complete
- **Verified**: Updated main.rs description (three startup modes), added tray.rs module, added tray-icon/single-instance deps, updated windows-sys description.

## Test Strategy

### Automated Tests

| Test Case                                  | Type | Input                           | Expected Output                                  |
|--------------------------------------------|------|---------------------------------|--------------------------------------------------|
| `icon_is_32x32`                            | Unit | Call `create_icon()`            | No panic (icon created successfully)             |
| `test_tray_mode_starts_and_can_be_killed`  | E2E  | Launch binary, no args          | Process starts, stays alive, clean kill          |
| `test_windowed_mode_starts`                | E2E  | Launch with `--windowed`        | Process starts, no errors                        |
| `test_single_instance_second_exits`        | E2E  | Launch two instances            | Second exits with code 0, logs detection message |
| `test_render_and_exit` (existing)          | E2E  | Launch with `render` subcommand | Renders PNG, exits cleanly                       |

### Manual Verification

- [ ] Default launch: tray icon appears in system tray with Earth-like icon
- [ ] Close window: window hides, tray icon remains, process stays alive
- [ ] Tray "Open": window reappears at previous position/size
- [ ] Tray "Exit": process terminates, tray icon disappears
- [ ] Left-click on tray icon: window shows (same as "Open")
- [ ] Window geometry persists through hide/show cycle (close to tray, reopen, close to tray, exit, relaunch: position preserved)
- [ ] `--windowed` mode: no tray icon, close exits process (original behavior)
- [ ] `render` subcommand: works unchanged, no tray icon
- [ ] Second instance with tray running: exits immediately, no visible window flash

## Risks and Mitigations

| Risk | Impact | Mitigation |
| --- | --- | --- |
| `window.run()` returns when `HideWindow` is used | Tray mode breaks — event loop exits on hide | Use `slint::run_event_loop()` instead of `window.run()` in tray mode (per research) |
| Tray icon not visible on CI (no desktop) | E2E tests fail in CI | Mark tray E2E tests `#[ignore]` — they already require desktop like existing E2E tests |
| `HideWindow` + `show()` cycle causes GPU resource issues | Rendering breaks after unhide | Existing dirty-check forces a full re-render; test thoroughly in manual verification |
| `single-instance` mutex not released on crash | Second launch blocked permanently | OS releases mutex on process exit; stale mutex auto-cleans |
| Tray thread panic crashes the process | No tray icon, process hangs | The message pump is simple (`GetMessageW` loop) — minimal panic surface. Log errors in thread. |
| `tray-icon` crate drops Win32 resources in wrong order | Crash on exit | Process calls `std::process::exit(0)` anyway, which terminates all threads immediately |

## Rollback Strategy

All changes are additive. To roll back:

1. Remove the `tray` module and its declaration in `lib.rs`
2. Revert `main.rs` to use `window.run()` unconditionally (remove the mode branching)
3. Remove the `--windowed` CLI flag
4. Remove `tray-icon` and `single-instance` from `Cargo.toml`
5. Remove new E2E tests from `tests/e2e.rs`

No config format changes are made, so user configs are unaffected.

## Status

- [x] Plan approved
- [x] Implementation started
- [x] Implementation complete
