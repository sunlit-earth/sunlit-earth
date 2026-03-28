# Hypothesis Verification: Tray App Event Loop on Windows (2026-03-28)

This document records the research questions that motivated the session,
the answers found through web research, and the empirical verification
that disproved the central recommendation from the research.

**Prior art:**

- `docs/plans/2026-03-25-e2e-fix-investigation.md` — original debugging
  session, identified four platform issues, left
  `test_tray_mode_ipc_lifecycle` unresolved
- `docs/plans/2026-03-28-research-slint-tray-app-patterns.md` — web
  research on Slint tray app architecture
- `docs/plans/2026-03-28-research-invoke-from-event-loop-deadlock.md` —
  web research on the `invoke_from_event_loop` deadlock
- `docs/plans/2026-03-28-research-e2e-test-signaling.md` — web research
  on reliable e2e test signaling

---

## Research questions and answers

### Issue 1: Why does `invoke_from_event_loop` deadlock?

**Answer: Confirmed as a known winit bug.**

`invoke_from_event_loop` delegates to winit's `EventLoopProxy::send_event`,
which calls `PostMessageW` to a hidden event-target window. This message
is not processed when the winit event loop is in certain states:

- **Win32 modal loop** (window drag/resize/menu): `DefWindowProc` enters
  an internal message loop that does not dispatch custom registered
  messages. Confirmed by winit #3272 and Slint #5720.
- **`run_app_on_demand` cycling**: `run_event_loop_until_quit()` wraps
  `run_app_on_demand` in a loop. During the gap between runs, posted
  messages may not be pumped.

The timer-based command queue workaround succeeds because Slint timers
use kernel-level waitable timer objects (`SetWaitableTimer` +
`MsgWaitForMultipleObjectsEx`), which bypass the Win32 message queue
entirely.

**Implication:** `invoke_from_event_loop` cannot be used reliably from
background threads in this app. The tray module (`tray.rs`) still uses
it and needs to be converted to the command queue pattern.

**Confidence: High.** Multiple independent Slint and winit issue reports
confirm the mechanism. Source code of winit 0.30.13 was inspected.

### Issue 2: `window.run()` crash on quit (already resolved)

Already understood and fixed before this session. `window.run()` calls
`hide()` after the event loop exits, triggering `RenderingTeardown`
during an unsafe wgpu state. Fixed by using `show()` + `run_event_loop()`
instead.

### Issue 3: Timers die under `run_event_loop_until_quit()`

**Answer: Confirmed empirically (see verification below).** The research
claimed timers would keep firing under `run_event_loop_until_quit()` with
hidden windows. Testing proved this wrong on Windows.

### Issue 4: What is the correct Slint tray app architecture?

**Answer: The recommended pattern does not work on Windows.**

The Slint-recommended pattern from discussions #933, #3266, #4362 is:

1. Use `run_event_loop_until_quit()` (not `window.run()`)
2. Return `CloseRequestResponse::HideWindow` from `on_close_requested`
3. Use `window.hide()` / `window.show()` for visibility
4. Use `invoke_from_event_loop` from the tray thread

All four of these recommendations fail on Windows with the winit backend
in Slint 1.15:

1. `run_event_loop_until_quit()` stops processing timers when all
   windows are hidden (verified)
2. `HideWindow` hides the window, which triggers (1)
3. `window.hide()` kills timers (verified)
4. `invoke_from_event_loop` deadlocks (Issue 1)

**Additional findings from the research:**

- winit's `Suspended` event does NOT fire on Windows desktop (winit
  #2185). The original debugging session's hypothesis that
  `set_minimized(true)` triggers `Suspended` was a misdiagnosis.
- `SLINT_DESTROY_WINDOW_ON_HIDE` should not be used — it causes
  `BorrowMutError` crashes on the winit backend.
- Destroying and recreating the window is not viable because
  `set_rendering_notifier` can only be called once per window and wgpu
  resources are tied to the rendering notifier lifecycle.

**Confidence: High** for the negative result. No working alternative
pattern was found in the research.

### Issue 5: Is `process::exit(0)` safe?

Not researched. Low priority.

### Issue 6: Why does stderr-based test signaling fail?

**Answer: Pipe buffer congestion from the non-blocking tracing writer.**

`tracing-appender::non_blocking` routes all log writes through a
background thread. During GPU setup, 15-20 debug messages fill the 4 KB
Windows anonymous pipe buffer. The background thread blocks on `write()`
while holding the global stderr lock. Any `eprintln!` from another thread
(IPC handler, timer callback) also blocks.

**Solution:** Use stdout as a dedicated signaling channel
(`println!("SIGNAL:...")`), separate from the tracing stderr stream.
Stdout has its own pipe and lock, so there is no contention. Signal
messages are small and infrequent, so the pipe buffer never fills.

**Additional finding:** `SUNLIT_EARTH_SYNC_LOG` (synchronous stderr
writer) was implemented as a potential fix but actually makes things
*worse* — it blocks the event loop thread directly on pipe writes,
preventing timers from firing. This was the cause of additional
intermittent test failures during verification.

**Confidence: High.** Root cause confirmed by testing. The stdout signal
channel works reliably.

---

## Verification of the central hypothesis

### Hypothesis

`run_event_loop_until_quit()` + `CloseRequestResponse::HideWindow` +
`window.hide()` / `window.show()` keeps the Slint event loop alive and
timers firing when the window is hidden on Windows.

### Result

**Rejected.** Timers stop firing after the window is hidden, regardless
of the event loop function or hide mechanism used.

### Test design

A new e2e test (`test_tray_hide_show_cycle`) was written to verify the
hypothesis. The test spawns the app as a subprocess with IPC enabled,
sends commands via local socket, and observes state changes via stdout
signals (`SIGNAL:ipc_listener_ready`, `SIGNAL:window_shown`,
`SIGNAL:window_hidden`, `SIGNAL:first_frame_rendered`).

The critical assertion: after hiding the window, send `show-window`
again. If timers survived the hide, the command is processed and
`SIGNAL:window_shown` appears. If timers died, the command is never
processed and the test times out.

### Steps and observations

**Step 1 — Baseline: switch to `HideWindow` + `window.hide()`.**

Changed `on_close_requested` to return `HideWindow` instead of
`KeepWindowShown`, and the IPC `HideWindow` command to call
`win.hide()` instead of off-screen positioning.

Result: test timed out waiting for `SIGNAL:window_hidden`. The IPC
command was queued (confirmed via diagnostic `SIGNAL:command_queued`)
but the timer never processed it. The timer never fired at all — zero
`SIGNAL:tick_N` diagnostic signals in 10 seconds.

Ran 2 times, failed both. The timer sometimes fired in earlier runs
(non-deterministic), but `window.hide()` consistently killed it.

**Step 2 — Always show window before event loop.**

The app was starting with `--tray-start hidden`, meaning the window was
never shown before `run_event_loop_until_quit()`. Changed to always call
`window.show()` first, then hide via a `Timer::single_shot(ZERO, ...)`
after the event loop starts.

Result: the zero-duration timer fired (hiding the window), but no
subsequent timers fired. Same as the investigation document's earlier
finding: "Zero-duration timer to hide after loop starts — Timer fires
once, hides window, subsequent timers die."

**Step 3 — Switch to `run_event_loop()` instead of
`run_event_loop_until_quit()`.**

Since `run_event_loop_until_quit()` was unreliable, tried
`run_event_loop()` with `KeepWindowShown` (the window is never actually
closed, just hidden). Used off-screen positioning (-32000, -32000)
instead of `window.hide()`.

Result: intermittent. 3/5 passes when run solo. Timer sometimes kept
firing after the off-screen move, sometimes didn't.

**Step 4 — Try 1x1 window at (0,0) instead of off-screen.**

Replaced off-screen positioning with shrinking the window to 1x1 pixel
at position (0,0), keeping it "visible" to the DWM compositor.

Result: still intermittent, approximately same 3/5 pass rate.

**Step 5 — Isolate `SUNLIT_EARTH_SYNC_LOG` as the cause of
intermittency.**

Noticed that `SUNLIT_EARTH_SYNC_LOG=1` (synchronous stderr writer) was
set in the test. This makes every `debug!()` call from the event loop
thread block on the stderr pipe write. During rendering (which generates
many log messages), the pipe buffer fills and the event loop thread
blocks waiting for the test process to drain the pipe. While blocked,
the timer cannot fire.

Removed `SUNLIT_EARTH_SYNC_LOG` from the test. Switched all test gates
from stderr (`wait_for_log`) to stdout signals (`wait_for_signal`).

Result with 1x1 approach, no SYNC_LOG: **10/10 passes** when run solo.
Result with off-screen (-32000) approach, no SYNC_LOG: **3/5 passes**
when run solo.

**Step 6 — Cross-test interference in the full suite.**

When running all 6 e2e tests together (serial execution), the tray tests
failed even though they passed 10/10 solo. The `test_tray_mode_ipc_lifecycle`
test (which uses `--tray-start hidden`) consistently failed in the full
suite. This suggests state carryover from previous tests' child processes
(GPU resources, window manager state, or pipe handles not fully released
after `process::exit(0)`).

### Conclusions

1. **`window.hide()` kills timers on Windows** — confirmed under both
   `run_event_loop()` and `run_event_loop_until_quit()`.

2. **`run_event_loop_until_quit()` does not process timers with hidden
   windows** — the documented contract ("continues to run even when the
   last window is closed") does not hold for timer dispatch on the winit
   backend.

3. **Off-screen positioning is unreliable** — moving the window to
   (-32000, -32000) intermittently causes the DWM to stop processing
   the window, which stops timer dispatch.

4. **The 1x1 shrink approach is the most reliable workaround** found so
   far (10/10 solo), but has UX drawbacks (window still appears in
   taskbar, size/position must be saved and restored).

5. **`SUNLIT_EARTH_SYNC_LOG` is harmful for tests** — synchronous
   stderr writes block the event loop thread on pipe writes, preventing
   timers from firing. The stdout signal channel is the correct approach
   for test synchronization.

6. **Cross-test interference is a separate unresolved issue** — tray
   tests that pass solo fail in the full suite.

---

## Remaining open problems

1. **No reliable way to hide a Slint window on Windows while keeping
   timers alive.** The 1x1 shrink is the best workaround but is not a
   real hide. Filing a Slint issue with a minimal reproduction may be
   the right next step.

2. **`tray.rs` still uses `invoke_from_event_loop`**, which deadlocks.
   Needs to be converted to the command queue pattern. This was
   attempted in this session but the changes were reverted because the
   build was broken.

3. **Cross-test interference in the full e2e suite.** Tests pass solo
   but fail when run together. Likely caused by state carryover from
   killed child processes.

4. **`--tray-start hidden` is fundamentally problematic.** Starting
   with no visible window (or a 1x1 window) before the event loop
   enters dispatch can prevent timers from ever firing. Deferring the
   shrink via `Timer::single_shot` helps but is fragile.
