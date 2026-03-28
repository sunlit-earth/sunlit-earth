# Plan: Fix Tray Mode by Switching to `run_event_loop_until_quit()`

## Summary

Switch Sunlit Earth from `run_event_loop()` to
`run_event_loop_until_quit()` and adopt the Slint-recommended tray
app pattern, removing all workarounds that were adopted to
compensate for the wrong event loop function.

## Stakes Classification

**Level**: High

**Rationale**: This changes the core event loop, close behavior,
IPC dispatch, and quit mechanism. Every code path through
`run_event_loop()` in `main.rs` is affected. The tray thread,
IPC module, and e2e tests all need updates. However, the minimal
reproduction project (`slint-tray-minimal`) has proven the target
pattern works with Manual wgpu config, heavy rendering, and
background threads — so the risk is well-understood.

## Research

- `docs/plans/2026-03-28-minimal-vs-sunlit-comparison.md`
- `docs/plans/2026-03-28-research-slint-tray-app-patterns.md`
- `docs/plans/2026-03-28-hypothesis-verification.md`
- Working reference implementation:
  `C:\Workspace\rustrover\slint-tray-minimal\`

## Root Cause (confirmed)

`run_event_loop()` exits when the last window is hidden. When tray
mode hides the window (via `HideWindow` or `window.hide()`), the
event loop terminates entirely. All workarounds in the current code
were downstream consequences:

- Off-screen 1x1 positioning instead of `window.hide()` — avoids
  triggering "last window closed"
- `KeepWindowShown` instead of `HideWindow` — avoids closing
- Shared command queue + polling timer instead of
  `invoke_from_event_loop` — works around event loop exiting
- `process::exit(0)` in IPC quit — works around event loop already
  having exited
- Deferred shrink via `Timer::single_shot` for `--tray-start hidden`
  — works around fragile startup sequencing

## Target Architecture

The Slint-recommended pattern, proven in `slint-tray-minimal`:

- `run_event_loop_until_quit()` keeps the event loop alive after
  all windows are hidden
- `CloseRequestResponse::HideWindow` from `on_close_requested`
- `window.hide()` / `window.show()` for visibility
- `invoke_from_event_loop` for all cross-thread communication
  (IPC and tray)
- Direct `quit_event_loop()` inside `invoke_from_event_loop` for
  quit

## Success Criteria

- [ ] `cargo test` passes (unit tests)
- [ ] `cargo clippy` clean
- [ ] `cargo test --test e2e -- --ignored` passes (all 5 e2e tests)
- [ ] Manual: tray icon appears, window hides on close, tray
  "Open" shows window, tray "Exit" exits cleanly
- [ ] Manual: `--tray-start hidden` starts with window hidden,
  tray "Open" shows it

## Implementation Steps

### Step 1: Rewrite `ipc.rs` to use `invoke_from_event_loop`

**Files**: `src/ipc.rs`

Remove the `CommandQueue` struct, the `start_command_timer` function,
and the `IpcCommand` enum. Replace with a single
`spawn_ipc_listener(socket_name, window_weak)` function that calls
`invoke_from_event_loop` directly from the listener thread.

Commands:

- `quit` — `invoke_from_event_loop(|| quit_event_loop())`
- `show-window` — `invoke_from_event_loop(move || win.show())`
  plus `println!("SIGNAL:window_shown")`
- `hide-window` — `invoke_from_event_loop(move || win.hide())`
  plus `println!("SIGNAL:window_hidden")`

Add `debug!` logging for each command received and dispatched.

Note: the `quit` handler currently uses `process::exit(0)` which
bypasses the event loop entirely. With `run_event_loop_until_quit()`,
we can use the clean `quit_event_loop()` path instead. This means
the code after `run_event_loop_until_quit()` in `main.rs` will
actually execute now (geometry save, memory log, etc.).

**Verify**: `cargo build`

### Step 2: Update `tray.rs` to use `window.show()` directly

**Files**: `src/tray.rs`

The tray module already uses `invoke_from_event_loop` — no
structural change needed. But the "Open" handler currently calls
`win.show()` which is correct, while the "Exit" handler calls
`quit_event_loop()` also correctly.

Changes:

- Add `debug!` log before each `invoke_from_event_loop` call so
  we can see tray events in the log even if the event loop
  dispatch fails.
- The tray "Open" handler should also call
  `win.window().set_visible(true)` or `win.show()` — verify this
  is already the case (it is, the current code calls `win.show()`).

Actually, reviewing `tray.rs` more carefully: it already follows
the recommended pattern. The only issue was that
`invoke_from_event_loop` appeared to deadlock — but that was
because `run_event_loop()` had already exited after hide, so the
closure was never dispatched. With `run_event_loop_until_quit()`,
`invoke_from_event_loop` will work correctly.

**Changes needed**:

- Add `debug!` log in the menu event handler before calling
  `invoke_from_event_loop`, so we can distinguish "tray event
  fired" from "event loop dispatched the closure".

**Verify**: `cargo build`

### Step 3: Rewrite `run_event_loop()` in `main.rs`

**Files**: `src/main.rs`

This is the core change. Replace the entire `run_event_loop`
function body:

1. **Event loop function**: Change `slint::run_event_loop()` to
   `slint::run_event_loop_until_quit()`.

2. **Close handler (tray mode)**: Replace the `KeepWindowShown` +
   1x1 shrink + geometry save with:

   ```rust
   window.window().on_close_requested(move || {
       // Save geometry before hiding.
       if let Some(win) = window_weak.upgrade() {
           let size = win.window().size();
           let pos = win.window().position();
           config::save_window_geometry(pos.x, pos.y, size.width, size.height);
       }
       debug!("main window hidden (minimized to tray)");
       slint::CloseRequestResponse::HideWindow
   });
   ```

3. **IPC setup**: Change from `CommandQueue` + `start_command_timer`
   to just `spawn_ipc_listener(&name, window.as_weak())`. No timer
   needed.

4. **`--tray-start hidden`**: Replace the deferred
   `Timer::single_shot` shrink with:

   ```rust
   if use_tray && matches!(tray_start, TrayStart::Hidden) {
       window.hide().expect("Failed to hide window");
   }
   ```

   This works because `run_event_loop_until_quit()` stays alive
   even with no visible windows.

5. **Post-event-loop cleanup**: Remove `process::exit(0)`.
   `run_event_loop_until_quit()` returns cleanly after
   `quit_event_loop()`, so we can save geometry and exit normally.
   Remove `mem::forget` for timers — let them drop naturally.
   However: test whether timer Drop panics after
   `quit_event_loop()` and add `mem::forget` back ONLY if needed.

6. **Windowed mode**: For `--mode window`, keep using
   `run_event_loop()` (the current behavior is correct — close
   exits). Or switch to `run_event_loop_until_quit()` with a close
   handler that calls `quit_event_loop()` instead of just closing.
   The latter is simpler (one event loop function for all modes).
   Let's use `run_event_loop_until_quit()` for all modes:
   - Tray mode: `on_close_requested` returns `HideWindow`
   - Windowed mode: `on_close_requested` calls `quit_event_loop()`
     and returns `KeepWindowShown` (keeps the window alive long
     enough for the quit to process)
   - Render mode: no close handler (the render timer calls
     `quit_event_loop()` when done)

7. **Logging**: Add `info!` log for:
   - Event loop function chosen (`run_event_loop_until_quit`)
   - Close request handler installed (tray vs windowed)
   - Event loop started
   - Event loop exited (after `run_event_loop_until_quit()` returns)
   - IPC listener spawned
   - Tray thread spawned

**Verify**: `cargo build`, `cargo clippy`

### Step 4: Update e2e tests

**Files**: `tests/e2e.rs`

The e2e tests need updates because the IPC protocol has changed:

1. **`test_tray_mode_ipc_lifecycle`**: Currently uses
   `--tray-start hidden` which starts with a 1x1 window. After the
   fix, it starts with the window actually hidden. The test sends
   `show-window` → waits for `SIGNAL:window_shown` → waits for
   `first_frame_rendered` → sends `hide-window` → waits for
   `SIGNAL:window_hidden` → sends `quit`. This flow should work
   as-is since the signals are now emitted from
   `invoke_from_event_loop` closures.

   Change: remove the `SUNLIT_EARTH_NO_CLOUDS` env var dependency
   for `invoke_from_event_loop` (the comment says it's needed to
   avoid deadlock — no longer true). Actually, keep
   `SUNLIT_EARTH_NO_CLOUDS` to avoid network access in tests, but
   update the comment to reflect the real reason (no network).

2. **`test_tray_hide_show_cycle`**: This test was specifically
   written to verify the off-screen workaround. After the fix, it
   tests the real `window.hide()` / `window.show()` path. The
   test flow is the same, but the actual mechanism is cleaner.

   Change: update the comments (remove references to "off-screen
   positioning").

3. **`test_windowed_mode_graceful_shutdown`**: The `quit` command
   now goes through `invoke_from_event_loop` → `quit_event_loop()`
   instead of `process::exit(0)`. The test expects exit code 0 and
   the "exiting" log message. This should still work, but the
   process exit path is different (clean return vs abort).

   Change: verify the test still passes. The "exiting" debug log
   must still be emitted after the event loop exits.

4. **`test_single_instance_second_exits`**: No change needed — this
   test doesn't interact with the event loop or IPC protocol, it
   just checks that the second instance detects the mutex and exits.

5. **`test_render_and_exit`**: No change needed — render mode
   doesn't use tray.

6. **Update comments**: Remove all references to
   `invoke_from_event_loop` deadlocking, command queue workaround,
   off-screen positioning, etc.

**Verify**: `cargo test --test e2e -- --ignored`

### Step 5: Clean up stale comments and documentation

**Files**: `src/main.rs`, `src/ipc.rs`, `src/tray.rs`,
`src/cloud_fetcher.rs`, `CLAUDE.md`

Remove all comments and documentation that reference:

- `invoke_from_event_loop` deadlocking
- The command queue workaround
- Off-screen positioning / 1x1 shrink workaround
- `process::exit(0)` as an IPC quit mechanism
- `run_event_loop()` vs `run_event_loop_until_quit()` trade-offs
  (the decision is now made)
- `SUNLIT_EARTH_NO_CLOUDS` being needed to avoid
  `invoke_from_event_loop` deadlock (update to say it's for
  network access avoidance)

Update `CLAUDE.md`:

- Update the `ipc.rs` module description (no more command queue)
- Update the `main.rs` module description (event loop function)
- Remove mentions of `SUNLIT_EARTH_SYNC_LOG` if it's no longer
  needed (it was a test workaround for pipe congestion — check if
  still useful)

**Verify**: `cargo clippy`, read through diffs

### Step 6: Run full verification

**Files**: none (verification only)

1. `cargo test` — all unit tests
2. `cargo clippy` — no warnings
3. `cargo test --test e2e -- --ignored` — all 5 e2e tests
4. Manual testing:
   - Run app, close window → hides to tray, tray icon present
   - Click tray "Open" → window reappears
   - Click tray "Exit" → clean exit
   - `--tray-start hidden` → no window, tray icon, "Open" shows
   - `--mode window` → close exits immediately

## Risks and Mitigations

**Timer Drop panics after `quit_event_loop()`**
Impact: Crash on exit.
Mitigation: Test, add `mem::forget` only if needed.

**`window.hide()` triggers `RenderingTeardown`**
Impact: GPU resources lost, re-show fails.
Mitigation: Research says this does NOT happen on winit/Windows
without `SLINT_DESTROY_WINDOW_ON_HIDE`; confirmed in minimal
project.

**Cloud fetcher's `upgrade_in_event_loop` deadlocks**
Impact: Clouds never load.
Mitigation: Test with clouds enabled; should work now since
the event loop stays alive.

**`--tray-start hidden` prevents `RenderingSetup` from firing**
Impact: No GPU resources until first show.
Mitigation: Test: show via tray, verify first frame renders.

**Window geometry save fails after `quit_event_loop()`**
Impact: Last geometry lost.
Mitigation: Test by checking config file after quit.

## Rollback Strategy

All changes are in `src/main.rs`, `src/ipc.rs`, `src/tray.rs`,
`tests/e2e.rs`, and `CLAUDE.md`. If the changes cause regressions,
`git revert` the commit.

## Status

- [ ] Plan approved
- [ ] Implementation started
- [ ] Implementation complete
