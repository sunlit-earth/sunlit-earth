# Plan: Custom Date/Time (2026-03-17)

## Summary

Add a custom date/time feature that lets users override the
real-time sun position with a user-chosen date and time. The UI
adds a "Date / Time" GroupBox with a checkbox toggle, hour slider
(HH:MM label), day-of-year slider ("Mon DD" label), and year
ComboBox (current year +/- 10). When enabled, the renderer
computes the sun direction for the chosen datetime instead of the
system clock. All new settings persist to config. The controls
panel gains a ScrollView wrapper to handle overflow. The "Reset
Camera" button is renamed and extended to also reset datetime.
Wallpaper export respects the custom datetime when enabled.

## Stakes Classification

**Level**: Medium
**Rationale**: The change touches multiple files (sun.rs, mod.rs,
main.rs, main.slint, config.rs, new datetime.rs) and adds a new
public module, but the core rendering pipeline is unchanged --- the
only integration point is replacing a single `sun_direction_now()`
call with a conditional that either returns the same value or a
custom one. The existing dirty-checking, uniform upload, and shader
code are untouched. All new pure conversion functions are
thoroughly testable. Failure mode is benign: if custom datetime
code has a bug, unchecking the checkbox restores real-time
behavior. Rollback is straightforward: revert the commits.

## Context

**Research**: `docs/plans/2026-03-17-custom-datetime-research.md`
**Supporting research**:
`docs/plans/2026-03-17-custom-datetime-codebase.md`,
`docs/plans/2026-03-17-custom-datetime-external.md`

**Affected Areas**:

- `src/scene/sun.rs` --- make `make_time()` public, add
  `sun_direction_at()` public function
- `src/scene/datetime.rs` --- new module with pure conversion
  functions (`hour_float_to_hm`, `day_of_year_to_month_day`,
  `is_leap_year`, `days_in_year`, `month_day_label`)
- `src/scene/mod.rs` --- new module declaration
- `src/config.rs` --- four new fields in `AppConfig`
- `ui/main.slint` --- ScrollView wrapper, "Date / Time" GroupBox,
  new properties, rename reset button
- `src/main.rs` --- config apply/read for datetime fields, year
  ComboBox model, reset callback extension
- `src/renderer/mod.rs` --- conditional sun direction at line 328

## Success Criteria

- [ ] Unchecking custom datetime uses real-time sun position
      (identical to current behavior)
- [ ] Checking custom datetime and moving the hour slider visibly
      rotates the sun around the globe
- [ ] Checking custom datetime and moving the day-of-year slider
      shifts the sun's declination (north/south movement)
- [ ] Year ComboBox shows current year +/- 10 (21 entries)
- [ ] Hour slider label shows HH:MM format (e.g. "14:30")
- [ ] Day-of-year slider label shows "Mon DD" format (e.g.
      "Mar 17")
- [ ] Day-of-year slider maximum adjusts to 366 for leap years,
      365 otherwise
- [ ] Controls panel is scrollable when the window is short
- [ ] Renderer info text scrolls with the controls
- [ ] "Reset All" button resets camera AND unchecks custom
      datetime, resetting sliders to defaults
- [ ] All datetime settings persist to config.toml and restore
      on restart
- [ ] Wallpaper export renders at the custom datetime when enabled
- [ ] `cargo test` passes with new unit and proptest tests
- [ ] `cargo clippy` passes cleanly

## Implementation Steps

### Phase 1: Pure Conversion Functions (datetime.rs)

This phase creates the new `datetime.rs` module with all pure
conversion functions and thorough tests. No other files depend on
this phase's outputs yet, so it can be verified in isolation.

#### Step 1.1: Create datetime.rs with leap year functions

RED then GREEN. Create the module and implement `is_leap_year`
and `days_in_year`.

- **Files**: `src/scene/datetime.rs` (new), `src/scene/mod.rs`
- **Action**: Create the module file, declare it in `mod.rs`. Write
  `is_leap_year(year: i32) -> bool` and
  `days_in_year(year: i32) -> u16`. Write tests first, then
  implement.
- **Test cases**:
  - `is_leap_year(2024)` -> true (divisible by 4)
  - `is_leap_year(2025)` -> false (not divisible by 4)
  - `is_leap_year(1900)` -> false (century, not 400)
  - `is_leap_year(2000)` -> true (400-year century)
  - `is_leap_year(2100)` -> false (century, not 400)
  - `days_in_year(2024)` -> 366
  - `days_in_year(2025)` -> 365
  - `days_in_year(2000)` -> 366
  - proptest: `days_in_year(y)` is always 365 or 366
  - proptest: `days_in_year(y) == 366` iff `is_leap_year(y)`
- **Verify**: `cargo test datetime` passes
- **Complexity**: Small

#### Step 1.2: Add `day_of_year_to_month_day`

RED then GREEN. Implement day-of-year to (month, day) conversion.

- **Files**: `src/scene/datetime.rs`
- **Action**: Write
  `day_of_year_to_month_day(doy: u16, year: i32) -> (u8, u8)`
  returning (month 1-12, day 1-31). Write tests first, then
  implement using cumulative day-count table with leap year
  adjustment.
- **Test cases**:
  - `(1, 2025)` -> (1, 1) --- Jan 1
  - `(31, 2025)` -> (1, 31) --- Jan 31
  - `(32, 2025)` -> (2, 1) --- Feb 1
  - `(59, 2025)` -> (2, 28) --- Feb 28 non-leap
  - `(60, 2025)` -> (3, 1) --- Mar 1 non-leap
  - `(60, 2024)` -> (2, 29) --- Feb 29 leap year
  - `(61, 2024)` -> (3, 1) --- Mar 1 leap year
  - `(365, 2025)` -> (12, 31) --- Dec 31 non-leap
  - `(366, 2024)` -> (12, 31) --- Dec 31 leap year
  - `(182, 2025)` -> (7, 1) --- Jul 1 (midyear sanity check)
  - proptest for all valid doy in 1..=days\_in\_year(y):
    month is in 1..=12 and day is in 1..=31
  - proptest: output day never exceeds days-in-month for that
    month/year
- **Verify**: `cargo test datetime` passes
- **Complexity**: Small

#### Step 1.3: Add `month_day_label`

RED then GREEN. Implement label formatting for the day slider.

- **Files**: `src/scene/datetime.rs`
- **Action**: Write
  `month_day_label(doy: u16, year: i32) -> String` that returns
  "Mon DD" format (e.g. "Mar 17", "Jan 1", "Dec 31"). Uses
  `day_of_year_to_month_day` internally.
- **Test cases**:
  - `(1, 2025)` -> "Jan 1"
  - `(76, 2025)` -> "Mar 17"
  - `(365, 2025)` -> "Dec 31"
  - `(366, 2024)` -> "Dec 31"
  - `(60, 2024)` -> "Feb 29"
  - `(60, 2025)` -> "Mar 1"
- **Verify**: `cargo test datetime` passes
- **Complexity**: Small

#### Step 1.4: Add `hour_float_to_hm` and `hour_label`

RED then GREEN. Implement hour decomposition and label formatting.

- **Files**: `src/scene/datetime.rs`
- **Action**: Write `hour_float_to_hm(h: f32) -> (u8, u8)`
  returning (hour 0-23, minute 0-59). Clamp input to
  `[0.0, 24.0)`. A value of exactly 24.0 clamps to (23, 59).
  Write `hour_label(h: f32) -> String` returning "HH:MM" format.
- **Test cases** for `hour_float_to_hm`:
  - `0.0` -> (0, 0)
  - `12.0` -> (12, 0)
  - `14.5` -> (14, 30)
  - `14.75` -> (14, 45)
  - `23.99` -> (23, 59)
  - `24.0` -> (23, 59) (clamped)
  - `0.016667` (~1 min) -> (0, 1)
  - proptest for h in 0.0..24.0: hour < 24 and minute < 60
- **Test cases** for `hour_label`:
  - `0.0` -> "00:00"
  - `9.5` -> "09:30"
  - `14.75` -> "14:45"
  - `23.99` -> "23:59"
- **Verify**: `cargo test datetime` passes
- **Complexity**: Small

#### Step 1.5: Add `hour_float_to_hms` for Astronomy Engine

RED then GREEN. Implement the FFI-typed hour decomposition.

- **Files**: `src/scene/datetime.rs`
- **Action**: Write
  `hour_float_to_hms(h: f32) -> (i32, i32, f64)` returning
  (hour, minute, second) in the types expected by
  `Astronomy_MakeTime`. This is distinct from `hour_float_to_hm`
  because it preserves fractional seconds and uses `i32`/`f64`
  types matching the FFI signature.
- **Test cases**:
  - `0.0` -> (0, 0, 0.0)
  - `14.5` -> (14, 30, 0.0)
  - `14.75` -> (14, 45, 0.0)
  - `12.5025` -> (12, 30, ~9.0) (fractional seconds)
  - `24.0` -> (23, 59, 59.0) approximately (clamped)
  - proptest for h in 0.0..24.0: hour in 0..24, minute in 0..60,
    second in 0.0..60.0
- **Verify**: `cargo test datetime` passes
- **Complexity**: Small

### Phase 2: Sun Direction API (sun.rs)

This phase exposes the existing private helpers and adds a public
entry point for computing sun direction at an arbitrary datetime.

#### Step 2.1: Add public `sun_direction_at`

RED then GREEN. Make existing helpers public and add the new
entry point.

- **Files**: `src/scene/sun.rs`
- **Action**:
  1. Remove `#[cfg(test)]` from `make_time()`, make it `pub`.
  2. Change `sun_direction_from_time()` from `fn` to `pub fn`.
  3. Add a new public function:

     ```rust
     pub fn sun_direction_at(
         year: i32, month: i32, day: i32,
         hour: i32, minute: i32, second: f64,
     ) -> Vec3
     ```

     that calls `make_time()` then `sun_direction_from_time()`.
  4. Write tests for `sun_direction_at` verifying it produces the
     same results as the existing test helper `sun_dir_at`.
- **Test cases**:
  - `sun_direction_at(2025, 3, 20, 12, 0, 0.0)` matches
    march equinox noon expectations (z ~ 1.0, y ~ 0, x ~ 0)
  - `sun_direction_at(2025, 6, 21, 12, 0, 0.0)` matches
    june solstice expectations (y ~ 0.40)
  - Result is always a unit vector (length ~ 1.0)
  - `sun_direction_at` with same args as `sun_dir_at` test helper
    produces identical results
- **Verify**: `cargo test sun` passes; existing tests unchanged
- **Complexity**: Small

### Phase 3: Config Persistence

This phase adds the new config fields so they can be persisted.
Independent of Phases 1 and 2.

#### Step 3.1: Add datetime fields to `AppConfig`

RED then GREEN. Add four new fields with appropriate defaults.

- **Files**: `src/config.rs`
- **Action**: Add four fields to `AppConfig`:

  ```rust
  pub use_custom_datetime: bool,
  pub custom_hour: f32,
  pub custom_day_of_year: f32,
  pub custom_year: i32,
  ```

  Update `Default` impl: `use_custom_datetime: false`,
  `custom_hour: 12.0`, `custom_day_of_year: 1.0`,
  `custom_year` defaults to current year via
  `time::OffsetDateTime::now_utc().year()`. Add
  `#[serde(default = "default_custom_year")]` on `custom_year` so
  existing config files without this field get the current year
  instead of 0. The other three fields can use the struct-level
  `#[serde(default)]` since their zero/false defaults are
  acceptable.
- **Test cases**:
  - Default `use_custom_datetime` is false
  - Default `custom_hour` is 12.0
  - Default `custom_day_of_year` is 1.0
  - Default `custom_year` is the current year (2026 or
    whichever year the test runs in)
  - Serde round-trip with new fields preserves all values
  - Deserializing TOML without new fields fills defaults (existing
    test `deserialize_missing_fields` still passes, add a new
    variant that checks the datetime defaults specifically)
  - Deserializing TOML with `custom_year = 0` loads as 0 (the
    serde default function only applies when the field is
    *missing*, not when it is explicitly 0)
- **Verify**: `cargo test config` passes
- **Complexity**: Small

### Phase 4: Slint UI Changes

This phase modifies the Slint UI file. It depends on nothing from
Phases 1-3 at the Slint level (the properties just need to exist),
but the Rust integration in Phase 5 will connect them.

#### Step 4.1: Add new properties and import ScrollView

- **Files**: `ui/main.slint`
- **Action**:
  1. Add `ScrollView` to the import statement on line 1.
  2. Add new `in-out` properties on `MainWindow`:

     ```slint
     in-out property <bool> use-custom-datetime: false;
     in-out property <float> custom-hour: 12.0;
     in-out property <float> custom-day-of-year: 1.0;
     in-out property <int> custom-year-index: 10;
     in property <[string]> year-options;
     in property <string> hour-label: "12:00";
     in property <string> day-label: "Jan 1";
     in property <int> max-day-of-year: 365;
     ```

  3. Rename the `reset-camera` callback to `reset-all`.
  4. Rename the "Reset Camera" button text to "Reset All".
- **Verify**: `cargo build` succeeds (Slint compiles)
- **Complexity**: Small

#### Step 4.2: Wrap controls panel in ScrollView

- **Files**: `ui/main.slint`
- **Action**: Wrap the existing `VerticalLayout` (lines 69-457)
  inside a `ScrollView` with:

  ```slint
  ScrollView {
      horizontal-scrollbar-policy: always-off;
      width: parent.width;
      height: parent.height;
      VerticalLayout {
          width: parent.visible-width;
          alignment: start;
          ...existing content...
      }
  }
  ```

  Move the renderer info text (lines 460-466) from its current
  absolutely-positioned location into the bottom of the
  `VerticalLayout` inside the ScrollView, using a normal flow
  element instead of absolute positioning. Remove the absolute
  `x`/`y` positioning. Add a small spacer before it.
- **Verify**: `cargo run`, visually confirm controls scroll when
  window is short, renderer info scrolls with controls
- **Complexity**: Medium

#### Step 4.3: Add "Date / Time" GroupBox

- **Files**: `ui/main.slint`
- **Action**: After the "Lighting" GroupBox and before the spacer
  `Rectangle { height: 8px; }`, add a new GroupBox:

  ```slint
  GroupBox {
      title: "Date / Time";
      VerticalLayout {
          spacing: 4px;
          HorizontalLayout {
              spacing: 4px;
              Text {
                  text: "Custom";
                  vertical-alignment: center;
                  min-width: 70px;
              }
              CheckBox {
                  text: "Override date/time";
                  checked <=> root.use-custom-datetime;
                  toggled => {
                      root.sliders-changed();
                  }
              }
          }
          if root.use-custom-datetime :
          VerticalLayout {
              spacing: 4px;
              HorizontalLayout {
                  // Hour slider row
              }
              HorizontalLayout {
                  // Day slider row
              }
              HorizontalLayout {
                  // Year ComboBox row
              }
          }
      }
  }
  ```

  Each slider/ComboBox row follows the same pattern as existing
  controls (label Text with min-width: 70px, control, value Text
  with min-width: 45px). The hour slider range is 0.0 to 23.99.
  The day slider range is 1.0 to `root.max-day-of-year`. The
  year ComboBox uses `root.year-options` as its model.
- **Verify**: `cargo run`, confirm GroupBox appears after Lighting.
  Checkbox toggles visibility of sliders and ComboBox. Controls
  collapse when unchecked.
- **Complexity**: Medium

### Phase 5: Rust Integration (main.rs)

This phase wires the Slint properties to the Rust config and
renderer. Depends on Phases 1-4.

#### Step 5.1: Populate year ComboBox model and config I/O

- **Files**: `src/main.rs`
- **Action**:
  1. After creating the window and loading config, compute the
     year range: `current_year - 10` to `current_year + 10`.
     Build a `Vec<slint::SharedString>` of year strings and set
     it on the window via `set_year_options()`.
  2. In `apply_config_to_window()`, add:
     - `set_use_custom_datetime(config.use_custom_datetime)`
     - `set_custom_hour(config.custom_hour)`
     - `set_custom_day_of_year(config.custom_day_of_year)`
     - Compute `custom_year_index` from `config.custom_year` by
       subtracting the base year, clamping to 0..20.
       Set via `set_custom_year_index()`.
  3. In `read_config_from_window()`, add:
     - Read `get_use_custom_datetime()`
     - Read `get_custom_hour()`
     - Read `get_custom_day_of_year()`
     - Convert `get_custom_year_index()` back to absolute year
       by adding the base year. Store in `custom_year`.
  4. Compute and set `hour_label` and `day_label` from the
     current slider values using `datetime::hour_label()` and
     `datetime::month_day_label()`.
  5. Compute and set `max_day_of_year` based on whether the
     selected year is a leap year via `datetime::days_in_year()`.
- **Verify**: `cargo build` succeeds
- **Complexity**: Medium

#### Step 5.2: Add label update logic in sliders-changed

- **Files**: `src/main.rs`
- **Action**: In the existing `on_sliders_changed` callback
  (or create a small helper called from it), update the display
  labels and max day-of-year whenever the datetime sliders change:
  1. Read `custom_hour` from the window, compute
     `datetime::hour_label(h)`, set `hour_label` on the window.
  2. Read `custom_day_of_year` and the current year (from index),
     compute `datetime::month_day_label(doy, year)`, set
     `day_label` on the window.
  3. Read the current year, compute
     `datetime::days_in_year(year)`,
     set `max_day_of_year` on the window. If the current
     `custom_day_of_year` exceeds the new max, clamp it and
     write it back.
- **Verify**: `cargo run`, move hour slider and confirm label
  updates. Move day slider and confirm "Mon DD" label updates.
  Switch year and confirm max day adjusts for leap years.
- **Complexity**: Small

#### Step 5.3: Rename reset-camera to reset-all

- **Files**: `src/main.rs`
- **Action**:
  1. Rename `on_reset_camera` to `on_reset_all`.
  2. In the callback body, after resetting camera params, also:
     - `set_use_custom_datetime(false)`
     - `set_custom_hour(12.0)`
     - `set_custom_day_of_year(1.0)`
     - Set `custom_year_index` to the center index (10, i.e.
       current year)
     - Update labels (`hour_label`, `day_label`,
       `max_day_of_year`)
- **Verify**: `cargo run`, click "Reset All", confirm camera resets
  AND datetime checkbox unchecks, sliders return to defaults.
- **Complexity**: Small

### Phase 6: Renderer Integration

This phase connects the custom datetime to the sun direction
computation. Depends on Phase 2 (sun API) and Phase 4 (UI
properties).

#### Step 6.1: Conditional sun direction in rendering callback

- **Files**: `src/renderer/mod.rs`
- **Action**: At line 328, replace:

  ```rust
  let sun_dir = sun::sun_direction_now();
  ```

  with a conditional that reads the custom datetime properties
  from the window:

  ```rust
  let sun_dir = if win.get_use_custom_datetime() {
      let hour = win.get_custom_hour();
      let doy = win.get_custom_day_of_year() as u16;
      let year_index = win.get_custom_year_index();
      let year = year_index + datetime::base_year();
      let (month, day) =
          datetime::day_of_year_to_month_day(doy, year);
      let (h, m, s) = datetime::hour_float_to_hms(hour);
      sun::sun_direction_at(
          year, month.into(), day.into(), h, m, s,
      )
  } else {
      sun::sun_direction_now()
  };
  ```

  Add the necessary `use crate::scene::datetime;` import. The
  base year constant should be computed via the shared
  `datetime::base_year()` helper (Phase 7).
- **Verify**: `cargo run`, enable custom datetime, move hour
  slider --- sun visibly rotates around the globe. Move day
  slider --- declination changes. Uncheck --- returns to
  real-time.
- **Complexity**: Small

#### Step 6.2: Verify wallpaper export respects custom datetime

- **Files**: `src/renderer/mod.rs`
- **Action**: The `export_wallpaper_image` function at line 81
  reuses `last_shading` which already contains the `sun_dir` from
  the last rendered frame. Since Step 6.1 ensures that `sun_dir`
  reflects the custom datetime during `BeforeRendering`, and the
  export path uses `last_shading.sun_dir`, no additional changes
  are needed for wallpaper export --- it automatically uses
  whatever sun direction was last rendered.

  Verify this by reading `export_wallpaper_image` and confirming
  `shading.sun_dir` flows from `res.last_shading` which is set
  during the render pass.
- **Verify**: `cargo run`, set custom datetime to midnight, click
  "Set as Wallpaper", confirm the exported wallpaper shows the
  nightside facing the camera (same as the preview).
- **Complexity**: Small (verification only, likely no code change)

### Phase 7: Shared Year Base Constant

#### Step 7.1: Extract base year computation

- **Files**: `src/scene/datetime.rs`, `src/main.rs`,
  `src/renderer/mod.rs`
- **Action**: Add a `pub fn year_range() -> (i32, i32)` function
  to `datetime.rs` that returns
  `(current_year - 10, current_year + 10)`.
  Also add `pub fn base_year() -> i32` returning
  `current_year - 10`.
  Use this in both `main.rs` (when building the ComboBox model
  and converting indices) and `renderer/mod.rs` (when converting
  the year index to an absolute year). This avoids duplicating
  the `current_year - 10` logic.
- **Test cases**:
  - `base_year()` returns `current_year - 10`
  - `year_range()` returns
    `(current_year - 10, current_year + 10)`
  - The range contains exactly 21 years
- **Verify**: `cargo test datetime` passes; `cargo clippy` clean
- **Complexity**: Small

### Phase 8: Final Verification

#### Step 8.1: Full test suite and clippy

- **Files**: All
- **Action**: Run `cargo test` and `cargo clippy` to confirm no
  regressions. Fix any warnings or failures.
- **Verify**: `cargo test` all green, `cargo clippy` clean
- **Complexity**: Small

#### Step 8.2: Manual end-to-end verification

- **Files**: N/A (manual verification)
- **Action**: Comprehensive manual testing.
- **Manual test cases**:
  - Launch app fresh (delete config.toml) --- defaults load,
    custom datetime is unchecked, real-time sun
  - Check "Override date/time" --- sliders and ComboBox appear
  - Move hour slider --- sun rotates, label shows HH:MM
  - Move day slider --- declination shifts, label shows "Mon DD"
  - Select a leap year (2024) --- day slider max becomes 366
  - Select a non-leap year (2025) after setting day 366 --- day
    clamps to 365
  - Click "Reset All" --- camera resets, datetime unchecks
  - Resize window very short --- controls scroll, no clipping
  - Scroll the controls panel --- renderer info scrolls with it
  - Set custom datetime, click "Set as Wallpaper" --- wallpaper
    matches the preview
  - Close and reopen app --- all datetime settings restored from
    config
  - ComboBox popup opens correctly inside ScrollView
    (not clipped)
- **Verify**: All manual checks pass
- **Complexity**: Small

## Test Strategy

### Automated Tests

<!-- markdownlint-disable MD013 -->

| Test Case | Type | Location | Key Assertion |
| --- | --- | --- | --- |
| `is_leap_year` known values | Unit | `datetime.rs` | 2000, 1900, 2024, 2025, 2100 |
| `is_leap_year` proptest | Proptest | `datetime.rs` | Consistent with `days_in_year` |
| `days_in_year` values | Unit | `datetime.rs` | 365 or 366 matching leap status |
| `day_of_year_to_month_day` boundaries | Unit | `datetime.rs` | Jan 1, month transitions, Dec 31 |
| `day_of_year_to_month_day` leap | Unit | `datetime.rs` | (2, 29) for doy 60 in leap year |
| `day_of_year_to_month_day` proptest | Proptest | `datetime.rs` | Month 1-12, day 1-31 |
| `month_day_label` formatting | Unit | `datetime.rs` | "Mon DD" matches known dates |
| `hour_float_to_hm` known values | Unit | `datetime.rs` | Correct HH:MM decomposition |
| `hour_float_to_hm` edge at 24.0 | Unit | `datetime.rs` | Clamps to (23, 59) |
| `hour_float_to_hm` proptest | Proptest | `datetime.rs` | hour < 24, minute < 60 |
| `hour_float_to_hms` known values | Unit | `datetime.rs` | Correct HH:MM:SS decomposition |
| `hour_label` formatting | Unit | `datetime.rs` | "HH:MM" format |
| `sun_direction_at` equinox | Unit | `sun.rs` | Matches equinox expectations |
| `sun_direction_at` unit vector | Unit | `sun.rs` | Length ~ 1.0 |
| `base_year` and `year_range` | Unit | `datetime.rs` | 21 years centered on now |
| `AppConfig` new field defaults | Unit | `config.rs` | false, 12.0, 1.0, current year |
| `AppConfig` serde round-trip | Unit | `config.rs` | New fields survive serde |
| `AppConfig` missing fields | Unit | `config.rs` | Defaults fill correctly |

<!-- markdownlint-enable MD013 -->

### Manual Verification

- [ ] Custom datetime checkbox toggles slider/ComboBox visibility
      with layout collapse (no blank gap)
- [ ] Hour slider smoothly changes sun position in real-time
      preview
- [ ] Day slider changes sun declination visibly
- [ ] Year ComboBox dropdown opens fully (not clipped by
      ScrollView)
- [ ] ScrollView scrolls when window height < content height
- [ ] "Reset All" resets both camera and datetime
- [ ] Wallpaper export at custom datetime matches preview
- [ ] Config persists across app restart

## Risks and Mitigations

<!-- markdownlint-disable MD013 -->

| Risk | Impact | Mitigation |
| --- | --- | --- |
| ScrollView breaks panel layout | Visual regression | Step 4.2 is isolated; manual check before proceeding |
| ComboBox popup clipped in ScrollView | Dropdown unusable | Slint popups are overlays; manual test in Step 8.2 |
| Leap year day clamping causes jump | Minor UX confusion | Explicit clamp-and-write-back in Step 5.2 |
| `base_year` computed at different times | Year index off by 1 | Extract to shared `datetime::base_year()` in Phase 7 |
| Astronomy Engine at extreme dates | Wrong sun position | Year range limited to +/- 10 years |
| Config files missing new fields | App crash on load | `#[serde(default)]` handles this; verified in Step 3.1 |

<!-- markdownlint-enable MD013 -->

## Rollback Strategy

All changes are additive. The custom datetime feature can be
disabled by unchecking the checkbox, which restores the original
real-time behavior. To fully roll back, revert the commits ---
no database migrations, no config file format breaks (old configs
without the new fields load cleanly via serde defaults).

## Status

- [ ] Plan approved
- [ ] Implementation started
- [ ] Implementation complete
