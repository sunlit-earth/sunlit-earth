# Wallpaper Scheduler - Consolidated Research

Date: 2026-03-28

## Overview

Research for adding automatic wallpaper update scheduling
to Sunlit Earth, including UI controls, tray menu
integration, and the tray-start-hidden edge case.

## Feature Requirements

1. **Timer**: Auto-update wallpaper on configurable
   interval (1-30 min, default 5)
2. **UI**: Checkbox to enable, slider for interval
3. **Startup**: If enabled, update wallpaper on launch
4. **Tray menu**: "Refresh Now" and checkable
   "Auto-refresh" items
5. **Edge case**: Handle tray mode with hidden window

## Existing Patterns to Reuse

### Timer Pattern (src/main.rs:269-281)

The sun position timer uses `slint::Timer`:

```rust
let timer = slint::Timer::default();
timer.start(
    slint::TimerMode::Repeated,
    Duration::from_secs(120),
    move || { /* callback */ },
);
std::mem::forget(timer); // avoid destructor crash
```

Timers fire even when the window is hidden, as long as
`run_event_loop_until_quit()` is used.

### Wallpaper Export (src/ui_callbacks.rs:373-381)

`do_set_wallpaper()` is public and handles the full
pipeline: monitor resolution, GPU render, PNG encode,
Win32 wallpaper apply. The scheduler calls this directly.

### Config (src/config.rs:28-90)

`AppConfig` uses `#[serde(default)]` for forward
compatibility. New fields get defaults automatically.
Config saves only on "Set as Wallpaper" click.
Bridge functions: `apply_config_to_window()` and
`read_config_from_window()`.

### Tray Menu (src/tray.rs)

Menu built with `muda` (re-exported by `tray-icon`).
Background thread dispatches to Slint via
`slint::invoke_from_event_loop()`.

### UI Pattern (ui/main.slint)

Checkbox + slider combos exist in Date/Time and
Atmosphere sections. Window properties use `in-out`
with `<=>` slider bindings.

## Critical Finding: Hidden Window GPU Init

**GPU resources require a visible window.** The rendering
pipeline initializes during `RenderingSetup`, which only
fires when the window is shown. Key constraints:

- `export_wallpaper_image()` needs `GPU_RESOURCES`,
  `last_state`, and `last_shading` -- all populated
  only after `RenderingSetup` + `BeforeRendering`
- `window.hide()` destroys the native window and
  triggers `RenderingTeardown`
- No headless rendering path exists

### Current Deferred Hide Pattern

For `--tray-start hidden`:

1. Window IS shown at startup (`src/main.rs:377`)
2. Zero-duration timer defers hide to after event loop
   starts (`src/main.rs:382-391`)
3. `RenderingSetup` fires before the hide
4. After hide, `RenderingTeardown` cleans up GPU

### Recommended Approach for Initial Update

When auto-update is enabled and starting in hidden tray
mode, we need the window visible long enough for GPU
init + first render + wallpaper export:

1. Show window (triggers `RenderingSetup`)
2. Use a short deferred timer (e.g., 500ms) to allow
   `BeforeRendering` to fire and render a frame
3. In the deferred callback: call `do_set_wallpaper()`,
   then hide the window
4. Subsequent scheduled updates: show window briefly,
   render, export, hide again

Alternative: keep window visible for the first render
cycle, export wallpaper in `BeforeRendering` on the
first frame (but this couples renderer to wallpaper
logic).

**Simplest approach:** Delay the deferred hide. Instead
of hiding immediately after event loop starts, wait for
the first frame to render, export wallpaper, then hide.
The existing deferred-hide timer already fires after
`RenderingSetup`. We just need to wait one more frame.

## Design Decisions

### Timer Management

The scheduler timer needs to be restartable (interval
changes) and stoppable (checkbox toggle). Unlike the
sun timer which is fire-and-forget, the scheduler timer
must be mutable. Options:

- **Rc<slint::Timer>**: Store timer in an Rc so
  callbacks can restart it. Timer stays alive via Rc,
  no need for `mem::forget`.
- **Thread-local**: Like GPU_RESOURCES. Overkill.

Recommended: `Rc<slint::Timer>` shared between the
change callback (restarts timer) and the timer callback
itself.

### Config Save Timing

Currently config only saves on "Set as Wallpaper".
The scheduler settings should also save when toggled,
since they affect background behavior. Options:

- Save config whenever scheduler settings change
- Save config on wallpaper export (scheduler already
  triggers this)
- Keep current behavior (only explicit save)

Recommended: Save config when the auto-update checkbox
is toggled or interval changes, since these settings
must persist across restarts.

### Tray Menu State Sync

The tray "Auto-refresh" checkmark must stay in sync with
the UI checkbox. Since the tray runs on a background
thread, state sync uses `invoke_from_event_loop()` to
read/write the window property.

### UI Placement

Scheduler controls belong in the always-visible
top section near "Set as Wallpaper" since they're
the primary way users interact with wallpaper updates.

## Architecture Summary

```text
UI Checkbox/Slider
  -> Slint property change
  -> Rust callback: restart/stop timer, save config

Timer fires
  -> do_set_wallpaper()
  -> update status text

Tray "Refresh Now"
  -> invoke_from_event_loop
  -> do_set_wallpaper()

Tray "Auto-refresh" toggle
  -> invoke_from_event_loop
  -> toggle window property
  -> restart/stop timer, save config

Hidden startup with auto-update
  -> show window (GPU init)
  -> deferred timer: render frame, export, hide
```

## Files to Modify

1. `src/config.rs` -- add `auto_update_enabled`,
   `auto_update_interval_minutes`
2. `ui/main.slint` -- add scheduler UI controls
3. `src/ui_callbacks.rs` -- add scheduler callbacks,
   extend `apply_config_to_window` /
   `read_config_from_window`
4. `src/main.rs` -- create scheduler timer in
   `run_event_loop()`, handle hidden startup
5. `src/tray.rs` -- add "Refresh Now" and
   "Auto-refresh" menu items

## Open Questions

1. Should the scheduler also save config before each
   wallpaper export? Currently `do_set_wallpaper()`
   doesn't save config -- the "Set as Wallpaper"
   callback does that separately.
2. For hidden-window updates: should we show a brief
   flash of the window, or try to minimize visibility
   (e.g., move off-screen before showing)?
3. Should the interval slider show minutes only, or
   also allow sub-minute intervals for testing?

## Source Documents

- [Codebase research](2026-03-28-wallpaper-scheduler-codebase.md)
- [Tray edge cases](2026-03-28-wallpaper-scheduler-tray-edge-cases.md)
