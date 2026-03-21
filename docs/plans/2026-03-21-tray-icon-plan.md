# Plan: System Tray Icon (2026-03-21)

## Summary

Add system tray icon support so Sunlit Earth can run as a lightweight background process. On first start, the app shows the main window and a tray icon. Closing the window minimizes to tray instead of exiting. Subsequent launches start in tray-only mode with no GPU overhead. The tray menu provides Refresh (update wallpaper now), Auto Refresh (checkbox with configurable interval), Settings (open main window), and Quit. A headless rendering pipeline (independent of Slint) enables background wallpaper export without a visible window. Single-instance detection prevents duplicate processes.

## Stakes Classification

**Level**: High
**Rationale**: This feature changes the application lifecycle (event loop, window creation/destruction), introduces a parallel threading model (tray thread + Slint thread), requires a new standalone rendering pipeline that must exactly match the existing Slint-coupled pipeline, and touches nearly every module boundary. Incorrect implementation risks data races, GPU resource leaks, or a broken render pipeline. The changes are hard to partially roll back once merged.

## Context

**Research**: [`2026-03-21-tray-icon-research.md`](2026-03-21-tray-icon-research.md), [`2026-03-21-tray-icon-rendering.md`](2026-03-21-tray-icon-rendering.md)
**Affected Areas**: `main.rs`, `renderer/mod.rs`, `renderer/gpu_setup.rs`, `renderer/render_pass.rs`, `renderer/textures.rs`, `config.rs`, `cloud_fetcher.rs`, `wgpu_init.rs`, `wallpaper.rs`, `lib.rs`, `Cargo.toml`, `ui/main.slint`

## Success Criteria

- [ ] Tray icon appears on startup with a context menu (Refresh, Auto Refresh, Settings, Quit)
- [ ] Closing the main window hides it to tray instead of exiting
- [ ] "Settings" menu item shows the main window; "Quit" exits the process
- [ ] "Refresh" renders and applies wallpaper using the headless pipeline
- [ ] Auto Refresh periodically updates the wallpaper at the configured interval
- [ ] On subsequent starts (after first run), app starts in tray-only mode with no GPU init
- [ ] Headless renderer produces pixel-identical output to the Slint-coupled preview at the same resolution
- [ ] Cloud cache freshness is checked before each headless wallpaper export
- [ ] Only one instance can run at a time; second instance exits silently
- [ ] Config file persists new fields: `auto_refresh_enabled`, `auto_refresh_interval_minutes`, `has_set_wallpaper`
- [ ] All existing tests continue to pass; new code has unit tests for pure functions
- [ ] `cargo clippy` passes clean; no new `unsafe` outside scoped `#[allow(unsafe_code)]` with `// SAFETY:` comments

## Implementation Steps

### Phase 1: Extract Standalone Rendering Core

Refactor the render pipeline so the same core code serves both Slint preview and headless background export. This is the foundational DRY refactoring that all later phases depend on.

#### Step 1.1: Create `wgpu_init::init_headless()` that returns raw device/queue

- **Files**: `src/wgpu_init.rs`
- **Action**: Extract the adapter selection and device creation logic from `init()` into a shared helper. Add a new public function `init_headless(force_software: bool) -> HeadlessContext` that returns a struct containing `device: wgpu::Device`, `queue: wgpu::Queue`, and `supported_sample_counts: Vec<u32>` (no Slint `WGPUConfiguration` wrapper). Refactor `init()` to call the shared helper and then wrap the results in `WGPUConfiguration::Manual`. This avoids duplicating adapter selection logic.
- **Test cases**:
  - `init_headless(false)` returns a context with a non-empty `supported_sample_counts` containing 1
  - `init_headless(true)` either returns a CPU adapter or falls back gracefully (same behavior as existing `init` with software flag)
  - `HeadlessContext.device` and `queue` are usable (can create a simple buffer)
- **Verify**: `cargo test wgpu_init` passes; `cargo clippy` clean
- **Complexity**: Small

#### Step 1.2: Make `ShadingParams` and `RenderTarget` crate-public

- **Files**: `src/renderer/render_pass.rs`
- **Action**: Change `pub(super)` to `pub(crate)` on `ShadingParams`, `RenderTarget`, `RenderTarget::new()`, `write_uniforms()`, `encode_and_submit()`. These functions are already pure wgpu with no Slint dependency; they just need wider visibility for the headless renderer. Do not change `execute_render_pass()` -- it uses `Image::try_from()` which is Slint-specific and stays internal to the renderer module.
- **Test cases**: No new tests needed; this is a visibility change only.
- **Verify**: `cargo build` succeeds; no other module uses these yet (no compile errors from visibility)
- **Complexity**: Small

#### Step 1.3: Make texture creation functions crate-public

- **Files**: `src/renderer/textures.rs`
- **Action**: Change `pub(super)` to `pub(crate)` on `create_mipmapped_texture()`, `create_bind_group()`, `TextureSlot`, and `DecodedTextureMessage`. The headless renderer needs these to build GPU textures from decoded image data.
- **Test cases**: No new tests; visibility-only change.
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

#### Step 1.4: Make `create_pipeline()`, `create_cloud_pipeline()`, `create_render_textures()` crate-public

- **Files**: `src/renderer/gpu_setup.rs`
- **Action**: Change `pub(super)` to `pub(crate)` on `create_pipeline()`, `create_cloud_pipeline()`, and `create_render_textures()`. Also extract the bind group layout creation, sampler creation, shader module creation, and mesh/buffer creation from `create_gpu_resources()` into standalone `pub(crate)` helper functions that can be called independently by the headless renderer. The extracted functions are:
  - `create_bind_group_layout(device) -> BindGroupLayout`
  - `create_sampler(device) -> Sampler`
  - `create_shader_module(device) -> ShaderModule`
  - `create_sphere_buffers(device) -> (Buffer, Buffer, u32)` (vertex, index, index_count)
  - `create_dummy_texture(device, queue) -> TextureView`
  - `create_pipeline_layout(device, layout) -> PipelineLayout`
- **Test cases**: No new tests; this is a refactoring that extracts existing inline code into named functions. Existing tests verify correctness.
- **Verify**: `cargo test` all pass; `cargo clippy` clean; `create_gpu_resources()` calls the new helpers instead of inline code
- **Complexity**: Medium

#### Step 1.5: Make `FrameState` and `build_frame_state()` crate-public

- **Files**: `src/renderer/frame.rs`
- **Action**: Change `pub(crate)` is already the visibility of `FrameState` and `build_frame_state()` -- verify this is the case. If not, widen to `pub(crate)`.
- **Test cases**: None; verification only.
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

#### Step 1.6: Build the headless renderer module

- **Files**: `src/headless.rs` (new), `src/lib.rs`
- **Action**: Create a new `headless` module that implements `render_wallpaper_headless(config: &AppConfig, dimensions: (u32, u32), force_software: bool) -> Result<Vec<u8>, String>`. This function:
  1. Calls `wgpu_init::init_headless(force_software)` to get device/queue
  2. Creates sphere buffers, shader module, pipeline layout, bind group layout, sampler, dummy texture using the helpers from Step 1.4
  3. Creates the grid texture (slot 0, always available)
  4. Loads day/night textures synchronously from disk using `texture_loader::load()` based on the config's `texture_index`
  5. Loads cloud texture synchronously from the disk cache (`clouds_cache.jpg`) using `cloud_fetcher`'s decode function (made crate-public)
  6. Creates bind groups, pipeline, cloud pipeline
  7. Creates render textures with `COPY_SRC` usage at target dimensions
  8. Computes sun direction from config (custom datetime or current time)
  9. Builds `ShadingParams` from config fields
  10. Calls `write_uniforms()` + `encode_and_submit()` + `read_texture_rgba8()`
  11. Drops all GPU resources and returns pixel data
- **Test cases**:
  - `render_wallpaper_headless` with default config and 64x64 dimensions returns `Ok(pixels)` with `pixels.len() == 64 * 64 * 4`
  - Returned pixels are not all-zero (something was rendered -- the grid texture at minimum)
  - With `texture_index = 0` (grid), output is deterministic across two calls with identical config
  - With invalid dimensions (0x0), returns an `Err`
  - Note: pixel-exact comparison between headless and Slint path is not practical in automated tests due to float variance across device creation cycles, but the rendering code path is identical
- **Verify**: `cargo test headless` passes; `cargo clippy` clean
- **Complexity**: Large

#### Step 1.7: Make `decode_cloud_jpeg()` crate-public and extract `cache_image_path()`

- **Files**: `src/cloud_fetcher.rs`
- **Action**: Change `decode_cloud_jpeg()` from private to `pub(crate)`. Also change `cache_image_path()` to `pub(crate)` so the headless renderer can locate the cached cloud image. Add a `pub(crate) fn check_and_refresh_cloud_cache()` function that runs the freshness check + download synchronously (single call, blocking), reusing the existing `check_freshness()` and `download_image()` logic. This is called before each headless render to ensure cloud data is current.
- **Test cases**:
  - `cache_image_path()` returns a path ending in `clouds_cache.jpg` inside the SunlitEarth directory
  - `check_and_refresh_cloud_cache()` succeeds without panic when no cache exists (downloads fresh)
  - `check_and_refresh_cloud_cache()` succeeds when cache already exists and is fresh (304 path)
  - Note: network tests should be `#[ignore]` for CI; verified manually
- **Verify**: `cargo test cloud_fetcher` passes; `cargo clippy` clean
- **Complexity**: Medium

#### Step 1.8: Refactor `export_wallpaper_image()` to use headless renderer

- **Files**: `src/renderer/mod.rs`, `src/main.rs`
- **Action**: Replace the current `export_wallpaper_image()` (which reads from `GPU_RESOURCES` thread-local) with a new implementation that calls `headless::render_wallpaper_headless()`. This means the "Set as Wallpaper" button uses the same code path as background refresh. Update the `do_set_wallpaper()` function in `main.rs` to pass the current config. The old `export_wallpaper_image()` function is removed.
- **Test cases**:
  - Manual: Click "Set as Wallpaper" in the UI -- wallpaper is set correctly
  - Manual: Verify the wallpaper matches the current preview (same camera, same lighting)
  - The wallpaper export no longer requires a prior rendered frame (it's self-contained)
- **Verify**: `cargo build` succeeds; manual wallpaper export test passes
- **Complexity**: Medium

### Phase 2: Config and UI Extensions

Add the new configuration fields and UI controls for auto-refresh. This phase can begin once Phase 1's config-related types are understood but does not depend on the headless renderer being complete. However, the auto-refresh timer depends on Phase 3 (tray icon), so the UI controls are wired but the timer logic is deferred.

#### Step 2.1: Extend `AppConfig` with tray and refresh fields

- **Files**: `src/config.rs`
- **Action**: Add three new fields to `AppConfig`:
  - `auto_refresh_enabled: bool` (default `false`)
  - `auto_refresh_interval_minutes: u32` (default `5`)
  - `has_set_wallpaper: bool` (default `false`)
  All fields use `#[serde(default)]` at the struct level (already present), so existing config files without these fields will get defaults on load.
- **Test cases**:
  - `AppConfig::default().auto_refresh_enabled` is `false`
  - `AppConfig::default().auto_refresh_interval_minutes` is `5`
  - `AppConfig::default().has_set_wallpaper` is `false`
  - Deserializing a TOML string with none of the new fields yields correct defaults (forward compatibility)
  - Serde round-trip with new fields set to non-default values preserves them
  - Deserializing with `auto_refresh_interval_minutes = 0` succeeds (clamped at usage site, not parse time)
- **Verify**: `cargo test config` passes
- **Complexity**: Small

#### Step 2.2: Add auto-refresh UI controls to Slint

- **Files**: `ui/main.slint`
- **Action**: Add a new "Auto Refresh" `GroupBox` section in the controls panel (after the Clouds section, before Date / Time). Contains:
  - `CheckBox` for auto-refresh enabled/disabled (property: `auto-refresh-enabled`)
  - `Slider` for interval in minutes, 1--30, only visible when checkbox is checked (property: `auto-refresh-interval`)
  - `Text` label showing the current interval value (e.g., "5 min")
  Add `in-out` properties for both values. Add a callback `auto-refresh-changed()` that fires when either control changes.
- **Test cases**:
  - Manual: Checkbox toggles; slider appears/disappears based on checkbox state
  - Manual: Slider range is 1--30; dragging updates the label
- **Verify**: `cargo build` succeeds; manual UI verification
- **Complexity**: Small

#### Step 2.3: Wire auto-refresh config to window and save callbacks

- **Files**: `src/main.rs`
- **Action**: In `apply_config_to_window()`, set the new auto-refresh properties from config. In `read_config_from_window()`, read them back. Add a `window.on_auto_refresh_changed()` callback that restarts the debounced config save timer, same as the other `on_*_changed` callbacks. In `on_reset_all`, reset auto-refresh fields to defaults. Set `has_set_wallpaper = true` inside `do_set_wallpaper()` when wallpaper export succeeds, and save config immediately (not debounced, since this is a significant state change).
- **Test cases**:
  - Manual: Change auto-refresh interval, close and reopen app, verify the setting persisted
  - Manual: Set wallpaper, close and reopen, verify `has_set_wallpaper` is true in config file
  - Manual: Reset All resets auto-refresh to defaults
- **Verify**: `cargo build` succeeds; manual verification of config persistence
- **Complexity**: Small

### Phase 3: System Tray Icon

Introduce the tray icon, tray thread, and close-to-tray behavior. This phase depends on Phase 1 (headless renderer) for the Refresh action, and on Phase 2 (config) for reading auto-refresh settings.

#### Step 3.1: Add `tray-icon` and `muda` dependencies

- **Files**: `Cargo.toml`
- **Action**: Add to `[target.'cfg(windows)'.dependencies]`:
  - `tray-icon = "0.21"` -- tray icon support
  - `muda = "0.17"` -- context menu (companion crate from the Tauri project, re-exported by `tray-icon`)
  Note: `muda` is a transitive dependency of `tray-icon` but adding it explicitly gives direct access to menu builder APIs. Verify the compatible `muda` version by checking `tray-icon` 0.21's `Cargo.toml`.
- **Test cases**: None (dependency addition).
- **Verify**: `cargo build` succeeds on Windows
- **Complexity**: Small

#### Step 3.2: Create placeholder tray icon asset

- **Files**: `assets/tray_icon.ico` (new)
- **Action**: Create a minimal 32x32 `.ico` file with a colored circle (blue/green earth-like). This is a placeholder; real art comes later. Embed it in the binary via `include_bytes!("../assets/tray_icon.ico")` in the tray module. Alternatively, generate a simple RGBA buffer programmatically (32x32 circle) and pass it to `tray_icon::Icon::from_rgba()`, avoiding an external file entirely.
- **Test cases**:
  - If file-based: `include_bytes!` compiles without error
  - If programmatic: unit test that the generated RGBA buffer has length `32 * 32 * 4`
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

#### Step 3.3: Create the `tray.rs` module with tray icon setup

- **Files**: `src/tray.rs` (new), `src/lib.rs`
- **Action**: Create a new `tray` module (behind `#[cfg(windows)]`) that provides:
  - `pub fn spawn_tray_thread(window_weak: slint::Weak<MainWindow>, config: AppConfig)` -- spawns a dedicated `std::thread` that:
    1. Creates a `muda::Menu` with four items: "Refresh" (regular), "Auto Refresh" (check menu item, initially set from `config.auto_refresh_enabled`), "Settings" (regular), separator, "Quit" (regular)
    2. Creates a `tray_icon::TrayIcon` with the placeholder icon and menu
    3. Runs a Win32 message pump (`GetMessage` / `TranslateMessage` / `DispatchMessage` loop) so tray events are processed
    4. Polls `muda::MenuEvent::receiver()` inside the message pump loop
    5. On "Settings": calls `slint::invoke_from_event_loop()` with `window_weak.upgrade()?.show()`
    6. On "Quit": calls `slint::invoke_from_event_loop()` then `slint::quit_event_loop()`
    7. On "Refresh": spawns a thread that calls the headless wallpaper export pipeline (Phase 1), then notifies the tray thread on completion
    8. On "Auto Refresh" toggle: updates config and saves to disk
  - The message pump requires `unsafe` Win32 calls; use `#[allow(unsafe_code)]` with `// SAFETY:` comments, consistent with `sun.rs` and `wallpaper.rs`.
- **Test cases**:
  - The menu item IDs are distinct (unit test: verify each `MenuId` is unique)
  - `generate_tray_icon_rgba()` returns a buffer of correct size (if using programmatic icon)
  - Manual: tray icon appears; right-click shows menu with all four items
  - Manual: "Settings" opens the main window
  - Manual: "Quit" exits the process
- **Verify**: `cargo build` succeeds; manual tray icon testing
- **Complexity**: Large

#### Step 3.4: Change event loop to `run_event_loop_until_quit()`

- **Files**: `src/main.rs`
- **Action**: Replace `window.run()` with the pattern:
  1. `window.show().expect("Failed to show window");`
  2. `slint::run_event_loop_until_quit().expect("Event loop error");`
  This keeps the event loop alive after the window is hidden. The `std::process::exit(0)` at the end of `main()` remains (avoids thread-local destruction panics). Also update the backstop config save to happen after the event loop exits, same as before.
- **Test cases**:
  - Manual: App starts, window appears, closing the window does NOT exit the process (event loop stays alive)
  - Manual: "Quit" from tray menu terminates the event loop and process
- **Verify**: `cargo build` succeeds; manual testing
- **Complexity**: Small

#### Step 3.5: Wire close-to-tray via `on_close_requested`

- **Files**: `src/main.rs`
- **Action**: Add `window.window().on_close_requested(|| slint::CloseRequestResponse::HideWindow)` callback. This makes the window close button hide the window instead of destroying it. When the window is hidden, Slint fires `RenderingTeardown` which drops GPU resources -- this is fine because the headless renderer (Phase 1) creates its own GPU resources independently. Verified: `slint::CloseRequestResponse::HideWindow` and `slint::run_event_loop_until_quit()` are both available in Slint 1.15.1 (our pinned version). No upgrade needed.
- **Test cases**:
  - Manual: Click the X button -- window disappears but tray icon remains and process is still running
  - Manual: Click "Settings" in tray -- window reappears with all previous settings intact
  - Manual: Click X again, then "Settings" again -- window shows again (can toggle repeatedly)
- **Verify**: Manual testing of hide/show cycle
- **Complexity**: Small

#### Step 3.6: Integrate tray thread spawn into `main()`

- **Files**: `src/main.rs`
- **Action**: After creating the `MainWindow` and loading config, call `tray::spawn_tray_thread(window.as_weak(), config.clone())`. This starts the tray icon before the event loop. The tray thread communicates with the Slint event loop via `slint::invoke_from_event_loop()`. Pass the config so the tray thread knows the initial auto-refresh state.
- **Test cases**:
  - Manual: App starts, both window and tray icon are visible
  - Manual: All tray menu actions work (tested in Step 3.3)
- **Verify**: Manual integration testing
- **Complexity**: Small

### Phase 4: Background Wallpaper Auto-Refresh

Wire the auto-refresh timer that periodically exports wallpaper using the headless renderer. Depends on Phases 1 and 3.

#### Step 4.1: Implement background refresh thread

- **Files**: `src/tray.rs` (or `src/refresh.rs` if the tray module grows too large)
- **Action**: Create a refresh timer using `std::thread::spawn` + `std::thread::sleep` (not a Slint timer, which requires the event loop and a visible window). The thread:
  1. Sleeps for the configured interval
  2. On wake: reads the current config from disk (not from UI, since the window may be hidden)
  3. Checks `has_set_wallpaper` -- if false, skips this cycle (auto-refresh only activates after the user has set a wallpaper at least once)
  4. Checks `auto_refresh_enabled` -- if false, sleeps and re-checks
  5. Calls `cloud_fetcher::check_and_refresh_cloud_cache()` to ensure fresh cloud data
  6. Calls `headless::render_wallpaper_headless()` at the primary monitor resolution
  7. Calls `wallpaper::save_wallpaper_image()` + `wallpaper::set_wallpaper()`
  8. Logs success/failure to stderr
  9. Loops back to sleep
  The thread is spawned from `spawn_tray_thread()` when auto-refresh is enabled and `has_set_wallpaper` is true. A `std::sync::atomic::AtomicBool` (shared with the tray thread) controls whether the refresh loop is active, so toggling the "Auto Refresh" menu item takes effect immediately.
  Use an `Arc<AtomicU32>` for the interval so the tray thread can update it when the user changes the slider.
- **Test cases**:
  - Unit test: the refresh loop correctly skips when `has_set_wallpaper` is false
  - Unit test: the refresh loop correctly skips when `auto_refresh_enabled` is false
  - Manual: Enable auto-refresh, set interval to 1 minute, set wallpaper -- verify wallpaper updates after ~1 minute
  - Manual: Disable auto-refresh -- verify wallpaper stops updating
  - Manual: Close the main window (tray only) -- verify auto-refresh continues working
- **Verify**: `cargo test refresh` passes; manual end-to-end testing
- **Complexity**: Large

#### Step 4.2: Wire "Refresh" tray menu action

- **Files**: `src/tray.rs`
- **Action**: When the "Refresh" menu item is clicked, spawn a one-shot thread that:
  1. Reads config from disk
  2. Calls `cloud_fetcher::check_and_refresh_cloud_cache()`
  3. Calls `headless::render_wallpaper_headless()` at primary monitor resolution
  4. Calls `wallpaper::save_wallpaper_image()` + `wallpaper::set_wallpaper()`
  5. If the main window is visible, updates the status text via `invoke_from_event_loop`
  This is the same pipeline as auto-refresh but triggered manually. Factor the common export logic into a shared `do_headless_wallpaper_export(config) -> Result<(), String>` helper.
- **Test cases**:
  - Manual: Click "Refresh" in tray menu -- wallpaper updates to current sun position
  - Manual: Click "Refresh" while main window is hidden -- wallpaper updates, no crash
  - Manual: Click "Refresh" with window visible -- status text shows success/failure
- **Verify**: Manual testing
- **Complexity**: Medium

### Phase 5: Tray-Only Startup and Single-Instance Detection

Enable subsequent starts in tray-only mode and prevent multiple instances.

#### Step 5.1: Add `--tray-only` CLI flag

- **Files**: `src/main.rs`
- **Action**: Add a `--tray-only` flag to the `Cli` struct (clap). When present (or when `config.has_set_wallpaper` is true and the user has previously closed the window), skip `MainWindow` creation and Slint rendering setup. The startup flow becomes:
  1. Parse CLI
  2. Check single-instance (Step 5.3)
  3. Load config
  4. If tray-only mode: skip wgpu init, skip Slint backend configuration, create `MainWindow` lazily
  5. Spawn tray thread
  6. Enter `slint::run_event_loop_until_quit()`
  The Slint event loop must still run even without a visible window (it processes `invoke_from_event_loop` callbacks from the tray thread). When "Settings" is clicked, the `MainWindow` is created lazily with full GPU setup at that point.
- **Test cases**:
  - Manual: `sunlit-earth --tray-only` starts with only the tray icon visible
  - Manual: Click "Settings" in tray -- window appears with all saved settings
  - Manual: Close window, click "Settings" again -- window reappears
  - Manual: Task Manager shows low memory usage in tray-only mode (no GPU memory)
- **Verify**: Manual testing of startup modes
- **Complexity**: Medium

#### Step 5.2: Implement lazy window creation

- **Files**: `src/main.rs`
- **Action**: Refactor `main()` so that `MainWindow::new()` and all the callback wiring (slider callbacks, mouse drag, wallpaper button, rendering notifier, cloud fetcher) are encapsulated in a function `create_and_show_window(config: &AppConfig, force_software: bool) -> Weak<MainWindow>`. This function is called eagerly on first start, or lazily from the tray thread's "Settings" handler via `invoke_from_event_loop`. The `Weak<MainWindow>` is stored in an `Rc<RefCell<Option<Weak<MainWindow>>>>` shared between tray callbacks.
- **Test cases**:
  - Manual: In tray-only mode, "Settings" creates the window correctly
  - Manual: Window has all persisted settings (same as normal startup)
  - Manual: Cloud fetcher starts when window is created
  - Manual: Closing the window drops GPU resources (visible in stderr logs)
- **Verify**: Manual testing
- **Complexity**: Large

#### Step 5.3: Single-instance detection via `single-instance` crate

- **Files**: `Cargo.toml`, `src/main.rs`
- **Action**: Add `single-instance = "0.3"` to `[dependencies]` (not platform-gated — the crate is cross-platform, which will be useful when adding Linux and macOS support). At the start of `main()`, before any Slint or wgpu initialization, create a `SingleInstance::new("SunlitEarth")`. If `is_single()` returns `false`, print a message to stderr and exit with code 0. The `SingleInstance` value must be kept alive for the process lifetime (store in a `_guard` variable in `main()`). On Windows this uses a named mutex internally; on Linux/macOS it uses file locks.
- **Test cases**:
  - Manual: Start the app, then try to start a second instance -- second instance exits silently
  - Manual: After the first instance exits, a new instance can start normally
- **Verify**: `cargo build` succeeds; manual testing of duplicate prevention
- **Complexity**: Small

#### Step 5.4: Detect tray-only mode from config on startup

- **Files**: `src/main.rs`
- **Action**: After loading config, if `config.has_set_wallpaper` is true and `--tray-only` is not explicitly set, check if this is a "subsequent start" and default to tray-only mode. The heuristic: if `has_set_wallpaper` is true, start in tray-only mode unless the `--show-window` flag is passed (add this CLI flag). This way, after the first wallpaper set, the app always starts minimized. Users can override with `--show-window`.
- **Test cases**:
  - Manual: First run (fresh config) -- window shows
  - Manual: After setting wallpaper, restart -- tray-only mode
  - Manual: After setting wallpaper, `--show-window` -- window shows
- **Verify**: Manual testing
- **Complexity**: Small

### Phase 6: Integration and Polish

#### Step 6.1: Update CLAUDE.md architecture docs

- **Files**: `CLAUDE.md`
- **Action**: Add entries for new modules: `tray.rs`, `headless.rs`, `single_instance.rs` (if separate). Update the `main.rs` description to reflect the new startup flow. Update the wallpaper export pipeline description to reference the headless renderer. Add `tray-icon` and `muda` to notable dependencies.
- **Test cases**: None (documentation).
- **Verify**: Review for accuracy
- **Complexity**: Small

#### Step 6.2: End-to-end integration testing

- **Files**: N/A (manual testing)
- **Action**: Test the complete workflow:
  1. Fresh install (no config file): window + tray icon appear
  2. Adjust camera/lighting settings, set wallpaper
  3. Close window: tray icon remains, process running
  4. "Settings" opens window with saved settings
  5. Enable auto-refresh at 1-minute interval
  6. Close window, wait 2 minutes: wallpaper refreshes (sun moves)
  7. "Quit" exits the process
  8. Restart app: starts in tray-only mode
  9. "Refresh" from tray: wallpaper updates immediately
  10. Start a second instance: exits silently
- **Test cases**: All items above are manual test cases.
- **Verify**: All 10 scenarios pass
- **Complexity**: Medium

## Test Strategy

### Automated Tests

| Test Case | Type | Input | Expected Output |
| --- | --- | --- | --- |
| `init_headless` creates usable device | Unit | `force_software: false` | Returns `HeadlessContext` with valid device/queue |
| `init_headless` sample counts include 1 | Unit | any | `supported_sample_counts` contains 1 |
| `AppConfig` new field defaults | Unit | `AppConfig::default()` | `auto_refresh_enabled = false`, `interval = 5`, `has_set_wallpaper = false` |
| Config serde round-trip with new fields | Unit | Non-default values | Deserialized values match originals |
| Config forward compatibility | Unit | TOML without new fields | Defaults filled correctly |
| Headless render returns correct size | Integration (GPU) | 64x64, default config | `Ok(pixels)` with `len == 64*64*4` |
| Headless render produces non-zero output | Integration (GPU) | 64x64, grid texture | At least some non-zero pixel values |
| Headless render deterministic | Integration (GPU) | Two identical calls | Same pixel output |
| `cache_image_path` returns expected path | Unit | None | Path ends in `clouds_cache.jpg` |
| Tray icon RGBA buffer size | Unit | 32x32 | Buffer length `32*32*4` |
| Menu item IDs are unique | Unit | All menu IDs | No duplicates |

### Manual Verification

- [ ] Tray icon appears on startup with correct menu items
- [ ] Closing window hides to tray (process stays alive)
- [ ] "Settings" menu item shows the main window
- [ ] "Quit" menu item terminates the process
- [ ] "Refresh" updates wallpaper from tray-only mode
- [ ] Auto-refresh updates wallpaper at configured interval
- [ ] Tray-only startup mode skips window creation
- [ ] Second instance exits silently
- [ ] Cloud cache is refreshed before headless wallpaper export
- [ ] Wallpaper from headless renderer matches preview quality
- [ ] Memory usage is low in tray-only mode (no GPU allocation)

## Risks and Mitigations

| Risk | Impact | Mitigation |
| --- | --- | --- |
| `CloseRequestResponse::HideWindow` missing in Slint ~1.15 | N/A — **confirmed available** in Slint 1.15.1 | No action needed; both `HideWindow` and `run_event_loop_until_quit()` exist in our pinned version |
| `tray-icon` message pump blocks or conflicts with Slint event loop | Tray menu unresponsive | Use dedicated thread (Option A from research); fallback to Timer polling if threading proves problematic |
| Headless renderer output differs from Slint preview | User confusion about wallpaper appearance | Use identical shader, pipeline config, and uniform computation; differences limited to float rounding from separate device creation |
| `single-instance` lock not released on crash | Subsequent instance cannot start | On Windows, the named mutex is automatically released when the owning process terminates. On Linux/macOS, file locks are released on process exit. Verified behavior for the `single-instance` crate. |
| Cloud fetcher network request blocks headless render | Slow wallpaper refresh | Add a timeout (30s) on the synchronous cloud refresh; proceed with stale cache on timeout |
| GPU adapter selection differs between Slint path and headless path | Different rendering quality | Use the same `select_adapter()` logic in both paths; headless path uses `init_headless()` which shares the selection code |
| Slint event loop without a window may behave unexpectedly | Tray-only mode unstable | Test thoroughly; `run_event_loop_until_quit()` is designed for this use case per Slint docs |

## Rollback Strategy

Each phase is independently revertible via `git revert`:

- **Phase 1 (rendering core)**: Reverts are clean since existing callers are preserved until Phase 1.8. If Phase 1.8 causes issues, revert only that step to restore the old `export_wallpaper_image()`.
- **Phase 2 (config/UI)**: New config fields have defaults; removing them is backward-compatible. Slint UI changes are isolated to new GroupBox sections.
- **Phase 3 (tray icon)**: The tray module is self-contained behind `#[cfg(windows)]`. Reverting removes the tray thread and restores `window.run()`.
- **Phase 4 (auto-refresh)**: The refresh thread is spawned from the tray module. Reverting the thread spawn disables auto-refresh.
- **Phase 5 (startup/singleton)**: CLI flags and lazy creation can be reverted independently. The mutex is a few lines of code.

For a full rollback, revert the branch to its pre-tray-icon state. All changes are on a feature branch.

## Dependencies Between Phases

```text
Phase 1 (Rendering Core) ──┬──> Phase 3 (Tray Icon) ──> Phase 4 (Auto-Refresh)
                            │                                     │
Phase 2 (Config/UI) ───────┘                                     │
                                                                  v
                                                           Phase 5 (Startup)
                                                                  │
                                                                  v
                                                           Phase 6 (Integration)
```

Phases 1 and 2 can proceed in parallel (they modify different files). Phase 3 depends on both. Phases 4 and 5 depend on Phase 3. Phase 6 depends on all previous phases.

## Status

- [ ] Plan approved
- [ ] Implementation started
- [ ] Phase 1: Extract Standalone Rendering Core
- [ ] Phase 2: Config and UI Extensions
- [ ] Phase 3: System Tray Icon
- [ ] Phase 4: Background Wallpaper Auto-Refresh
- [ ] Phase 5: Tray-Only Startup and Single-Instance Detection
- [ ] Phase 6: Integration and Polish
- [ ] Implementation complete
