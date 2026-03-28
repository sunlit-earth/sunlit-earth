# Plan: slint-tray-minimal (2026-03-28)

## Summary

Build a minimal standalone Rust project at
`C:\Workspace\rustrover\slint-tray-minimal\` that tests whether
Slint + tray-icon on Windows can reliably keep timers alive
after `window.hide()`, using `run_event_loop_until_quit()`. This
isolates the Slint/winit event loop behavior from wgpu rendering,
GPU resources, and other Sunlit Earth complexity. The project
includes an integration test that exercises the full
hide/show/timer lifecycle via IPC.

## Stakes Classification

**Level**: Medium

**Rationale**: This is a new standalone project (no risk to
existing code), but it tests a critical architectural question.
The results directly determine whether Sunlit Earth's tray mode
uses `window.hide()` + `run_event_loop_until_quit()` (the
documented pattern) or must continue with the current workaround
(off-screen 1x1 window + `run_event_loop()`). Getting the test
harness wrong wastes time without answering the question.

## Context

**Research**:
`docs/plans/2026-03-28-research-slint-tray-app-patterns.md`

**Affected Areas**: New standalone project only; no changes to
Sunlit Earth

### Key findings from research

1. `run_event_loop_until_quit()` is the correct function for
   tray apps -- keeps the event loop alive after
   `window.hide()`.
2. `window.run()` internally calls `run_event_loop()`, which
   exits when the last window is hidden -- wrong for tray apps.
3. The parent project currently uses `run_event_loop()` with a
   1x1 off-screen window workaround because `window.hide()`
   appeared to kill timers.
4. Research concludes timers *should* survive under
   `run_event_loop_until_quit()`, but this has not been
   empirically verified without wgpu.
5. `invoke_from_event_loop` is the recommended cross-thread
   communication mechanism. The parent project avoids it due to
   deadlocks, but those may be wgpu-specific.
6. `quit_event_loop()` should be called directly from
   `invoke_from_event_loop`. The parent project defers via
   `Timer::single_shot(Duration::ZERO, ...)` but that was a
   wgpu-specific crash workaround.

### Philosophy

This project follows the RESEARCHED Slint recommendations
exactly, not the Sunlit Earth workarounds. The goal is to
determine whether the documented pattern works as specified.
A failing test is a legitimate outcome — it would confirm the
issues are in Slint/winit, not caused by wgpu side effects,
and produce a minimal reproduction for a bug report.

### What this project tests

- Does `run_event_loop_until_quit()` keep periodic timers alive
  after `window.hide()`?
- Does `invoke_from_event_loop` work reliably from a background
  thread (IPC or tray) without wgpu?
- Is the timer-deferred quit pattern needed without wgpu, or can
  `quit_event_loop()` be called directly?
- Does `window.show()` after `window.hide()` reliably restore
  the window?

## Success Criteria

- [ ] Standalone project compiles and runs on Windows
- [ ] Tray icon appears with "Show" and "Exit" menu items
- [ ] Window close button hides to tray (does not exit)
- [ ] Periodic timer (1-second) survives `window.hide()` --
      tick signals continue printing to stdout
- [ ] IPC commands (`quit`, `show-window`, `hide-window`) work
      from external process
- [ ] Integration test passes: spawns binary, hides window,
      verifies ticks continue, shows window, quits
- [ ] All stdout signals fire at expected times:
      `ipc_listener_ready`, `window_shown`, `window_hidden`,
      `tick_N`, `quit`

## Implementation Steps

### Phase 1: Project scaffolding

#### Step 1.1: Create project directory and Cargo.toml

- **Files**:
  `C:\Workspace\rustrover\slint-tray-minimal\Cargo.toml`
- **Action**: Run `cargo init` at the target path. Then edit
  `Cargo.toml` to add dependencies:
  - `slint = "~1.15"` (no `unstable-wgpu-28` -- default backend)
  - `tray-icon = "0.21"` (same version as parent)
  - `interprocess = "2"` (same version as parent)
  - `clap = { version = "4", features = ["derive"] }`
  - `windows-sys` (same features as parent, for message pump)
  - Dev-dependency: `interprocess = "2"` (for test IPC client)
  - Build-dependency: `slint-build = "~1.15"`
- **Verify**: `cargo check` succeeds
- **Complexity**: Small

#### Step 1.2: Create minimal Slint UI file

- **Files**:
  `C:\Workspace\rustrover\slint-tray-minimal\ui\main.slint`
- **Action**: Create a minimal Slint window with:
  - A `Text` element displaying `"Tick: {tick-count}"`
  - An `in-out property <int> tick-count: 0`
  - A `Button` labeled "Close" with callback `close-requested()`
  - Window title: "Slint Tray Minimal"
  - Preferred size: 400x300
- **Verify**: File is valid Slint syntax (verified by build in
  next step)
- **Complexity**: Small

#### Step 1.3: Create build.rs

- **Files**:
  `C:\Workspace\rustrover\slint-tray-minimal\build.rs`
- **Action**: Create a standard `slint-build` build script that
  compiles `ui/main.slint`
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

### Phase 2: Binary implementation

#### Step 2.1: Implement CLI argument parsing

- **Files**:
  `C:\Workspace\rustrover\slint-tray-minimal\src\main.rs`
- **Action**: Define a clap `Cli` struct with a single optional
  argument: `--ipc-socket <name>`. Parse CLI in `main()`. Print
  parsed args for debugging.
- **Test cases**:
  - Manual: `cargo run` starts without error
  - Manual: `cargo run -- --ipc-socket test` starts without error
- **Verify**: Binary compiles and runs, prints IPC socket name
  when provided
- **Complexity**: Small

#### Step 2.2: Implement the IPC listener module

- **Files**:
  `C:\Workspace\rustrover\slint-tray-minimal\src\ipc.rs`
- **Action**: Implement the IPC control channel using the
  RECOMMENDED Slint pattern (`invoke_from_event_loop` directly),
  NOT the Sunlit Earth workaround (shared queue + polling timer):
  - `spawn_ipc_listener(socket_name, window_weak)` function:
    - Creates local socket listener
    - Prints `SIGNAL:ipc_listener_ready` when bound
    - Accepts connections in a loop, reads single-line commands
    - For each command, calls `invoke_from_event_loop` directly
      with the appropriate action:
      - `show-window`: `window.show()` + print
        `SIGNAL:window_shown`
      - `hide-window`: `window.hide()` + print
        `SIGNAL:window_hidden`
      - `quit`: `quit_event_loop()` directly (NOT timer-deferred
        -- the timer deferral was a wgpu-specific workaround;
        test whether direct quit works without wgpu)
  - This follows the researched recommendation: use
    `invoke_from_event_loop` for cross-thread communication.
    If `invoke_from_event_loop` deadlocks without wgpu, that is
    a legitimate finding.
- **Test cases**:
  - Manual: Start binary with `--ipc-socket test-name`, verify
    `SIGNAL:ipc_listener_ready` appears on stdout
  - Manual: Connect to socket, send `quit\n`, verify process
    exits
- **Verify**: IPC listener starts, accepts connections,
  dispatches commands
- **Complexity**: Medium

#### Step 2.3: Implement the tray icon module

- **Files**:
  `C:\Workspace\rustrover\slint-tray-minimal\src\tray.rs`
- **Action**: Implement system tray icon, closely following the
  parent's `src/tray.rs` but simplified:
  - `create_icon()` function (reuse the parent's simple gradient
    icon generator)
  - `spawn_tray_thread(window_weak)` function:
    - Creates `TrayIcon` with menu ("Show", "Exit")
    - "Show" menu item: `invoke_from_event_loop` then
      `window.show()` (test whether this deadlocks without wgpu)
    - "Exit" menu item: `invoke_from_event_loop` then
      `Timer::single_shot(ZERO)` then `quit_event_loop()`
    - Left-click on icon: same as "Show"
    - Win32 message pump (`GetMessageW` loop)
  - Key difference from parent: use `invoke_from_event_loop`
    directly (the parent avoids this due to deadlocks -- test
    whether the deadlock is wgpu-specific)
- **Test cases**:
  - Manual: Run binary, verify tray icon appears in system tray
  - Manual: Click "Exit" in tray menu, verify process exits
  - Manual: Click "Show" after hiding, verify window reappears
- **Verify**: Tray icon appears, menu items work
- **Complexity**: Medium

#### Step 2.4: Wire everything together in main.rs

- **Files**:
  `C:\Workspace\rustrover\slint-tray-minimal\src\main.rs`
- **Action**: Implement the full event loop lifecycle following
  the researched Slint tray app pattern exactly:
  1. Parse CLI args
  2. Create `MainWindow`
  3. Set up periodic 1-second timer that:
     - Increments `tick-count` on the window
     - Prints `SIGNAL:tick_N` to stdout (N = tick number)
  4. Set `on_close_requested` returning
     `CloseRequestResponse::HideWindow` (recommended pattern)
  5. If `--ipc-socket` provided: spawn IPC listener thread
     (passes `window.as_weak()`, uses `invoke_from_event_loop`)
  6. Spawn tray thread (passes `window.as_weak()`, uses
     `invoke_from_event_loop`)
  7. `window.show()`
  8. `slint::run_event_loop_until_quit()` -- THE critical choice
     (NOT `window.run()`, NOT `run_event_loop()`)
  9. After event loop exits: print `SIGNAL:quit`
  - Declare `mod ipc;` and `mod tray;`
  - No shared command queue, no polling timer -- all cross-thread
    communication goes through `invoke_from_event_loop` as
    recommended
- **Test cases**:
  - Manual: Run binary, observe tick counter incrementing in
    window and on stdout
  - Manual: Close window (X button), verify ticks continue on
    stdout (THE critical test)
  - Manual: Click tray "Show", verify window reappears with
    correct tick count
  - Manual: Click tray "Exit", verify clean exit
- **Verify**: Full lifecycle works manually
- **Complexity**: Medium

### Phase 3: Integration test

#### Step 3.1: Create integration test file

- **Files**:
  `C:\Workspace\rustrover\slint-tray-minimal\tests\tray_lifecycle.rs`
- **Action**: Implement an integration test that exercises the
  full lifecycle:

  ```text
  test_timer_survives_hide:
    1. Spawn binary with --ipc-socket <unique-name>
    2. Wait for SIGNAL:ipc_listener_ready (timeout: 10s)
    3. Wait for at least SIGNAL:tick_2 (timer is running)
    4. Send hide-window via IPC
    5. Wait for SIGNAL:window_hidden (timeout: 5s)
    6. Record the tick count at hide time
    7. Wait 3 seconds
    8. Verify at least 2 more SIGNAL:tick_N after hide
       (THIS IS THE CRITICAL ASSERTION)
    9. Send show-window via IPC
   10. Wait for SIGNAL:window_shown (timeout: 5s)
   11. Send quit via IPC
   12. Wait for process exit (timeout: 5s)
   13. Assert exit code 0
  ```

  Include helper infrastructure:
  - `ChildGuard` RAII struct (kill-on-drop, same as parent)
  - `StdoutWatcher` (background thread collecting stdout lines,
    same pattern as parent)
  - `wait_with_timeout()` (poll `try_wait()`, same as parent)
  - `send_ipc_command()` (connect to local socket, send command,
    same as parent)
  - `unique_socket_name()` (PID + counter, same as parent)
  - `wait_for_signal()` method on `StdoutWatcher`
  - New: `count_signals_matching(prefix)` method to count tick
    signals
  - New: `max_tick_number()` method to extract the highest N
    from `SIGNAL:tick_N` lines

- **Test cases** (for the test itself):
  - `SIGNAL:ipc_listener_ready` appears within 10 seconds
  - `SIGNAL:tick_2` appears (timer is working)
  - `SIGNAL:window_hidden` appears after `hide-window` command
  - At least 2 tick signals appear AFTER the hide signal (timer
    survives hide)
  - `SIGNAL:window_shown` appears after `show-window` command
  - Process exits with code 0 after `quit` command
  - Timeout on any step produces a clear error message
    identifying which step failed

- **Verify**: `cargo test --test tray_lifecycle` passes (on a
  machine with a desktop environment)
- **Complexity**: Large

#### Step 3.2: Add a secondary test for invoke\_from\_event\_loop

- **Files**:
  `C:\Workspace\rustrover\slint-tray-minimal\tests\tray_lifecycle.rs`
- **Action**: Add a second test function
  `test_invoke_from_event_loop_no_deadlock` that:
  1. Spawns the binary with `--ipc-socket`
  2. Waits for readiness
  3. Rapidly sends 10 `show-window` commands in sequence (each
     triggers `invoke_from_event_loop` in the tray handler)
  4. Waits for the 10th `SIGNAL:window_shown` (timeout: 10s)
  5. Sends `quit`, waits for exit
  6. Asserts no deadlock occurred (the process did not hang)

  This tests whether `invoke_from_event_loop` deadlocks without
  wgpu.

- **Test cases**:
  - 10 rapid `show-window` commands do not hang the process
  - Process exits cleanly after `quit`
- **Verify**: Test passes without timeout
- **Complexity**: Medium

### Phase 4: Verification and documentation

#### Step 4.1: Run clippy and fix warnings

- **Files**: All `.rs` files in the project
- **Action**: Run `cargo clippy` and fix any warnings. Add the
  same pedantic clippy configuration as the parent project.
- **Verify**: `cargo clippy` produces no warnings
- **Complexity**: Small

#### Step 4.2: Run the full test suite

- **Files**: N/A
- **Action**: Run `cargo test --test tray_lifecycle` on a
  Windows machine with a desktop environment. Document the
  results:
  - Did the timer survive `window.hide()`?
  - Did `invoke_from_event_loop` deadlock?
  - Did the quit pattern work without the timer deferral?
  - Any unexpected behaviors?
- **Manual test cases**:
  - [ ] `cargo test --test tray_lifecycle` passes
  - [ ] Run binary manually, close window, verify stdout ticks
        continue
  - [ ] Run binary manually, use tray menu to show/hide/exit
- **Verify**: All tests pass, results documented
- **Complexity**: Small

#### Step 4.3: Document findings

- **Files**:
  `C:\Workspace\rustrover\slint-tray-minimal\FINDINGS.md`
- **Action**: Create a findings document recording:
  - Whether `run_event_loop_until_quit()` + `window.hide()`
    keeps timers alive
  - Whether `invoke_from_event_loop` deadlocks without wgpu
  - Whether timer-deferred quit is needed without wgpu
  - Whether `window.show()` reliably restores after `hide()`
  - Implications for the Sunlit Earth tray implementation
- **Verify**: Document exists with clear yes/no answers
- **Complexity**: Small

## Test Strategy

### Automated Tests

<!-- markdownlint-disable MD013 -->

| Test Case | Type | Input | Expected Output |
| --- | --- | --- | --- |
| Timer survives hide | Integration | hide-window IPC, wait 3s | 2+ tick signals after hide |
| IPC lifecycle | Integration | Sequence of IPC commands | All signals in order |
| No deadlock | Integration | 10 rapid show-window cmds | All 10 processed |
| Clean quit | Integration | quit IPC command | Exit code 0 |

<!-- markdownlint-enable MD013 -->

### Manual Verification

- [ ] Run binary, close window with X button, verify stdout
      still shows tick signals (timer alive)
- [ ] Run binary, verify tray icon appears with tooltip
      "Slint Tray Minimal"
- [ ] Right-click tray icon, select "Show", verify window
      reappears
- [ ] Right-click tray icon, select "Exit", verify process
      exits
- [ ] Run binary, close window, wait 30+ seconds, click "Show"
      in tray, verify window shows with high tick count
- [ ] Run binary with `--ipc-socket test`, connect manually
      via netcat/script, send commands

## Risks and Mitigations

<!-- markdownlint-disable MD013 -->

| Risk | Impact | Mitigation |
| --- | --- | --- |
| Timer dies after hide even without wgpu | High -- confirms issue is in Slint/winit | Document as definitive finding; file Slint bug report with minimal repro |
| `invoke_from_event_loop` deadlocks without wgpu | Medium -- tray menu stops working | Fall back to shared queue + polling timer pattern from parent project |
| Default backend behaves differently from wgpu backend | Medium -- results may not transfer | Note backend difference in findings; may need second test with wgpu feature |
| `tray-icon` menu events don't fire on tray thread | Low -- already working in parent | Parent project's Win32 message pump pattern is proven |
| IPC socket name collision between test runs | Low -- flaky tests | Use PID + counter for unique names, same as parent |

<!-- markdownlint-enable MD013 -->

## Rollback Strategy

This is a standalone project with no impact on Sunlit Earth. If
the approach proves unworkable, simply delete
`C:\Workspace\rustrover\slint-tray-minimal\`. No rollback needed
for the parent project.

## File Structure

```text
C:\Workspace\rustrover\slint-tray-minimal\
  Cargo.toml
  build.rs
  src/
    main.rs           # CLI, window creation, event loop, timer
    ipc.rs            # IPC listener + command queue + dispatcher
    tray.rs           # Tray icon, menu, Win32 message pump
  ui/
    main.slint        # Minimal UI (tick counter + close button)
  tests/
    tray_lifecycle.rs # Integration tests
  FINDINGS.md         # Results document (created in Phase 4)
```

## Dependency Versions

Pinned to match the parent project where relevant:

<!-- markdownlint-disable MD013 -->

| Crate | Version | Notes |
| --- | --- | --- |
| `slint` | `~1.15` | Same as parent, WITHOUT `unstable-wgpu-28` |
| `slint-build` | `~1.15` | Build dependency for `.slint` compilation |
| `tray-icon` | `0.21` | Same as parent |
| `interprocess` | `2` | Same as parent, for IPC |
| `clap` | `4` (with `derive`) | Same as parent |
| `windows-sys` | `0.59` | Same as parent, for Win32 message pump |

<!-- markdownlint-enable MD013 -->

## Status

- [ ] Plan approved
- [ ] Implementation started
- [ ] Implementation complete
