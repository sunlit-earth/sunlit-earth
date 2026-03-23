# Plan: UI Overhaul (2026-03-23)

## Summary

Reorganize the Sunlit Earth control panel so that the most common actions (setting wallpaper, choosing a camera preset) are immediately accessible at the top, while all advanced tuning controls are collapsed by default behind a single toggle. Replace auto-save with explicit save-on-wallpaper semantics, add a 3x3 camera preset grid, and provide Load Defaults / Reset buttons for deliberate state management.

## Stakes Classification

**Level**: Medium

**Rationale**: The changes touch three files (`ui/main.slint`, `src/main.rs`, `src/config.rs`) and affect UI layout, callback wiring, and save behavior. However, the rendering pipeline, shaders, and camera math are completely untouched. The existing test suite covers the config module and GPU pipeline, so regressions are detectable. Rollback is straightforward since the changes are confined to well-defined layers.

## Context

**Research**: `docs/plans/2026-03-23-ui-overhaul-research.md`

**Affected files**:

| File | Nature of change |
| --- | --- |
| `ui/main.slint` | Major restructure: reorder sections, add preset grid, add collapsible advanced section, move buttons, add new callbacks |
| `src/main.rs` | New callback handlers (`apply-preset`, `load-defaults`, `reset`), remove debounce timer and auto-save, modify `set-wallpaper` to save config first, remove save-on-exit |
| `src/config.rs` | No structural changes; the existing `AppConfig::default()` and `load_config()` are sufficient for the new Load Defaults and Reset flows |

**Files unchanged**: `src/renderer/*`, `src/scene/camera.rs`, `src/scene/sun.rs`, `shaders/*.wgsl`, `src/wallpaper.rs`, `src/cloud_fetcher.rs`

## Success Criteria

- [ ] "Set as Wallpaper" button is the first control in the panel, saves config to disk before applying the wallpaper
- [ ] "Load Defaults" button restores all settings to `AppConfig::default()` values without saving to disk
- [ ] "Reset" button reloads config from disk and restores all UI elements to the last-saved state
- [ ] 3x3 camera preset grid with buttons in the specified order (Europe, N. America, S. America, Africa, Asia, Oceania, Pacific, Blue Marble, Earthrise)
- [ ] Presets only change camera values (longitude, latitude, zoom, tilt, yaw, pitch, offset_x, offset_y) and do not reset any other settings
- [ ] All advanced controls are behind a single collapsible toggle that starts closed
- [ ] Existing `if`-gated sub-sections (atmosphere enable, date/time custom) still hide their children when unchecked
- [ ] Individual subsections within the advanced section do NOT collapse on their own
- [ ] Auto-save debounce timer is completely removed (no timer, no save-on-exit)
- [ ] The only way to persist config to disk is clicking "Set as Wallpaper"
- [ ] The `reset-all` callback is replaced by `load-defaults`; the old callback no longer exists
- [ ] `cargo build` succeeds; `cargo test` passes; `cargo clippy` passes
- [ ] UI ordering matches the specified sequence from top to bottom

## Camera Preset Data

These values are stored as a Rust-side `const` array. Presets set all 8 camera parameters; tilt, yaw, offset_x, and offset_y are 0 for all presets except Earthrise.

| Index | Label | Longitude | Latitude | Zoom | Tilt | Yaw | Pitch | Offset X | Offset Y |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 0 | Europe | 15.0 | 52.0 | 0.42 | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 |
| 1 | N. America | -100.0 | 45.0 | 0.40 | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 |
| 2 | S. America | -60.0 | -15.0 | 0.42 | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 |
| 3 | Africa | 17.0 | 2.0 | 0.42 | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 |
| 4 | Asia | 90.0 | 35.0 | 0.35 | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 |
| 5 | Oceania | 135.0 | -25.0 | 0.42 | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 |
| 6 | Pacific | -170.0 | 0.0 | 0.15 | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 |
| 7 | Blue Marble | 37.4 | -26.3 | 0.15 | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 |
| 8 | Earthrise | -12.0 | 4.0 | 0.10 | 0.0 | 0.0 | 45.0 | 0.0 | 0.0 |

## Implementation Steps

### Phase 1: Slint UI Restructure

This phase rewrites the Slint layout. No Rust changes yet, so existing callbacks will temporarily be disconnected (compile will still succeed because Slint callbacks are optional to wire).

#### Step 1.1: Add new callbacks and the `advanced-open` property to `MainWindow`

- **Files**: `ui/main.slint` (lines 69-87, the callback declarations)
- **Action**:
  - Add `callback apply-preset(int);`
  - Add `callback load-defaults();`
  - Add `callback reset();`
  - Add `in-out property <bool> advanced-open: false;`
  - Remove `callback reset-all();`
- **Verify**: `cargo build` succeeds (the `on_reset_all` call in `main.rs` will need to be temporarily commented out or will produce a compile error that is fixed in Phase 2)
- **Complexity**: Small

#### Step 1.2: Restructure the controls panel layout

- **Files**: `ui/main.slint` (lines 89-943, the entire left-panel `ScrollView` content)
- **Action**: Replace the entire `VerticalLayout` inside the `ScrollView` with the new ordering. The new structure within the `VerticalLayout` is:

  1. **"Set as Wallpaper" button** (existing, moved to top)
  2. **Wallpaper status text** (existing, moved to just below the button)
  3. **"Load Defaults" and "Reset" buttons** side by side in a `HorizontalLayout`
     - "Load Defaults" button: `clicked => { root.load-defaults(); }`
     - "Reset" button: `clicked => { root.reset(); }`
  4. **3x3 preset grid** using `GridLayout` with `spacing: 4px`:
     - Row 1: Europe (0), N. America (1), S. America (2)
     - Row 2: Africa (3), Asia (4), Oceania (5)
     - Row 3: Pacific (6), Blue Marble (7), Earthrise (8)
     - Each button: `clicked => { root.apply-preset(N); }`
  5. **Advanced toggle**: A `TouchArea` header row with a Text showing ">" or "v" and "Advanced" label. `clicked => { root.advanced-open = !root.advanced-open; }`
  6. **`if root.advanced-open :` block** containing a single `VerticalLayout` with all GroupBox sections in this order:
     - Camera Position
     - Camera Orientation
     - Framing (renamed from "Camera Framing" in the UI ordering spec -- keep the existing "Framing" GroupBox title)
     - Date / Time
     - Clouds
     - Atmosphere
     - Lighting
     - Color Correction
     - Rendering
     - Renderer info text (existing, moved inside advanced section at the bottom)
  7. Remove the old "Reset All" button (line 919-922)
  8. Remove the spacer `Rectangle` elements that were between the old buttons and renderer info

  All GroupBox contents remain exactly as they are today. The only change is their ordering and nesting inside the `if root.advanced-open` block. The existing `if root.atmo-enabled` and `if root.use-custom-datetime` conditional blocks within Atmosphere and Date/Time sections remain unchanged.

- **Verify**: `cargo build` succeeds. Launch the app (`cargo run`) and confirm:
  - "Set as Wallpaper" is at the top
  - "Load Defaults" and "Reset" are side by side below it
  - 3x3 grid of 9 labeled preset buttons is visible
  - Clicking "Advanced" shows/hides all GroupBox sections
  - Advanced section starts closed
  - The Atmosphere and Date/Time internal collapsing still works when their checkboxes are toggled
- **Complexity**: Large

### Phase 2: Rust Callback Wiring

This phase implements the Rust-side handlers for the new callbacks and removes the old auto-save machinery.

#### Step 2.1: Define the `PRESETS` constant array

- **Files**: `src/main.rs` (add near the top, after imports)
- **Action**: Add a `const PRESETS: [CameraParams; 9]` array using the values from the Camera Preset Data table above. Each element is a `CameraParams` struct literal. The `CameraParams` struct already has all 8 fields needed (longitude, latitude, zoom, offset_x, offset_y, tilt_deg, yaw_deg, pitch_deg).
- **Test cases** (unit, in `src/main.rs` or a test module):
  - `PRESETS` has exactly 9 elements
  - `PRESETS[0].longitude` is `15.0` (Europe)
  - `PRESETS[6].longitude` is `-170.0` (Pacific)
  - `PRESETS[8].pitch_deg` is `45.0` (Earthrise)
  - All non-Earthrise presets have `tilt_deg == 0.0`, `yaw_deg == 0.0`, `pitch_deg == 0.0`
  - All presets have `offset_x == 0.0` and `offset_y == 0.0`
- **Verify**: `cargo test` passes with new assertions
- **Complexity**: Small

#### Step 2.2: Implement `apply-preset` callback

- **Files**: `src/main.rs` (add after the existing mouse callback registrations)
- **Action**: Register `window.on_apply_preset(move |index| { ... })`. The handler:
  1. Gets the `CameraParams` from `PRESETS[index as usize]` (with bounds check -- return early if out of range)
  2. Calls `win.set_camera_longitude(preset.longitude)`, `set_camera_latitude(preset.latitude)`, `set_camera_zoom(preset.zoom)`, `set_camera_offset_x(preset.offset_x)`, `set_camera_offset_y(preset.offset_y)`, `set_camera_tilt(preset.tilt_deg)`, `set_camera_yaw(preset.yaw_deg)`, `set_camera_pitch(preset.pitch_deg)`
  3. Calls `win.window().request_redraw()`
  4. Does NOT save config, does NOT touch any non-camera settings
- **Manual verification**:
  - Click "Europe" -- globe rotates to show Europe
  - Click "Pacific" -- globe zooms out and shows the Pacific Ocean
  - Click "Earthrise" -- globe tilts with pitch 45 degrees
  - Change a lighting slider, then click a preset -- lighting setting is preserved
- **Verify**: `cargo build` succeeds. Manual test of preset buttons in the running app
- **Complexity**: Small

#### Step 2.3: Implement `load-defaults` callback

- **Files**: `src/main.rs` (replace the existing `on_reset_all` handler)
- **Action**: Register `window.on_load_defaults(move || { ... })`. The handler:
  1. Calls `apply_config_to_window(&win, &AppConfig::default())` to restore all settings to defaults
  2. Uses `invoke_from_event_loop` to defer setting `aa_index` and `texture_index` to their default values (texture_index=3, sample_count=8 -> find the corresponding aa_index), matching the pattern used at startup
  3. Calls `update_datetime_labels(&win, base_year)`
  4. Calls `win.window().request_redraw()`
  5. Does NOT save config to disk
- **Manual verification**:
  - Change several sliders, click "Load Defaults" -- all settings return to defaults
  - The config file on disk is unchanged after clicking "Load Defaults"
  - The globe re-renders with default settings
- **Verify**: `cargo build` succeeds. Manual test in running app
- **Complexity**: Small

#### Step 2.4: Implement `reset` callback (reload from disk)

- **Files**: `src/main.rs` (add new handler)
- **Action**: Register `window.on_reset(move || { ... })`. The handler:
  1. Calls `config::load_config()` to read the current config from disk
  2. Calls `apply_config_to_window(&win, &config)` to apply it
  3. Uses `invoke_from_event_loop` to defer setting `aa_index` and `texture_index` from the loaded config (same deferred pattern as startup)
  4. Calls `update_datetime_labels(&win, base_year)`
  5. Calls `win.window().request_redraw()`
- **Manual verification**:
  - Set wallpaper (saving config), change sliders, click "Reset" -- sliders return to the last-saved state
  - If no config file exists, "Reset" behaves like "Load Defaults" (returns to `AppConfig::default()`)
- **Verify**: `cargo build` succeeds. Manual test in running app
- **Complexity**: Small

#### Step 2.5: Modify `set-wallpaper` to save config before applying

- **Files**: `src/main.rs` (lines 211-233, the existing `on_set_wallpaper` handler)
- **Action**: In the `#[cfg(windows)]` block, add `config::save_config(&read_config_from_window(&win, &aa_counts_for_save));` as the first action before calling `do_set_wallpaper()`. The `aa_counts` clone needed for this closure must be captured (add `aa_counts_for_save` clone like the existing patterns).
- **Manual verification**:
  - Change a slider, click "Set as Wallpaper" -- config file on disk is updated with the new value
  - The wallpaper is still set correctly after the save
- **Verify**: `cargo build` succeeds. Inspect config file after clicking "Set as Wallpaper" to confirm it was updated
- **Complexity**: Small

#### Step 2.6: Remove auto-save debounce timer and save-on-exit

- **Files**: `src/main.rs`
- **Action**:
  1. **Remove the `config_timer` creation** (lines 168-179): delete the `Rc::new(slint::Timer::default())` and its initial `start()`/`stop()` sequence
  2. **Remove all `config_timer_handle` clones and `config_timer_handle.restart()` calls** from every callback: `on_sliders_changed`, `on_msaa_changed`, `on_texture_changed`, `on_mouse_drag_globe`, `on_mouse_drag_frame`, `on_mouse_drag_orient`, `on_mouse_drag_tilt`, `on_mouse_scroll`. Each of these callbacks currently clones the timer handle and calls `.restart()` -- remove those lines but keep the `request_redraw()` calls
  3. **Remove the save-on-exit call** (line 428-429): delete `config::save_config(&read_config_from_window(&window, &aa_counts_for_exit));` and the `aa_counts_for_exit` clone (line 118)
  4. **Remove `drop(config_timer);`** (line 433)
  5. **Remove the `info!("event loop exited, saving config");` log line** (line 427)
  6. Keep `drop(sun_timer);` and the `std::process::exit(0)` call
- **Test cases**:
  - `cargo build` succeeds with no references to `config_timer`
  - Grep the file for `config_timer` -- zero matches
  - Grep the file for `restart()` -- zero matches
  - Grep the file for `aa_counts_for_exit` -- zero matches
- **Verify**: `cargo build` succeeds; `cargo test` passes
- **Complexity**: Medium

#### Step 2.7: Remove the old `on_reset_all` handler

- **Files**: `src/main.rs` (lines 339-387)
- **Action**: Delete the entire `window.on_reset_all(move || { ... })` block. This callback no longer exists in the Slint file (removed in Step 1.1). The `load-defaults` callback (Step 2.3) replaces it.
- **Verify**: `cargo build` succeeds; no references to `reset_all` remain in the codebase
- **Complexity**: Small

### Phase 3: Cleanup and Verification

#### Step 3.1: Run the full build and test suite

- **Files**: N/A
- **Action**:
  1. `cargo build` -- must succeed
  2. `cargo test` -- all existing tests must pass
  3. `cargo clippy` -- no new warnings (existing allows are fine)
- **Verify**: All three commands succeed with zero errors
- **Complexity**: Small

#### Step 3.2: End-to-end manual verification

- **Files**: N/A
- **Action**: Run the app with `cargo run` and verify the following:
- **Manual test cases**:
  - [ ] "Set as Wallpaper" button is the topmost control in the panel
  - [ ] Wallpaper status text appears below the button
  - [ ] "Load Defaults" and "Reset" buttons are side by side below status text
  - [ ] 3x3 preset grid is visible with correct labels in correct order
  - [ ] "Advanced" toggle is visible below the preset grid and starts closed
  - [ ] Clicking "Advanced" shows all GroupBox sections in the correct order: Camera Position, Camera Orientation, Framing, Date/Time, Clouds, Atmosphere, Lighting, Color Correction, Rendering, GPU info
  - [ ] Clicking "Advanced" again hides them all
  - [ ] Atmosphere sub-controls still hide when the Enable checkbox is unchecked
  - [ ] Date/Time sub-controls still hide when the Custom checkbox is unchecked
  - [ ] Clicking a preset button changes the globe position but does not reset lighting/color/atmosphere/cloud settings
  - [ ] "Load Defaults" restores everything to defaults
  - [ ] After "Load Defaults", the config file on disk is unchanged
  - [ ] Changing sliders and clicking "Set as Wallpaper" saves config AND sets wallpaper
  - [ ] After "Set as Wallpaper", clicking "Reset" reloads the saved config
  - [ ] Mouse drag (left=globe, right=frame, middle=orient, left+right=tilt) still works
  - [ ] Mouse scroll zoom still works
  - [ ] Closing and reopening the app does NOT auto-save (only "Set as Wallpaper" saves)
  - [ ] All sliders fire `sliders-changed` and the globe re-renders
- **Verify**: All manual test cases pass
- **Complexity**: Medium

#### Step 3.3: Update CLAUDE.md

- **Files**: `CLAUDE.md`
- **Action**: Update the following sections to reflect the new UI structure:
  - **UI description**: Replace the current `GroupBox` section list with the new ordering, mention the collapsible advanced section, the 3x3 preset grid, and the new button layout
  - **Callbacks**: Update the callback list to reflect `apply-preset(int)`, `load-defaults()`, `reset()` replacing `reset-all()`
  - **Save behavior**: Remove mention of debounce timer auto-save; document that "Set as Wallpaper" is the only save point
  - **Key Constraints / Dirty-checking**: No changes needed (dirty-check fields are unchanged)
- **Verify**: Read through the updated CLAUDE.md to confirm accuracy against the implemented code
- **Complexity**: Small

## Test Strategy

### Automated Tests

| Test Case | Type | Input | Expected Output |
| --- | --- | --- | --- |
| `PRESETS` has 9 elements | Unit | `PRESETS.len()` | 9 |
| Europe preset longitude | Unit | `PRESETS[0].longitude` | 15.0 |
| Pacific preset longitude | Unit | `PRESETS[6].longitude` | -170.0 |
| Earthrise preset pitch | Unit | `PRESETS[8].pitch_deg` | 45.0 |
| Non-Earthrise presets have zero orientation | Unit | `PRESETS[0..8].iter()` | All tilt, yaw, pitch = 0.0 |
| All presets have zero offsets | Unit | `PRESETS.iter()` | All offset_x, offset_y = 0.0 |
| Existing config tests still pass | Unit/Integration | `cargo test` | All pass |

### Manual Verification

- [ ] Full UI layout matches the specified ordering (see Step 3.2 checklist)
- [ ] Preset buttons apply correct camera positions
- [ ] Save behavior: only "Set as Wallpaper" writes to disk
- [ ] "Load Defaults" does not write to disk
- [ ] "Reset" reloads from disk
- [ ] All mouse interactions (drag, scroll) still work without config timer

## Risks and Mitigations

| Risk | Impact | Mitigation |
| --- | --- | --- |
| Slint layout breaks at narrow panel widths (160px minimum) | Preset button labels may be truncated or overlap | Test at minimum splitter position; use abbreviated labels if needed ("N. America" not "North America") |
| Removing auto-save causes users to lose settings on crash | Settings changed since last "Set as Wallpaper" are lost | This is the intended behavior per user clarification; "Reset" provides recovery to last-saved state |
| `apply_config_to_window` does not set `aa_index`/`texture_index` directly | ComboBox indices require deferred setting via `invoke_from_event_loop` | Both `load-defaults` and `reset` must use the same deferred pattern as startup (documented in Steps 2.3 and 2.4) |
| Gamma slider encoding asymmetry (0-1 slider vs 0.2-3.0 actual) | "Load Defaults" could set wrong gamma if encoding is missed | `apply_config_to_window` already handles the `gamma_value_to_slider()` conversion; reuse it for both `load-defaults` and `reset` |
| `if` element destroys/recreates children on toggle | Slider state could be lost | Safe because all values are `in-out` root properties with `<=>` bindings (confirmed in research) |

## Rollback Strategy

All changes are confined to three files. If issues arise:

1. `git checkout HEAD -- ui/main.slint src/main.rs src/config.rs CLAUDE.md` restores the previous state
2. No database migrations, no config file format changes, no new dependencies -- rollback is clean

## Status

- [x] Plan approved
- [x] Implementation started
- [x] Implementation complete
