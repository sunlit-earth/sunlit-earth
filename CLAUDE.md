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
cargo run -- --tray-only   # Start in tray-only mode (no window)
cargo run -- --show-window # Force showing window (overrides tray-only)
cargo llvm-cov --html      # Generate HTML coverage report (target/llvm-cov/html/)
cargo llvm-cov --lcov      # Generate LCOV coverage report (for CI)
```

## Architecture

Sunlit Earth is a desktop app that renders a 3D Earth using wgpu and displays it in a Slint window, intended to be set as a wallpaper. It runs as a system tray application: on first start the main window is shown; after setting a wallpaper, subsequent starts default to tray-only mode.

**Application lifecycle:**
- First start: main window + tray icon appear; user configures scene and sets wallpaper
- Subsequent starts: tray-only mode (no window, no GPU overhead for preview); "Settings" opens the window on demand
- Close button hides window to tray (`CloseRequestResponse::HideWindow`); "Quit" from tray exits
- Single-instance detection via `single-instance` crate prevents duplicate processes
- Event loop uses `run_event_loop_until_quit()` so it stays alive after window is hidden

**Render-to-texture pipeline:** wgpu renders the scene to an offscreen texture, which is converted to a Slint `Image` via `Image::try_from(Texture)` and displayed in the UI. This is not a traditional swap-chain render — the GPU output flows through Slint's image component.

**Rendering lifecycle** is driven by Slint's `set_rendering_notifier()` callback:
- `RenderingSetup` — create GPU resources (pipeline, buffers, textures)
- `BeforeRendering` — compute camera matrix and sun direction, render sphere with day/night blending, convert texture to image
- `RenderingTeardown` — drop GPU resources
- A 2-minute periodic Slint timer triggers automatic redraws so the terminator moves with the sun

**GPU resources** are stored in a `thread_local! { RefCell<Option<GpuResources>> }` in `renderer/mod.rs` because the rendering notifier callback requires `'static` lifetime.

**Headless renderer** (`headless.rs`): self-contained rendering pipeline that creates its own wgpu device/queue, builds the full pipeline, renders one frame at target resolution, reads back pixels, and drops all GPU resources. Used by the "Set as Wallpaper" button and background auto-refresh — no dependency on `GPU_RESOURCES` or a visible window.

**Key modules:**
- `lib.rs` — crate root, module declarations, `slint::include_modules!()` macro invocation
- `main.rs` — binary entry point: CLI (clap with `--tray-only`, `--show-window`, `--software-rendering`), single-instance guard, startup mode detection, window creation, slider/MSAA/wallpaper/mouse-drag/mouse-scroll/reset-all callbacks, rendering notifier setup, periodic sun timer, tray thread spawn, event loop
- `headless.rs` — standalone headless renderer: `render_wallpaper_headless(config, dimensions, force_software)` creates device/queue, loads textures synchronously, renders one frame, reads back RGBA8 pixels. Independent of Slint and `GPU_RESOURCES`.
- `tray.rs` — system tray icon (Windows, `cfg(windows)`): dedicated background thread with Win32 message pump, context menu (Refresh, Auto Refresh, Settings, Quit), auto-refresh timer thread with `AtomicBool`/`AtomicU32` control, `do_headless_wallpaper_export()` shared helper
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
  - `renderer/render_pass.rs` — render pass encoding, uniform writes, texture-to-Slint-image conversion, `read_texture_rgba8` GPU-to-CPU pixel readback. `ShadingParams`, `RenderTarget`, `write_uniforms()`, `encode_and_submit()` are `pub(crate)` for headless renderer access.
  - `renderer/texture_routing.rs` — blend mode detection, texture load spawning, loading indicator text
  - `renderer/gpu_setup.rs` — `create_gpu_resources()` and extracted `pub(crate)` helpers: `create_bind_group_layout()`, `create_sampler()`, `create_shader_module()`, `create_sphere_buffers()`, `create_dummy_texture()`, `create_pipeline_layout()`, `create_pipeline()`, `create_cloud_pipeline()`, `create_render_textures()`. MSAA/resize rebuild functions.
  - `renderer/textures.rs` — `TextureSlot`, texture loading/decoding, composite bind group, `create_mipmapped_texture()`, `create_bind_group()`, `downsample_2x()`
  - `renderer/uniforms.rs` — `Uniforms` struct (160 bytes) with `#[repr(C)]`, compile-time size assertion. Includes color correction fields (day_gamma, day_saturation, night_gamma, night_saturation at offsets 128-143) and cloud fields (cloud_sphere_radius, cloud_opacity, cloud_floor, cloud_gamma at offsets 144-159).
- `cloud_fetcher.rs` — background cloud texture fetcher: downloads 8K equirectangular cloud JPEG from matteason/live-cloud-maps, caches to `%LOCALAPPDATA%\SunlitEarth\clouds_cache.jpg` with ETag metadata, polls for updates every 60 minutes using HEAD + `If-None-Match` freshness checks, decodes JPEG and sends decoded pixels via `mpsc` channel to the renderer. Also provides `check_and_refresh_cloud_cache()` for synchronous cache refresh before headless renders.
- `config.rs` — `AppConfig` struct with all persisted settings including `auto_refresh_enabled`, `auto_refresh_interval_minutes`, `has_set_wallpaper`. TOML serialization under `[sunlit.earth]` table. Atomic write via tilde-suffix temp files.
- `texture_loader.rs` — generic equirectangular texture loading (JXL via jxl-oxide hook, with coordinate transforms)
- `wallpaper.rs` — Windows-only wallpaper export (`cfg(windows)`): monitor resolution detection via `EnumDisplayMonitors`/`GetMonitorInfoW`, PNG save via `image` crate, wallpaper application via `SystemParametersInfoW` (`windows-sys`)
- `wgpu_init.rs` — manual adapter selection (discrete > integrated > CPU), device creation, `adapter_type_rank()` for testable GPU preference ordering. Provides both `init()` (for Slint integration) and `init_headless()` (raw device/queue for headless rendering).

**Shader:** Split into two files concatenated at load time by `renderer/gpu_setup.rs`:
- `shaders/blend.wgsl` — pure `blend_fragment()` function: day/night blending with diffuse shading and per-channel `min(night, day)` clamp. Also contains `apply_gamma()` and `adjust_saturation()` helper functions for per-texture color correction.
- `shaders/sphere.wgsl` — vertex transform, texture sampling, uniforms; calls `blend_fragment()`. Single-texture mode uses `terminator_width < 0` as sentinel. `schlick_fresnel()` computes Schlick approximation of water Fresnel reflectance, used for both specular modulation and diffuse color shift on ocean pixels. Color correction (gamma, saturation) is applied per-texture after sampling but before blending and shading. `fs_cloud` applies cloud floor and gamma uniforms for real-time contrast tuning: floor removes thin clouds below a threshold, gamma adjusts midtone contrast via power curve.

**UI:** `ui/main.slint` — resizable split layout with controls panel wrapped in a `ScrollView`, organized into nine `GroupBox` sections: Rendering (Texture, Anti-Aliasing), Camera Position (Longitude, Latitude, Zoom), Camera Orientation (Tilt, Yaw, Pitch), Framing (Offset X, Offset Y), Lighting (Terminator Width, Diffuse checkbox + Floor + Ramp, Shininess, Glint, Fresnel Mix, Fresnel Extent), Color Correction (Day Gamma, Day Saturation, Night Gamma, Night Saturation), Clouds (Opacity, Floor, Gamma), Auto Refresh (Enable checkbox, Interval slider 1-30 min), Date / Time (Custom checkbox, Hour slider, Day slider, Year ComboBox). Reset All and Set as Wallpaper buttons are below the groups. Renderer info scrolls with the controls. Camera properties are `in-out` (bidirectional) with `<=>` slider bindings so Rust can write values back from mouse events. A `TouchArea` overlay in `image-container` handles mouse drag (globe rotation) and scroll (zoom). The Date / Time and Auto Refresh controls collapse when their checkboxes are unchecked.

**Wallpaper export pipeline:** The "Set as Wallpaper" button and background auto-refresh both use the headless renderer: `headless::render_wallpaper_headless()` creates its own wgpu device, builds the full pipeline, renders at the primary monitor's native resolution, and returns RGBA8 pixels. These are encoded as PNG (fast compression), saved to `%LOCALAPPDATA%\SunlitEarth\wallpaper.png`, and applied via Win32 `SystemParametersInfoW`. The headless renderer is self-contained and does not depend on `GPU_RESOURCES` or a visible window. PNG is used instead of TIFF because Windows preserves PNG wallpapers losslessly, whereas TIFF wallpapers are JPEG-transcoded at 85% quality, causing visible banding in smooth gradients.

**Notable dependencies beyond wgpu/slint:**
- `astronomy-engine-bindings` — C FFI bindings to the Astronomy Engine library (requires `clang` at build time for bindgen)
- `image` — PNG/JPEG encoding/decoding for wallpaper export (via `png` feature) and cloud image decoding (via `jpeg` feature)
- `time` — UTC time decomposition for astronomy calculations
- `ureq` — HTTP client for cloud texture fetching (with `rustls` TLS backend)
- `windows-sys` — Win32 FFI for wallpaper export and tray message pump (`cfg(windows)` only)
- `tray-icon` — system tray icon support (`cfg(windows)` only)
- `muda` — context menu for tray icon (`cfg(windows)` only, companion crate from the Tauri project)
- `single-instance` — cross-platform single-instance detection via named mutex (Windows) or file lock (Linux/macOS)

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
- Git worktrees must be created in the `.worktrees/` folder at the repo root

## Key Constraints

- `unsafe_code = "deny"` in Cargo.toml — use `deny` not `forbid` because Slint macros internally need unsafe. `sun.rs`, `wallpaper.rs`, and `tray.rs` have scoped `#[allow(unsafe_code)]` on individual FFI call sites with `// SAFETY:` comments.
- Slint version pinned to `~1.15` with `unstable-wgpu-28` feature — this is the integration point between Slint and wgpu 28
- Render texture size is quantized to 64px boundaries to reduce GPU texture churn during window resize
- Dirty-checking via `FrameState` compares (longitude, latitude, zoom, offset_x, offset_y, tilt, yaw, pitch, sample_count, texture_index, dimensions, sun_direction, terminator_width, diffuse_shading, diffuse_floor, diffuse_ramp, spec_shininess, spec_intensity, fresnel_mix, fresnel_exp, cloud_opacity, cloud_floor, cloud_gamma, day_gamma, day_saturation, night_gamma, night_saturation) to skip redundant renders. Float values are quantized to integer thousandths for stable comparison.
- Zoom slider is normalized (0.0 to 1.0) with exponential mapping: `distance = 1.5 * (80.0 / 1.5)^t`. Use `zoom_to_distance(t)` and `distance_to_zoom(d)` in `scene/camera.rs`.
- Grid texture uses 16x anisotropic filtering with trilinear mipmaps
- WGSL `vec3<f32>` has 16-byte alignment in storage buffers — Rust `#[repr(C)]` structs must include explicit `_pad: f32` after every `[f32; 3]` field to match layout
- GPU integration tests (`tests/shading.rs`, `tests/render_pipeline.rs`) run the real WGSL on the GPU — use `LazyLock<Mutex<...>>` to share the device across parallel test threads (per-test device creation crashes on Windows)
- LF line endings globally
