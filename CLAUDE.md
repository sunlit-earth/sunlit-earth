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
- `main.rs` — thin binary entry point: CLI (clap), logging initialization (`init_logging()`), window creation, slider/MSAA/wallpaper/mouse-drag/mouse-scroll/apply-preset/load-defaults/reset callbacks, `PRESETS` const array (9 camera presets), rendering notifier setup, periodic sun timer, custom datetime label updates. Config is saved only when "Set as Wallpaper" is clicked (no auto-save timer).
- `scene/` — scene-level abstractions:
  - `scene/camera.rs` — `CameraParams` struct grouping all camera parameters; `OrbitalCamera` with offset, tilt, yaw, pitch; exponential zoom mapping (`zoom_to_distance`/`distance_to_zoom`); orbital camera: (longitude, latitude, distance) -> MVP matrix with post-view rotations and post-projection offset
  - `scene/sun.rs` — safe wrapper around Astronomy Engine FFI for sun position computation (right ascension, declination, sidereal time -> renderer coordinate frame); `sun_direction_at()` for custom date/time
  - `scene/datetime.rs` — pure conversion functions for custom date/time UI: leap year, day-of-year to month/day, hour decomposition, year range; no FFI or side effects
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
  - `renderer/uniforms.rs` — `Uniforms` struct (192 bytes) with `#[repr(C)]`, compile-time size assertion. Includes color correction fields (day_gamma, day_saturation, night_gamma, night_saturation at offsets 128-143), cloud fields (cloud_sphere_radius, cloud_opacity, cloud_floor, cloud_gamma at offsets 144-159), and atmosphere fields (rayleigh_intensity, rayleigh_falloff, nightglow_intensity, nightglow_falloff, nightglow_balance, rayleigh_radius, nightglow_orange_radius, nightglow_green_radius at offsets 160-191).
- `cloud_fetcher.rs` — background cloud texture fetcher: downloads 8K equirectangular cloud JPEG from matteason/live-cloud-maps, caches to `%LOCALAPPDATA%\SunlitEarth\clouds_cache.jpg` with ETag metadata, polls for updates every 60 minutes using HEAD + `If-None-Match` freshness checks, decodes JPEG and sends decoded pixels via `mpsc` channel to the renderer
- `memory.rs` — process-level memory tracking: `current_rss_bytes()` reads Windows working set via `GetProcessMemoryInfo`, `log_memory_usage()` emits structured `debug!` events at key allocation points
- `texture_loader.rs` — generic equirectangular texture loading (JXL via jxl-oxide hook, with coordinate transforms)
- `wallpaper.rs` — Windows-only wallpaper export (`cfg(windows)`): monitor resolution detection via `EnumDisplayMonitors`/`GetMonitorInfoW`, PNG save via `image` crate, wallpaper application via `SystemParametersInfoW` (`windows-sys`)
- `wgpu_init.rs` — manual adapter selection (discrete > integrated > CPU), device creation, `adapter_type_rank()` for testable GPU preference ordering

**Shader:** Split into two files concatenated at load time by `renderer/gpu_setup.rs`:
- `shaders/blend.wgsl` — pure `blend_fragment()` function: day/night blending with diffuse shading and per-channel `min(night, day)` clamp. Also contains `apply_gamma()` and `adjust_saturation()` helper functions for per-texture color correction.
- `shaders/sphere.wgsl` — vertex transform, texture sampling, uniforms; calls `blend_fragment()`. Single-texture mode uses `terminator_width < 0` as sentinel. `schlick_fresnel()` computes Schlick approximation of water Fresnel reflectance, used for both specular modulation and diffuse color shift on ocean pixels. Color correction (gamma, saturation) is applied per-texture after sampling but before blending and shading. `fs_cloud` applies cloud floor and gamma uniforms for real-time contrast tuning: floor removes thin clouds below a threshold, gamma adjusts midtone contrast via power curve. Three concentric atmosphere shells with additive blending, each a separate vertex/fragment entry point pair: `vs_rayleigh`/`fs_rayleigh` (blue Rayleigh scattering on the day-side limb with orange at the terminator, radius ~1.003), `vs_nightglow_orange`/`fs_nightglow_orange` (sodium D + FeO orange nightglow strongest near the terminator, radius ~1.014), `vs_nightglow_green`/`fs_nightglow_green` (OI 557.7nm green nightglow strongest at midnight, radius ~1.015). Both nightglow shells include latitude modulation enhanced near +/-23 degrees. Draw order: Earth, Rayleigh, Nightglow Orange, Nightglow Green, Clouds.

**UI:** `ui/main.slint` — resizable split layout with controls panel wrapped in a `ScrollView`. The top-level controls (always visible) are: "Set as Wallpaper" button, wallpaper status text, "Load Defaults" and "Reset" buttons side by side, and a 3x3 `GridLayout` of camera preset buttons (Europe, N. America, S. America, Africa, Asia, Oceania, Pacific, Blue Marble, Earthrise). Below these is a collapsible "Advanced" toggle (`advanced-open` property, starts closed) containing nine `GroupBox` sections: Camera Position (Longitude, Latitude, Zoom), Camera Orientation (Tilt, Yaw, Pitch), Framing (Offset X, Offset Y), Date / Time (Custom checkbox, Hour slider, Day slider, Year ComboBox), Clouds (Opacity, Floor, Gamma), Atmosphere (Enable checkbox, Rayleigh sub-section with Intensity, Sharpness, and Haze sliders, Nightglow sub-section with Intensity, Balance, and Falloff sliders), Lighting (Terminator Width, Diffuse checkbox + Floor + Ramp, Shininess, Glint, Fresnel Mix, Fresnel Extent), Color Correction (Day Gamma, Day Saturation, Night Gamma, Night Saturation), Rendering (Texture, Anti-Aliasing), and renderer info text at the bottom. Camera properties are `in-out` (bidirectional) with `<=>` slider bindings so Rust can write values back from mouse events. A `TouchArea` overlay in `image-container` handles mouse drag (globe rotation) and scroll (zoom). The Atmosphere and Date / Time sub-controls collapse when their respective checkboxes are unchecked. Callbacks: `apply-preset(int)` sets camera from the `PRESETS` array, `load-defaults()` restores all settings to `AppConfig::default()` without saving, `reset()` reloads config from disk. The only way to persist config is clicking "Set as Wallpaper" (which saves config then applies the wallpaper).

**Wallpaper export pipeline:** The "Set as Wallpaper" button renders the current scene at the primary monitor's native resolution using temporary GPU textures with `COPY_SRC` usage (distinct from the preview textures which use `TEXTURE_BINDING`). Pixels are read back via a staging buffer with 256-byte row alignment, encoded as PNG (fast compression), saved to `%LOCALAPPDATA%\SunlitEarth\wallpaper.png`, and applied via Win32 `SystemParametersInfoW`. The export reuses the existing pipeline and bind groups but creates fresh textures at the target resolution that are dropped after the export completes. PNG is used instead of TIFF because Windows preserves PNG wallpapers losslessly, whereas TIFF wallpapers are JPEG-transcoded at 85% quality, causing visible banding in smooth gradients.

**Notable dependencies beyond wgpu/slint:**
- `astronomy-engine-bindings` — C FFI bindings to the Astronomy Engine library (requires `clang` at build time for bindgen)
- `image` — PNG/JPEG encoding/decoding for wallpaper export (via `png` feature) and cloud image decoding (via `jpeg` feature)
- `time` — UTC time decomposition for astronomy calculations
- `tracing` / `tracing-subscriber` / `tracing-appender` — structured logging with `max_level_debug` (debug builds) and `release_max_level_warn` (release builds); `EnvFilter` respects `RUST_LOG`; non-blocking stderr writer with `FmtSpan::CLOSE` for automatic span timing
- `ureq` — HTTP client for cloud texture fetching (with `rustls` TLS backend)
- `windows-sys` — Win32 FFI for wallpaper export and memory tracking (`cfg(windows)` only): `SystemParametersInfoW`, `EnumDisplayMonitors`, `GetMonitorInfoW`, `GetProcessMemoryInfo`

## Testing

### Coverage

Coverage targets by module type:
- **90-100%**: Pure functions (`build_aa_options`, `quantize_to_granularity`, `adapter_type_rank`, `downsample_2x`, `shift_horizontal`)
- **80-90%**: Business logic with extracted pure functions (`build_frame_state`, `grid_texture::generate`)
- **20-40%**: GPU pipeline code (tested indirectly via integration tests)
- **<20%**: `main.rs` / UI glue (not unit-testable without Slint test backend)
- **Overall target**: 60-70%

### Conventions

- **Test behavior, not constants**: Tests verify that the application behaves correctly (invariants, math, pipelines), not that a constant has a specific value. Changing a preset or default should not cause test failures.
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

- Always run `cargo test` and `cargo clippy` after making code changes to catch regressions and lint issues before presenting work
- Do not commit or push without explicit user approval. Wait for explicit user confirmation that a change works before committing.
- Git worktrees must be created in the `.worktrees/` folder at the repo root
- Keep `docs/roadmap.md` up to date when implementing features — check off completed items and add new entries as needed

## CI/CD

Two GitHub Actions workflows in `.github/workflows/`:

- **`ci.yml`** -- Runs on every push to `main` and every PR targeting `main`. One job:
  - `test` (Windows): `cargo test --locked` -- full test suite including GPU integration tests on the software adapter
  - `fmt` is commented out pending a codebase-wide reformat (see `docs/notes.md`)
- **`release.yml`** -- Runs on semver tag pushes (`v[0-9]+.[0-9]+.[0-9]+`). Builds an optimized binary with `cargo build --release --locked`, packages it as a zip, and creates a GitHub Release with auto-generated notes.

Key CI details:
- LLVM 19 is pinned explicitly on all Windows jobs via `KyleMayes/install-llvm-action@v2` to avoid runner-image Clang version instability
- `LIBCLANG_PATH` is set to `$LLVM_PATH/lib` so bindgen can find `libclang.dll`
- `RUSTFLAGS: "-D warnings"` is commented out pending a lint cleanup (see `docs/roadmap.md`)
- All `cargo` commands use `--locked` for reproducible builds from `Cargo.lock`
- GPU integration tests use the wgpu software adapter on CI runners (no hardware GPU available)
- Clippy is run locally only, not in CI (cargo clippy artifacts are incompatible with cargo test cache, causing full recompilation)
- Release uses a separate cache (`shared-key: release-windows`) because release artifacts differ from debug

## Key Constraints

- `unsafe_code = "deny"` in Cargo.toml — use `deny` not `forbid` because Slint macros internally need unsafe. `sun.rs` and `wallpaper.rs` have scoped `#[allow(unsafe_code)]` on individual FFI call sites with `// SAFETY:` comments.
- Slint version pinned to `~1.15` with `unstable-wgpu-28` feature — this is the integration point between Slint and wgpu 28
- Render texture size is quantized to 64px boundaries to reduce GPU texture churn during window resize
- Dirty-checking via `FrameState` compares (longitude, latitude, zoom, offset_x, offset_y, tilt, yaw, pitch, sample_count, texture_index, dimensions, sun_direction, terminator_width, diffuse_shading, diffuse_floor, diffuse_ramp, spec_shininess, spec_intensity, fresnel_mix, fresnel_exp, cloud_opacity, cloud_floor, cloud_gamma, day_gamma, day_saturation, night_gamma, night_saturation, rayleigh_intensity, rayleigh_falloff, nightglow_intensity, nightglow_falloff, nightglow_balance) to skip redundant renders. Float values are quantized to integer thousandths for stable comparison.
- Zoom slider is normalized (0.0 to 1.0) with exponential mapping: `distance = 1.5 * (80.0 / 1.5)^t`. Use `zoom_to_distance(t)` and `distance_to_zoom(d)` in `scene/camera.rs`.
- Grid texture uses 16x anisotropic filtering with trilinear mipmaps
- WGSL `vec3<f32>` has 16-byte alignment in storage buffers — Rust `#[repr(C)]` structs must include explicit `_pad: f32` after every `[f32; 3]` field to match layout
- GPU integration tests (`tests/shading.rs`, `tests/render_pipeline.rs`) run the real WGSL on the GPU — use `LazyLock<Mutex<...>>` to share the device across parallel test threads (per-test device creation crashes on Windows)
- LF line endings globally
