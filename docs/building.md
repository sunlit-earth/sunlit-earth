# Building and developer commands

The [README](../README.md#build-from-source) covers a first build. This guide adds platform setup details and commands for development. See [assets.md](assets.md) for texture downloads and asset preparation, [testing.md](testing.md) for verification, and [vm-setup.md](vm-setup.md) for VM testing and release bundles.

## Toolchain and native dependencies

Install Rust through rustup. [rust-toolchain.toml](../rust-toolchain.toml) pins the compiler and includes rustfmt and Clippy; rustup selects it inside the checkout regardless of the host default. The VM builder images install the same channel so release builds record a consistent compiler version.

On Debian and Ubuntu, the full development setup is:

```bash
sudo apt-get install build-essential pkg-config clang libclang-dev \
  libfontconfig-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev \
  mesa-vulkan-drivers xvfb git-lfs
```

Clang and `libclang-dev` support bindgen, which generates Astronomy Engine bindings during the build. Fontconfig, XCB, and xkbcommon support the Slint UI. `mesa-vulkan-drivers` provides lavapipe for software rendering and GPU tests. `xvfb` provides the virtual display used by hosted CI; it is not needed for an ordinary desktop launch. CI may install a smaller package list because its runner image already includes compiler tools.

On Windows, use Rust's MSVC toolchain, Visual Studio Build Tools with the Desktop development with C++ workload, and LLVM. CI uses LLVM 19 and points `LIBCLANG_PATH` at its `lib` directory. Locally, point that variable at the directory containing `libclang.dll` if bindgen cannot find it.

On macOS, install Xcode command line tools. If bindgen cannot find `libclang.dylib`, locate it under the active developer directory reported by `xcode-select -p` and set `LIBCLANG_PATH` to its containing directory. With full Xcode selected, the following is a common location; check that it exists for your installation:

```bash
export LIBCLANG_PATH="$(xcode-select -p)/Toolchains/XcodeDefault.xctoolchain/usr/lib"
```

An `Unable to find libclang` error means bindgen cannot locate the Clang C API library. On Debian and Ubuntu, LLVM runtime packages alone are insufficient; install `libclang-dev`.

## Building through WSL

Install the Linux dependencies inside the distribution. When using a checkout under `/mnt/c/`, set `CARGO_TARGET_DIR` to a directory in the Linux filesystem before building. This keeps Windows and Linux build artifacts separate. The app's build script rejects a target directory claimed by another platform, but that check only runs after dependencies have begun compiling.

For example, from Windows, adapting the distribution name and checkout path:

```powershell
wsl -d Ubuntu-22.04 -- bash -lc 'cd /mnt/c/path/to/sunlit-earth && CARGO_TARGET_DIR=$HOME/sunlit-target cargo test --workspace'
```

WSL may expose a host GPU through a GL passthrough adapter. Install lavapipe for the software rendering path used by the headless tests; see [testing.md](testing.md) for adapter requirements and [roadmap.md](roadmap.md) for known WSL test issues.

## Local builds and launch overrides

Run these commands from the repository root:

```bash
cargo build                              # debug workspace build
cargo build --release                    # optimized build with LTO and stripped symbols
cargo run                                # launch the app
cargo run -- --software-rendering        # request software rendering
cargo run -- --quality high              # override the quality tier for this run
cargo run -- --texture-resolution 2048    # override texture width for this run
cargo run -- render --output earth.png --width 1920 --height 1080
```

See [usage.md](usage.md) for launch flags and display diagnostics. The normal build uses committed assets and does not require the asset preparation tools.

## Developer tooling

`cargo xtask` invokes the workspace's tool crate. Asset commands and their outputs are described in [assets.md](assets.md). The VM commands include:

```bash
cargo xtask vm doctor
cargo xtask vm setup
cargo xtask vm build-image <image>
cargo xtask e2e --target <host|windows|linux> [--keep] [--desktop <kde|gnome|xfce|cinnamon>] [--screens <n>]
cargo xtask dist [--target <windows|linux|all>] [--keep] [--no-verify] [--no-cache] [--allow-expired-image] [--allow-dirty]
cargo xtask bundle --platform <windows|linux|macos> --exe <path> [--out <dir>] [--verify]
```

`bundle` is the half of `dist` that wraps a binary somebody already built in the archive its platform's users open, without a VM and without a hypervisor, which is what the release runners call. It is also the only route to a macOS archive, since there is no macOS guest: on a Mac it assembles `Sunlit Earth.app`, ad-hoc signs it with `codesign` and zips it with `ditto`, plus a plain tarball beside it; on any other host it writes the tarball and says which of Apple's tools it wanted. `--verify` unpacks each archive and renders from it twice, so it needs a host of the platform being bundled. The archive names carry the architecture as well as the platform, `sunlit-earth-<version>-<platform>-<arch>` with `<arch>` being `x86_64` or `aarch64`, and both are read out of the binary's header rather than taken from a flag: a binary of another platform than `--platform`, or a universal Mach-O, is refused.

`vm doctor` inspects the host without changing it. `vm setup` prepares the host and requires elevation on Windows. Image names are `windows`, `linux`, `windows-builder`, and `linux-builder`. The VM lifecycle commands include `up`, `ssh`, `view`, `smoke`, `status`, `down`, and `purge`; use the [VM guide](vm-setup.md) for setup, operation, and cleanup. [vm-internals.md](vm-internals.md) explains the implementation.
