# main() Refactoring Plan

Based on [2026-03-25-main-refactoring-research.md](2026-03-25-main-refactoring-research.md).

## Summary

Break up the 448-line `main()` function in `src/main.rs` (currently suppressing `clippy::too_many_lines`) into focused modules. Goals:
1. Compartmentalize code into logical modules
2. Reduce stack pressure (fewer locals in main's frame)
3. Enable unit testing of pure math currently trapped in closures
4. Remove the `clippy::too_many_lines` suppression

This is a purely structural refactoring with no behavior changes.

## Stakes: Medium-Low

Every step can be verified with `cargo test` and `cargo clippy`. The existing E2E tests (`test_render_and_exit` with pixel validation) provide a strong safety net. Risk is almost entirely in accidentally changing behavior during extraction.

## Design Decisions

**Module structure**: Two new library modules:
- **`src/mouse_math.rs`** — Pure functions for mouse interaction math. No Slint dependency. Fully unit-testable.
- **`src/ui_callbacks.rs`** — Callback registration functions grouped by category. Also contains the config-to-window bridge functions (`apply_config_to_window`, `read_config_from_window`, `update_datetime_labels`) and the deferred ComboBox index helper (currently copy-pasted 3 times).

**No shared state struct**: The research showed a struct holding `Weak<MainWindow>` + `aa_counts` + `base_year` could unify closures, but the indirection isn't worth it. Moving registration to `ui_callbacks` solves the line-count problem without a new type.

**`mouse_math` separate from `scene/camera`**: The mouse math functions are about UI interaction, not camera geometry. They depend on `scene::camera::zoom_to_distance` but belong in their own module.

**Helper functions move to `ui_callbacks`**: `apply_config_to_window`, `read_config_from_window`, `update_datetime_labels` are only used by callbacks and by main's startup code. Moving them to the library crate lets both `main.rs` and `ui_callbacks.rs` use them. `do_set_wallpaper` and `save_render_png` also move since they're called from callbacks or startup mode logic.

**Stack reduction**: Natural from moving initialization into sub-functions (locals live in sub-function frames, not main's). No boxing needed — the research confirmed ~1,200 bytes of locals and that prior stack overflows were from deep call chains.

**Render mode**: Not optimizing callback registration for render mode — the callbacks are just function pointer registrations, not allocations. Not worth the code path difference.

## Steps

### Phase 1: Extract Mouse Math (Pure Functions)

#### Step 1.1: Create `src/mouse_math.rs`

**Files**: `src/mouse_math.rs` (new), `src/lib.rs`

Extract these pub functions from the closure bodies in `main.rs`:
- `wrap_longitude(lon: f32) -> f32` — the `((lon + 180.0) % 360.0 + 360.0) % 360.0 - 180.0` pattern
- `apply_globe_drag(lon: f32, lat: f32, tilt_deg: f32, zoom: f32, dx: f32, dy: f32) -> (f32, f32)` — returns new (longitude, latitude). Uses `zoom_to_distance` from `scene::camera`.
- `apply_frame_drag(offset_x: f32, offset_y: f32, zoom: f32, dx: f32, dy: f32) -> (f32, f32)` — returns new (offset_x, offset_y)
- `apply_orient_drag(yaw: f32, pitch: f32, dx: f32, dy: f32) -> (f32, f32)` — returns new (yaw, pitch)
- `apply_tilt_drag(tilt: f32, dx: f32) -> f32` — returns new tilt
- `apply_zoom_scroll(zoom: f32, delta: f32) -> f32` — returns new zoom

Add `pub mod mouse_math;` to `src/lib.rs`. Do NOT modify `main.rs` yet.

**Verify**: `cargo test` and `cargo clippy` pass. New module compiles but is unused.

#### Step 1.2: Add unit tests for mouse math

**Files**: `src/mouse_math.rs` (add `#[cfg(test)]` module)

Tests for each function:
- `wrap_longitude`: boundaries (-180, 180, 0, 360, -360, 540)
- `apply_globe_drag`: tilt correction, zoom sensitivity scaling, latitude clamping at ±89, longitude wrapping
- `apply_frame_drag`: clamping at ±3.0, zoom sensitivity
- `apply_orient_drag`: clamping at ±90.0
- `apply_tilt_drag`: wrapping at ±180
- `apply_zoom_scroll`: clamping at 0.0 and 1.0
- Use `approx::assert_relative_eq!` per conventions
- Use `proptest` for `wrap_longitude` (output always in [-180, 180]) and `apply_zoom_scroll` (output always in [0, 1])

**Verify**: `cargo test mouse_math` passes.

#### Step 1.3: Wire mouse math into main.rs closures

**Files**: `src/main.rs`

Replace inline math in the 5 mouse callbacks with calls to `mouse_math::*`. Example:
```rust
window.on_mouse_drag_globe(move |dx, dy| {
    let Some(win) = window_weak.upgrade() else { return };
    let (lon, lat) = sunlit_earth::mouse_math::apply_globe_drag(
        win.get_camera_longitude(), win.get_camera_latitude(),
        win.get_camera_tilt(), win.get_camera_zoom(), dx, dy,
    );
    win.set_camera_longitude(lon);
    win.set_camera_latitude(lat);
    win.window().request_redraw();
});
```

**Verify**: `cargo test` and `cargo clippy` pass. Manual smoke test: launch app, drag globe, verify rotation with tilt correction.

### Phase 2: Extract UI Callbacks Module

#### Step 2.1: Create `src/ui_callbacks.rs`

**Files**: `src/ui_callbacks.rs` (new), `src/lib.rs`

Three public registration functions, plus helper functions moved from `main.rs`:

**`pub fn register_mouse_callbacks(window: &MainWindow)`**
Registers: `on_mouse_drag_globe`, `on_mouse_drag_frame`, `on_mouse_drag_orient`, `on_mouse_drag_tilt`, `on_mouse_scroll`, `on_apply_preset`.

**`pub fn register_change_callbacks(window: &MainWindow, base_year: i32)`**
Registers: `on_sliders_changed`, `on_datetime_override_toggled`, `on_msaa_changed`, `on_texture_changed`.

**`pub fn register_action_callbacks(window: &MainWindow, aa_counts: &[u32])`**
Registers: `on_set_wallpaper`, `on_load_defaults`, `on_reset`.

**Moved functions (pub):**
- `apply_config_to_window(window: &MainWindow, config: &AppConfig)`
- `read_config_from_window(window: &MainWindow, aa_counts: &[u32]) -> AppConfig`
- `update_datetime_labels(window: &MainWindow, base_year: i32)`
- `save_render_png(path: &Path, width: u32, height: u32, pixels: &[u8]) -> Result<(), String>`
- `do_set_wallpaper() -> Result<(), String>` (with `#[cfg(windows)]`)

**Deduplicated helper:**
```rust
pub fn defer_combobox_indices(window_weak: &slint::Weak<MainWindow>, aa_index: i32, texture_index: i32)
```
Replaces the 3 identical copies (startup, load_defaults, reset).

Add `pub mod ui_callbacks;` to `src/lib.rs`. Do NOT modify `main.rs` yet.

**Verify**: `cargo test` and `cargo clippy` pass. Module compiles but is unused.

#### Step 2.2: Wire main.rs to use `ui_callbacks`

**Files**: `src/main.rs`

Replace all callback registration code (~220 lines) with three calls:
```rust
sunlit_earth::ui_callbacks::register_change_callbacks(&window, base_year);
sunlit_earth::ui_callbacks::register_mouse_callbacks(&window);
sunlit_earth::ui_callbacks::register_action_callbacks(&window, &aa_counts);
```

Replace startup deferred ComboBox code with:
```rust
sunlit_earth::ui_callbacks::defer_combobox_indices(
    &window.as_weak(), config_aa_index, config_texture_index,
);
```

Replace calls to moved helper functions with `ui_callbacks::*` paths. Remove unused imports.

**Verify**: `cargo test` and `cargo clippy` pass. Manual smoke test: all mouse interactions, wallpaper button, load defaults, reset, presets.

### Phase 3: Extract Initialization Helpers

#### Step 3.1: Extract setup sub-functions in main.rs

**Files**: `src/main.rs`

Extract remaining initialization sequences into local helper functions:

**`fn init_ui_models(window: &MainWindow, config: &AppConfig, aa_counts: &[u32], base_year: i32, end_year: i32)`**
Contains: texture options setup, year ComboBox setup, initial config application with deferred index updates.

**`fn init_texture_system(window: &MainWindow, textures_dir: Option<&Path>, aa_counts: Vec<u32>, texture_paths: Vec<Option<PathBuf>>) -> Arc<AtomicBool>`**
Contains: mpsc channel creation, `setup_rendering_notifier`, cloud fetcher spawn. Returns `textures_ready`.

**`fn run_event_loop(window: MainWindow, is_render: bool, windowed: bool, textures_ready: Arc<AtomicBool>, cli_command: Option<Commands>)`**
Contains: sun timer creation, optional render timer, startup mode branching (tray/windowed/render), single-instance enforcement, tray thread spawn, event loop execution, geometry save, memory logging, `process::exit(0)`. Timer variables and guard variables (`_instance_guard`, `_tray_handle`) live in this function's scope.

**Verify**: `cargo test` and `cargo clippy` pass. `main()` is now ~80-120 lines.

#### Step 3.2: Remove `#[allow(clippy::too_many_lines)]`

**Files**: `src/main.rs`

Remove the suppression attribute. Verify clippy is happy.

**Verify**: `cargo clippy` passes without the suppression. `cargo test` passes.

### Phase 4: Documentation

#### Step 4.1: Update CLAUDE.md and roadmap

**Files**: `CLAUDE.md`, `docs/roadmap.md`

Update "Key modules" section to include `mouse_math.rs` and `ui_callbacks.rs`. Update `main.rs` description to reflect it is now a thin orchestrator. Check off any relevant roadmap items.

**Verify**: Documentation accurately describes the new structure.

## Step Dependencies

```
1.1 (create mouse_math)
 |
1.2 (add tests) ─── depends on 1.1
 |
1.3 (wire into main.rs) ─── depends on 1.2
 |
2.1 (create ui_callbacks) ─── depends on 1.1
 |
2.2 (wire main.rs) ─── depends on 2.1, 1.3
 |
3.1 (extract init helpers) ─── depends on 2.2
 |
3.2 (remove clippy suppression) ─── depends on 3.1
 |
4.1 (docs) ─── depends on 3.2
```

Steps 1.1 + 1.2 can be combined. Steps 4.1 is trivial cleanup.

## Risk Assessment

**Highest risk**: Step 2.2 (wiring main.rs to use ui_callbacks). Most code moves at once, capture errors most likely. Mitigation: careful diff review, E2E tests, manual smoke test.

**Second highest**: Step 3.1 (extracting run_event_loop). Timer lifetimes and `process::exit(0)` must survive. Guard variables (`_guard`, `_instance_guard`, `_tray_handle`) must remain alive until event loop exits. Mitigation: these variables live in `run_event_loop`'s scope.

**Low risk**: Steps 1.1-1.3 (mouse math). Pure functions with known I/O. Unit tests validate correctness before wiring.

## Success Criteria

- [ ] `main()` is under 200 lines (well within clippy's default threshold)
- [ ] `#[allow(clippy::too_many_lines)]` removed from `main()`
- [ ] `cargo test` passes (all existing tests)
- [ ] `cargo clippy` passes with no new suppressions
- [ ] Mouse drag/scroll math has unit tests
- [ ] Deferred ComboBox index pattern deduplicated (was 3 copies, now 1)
- [ ] CLAUDE.md and roadmap.md updated
