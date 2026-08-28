# VM release builds: builder images and `cargo xtask dist`

Written 2026-08-28, revised the same day after review. Follows the phase 3 VM orchestration plan (`2026-08-19-phase3-vm-orchestration-plan.md`), its Hyper-V amendment (`2026-08-20-phase3-amendment-hyperv-windows-build.md`) and the phase 5 Linux guest plan (`2026-08-21-phase5-linux-vm-and-parity-plan.md`). Intended for its own branch, `feat/vm-release-build`.

## Context

There are two ways to get a release binary today, and neither is a clean one. `release.yml` builds Windows only, on a hosted runner that is billed and dispatched by hand. A local `cargo build --release` builds whatever this host's environment makes of the tree: the toolchain `rustup` happens to default to, the LLVM on `PATH`, the Visual Studio that is installed, `RUSTFLAGS` if any are set, and a `target/` directory that has seen every branch this checkout was ever on. Nothing records which of those a given binary came from.

The VM orchestration boots pristine overlays of two golden images on demand, reaches the network from inside them (the Hyper-V guest through the `Default Switch` NAT, the QEMU guest through user-mode networking; the Windows bootstrap already downloads the Visual C++ runtime that way), and runs a job through a contract that survives a dropped connection. What the images do not have is a compiler. Phase 3's decision 8 built the e2e binaries on the host and copied them in, which was right for a debug build of a test harness and is wrong for a release: the point of a release build in a guest is that the host is not in it.

Two things the review settled that the first draft had wrong. The Linux desktop image is Debian 13, glibc 2.41, and a binary linked there runs on nothing older, Ubuntu 24.04 LTS included; the floor a binary carries is the glibc of the machine that linked it, so the builder has to be an older userland than the users'. And a compiler in the e2e images would cost the fidelity that found the vcruntime bug: the guest that found it was a stock Windows, and a guest with Build Tools on it would not find the next bug of that class. So the builders are their own images, and the desktop images stay as they are.

Facts measured for this plan:

- The images: Windows `golden.qcow2` 14.9 GiB and `golden.vhdx` 19.3 GiB on a 64 GiB virtual disk; Linux `golden.qcow2` 4.0 GiB on a 32 GiB one. Guests get 4 vCPUs and 6 GiB (Windows) or 4 GiB (Linux) from `qemu::resources_for`, which the Hyper-V create script agrees with by test.
- The WSL-built Linux binary (Ubuntu 22.04) has four `NEEDED` libraries: `libc`, `libm`, `libgcc_s` and `libfontconfig.so.1`. X11, xcb, xkbcommon and EGL are `dlopen`ed by winit and glutin and are not link-time dependencies. Its highest imported symbol version is `GLIBC_2.35`, the glibc it was built on. Anything built on glibc 2.34 or later imports `GLIBC_2.34`, where libpthread merged into libc, so a floor below that means a pre-2.34 builder, and the only supported one left is AlmaLinux 8.
- Ubuntu 22.04's clang 14 already builds this workspace through bindgen: it is the WSL builder the e2e suite uses.
- The host toolchain is `rustc 1.94.0` stable through `rustup 1.28.2`, and the repository has no `rust-toolchain.toml`. Every dependency in `Cargo.lock` is from crates.io.
- `git archive --format=tar HEAD ':!textures'` is 8.5 MB uncompressed and 378 entries. Nothing under `textures/` is embedded in the binary: what `include_bytes!` and `include_str!` pull in is the star catalog under `crates/sunlit-core/src/assets/stars/`, the icon rasters under `assets/icon/baked/`, and the shaders, all of which are in the archive. The JXL maps are read from disk at runtime.
- The Windows guest's job runs through the `sunlit-e2e-job` scheduled task, whose `ExecutionTimeLimit` is three hours. That is the ceiling on any job there.
- Windows has no glibc problem of its own: Rust's standard library since 1.78 requires Windows 10 or later and imports only from DLLs every Windows 10 ships. What it has is the C runtime: an MSVC binary links `vcruntime140.dll` dynamically, a clean Windows does not carry it, and the redistributable on the target has to be at least as new as the toolset that linked the binary. `-C target-feature=+crt-static` links the runtime in, which is the normal supported configuration on Windows, and the `cc` crate follows it (`/MT`) for the Astronomy Engine's C code.

## Goals

1. `cargo xtask dist --target <windows|linux|all>` builds the `sunlit-earth` binary in release inside a pristine overlay of that target's builder image, from the committed tree, with a toolchain the repository pins, then proves the result runs in that target's desktop image, and copies the binary with a build record into `target/dist/<target>/`.
2. A Linux release binary runs on any distribution with glibc 2.35 or newer and fontconfig installed, which is Ubuntu 22.04 and everything since.
3. A Windows release binary depends on nothing but Windows 10 or later: no Visual C++ redistributable.
4. Nothing of the host reaches the build except the source archive and the pinned toolchain name. No host `target/`, no host `~/.cargo`, no host environment.
5. The two desktop images are untouched and not rebuilt. The e2e path is unchanged: it still builds on the host, in WSL for the Linux guest, because that is the fast path for iterating on a test.

## Non-goals

- Publishing. No tags, no zips, no GitHub release; `release.yml` stays as it is. This produces the binary a release would be made from.
- A host target. `cargo build --release` already does that, and the point of this command is the environment it does not run in.
- Moving the e2e suite's own build into a guest. It could now, and it would remove the WSL dependency, but it would turn every `vm up` into a cold release-grade compile.
- The Windows builder under the QEMU provider override (`SUNLIT_EARTH_VM_PROVIDER=qemu` on a Windows host). See decision 3 for why a layer has one format per host.
- A Windows Server Core builder. It would be about 7 GB smaller than a Windows 11 install and its evaluation runs 180 days, at the cost of a second ISO, a second unattend file and an install path nobody has measured. Recorded here as the option to take if the layer turns out heavier than expected.
- macOS. There is no macOS guest.
- Vendoring crates. The guest downloads them, verified against the checksums in `Cargo.lock`.

## Decisions

1. **Four images, described by one model.** The store grows from "an image per target" to "an image per slug", and an image is either a base, installed from media, or a layer, provisioned over a named parent:

   | slug | kind | OS | what it is for |
   |---|---|---|---|
   | `windows` | base | Windows 11 Enterprise evaluation | the e2e desktop guest, unchanged |
   | `linux` | base | Debian 13, four desktops | the e2e desktop guest, unchanged |
   | `windows-builder` | layer over `windows` | the same Windows plus MSVC, libclang, rustup | release builds |
   | `linux-builder` | base | Ubuntu 22.04 cloud image, no graphics stack | release builds |

   In the code this is an `Image` enum beside `Target`: `Image::slug`, `Image::target` (the OS, which is what the provider matrix, the guest root, the job scripts and the SSH account key on), `Image::parent` (`Some(Windows)` for the layer, `None` for the bases), `Image::template_dir` (`vm/<slug>/`), `Image::vm_name`, `Image::has_eval_expiry` (the Windows base and its layer), `Image::ALL`. `Store::image_dir`, `qcow2`, `vhdx`, `manifest`, `run_dir`, `overlay`, `state_file`, `build_dir`, `results_dir` take an `Image`. The two existing slugs keep their paths exactly, so the images on disk stay valid. `RunState` records the image's slug, defaulting for existing files to the desktop image of the recorded target. `vm build-image`, `vm up`, `vm ssh`, `vm view`, `vm smoke`, `vm down` and `vm purge` take an image slug where they took a target (`all` still means everything); `e2e --target` keeps taking a target, since it only ever means a desktop image; `dist --target` takes a target and picks that target's builder and desktop images itself. `--desktop` is refused for any image but `linux`, by the same rule that refuses it for Windows today. The VM name prefix stays `sunlit-e2e-`, since it is the ownership marker every teardown checks and nothing about it says e2e to a hypervisor; a builder guest is `sunlit-e2e-windows-builder`.

2. **The Windows builder is a differencing layer over the Windows desktop image**, the same copy-on-write mechanism the runtime overlays already use, kept instead of thrown away. Building it: create `layer.vhdx` as a differencing child of `images/windows/golden.vhdx` (in the build directory, moved into `images/windows-builder/` on success), boot a VM on it under `StartReason::Build` so `vm status`, the one-VM-at-a-time rule and `vm down` all apply, wait for SSH and the session marker exactly as any boot does, copy `vm/windows-builder/toolchain.ps1` in and run it over SSH with the pinned channel as its argument (a Windows OpenSSH session for an administrator carries the full token, so the installers run elevated), run `vm/windows-builder/finalize.ps1` to verify, shut the guest down, drop the VM keeping the disk (both of which `build_hyperv` already has functions for), and write the manifest. No install from media, no unattend, no bootstrap: all of that is inherited from the parent, including the job task, the session marker, the autologon and the firewall rule. Twenty to thirty minutes, most of it the Visual Studio installer, against an hour for a second Windows install; and the layer holds only what the toolchain wrote, an estimated 7 to 10 GB against a second 15 GB image. A run boots a throwaway overlay over the layer, three disks deep, which both hypervisors support at negligible cost. On a Linux host the same command does the same thing with `qemu-img create -b` over `golden.qcow2`, so the layer exists in the format of the provider that built it.

3. **A layer carries its parent's identity, and the parent must not change under it.** Hyper-V refuses to attach a child whose parent's identifier changed; qcow2 reads garbage silently. So the layer's manifest records the parent slug and the parent's image checksum as the parent's own manifest states it, and the inventory judges a layer by both: parent missing or checksum different is a new `ImageCondition::Detached`, which blocks a boot and names `cargo xtask vm build-image windows-builder`. The evaluation clock is the parent's, because the install date is: the layer's manifest carries its own `built_unix` for staleness against its template, and its expiry is derived from the parent's manifest, never from its own timestamp. Rebuilding the desktop image therefore costs the layer's twenty minutes as well, and the command that rebuilds a base says so when a layer depends on it. A purge of `windows --image` lists the layer with it and takes both, since a layer without its parent is unreadable; `vm purge windows-builder --image` takes only the layer. Converting a layer between VHDX and qcow2 means flattening it through a full copy of the parent, and it is not certain `qemu-img` reads a differencing VHDX at all, so a layer has one format per host and the QEMU override for the Windows builder is refused with a message.

4. **The Linux builder is a base image on the Ubuntu 22.04 cloud image**, `vm/linux-builder/`, built by Packer on the same path as the Debian template: `jammy-server-cloudimg-amd64.img` behind Ubuntu's `current` path with its `SHA256SUMS`, a NoCloud seed for the account and key, and provisioners. It carries no display manager, no X server, no Mesa and no desktop; what it has is `build-essential pkg-config clang libclang-dev libfontconfig-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev curl ca-certificates binutils` (the CLAUDE.md Ubuntu list's development headers, plus `binutils` for decision 10's `objdump`), rustup with the pinned toolchain at the minimal profile as `tester`, and the guest contract. `snapd` and unattended upgrades go, for the same reason the Debian image disables its apt timers. glibc 2.35 is the floor this gives, and it is the same userland the WSL builder already is. It has a shelf life: standard support for 22.04 ends April 2027, and the successor is 24.04 at glibc 2.39, which is one line in the template and a note in the roadmap. An estimated 4 GB image built in ten to fifteen minutes, booting to SSH in seconds.

5. **The guest contract is the same in a builder as in a desktop guest**, so nothing in `vm::boot`, `job::run` or `collect_results` branches on the image. The Windows layer inherits the whole contract from its parent. The Linux builder gets the runner and the results directory from a `guest-contract.sh` of its own, and since there is no desktop session to write the `ready` marker, a oneshot unit writes it at boot instead; the runner still sources `session.env` when there is one and there never is, which leaves `DISPLAY` at its harmless default. `bring_up` therefore waits for the marker on every image and is right to.

6. **A build boot gets more machine than a desktop boot.** Resources are a property of the image: the two desktop images keep what they have, and both builders get 8 GiB and 8 vCPUs, read from one function that the Hyper-V create script and the QEMU launch both consume, so the existing pinning test extends rather than splits. The release profile is `lto = true`, `codegen-units = 1`, and four parallel `rustc` jobs on a 4 GiB guest with no swap is an out-of-memory kill waiting to happen during the wgpu or Slint crates. If the host cannot spare that, the implementer caps it and records the figure.

7. **The toolchain is pinned by a `rust-toolchain.toml` at the repository root**, `channel = "1.94.0"`, `profile = "minimal"`, `components = ["rustfmt", "clippy"]`, and the xtask reads it (`guest::toolchain::pinned`, a pure parse with tests) rather than spelling the version anywhere else. The channel reaches the Linux builder as a Packer variable and the Windows layer as `toolchain.ps1`'s argument, and both jobs install it by name. This pins the host too: `rustup` in this checkout uses 1.94.0 whatever the default is, and a CI job that installs `stable` installs the pinned version on its first `cargo` call, which is the intended effect. The job does not rely on rustup's automatic install of a missing toolchain, which 1.28.0 removed and 1.28.1 restored behind a variable; it runs `rustup toolchain install <channel> --profile minimal` explicitly, a no-op when the image already has it and a download when the repository has moved on since.

8. **The Windows release binary is linked with the static C runtime**, through `.cargo/config.toml`:

   ```toml
   [target.x86_64-pc-windows-msvc]
   rustflags = ["-C", "target-feature=+crt-static"]
   ```

   In the config rather than in the dist job, so every Windows build of this tree agrees, the e2e binaries included, and a host `cargo build --release` produces the same kind of binary the builder does. Step 1 confirms `cargo test` and `cargo clippy --all-targets` still pass on the host with it, since the flag also reaches build scripts and proc macros; if anything there objects, the fallback is `RUSTFLAGS` in the dist job alone, recorded as a departure. The roadmap item about the release not carrying its runtime closes with this, and decision 10 is what proves it took.

9. **The source is `git archive --format=tar --prefix=src/ HEAD ':!textures'`**, written to `<store>/run/<builder>/dist/src.tar`, copied into the guest as `<root>/src.tar` and extracted by the job into `<root>/src/`. A dirty working tree (tracked files modified or staged, from `git status --porcelain --untracked-files=no`) is refused before anything boots unless `--allow-dirty` is given, in which case the build is still of `HEAD` and the record says `dirty: true`. Untracked files never reach the guest. `textures/` is excluded because the build does not read it and the archive would otherwise carry Git LFS pointer files. Building the working tree rather than the commit is possible (a temporary index plus `git write-tree`) and deliberately not in this phase: a release build should be of something that can be checked out again.

10. **The build job**, in order, on both builders: `rustup toolchain install <channel> --profile minimal`; extract the archive; `cargo build --release --locked -p sunlit-earth` with `cargo` named by absolute path (`$HOME/.cargo/bin/cargo`, `%USERPROFILE%\.cargo\bin\cargo.exe`), following the job scripts' rule that nothing depends on the working directory or `PATH`; copy the executable into `$SUNLIT_E2E_ARTIFACTS/`; write `toolchain.txt` there with `rustc -vV` and `cargo -V`; and write `deps.txt` there from the tool the builder has for reading a binary's imports, which is what makes the two static claims checkable on the host without trusting the build. On Linux that is `readelf -d` for `NEEDED` and `objdump -T` for the highest `GLIBC_` version, and the host asserts the floor is at most 2.35 and the `NEEDED` set is the four libraries above. On Windows it is `dumpbin /dependents`, located through `vswhere` under the installed MSVC, and the host asserts no `vcruntime140.dll` or `msvcp140.dll` among the imports, which is `crt-static` proven on the artifact itself, since a desktop guest with the redistributable installed could not prove it by running. The Windows job sets `LIBCLANG_PATH` from a constant, the way it sets `SLINT_BACKEND` today, and both jobs set `CARGO_NET_RETRY`. `collect_results` brings `results/` back like every other job's.

11. **The artifact is validated in the desktop guest, not the builder.** After the builder is torn down, `dist` boots the desktop image of the same target, copies the executable and, when the repository holds the assets, `textures/` into it (reusing the size check `artifacts::host_textures` already makes), and runs one job: `render --output $SUNLIT_E2E_ARTIFACTS/smoke.png --width 640 --height 360`, with `SUNLIT_EARTH_TEXTURES` set exactly when the textures were staged, on the same rule the e2e job follows. The results come back and the host checks the PNG's IHDR says 640x360, the check `ci.yml`'s smoke step already makes. This is what proves the binary starts and renders on a machine that did not build it, on WARP in one guest and lavapipe in the other. It costs a boot per target, so it is on by default with `--no-verify` to skip it. The desktop image is checked before anything boots, so a missing or expired one is refused up front rather than after a twenty-minute build.

12. **The build is not silent.** The poll loop that waits for `exit_code.txt` also reads `output.log` each poll and prints what is new since the last one, tracked by byte offset (`cat` on Linux, `type` on Windows, which reads a file another process holds open). Cargo prints plain `Compiling` lines when its stderr is not a terminal. Done as an optional progress hook beside `job::wait_for_exit_code`; the e2e path does not take it.

13. **A build guest is named as one.** `StartReason::Dist`, label "a release build (xtask dist)", with a `cost_of_ending` clause so `vm status`, the one-VM-at-a-time refusal and a teardown all say what they are ending. `--keep` keeps the last guest the run booted (the desktop guest when verifying, the builder with `--no-verify`), hands it over the way `e2e --keep` does, and the closing text names what is in it: the source tree and its `target/release` in a builder, the staged binary in a desktop guest.

14. **The output is `<target dir>/dist/<target>/`**, where the target dir is `CARGO_TARGET_DIR` if set and `<repo>/target` otherwise: the executable, `build-info.json`, `build.log` (the builder's `output.log`) and `smoke.png` when verification ran. On success the directory is replaced wholesale; on failure it is not touched, so the previous artifact survives a failed rebuild and the command says so. `build-info.json` records the commit, `git describe --tags --always`, `dirty`, the build's UTC time and duration, the target, the builder image's manifest (`template_hash`, `built_utc`, `source`, and for the layer its parent's checksum), the pinned channel, the guest's `rustc -vV` and `cargo -V`, the glibc floor and `NEEDED` set on Linux, `crt_static: true` on Windows once `deps.txt` proved it, whether verification ran and in which image, and the xtask version. `serde_json`, with a round-trip test, like the manifest and the run state.

15. **A builder without a toolchain is refused before the source goes in.** After the boot, one probe (`test -x $HOME/.cargo/bin/cargo`, `if exist %USERPROFILE%\.cargo\bin\cargo.exe`) decides, and the refusal names the rebuild. The template hash marks a stale image as a warning that still boots, and a build that fails twenty seconds into its job with "cargo is not recognized" is a worse message.

16. **`--target all` runs the two targets in sequence**, one VM at a time as the rule requires, each guest torn down before the next boots, so a full run is four boots: Windows builder, Windows desktop, Linux builder, Linux desktop. A failure in one target does not stop the other; the command prints a summary line per target and exits nonzero if any failed. `--target` defaults to `all`.

17. **The Windows toolchain install is trimmed to what a Rust build uses.** `toolchain.ps1` installs the two Visual Studio 2022 Build Tools components rather than the workload, `Microsoft.VisualStudio.Component.VC.Tools.x86.x64` and one Windows 11 SDK component, from the bootstrapper at `https://aka.ms/vs/17/release/vs_BuildTools.exe` with `--quiet --wait --norestart --nocache` (exit 0 or 3010, like the runtime); the SDK is what brings `rc.exe`, which `embed-resource` requires and `manifest_required()` fails without. For `bindgen` it extracts `libclang.dll` and the clang headers from the `clang+llvm-19.1.x-x86_64-pc-windows-msvc.tar.xz` release archive under `C:\tools\llvm\` rather than running the 2 GB LLVM installer, matching the major `ci.yml` pins; `LIBCLANG_PATH` in the job names that directory. `rustup-init.exe` from `static.rust-lang.org` runs with `-y --default-toolchain <channel> --profile minimal` as `tester`, who is the SSH account and the account the job task runs as. Defender real-time exclusions for `C:\sunlit-e2e`, `.cargo` and `.rustup` go in the same script, since scanning every object file a build writes is a measurable tax in a throwaway guest. `finalize.ps1` verifies: `cargo.exe` and `rustc +<channel> -vV` answer, `vswhere` finds the VC tools component, `rc.exe` is under the SDK, `libclang.dll` is where the job's constant says, `dumpbin.exe` is findable. The layer's size is measured and written down; `Optimize-VHD` on the finished disk and the Windows Server Core option are what to reach for if it disappoints.

18. **The crate registry is not warmed into either builder.** Each build downloads the lockfile's crates, a few hundred megabytes that `--locked` and the checksums make identical every time. Warming `~/.cargo/registry` at image-build time would save a minute or two per build and cost a source archive in both builders' provisioning plus a `cargo fetch` whose failure modes are the network's. Stretch, only if it turns out to be a dozen lines; otherwise a departure says it was skipped.

## The command

```
cargo xtask dist [--target <windows|linux|all>] [--keep] [--no-verify] [--allow-expired-image] [--allow-dirty]
```

What one target does, in order: read the pinned toolchain and the git facts and refuse a dirty tree; check the builder image and, unless `--no-verify`, the desktop image; boot a pristine overlay of the builder; probe for the toolchain; write and copy in the source archive; run the build job, printing its output as it arrives; collect the results and tear the builder down; check `deps.txt` on the host; boot the desktop image, stage the binary and the textures, run the render job, collect it, tear it down; write `target/dist/<target>/`; print where it is, what it was built from, and what it needs to run.

## Steps

1. `rust-toolchain.toml` at the root, `guest::toolchain` with its parser and tests, and the `crt-static` block in `.cargo/config.toml`. Confirm `cargo test` and `cargo clippy --all-targets` pass on the host through the pinned toolchain with the static runtime. Commit.
2. The image model: `Image`, the store paths keyed by it, `RunState`'s image field with its default, the manifest's parent record, `ImageCondition::Detached` and the parent-derived expiry in the inventory, `vm status`'s rendering of four entries, the teardown plan's treatment of a layer and its parent, the CLI's image arguments, the providers creating children over whichever disk the image names, and resources per image. No behavior changes for the two existing images, and both stay valid on disk. Every pure decision gets a test in the same commit as the function. Commit, possibly in two.
3. The Linux builder: `vm/linux-builder/` (template, cloud-init seed, `toolchain.sh`, `guest-contract.sh` with the boot-time marker unit, `finalize.sh`), `build_image` passing the channel. The shell-syntax test picks the new scripts up by directory. Commit, then `cargo xtask vm build-image linux-builder` live and `cargo xtask vm smoke linux-builder` on the result.
4. The Windows layer: `commands::build_layer` and the provider's layer creation, `vm/windows-builder/` (`toolchain.ps1`, `finalize.ps1`), the manifest with the parent record, the announce text. The PowerShell parse test picks the scripts up. Commit, then `cargo xtask vm build-image windows-builder` live and `cargo xtask vm smoke windows-builder`. Record the layer's size.
5. The `dist` command: `StartReason::Dist`, the source archive and dirty check, the toolchain probe, the two build jobs, the progress hook, the `deps.txt` checks, the verification boot and its render job, `build-info.json`, the dist directory, the `all` sequencing and summary. The generated job scripts join `script_syntax`'s list of generated scripts. Commit by decision.
6. Live: `cargo xtask dist --target linux`, `--target windows`, `--target all`, and `--no-verify` once. Verify the artifacts against the acceptance criteria. Record the measurements: build duration per target, layer and image sizes, peak memory if observable.
7. Docs: CLAUDE.md (the build commands, the VM orchestration section's image model and the dist paragraph, a note beside decision 8's "build on the host" that the release path is the exception and why, `rust-toolchain.toml` and `crt-static` under Key Constraints), `docs/vm-setup.md` (the four images and what a layer is, a "Release builds" section, the disk budget, troubleshooting entries for a detached layer, a builder without a toolchain, an out-of-memory build and a build over the task's time limit), `vm/README.md`, and `docs/roadmap.md` (the item, the runtime item closing on the Windows side, the glibc floor and its April 2027 date under the Linux builder, the Server Core option). Commit.
8. Gates at the tip: `cargo test` on Windows, `cargo clippy --all-targets` with zero warnings, `cargo fmt --check`, and the WSL leg, with the known `tests/shading.rs` flake not failing the gate when a rerun passes.

## Acceptance criteria

1. `cargo xtask vm build-image linux-builder` and `cargo xtask vm build-image windows-builder` both succeed, each `finalize` verifies its toolchain, and `cargo xtask vm smoke <image>` passes on both.
2. `cargo xtask vm status` lists four images, the layer under its parent, with the layer's expiry equal to the parent's; `vm doctor` reports all four current; the two desktop images were not rebuilt.
3. `cargo xtask dist --target windows` produces `target/dist/windows/sunlit-earth.exe`, `build-info.json`, `build.log` and `smoke.png`; `deps.txt` from the builder lists no `vcruntime140.dll` or `msvcp140.dll` and `build-info.json` says `crt_static: true`; the exe also runs `render --output x.png --width 640 --height 360` on the host.
4. `cargo xtask dist --target linux` produces the same four files; `build-info.json` records a glibc floor of 2.35 or lower and a `NEEDED` set of `libc`, `libm`, `libgcc_s` and `libfontconfig.so.1`; the render in the Debian 13 desktop guest passed and its `smoke.png` is 640x360.
5. `build-info.json` on both targets names the commit that was built, the pinned channel and a `rustc -vV` whose release matches it; a build with `--allow-dirty` on a dirty tree records `dirty: true`, and one without the flag refuses before booting anything.
6. A dist run against a builder without a toolchain (a kept builder guest with `cargo` renamed is the cheap way) is refused after the boot with the message naming the rebuild; a dist run after the Windows desktop image is rebuilt is refused before the boot as detached, naming `vm build-image windows-builder`.
7. `cargo xtask dist --target all` runs both targets through four boots, tears each guest down, prints two summary lines, and exits nonzero when one is made to fail; `--no-verify` skips the two desktop boots and `build-info.json` says so.
8. During a build the terminal shows cargo's `Compiling` lines as they happen.
9. `vm status` during a build reports "a release build (xtask dist)"; a second `vm up` during one is refused with the clause naming what `vm down` would end.
10. The e2e suite is unaffected: `cargo xtask e2e --target linux` (KDE) and `--target windows` both green on the existing desktop images, with the e2e binaries now statically linked.
11. `vm purge windows --image` lists the layer with the base and says why; `vm purge windows-builder --image` lists only the layer.
12. Gates as in step 8.

## Risks

- **The Visual Studio installer.** Quiet for ten to twenty minutes, several gigabytes of download, and it can end in 3010 asking for a reboot, which the shutdown that ends the layer build absorbs. It runs over SSH here rather than at first logon, so its output is on the terminal as it happens; a hung installer is diagnosed through `vm view windows-builder`.
- **The LLVM archive comes from GitHub**, which the image builds did not depend on before, and extracting a `.tar.xz` on Windows needs bsdtar, which Windows 11 ships as `tar.exe`. One retry and a line naming the URL is the fix within reach.
- **Layer identity.** Hyper-V's parent check is a safety net with a confusing message; the manifest check has to fire first and say "detached" rather than let Hyper-V say "chain broken". Moving the store breaks every chain, which is already true of the runtime overlays and worth a line in the docs.
- **The Ubuntu cloud image under `virtio-vga` without a DRM driver.** The `linux-image-virtual` kernel in the cloud image may or may not carry `virtio_gpu`; the builder needs no graphics, so a text-mode VNC console is fine, but `vm view linux-builder` should show something rather than nothing. Measured at the first boot.
- **Memory.** Decision 6's 8 GiB is an estimate. If the Linux build still dies, `-j4` in the job or a swap file in `toolchain.sh` is the fallback, and the measurement goes in the validation record.
- **Time.** A cold release build with fat LTO on eight virtual cores plus a crate download should land between fifteen and forty minutes per target, and verification adds a boot each. The job timeout is two hours, under the scheduled task's three; if a build ever approaches that, the task's limit is the constant to raise, in the parent's `bootstrap.ps1`, which is a desktop image rebuild.
- **`crt-static` on the host.** It reaches build scripts, proc macros and the test binaries. Expected to be fine on the MSVC target, where it is a common global setting, but step 1 is where that is found out rather than assumed.
- **The pinned toolchain moves the host too.** A developer whose default is newer sees rustup use 1.94.0 in this checkout. Intended, and worth one line in CLAUDE.md.
- **Two evaluation clocks are one.** The layer expires when its parent does, so a Windows desktop rebuild for expiry costs the layer as well, twenty minutes on top of the hour.

## Open questions

1. **Textures beside the binary.** `target/dist/<target>/` holds the executable and not `textures/`, so it is not a runnable install on its own. Copying the repository's `textures/` in when the assets are present is a few lines, but it decides how textures are meant to be distributed, which this plan does not otherwise touch. Default: not copied; verification stages them into the desktop guest and nothing more.
2. **The VM name prefix.** `sunlit-e2e-windows-builder` is accurate about ownership and slightly misleading about purpose. Renaming the prefix touches every teardown test and the docs; default is to leave it.

## Departures

1. **A layer's evaluation clock is read from the timestamp it recorded for its parent, not from the parent's manifest at the moment of asking.** Decision 3 says the expiry is derived from the parent's manifest and never from the layer's own timestamp, and both halves of that hold; what differs is when the parent's build time is read. `build_layer` copies it into the layer's `ParentRecord` alongside the parent's checksum, and `Manifest::eval_epoch` reads it from there. The two answers can only differ if the parent was rebuilt under the layer, and that changes the parent's checksum, which makes the layer `Detached` and blocks a boot before anything asks about the clock. Recording it keeps the inventory's judgement a pure function of one manifest plus the small `ParentFacts` the scan collects, rather than making every condition query read a second manifest.

2. **The crate registry is not warmed into either builder, as decision 18 anticipated.** Neither image ships a `~/.cargo/registry`, so every build downloads the lockfile's crates. Warming one would have meant a source archive in both builders' provisioning and a `cargo fetch` whose failure modes are the network's, in exchange for a minute or two per build, and it would have made the images depend on the lockfile of the day they were built. The download is visible in the build's own output, `--locked` and the checksums make it identical every time, and `CARGO_NET_RETRY` is set in both jobs.

3. **`resources_for` lives in `provider`, not in `provider::qemu`.** Decision 6 asks for one function that the Hyper-V create script and the QEMU launch both read, and the QEMU module was the wrong home for the one the Hyper-V provider now consumes. `provider::resources_for(image)` is that function; `hyperv::MEMORY_BYTES` and `hyperv::CPUS` are gone, both create scripts take the pair as an argument, and the test that pinned the two providers against each other now reads the printed figure back out of the lifecycle text for every image.

4. **`vm smoke` asks each image about what it has.** The Linux smoke job probed the X server, which a builder does not have: the first live smoke of `linux-builder` printed `xdpyinfo: command not found` and still passed, which reads as a broken image rather than a script asking the wrong question. `vm::smoke_script` now asks a desktop image for its display and a builder for its `cargo -V`. Not in the plan, and the same class of thing as the boot line that used to promise a builder guest a desktop session.

5. **A builder with no toolchain was demonstrated by pointing the builder slot at the desktop image, not by renaming `cargo` in a kept guest.** Acceptance criterion 6 suggests the second, and it cannot work: every boot creates a throwaway overlay of the golden disk, so a change made inside a live guest is not in the guest the next run boots, and the only way to make the rename stick would be to modify a golden image, which goal 5 forbids and which would cost a rebuild to undo. What the criterion is really about is the probe, its refusal and the teardown that follows it, and all three are exercised by a run whose builder genuinely has no cargo in it. `Image::builder(Target::Linux)` was pointed at the Debian desktop image for one run, which is a real image with no toolchain, and the run booted it, probed it, destroyed the guest and refused by naming the rebuild of the image it had probed. The one line was reverted immediately and the run is recorded with the method it used.

6. **The source archive is written to `<store>/run/<builder>/src.tar`, one directory shallower than decision 9 says, and it is deleted as soon as the guest has it.** The decision spells the path `<store>/run/<builder>/dist/src.tar`, and that subdirectory is outside everything that accounts for run state: the inventory scanned a run directory one level deep and the teardown removes only an empty directory, so three archives of 8.4 MiB each sat in the store with `vm status` reporting "no overlays or run state, 0 B" over them. The archive is run state like the overlay and the state file, so it belongs on the same shelf as those, and the host's copy is redundant the moment the guest has one: `git archive` remakes it in a second from the commit it names. So the copy goes when the copy into the guest succeeds, and a copy that failed leaves it where the next `vm status` reports it and `vm down` removes it. The scan is recursive as of the same round, so the deeper path would now be accounted for too; the shallower one is kept because nothing about the archive wanted a directory of its own.

## Validation record

### The Linux builder image

`cargo xtask vm build-image linux-builder`, on this Windows host through Packer and QEMU on WHPX:

- **Second attempt succeeded in 1 minute 12 seconds** of Packer time, plus the Ubuntu cloud image download on the first (700 MB, cached afterwards under the build directory).
- **`golden.qcow2` is 3.1 GiB** (3,323,854,848 bytes) against the Debian desktop image's 4.0 GiB, on a 48 GiB virtual disk. `fstrim` reported 44 GiB trimmed at the end.
- The first attempt failed in `finalize.sh` with exit 141, which is SIGPIPE: `ldd --version | head -1` under `pipefail` kills `ldd` when `head` closes the pipe. `awk 'NR == 1'` reads to EOF and does not. Worth knowing before writing another `| head` in a script with `set -o pipefail`.
- `finalize.sh`'s checks all passed on the second attempt: `rustc +1.94.0 -vV` and `cargo +1.94.0 -V` answer, `libclang.so` is at `/usr/lib/llvm-14/lib/`, and `readelf` and `objdump` are both on `PATH`.

`cargo xtask vm smoke linux-builder`: **SSH answered 16 seconds after the boot, the readiness marker was already there (0 s), the job ran and its results came back, and the guest was destroyed. 17 seconds in total.** The boot-time marker unit is what makes the wait for a session return immediately in an image that has none, which is decision 5's whole claim.

### `cargo xtask dist --target linux`

The first live release build, of commit `0b7c9a4`, on 2026-08-28. Green end to end.

| | |
|---|---|
| source archive | 8.4 MiB, `HEAD` without `textures/`, clean tree |
| the build itself | **4 minutes 51 seconds** in the guest, 519 crates, cold registry |
| the whole target | **5 minutes 40 seconds**, two boots included |
| the binary | 31,852,608 bytes (30.4 MiB), stripped by the release profile |
| glibc floor | **2.35**, which is the floor the builder exists to give it |
| `NEEDED` | `libfontconfig.so.1`, `libgcc_s.so.1`, `libm.so.6`, `libc.so.6`, and nothing else |
| verification | the Debian 13 desktop guest: SSH at 12 s, textures staged, `--version` answered, `render` produced a 640x360 PNG of 375 KiB |

Four things this measured that the plan could only estimate. A cold release build with fat LTO is **five minutes on eight virtual cores**, not the fifteen to forty the plan budgeted, so the two-hour job timeout is generous by an order of magnitude and the crate download is not worth warming (departure 2). The `NEEDED` set is exactly the four the plan predicted from the WSL build, so the assumption that X11, xcb, xkbcommon and EGL stay `dlopen`ed survives a release profile with LTO. Nothing in the guest needed a display for the build, and nothing in the desktop guest needed one for the render. And the progress hook works: cargo's `Compiling` lines arrived on the terminal as they happened, which is acceptance criterion 8 observed rather than reviewed.

`build-info.json` came out with the commit, `describe`, `dirty: false`, the channel, the guest's own `rustc -vV` and `cargo -V`, the builder image with its template hash and build time, the linkage, `verified_in: linux`, and the xtask version. The Windows-only fields are absent rather than empty, which is what the `skip_serializing_if` on them is for.

### The Windows builder layer

`cargo xtask vm build-image windows-builder`, on this Windows host over the existing Windows desktop image. The first build succeeded; `vm smoke windows-builder` on it did not, and the reason was in the image rather than in the orchestration. Both are recorded because the second is the interesting one.

The build itself, first attempt:

- **Six minutes four seconds**, from the command to the manifest, of which the Visual Studio Build Tools installer is most. It exited 0 rather than 3010, so no reboot was even deferred. That is a fifth of the plan's estimate, and the reason is decision 17: two components rather than the workload, and the LLVM release archive rather than the 2 GB installer.
- **The differencing child boots.** SSH answered immediately after the address appeared, which settles the one thing the layer approach could not be argued into: a generation 2 VM on a differencing VHDX whose parent is the golden image is a guest like any other.
- **`finalize.ps1` found every part of the toolchain**: `rustc 1.94.0` and `cargo 1.94.0` by channel name, the VC tools under `BuildTools`, `rc.exe` at `Windows Kits\10\bin\10.0.22621.0\x64\rc.exe`, `dumpbin.exe` under `VC\Tools\MSVC\14.44.35207\bin\Hostx64\x64`, and `libclang.dll` at `C:\tools\llvm\bin`.
- **The evaluation clock is the parent's**, as designed: the closing line read "evaluation day 6 of 90, 84 left" on a layer built the same day, because the Windows image behind it was installed six days earlier.
- **`layer.vhdx` is 16.6 GiB**, against the plan's estimate of 7 to 10 GB. A differencing child records every block the guest changed, not every byte it installed, and a Windows guest with 8 GiB of memory changes a great many: the page file alone can be gigabytes, and the servicing stack, the registry and the event logs all move. It is still 16.6 GiB against the 19.3 GiB a second Windows install would have cost, plus an hour, so the mechanism earns its place; what it does not earn is the estimate. `Optimize-VHD` after an in-guest `Optimize-Volume -ReTrim` is the next thing to try, and the Server Core option in the non-goals is the one after that.

Then `vm smoke windows-builder` failed, and it is worth writing down what it found. **The parent image writes its readiness marker from the console session at logon, and the layer build logs on, so the marker ended up inside the layer.** Every boot of that layer therefore looked ready before it had a session: `bring_up`'s session wait returned in zero seconds, `schtasks /run` fired the interactive job task, reported success because the task exists, and the job never started for want of a session to start it in. The wait then sat out its whole five minutes for an exit code that was never coming.

The fix is in the image, where the same clause already exists in the parent's own `finalize.ps1`: the layer's finalize now clears `ready`, `job.cmd` and `results/` and fails the build if the marker survives. The host was left alone deliberately. `bring_up` could delete the marker after SSH answers and wait for it to reappear, which would be a general fix rather than a per-image one, but it would change the timing of the e2e path that is currently green, and the marker being absent from a golden image is a property every other image already has.

The lesson generalizes past this one file: **a layer inherits its parent's run-time leftovers, not just its installation.** Anything a boot of the parent writes into the guest root is in the child unless the child's finalize takes it out.

The rebuild with that fix, and the smoke on it:

- **Four minutes twenty-four seconds**, and `layer.vhdx` came out at 16.5 GiB, within a tenth of a gigabyte of the first. So the two measurements of a layer build are four and six minutes, not the twenty to thirty the plan budgeted, and the size is a property of what a Windows guest touches rather than of what this build installed.
- **`vm smoke windows-builder` passes**: the address appeared, SSH answered at once, **the session wait took 6 seconds rather than returning instantly**, which is the fix showing itself, and the job ran and exited 0 nineteen seconds after the boot. The guest was destroyed and its overlay removed.
- `vm status` afterwards: all four images `ok (current)`, no VM recorded, **67.7 GiB in the store** (Windows 34.1 GiB across its two formats, the layer 16.5, Debian 4.0, the Linux builder 3.1, and the 6.6 GB of media).

Acceptance criterion 1 is met for both builders, and criterion 2 is met but for `vm doctor`'s four-current line, which needs one more run to be quoted here.

### `cargo xtask dist --target windows`

Of commit `e348f64`, on 2026-08-28. Green end to end at the first attempt, after one bug found by reading the generated script rather than by running it.

**The bug first, because it would have cost the run.** The build job's `dumpbin` lookup is a `for /f` loop, and a batch file doubles a loop variable: `%%i`. The generated script carried `%%%%i`, because the four percent signs in the Rust format string are four percent signs in the output, and `cmd` refuses `%%%%i` outright with "cannot be processed syntactically" rather than finding nothing. The test that existed asked whether the script contains `%%i`, which four percent signs also satisfy; it now also asks that there is no run of three. Measured against `cmd` on the host before the fix went in, which is the only way to be sure which of the three forms it wants.

| | |
|---|---|
| source archive | 8.4 MiB, `HEAD` without `textures/`, clean tree |
| the build itself | **6 minutes 45 seconds** in the guest, 519 crates, cold registry |
| the whole target | **7 minutes 47 seconds**, two boots included |
| the binary | 29,183,488 bytes (27.8 MiB) |
| imports | **26, none of them the Visual C++ runtime**, so `crt_static: true` |
| verification | the Windows desktop guest: SSH at 0 s, session at 6 s, textures staged, `render` produced a 640x360 PNG of 362.5 KiB |
| on the host | the same exe answered `--version` and rendered a 640x360 PNG of 332,910 bytes with an IHDR that says so |

What this measured that the Linux target could not. The Windows build is **six minutes forty-five against Linux's four fifty-one** on the same eight virtual cores, which is the ordering anyone would predict and is still nowhere near the plan's fifteen to forty minutes. `dumpbin` came out of `vswhere -find` as designed, and its version line reads 14.44.35228.0 against the 14.44.35207 `finalize.ps1` reported at image build time, which is the same MSVC toolset moving under a servicing update rather than a second one. `tar.exe -xf` unpacked the source archive with no complaint, and `cargo build --release` found `link.exe` with no `vcvarsall` anywhere in sight: rustc's own MSVC probing is what makes the job script as short as it is. The 26 imports are the four API sets and the twenty-two system DLLs a Slint and wgpu binary asks for, and `bcryptprimitives.dll` rather than `vcruntime140.dll` is decision 8 proved on the artifact.

Acceptance criterion 3 is met, including the host run.

### `cargo xtask dist --target all`, and the runs built to fail

`--target all` of commit `3978187`, clean tree, verification on: **thirteen minutes for both targets through four boots**, Windows built in 7m40s and Linux in 5m22s, each guest torn down before the next one booted, two summary lines at the end and exit 0. Both `build-info.json` name the commit that was archived, the pinned `1.94.0`, and a `rustc -vV` whose `release:` line reads 1.94.0; the Windows record carries `crt_static: true` with its 26 imports and the Linux one the glibc 2.35 floor with its four libraries. That is acceptance criterion 7's first half, and criterion 4 again on a second commit.

Then three runs whose point was to fail, because a refusal nobody has seen is a refusal nobody has tested.

**A dirty tree, with and without the flag.** One tracked file touched, no flag: refused before anything booted, naming both ways out. With `--allow-dirty --no-verify` and the layer detached, the Linux target built and its record says `dirty: true`. `verified_in` is absent rather than `false`, which is how the file already says a Windows field does not apply, and the summary line says it in words: "linux: built in 5m04s and not verified".

**A detached layer.** The layer's manifest was edited by hand to name a parent checksum the parent does not have, which is what a rebuild of the Windows image would do without costing an hour and a fresh evaluation clock. `vm status` called it `detached (the windows image was rebuilt: golden.vhdx is crc32:4e276f83 now and this layer was built over crc32:deadbeef)`, and the dist run refused the Windows target before booting. **What the refusal did not do was name the rebuild**: `Detached` had no arm of its own in `check_image` and fell through to the catch-all, which names the condition and no way out of it. Decision 3 asks for the rebuild to be named, so `vm::detached_help` now does, and says the thing the condition needs said: the disk's own checksum matches and it is unreadable anyway, because a differencing child holds only what its own build changed. Fixed in `bb53faf` with a test.

**A run against a builder with no toolchain.** Done by pointing `Image::builder(Target::Linux)` at the Debian desktop image for one run rather than by renaming `cargo` in a guest, which is what the criterion suggests and which a throwaway overlay undoes: every boot is a fresh child of the golden disk, so nothing a live guest is made to forget survives into the run that would notice. The desktop image genuinely has no cargo, so the probe is production code answering honestly about a real image: the guest booted, the probe found nothing, **the guest was destroyed rather than left running**, and the refusal named `cargo xtask vm build-image linux`, the image it had actually probed. Criterion 6, both halves.

**Exit codes and summaries.** The run with the detached layer exited 1 with `windows: failed` and `linux: built in 5m04s and not verified`, which is criterion 7's second half: a failure in one target does not stop the other, and the process says so.

### During a build, and the two purge listings

Criterion 9, both halves, taken while the first Windows build was compiling: `vm status` listed `sunlit-e2e-windows-builder is running, started 2026-08-28T19:34:29Z (a release build (xtask dist))` with its 6.2 GiB overlay, and `cargo xtask vm up windows` was refused with `cargo xtask vm down windows-builder frees it, which ends the release build running in it and starts that compile over from an empty target directory`. The refusal is ahead of `create_from_golden`, so trying it costs nothing.

Criterion 11 found the other missing sentence. `vm purge windows --image` lists five files including the layer's two and frees 50.6 GiB; `vm purge windows-builder --image` lists two and frees 16.5 GiB. Both answered `n`, and nothing was deleted, which is what `confirm` does with anything that is not plainly yes. **The base's listing did not say why the layer was in it**, though `Selection::reaches_as_a_layer`'s own comment claims it does, and that rule had no test at all. The plan now carries both: a `layers` field the rendering turns into one line, and a test in both directions.

### Where the Windows layer's 16.5 GiB was, and what came off it

Decision 17 says to measure the layer and reach for trimming if it disappoints, and 16.5 GiB against an estimate of 7 to 10 GB disappoints. So a guest was booted on the layer and asked what it holds:

| | |
|---|---|
| Visual Studio Build Tools | 1.76 GiB |
| the Windows 11 SDK | 1.86 GiB |
| `.rustup` (one toolchain, minimal) | 0.54 GiB |
| the Visual Studio installer's `Package Cache` | 0.53 GiB |
| Windows Update and Delivery Optimization downloads | 0.34 GiB |
| `%TEMP%` | 0.18 GiB |
| libclang and the clang headers | 0.11 GiB |
| `WinSxS`, inherited from the parent | 9.43 GiB |

The installed toolchain is **4.8 GiB of it**, which is inside the plan's own estimate. The rest is what a differencing child does: it records every block the guest wrote, including the ones it freed again. The Visual Studio installer's downloads, the LLVM release archive and the two-gigabyte tree it was unpacked into are all deleted by `toolchain.ps1` and all still in the file, because deleting a file tells the guest's file system and nothing else.

Two steps, one on each side of the boundary, and neither is a rebuild of anything. `finalize.ps1` deletes what a build does not read and then runs `Optimize-Volume -DriveLetter C -ReTrim`, which is what tells a virtual disk a block is free; the build then runs `Optimize-VHD -Mode Full` on the host between the move into the store and the manifest, so the manifest measures the file as it will be read. Both are best effort, because a layer that could not be trimmed is a larger layer rather than a failed build, and the trim runs *before* the toolchain checks rather than after them, so anything it breaks fails that build.

| | |
|---|---|
| deleted inside the guest | 805 MB (Package Cache 543, `%TEMP%` 183, Delivery Optimization 66, Windows Update 13) |
| the disk when the guest shut down | **15.8 GiB** |
| after `Optimize-VHD -Mode Full` | **13.5 GiB** |
| against the untrimmed layer | **16.5 GiB, so 3.0 GiB off, 18%** |
| the build | 4 minutes 21 seconds, compaction included |
| the store | 67.7 GiB down to 64.8 GiB |

So the deletions are the smaller half and the compaction is the larger one: 2.3 of the 3.0 GiB is blocks the guest had already freed and nothing had told the disk about. `vm smoke windows-builder` passes on the result with its session wait at 6 seconds rather than 0, which is the marker fix still holding, and `finalize.ps1` found `rc.exe`, `dumpbin.exe` and `libclang.dll` where the job's constants say they are after the trim rather than before it.

What is left is 13.5 GiB of which 4.8 is the toolchain, and the remaining 8.7 is a Windows guest having been booted: the page file, the servicing stack, the registry, Defender's definitions and the event logs, none of which a differencing child can be talked out of recording. Nothing cheap is left. Squeezing it further means the Windows Server Core option in the non-goals, which is a second ISO, a second unattend file and an install path nobody has measured, and which would replace the 19.3 GiB parent rather than shrink the 13.5 GiB child. That is the next thing to reach for and it is not cheap, so the layer stays as it is.

### The e2e suite on the existing desktop images

Criterion 10, run after every change on this branch and on the two desktop images as they were. `cargo xtask e2e --target linux --desktop kde`: **10 of 10 passed in 283 seconds**, `test_set_wallpaper` included, with its usual note that Plasma's setter has nothing to ask what the wallpaper is. `cargo xtask e2e --target windows`: **11 of 11 passed in 203 seconds**, `test_session_end_shuts_down_promptly` and `test_set_wallpaper` included. Nothing about the image refactor or the static C runtime disturbed either.

The static runtime reached those binaries too, which is what makes the run more than a regression check: they are built on the host in debug, and `crt-static` is in `.cargo/config.toml` rather than in a profile, so the debug `sunlit-earth.exe` the suite staged has no `vcruntime140` in it either. A dynamically linked one would have run in that guest anyway, since the parent image's first logon installs the redistributable, which is exactly why decision 10 checks the imports rather than the run.

### `vm doctor` on a host with four images

Criterion 2's other half, with nothing running: **22 passed, 0 warnings, 0 failed**, the last four lines being `windows image ok: current`, `windows-builder image ok: current`, `linux image ok: current`, `linux-builder image ok: current`. The two desktop images report the build times they had before this branch started, 2026-08-21 and 2026-08-23, which is goal 5 stated as a measurement: neither was rebuilt.

### `--target all` again, on the trimmed layer

Of commit `40c815e`, after the trim changed the layer under it: **Windows built in 7m46s and Linux in 5m21s**, both verified in their desktop guests, exit 0. The Windows figure is within four seconds of the run before the trim, so nothing the trim deleted was something a build reads, which is the claim the ordering inside `finalize.ps1` was chosen to make falsifiable. `build-info.json` names the layer's new build time, and the store is 64.8 GiB with all four images current.

### The gates

At `dac4e3c`, on this Windows host and in WSL.

- `cargo test`: **all green**, 450 + 54 + 14 + 21 + 12 + 1 + 46 + 2 + 27 + 494 passing across the workspace with the 11 e2e cases ignored as they are meant to be.
- `cargo clippy --all-targets`: **zero warnings**, which is what keeps CI's `-D warnings` honest.
- `cargo fmt --check`: **clean**.
- The WSL leg, `cargo test --workspace` under Ubuntu 22.04 into a Linux target directory: **failed once and passed on the rerun**, which is the documented treatment of the known flake. The failure was exactly it: `software_adapter_produces_correct_results` panicking at `wgpu-hal-28.0.1/src/gles/egl.rs:308` with `called Result::unwrap() on an Err value: BadAccess`. `cargo test` stops at the first failing target, so that run never reached the five targets after `shading`; the rerun covered all thirteen and every one passed. The xtask's own count there is 483 against 494 on Windows, the difference being the cases behind `cfg(windows)`.

One thing step 6 asks for was not measured: peak memory in a builder during a build. Nothing came close to needing it, since neither build failed at any point and the fallback the plan names (fewer parallel jobs) was never reached for, and observing it properly would mean a sampler inside the job rather than a number the run already has. `docs/vm-setup.md` carries the troubleshooting entry that names the fix if a build ever does die that way.

### What the criteria came to

| | |
|---|---|
| 1 | met: both builders built, `finalize` verified each, both smokes pass, and the Windows one was rebuilt and smoked again after the trim |
| 2 | met: four images in `vm status` with the layer under its parent and the parent's expiry, `vm doctor` 22 passed, the two desktop images still carrying their August 21 and August 23 build times |
| 3 | met: all four files, no Visual C++ runtime among the 26 imports, `crt_static: true`, and the exe renders 640x360 on the host |
| 4 | met, twice: glibc floor 2.35, the four expected `NEEDED`, a 640x360 render in the Debian guest |
| 5 | met: the commit, the channel and a matching `rustc` release on both targets; `dirty: true` with the flag and a refusal without it |
| 6 | met, the second half by hand-editing the layer's manifest and the first by departure 5's method |
| 7 | met: four boots, two summary lines, exit 0 clean and exit 1 when the Windows target was made to fail; `--no-verify` skips both desktop boots |
| 8 | met: cargo's `Compiling` lines arrive on the terminal as they happen, on both targets |
| 9 | met: the start reason is named in `vm status` and in the refusal a second `vm up` gets |
| 10 | met: 10 of 10 in the Linux guest under KDE, 11 of 11 in the Windows guest |
| 11 | met, after the listing was made to say why the layer goes with its base |
| 12 | met: the four gates above |

### Validator round 1, 2026-08-28, range cc7b184..ca309ef: 1 MAJOR, 8 MINOR, all fixed

A validator with fresh context ran the four gates at ca309ef itself (all green at the first
attempt, the WSL leg included, with no flake), checked the artifacts without trusting the
command that made them (no `vcruntime140` or `msvcp140` among the exe's strings, `readelf`
naming exactly the four expected libraries, a glibc floor of 2.35, and the exe rendering
640x360 on the host), re-ran `dist --target linux` live in 5m27s, took `vm status` and
`vm doctor` on a host with four current images, and read the code against every decision and
departure. What it found is below, most severe first, with what each one came to.

1. **MAJOR. `--keep` kept one guest per target, so a `dist --keep` of both targets failed its
   second one.** Three arms decided it independently and none of them knew how many targets
   the run had: the Windows target kept its desktop guest, and `vm::boot` of the Linux
   builder was then refused by `check_no_other_vm`, which is decision 16's rule doing exactly
   what it exists for. The validator demonstrated the refusal live. Decision 13 says `--keep`
   keeps "the last guest the run booted", singular, and `docs/vm-setup.md` repeats it, so this
   was the code disagreeing with the contract rather than the contract being unclear. Fixed in
   `98f6528`: `dist::keeps_guest` is that rule as one pure function over the run's options and
   the boot in front of it, `Options` carries which targets the run covers because the answer
   depends on them, all three arms read it, and the closing summary names the one guest that
   was kept. `a_run_keeps_the_last_guest_it_booted_and_nothing_before_it` walks the four boots
   of `--target all --keep` and both no-verification and failed-build variants; put back to the
   old inline rule, it fails on the second assertion, which is the Windows desktop guest the
   old code kept.

2. **A character caught half-written panicked the build's progress hook.** `OutputTail::absorb`
   kept a byte index into the previous poll's lossy decode, and a multi-byte character the
   guest was still writing arrives as one three-byte replacement character and becomes itself a
   poll later, which moves every byte after it: the validator's case, poll N of `a\u{FFFD}` and
   poll N+1 of `a🎉`, panicked with "byte index 4 is not a char boundary", and a two- or
   three-byte character dropped the rest of the line in silence instead. Seven minutes into a
   build either way. Fixed in `e3284ca`: what a decode had to replace is held back for the poll
   that has the whole of it, and the index is clamped to a character boundary before it is used.
   The test carries all three widths and fails on the old code with the validator's own panic.

3. **The source archive sat where nothing counted it and nothing deleted it.** It was written to
   `<run dir>/dist/src.tar`, the inventory scanned a run directory one level deep, and the
   teardown removes only an empty directory, so `vm status` reported "no overlays or run state,
   0 B" over three archives of 8.4 MiB each. The teardown's own comment claimed anything
   unexpected inside would show up in the next `vm status`, which was true of a file and not of
   a subdirectory. Fixed in three commits: `98f6528` puts the archive at the run directory's
   root and removes it as soon as the guest has it, so a copy that failed leaves it visible;
   `3137294` scans a run directory whole and reaches through directories to remove the empty
   ones, which makes that comment true; `d367e93` lists a run file by where it sits under the
   run directory rather than by its name alone. Live: `vm down all` then deleted 9 files and
   freed 25.3 MiB, naming all three archives, and left the run directory empty.

4. **The boot line promised every image a desktop session.** "waiting for the desktop session"
   and "the desktop was ready after 0s" printed for `linux-builder`, which has no X server at
   all and writes its readiness marker from a oneshot unit at boot. Departure 4 names that line
   as the class of thing it fixed and never touched it. Fixed in `2b95fbe`: the two lines come
   from `readiness_wait_line` and `readiness_ready_line`, which ask `Image::has_desktop`. Live,
   on the rebuilt builder: "waiting for the guest to be ready for a job / the guest was ready
   after 0s".

5. **The kept-guest text offered the Linux builder an applications menu.** The `Target::Linux`
   arms of `guest_environment_note` branched on the operating system, so `vm smoke
   linux-builder --keep` was told to look for desktop entries in an image that has no session
   to show them. Fixed in `2b95fbe`: both Linux arms ask `Image::has_desktop`, and the builder's
   texts name the shell and `vm ssh` instead. The Windows builder is deliberately not asked,
   because it is a layer over the desktop image and its console session is its parent's.

6. **Only the Linux half of the smoke script asked a builder for its toolchain.** `vm smoke
   windows-builder` ran the desktop image's script, so a Windows builder with no cargo in it
   smoked green. Fixed in `2b95fbe`, and live: the Windows builder's smoke now prints
   `cargo 1.94.0 (85eff7c80 2026-01-15)` and exits 0 after 26 seconds.

7. **`--allow-expired-image` was documented nowhere.** Neither CLAUDE.md's usage line nor
   `docs/vm-setup.md`'s release builds section named it. Fixed in `dbb4d7d`, which also states
   in the guide that `--keep` is one guest per run and why. `the_docs_spell_out_every_flag_dist_takes`
   reads the usage line out of both documents and compares it against the command clap defines,
   so the next flag cannot be added in silence.

8. **The Linux builder installed `git`, which decision 4's package list does not name.** Nothing
   in the guest uses it: the source arrives as a tar the host made, the lockfile names no git
   dependency, and cargo's registry protocol needs none. Removed in `dbb4d7d` rather than
   recorded as a departure, and the image rebuilt over it in 1 minute 12 seconds to 3.1 GiB
   (3,349,282,816 bytes), which is within a tenth of a percent of the image that had it. The
   Linux target of the `--target all` run below is what proves nothing missed it.

9. **`TeardownImage` duplicated `Image::ALL` with nothing joining the two lists.** A fifth image
   would have left `vm down` and `vm purge` unable to name it with nothing failing anywhere.
   Fixed in `dbb4d7d` with `the_teardowns_can_name_every_image_and_nothing_else`, which is the
   same shape as `the_guest_accepts_exactly_the_sessions_the_host_can_ask_for`.

Nothing was declined, and the round found one thing the review had not. `--keep` now always
ends a run with a guest still up, which made it visible that `record_kept` moved a test run's
reason to `Keep` and left a release build's alone: the kept guest reported itself as a build
in progress for as long as it existed, and the `vm down` its own closing text recommends
warned that it would end that build and start the compile over from an empty target
directory. Fixed in `6650133`, where both of the reasons that describe work in progress
become the one that describes a guest waiting for somebody, and the label loses the word
`test` because two commands leave a guest behind now.

What the round cost live, in order. The Linux builder was rebuilt without `git` in
1 minute 12 seconds to 3.1 GiB (3,349,282,816 bytes), and `vm smoke linux-builder` passed
in 17 seconds with the new line, "waiting for the guest to be ready for a job / the guest
was ready after 0s", and `cargo 1.94.0` from the probe. `vm smoke windows-builder` passed
in 26 seconds with the same `cargo 1.94.0`, which is a question that image had never been
asked. `vm down all` swept the three orphaned archives, 9 files and 25.3 MiB, and left the
run directory empty.

Then the run the major finding is about. **`cargo xtask dist --target all --keep` of commit
`e7514f4`, clean tree, verification on: four boots, exit 0, Windows built in 7m48s and
rendered in the Windows guest, Linux in 5m22s and rendered in the Linux guest.** Both
figures are within two seconds of the same run before this round, so nothing here cost
anything. The Windows target's guest and the Windows desktop guest were both taken down
although `--keep` was given, the Linux builder was taken down after its build, and the run
ended with one guest and one line naming it: "the sunlit-e2e-linux guest is still up,
because --keep was given; `cargo xtask vm down linux` ends it". `vm status` then reported
exactly one VM running, `sunlit-e2e-linux`, with no source archive anywhere in the store,
and `vm down linux` stopped it and freed 122.1 MiB across four files with no warning about
ending a build, which is what the last commit of the round is for. The Linux binary was
built on the rebuilt image, so nothing in the workspace missed the `git` that went.

Gates at `6650133`, on this Windows host and in WSL:

- `cargo test`: **all green**, 450 + 54 + 14 + 21 + 12 + 1 + 46 + 2 + 27 + 505 with the 11
  e2e cases ignored as they are meant to be. The xtask's own count is 505 against 494 at
  ca309ef, which is the eleven tests this round added.
- `cargo clippy --all-targets`: **zero warnings**.
- `cargo fmt --check`: **clean**.
- The WSL leg, `cargo test --workspace` under Ubuntu 22.04 into a Linux target directory:
  **green at the first attempt**, all thirteen targets, with none of the known
  `tests/shading.rs` flake. Its xtask count is 494 against 505 on Windows, the eleven
  `cfg(windows)` cases being the difference, and both counts rose by the eleven tests this
  round added.
