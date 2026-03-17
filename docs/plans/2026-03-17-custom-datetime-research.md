# Custom Date/Time Feature — Consolidated Research

**Date:** 2026-03-17
**Sources:** Codebase analysis (`2026-03-17-custom-datetime-codebase.md`), Slint UI research (`2026-03-17-custom-datetime-external.md`)

## 1. Current Architecture

### Sun Position Pipeline

The sun's position flows through three stages:

1. **Computation** (`src/scene/sun.rs`): `sun_direction_now()` calls `Astronomy_CurrentTime()` via FFI to get the system clock, then delegates to `sun_direction_from_time()`. That function creates a geocentric observer at (0,0,0), calls `Astronomy_Equator()` for the sun's right ascension and declination, obtains sidereal time via `Astronomy_SiderealTime()`, and converts to the renderer's coordinate frame (+Z = prime meridian, +Y = north, +X = 90 degrees E). The output is a unit `Vec3<f32>`.

2. **Dirty-checking** (`src/renderer/mod.rs`, `src/renderer/frame.rs`): The renderer calls `sun_direction_now()` on every `BeforeRendering` frame (line 328). The resulting direction is passed to `build_frame_state()`, which quantizes it to integer milliradians for stable comparison in `FrameState::sun_direction: [i32; 3]`. If the frame state matches the previous frame, the render is skipped entirely.

3. **GPU upload** (`src/renderer/render_pass.rs`, `src/renderer/uniforms.rs`): `write_uniforms()` takes a `ShadingParams` struct containing `sun_dir: glam::Vec3` and writes it into the GPU uniform buffer at offset 64. The shader reads this to compute diffuse lighting and day/night blending.

### Key Observation

Only the single call site at `renderer/mod.rs` line 328 (`let sun_dir = sun::sun_direction_now()`) needs to change. Everything downstream -- dirty-checking, uniform upload, shader -- is already parameterized on an arbitrary `Vec3` sun direction.

### Existing Infrastructure

`sun.rs` already has a `make_time()` helper that wraps `Astronomy_MakeTime(year, month, day, hour, minute, second)`, and `sun_direction_from_time()` accepts arbitrary `astro_time_t` values. Both are currently test-only but require only visibility changes to be usable from the renderer.

## 2. Extension Points

### sun.rs

- Make `make_time()` public (or create a thin public wrapper).
- Add a public function `sun_direction_at(year: i32, month: i32, day: i32, hour: i32, minute: i32, second: f64) -> Vec3` that combines `make_time()` and `sun_direction_from_time()`.
- No changes needed to `sun_direction_from_time()` itself.

### renderer/mod.rs

- At line 328, replace the unconditional `sun_direction_now()` call with a conditional: read the custom-datetime UI properties from the window; if custom mode is enabled, call the new `sun_direction_at()` with the converted slider/dropdown values; otherwise call `sun_direction_now()` as before.
- Access pattern: the rendering callback already holds a weak reference to `MainWindow` and reads properties like camera values from it (lines 330-345). The same pattern extends to `get_use_custom_datetime()`, `get_custom_hour()`, `get_custom_day_of_year()`, `get_custom_year()`.

### renderer/frame.rs

- No structural changes needed. `FrameState` already captures `sun_direction: [i32; 3]`, so if the custom datetime values produce the same sun direction across frames, dirty-checking correctly skips the render. The custom datetime values themselves do not need to appear in `FrameState` because they affect the frame only through `sun_direction`.

### UI (ui/main.slint)

- Add `ScrollView` to the import statement.
- Wrap the existing `VerticalLayout` controls panel inside a `ScrollView` with `horizontal-scrollbar-policy: always-off`.
- Set `width: parent.visible-width` and `alignment: start` on the inner `VerticalLayout`.
- Add a new "Date/Time" `GroupBox` section containing:
  - CheckBox for `use-custom-datetime`
  - Conditionally visible controls (via `if` syntax) for hour slider, day-of-year slider, and year ComboBox
- Declare new `in-out` properties: `use-custom-datetime` (bool), `custom-hour` (float), `custom-day-of-year` (float), `custom-year` (int).
- Add a new callback (e.g. `datetime-changed()`) or reuse `sliders-changed()` if the redraw/save semantics are identical.

### config.rs

- Add fields to `AppConfig`:
  ```rust
  pub use_custom_datetime: bool,
  pub custom_hour: f32,         // 0.0-24.0
  pub custom_day_of_year: f32,  // 0.0-365.0
  pub custom_year: i32,         // e.g. 2026
  ```
- `#[serde(default)]` on `AppConfig` means existing config files without these fields will deserialize with Rust defaults (false, 0.0, 0.0, 0). The year default of 0 needs a custom `Default` impl or `#[serde(default = "current_year")]` to default to the current year instead.

### main.rs

- `apply_config_to_window()`: set the new UI properties from config.
- `read_config_from_window()`: read the new UI properties into config.
- Add a callback handler for the datetime controls that calls `request_redraw()` and triggers the debounced config save, following the same pattern as `sliders-changed()`.
- Populate the year ComboBox model (e.g. `["2020", "2021", ..., "2030"]`) and set the default index to the current year's position.

### Data Conversion Functions

Two pure conversion functions are needed (likely in a new `src/scene/datetime.rs` or alongside `sun.rs`):

- `day_of_year_to_month_day(doy: u16, year: i32) -> (i32, i32)` -- converts a 1-based day-of-year to (month, day), accounting for leap years.
- `hour_float_to_hms(h: f32) -> (i32, i32, f64)` -- converts e.g. 14.5 to (14, 30, 0.0).

These are pure functions and excellent candidates for thorough unit testing and `proptest` property-based testing.

## 3. Slint UI Components

### ScrollView

Wraps the controls panel to handle overflow when the window is short. Key configuration:
- `horizontal-scrollbar-policy: always-off` -- prevents horizontal scrolling.
- Inner `VerticalLayout` needs `width: parent.visible-width` (not `parent.width`) to account for the vertical scrollbar's width, and `alignment: start` so items stack from the top rather than stretching to fill the viewport height.
- The `ScrollView` fills the full panel rectangle dimensions.
- `viewport-height` is automatically driven by the inner layout's minimum size -- no manual calculation needed.

### ComboBox

Used for the year dropdown. Properties: `model` (string array), `current-index` (int, `in-out`, supports `<=>`), `current-value` (string, `in-out`). The `selected(string)` callback fires on user selection.

For the year selector, the model should be populated from Rust as a dynamic property (e.g. `in property <[string]> year-options`), since the range should be centered on the current year. Bind `current-index <=> root.custom-year-index` and convert the index to an actual year in Rust.

### CheckBox

Used for the `use-custom-datetime` toggle. `checked` is `in-out` and supports `<=>`. The `toggled()` callback fires on state change. This follows the exact pattern already used for the `diffuse-shading` checkbox.

### Conditional Visibility

Two approaches exist in Slint:
- **`visible: false`** -- hides the element but it **still occupies layout space** (equivalent to CSS `visibility: hidden`).
- **`if condition : Element { ... }`** -- fully removes the element from layout flow when false, collapsing the space (equivalent to CSS `display: none`).

The `if` syntax is the correct choice for the datetime controls. When the checkbox is unchecked, the date/time sliders and year dropdown should collapse completely so the panel does not have a blank gap. The codebase already uses this pattern for the loading overlay in `main.slint`.

## 4. Data Conversion

### Hour Slider to Hour/Minute/Second

The hour slider ranges from 0.0 to 24.0 as a float. Conversion to HMS for the Astronomy Engine:

```
hour   = floor(h)
minute = floor((h - hour) * 60)
second = ((h - hour) * 60 - minute) * 60
```

Example: 14.75 -> (14, 45, 0.0). The slider's display label should show `HH:MM` format.

Edge case: a value of exactly 24.0 represents midnight of the next day. This can be handled by clamping to 23.999... or by rolling it into the date math (incrementing the day by 1 and using hour 0). Clamping the slider maximum to just under 24.0 (e.g. 23.99) is simpler.

### Day-of-Year Slider to Month/Day

The day-of-year slider ranges from 1.0 to 365.0 (or 366.0 for leap years). The slider value is truncated to an integer day-of-year, then converted to (month, day) using cumulative day counts per month.

Standard cumulative days (non-leap): `[0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334]`. For leap years, add 1 to every entry from March onward. Find the last entry less than the day-of-year to determine the month; the remainder is the day within the month.

Leap year rule: divisible by 4, except centuries, except 400-year centuries. For the practical year range (2000-2100), only 2000 is a century (and it is a leap year), so the standard `(year % 4 == 0)` check suffices within a reasonable dropdown range.

The slider's display label should show the date as `MMM DD` (e.g. "Mar 17").

### Year ComboBox to Integer

The ComboBox model is a string array (e.g. `["2020", "2021", ..., "2030"]`). The `current-index` maps to a year by adding the base year: `year = base_year + index`. The Rust side populates the model and reads back `custom_year_index`, converting to an absolute year.

## 5. Key Decisions

### Conditional `if` vs `visible` for datetime controls

**Decision: Use `if`.**

The datetime controls (hour slider, day-of-year slider, year dropdown) should collapse out of the layout when the checkbox is unchecked. The `if` syntax removes them from layout flow entirely, which keeps the panel compact. Using `visible: false` would leave a blank gap where the controls were, degrading the layout. The codebase already uses `if` for the loading overlay, establishing the pattern.

### Leap year handling for the day-of-year slider

**Decision: Adjust slider maximum dynamically based on selected year.**

When the selected year is a leap year, the day-of-year slider maximum should be 366; otherwise 365. If the user switches from a leap year to a non-leap year while day 366 is selected, clamp the value to 365. The slider maximum can be bound to a computed property in Slint or adjusted from Rust when the year changes.

### 2-minute timer behavior in custom datetime mode

**Decision: No timer changes needed.**

The existing 2-minute periodic timer calls `request_redraw()`, which triggers `BeforeRendering`. When custom datetime is active, `sun_direction_at()` returns the same direction every time (since the inputs are fixed), so the dirty-check in `FrameState` comparison will detect no change and skip the render. The timer fires harmlessly with negligible cost. No conditional logic is needed to disable or modify the timer.

### Callback strategy for datetime controls

**Decision: Reuse the existing `sliders-changed()` callback.**

The datetime sliders and ComboBox need the same behavior as other controls: trigger a redraw and debounce a config save. Adding a separate `datetime-changed()` callback is possible but unnecessary -- the existing callback handler already calls `request_redraw()` and kicks the debounced save timer. Using the same callback reduces both Slint boilerplate and Rust glue code.

### Where to place the "Date/Time" section in the panel

**Decision: Place it after the "Lighting" section, before the buttons.**

The datetime controls are an advanced feature. Putting them at the bottom (just above Reset Camera / Set as Wallpaper) keeps the most-used controls (Camera, Framing, Lighting) at the top. The ScrollView addition ensures they remain accessible even on shorter windows.

### Config field defaults for custom_year

**Decision: Use a custom default function.**

Since `#[serde(default)]` gives `i32` a default of 0, the `custom_year` field needs a `#[serde(default = "default_year")]` annotation where `default_year()` returns the current year (via `time::OffsetDateTime::now_utc().year()`). Alternatively, handle the 0 case in `apply_config_to_window()` by replacing it with the current year.

## 6. Risks and Constraints

### ScrollView interaction with the existing layout

The current panel is a `Rectangle` containing a `VerticalLayout` with pinned content at the bottom (renderer info text at lines 460-466). Wrapping the controls in a `ScrollView` means the bottom-pinned text must be handled carefully. Two approaches:
- Place the renderer info *inside* the ScrollView (it scrolls with everything else -- simplest, but the info may scroll off-screen).
- Split the panel: ScrollView for controls, fixed-height area below for renderer info (keeps info always visible, but requires restructuring the layout).

This layout restructuring is the most likely source of visual regressions.

### Astronomy Engine time range

The Astronomy Engine library (`astronomy-engine-bindings`) may have a valid time range. The `Astronomy_MakeTime()` function accepts arbitrary values, but extreme dates (very far in the past or future) could produce inaccurate results. Constraining the year dropdown to a reasonable range (e.g. 2000-2100) mitigates this.

### Float quantization in dirty-checking

The custom datetime slider values (hour, day-of-year) are floats that get converted to a sun direction. Two very slightly different slider positions could produce the same quantized `FrameState`, causing the render to be skipped when the user expects a visual change. In practice, the milliradians quantization (integer thousandths) is fine-grained enough that any perceptible slider movement will produce a different sun direction. This is not a real risk.

### Config migration

Existing users have `config.toml` files without the new datetime fields. The `#[serde(default)]` annotation on `AppConfig` handles this -- missing fields get Rust default values on deserialization. No migration code is needed.

### Wallpaper export

The wallpaper export pipeline (`export_wallpaper_image`) also calls `sun_direction_now()` (or will need to respect custom datetime mode). If the user has set a custom datetime, the wallpaper should render at that time, not the current time. The export code path must read the same custom datetime properties and pass them through to the sun direction computation.

### ComboBox popup clipping in ScrollView

ComboBox dropdowns open as popups. In some UI frameworks, a ComboBox inside a ScrollView can have its popup clipped by the scroll container. Slint popups are rendered as overlays above all other content, so this should not be an issue, but it warrants manual testing.

## 7. Open Questions

1. **Display label format for the day-of-year slider.** Should the label show "Day 76" (raw number) or "Mar 17" (human-readable month/day)? The month/day format is more user-friendly but requires the conversion function to run on every slider movement for the label text. This is cheap to compute.

2. **Slider granularity for the hour slider.** Should the slider snap to whole minutes, or allow arbitrary fractional positions? For a wallpaper app where the sun moves slowly, whole-minute granularity (0.0 to 24.0 in 1/60 steps) is likely sufficient. Slint sliders do not have a built-in step property, so snapping would need to be done in the `changed` callback by rounding and writing back.

3. **Where exactly to put the checkbox.** Options: (a) standalone at the top of a "Date/Time" GroupBox, with the sliders conditionally appearing below it in the same box; (b) checkbox in a separate "Time Source" GroupBox, and the sliders in a separate conditionally-visible "Custom Date/Time" GroupBox below it. Option (a) is more compact.

4. **Year range for the ComboBox.** Should it be fixed (e.g. 2000-2100), centered on the current year (e.g. current year +/- 10), or configurable? A fixed range of ~20 years centered on the current year keeps the dropdown manageable.

5. **Renderer info placement after ScrollView addition.** Should the renderer info text scroll with the controls (simpler) or remain pinned at the bottom of the panel (always visible)? Pinned requires splitting the panel into a scrollable region and a fixed footer.

6. **Should the "Reset Camera" button also reset custom datetime to off?** If so, the reset callback needs to clear the datetime fields and uncheck the box. If not, camera and datetime are independent concerns.
