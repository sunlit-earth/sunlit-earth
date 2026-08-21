# Sunlit Earth

A desktop application that renders a realistic 3D view of Earth as seen from space and sets it as your wallpaper. The image reflects the current date and time, so the day/night terminator tracks the real sun, and a near real-time cloud layer is composited on top of NASA Blue Marble surface textures.

Unlike wallpaper tools that reuse geostationary satellite imagery, the camera can sit anywhere: centered on Europe, looking down on a pole, or recreating the framing of the original Blue Marble photograph.

Written in Rust, rendered with wgpu, with a Slint settings window and a system tray icon.

## Status

Early development, version 0.1.0. Frequent breaking changes. Windows is the platform that ships; Linux and macOS build, test, and render headlessly, but cannot set a wallpaper yet.

| | Windows | Linux | macOS |
|---|---|---|---|
| Build and full test suite | yes | yes (lavapipe) | yes (Metal) |
| `render` subcommand (headless PNG) | yes | yes | yes |
| Settings window | yes | untested | untested |
| Set the desktop wallpaper | yes | no | no |

Off Windows, asking for a wallpaper returns "not supported on this platform yet" in the status line instead of rendering something it cannot apply. The headless `render` subcommand is the cross-platform mode today. See [docs/roadmap.md](docs/roadmap.md) for what is planned.

## Prerequisites

A recent stable Rust toolchain. The workspace is on edition 2024 with resolver 3, so nothing older than Rust 1.85 will work at all, and individual dependencies may want newer. CI and local development both use current stable.

Git LFS, for the texture assets. See [Textures](#textures) below.

### Linux (Ubuntu / Debian)

```bash
sudo apt-get install build-essential pkg-config clang libclang-dev \
  libfontconfig-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev \
  mesa-vulkan-drivers xvfb git-lfs
```

`clang` and `libclang-dev` are for bindgen, which generates the Astronomy Engine FFI bindings at build time. The `libfontconfig` and `libxcb` packages are Slint's build dependencies. `mesa-vulkan-drivers` supplies lavapipe, the software Vulkan adapter that the GPU tests and `--software-rendering` use. `xvfb` is only needed if you want to run the suite exactly the way CI does; nothing in it opens a window today.

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

The app looks for `world.topo.200405.jxl` (day side) and `BlackMarble_2016.jxl` (night lights) in the textures directory, resolved in this order: the `--textures-dir` flag, `SUNLIT_EARTH_TEXTURES`, `textures/` relative to the current working directory, then `textures/` in a parent of the executable.

With no textures at all, the renderer falls back to a procedural grid, which is enough to check that the pipeline works. Leaving unfetched LFS pointer files in place is worse than having no textures: they look like assets and then fail to decode as JPEG XL. If you do not want the assets, point at an empty directory rather than at the pointer files.

The Rendering group in the settings window chooses the width they are loaded at: 8192, 4096, or 2048. The default is 4096, which halves the sources once and keeps the result in a cache beside the config, so later launches are quicker than the full width and cost about a quarter of the memory. Settings saved before this existed have no entry for it and land on 4096 too; pick 8192 once and it persists. The cache is disposable and can be deleted at any time.

Cloud imagery is downloaded at runtime from [clouds.matteason.co.uk](https://clouds.matteason.co.uk) and cached; nothing needs to be prepared up front. Set `SUNLIT_EARTH_NO_CLOUDS` to skip it entirely.

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

The main flags are `--mode <tray|window>`, `--tray-start <visible|hidden>`, `--quality <low|medium|high>`, `--texture-resolution <8192|4096|2048>`, `--software-rendering`, `--textures-dir`, `--log-level`, and `--ipc-socket`. `cargo run -- --help` has the full list.

## Where files are written

`config.toml`, the cloud cache, the downscaled surface textures in `texture_cache/`, the exported `wallpaper.png`, and the memory metrics CSV all live in the platform local data directory under `SunlitEarth`: `%LOCALAPPDATA%\SunlitEarth` on Windows, `~/.local/share/SunlitEarth` on Linux, `~/Library/Application Support/SunlitEarth` on macOS.

### Environment overrides

Every one of those locations can be moved, which is also how the tests stay out of your real settings. Variables that carry a value treat unset and blank the same.

| Variable | Effect |
|---|---|
| `SUNLIT_EARTH_CONFIG` | Path to the config file |
| `SUNLIT_EARTH_TEXTURES` | Textures directory |
| `SUNLIT_EARTH_CACHE_DIR` | Cache directory, for the cloud image and the downscaled textures |
| `SUNLIT_EARTH_METRICS_DIR` | Memory metrics directory |
| `SUNLIT_EARTH_CLOUD_URL` | Cloud image URL, winning over the quality tier |
| `SUNLIT_EARTH_CLOUD_POLL_SECS` | Cloud poll interval in seconds |
| `SUNLIT_EARTH_NO_CLOUDS` | Presence only: disable cloud fetching entirely |

Three more exist for the test suite rather than for running the app: `SUNLIT_EARTH_SYNC_LOG` (synchronous stderr logging, used by the e2e tests), `SUNLIT_EARTH_UPDATE_GOLDEN` (regenerate golden references), and `SUNLIT_EARTH_CONTACT_SHEET` (where the contact sheet is written).

## Tests

```bash
cargo test                 # everything
cargo unit                 # unit tests only, about a second
cargo clippy --all-targets # lint; pedantic is on
cargo fmt --check          # format gate, also run in CI
```

The suite includes real-GPU integration tests, a mock-clock soak test that simulates 14 days in about 13 seconds, and golden image comparisons against per-adapter references. Golden references are keyed by adapter (`warp` on Windows, `lavapipe` on Linux, `metal` on macOS), so the software adapter has to be available for those to run. The desktop end-to-end suite is `#[ignore]`d and run by hand on Windows with `cargo e2e`, since it needs a real interactive desktop.

CI runs the same suite on Ubuntu, Windows, and macOS with `RUSTFLAGS: "-D warnings"`, so a warning fails the build there.

## Repository layout

```
crates/sunlit-core/   headless engine: wgpu renderer, shaders, astronomy, assets, config
crates/sunlit-app/    Slint shell: window, tray, IPC, CLI (package name: sunlit-earth)
docs/                 vision, technical decisions, roadmap, retrospective, plans
textures/             8K JPEG XL assets, Git LFS
tools/                offline Python tools for preparing assets
```

The organizing principle is headless first: the engine runs to completion with no window at all, and the settings window is one optional client. [docs/README.md](docs/README.md) indexes the design documents; [docs/tech.md](docs/tech.md) covers the technology decisions and [docs/retrospective-2026-08.md](docs/retrospective-2026-08.md) explains why the architecture looks the way it does.

### Asset tools

Two standalone Python tools under `tools/`, both managed with [uv](https://docs.astral.sh/uv/) and not part of the Rust build:

- [`texture-pipeline`](tools/texture-pipeline/README.md) converts NASA Blue Marble sources into JPEG XL at several resolutions.
- [`cloud-fetch`](tools/cloud-fetch/README.md) fetches and processes satellite cloud imagery, from either the Matteason composite or NOAA GMGSI.

## Troubleshooting

`Unable to find libclang` during the build. Bindgen needs the Clang C API library to generate the Astronomy Engine bindings. On Debian and Ubuntu, install `libclang-dev`; on Windows, install LLVM and set `LIBCLANG_PATH`; on macOS, point `LIBCLANG_PATH` at the Xcode toolchain as shown above. The `libllvm*` runtime packages are not enough, the `libclang.so` from the development package is what is looked for.

The globe renders as a grid of lines. The textures were not found. Check that `git lfs pull` has actually replaced the files in `textures/` with multi-megabyte images. A debug build logs a `resolved texture paths` line at startup that says which paths it settled on; release builds compile info-level logging out, so run a debug build to see it.

`render` sits for two minutes and then writes an image anyway, with JPEG XL decode errors in the log. The files in `textures/` are LFS pointers rather than images, so they are found but never decode. Same fix as above.

## License

GPL-3.0-or-later, as declared in `Cargo.toml`. The full license text is not in the repository yet. Slint is used under its GPLv3 option, and Astronomy Engine is MIT.
