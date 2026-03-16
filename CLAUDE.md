# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Status

This project is in its early stages and will continue to evolve with frequent breaking changes. Keep this CLAUDE.md up to date as the codebase changes.

Project vision, technical decisions, and implementation plans are documented in `docs/` — see `docs/README.md` for an overview.

## Build Commands

```bash
cargo build                # Debug build
cargo build --release      # Release build (LTO, stripped)
cargo test                 # Run all tests
cargo test camera          # Run tests in a single module
cargo test --test shading  # Run GPU shader integration tests only
cargo test --test render_pipeline  # Run render pipeline GPU tests only
cargo clippy               # Lint (pedantic enabled, see Cargo.toml for allows)
cargo run                  # Run the app
cargo run -- --software-rendering  # Force CPU rendering
cargo llvm-cov --html      # Generate HTML coverage report (target/llvm-cov/html/)
cargo llvm-cov --lcov      # Generate LCOV coverage report (for CI)
```

## Architecture

Sunlit Earth is a desktop app that renders a 3D Earth using wgpu and displays it in a Slint window, intended to be set as a wallpaper.

**Render-to-texture pipeline:** wgpu renders the scene to an offscreen texture, which is converted to a Slint `Image` via `Image::try_from(Texture)` and displayed in the UI. This is not a traditional swap-chain render — the GPU output flows through Slint's image component.

**Rendering lifecycle** is driven by Slint's `set_rendering_notifier()` callback:
- `RenderingSetup` — create GPU resources (pipeline, buffers, textures)
- `BeforeRendering` — compute camera matrix and sun direction, render sphere with day/night blending, convert texture to image
- `RenderingTeardown` — drop GPU resources
- A 2-minute periodic Slint timer triggers automatic redraws so the terminator moves with the sun

**GPU resources** are stored in a `thread_local! { RefCell<Option<GpuResources>> }` in `renderer/mod.rs` because the rendering notifier callback requires `'static` lifetime.

**Key modules:**
- `lib.rs` — crate root, module declarations, `slint::include_modules!()` macro invocation
- `main.rs` — thin binary entry point: CLI (clap), window creation, slider/MSAA/wallpaper callbacks, rendering notifier setup, periodic sun timer
- `scene/` — scene-level abstractions:
  - `scene/camera.rs` — orbital camera: (longitude, latitude, distance) -> MVP matrix
  - `scene/sun.rs` — safe wrapper around Astronomy Engine FFI for sun position computation (right ascension, declination, sidereal time -> renderer coordinate frame)
- `geometry/` — mesh and procedural texture generation:
  - `geometry/sphere.rs` — parametric UV sphere mesh generation (64x64, position + UV only)
  - `geometry/grid_texture.rs` — procedural equirectangular grid texture (2048x1024) with CPU-computed mipmaps
- `renderer/` — GPU pipeline, frame rendering, dirty-checking, split into focused submodules:
  - `renderer/mod.rs` — public API (`setup_rendering_notifier`, `build_aa_options`, `export_wallpaper_image`), rendering callback dispatcher, `GpuResources` struct, `quantize_to_granularity`, constants, thread-local `GPU_RESOURCES`
  - `renderer/frame.rs` — `FrameState` struct and `build_frame_state()` for dirty-check comparison
  - `renderer/render_pass.rs` — render pass encoding, uniform writes, texture-to-Slint-image conversion, `read_texture_rgba8` GPU-to-CPU pixel readback
  - `renderer/texture_routing.rs` — blend mode detection, texture load spawning, loading indicator text
  - `renderer/gpu_setup.rs` — `create_gpu_resources()`, `create_pipeline()`, `create_render_textures()`, MSAA/resize rebuild functions
  - `renderer/textures.rs` — `TextureSlot`, texture loading/decoding, composite bind group, `create_mipmapped_texture()`, `downsample_2x()`
  - `renderer/uniforms.rs` — `Uniforms` struct with `#[repr(C)]`, compile-time size assertion
- `texture_loader.rs` — generic equirectangular texture loading (JXL via jxl-oxide hook, with coordinate transforms)
- `wallpaper.rs` — Windows-only wallpaper export (`cfg(windows)`): monitor resolution detection via `EnumDisplayMonitors`/`GetMonitorInfoW`, PNG save via `image` crate, wallpaper application via `SystemParametersInfoW` (`windows-sys`)
- `wgpu_init.rs` — manual adapter selection (discrete > integrated > CPU), device creation, `adapter_type_rank()` for testable GPU preference ordering

**Shader:** Split into two files concatenated at load time by `renderer/gpu_setup.rs`:
- `shaders/blend.wgsl` — pure `blend_fragment()` function: day/night blending with diffuse shading and per-channel `min(night, day)` clamp
- `shaders/sphere.wgsl` — vertex transform, texture sampling, uniforms; calls `blend_fragment()`. Single-texture mode uses `terminator_width < 0` as sentinel.

**UI:** `ui/main.slint` — resizable split layout with controls panel (texture combobox with Day/Night Blend mode, MSAA combobox, longitude/latitude/zoom sliders, terminator width slider, diffuse shading checkbox, "Set as Wallpaper" button with status text, adapter info) and image display area.

**Wallpaper export pipeline:** The "Set as Wallpaper" button renders the current scene at the primary monitor's native resolution using temporary GPU textures with `COPY_SRC` usage (distinct from the preview textures which use `TEXTURE_BINDING`). Pixels are read back via a staging buffer with 256-byte row alignment, encoded as PNG (fast compression), saved to `%LOCALAPPDATA%\SunlitEarth\wallpaper.png`, and applied via Win32 `SystemParametersInfoW`. The export reuses the existing pipeline and bind groups but creates fresh textures at the target resolution that are dropped after the export completes. PNG is used instead of TIFF because Windows preserves PNG wallpapers losslessly, whereas TIFF wallpapers are JPEG-transcoded at 85% quality, causing visible banding in smooth gradients.

**Notable dependencies beyond wgpu/slint:**
- `astronomy-engine-bindings` — C FFI bindings to the Astronomy Engine library (requires `clang` at build time for bindgen)
- `image` — PNG encoding for wallpaper export (via `png` feature)
- `time` — UTC time decomposition for astronomy calculations
- `windows-sys` — Win32 FFI for wallpaper export (`cfg(windows)` only): `SystemParametersInfoW`, `EnumDisplayMonitors`, `GetMonitorInfoW`

## Testing

### Coverage

Coverage targets by module type:
- **90-100%**: Pure functions (`build_aa_options`, `quantize_to_granularity`, `adapter_type_rank`, `downsample_2x`, `shift_horizontal`)
- **80-90%**: Business logic with extracted pure functions (`build_frame_state`, `grid_texture::generate`)
- **20-40%**: GPU pipeline code (tested indirectly via integration tests)
- **<20%**: `main.rs` / UI glue (not unit-testable without Slint test backend)
- **Overall target**: 60-70%

### Conventions

- **Float comparisons**: Use `approx::assert_relative_eq!` (not raw epsilon patterns). `tests/shading.rs` is an exception — its GPU tolerance pattern predates this convention and works well as-is.
- **GPU integration tests**: Assert behavioral invariants (monotonicity, bounds, visibility), not pixel-exact values, due to cross-hardware float variance.
- **GPU device sharing**: Use `LazyLock<Mutex<GpuContext>>` to share a single device across parallel test threads. Per-test device creation crashes on Windows.
- **Shared GPU helpers**: `tests/common/mod.rs` provides `GpuContext`, `create_gpu_context()`, `read_buffer()`, and `read_texture_rgba8()`.
- **Property-based testing**: `proptest` for invariants of pure functions (output size, identity after N applications).

### Dev-Dependencies

```toml
[dev-dependencies]
approx = "0.5"   # assert_relative_eq! for float comparisons
proptest = "1"    # property-based testing for pure functions
```

## Workflow

- Do not commit during interactive debugging — wait for explicit user confirmation that a change works before committing

## Key Constraints

- `unsafe_code = "deny"` in Cargo.toml — use `deny` not `forbid` because Slint macros internally need unsafe. `sun.rs` and `wallpaper.rs` have scoped `#[allow(unsafe_code)]` on individual FFI call sites with `// SAFETY:` comments.
- Slint version pinned to `~1.15` with `unstable-wgpu-28` feature — this is the integration point between Slint and wgpu 28
- Render texture size is quantized to 64px boundaries to reduce GPU texture churn during window resize
- Dirty-checking via `FrameState` compares (longitude, latitude, zoom, sample_count, texture_index, dimensions, sun_direction, terminator_width, diffuse_shading, diffuse_floor, diffuse_ramp) to skip redundant renders. Float values are quantized to integer thousandths for stable comparison.
- Grid texture uses 16x anisotropic filtering with trilinear mipmaps
- WGSL `vec3<f32>` has 16-byte alignment in storage buffers — Rust `#[repr(C)]` structs must include explicit `_pad: f32` after every `[f32; 3]` field to match layout
- GPU integration tests (`tests/shading.rs`, `tests/render_pipeline.rs`) run the real WGSL on the GPU — use `LazyLock<Mutex<...>>` to share the device across parallel test threads (per-test device creation crashes on Windows)
- LF line endings globally
