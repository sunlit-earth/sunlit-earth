# Plan: Fresh astronomical state for wallpaper scheduler (2026-03-30)

## Summary

Extract the astronomical computation (currently sun direction, future moon/planets/stars) from `BeforeRendering` into a pure function so the wallpaper auto-refresh scheduler can compute fresh state when the window is hidden. Currently, `BeforeRendering` stops firing when the window is hidden to tray, freezing the sun direction in `last_shading`. The scheduler timer keeps running and the GPU export succeeds, but every export renders the same stale scene.

## Stakes Classification

**Level**: Medium
**Rationale**: Touches the BeforeRendering hot path and the scheduler timer, but changes are mechanical extractions with no new logic. The existing test suite covers the rendering pipeline and the slint-tray-minimal experiment validates the fix pattern.

## Context

**Research**: Reproduced and diagnosed in `slint-tray-minimal` experiment 9 (`exp9_stale_render_state_after_hide`). The test confirms `BeforeRendering` stops firing when the window is hidden, freezing `last_render_epoch_ms` (proxy for sun direction).
**Affected Areas**: `scene/sun.rs`, `scene/datetime.rs`, `renderer/mod.rs`, `main.rs`

## Success Criteria

- [ ] New `DateTimeInput` struct and `compute_sun_direction(dt)` pure function in `scene/sun.rs`
- [ ] New `read_datetime_input(win)` function extracts datetime params from the Slint window
- [ ] `BeforeRendering` uses the extracted functions (no behavior change)
- [ ] Scheduler timer computes fresh sun direction and writes it to `last_shading` before calling `do_set_wallpaper()`
- [ ] `exp9_stale_render_state_after_hide` in slint-tray-minimal passes when un-ignored (or equivalent manual verification in sunlit-earth)
- [ ] `cargo test` and `cargo clippy` pass with no regressions

## Implementation Steps

### Phase 1: Extract pure astronomical computation

#### Step 1.1: Add `DateTimeInput` struct and `compute_sun_direction` to `scene/sun.rs`

- **Files**: `src/scene/sun.rs`
- **Action**: Add a `DateTimeInput` struct capturing the custom datetime UI state as plain values, and a `compute_sun_direction(dt: &DateTimeInput) -> Vec3` function that encapsulates the custom-vs-now branching currently inline in BeforeRendering. The struct fields are:
  - `use_custom: bool`
  - `custom_hour: f32`
  - `custom_day_of_year: u16`
  - `custom_year: i32` (calendar year, not ComboBox index)

  The function calls `sun_direction_at()` for custom datetime, `sun_direction_now()` otherwise. Uses `datetime::day_of_year_to_month_day` and `datetime::hour_float_to_hms` for the conversion (same logic currently at `renderer/mod.rs:435-444`).
- **Test cases**:
  - `use_custom: false` returns `sun_direction_now()` (fuzzy match, within epsilon of calling the underlying function)
  - `use_custom: true` with a known date (e.g. 2025-06-21 12:00 UTC) returns the same result as `sun_direction_at(2025, 6, 21, 12, 0, 0.0)`
  - `custom_day_of_year: 0` is clamped to 1 (existing `.max(1)` behavior)
- **Verify**: `cargo test sun` passes, new tests pass
- **Complexity**: Small

#### Step 1.2: Add `read_datetime_input` to `ui_callbacks.rs`

- **Files**: `src/ui_callbacks.rs`
- **Action**: Add a `pub fn read_datetime_input(window: &MainWindow) -> DateTimeInput` that reads `use_custom_datetime`, `custom_hour`, `custom_day_of_year`, `custom_year_index` from the window and converts `custom_year_index` to a calendar year via `datetime::base_year()`. This is a thin extraction — no logic, just property reads.
- **Verify**: Compiles. No dedicated test needed (trivial getter delegation).
- **Complexity**: Small

### Phase 2: Refactor BeforeRendering to use extracted functions

#### Step 2.1: Replace inline sun computation in `BeforeRendering`

- **Files**: `src/renderer/mod.rs:435-444`
- **Action**: Replace the inline `sun_dir` computation block with:

  ```rust
  let dt = ui_callbacks::read_datetime_input(&win);
  let sun_dir = sun::compute_sun_direction(&dt);
  ```

- **Verify**: `cargo test` passes (no behavior change). `cargo clippy` clean.
- **Complexity**: Small

### Phase 3: Wire scheduler timer to compute fresh state

#### Step 3.1: Add `update_sun_direction` to `renderer/mod.rs`

- **Files**: `src/renderer/mod.rs`
- **Action**: Add a public function that writes a fresh sun direction into the thread-local `GPU_RESOURCES.last_shading`:

  ```rust
  pub fn update_sun_direction(sun_dir: glam::Vec3) {
      GPU_RESOURCES.with(|r| {
          if let Some(res) = r.borrow_mut().as_mut() {
              if let Some(shading) = &mut res.last_shading {
                  shading.sun_dir = sun_dir;
              }
          }
      });
  }
  ```

  This runs on the main thread (same thread as timer callbacks and GPU_RESOURCES), so no synchronization needed.
- **Verify**: Compiles. The function is exercised by step 3.2.
- **Complexity**: Small

#### Step 3.2: Update scheduler timer to refresh sun direction before export

- **Files**: `src/main.rs:294-308` (initial timer), `src/main.rs:335-349` (restarted timer)
- **Action**: In both scheduler timer closures (the initial start and the restart in `on_auto_refresh_changed`), add sun direction refresh before calling `do_set_wallpaper()`:

  ```rust
  let dt = sunlit_earth::ui_callbacks::read_datetime_input(&win);
  let sun_dir = sunlit_earth::scene::sun::compute_sun_direction(&dt);
  sunlit_earth::renderer::update_sun_direction(sun_dir);
  ```

  The `win` is already available from `ww.upgrade()` in both closures.
- **Verify**: Manual verification: enable auto-refresh, close window to tray, wait for refresh interval, confirm wallpaper shows updated sun position. `cargo test` and `cargo clippy` pass.
- **Complexity**: Small

### Phase 4: Verify

#### Step 4.1: Run full test suite

- **Files**: N/A
- **Action**: `cargo test && cargo clippy`
- **Verify**: All tests pass, no warnings.
- **Complexity**: Small

#### Step 4.2: Manual end-to-end verification

- **Action**:
  1. `cargo run`, enable auto-refresh with 1-minute interval
  2. Click "Set as Wallpaper" — confirm wallpaper is set
  3. Close window to tray (X button)
  4. Wait 2 minutes
  5. Observe wallpaper has subtly changed (terminator shifted ~0.5 degrees)
  6. Re-open window from tray, confirm UI is responsive
- **Verify**: Wallpaper file modification time advances on each refresh cycle while the window is hidden.
- **Complexity**: Small

## Risks and Mitigations

| Risk | Impact | Mitigation |
| --- | --- | --- |
| BeforeRendering regression from refactor | Broken preview rendering | Step 2.1 is a mechanical replacement; existing test suite catches regressions |
| Thread-local borrow conflict in `update_sun_direction` | Panic at runtime | Timer callbacks and GPU_RESOURCES both live on the main thread; no concurrent access possible |
| `ShadingParams` visibility change needed | Compile error | `ShadingParams` is `pub(super)` — `update_sun_direction` lives in the same module, so no change needed |

## Future Extension

When moon, planet, or star positions are added:

1. Extend `DateTimeInput` if new inputs are needed (unlikely — same datetime drives all astronomy)
2. Rename/expand `compute_sun_direction` to `compute_astro_state` returning a struct with `sun_dir`, `moon_dir`, etc.
3. Expand `update_sun_direction` to `update_astro_state` accepting the full struct
4. `ShadingParams` gains corresponding fields

The two-layer architecture (`read_datetime_input` → `compute_*`) stays the same.

## Rollback Strategy

All changes are additive extractions. Reverting is a single `git revert`. The old inline code can be restored by undoing step 2.1.

## Status

- [ ] Plan approved
- [ ] Implementation started
- [ ] Implementation complete
