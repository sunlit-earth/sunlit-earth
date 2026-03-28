# Minimal vs Sunlit Earth: Architectural Comparison

This document compares the minimal Slint + tray-icon + wgpu project
(`slint-tray-minimal`) against Sunlit Earth to identify which
architectural differences cause the tray/IPC/timer failures in Sunlit
Earth.

Reference codebases:

- Minimal (working): `C:\Workspace\rustrover\slint-tray-minimal\`
- Sunlit Earth (failing): `C:\Workspace\rustrover\sunlit-earth\`

Prior art:

- `docs/plans/2026-03-28-hypothesis-verification.md` -- records the
  empirical failures and workarounds attempted in Sunlit Earth

---

## Section 1: Side-by-side comparison

Each entry below describes one architecturally significant difference
between the two projects. "Could explain failures" is assessed based
on the known failure modes documented in the hypothesis verification
document.

### 1. Event loop function

- Minimal: `slint::run_event_loop_until_quit()`
- SE: `slint::run_event_loop()`
- Could explain failures: YES. SE switched away from
  `run_event_loop_until_quit()` because timers died after hide. But
  the minimal project proves it works. The root cause is elsewhere.

### 2. wgpu configuration

- Minimal: `WGPUConfiguration::Automatic(Default::default())`
- SE: `WGPUConfiguration::Manual { instance, adapter, device, queue }`
- Could explain failures: YES (high suspicion). SE creates the wgpu
  instance, adapter, and device before selecting the Slint backend,
  then passes ownership via `Manual`. The minimal project lets Slint
  create the wgpu device internally via `Automatic`. This is the
  single largest architectural difference. Manual wgpu init may
  interact with Slint's internal winit event loop setup in ways that
  cause the observed deadlocks and timer failures.

### 3. Close request response

- Minimal: `CloseRequestResponse::HideWindow`
- SE: `CloseRequestResponse::KeepWindowShown`
- Could explain failures: YES. SE avoids `HideWindow` because
  `window.hide()` kills timers. But the minimal project uses
  `HideWindow` successfully. This suggests the timer death is not
  inherent to `HideWindow` -- it is caused by something else in
  SE's setup (see #2).

### 4. Hide mechanism

- Minimal: `window.hide()` (Slint-native)
- SE: Shrink to 1x1 pixel at (0,0), then `set_position`/`set_size`
  to restore
- Could explain failures: YES. SE's workaround avoids `window.hide()`
  entirely, using off-screen positioning or 1x1 shrink. The minimal
  project proves `window.hide()` works. The difference must be caused
  by something in SE's environment (see #2).

### 5. Cross-thread IPC dispatch

- Minimal: `slint::invoke_from_event_loop` called directly from the
  IPC thread
- SE: `CommandQueue` (shared `Arc<Mutex<VecDeque>>`) polled by a 50ms
  Slint timer
- Could explain failures: YES. SE abandoned `invoke_from_event_loop`
  because it deadlocks. The minimal project uses it successfully.
  This deadlock may be specific to the `Manual` wgpu configuration
  (see #2) or to SE's heavier rendering workload.

### 6. Cross-thread tray dispatch

- Minimal: `slint::invoke_from_event_loop` from tray menu/click
  handlers
- SE: `slint::invoke_from_event_loop` from tray menu/click handlers
  (same pattern)
- Could explain failures: NO difference in pattern. SE's `tray.rs`
  uses the same `invoke_from_event_loop` pattern as the minimal
  project. However, the hypothesis doc notes SE's tray still uses
  `invoke_from_event_loop` and "needs to be converted to the command
  queue pattern." This means the tray's `invoke_from_event_loop`
  calls may deadlock in SE but work in the minimal project.

### 7. Quit mechanism

- Minimal: Direct `slint::quit_event_loop()` inside
  `invoke_from_event_loop` closure
- SE: `std::process::exit(0)` called directly from IPC listener
  thread (bypasses Slint entirely)
- Could explain failures: YES. SE's `quit` command does
  `process::exit(0)` from the IPC thread without going through the
  Slint event loop at all. This is a workaround for the
  `invoke_from_event_loop` deadlock. The minimal project's direct
  `quit_event_loop()` works cleanly.

### 8. wgpu device creation timing

- Minimal: After `BackendSelector::select()`, implicitly during the
  `set_rendering_notifier` RenderingSetup callback
- SE: Before `BackendSelector::select()`, in `wgpu_init::init()`.
  Adapter enumeration via `pollster::block_on`, device request via
  `pollster::block_on`.
- Could explain failures: YES (high suspicion). SE blocks on async
  wgpu operations (`enumerate_adapters`, `request_device`) before the
  Slint backend is initialized. The `pollster::block_on` calls may
  interfere with or pre-empt the winit event loop's own async
  infrastructure. The minimal project never touches wgpu directly --
  Slint handles all device creation internally.

### 9. Adapter selection

- Minimal: Slint default (Automatic)
- SE: Manual: enumerate all adapters, rank by type, optionally filter
  by env var or `--software-rendering`
- Could explain failures: INDIRECTLY. The manual adapter selection
  itself is fine, but it means SE passes a pre-created device to
  Slint. If there is any mismatch between what SE's device supports
  and what Slint's winit backend expects, it could cause subtle event
  loop issues.

### 10. Rendering complexity

- Minimal: Single `clear` render pass (solid color), one
  `Image::try_from(Texture)`
- SE: Full pipeline: vertex/fragment shaders, multiple render passes
  (Earth, 3 atmosphere shells, clouds), MSAA resolve, dirty-checking,
  dynamic texture loading, mipmap generation
- Could explain failures: POSSIBLY. The heavier GPU workload could
  cause the event loop to stall during long render frames, which
  might interact with the timer/`invoke_from_event_loop` mechanisms.
  However, this alone should not kill timers permanently.

### 11. Texture channel / background loading

- Minimal: None
- SE: `mpsc::channel<DecodedTextureMessage>` for texture loading;
  cloud fetcher uses `upgrade_in_event_loop` from a background thread
- Could explain failures: YES. The cloud fetcher calls
  `ww.upgrade_in_event_loop(...)` from a background thread. This is
  functionally equivalent to `invoke_from_event_loop` (it dispatches
  to the Slint event loop). If `invoke_from_event_loop` deadlocks in
  SE, so does `upgrade_in_event_loop`. SE disables clouds in tests
  via `SUNLIT_EARTH_NO_CLOUDS` precisely to avoid this.

### 12. Timer intervals

- Minimal: 1 second (tick timer)
- SE: 120 seconds (sun timer), 50ms (IPC command queue timer), 200ms
  (render poll timer)
- Could explain failures: UNLIKELY. Timer interval length should not
  affect whether timers survive `window.hide()`.

### 13. Timer ownership

- Minimal: `_keep_timer` binding holds the timer alive
- SE: `sun_timer` and `render_timer` explicitly `mem::forget`-ed
  before `process::exit(0)`
- Could explain failures: NO. Both approaches keep timers alive
  during the event loop. The `mem::forget` is only at shutdown.

### 14. Window show timing

- Minimal: `window.show()` then `run_event_loop_until_quit()`
- SE: `window.show()` then `run_event_loop()`. If
  `--tray-start hidden`, a `Timer::single_shot(ZERO, ...)` shrinks
  the window after the loop starts.
- Could explain failures: POSSIBLY. The deferred shrink via
  `Timer::single_shot` is fragile. The hypothesis doc confirms:
  "Zero-duration timer to hide after loop starts -- Timer fires once,
  hides window, subsequent timers die."

### 15. `windows_subsystem` attribute

- Minimal: Not present (console subsystem)
- SE: `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]`
- Could explain failures: UNLIKELY in debug builds. The `cfg_attr`
  only applies in release builds (`not(debug_assertions)`), so debug
  test builds still use the console subsystem. However, if tests were
  run against a release build, this would suppress the console window
  and could affect stdout signal delivery.

### 16. Logging framework

- Minimal: None (bare `signal()` via stdout)
- SE: `tracing` + `tracing-subscriber` + `tracing-appender`
  (non-blocking stderr). Optional `SUNLIT_EARTH_SYNC_LOG` mode.
- Could explain failures: YES (for test reliability). The hypothesis
  doc confirmed that `SUNLIT_EARTH_SYNC_LOG` blocks the event loop
  thread on pipe writes, preventing timers from firing. The
  non-blocking writer contends for the stderr pipe buffer with
  diagnostic `eprintln!` calls.

### 17. UI complexity

- Minimal: Single `Image` + `Text` + `Button`
- SE: Full controls panel: ScrollView, GridLayout, 9+ GroupBoxes,
  dozens of sliders, ComboBoxes, callbacks
- Could explain failures: POSSIBLY. The complex UI could cause
  Slint's layout engine to do more work during each event loop
  iteration, leaving less time for timer dispatch. Unlikely to be a
  root cause.

### 18. Config system

- Minimal: None
- SE: Loads/saves TOML config, applies to window properties, saves
  geometry
- Could explain failures: NO. Config I/O happens at discrete points
  (startup, "Set as Wallpaper" click, close), not continuously.

### 19. Rust edition

- Minimal: 2021
- SE: 2024
- Could explain failures: NO. Rust edition does not affect runtime
  behavior.

### 20. Pixel readback in render loop

- Minimal: Yes -- synchronous `readback_pixel()` with
  `device.poll(wait_indefinitely)` in every `BeforeRendering` callback
- SE: No readback during normal rendering (only during wallpaper
  export)
- Could explain failures: NO (opposite direction). The minimal
  project does more blocking GPU work per frame than SE during normal
  rendering. If blocking were the issue, the minimal project would
  fail first.

---

## Section 2: Ranked hypotheses

### Hypothesis 1: `WGPUConfiguration::Manual` breaks timer dispatch after `window.hide()`

Priority: 1 (test first).

What we think causes the failure: When SE passes a pre-created wgpu
device via `WGPUConfiguration::Manual`, Slint's winit backend may set
up its internal event loop differently than when it creates the device
itself via `Automatic`. Specifically:

- The `pollster::block_on` calls during manual adapter enumeration
  and device creation may create a tokio/futures runtime context that
  interferes with winit's own event handling.
- The `Manual` configuration may cause Slint to skip certain
  initialization steps for the wgpu-winit integration that are needed
  for `invoke_from_event_loop` message dispatch and timer processing
  after `window.hide()`.
- The pre-created wgpu instance may hold resources (surfaces,
  adapters) that interact with winit's hidden window state in ways
  that prevent the event loop from waking up on timer signals.

How to test in the minimal project: Change the minimal project's
backend selection from `Automatic` to `Manual`. Replicate SE's
`wgpu_init::init()` pattern:

1. Create a `wgpu::Instance` manually.
2. Enumerate adapters via
   `pollster::block_on(instance.enumerate_adapters(...))`.
3. Select the best adapter.
4. Request a device via
   `pollster::block_on(adapter.request_device(...))`.
5. Pass `WGPUConfiguration::Manual { ... }` to
   `BackendSelector::require_wgpu_28`.

Then run `test_timer_survives_hide`. If it starts failing (ticks stop
after hide), the hypothesis is confirmed.

Expected result if correct: The `test_timer_survives_hide` test will
time out at the "CRITICAL: ticks after window.hide()" step, exactly
matching SE's observed behavior.

### Hypothesis 2: Pre-backend `pollster::block_on` corrupts async context

Priority: 2.

What we think causes the failure: `pollster::block_on` installs a
waker on the current thread. If this is called before
`BackendSelector::select()`, the waker state may conflict with winit's
own thread waker (used for `invoke_from_event_loop` message delivery
via `PostMessageW`). This could explain why `invoke_from_event_loop`
deadlocks in SE but not in the minimal project.

How to test in the minimal project: Before calling
`BackendSelector::select()`, add a dummy `pollster::block_on` call:

```rust
let instance = wgpu::Instance::new(
    &wgpu::InstanceDescriptor::default(),
);
let adapters = pollster::block_on(
    instance.enumerate_adapters(wgpu::Backends::all()),
);
drop(adapters);
drop(instance);
// Now proceed with Automatic configuration as before
```

Then run `test_invoke_from_event_loop_no_deadlock`. If it deadlocks,
the hypothesis is confirmed.

Expected result if correct: Some or all of the 10 rapid
`invoke_from_event_loop` calls will hang, matching SE's deadlock
behavior.

### Hypothesis 3: Heavy rendering workload blocks timer dispatch

Priority: 3.

What we think causes the failure: SE's rendering callback does
substantially more GPU work than the minimal project (full shader
pipeline, multiple render passes, MSAA, dirty-checking, texture
loading). If a single `BeforeRendering` call takes long enough, it
could delay the event loop's return to its message pump, causing
`invoke_from_event_loop` messages to pile up and eventually deadlock
(especially if winit's event-target window message queue fills up).

How to test in the minimal project: Add artificial delay to the
`BeforeRendering` callback:

```rust
std::thread::sleep(Duration::from_millis(100));
```

Or add multiple render passes, texture uploads, and buffer readbacks.
Then run `test_invoke_from_event_loop_no_deadlock` with rapid IPC
commands.

Expected result if correct: With sufficient render delay, the rapid
`invoke_from_event_loop` calls from IPC will start deadlocking or
taking much longer than expected.

### Hypothesis 4: Background `upgrade_in_event_loop` interferes

Priority: 4.

What we think causes the failure: Even with
`SUNLIT_EARTH_NO_CLOUDS`, the `mpsc::channel` for texture messages
is still created and the `texture_rx` receiver is moved into the
rendering callback. The channel infrastructure or the `texture_tx`
sender being held in scope could have subtle effects. More
importantly, if the cloud fetcher is running (non-test mode), its
`upgrade_in_event_loop` calls would compound the
`invoke_from_event_loop` deadlock.

How to test in the minimal project: Add an `mpsc::channel` and pass
the receiver into the rendering callback (mimicking SE's texture
loading infrastructure). Spawn a background thread that periodically
calls `window_weak.upgrade_in_event_loop(...)` every 100ms. Then run
the hide/show test.

Expected result if correct: The test will deadlock or timers will die
when the background thread's `upgrade_in_event_loop` call coincides
with the hidden window state.

### Hypothesis 5: `KeepWindowShown` + 1x1 shrink creates inconsistent state

Priority: 5.

What we think causes the failure: Returning `KeepWindowShown` from
`on_close_requested` while simultaneously shrinking the window to 1x1
may confuse Slint's internal window visibility tracking. Slint thinks
the window is visible (because `KeepWindowShown` was returned), but
the window is effectively invisible (1x1 at 0,0). This inconsistency
could cause Slint to skip timer processing because it believes
rendering is active but the compositor is not actually compositing the
window.

How to test in the minimal project: Change the close request handler
to return `KeepWindowShown` instead of `HideWindow`, and change the
IPC `hide-window` handler to use the 1x1 shrink pattern instead of
`win.hide()`. Then run `test_timer_survives_hide`.

Expected result if correct: Timers will continue to fire with the 1x1
approach (no change from current behavior), disproving this
hypothesis. If timers die, it confirms the shrink approach is itself
problematic.

### Hypothesis 6: Non-blocking tracing writer contends with stdout signals

Priority: 6.

What we think causes the failure: Although SE uses stdout for signals
and stderr for tracing, the non-blocking tracing writer's background
thread and the main thread's `println!` calls may contend on
process-level I/O resources under Windows. When the stderr pipe buffer
fills during heavy logging (GPU setup generates 15-20 debug messages),
the tracing background thread blocks on `write()`. This could
indirectly delay the main thread if there is any shared kernel-level
I/O lock.

How to test in the minimal project: Add `tracing` and
`tracing-appender` with a non-blocking stderr writer. Add `debug!()`
calls throughout the rendering callback to simulate SE's logging
volume. Then run the hide/show test and observe whether signal
delivery is delayed.

Expected result if correct: Signal delivery will become unreliable,
causing test timeouts even though the app is functioning correctly.

---

## Section 3: Proposed test matrix

Each test changes ONE variable from the minimal project's working
configuration. Tests are ordered by likelihood of reproducing the
failure (matching the hypothesis priority).

### Test 1: Switch from Automatic to Manual wgpu configuration

Variable changed: `WGPUConfiguration::Automatic` to `Manual`.

Steps:

1. Add `pollster = "0.4"` to the minimal project's dependencies.
2. In `main.rs`, before `BackendSelector::select()`:
   - Create `wgpu::Instance::new(Default::default())`
   - `pollster::block_on(instance.enumerate_adapters(...))`
   - Select best adapter
   - `pollster::block_on(adapter.request_device(...))`
3. Pass `WGPUConfiguration::Manual { ... }`.
4. Run `test_timer_survives_hide` and
   `test_invoke_from_event_loop_no_deadlock`.

Success criteria for hypothesis confirmation: Either test fails
(timer death or deadlock).

### Test 2: Add pollster block_on before backend init

Variable changed: Pre-backend `pollster::block_on` calls.

Steps:

1. Add `pollster = "0.4"` to the minimal project's dependencies.
2. In `main.rs`, before `BackendSelector::select()`, call
   `pollster::block_on(async {})` or enumerate adapters and drop
   them.
3. Keep `WGPUConfiguration::Automatic`.
4. Run both tests.

Success criteria: If this alone causes failures, the issue is
`pollster` async context pollution, not `Manual` config per se.

### Test 3: Add artificial render delay

Variable changed: Rendering callback duration.

Steps:

1. Add `std::thread::sleep(Duration::from_millis(200))` inside the
   `BeforeRendering` handler.
2. Run `test_invoke_from_event_loop_no_deadlock`.

Success criteria: Deadlock occurs during rapid IPC commands.

### Test 4: Add upgrade_in_event_loop from a background thread

Variable changed: Background thread calling into event loop.

Steps:

1. Spawn a background thread that calls
   `window_weak.upgrade_in_event_loop(|_| {})` every 100ms.
2. Run `test_timer_survives_hide`.

Success criteria: Timers die or the test deadlocks.

### Test 5: Switch from HideWindow to KeepWindowShown + 1x1 shrink

Variable changed: Hide mechanism.

Steps:

1. Change `on_close_requested` to return `KeepWindowShown`.
2. Change IPC `hide-window` to shrink to 1x1 at (0,0) instead of
   `win.hide()`.
3. Change IPC `show-window` to restore size to 400x300.
4. Run `test_timer_survives_hide`.

Success criteria: If timers still survive, the shrink approach is
equivalent to `hide()` in the minimal project's context. If they
die, the shrink approach itself is problematic.

### Test 6: Add non-blocking tracing infrastructure

Variable changed: Logging framework.

Steps:

1. Add `tracing`, `tracing-subscriber`, `tracing-appender` to
   dependencies.
2. Initialize a non-blocking stderr writer before backend init.
3. Add `debug!()` calls in the rendering callback and timer callback.
4. Run both tests.

Success criteria: Signal delivery becomes unreliable or timers appear
to die (actually just delayed beyond test timeouts).

### Test 7: Compound test (Manual + heavy render + background thread)

Variable changed: Multiple (compound).

Steps:

1. Apply changes from Tests 1, 3, and 4 simultaneously.
2. Run both tests.

Success criteria: This should reproduce the full set of SE failures
if the individual hypotheses are each partial contributors. Only run
this after individual tests to distinguish root causes from
compounding factors.

### Test 8: Add windows_subsystem attribute

Variable changed: Subsystem attribute.

Steps:

1. Add `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]`
   to `main.rs`.
2. Build in release mode.
3. Run tests against the release binary.

Success criteria: If stdout signals stop arriving (because there is
no console), this confirms a test infrastructure issue rather than a
fundamental event loop issue. Low priority because SE tests run in
debug mode.

---

## Experiment results (2026-03-28)

All experiments were run as parametrized integration tests in
`slint-tray-minimal`. Each test changes one variable from the
working baseline. **All 10 tests passed.**

| Experiment | Variable changed | Timer survives hide | invoke no deadlock |
| --- | --- | --- | --- |
| Baseline | none (Automatic) | PASS | PASS |
| Exp 1 | `WGPUConfiguration::Manual` | PASS | PASS |
| Exp 2 | 200ms render delay | PASS | PASS |
| Exp 3 | Background `invoke_from_event_loop` | PASS | PASS |
| Exp 4 | Manual + delay + background (compound) | PASS | PASS |

### Conclusions from experiments

- **Hypothesis 1 REJECTED**: `WGPUConfiguration::Manual` with
  `pollster::block_on` before Slint backend init does NOT break
  timer dispatch or cause `invoke_from_event_loop` deadlocks.
- **Hypothesis 2 REJECTED**: Pre-backend `pollster::block_on` does
  not corrupt async context.
- **Hypothesis 3 REJECTED**: Heavy rendering (200ms delay per frame)
  does not block timer dispatch or cause deadlocks.
- **Hypothesis 4 REJECTED**: Background `invoke_from_event_loop`
  calls do not interfere with timer dispatch.
- **Hypothesis 5 SKIPPED**: `KeepWindowShown` + 1x1 shrink test
  omitted (undesirable workaround).
- **Hypothesis 6 NOT YET TESTED**: Non-blocking tracing writer.

### What remains untested

The one variable NOT tested is the event loop function itself:
`run_event_loop()` vs `run_event_loop_until_quit()`. Sunlit Earth
currently uses `run_event_loop()` because earlier testing concluded
that `run_event_loop_until_quit()` also failed. However, those earlier
tests were confounded by `SUNLIT_EARTH_SYNC_LOG` and other variables.

**New primary hypothesis**: Sunlit Earth's tray failures are caused
by using `run_event_loop()` instead of `run_event_loop_until_quit()`.
All the workarounds (off-screen positioning, command queue, timer-
deferred quit, `process::exit()`) were adopted to work around the
consequences of this wrong event loop function choice. The earlier
testing that "disproved" `run_event_loop_until_quit()` was confounded.

### Experiment 5: `run_event_loop()` vs `run_event_loop_until_quit()`

| Test | `run_event_loop()` | `run_event_loop_until_quit()` |
| --- | --- | --- |
| Timer survives hide | **FAIL** | PASS |
| invoke no deadlock | PASS | PASS |

**ROOT CAUSE CONFIRMED.** When `run_event_loop()` is used with
`CloseRequestResponse::HideWindow`, hiding the last window causes
the event loop to exit entirely. The timer doesn't "die" — the
whole event loop terminates. The stdout output shows:

```text
SIGNAL:window_hidden
SIGNAL:quit          ← event loop exits immediately
```

This matches the behavior documented in the Slint API:
`run_event_loop()` "runs the event loop and returns when the last
window is closed." `HideWindow` counts as closing the window.

**All of Sunlit Earth's workarounds were addressing this single root
cause:**

- Off-screen positioning instead of `window.hide()` → avoids
  triggering the "last window closed" condition
- `KeepWindowShown` instead of `HideWindow` → avoids closing
- Command queue + polling timer instead of `invoke_from_event_loop`
  → works around the event loop exiting (not a real deadlock)
- `process::exit(0)` instead of `quit_event_loop()` → works around
  the event loop already having exited

**Fix for Sunlit Earth:** Switch from `run_event_loop()` to
`run_event_loop_until_quit()`. Remove all workarounds. Use the
recommended pattern that the minimal project proves works:
`HideWindow`, `window.hide()`, `invoke_from_event_loop`, direct
`quit_event_loop()`. All experiments confirm this works with
`WGPUConfiguration::Manual`, heavy rendering, and background threads.
