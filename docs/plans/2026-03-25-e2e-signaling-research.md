# Research: E2E Test Signaling (2026-03-25)

## Problem Statement

The existing e2e tests for Sunlit Earth's tray and windowed
modes use a crude sleep-then-kill approach: spawn the binary,
sleep 3 seconds, kill the process. This cannot verify that
rendering actually completed, that the window can be
shown/hidden programmatically, or that shutdown is graceful.
A signaling mechanism is needed so that an external test
harness can:

1. Detect when the app is ready (first frame rendered,
   textures loaded).
2. Control window visibility (show/hide from tray).
3. Trigger graceful shutdown (flush state, drop GPU
   resources, exit code 0).
4. Run parallel test instances without interference.

## Requirements

- **Windows primary**: The project targets Windows first;
  Linux/macOS are secondary.
- **Minimal complexity**: The IPC mechanism must be simple
  enough to justify its maintenance burden in an early-stage
  project.
- **Test isolation**: Parallel test runs must not interfere
  with each other.
- **No Tokio dependency**: The app does not use Tokio; the
  IPC mechanism should work with synchronous I/O on a
  background thread.
- **Graceful shutdown**: The `quit` command must cause a
  clean exit (GPU teardown, config flush, exit code 0), not
  a process kill.
- **Readiness signaling**: Tests must be able to detect when
  the app is ready to accept commands.

## Findings

### Current E2E Test Infrastructure

The existing e2e tests in `tests/e2e.rs` spawn the compiled
binary as a child process. All tests are `#[ignore]` (require
desktop + GPU) and `#[serial]`. Key infrastructure includes:

- `wait_with_timeout(child, timeout)` polls
  `child.try_wait()` every 200ms.
- `spawn_run_and_kill(args, run_duration)` spawns the binary,
  sleeps, then kills.
- `parse_memory_entries(stderr)` extracts structured memory
  log lines.
- Pixel assertion helpers validate geographic correctness of
  rendered output.

The **render mode** test (`test_render_and_exit`) is
comprehensive: it validates exit code, output dimensions,
pixel correctness, absence of errors, and memory profiling.
It works well because render mode has a natural completion
signal (`textures_ready` AtomicBool triggers
`quit_event_loop()`).

The **tray and windowed mode** tests are minimal: they only
verify that the process stays alive for 3 seconds and that
the startup banner appears. They cannot verify rendering
completion, window show/hide behavior, or graceful shutdown.

### Existing Signaling via Log Messages

The application already emits structured log messages that
serve as implicit signals:

| Message                       | Level | Meaning                  |
| ----------------------------- | ----- | ------------------------ |
| `"first frame rendered"`      | info  | First GPU frame done     |
| `"textures ready"`            | debug | All textures loaded      |
| `"main window hidden ..."`    | debug | Window hidden (tray)     |
| `"main window shown ..."`     | debug | Window re-shown          |
| `"exiting"`                   | debug | About to exit            |

For read-only observation (detecting readiness, confirming
state changes), streaming stderr line-by-line and reacting to
these messages is sufficient. The `"first frame rendered"`
info-level message is the natural readiness signal for
tray/windowed mode tests.

However, log-based signaling is one-directional (app to test)
and cannot send commands to the app.

### IPC Mechanism Selection

Eight approaches were evaluated:

| Approach          | Windows    | Linux/macOS | Complexity |
| ----------------- | ---------- | ----------- | ---------- |
| Named pipe/socket | Named pipe | Unix socket | Low        |
| Signals/Win evts  | Ctrl+C     | SIGTERM     | Medium     |
| TCP localhost     | Port file  | Port file   | Medium     |
| File-based        | Yes        | Yes         | Low        |
| stdin/stdout      | Parent     | Parent      | Low        |
| D-Bus / COM       | COM        | D-Bus       | Very high  |
| Named semaphores  | Yes        | Yes         | Medium     |
| Memory-mapped     | Yes        | Yes         | High       |

| Approach          | Reliability | Security | Latency  |
| ----------------- | ----------- | -------- | -------- |
| Named pipe/socket | High        | Medium   | <1ms     |
| Signals/Win evts  | Medium      | High     | Variable |
| TCP localhost     | High        | Low      | <1ms     |
| File-based        | Low (races) | High     | 10-100ms |
| stdin/stdout      | High        | High     | <1ms     |
| D-Bus / COM       | High        | High     | <1ms     |
| Named semaphores  | High        | High     | <1ms     |
| Memory-mapped     | High        | Medium   | <1ms     |

**Named pipes / local sockets** emerged as the clear winner.
TCP adds unnecessary port-file coordination. File-based
signals have race conditions. stdin/stdout cannot address the
single-instance use case (second instance communicating with
the first). D-Bus/COM and memory-mapped files are
over-engineered for this purpose. Unix signals do not exist
on Windows.

### Recommended IPC: `interprocess` Crate

The `interprocess` crate (v2.4.0, actively maintained, 967
commits) provides a cross-platform `local_socket` API that
abstracts named pipes on Windows and Unix domain sockets on
Linux/macOS. It supports both synchronous and async (Tokio)
I/O.

**Socket naming:** Use `GenericNamespaced` to avoid
filesystem cleanup issues:

- Windows: `\\.\pipe\sunlit-earth-ipc`
- Linux/macOS: abstract namespace `@sunlit-earth-ipc`

```rust
let name = "sunlit-earth-ipc"
    .to_ns_name::<GenericNamespaced>()
    .unwrap();
```

**Protocol:** Newline-delimited plain text commands, one per
connection:

- `quit` -- begin graceful shutdown
- `show-window` -- show and focus the main window
- `hide-window` -- hide the main window to tray

**Thread model:** The IPC listener runs on a dedicated
background thread (no Tokio required). Commands are forwarded
to the Slint event loop via `mpsc` channel or
`Arc<AtomicBool>`, matching the existing pattern used by the
cloud fetcher and tray threads.

### Real-World Precedents

**Alacritty** uses Unix domain sockets with newline-delimited
JSON messages. The IPC module lives in
`alacritty/src/ipc.rs` and spawns a background listener
thread. Commands: `CreateWindow`, `Config`, `GetConfig`.
Notably, Alacritty's IPC is Unix-only with no Windows named
pipe equivalent, which is why Sunlit Earth should use the
`interprocess` abstraction.

**Visual Studio Code** uses named pipes for IPC between the
renderer and extension host processes. The pipe name is passed
via CLI argument -- the same `--ipc-socket <name>` pattern
recommended here.

**Zed** uses a headless mode (`zed --headless`) for
programmatic control, a startup flag approach rather than
runtime IPC.

### CLI Extensions

The existing clap CLI should be extended with:

- `--ipc-socket <name>` -- override the default socket name
  (essential for parallel test isolation; each test passes a
  unique name like `sunlit-earth-test-{uuid}`)
- `--no-window` -- start in tray mode without showing the
  window (for tests that need the app running but hidden)

### Security Considerations

Named pipes on Windows default to allowing `Everyone` to
read, which means any process on the machine can connect. For
e2e testing this is acceptable. For production use, explicit
ACL configuration via `SECURITY_ATTRIBUTES` would be
required. Unix domain sockets rely on filesystem permissions,
which are more predictable.

Since IPC is gated behind a CLI flag (`--ipc-socket`), it is
not active in normal usage. Only test runs and explicitly
opted-in sessions expose the control channel.

### Startup and Shutdown Sequencing

**Readiness detection:** The test should use a retry loop
with backoff (10ms sleep, 5s timeout) to repeatedly attempt
socket connection until success. This is more robust than
stdout-based readiness messages or file polling.

**Graceful shutdown sequence** when `quit` is received:

1. Stop the cloud fetcher thread (drop channel sender)
2. Drop GPU resources (via `RenderingTeardown`)
3. Exit the Slint event loop via `slint::quit_event_loop()`
4. Exit the process with code 0

The existing `process::exit(0)` at the end of `main()`
ensures the exit code is always 0 on success.

**Single-instance test scenario:**

1. Start instance A with `--ipc-socket sunlit-earth-test-a`.
2. Start instance B (detects A via OS mutex, exits with
   `"another instance is already running"`).
3. Send `show-window` to A's socket and observe the window
   becomes visible.
4. Send `quit` to A's socket for graceful shutdown.

## External Research

All external findings are from the codebase research agent's
web investigation. Source citations and confidence levels:

| Finding                              | Confidence | Notes        |
| ------------------------------------ | ---------- | ------------ |
| `interprocess` v2.4.0 is best        | High       | crates.io    |
| Local socket = pipe/Unix socket      | High       | Documented   |
| Alacritty uses Unix domain sockets   | High       | Source code  |
| Win named pipes have ACL risk        | High       | Well-known   |
| `single-instance` has no messages    | High       | Docs confirm |
| Zed uses headless mode, not IPC      | Medium     | News sources |

## Technical Constraints

- **No Tokio**: The app is synchronous with Slint's event
  loop. The IPC listener must use blocking I/O on a
  background thread.
- **Thread-local GPU resources**: GPU resources live in
  `thread_local! { RefCell<Option<GpuResources>> }` and can
  only be accessed from the rendering callback thread. The
  `quit` command must go through `slint::quit_event_loop()`,
  not attempt direct resource cleanup.
- **`process::exit(0)` termination**: The app always exits
  via `process::exit(0)` to avoid thread-local destruction
  ordering panics. IPC cleanup must not depend on normal drop
  sequencing.
- **Compile-time log gates**: Release builds gate at
  `release_max_level_warn`, so `"textures ready"` (debug
  level) is unavailable in release. The
  `"first frame rendered"` (info level) is available in all
  builds.
- **Single-instance mutex**: The `single-instance` crate's
  OS mutex is separate from IPC. It only detects existing
  instances; it cannot pass messages. IPC and single-instance
  are complementary, not overlapping.

## Open Questions

1. **Should IPC be always-on or opt-in?** The current
   recommendation is opt-in via `--ipc-socket`. An
   alternative is always-on with a well-known name, which
   would allow external tools to control the app without
   special flags. The trade-off is attack surface vs.
   convenience.

2. **Should the protocol support responses?** The current
   design is fire-and-forget (send command, disconnect).
   Adding response messages (e.g., `"ok\n"` or
   `"error: ...\n"`) would enable the test to confirm
   command receipt, but adds protocol complexity.

3. **Should `show-window` be testable without a real
   display?** The existing e2e tests require a desktop with
   GPU. If CI runners lack a display, `show-window` cannot
   be verified visually. The test may need to settle for
   confirming the log message appears.

4. **Cloud fetcher interaction**: When `quit` is received,
   the cloud fetcher thread (which may be blocked on an HTTP
   request) needs to be interrupted. The current `mpsc`
   channel approach drops the sender, but the fetcher thread
   may be blocked in `ureq::get()`. Does this need a timeout
   or cancellation mechanism?

## Recommendations

1. **Add `interprocess` as a dependency** and implement a
   local socket IPC listener on a background thread, gated
   behind the `--ipc-socket <name>` CLI flag.

2. **Implement three commands**: `quit`, `show-window`,
   `hide-window` with newline-delimited plain text protocol.

3. **Add `--no-window` CLI flag** for starting in tray mode
   without an initial window show.

4. **Upgrade the tray/windowed e2e tests** to:
   - Pass `--ipc-socket` with a UUID-based name for
     isolation.
   - Stream stderr to detect `"first frame rendered"`
     instead of sleeping.
   - Send `quit` via the socket for graceful shutdown
     instead of killing.
   - Validate exit code 0 and absence of errors.

5. **Add a new e2e test for window show/hide** that starts
   the app with `--no-window`, sends `show-window` via
   socket, confirms the log message, sends `hide-window`,
   confirms, then sends `quit`.

6. **Keep SIGTERM/Ctrl+C handling** as a supplementary
   shutdown path for CI timeout scenarios, but do not make
   it the primary test mechanism.

## Sources

| Document                             | Researcher      | Focus Area        |
| ------------------------------------ | --------------- | ----------------- |
| `2026-03-25-e2e-signaling-codebase`  | Codebase agent  | Tray, CLI, logs   |
| `2026-03-25-e2e-signaling-external`  | External agent  | IPC, crates, apps |
