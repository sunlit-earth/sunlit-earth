# Research: `--tray-start hidden` Fails with `run_event_loop_until_quit()`

**Date:** 2026-03-28

**Problem:** When Sunlit Earth starts with `--tray-start hidden`, the
current code calls `window.show()` then `window.hide()` then
`slint::run_event_loop_until_quit()`. The event loop enters with no
visible windows and `invoke_from_event_loop` closures posted from the
IPC listener thread are never dispatched. The `show-window` IPC command
never takes effect.

This does NOT happen when the window starts visible and is hidden later
via IPC (`test_tray_hide_show_cycle` passes).

**Prior art:**

- `2026-03-28-hypothesis-verification.md` — original empirical failures
- `2026-03-28-research-slint-tray-app-patterns.md` — Slint tray patterns
- `2026-03-28-minimal-vs-sunlit-comparison.md` — slint-tray-minimal
  comparison and root cause confirmation
- `2026-03-28-tray-mode-fix-plan.md` — implementation plan and results

---

## Question 1: Deferred hide via zero-duration timer

### Answer: Yes, with high confidence

A zero-duration timer fires on the **second iteration** of the event
loop, after the loop has fully started and is in its active dispatch
state. The dispatch order is:

1. `run_event_loop_until_quit()` calls winit's `run_app_on_demand()`.
2. First iteration: `prepare_wait()` transitions the runner from
   `Uninitialized` to `Idle`, firing `StartCause::Init`. Slint's
   `new_events()` handler calls `update_timers_and_animations()`.
   Then `about_to_wait()` sees the zero-duration timer and sets
   `ControlFlow::WaitUntil(now)`, causing `MsgWaitForMultipleObjectsEx`
   to return immediately (timeout=0).
3. Second iteration: `new_events` fires with
   `StartCause::ResumeTimeReached`. `update_timers_and_animations()`
   fires the zero-duration timer callback. **The hide happens here,
   inside the running event loop.**

At this point:

- The event loop is fully running and in its active dispatch state.
- `invoke_from_event_loop` closures posted via `PostMessageW` are
  processed normally by `dispatch_peeked_messages()`.
- The winit event-target window's self-wake cycle (`RedrawWindow` →
  `WM_PAINT` → process → `RedrawWindow`) has been bootstrapped.
- All timers, IPC handlers, and tray thread communication work.

The old code tested this pattern with `run_event_loop()` (wrong
function) and `SUNLIT_EARTH_SYNC_LOG` (synchronous stderr that blocks
the event loop thread on pipe writes). Both confounders masked the
real behavior. The hypothesis-verification document's Step 2 finding
("Zero-duration timer fires once, hides window, subsequent timers
die") was tested under these confounders.

With `run_event_loop_until_quit()` and no `SUNLIT_EARTH_SYNC_LOG`,
the deferred hide should work because:

- `run_event_loop_until_quit()` does not exit when the last window is
  hidden (confirmed by slint-tray-minimal, all 11/12 tests pass).
- The event loop is already pumping when the timer fires, so the
  transition from "visible" to "hidden" is the same as the working
  `test_tray_hide_show_cycle` path.

**Confidence: High.** The winit dispatch order is deterministic and
well-understood from source code inspection. The only remaining risk
is a Slint-layer interaction not present in the minimal project.

**Recommended experiment:** Add a test to slint-tray-minimal that
does `window.show()` → `Timer::single_shot(ZERO, || window.hide())`
→ `run_event_loop_until_quit()`, then sends `show-window` via IPC
and verifies the window becomes visible.

---

## Question 2: Winit behavior with no visible windows at startup

### Answer: Loop pumps messages but may exit early

The winit event loop on Windows has **no special "no visible windows"
code path**. The core loop is always:

```text
loop {
    wait_for_messages()     // MsgWaitForMultipleObjectsEx
    dispatch_peeked_messages()  // PeekMessageW + DispatchMessageW
    check_exit_code()
}
```

`MsgWaitForMultipleObjectsEx` is called with `QS_ALLINPUT` (which
includes `QS_POSTMESSAGE`) and `MWMO_INPUTAVAILABLE` (which returns
even for already-seen messages). A `PostMessageW` from
`invoke_from_event_loop` sets `QS_POSTMESSAGE` and will wake
`MsgWaitForMultipleObjectsEx` regardless of window visibility.

However, there are two mechanisms that could prevent the loop from
reaching its message pump:

### Mechanism A: `quit_on_last_window_closed` fires before the pump starts

`run_event_loop_until_quit()` is supposed to disable this behavior by
setting `EventLoopQuitBehavior::ExplicitQuit`. But if `window.hide()`
before the loop triggers Slint's internal window close tracking (the
strong reference to the window is dropped when `hide()` is called),
the loop may see zero active windows in its first `about_to_wait()`
scan. If there is a race or ordering issue in how
`EventLoopQuitBehavior` is applied vs. when the window count is
checked, the loop could exit immediately.

**Diagnostic:** Add `debug!("event loop returned")` immediately after
`run_event_loop_until_quit()` returns. If this fires within
milliseconds of startup, the loop is exiting, not sleeping.

### Mechanism B: `invoke_from_event_loop` silently fails

The IPC dispatch code does:

```rust
slint::invoke_from_event_loop(move || { ... }).ok();
```

If the event loop has already exited, `invoke_from_event_loop` returns
`Err(EventLoopError::EventLoopTerminated)`. The `.ok()` silently
discards this error. The closure is never executed.

**Diagnostic:** Change `.ok()` to log the error:

```rust
if let Err(e) = slint::invoke_from_event_loop(move || { ... }) {
    warn!("invoke_from_event_loop failed: {e}");
}
```

### Why "started visible then hidden later" works

When the window starts visible, the event loop enters its active
dispatch state with at least one window. The winit event-target
window's self-wake cycle is bootstrapped (each processed message
triggers `RedrawWindow(RDW_INTERNALPAINT)` which posts a `WM_PAINT`,
which wakes the loop again). When the window is later hidden via IPC,
the loop is already running and continues processing events.

When the window is hidden BEFORE the loop starts, this bootstrap never
happens. The loop enters `MsgWaitForMultipleObjectsEx` in a cold
state — though `MWMO_INPUTAVAILABLE` should still cause it to wake on
posted messages. If the loop exited (Mechanism A), the posted messages
go nowhere.

**Confidence: Medium.** The winit message pump mechanics are
well-understood (High confidence), but the exact Slint-layer behavior
when `hide()` is called before the loop starts is inferred, not
directly observed (Medium confidence). The diagnostics above will
resolve this definitively.

---

## Question 3: Does `window.set_minimized(true)` keep the event loop active?

### Answer: Yes

A minimized window on Windows is still an active OS window. Its `HWND`
receives messages and the winit event loop continues pumping normally.
The window simply has no client-area pixels to draw.

Key properties of a minimized window:

- `is_visible()` returns `false` (Slint docs: "can return false even if
  you previously called show() on it, for example if the user minimized
  the window")
- The event loop remains active; `about_to_wait()`, user events, and
  timer callbacks all continue to fire
- `invoke_from_event_loop` closures are dispatched normally
- The window appears in the taskbar (not hidden to tray)
- `BeforeRendering` likely does NOT fire (the OS suppresses `WM_PAINT`
  for minimized windows, so Slint has no reason to request a frame)
- `Suspended` does NOT fire on Windows (winit #2185)

The critical distinction vs. `window.hide()`:

| | `set_minimized(true)` | `hide()` |
| --- | --- | --- |
| HWND exists | Yes | Destroyed/released |
| Event loop runs | Yes | Only with `run_event_loop_until_quit()` |
| Slint strong ref | Retained | Dropped |
| Taskbar entry | Yes (visible) | No |
| Restore mechanism | `set_minimized(false)` | `show()` (recreates window) |

**Problem for tray mode:** Minimizing puts the window in the taskbar,
which defeats the purpose of a tray-only app. There is no Win32 API to
minimize without a taskbar entry. The `tray-icon` crate's tray icon is
a separate Win32 construct; the app window would appear both in the
taskbar and in the tray.

**Confidence: High** for event loop staying active. **Medium** for
rendering behavior (inferred from Win32 `WM_PAINT` suppression, not
directly tested).

---

## Question 4: Can we create the window without `show()` and call it later?

### Answer: Partially — GPU resources need `show()`

`MainWindow::new()` creates the Slint component without displaying it.
The window object is fully valid and can hold callbacks, property
bindings, and a rendering notifier.

`set_rendering_notifier()` registration succeeds on an unshown window.
`SetRenderingNotifierError` only has two variants (`Unsupported` and
`AlreadySet`), neither of which requires the window to be visible. The
callbacks fire correctly once the window becomes visible.

However:

- `RenderingSetup` fires lazily when the winit surface is created,
  which happens inside the first `show()`.
- No GPU resources (pipeline, textures, buffers) exist until
  `RenderingSetup` fires.
- `Window::size()` returns a default `800x600` stub until the window
  has been displayed at least once (Slint issue #6724).
- If `show()` is never called before the event loop starts, there is
  no winit window at all. The event loop would start with zero windows.
  `run_event_loop_until_quit()` should still run, but there is nothing
  for it to manage.

**The skip-show approach is not viable for our case.** We need
`show()` before the event loop to:

1. Create the winit window and wgpu surface.
2. Bootstrap the event loop's self-wake cycle.
3. Allow `RenderingSetup` to fire on the first iteration.

The correct pattern is show → enter loop → hide (deferred), not
skip show entirely.

**Confidence: High.** `set_rendering_notifier` behavior is documented.
The lazy `RenderingSetup` is confirmed by Slint source code
(rendering notifier callbacks are dispatched from the winit
`BeforeRendering` event, which requires a visible window).

---

## Question 5: Does `with_winit_custom_application_handler()` help?

### Answer: `about_to_wait()` works but `resumed()` does not

`BackendSelector::with_winit_custom_application_handler()` is
available behind the `unstable-winit-030` feature flag in Slint 1.15.
It provides a `CustomApplicationHandler` trait that mirrors winit's
`ApplicationHandler`. All methods fire BEFORE Slint's own handler and
return `EventResult` (`Propagate` or `PreventDefault`).

### Available callbacks

```rust
fn resumed(&mut self, event_loop: &ActiveEventLoop) -> EventResult
fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) -> EventResult
fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) -> EventResult
fn window_event(...) -> EventResult
fn device_event(...) -> EventResult
fn suspended(...) -> EventResult
fn exiting(...) -> EventResult
fn memory_warning(...) -> EventResult
```

### `resumed()` — NOT viable on Windows

`resumed()` is **unsupported** on Windows (and macOS, Orbital, Wayland,
X11). It fires only on iOS, Android, and Web where the platform has a
formal suspend/resume lifecycle. On Windows, the startup sequence is:

```text
new_events(StartCause::Init)
  → can_create_surfaces()
  → about_to_wait()
  [normal event pumping]
```

`resumed()` is never dispatched. Using it to defer `window.hide()`
would silently do nothing.

### `about_to_wait()` — viable but unnecessary

`about_to_wait()` fires after every batch of events, including the
first batch after init. A one-shot boolean in the handler state could
call `window.hide()` on the first iteration:

```rust
fn about_to_wait(&mut self, _: &ActiveEventLoop) -> EventResult {
    if !self.initial_hide_done {
        self.initial_hide_done = true;
        if let Some(w) = self.window_weak.upgrade() {
            w.hide().ok();
        }
    }
    EventResult::Propagate
}
```

This achieves the same result as `Timer::single_shot(Duration::ZERO)`
but requires enabling the `unstable-winit-030` feature flag and adds
more complexity. The timer approach is simpler and idiomatic.

### No public examples

No Slint examples demonstrate `with_winit_custom_application_handler`
for tray-mode or hidden-window scenarios. The API was introduced in
PR #2617 as a power-user escape hatch for winit interop.

**Confidence: High.** The `resumed()` non-support on Windows is
confirmed by winit's authoritative documentation and tested by the
minimal project. The `about_to_wait()` approach is viable but
unnecessary given simpler alternatives.

---

## Recommended Approach

### Primary recommendation: Deferred hide via `Timer::single_shot`

```rust
window.show().expect("Failed to show window");

if use_tray && matches!(tray_start, TrayStart::Hidden) {
    let window_weak = window.as_weak();
    slint::Timer::single_shot(std::time::Duration::ZERO, move || {
        if let Some(win) = window_weak.upgrade() {
            debug!("hiding window for --tray-start hidden (deferred)");
            win.hide().ok();
        }
    });
}

slint::run_event_loop_until_quit().expect("Failed to run event loop");
```

**Why this works:**

1. `window.show()` creates the winit window and wgpu surface.
2. `run_event_loop_until_quit()` starts with a visible window, entering
   its active dispatch state.
3. On the second iteration, the zero-duration timer fires and calls
   `window.hide()`. The event loop is already running, so it continues
   processing events even after the hide.
4. `invoke_from_event_loop` closures from the IPC thread and tray
   thread are dispatched normally.

**Trade-off:** There is a brief window flash on startup (one frame,
typically <16ms). For a wallpaper app that starts hidden on boot, this
flash may be imperceptible. If it's visible, the window could be
created at position (-32000, -32000) and moved to the correct position
on the first show-window IPC command.

### Alternative: Deferred hide via `invoke_from_event_loop`

```rust
window.show().expect("Failed to show window");

if use_tray && matches!(tray_start, TrayStart::Hidden) {
    let window_weak = window.as_weak();
    slint::invoke_from_event_loop(move || {
        if let Some(win) = window_weak.upgrade() {
            debug!("hiding window for --tray-start hidden (deferred)");
            win.hide().ok();
        }
    }).expect("event loop not started");
}

slint::run_event_loop_until_quit().expect("Failed to run event loop");
```

`invoke_from_event_loop` closures queued before the loop starts are
held in an internal queue and dispatched once the loop begins
processing. This is documented behavior: "adds the specified function
to an internal queue, notifies the event loop to wake up."

**Trade-off:** Same window flash. Relies on the queued closure being
dispatched early enough (before the first `BeforeRendering` callback),
which is not guaranteed by the Slint API.

### Not recommended: `set_minimized(true)`

While this keeps the event loop active, it shows the window in the
taskbar, which defeats the purpose of tray-only mode.

### Not recommended: `with_winit_custom_application_handler`

More complexity for the same result. Requires `unstable-winit-030`
feature flag. No public examples or community precedent.

### Not recommended: Skip `show()` entirely

No GPU resources would be created. The event loop would start with
zero windows. Even if the loop stays alive, the first `show-window`
IPC command would need to bootstrap the entire rendering pipeline.

---

## Experiment results (slint-tray-minimal)

Three experiments were added to `slint-tray-minimal` (commit
`a79c780`) with `--hide-immediate` and `--hide-deferred` CLI flags,
plus `invoke_from_event_loop` error logging and a
`SIGNAL:event_loop_exited` signal after the loop returns.

| Exp | Description | Result |
| --- | --- | --- |
| 6 | Immediate hide before loop | **Bug reproduced** |
| 7 | Deferred hide (zero-duration timer) | **PASS** |
| 8 | Deferred hide + Manual wgpu | **PASS** |

### Experiment 6: Immediate hide (reproduce the bug)

`show()` → `hide()` → `run_event_loop_until_quit()`.

`SIGNAL:event_loop_exited` appeared immediately — the event loop
returned without ever entering its dispatch phase. `invoke_failed`
was NOT seen because the IPC command arrived after the loop had
already exited (the `.ok()` silently discarded
`EventLoopTerminated`).

**Root cause confirmed: Mechanism A.** `run_event_loop_until_quit()`
exits immediately when `window.hide()` is called before the loop
starts. The loop never reaches its message pump.

### Experiment 7: Deferred hide (proposed fix)

`show()` → `Timer::single_shot(ZERO, hide)` →
`run_event_loop_until_quit()`.

The deferred timer fired, the window was hidden, and
`invoke_from_event_loop` closures from the IPC thread were
dispatched correctly. `show-window` via IPC worked after the
deferred hide.

### Experiment 8: Deferred hide + Manual wgpu (compound)

Same as experiment 7 with `WGPUConfiguration::Manual`. Passed —
Manual wgpu config is not a factor.

---

## Implementation (Sunlit Earth)

Applied the deferred-timer approach to `src/main.rs`. Replaced:

```rust
if use_tray && matches!(tray_start, TrayStart::Hidden) {
    window.hide().expect("Failed to hide window");
}
```

With:

```rust
if use_tray && matches!(tray_start, TrayStart::Hidden) {
    let ww = window.as_weak();
    slint::Timer::single_shot(std::time::Duration::ZERO, move || {
        if let Some(win) = ww.upgrade() {
            win.hide().ok();
        }
        println!("SIGNAL:window_hidden_deferred");
    });
}
```

Updated `tests/e2e.rs` to wait for `SIGNAL:window_hidden_deferred`
before sending `show-window` (avoids a race where show fires before
the deferred timer).

### Verification

| Check | Result |
| --- | --- |
| `cargo build` | OK |
| `cargo clippy` | Clean |
| `cargo test` (294 tests) | All pass |
| `test_binary_exists` | PASS |
| `test_render_and_exit` | PASS |
| `test_windowed_mode_graceful_shutdown` | PASS |
| `test_tray_hide_show_cycle` | PASS |
| `test_single_instance_second_exits` | PASS |
| `test_tray_mode_ipc_lifecycle` | **PASS** (previously failing) |
| Manual: `cargo run -- --tray-start hidden` | Works |
