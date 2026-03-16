# Codebase Research: Camera and Rendering Systems

Goal: understand the current camera system, how it feeds into the rendering pipeline, and how the wallpaper export path uses camera parameters, in order to plan improved camera controls (pan/offset, tilt, zoom curve, mouse interaction).

## 1. Camera System (`src/scene/camera.rs`)

### Structure

The camera is a simple orbital camera defined by three parameters:

```rust
pub struct OrbitalCamera {
    pub longitude_deg: f32,   // -180 to 180
    pub latitude_deg: f32,    // -90 to 90 (clamped to -89.9..89.9)
    pub distance: f32,        // distance from the origin
    pub fov_deg: f32,         // vertical FOV, hardcoded to 20.0
}
```

The constructor `new(longitude_deg, latitude_deg, distance)` hardcodes `fov_deg: 20.0` and clamps latitude to `[-89.9, 89.9]` to avoid gimbal lock (at +/-90 degrees, the eye aligns with the Y-up vector, making `look_at_rh` produce NaN).

### Eye Position Computation

`eye_position()` converts spherical coordinates to Cartesian:

```rust
let x = distance * lat.cos() * lon.sin();
let y = distance * lat.sin();
let z = distance * lat.cos() * lon.cos();
```

This places the camera on a sphere of radius `distance` centered at the origin. At `(lon=0, lat=0)`, the camera is on the `+Z` axis. Longitude rotates around Y; latitude tilts up/down.

### Matrix Pipeline

- **View matrix**: `Mat4::look_at_rh(eye, center=ZERO, up=Y)` -- always looks at the origin, always uses Y-up.
- **Projection matrix**: `Mat4::perspective_rh(fov_deg.to_radians(), aspect_ratio, 0.1, 100.0)` -- near/far planes at 0.1 and 100.0.
- **MVP matrix**: `projection * view` (no model matrix -- the sphere sits at the origin with identity model transform).

### What the camera does NOT support

- **Pan/offset**: The camera always looks at the exact origin. There is no target offset or screen-space panning.
- **Tilt/roll**: The up vector is always `Vec3::Y`. There is no roll angle.
- **Zoom curve**: The "zoom" slider directly becomes `distance` (the third parameter to `OrbitalCamera::new`). The relationship between slider value and visual zoom is linear in distance, not perceptually linear.
- **Mouse interaction**: There is no mouse input handling anywhere in the camera code. All input comes from Slint sliders.

### Coordinate Frame

The camera and sun share the same coordinate convention:
- `+Z` = prime meridian at the equator (0N, 0E)
- `+Y` = north pole
- `+X` = 90 degrees East (0N, 90E)

This is confirmed by `sun.rs` which documents the same frame and constructs its direction vector identically: `x = cos(lat) * sin(lon)`, `y = sin(lat)`, `z = cos(lat) * cos(lon)`.

## 2. UI Slider Configuration (`ui/main.slint`)

The Slint UI exposes these camera-related sliders:

| Slider | Property | Min | Max | Default | Notes |
|--------|----------|-----|-----|---------|-------|
| Longitude | `camera-longitude` | -180 | 180 | 0 | Degrees |
| Latitude | `camera-latitude` | -89 | 89 | 30 | Degrees |
| Zoom | `camera-zoom` | 2.0 | 50.0 | 8.0 | Direct distance value |

The `camera-longitude`, `camera-latitude`, and `camera-zoom` properties are declared as `out property <float>` tied to their respective slider values. The `sliders-changed` callback fires when any slider changes, triggering `window.request_redraw()` in `main.rs`.

The zoom slider maps directly to distance. At `distance=2.0` (minimum), the camera is very close to the unit sphere (near-clipping at 0.1 from the sphere surface). At `distance=50.0` (maximum), the Earth is very small. With `fov_deg=20.0`, the angular size of the unit sphere at distance `d` is approximately `2 * atan(1/d)`, so at `d=2` it subtends ~53 degrees (overfilling the 20-degree FOV) and at `d=50` it subtends ~2.3 degrees (tiny).

The viewport size is exported as `viewport-width` and `viewport-height` (Slint `length` type), which the renderer reads and scales by `window.scale_factor()` for DPI awareness.

## 3. Data Flow: UI to GPU

### Step 1: Rendering Callback (`renderer/mod.rs`)

In `rendering_callback` for `BeforeRendering`, the renderer reads values directly from the Slint window properties:

```rust
let current_state = build_frame_state(
    win.get_camera_longitude(),   // f32, from slider
    win.get_camera_latitude(),    // f32, from slider
    win.get_camera_zoom(),        // f32, from slider
    res.sample_count,
    win.get_texture_index(),
    res.render_width,
    res.render_height,
    sun_dir,
    terminator_width_f,
    diffuse_shading,
    diffuse_floor_f,
    diffuse_ramp_f,
);
```

### Step 2: Dirty-Checking (`renderer/frame.rs`)

`FrameState` stores `longitude`, `latitude`, and `zoom` as raw `f32` values (not quantized for these fields -- only sun direction and shading params are quantized to integer thousandths). A render is skipped when `last_state == current_state` and no new textures arrived.

```rust
pub(crate) struct FrameState {
    pub longitude: f32,
    pub latitude: f32,
    pub zoom: f32,
    pub sample_count: u32,
    pub texture_index: i32,
    pub width: u32,
    pub height: u32,
    pub sun_direction: [i32; 3],    // quantized
    pub terminator_width: i32,      // quantized
    pub diffuse_shading: bool,
    pub diffuse_floor: i32,         // quantized
    pub diffuse_ramp: i32,          // quantized
}
```

Note: `longitude`, `latitude`, and `zoom` use `f32` equality via the derived `PartialEq`. This works because the values come directly from Slint sliders which produce deterministic float values. Adding new camera parameters (pan offset, tilt) would require adding them to `FrameState` for correct dirty-checking.

### Step 3: Uniform Buffer Write (`renderer/render_pass.rs`)

`write_uniforms()` creates an `OrbitalCamera` and computes the MVP:

```rust
let camera = OrbitalCamera::new(longitude, latitude, zoom);
let mvp = camera.mvp_matrix(aspect);
let uniforms = Uniforms {
    mvp: mvp.to_cols_array(),
    sun_dir: shading.sun_dir.into(),
    terminator_width: ...,
    flags: ...,
    diffuse_floor: ...,
    diffuse_ramp: ...,
    _pad: 0.0,
};
queue.write_buffer(uniform_buffer, 0, bytemuck::cast_slice(&[uniforms]));
```

This is the single point where camera parameters become a GPU matrix. Both the preview render pass (`execute_render_pass`) and the export path (`export_wallpaper_image`) call `write_uniforms()` with the same signature.

### Step 4: Shader Consumption (`shaders/sphere.wgsl`)

The vertex shader applies the MVP to each vertex position:

```wgsl
out.clip_position = uniforms.mvp * vec4<f32>(in.position, 1.0);
out.world_normal = in.position;  // unit sphere: position IS the normal
```

The MVP is the only camera-related uniform. There is no separate view or projection matrix on the GPU side. The normal is passed through in world space for lighting calculations.

## 4. Uniform Buffer Layout (`renderer/uniforms.rs`)

```rust
#[repr(C)]
pub(crate) struct Uniforms {
    pub mvp: [f32; 16],           // 64 bytes, offset 0
    pub sun_dir: [f32; 3],        // 12 bytes, offset 64
    pub terminator_width: f32,    // 4 bytes, offset 76
    pub flags: u32,               // 4 bytes, offset 80
    pub diffuse_floor: f32,       // 4 bytes, offset 84
    pub diffuse_ramp: f32,        // 4 bytes, offset 88
    pub _pad: f32,                // 4 bytes, offset 92
}
// Total: 96 bytes (compile-time assertion)
```

The WGSL side mirrors this exactly:

```wgsl
struct Uniforms {
    mvp: mat4x4<f32>,          // 64 bytes, offset 0
    sun_dir: vec3<f32>,        // 12 bytes, offset 64
    terminator_width: f32,     // 4 bytes, offset 76
    flags: u32,                // 4 bytes, offset 80
    diffuse_floor: f32,        // 4 bytes, offset 84
    diffuse_ramp: f32,         // 4 bytes, offset 88
    _pad: f32,                 // 4 bytes, offset 92
};
```

The `_pad` field is needed because WGSL uniform buffers must have sizes that are multiples of 16 bytes (std140 layout). The `sun_dir` vec3 naturally packs with `terminator_width` into a 16-byte slot because `vec3<f32>` in a uniform struct in WGSL has 16-byte alignment, and the next field fills the remaining 4 bytes.

### Alignment Constraints for Changes

Any new uniform fields must respect WGSL std140 alignment:
- `f32` fields align to 4 bytes
- `vec2<f32>` aligns to 8 bytes
- `vec3<f32>` aligns to 16 bytes (wastes 4 bytes unless paired with an f32)
- `vec4<f32>` and `mat4x4<f32>` align to 16 bytes
- Total struct size must be a multiple of 16

If new camera-related uniforms are needed on the GPU side (e.g., a separate view matrix for offset computation, or a screen-space offset vector), they must be added with correct alignment and the Rust `Uniforms` struct updated to match.

## 5. Wallpaper Export Path (`renderer/mod.rs` -- `export_wallpaper_image`)

The export path reuses the same camera parameters as the last preview frame:

```rust
let state = res.last_state.as_ref().ok_or("No frame rendered yet")?;
let shading = res.last_shading.as_ref().ok_or("No frame rendered yet")?;

// ...creates temporary textures at target resolution...

let aspect = target_width as f32 / target_height as f32;
render_pass::write_uniforms(
    &res.queue,
    &res.uniform_buffer,
    state.longitude,
    state.latitude,
    state.zoom,
    aspect,
    shading,
);
```

Key observations:
- It reads `longitude`, `latitude`, and `zoom` from the stored `FrameState` (from the last rendered preview frame).
- The aspect ratio is recalculated from the export target dimensions (monitor resolution), which may differ from the preview window's aspect ratio.
- The same `write_uniforms` function is called, so the camera transform is identical except for the different aspect ratio.
- Any new camera parameters must be stored in `FrameState` (or `last_shading`, or a new analogous field on `GpuResources`) so the export path can reproduce the scene.

## 6. Sphere Geometry (`src/geometry/sphere.rs`)

The sphere is generated as a 64x64 UV sphere (parametric mesh) with:
- Vertex format: `position: [f32; 3]` + `uv: [f32; 2]` = 20 bytes per vertex
- Positions lie on the unit sphere (radius 1.0)
- UV coordinates: `u = j/sectors` (longitude), `v = i/stacks` (latitude)
- Triangle winding: CCW from outside (front face)
- The pipeline uses back-face culling (`cull_mode: Some(Face::Back)`)

The sphere has identity model transform. Because positions are on the unit sphere, `position == surface_normal`, which the shader exploits directly.

## 7. Render Pipeline Configuration (`renderer/gpu_setup.rs`)

Pipeline settings relevant to camera work:
- Primitive topology: `TriangleList`, front face `Ccw`, cull mode `Back`
- Depth: `Depth32Float`, write enabled, compare `Less`
- Near/far clip planes: 0.1 and 100.0 (set in `OrbitalCamera::projection_matrix`)
- Render texture usage: `RENDER_ATTACHMENT | TEXTURE_BINDING` for preview, `RENDER_ATTACHMENT | COPY_SRC` for export
- Clear color: `(0.02, 0.02, 0.05, 1.0)` -- dark blue/black background

## 8. GpuResources Struct

The `GpuResources` struct stores all GPU state. Camera-relevant fields:

```rust
struct GpuResources {
    // ...pipeline, buffers, textures...
    uniform_buffer: wgpu::Buffer,       // holds the Uniforms (including MVP)
    render_width: u32,                  // current quantized viewport width
    render_height: u32,                 // current quantized viewport height
    last_state: Option<FrameState>,     // last rendered frame (for dirty-check + export)
    last_shading: Option<ShadingParams>,// shading params (for export)
    // ...
}
```

The uniform buffer is allocated once at setup (96 bytes) and rewritten each frame via `queue.write_buffer`. It does not need to be recreated when camera parameters change.

## 9. Constants and Limits

| Constant | Value | Location | Purpose |
|----------|-------|----------|---------|
| `SIZE_GRANULARITY` | 64 | `renderer/mod.rs` | Render texture quantization step |
| `DEFAULT_WIDTH` | 800 | `renderer/mod.rs` | Fallback viewport width |
| `DEFAULT_HEIGHT` | 600 | `renderer/mod.rs` | Fallback viewport height |
| Latitude clamp | +/-89.9 | `camera.rs` | Prevents gimbal lock |
| FOV | 20.0 degrees | `camera.rs` | Fixed vertical field of view |
| Near plane | 0.1 | `camera.rs` | Perspective frustum near |
| Far plane | 100.0 | `camera.rs` | Perspective frustum far |
| Zoom slider min | 2.0 | `main.slint` | Minimum distance |
| Zoom slider max | 50.0 | `main.slint` | Maximum distance |
| Zoom slider default | 8.0 | `main.slint` | Default distance |
| Longitude slider | -180..180 | `main.slint` | Full range |
| Latitude slider | -89..89 | `main.slint` | Integer bounds (camera clamps to +/-89.9) |

## 10. Summary of Extension Points

To implement improved camera controls, modifications will be needed at each layer:

1. **`OrbitalCamera`**: Add fields for pan offset (target point), tilt/roll angle, and possibly a non-linear zoom function. The `eye_position()`, `view_matrix()`, and `projection_matrix()` methods are the core transform logic.

2. **`main.slint`**: Add new UI controls (sliders or other widgets) and export their values as properties. New callbacks or extensions to `sliders-changed` will be needed.

3. **`FrameState`**: Add new camera parameters so dirty-checking detects changes. Decide whether to use raw f32 or quantized values.

4. **`write_uniforms()` in `render_pass.rs`**: The function signature `(longitude, latitude, zoom, aspect, shading)` is the funnel point. New camera parameters either flow through this function (if they affect the MVP on the CPU side) or become new GPU uniforms (if the shader needs them).

5. **`Uniforms` struct + WGSL**: Only needs changes if the shader needs new data beyond what the MVP matrix encodes. A screen-space pan offset, for instance, could either be baked into the MVP on the CPU or sent as a separate uniform.

6. **`export_wallpaper_image()`**: Must store and replay any new camera parameters from the last preview frame. Currently reads from `FrameState` which already stores `longitude`, `latitude`, `zoom`.

7. **Mouse interaction**: Currently no mouse handling exists in the codebase. Slint provides `TouchArea` elements that can capture mouse events. The rendering callback runs on the UI thread, so mouse state could be read from Slint properties just like sliders are today.
