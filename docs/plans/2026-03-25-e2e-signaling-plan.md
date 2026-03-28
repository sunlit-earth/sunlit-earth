# Plan: E2E Test Signaling via IPC (2026-03-25)

## Summary

Add an opt-in IPC control channel to Sunlit Earth using
the `interprocess` crate's `local_socket` API, replace the
`--windowed` boolean flag with a `--mode <tray|window>`
enum and add `--tray-start <visible|hidden>`, and upgrade
the existing e2e tests to use IPC-based graceful shutdown
and stderr-based readiness detection instead of
sleep-then-kill.

## Stakes Classification

**Level**: Medium
**Rationale**: The change touches CLI parsing (breaking
change to `--windowed`), adds a new module (`ipc.rs`),
modifies `main.rs` orchestration, and rewrites all three
interactive-mode e2e tests. However, the IPC is opt-in (no
impact on default usage), the `render` subcommand is
unchanged, and all changes are testable. Rollback is
straightforward since the IPC module is self-contained.

## Context

**Research**:
[2026-03-25-e2e-signaling-research.md](2026-03-25-e2e-signaling-research.md)

**Affected Areas**:

- `src/main.rs` -- CLI definition, startup mode branching,
  IPC thread spawn
- `src/ipc.rs` -- new module: IPC listener thread, command
  dispatch
- `src/lib.rs` -- module declaration for `ipc`
- `tests/e2e.rs` -- rewritten tray/windowed tests, new
  show/hide test
- `Cargo.toml` -- new dependency `interprocess`
- `CLAUDE.md` -- updated CLI flags and module documentation

## Success Criteria

- [x] `--mode tray` (default) starts tray mode;
  `--mode window` starts windowed mode; the old
  `--windowed` flag is removed
- [x] `--tray-start hidden` starts the app in tray mode
  with the window hidden
- [x] `--ipc-socket <name>` spawns a background listener
  thread on a local socket; without this flag, no IPC
  listener is created
- [x] Sending `quit\n` to the IPC socket triggers
  `slint::quit_event_loop()` and the process exits with
  code 0
- [x] Sending `show-window\n` and `hide-window\n` controls
  window visibility and produces the expected log messages
- [x] E2E tests use stderr streaming to detect
  `"first frame rendered"` for readiness instead of
  sleeping
- [x] E2E tests use the IPC socket to send `quit` for
  graceful shutdown instead of killing the process
- [x] E2E tests pass exit code 0 assertions after
  IPC-triggered shutdown
- [x] `cargo test` and `cargo clippy` pass with no
  regressions

## Implementation Steps

### Phase 1: CLI Refactoring

#### Step 1.1: Replace `--windowed` with `--mode` and `--tray-start`

- **Files**: `src/main.rs:22-45`
- **Action**:
  - Define a `#[derive(Clone, Copy, ValueEnum)]` enum
    `Mode { Tray, Window }` with
    `default_value_t = Mode::Tray`.
  - Define a `#[derive(Clone, Copy, ValueEnum)]` enum
    `TrayStart { Visible, Hidden }` with
    `default_value_t = TrayStart::Visible`.
  - Replace the `windowed: bool` field with `mode: Mode`.
  - Add `tray_start: TrayStart` field with a runtime
    validation that `--tray-start hidden` is only valid
    when `--mode tray`.
  - Add `ipc_socket: Option<String>` field with
    `#[arg(long)]`.
  - Update all references to `cli.windowed` to use
    `cli.mode` instead: `run_event_loop` signature,
    startup mode branching, debug log.
- **Verify**: `cargo build` succeeds.
  `cargo run -- --help` shows the new `--mode` and
  `--tray-start` flags.
  `cargo run -- --mode window` starts windowed mode.
  `cargo run -- --tray-start hidden` starts with window
  hidden.
- **Complexity**: Medium

#### Step 1.2: Wire `--tray-start hidden` into `run_event_loop`

- **Files**: `src/main.rs:206-327` (`run_event_loop`)
- **Action**:
  - Add a `tray_start` parameter to `run_event_loop`.
  - In the tray mode branch (around line 303-308), if
    `tray_start` is `Hidden`, skip the `window.show()`
    call so the window starts hidden in the tray. The
    event loop still runs via
    `run_event_loop_until_quit()`.
  - Pass `cli.tray_start` from `main()` into
    `run_event_loop()`.
- **Verify**: `cargo run -- --tray-start hidden` starts
  with no visible window but the tray icon appears.
  Clicking "Open" in the tray menu shows the window.
- **Complexity**: Small

### Phase 2: IPC Module

#### Step 2.1: Add `interprocess` dependency

- **Files**: `Cargo.toml`
- **Action**: Add `interprocess = "2"` to
  `[dependencies]`. Use only the default features (which
  include sync local sockets).
- **Verify**: `cargo build` succeeds with the new
  dependency.
- **Complexity**: Small

#### Step 2.2: Create `src/ipc.rs` with listener and dispatch

- **Files**: `src/ipc.rs` (new file), `src/lib.rs`
- **Action**:
  - Add `pub mod ipc;` to `src/lib.rs`.
  - Create `src/ipc.rs` with the following public API:
    `pub fn spawn_ipc_listener(socket_name: &str,
    window_weak: slint::Weak<crate::MainWindow>)
    -> std::thread::JoinHandle<()>` -- spawns a
    background thread that:
    1. Converts `socket_name` to a namespaced name via
       `to_ns_name::<GenericNamespaced>()`.
    2. Creates a `ListenerOptions` and calls
       `create_sync()` to bind the local socket.
    3. Logs `info!("ipc listener ready on
       {socket_name}")` -- this is the signal that e2e
       tests can use to know the socket is accepting
       connections.
    4. Loops on `listener.accept()`, reading each
       connection line-by-line (via `BufReader`).
    5. Dispatches recognized commands (`show-window`,
       `hide-window`, `quit`) via
       `slint::invoke_from_event_loop`.
    6. For `quit`: calls
       `slint::quit_event_loop().ok()` inside the
       invoked closure.
    7. For `show-window`: calls `win.show().ok()` and
       logs `debug!("main window shown (from ipc)")`.
    8. For `hide-window`: calls
       `win.window().hide().ok()` and logs
       `debug!("main window hidden (from ipc)")`.
    9. Unrecognized commands are logged as
       `warn!("unknown ipc command: {cmd}")` and
       ignored.
  - The listener thread should handle `accept()` errors
    gracefully (log and continue for transient errors;
    break on fatal errors).
  - The thread name should be `"ipc-listener"`.
  - **Design notes**:
    - Each client connection is short-lived: connect,
      send one command line, disconnect. No response is
      sent back (fire-and-forget protocol). This matches
      the research recommendation.
    - The listener thread blocks on `accept()`, so when
      `quit` triggers `process::exit(0)`, the thread is
      terminated along with the process. No explicit
      shutdown of the listener is needed.
    - Use `GenericNamespaced` for the socket name to
      avoid filesystem cleanup issues (abstract
      namespace on Linux, `\\.\pipe\` on Windows).
- **Test cases**: No unit tests for the listener (it
  requires a running event loop and is tested via e2e).
  The module is thin glue code.
- **Verify**: `cargo build` succeeds. `cargo clippy`
  passes.
- **Complexity**: Medium

#### Step 2.3: Wire IPC into `main.rs`

- **Files**: `src/main.rs`
- **Action**:
  - In `run_event_loop`, after the tray/windowed mode
    branching setup but before starting the event loop,
    if `ipc_socket` is `Some(name)`:
    - Call `sunlit_earth::ipc::spawn_ipc_listener(
      &name, window.as_weak())`.
    - Store the `JoinHandle` in a `_ipc_handle` variable
      to keep it alive.
  - Add `ipc_socket: Option<String>` parameter to
    `run_event_loop` and pass `cli.ipc_socket` from
    `main()`.
- **Verify**: `cargo build` succeeds. Running with
  `--ipc-socket test-name` logs `"ipc listener ready"`.
- **Complexity**: Small

### Phase 3: E2E Test Rewrite

#### Step 3.1: Add IPC client helper to e2e tests

- **Files**: `tests/e2e.rs`, `Cargo.toml`
- **Action**:
  - Add `interprocess = "2"` to `[dev-dependencies]` in
    `Cargo.toml`.
  - Add a helper function
    `send_ipc_command(socket_name: &str, command: &str)`
    that:
    1. Converts `socket_name` to a namespaced name via
       `to_ns_name::<GenericNamespaced>()`.
    2. Creates a `Stream::connect()` to the local
       socket.
    3. Writes `"{command}\n"` to the stream.
    4. Drops the stream (close connection).
  - Add a `StderrWatcher` struct that:
    1. Takes ownership of `child.stderr` via `.take()`,
       wraps it in a `BufReader`.
    2. Spawns a thread that reads stderr line-by-line,
       collecting lines into a `Vec<String>` behind an
       `Arc<Mutex<>>`.
    3. Provides a `wait_for_log(needle: &str,
       timeout: Duration)` method that polls the
       collected lines for the given substring with a
       retry loop (10ms sleep, `timeout` deadline).
       Panics if the timeout elapses.
    4. Provides a `lines() -> Vec<String>` method to
       retrieve all accumulated stderr lines for
       post-exit assertions.
  - This streaming approach allows tests to wait for
    specific log messages at each state transition
    (rendering complete, window shown, window hidden)
    rather than asserting on final output only.
  - Add a helper function
    `unique_socket_name() -> String` that returns
    `format!("sunlit-earth-test-{}", std::process::id())`
    combined with a counter or timestamp for uniqueness.
- **Verify**: Helpers compile. Used by subsequent test
  steps.
- **Complexity**: Medium

#### Step 3.2: Rewrite tray mode test with full IPC lifecycle

- **Files**: `tests/e2e.rs`
- **Action**:
  - Rename `test_tray_mode_starts_and_can_be_killed` to
    `test_tray_mode_ipc_lifecycle`.
  - Generate a unique socket name via
    `unique_socket_name()`.
  - Spawn the binary with `["--log-level", "debug",
    "--tray-start", "hidden", "--ipc-socket",
    &socket_name]`.
  - Use `wait_for_log()` to stream stderr and wait for
    `"first frame rendered"` (confirms rendering works
    even when window is hidden).
  - Send `show-window` via IPC. Use `wait_for_log()` to
    confirm `"main window shown (from ipc)"` appears.
  - Send `hide-window` via IPC. Use `wait_for_log()` to
    confirm `"main window hidden (from ipc)"` appears.
  - Send `quit` via IPC.
  - Call `wait_with_timeout()` (existing helper) with a
    10-second timeout to collect the exit.
  - Assert exit code 0.
  - Assert stderr contains `"startup mode: tray"`.
  - Assert stderr contains `"ipc listener ready"`.
  - Assert stderr contains `"exiting"`.
  - Assert no `" ERROR "` lines in stderr.
- **Test cases**: Tray mode starts hidden, IPC listener
  binds, first frame renders, show-window makes window
  visible (verified via log), hide-window hides it
  (verified via log), `quit` triggers graceful exit
  with code 0. This single test validates the full
  tray mode IPC lifecycle: startup, rendering, window
  control, and graceful shutdown.
- **Verify**: `cargo test --test e2e
  test_tray_mode_ipc_lifecycle -- --ignored` passes.
- **Complexity**: Medium

#### Step 3.3: Rewrite windowed mode test to use IPC

- **Files**: `tests/e2e.rs`
- **Action**:
  - Rename `test_windowed_mode_starts` to
    `test_windowed_mode_graceful_shutdown`.
  - Generate a unique socket name.
  - Spawn with `["--mode", "window", "--log-level",
    "debug", "--ipc-socket", &socket_name]`.
  - Call `stderr watcher's `wait_for_log()`` to detect readiness.
  - Send `quit` via IPC.
  - Assert exit code 0, `"startup mode: windowed"`,
    `"exiting"`, no errors.
- **Test cases**: Windowed mode starts, IPC quit triggers
  graceful exit with code 0.
- **Verify**: `cargo test --test e2e
  test_windowed_mode_graceful_shutdown -- --ignored`
  passes.
- **Complexity**: Small

#### Step 3.4: Rewrite single-instance test to use IPC cleanup

- **Files**: `tests/e2e.rs`
- **Action**:
  - Generate a unique socket name for instance A.
  - Spawn instance A with `["--log-level", "debug",
    "--ipc-socket", &socket_name]`.
  - Call `stderr watcher's `wait_for_log()`` on instance A.
  - Spawn instance B with `["--log-level", "debug"]`
    (no IPC socket needed for B -- it will detect A's
    mutex and exit).
  - Assert instance B exits with code 0 and stderr
    contains `"another instance is already running"`.
  - Clean up instance A via
    `send_ipc_command(&socket_name, "quit")` followed by
    `wait_with_timeout()`.
  - Assert instance A exits with code 0.
- **Test cases**: Second instance detects first via OS
  mutex, exits cleanly. First instance shuts down
  gracefully via IPC.
- **Verify**: `cargo test --test e2e
  test_single_instance_second_exits -- --ignored` passes.
- **Complexity**: Small

#### Step 3.5: Remove `spawn_run_and_kill` helper

- **Files**: `tests/e2e.rs`
- **Action**: Delete the `spawn_run_and_kill` function,
  which is no longer used after the test rewrites. All
  interactive tests now use `wait_for_ready` + IPC
  `quit` + `wait_with_timeout`.
- **Verify**: `cargo build --test e2e` succeeds with no
  dead code warnings.
- **Complexity**: Small

### Phase 4: Documentation Updates

#### Step 4.1: Update CLAUDE.md

- **Files**: `CLAUDE.md`
- **Action**:
  - Update the CLI description in the `main.rs` module
    entry to document `--mode <tray|window>`,
    `--tray-start <visible|hidden>`, and
    `--ipc-socket <name>`.
  - Add `ipc.rs` to the **Key modules** list with a
    brief description.
  - Add `interprocess` to the **Notable dependencies**
    list.
  - Update the `run_event_loop` description to mention
    IPC thread spawning.
  - Update the e2e test description to mention IPC-based
    shutdown and stderr-based readiness.
- **Verify**: CLAUDE.md accurately reflects the new code.
- **Complexity**: Small

## Test Strategy

### Automated Tests

<!-- markdownlint-disable MD013 -->

| Test Case | Type | Expected Outcome |
| --- | --- | --- |
| Tray mode IPC lifecycle | E2E | Start hidden, render, show, hide, quit — all verified via log streaming, exit 0 |
| Windowed mode graceful shutdown | E2E | Exit 0, `"exiting"` in stderr |
| Single-instance + IPC cleanup | E2E | B exits 0, A exits 0 via quit |

<!-- markdownlint-enable MD013 -->

### Manual Verification

- [ ] `cargo run -- --mode tray` starts with tray icon,
  close hides to tray (same as before)
- [ ] `cargo run -- --mode window` starts without tray
  icon, close exits (same as before)
- [ ] `cargo run -- --tray-start hidden` starts with no
  visible window, tray icon present
- [ ] `cargo run -- --mode window --tray-start hidden` is
  rejected (validation error)
- [ ] `cargo run` (no flags) starts in tray mode with
  visible window (default behavior preserved)
- [ ] `cargo test --test e2e -- --ignored` passes all
  e2e tests
- [ ] `cargo clippy` passes with no new warnings

## Risks and Mitigations

<!-- markdownlint-disable MD013 -->

| Risk | Impact | Mitigation |
| --- | --- | --- |
| `interprocess` build issues on Windows | Build failure | Pin to `interprocess = "2"` (actively maintained). Test on Windows CI early. |
| IPC listener blocks on `accept()` after quit | Process hangs | Not an issue: `process::exit(0)` terminates all threads. |
| Stderr streaming misses readiness message | Test flakiness | Tracing uses non-blocking writer; BufReader reads lines as they arrive. |
| Removing `--windowed` breaks scripts | User confusion | Flag was only used in dev/tests. `--mode window` is a direct replacement. |
| Socket name collision in parallel tests | Test flakiness | Unique names via PID + timestamp. Tests are `#[serial]` anyway. |
| `first frame rendered` may not fire when hidden | Test hangs | Rendering notifier fires regardless of visibility. Fallback: use `"ipc listener ready"`. |

<!-- markdownlint-enable MD013 -->

## Rollback Strategy

All changes are additive and isolated:

- `src/ipc.rs` is a new file that can be deleted.
- CLI changes in `main.rs` can be reverted to restore
  `--windowed`.
- The `interprocess` dependency can be removed from
  `Cargo.toml`.
- E2E tests can be reverted independently.

No existing functionality is modified in a way that cannot
be trivially undone.

## Open Decisions (Resolved from Research)

The research document raised four open questions. Here are
the resolutions based on the user's requirements:

1. **IPC opt-in vs. always-on**: Opt-in via
   `--ipc-socket`. Resolved by user requirement.
2. **Protocol responses**: Fire-and-forget (no
   responses). The test validates behavior via stderr log
   messages and exit codes, not IPC responses. This keeps
   the protocol minimal.
3. **Show-window testability without display**: E2E tests
   are already `#[ignore]` and require a desktop + GPU.
   The test confirms behavior via log messages
   (`"main window shown (from ipc)"`), not visual
   inspection.
4. **Cloud fetcher interruption on quit**: Not a concern.
   `process::exit(0)` terminates all threads including
   any blocked `ureq::get()` call. No cancellation
   mechanism needed.

## Status

- [x] Plan approved
- [x] Implementation started
- [x] Implementation complete
