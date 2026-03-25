# Tray Icon Research — 2026-03-25

## Goal

Add system tray icon support to Sunlit Earth so the app can minimize to tray on close, enforce single-instance, and provide a context menu with Open/Exit. The `render` subcommand must bypass tray mode entirely. A new CLI flag enables regular (non-tray) window mode.

## Scope (Deliberately Narrow)

Only these features are in scope:

1. Tray icon with context menu: **Open** (show window) and **Exit** (quit)
2. Minimize to tray on window close (instead of exiting)
3. Single-instance enforcement (second launch exits silently)
4. CLI flag `--windowed` to run without tray icon (original behavior)
5. `render` subcommand bypasses tray entirely

**Out of scope**: headless renderer, auto-refresh, wallpaper re-render from tray, IPC to bring existing instance to foreground.

## Crate Research

### `tray-icon` v0.21.3

- Maintained by Tauri project, ~1.7M downloads/month
- Re-exports `muda` as `tray_icon::menu` — no need for separate `muda` dependency
- **Icon format**: `Icon::from_rgba(rgba: Vec<u8>, width: u32, height: u32)` — only raw RGBA bytes accepted. Decode an embedded PNG with `image` crate (already a dependency) or generate programmatically.
- **Menu building**: `Menu`, `MenuItem`, `PredefinedMenuItem::separator()`. Each item has an `id()` (opaque `MenuId`) used to identify clicks.
- **Event model**: Two options (mutually exclusive):
  - Polling: `MenuEvent::receiver().try_recv()` / `TrayIconEvent::receiver().try_recv()`
  - Callback: `MenuEvent::set_event_handler(Some(|event| { ... }))` — fires on the message loop thread
  - Callback model is preferred for this project since it integrates naturally with `slint::invoke_from_event_loop`.
- **Threading**: `TrayIcon` is `!Send`. Must be created and used on the same thread. That thread must run a Win32 message loop (`GetMessageW` + `DispatchMessageW`). Does NOT need to be the main thread.
- **Lifetime**: Dropping `TrayIcon` removes it from the system tray. Must keep it alive for the process lifetime.
- **Left-click**: By default, left-click also opens the menu. Use `.with_menu_on_left_click(false)` to handle left-click as "show window" instead.

### `single-instance` v0.3.3

- Simple cross-platform single-instance detection
- On Windows: uses `CreateMutexW` + `GetLastError(ERROR_ALREADY_EXISTS)`
- API: `SingleInstance::new("name")` → `.is_single()` returns `bool`
- **No IPC**: Cannot communicate with existing instance. The second instance can only detect and exit.
- `SingleInstance` must remain alive for the process duration (dropping releases the mutex)
- Depends on `winapi` (not `windows-sys`), which is harmless — they're parallel FFI crates

## Current Codebase Analysis

### Window Lifecycle (main.rs)

```
main()
  ├── Cli::parse()
  ├── wgpu_init::init()
  ├── slint::BackendSelector → select wgpu backend
  ├── MainWindow::new()
  ├── load config, apply to window
  ├── set up callbacks (sliders, mouse, wallpaper, presets, etc.)
  ├── setup_rendering_notifier()
  ├── spawn_cloud_fetcher()
  ├── start sun_timer (2-min periodic redraw)
  ├── if render subcommand: start render_timer (polls textures_ready)
  ├── window.run()          ← BLOCKS until window is closed
  ├── save_window_geometry() on close
  └── std::process::exit(0)
```

Key observations:
- `window.run()` is the event loop — blocks until close
- Window close = process exit (via `std::process::exit(0)`)
- No mechanism to hide/show the window after creation
- `render` subcommand creates a window but auto-closes via `slint::quit_event_loop()` after export
- The `std::process::exit(0)` at line 510 is a workaround for thread-local destruction ordering panics

### CLI Structure (clap)

```rust
struct Cli {
    software_rendering: bool,
    textures_dir: Option<PathBuf>,
    log_level: Option<String>,
    command: Option<Commands>,
}

enum Commands {
    Render { output, width, height, config },
}
```

The `--windowed` flag fits naturally as a new top-level `Cli` field.

### Slint API Availability

Confirmed available in Slint ~1.15:
- `window.show()` / `window.hide()` — show/hide the window
- `window.on_close_requested(|| CloseRequestResponse::HideWindow)` — hide instead of close
- `slint::run_event_loop()` / `slint::quit_event_loop()` — explicit event loop control
- `slint::invoke_from_event_loop(callback)` — thread-safe callback into the Slint event loop
- `slint::Weak<T>` is `Send` — can be passed to the tray thread

### Required `windows-sys` Features

Already present in Cargo.toml: `Win32_UI_WindowsAndMessaging` (covers `GetMessageW`, `TranslateMessage`, `DispatchMessageW`, `PostThreadMessageW`, `WM_QUIT`), `Win32_System_Threading` (covers `GetCurrentThreadId`).

No new `windows-sys` features needed.

## Integration Architecture

### Tray Thread Pattern

```
Main Thread                           Tray Thread (background)
─────────────────────                 ────────────────────────
wgpu_init()
select_backend()
MainWindow::new()
setup callbacks
  ├── on_close_requested:
  │   → HideWindow
  │
start tray thread ──────────────────→ build TrayIcon
                                      set MenuEvent handler:
                                        Open → invoke_from_event_loop(show)
                                        Exit → invoke_from_event_loop(quit)
                                      GetMessageW loop (blocking)
                                        ↓
slint::run_event_loop() ←←←←←←←←←←← handler fires invoke_from_event_loop
  (blocks)                               ↓
  handles invoke callbacks              posts WM_QUIT to break loop
  quit_event_loop() ──────────────────→ GetMessageW returns 0
                                      _tray dropped → icon removed
cleanup + exit
```

### Startup Flow

```
if render subcommand → skip tray, run render pipeline, exit
if --windowed        → skip tray, use window.run() (current behavior)
else (default)       → tray mode:
  1. SingleInstance::new("sunlit-earth") → if !is_single() → exit
  2. Create window + apply config
  3. Spawn tray thread with window.as_weak()
  4. window.show() (first start — show the window)
  5. slint::run_event_loop() (blocks until Exit from tray)
  6. cleanup + exit
```

### Window Close Behavior in Tray Mode

Register `on_close_requested` to return `CloseRequestResponse::HideWindow`. This hides the window but keeps the Slint event loop running. The tray icon remains active. User clicks "Open" in tray menu → `invoke_from_event_loop(|| window.show())`.

**Important**: When using `CloseRequestResponse::HideWindow`, `window.run()` may not be appropriate. Need to verify whether `window.run()` returns when the window is hidden, or if we need `slint::run_event_loop()` instead. The Slint docs suggest `run_event_loop()` is the correct choice since it only exits on `quit_event_loop()`.

### Stopping the Tray Thread

When the user clicks "Exit" in the tray menu:
1. Menu handler fires `slint::quit_event_loop()` via `invoke_from_event_loop`
2. To stop the Win32 message loop, post `WM_QUIT` to the tray thread: capture the thread ID at spawn time, then call `PostThreadMessageW(thread_id, WM_QUIT, 0, 0)` from the Slint event loop's quit path

Alternative: Save the tray thread's `JoinHandle` and just let it die when the process exits (since we call `std::process::exit(0)` anyway). This is simpler and already the existing pattern.

### Icon Strategy

Two options:
1. **Embedded PNG**: `include_bytes!("../assets/tray-icon.png")` decoded at runtime with `image` crate → `Icon::from_rgba`. Requires creating an icon asset file.
2. **Programmatic**: Generate a simple 32x32 RGBA circle (blue-green Earth-like) at runtime. No asset file needed.

Option 2 is simpler for the first iteration. Can upgrade to a proper icon later.

## E2E Testing Strategy

The existing `tests/e2e.rs` spawns the binary as a child process with `wait_with_timeout`. Tray mode tests can follow the same pattern:

1. **Single-instance test**: Spawn instance A (tray mode), spawn instance B → B should exit quickly. Kill A.
2. **Render bypasses tray**: Existing `test_render_and_exit` already validates this — render exits cleanly.
3. **Windowed mode test**: Spawn with `--windowed`, verify it starts and can be killed (similar to current behavior).
4. **Tray startup test**: Spawn in default mode, wait briefly, then kill. Verify clean exit.

Process-based E2E tests can verify startup/exit behavior. They cannot easily verify tray menu interactions (that would require Win32 automation).

## Risk Assessment

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| `window.run()` returns on hide | Medium | Use `slint::run_event_loop()` instead |
| Tray icon not visible on CI (no desktop) | High | Skip tray E2E tests on CI like other `#[ignore]` tests |
| Thread-local GPU resources panic on tray thread | Low | Tray thread never touches GPU — only calls `invoke_from_event_loop` |
| `single-instance` mutex not released on crash | Low | OS releases on process exit; stale mutex auto-cleans |
| `HideWindow` + `show()` cycle causes GPU resource issues | Medium | Test thoroughly; existing dirty-check should handle re-creation |

## Open Questions

1. **`window.run()` vs `slint::run_event_loop()`**: Does `window.run()` block indefinitely when `CloseRequestResponse::HideWindow` is used, or does it return? If it returns, we need `slint::run_event_loop()`. Test empirically.
2. **Window geometry save on hide**: Currently `save_window_geometry` runs after `window.run()`. In tray mode, geometry should be saved when the window is hidden, not on process exit. May need to save inside the `on_close_requested` handler.
