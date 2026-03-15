# Codebase Research: Day/Night Texture Blending

Goal: understand the current rendering pipeline in detail so we can plan how to add day/night texture blending driven by sun position.

## 1. Shader: `shaders/sphere.wgsl`

The shader is minimal. It receives exactly three bindings:

```wgsl
@group(0) @binding(0) var<uniform> uniforms: Uniforms;  // mat4x4<f32> mvp
@group(0) @binding(1) var sphere_texture: texture_2d<f32>;
@group(0) @binding(2) var sphere_sampler: sampler;
```

The vertex shader transforms position by MVP and passes UV through. The fragment shader samples a single texture:

```wgsl
let color = textureSample(sphere_texture, sphere_sampler, in.uv).rgb;
return vec4<f32>(color, 1.0);
```

There is no lighting, no second texture, no additional uniforms beyond the MVP matrix. The vertex output only carries `clip_position` and `uv` -- no world-space position or normal.

### What needs to change for day/night blending

The shader must:
- Receive a second texture (night) alongside the existing day texture
- Receive the sun direction as a uniform (vec3, world-space)
- Receive the vertex position in world space (or equivalently, the surface normal, since the sphere is unit-radius centered at origin so `normal == position`)
- Compute `dot(normal, sun_dir)` per fragment to determine day/night blend factor
- Apply a smooth transition (smoothstep) across the terminator
- Sample both textures and lerp between them based on the blend factor

The vertex shader must pass the world-space position (or normal) to the fragment shader as an additional varying.

## 2. Uniform Buffer and Bind Group Layout

The uniform buffer is 64 bytes -- exactly one `mat4x4<f32>` for the MVP matrix. Created at `renderer.rs:471`:

```rust
let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
    label: Some("mvp_uniform"),
    size: 64,
    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    ..
});
```

Written each frame at `renderer.rs:264`:

```rust
res.queue.write_buffer(&res.uniform_buffer, 0, bytemuck::cast_slice(mvp.as_ref()));
```

The bind group layout has three entries (`renderer.rs:490-520`):
- Binding 0: Uniform buffer (VERTEX stage only)
- Binding 1: Texture 2D (FRAGMENT stage only)
- Binding 2: Sampler (FRAGMENT stage only)

Each texture slot gets its own bind group containing the same uniform buffer, that slot's texture view, and the shared sampler.

### What needs to change

- The uniform buffer must grow to hold additional data: at minimum a `vec3<f32>` for the sun direction. With std140 padding, adding a vec3 after the mat4x4 would expand the buffer from 64 to 80 bytes (vec3 padded to 16 bytes in std140). Could also add a `vec4<f32>` for the sun direction (w unused or used for terminator width) for simpler alignment.
- Binding 0 visibility must include FRAGMENT (currently VERTEX-only), since the fragment shader needs the sun direction.
- A second texture binding (binding 3) is needed for the night texture, plus possibly a second sampler (binding 4) or reuse the existing sampler.
- The bind group layout must be extended with these new entries.

**Alternative design:** Instead of putting both textures in one bind group, we could use two bind groups (group 0 for uniforms, group 1 for day texture, group 2 for night texture). But the simplest change is to extend the single bind group with an additional texture view.

## 3. TextureSlot System and Texture Management

Textures are managed via a `Vec<TextureSlot>` inside `GpuResources` (`renderer.rs:51-58`):

```rust
struct TextureSlot {
    bind_group: Option<wgpu::BindGroup>,
    source_path: Option<PathBuf>,
    loading: bool,
}
```

Slot 0 is always the procedural grid texture (loaded synchronously at setup). Slots 1+ correspond to file-based textures (day, night) loaded lazily on background threads.

Currently, `main.rs` builds the slot list:
- Slot 0: Grid (procedural, always loaded)
- Slot 1: Day texture (`world.topo.200405.jxl`)
- Slot 2: Night texture (`BlackMarble_2016.jxl`)

Each slot has its own `wgpu::BindGroup` containing the uniform buffer + that slot's texture view + sampler. The texture combobox selects which single slot to render with. Only one texture is active at a time.

### How lazy loading works

1. `maybe_spawn_texture_load()` checks if the selected slot needs loading (no bind group, not already loading, has a source path).
2. Spawns a `std::thread` that calls `texture_loader::load()`, sends the decoded pixels back via `mpsc::channel`.
3. `process_decoded_textures()` drains the channel in `BeforeRendering`, creates the GPU texture with mipmaps, and builds the bind group.
4. The background thread also calls `window_weak.upgrade_in_event_loop()` to wake the UI thread for a redraw.

### What needs to change

For day/night blending, both the day and night textures must be loaded simultaneously. The current system loads on-demand when a slot is selected -- but blending requires both textures to be GPU-resident at the same time.

Options:
- **Eagerly load both day and night slots** when either is needed for blending mode
- **Create a new composite bind group** that contains both texture views (day + night) plus the expanded uniform buffer
- The per-slot bind groups can remain for the non-blended "preview" mode (Grid, Day-only, Night-only in the combobox)

The `resolve_render_index()` logic currently picks a single slot. For blending, we need a different code path that doesn't pick one slot but instead uses a combined bind group with both textures.

## 4. Texture Combobox and Switching

In `main.rs:64-65`, three labels are set: `["Grid", "Day", "Night"]`. The `texture_index` property (0, 1, or 2) is read each frame in `rendering_callback` at `renderer.rs:219`:

```rust
texture_index: win.get_texture_index(),
```

This index is used to select which `TextureSlot` to render with. The `on_texture_changed` callback triggers a redraw.

### What needs to change

A fourth option could be added: `"Day/Night Blend"` (or the existing Day/Night options could be replaced by the blend mode). Alternatively, blending could be always-on when both textures are available, with the combobox controlling which individual texture to preview.

## 5. Sphere Mesh and Vertex Data

The sphere is a 64x64 UV sphere generated by `sphere::generate_uv_sphere()`. Vertices have:
- `position: [f32; 3]` -- point on unit sphere
- `uv: [f32; 2]` -- equirectangular mapping

The vertex buffer layout (`sphere.rs:13-33`) defines two attributes at locations 0 and 1, with stride 20 bytes (5 floats).

Since the sphere is a unit sphere centered at origin, the vertex position IS the surface normal. The fragment shader can use the interpolated world-space position directly as the normal for lighting calculations. No additional vertex attribute is needed -- we just need to pass the position through to the fragment shader as a varying.

## 6. Camera and MVP Matrix

`camera.rs` defines `OrbitalCamera` with longitude, latitude, distance, and 20-degree FOV. The MVP matrix is `projection * view` (no model matrix -- identity). The camera looks at the origin.

The MVP is the only data currently sent to the GPU. For day/night blending, we also need to send the sun direction vector. This could be computed on the CPU from the current date/time using an astronomy library, or from a UI control for debugging.

## 7. Dirty-Checking and Re-rendering

`FrameState` (`renderer.rs:98-106`) captures all inputs that affect the rendered image:

```rust
struct FrameState {
    longitude: f32,
    latitude: f32,
    zoom: f32,
    sample_count: u32,
    texture_index: i32,
    width: u32,
    height: u32,
}
```

If `FrameState` hasn't changed and no texture was just decoded, the render pass is skipped entirely. This is efficient but means the scene only updates when the user interacts.

### What needs to change

For time-based sun position updates, the sun direction (or a timestamp) must be part of `FrameState` so that changes in sun position trigger re-renders. A periodic timer would need to request redraws (e.g., every few minutes).

## 8. MSAA and Render Pipeline

MSAA is fully dynamic. The pipeline is recreated when sample count changes (`rebuild_msaa_resources`). Render textures are also recreated when viewport size changes. The pipeline uses:
- `Rgba8Unorm` color format
- `Depth32Float` depth format
- Back-face culling (CCW front face)
- `TriangleList` topology

This is all independent of the texture blending feature and should not need changes.

## 9. Render-to-Texture Flow

The render output goes to an offscreen `wgpu::Texture` (not a swapchain). After the render pass, `Image::try_from(texture)` converts it to a Slint image displayed in the UI. This decoupled design means the rendering changes are entirely within the wgpu render pass -- Slint integration is unaffected.

## Summary of Changes Required

### Shader (`sphere.wgsl`)
- Add `position_ws: vec3<f32>` to `VertexOutput` (world-space position as normal)
- Pass vertex position through from vertex shader
- Add second texture binding (`night_texture`)
- Expand `Uniforms` struct: add `sun_dir: vec3<f32>` (and padding or terminator width)
- Fragment shader: sample both textures, compute `dot(normal, sun_dir)`, smoothstep blend

### Bind Group Layout (`renderer.rs`)
- Add binding 3: second texture (FRAGMENT, texture_2d)
- Possibly add binding 4: second sampler (or reuse binding 2)
- Change binding 0 visibility to VERTEX | FRAGMENT
- Expand uniform buffer from 64 to 80+ bytes

### Uniform Buffer (`renderer.rs`)
- Create a new uniform struct (e.g., `#[repr(C)] struct Uniforms { mvp: Mat4, sun_dir: Vec4 }`)
- Write sun direction alongside MVP each frame

### Texture Management (`renderer.rs`)
- Ensure both day and night textures are loaded when blend mode is active
- Create a composite bind group containing both texture views for the blend render path
- Keep individual bind groups for single-texture modes (grid, day-only, night-only)

### Sun Position (new module)
- Compute sun direction from current date/time
- Either integrate an astronomy library or use a simplified formula
- Pass the direction vector to the renderer each frame

### Dirty-Checking (`renderer.rs`)
- Add sun direction (or timestamp) to `FrameState`
- Add a periodic timer to trigger redraws as the sun moves

### UI (`main.slint`)
- Add a blend mode option (or make it the default)
- Optionally add a manual sun-position override for debugging

## Key Files

| File | Role |
|---|---|
| `shaders/sphere.wgsl` | Vertex/fragment shader -- needs second texture + sun uniform |
| `src/renderer.rs` | GPU resources, bind groups, render loop, dirty-checking |
| `src/main.rs` | Window setup, texture path resolution, UI callbacks |
| `src/camera.rs` | Orbital camera, MVP computation |
| `src/sphere.rs` | Vertex format, mesh generation |
| `src/texture_loader.rs` | Image loading/decoding (unchanged for this feature) |
| `ui/main.slint` | UI layout and properties |
