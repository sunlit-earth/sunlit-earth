# Research: System Tray Icon (2026-03-21)

## Problem Statement

Sunlit Earth currently runs as a windowed desktop application that exits when the window is closed. The goal is to add system tray icon support so the app can run as a lightweight background process: show the main window and tray icon on first start, minimize to tray on close, and on subsequent starts show only the tray icon. When hidden, the app should consume minimal resources (no GPU, no window RAM). The tray icon should also support auto-refresh wallpaper at a configurable interval (1--30 minutes), with a context menu providing Refresh, Auto Refresh (checkbox), Settings (open window), and Quit actions.

## Requirements

1. **Tray icon with context menu:** Refresh, Auto Refresh (checkbox toggle), Settings (opens the main window), Quit.
2. **Close-to-tray:** Clicking the window close button hides the window instead of exiting.
3. **Subsequent starts:** After the first run, start in tray-only mode (no window, no GPU init).
4. **Lightweight tray-only mode:** No GPU resources, no Slint rendering pipeline, no cloud fetcher -- only the tray icon and a wallpaper refresh timer.
5. **Auto-refresh wallpaper:** Configurable interval (1--30 minutes). Must work without a visible window by temporarily spinning up GPU resources, exporting, then tearing them down.
6. **Single-instance detection:** Prevent multiple copies from running simultaneously.
7. **Persist tray/refresh settings:** Extend the existing config file with new fields.

## Findings

### Tray Icon Crate Selection

The `tray-icon` crate (v0.21.3, MIT/Apache-2.0) from the Tauri project is the clear choice. It is actively maintained (latest release January 2026), has a clean builder API, supports context menus via the companion `muda` crate (re-exported), and works without a visible application window. On Windows it creates a hidden message-only window (`HWND_MESSAGE`) internally for Shell notify messages.

Alternative crates were evaluated and rejected:

| Crate | Verdict | Reason |
| --- | --- | --- |
| `tray-item` v0.10 | Not recommended | Sparse docs, less flexible, last release April 2024 |
| `systray` / `systray-rs` | Do not use | Unmaintained since 2019 |
| `trayicon` (Ciantic) | Not recommended | Windows/KDE only, last updated 2021 |
| `notify-icon` v0.1.4 | Too low-level | Requires manual WNDPROC, not cross-platform |
| `tray-icon-win` v0.1.5 | Not recommended | Personal fork; upstream recommended by its own docs |

`tray-icon` adds `muda`, `crossbeam-channel`, and `windows-sys` (already present) as transitive dependencies. No heavy new dependencies on Windows.

### Threading Architecture

The central integration challenge is that Slint's event loop and `tray-icon`'s Win32 message loop cannot directly share a thread without one blocking the other.

Three options were evaluated:

**Option A -- Background thread for tray icon (recommended).** Spawn a dedicated `std::thread` that creates the `TrayIcon` and runs a Win32 message pump (`GetMessage` / `TranslateMessage` / `DispatchMessage`). Tray menu events call `slint::invoke_from_event_loop(...)` to communicate with the main thread. This cleanly separates concerns and avoids polling latency.

**Option B -- Forward via EventLoopProxy.** The standard winit/tao pattern. Not usable because Slint abstracts away winit and does not expose an `EventLoopProxy` to application code.

**Option C -- Poll tray events from a Slint Timer.** Create the tray icon on the main thread before entering `run_event_loop_until_quit()`, then use a recurring `slint::Timer` to call `TrayIconEvent::receiver().try_recv()`. Avoids a second thread but introduces latency proportional to the timer interval, making menu interactions feel sluggish.

Option A is recommended. The `unsafe` Win32 message pump code can be isolated in a small helper function with `#[allow(unsafe_code)]` and a `// SAFETY:` comment, consistent with the existing patterns in `sun.rs` and `wallpaper.rs`.

An open question remains: whether `tray-icon` internally provides a message pump when `build()` is called on a spawned thread, or whether the calling code must supply its own `GetMessage` loop. This needs verification against the `tray-icon` source or a minimal test during implementation.

### Event Loop Lifecycle

The current app calls `window.run()`, which exits when the window closes, followed by `std::process::exit(0)`. This must change to `slint::run_event_loop_until_quit()`, which keeps the event loop alive after the window is hidden and only exits when `slint::quit_event_loop()` is called explicitly. This API has been available since at least Slint v1.5 (tracked in slint-ui/slint#1499, merged in PR #4315).

The tray "Quit" action calls `slint::quit_event_loop()` to terminate the event loop and exit the process.

### Window Show/Hide

Slint provides the necessary APIs on `ComponentHandle`:

- `show()` -- displays the window, creates a strong internal reference
- `hide()` -- makes the window invisible, drops the internal reference
- `window().is_visible()` -- checks visibility

For close-to-tray behavior, the `on_close_requested` callback should return `slint::CloseRequestResponse::HideWindow`, which is a Slint built-in that hides the window instead of destroying it when the user clicks the close button. This needs verification that it exists in Slint ~1.15 (the pinned version).

Since `show()` and `hide()` touch Slint state, they must be called from the Slint event loop thread. The tray thread communicates via `slint::invoke_from_event_loop`, which is thread-safe, accepts `Send + 'static` closures, and queues execution on the Slint thread. Use `Weak<AppWindow>` handles (which are `Send`) rather than strong references.

**Taskbar icon caveat:** There is no official Slint API to hide the taskbar button while the window is hidden. Achieving this requires reaching into the private `i-slint-backend-winit` crate (per slint-ui/slint#5299). Acceptable to defer -- the taskbar button is only visible when the window is visible, and hiding it when minimized to tray can be addressed separately.

### GPU Resource Management and Wallpaper Export

GPU resources live in a `thread_local! { RefCell<Option<GpuResources>> }` with a clean teardown/recreation lifecycle driven by Slint's rendering notifier:

- `RenderingSetup` -- creates all GPU resources from a Slint-provided device/queue
- `BeforeRendering` -- renders the frame
- `RenderingTeardown` -- sets the thread-local to `None`, dropping everything

This architecture already supports full teardown and recreation, which is essential for lightweight tray-only mode.

**Wallpaper export constraint:** The current `export_wallpaper_image()` function reuses the last frame's bind groups and state. It returns errors if GPU resources are `None`, if no frame has been rendered, or if no bind group is available. It cannot render independently -- it depends on existing GPU state from a prior frame.

**For background refresh without a visible window**, the export pipeline needs to temporarily spin up GPU resources, render one frame, export, and tear down. This is the most significant design challenge. The approach would be: create a temporary Slint window (possibly hidden) to trigger `RenderingSetup`, render a frame with current sun position and saved camera settings, export to wallpaper, then hide/destroy the window to trigger `RenderingTeardown`. Alternatively, the export pipeline could be refactored to initialize its own standalone wgpu device and pipeline without Slint, but this would be a larger change.

### Lightweight Tray-Only Mode

When starting in tray-only mode, the following can be skipped entirely:

- `wgpu_init::create_device()` -- no GPU adapter needed
- Slint UI component instantiation (compiled in but not created)
- Cloud fetcher background thread -- deferred until window opens
- Astronomy engine calculations -- only needed for rendering

The minimal footprint pattern: parse CLI args, check single-instance lock, start the tray icon thread with its menu and message pump, enter `slint::run_event_loop_until_quit()` on the main thread. The `AppWindow` Slint component is created lazily inside an `invoke_from_event_loop` callback when the user clicks "Settings" in the tray menu. GPU resources initialize automatically via `RenderingSetup` when the window is first shown.

### Auto-Refresh Wallpaper

The cloud fetcher in `cloud_fetcher.rs` provides a proven pattern for background work:

- `std::thread::spawn` with a polling loop
- `mpsc` channel for main thread communication
- `upgrade_in_event_loop()` for safe UI updates from background threads
- Exponential backoff on errors (15s initial, 300s max)
- Disk caching with atomic writes

Slint timers (`slint::Timer`) only fire while the event loop is running and depend on the event loop thread. For background wallpaper refresh in tray-only mode, use `std::thread::spawn` + `std::thread::sleep` instead, mirroring the cloud fetcher pattern.

The existing 2-minute sun timer in `main.rs` uses a Slint timer, which is appropriate for window-visible mode but will not fire in tray-only mode when no window exists.

### Config Persistence

The existing `AppConfig` in `config.rs` uses `serde` with TOML serialization, stored at `%LOCALAPPDATA%\SunlitEarth\config.toml`. It already has:

- Debounced saving (1-second timer restarts on every UI change)
- Backstop save on exit
- Atomic writes (write to `config.toml~` then rename)

Extending it with tray/refresh fields is straightforward. New fields needed:

- `start_minimized_to_tray: bool` -- whether to start in tray-only mode
- `auto_refresh_enabled: bool` -- whether wallpaper auto-refresh is active
- `auto_refresh_interval_minutes: u32` -- refresh interval (1--30)

No registry access needed for tray-only startup behavior. The config file approach is easily inspectable and reversible.

### Single-Instance Detection

Two approaches were identified:

**`single-instance` crate (v0.3.3):** Creates a named mutex via `CreateMutexW` on Windows. Simple API -- call `is_single()` to check. The `SingleInstance` value must be kept alive for the process lifetime.

**Manual approach with `windows-sys`:** Call `CreateMutexW` + `GetLastError` == `ERROR_ALREADY_EXISTS` directly. Avoids adding a dependency since `windows-sys` is already present. A few lines of `unsafe` code.

Both approaches only detect whether another instance exists. To signal the existing instance to show its window, a Windows named pipe or `PostMessage` to a named window would be needed. For v1, the simpler approach is acceptable: subsequent instances exit silently, and the user clicks the tray icon to open the window. Named pipe signaling can be added later.

### Auto-Start at Login (Optional)

The `auto-launch` crate (v0.6.0) sets `HKEY_CURRENT_USER\SOFTWARE\Microsoft\Windows\CurrentVersion\Run` to launch the app at login with `--tray-only` argument. No admin privileges needed for per-user registration. This is a separate feature from the core tray icon work and can be deferred.

## Technical Constraints

1. **Slint version ~1.15 pinned:** `CloseRequestResponse::HideWindow` and `run_event_loop_until_quit()` availability needs verification against this specific version.
2. **`unsafe_code = "deny"`:** The Win32 message pump requires `unsafe`. Scoped `#[allow(unsafe_code)]` with `// SAFETY:` comments, consistent with `sun.rs` and `wallpaper.rs`.
3. **`TrayIcon` is `!Send`:** It cannot be moved between threads after creation. Must be created and used on the same thread (the tray thread).
4. **GPU resources are thread-local:** Wallpaper export must happen on the same thread where GPU resources were created (the Slint main thread).
5. **Wallpaper export depends on prior frame state:** Cannot currently render independently. Background refresh needs either a temporary window cycle or a refactored standalone export path.
6. **Icon asset needed:** A 16x16 or 32x32 `.ico` file for the tray icon. Can be embedded via `include_bytes!` or loaded from disk. The `image` crate (already a dependency) can decode `.ico` to RGBA bytes for `tray_icon::Icon::from_rgba()`.

## Open Questions

1. **Win32 message pump requirement:** Does `tray-icon` internally run its own message pump on the spawned thread, or must the calling code provide an explicit `GetMessage` loop? Needs source inspection or a minimal test.
2. **`CloseRequestResponse::HideWindow` in Slint ~1.15:** Is this enum variant available in the pinned Slint version? Needs verification.
3. **Background wallpaper export architecture:** What is the best approach for rendering a frame without a visible window? Options include a hidden temporary window, a headless wgpu pipeline, or briefly showing/hiding a window. Each has trade-offs in complexity and resource usage.
4. **Tray icon asset:** What image to use? A miniature Earth icon, the app logo, or something else? Whether to embed or load from disk.
5. **Timer-on-main-thread alternative:** Could the tray icon be created on the main thread before `run_event_loop_until_quit()` and polled via a Slint `Timer`, avoiding a second thread? Needs testing for menu responsiveness.
6. **Single-instance signaling:** For v2, how to signal the existing instance to show its window when a second instance is launched? Named pipe vs. `PostMessage` trade-offs.

## Recommendations

1. **Use `tray-icon` v0.21** as the tray icon crate. It is the best-maintained, most ergonomic option and adds minimal dependency weight.

2. **Use Option A (background thread)** for tray icon integration. Spawn a dedicated thread that owns the tray icon and Win32 message pump, communicating with Slint via `invoke_from_event_loop`. This provides responsive menu interactions without polling latency.

3. **Replace `window.run()` with `slint::run_event_loop_until_quit()`** and wire the close button to `CloseRequestResponse::HideWindow` for close-to-tray behavior.

4. **Implement in phases:**
   - **Phase 1:** Tray icon with menu (Refresh, Settings, Quit), close-to-tray, `run_event_loop_until_quit()` lifecycle change. No background refresh yet -- "Refresh" in the menu shows the window briefly to render and export.
   - **Phase 2:** Background wallpaper auto-refresh with configurable interval. Design the temporary GPU resource lifecycle for headless export.
   - **Phase 3:** Tray-only startup mode on subsequent launches via config flag. Lazy `AppWindow` creation.
   - **Phase 4 (optional):** Single-instance detection and signaling. Auto-start at login.

5. **Extend `AppConfig`** with `start_minimized_to_tray`, `auto_refresh_enabled`, and `auto_refresh_interval_minutes` fields. Use the existing debounced save mechanism.

6. **Use `windows-sys` directly** for the single-instance mutex rather than adding the `single-instance` crate, since `windows-sys` is already a dependency. For v1, subsequent instances exit silently.

7. **Defer taskbar icon hiding** and auto-launch at login as separate features.

## Sources

| Document | Focus Area |
| --- | --- |
| `docs/plans/2026-03-21-tray-icon-codebase.md` | Codebase analysis: window lifecycle, GPU resources, wallpaper export, config, timers, dependencies |
| `docs/plans/2026-03-21-tray-icon-external.md` | External research: tray-icon crate, Slint integration, threading, show/hide APIs, single-instance, auto-start |
