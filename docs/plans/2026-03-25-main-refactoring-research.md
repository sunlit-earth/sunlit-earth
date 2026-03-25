# main() Refactoring Research

Research completed 2026-03-25. This document provides a comprehensive analysis of `src/main.rs` to support designing a refactoring plan for the bloated `main()` function (currently 448 lines, `clippy::too_many_lines` suppressed on line 116).

## A. Stack Pressure Map

All local variables declared in `main()` (lines 117-564), with types and estimated stack sizes.

### Scalars and Small Structs

| Variable | Type | Est. Size | Line | Notes |
|---|---|---|---|---|
| `cli` | `Cli` | ~80 bytes | 118 | Contains `Option<PathBuf>` (24 bytes heap-backed), `Option<String>` (24 bytes), `bool` x2, `Option<Commands>` (nested enum with PathBuf + u32 + u32 + Option<PathBuf>) |
| `_guard` | `WorkerGuard` | ~32 bytes | 119 | Tracing non-blocking writer guard, must live until exit |
| `wgpu_context` | `WgpuContext` | ~240+ bytes | 129 | Contains `WGPUConfiguration::Manual` (Instance + Adapter + Device + Queue = all heap-backed Arc types), `String`, `Vec<u32>` |
| `window` | `MainWindow` | ~16 bytes | 137 | Slint component handle (internal Rc) |
| `config` | `AppConfig` | ~180 bytes | 143 | 35 f32 fields (140 bytes), 1 bool (1), 2 i32 (8), 1 u32 (4), 4 Option<i32/u32> (32) |
| `aa_labels` | `Vec<SharedString>` | 24 bytes | 155 | On stack; actual strings on heap. Immediately moved into `aa_model`. |
| `aa_counts` | `Vec<u32>` | 24 bytes | 155 | Cloned 3 times into closures |
| `_aa_default` | `i32` | 4 bytes | 155 | Unused (prefixed with `_`) |
| `aa_model` | `VecModel<SharedString>` | ~32 bytes | 157 | Immediately consumed by `set_aa_options` |
| `textures_dir` | `Option<PathBuf>` | 24 bytes | 165 | |
| `day_path` | `Option<PathBuf>` | 24 bytes | 166 | Moved into `texture_paths` |
| `night_path` | `Option<PathBuf>` | 24 bytes | 170 | Moved into `texture_paths` |
| `texture_paths` | `Vec<Option<PathBuf>>` | 24 bytes | 174 | Moved into `setup_rendering_notifier` |
| `labels` | `Vec<SharedString>` | 24 bytes | 178 | Immediately consumed |
| `base_year` | `i32` | 4 bytes | 187 | Used in multiple closures (copied, not cloned) |
| `end_year` | `i32` | 4 bytes | 187 | Used once |
| `year_labels` | `Vec<SharedString>` | 24 bytes | 188 | Immediately consumed |
| `config_aa_index` | `i32` | 4 bytes | 193 | Moved into closure |
| `config_texture_index` | `i32` | 4 bytes | 194 | Moved into closure |
| `window_weak` | `Weak<MainWindow>` | ~16 bytes | 195, 206, 215, ... | Declared ~12 times, each moved into a closure |
| `texture_tx` | `mpsc::Sender<DecodedTextureMessage>` | ~8 bytes | 432 | Arc-backed, cloned once |
| `texture_rx` | `mpsc::Receiver<DecodedTextureMessage>` | ~8 bytes | 432 | Moved into renderer |
| `textures_ready` | `Arc<AtomicBool>` | ~8 bytes | 435 | Cloned twice |
| `sun_timer` | `slint::Timer` | ~16 bytes | 456 | Must survive until event loop exit |
| `is_render` | `bool` | 1 byte | 470 | |
| `render_timer` | `Option<slint::Timer>` | ~24 bytes | 471 | Must survive until event loop exit |
| `use_tray` | `bool` | 1 byte | 505 | |
| `_instance_guard` | `Option<SingleInstance>` | ~24 bytes | 515 | OS mutex guard, must survive |
| `_tray_handle` | `Option<JoinHandle<()>>` | ~24 bytes | 521 | Thread handle, must survive |
| `aa_counts_for_save` | `Vec<u32>` (clone) | 24 bytes | 248 | Clone of `aa_counts` for wallpaper closure |
| `aa_counts_for_defaults` | `Vec<u32>` (clone) | 24 bytes | 380 | Clone of `aa_counts` for defaults closure |
| `aa_counts_for_reset` | `Vec<u32>` (clone) | 24 bytes | 406 | Clone of `aa_counts` for reset closure |

### Total Estimated Stack Usage

Approximately **1,000-1,200 bytes** of directly declared locals. The `WgpuContext` is the largest at ~240 bytes but most of its data is heap-backed via Arcs. The `AppConfig` at ~180 bytes is the next largest value type. The `Cli` struct with its nested `Commands` enum is also substantial.

**Key concern:** The past stack overflow issue was likely not from these locals alone but from deeply nested function calls during wgpu initialization or Slint window creation, combined with the default 1 MB stack on Windows. The actual stack frame for `main()` itself is modest.

### Variables Cloned Into Multiple Closures

| Variable | Clone Count | Closures |
|---|---|---|
| `aa_counts` | 3 clones | `on_set_wallpaper`, `on_load_defaults`, `on_reset` |
| `window.as_weak()` | ~12 calls | Every callback + tray + timers |
| `textures_ready` | 2 `Arc::clone` | `setup_rendering_notifier`, render timer |
| `texture_tx` | 1 clone | `spawn_cloud_fetcher` (original moved to renderer) |
| `base_year` | 0 (Copy) | Captured by value in 2 closures (`on_sliders_changed`, `on_datetime_override_toggled`) |


## B. Closure Capture Analysis

### 1. Deferred ComboBox Index Update (line 196)

- **Callback type:** `slint::invoke_from_event_loop` (one-shot)
- **Captures:** `window_weak` (moved), `config_aa_index` (i32 copy), `config_texture_index` (i32 copy)
- **Shared state potential:** Could be part of a "set deferred indices" helper function

### 2. `on_sliders_changed` (line 207)

- **Captures:** `window_weak` (moved), `base_year` (i32 copy)
- **Body:** Calls `update_datetime_labels()`, then `request_redraw()`
- **Shared state potential:** Yes -- only needs window reference and base_year constant

### 3. `on_datetime_override_toggled` (line 216)

- **Captures:** `window_weak` (moved), `base_year` (i32 copy)
- **Body:** Initializes datetime sliders to current UTC, calls `update_datetime_labels()`, `request_redraw()`
- **Shared state potential:** Yes -- same captures as sliders_changed

### 4. `on_msaa_changed` (line 232)

- **Captures:** `window_weak` (moved)
- **Body:** Just `request_redraw()`
- **Shared state potential:** Trivial; could be combined with other "just redraw" callbacks

### 5. `on_texture_changed` (line 239)

- **Captures:** `window_weak` (moved)
- **Body:** Just `request_redraw()`
- **Shared state potential:** Same as msaa_changed

### 6. `on_set_wallpaper` (line 249)

- **Captures:** `window_weak` (moved), `aa_counts_for_save` (Vec<u32> clone)
- **Body:** Reads config from window, saves to disk, calls `do_set_wallpaper()`
- **Shared state potential:** Yes -- needs window ref and aa_counts

### 7. `on_mouse_drag_globe` (line 265)

- **Captures:** `window_weak` (moved)
- **Body:** Tilt-corrected globe rotation from mouse delta. Reads tilt, zoom, lon, lat from window; writes wrapped lon, clamped lat back.
- **Shared state potential:** Self-contained math; could be an extracted function taking `&MainWindow`

### 8. `on_mouse_drag_frame` (line 297)

- **Captures:** `window_weak` (moved)
- **Body:** Adjusts offset X/Y from mouse delta, clamped to [-3, 3]
- **Shared state potential:** Same pattern as drag_globe

### 9. `on_mouse_drag_orient` (line 314)

- **Captures:** `window_weak` (moved)
- **Body:** Adjusts yaw/pitch from mouse delta, clamped to [-90, 90]
- **Shared state potential:** Same pattern

### 10. `on_mouse_drag_tilt` (line 330)

- **Captures:** `window_weak` (moved)
- **Body:** Adjusts tilt from horizontal mouse delta, wraps to [-180, 180]
- **Shared state potential:** Same pattern

### 11. `on_mouse_scroll` (line 346)

- **Captures:** `window_weak` (moved)
- **Body:** Adjusts zoom from scroll delta, clamped to [0, 1]
- **Shared state potential:** Same pattern

### 12. `on_apply_preset` (line 360)

- **Captures:** `window_weak` (moved)
- **Body:** Looks up `PRESETS[index]`, writes camera params to window
- **Shared state potential:** Self-contained; pure mapping

### 13. `on_load_defaults` (line 381)

- **Captures:** `window_weak` (moved), `aa_counts_for_defaults` (Vec<u32> clone)
- **Body:** Creates `AppConfig::default()`, calls `apply_config_to_window()`, defers ComboBox index updates
- **Shared state potential:** Yes -- same pattern as startup config application

### 14. `on_reset` (line 407)

- **Captures:** `window_weak` (moved), `aa_counts_for_reset` (Vec<u32> clone)
- **Body:** Calls `load_config()`, `apply_config_to_window()`, defers ComboBox index updates
- **Shared state potential:** Nearly identical to load_defaults except config source

### 15. Sun Timer (line 460)

- **Captures:** `window_weak` (moved)
- **Body:** Logs memory, calls `request_redraw()`
- **Shared state potential:** Trivial

### 16. Render Timer (line 477)

- **Captures:** `output_path` (PathBuf, moved), `render_width` (u32 copy), `render_height` (u32 copy), `textures_ready` (Arc clone)
- **Body:** Polls `textures_ready`, calls `export_wallpaper_image`, saves PNG, quits event loop
- **Shared state potential:** Render-mode specific; could be its own function

### 17. `window.on_close_requested` (line 526, tray mode only)

- **Captures:** `window_weak` (moved)
- **Body:** Saves window geometry, returns `HideWindow`
- **Shared state potential:** Tray-mode specific

### Summary of Capture Patterns

- **12 of 17 closures** capture only `window_weak` (or `window_weak` plus a `Copy` type like `base_year`)
- **3 closures** also capture a cloned `Vec<u32>` (`aa_counts`): set_wallpaper, load_defaults, reset
- **1 closure** (render timer) captures render-mode specific data
- A shared state struct holding `Weak<MainWindow>`, `aa_counts: Vec<u32>`, and `base_year: i32` would eliminate all clones and unify 15 of 17 closures


## C. Logical Sections of main()

### Section 1: CLI Parsing and Logging (lines 118-127)

- Parse CLI args, init tracing subscriber
- Dependencies: None
- Produces: `cli`, `_guard`

### Section 2: wgpu Initialization (lines 129-131)

- Initialize wgpu instance, adapter, device
- Dependencies: `cli.software_rendering`
- Produces: `wgpu_context`

### Section 3: Slint Backend Selection and Window Creation (lines 132-139)

- Select wgpu backend, create MainWindow, set adapter info
- Dependencies: `wgpu_context.config`
- Produces: `window`

### Section 4: Config Load and Application (lines 141-152)

- Load config from disk (or --config path), apply to window, restore geometry
- Dependencies: `cli.command`, `window`
- Produces: `config` (temporary, used then dropped)

### Section 5: AA Options Setup (lines 154-158)

- Build AA ComboBox labels/counts from supported sample counts
- Dependencies: `wgpu_context.supported_sample_counts`
- Produces: `aa_labels`, `aa_counts`, `_aa_default`

### Section 6: JXL Hook and Texture Path Resolution (lines 160-175)

- Register JXL decoding hook, resolve texture directory, build path list
- Dependencies: `cli.textures_dir`
- Produces: `texture_paths`

### Section 7: UI Model Setup (lines 177-203)

- Set texture options model, year ComboBox model, defer AA/texture index updates
- Dependencies: `window`, `config`, `aa_counts`
- Produces: (side effects on window)

### Section 8: Slider/Change Callbacks (lines 205-243)

- Register `on_sliders_changed`, `on_datetime_override_toggled`, `on_msaa_changed`, `on_texture_changed`
- Dependencies: `window`, `base_year`
- All follow the same pattern: upgrade weak, do work, request_redraw

### Section 9: Wallpaper Button Callback (lines 245-261)

- Register `on_set_wallpaper`
- Dependencies: `window`, `aa_counts` (cloned)

### Section 10: Mouse Interaction Callbacks (lines 263-355)

- Register 5 callbacks: drag_globe, drag_frame, drag_orient, drag_tilt, scroll
- Dependencies: `window` only
- Contains non-trivial tilt-corrected rotation math (drag_globe, lines 269-292)

### Section 11: Preset Callback (lines 357-376)

- Register `on_apply_preset`
- Dependencies: `window`, `PRESETS` constant

### Section 12: Load-Defaults and Reset Callbacks (lines 378-428)

- Register `on_load_defaults`, `on_reset`
- Dependencies: `window`, `aa_counts` (cloned twice)
- Both contain deferred ComboBox index update sub-closures

### Section 13: Texture Channel and Renderer Setup (lines 430-444)

- Create mpsc channel, create `textures_ready` flag, call `setup_rendering_notifier`
- Dependencies: `window`, `aa_counts` (moved), `texture_paths` (moved)
- This is the point where `aa_counts` and `texture_paths` are consumed

### Section 14: Cloud Fetcher Spawn (lines 446-452)

- Spawn background cloud fetcher thread
- Dependencies: `texture_tx` (clone), `window.as_weak()`, slot index constant

### Section 15: Sun Timer (lines 454-466)

- Create periodic 2-minute timer for sun position updates
- Dependencies: `window`

### Section 16: Render Timer (lines 468-499)

- If render subcommand: create polling timer that saves PNG when textures ready
- Dependencies: `cli.command` (moved/destructured), `textures_ready` (cloned)

### Section 17: Startup Mode Branching (lines 501-548)

- Determine mode (render / tray / windowed)
- Tray mode: enforce single instance, register on_close_requested (hide), spawn tray thread
- Execute event loop (different API call depending on mode)
- Dependencies: `is_render`, `cli.windowed`, `window`

### Section 18: Shutdown (lines 550-564)

- Save window geometry, log memory, drop timers, `process::exit(0)`
- Dependencies: `window`, `sun_timer`, `render_timer`


## D. Testability Gaps

### Currently Untestable Logic

1. **Mouse drag math** (lines 269-292, 300-310, 316-326, 331-342, 348-354): Tilt-corrected rotation, sensitivity scaling, longitude wrapping, latitude clamping. All embedded in closures that capture a `Weak<MainWindow>`. The math itself is pure but cannot be unit-tested without extracting it.

2. **Datetime initialization on toggle** (lines 218-225): Gets current UTC time and sets slider values. The time-getting part is impure, but the hour decomposition could be tested if extracted.

3. **Wallpaper workflow orchestration** (lines 249-261): The save-then-set-wallpaper sequence. `do_set_wallpaper()` is already extracted but `on_set_wallpaper` closure glue is not.

4. **Config application + deferred index pattern** (lines 381-402, 407-428): The load_defaults and reset callbacks both use a repeated pattern of "apply config, then defer ComboBox updates." This pattern is duplicated and cannot be tested.

5. **Startup mode branching logic** (lines 501-548): The decision tree for which event loop API to call. The boolean logic (`is_render`, `use_tray`) is trivial, but the tray setup sequence is untestable.

6. **Render timer polling logic** (lines 477-496): The "poll textures_ready then export" sequence. This is covered by the E2E test but not a unit test.

### What Could Become Testable With Extraction

| Logic | Extraction Target | Test Type |
|---|---|---|
| Tilt-corrected globe rotation (drag_globe) | `fn apply_globe_drag(lon: f32, lat: f32, tilt: f32, zoom: f32, dx: f32, dy: f32) -> (f32, f32)` | Unit test with known inputs |
| Frame drag sensitivity | `fn apply_frame_drag(offset_x: f32, offset_y: f32, zoom: f32, dx: f32, dy: f32) -> (f32, f32)` | Unit test |
| Orientation drag | `fn apply_orient_drag(yaw: f32, pitch: f32, dx: f32, dy: f32) -> (f32, f32)` | Unit test |
| Tilt drag with wrapping | `fn apply_tilt_drag(tilt: f32, dx: f32) -> f32` | Unit test |
| Zoom scroll | `fn apply_zoom_scroll(zoom: f32, delta: f32) -> f32` | Unit test |
| Longitude wrapping | `fn wrap_longitude(lon: f32) -> f32` | Unit test (currently inline) |
| Config-to-window / window-to-config | Already extracted as helper functions | Already testable (but require Slint testing backend) |
| Deferred ComboBox index pattern | `fn defer_combobox_indices(window: &Weak<MainWindow>, aa_index: i32, texture_index: i32)` | Reduces duplication, testable with Slint backend |
| Startup mode decision | `fn startup_mode(is_render: bool, windowed: bool) -> StartupMode` | Unit test |


## E. Natural Module Boundaries

Based on the callback groupings and shared state analysis, the following module boundaries emerge:

### 1. `app_callbacks` or `ui_callbacks` module

**Contains:** All callback registration functions, organized by category.

Sub-groups:
- **Mouse interaction:** `register_mouse_callbacks(window, ...)` covering drag_globe, drag_frame, drag_orient, drag_tilt, scroll. Each callback's math would be an extracted pure function.
- **UI change notifications:** `register_change_callbacks(window, base_year)` covering sliders_changed, datetime_override_toggled, msaa_changed, texture_changed.
- **Actions:** `register_action_callbacks(window, aa_counts)` covering set_wallpaper, apply_preset, load_defaults, reset.

### 2. `app_setup` or `startup` module

**Contains:** Window initialization logic.

- `init_wgpu(force_software: bool) -> WgpuContext` (already exists as `wgpu_init::init`)
- `init_window(wgpu_context: &WgpuContext) -> MainWindow`
- `apply_initial_config(window: &MainWindow, cli: &Cli)`
- `init_ui_models(window: &MainWindow, aa_counts: &[u32], base_year: i32, end_year: i32)`
- `init_texture_system(window: &MainWindow, cli: &Cli, aa_counts: Vec<u32>) -> Arc<AtomicBool>`

### 3. `mouse_math` or `interaction` module

**Contains:** Pure functions for mouse interaction math.

- `apply_globe_drag(...) -> (f32, f32)` -- tilt-corrected rotation
- `apply_frame_drag(...) -> (f32, f32)` -- offset adjustment
- `apply_orient_drag(...) -> (f32, f32)` -- yaw/pitch adjustment
- `apply_tilt_drag(...) -> f32` -- tilt with wrapping
- `apply_zoom_scroll(...) -> f32` -- zoom clamping
- `wrap_longitude(f32) -> f32` -- reusable wrapping

### 4. `startup_mode` module (or expand `tray.rs`)

**Contains:** Mode selection and event loop dispatch.

- `enum StartupMode { Render, Tray, Windowed }`
- `fn determine_mode(is_render: bool, windowed: bool) -> StartupMode`
- `fn run(mode: StartupMode, window: MainWindow, ...)`

### 5. Keep existing helpers in `main.rs`

`apply_config_to_window`, `read_config_from_window`, `update_datetime_labels`, `save_render_png`, `do_set_wallpaper` are already well-extracted. They could remain in `main.rs` or move to `config.rs` / a new `ui_sync.rs`.


## F. Helper Function Analysis

### `apply_config_to_window(window: &MainWindow, config: &AppConfig)` (line 573)

- **Parameters:** Slint window reference, AppConfig reference
- **Return:** `()`
- **Body:** 32 setter calls mapping config fields to window properties. Includes gamma slider conversion via `gamma_value_to_slider()`. Calls `update_datetime_labels()` at the end.
- **Method potential:** Could be a method on an `AppState` struct, or remain a free function. It is already well-extracted and called from 3 places (startup, load_defaults, reset).
- **Concern:** The function is tightly coupled to both `AppConfig` and `MainWindow` -- it is essentially the "config -> UI" bridge. A trait or method on a bridge struct could make this more composable.

### `read_config_from_window(window: &MainWindow, aa_counts: &[u32]) -> AppConfig` (line 620)

- **Parameters:** Slint window reference, AA counts slice
- **Return:** `AppConfig`
- **Body:** 34 getter calls constructing an `AppConfig`. Includes gamma slider conversion via `gamma_slider_to_value()`. Also reads window geometry.
- **Method potential:** Same as above -- the "UI -> config" bridge. Could be a method on a bridge struct.
- **Called from:** `on_set_wallpaper` callback only (line 253)

### `update_datetime_labels(window: &MainWindow, base_year: i32)` (line 674)

- **Parameters:** Slint window reference, base year constant
- **Return:** `()`
- **Body:** Reads hour/day/year from window, formats labels, handles leap year transitions, writes labels back.
- **Method potential:** Could be part of the callback module. It's pure UI synchronization logic.
- **Called from:** `on_sliders_changed`, `on_datetime_override_toggled`, `apply_config_to_window`

### `save_render_png(path: &Path, width: u32, height: u32, pixels: &[u8]) -> Result<(), String>` (line 694)

- **Parameters:** Output path, dimensions, pixel buffer
- **Return:** `Result<(), String>`
- **Body:** Creates `ImageBuffer` from raw pixels, saves as PNG via the `image` crate.
- **Method potential:** This is a pure utility function. Could move to `renderer` module or a shared `export` module. It has no Slint dependency.
- **Called from:** render timer closure (line 484)

### `do_set_wallpaper() -> Result<(), String>` (line 710, `#[cfg(windows)]`)

- **Parameters:** None
- **Return:** `Result<(), String>`
- **Body:** Orchestrates the wallpaper export pipeline: get monitor resolution, render, save, apply. Calls into `wallpaper::*` and `renderer::export_wallpaper_image`.
- **Method potential:** Could move to `wallpaper.rs` as the top-level orchestration function. Currently in main.rs because it calls both `wallpaper::*` and `renderer::*`.
- **Called from:** `on_set_wallpaper` callback (line 255)


## G. Startup Mode Divergence

### Mode 1: Render (`render` subcommand)

**Code path:**
1. CLI parsing (shared) -- line 118
2. wgpu init (shared) -- line 129
3. Window creation (shared) -- line 137
4. Config load from `--config` path if provided, else default path -- line 143-146
5. All shared setup (AA options, textures, callbacks, renderer) -- lines 154-444
6. Cloud fetcher spawn (shared) -- line 447
7. Sun timer (shared) -- line 456
8. **Render timer created** -- lines 471-499: Polls `textures_ready`, renders, saves PNG, calls `quit_event_loop()`
9. `is_render = true`, `use_tray = false` -- line 505
10. No single-instance enforcement -- line 515 (None)
11. No tray icon -- line 521 (None)
12. `window.run()` (standard event loop) -- line 548
13. Shutdown (shared) -- lines 550-564

**Unique characteristics:**
- Creates a render timer that polls and exits
- Loads config from a custom `--config` path
- No user interaction expected (headless-ish)
- Could theoretically skip callback registration (but currently registers them)

### Mode 2: Tray (default on Windows)

**Code path:**
1-7. Same as render mode (shared setup)
8. No render timer -- line 498 (None)
9. `is_render = false`, `use_tray = true` -- line 505
10. **Single-instance enforcement** via OS mutex -- line 516 (may exit here if another instance runs)
11. **Window on_close_requested** handler: save geometry, return `HideWindow` -- lines 525-535
12. **Tray thread spawned** with context menu and message pump -- line 536
13. `window.show()` + `slint::run_event_loop_until_quit()` -- lines 545-546
14. Shutdown (shared) -- lines 550-564

**Unique characteristics:**
- Event loop stays alive after window hide (using `run_event_loop_until_quit`)
- Window geometry saved on both hide (on_close_requested) and final shutdown
- Tray thread provides Open/Exit menu and left-click show

### Mode 3: Windowed (`--windowed` flag)

**Code path:**
1-7. Same as render mode (shared setup)
8. No render timer -- line 498 (None)
9. `is_render = false`, `use_tray = false` -- line 505
10. No single-instance enforcement -- line 515 (None)
11. No tray icon -- line 521 (None)
12. `window.run()` (standard event loop, closes on window close) -- line 548
13. Shutdown (shared) -- lines 550-564

**Unique characteristics:**
- Simplest mode: close button exits the process
- No tray, no single-instance
- Same event loop API as render mode

### Shared vs. Mode-Specific Code

| Code Section | Render | Tray | Windowed |
|---|---|---|---|
| CLI parse, logging | shared | shared | shared |
| wgpu init | shared | shared | shared |
| Window creation | shared | shared | shared |
| Config load | custom path | default path | default path |
| AA/texture/model setup | shared | shared | shared |
| All 14 UI callbacks | shared (unused) | shared | shared |
| Renderer setup | shared | shared | shared |
| Cloud fetcher | shared | shared | shared |
| Sun timer | shared | shared | shared |
| Render timer | **yes** | no | no |
| Single-instance | no | **yes** | no |
| Tray icon | no | **yes** | no |
| on_close_requested | no | **yes** (HideWindow) | no |
| Event loop API | `window.run()` | `show()` + `run_event_loop_until_quit()` | `window.run()` |
| Geometry save on hide | no | **yes** | no |

The mode-specific code is concentrated in Section 17 (lines 501-548) and Section 16 (lines 468-499), totaling about 80 lines. The remaining ~370 lines are shared across all modes.


## H. Existing Test Coverage

### `tests/e2e.rs`

**Pattern:** Black-box testing of the compiled binary via `Command::new(BINARY)`.

**Test inventory:**
1. `test_binary_exists` -- verifies the binary was compiled
2. `test_render_and_exit` -- the primary E2E test:
   - Spawns binary with `render` subcommand, custom config, 800x800 output
   - Waits up to 60 seconds for exit
   - Validates: exit code 0, output file exists and non-empty
   - Opens the PNG and validates pixel colors at geographic locations (Europe center is green, Sahara is yellow, Atlantic is blue, Greenland is white, Indian Ocean at night is dark, Tibet at night is dim blue, corners are black)
   - Parses stderr for memory usage entries, validates early memory < 300 MB, peak < 3 GB, exit memory < 1 GB and settled below peak
   - Checks for ERROR lines in stderr
3. `test_tray_mode_starts_and_can_be_killed` -- spawns default (tray) mode, waits 3s, kills, checks it was alive and has startup banner, no errors
4. `test_windowed_mode_starts` -- same pattern but with `--windowed`
5. `test_single_instance_second_exits` -- spawns two tray-mode instances, verifies second exits immediately with "already running" message

**Key infrastructure:**
- `wait_with_timeout()` helper with kill-on-timeout
- `spawn_run_and_kill()` for long-running modes
- `parse_memory_entries()` for structured log parsing
- Pixel assertion helpers: `assert_black`, `assert_greenish`, `assert_yellowish`, `assert_blue`, `assert_ice`, `assert_night_ocean`, `assert_night_land`
- All tests are `#[ignore]` and `#[serial]`
- Uses `tests/fixtures/e2e_config.toml` for deterministic rendering

### `tests/slint_ui.rs`

**Pattern:** Headless Slint UI tests using `i-slint-backend-testing`.

**Test inventory:**
1. `test_window_creates_successfully` -- baseline: window creation works
2. `test_camera_longitude_roundtrip` -- set/get property verification
3. `test_camera_latitude_roundtrip` -- same
4. `test_camera_zoom_roundtrip` -- same
5. `test_bool_property_roundtrip` -- diffuse_shading toggle
6. `test_int_property_roundtrip` -- texture_index
7. `test_preset_europe_fires_callback` -- click Europe button, verify callback receives index 0
8. `test_preset_earthrise_fires_callback` -- click Earthrise button, verify index 8
9. `test_preset_changes_camera_properties` -- register a preset handler, click Europe, verify properties changed
10. `test_load_defaults_fires_callback` -- register handler with AppConfig::default(), click Load Defaults, verify properties reset
11. `test_load_defaults_resets_multiple_properties` -- same but tests multiple fields
12. `test_advanced_section_starts_closed` -- verifies `advanced-open` defaults to false and longitude slider is absent
13. `test_advanced_section_opens` -- sets `advanced-open = true`, verifies slider appears
14. `test_advanced_section_closes` -- toggle open then closed, slider gone
15. `test_atmosphere_sliders_hidden_when_disabled` -- rayleigh slider absent when atmo_enabled is false
16. `test_atmosphere_sliders_visible_when_enabled` -- rayleigh slider present when atmo_enabled is true

**Key infrastructure:**
- `init()` with thread-local guard for `i_slint_backend_testing::init_no_event_loop()`
- `create_window()` sets a large physical height (5000px) so ScrollView content materializes
- Uses `ElementHandle::find_by_accessible_label()` and `find_by_element_id()` for element discovery
- Uses `invoke_accessible_default_action()` to simulate clicks
- Tests are NOT `#[ignore]` -- they run as part of `cargo test`
- Tests duplicate callback logic from main.rs (register preset/defaults handlers manually)

### Coverage Gaps Relevant to Refactoring

- **Mouse drag math:** No unit tests exist for the tilt-corrected rotation, sensitivity scaling, wrapping, or clamping logic
- **Datetime initialization:** No test verifies the "toggle custom datetime on" behavior (setting sliders to current UTC)
- **Config <-> Window sync:** `apply_config_to_window` and `read_config_from_window` have no dedicated tests (they are exercised indirectly by E2E and Slint UI tests)
- **Deferred ComboBox pattern:** The invoke_from_event_loop + set_aa_index/texture_index pattern is untested
- **Startup mode branching:** No unit test for the `is_render / use_tray` decision logic (covered only by E2E)
- **`update_datetime_labels`:** No unit test for leap year clamping or label formatting (the pure functions it calls are tested, but the orchestration is not)


## Appendix: Module and Public API Surface Summary

### `src/config.rs` -- Public API

- `AppConfig` struct (35 fields, `Serialize`/`Deserialize`, `Default`)
- `config_path() -> Option<PathBuf>`
- `load_config() -> AppConfig`
- `load_config_from(path: &Path) -> AppConfig`
- `save_config(config: &AppConfig)`
- `save_window_geometry(x, y, width, height)`
- `validated_window_geometry(config: &AppConfig) -> Option<(i32, i32, u32, u32)>`
- `find_sample_count_index(aa_counts: &[u32], desired: u32) -> i32`

### `src/scene/camera.rs` -- Public API

- `CameraParams` struct (8 fields: longitude, latitude, zoom, offset_x, offset_y, tilt_deg, yaw_deg, pitch_deg)
- `PRESETS: [CameraParams; 9]`
- `zoom_to_distance(t: f32) -> f32`
- `distance_to_zoom(d: f32) -> f32`
- `OrbitalCamera` struct with `new()`, `eye_position()`, `view_matrix()`, `projection_matrix()`, `mvp_matrix()`

### `src/scene/datetime.rs` -- Public API

- `base_year() -> i32`
- `year_range() -> (i32, i32)`
- `is_leap_year(year: i32) -> bool`
- `days_in_year(year: i32) -> u16`
- `day_of_year_to_month_day(doy: u16, year: i32) -> (u8, u8)`
- `month_day_label(doy: u16, year: i32) -> String`
- `hour_float_to_hm(h: f32) -> (u8, u8)`
- `hour_label(h: f32) -> String`
- `hour_float_to_hms(h: f32) -> (i32, i32, f64)`

### `src/renderer/mod.rs` -- Public API

- `build_aa_options(supported: &[u32]) -> (Vec<SharedString>, Vec<u32>, i32)`
- `gamma_slider_to_value(t: f32) -> f32`
- `gamma_value_to_slider(gamma: f32) -> f32`
- `setup_rendering_notifier(window, aa_counts, texture_paths, texture_tx, texture_rx, textures_ready)`
- `export_wallpaper_image(width, height) -> Result<Vec<u8>, String>`
- `read_texture_rgba8(device, queue, texture, width, height) -> Vec<u8>` (re-export from render_pass)
- `DecodedTextureMessage` (re-export from textures)

### `src/tray.rs` -- Public API

- `create_icon() -> Icon`
- `enforce_single_instance() -> SingleInstance`
- `spawn_tray_thread(window_weak: Weak<MainWindow>) -> JoinHandle<()>`

### `src/cloud_fetcher.rs` -- Public API

- `spawn_cloud_fetcher(tx: Sender<DecodedTextureMessage>, window_weak: Weak<MainWindow>, clouds_slot: usize)`

### `src/wallpaper.rs` -- Public API (Windows only)

- `wallpaper_dir() -> Result<PathBuf, String>`
- `get_primary_monitor_resolution() -> Result<(u32, u32), String>`
- `set_wallpaper(path: &Path) -> Result<(), String>`
- `save_wallpaper_image(pixels: &[u8], width: u32, height: u32) -> Result<PathBuf, String>`

### `src/memory.rs` -- Public API

- `MemorySnapshot` struct (rss_bytes, peak_rss_bytes, private_bytes)
- `snapshot() -> Option<MemorySnapshot>`
- `log_memory_usage(context: &str)`

### `src/wgpu_init.rs` -- Public API

- `WgpuContext` struct (config, adapter_info, supported_sample_counts)
- `init(force_software: bool) -> WgpuContext`

### `src/texture_loader.rs` -- Public API

- `DecodedImage` struct (pixels, width, height)
- `register_jxl_hook()`
- `load(path: &Path) -> Result<DecodedImage, String>`
- `shift_horizontal(pixels: &mut [u8], width: u32, height: u32)` (pub(crate))
- `resolve_textures_dir(cli_override: Option<&Path>) -> Option<PathBuf>`

### `ui/main.slint` -- Callbacks

| Callback | Parameters | Direction |
|---|---|---|
| `sliders-changed` | none | out (Slint -> Rust) |
| `msaa-changed` | none | out |
| `texture-changed` | none | out |
| `set-wallpaper` | none | out |
| `mouse-drag-globe` | `(float, float)` | out |
| `mouse-drag-frame` | `(float, float)` | out |
| `mouse-drag-orient` | `(float, float)` | out |
| `mouse-drag-tilt` | `(float, float)` | out |
| `mouse-scroll` | `(float)` | out |
| `apply-preset` | `(int)` | out |
| `load-defaults` | none | out |
| `reset` | none | out |
| `datetime-override-toggled` | none | out |

### `ui/main.slint` -- Properties

| Property | Type | Direction | Notes |
|---|---|---|---|
| `rendered-image` | `image` | in | Set by renderer each frame |
| `renderer-info` | `string` | in | Adapter description |
| `aa-options` | `[string]` | in | ComboBox model |
| `aa-index` | `int` | in-out | Selected AA mode |
| `texture-options` | `[string]` | in | ComboBox model |
| `texture-index` | `int` | in-out | Selected texture mode |
| `loading-text` | `string` | in | Status text overlay |
| `camera-longitude` | `float` | in-out | Slider <=> Rust |
| `camera-latitude` | `float` | in-out | |
| `camera-zoom` | `float` | in-out | |
| `camera-offset-x` | `float` | in-out | |
| `camera-offset-y` | `float` | in-out | |
| `camera-tilt` | `float` | in-out | |
| `camera-yaw` | `float` | in-out | |
| `camera-pitch` | `float` | in-out | |
| `viewport-width` | `length` | out | Read by renderer |
| `viewport-height` | `length` | out | |
| `terminator-width` | `float` | in-out | |
| `diffuse-shading` | `bool` | in-out | |
| `diffuse-floor` | `float` | in-out | |
| `diffuse-ramp` | `float` | in-out | |
| `spec-shininess` | `float` | in-out | |
| `spec-intensity` | `float` | in-out | |
| `fresnel-mix` | `float` | in-out | |
| `fresnel-exp` | `float` | in-out | |
| `day-gamma` | `float` | in-out | Normalized 0-1, converted in Rust |
| `day-saturation` | `float` | in-out | |
| `night-gamma` | `float` | in-out | |
| `night-saturation` | `float` | in-out | |
| `cloud-opacity` | `float` | in-out | |
| `cloud-floor` | `float` | in-out | |
| `cloud-gamma` | `float` | in-out | |
| `atmo-enabled` | `bool` | in-out | |
| `rayleigh-intensity` | `float` | in-out | |
| `rayleigh-sharpness` | `float` | in-out | |
| `rayleigh-haze` | `float` | in-out | |
| `nightglow-intensity` | `float` | in-out | |
| `nightglow-balance` | `float` | in-out | |
| `nightglow-falloff` | `float` | in-out | |
| `zoom-display-distance` | `float` | in | Computed by renderer |
| `use-custom-datetime` | `bool` | in-out | |
| `custom-hour` | `float` | in-out | |
| `custom-day-of-year` | `float` | in-out | |
| `custom-year-index` | `int` | in-out | |
| `year-options` | `[string]` | in | ComboBox model |
| `hour-label` | `string` | in | Formatted by Rust |
| `day-label` | `string` | in | Formatted by Rust |
| `max-day-of-year` | `int` | in | |
| `advanced-open` | `bool` | in-out | Collapsible section state |
