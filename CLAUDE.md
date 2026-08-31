# CLAUDE.md

Guidance for Claude Code in this repository. This file is the map and the rules; the detail lives in `docs/` and is linked from the table below. When you learn something that belongs in a document, put it there and keep this file short.

## Project status

Early stage with frequent breaking changes. Windows is the platform that ships; Linux builds, tests, renders and sets a wallpaper per desktop; macOS builds, tests and renders headlessly and cannot set a wallpaper. Keep the docs current as the code changes, `docs/roadmap.md` included.

## Where to read

| Question | Read |
|---|---|
| How the crates, the engine thread, `SceneParams`, the renderer and the app fit together | `docs/architecture.md` |
| Shaders, draw order, the Sun, the Moon, the stars, the Milky Way, the clouds | `docs/rendering.md` |
| Test layers, conventions, golden images, the CI workflows | `docs/testing.md` |
| What each OS does, the per-OS implementations, the Linux wallpaper setter, e2e gating | `docs/platforms.md` |
| The icon bake and which surface consumes which raster | `docs/app-icon.md` |
| Running the e2e suite or a release build in a VM | `docs/vm-setup.md` (the guide), `docs/vm-internals.md` (how the xtask is built) |
| Why the architecture is headless first | `docs/retrospective-2026-08.md` section 7, before changing the engine or the crate split |
| What is planned and what is known to be broken | `docs/roadmap.md` |
| The research and plan behind a feature | `docs/plans/`, dated, one research and one plan document per feature |
| Prerequisites, textures, CLI flags, environment variables, where files are written | `README.md` |

## Commands

`cargo unit`, `cargo e2e` and `cargo xtask` are aliases in `.cargo/config.toml`.

### Build, test, lint

```bash
cargo build                        # debug; --release for LTO and stripped
cargo test                         # everything in the workspace
cargo unit                         # unit tests only (~1 s): no integration targets, no doc tests
cargo test -p sunlit-core --test engine    # one integration target; also soak, golden, shading, render_pipeline
cargo test -p sunlit-earth --test slint_ui
cargo clippy --all-targets         # pedantic on; not run in CI, so it is on you
cargo fmt --check                  # CI gate
cargo run                          # the app; --software-rendering, --quality <tier>, --texture-resolution <w>
cargo run -- render --output x.png --width 640 --height 360   # headless render, all three OSes
SUNLIT_EARTH_UPDATE_GOLDEN=1 cargo test -p sunlit-core --test golden   # regenerate goldens for this adapter
```

Building the Linux port from Windows goes through WSL with `CARGO_TARGET_DIR` pointed into the distribution, or the two builds fight over `target/`; the command is in README under "Linux from Windows (WSL)".

### The desktop e2e suite and the VMs

The e2e suite (`crates/sunlit-app/tests/e2e.rs`) opens real windows and sets a real wallpaper, so it is `#[ignore]`d and runs in one of two places. `cargo e2e` runs it on this desktop, which takes the screen over for a minute and skips the wallpaper case. `cargo xtask e2e --target <windows|linux>` boots a throwaway VM from a prebuilt image, copies the current binaries in, runs every case in the guest's own desktop session, pulls the results back to the image store, and destroys the VM. The Windows guest runs on Hyper-V, the Linux guest on QEMU; the Linux image carries four desktops and `--desktop <kde|gnome|xfce|cinnamon>` picks the session, KDE by default.

```bash
cargo xtask vm doctor                       # read-only: is this host set up? Run it first when a VM command fails
cargo xtask vm status                       # images and their age, running guests, disk footprint
cargo xtask e2e --target linux              # the suite in a fresh Linux guest; --desktop gnome for another session
cargo xtask e2e --target windows --keep     # leave the guest up afterwards to look at the aftermath
cargo xtask vm up linux [--desktop xfce]    # boot a guest with the current binaries in it, run nothing
cargo xtask vm ssh linux ["command"]        # a shell or one command in the running guest
cargo xtask vm view linux                   # its desktop (vmconnect for Hyper-V, VNC for QEMU)
cargo xtask vm down <image|all>             # end the guest; the image stays. Always do this when finished
```

Things that follow from how it is built:

- Only one guest runs at a time; a second boot is refused until `vm down`. Nothing runs in the background unasked: a guest exists during a run, after `--keep`, or after `vm up`.
- A guest is pristine on every boot. Nothing done inside one survives `vm down`; results come back on their own under the image store, and the run prints the path.
- Every guest's binaries are compiled on the operating system they are for, never cross-compiled: natively on a matching host, in WSL for the Linux guest on a Windows host, and in the `windows-builder` guest for the Windows guest on a Linux host. The last two are why the first `vm up` after a change takes minutes, and the builder-guest one needs that image built first.
- `vm setup` (elevated, once per machine) and `vm build-image <image>` (tens of minutes to an hour per image) are the rare commands; do not run them without asking. The Windows image is a 90-day evaluation and `vm status` shows its age.
- The guests have no OpenGL and no real GPU: the Windows job sets `SLINT_BACKEND=winit-software` and both render on a software adapter, so timings and pixels there are not those of a real desktop.

`docs/vm-setup.md` has the full guide and the troubleshooting list; `docs/vm-internals.md` has the design.

### Release builds

```bash
cargo xtask dist [--target <windows|linux|all>] [--keep] [--no-verify] [--no-cache] [--allow-expired-image] [--allow-dirty]
```

`dist` builds `sunlit-earth` in release mode inside a pristine builder guest from a `git archive` of `HEAD`, then boots the desktop guest of the same target to prove the bundle runs and finds its textures. Four to six minutes per target plus two boots; output under `target/dist/<target>/`. A dirty working tree is refused without `--allow-dirty`.

### Rare

`cargo xtask bake-icon` and `cargo xtask bake-stars` regenerate committed assets from their sources (`assets/icon/*.svg`, HYG v4.4); a test compares the committed output against a fresh bake, so they are only ever run after changing a source. `cargo llvm-cov --html` writes a coverage report under `target/llvm-cov/html/`.

## Workspace

```
crates/sunlit-core/   headless: engine thread, wgpu renderer, WGSL shaders, scene and astronomy,
                      assets (textures, cloud fetcher, star blob), config, memory. No Slint.
crates/sunlit-app/    Slint shell: window, tray, IPC, CLI. Package name sunlit-earth.
crates/xtask/         VM orchestration, release builds, the icon and star bakes
assets/               icon sources and bake, the Linux desktop entry
textures/             the four JXL assets, Git LFS; a checkout without the objects holds pointer files
vm/                   templates and guest scripts, one directory per image slug
docs/                 see the table above
```

## Architecture in brief

- Headless first. The engine runs to completion with no window; the settings window is one optional client. Hiding the window removes a client, it does not half-suspend the machinery.
- One engine thread (`sunlit_core::engine`) owns the wgpu device, the `Renderer`, the texture mailbox and the schedule. Clients send `EngineCommand`s and receive `EngineEvent`s. The loop blocks on the command channel with a 50 ms timeout and asks the injected `Clock` what is due; it never sleeps on wall time. `Clock`, `CloudSource` and `WallpaperSink` are injected, which is what makes the soak test and the fixture-driven engine tests possible.
- `SceneParams` is the single description of what to draw. Exactly two translation points: `ui_callbacks::read_params_from_window` / `apply_params_to_window` in the app, and `renderer::render_pass::write_uniforms` in core. `ParamsDigest` is the quantized dirty check; `datetime` is not in it, the derived sun direction is compared separately.
- Adding a shader parameter means: the `.slint` property and row, the `AppConfig` field, `SceneParams` and its `ParamsDigest`, `Uniforms`, and the WGSL. `params.rs` has a table-driven test that fails when a parameter does not change the digest.
- A setting that is not a shader parameter (`texture_resolution` is the example) gets its own `EngineCommand` and callback and stays out of `SceneParams`.
- Texture slots: grid 0, then one per file-backed path in order (day 1, night 2, moon 3, Milky Way 4), clouds last. `SlotLayout` is the one place that says so.
- Preview frames cross to the UI as RGBA pixel buffers through a latest-value mailbox, never as shared GPU textures. Slint has no wgpu feature and shares no device.
- Every queue that crosses a thread is bounded, latest-value, or unbounded with the reasoning at the declaration. Decoded pixel buffers are never parked in queues, caches or long-lived structs. Every background producer names its consumer and the condition under which it runs, and that condition is "always".
- `query-memory`'s single `SIGNAL:memory ...` IPC line is a parsing contract the e2e suite depends on; `memory-report` is a separate command for that reason.

## Key constraints

- `rust-toolchain.toml` pins `1.94.0`; the builder images install the same channel from it.
- `.cargo/config.toml` sets `+crt-static` for `x86_64-pc-windows-msvc`, so every Windows binary of this tree, e2e binaries included, needs no Visual C++ redistributable. `cargo xtask dist` proves it on the artifact.
- `unsafe_code = "deny"` workspace-wide. FFI call sites carry a scoped `#[allow(unsafe_code)]` and a `// SAFETY:` comment; follow that pattern for any new FFI.
- Slint is `~1.17` with no wgpu feature.
- One wgpu instance per process, ever: `wgpu_init::instance()`, enforced by `clippy.toml`'s `disallowed-methods`. Dropping the last instance `dlclose`s the Vulkan loader under Mesa's TLS destructors and kills the next thread to exit.
- One GPU device at a time in tests: shader tests share a `LazyLock<Mutex<GpuContext>>`, and the engine, soak and golden tests hold `GPU_SERIAL` for the lifetime of their engine. Per-test device creation crashes on Windows.
- WGSL `vec3<f32>` is 16-byte aligned: every `[f32; 3]` in `Uniforms` is followed by `_pad: f32`, and `uniforms.rs` asserts the struct size at compile time.
- Render texture size is quantized to 64 px and capped by the quality tier. Zoom is normalized 0 to 1 through `zoom_to_distance` / `distance_to_zoom` in `scene/camera.rs`.
- Every `SUNLIT_EARTH_*` variable that carries a value goes through `sunlit_core::env_override`, which treats blank as unset. The xtask and the e2e harness read theirs directly under the same rule. The tables are in README.
- CI sets `RUSTFLAGS: "-D warnings"`; a warning is a build failure there. LF line endings everywhere.

## Testing conventions

- Test behavior, not constants: changing a preset or a default must not break a test.
- Float comparisons use `approx::assert_relative_eq!`. GPU tests assert invariants (monotonicity, bounds, visibility), not exact pixels.
- Golden images run on the software adapter with per-adapter references under `tests/golden/<adapter>/`, listed in `GENERATED_ADAPTERS`; tolerance is a mean channel difference under 2/255 with at most 1% of pixels off by more than 24. A missing case fails; a companion test keeps every pair of references distinguishable.
- A test that needs the real 8K assets checks their size (LFS pointers exist) and skips with a printed reason without them. Everything else uses generated fixtures.
- The desktop e2e suite is `#[ignore]`d, not `cfg`-gated: it compiles on all three OSes and runs by hand, on the desktop or in a VM. Cases that need a tray, a wallpaper setter, or Win32 gate themselves at runtime and print why they skipped.
- `the_docs_spell_out_every_flag_dist_takes` in the xtask reads the `cargo xtask dist [...]` line in this file and in `docs/vm-setup.md`; keep it a complete usage line.

## Workflow

- Run `cargo test` and `cargo clippy --all-targets` after making changes.
- Do not commit or push without explicit user approval, and wait for confirmation that a change works before committing.
- Git worktrees go in `.worktrees/` at the repo root.
- When implementing a feature, update `docs/roadmap.md` and whichever of the documents above describes the area; measurements and reasoning go there, not here.
