# Sunlit Earth

A desktop application that renders a realistic 3D view of Earth as seen from space and sets it as your wallpaper. The image reflects the current date and time, so the day/night terminator tracks the real sun, and a near real-time cloud layer is composited on top of NASA Blue Marble surface textures.

Unlike wallpaper tools that reuse geostationary satellite imagery, the camera can sit anywhere: centered on Europe, looking down on a pole, or recreating the framing of the original Blue Marble photograph.

Written in Rust, rendered with wgpu, with a Slint settings window and a system tray icon.

## Status

Early development, version 0.1.0. Frequent breaking changes. Windows is the platform that ships. Linux builds, tests, renders headlessly, sets a wallpaper on the desktops listed below, and exits cleanly when its session ends. macOS builds, tests, and renders headlessly, and cannot set a wallpaper yet.

| | Windows | Linux | macOS |
|---|---|---|---|
| Build and full test suite | yes | yes (lavapipe) | yes (Metal) |
| `render` subcommand (headless PNG) | yes | yes | yes |
| Settings window | yes | yes, in the test VM | untested |
| Set the desktop wallpaper | yes | yes, per desktop | no |
| Clean exit when the session ends | yes | yes (SIGTERM) | no |

On Linux the wallpaper setter is chosen by `XDG_CURRENT_DESKTOP`: GNOME, KDE Plasma, XFCE and Cinnamon have been run; MATE, LXQt and Budgie have a setter written from their documentation and never run. On macOS asking for a wallpaper returns "not supported on this platform yet" in the status line instead of rendering something it cannot apply; the headless `render` subcommand is the mode there. [docs/platforms.md](docs/platforms.md) has the per-OS detail and [docs/roadmap.md](docs/roadmap.md) what is planned.

## Prerequisites

Rust, through rustup: `rust-toolchain.toml` pins the channel (currently `1.94.0`, with rustfmt and clippy), and rustup selects it inside the checkout whatever the host's default is. A release build has to be able to say which compiler made it, and the VM builder images install the same channel from that file.

Git LFS, for the texture assets. See [Textures](#textures) below.

### Linux (Ubuntu / Debian)

```bash
sudo apt-get install build-essential pkg-config clang libclang-dev \
  libfontconfig-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev \
  mesa-vulkan-drivers xvfb git-lfs
```

`clang` and `libclang-dev` are for bindgen, which generates the Astronomy Engine FFI bindings at build time. The `libfontconfig` and `libxcb` packages are Slint's build dependencies. `mesa-vulkan-drivers` supplies lavapipe, the software Vulkan adapter that the GPU tests and `--software-rendering` use. `xvfb` is only needed if you want to run the suite exactly the way CI does; nothing in it opens a window today.

That list is a superset of what `ci.yml` installs, on purpose: the GitHub runner image already carries `build-essential`, `pkg-config` and `clang`, so the workflow installs only the rest. A bare install carries none of them.

### Linux from Windows (WSL)

The Linux port is developed through WSL. Build into a Linux-native target directory, or the Windows and Linux artifacts fight over `target/`:

```bash
wsl -d Ubuntu-22.04 -- bash -lc 'cd /mnt/c/path/to/sunlit-earth && CARGO_TARGET_DIR=$HOME/sunlit-target cargo test --workspace'
```

Install the Ubuntu package list above inside the distribution first. WSL's default adapter is a GL passthrough to the host GPU rather than lavapipe; `--software-rendering` and the headless tests select lavapipe, which is what CI uses.

### Windows

Rust with the MSVC toolchain, plus an LLVM installation for bindgen. CI installs LLVM 19 and sets `LIBCLANG_PATH` to its `lib` directory; do the same locally if `libclang.dll` is not already on the search path.

### macOS

Xcode command line tools provide both the compiler and a `libclang.dylib`. If bindgen cannot find it, point at it explicitly:

```bash
export LIBCLANG_PATH="$(xcode-select -p)/Toolchains/XcodeDefault.xctoolchain/usr/lib"
```

## Textures

The 8K JPEG XL surface textures live in `textures/` and are stored with Git LFS, so a plain `git clone` leaves small pointer files in their place. Fetch the real files:

```bash
git lfs install
git lfs pull
```

The app looks for four files in the textures directory: `world.topo.200405.jxl` (day side), `BlackMarble_2016.jxl` (night lights), `lroc_color_poles_1k.jxl` (the Moon's surface) and `milkyway_2020_4k.jxl` (the Milky Way panorama); `textures/PROVENANCE.md` records where each came from. The directory is resolved in this order: the `--textures-dir` flag, `SUNLIT_EARTH_TEXTURES`, `textures/` relative to the current working directory, then `textures/` in a parent of the executable. The Moon and the Milky Way are overlays: without their files the sky simply has no Moon or no band, and nothing waits for them.

With no textures at all, the renderer falls back to a procedural grid, which is enough to check that the pipeline works. Leaving unfetched LFS pointer files in place is worse than having no textures: they look like assets and then fail to decode as JPEG XL. If you do not want the assets, point at an empty directory rather than at the pointer files.

The Rendering group in the settings window chooses the width they are loaded at: 8192, 4096, or 2048. The default is 4096, which halves the sources once and keeps the result in a cache beside the config, so later launches are quicker than the full width and cost about a quarter of the memory. Settings saved before this existed have no entry for it and land on 4096 too; pick 8192 once and it persists. The cache is disposable and can be deleted at any time.

Cloud imagery is downloaded at runtime from [clouds.matteason.co.uk](https://clouds.matteason.co.uk) and cached; nothing needs to be prepared up front. Set `SUNLIT_EARTH_NO_CLOUDS` to skip it entirely. The same Rendering setting picks which of the three published cloud sizes is downloaded, so lowering the width lowers the download and the largest texture the app holds along with the two surface ones. Changing it does not blank the clouds while the new image arrives: the one on screen stays until the download lands, and if the network is down it stays until it comes back.

## Build and run

```bash
cargo build                        # debug build of the whole workspace
cargo build --release              # release build (LTO, stripped)
cargo run                          # run the app
cargo run -- --software-rendering  # force CPU rendering
cargo run -- --quality high        # override the quality tier for one run
cargo run -- --texture-resolution 2048   # load the surface textures narrower, for one run
```

Render a single frame without opening a window, on any of the three platforms:

```bash
cargo run -- render --output earth.png --width 1920 --height 1080
```

Print the screens this session has and the wallpaper plan they come to:

```bash
cargo run -- displays                  # the layout, the anchor screen, and every image the plan would render
cargo run -- displays --out ./plan     # and write those images into a directory, leaving the desktop alone
```

Without `--out` it creates no GPU device and touches nothing, so it is the cheap way to see what the app makes of an unfamiliar layout, and the useful thing to paste into a bug report.

The main flags are `--mode <tray|window>`, `--tray-start <visible|hidden>`, `--quality <low|medium|high>`, `--texture-resolution <8192|4096|2048>`, `--software-rendering`, `--textures-dir`, `--log-level`, and `--ipc-socket`. `cargo run -- --help` has the full list.

### Several screens

The wallpaper is one image per monitor at that monitor's own resolution, and the Displays group in the settings window says how the screens relate. It offers three modes: **One screen** paints one of them and leaves the rest alone, **Same view on every screen** (the default) gives each its own render at its own size and aspect ratio, and **One view across all screens** renders one continuous view over the whole desktop and cuts it up. Below the modes is the screen the plan is anchored to, which is the one that gets the picture in one-screen mode and the one the view is centred on in the other two; leave it on **Primary (automatic)** to follow whatever the system calls primary. A diagram of the layout sits under both, with the anchor highlighted. The group is hidden on a session with one screen, where all three modes are the same thing, and both settings are stored in `config.toml` as `display_mode` and `anchor_monitor`.

What a desktop can do with the plan differs. Windows and XFCE address each monitor individually; GNOME, Cinnamon and MATE hold one image and can stretch it across everything; KDE Plasma and LXQt hold one image for all screens and get the anchor's. Where a desktop cannot do what the mode asked, the app does the nearest thing and the status line says which. [docs/platforms.md](docs/platforms.md) has the table.

### Developer tooling

`cargo xtask` is the workspace's own tool crate. It bakes the committed assets, runs the desktop e2e suite in local VMs, and builds the release binaries in a pristine guest:

```bash
cargo xtask bake-icon              # rasterize the icon SVGs into assets/icon/baked/
cargo xtask bake-icon --review DIR # the small-size contact sheet, for judging 16 and 24 px by eye
cargo xtask bake-stars --input hyg_v44.csv --output crates/sunlit-core/src/assets/stars/hyg_v4_4_mag7.bin
cargo xtask vm doctor              # can this host run the VM suite? Read-only.
cargo xtask vm setup               # the one command that changes the host; elevated on Windows
cargo xtask vm build-image <image> # windows | linux | windows-builder | linux-builder
cargo xtask vm up|ssh|view|smoke|status|down|purge ...
cargo xtask e2e --target <host|windows|linux> [--keep] [--desktop <kde|gnome|xfce|cinnamon>] [--screens <n>]
cargo xtask dist [--target <windows|linux|all>] [--keep] [--no-verify] [--no-cache] [--allow-expired-image] [--allow-dirty]
cargo llvm-cov --html              # HTML coverage report under target/llvm-cov/html/
```

[docs/vm-setup.md](docs/vm-setup.md) is the guide to the VM commands, from host setup to release builds and cleanup; [docs/vm-internals.md](docs/vm-internals.md) is how they are built.

## Where files are written

`config.toml`, the cloud cache, the downscaled surface textures in `texture_cache/`, the exported wallpaper, and the memory metrics CSV all live in the platform local data directory under `SunlitEarth`: `%LOCALAPPDATA%\SunlitEarth` on Windows, `~/.local/share/SunlitEarth` on Linux, `~/Library/Application Support/SunlitEarth` on macOS.

The wallpaper is one file per screen, `wallpaper-<slot>-<index>.png`, plus `wallpaper-<slot>-canvas.png` where one view is spread across every screen. There are two slots and each publish writes whichever slot the desktop is not currently showing, emptying it first. A Linux desktop shell keys the wallpaper it has loaded on the path it was handed, so a new image at the path already in that setting is one nothing reloads; alternating means every publish hands over a path the desktop has to read.

### Environment overrides

Every one of those locations can be moved, which is also how the tests stay out of your real settings. Variables that carry a value treat unset and blank the same.

| Variable | Effect |
|---|---|
| `SUNLIT_EARTH_CONFIG` | Path to the config file |
| `SUNLIT_EARTH_TEXTURES` | Textures directory |
| `SUNLIT_EARTH_CACHE_DIR` | Cache directory, for the cloud image and the downscaled textures |
| `SUNLIT_EARTH_METRICS_DIR` | Memory metrics directory |
| `SUNLIT_EARTH_CLOUD_URL` | Cloud image URL, winning over the size the texture resolution picks |
| `SUNLIT_EARTH_CLOUD_POLL_SECS` | Cloud poll interval in seconds |
| `SUNLIT_EARTH_NO_CLOUDS` | Presence only: disable cloud fetching entirely |

Three more exist for the test suite rather than for running the app: `SUNLIT_EARTH_SYNC_LOG` (presence only: synchronous stderr logging, used by the e2e tests), `SUNLIT_EARTH_UPDATE_GOLDEN` (presence only: regenerate golden references), and `SUNLIT_EARTH_CONTACT_SHEET` (where the contact sheet is written).

The e2e harness and `cargo xtask` read a further set. They follow the same blank-is-unset rule.

| Variable | Effect |
|---|---|
| `SUNLIT_EARTH_BIN` | The app binary the e2e suite spawns. Falls back to the compile-time `CARGO_BIN_EXE` path, which is wrong inside a guest |
| `SUNLIT_EARTH_E2E_FIXTURES` | The e2e fixtures directory, for the same reason |
| `SUNLIT_EARTH_E2E_WALLPAPER` | Presence only: lets `test_set_wallpaper` run. Only the generated guest jobs set it, because the case replaces the desktop wallpaper of whatever machine runs it |
| `SUNLIT_EARTH_VM_DIR` | The VM image store. Defaults to `%LOCALAPPDATA%\SunlitEarth\vm` or `~/.local/share/SunlitEarth/vm` |
| `SUNLIT_EARTH_VM_PROVIDER` | Overrides the provider matrix (`hyperv` or `qemu`), mostly to drive the Windows guest through QEMU on a Windows host. Refused for a layer image |
| `SUNLIT_EARTH_VM_RESOLUTION` | Either guest console's resolution as `WxH`. Unset means the largest Hyper-V console mode that fits the host's screen, and 1920x1080 for a QEMU guest |
| `SUNLIT_EARTH_REPO` | The repository root, for running the xtask binary from outside its checkout. Defaults to the compile-time location of the crate |

## Tests

```bash
cargo test                 # everything
cargo unit                 # unit tests only, about a second
cargo clippy --all-targets # lint; pedantic is on
cargo fmt --check          # format gate, also run in CI
```

The suite includes real-GPU integration tests, a mock-clock soak test that simulates 14 days in about 13 seconds, and golden image comparisons against per-adapter references. Golden references are keyed by adapter (`warp` on Windows, `lavapipe` on Linux, `metal` on macOS), so the software adapter has to be available for those to run. The desktop end-to-end suite is `#[ignore]`d because it needs a real interactive desktop: `cargo e2e` runs it on this desktop, and `cargo xtask e2e --target <windows|linux>` runs it in a local VM, which is where all of its cases pass. [docs/testing.md](docs/testing.md) has the layers and the conventions.

Hosted CI (`ci.yml`) is dispatched by hand rather than run on every push, since the repository's Actions minutes are billed and the local VM suite covers what the runners were for. A dispatch runs the suite on Ubuntu, Windows, and macOS with `RUSTFLAGS: "-D warnings"`, so a warning fails the build there.

## Repository layout

```
crates/sunlit-core/   headless engine: wgpu renderer, shaders, astronomy, assets, config
crates/sunlit-app/    Slint shell: window, tray, IPC, CLI (package name: sunlit-earth)
crates/xtask/         developer tooling: VM orchestration, release builds, the asset bakes
assets/icon/          the app icon: SVG master, size variants, and the committed bake
assets/linux/         the desktop entry and the user-local install script
docs/                 architecture, rendering, testing, platforms, roadmap, plans
textures/             JPEG XL assets (Earth day and night, Moon, Milky Way), Git LFS
tools/                offline Python tools for preparing assets
vm/                   VM templates and guest scripts, one directory per image
```

The organizing principle is headless first: the engine runs to completion with no window at all, and the settings window is one optional client. [docs/README.md](docs/README.md) indexes the documentation. [docs/architecture.md](docs/architecture.md) is the engine, the parameters and the app; [docs/rendering.md](docs/rendering.md) is the shaders and every layer of the sky; [docs/retrospective-2026-08.md](docs/retrospective-2026-08.md) explains why the architecture looks the way it does.

### Asset tools

Two standalone Python tools under `tools/`, both managed with [uv](https://docs.astral.sh/uv/) and not part of the Rust build:

- [`texture-pipeline`](tools/texture-pipeline/README.md) converts NASA Blue Marble sources into JPEG XL at several resolutions.
- [`cloud-fetch`](tools/cloud-fetch/README.md) fetches and processes satellite cloud imagery, from either the Matteason composite or NOAA GMGSI.

## Troubleshooting

`Unable to find libclang` during the build. Bindgen needs the Clang C API library to generate the Astronomy Engine bindings. On Debian and Ubuntu, install `libclang-dev`; on Windows, install LLVM and set `LIBCLANG_PATH`; on macOS, point `LIBCLANG_PATH` at the Xcode toolchain as shown above. The `libllvm*` runtime packages are not enough, the `libclang.so` from the development package is what is looked for.

The globe renders as a grid of lines. The textures were not found. Check that `git lfs pull` has actually replaced the files in `textures/` with multi-megabyte images. A debug build logs a `resolved texture paths` line at startup that says which paths it settled on; release builds compile info-level logging out, so run a debug build to see it.

`render` sits for two minutes and then writes an image anyway, with JPEG XL decode errors in the log. The files in `textures/` are LFS pointers rather than images, so they are found but never decode. Same fix as above.

## License

GPL-3.0-or-later, as declared in `Cargo.toml`; the full text is in [LICENSE](LICENSE). Slint is used under its GPLv3 option, and Astronomy Engine is MIT. The star catalog's attribution is in `crates/sunlit-core/src/assets/stars/ATTRIBUTION.md` and the imagery's in `textures/PROVENANCE.md`.
