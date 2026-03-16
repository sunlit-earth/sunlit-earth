# Research: Camera Controls UI & Input Handling (2026-03-16)

## Problem Statement

Sunlit Earth needs improved camera controls: mouse drag to rotate the globe, scroll wheel to zoom, and new pan/tilt sliders in the controls panel. This document researches the existing UI definition, input handling, Slint-Rust integration patterns, and Slint APIs relevant to implementing these features.

## 1. Slint UI Definition (`ui/main.slint`)

### Layout Structure

The UI is a single `MainWindow` component with a resizable two-pane layout:

- **Left pane** (controls panel): A `Rectangle` with `width: splitter.x`, containing a `VerticalLayout` of controls and a bottom-pinned renderer info label.
- **Splitter**: A custom `Splitter` component (4px wide, draggable) that separates the panes. It uses a `TouchArea` to handle horizontal drag, clamped between `160px` and `root.width - 200px`.
- **Right pane** (`image-container`): A `Rectangle` from the splitter to the right edge, containing an `Image` component that displays the rendered frame. A conditional loading overlay appears in the bottom-right corner when textures are loading.

### Existing Controls

All controls live inside the left pane's `VerticalLayout`:

| Control | Type | ID | Range/Values | Default | Callback |
|---------|------|----|-------------|---------|----------|
| Texture | ComboBox | `texture-combo` | Grid, Day, Night, Day/Night Blend | index 3 | `texture-changed()` |
| Anti-Aliasing | ComboBox | `aa-combo` | Dynamic from adapter | index from code | `msaa-changed()` |
| Longitude | Slider | `longitude-slider` | -180 to 180 | 0 | `sliders-changed()` |
| Latitude | Slider | `latitude-slider` | -89 to 89 | 30 | `sliders-changed()` |
| Zoom | Slider | `zoom-slider` | 2.0 to 50.0 | 8.0 | `sliders-changed()` |
| Terminator | Slider | `terminator-slider` | 0.01 to 0.3 | 0.1 | `sliders-changed()` |
| Diffuse shading | CheckBox | `diffuse-check` | on/off | checked | `sliders-changed()` |
| Floor | Slider | `diffuse-floor-slider` | 0.0 to 1.0 | 0.50 | `sliders-changed()` |
| Ramp | Slider | `diffuse-ramp-slider` | 0.1 to 1.0 | 0.25 | `sliders-changed()` |
| Set as Wallpaper | Button | (anonymous) | n/a | n/a | `set-wallpaper()` |

Each slider row follows a consistent pattern: `HorizontalLayout` containing a label `Text` (min-width 70px), the `Slider`, and a value display `Text` (min-width 35px). The value display uses Slint's `round()` for formatting.

### Properties Exposed to Rust

Properties are the bridge between the Slint UI and the Rust rendering backend. The direction matters:

**`in` properties** (Rust writes, Slint reads):
- `rendered-image: image` -- the GPU-rendered frame
- `renderer-info: string` -- adapter name shown at bottom of controls
- `aa-options: [string]` -- MSAA option labels
- `texture-options: [string]` -- texture mode labels
- `loading-text: string` -- loading indicator overlay text

**`out` properties** (Slint writes, Rust reads):
- `camera-longitude: float` -- bound to `longitude-slider.value`
- `camera-latitude: float` -- bound to `latitude-slider.value`
- `camera-zoom: float` -- bound to `zoom-slider.value`
- `viewport-width: length` -- bound to `image-container.width`
- `viewport-height: length` -- bound to `image-container.height`
- `terminator-width: float` -- bound to `terminator-slider.value`
- `diffuse-shading: bool` -- bound to `diffuse-check.checked`
- `diffuse-floor: float` -- bound to `diffuse-floor-slider.value`
- `diffuse-ramp: float` -- bound to `diffuse-ramp-slider.value`

**`in-out` properties** (bidirectional):
- `aa-index: int` -- current AA combobox index
- `texture-index: int` -- current texture combobox index
- `wallpaper-status: string` -- status message for wallpaper export

**Callbacks** (Slint invokes, Rust handles):
- `sliders-changed()` -- fired by any slider or checkbox change
- `msaa-changed()` -- fired by AA combobox selection
- `texture-changed()` -- fired by texture combobox selection
- `set-wallpaper()` -- fired by wallpaper button click

### Key Observation: Camera Properties Are `out` (Read-Only from Rust)

The camera properties (`camera-longitude`, `camera-latitude`, `camera-zoom`) are declared as `out property`, meaning Rust can read them via `win.get_camera_longitude()` but **cannot write them** via `set_camera_longitude()`. They are bound to slider values, so the only way to change them is through the slider UI.

For mouse-driven camera control, this must change. Either:
1. Change the camera properties to `in-out` so Rust can set them (and update the sliders accordingly via two-way binding), or
2. Add new `in-out` properties and have the sliders bind to them, or
3. Handle mouse input entirely in Slint and update the slider values from Slint code.

## 2. Main Entry Point (`src/main.rs`)

### Window Creation

The startup sequence in `main()` is:

1. Parse CLI (`Cli::parse()`)
2. Initialize wgpu (`wgpu_init::init()`)
3. Configure Slint backend (`BackendSelector::new().require_wgpu_28()`)
4. Create `MainWindow` (`MainWindow::new()`)
5. Set adapter info, AA options, texture options
6. Register JXL decoding hook
7. Wire up callbacks
8. Set up rendering notifier
9. Start the 2-minute sun timer
10. `window.run()` (enters Slint event loop)

### Callback Wiring Pattern

Every callback follows the same pattern:

```rust
let window_weak = window.as_weak();
window.on_sliders_changed(move || {
    if let Some(win) = window_weak.upgrade() {
        win.window().request_redraw();
    }
});
```

The pattern is:
1. Create a `slint::Weak<MainWindow>` (required because callbacks must be `'static`).
2. Register the callback via `window.on_<callback_name>(closure)`.
3. In the closure, upgrade the weak reference and call `win.window().request_redraw()` to trigger a new frame.

All three control callbacks (`sliders-changed`, `msaa-changed`, `texture-changed`) do exactly the same thing: request a redraw. The actual values are read from the window properties during the `BeforeRendering` callback in the renderer.

This means no immediate processing happens when a slider moves -- the value is stored in the Slint property and read lazily during the next render cycle.

### Rendering Notifier Setup

`renderer::setup_rendering_notifier(&window, aa_counts, texture_paths)` registers a closure via `window.window().set_rendering_notifier()`. This closure is called by Slint at three lifecycle points (`RenderingSetup`, `BeforeRendering`, `RenderingTeardown`). A `slint::Weak<MainWindow>` is captured into the closure.

### Sun Timer

A `slint::Timer` fires every 120 seconds in `Repeated` mode. Its callback also just calls `request_redraw()`. The timer is kept alive by storing it in a local variable that is explicitly dropped after `window.run()` returns.

## 3. Slint-Rust Integration Patterns

### Property Flow: UI to Renderer

The property flow during a render cycle is:

1. **User moves slider** -> Slint updates the slider's `value` property.
2. **Slider fires `changed` callback** -> invokes `sliders-changed()` callback in Rust -> calls `win.window().request_redraw()`.
3. **Slint calls `BeforeRendering`** -> `rendering_callback()` runs in `renderer/mod.rs`.
4. **Renderer reads properties**: `win.get_camera_longitude()`, `win.get_camera_latitude()`, `win.get_camera_zoom()`, `win.get_terminator_width()`, etc.
5. **Build `FrameState`** for dirty-checking (floats quantized to integer thousandths).
6. **Skip render if state unchanged** compared to `res.last_state`.
7. **Write uniforms** to GPU buffer: creates `OrbitalCamera`, computes MVP matrix, writes `Uniforms` struct.
8. **Execute render pass** and convert texture to `slint::Image`.
9. **Set image**: `win.set_rendered_image(image)`.

### Property Access API

The `slint::include_modules!()` macro in `lib.rs` generates the `MainWindow` struct with getter/setter methods. The naming convention converts kebab-case Slint properties to snake_case Rust methods:

- `camera-longitude` (Slint) -> `get_camera_longitude()` / `set_camera_longitude()` (Rust)
- `aa-index` (Slint) -> `get_aa_index()` / `set_aa_index()` (Rust)
- `rendered-image` (Slint) -> `set_rendered_image()` (Rust)

Note: Even for `out` properties, the Rust API generates both `get_` and `set_` methods, but using `set_` on an `out` property from Rust may not work correctly because Slint's binding system expects the value to flow from the `.slint` file outward. For bidirectional updates, properties should be `in-out`.

### ComboBox Callback Pattern (Model for New Controls)

The MSAA combobox demonstrates the full pattern for a Slint control that affects rendering:

**Slint side** (`main.slint`):
```slint
in property <[string]> aa-options: ["None", "MSAA 4x"];
in-out property <int> aa-index: 1;
callback msaa-changed();

aa-combo := ComboBox {
    model: aa-options;
    current-index <=> root.aa-index;
    selected => { root.msaa-changed(); }
}
```

**Rust side** (`main.rs`):
```rust
// Populate the model
window.set_aa_options(slint::ModelRc::new(aa_model));

// Wire up the callback
window.on_msaa_changed(move || {
    win.window().request_redraw();
});

// Deferred index setting (after Slint processes model changes)
slint::invoke_from_event_loop(move || {
    win.set_aa_index(aa_default);
});
```

**Renderer side** (`renderer/mod.rs`):
```rust
let desired = lookup_sample_count(&win, aa_counts);
if desired != res.sample_count {
    rebuild_msaa_resources(res, desired);
}
```

The notable detail is the deferred `set_aa_index()` call via `invoke_from_event_loop()`, which ensures the model is set before the index.

## 4. Input Handling

### Existing Mouse Input

The codebase has exactly **one** `TouchArea`: the `Splitter` component used for resizing the controls panel. It handles horizontal drag only:

```slint
touch := TouchArea {
    width: 10px;
    x: -3px;
    mouse-cursor: ew-resize;
    moved => {
        root.moved(self.mouse-x - self.pressed-x);
    }
}
```

There is **no mouse input on the image/viewport area** currently. The `image-container` Rectangle and the `Image` inside it have no `TouchArea` or interaction handlers.

### No Keyboard Input

There is no `FocusScope`, no key event handling, no keyboard shortcuts anywhere in the UI.

### Slint TouchArea API (for Mouse Drag and Scroll)

Slint's `TouchArea` element (documented in Slint 1.x) provides these relevant callbacks and properties:

**Callbacks:**
- `clicked()` -- single click completed
- `moved()` -- mouse moved while pressed (drag)
- `pointer-event(PointerEvent)` -- raw pointer events (down, up, move, cancel)
- `scroll-event(PointerScrollEvent) -> EventResult` -- scroll wheel events

**Properties (read in callbacks):**
- `mouse-x`, `mouse-y` -- current cursor position relative to the `TouchArea`
- `pressed-x`, `pressed-y` -- position where the press started (for drag delta computation)
- `pressed` -- whether a button is currently held
- `has-hover` -- whether the cursor is over the element
- `mouse-cursor` -- cursor style

**`PointerScrollEvent` struct:**
- `delta-x: length` -- horizontal scroll amount
- `delta-y: length` -- vertical scroll amount

**`scroll-event` return value:**
- Returns `EventResult.accept` to consume the event, or `EventResult.reject` to pass it through.

### Design Implications for Mouse Drag Rotation

To implement mouse drag rotation on the image area, a `TouchArea` must be overlaid on (or replace) the `image-container`. The drag deltas can be computed from `mouse-x - pressed-x` and `mouse-y - pressed-y` during the `moved` callback.

**Option A: Handle in Slint, update slider values.**
The `TouchArea.moved` callback computes longitude/latitude deltas and directly updates the slider values. This keeps the logic simple but couples the interaction to the slider ranges.

**Option B: Handle in Slint, fire a callback to Rust.**
The `TouchArea` fires a callback like `mouse-drag(dx: float, dy: float)` that Rust handles by computing new camera values. This requires `in-out` properties so Rust can write the new values back.

**Option C: Handle entirely in Slint with `in-out` camera properties.**
The `TouchArea` directly modifies `in-out` camera properties using Slint arithmetic. The sliders bind bidirectionally to these properties.

### Design Implications for Scroll Wheel Zoom

The `scroll-event` callback on a `TouchArea` receives `PointerScrollEvent.delta-y`, which can be mapped to zoom changes. The callback must return `EventResult.accept` to prevent the scroll from propagating (e.g., scrolling the controls panel).

### Important: TouchArea Layering

A `TouchArea` placed over the `Image` component will intercept all mouse events on the viewport. This is the standard Slint pattern for interactive image displays. The `TouchArea` should be a sibling of (or wrap) the `Image` inside `image-container`, sized to fill the container.

## 5. Camera System Details

### `OrbitalCamera` (scene/camera.rs)

The camera uses spherical coordinates:
- **longitude_deg** (-180 to 180): rotation around Y axis
- **latitude_deg** (-89.9 to 89.9): tilt up/down (clamped to avoid gimbal lock at poles)
- **distance**: distance from origin (the "zoom" slider value)
- **fov_deg**: fixed at 20.0 degrees (hardcoded in `new()`)

The camera always looks at the origin (`Vec3::ZERO`) with `Vec3::Y` as up.

The MVP matrix is computed as `projection * view` (no model transform, since the sphere is at the origin).

### Current Slider Ranges and Semantics

| Parameter | Slider Range | Default | Camera Field | Interpretation |
|-----------|-------------|---------|-------------|----------------|
| Longitude | -180 to 180 | 0 | `longitude_deg` | Degrees, directly passed |
| Latitude | -89 to 89 | 30 | `latitude_deg` | Degrees, clamped to +-89.9 in `OrbitalCamera::new()` |
| Zoom | 2.0 to 50.0 | 8.0 | `distance` | Distance in world units (1.0 = sphere surface) |

The zoom slider controls distance, not magnification. Lower values = closer to the sphere. With `fov_deg = 20.0`, a distance of 2.0 is very close (sphere fills most of the viewport), and 50.0 is very far away.

### Pan and Tilt Concepts

"Pan" and "tilt" could refer to:
1. **Pan = longitude rotation, Tilt = latitude rotation**: This is exactly what the current longitude/latitude sliders already do.
2. **Pan = horizontal offset of the look-at point, Tilt = vertical offset**: This would require changing the camera model to support an off-center look-at point.
3. **Pan = camera-relative horizontal rotation, Tilt = camera-relative vertical rotation**: Similar to (1) but in camera-local space rather than world space.

The current `OrbitalCamera` always looks at the origin. Adding true pan/tilt would require either shifting the look-at target or adding rotational offsets to the view matrix. This is a design decision, not a technical constraint.

## 6. Dirty-Checking Impact

The `FrameState` struct includes `longitude`, `latitude`, and `zoom` for dirty-checking. Any new camera parameters (pan, tilt) must also be added to `FrameState` and `build_frame_state()`, or the renderer will skip frames where only the new parameters changed.

Currently `FrameState` stores `longitude` and `latitude` as raw `f32` values (not quantized like the other parameters). This means any change to these values, no matter how small, triggers a re-render. The zoom value is also stored as raw `f32`.

## 7. Wallpaper Export Impact

The wallpaper export path (`export_wallpaper_image`) reads camera values from the stored `FrameState` (`res.last_state`):
```rust
state.longitude, state.latitude, state.zoom
```

Any new camera parameters must also be stored in `FrameState` (or `ShadingParams`) and propagated through the export path, or the exported wallpaper will not reflect the current camera position.

## 8. Summary of Key Integration Points

For implementing improved camera controls, the following files must be modified:

| File | Changes Needed |
|------|---------------|
| `ui/main.slint` | Add `TouchArea` on viewport, new `in-out` camera properties (or change `out` to `in-out`), new slider controls, scroll-event handler, new callbacks |
| `src/main.rs` | Wire new callbacks to `request_redraw()`, potentially handle mouse-drag callback with camera math |
| `src/scene/camera.rs` | Possibly extend `OrbitalCamera` if pan/tilt differs from longitude/latitude |
| `src/renderer/mod.rs` | Read new camera properties in `rendering_callback`, pass to `build_frame_state` |
| `src/renderer/frame.rs` | Add new camera fields to `FrameState` and `build_frame_state()` |
| `src/renderer/render_pass.rs` | Pass new camera params through `write_uniforms` if they affect the MVP matrix |

### No Changes Needed

| File | Reason |
|------|--------|
| `shaders/sphere.wgsl` | Camera changes flow through the MVP matrix uniform, no shader changes needed |
| `shaders/blend.wgsl` | Purely fragment-level blending, not affected by camera |
| `src/renderer/gpu_setup.rs` | Pipeline/buffer setup is camera-independent |
| `src/renderer/textures.rs` | Texture loading is camera-independent |
| `src/renderer/uniforms.rs` | MVP matrix is already a `mat4x4`, any camera change is absorbed by it |
