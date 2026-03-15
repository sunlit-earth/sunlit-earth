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
cargo clippy               # Lint (pedantic enabled, see Cargo.toml for allows)
cargo run                  # Run the app
cargo run -- --software-rendering  # Force CPU rendering
```

## Architecture

Sunlit Earth is a desktop app that renders a 3D Earth using wgpu and displays it in a Slint window, intended to be set as a wallpaper.

**Render-to-texture pipeline:** wgpu renders the scene to an offscreen texture, which is converted to a Slint `Image` via `Image::try_from(Texture)` and displayed in the UI. This is not a traditional swap-chain render — the GPU output flows through Slint's image component.

**Rendering lifecycle** is driven by Slint's `set_rendering_notifier()` callback:
- `RenderingSetup` — create GPU resources (pipeline, buffers, textures)
- `BeforeRendering` — compute camera matrix from UI sliders, render sphere, convert texture to image
- `RenderingTeardown` — drop GPU resources

**GPU resources** are stored in a `thread_local! { RefCell<Option<GpuResources>> }` in `renderer.rs` because the rendering notifier callback requires `'static` lifetime.

**Key modules:**
- `main.rs` — CLI (clap), window creation, slider/MSAA callbacks, rendering notifier setup
- `renderer.rs` — GPU pipeline, frame rendering, dirty-checking, MSAA management, texture recreation
- `wgpu_init.rs` — manual adapter selection (discrete > integrated > CPU), device creation
- `camera.rs` — orbital camera: (longitude, latitude, distance) → MVP matrix
- `sphere.rs` — parametric UV sphere mesh generation (64×64, position + UV only)
- `grid_texture.rs` — procedural equirectangular grid texture (2048×1024) with CPU-computed mipmaps
- `earth_texture.rs` — JPG loading with coordinate transforms (flip + shift) for NASA Blue Marble textures

**Shader:** `shaders/sphere.wgsl` — vertex transform by MVP, fragment samples texture.

**UI:** `ui/main.slint` — resizable split layout with controls panel (texture combobox, MSAA combobox, longitude/latitude/zoom sliders, adapter info) and image display area.

## Key Constraints

- `unsafe_code = "deny"` in Cargo.toml — use `deny` not `forbid` because Slint macros internally need unsafe
- Slint version pinned to `~1.15` with `unstable-wgpu-28` feature — this is the integration point between Slint and wgpu 28
- Render texture size is quantized to 64px boundaries to reduce GPU texture churn during window resize
- Dirty-checking compares (longitude, latitude, zoom, sample_count, texture_index, dimensions) to skip redundant renders
- Grid texture uses 16× anisotropic filtering with trilinear mipmaps
- LF line endings globally
