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
cargo unit                         # Unit tests only (~1 s): skips the integration targets and doc tests
cargo test -p sunlit-core          # Core only
cargo test -p sunlit-core --test engine   # Engine integration tests
cargo test -p sunlit-core --test soak     # Mock-clock soak test (14 simulated days)
cargo test -p sunlit-core --test golden   # Golden images + contact sheet
cargo e2e                          # Desktop e2e suite, alias for `cargo test --test e2e -- --ignored --test-threads=1 --nocapture` (needs a real desktop and GPU)
cargo fmt --check                  # Format gate (CI runs this)
cargo clippy --all-targets         # Lint (pedantic enabled, see Cargo.toml for allows)
cargo run                          # Run the app
cargo run -- --software-rendering  # Force CPU rendering
cargo run -- --quality high        # Override the quality tier for one run
cargo run -- render --output x.png --width 640 --height 360   # Headless render, works on all three OSes
cargo llvm-cov --html              # HTML coverage report (target/llvm-cov/html/)

SUNLIT_EARTH_UPDATE_GOLDEN=1 cargo test -p sunlit-core --test golden  # Regenerate goldens for this machine's adapter
```

### VM orchestration (`cargo xtask`)

The desktop e2e suite runs in local VMs instead of taking over the developer's desktop. `docs/vm-setup.md` is the human guide; the design is in `docs/plans/2026-08-19-phase3-vm-orchestration-plan.md`.

```bash
cargo xtask vm doctor              # Unelevated, read-only: can this host run the VM suite?
cargo xtask vm setup               # The one command that changes the host. Elevated on Windows.
cargo xtask vm build-image <windows|linux>   # Packer, then a manifest. Tens of minutes.
cargo xtask vm up <target>         # An interactive guest, with the current binaries in it
cargo xtask vm ssh <target>        # A shell in the running guest
cargo xtask vm view <target>       # Its desktop (vmconnect for Hyper-V, VNC for QEMU)
cargo xtask vm smoke <target>      # Boot, run a trivial job through the guest contract, destroy
cargo xtask vm status              # Images, media, overlays, running VMs, disk footprint
cargo xtask vm destroy <windows|linux|all> [--purge]
cargo xtask e2e --target <host|windows|linux> [--keep] [--allow-expired-image]
```

`vm setup` never reboots or signs anyone out; it reports what needs one. `vm doctor` changes nothing. `e2e --target host` is what `cargo e2e` does, kept as one command so the manual real-GPU run and the VM runs are the same thing.

CI sets `RUSTFLAGS: "-D warnings"`, so a warning is a build failure there. `cargo clippy --all-targets` locally is what keeps that true; clippy is not run in CI because its artifacts do not share the test cache and would force a full recompile.

### Building on Linux from Windows

The Linux port is developed through WSL. Build into a Linux-native target
directory, or the Windows and Linux artifacts fight over `target/`:

```bash
wsl -d Ubuntu-22.04 -- bash -lc 'cd /mnt/c/path/to/sunlit-earth && CARGO_TARGET_DIR=$HOME/sunlit-target cargo test --workspace'
```

Build dependencies (Ubuntu): `build-essential pkg-config clang libclang-dev libfontconfig-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev mesa-vulkan-drivers xvfb`. `mesa-vulkan-drivers` supplies lavapipe, which is the software adapter the GPU tests use there.

That list is a superset of what `ci.yml` installs, on purpose: the GitHub runner image already carries `build-essential`, `pkg-config` and `clang`, so the workflow installs only the rest. A bare WSL install carries none of them.

Note that WSL's default adapter is a GL passthrough to the host GPU, not lavapipe; `--software-rendering` and `EngineConfig::headless` select lavapipe, which is what CI uses.

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
    xtask/           # developer tooling: VM orchestration for the desktop e2e suite
  textures/          # local 8K JXL assets, not part of the build
  vm/                # Packer templates and guest assets for the test VMs
    linux/           # Ubuntu 22.04, GNOME on Xorg, cloud-init seed
    windows/         # Windows 11 Enterprise eval, autounattend, bootstrap
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
- **The preview follows window visibility.** The app sends `SetPreviewEnabled(false)` on every hide and `(true)` on every show, so a hidden window costs no readback. The engine keeps rendering regardless (the wallpaper export depends on it); only delivery stops. Re-showing pays back an "owed" frame from the existing texture, since the dirty check would otherwise suppress a re-render and leave the window blank.
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
- `ipc.rs`: opt-in control channel over `interprocess` local sockets. Commands: `quit`, `show-window`, `hide-window`, `export-test`, `query-memory`, `set-wallpaper`. Fire-and-forget, with `SIGNAL:` lines on stdout as the reply channel. `export-test` and `query-memory` are answered on the listener thread, so they work while the event loop is idle.
- `tray.rs`: the procedurally generated 32x32 icon, the tray callback wiring, and single-instance enforcement. The tray icon itself is a `SystemTrayIcon` component in `ui/main.slint`, so Slint owns the platform integration.
- `session_end.rs`: Windows only. An invisible top-level window on its own thread that answers `WM_QUERYENDSESSION` and quits the event loop on `WM_ENDSESSION`, so a reboot does not have to wait for Windows to kill the process. winit handles neither message, so without this nothing in the app ever learned the session was ending. The decision is a pure function (`classify`), unit-tested everywhere; the Win32 window is tested by sending it both messages.
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

The e2e harness and the xtask read six more. They do not go through `env_override` (the xtask does not depend on `sunlit-core`), but they follow the same blank-is-unset rule.

| Variable | Effect |
|---|---|
| `SUNLIT_EARTH_BIN` | The app binary the e2e suite spawns. Falls back to the compile-time `CARGO_BIN_EXE` path, which is wrong inside a guest. |
| `SUNLIT_EARTH_E2E_FIXTURES` | The e2e fixtures directory, for the same reason. |
| `SUNLIT_EARTH_E2E_WALLPAPER` | Presence-only: lets `test_set_wallpaper` run. Only the generated Windows guest job sets it, because the case replaces the desktop wallpaper of whatever machine runs it. |
| `SUNLIT_EARTH_VM_DIR` | The image store. Defaults to `%LOCALAPPDATA%\SunlitEarth\vm` or `~/.local/share/SunlitEarth/vm`. |
| `SUNLIT_EARTH_VM_PROVIDER` | Overrides the provider matrix (`hyperv` or `qemu`), mostly to drive the Windows guest through QEMU on a Windows host. |
| `SUNLIT_EARTH_REPO` | The repository root, for running the xtask binary from outside its checkout. Defaults to the compile-time location of the crate. |

### Notable dependencies

- `astronomy-engine-bindings`: C FFI bindings to the Astronomy Engine library (requires `clang` at build time for bindgen)
- `image`: PNG/JPEG encode and decode. `jxl-oxide`: the JPEG XL decoding hook
- `time`: UTC decomposition for astronomy
- `tracing` / `tracing-subscriber` / `tracing-appender`: `max_level_trace` with `release_max_level_warn`; `EnvFilter` respects `RUST_LOG`; non-blocking stderr writer with `FmtSpan::CLOSE`
- `ureq` (rustls): cloud fetching. `crossbeam-channel`: engine command and reply channels
- `interprocess`: local socket IPC. `single-instance`: the OS mutex (app only)
- `windows-sys`: Win32 FFI, `SystemParametersInfoW`, `EnumDisplayMonitors`, `GetMonitorInfoW`, `GetProcessMemoryInfo` in core; `AttachConsole` in the app
- `mach2`: Mach FFI on macOS, for `task_info(TASK_VM_INFO)` in `memory.rs` and nothing else. Declarations only; the `unsafe` call site is ours

## Platform support

Windows is the platform that ships. Linux and macOS build, test, and render headlessly; what they do not do yet is set a wallpaper.

| | Windows | Linux | macOS |
|---|---|---|---|
| Build, unit, engine, GPU shader, soak | yes | yes (lavapipe) | yes (Metal) |
| Golden images | yes (`warp`) | yes (`lavapipe`) | yes (`metal`) |
| `render` subcommand | yes | yes | yes |
| Settings window | yes | untested | untested |
| Set the desktop wallpaper | yes | no | no |
| Desktop e2e (`tests/e2e.rs`) | yes, on the desktop (9 of 10 cases) or in a local VM (all 10) | yes, in a local VM (6 of 9 cases) | compiles, unrun |

Per-OS implementations live in four places, each behind a `cfg` and each documented where it sits:

- `memory::snapshot`: `GetProcessMemoryInfo`, `/proc/self/{status,smaps_rollup}`, `task_info(TASK_VM_INFO)`. Same `MemorySnapshot`, same CSV, so every memory assertion in the suite is live on all three.
- `engine::wallpaper_sink::SystemWallpaper`: off Windows, `check_supported` returns "not supported on this platform yet" before anything is rendered, `publish` returns the same string if it is reached anyway, and `target_size` returns a documented 2560x1440. Not a stub that pretends to succeed, and not a refusal that arrives after a full-resolution render and readback.
- `config::is_position_on_screen`: Win32 monitor enumeration on Windows; elsewhere a coordinate-range sanity check against X11's INT16 window-position range, which is the coarse portable half of the same question.
- `session_end::install`: the Win32 listener above on Windows; elsewhere it returns `None` and says so, because a Linux desktop asks over the session bus rather than with window messages.

The desktop e2e suite is `#[ignore]`d, not `cfg`-gated: it compiles on all three OSes (which is free coverage for the IPC and process plumbing) and never runs in CI, because hosted runners have no interactive desktop. It runs on the developer's desktop with `cargo e2e`, and in a local VM with `cargo xtask e2e --target <windows|linux>`; see `docs/vm-setup.md`.

Three cases inside it are gated at runtime rather than by `cfg`, following the same convention as `software_adapter_produces_correct_results`. The tray-start-hidden lifecycle and single-instance enforcement need a tray icon, which the app has on Windows only, and single-instance is additionally tray-mode-only in the product. `test_set_wallpaper` sets a real desktop wallpaper, so it is opt-in through `SUNLIT_EARTH_E2E_WALLPAPER`, which only the generated Windows guest job sets; it asks `SystemWallpaper::check_supported` for the capability itself and asserts that answer matches the platform, so a Windows regression fails rather than skips. All three print why they skipped. Two further cases pick their startup mode by the tray capability, running windowed where there is no tray, which tests the same thing minus the icon. macOS has no VM story: it stays on hosted runners.

The tenth case, `test_session_end_shuts_down_promptly`, is the one exception to "`#[ignore]`d, not `cfg`-gated": it delivers `WM_QUERYENDSESSION` and `WM_ENDSESSION` to the running binary, which are Win32 calls, so the body does not compile elsewhere. It asserts the app exits successfully and in under five seconds, which is the number Windows gives an application before it names it on the shutdown screen.

## Testing

### Layers

| Layer | Where | What | Runs on |
|---|---|---|---|
| Unit + property | both crates | pure functions, `proptest` invariants | all three |
| Engine integration | `sunlit-core/tests/engine.rs` | real engine, real GPU, headless | all three |
| Soak | `sunlit-core/tests/soak.rs` | mock clock, fixture cloud, 14 simulated days | all three |
| Golden images | `sunlit-core/tests/golden.rs` | fixed scenes, software adapter, perceptual tolerance | all three, per-adapter references |
| GPU shader | `sunlit-core/tests/{shading,render_pipeline}.rs` | real WGSL on the GPU | all three |
| UI logic | `sunlit-app/tests/slint_ui.rs` | `i-slint-backend-testing` | all three |
| Desktop e2e | `sunlit-app/tests/e2e.rs` | the real binary over IPC, `#[ignore]`d | built everywhere; `cargo e2e` on the desktop, `cargo xtask e2e --target <windows\|linux>` in a VM |
| VM orchestration | `crates/xtask/src/**` | pure decision logic against fabricated hosts, no VM | all three |

### Conventions

- **Test behavior, not constants.** Changing a preset or a default must not break a test.
- **Float comparisons**: `approx::assert_relative_eq!`. `tests/shading.rs` predates this and keeps its own GPU tolerance pattern.
- **GPU tests assert invariants** (monotonicity, bounds, visibility), not exact pixels, because of cross-adapter float variance.
- **One GPU device at a time.** Per-test device creation crashes on Windows. Shader tests share a device through `LazyLock<Mutex<GpuContext>>`; engine, soak, and golden tests each hold a `GPU_SERIAL` mutex for the lifetime of their engine.
- **One wgpu instance per process, ever.** `wgpu_init::instance()` holds it in a `OnceLock` and nothing else may call `wgpu::Instance::new`; `clippy.toml` enforces that through `disallowed-methods`, so a second call site has to allow the lint by name. An instance owns the loaded driver libraries, and dropping the last one `dlclose`s the Vulkan loader while Mesa's pthread TLS destructors still point into it, so the next thread to exit dies in `__nptl_deallocate_tsd`. That is not theoretical: it killed all 14 engine tests on lavapipe.
- **Golden images** force the software adapter where the platform has one, so a developer machine and a CI runner compare against the same references. References are per adapter (`tests/golden/warp/`, `lavapipe/`, `metal/`), keyed by `wgpu_init::adapter_key`; the reasoning and the measured cross-adapter deltas are on that function. Which adapters have a set is listed in the test's `GENERATED_ADAPTERS`, not inferred from the filesystem: an adapter on the list whose directory is missing fails, and only an adapter that has genuinely never been generated skips. A missing single case fails every run, with its render written under `CARGO_TARGET_TMPDIR` for review rather than into the tracked tree. Tolerance: mean channel difference under 2/255 and at most 1% of pixels differing by more than 24. A companion test asserts every pair of references is distinguishable, which is what stops the others from becoming vacuous.
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

Three GitHub Actions workflows in `.github/workflows/`:

- **`ci.yml`**: on every push to `main` and every PR regardless of its base branch (stacked PRs target other PR branches). A `fmt` job runs `cargo fmt --check` once on Ubuntu, and a `test` matrix runs `cargo test --locked -- --show-output` plus a headless `render` smoke test on `ubuntu-latest`, `windows-latest`, and `macos-latest`, uploading the contact sheet and the smoke render per OS.
- **`golden.yml`**: `workflow_dispatch` only. Pick an OS, run it, download the `golden-<adapter>` artifact, review the images, commit them. It regenerates, reads the adapter key from the test's marker line, runs the suite again without `SUNLIT_EARTH_UPDATE_GOLDEN` so the job verifies what it produced, and uploads only that adapter's directory. GitHub only registers dispatchable workflows from the default branch, so it is usable once merged; before that, regenerate locally on the adapter in question.
- **`release.yml`**: on semver tag pushes (`v[0-9]+.[0-9]+.[0-9]+`). Builds `cargo build --release --locked`, zips `target/release/sunlit-earth.exe`, and creates a GitHub Release. Windows only; cross-platform release artifacts are still a roadmap item.

Key details:

- `fail-fast: false` on the matrix. All three results, every time: cancelling macOS because Linux failed costs a round trip to learn something the same run already knew.
- Per-OS setup, all of it explicit rather than relied on from the runner image: LLVM 19 pinned on Windows via `KyleMayes/install-llvm-action@v2` with `LIBCLANG_PATH`; Slint's build dependencies plus `mesa-vulkan-drivers` and `xvfb` via apt on Ubuntu; Xcode's `libclang.dylib` located defensively on macOS.
- Linux tests run under `xvfb-run -a`. Nothing opens a window today, so this is for the first windowed test to arrive. It does propagate the exit status, so a failing suite still fails the job.
- The render smoke test is deliberately **not** wrapped in xvfb: `run_render` claims to need no window, and running it with no `DISPLAY` is what makes that a tested claim. It asserts the output is a 640x360 PNG by reading the IHDR header, not that the file is merely non-trivial in size.
- `--show-output` on the test step, because the numbers worth having from a CI run come from tests that pass: the golden suite's adapter key and the soak test's per-OS memory profile. libtest discards those otherwise.
- Both `ci.yml` and `golden.yml` declare `permissions: contents: read` at the top level. `release.yml` needs write and says so itself.
- Cache keys are `ci-linux` / `ci-windows` / `ci-macos`. The Windows one is spelled out rather than derived from the runner label so it kept the key it had before the matrix.
- The first run on a new PR builds cold (roughly 45 minutes on Windows) because Actions caches are scoped per merge ref. Later runs on the same PR restore it. That is expected, not a regression.
- All `cargo` commands use `--locked`. `Cargo.lock` lives at the workspace root.
- GPU tests run on the software adapter where the platform has one: WARP on Windows, lavapipe on Linux. macOS has no CPU adapter, so it falls back to the runner's paravirtual Metal GPU, which is a real one.
- Clippy runs locally only (its artifacts are incompatible with the test cache and force full recompilation). `-D warnings` in CI covers rustc's own lints.

## Key Constraints

- `unsafe_code = "deny"` in `[workspace.lints.rust]`. It is `deny` and not `forbid` because Slint macros need unsafe internally. `scene/sun.rs`, `wallpaper.rs`, `config.rs`, `memory.rs`, and `main.rs` have scoped `#[allow(unsafe_code)]` on individual FFI call sites with `// SAFETY:` comments. New FFI, on any platform, follows that pattern; the macOS `task_info` call in `memory.rs` is the most recent example.
- Slint is pinned to `~1.17` with no wgpu feature. The app does not share a device with Slint, so the wgpu version is independent of the Slint version.
- Render texture size is quantized to 64px boundaries to reduce GPU texture churn during resize, and then capped by the quality tier.
- Zoom is normalized (0.0 to 1.0) with exponential mapping: `distance = 1.5 * (80.0 / 1.5)^t`. Use `zoom_to_distance` / `distance_to_zoom` in `scene/camera.rs`.
- The grid texture uses 16x anisotropic filtering with trilinear mipmaps.
- WGSL `vec3<f32>` has 16-byte alignment, so `#[repr(C)]` structs need an explicit `_pad: f32` after every `[f32; 3]` field. `uniforms.rs` has a compile-time size assertion.
- LF line endings globally.
