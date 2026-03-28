# Research: E2E Test Signaling — Pipe Congestion and Alternatives (2026-03-28)

## Problem Statement

The e2e tests in `tests/e2e.rs` spawn the app as a child process and use a
`StderrWatcher` to detect app readiness and state changes by watching for
specific substrings in stderr log output (e.g., `"ipc listener ready"`,
`"main window shown (from ipc)"`, `"first frame rendered"`). The current
approach has confirmed flaky failures caused by log messages arriving late or
not at all.

The root causes are documented in
[2026-03-25-e2e-fix-investigation.md](2026-03-25-e2e-fix-investigation.md):

1. `tracing-appender::non_blocking` uses a background worker thread and a
   128,000-line internal channel. When that channel is drained to stderr (a
   pipe), and the Windows anonymous pipe buffer (~4 KB) fills, the worker
   thread blocks on `write()`. The global stderr lock is held during this
   write, so synchronous `eprintln!` calls from any other thread (IPC
   listener, timers) also block waiting for the lock.

2. During GPU setup, 15–20 debug messages arrive in a burst, filling the
   4 KB pipe buffer before the test's `StderrWatcher` thread can drain it.
   The resulting backpressure causes the log-delivery delay.

3. The combined effect is that IPC state-change markers written via
   `eprintln!` on the IPC thread (`"main window shown (from ipc)"`) block
   behind a queue of tracing debug messages, sometimes arriving after the
   test's 10-second timeout expires.

This document researches reliable alternatives.

---

## Research Findings by Question

### Q1: Best Practices for E2E Subprocess Signaling

#### Electron (Node.js IPC channel)

Electron's documented test driver pattern uses Node.js's built-in IPC
channel: the parent process spawns the Electron app with `fork()` and
exchanges messages via `process.send()` / `process.on('message')`. The test
harness sends an `isReady` RPC call; the app responds `true` once
initialised. This is a **bidirectional, synchronous request/response**
channel that is entirely separate from stdout and stderr. Playwright's
Electron integration uses CDP (Chrome DevTools Protocol) over a named
socket, also separate from stdio.

**Key lesson:** mature frameworks use a dedicated out-of-band channel, not
log-scraping, for readiness and state signals. Stderr is reserved for
human-readable diagnostic output.

**Confidence:** High. Official Electron docs and Playwright API reference.

#### Tauri (WebDriver)

Tauri's official testing approach uses `tauri-driver`, a wrapper around
WebDriver's native server (WinAppDriver on Windows, WebKitWebDriver on
Linux). The test harness connects to a well-known localhost port that
WebDriver opens; app readiness is detected by polling the WebDriver
connection until it accepts. This is socket-based, not stdio-based.

**Key lesson:** even a relatively small Rust desktop framework avoids
log-scraping for test synchronisation.

**Confidence:** High. Official Tauri docs.

#### rexpect / expect-based tools

`rexpect` (a Rust port of Python's `pexpect`) reads a PTY (pseudo-terminal)
rather than a pipe, which eliminates pipe buffering. It blocks until a regex
matches in the output stream, with timeout support. However, it is
**Unix-only** — it depends on PTY allocation, which has no equivalent on
Windows. Not usable for this project.

**Confidence:** High. rexpect docs explicitly state Unix-only.

#### assert_cmd

`assert_cmd` focuses on single-shot subprocess invocation and post-exit
assertion. It has no readiness-detection or interactive signaling mechanism.
Useful for the `render` subcommand test but not for long-running window
tests.

**Confidence:** High. assert_cmd docs.

#### General consensus

The pattern used by almost every mature test framework for interactive GUI
subprocesses is: **a dedicated bidirectional channel (socket, pipe, or
platform IPC) separate from stdio, with a request/response protocol**.
Stderr is treated as a lossy diagnostic stream, never as a reliable
signaling channel.

---

### Q2: Increasing Windows Pipe Buffer Size

#### Anonymous pipes (`CreatePipe`)

`std::process::Command::stderr(Stdio::piped())` calls the Rust standard
library, which on Windows calls `CreatePipe` with `nSize = 0`. Per the
Win32 docs, `nSize = 0` means "use the system default buffer size." The
default on Windows is 4 KB (4096 bytes).

The `nSize` parameter is documented as "a suggestion; the system uses the
value to calculate an appropriate buffering mechanism." You can pass a
larger value (e.g., 65536) and Windows will allocate that much in nonpaged
pool, but you cannot do this through `std::process::Command` because Rust's
standard library provides no API to set the pipe buffer size.

#### `os_pipe` crate

`os_pipe` wraps `CreatePipe` on Windows and `pipe()` on Unix. It exposes
`PipeReader` and `PipeWriter` where `PipeWriter: Into<Stdio>`, so you can
create the pipe yourself and pass it to `Command::stderr()`. However, the
`os_pipe` docs note that "pipe buffers are 64 KiB by default on Linux" —
for Windows they still call `CreatePipe` with default parameters. There is
no API to specify `nSize`.

To get a large-buffer pipe on Windows, you would need to:

1. Call `CreateNamedPipeW` directly (via `windows-sys`) with
   `nOutBufferSize` set to e.g. 64 KB.
2. Wrap the resulting handle in an `OwnedHandle` and convert it to `Stdio`
   via unsafe `from_raw_handle`.
3. Pass it to `Command::stderr()`.

This requires `unsafe` code, adds a `windows-sys` feature, and is
platform-specific. The upside is that a 64 KB buffer would survive the GPU
setup log burst (15–20 messages × ~200 bytes each ≈ 3–4 KB, well under
64 KB). But it only addresses the pipe capacity symptom, not the stderr
lock contention root cause.

**Confidence:** High. Win32 `CreatePipe` and `CreateNamedPipe` docs
confirmed.

---

### Q3: Making `tracing-appender` Flush Synchronously

#### `NonBlockingBuilder::lossy(false)` — lossless (backpressure) mode

`tracing_appender::non_blocking::NonBlockingBuilder` exposes three
configuration methods: `buffered_lines_limit(n)`, `lossy(bool)`, and
`thread_name(s)`.

In default (lossy) mode, log lines are silently dropped when the
128,000-line internal channel is full. In non-lossy mode (`lossy(false)`),
senders block until the channel has space. This prevents drops but does not
help with the pipe-filling problem: the non-blocking writer's background
thread is still the one that writes to stderr, and it still blocks on the
full pipe. Callers on the event loop thread block on the channel, not on
the pipe directly — but the net effect is that the event loop thread
becomes unresponsive during the burst, which is worse than the current
situation.

**Key insight:** `lossy(false)` trades dropped messages for event loop
stalls. Neither is acceptable.

#### Using a synchronous (blocking) writer directly

`tracing_subscriber::fmt::Layer::with_writer()` accepts any `MakeWriter`
implementation. The standard `std::io::stderr` satisfies this without a
non-blocking wrapper:

```rust
// Synchronous, blocking, no background thread
let fmt_layer = fmt::layer()
    .with_writer(std::io::stderr)
    .with_target(true)
    .with_thread_ids(true)
    .with_span_events(FmtSpan::CLOSE);
```

With a synchronous writer, every `debug!()` call blocks the calling thread
until stderr acknowledges the write. The tracing-subscriber docs include a
`TestWriter` type specifically for use in test contexts, which also writes
synchronously.

The downside is that the event loop thread blocks on every log line during
the GPU setup burst. For production use this is unacceptable (it would
cause frame drops), but for test binaries it is fine because test
throughput matters more than frame rate.

This is the basis for a conditional logging strategy: use `non_blocking` in
production and `std::io::stderr` directly in test mode, gated on an
environment variable.

**Implementation sketch:**

```rust
fn init_logging(
    cli_level: Option<&str>,
) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let sync_log = std::env::var("SUNLIT_EARTH_SYNC_LOG").is_ok();

    let fmt_layer_base = fmt::layer()
        .with_target(true)
        .with_thread_ids(true)
        .with_span_events(FmtSpan::CLOSE);

    if sync_log {
        // Blocking writer: no dropped messages, no WorkerGuard needed
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt_layer_base.with_writer(std::io::stderr))
            .init();
        None
    } else {
        // Non-blocking writer: no event loop stalls
        let (nb, guard) = tracing_appender::non_blocking(std::io::stderr());
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt_layer_base.with_writer(nb))
            .init();
        Some(guard)
    }
}
```

The test harness sets `SUNLIT_EARTH_SYNC_LOG=1` in the child process's
environment. Production builds never set this variable.

**Confidence:** High. tracing-subscriber docs confirm
`with_writer(io::stderr)` is valid.

---

### Q4: Separate Signaling Channel Instead of Stderr

The key insight from industry practice (Electron, Tauri, Playwright) is
that **test signals should not travel through the same channel as diagnostic
logs**. Here is an evaluation of the available approaches:

#### Option A: Bidirectional IPC over the existing local socket

The project already has `interprocess` and a local socket IPC channel. The
current protocol is fire-and-forget (client sends command, server does not
respond). Making it bidirectional is straightforward: the
`interprocess::local_socket::Stream` type supports both reading and writing
on the same connection. After processing a command, the server can write a
response line (e.g., `"ok\n"` or `"shown\n"`) that the test harness reads
before proceeding.

Protocol sketch:

```text
test -> app:   "show-window\n"
app -> test:   "window-shown\n"

test -> app:   "hide-window\n"
app -> test:   "window-hidden\n"

test -> app:   "quit\n"
app -> test:   (connection closes — process exiting)
```

The test uses `send_ipc_command_and_wait(name, cmd, expected_response)` and
blocks on reading the response, with a timeout. This eliminates all
dependency on stderr for state confirmation.

**Pros:** no new dependencies, no new protocol, reuses existing
infrastructure, reliable (socket delivery is guaranteed, unlike pipe write
timing).
**Cons:** requires changes to `ipc.rs` and test helpers; the IPC thread now
needs to write back on the connection while also processing the command on
the Slint timer — requires keeping the stream open until the command is
processed.

**Implementation complexity:** Medium. The timer-based dispatch (50 ms
polling) adds latency: after writing a command, the app does not process it
until the next timer tick. The response cannot be written by the IPC thread
immediately after queueing — it must be written after the Slint timer has
actually executed the command. This requires threading the response channel
back from the timer to the IPC listener thread, which is non-trivial.

**Alternative:** process `show-window` and `hide-window` directly on the
IPC listener thread (not via the Slint timer), writing the response
synchronously. Window show/hide via `invoke_from_event_loop` is documented
to deadlock the Slint/winit event loop on Windows (confirmed in the
investigation document). So direct dispatch is not safe.

**Verdict:** Viable but adds meaningful complexity. Best combined with one
of the other quick wins below to eliminate the dependency on stderr for the
`"first frame rendered"` signal.

**Confidence:** Medium-high. interprocess docs confirm bidirectional
streams.

#### Option B: Stdout for signals, stderr for logs

Redirect application state signals to stdout while keeping tracing logs on
stderr. The app writes `"READY\n"` or `"SHOWN\n"` to stdout at key points;
the test harness reads child stdout in a separate watcher thread.

**Pros:** zero new dependencies, trivially implemented with `println!`,
completely separate from the tracing writer and its lock.
**Cons:** stdout is typically used for user-visible output; mixing signal
protocol with stdout is unconventional and confusing. The `println!` macro
goes through a different global lock (stdout lock) which is not shared with
the tracing stderr writer — so there is no lock contention. The stdout pipe
also has the same 4 KB default buffer, but signal messages are small
(< 50 bytes each) and infrequent enough that the buffer rarely fills.

Stdout is not piped by `windows_subsystem = "windows"` release builds.
However, e2e tests run against debug builds (or at minimum with
`--log-level debug`), which use the console subsystem. In test mode, stdout
is piped and available.

**Implementation complexity:** Very low. Add `println!("SIGNAL: {event}")`
at each key point. The test harness adds a `StdoutWatcher` alongside the
existing `StderrWatcher`.

**Verdict:** The simplest option with the highest reliability. Zero risk of
interference from the tracing system.

**Confidence:** High.

#### Option C: Sentinel files

The app creates a file at a known path (e.g.,
`%TEMP%\sunlit-earth-{socket-name}.ready`) when it reaches a state. The
test polls for the file's existence with a timeout.

**Pros:** completely decoupled from all I/O channels; no locks, no pipes,
no sockets.
**Cons:** filesystem polling has 50–100 ms granularity (typically);
requires a cleanup step; file creation may fail if the temp directory is
full or the process does not have write permission; file paths must be
communicated to the test (e.g., via the socket name prefix).

**Verdict:** Useful as a fallback, especially for "app crashed before
writing the file" detection. Not recommended as a primary mechanism due to
polling latency and cleanup burden.

**Confidence:** High.

#### Option D: Windows Named Events (`CreateEventW` / `SetEvent`)

Create a named Windows event object (e.g.,
`Global\SunlitEarth-{name}-ready`) that the app signals with `SetEvent`
and the test waits on with `WaitForSingleObject(handle, timeout_ms)`.

**Pros:** precise, low-latency signaling; no pipes, no files;
`WaitForSingleObject` can be used with an exact millisecond timeout; the OS
cleans up handles when the process exits.
**Cons:** Windows-only (breaks cross-platform compatibility for tests);
requires `unsafe` Win32 FFI; requires `windows-sys` feature additions; adds
cognitive complexity; the event name must be unique per test run.

**Verdict:** Powerful but disproportionately complex for this use case. The
cross-platform cost is significant.

**Confidence:** High (well-documented Win32 API).

#### Option E: Windows `OutputDebugString` / ETW

`OutputDebugString` writes to the Windows debug output channel, which can
be read by another process using `WaitForDebugEvent` or a dedicated monitor
tool like DebugView. ETW (Event Tracing for Windows) is the
production-grade version of this, used by the Windows engineering system.

**Pros:** does not go through stdio at all; no pipe buffer issues.
**Cons:** reading `OutputDebugString` from a test process requires either a
special debugger attachment or a shared memory segment (`DBWIN_BUFFER`),
which is complex to implement correctly. ETW requires a provider manifest,
GUIDs, and a session controller — massive overhead for a test harness. Both
are Windows-only.

**Verdict:** Not recommended. Extreme complexity for minimal benefit over
the stdout approach.

**Confidence:** High.

#### Option F: Shared memory

Create a named file mapping (`CreateFileMappingW` with
`INVALID_HANDLE_VALUE`) containing a status byte. The app sets the byte;
the test polls it.

**Pros:** zero pipe involvement; essentially instantaneous.
**Cons:** Windows-only; requires `unsafe` FFI; polling granularity; complex
cleanup; overkill for a handful of state signals.

**Verdict:** Not recommended.

**Confidence:** High.

---

### Q5: How Other Rust Projects Handle E2E Subprocess Communication

#### Alacritty

Alacritty's IPC implementation uses Unix domain sockets with a
JSON-over-newlines protocol. It is Unix-only (no Windows named pipe
equivalent). The approach is unidirectional: external clients send
commands, the app does not send responses. Readiness is not signaled —
callers must retry connection until it succeeds. This matches the current
Sunlit Earth IPC design.

**Lesson:** even a mature terminal emulator uses retry-until-connect rather
than an explicit readiness signal. However, for GUI apps with lengthy
startup (GPU init, texture loading), a retry loop can take 10–30 seconds.

**Confidence:** High. Alacritty source code was available for review in
earlier research sessions.

#### `assert_cmd` / `duct`

`assert_cmd` is for single-shot processes only. `duct` provides process
pipeline composition and can pipe stdout/stderr to arbitrary `Read` streams,
but has no readiness-signaling abstraction. Both libraries treat
subprocesses as black boxes.

**Confidence:** High.

#### Cargo itself (for test binaries)

`cargo test` captures subprocess output in-memory and reports it only on
failure. It uses `std::process::Command` with `Stdio::piped()` — the same
default 4 KB pipe. For short-lived test processes this is fine. For
long-lived GUI subprocesses, Cargo's model does not apply.

---

### Q6: Making IPC Bidirectional with `interprocess`

The `interprocess::local_socket::Stream` type implements `Read + Write`
(via the `RecvHalf`/`SendHalf` split or directly). After reading a command
line from the client, the server can write a response line on the same
stream before the client disconnects.

The current fire-and-forget protocol (client connects, sends one line,
disconnects) would need to change to:

1. Client connects.
2. Client sends command (`"show-window\n"`).
3. Server reads command, queues it.
4. Server waits for the Slint timer to execute the command (up to 50 ms).
5. Server writes response (`"window-shown\n"`).
6. Client reads response and verifies it.
7. Both sides disconnect.

The blocking issue is step 4: the IPC listener thread cannot know when the
Slint timer has executed the command unless there is a feedback mechanism.
Options:

- **Condition variable:** The timer callback signals a `Condvar` after
  execution. The IPC listener thread waits on it (with timeout) before
  writing the response. This is correct but requires thread coordination.
- **Direct execution on IPC thread (unsafe):** Execute window show/hide
  directly from the IPC listener thread using `invoke_from_event_loop`.
  This deadlocks on Windows (confirmed in the investigation document).
- **Timeout + assume success:** The IPC thread waits a fixed 100 ms after
  queueing the command and then writes `"ok\n"`. Simple but not a true
  confirmation.

The Condvar approach is the correct one. It adds ~20 lines of code to
`ipc.rs` but provides genuine command acknowledgement.

**Implementation complexity:** Medium. No new dependencies. `interprocess`
already supports bidirectional streams.

**Confidence:** High. interprocess docs confirm bidirectional stream
support.

---

### Q7: Windows-Specific Solutions

#### `OutputDebugString`

As covered above: reading it from a separate process requires attaching as
a debugger or polling the `DBWIN_BUFFER` shared memory segment. Not
practical.

#### ETW

Production-grade event tracing. Requires GUID-based provider registration,
a session controller, and a consumer. Overkill.

#### Named events (`CreateEventW`)

The cleanest Windows-specific option. Low latency, OS-managed cleanup. But
Windows-only, requires unsafe FFI. Better options exist that also work on
non-Windows.

#### Named pipes with explicit buffer size (`CreateNamedPipeW`)

As covered in Q2: you can get a 64 KB buffer by calling `CreateNamedPipeW`
directly and passing the handle to `Command::stderr()`. This fixes the pipe
capacity symptom but not the stderr lock contention root cause.

---

## Confidence Ratings Summary

<!-- markdownlint-disable MD013 -->

| Finding | Confidence |
| --- | --- |
| `Stdio::piped()` creates a 4 KB pipe on Windows | High — Win32 `CreatePipe` docs |
| `os_pipe` does not expose buffer size control on Windows | High — os_pipe docs |
| `NonBlockingBuilder::lossy(false)` blocks senders, not pipe writes | High — tracing-appender source |
| `fmt::layer().with_writer(io::stderr)` is a valid synchronous writer | High — tracing-subscriber docs |
| `interprocess::Stream` supports bidirectional I/O | High — interprocess docs |
| `rexpect` is Unix-only (PTY dependency) | High — rexpect docs |
| Electron uses Node.js IPC channel, not stdio, for test readiness | High — official Electron docs |
| Tauri uses WebDriver socket, not stdio, for test readiness | High — official Tauri docs |
| Windows named events are usable but Windows-only | High — Win32 docs |
| ETW is overkill for test signaling | High — ETW docs |
| `SUNLIT_EARTH_SYNC_LOG` env-var approach is implementable | High — tracing-subscriber confirmed |
| Condvar + IPC bidirectionality is right for response signals | Medium-high — design reasoning |

<!-- markdownlint-enable MD013 -->

---

## Recommended Approach

The root problem is using a lossy, contended channel (the tracing stderr
pipe) as a reliable signaling mechanism. The fix has two parts, which can
be implemented independently.

### Part 1: Switch to a synchronous writer in test mode (immediate fix)

Add an environment variable `SUNLIT_EARTH_SYNC_LOG=1` that causes
`init_logging()` to use `std::io::stderr` directly instead of
`tracing_appender::non_blocking`. The test harness sets this variable in
the child process's environment via
`Command::env("SUNLIT_EARTH_SYNC_LOG", "1")`.

Effects:

- No background writer thread, no internal channel, no pipe-write race.
- Every `debug!()`, `info!()`, etc. call blocks the calling thread until
  stderr acknowledges the write.
- The stderr lock is still a single mutex, but contention is now bounded
  by the frequency of log calls rather than burst behaviour of the
  nonblocking worker thread.
- During the GPU setup burst, the event loop thread will block briefly on
  each log write. This is acceptable in tests because: (a) tests do not
  care about frame rate, and (b) the pipe buffer is drained continuously
  by the `StderrWatcher` thread on the test side.
- The `WorkerGuard` is no longer needed and can be omitted (or kept as
  `Option<WorkerGuard>` for type safety).

This change is entirely contained in `init_logging()` in `main.rs`. It
does not affect the IPC protocol, the test structure, or the app's
behaviour in production.

**Risk:** On heavily loaded CI machines, the synchronous stderr writes
during the GPU setup burst may still cause 100–200 ms delays. This is
acceptable given test timeouts of 10–30 seconds. If it is still flaky in
practice, the next step is Part 2.

### Part 2: Add a stdout signaling channel (medium-term improvement)

For the specific signals that tests depend on most critically — "IPC
listener ready", "first frame rendered", "window shown", "window hidden" —
add a companion `println!("SIGNAL:{marker}")` alongside the existing
`debug!()` or `info!()` call. The test harness adds a `StdoutWatcher`
(identical in structure to the existing `StderrWatcher`) and uses it
instead of the `StderrWatcher` for readiness gates.

Key properties:

- stdout has a separate pipe and a separate lock from stderr.
- Signal messages are small (<50 bytes) and infrequent, so the 4 KB
  stdout pipe buffer is never at risk of filling.
- No lock contention: the stdout lock is not shared with the tracing
  stderr writer.
- Zero new dependencies.
- Trivially cross-platform.

The signal format should be distinct from normal log output:

```text
SIGNAL:ipc_listener_ready
SIGNAL:first_frame_rendered
SIGNAL:window_shown
SIGNAL:window_hidden
SIGNAL:exiting
```

The test harness waits on
`StdoutWatcher::wait_for_signal("ipc_listener_ready", timeout)` instead of
`StderrWatcher::wait_for_log("ipc listener ready", timeout)`. The
`StderrWatcher` is retained for the `"no ERROR lines"` assertion.

Signal emission is conditioned on the IPC socket being active (since
signals are only meaningful in test mode):

```rust
if ipc_active {
    println!("SIGNAL:ipc_listener_ready");
}
```

This avoids polluting stdout in normal user sessions.

### Part 3: Bidirectional IPC for command acknowledgement (optional)

If Part 1 and Part 2 together do not fully eliminate flakiness, the
remaining source is the 50 ms timer polling latency for `show-window` and
`hide-window` commands. The test currently infers that a command was
processed by watching for a log message; with the stdout signal channel
from Part 2, it reads a `SIGNAL:window_shown` on stdout instead.

The timer writes the stdout signal after processing the command:

```rust
IpcCommand::ShowWindow => {
    if let Some(win) = window_weak.upgrade() {
        win.window().set_minimized(false);
        win.show().ok();
    }
    if ipc_active {
        println!("SIGNAL:window_shown");
    }
}
```

This does not require bidirectional IPC — the signal flows over stdout, not
over the socket. True IPC bidirectionality (the socket response pattern
from Q6) remains optional and would only be needed if the test must confirm
command receipt before the Slint timer has processed it (i.e., within the
same 50 ms window).

---

## Quick Wins (Low Effort, High Impact)

The following improvements can be made immediately with minimal risk:

### QW1: Set `SUNLIT_EARTH_SYNC_LOG=1` in the test harness

**Effort:** 1–2 lines in `tests/e2e.rs`.
**Impact:** Eliminates the pipe congestion root cause for all stderr-based
signals. The `StderrWatcher` becomes reliable because log messages are
written synchronously.

```rust
Command::new(BINARY)
    .env("SUNLIT_EARTH_SYNC_LOG", "1")  // synchronous tracing writer
    .args([...])
```

This also requires `init_logging()` in `main.rs` to support the env var.

### QW2: Increase stderr pipe read frequency in `StderrWatcher`

**Effort:** 0 code changes — `BufReader::lines()` already blocks until a
newline, so the watcher reads as fast as the pipe delivers lines.

Current `StderrWatcher` is already optimal for reading. The problem is not
the reader speed but the writer latency. No improvement here without fixing
the writer side.

### QW3: Add `buffered_lines_limit` to the non-blocking writer

```rust
let (non_blocking, guard) = tracing_appender::NonBlockingBuilder::default()
    .buffered_lines_limit(4096)
    .lossy(true)
    .finish(std::io::stderr());
```

This increases the internal channel from 128,000 lines (overkill) to 4096
lines (still far more than the GPU setup burst). It does not help with the
pipe buffer or lock contention. **No practical benefit for this problem.**

### QW4: Write state-change markers on a separate channel immediately

As described in Part 2: emit a `println!("SIGNAL:...")` at each key state
transition. This can be done before implementing the full stdout signaling
infrastructure — even a single `println!("SIGNAL:ipc_listener_ready")` in
`ipc.rs` and waiting for that instead of the debug log would fix the most
flaky `wait_for_log("ipc listener ready", ...)` call.

**Effort:** 2–3 lines of code.
**Impact:** The "ipc listener ready" wait (the first gate in two of the
three failing tests) becomes unconditionally reliable.

---

## Approaches Not Recommended

<!-- markdownlint-disable MD013 -->

| Approach | Why Not |
| --- | --- |
| Increase Windows pipe buffer via `CreateNamedPipeW` | Requires unsafe FFI; fixes symptoms, not root cause; Windows-only |
| `NonBlockingBuilder::lossy(false)` | Trades dropped messages for event loop stalls; worse |
| `rexpect` | Unix-only (PTY) |
| Windows named events | Windows-only, unsafe FFI, overkill |
| ETW | Massive complexity, Windows-only |
| Shared memory | Windows-only, unsafe FFI, overkill |
| Polling sentinel files | Slow (50–100 ms granularity), cleanup burden |

<!-- markdownlint-enable MD013 -->

---

## Implementation Priority

<!-- markdownlint-disable MD013 -->

| Step | Description | Effort | Impact |
| --- | --- | --- | --- |
| 1 | Support `SUNLIT_EARTH_SYNC_LOG` env var in `init_logging()` | ~20 lines | Eliminates pipe congestion |
| 2 | Set `SUNLIT_EARTH_SYNC_LOG=1` in test harness `Command::env` | ~1 line per test | Tests use synchronous stderr |
| 3 | Add `println!("SIGNAL:...")` for critical state transitions | ~5 lines | Decouples readiness from tracing |
| 4 | Add `StdoutWatcher` to test harness | ~30 lines | Tests no longer depend on stderr for gates |
| 5 | Switch `wait_for_log` calls to `wait_for_signal` in tests | ~10 lines | Tests use the reliable channel |
| 6 | (Optional) Bidirectional IPC with Condvar acknowledgement | ~60 lines | Confirms command execution |

<!-- markdownlint-enable MD013 -->

Steps 1–5 form a complete, self-consistent fix. Step 6 is optional and
adds true request/response semantics to the IPC protocol if step 5 proves
insufficient.

---

## Sources

<!-- markdownlint-disable MD013 -->

| Source | URL / Location | Notes |
| --- | --- | --- |
| `tracing-appender` `NonBlockingBuilder` docs | docs.rs/tracing-appender | Confirmed `lossy(false)`, `buffered_lines_limit` |
| `tracing-appender` source | github.com/tokio-rs/tracing | Confirmed default 128,000-line capacity |
| `tracing-subscriber` `fmt::Layer` docs | docs.rs/tracing-subscriber | Confirmed `with_writer(io::stderr)` is valid |
| Win32 `CreatePipe` docs | learn.microsoft.com | Confirmed `nSize=0` → 4 KB default |
| Win32 `CreateNamedPipeW` docs | learn.microsoft.com | Confirmed advisory buffer size hint |
| `os_pipe` crate docs | docs.rs/os_pipe | Confirmed no Windows buffer size control |
| `interprocess` local_socket docs | docs.rs/interprocess | Confirmed bidirectional Stream support |
| `rexpect` docs | docs.rs/rexpect | Confirmed Unix-only |
| `assert_cmd` docs | docs.rs/assert_cmd | Confirmed no readiness signaling |
| Electron automated testing docs | electronjs.org/docs | Confirmed Node.js IPC channel for readiness |
| Tauri WebDriver docs | tauri.app | Confirmed socket-based readiness, not stdio |
| Win32 event object docs | learn.microsoft.com | Confirmed `CreateEventW` / `SetEvent` pattern |
| Win32 ETW docs | learn.microsoft.com | Confirmed complexity/overhead |
| Investigation document | docs/plans/2026-03-25-e2e-fix-investigation.md | Root cause analysis baseline |

<!-- markdownlint-enable MD013 -->
