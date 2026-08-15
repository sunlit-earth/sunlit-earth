# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Status

This project is in its early stages and will continue to evolve with frequent breaking changes. Keep this CLAUDE.md up to date as the codebase changes.

Project vision, technical decisions, and implementation plans are documented in `docs/`; see `docs/README.md` for an overview. `docs/retrospective-2026-08.md` explains why the architecture looks the way it does; read section 7 before changing the engine or the crate split.

## Build Commands

```bash
cargo build                        # Debug build (whole workspace)
cargo build --release              # Release build (LTO, stripped)
cargo test                         # Run all tests in the workspace
cargo test -p sunlit-core          # Core only
cargo test -p sunlit-core --test engine   # Engine integration tests
cargo test -p sunlit-core --test soak     # Mock-clock soak test (14 simulated days)
cargo test -p sunlit-core --test golden   # Golden images + contact sheet
cargo test --test e2e -- --ignored # Desktop e2e suite (needs a real desktop and GPU)
cargo clippy --all-targets         # Lint (pedantic enabled, see Cargo.toml for allows)
cargo run                          # Run the app
cargo run -- --software-rendering  # Force CPU rendering
cargo run -- --quality high        # Override the quality tier for one run
cargo llvm-cov --html              # HTML coverage report (target/llvm-cov/html/)

SUNLIT_EARTH_UPDATE_GOLDEN=1 cargo test -p sunlit-core --test golden  # Regenerate goldens
```

## Workspace Layout

```
sunlit-earth/
  crates/
    sunlit-core/     # headless: no Slint, no window, no event loop
      shaders/       # WGSL, included at compile time by renderer/gpu_setup.rs
      src/assets/    # texture loading, cloud source + updater, texture mailbox
      src/engine/    # the engine thread, injectable clock, wallpaper sink
      src/geometry/  # sphere mesh, procedural grid texture
      src/renderer/  # wgpu pipeline, offscreen render, readback
      src/scene/     # camera, sun (astronomy FFI), datetime
      src/config.rs  # AppConfig, QualityTier, persistence
      src/params.rs  # SceneParams, ParamsDigest, gamma slider mapping
      tests/         # engine, soak, golden, shading, render_pipeline
    sunlit-app/      # Slint shell: window, tray, IPC, config bridge
      ui/main.slint  # MainWindow and TrayIcon
      tests/         # e2e (desktop-gated), slint_ui
  textures/          # local 8K JXL assets, not part of the build
```

The package inside `crates/sunlit-app` is still named `sunlit-earth`, so the binary, `CARGO_BIN_EXE_sunlit-earth`, and `target/release/sunlit-earth.exe` in the release workflow are unchanged by the directory name.

## Architecture

The organizing principle is **headless first**. The engine runs to completion with no window at all; the settings window is one optional client. Hiding the window removes a client, it does not half-suspend the machinery. This is what removed the tray-mode memory leak class, made soak tests possible, and retired the teardown hacks.

### The engine (`sunlit_core::engine`)

One thread owns the wgpu device, the `Renderer`, the texture mailbox, and the schedule. Clients send `EngineCommand`s and receive `EngineEvent`s.

- **Commands**: `UpdateParams`, `SetPreviewSize`, `SetPreviewEnabled`, `RenderWallpaperNow`, `RenderToFile`, `ExportPixels`, `SetAutoRefresh`, `Poke`, `Shutdown`.
- **Events**: `PreviewFrame { rgba, width, height }`, `TexturesReady`, `WallpaperSet(Result)`, `Status(String)`.
- **The loop never sleeps on wall time to decide what is due.** It blocks on the command channel with a 50 ms timeout and, on each wake, asks `clock.elapsed()` what is due: the texture drain (5 s), the sun-position refresh (120 s), the cloud poll, the memory metrics sample (600 s), and the auto-refresh export. `Schedule::due` recomputes its deadline from `now` rather than accumulating, so a long stall produces one run and not a burst of catch-up runs.
- **Injected `Clock`.** `SystemClock` in production, `MockClock` in tests. `MockClock` advances UTC too, so simulated days really do rotate the Earth. This is what makes 14 simulated days run in 13 seconds.
- **Injected `CloudSource`.** `HttpCloudSource` in production, fixtures in tests. A dedicated cloud worker thread does network I/O and JPEG decoding and never touches the GPU; it parks frames in the mailbox and pokes the engine, which uploads on its own schedule. A poll skipped because the worker is busy retries on the next tick.
- **Injected `WallpaperSink`.** `SystemWallpaper` writes a PNG and calls the Win32 API; `CountingSink` lets the soak test run for simulated weeks without touching the desktop.
- **Preview frames are pixel buffers**, not shared GPU textures. The engine reads its offscreen target back and hands over RGBA bytes; the app wraps them in `slint::Image::from_rgba8`. Slint therefore needs no wgpu feature and shares no device, which is why teardown is ordinary drop order.

### Parameters (`sunlit_core::params`)

`SceneParams` is the single description of what to draw: camera, texture selection, sample count, lighting, clouds, atmosphere, color correction, and the datetime input. There are exactly two translation points:

1. `ui_callbacks::read_params_from_window` / `apply_params_to_window` in the app.
2. `renderer::render_pass::write_uniforms` in core.

Adding a shader parameter means: the `.slint` property and slider, the `AppConfig` field, `SceneParams` + its `ParamsDigest`, `Uniforms`, and the WGSL. The bridge functions and the dirty check follow from the struct. `params.rs` has a table-driven test that walks every parameter and asserts it changes the digest, so forgetting the dirty check is a test failure rather than a stale-frame bug.

`ParamsDigest` is the quantized snapshot used for dirty checking: camera floats compare exactly, everything else is rounded to integer thousandths. `datetime` is deliberately not in the digest; what the shader consumes is the sun direction derived from it, and `FrameState` compares that separately along with the render size.

### The renderer (`sunlit_core::renderer`)

`Renderer` owns every GPU object and renders offscreen into its own texture (`RENDER_ATTACHMENT | TEXTURE_BINDING | COPY_SRC`). It knows nothing about windows. Key methods: `render(&SceneParams, sun_dir) -> RenderOutcome`, `resize`, `drain_texture_updates`, `export_image`, `read_preview_pixels`, `textures_ready`, `loading_text`.

Submodules: `gpu_setup` (construction, pipelines, render targets), `render_pass` (uniform encoding, pass encoding, `Overlays::select`, `read_texture_rgba8`), `textures` (slots, mailbox draining, mipmapped upload, `downsample_2x`), `texture_routing` (which bind group and blend mode), `frame` (`FrameState` dirty check), `uniforms` (the 192-byte `#[repr(C)]` struct).

### The app (`sunlit-app`)

- `main.rs`: CLI (clap), logging, config load, then one of two paths. `run_render` is fully headless: no window, no Slint backend, no event loop; it starts the engine with the preview disabled, waits for `TexturesReady`, calls `render_to_file`, and returns an `ExitCode`. `run_app` creates the window, starts the engine, wires the UI, and runs the event loop. CLI flags: `--mode <tray|window>`, `--tray-start <visible|hidden>`, `--ipc-socket <name>`, `--quality <low|medium|high>`, `--software-rendering`, `--textures-dir`, `--log-level`, plus the `render` subcommand.
- `engine_client.rs`: `EngineLink` (send commands, push window state as `SceneParams`) and `event_forwarder` (engine events to the window). Preview frames cross the thread boundary through a latest-value mailbox with a single pending wake-up: the newest frame replaces the parked one and only one `invoke_from_event_loop` closure is ever in flight.
- `ui_callbacks.rs`: callback registration grouped into mouse, change, and action callbacks; every one of them ends in `link.push_params(&window)`. Also the config bridge (`apply_config_to_window`, `read_config_from_window`) and `defer_combobox_indices`.
- `ipc.rs`: opt-in control channel over `interprocess` local sockets. Commands: `quit`, `show-window`, `hide-window`, `export-test`, `query-memory`. Fire-and-forget, with `SIGNAL:` lines on stdout as the reply channel. `export-test` and `query-memory` are answered on the listener thread, so they work while the event loop is idle.
- `tray.rs`: the procedurally generated 32x32 icon, the tray callback wiring, and single-instance enforcement. The tray icon itself is a `SystemTrayIcon` component in `ui/main.slint`, so Slint owns the platform integration.
- `mouse_math.rs`: pure functions for mouse interaction (globe drag with tilt correction, frame drag, orient drag, tilt drag, zoom scroll). No Slint dependency; unit-tested with `proptest` invariants.

### UI (`ui/main.slint`)

`MainWindow`: resizable split layout, controls panel in a `ScrollView`. Top-level controls are "Set as Wallpaper", "Load Defaults" and "Reset", and a 3x3 grid of camera presets. Below is a collapsible "Advanced" section with `GroupBox`es for Camera Position, Camera Orientation, Framing, Date / Time, Clouds, Atmosphere, Lighting, Color Correction, and Rendering. Camera properties are `in-out` with `<=>` slider bindings. A `TouchArea` over the image handles drag and scroll.

`TrayIcon` inherits `SystemTrayIcon`: menu (Open, Refresh Now, checkable Auto-refresh, Exit) and `clicked()` to toggle the window. Only properties *declared* on the derived component are exposed to Rust, so the inherited `icon` is bound to a declared `tray-image` property. A `SystemTrayIcon`-rooted component implements `StrongHandle` but not `ComponentHandle`, so there is no `as_weak()`; the handle is kept in an `Rc`.

### Quality tiers

`QualityTier` (low, medium, high) is persisted in the config and overridable per run with `--quality` (the override is not written back). It has no widget in the settings window, which is why `read_config_from_window` is a read-modify-write against the stored config rather than a fresh `AppConfig::default()`: any persisted setting the UI does not manage has to survive a save untouched. It caps the MSAA sample count (1, 4, unlimited), the preview width (1280, 1920, unlimited, aspect preserved), and selects the cloud image variant (2048x1024, 4096x2048, 8192x4096). Default: low in debug builds, high in release; `EngineConfig::headless` pins low so tests do not depend on the build profile.

### Sample counts

An MSAA sample count the adapter does not support is not a warning inside wgpu, it is a validation error that kills whichever thread builds the render target. Two layers guard it, and they are not redundant:

1. **The combo box** is built by `renderer::build_aa_options(adapter_supported, tier_cap)`, so the UI only ever offers counts that are both supported and within the tier.
2. **The engine resolves every requested count** through `renderer::resolve_sample_count` in `Engine::new` and again on every `UpdateParams`, and logs a `warn!` when it has to fall back. This is the single source of truth, and it is the one that matters: a config file, a hand-edited value, or a combo box index saved on a machine with a different GPU all arrive as a bare number in `SceneParams` and never go through the combo box.

The rule is "the highest supported count at most the requested one, otherwise the lowest on offer". `tests/engine.rs` starts an engine at the High tier (which does not cap) with `sample_count = 64` and asserts a frame still arrives.

### Shaders

`shaders/blend.wgsl` and `shaders/sphere.wgsl` are concatenated at load time by `renderer/gpu_setup.rs`.

- `blend.wgsl`: `blend_fragment()` (day/night blending with diffuse shading and a per-channel `min(night, day)` clamp), plus `apply_gamma()` and `adjust_saturation()`.
- `sphere.wgsl`: vertex transform, texture sampling, uniforms. Single-texture mode uses `terminator_width < 0` as a sentinel, and in that mode the shader ignores the sun entirely. `schlick_fresnel()` drives both specular modulation and the diffuse color shift on ocean pixels. `fs_cloud` applies the cloud floor and gamma. Three concentric atmosphere shells, each with its own vertex/fragment pair: `vs_rayleigh`/`fs_rayleigh` (radius ~1.015), `vs_nightglow_orange`/`fs_nightglow_orange` (~1.014), `vs_nightglow_green`/`fs_nightglow_green` (~1.015). Draw order: Earth, Clouds, Rayleigh, Nightglow Orange, Nightglow Green.

### Wallpaper export

The engine renders at the sink's native resolution using temporary GPU textures with `COPY_SRC`, reads them back through a staging buffer with 256-byte row alignment, encodes PNG, saves to `%LOCALAPPDATA%\SunlitEarth\wallpaper.png`, and applies it with `SystemParametersInfoW`. PNG rather than TIFF because Windows preserves PNG wallpapers losslessly; TIFF wallpapers are JPEG-transcoded at 85% quality and band visibly in smooth gradients.

### Environment knobs

All `SUNLIT_EARTH_*` variables that carry a value go through `sunlit_core::env_override`, which treats unset and blank the same.

| Variable | Effect |
|---|---|
| `SUNLIT_EARTH_CLOUD_URL` | Overrides the cloud image URL. Wins over the quality tier. |
| `SUNLIT_EARTH_CLOUD_POLL_SECS` | Overrides the poll interval. |
| `SUNLIT_EARTH_CACHE_DIR` | Overrides the cloud cache directory. |
| `SUNLIT_EARTH_CONFIG` | Overrides the config file path. |
| `SUNLIT_EARTH_METRICS_DIR` | Overrides the memory metrics directory. |
| `SUNLIT_EARTH_TEXTURES` | Overrides the textures directory. |
| `SUNLIT_EARTH_NO_CLOUDS` | Presence-only: disables cloud fetching entirely. |
| `SUNLIT_EARTH_SYNC_LOG` | Presence-only: synchronous stderr logging (for e2e). |
| `SUNLIT_EARTH_UPDATE_GOLDEN` | Presence-only: regenerate golden references. |
| `SUNLIT_EARTH_CONTACT_SHEET` | Overrides where the contact sheet is written. |

### Notable dependencies

- `astronomy-engine-bindings`: C FFI bindings to the Astronomy Engine library (requires `clang` at build time for bindgen)
- `image`: PNG/JPEG encode and decode. `jxl-oxide`: the JPEG XL decoding hook
- `time`: UTC decomposition for astronomy
- `tracing` / `tracing-subscriber` / `tracing-appender`: `max_level_trace` with `release_max_level_warn`; `EnvFilter` respects `RUST_LOG`; non-blocking stderr writer with `FmtSpan::CLOSE`
- `ureq` (rustls): cloud fetching. `crossbeam-channel`: engine command and reply channels
- `interprocess`: local socket IPC. `single-instance`: the OS mutex (app only)
- `windows-sys`: Win32 FFI, `SystemParametersInfoW`, `EnumDisplayMonitors`, `GetMonitorInfoW`, `GetProcessMemoryInfo` in core; `AttachConsole` in the app

## Testing

### Layers

| Layer | Where | What |
|---|---|---|
| Unit + property | both crates | pure functions, `proptest` invariants |
| Engine integration | `sunlit-core/tests/engine.rs` | real engine, real GPU, headless |
| Soak | `sunlit-core/tests/soak.rs` | mock clock, fixture cloud, 14 simulated days |
| Golden images | `sunlit-core/tests/golden.rs` | fixed scenes, software adapter, perceptual tolerance |
| GPU shader | `sunlit-core/tests/{shading,render_pipeline}.rs` | real WGSL on the GPU |
| UI logic | `sunlit-app/tests/slint_ui.rs` | `i-slint-backend-testing` |
| Desktop e2e | `sunlit-app/tests/e2e.rs` | the real binary over IPC, `#[ignore]`d |

### Conventions

- **Test behavior, not constants.** Changing a preset or a default must not break a test.
- **Float comparisons**: `approx::assert_relative_eq!`. `tests/shading.rs` predates this and keeps its own GPU tolerance pattern.
- **GPU tests assert invariants** (monotonicity, bounds, visibility), not exact pixels, because of cross-adapter float variance.
- **One GPU device at a time.** Per-test device creation crashes on Windows. Shader tests share a device through `LazyLock<Mutex<GpuContext>>`; engine, soak, and golden tests each hold a `GPU_SERIAL` mutex for the lifetime of their engine.
- **Golden images** force the software adapter so a developer machine and a CI runner compare against the same references. Tolerance: mean channel difference under 2/255 and at most 1% of pixels differing by more than 24. A companion test asserts every pair of references is distinguishable, which is what stops the others from becoming vacuous.
- **Soak measurements** take their baseline after warm-up (the first cloud texture and wgpu's allocator pools are a one-off ~85 MiB); the assertion is on the remaining simulated days.

### Resource-flow rules (from the retrospective, section 8.2)

- Every queue crossing a thread boundary is bounded, latest-value, or unbounded with the reasoning written down at the declaration site. There are three crossings today:
  1. **Decoded textures** (`assets::mailbox`, core): latest-value, one slot per texture. This is the Phase 0 fix.
  2. **Preview frames** (`engine_client`, app): latest-value, one slot, with a single pending wake-up so the UI thread cannot accumulate frame buffers either.
  3. **Engine commands** (`engine::start`, both directions): unbounded, deliberately. The consumer is unconditional and runs at most 50 ms apart, the producers are human-rate, and the payloads carry no pixels (`command_payload_is_small` pins that). A bounded channel would either block the UI thread against a mid-export engine or drop an `UpdateParams` that might be the last one. The full argument is a comment on the channel itself; keep it honest if any of those premises change.
- Decoded pixel buffers are never parked in queues, caches, or long-lived structs.
- Every background producer names its consumer and the condition under which the consumer runs. If that condition is not "always", the design is wrong.

## Workflow

- Always run `cargo test` and `cargo clippy --all-targets` after making changes.
- Do not commit or push without explicit user approval. Wait for explicit confirmation that a change works before committing.
- Git worktrees go in `.worktrees/` at the repo root.
- Keep `docs/roadmap.md` up to date when implementing features.

## CI/CD

Two GitHub Actions workflows in `.github/workflows/`:

- **`ci.yml`**: on every push to `main` and every PR targeting `main`. One `test` job on Windows: `cargo test --locked` across the workspace, then uploads `target/contact-sheet.png` as an artifact. `fmt` is commented out pending a codebase-wide reformat (see `docs/notes.md`).
- **`release.yml`**: on semver tag pushes (`v[0-9]+.[0-9]+.[0-9]+`). Builds `cargo build --release --locked`, zips `target/release/sunlit-earth.exe`, and creates a GitHub Release.

Key details:

- LLVM 19 is pinned on all Windows jobs via `KyleMayes/install-llvm-action@v2`; `LIBCLANG_PATH` is set to `$LLVM_PATH/lib` so bindgen finds `libclang.dll`.
- `RUSTFLAGS: "-D warnings"` is commented out pending a lint cleanup.
- All `cargo` commands use `--locked`. `Cargo.lock` lives at the workspace root.
- GPU tests run on the software adapter on CI runners.
- Clippy runs locally only (its artifacts are incompatible with the test cache and force full recompilation).

## Key Constraints

- `unsafe_code = "deny"` in `[workspace.lints.rust]`. It is `deny` and not `forbid` because Slint macros need unsafe internally. `scene/sun.rs`, `wallpaper.rs`, `config.rs`, `memory.rs`, and `main.rs` have scoped `#[allow(unsafe_code)]` on individual FFI call sites with `// SAFETY:` comments.
- Slint is pinned to `~1.17` with no wgpu feature. The app does not share a device with Slint, so the wgpu version is independent of the Slint version.
- Render texture size is quantized to 64px boundaries to reduce GPU texture churn during resize, and then capped by the quality tier.
- Zoom is normalized (0.0 to 1.0) with exponential mapping: `distance = 1.5 * (80.0 / 1.5)^t`. Use `zoom_to_distance` / `distance_to_zoom` in `scene/camera.rs`.
- The grid texture uses 16x anisotropic filtering with trilinear mipmaps.
- WGSL `vec3<f32>` has 16-byte alignment, so `#[repr(C)]` structs need an explicit `_pad: f32` after every `[f32; 3]` field. `uniforms.rs` has a compile-time size assertion.
- LF line endings globally.
