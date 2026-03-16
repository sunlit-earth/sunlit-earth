# Plan: Improved Camera Controls (2026-03-16)

## Summary

Add five camera control features to Sunlit Earth: (1) pan/offset via screen-space post-projection translation so the Earth can be positioned off-center for wallpaper compositions, (2) camera rotations -- tilt (roll), yaw (horizontal look), and pitch (vertical look) -- via post-view rotation matrices for angled and redirected compositions, (3) non-linear exponential zoom curve with extended range (1.5--80.0) for perceptually even zooming, and (4) mouse controls for direct manipulation (drag to rotate, scroll to zoom). Reorganize the controls panel into five `GroupBox` sections (Rendering, Camera Position, Camera Orientation, Framing, Lighting) for clearer visual structure. All features modify only the CPU-side MVP matrix computation -- no shader or uniform buffer layout changes are required. The implementation introduces a `CameraParams` struct to group the growing number of camera parameters, changes Slint camera properties from `out` to `in-out` to support bidirectional updates, and adds a `TouchArea` overlay on the viewport for mouse interaction.

## Stakes Classification

**Level**: Medium
**Rationale**: The changes span 6--7 files across the camera model, renderer pipeline, Slint UI, and main wiring, but all camera transforms are absorbed into the existing MVP matrix with no GPU-side changes. The dirty-checking system (`FrameState`) and wallpaper export path must be extended but follow established patterns. There are no new `unsafe` blocks, no new dependencies, and no architectural changes. Rollback is straightforward since the render pipeline and shaders are untouched. The main risk is interaction between features (tilt/yaw/pitch affecting mouse drag math), which is mitigated by implementing camera rotations before mouse controls and using a rotation-aware drag callback.

## Context

**Research**: [`docs/plans/2026-03-16-camera-controls-research.md`](2026-03-16-camera-controls-research.md)
**Supporting Research**: [`docs/plans/2026-03-16-camera-controls-codebase.md`](2026-03-16-camera-controls-codebase.md), [`docs/plans/2026-03-16-camera-controls-ui.md`](2026-03-16-camera-controls-ui.md)
**Affected Areas**: `src/scene/camera.rs`, `src/renderer/frame.rs`, `src/renderer/render_pass.rs`, `src/renderer/mod.rs`, `ui/main.slint`, `src/main.rs`, `CLAUDE.md`

## Success Criteria

- [ ] Controls panel is organized into five `GroupBox` sections: Rendering, Camera Position, Camera Orientation, Framing, Lighting
- [ ] `OrbitalCamera` accepts `offset_x`, `offset_y`, `tilt_deg`, `yaw_deg`, `pitch_deg` parameters and applies them to the MVP matrix
- [ ] Zoom uses exponential mapping: `distance = 1.5 * (80.0 / 1.5)^t` where `t` is the normalized slider value (0.0 to 1.0)
- [ ] Pan sliders (Offset X, Offset Y) range from -1.0 to 1.0, default 0.0, shifting the Earth within the viewport
- [ ] Tilt slider ranges from -180 to 180 degrees, default 0
- [ ] Yaw slider ranges from -90 to 90 degrees, default 0
- [ ] Pitch slider ranges from -90 to 90 degrees, default 0
- [ ] Zoom slider ranges from 0.0 to 1.0, default is the `t` value that produces the old default distance of 8.0
- [ ] Mouse drag on the viewport rotates the globe (updates longitude/latitude)
- [ ] Mouse scroll on the viewport zooms (updates zoom slider value)
- [ ] All new parameters are included in `FrameState` for dirty-checking
- [ ] Wallpaper export reflects all new camera parameters (verified via `FrameState`)
- [ ] Camera properties are `in-out` in Slint, with bidirectional slider bindings
- [ ] A "Reset Camera" button restores all camera parameters to their defaults
- [ ] `cargo build` succeeds with no warnings
- [ ] `cargo clippy` passes
- [ ] `cargo test` passes (existing tests updated, new tests added)
- [ ] Existing rendering behavior is unchanged when all new parameters are at defaults

## Implementation Steps

### Phase 0: GroupBox UI Reorganization

This phase restructures the existing flat control list in `ui/main.slint` into five `GroupBox` sections before any new controls are added. This is a pure visual refactor with no behavioral change -- all existing controls keep the same property names, callbacks, and ranges.

#### Step 0.1: Reorganize controls panel into GroupBox sections

- **Files**: `ui/main.slint`
- **Action**: Add `GroupBox` to the import line from `"std-widgets.slint"`. Replace the flat `VerticalLayout` of controls with five `GroupBox` sections inside the existing `VerticalLayout`:

  1. **Rendering** (`GroupBox { title: "Rendering"; }`) -- contains the Texture combobox row and the Anti-Aliasing combobox row.
  2. **Camera Position** (`GroupBox { title: "Camera Position"; }`) -- contains the Longitude, Latitude, and Zoom slider rows.
  3. **Camera Orientation** (`GroupBox { title: "Camera Orientation"; }`) -- initially empty; will receive Tilt, Yaw, and Pitch sliders in Phase 4. Include the `GroupBox` now as an empty placeholder so the visual structure is established. (Alternatively, defer adding this `GroupBox` until Phase 4 -- either approach is acceptable as long as the final state has all five groups.)
  4. **Framing** (`GroupBox { title: "Framing"; }`) -- initially empty; will receive Offset X and Offset Y sliders in Phase 3.
  5. **Lighting** (`GroupBox { title: "Lighting"; }`) -- contains the Terminator Width slider row, the Diffuse checkbox row, the Floor slider row, and the Ramp slider row.

  Below the groups (still inside the outer `VerticalLayout`, but outside any `GroupBox`): the "Set as Wallpaper" button and the wallpaper status text. The renderer info text stays pinned to the bottom of the panel via absolute positioning (unchanged).

  Each `GroupBox` contains a `VerticalLayout` with the same `spacing: 4px` as the current layout. The existing `HorizontalLayout` rows for each control move into their respective `GroupBox` unchanged -- same element IDs, same property bindings, same callbacks.

- **Test cases** (manual):
  - All existing controls (Texture combobox, AA combobox, Longitude/Latitude/Zoom sliders, Terminator slider, Diffuse checkbox + Floor + Ramp sliders) function identically to before.
  - The controls are visually grouped with `GroupBox` borders and titles.
  - The "Set as Wallpaper" button and status text remain below the groups.
  - The renderer info text remains pinned to the bottom.
  - Window resize and splitter drag work correctly.
- **Verify**: `cargo build` succeeds. Manual verification that all existing controls and their callbacks work without change.
- **Complexity**: Small

### Phase 1: Camera Parameter Struct

This phase introduces a `CameraParams` struct to group all camera parameters, reducing the argument count of `build_frame_state()` and `write_uniforms()` and providing a clean extension point for the new parameters.

#### Step 1.1: Add `CameraParams` struct to `camera.rs`

- **Files**: `src/scene/camera.rs`
- **Action**: Define a new `CameraParams` struct that groups all camera-related values that flow from the UI to the renderer. Initially it contains only the existing three fields (`longitude`, `latitude`, `zoom`). The struct is `Copy + Clone + Debug` for convenience.

  ```rust
  #[derive(Clone, Copy, Debug)]
  pub struct CameraParams {
      pub longitude: f32,
      pub latitude: f32,
      pub zoom: f32,
  }
  ```

  Add a `default()` implementation returning the current defaults (longitude 0.0, latitude 30.0, zoom 8.0).

- **Test cases** (unit, in `camera.rs`):
  - `camera_params_default_values`: Assert that `CameraParams::default()` returns longitude 0.0, latitude 30.0, zoom 8.0.
- **Verify**: `cargo test camera` passes. The struct exists and is accessible from other modules.
- **Complexity**: Small

#### Step 1.2: Thread `CameraParams` through `build_frame_state` and `write_uniforms`

- **Files**: `src/renderer/frame.rs`, `src/renderer/render_pass.rs`, `src/renderer/mod.rs`
- **Action**: Change `build_frame_state()` to accept a `&CameraParams` instead of separate `longitude`, `latitude`, `zoom` arguments. Change `write_uniforms()` similarly. Update all call sites in `renderer/mod.rs` (`rendering_callback` and `export_wallpaper_image`) to construct a `CameraParams` and pass it. The `FrameState` struct itself keeps its flat fields for now (the quantization logic stays in `build_frame_state`).
- **Test cases**:
  - All existing `frame.rs` tests updated to pass `CameraParams` instead of separate args. All must still pass with identical behavior.
  - All existing `camera.rs` tests still pass unchanged.
- **Verify**: `cargo test` passes. `cargo clippy` passes. No behavioral change.
- **Complexity**: Medium

### Phase 2: Zoom Curve

This phase replaces the linear zoom slider with an exponential mapping. It has the smallest blast radius of the four features and changes only the slider range and the mapping function.

#### Step 2.1: Add exponential zoom mapping function to `camera.rs`

- **Files**: `src/scene/camera.rs`
- **Action**: Add a pure function `pub fn zoom_to_distance(t: f32) -> f32` that applies the exponential mapping `1.5 * (80.0_f32 / 1.5).powf(t)`. Also add the inverse function `pub fn distance_to_zoom(distance: f32) -> f32` that computes `(distance / 1.5).ln() / (80.0_f32 / 1.5).ln()` for converting existing distance values back to slider positions. Define constants `ZOOM_DISTANCE_MIN: f32 = 1.5` and `ZOOM_DISTANCE_MAX: f32 = 80.0`.
- **Test cases** (unit, in `camera.rs`):
  - `zoom_to_distance_at_zero`: `zoom_to_distance(0.0)` returns 1.5 (within epsilon).
  - `zoom_to_distance_at_one`: `zoom_to_distance(1.0)` returns 80.0 (within epsilon).
  - `zoom_to_distance_at_half`: `zoom_to_distance(0.5)` returns approximately `sqrt(1.5 * 80.0)` = ~10.95 (within 0.1).
  - `zoom_to_distance_monotonic`: For t values 0.0, 0.25, 0.5, 0.75, 1.0, assert each successive distance is strictly greater.
  - `distance_to_zoom_roundtrip`: For several distance values, assert `zoom_to_distance(distance_to_zoom(d))` equals `d` within epsilon.
  - `distance_to_zoom_default`: `distance_to_zoom(8.0)` returns a value in (0.0, 1.0), which becomes the new slider default.
  - Property test with `proptest`: For `t` in 0.0..=1.0, `zoom_to_distance(t)` is in `[1.5, 80.0]`.
- **Verify**: `cargo test camera` passes.
- **Complexity**: Small

#### Step 2.2: Apply zoom mapping in `write_uniforms` and update defaults

- **Files**: `src/renderer/render_pass.rs`, `src/scene/camera.rs`
- **Action**: In `write_uniforms()`, apply `zoom_to_distance(camera_params.zoom)` to convert the slider value to a camera distance before passing it to `OrbitalCamera::new()`. Update `CameraParams::default()` so `zoom` is `distance_to_zoom(8.0)` (the normalized value that produces the old default distance of 8.0).
- **Test cases**:
  - Existing `write_uniforms` callers continue to work. The wallpaper export path in `export_wallpaper_image` already uses `state.zoom` from `FrameState`, which now stores the raw slider value. The mapping is applied in `write_uniforms`, so both preview and export apply it consistently.
  - Manual verification: the app starts with the same visual zoom as before (Earth at approximately the same size).
- **Verify**: `cargo build` succeeds. `cargo test` passes. Visual verification that the default zoom matches the old behavior.
- **Complexity**: Small

#### Step 2.3: Update zoom slider range in Slint UI

- **Files**: `ui/main.slint`
- **Action**: In the **Camera Position** `GroupBox`, change the zoom slider range from `minimum: 2.0` / `maximum: 50.0` / `value: 8.0` to `minimum: 0.0` / `maximum: 1.0` / `value: <default_zoom>` where `<default_zoom>` is the result of `distance_to_zoom(8.0)` (approximately 0.42). Update the value display text to show the mapped distance: `round(1.5 * pow(80.0 / 1.5, zoom-slider.value) * 10) / 10`. Since Slint does not have a `pow()` function, the display will instead show the raw slider percentage: `round(zoom-slider.value * 100) + "%"`. Alternatively, the mapped distance can be computed in Rust and written to an `in` property.

  **Chosen approach**: Add an `in property <float> zoom-display-distance` to `MainWindow`. Rust computes `zoom_to_distance(zoom_value)` in the `BeforeRendering` callback and writes it via `set_zoom_display_distance()`. The Slint value label reads this property: `round(root.zoom-display-distance * 10) / 10`.

- **Test cases** (manual):
  - Zoom slider moves from 0% to 100%, label shows distance from ~1.5 to ~80.0.
  - Default position shows distance ~8.0.
  - Zoom feels perceptually even: slow changes at close range, faster at far range.
- **Verify**: `cargo build` succeeds. Manual verification of slider behavior.
- **Complexity**: Small

### Phase 3: Pan/Offset

This phase adds screen-space pan (post-projection translation) so the Earth can be offset from the center of the viewport.

#### Step 3.1: Add offset fields to `CameraParams` and `OrbitalCamera`

- **Files**: `src/scene/camera.rs`
- **Action**: Add `offset_x: f32` and `offset_y: f32` to `CameraParams` (default 0.0). Add the same fields to `OrbitalCamera`. Modify `OrbitalCamera::mvp_matrix()` to apply a post-projection translation: multiply the current MVP by `Mat4::from_translation(Vec3::new(offset_x, offset_y, 0.0))` on the left side (i.e., `translation * projection * view`). This shifts the clip-space output, moving the rendered image within the viewport.
- **Test cases** (unit, in `camera.rs`):
  - `mvp_with_zero_offset_unchanged`: Assert that `mvp_matrix` with `offset_x=0, offset_y=0` produces the same result as the old computation (compare against a camera constructed without offsets).
  - `mvp_with_positive_x_offset_shifts_right`: Render a point at the origin through two MVPs (one with offset_x=0.5, one with offset_x=0.0). Assert the clip-space X coordinate is larger with the offset.
  - `mvp_with_offset_preserves_depth`: Assert that the Z (depth) component of a transformed point is unchanged by the offset.
- **Verify**: `cargo test camera` passes.
- **Complexity**: Small

#### Step 3.2: Add offset to `FrameState` and thread through pipeline

- **Files**: `src/renderer/frame.rs`, `src/renderer/render_pass.rs`, `src/renderer/mod.rs`
- **Action**: Add `offset_x: f32` and `offset_y: f32` to `FrameState` (raw float, like longitude/latitude). Update `build_frame_state` to accept them from `CameraParams`. Update `write_uniforms` to pass them to `OrbitalCamera`. Update call sites in `rendering_callback` and `export_wallpaper_image`.
- **Test cases**:
  - `frame_state_offset_triggers_dirty`: Assert that changing `offset_x` or `offset_y` in `CameraParams` produces a different `FrameState`.
- **Verify**: `cargo test` passes.
- **Complexity**: Small

#### Step 3.3: Add pan sliders to Slint UI

- **Files**: `ui/main.slint`, `src/renderer/mod.rs`
- **Action**: Add two new sliders ("Offset X" and "Offset Y") inside the **Framing** `GroupBox`. Each ranges from -1.0 to 1.0 with default 0.0. Add two `out` properties `camera-offset-x` and `camera-offset-y` bound to the slider values. The sliders fire `sliders-changed()` on change. In `renderer/mod.rs`, read these properties via `win.get_camera_offset_x()` and `win.get_camera_offset_y()` and include them in the `CameraParams`.
- **Test cases** (manual):
  - Moving Offset X right shifts the Earth to the right.
  - Moving Offset Y up shifts the Earth upward.
  - At default (0, 0) the Earth is centered as before.
  - Wallpaper export with non-zero offset produces a correctly offset image.
- **Verify**: `cargo build` succeeds. Manual verification.
- **Complexity**: Small

### Phase 4: Camera Rotations (Tilt, Yaw, Pitch)

This phase adds three post-view camera rotations: tilt (roll around the view Z-axis), yaw (rotation around the view Y-axis for horizontal look redirection), and pitch (rotation around the view X-axis for vertical look redirection). All three are composed as post-view rotations: `rotated_view = Rz(tilt) * Rx(pitch) * Ry(yaw) * base_view`. Yaw and pitch create perspective shifts that change where the camera is pointing, distinct from the screen-space offset/pan in Phase 3.

#### Step 4.1: Add tilt, yaw, and pitch fields to `CameraParams` and `OrbitalCamera`

- **Files**: `src/scene/camera.rs`
- **Action**: Add `tilt_deg: f32`, `yaw_deg: f32`, and `pitch_deg: f32` to `CameraParams` (all default 0.0) and `OrbitalCamera`. Modify `OrbitalCamera::view_matrix()` to apply the three rotations as a composed post-view transformation:

  ```rust
  let tilt = Mat4::from_rotation_z(self.tilt_deg.to_radians());
  let pitch = Mat4::from_rotation_x(self.pitch_deg.to_radians());
  let yaw = Mat4::from_rotation_y(self.yaw_deg.to_radians());
  let rotated_view = tilt * pitch * yaw * base_view;
  ```

  This composition order means yaw is applied first (rotating the view direction left/right), then pitch (rotating up/down), then tilt (rolling the camera). The order ensures that tilt does not affect the axes that yaw and pitch rotate around, keeping each rotation's effect intuitive.

- **Test cases** (unit, in `camera.rs`):
  - `view_with_zero_rotations_unchanged`: Assert that `view_matrix` with `tilt_deg=0.0`, `yaw_deg=0.0`, `pitch_deg=0.0` equals the result without any rotations applied.
  - `view_with_180_tilt_flips_vertical`: Transform a point at (0, 1, 0) through view matrices with tilt=0 and tilt=180. Assert the Y components have opposite signs (the view is flipped upside down).
  - `view_with_tilt_preserves_determinant`: Assert the view matrix determinant is non-zero for tilt values 0, 45, 90, 135, 180.
  - `tilt_360_equals_zero`: Assert `view_matrix` with `tilt_deg=360.0` approximately equals the tilt=0 case.
  - `view_with_yaw_rotates_horizontally`: Transform a point at (0, 0, -1) (directly ahead in view space) through view matrices with yaw=0 and yaw=45. Assert the clip-space X coordinates differ (the view has rotated horizontally).
  - `view_with_negative_yaw_mirrors_positive`: Assert that yaw=30 and yaw=-30 produce view matrices that are mirror images in the X component of transformed points.
  - `view_with_yaw_preserves_determinant`: Assert the view matrix determinant is non-zero for yaw values -90, -45, 0, 45, 90.
  - `view_with_pitch_rotates_vertically`: Transform a point at (0, 0, -1) through view matrices with pitch=0 and pitch=30. Assert the clip-space Y coordinates differ (the view has rotated vertically).
  - `view_with_negative_pitch_mirrors_positive`: Assert that pitch=30 and pitch=-30 produce view matrices that are mirror images in the Y component of transformed points.
  - `view_with_pitch_preserves_determinant`: Assert the view matrix determinant is non-zero for pitch values -90, -45, 0, 45, 90.
  - `all_rotations_zero_equals_no_rotation`: Assert that applying tilt=0, yaw=0, pitch=0 gives the same matrix as the base view with no rotation code path.
  - `rotations_compose_correctly`: Assert that applying tilt=30, yaw=20, pitch=10 all at once gives the same result as manually composing `Rz(30) * Rx(10) * Ry(20) * base_view`.
- **Verify**: `cargo test camera` passes.
- **Complexity**: Small

#### Step 4.2: Add tilt, yaw, and pitch to `FrameState` and thread through pipeline

- **Files**: `src/renderer/frame.rs`, `src/renderer/render_pass.rs`, `src/renderer/mod.rs`
- **Action**: Add `tilt: f32`, `yaw: f32`, and `pitch: f32` to `FrameState`. Update `build_frame_state` and `write_uniforms` to pass tilt, yaw, and pitch from `CameraParams`. Update call sites.
- **Test cases**:
  - `frame_state_tilt_triggers_dirty`: Assert changing `tilt_deg` in `CameraParams` produces a different `FrameState`.
  - `frame_state_yaw_triggers_dirty`: Assert changing `yaw_deg` in `CameraParams` produces a different `FrameState`.
  - `frame_state_pitch_triggers_dirty`: Assert changing `pitch_deg` in `CameraParams` produces a different `FrameState`.
- **Verify**: `cargo test` passes.
- **Complexity**: Small

#### Step 4.3: Add tilt, yaw, and pitch sliders to Slint UI

- **Files**: `ui/main.slint`, `src/renderer/mod.rs`
- **Action**: Add three sliders inside the **Camera Orientation** `GroupBox`:
  - "Tilt" slider: range -180 to 180, default 0. Add an `out property <float> camera-tilt` bound to the slider value.
  - "Yaw" slider: range -90 to 90, default 0. Add an `out property <float> camera-yaw` bound to the slider value.
  - "Pitch" slider: range -90 to 90, default 0. Add an `out property <float> camera-pitch` bound to the slider value.

  All three sliders fire `sliders-changed()` on change. In `renderer/mod.rs`, read via `win.get_camera_tilt()`, `win.get_camera_yaw()`, and `win.get_camera_pitch()`.

- **Test cases** (manual):
  - Moving the tilt slider rotates the Earth's horizon (roll).
  - Moving the yaw slider shifts the view horizontally, creating a perspective change where different parts of the Earth become visible on the left/right edges.
  - Moving the pitch slider shifts the view vertically, creating a perspective change where different parts of the Earth become visible on the top/bottom edges.
  - At default (0, 0, 0) the view is upright and centered as before.
  - Tilt interacts correctly with pan offsets (the Earth tilts in place, offset is still in screen space).
  - Yaw and pitch interact correctly with pan offsets.
  - All three rotations combine as expected (e.g., tilt=45 + yaw=30 produces a tilted and yaw-shifted view).
- **Verify**: `cargo build` succeeds. Manual verification.
- **Complexity**: Small

### Phase 5: Mouse Controls

This phase adds direct manipulation via mouse drag (rotate) and scroll wheel (zoom). It depends on all previous phases because the drag math must account for tilt, and the scroll must use the exponential zoom mapping.

#### Step 5.1: Change camera properties from `out` to `in-out` in Slint

- **Files**: `ui/main.slint`
- **Action**: Change the camera-related properties from `out` to `in-out` so Rust can write values back when processing mouse events. Change the slider bindings from one-way value assignments to bidirectional bindings (`<=>`). The affected properties are: `camera-longitude`, `camera-latitude`, `camera-zoom`, `camera-offset-x`, `camera-offset-y`, `camera-tilt`, `camera-yaw`, `camera-pitch`.

  Before:

  ```slint
  out property <float> camera-longitude: longitude-slider.value;
  ```

  After:

  ```slint
  in-out property <float> camera-longitude: 0;
  // ...
  longitude-slider := Slider {
      minimum: -180;
      maximum: 180;
      value <=> root.camera-longitude;
      changed => { root.sliders-changed(); }
  }
  ```

  Set the initial default values on the properties themselves (not on the sliders), since the properties are now the source of truth.

- **Test cases** (manual):
  - All sliders still work as before (moving a slider changes the rendering).
  - The value display text updates correctly.
  - The generated Rust `set_camera_*` methods are callable.
- **Verify**: `cargo build` succeeds. Manual verification that sliders still work.
- **Complexity**: Small

#### Step 5.2: Add `mouse-drag` callback for rotation-aware globe rotation

- **Files**: `ui/main.slint`, `src/main.rs`
- **Action**: Since tilt changes the mapping between screen-space drag and longitude/latitude, the drag math must be done in Rust (Option B from the research document). Add a `TouchArea` inside `image-container`, filling the entire viewport area, placed after the `Image` element so it is on top. Add a Slint callback `callback mouse-drag(float, float)` that fires from the `TouchArea.moved` handler with incremental deltas.

  To compute incremental deltas, add two Slint properties `property <length> last-mouse-x` and `property <length> last-mouse-y` inside the `image-container`. On `pointer-event(down)`, store the current `mouse-x`/`mouse-y`. On `moved`, compute `delta-x = mouse-x - last-mouse-x`, `delta-y = mouse-y - last-mouse-y`, update the stored values, and fire the callback with the deltas converted to `float` (dividing by `1px`).

  In `main.rs`, wire `on_mouse_drag` to read the current tilt angle, rotate the delta vector by negative tilt to undo the screen-space rotation, scale the deltas by a sensitivity factor proportional to `zoom_to_distance(current_zoom)`, and update the camera longitude and latitude properties via `set_camera_longitude()` and `set_camera_latitude()`. Clamp latitude to [-89, 89]. Wrap longitude to [-180, 180]. Call `request_redraw()`.

  The sensitivity formula:

  ```text
  degrees_per_px = 0.3 * zoom_to_distance(zoom) / 8.0
  ```

  This gives 0.3 degrees/px at the default zoom and scales proportionally with distance.

  The tilt correction rotates the (dx, dy) vector by `-tilt_rad`:

  ```text
  corrected_dx = dx * cos(-tilt) - dy * sin(-tilt)
  corrected_dy = dx * sin(-tilt) + dy * cos(-tilt)
  ```

  Note: Yaw and pitch do not affect the screen-space drag direction the way tilt does. Tilt rotates the entire rendered image on screen, so a horizontal screen drag no longer corresponds to a pure longitude change. Yaw and pitch, by contrast, redirect the camera's look direction but do not rotate the screen axes -- a horizontal drag still maps to a horizontal camera movement. Therefore, only tilt needs to be compensated in the drag correction math.

- **Test cases** (manual):
  - Dragging right on the viewport rotates the Earth so different longitudes come into view (longitude decreases, globe appears to rotate right).
  - Dragging up shows more of the northern hemisphere (latitude increases).
  - Drag direction is correct at 0 tilt: horizontal drag changes longitude, vertical drag changes latitude.
  - Drag direction is correct at 90-degree tilt: horizontal drag changes latitude, vertical drag changes longitude (because the screen is rotated).
  - Drag direction remains correct at non-zero yaw and pitch values (no correction needed for these).
  - Sliders update in real-time during drag.
  - Releasing the mouse stops rotation.
- **Verify**: `cargo build` succeeds. Manual drag testing at 0 and non-zero tilt values, and at non-zero yaw/pitch values.
- **Complexity**: Medium

#### Step 5.3: Add scroll-to-zoom handler

- **Files**: `ui/main.slint`, `src/main.rs`
- **Action**: Add a `scroll-event` handler on the same `TouchArea`. Add a Slint callback `callback mouse-scroll(float)` that fires with the vertical scroll delta (converted to float). Return `EventResult.accept` to prevent scroll propagation.

  In `main.rs`, wire `on_mouse_scroll` to adjust the zoom property: `new_zoom = (current_zoom - delta * scroll_sensitivity).clamp(0.0, 1.0)`. A `scroll_sensitivity` of 0.05 gives a reasonable zoom step per scroll notch. Write back via `set_camera_zoom()` and call `request_redraw()`.

- **Test cases** (manual):
  - Scrolling up zooms in (zoom slider moves left, Earth gets bigger).
  - Scrolling down zooms out (zoom slider moves right, Earth gets smaller).
  - Zoom is clamped to the slider range (0.0 to 1.0).
  - The zoom slider updates in sync with scroll wheel changes.
  - Scroll does not propagate to the window (no window scrolling).
- **Verify**: `cargo build` succeeds. Manual scroll testing.
- **Complexity**: Small

### Phase 6: Reset Button and Polish

#### Step 6.1: Add "Reset Camera" button to Slint UI

- **Files**: `ui/main.slint`, `src/main.rs`
- **Action**: Add a "Reset Camera" button in the controls panel, placed below the five `GroupBox` sections and above the "Set as Wallpaper" button. Add a callback `callback reset-camera()`. In `main.rs`, wire the callback to set all camera properties to their defaults: longitude 0, latitude 30, zoom `distance_to_zoom(8.0)`, offset_x 0, offset_y 0, tilt 0, yaw 0, pitch 0. Call `request_redraw()`. The Reset Camera button resets the Camera Position, Camera Orientation, and Framing groups; it does not affect the Rendering or Lighting groups.
- **Test cases** (manual):
  - Move all camera controls to non-default values, click Reset Camera, all return to defaults.
  - The rendered view matches the initial startup view after reset.
  - Rendering and Lighting settings are unaffected by the reset.
- **Verify**: `cargo build` succeeds. Manual verification.
- **Complexity**: Small

#### Step 6.2: Run full test suite, clippy, and manual verification

- **Files**: N/A
- **Action**: Run `cargo test` to verify all tests pass. Run `cargo clippy` to verify no warnings. Perform comprehensive manual testing of all features.
- **Manual test cases**:
  - Application starts with the same default view as before (zoom curve's default matches old distance 8.0).
  - Longitude, latitude sliders work as before.
  - Zoom slider feels perceptually even across its range.
  - Offset X and Y sliders shift the Earth in the expected directions.
  - Tilt slider rotates the view around the center.
  - Yaw slider shifts the view direction horizontally.
  - Pitch slider shifts the view direction vertically.
  - All three rotation sliders compose correctly (e.g., tilt + yaw + pitch together).
  - Mouse drag rotates the globe, with correct behavior at various tilt angles.
  - Mouse drag remains correct at non-zero yaw/pitch values (no correction needed).
  - Mouse scroll zooms in/out.
  - Reset Camera returns all parameters to defaults.
  - Wallpaper export reflects the current camera state (offset, tilt, yaw, pitch, zoom).
  - All existing features (texture switching, MSAA, terminator, diffuse shading) work without regression.
  - Window resize works correctly.
  - `cargo run -- --software-rendering` still works.
  - GroupBox sections display correctly with proper titles and grouping.
- **Verify**: All automated tests and manual checks pass.
- **Complexity**: Medium

#### Step 6.3: Update `CLAUDE.md`

- **Files**: `CLAUDE.md`
- **Action**: Update architecture documentation to reflect:
  - `OrbitalCamera` now accepts offset, tilt, yaw, pitch, and uses exponential zoom mapping.
  - `CameraParams` struct groups all camera parameters.
  - Camera properties are `in-out` (bidirectional) in Slint.
  - `TouchArea` in `image-container` handles mouse drag and scroll.
  - New UI controls: Offset X, Offset Y, Tilt, Yaw, Pitch sliders; Reset Camera button.
  - Controls panel is organized into five `GroupBox` sections: Rendering (Texture, Anti-Aliasing), Camera Position (Longitude, Latitude, Zoom), Camera Orientation (Tilt, Yaw, Pitch), Framing (Offset X, Offset Y), Lighting (Terminator Width, Diffuse checkbox + Floor + Ramp). Reset Camera and Set as Wallpaper buttons are below the groups. Renderer info is pinned to the bottom.
  - `FrameState` now includes offset_x, offset_y, tilt, yaw, and pitch.
  - Zoom slider is normalized (0.0 to 1.0) with exponential mapping.
  - Update the `dirty-checking` line in Key Constraints to include the new fields.
- **Verify**: `CLAUDE.md` accurately reflects the updated architecture.
- **Complexity**: Small

## Test Strategy

### Automated Tests

| Test Case | Type | Input | Expected Output |
| --- | --- | --- | --- |
| `camera_params_default_values` | Unit | `CameraParams::default()` | longitude 0.0, latitude 30.0, zoom ~0.42 |
| `zoom_to_distance_at_zero` | Unit | t=0.0 | 1.5 |
| `zoom_to_distance_at_one` | Unit | t=1.0 | 80.0 |
| `zoom_to_distance_at_half` | Unit | t=0.5 | ~10.95 |
| `zoom_to_distance_monotonic` | Unit | t=0.0, 0.25, 0.5, 0.75, 1.0 | Strictly increasing distances |
| `distance_to_zoom_roundtrip` | Unit | Various distances | Roundtrip within epsilon |
| `distance_to_zoom_default` | Unit | distance=8.0 | Value in (0.0, 1.0) |
| `zoom_range_proptest` | Property | t in 0.0..=1.0 | Result in [1.5, 80.0] |
| `mvp_with_zero_offset_unchanged` | Unit | offset_x=0, offset_y=0 | Same MVP as without offsets |
| `mvp_with_positive_x_offset_shifts_right` | Unit | offset_x=0.5 vs 0.0 | Clip-space X is larger |
| `mvp_with_offset_preserves_depth` | Unit | offset_x=0.5 | Z unchanged |
| `view_with_zero_rotations_unchanged` | Unit | tilt=0, yaw=0, pitch=0 | Same view matrix as base |
| `view_with_180_tilt_flips_vertical` | Unit | tilt_deg=180 | Y components flip sign |
| `view_with_tilt_preserves_determinant` | Unit | Various tilt values | Non-zero determinant |
| `tilt_360_equals_zero` | Unit | tilt_deg=360 | Approx. equals tilt=0 |
| `view_with_yaw_rotates_horizontally` | Unit | yaw=0 vs yaw=45 | Clip-space X differs |
| `view_with_negative_yaw_mirrors_positive` | Unit | yaw=30 vs yaw=-30 | X components mirror |
| `view_with_yaw_preserves_determinant` | Unit | Various yaw values | Non-zero determinant |
| `view_with_pitch_rotates_vertically` | Unit | pitch=0 vs pitch=30 | Clip-space Y differs |
| `view_with_negative_pitch_mirrors_positive` | Unit | pitch=30 vs pitch=-30 | Y components mirror |
| `view_with_pitch_preserves_determinant` | Unit | Various pitch values | Non-zero determinant |
| `all_rotations_zero_equals_no_rotation` | Unit | tilt=0, yaw=0, pitch=0 | Same as base view |
| `rotations_compose_correctly` | Unit | tilt=30, yaw=20, pitch=10 | Equals manual Rz*Rx*Ry*base |
| `frame_state_offset_triggers_dirty` | Unit | Changed offset_x/y | Different FrameState |
| `frame_state_tilt_triggers_dirty` | Unit | Changed tilt | Different FrameState |
| `frame_state_yaw_triggers_dirty` | Unit | Changed yaw | Different FrameState |
| `frame_state_pitch_triggers_dirty` | Unit | Changed pitch | Different FrameState |
| Existing `frame_state_*` tests | Unit | Updated for CameraParams | All pass unchanged |
| Existing `camera.rs` tests | Unit | Unchanged | All pass unchanged |

### Manual Verification

- [ ] Application starts with default view matching pre-change behavior
- [ ] Controls panel displays five GroupBox sections with correct titles and control grouping
- [ ] Zoom slider feels perceptually even (slow close, fast far)
- [ ] Offset X/Y sliders shift the Earth in the expected screen-space directions
- [ ] Tilt slider rotates the view around the camera's forward axis
- [ ] Yaw slider shifts the view direction horizontally (perspective change, not screen-space shift)
- [ ] Pitch slider shifts the view direction vertically (perspective change, not screen-space shift)
- [ ] Yaw and pitch at their limits (-90, 90) produce expected extreme perspective views
- [ ] All three rotations compose correctly when used together
- [ ] Mouse drag rotates the globe; direction is correct at 0 and non-zero tilt
- [ ] Mouse drag direction is unaffected by yaw/pitch values
- [ ] Mouse scroll zooms in/out; slider updates in sync
- [ ] Reset Camera button restores all defaults for Position, Orientation, and Framing groups (including yaw and pitch)
- [ ] Reset Camera button does not affect Rendering or Lighting settings
- [ ] Wallpaper export reflects current camera state including offset, tilt, yaw, pitch, zoom
- [ ] All existing features work without regression
- [ ] `cargo build --release` succeeds
- [ ] `cargo clippy` passes
- [ ] `cargo test` passes

## Risks and Mitigations

| Risk | Impact | Mitigation |
| --- | --- | --- |
| Tilt interacts with mouse drag direction, making rotation feel wrong if drag math does not account for tilt | Confusing user experience | Implement camera rotations (Phase 4) before mouse controls (Phase 5). The drag callback rotates the (dx, dy) vector by negative tilt to correct the screen-space mapping. |
| Yaw/pitch could theoretically interact with mouse drag direction | Confusing user experience | Analysis shows yaw and pitch redirect the camera look direction without rotating the screen axes, so no drag correction is needed. Verify during manual testing. |
| Changing camera properties from `out` to `in-out` could cause unexpected initialization ordering in Slint | Camera starts at wrong defaults | Set default values on the properties themselves. Verify defaults match the old behavior after the change. |
| Zoom curve changes the default view if `distance_to_zoom(8.0)` does not produce exactly the old default distance | Visual regression on startup | Use `distance_to_zoom(8.0)` as the default slider value and verify via automated roundtrip tests that the mapped distance matches 8.0 within a tight epsilon. |
| `build_frame_state` argument list is already long; adding more parameters exacerbates ergonomic issues | Hard-to-maintain code | Phase 1 introduces `CameraParams` struct before adding new parameters, keeping the call sites clean. |
| `TouchArea` on viewport consumes all mouse events, preventing future click-based interaction | Limits future features | Document this constraint. The `TouchArea` can distinguish drag from click via `pointer-event` if needed later. |
| Mouse drag sensitivity feels wrong at extreme zoom levels | Poor user experience | Scale sensitivity proportionally to `zoom_to_distance(zoom)`. Tune the base factor (0.3 degrees/px) during manual testing. |
| At extreme offset + close zoom, Earth may leave the viewport entirely | Confusing for users | This is intentional behavior for composition. The Reset Camera button provides a quick escape. |
| Scroll delta sign varies by platform or input device | Inverted zoom direction | Test on Windows with standard mouse. The sign can be flipped with a single negation if needed. |
| Combining extreme yaw/pitch with extreme tilt could produce disorienting views | Confusing for users | The narrower yaw/pitch range (-90 to 90) limits the extremes. The Reset Camera button provides a quick escape. |
| GroupBox adds visual overhead (borders, titles) that could feel crowded in a narrow panel | Cluttered UI | The splitter allows the user to resize the controls panel. Test at the minimum panel width (160px) to ensure GroupBox titles and controls remain readable. |

## Rollback Strategy

Revert the commits from this plan. The changes are self-contained:

- Revert `ui/main.slint`: Remove `GroupBox` import and section structure, restore the flat control list. Remove new sliders, `TouchArea`, `in-out` change, Reset button. Restore `out` properties.
- Revert `src/scene/camera.rs`: Remove `CameraParams`, offset/tilt/yaw/pitch fields, zoom mapping functions, and new `OrbitalCamera` fields.
- Revert `src/renderer/frame.rs`: Remove new `FrameState` fields and `CameraParams` parameter.
- Revert `src/renderer/render_pass.rs`: Revert `write_uniforms` signature to separate arguments.
- Revert `src/renderer/mod.rs`: Remove `CameraParams` construction and new property reads.
- Revert `src/main.rs`: Remove mouse-drag, mouse-scroll, and reset-camera callbacks.
- Revert `CLAUDE.md`.

No shader, uniform buffer, or GPU pipeline changes were made. The render pipeline is entirely unchanged by the rollback.

## File Inventory

```text
src/scene/camera.rs              (modified: CameraParams struct, offset/tilt/yaw/pitch fields,
                                  zoom mapping functions, modified view_matrix/mvp_matrix)
src/renderer/frame.rs            (modified: new FrameState fields, CameraParams parameter)
src/renderer/render_pass.rs      (modified: write_uniforms accepts CameraParams)
src/renderer/mod.rs              (modified: reads new UI properties, constructs CameraParams,
                                  writes zoom-display-distance)
ui/main.slint                    (modified: GroupBox import, controls reorganized into five
                                  GroupBox sections, in-out properties, bidirectional slider
                                  bindings, new sliders for offset/tilt/yaw/pitch, TouchArea,
                                  mouse callbacks, reset button, zoom-display-distance property)
src/main.rs                      (modified: mouse-drag, mouse-scroll, reset-camera callbacks)
CLAUDE.md                        (modified: updated architecture documentation)
```

## Open Decisions

These items from the research document are resolved as follows for this plan:

1. **Pan range limits** (research question 1): No clamping. The user is free to pan the Earth off-screen. The Reset Camera button mitigates accidental full-offset.

2. **Zoom slider display** (research question 2): Show the mapped distance (computed in Rust, displayed via `zoom-display-distance` property) formatted to one decimal place.

3. **Mouse drag sensitivity scaling** (research question 3): Yes, sensitivity scales with `zoom_to_distance(zoom) / 8.0`, giving finer control at close zoom and coarser control at far zoom.

4. **Tilt range** (research question 4): Full -180 to 180 degrees. This allows any orientation including fully inverted views. Users who want only moderate tilt can simply stay near the center of the slider range.

5. **Yaw and pitch range**: -90 to 90 degrees each. This range covers the full useful hemisphere of look-direction redirection. Beyond 90 degrees the camera would be looking away from the Earth, which is not useful. The narrower range compared to tilt reflects the different nature of these rotations: tilt is a compositional roll that benefits from full 360-degree freedom, while yaw and pitch redirect where the camera points.

6. **Cumulative vs. incremental drag deltas** (research question 5): Incremental deltas, tracked via `last-mouse-x`/`last-mouse-y` Slint properties, with the actual rotation computed in Rust via the `mouse-drag` callback (Option B).

7. **Reset mechanism** (research question 6): A "Reset Camera" button in Phase 6, placed in the controls panel below the GroupBox sections. It resets all camera parameters (Position, Orientation, and Framing groups) including yaw and pitch, but does not affect Rendering or Lighting settings.

8. **Mouse drag correction for yaw/pitch**: Only tilt requires drag correction. Yaw and pitch redirect the camera's look direction without rotating the screen coordinate axes, so screen-space drag directions remain aligned with longitude/latitude even at non-zero yaw/pitch values. This is verified during manual testing.

## Status

- [x] Plan approved
- [x] Implementation started
- [ ] Implementation complete
