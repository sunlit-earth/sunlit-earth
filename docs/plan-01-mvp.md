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
│  │   wgpu-rendered sphere        │  │
│  │   (underlay, behind Slint)    │  │
│  │                               │  │
│  └───────────────────────────────┘  │
│  Longitude: [━━━━━━●━━━━━━━━━━━━]   │
│  Latitude:  [━━━━━━━━━━━━●━━━━━━]   │
│  Zoom:      [━━━●━━━━━━━━━━━━━━━]   │
└─────────────────────────────────────┘
```

**Rendering approach:** Render the sphere directly into Slint's render pass using `Window::set_rendering_notifier()`. Slint's wgpu backend (feature `unstable-wgpu-28`) provides a `BeforeRendering` callback with access to the wgpu `Device` and `Queue`. We render the sphere as an **underlay** — our 3D scene draws first, then Slint composites its UI (sliders, labels) on top with a transparent background over the viewport area. Everything stays on the GPU, no pixel readback, so slider interaction is smooth and immediate.

## Steps

### Step 1: Project setup and dependencies

Add dependencies to `Cargo.toml`:

| Crate | Purpose |
|---|---|
| `slint` (with feature `unstable-wgpu-28`) | GUI framework + wgpu integration |
| `wgpu` | GPU rendering (version must match Slint's wgpu — currently wgpu 28.x) |
| `glam` | Math (vectors, matrices) — lightweight, widely used with wgpu |
| `bytemuck` | Safe casting for GPU buffer data |

Set up the Slint build script (`build.rs`) to compile `.slint` files.

**Deliverable:** `cargo build` succeeds, empty Slint window opens.

### Step 2: Slint UI layout

Create `ui/main.slint` with:
- A transparent viewport area where the wgpu underlay is visible (the sphere renders behind this region)
- Three `Slider` controls: longitude (-180 to 180), latitude (-90 to 90), zoom (sensible range)
- Labels for each slider showing the current value
- A renderer info line showing GPU name, backend, and device type (e.g. "NVIDIA GeForce RTX 4070 (Dx12, DiscreteGpu)" or "Microsoft Basic Render Driver (Dx12, Cpu)")
- Properties for slider values that Rust code can read, plus a string property for the renderer info

The UI controls panel should have an opaque background so it draws over the underlay. The viewport area must be transparent so the wgpu-rendered sphere shows through.

**Deliverable:** Window opens with working sliders and transparent viewport area.

### Step 3: wgpu initialization via Slint's rendering notifier

Register a `set_rendering_notifier()` callback on the Slint window. This callback fires at different rendering stages:

- **`RenderingSetup`** — wgpu is initialized. We receive `GraphicsAPI::WGPU28 { device, queue, instance }`. Use this to create our GPU resources (buffers, pipeline, depth texture). Also read `AdapterInfo` here for the renderer info display (`name`, `backend`, `device_type` — `DeviceType::Cpu` means software rendering).
- **`BeforeRendering`** — called every frame, before Slint draws its UI. We render the sphere here as an underlay.
- **`RenderingTeardown`** — clean up GPU resources.

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
4. Runs the render pass: clear to black, draw the sphere
5. Slint then draws its UI on top (sliders, labels, renderer info)

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

- **wgpu + Slint integration works** via the rendering notifier / underlay approach
- **Direct GPU rendering** is smooth and responsive for interactive camera control
- **The rendering pipeline** (geometry → shader → screen) is functional
- **Camera math** is correct and responsive
- **The architecture** (wgpu underlay + Slint overlay) is sound for future development

## Note: wallpaper export will need a separate path

The underlay approach renders directly to screen — great for the interactive preview. For wallpaper export (saving to a PNG), we'll later need a separate offscreen render-to-texture path with CPU readback. That's not in this MVP but is a natural addition: same sphere/camera/shader code, different render target.

## What comes next (not in this plan)

- Real Earth textures (NASA Blue Marble)
- Day/night terminator with sun position
- City lights on the night side
- Atmosphere glow
- Cloud overlay
- Astronomy Engine integration
- Wallpaper export and OS API
- System tray / background operation
