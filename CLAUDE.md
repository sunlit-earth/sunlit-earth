# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Status

This project is in its early stages and will continue to evolve with frequent breaking changes. Keep this CLAUDE.md up to date as the codebase changes.

Project vision, technical decisions, and implementation plans are documented in `docs/`:
- `docs/project.md` — high-level goals and technology stack
- `docs/tech.md` — deep technical decisions, constraints, rendering approach, wallpaper APIs, astronomy
- `docs/plan-01-mvp.md` — step-by-step MVP implementation plan
- `docs/related.md` — competitive analysis of existing solutions
- `docs/notes.md` — open issues and research topics (e.g. memory usage at high MSAA + 4K)

## Build Commands

```bash
cargo build                # Debug build
cargo build --release      # Release build (LTO, stripped)
cargo test                 # Run all tests
cargo test camera          # Run tests in a single module
cargo test --test shading  # Run GPU shader integration tests only
cargo clippy               # Lint (pedantic enabled, see Cargo.toml for allows)
cargo run                  # Run the app
cargo run -- --software-rendering  # Force CPU rendering
```

## Architecture

Sunlit Earth is a desktop app that renders a 3D Earth using wgpu and displays it in a Slint window, intended to be set as a wallpaper.

**Render-to-texture pipeline:** wgpu renders the scene to an offscreen texture, which is converted to a Slint `Image` via `Image::try_from(Texture)` and displayed in the UI. This is not a traditional swap-chain render — the GPU output flows through Slint's image component.

**Rendering lifecycle** is driven by Slint's `set_rendering_notifier()` callback:
- `RenderingSetup` — create GPU resources (pipeline, buffers, textures)
- `BeforeRendering` — compute camera matrix and sun direction, render sphere with day/night blending, convert texture to image
- `RenderingTeardown` — drop GPU resources
- A 2-minute periodic Slint timer triggers automatic redraws so the terminator moves with the sun

**GPU resources** are stored in a `thread_local! { RefCell<Option<GpuResources>> }` in `renderer.rs` because the rendering notifier callback requires `'static` lifetime.

**Key modules:**
- `main.rs` — CLI (clap), window creation, slider/MSAA callbacks, rendering notifier setup, periodic sun timer
- `renderer.rs` — GPU pipeline, frame rendering, dirty-checking, MSAA management, texture recreation, lazy texture loading via `TextureSlot` Vec, day/night composite bind group, 96-byte uniform buffer (MVP + sun_dir + terminator_width + flags)
- `sun.rs` — safe wrapper around Astronomy Engine FFI for sun position computation (right ascension, declination, sidereal time → renderer coordinate frame)
- `wgpu_init.rs` — manual adapter selection (discrete > integrated > CPU), device creation
- `camera.rs` — orbital camera: (longitude, latitude, distance) → MVP matrix
- `sphere.rs` — parametric UV sphere mesh generation (64×64, position + UV only)
- `grid_texture.rs` — procedural equirectangular grid texture (2048×1024) with CPU-computed mipmaps
- `texture_loader.rs` — generic equirectangular texture loading (JXL via jxl-oxide hook, with coordinate transforms)

**Shader:** Split into two files concatenated at load time by `renderer.rs`:
- `shaders/blend.wgsl` — pure `blend_fragment()` function: day/night blending with diffuse shading and per-channel `min(night, day)` clamp
- `shaders/sphere.wgsl` — vertex transform, texture sampling, uniforms; calls `blend_fragment()`. Single-texture mode uses `terminator_width < 0` as sentinel.

**UI:** `ui/main.slint` — resizable split layout with controls panel (texture combobox with Day/Night Blend mode, MSAA combobox, longitude/latitude/zoom sliders, terminator width slider, diffuse shading checkbox, adapter info) and image display area.

**Notable dependencies beyond wgpu/slint:**
- `astronomy-engine-bindings` — C FFI bindings to the Astronomy Engine library (requires `clang` at build time for bindgen)
- `time` — UTC time decomposition for astronomy calculations

## Workflow

- Do not commit during interactive debugging — wait for explicit user confirmation that a change works before committing

## Key Constraints

- `unsafe_code = "deny"` in Cargo.toml — use `deny` not `forbid` because Slint macros internally need unsafe. `sun.rs` has scoped `#[allow(unsafe_code)]` on individual FFI call sites.
- Slint version pinned to `~1.15` with `unstable-wgpu-28` feature — this is the integration point between Slint and wgpu 28
- Render texture size is quantized to 64px boundaries to reduce GPU texture churn during window resize
- Dirty-checking compares (longitude, latitude, zoom, sample_count, texture_index, dimensions, sun_direction, terminator_width, diffuse_shading) to skip redundant renders
- Grid texture uses 16× anisotropic filtering with trilinear mipmaps
- WGSL `vec3<f32>` has 16-byte alignment in storage buffers — Rust `#[repr(C)]` structs must include explicit `_pad: f32` after every `[f32; 3]` field to match layout
- GPU integration tests (`tests/shading.rs`) run the real WGSL on the GPU via compute shader — use `LazyLock<Mutex<GpuContext>>` to share the device across parallel test threads (per-test device creation crashes on Windows)
- LF line endings globally
