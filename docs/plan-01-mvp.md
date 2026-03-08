# Plan 01 — MVP: Spinning Globe with Camera Controls

**Goal:** Prove the core building blocks work together — wgpu rendering a 3D sphere, embedded in a Slint window, with interactive camera controls. No wallpaper setting, no astronomy, no textures from NASA. Just a grid-textured sphere you can look at from any angle.

## What the MVP does

A window opens showing a sphere rendered with wgpu. The sphere has a simple procedural grid/wireframe texture (latitude/longitude lines) so you can see it rotating and verify the camera is working. Below or beside the rendered image, Slint UI controls let you:

- **Longitude** slider — orbit the camera east/west around the sphere
- **Latitude** slider — orbit the camera north/south
- **Zoom** slider — move the camera closer/farther

The rendered image updates live as you drag the sliders.

## Architecture

```
┌─────────────────────────────────────┐
│  Slint Window                       │
│  ┌───────────────────────────────┐  │
│  │                               │  │
│  │   Image component             │  │
│  │   (displays wgpu texture)     │  │
│  │                               │  │
│  └───────────────────────────────┘  │
│  Longitude: [━━━━━━●━━━━━━━━━━━━]   │
│  Latitude:  [━━━━━━━━━━━━●━━━━━━]   │
│  Zoom:      [━━━●━━━━━━━━━━━━━━━]   │
└─────────────────────────────────────┘
```

**Rendering approach:** Render the sphere to an offscreen wgpu texture, then pass it to Slint as an `Image` via `Image::try_from(wgpu::Texture)`. Slint's wgpu backend (feature `unstable-wgpu-28`) provides rendering callbacks with access to the wgpu `Device` and `Queue`. In the `BeforeRendering` callback, we render the sphere to our own texture, convert it to a Slint `Image`, and set it on an `Image` component in the UI. The texture stays on the GPU — no CPU pixel readback — so slider interaction is smooth and immediate.

### Why render-to-texture instead of underlay?

Slint's wgpu integration (`GraphicsAPI::WGPU28`) exposes `device`, `queue`, and `instance`, but **not** the window surface or current `TextureView`. Without access to the surface, we cannot render directly to the window as an underlay (unlike Slint's OpenGL path, where the default framebuffer is implicitly available). The render-to-texture approach is the officially supported pattern — it's what Slint's own `wgpu_texture` example uses. Performance is equivalent since the texture stays on the GPU and Slint composites it without readback. As a bonus, this approach also means the wallpaper export path (rendering to a texture for PNG save) shares the same code — no separate offscreen path needed later.

## Steps

### Step 1: Project setup and dependencies

Add dependencies to `Cargo.toml`:

| Crate | Purpose |
|---|---|
| `slint` (with feature `unstable-wgpu-28`) | GUI framework + wgpu integration |
| `wgpu` | GPU rendering (version must match Slint's wgpu — currently wgpu 28.x) |
| `glam` | Math (vectors, matrices) — lightweight, widely used with wgpu |
| `bytemuck` | Safe casting for GPU buffer data |

Pin Slint with a tilde requirement (`slint = { version = "~1.15", ... }`) because the `unstable-wgpu-28` feature is not covered by Slint's semver stability guarantees — it can change on any minor release when wgpu bumps its major version.

Set up the Slint build script (`build.rs`) to compile `.slint` files.

**Deliverable:** `cargo build` succeeds, empty Slint window opens.

### Step 2: Slint UI layout

Create `ui/main.slint` with:
- An `Image` component that fills the viewport area (Rust code sets its source from the wgpu-rendered texture)
- Three `Slider` controls: longitude (-180 to 180), latitude (-90 to 90), zoom (sensible range)
- Labels for each slider showing the current value
- A renderer info line showing GPU name, backend, and device type (e.g. "NVIDIA GeForce RTX 4070 (Dx12, DiscreteGpu)" or "Microsoft Basic Render Driver (Dx12, Cpu)")
- Properties for slider values that Rust code can read, an `image` property for the rendered texture, plus a string property for the renderer info

**Deliverable:** Window opens with working sliders and an Image component ready to receive the rendered texture.

### Step 3: wgpu initialization via Slint's rendering notifier

First, ensure Slint uses the wgpu backend by calling `slint::BackendSelector::new().require_wgpu_28(WGPUConfiguration::Automatic(...)).select()` before creating the window. Without this, Slint may choose a different renderer (e.g. FemtoVG) and the wgpu integration won't activate.

Register a `set_rendering_notifier()` callback on the Slint window. This callback fires at different rendering stages:

- **`RenderingSetup`** — wgpu is initialized. We receive `GraphicsAPI::WGPU28 { device, queue, instance }`. Use this to create our GPU resources (buffers, pipeline, depth texture, offscreen render texture). Read `AdapterInfo` via `device.adapter_info()` for the renderer info display (`name`, `backend`, `device_type` — `DeviceType::Cpu` means software rendering).
- **`BeforeRendering`** — called every frame, before Slint draws its UI. We render the sphere to our offscreen texture, convert it to a Slint `Image` via `Image::try_from(texture)`, and set it on the UI's `Image` component.
- **`AfterRendering`** — called after Slint draws its UI. Not used in the MVP, but the match arm must be present.
- **`RenderingTeardown`** — clean up GPU resources.

Note: `GraphicsAPI` and `RenderingState` are both `#[non_exhaustive]`, so match arms must include a wildcard `_ => {}` fallback.

Since Slint owns the wgpu instance, we share its `Device` and `Queue` rather than creating our own. No separate adapter request needed — we use whatever Slint selected (including its fallback behavior).

**Deliverable:** Rendering notifier registered, `RenderingSetup` fires and we can access wgpu resources. Renderer info displayed in UI.

### Step 4: Sphere geometry

Generate a UV sphere mesh (vertices + indices):
- Parametric sphere with configurable stacks/sectors (e.g. 64x64)
- Each vertex has: position (vec3), normal (vec3), UV coordinates (vec2)
- Store in a vertex buffer + index buffer on the GPU

Use `glam` for the math, `bytemuck` to cast vertex data to bytes for the GPU buffer.

**Deliverable:** Sphere geometry uploaded to GPU buffers.

### Step 5: Shader and render pipeline

Write a WGSL shader (`sphere.wgsl`) that:
- **Vertex shader:** Transforms vertices by a model-view-projection matrix
- **Fragment shader:** Generates a procedural grid pattern from UV coordinates (longitude/latitude lines)

Create the wgpu render pipeline:
- Vertex buffer layout matching our vertex struct
- Uniform buffer for the MVP matrix
- Depth buffer for correct rendering
- Pipeline with the shader module

The grid pattern in the fragment shader: draw lines at every 15° of latitude and longitude. Color the sphere light blue/green with white grid lines, or similar — just enough to see the geometry clearly.

**Deliverable:** Pipeline created, shader compiles.

### Step 6: Camera and projection

Implement a simple orbital camera:
- **Input:** longitude, latitude, distance (from sliders)
- **Output:** view matrix (via `glam::Mat4::look_at_rh`)
- Projection: perspective matrix with reasonable FOV

The camera orbits around the origin (where the sphere is). Longitude rotates around the Y axis, latitude tilts up/down, zoom changes the distance.

Compute the MVP matrix = projection × view × model (model is identity since the sphere is at origin).

**Deliverable:** Camera produces correct matrices from slider values.

### Step 7: Render loop integration

Connect everything:
1. The `BeforeRendering` callback reads current slider values from Slint properties
2. Computes the MVP matrix from those values (camera module)
3. Writes the MVP matrix to the uniform buffer
4. Runs the render pass targeting the offscreen texture: clear to black, draw the sphere
5. Converts the texture to a Slint `Image` via `Image::try_from(texture)` and sets it on the UI's `Image` component
6. Calls `window.request_redraw()` to ensure the next frame is scheduled

When a slider value changes, call `window.request_redraw()` to trigger a new frame. This causes `BeforeRendering` to fire again with updated values. The result is smooth, live updates as you drag — the sphere re-renders every frame during interaction, same as a game.

When sliders are idle, Slint only redraws when needed (e.g. hover effects), so there's no wasted GPU work.

**Deliverable:** The sphere appears in the window. Moving sliders changes the view in real time.

### Step 8: Polish

- Reasonable default camera position (e.g. looking at 0°N 0°E, medium zoom)
- Smooth slider ranges and step sizes
- Window title: "Sunlit Earth — MVP"
- Handle window resize gracefully (or just use fixed size for now)
- Error handling: if wgpu fails to initialize, show an error message in the UI

**Deliverable:** A polished MVP that demonstrates the core rendering pipeline.

## File structure (expected)

```
sunlit-earth/
├── Cargo.toml
├── build.rs              # Slint build script
├── ui/
│   └── main.slint        # UI layout
├── src/
│   ├── main.rs           # Entry point, Slint window setup
│   ├── renderer.rs       # wgpu rendering notifier, pipeline setup, draw calls
│   ├── sphere.rs         # Sphere mesh generation
│   └── camera.rs         # Orbital camera math
└── shaders/
    └── sphere.wgsl       # Vertex + fragment shader
```

## What this MVP validates

- **wgpu + Slint integration works** via the rendering notifier / render-to-texture approach
- **GPU-accelerated rendering** is smooth and responsive for interactive camera control
- **The rendering pipeline** (geometry → shader → texture → screen) is functional
- **Camera math** is correct and responsive
- **The architecture** (wgpu render-to-texture + Slint Image display) is sound for future development

## Note: wallpaper export reuses the same path

The render-to-texture approach already renders to an offscreen texture — the same path needed for wallpaper export. For wallpaper saving, we just add a CPU readback step (map the texture, save to PNG) using the same render code. No separate offscreen path needed.

## What comes next (not in this plan)

- Real Earth textures (NASA Blue Marble)
- Day/night terminator with sun position
- City lights on the night side
- Atmosphere glow
- Cloud overlay
- Astronomy Engine integration
- Wallpaper export and OS API
- System tray / background operation
