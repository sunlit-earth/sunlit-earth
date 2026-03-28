# Research: Slint Tray App Patterns — 2026-03-28

**Context:** Sunlit Earth is a Windows desktop app using Slint ~1.15 (winit
backend, wgpu rendering). We want minimize-to-tray behavior. We have hit
several concrete problems in the process. This document researches each
problem systematically.

**Prior art in this repo:**

- `docs/plans/2026-03-25-tray-icon-research.md` — crate selection, integration
  architecture, initial risk assessment
- `docs/plans/2026-03-25-slint-shutdown-research.md` — quit_event_loop,
  invoke_from_event_loop, RenderingTeardown, wgpu Drop sequence

This document focuses specifically on the event loop / window visibility
problems that have been observed in practice, and does not re-cover ground
already documented above.

---

## 1. How Do Other Slint Apps Implement Minimize-to-Tray?

### 1.1 Findings

Slint has no native tray icon API as of 1.15. The feature request tracking
native support is issue #6053 (opened September 2024, labeled "roadmap",
open as of March 2026).

The maintainer-recommended approach, repeated consistently across multiple
discussions (#933, #3266, #4362), is:

1. Use an external tray crate (`tray-icon`, `tray-item`, or `ksni`).
2. Call `window.hide()` / `window.show()` to control visibility.
3. Use `run_event_loop_until_quit()` (not `window.run()` or
   `run_event_loop()`) to keep the event loop alive after the window is
   hidden.
4. Use `invoke_from_event_loop()` to cross from the tray thread into the
   Slint event loop thread for `show()` / `hide()` / `quit_event_loop()`.

No complete open-source example combining `slint` + `tray-icon` + wgpu was
found in public repositories as of March 2026. The pattern is described
verbally in discussions but not demonstrated in an official Slint example.

The `tray-icon` crate (tauri-apps, ~1.7M downloads/month) is the
community-preferred choice on Windows. On Windows, it requires a Win32
message loop on the same thread where the icon is created. This does not
need to be the main thread — a dedicated background thread with
`GetMessageW` loop is the standard pattern.

**Sources:**

- [Slint discussion #933: how to create a tray application](https://github.com/slint-ui/slint/discussions/933)
- [Slint issue #6053: Request for Support System Tray Icon](https://github.com/slint-ui/slint/issues/6053)
- [tray-icon crate docs](https://docs.rs/tray-icon/latest/tray_icon/)

**Confidence: High.** The "no native API, use external crate +
`run_event_loop_until_quit`" pattern is consistent across all authoritative
sources.

---

## 2. Keeping an Event Loop Alive with No Visible Window

### 2.1 Findings

`run_event_loop_until_quit()` is the correct function. Its documented
contract:

> "Enters the main event loop and continues to run even when the last window
> is closed, until `quit_event_loop()` is called. This is useful for system
> tray applications where the application needs to stay alive even if no
> windows are visible."

This was the resolution of issue #1499 ("Keep the eventloop running when
windows are closed"), closed via PR #4315. The old workaround (private API
`i-slint-backend-selector::set_event_loop_quit_on_last_window_closed(false)`)
is now superseded by the public `run_event_loop_until_quit()` function.

`window.run()` is a convenience wrapper: `show()` + `run_event_loop()` +
`hide()`. It uses `run_event_loop()` internally, which exits when the last
window is closed (or `quit_event_loop()` is called). This means:

- `window.run()` with `CloseRequestResponse::HideWindow`: the window is
  hidden by Slint (satisfying "last window closed"), and `run_event_loop()`
  exits. The app appears to quit even though the tray icon is still active.
  **This is the wrong function for tray apps.**

- `run_event_loop_until_quit()` with `CloseRequestResponse::HideWindow`:
  hiding the window does not trigger the event loop to exit. The loop
  continues running, timers fire, and the tray thread can still call
  `invoke_from_event_loop`. **This is the correct function for tray apps.**

**Sources:**

- [docs.rs: run_event_loop_until_quit](https://docs.rs/slint/latest/slint/fn.run_event_loop_until_quit.html)
- [Slint issue #1499: Keep the eventloop running when windows are closed](https://github.com/slint-ui/slint/issues/1499)
- [Slint discussion #4362: How to hide, then restore a window?](https://github.com/slint-ui/slint/discussions/4362)

**Confidence: High.**

---

## 3. Is `SLINT_DESTROY_WINDOW_ON_HIDE` Relevant?

### 3.1 Findings

`SLINT_DESTROY_WINDOW_ON_HIDE=1` is an undocumented environment variable
that causes Slint to call `suspend()` on the window adapter when a window
is hidden, rather than merely setting visibility. This has three observable
effects:

1. **GPU resource release on hide**: `suspend()` → `renderer.suspend()` →
   `clear_graphics_context()` → fires `RenderingTeardown`. This means GPU
   resources (wgpu Device, textures, etc.) are freed when the window is
   hidden, not just when the app exits.

2. **BorrowMutError crash on winit backend**: Setting this variable has been
   reported to cause a panic (`already borrowed: BorrowMutError`) in
   `i_slint_backend_winit::winitwindowadapter.rs` when toggling window
   visibility. This is a known bug. Discussion #5854 confirmed the variable
   "did nothing" on Arch Linux/Wayland in one test, and caused a crash in
   another (Windows/winit).

3. **"Terminates the Slint thread after the last window hides"**: One
   reporter described the variable as causing the event loop to exit after
   the last window hides — directly contradicting the purpose of tray mode.

**Conclusion for Sunlit Earth:** Do not set `SLINT_DESTROY_WINDOW_ON_HIDE`.
It causes crashes on the winit backend, may exit the event loop prematurely,
and is only needed as a GPU memory optimization for apps that hide windows
for long periods and want to reclaim VRAM. The workaround context for issue
\#8795 (where this variable is mentioned) is already handled by Slint 1.13's
`suspend_all_hidden_windows()` fix inside `CustomEvent::Exit`.

The environment variable is relevant only as an explanation for why wgpu
resources are *not* destroyed on `window.hide()` by default — which is
correct behavior for tray apps that need to re-show the window without
reinitializing the entire rendering pipeline.

**Sources:**

- [Slint discussion #5854: Memory usage question](https://github.com/slint-ui/slint/discussions/5854)
- [Slint discussion #6718: Unexpected memory allocation](https://github.com/slint-ui/slint/discussions/6718)
- Slint source analysis: `docs/plans/2026-03-25-slint-shutdown-research.md`

**Confidence: Medium.** The BorrowMutError crash is confirmed by user
reports. Whether this is fixed in 1.15 is unknown without testing.

---

## 4. Should the Window Be Destroyed and Recreated Instead of Hidden?

### 4.1 Findings

This pattern (destroy on close, create new on tray-click) is used in some
frameworks but is not viable for Sunlit Earth for the following reasons.

**`set_rendering_notifier` can only be called once per window.** The
rendering notifier is registered on window creation and cannot be
re-registered after teardown on the same window object. Recreating the
window would require recreating the entire `MainWindow` component (the
Slint-generated type), re-registering all callbacks, re-setting all UI
state, and re-initializing the rendering notifier. This is architecturally
complex and error-prone.

**wgpu resources are tied to the rendering notifier lifecycle.** The
`GpuResources` stored in `GPU_RESOURCES` (a
`thread_local! { RefCell<Option<...>> }`) are populated during
`RenderingSetup` and freed during `RenderingTeardown`. Destroying and
recreating the window would require a full GPU pipeline reinitialize,
re-creating the adapter, device, queue, bind groups, and textures. Given the
current architecture, this would effectively be a full app restart.

**The hide/show path is well-supported.** `window.hide()` keeps the Slint
component alive, keeps GPU resources allocated (by default, without
`SLINT_DESTROY_WINDOW_ON_HIDE`), and allows `window.show()` to make the
window visible again without any re-initialization. The rendering notifier
continues to fire `BeforeRendering` on the next redraw request.

**`RenderingTeardown` is NOT called when `window.hide()` is used** (unless
`SLINT_DESTROY_WINDOW_ON_HIDE=1`). This was confirmed by reading the Slint
winit backend source (see `2026-03-25-slint-shutdown-research.md`, section
3.3): teardown fires from `FemtoVGRenderer::drop()` (actual destruction),
from `suspend()` (only on Wayland or with the env var), or from
`suspend_all_hidden_windows()` at exit. A simple `window.hide()` on Windows
with the winit backend does not trigger teardown.

**Slint does not support creating a second event loop after the first has
exited.** Issue #4468 documents that after `run_app_on_demand` exits,
winit's internal `exit: Option<i32>` field is `Some(0)`, and subsequent
calls to `pump_events` immediately return `PumpStatus::Exit`. The only
workaround is `run_on_demand`, which re-initializes internal state. This
makes destroy-then-recreate with a second event loop invocation unreliable.

**Recommendation:** Use `window.hide()` / `window.show()` exclusively. Do
not destroy and recreate.

**Sources:**

- [Slint issue #4468: Running the event loop a second time doesn't work](https://github.com/slint-ui/slint/issues/4468)
- [Slint discussion #2828: How to hide a window instead of closing it](https://github.com/slint-ui/slint/discussions/2828)
- Slint source analysis: `docs/plans/2026-03-25-slint-shutdown-research.md`

**Confidence: High.**

---

## 5. Timer Contract Under `run_event_loop_until_quit()`

### 5.1 Findings

**Official contract:** "Enters the main event loop and continues to run even
when the last window is closed, until `quit_event_loop()` is called."

No official documentation describes timer behavior changes when windows are
hidden. The Slint architecture (documented in the DeepWiki analysis of the
Slint source) shows that timers are fired from
`TimerList::maybe_activate_timers()` during every event loop iteration,
regardless of window visibility. The event loop continues iterating under
`run_event_loop_until_quit()` even with all windows hidden.

**The problems we observed** (timers stop firing after hide) are likely
caused by a different mechanism:

- **`window.run()` was used instead of `run_event_loop_until_quit()`**: As
  established in section 2, `window.run()` calls `run_event_loop()` which
  exits when the last window is hidden. Timers are only fired during event
  loop iterations; if the loop has exited, no timers fire. This matches the
  observed symptom precisely.

- **`set_minimized(true)` vs `window.hide()`**: Calling
  `set_minimized(true)` on the underlying winit window does NOT hide the
  window from Slint's perspective. Slint's event loop does not count
  "minimized" as "closed". However, a minimized window does not request
  redraws, so `BeforeRendering` does not fire. The 2-minute timer that
  triggers redraws would still fire (the timer mechanism is independent of
  rendering). The timer should still work; the renderer just would not
  produce visible output.

- **`run_app_on_demand` re-entry**: If `window.run()` was called again after
  it returned (e.g., to show the window again), the behavior is undefined
  because the winit backend's `exit` status is not reset. This matches the
  "re-entry loops where timer state is lost" symptom.

**Timer reliability under `run_event_loop_until_quit()`:** No known bug
reports or Slint issues document timers becoming unreliable under
`run_event_loop_until_quit()` when windows are hidden. The timer subsystem
is independent of window visibility.

**The `invoke_from_event_loop` + `Timer::single_shot` pattern** (already
used in `src/ipc.rs`) is a confirmed-sound approach for deferred execution.
Timers with duration zero fire on the next event loop iteration after being
registered.

**Sources:**

- [docs.rs: run_event_loop_until_quit](https://docs.rs/slint/latest/slint/fn.run_event_loop_until_quit.html)
- [docs.rs: Timer](https://docs.rs/slint/latest/slint/struct.Timer.html)
- [DeepWiki: Slint Window System and Event Handling](https://deepwiki.com/slint-ui/slint/2.3-window-system-and-event-handling)

**Confidence: High** (for the "use `run_event_loop_until_quit`" conclusion).
**Confidence: Medium** (for the diagnosis of why timers appeared to stop —
this is an inference from the event loop behavior, not a confirmed bug
report).

---

## 6. Winit `Suspended` Events on Windows Desktop

### 6.1 Findings

**`Suspended` does NOT fire on Windows desktop.** This is confirmed by the
winit team in issue #2185:

> "Currently [Suspended and Resumed] are just emitted on Android + iOS."

The documentation further confirms: "Not all platforms support the notion of
suspending applications... Winit does not currently try to emit pseudo
Suspended events before the application quits on platforms without an
application lifecycle (currently only Android, iOS, and Web do)."

**`set_minimized(true)` on Windows:** Minimizing a window via winit does
not fire `Suspended`. It fires a `WindowEvent::Occluded(true)` event
(window is occluded / not visible to the user). The event loop continues
running normally. Winit issue #1578 confirms events for maximized/minimized
state exist, but are separate from `Suspended`.

**Therefore:** The `Suspended` event is not involved in any tray app window
management on Windows. Any behavior attributed to `Suspended` firing on
minimize is a misdiagnosis. The winit source documentation confirms desktop
platforms never receive this event during normal operation.

**What `set_minimized(true)` actually does:**

- Minimizes the window to the taskbar (not the tray).
- Does not hide the window from the event loop.
- Does not reduce event loop frequency or timer dispatch.
- Does not trigger `RenderingTeardown`.
- The window remains in Slint's window list; `run_event_loop()` would not
  consider the last window closed.

**Sources:**

- [winit issue #2185: Suspended/Resumed documentation lacking](https://github.com/rust-windowing/winit/issues/2185)
- [winit issue #1578: Events for window maximized/minimized](https://github.com/rust-windowing/winit/issues/1578)

**Confidence: High.**

---

## 7. Using Slint's Platform Abstraction Layer

### 7.1 Findings

Slint provides two levels of platform customization.

### 7.2 `BackendSelector::with_winit_custom_application_handler()`

This is the most relevant mechanism. Available in Slint 1.12+ via the
`BackendSelector` API (requires the `unstable-winit-030` feature flag,
which is already enabled in Sunlit Earth via `unstable-wgpu-28`):

```rust
pub fn with_winit_custom_application_handler(
    self,
    custom_application_handler: impl CustomApplicationHandler + 'static
) -> BackendSelector
```

The `CustomApplicationHandler` trait "imitates winit's ApplicationHandler"
with all methods called **before** Slint processes the same event. Methods
include:

- `resumed()` — application resume
- `window_event()` — per-window events (with access to both winit and Slint
  window)
- `new_events()` — start of a new event batch
- `device_event()` — raw device events
- `about_to_wait()` — before the event loop sleeps
- `suspended()` — application suspension
- `exiting()` — application exit

Each method returns `EventResult` to optionally prevent Slint from seeing
the intercepted event.

**Tray application use case:** `about_to_wait()` is fired before the event
loop blocks waiting for the next event. This is where the `tray-icon`
crate's recommended integration point lives
(`TrayIconEvent::receiver().try_recv()` polling). Using
`with_winit_custom_application_handler()`, tray events can be polled
directly on the Slint event loop thread, eliminating the need for a separate
Win32 message loop thread.

However, the `tray-icon` crate documentation also supports a callback model
(`TrayIconEvent::set_event_handler`) which, when combined with
`invoke_from_event_loop`, works without `CustomApplicationHandler`. The
separate Win32 message loop thread approach (already implemented in Sunlit
Earth's `tray.rs`) is also valid.

**Caveat:** `unstable-winit-030` is explicitly marked as not subject to
Slint's stability guarantees because winit releases major versions
frequently. The API may change in a Slint minor release if winit bumps to
0.31+.

### 7.3 Custom `Platform` trait implementation

Implementing the full `slint::platform::Platform` trait to replace the
winit backend entirely. This would allow a completely custom event loop
(e.g., a pure Win32 message loop) and no dependency on winit at all.

This is primarily used for embedded/MCU targets where winit is not
appropriate. For a desktop Windows app that already works well with the
winit backend, this is not a practical path — it would require implementing
window creation, wgpu context creation, input event translation, and all the
other work that winit provides.

**Sources:**

- [docs.rs: BackendSelector](https://docs.rs/slint/latest/slint/struct.BackendSelector.html)
- [docs.rs: CustomApplicationHandler](https://docs.rs/slint/latest/slint/winit_030/trait.CustomApplicationHandler.html)
- [Slint issue #6583: Process non-WindowEvent events on winit backend](https://github.com/slint-ui/slint/issues/6583)
- [Slint docs: slint::platform](https://snapshots.slint.dev/master/docs/rust/slint/platform/)

**Confidence: High** (the API exists and is documented). **Medium** (for
whether it solves the specific problems — no concrete example combining it
with tray-icon was found).

---

## Recommended Architecture

Based on the research, the following architecture addresses all observed
problems.

### Event Loop

Use `run_event_loop_until_quit()`. Never use `window.run()` in tray mode.
`window.run()` internally calls `run_event_loop()`, which exits when the
window is hidden — exactly the wrong behavior.

```rust
// Tray mode startup
window.show().unwrap();
spawn_tray_thread(window.as_weak());
slint::run_event_loop_until_quit().unwrap();
// Cleanup here (tray thread already posted WM_QUIT to itself)
```

### Window Visibility

Use `window.hide()` and `window.show()` only. Do not call
`set_minimized(true)` on the winit window for tray purposes — minimizing
goes to the taskbar, not the tray. Use `CloseRequestResponse::HideWindow`
from `on_close_requested` to intercept the OS close button.

```rust
window.on_close_requested(|| CloseRequestResponse::HideWindow);
```

### Cross-Thread Communication from Tray Thread

Use `invoke_from_event_loop` with a `slint::Weak<MainWindow>`:

```rust
// Show: called from tray thread or menu event handler
slint::invoke_from_event_loop(move || {
    if let Some(window) = weak.upgrade() {
        window.show().unwrap();
    }
}).ok();

// Quit: use the timer deferral pattern from 2026-03-25-slint-shutdown-research.md
slint::invoke_from_event_loop(|| {
    slint::Timer::single_shot(std::time::Duration::ZERO, || {
        slint::quit_event_loop().ok();
    });
}).ok();
```

The `Timer::single_shot(Duration::ZERO, ...)` deferral is mandatory for
quit. Calling `quit_event_loop()` directly from inside
`invoke_from_event_loop` crashes the wgpu backend on Windows
(STATUS_STACK_BUFFER_OVERRUN). This is already documented in
`2026-03-25-slint-shutdown-research.md` and is the pattern currently used
in `src/ipc.rs`.

### GPU Resources and Window Visibility

`RenderingTeardown` is NOT called when `window.hide()` is used (on Windows,
with winit backend, without `SLINT_DESTROY_WINDOW_ON_HIDE`). GPU resources
remain allocated while the window is hidden. When `window.show()` is called,
the rendering notifier continues with `BeforeRendering` on the next redraw
— no `RenderingSetup` is re-fired. This is correct and desired behavior;
the existing `GPU_RESOURCES` thread-local remains valid across hide/show
cycles.

### Timers

Slint timers fire during every event loop iteration under
`run_event_loop_until_quit()`, regardless of window visibility. The 2-minute
sun timer (periodic redraw) continues to fire while the window is hidden.
This means `BeforeRendering` is called even when the window is hidden, but
the rendering simply produces no visible output (the OS suppresses drawing
to hidden windows). This is harmless.

If completely preventing hidden-window rendering is desired, the
`BeforeRendering` callback can check a flag and return early. However, this
optimization is not necessary for correctness.

### Tray Thread Lifecycle

The tray thread runs a Win32 message loop (`GetMessageW`) and creates the
`TrayIcon` on that same thread. When the user selects "Exit", the menu
event handler calls `invoke_from_event_loop` (with the timer deferral for
quit). The tray thread can then post `WM_QUIT` to itself (using the thread
ID captured at spawn) to exit the `GetMessageW` loop, or simply let the
process exit (which is what the current `std::process::exit(0)` already
does).

### Optional Enhancement: `BackendSelector` Integration

If the separate Win32 tray thread proves problematic,
`with_winit_custom_application_handler()` offers an alternative: poll
`TrayIconEvent::receiver().try_recv()` inside the `about_to_wait()` handler
on the main event loop thread. This eliminates the need for a second thread
and its associated `invoke_from_event_loop` overhead. This requires the
`unstable-winit-030` feature flag, which is already indirectly enabled (the
`unstable-wgpu-28` feature is active; check whether it implies
`unstable-winit-030` in the Slint 1.15 feature graph).

---

## Known Limitations

1. **No Slint native tray API.** External crate (`tray-icon`) is mandatory.
   Native support is on the roadmap (issue #6053) but not yet scheduled.

2. **`unstable-winit-030` feature is not stability-guaranteed.** If Slint
   bumps winit from 0.30 to 0.31 in a minor release, the
   `CustomApplicationHandler` API may change. Avoid deep reliance on it.

3. **`SLINT_DESTROY_WINDOW_ON_HIDE` causes crashes on winit backend.** Do
   not use it. This means GPU memory is not released when the window is
   hidden. For Sunlit Earth (a single GPU-heavy window), this is not a
   concern — the VRAM is in use for the wallpaper regardless.

4. **`RenderingSetup` does not re-fire after `window.show()`.** GPU
   resources must survive hide/show cycles without re-initialization. The
   current `thread_local! { RefCell<Option<GpuResources>> }` pattern handles
   this correctly because resources remain `Some(...)` across hide/show.

5. **Timers continue firing when the window is hidden.** `BeforeRendering`
   is called but produces invisible output. This is correct behavior but
   means the wgpu pipeline executes renders to a hidden window. The cost is
   one render every 2 minutes (driven by the sun timer) — negligible.

6. **Single-instance enforcement cannot communicate with the existing
   instance.** The `single-instance` crate (used in Sunlit Earth) uses a
   Windows mutex and can only detect / block second instances. It cannot
   send a "show window" signal to the running instance. Winit issue #3964
   (bring-to-foreground API) is open with no implementation. IPC via named
   pipe or socket (as already implemented in `src/ipc.rs`) is the correct
   approach.

7. **winit's `Suspended` event is irrelevant on Windows desktop.** Any
   observed behavior changes on minimize are due to rendering pipeline
   changes, not `Suspended` events.

---

## Open Questions

1. **Does `BeforeRendering` fire while the window is hidden?** The research
   suggests yes (timer fires → redraw requested → `BeforeRendering` called),
   but this has not been confirmed empirically by reading the
   `set_visible(false)` path in the winit adapter. It is possible Slint
   suppresses `BeforeRendering` for hidden windows at the
   `WindowAdapter::render()` call site.

2. **Does `with_winit_custom_application_handler()` require a separate
   `unstable-winit-030` feature flag in Slint 1.15?** The Sunlit Earth
   `Cargo.toml` enables `unstable-wgpu-28` but not `unstable-winit-030`
   explicitly. These may be separate gates. If they are, enabling
   `unstable-winit-030` may conflict with the pinned winit version via
   `unstable-wgpu-28`.

3. **Does `window.show()` after `window.hide()` reliably bring the window
   to the foreground on Windows?** Winit issue #3964 documents that
   `set_visible(true)` does not always unminimize or foreground the window.
   Since Sunlit Earth uses `hide()` (not `set_minimized`), `show()` should
   reliably restore visibility, but focus-stealing prevention in Windows 10+
   may prevent the window from appearing in front of other apps.

4. **What is the exact behavior of `run_event_loop_until_quit()` +
   `window.hide()` + periodic timer on the current Sunlit Earth build (Slint
   1.15.1)?** The prior tray implementation used `window.run()`, which is
   the root cause of the observed problems. Switching to
   `run_event_loop_until_quit()` should resolve them, but this needs
   empirical verification.

---

## Sources

- [Slint discussion #933: how to create a tray application](https://github.com/slint-ui/slint/discussions/933)
- [Slint issue #1499: Keep the eventloop running when windows are closed](https://github.com/slint-ui/slint/issues/1499)
- [Slint issue #6053: Request for Support System Tray Icon](https://github.com/slint-ui/slint/issues/6053)
- [Slint issue #4468: Running the event loop a second time doesn't work](https://github.com/slint-ui/slint/issues/4468)
- [Slint issue #10966: KeepWindowShown can cause quit_event_loop to be ignored](https://github.com/slint-ui/slint/issues/10966)
- [Slint issue #6670: Window destroying/being-destroyed callback request](https://github.com/slint-ui/slint/issues/6670)
- [Slint issue #6583: Process non-WindowEvent events on winit backend](https://github.com/slint-ui/slint/issues/6583)
- [Slint discussion #3266: How to hide the window in the taskbar?](https://github.com/slint-ui/slint/discussions/3266)
- [Slint discussion #4362: How to hide, then restore a window?](https://github.com/slint-ui/slint/discussions/4362)
- [Slint discussion #2828: How to hide a window instead of closing it](https://github.com/slint-ui/slint/discussions/2828)
- [Slint discussion #5854: A question about memory usage](https://github.com/slint-ui/slint/discussions/5854)
- [Slint discussion #6718: Unexpected memory allocation](https://github.com/slint-ui/slint/discussions/6718)
- [docs.rs: run_event_loop_until_quit](https://docs.rs/slint/latest/slint/fn.run_event_loop_until_quit.html)
- [docs.rs: run_event_loop](https://docs.rs/slint/latest/slint/fn.run_event_loop.html)
- [docs.rs: RenderingState](https://docs.rs/slint/latest/slint/enum.RenderingState.html)
- [docs.rs: BackendSelector](https://docs.rs/slint/latest/slint/struct.BackendSelector.html)
- [docs.rs: CustomApplicationHandler](https://docs.rs/slint/latest/slint/winit_030/trait.CustomApplicationHandler.html)
- [docs.rs: tray-icon](https://docs.rs/tray-icon/latest/tray_icon/)
- [winit issue #2185: Suspended/Resumed documentation lacking](https://github.com/rust-windowing/winit/issues/2185)
- [winit issue #1578: Events for window maximized/minimized](https://github.com/rust-windowing/winit/issues/1578)
- [winit issue #3964: Support for activating already running app](https://github.com/rust-windowing/winit/issues/3964)
- [winit discussion #3835: Tray Icons discussion](https://github.com/rust-windowing/winit/discussions/3835)
- [DeepWiki: Slint Window System and Event Handling](https://deepwiki.com/slint-ui/slint/2.3-window-system-and-event-handling)
- Prior research in this repo: `docs/plans/2026-03-25-slint-shutdown-research.md`
- Prior research in this repo: `docs/plans/2026-03-25-tray-icon-research.md`
