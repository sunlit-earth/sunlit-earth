# Investigation: E2E Test Failures on Windows (2026-03-25)

This document records the debugging session for the three failing
e2e tests on the `feat/ipc-e2e-signaling` branch. It covers
every approach tried, the observed outcomes, and the platform
issues discovered. It is intended as a foundation for the next
iteration.

## Starting state

Three e2e tests failed:

<!-- markdownlint-disable MD013 -->

| Test | Symptom |
| --- | --- |
| `test_windowed_mode_graceful_shutdown` | Crash `0xC0000409` (~50%) |
| `test_tray_mode_ipc_lifecycle` | `invoke_from_event_loop` closures never processed when window is hidden |
| `test_single_instance_second_exits` | Crash `0xC0000409` (same as #1, tray mode) |

## Current state after this session

| Test | Status | What fixed it |
| --- | --- | --- |
| `test_windowed_mode_graceful_shutdown` | PASSES | `show()` + `run_event_loop()` instead of `window.run()`, avoids `hide()` after quit |
| `test_single_instance_second_exits` | PASSES | Same as above + IPC quit via `process::exit(0)` from IPC thread |
| `test_tray_mode_ipc_lifecycle` | STILL FAILS | Blocked by multiple platform issues (see below) |

<!-- markdownlint-enable MD013 -->

## Changes made (kept)

### `src/ipc.rs` — Replaced `invoke_from_event_loop` with shared command queue

The original IPC dispatcher called `invoke_from_event_loop` for
every command (show, hide, quit). This was replaced with a
`CommandQueue` (`Arc<Mutex<VecDeque<IpcCommand>>>`) that the IPC
listener pushes to and a 50ms Slint `Timer` on the event loop
thread polls.

The `quit` command is now handled directly on the IPC listener
thread via `process::exit(0)`, bypassing both the timer and the
event loop entirely.

### `src/main.rs` — Event loop changes

- Windowed mode: `show()` + `run_event_loop()` instead of
  `window.run()` (avoids `hide()` after quit)
- Tray mode: `run_event_loop_until_quit()` (unchanged from
  original, needed to keep the loop alive when the window is
  hidden)
- IPC listener now receives a `CommandQueue` instead of a
  `Weak<MainWindow>`
- The IPC timer handle is stored alongside the listener handle
  and leaked via `std::mem::forget` before `process::exit(0)`
- `on_close_requested` returns `KeepWindowShown` + moves window
  off-screen (instead of `HideWindow`, which caused
  `run_event_loop()` to exit)

## Platform issues discovered

Four distinct issues were identified, each confirmed with
targeted experiments.

### Issue 1: `invoke_from_event_loop` deadlocks the event loop

**Discovery method:** Added diagnostic logging to the IPC
dispatcher. Confirmed `invoke_from_event_loop` returns `Ok(())`
but the closure never executes. Added a 3-second delay in the
IPC handler before calling `invoke_from_event_loop`; the event
loop ran normally during the delay (DIAG timer fired, first
frame rendered) but froze the moment `invoke_from_event_loop`
was called. Tested with a no-op closure — same deadlock.

**Scope:** Affects any `invoke_from_event_loop` call from a
background thread while the event loop is running. Calls from
the main thread before the event loop starts (e.g.,
`defer_combobox_indices`) work fine because the proxy event is
queued before the loop enters and processed synchronously on the
first iteration.

**Mechanism (hypothesis):** On Windows, winit's
`EventLoopProxy::send_event` posts a message to the internal
event loop window via `PostMessageW`. Something about processing
this message during an active event loop iteration causes the
message pump to stop dispatching further events. This may be a
winit bug or an interaction between winit's event dispatch and
the Slint rendering pipeline.

**Evidence:**

- Event loop runs normally (timers fire, frames render) until
  `invoke_from_event_loop` is called
- After the call, no timers fire, no rendering callbacks fire
- The closure is never executed
- `invoke_from_event_loop` returns `Ok(())`
- Confirmed with no-op closure (not a closure-content issue)
- The `cloud_fetcher` module uses `upgrade_in_event_loop`
  (which wraps `invoke_from_event_loop`) from a background
  thread — this likely also triggers the deadlock after its
  first poll check, but the timing is such that it fires during
  the first frame and doesn't cause visible issues

**Implication:** `invoke_from_event_loop` cannot be used from
background threads in this app. The tray module's event handlers
(`tray.rs:121,131,142`) also use it — they currently work
because they only fire on user interaction (not in automated
tests), but they would deadlock if triggered.

### Issue 2: `window.run()` → `hide()` crashes wgpu on quit

**Discovery method:** Direct observation — `window.run()` calls
`hide()` after `run_event_loop()` returns. The `hide()` triggers
`RenderingTeardown` which drops GPU resources during an unsafe
state.

**Fix applied:** Replace `window.run()` with `show()` +
`run_event_loop()` for windowed mode. Since `process::exit(0)`
follows, no explicit cleanup is needed.

**Evidence:**

- Exit code `0xC0000409` (`STATUS_STACK_BUFFER_OVERRUN`)
- Consistent with "panic during thread-local Drop" per the
  shutdown research document
- The render subcommand (which also uses `window.run()`) never
  crashes because `quit_event_loop()` is called from a timer
  callback at a clean point when the GPU is idle

### Issue 3: `run_event_loop_until_quit()` stops processing timers

**Discovery method:** Added a 200ms diagnostic `Repeated` timer
that logs every tick. With `run_event_loop_until_quit()`, the
timer fires zero times (or once, inconsistently). With
`run_event_loop()` (via `window.run()`), it fires normally.
Confirmed by running the same binary manually (timers work) vs.
from the test harness (timers stall).

**Additional observations:**

- `set_minimized(true)` before the event loop suppresses all
  timer ticks (likely triggers winit `Suspended` event)
- `window.window().hide()` before the event loop causes
  `run_event_loop_until_quit()` to re-enter
  `run_app_on_demand()` in a loop, incrementing the generation
  counter each time — timers survive the re-entry in theory but
  don't fire in practice
- A sentinel file written from the timer callback confirmed the
  timer does fire sometimes (2 out of 5 runs in full suite) but
  not reliably
- When run as the only test, the tray test passes 4/5 times;
  in the full suite, it fails consistently — suggesting a
  resource or state carryover from previous tests' child
  processes (which exit via `process::exit(0)` without GPU
  cleanup)

**Mechanism (hypothesis):** `run_event_loop_until_quit()` calls
`run_app_on_demand()` in a loop. When the window is hidden
(or minimized), `run_app_on_demand()` returns because there are
no active windows. The loop re-enters with a new generation.
During this cycling, timer state may be lost or the timer check
phase may be skipped. Additionally, the winit `Suspended` event
(triggered by minimize or hide) may disable timer polling
entirely.

### Issue 4: Tracing non-blocking writer + piped stderr drops log messages

**Discovery method:** Confirmed the timer callback runs (via
sentinel file) but the `eprintln!`/`debug!` output never reaches
the `StderrWatcher`. Tested with both `debug!` (tracing
non-blocking) and `eprintln!` (synchronous stderr).

**Mechanism:** During GPU setup, 15-20 debug log messages are
generated rapidly. The tracing `non_blocking` writer's background
thread writes to stderr (a pipe). The pipe buffer (~4KB on
Windows) fills up. The writer blocks on `write()` while holding
the global stderr lock. Any `eprintln!` call (from the timer
callback or IPC thread) blocks waiting for the stderr lock. The
`StderrWatcher` on the test side reads the pipe, eventually
draining it, but the timing can cause messages to arrive after
the test's 10-second timeout.

**Evidence:**

- `eprintln!` from the IPC thread (not the event loop) also
  fails to appear in time — the stderr lock is shared
- `debug!` messages (through tracing) are subject to the same
  pipe congestion
- The `StderrWatcher` reads continuously but can't keep up
  during the GPU setup burst

## Approaches tried and outcomes

### For `invoke_from_event_loop` deadlock

<!-- markdownlint-disable MD013 -->

| Approach | Outcome |
| --- | --- |
| Call `quit_event_loop()` directly from IPC thread | Still crashed (issue #2) |
| `invoke_from_event_loop` then `Timer::single_shot(0)` then `quit_event_loop()` | Original code; ~50% crash rate |
| Shared command queue + Slint polling timer | **Works** — avoids `invoke_from_event_loop` entirely |
| `process::exit(0)` from IPC thread for quit | **Works** — bypasses event loop and teardown |

### For wgpu teardown crash

| Approach | Outcome |
| --- | --- |
| `window.run()` (original) | Crashes in `hide()` after quit |
| `show()` + `run_event_loop()` | **Works** — skips `hide()` |
| `show()` + `run_event_loop_until_quit()` | Works for quit but has timer issues (issue #3) |

### For hidden window / timer stall

| Approach | Outcome |
| --- | --- |
| Show then `hide()` before event loop | `run_event_loop()` exits immediately; `run_event_loop_until_quit()` re-enters and timers die |
| Show then `set_minimized(true)` before event loop | Timers don't fire (winit Suspended) |
| Zero-duration timer to hide after loop starts | Timer fires once, hides window, subsequent timers die |
| Zero-duration timer to minimize after loop starts | Same — timers die after minimize |
| Move window off-screen instead of hiding | Event loop stays alive but `eprintln!` blocks (issue #4) |
| Skip hiding entirely (window stays visible) | Timer fires when run alone; inconsistent in full suite |
| `run_event_loop()` instead of `run_event_loop_until_quit()` | Event loop exits prematurely after first frame |

### For log message delivery

| Approach | Outcome |
| --- | --- |
| `debug!` (tracing non-blocking) | Messages dropped during pipe congestion |
| `eprintln!` (synchronous) | Blocks on stderr lock held by tracing writer |
| `info!` (higher level) | Same as `debug!` — same writer, same pipe |
| Log from IPC thread instead of timer | Same lock contention |

<!-- markdownlint-enable MD013 -->

## Remaining problem: `test_tray_mode_ipc_lifecycle`

This test requires:

1. App starts in tray mode with hidden window
2. IPC `show-window` makes the window visible
3. Rendering notifier fires (first frame)
4. IPC `hide-window` hides the window
5. IPC `quit` exits cleanly

The test depends on two capabilities that currently don't work
reliably on Windows:

- **Timer-based IPC command processing after the first frame:**
  The Slint polling timer fires once (processing show-window)
  but may not fire again after the rendering callback completes.
  This appears to be a `run_event_loop_until_quit()` + timer
  interaction issue in the Slint/winit backend.

- **Reliable log message delivery through piped stderr:**
  The test's `StderrWatcher` waits for specific log messages.
  The tracing non-blocking writer and the stderr pipe buffer
  cause messages to arrive late or not at all.

### Potential approaches for the next iteration

1. **Replace the tracing non-blocking writer with a blocking
   one** — eliminates issue #4 entirely, at the cost of
   blocking the event loop thread during log writes. This could
   be conditioned on an environment variable (e.g.,
   `SUNLIT_EARTH_SYNC_LOG=1`) so it only affects tests.

2. **Use `run_event_loop()` with a quit-prevention mechanism**
   — instead of `run_event_loop_until_quit()`, use
   `run_event_loop()` but prevent the window from being
   "closed" (e.g., via `KeepWindowShown`). The challenge is
   that `run_event_loop()` still exits when there are no
   visible windows, even if the window returns
   `KeepWindowShown`.

3. **Signal IPC command processing via a side channel** —
   instead of relying on stderr log messages, have the IPC
   timer write to a file or named pipe that the test reads
   independently. This decouples test signaling from the
   tracing system.

4. **Investigate the Slint timer stall** — file a minimal
   reproduction with the Slint project to determine if this
   is a known issue with `run_event_loop_until_quit()` on
   Windows. The generation counter cycling in the outer loop
   may be the root cause.

5. **Use `slint::Timer::single_shot` chaining** — instead of a
   repeated timer, have each timer callback schedule the next
   `single_shot`. This avoids the `Repeated` timer mode which
   may have different behavior across `run_app_on_demand`
   re-entries.

6. **Increase the stderr pipe buffer size** — on Windows, use
   `os_pipe` crate (or raw Win32 `CreatePipe`) to create
   pipes with larger buffers (e.g., 64KB) for the child
   process's stderr. This reduces pipe congestion during
   GPU setup logging.
