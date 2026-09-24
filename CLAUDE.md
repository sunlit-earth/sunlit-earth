# CLAUDE.md

Guidance for Claude Code in this repository. This file is the map and the rules; the detail lives in `docs/` and is linked from the table below. When you learn something that belongs in a document, put it there and keep this file short.

## Project status

Early stage with frequent breaking changes. Windows is the platform that ships; Linux builds, tests, renders and sets a wallpaper per desktop; macOS has the same platform code and has never run on a Mac, so `docs/platforms.md` gives every macOS row the tier of evidence it is at and nothing there claims more. Keep the docs current as the code changes, `docs/roadmap.md` included.

## Where to read

| Question | Read |
|---|---|
| How the crates, the engine thread, `SceneParams`, the renderer and the app fit together | `docs/architecture.md` |
| Shaders, draw order, the Sun, the Moon, the stars, the Milky Way, the clouds | `docs/rendering.md` |
| Test layers, conventions, golden images, the CI workflows | `docs/testing.md` |
| What each OS does, the per-OS implementations, the Linux wallpaper setter, e2e gating | `docs/platforms.md` |
| The icon bake and which surface consumes which raster | `docs/app-icon.md` |
| Running the e2e suite or a release build in a VM | `docs/vm-setup.md` (the guide), `docs/vm-internals.md` (how the xtask is built) |
| Why the architecture is headless first | `docs/reviews/2026-08-15-retrospective.md` section 7, before changing the engine or the crate split |
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
cargo run -- displays              # the monitors this session has and the plan they come to; --out <dir> renders it
SUNLIT_EARTH_UPDATE_GOLDEN=1 cargo test -p sunlit-core --test golden   # regenerate goldens for this adapter
```

Building the Linux port from Windows goes through WSL with `CARGO_TARGET_DIR` pointed into the distribution, or `target/` ends up holding two platforms' worth of artifacts; `sunlit-app`'s build script refuses a directory another platform has claimed, and the command is in README under "Linux from Windows (WSL)".

### The desktop e2e suite and the VMs

The e2e suite (`crates/sunlit-app/tests/e2e.rs`) opens real windows and sets a real wallpaper, so it is `#[ignore]`d and runs in one of two places. `cargo e2e` runs it on this desktop, which takes the screen over for a minute and skips the wallpaper case. `cargo xtask e2e --target <windows|linux>` boots a throwaway VM from a prebuilt image, copies the current binaries in, runs every case in the guest's own desktop session, pulls the results back to the image store, and destroys the VM. The Windows guest runs on Hyper-V, the Linux guest on QEMU; the Linux image carries four desktops plus sway and i3, `--desktop <kde|gnome|xfce|cinnamon|sway|i3>` picks the session, KDE by default, and `--session-type wayland` puts KDE or GNOME on Wayland instead of X11; sway runs on Wayland and i3 on X11 without the flag.

```bash
cargo xtask vm doctor                       # read-only: is this host set up? Run it first when a VM command fails
cargo xtask vm status                       # images and their age, running guests, disk footprint
cargo xtask e2e --target linux              # the suite in a fresh Linux guest; --desktop gnome for another session
cargo xtask e2e --target windows --keep     # leave the guest up afterwards to look at the aftermath
cargo xtask vm up linux [--desktop xfce] [--screens 2]   # boot a guest with the current binaries in it, run nothing
cargo xtask e2e --target linux --desktop gnome --session-type wayland   # the suite as a Wayland client
cargo xtask vm ssh linux ["command"]        # a shell or one command in the running guest
cargo xtask vm view linux                   # its desktop (vmconnect for Hyper-V, VNC for QEMU)
cargo xtask vm stop windows-builder         # end a builder and keep its build directory; vm start resumes it
cargo xtask vm down <image|all>             # end the guest; the image stays. Always do this when finished
```

Things that follow from how it is built:

- One desktop guest runs at a time; a second boot is refused until `vm down`. A builder is exempt in both directions, so a compiler may run beside the guest it compiles for, and the boot prints what the two hold together. Nothing runs in the background unasked: a guest exists during a run, after `--keep`, after `vm up`, or stopped.
- A desktop guest is pristine on every boot. Nothing done inside one survives `vm down`; results come back on their own under the image store, and the run prints the path.
- A builder is the exception, and `vm stop` and `vm start` exist for that reason: a stopped builder keeps its overlay, so the cargo build directory in it makes the next build a link rather than a compile. It holds no memory while stopped, `vm status` counts its overlay, and `vm down <builder>` is what frees it.
- Every guest's binaries are compiled on the operating system they are for, never cross-compiled: natively on a matching host, in WSL for the Linux guest on a Windows host, and in the `windows-builder` guest for the Windows guest on a Linux host. The last two are why the first `vm up` after a change takes minutes, and the builder-guest one needs that image built first; a build leaves the builder stopped, so the next one is warm.
- `vm setup` (elevated, once per machine) and `vm build-image <image>` (tens of minutes to an hour per image) are the rare commands; do not run them without asking. The Windows image is a 90-day evaluation and `vm status` shows its age.
- `--screens <n>` gives the Linux guest that many screens, up to four, placed left to right at the console resolution; `vm view` then opens one VNC window per screen. QEMU only, so the Windows guest refuses it: its adapter has one head whichever hypervisor holds it. X11 only too, since the placement is `xrandr`.
- Every Linux boot checks that the session that came up is the one asked for, from `XDG_SESSION_TYPE` and `XDG_CURRENT_DESKTOP` in the guest's `session.env`, and stops naming both when it is not. Under Wayland `session.env` carries `WAYLAND_DISPLAY`, so the app runs as a Wayland client, and the layout change case skips because `xrandr` cannot move a Wayland session's outputs.
- The guests have no OpenGL and no real GPU: the Windows job sets `SLINT_BACKEND=winit-software` and both render on a software adapter, so timings and pixels there are not those of a real desktop.

`docs/vm-setup.md` has the full guide and the troubleshooting list; `docs/vm-internals.md` has the design.

### Release builds

```bash
cargo xtask dist [--target <windows|linux|all>] [--keep] [--no-verify] [--no-cache] [--allow-expired-image] [--allow-dirty]
cargo xtask bundle --platform <windows|linux|macos> --exe <path> [--out <dir>] [--verify]
```

`dist` builds `sunlit-earth` in release mode inside a pristine builder guest from a `git archive` of `HEAD`, then boots the desktop guest of the same target to prove the bundle runs and finds its textures. Four to six minutes per target plus two boots; output under `target/dist/<target>/`. A dirty working tree is refused without `--allow-dirty`.

`bundle` is the second half of that on its own: it wraps a binary somebody else already built in the archive its platform's users open, writes `build-info.json` beside it, and with `--verify` unpacks the archive and renders from it twice to prove the textures are found. No VM and no hypervisor, which is what lets the GitHub release runners call it; `dist` calls the same functions. `--verify` runs the binary, so it needs a host of the platform being bundled. macOS is a platform here and not a `dist` target, because there is no macOS guest to build one in.

```bash
cargo xtask manifests --version <version> --assets <dir> --out <dir>
cargo xtask verify-install --exe <path> [--work <dir>]
```

`manifests` hashes a published release's five archives (as `gh release download` leaves them) and writes the Scoop manifest, the Homebrew cask and the Homebrew formula under `--out`, laid out as `sunlit-earth/scoop-bucket` and `sunlit-earth/homebrew-tap`; `verify-install` makes `bundle --verify`'s two renders through an installed command. `package-managers.yml` runs both when a release is published and is the only writer of those two repositories; `docs/testing.md` describes it.

### Rare

`cargo xtask bake icon`, `cargo xtask bake stars` and `cargo xtask bake licenses` regenerate committed assets from their sources (`assets/icon/*.svg`, HYG v4.4, and the dependency tree read against the license corpus in `assets/licenses/`); a test compares each committed output against a fresh bake, so they are only ever run after changing a source. `bake licenses` writes `assets/third-party.md` and `assets/THIRD-PARTY-LICENSES.md` and is the one to rerun after a dependency changes; the second lands at the top level of a release archive rather than under `assets/`, and it fetches nothing, so a license the corpus has no `assets/licenses/<identifier>.txt` for fails the bake with the URL to fetch it from. `cargo llvm-cov --html` writes a coverage report under `target/llvm-cov/html/`.

`cargo xtask sweep` deletes build artifacts no recent build has used, which cargo never does itself: an age pass for what nothing has touched in a week, then a size pass that takes the oldest artifacts until the directory fits in 25 GiB. Neither reaches the incremental caches, so it reports what those hold instead. Needs `cargo install cargo-sweep`; `--dry-run` reports and deletes nothing.

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
- Adding a shader parameter is fourteen edits in seven files: the `.slint` property and its `SettingRow`; `apply_params_to_window` and `read_params_from_window`; the `AppConfig` field and its default; in `params.rs` the `SceneParams` field, `from_config`, `write_to_config` and one line of the `scene_digest!` list; one line of `uniforms.rs`'s `uniform_block!` list; `write_uniforms`; and in `sphere.wgsl` the `Uniforms` field with its offset comment and the use. Those two macro lists carry the rest: a `SceneParams` field in no `scene_digest!` group does not compile, and `ParamsDigest`, `digest()` and the mutation table the tests walk are all generated from that one entry.
- A setting that is not a shader parameter gets its own `EngineCommand` and callback and stays out of `SceneParams`: `texture_resolution` through `SetTextureResolution`, and the display mode and anchor screen through `SetDisplayPlan`.
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
- One GPU device at a time in tests: shader tests share a `LazyLock<Mutex<GpuContext>>`, the engine and soak targets hold `GPU_SERIAL` for the lifetime of their engine, and the golden target serializes on its own shared `EngineHandle` mutex. Per-test device creation crashes on Windows.
- WGSL `vec3<f32>` is 16-byte aligned: every `[f32; 3]` in `Uniforms` is followed by `_pad: f32`. `uniforms.rs` declares the block once as a list of Rust type and WGSL type, asserts the struct size at compile time, and has unit tests that parse `sphere.wgsl` and compare the two field lists name for name and offset for offset, so a field added on one side alone fails, a field appended into the trailing padding included.
- Render texture size is quantized to 64 px and capped by the quality tier. Zoom is normalized 0 to 1 through `zoom_to_distance` / `distance_to_zoom` in `scene/camera.rs`.
- Every `SUNLIT_EARTH_*` variable that carries a value goes through `sunlit_core::env_override`, which treats blank as unset. The xtask and the e2e harness read theirs directly under the same rule. The tables are in README.
- CI sets `RUSTFLAGS: "-D warnings"`; a warning is a build failure there. LF line endings everywhere.

## Testing conventions

- Test behavior, not constants: changing a preset or a default must not break a test.
- Float comparisons use `approx::assert_relative_eq!`. GPU tests assert invariants (monotonicity, bounds, visibility), not exact pixels.
- Golden images run on the software adapter with per-adapter references under `tests/golden/<adapter>/`, listed in `GENERATED_ADAPTERS`; tolerance is a mean channel difference under 2/255 with at most 1% of pixels off by more than 24. A missing case fails; a companion test keeps every pair of references distinguishable.
- A test that needs the real 8K assets checks their size (LFS pointers exist) and skips with a printed reason without them. Everything else uses generated fixtures.
- The desktop e2e suite is `#[ignore]`d, not `cfg`-gated: it compiles on all three OSes and runs by hand, on the desktop or in a VM. Cases that need a tray, a wallpaper setter, or Win32 gate themselves at runtime and print why they skipped.
- `the_docs_spell_out_every_flag_dist_takes` and its `bundle` twin in the xtask read the `cargo xtask dist [...]` line in this file and in `docs/vm-setup.md`, and the `cargo xtask bundle ...` line in this file; keep both complete usage lines.

## Workflow

- Run `cargo test` and `cargo clippy --all-targets` after making changes.
- Do not commit or push without explicit user approval, and wait for confirmation that a change works before committing.
- `README.md` is edited only with the user's explicit permission. Anything that would have gone there goes in `docs/` instead.
- Git worktrees go in `.worktrees/` at the repo root.
- When implementing a feature, update `docs/roadmap.md` and whichever of the documents above describes the area; measurements and reasoning go there, not here.
