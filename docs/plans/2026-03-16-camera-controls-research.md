# Research: Improved Camera Controls (2026-03-16)

## Problem Statement

Sunlit Earth's camera is a minimal orbital model: three sliders (longitude, latitude, zoom) control a camera that always looks at the exact center of the Earth with a fixed orientation. This is sufficient for basic viewing but limits compositional flexibility -- the Earth must always be centered, there is no way to angle the view, zoom feels uneven across its range, and all interaction is via sliders rather than direct manipulation.

Four features are proposed to address this:

1. **Pan/offset** -- shift the camera's look-at point so the Earth can be positioned off-center (e.g., lower-right for a wallpaper composition).
2. **Tilt** -- rotate the camera around its view axis for angled compositions.
3. **Zoom curve** -- non-linear zoom mapping so the slider feels perceptually even, with extended range limits.
4. **Mouse controls** -- drag to rotate the globe, scroll to zoom, replacing slider-only interaction.

## Current State

### Camera Model (`src/scene/camera.rs`)

`OrbitalCamera` is defined by four fields:

| Field | Type | Range | Default | Notes |
|-------|------|-------|---------|-------|
| `longitude_deg` | f32 | -180 to 180 | 0 | Rotation around Y axis |
| `latitude_deg` | f32 | -89.9 to 89.9 | 30 | Tilt up/down, clamped to avoid gimbal lock |
| `distance` | f32 | 2.0 to 50.0 | 8.0 | Distance from origin ("zoom" slider value) |
| `fov_deg` | f32 | -- | 20.0 | Hardcoded in `new()`, not user-adjustable |

The camera always looks at `Vec3::ZERO` with `Vec3::Y` as the up vector. Eye position is computed via standard spherical-to-Cartesian conversion. The MVP matrix is `projection * view` with identity model transform (the unit sphere sits at the origin).

**Coordinate frame** (shared with `sun.rs`):
- `+Z` = prime meridian at equator (0N, 0E)
- `+Y` = north pole
- `+X` = 90 degrees East (0N, 90E)

### UI Controls (`ui/main.slint`)

Three camera sliders exist in the left-pane `VerticalLayout`, each in an `HorizontalLayout` with a label, slider, and value display:

| Slider | Property | Direction | Min | Max | Default |
|--------|----------|-----------|-----|-----|---------|
| Longitude | `camera-longitude` | `out` | -180 | 180 | 0 |
| Latitude | `camera-latitude` | `out` | -89 | 89 | 30 |
| Zoom | `camera-zoom` | `out` | 2.0 | 50.0 | 8.0 |

All three properties are `out` (Slint writes, Rust reads). Rust reads them via `win.get_camera_*()` during the `BeforeRendering` callback. Any slider change fires the shared `sliders-changed()` callback, which calls `win.window().request_redraw()` in Rust.

The viewport area (`image-container`) has no `TouchArea` and no mouse interaction. The only `TouchArea` in the entire UI is the panel splitter.

### Data Flow: Slider to GPU

1. User moves slider -> Slint updates property.
2. `sliders-changed()` callback fires -> Rust calls `request_redraw()`.
3. `BeforeRendering` callback reads `get_camera_longitude()`, `get_camera_latitude()`, `get_camera_zoom()`.
4. `build_frame_state()` creates a `FrameState` for dirty-checking. Camera values (`longitude`, `latitude`, `zoom`) are stored as raw `f32` (not quantized).
5. If state changed, `write_uniforms()` creates an `OrbitalCamera`, computes the MVP, writes it to the 96-byte uniform buffer.
6. The vertex shader applies `uniforms.mvp * vec4(position, 1.0)`. The MVP is the only camera-related data on the GPU.

### Wallpaper Export

`export_wallpaper_image()` reads `longitude`, `latitude`, and `zoom` from the stored `FrameState` (`res.last_state`) and calls the same `write_uniforms()` function with the monitor's aspect ratio. Any new camera parameters must be persisted in `FrameState` or the export will not match the preview.

### Uniform Buffer Layout (`renderer/uniforms.rs`)

```
Offset  Size  Field
0       64    mvp: mat4x4<f32>
64      12    sun_dir: vec3<f32>
76       4    terminator_width: f32
80       4    flags: u32
84       4    diffuse_floor: f32
88       4    diffuse_ramp: f32
92       4    _pad: f32
Total: 96 bytes
```

WGSL std140 alignment rules apply: `vec3<f32>` aligns to 16 bytes, total must be a multiple of 16. If new camera uniforms are needed on the GPU side, they must be added with correct alignment and both the Rust `Uniforms` struct and the WGSL struct must be updated in lockstep.

## Feature: Pan/Offset

### Concept

Shift the camera's look-at point away from the origin so the Earth appears off-center in the viewport. This enables wallpaper compositions where the Earth sits in one corner or along one edge.

### Implementation Approach

The current camera computes `look_at_rh(eye, center=ZERO, up=Y)`. Pan can be implemented by offsetting the center point:

- **Option A: World-space offset.** Add `target: Vec3` to `OrbitalCamera`. The view matrix becomes `look_at_rh(eye + target, target, up)`, keeping the camera at the same spherical position relative to the target. This shifts both the eye and the look-at point by the same offset, preserving the view direction. The offset is specified in world units.

- **Option B: Screen-space offset.** Apply a post-projection translation to the MVP matrix. This is simpler (just shift the NDC x/y coordinates) and more intuitive for composition (e.g., "shift the image 30% right"). It can be done entirely in the CPU MVP computation with no shader changes: multiply the MVP by a translation matrix `Mat4::from_translation(Vec3::new(offset_x, offset_y, 0.0))` applied in clip space.

**Recommendation:** Screen-space offset (Option B) is more natural for wallpaper composition. A world-space offset changes the camera-to-sphere distance at different offset values and interacts with the zoom level in complex ways. A screen-space offset leaves the zoom and framing intact and simply slides the rendered image within the viewport. Expose two parameters: `offset_x` and `offset_y`, both in the range `[-1.0, 1.0]` where 1.0 shifts the Earth by half the viewport width/height.

### UI

Two new sliders: "Pan X" and "Pan Y" (or "Offset X" / "Offset Y"), each ranging from -1.0 to 1.0 with default 0.0.

### Integration Points

- `OrbitalCamera`: Add `offset_x: f32` and `offset_y: f32` fields. Modify `mvp_matrix()` to apply a post-projection translation.
- `FrameState`: Add `offset_x: f32` and `offset_y: f32` for dirty-checking.
- `build_frame_state()`: Accept and store the new parameters.
- `write_uniforms()`: Pass the new parameters through to the camera constructor.
- `main.slint`: Add two sliders and two `out` (or `in-out`) properties.
- `export_wallpaper_image()`: Automatically covered if `FrameState` stores the values.
- No shader or uniform buffer changes needed -- the offset is baked into the MVP.

### Risks

- At extreme offset + close zoom, the Earth may be partially or fully outside the viewport. This is arguably the intended behavior (the user chose that composition), but it could be confusing. Consider documenting that offset works best at moderate zoom levels.
- The near/far clip planes (0.1 and 100.0) should remain adequate since offset does not change the camera-to-origin distance, only where the origin projects on screen.

## Feature: Tilt

### Concept

Rotate the camera around its own view axis (roll), so the horizon of the Earth appears tilted. This is purely compositional -- it does not change what part of the Earth is visible, only the angle of the image.

### Implementation Approach

The current camera uses `up = Vec3::Y` unconditionally. Tilt can be implemented by rotating the up vector around the view direction:

1. Compute the view direction: `forward = normalize(center - eye)`.
2. Rotate `Vec3::Y` around `forward` by the tilt angle: `up = Mat3::from_axis_angle(forward, tilt_rad) * Vec3::Y`.
3. Pass the rotated up vector to `look_at_rh`.

Alternatively, tilt can be applied as a post-view rotation: multiply the view matrix by a rotation around the Z axis (the camera's local forward axis in view space). This is equivalent and may be simpler to implement: `view_matrix = Mat4::from_rotation_z(tilt_rad) * base_view_matrix`.

**Recommendation:** Post-view Z rotation is simpler and avoids recomputing the up vector. Add a `tilt_deg: f32` field to `OrbitalCamera` and apply it as `Mat4::from_rotation_z(tilt_deg.to_radians())` multiplied after the view matrix.

### UI

One new slider: "Tilt", ranging from -180 to 180 degrees with default 0.

### Integration Points

- `OrbitalCamera`: Add `tilt_deg: f32`. Modify `view_matrix()` to apply post-rotation.
- `FrameState`: Add `tilt: f32`.
- `build_frame_state()`, `write_uniforms()`: Thread the new parameter through.
- `main.slint`: Add one slider and one property.
- No shader or uniform buffer changes -- tilt is baked into the MVP.

### Risks

- Gimbal lock: The existing latitude clamp at +/-89.9 degrees avoids the case where the eye aligns with the Y-up vector. Tilt does not introduce a new gimbal lock risk because it rotates the up vector itself, and `look_at_rh` receives an up vector that is always perpendicular to the view direction (by construction, since we rotate Y around the view direction). With the post-view rotation approach, the base view matrix is computed with the standard Y-up, so the existing clamp remains sufficient.
- Interaction with mouse drag: If mouse drag controls longitude/latitude, tilt changes the mapping between screen-space drag direction and longitude/latitude change. The drag math must account for the current tilt angle.

## Feature: Zoom Curve

### Concept

The current zoom slider maps linearly to camera distance, but perceptual zoom is non-linear. Moving from distance 2 to 4 is a dramatic change (Earth goes from filling the viewport to half-size), while moving from 40 to 50 is barely noticeable. A non-linear mapping makes the slider feel even across its range.

### Current Zoom Behavior

| Distance | Approx. Angular Size of Earth | Visual Description |
|----------|-------------------------------|-------------------|
| 2.0 | ~53 degrees | Earth overfills the 20-degree FOV |
| 5.0 | ~23 degrees | Earth fills most of the viewport |
| 8.0 (default) | ~14 degrees | Earth comfortably framed |
| 20.0 | ~5.7 degrees | Earth is small |
| 50.0 | ~2.3 degrees | Earth is tiny |

### Implementation Approach

Replace the direct distance mapping with a non-linear curve. The slider value `t` (0.0 to 1.0) maps to distance via:

- **Exponential:** `distance = d_min * (d_max / d_min)^t`. This produces even perceptual steps because the angular size of the sphere is roughly `1/distance`, so logarithmic spacing in distance gives linear spacing in perceived size.
- **Power curve:** `distance = d_min + (d_max - d_min) * t^gamma`. With `gamma > 1`, more of the slider range is allocated to close distances.

**Recommendation:** Exponential mapping. With `d_min = 1.5` and `d_max = 80.0`, the formula `distance = 1.5 * (80.0 / 1.5)^t` gives:
- `t = 0.0` -> distance 1.5 (very close, Earth overfills viewport)
- `t = 0.5` -> distance ~11 (Earth nicely framed)
- `t = 1.0` -> distance 80 (Earth very small)

This also naturally extends the zoom range beyond the current 2.0-50.0 limits.

### Extended Range Limits

The current near clip plane is 0.1. At distance 1.5 from the center of a unit sphere, the camera is 0.5 units from the surface, well within the near plane. At distance 1.2, the surface is 0.2 from the camera, still safe. Going below ~1.1 risks clipping the sphere. A minimum distance of 1.5 provides comfortable headroom.

The far clip plane at 100.0 supports distances up to ~100. A maximum distance of 80.0 stays well within the far plane.

### UI Change

The zoom slider should be relabeled to reflect that it no longer maps directly to distance. The slider value becomes a normalized 0-to-1 parameter, with the label showing the mapped distance or a descriptive "Zoom" percentage. Alternatively, the slider can remain labeled "Zoom" with min/max as abstract values, and the exponential mapping is applied internally.

### Integration Points

- `OrbitalCamera::new()` or a helper function: Apply the exponential mapping from slider value to distance.
- The mapping should be applied before constructing the camera, so the rest of the pipeline sees the mapped distance.
- `FrameState`: Store the raw slider value (not the mapped distance) for dirty-checking, since the slider value is what changes.
- `main.slint`: Change the zoom slider range to 0.0-1.0, or keep the current range and apply the mapping in Rust.
- `write_uniforms()`: No change needed -- it receives `distance` which will already be mapped.

### Risks

- Changing the slider range is a breaking change for any saved configurations or user muscle memory. Since the project is early-stage, this is acceptable.
- The wallpaper export reads `zoom` from `FrameState`. If `FrameState` stores the raw slider value, the export path must apply the same mapping. If it stores the mapped distance, no extra logic is needed but dirty-checking loses some precision at high zoom. **Recommendation:** Store the raw slider value in `FrameState` and apply the mapping in `write_uniforms()`.

## Feature: Mouse Controls

### Concept

Allow the user to rotate the globe by dragging on the viewport and zoom with the scroll wheel, providing direct manipulation instead of (or in addition to) slider interaction.

### Slint Input APIs

Slint's `TouchArea` element provides:

| Callback | Use Case |
|----------|----------|
| `moved()` | Mouse drag -- fires continuously while a button is held |
| `scroll-event(PointerScrollEvent) -> EventResult` | Scroll wheel -- `delta-y` for zoom |

Properties available during callbacks: `mouse-x`, `mouse-y`, `pressed-x`, `pressed-y` (all `length` type, relative to the `TouchArea`).

### Architecture Decision: Where to Compute Camera Updates

Three options were identified:

**Option A: Slint-side math, update slider values directly.** The `TouchArea.moved` callback computes longitude/latitude deltas using Slint arithmetic and writes them to the slider values. Pros: simple, no new Rust callbacks. Cons: couples mouse sensitivity to slider ranges; Slint arithmetic is limited (no trigonometry).

**Option B: Slint fires a callback, Rust computes new values.** The `TouchArea` fires `mouse-drag(dx: float, dy: float)` that Rust handles by computing new camera values and writing them back via `in-out` properties. Pros: full math capabilities in Rust. Cons: requires `in-out` properties and round-trip communication.

**Option C: Slint-side math with `in-out` camera properties.** The `TouchArea` directly modifies `in-out` camera properties. Sliders bind bidirectionally to these same properties, staying in sync. Pros: no Rust callback needed for basic drag. Cons: same limited Slint arithmetic.

**Recommendation:** Option C for basic drag rotation, with the camera properties changed from `out` to `in-out`. This is the simplest approach and sufficient for longitude/latitude rotation where the math is just addition. The scroll-wheel zoom can similarly update the camera-zoom property directly in Slint.

However, if tilt is implemented, mouse drag must account for the current tilt angle when converting screen-space dx/dy to longitude/latitude changes. This requires trigonometry that Slint cannot easily perform, which would favor Option B for the tilt-aware case. **Decision point:** If tilt is implemented before or alongside mouse controls, use Option B. If mouse controls ship first without tilt, Option C is simpler.

### Key Constraint: `out` to `in-out` Property Change

The current camera properties (`camera-longitude`, `camera-latitude`, `camera-zoom`) are `out` -- Rust can read but not write them. For mouse-driven updates to flow back to the sliders, these must become `in-out`:

```slint
// Before:
out property <float> camera-longitude: longitude-slider.value;
// After:
in-out property <float> camera-longitude: 0;
// With bidirectional binding on the slider:
longitude-slider := Slider { value <=> root.camera-longitude; }
```

This is a breaking change in the Slint property declarations but has no Rust API impact (the generated `get_`/`set_` methods exist for both `out` and `in-out`).

### Mouse Drag Rotation

In the `moved` callback, compute deltas from the last position and convert to longitude/latitude changes:

```
delta_longitude = -(mouse-x - pressed-x) * sensitivity / viewport-width * 360
delta_latitude  =  (mouse-y - pressed-y) * sensitivity / viewport-height * 180
```

The sensitivity factor controls how many degrees of rotation per pixel of drag. A value around 1.0-2.0 should feel natural for the default zoom level. Sensitivity should ideally scale with zoom (closer zoom = finer control), which can be done by dividing by the current zoom slider value.

**Important:** After applying the delta, `pressed-x` and `pressed-y` remain at the original press position, so subsequent `moved` events accumulate total drag distance. The implementation must either track the previous `mouse-x`/`mouse-y` (storing them in Slint properties) to compute incremental deltas, or compute cumulative deltas from the initial longitude/latitude at press time (storing those values at `pointer-event(down)`).

### Scroll Wheel Zoom

In the `scroll-event` callback, adjust the zoom property by `delta-y * scroll_sensitivity`. Return `EventResult.accept` to prevent scroll propagation.

### TouchArea Placement

A `TouchArea` must be added inside `image-container`, sized to fill it, overlaying the `Image` element. This is the standard Slint pattern for interactive image displays.

### Integration Points

- `main.slint`: Add `TouchArea` in `image-container`, change camera properties to `in-out`, add bidirectional slider bindings, implement `moved` and `scroll-event` handlers.
- `main.rs`: If using Option B, wire new `mouse-drag` callback. If using Option C, no new Rust callbacks needed -- the existing `sliders-changed` callback fires because the property changes trigger it.
- Camera/renderer: No changes beyond what the other features already require.

### Risks

- **Event ordering:** Slint fires `moved` on every mouse move while pressed. If the property update triggers `sliders-changed` which calls `request_redraw()`, there will be one redraw per mouse move event. This should be fine (Slint coalesces redraws), but worth verifying there is no stutter.
- **Scroll direction:** `delta-y` sign convention (scroll-up = positive or negative) varies by platform. Slint may normalize this, but it should be tested on Windows.
- **TouchArea consuming clicks:** A full-viewport `TouchArea` will consume all mouse events on the viewport. If future features need click events (e.g., clicking a location on the globe), the `TouchArea` must be designed to distinguish drag from click.

## Files Requiring Modification

| File | Pan/Offset | Tilt | Zoom Curve | Mouse | Notes |
|------|:---:|:---:|:---:|:---:|-------|
| `src/scene/camera.rs` | Yes | Yes | Yes | No | Add fields, modify matrix methods |
| `ui/main.slint` | Yes | Yes | Yes | Yes | New sliders, `in-out` properties, `TouchArea` |
| `src/renderer/frame.rs` | Yes | Yes | Yes | No | Add fields to `FrameState` and `build_frame_state()` |
| `src/renderer/render_pass.rs` | Yes | Yes | Yes | No | Extend `write_uniforms()` signature |
| `src/renderer/mod.rs` | Yes | Yes | Yes | No | Read new properties, pass to `build_frame_state` |
| `src/main.rs` | Maybe | Maybe | No | Maybe | Wire new callbacks if using Option B |

### Files NOT Requiring Modification

| File | Reason |
|------|--------|
| `shaders/sphere.wgsl` | All camera changes are absorbed by the MVP matrix |
| `shaders/blend.wgsl` | Fragment blending is camera-independent |
| `src/renderer/gpu_setup.rs` | Pipeline and buffer setup are camera-independent |
| `src/renderer/textures.rs` | Texture loading is camera-independent |
| `src/renderer/uniforms.rs` | The existing `mat4x4` MVP absorbs all camera transforms |

## Key Decisions

1. **Pan implementation: screen-space (recommended) vs. world-space.** Screen-space is simpler, more intuitive for composition, and does not interact with zoom level. World-space changes camera-to-sphere distance when panning, complicating the behavior.

2. **Tilt implementation: post-view Z rotation (recommended) vs. up-vector rotation.** Post-view rotation is simpler to implement and reason about. Both are mathematically equivalent.

3. **Zoom curve: exponential (recommended) vs. power.** Exponential gives perceptually uniform steps. The extended range (1.5 to 80) comfortably fits within the existing near/far clip planes.

4. **Mouse control architecture: Option C for initial implementation (recommended).** `in-out` properties with Slint-side drag math. Upgrade to Option B if tilt-aware drag is needed.

5. **Camera property direction: change from `out` to `in-out` (required for mouse controls).** This is a prerequisite for any mouse interaction that updates camera state.

6. **FrameState storage: raw slider values (recommended).** Store the zoom slider value (not mapped distance) so dirty-checking is precise. Apply the exponential mapping in `write_uniforms()`.

7. **Feature ordering.** Pan/offset, tilt, and zoom curve are independent of each other and can be implemented in any order. Mouse controls depend on the `out` to `in-out` property change and should be aware of tilt if tilt ships first. A natural order: (1) zoom curve, (2) pan/offset, (3) tilt, (4) mouse controls -- because the zoom curve has the smallest blast radius, and mouse controls benefit from having all other parameters settled.

## Technical Constraints

1. **Uniform buffer alignment.** If any new camera data needs to be sent to the GPU as a separate uniform (beyond the MVP), the Rust and WGSL structs must be extended with matching alignment. Currently, all four proposed features can be fully expressed through the MVP matrix, requiring no uniform buffer changes.

2. **Gimbal lock.** The latitude clamp at +/-89.9 degrees prevents NaN from `look_at_rh`. This clamp must remain. Tilt does not introduce a new gimbal lock vector because it operates on the up vector after the view direction is established.

3. **Near/far clip planes.** The current planes (0.1 and 100.0) support the extended zoom range of 1.5 to 80.0. Do not reduce the minimum distance below ~1.1 (sphere surface at 0.1 from camera hits near plane).

4. **`build_frame_state` already has many parameters.** Adding 3-4 more (offset_x, offset_y, tilt, zoom curve value) will push the argument count further. Consider grouping camera parameters into a struct passed to `build_frame_state` to improve ergonomics.

5. **Slint `length` vs `float` types.** `TouchArea` positions (`mouse-x`, `pressed-x`) are Slint `length` values (physical pixels scaled by DPI). Conversion to logical coordinates may be needed for consistent behavior across DPI settings. The existing viewport size properties already account for scale factor in the renderer.

6. **LF line endings.** All new and modified files must use LF line endings per project convention.

## Open Questions

1. **Pan range limits.** Should offset be clamped so the Earth always remains partially visible, or should the user be free to pan the Earth entirely off-screen? For wallpaper use, allowing full freedom seems appropriate, but an accidental full-offset could confuse users. Consider a "Reset Camera" button.

2. **Zoom slider UX.** After switching to exponential mapping, should the slider show the mapped distance as its value label, a "zoom percentage", or the raw slider position? Showing distance (e.g., "3.2x") may be most informative.

3. **Mouse drag sensitivity scaling.** Should drag sensitivity vary with zoom level? At close zoom, a small drag should produce less rotation; at far zoom, more. This makes direct manipulation feel consistent regardless of zoom. The scaling factor would be proportional to the current distance.

4. **Tilt range.** Full -180 to 180 range allows any orientation but may be excessive for typical use. A -45 to 45 range covers most practical compositions. This is a UX decision.

5. **Cumulative vs. incremental drag deltas.** Slint's `moved` callback provides absolute cursor position and the original press position, but not the previous frame's position. Incremental deltas require storing the last `mouse-x`/`mouse-y` in Slint properties. Cumulative deltas require storing the longitude/latitude at press time. Both work; cumulative is slightly simpler for Slint-only math.

6. **Reset mechanism.** With four new parameters, users may want a "Reset Camera" button to return to defaults. This is a small UX addition but worth planning for.

## Sources

| Document | Focus Area |
|----------|------------|
| `docs/plans/2026-03-16-camera-controls-codebase.md` | Camera system internals, uniform buffer layout, data flow from UI to GPU, wallpaper export path, extension points |
| `docs/plans/2026-03-16-camera-controls-ui.md` | Slint UI definition, property directions, input handling APIs, TouchArea capabilities, mouse interaction architecture options |
