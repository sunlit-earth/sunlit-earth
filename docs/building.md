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
wsl -d Ubuntu-22.04 -- bash -lc 'cd /mnt/c/path/to/sunlit-earth/.worktrees/<worktree> && CARGO_TARGET_DIR=$HOME/sunlit-target-<worktree> cargo test --workspace'
```

WSL may expose a host GPU through a GL passthrough adapter. Install lavapipe for the software rendering path used by the headless tests; see [testing.md](testing.md) for adapter requirements and [roadmap.md](roadmap.md) for known WSL test issues.

### Keeping the distribution small

Cargo never deletes stale artifacts, and a Linux target directory of this tree grows to tens of gigabytes. The distribution's virtual disk only grows, so unbounded directories once took it to 185 GB. These rules keep the total near 40 GB.

- One target directory per worktree: `CARGO_TARGET_DIR=$HOME/sunlit-target-<worktree>`, named after the worktree directory (`main` for the main checkout) and reused for every build of that worktree. Do not invent other names. A single directory shared by all worktrees does not work (cargo issue 12516).
- The only other directory is `~/sunlit-target`, which `cargo xtask` builds the Linux guest's binaries into.
- Removing a worktree removes its target directory with it, from Windows:

```powershell
wsl -d Ubuntu-22.04 -- rm -rf /home/<user>/sunlit-target-<worktree>
```

- At the end of any session that built in WSL, bound the directories with cargo-sweep (`cargo install cargo-sweep`). It finds a target directory through `cargo metadata`, so point `CARGO_TARGET_DIR` at each one and give it any checkout. This caps every directory at 8 GB by deleting the oldest artifacts first:

```bash
for d in "$HOME"/sunlit-target-*; do [ -d "$d" ] || continue; CARGO_TARGET_DIR=$d cargo sweep --maxsize 8GB /mnt/c/path/to/sunlit-earth; done
[ -d "$HOME/sunlit-target" ] && CARGO_TARGET_DIR=$HOME/sunlit-target cargo sweep --maxsize 10GB /mnt/c/path/to/sunlit-earth
```

`--dry-run` reports without deleting, and `--time 7` removes what nothing has used for a week instead of capping by size. The flags are exclusive, so run them one at a time.
- Temporary files of any kind live only in the session's scratchpad or run directory, in `.worktrees/`, or in these named target directories, never loose in the distribution's home.

### Reclaiming the disk space

Deleting files inside the distribution frees space there but does not shrink `ext4.vhdx` on the host. This is an occasional manual step, needed after a large cleanup. It stops every WSL distribution, including podman's, and needs an elevated PowerShell:

```powershell
wsl -d Ubuntu-22.04 -u root fstrim -v /
wsl --shutdown
Get-ChildItem HKCU:\Software\Microsoft\Windows\CurrentVersion\Lxss | ForEach-Object { Get-ItemProperty $_.PSPath } | Select-Object DistributionName, BasePath
Optimize-VHD -Path <BasePath>\ext4.vhdx -Mode Full
(Get-Item <BasePath>\ext4.vhdx).Length / 1GB
```

Sparse VHD (`wsl --manage --set-sparse`) is not an option: WSL puts it behind `--allow-unsafe` because of data corruption risk.

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
