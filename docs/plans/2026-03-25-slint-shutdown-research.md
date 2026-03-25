# Slint Event Loop Shutdown Research

**Date:** 2026-03-25
**Context:** Investigating shutdown behavior of the Slint event loop, particularly
the interaction between `invoke_from_event_loop`, `quit_event_loop`, wgpu teardown,
and the timer-based workaround currently used in `src/ipc.rs`.

---

## 1. Event Loop Functions: What They Do

### 1.1 `run_event_loop()` vs `run_event_loop_until_quit()`

`run_event_loop()` enters the main event loop, processing window system events
(user input, redraws). It returns when **either** the last window is closed
**or** `quit_event_loop()` is called. This is the default "quit on last window
closed" mode.

`run_event_loop_until_quit()` does the same but ignores window-close events for
the purpose of loop termination. The loop only returns when `quit_event_loop()`
is explicitly called. This is the mode used by sunlit-earth in tray mode, because
the window is hidden (not destroyed) when the user closes it.

Source: [docs.rs/slint run_event_loop](https://docs.rs/slint/latest/slint/fn.run_event_loop.html),
[internal/core/api.rs line ~1054](https://github.com/slint-ui/slint/blob/master/internal/core/api.rs)

### 1.2 Internal implementation of `run_event_loop`

In the winit backend (the one used on Windows/Linux/macOS), each call to
`run_event_loop` increments an `event_loop_generation` counter (stored in an
`Arc<AtomicUsize>`) before delegating to `winit`'s `run_app_on_demand`. This
counter is the mechanism that prevents stale exit signals from prior event loop
invocations from terminating a newly-started one.

The winit event loop instance itself is reused across calls to `run_event_loop`,
because winit does not support creating multiple event loop instances. After
`run_event_loop` returns, the loop object is stored back in `SharedBackendData`
for future reuse.

Source: [internal/backends/winit/lib.rs lines 797, 828](https://github.com/slint-ui/slint/blob/master/internal/backends/winit/lib.rs),
[internal/backends/winit/event_loop.rs lines 625-650](https://github.com/slint-ui/slint/blob/master/internal/backends/winit/event_loop.rs)

### 1.3 `quit_event_loop()`

**Documented behavior:**

> Schedules the main event loop for termination. This function is meant to be
> called from callbacks triggered by the UI. After calling the function, it will
> return immediately and once control is passed back to the event loop, the
> initial call to `slint::run_event_loop()` will return.
>
> This function can be called from any thread.
>
> **Any previously queued events may or may not be processed before the loop
> terminates. This is platform dependent behaviour.**

Source: [internal/core/api.rs lines 1052-1058](https://github.com/slint-ui/slint/blob/master/internal/core/api.rs)

**Implementation:** `quit_event_loop()` sends a `CustomEvent::Exit(generation)`
message to the winit event loop via `winit::event_loop::EventLoopProxy::send_event`.
The generation value is the current `event_loop_generation` counter. If the
counter does not match when the Exit event is processed, it is silently ignored
(this protects against sending a quit from a previous loop run).

When `CustomEvent::Exit(generation)` is processed in `user_event()`:

```rust
CustomEvent::Exit(generation) => {
    if self.shared_backend_data.event_loop_generation
        .load(std::sync::atomic::Ordering::Relaxed) == generation
    {
        self.suspend_all_hidden_windows();
        event_loop.exit()
    }
    // else ignore the event, since it's from a previous run of the event loop
}
```

Source: [internal/backends/winit/event_loop.rs lines 477-486](https://github.com/slint-ui/slint/blob/master/internal/backends/winit/event_loop.rs)

The `suspend_all_hidden_windows()` call (added to fix issue #8795) explicitly
suspends all hidden windows before exiting, ensuring GPU resources are released
cleanly before the winit loop terminates.

### 1.4 `invoke_from_event_loop()`

**Documented behavior:**

> Adds the specified function to an internal queue, notifies the event loop to
> wake up. Once woken up, any queued up functors will be invoked. This function
> is thread-safe and can be called from any thread, including the one running
> the event loop, but the provided functors will only be invoked from the thread
> that started the event loop.

Source: [docs.rs/slint invoke_from_event_loop](https://docs.rs/slint/latest/slint/fn.invoke_from_event_loop.html)

**Implementation:** Sends a `CustomEvent::UserEvent(Box<dyn FnOnce() + Send>)`
to the winit event loop proxy. The proxy then delivers it as a winit user event,
which is handled synchronously in `user_event()`:

```rust
CustomEvent::UserEvent(user_callback) => user_callback(),
```

This means the closure runs synchronously on the event loop thread, in the
`user_event` handler. The Slint comment in the source notes that on wasm32 an
extra `WakeEventLoopWorkaround` event is sent first, because `request_redraw`
doesn't wake the wasm event loop.

On the **error path**: if the winit proxy's `send_event` fails (loop is already
shut down), it returns `EventLoopError::EventLoopTerminated`.

Source: [internal/backends/winit/lib.rs lines 838-858](https://github.com/slint-ui/slint/blob/master/internal/backends/winit/lib.rs)

### 1.5 `EventLoopError` variants

```rust
pub enum EventLoopError {
    /// The event could not be sent because the event loop was terminated already
    EventLoopTerminated,
    /// The event could not be sent because the Slint platform abstraction was
    /// not yet initialized, or the platform does not support event loop.
    NoEventLoopProvider,
}
```

Source: [internal/core/api.rs lines 1062-1070](https://github.com/slint-ui/slint/blob/master/internal/core/api.rs)

---

## 2. Shutdown Sequence

### 2.1 The documented ordering guarantee (or lack thereof)

The documentation for `quit_event_loop()` explicitly states: **"Any previously
queued events may or may not be processed before the loop terminates. This is
platform dependent behaviour."**

This was clarified in [issue #6562](https://github.com/slint-ui/slint/issues/6562)
and [PR #8029](https://github.com/slint-ui/slint/pull/8029). The concern raised:
does calling `quit_event_loop()` after `invoke_from_event_loop()` guarantee the
closure runs before the loop exits? The answer is: **not guaranteed**. The Exit
event and the UserEvent are both queued to the same winit proxy channel, so they
will be processed in order in the winit event dispatch loop. However, if the Exit
event arrives first (or is processed first for platform-specific reasons), queued
closures may be dropped without execution.

### 2.2 Pending timers on shutdown

Timers in Slint are stored in a `thread_local! { RefCell<TimerList> }` called
`CURRENT_TIMERS` in `internal/core/timers.rs`. This is a per-thread structure
that only fires on the event loop thread.

`Timer::single_shot(duration, callback)` registers an entry in `CURRENT_TIMERS`
using `try_with` (safe for use during thread-local cleanup). The timer fires when
the event loop next processes timer events after `duration` has elapsed.

**Critical behavior:** There is no documented guarantee that pending timers fire
before the event loop exits. When `event_loop.exit()` is called (winit's internal
mechanism), the event loop stops dispatching all further events, including timer
checks. Pending timer callbacks that have not yet fired are simply dropped with
their closures when the thread-local `CURRENT_TIMERS` is destroyed at thread
cleanup.

Source: [internal/core/timers.rs lines 111-122, 188-200](https://github.com/slint-ui/slint/blob/master/internal/core/timers.rs)

### 2.3 What `invoke_from_event_loop` does after quit

If `quit_event_loop()` has already been called and the loop has terminated,
calling `invoke_from_event_loop()` returns `Err(EventLoopError::EventLoopTerminated)`.
The `winit::event_loop::EventLoopProxy::send_event` returns an error when the
event loop is no longer running, and Slint maps that to `EventLoopTerminated`.

If `quit_event_loop()` has been called but has **not yet been processed** (i.e.,
the Exit event is still in the queue), `invoke_from_event_loop()` will succeed
in sending its closure. Whether that closure runs before or after the Exit event
depends on the order the events were queued.

---

## 3. Slint wgpu Backend Integration

### 3.1 The `unstable-wgpu-28` feature

The `unstable-wgpu-28` Cargo feature exposes Slint's internal wgpu 28 types.
The Slint wgpu integration path for sunlit-earth is the `femtovg` renderer with
a wgpu backend (`internal/renderers/femtovg/wgpu.rs`). This is distinct from
the Skia/wgpu backend.

The `unstable-*` prefix signals that these APIs can break across Slint minor
versions because wgpu itself releases major versions frequently. The API types
exposed via `slint::wgpu_28` are re-exports of wgpu 28's types.

Source: [docs.slint.dev/latest/docs/rust/slint/wgpu_28](https://docs.slint.dev/latest/docs/rust/slint/wgpu_28/),
[slint::docs::cargo_features](https://docs.rs/slint/latest/slint/docs/cargo_features/index.html)

### 3.2 `set_rendering_notifier` lifecycle

The `set_rendering_notifier` callback receives a `RenderingState` enum value:

```rust
pub enum RenderingState {
    /// The window has been created and the graphics adapter/context initialized.
    RenderingSetup,
    /// The scene of items is about to be rendered.
    BeforeRendering,
    /// The scene of items was rendered, but the back buffer was not sent for
    /// display presentation yet (for example GL swap buffers).
    AfterRendering,
    /// The window will be destroyed and/or graphics resources need to be
    /// released due to other constraints.
    RenderingTeardown,
}
```

Source: [internal/core/api.rs lines 329-352](https://github.com/slint-ui/slint/blob/master/internal/core/api.rs)

The enum is `#[non_exhaustive]`, meaning new variants may be added in future
Slint releases without a major version bump.

### 3.3 When does `RenderingTeardown` fire?

In the femtovg renderer (`internal/renderers/femtovg/lib.rs`), teardown is
invoked from `clear_graphics_context()`, which is called from:

1. **`FemtoVGRenderer::drop()`** — when the renderer struct itself is dropped.
2. **`suspend()`** on `WinitWindowAdapter` — which is called when a window is
   hidden on Wayland (where Wayland doesn't support hiding windows, only
   destroying them) or when `SLINT_DESTROY_WINDOW_ON_HIDE=1` is set.
3. **`suspend_all_hidden_windows()`** — called from the `CustomEvent::Exit`
   handler just before `event_loop.exit()`.

The teardown condition inside `clear_graphics_context()`:

```rust
// If we've rendered a frame before, then we need to invoke the
// RenderingTearDown notifier.
if !self.rendering_first_time.get()
    && api.is_some()
    && let Some(callback) = self.rendering_notifier.borrow_mut().as_mut()
{
    self.with_graphics_api(|api| {
        callback.notify(RenderingState::RenderingTeardown, &api)
    })
    .ok();
}
```

Source: [internal/renderers/femtovg/lib.rs lines 466-540](https://github.com/slint-ui/slint/blob/master/internal/renderers/femtovg/lib.rs)

**Key implication:** `RenderingTeardown` only fires if at least one frame has
been rendered (`rendering_first_time` is false). If the app quits before the
first render (e.g., immediately on startup), teardown is skipped.

### 3.4 When does teardown fire relative to `quit_event_loop()`?

For a **visible** window on Windows (the sunlit-earth case in tray mode):

1. `quit_event_loop()` queues `CustomEvent::Exit(generation)`.
2. The winit event loop processes the Exit event in `user_event()`.
3. `suspend_all_hidden_windows()` is called — but this only affects **hidden**
   windows, not visible ones.
4. `event_loop.exit()` is called on winit's `ActiveEventLoop`.
5. Winit stops its event dispatch loop and returns from `run_app_on_demand`.
6. The `EventLoopState` goes out of scope; its `Drop` is invoked.
7. `SharedBackendData` and `WinitWindowAdapter` are dropped.
8. `WinitWindowAdapter::drop()` calls `unregister_window()`.
9. The renderer (`FemtoVGRenderer`) is dropped, which calls
   `clear_graphics_context()`, which fires `RenderingTeardown`.

For a **hidden** window:

- `suspend_all_hidden_windows()` calls `suspend()` on the adapter before the
  loop exits, which calls `renderer.suspend()`, triggering teardown earlier —
  while the winit event loop is still technically "running".

**Important:** For a visible window, teardown happens as part of normal Rust
`Drop` chain after `run_app_on_demand` returns, not inside the event loop itself.
The thread that runs the event loop is still the main thread, so the Drop runs
on the correct thread.

Source: [internal/backends/winit/winitwindowadapter.rs lines 573-595, 1560-1567](https://github.com/slint-ui/slint/blob/master/internal/backends/winit/winitwindowadapter.rs),
[internal/backends/winit/event_loop.rs lines 477-486, 111-128](https://github.com/slint-ui/slint/blob/master/internal/backends/winit/event_loop.rs)

---

## 4. Threading Concerns

### 4.1 Slint's threading model

Slint is fundamentally single-threaded for UI operations. The event loop must
run on one dedicated thread (in practice always the main thread on Windows,
due to Win32 requirements). All Slint API calls that touch UI state must happen
on the event loop thread.

The thread-safety guarantees provided:

- `invoke_from_event_loop(closure)` is the **only** safe cross-thread path into
  the event loop. It is `Send + 'static`.
- `slint::Weak<T>` handles are `Send`, allowing background threads to hold
  references that are then upgraded on the event loop thread.
- `quit_event_loop()` is explicitly documented as callable from any thread.
- Timers are **not thread-safe**: they can only be created and used on the
  event loop thread.

Source: [docs.rs invoke_from_event_loop](https://docs.rs/slint/latest/slint/fn.invoke_from_event_loop.html),
[docs.rs Timer](https://docs.rs/slint/latest/slint/struct.Timer.html)

### 4.2 `invoke_from_event_loop` called after `quit_event_loop`

If `invoke_from_event_loop()` is called from a background thread after
`quit_event_loop()` has been successfully processed (loop has exited),
`send_event` on the winit proxy fails and Slint returns
`Err(EventLoopError::EventLoopTerminated)`.

If called while the quit is still in-flight (queued but not processed), the
call succeeds and returns `Ok(())`, but the closure's execution before loop
termination depends on the order of events in the queue.

### 4.3 Background threads holding `slint::Weak` during shutdown

`slint::Weak<T>` is backed by a weak reference to the component's internal
`VRc`. If the component is dropped (which happens as part of the shutdown Drop
chain), `weak.upgrade()` returns `None`. The IPC thread in sunlit-earth holds
a `slint::Weak<MainWindow>` — after shutdown, any attempt to call
`invoke_from_event_loop` from that thread will either:

- Succeed in sending but have the weak ref fail to upgrade (safe), or
- Return `Err(EventLoopTerminated)` (safe, `.ok()` swallows it).

There is no crash risk from this pattern itself. The IPC thread loops on
`listener.incoming()`, which will block until the process exits (at which point
the thread is killed with the process).

---

## 5. Known Issues and Relevant History

### 5.1 Issue #8795: Skia-OpenGL resources not freed on window close (Windows)

**[Closed/Fixed in Slint 1.13]**

Python applications using ListView would enter a 100% CPU spin and fail to
terminate after closing a window using the `winit-skia-opengl` renderer on
Windows. The root cause was that GPU resources were being freed too late —
during thread-local cleanup after the event loop had already exited — rather
than synchronously within the event loop.

The fix: `suspend_all_hidden_windows()` was added to the `CustomEvent::Exit`
handler, so hidden windows have their GPU resources freed **before**
`event_loop.exit()` is called, while the winit event loop is still active
and the graphics context is valid.

Setting `SLINT_DESTROY_WINDOW_ON_HIDE=1` is a documented workaround.

Source: [github.com/slint-ui/slint/issues/8795](https://github.com/slint-ui/slint/issues/8795)

### 5.2 Issue #6562: `quit_event_loop()` documentation ambiguity

**[Closed]**

The concern: does calling `invoke_from_event_loop()` then `quit_event_loop()`
guarantee the closure runs first? The documented answer is: **no guarantee**,
it is platform-dependent. Both events are sent to the same winit proxy queue
and will be processed in order, but "platform dependent" means the loop could
exit before processing all pending user events on some platforms or conditions.

Source: [github.com/slint-ui/slint/issues/6562](https://github.com/slint-ui/slint/issues/6562)

### 5.3 Issue #5534: Crash on shutdown on Wayland

**[Fixed]**

Valgrind reported invalid reads during shutdown because the Wayland clipboard
was being destroyed while the display handle was still referenced. The fix
ensured proper lifetime ordering of platform-specific resources relative to
the event loop teardown.

Source: [github.com/slint-ui/slint/issues/5534](https://github.com/slint-ui/slint/issues/5534)

### 5.4 Issue #6627: `quit_event_loop()` causes Qt timer errors

**[Closed/Needs Info]**

When using the Qt backend, calling `quit_event_loop()` from `on_close_requested`
while a background thread runs `upgrade_in_event_loop`, Qt logs:
`QObject::~QObject: Timers cannot be stopped from another thread`. This is a
Qt-backend-specific issue due to Qt's own threading requirements. Not applicable
to the winit backend used by sunlit-earth.

Source: [github.com/slint-ui/slint/issues/6627](https://github.com/slint-ui/slint/issues/6627)

### 5.5 `STATUS_STACK_BUFFER_OVERRUN` on Windows

`STATUS_STACK_BUFFER_OVERRUN` (Windows exception code `0xC0000409`) has a
misleading name. Despite the name, it does **not** always indicate a stack buffer
overflow. According to Raymond Chen (Microsoft): "it just means the application
decided to terminate itself with great haste" — it is the error raised when
Windows detects that the security cookie on a function's stack frame has been
corrupted, which can be triggered by:

- A panic during thread-local `Drop` (the primary Rust-specific cause, documented
  on the Rust Users Forum)
- Structured Exception Handling (SEH) unwinding through a function without a
  proper SEH handler
- Corrupted stack frames from C FFI crossing into Rust code

In the context of Slint + wgpu on Windows, this error has been observed when
GPU resources are dropped in the wrong state (e.g., when the DX12 or Vulkan
backend performs work during resource destruction after the swap chain has been
invalidated).

Source:
[devblogs.microsoft.com/oldnewthing/20190108](https://devblogs.microsoft.com/oldnewthing/20190108-00/?p=100655),
[users.rust-lang.org STATUS_STACK_BUFFER_OVERRUN](https://users.rust-lang.org/t/status-stack-buffer-overrun-on-windows-without-any-usage-of-unsafe/128417),
[bevy issue #22440](https://github.com/bevyengine/bevy/issues/22440)

---

## 6. The `thread_local` `GpuResources` Pattern in sunlit-earth

Sunlit-earth stores GPU resources in:

```rust
thread_local! {
    static GPU_RESOURCES: RefCell<Option<GpuResources>> = RefCell::new(None);
}
```

This is populated during `RenderingSetup` and dropped during `RenderingTeardown`.
The rendering notifier callback itself is a `'static` closure that was registered
with `set_rendering_notifier`.

**Relationship to shutdown:**

1. `RenderingTeardown` fires during the Drop chain of `WinitWindowAdapter` (for
   a visible window), which happens after `run_app_on_demand` returns.
2. At that point, the event loop thread is no longer inside the winit event loop
   but is still the same thread, so `thread_local` access is valid.
3. The rendering notifier callback takes a mutable borrow of the closure that
   was registered. Inside that closure, `GPU_RESOURCES.borrow_mut().take()` is
   called to drop the GPU resources.
4. The wgpu `Device`, `Queue`, textures, and buffers are dropped in this
   sequence — all on the event loop thread.

**What the comment in `src/ipc.rs` describes:**

```rust
// Defer quit_event_loop() to a single-shot timer so it fires
// during a clean event loop iteration. Calling quit_event_loop()
// directly from invoke_from_event_loop crashes the wgpu backend
// on Windows (STATUS_STACK_BUFFER_OVERRUN). The timer approach
// matches how the render subcommand shuts down successfully.
```

The pattern used:

```rust
slint::invoke_from_event_loop(|| {
    slint::Timer::single_shot(std::time::Duration::ZERO, || {
        slint::quit_event_loop().ok();
    });
}).ok();
```

This uses two levels of deferred execution:

1. `invoke_from_event_loop` moves execution onto the event loop thread.
2. `Timer::single_shot(Duration::ZERO, ...)` defers the actual quit to the
   **next** event loop iteration after the current `user_event` handler returns.

---

## 7. Analysis: Why Direct `quit_event_loop` from `invoke_from_event_loop` Crashed

Based on the source code analysis and documentation, the likely cause chain is:

**Call sequence that crashed:**

```text
IPC thread
  -> invoke_from_event_loop(|| quit_event_loop())
     -> sends UserEvent to winit proxy
     -> winit processes UserEvent in user_event()
        -> runs the closure: quit_event_loop()
           -> sends Exit event to winit proxy
        -> user_event() returns
     -> winit processes Exit event in user_event()
        -> suspend_all_hidden_windows()
        -> event_loop.exit()
     -> run_app_on_demand() returns
  -> WinitWindowAdapter is dropped
  -> FemtoVGRenderer is dropped
  -> clear_graphics_context() fires RenderingTeardown
  -> GPU_RESOURCES.borrow_mut().take() drops GpuResources
     -> wgpu Device, Queue, textures dropped
```

The crash point: dropping wgpu resources while the winit event loop is exiting
or immediately after, in a state where the DX12/Vulkan backend is not in a clean
idle state. The `user_event()` call stack may still be partially unwound when
the Exit event processing starts, meaning the wgpu surface is in an
indeterminate state.

**Why the timer approach works:**

```text
IPC thread
  -> invoke_from_event_loop(|| Timer::single_shot(0, || quit_event_loop()))
     -> sends UserEvent to winit proxy
     -> winit processes UserEvent in user_event()
        -> registers a zero-duration timer in CURRENT_TIMERS
        -> user_event() returns cleanly
     -> winit returns to its normal event dispatch loop
     -> winit checks timers (Duration::ZERO has already elapsed)
     -> timer callback fires: quit_event_loop()
        -> sends Exit event
     -> winit processes Exit event
        -> suspend_all_hidden_windows()
        -> event_loop.exit()
     -> run_app_on_demand() returns
  -> (same Drop chain as above, but from a clean stack)
```

The key difference: when using the timer, `quit_event_loop()` is called from a
**timer callback context** — the same context used by normal application code
(button callbacks, timer callbacks). At this point, the wgpu surface has
completed its current frame, the command queues are idle, and the DX12/Vulkan
state machine is in a stable state.

The direct approach calls `quit_event_loop()` synchronously inside a
`user_event()` handler — an event type that can be dispatched at any point in
the winit event processing loop, potentially mid-frame or during a resize event.
This may leave the wgpu backend in a state where it is unsafe to destroy.

**The `STATUS_STACK_BUFFER_OVERRUN` symptom:**

This is consistent with a panic during `Drop` — for instance, if wgpu's DX12
backend attempts to signal a fence or release a resource while the device is in
a state where that operation triggers a Windows Structured Exception. The
Windows SEH-to-Rust panic conversion can produce this exception code when the
stack is unwound across a boundary that does not have proper SEH handlers (as
can happen when Rust code calls into the DX12 COM layer).

---

## 8. Implications for sunlit-earth

### 8.1 The current workaround is sound

The timer-based approach in `src/ipc.rs` correctly defers `quit_event_loop()` to
a clean event loop iteration. This is the same pattern used internally by other
parts of the Slint ecosystem and is consistent with how `render` subcommand
shutdown works in sunlit-earth.

### 8.2 No documented guarantee on timer-before-exit ordering

There is no Slint documentation guaranteeing that a zero-duration timer will
fire before the event loop exits. However, in practice:

- The timer is registered during a `user_event()` call.
- After `user_event()` returns, winit processes its next event.
- The `Exit` event has not yet been sent (it's sent by the timer callback).
- Therefore, winit will process normal events (including timer checks) before
  receiving the Exit.
- A zero-duration timer fires on the next timer check, which happens at the
  next event loop iteration.

This ordering is reliable in practice but is not formally guaranteed by the API.

### 8.3 `RenderingTeardown` and `GPU_RESOURCES` cleanup

For a visible window (the typical sunlit-earth tray mode case), `RenderingTeardown`
fires in the Drop chain after `run_app_on_demand` returns, on the main thread.
This is a valid context for dropping wgpu resources. The `thread_local`
`GPU_RESOURCES` is accessed on the correct thread.

For a window that was hidden before quit (unlikely in tray mode, but possible
if the user hides it then quits via IPC), the teardown sequence depends on
whether `suspend_all_hidden_windows` fires the teardown notifier before or
after the event loop exits. Based on the source, `suspend()` calls
`renderer.suspend()` which calls `clear_graphics_context()` which fires
`RenderingTeardown` — this happens **within** the Exit event handler, while
the event loop is still active. This is an even safer context for GPU
resource cleanup.

### 8.4 Post-shutdown IPC traffic

If the IPC thread receives commands after the event loop has exited,
`invoke_from_event_loop()` will return `Err(EventLoopTerminated)`, which is
silently discarded via `.ok()`. The IPC thread will then block on
`listener.incoming()` until the process terminates. This is correct behavior.

### 8.5 The `Weak` reference

The `window_weak: slint::Weak<MainWindow>` held by the IPC thread is safe to
hold across shutdown. After the main window's `Drop` completes, all calls to
`weak.upgrade()` will return `None`, which is handled correctly in
`show-window` and `hide-window` commands. There is no crash risk here.

---

## 9. Sources

- [docs.rs/slint `invoke_from_event_loop`](https://docs.rs/slint/latest/slint/fn.invoke_from_event_loop.html)
- [docs.rs/slint `run_event_loop`](https://docs.rs/slint/latest/slint/fn.run_event_loop.html)
- [docs.rs/slint `EventLoopError`](https://docs.rs/slint/latest/slint/enum.EventLoopError.html)
- [docs.rs/slint `Timer`](https://docs.rs/slint/latest/slint/struct.Timer.html)
- [Slint source: internal/core/api.rs](https://github.com/slint-ui/slint/blob/master/internal/core/api.rs)
- [Slint source: internal/backends/winit/event_loop.rs](https://github.com/slint-ui/slint/blob/master/internal/backends/winit/event_loop.rs)
- [Slint source: internal/backends/winit/lib.rs](https://github.com/slint-ui/slint/blob/master/internal/backends/winit/lib.rs)
- [Slint source: internal/backends/winit/winitwindowadapter.rs](https://github.com/slint-ui/slint/blob/master/internal/backends/winit/winitwindowadapter.rs)
- [Slint source: internal/renderers/femtovg/lib.rs](https://github.com/slint-ui/slint/blob/master/internal/renderers/femtovg/lib.rs)
- [Slint source: internal/core/timers.rs](https://github.com/slint-ui/slint/blob/master/internal/core/timers.rs)
- [Slint source: internal/core/platform.rs](https://github.com/slint-ui/slint/blob/master/internal/core/platform.rs)
- [Slint issue #8795: Python app fails to terminate (skia-opengl, Windows)](https://github.com/slint-ui/slint/issues/8795)
- [Slint issue #6562: Improve `quit_event_loop()` documentation](https://github.com/slint-ui/slint/issues/6562)
- [Slint issue #5534: Fix crash on app shutdown on Wayland](https://github.com/slint-ui/slint/issues/5534)
- [Slint issue #6627: `quit_event_loop()` causes QObject Timers error](https://github.com/slint-ui/slint/issues/6627)
- [Slint docs: RenderingState enum (master)](https://snapshots.slint.dev/master/docs/rust/slint/enum.renderingstate)
- [Slint docs: Backends and Renderers](https://docs.slint.dev/latest/docs/slint/guide/backends-and-renderers/backends_and_renderers/)
- [Slint docs: slint::wgpu_28](https://docs.slint.dev/latest/docs/rust/slint/wgpu_28/)
- [Raymond Chen (Microsoft): STATUS_STACK_BUFFER_OVERRUN meaning](https://devblogs.microsoft.com/oldnewthing/20190108-00/?p=100655)
- [Rust Users Forum: STATUS_STACK_BUFFER_OVERRUN in safe Rust](https://users.rust-lang.org/t/status-stack-buffer-overrun-on-windows-without-any-usage-of-unsafe/128417)
- [Bevy issue #22440: STATUS_STACK_BUFFER_OVERRUN with wgpu on Windows](https://github.com/bevyengine/bevy/issues/22440)
