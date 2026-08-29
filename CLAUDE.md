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
cargo xtask bake-icon              # Rasterize the icon SVGs into the committed outputs under assets/icon/baked/
cargo xtask bake-icon --review DIR # The small-size contact sheet, for the judgment no test can make
cargo xtask bake-stars --input hyg_v44.csv --output crates/sunlit-core/src/assets/stars/hyg_v4_4_mag7.bin

SUNLIT_EARTH_UPDATE_GOLDEN=1 cargo test -p sunlit-core --test golden  # Regenerate goldens for this machine's adapter
```

### VM orchestration (`cargo xtask`)

The desktop e2e suite runs in local VMs instead of taking over the developer's desktop. `docs/vm-setup.md` is the human guide; the design is in `docs/plans/2026-08-19-phase3-vm-orchestration-plan.md`, and the Linux guest's overhaul in `docs/plans/2026-08-21-phase5-linux-vm-and-parity-plan.md`.

```bash
cargo xtask vm doctor              # Unelevated, read-only: can this host run the VM suite?
cargo xtask vm setup               # The one command that changes the host. Elevated on Windows.
cargo xtask vm build-image <image>  # An install or a layer, then a manifest. Minutes to an hour.
cargo xtask vm up <image> [--desktop <kde|gnome|xfce|cinnamon>]   # An interactive guest, with the current binaries in it
cargo xtask vm ssh <image>         # A shell in the running guest
cargo xtask vm view <image>        # Its desktop (vmconnect for Hyper-V, VNC for QEMU)
cargo xtask vm smoke <image>       # Boot, run a trivial job through the guest contract, take it down
cargo xtask vm status              # Images, media, overlays, running VMs, disk footprint
cargo xtask vm down <image|all>    # End the guest, keep the image
cargo xtask vm purge <image|all> [--vm] [--image] [--iso] [-f]
cargo xtask e2e --target <host|windows|linux> [--keep] [--allow-expired-image] [--desktop <d>]
cargo xtask dist [--target <windows|linux|all>] [--keep] [--no-verify] [--allow-expired-image] [--allow-dirty]
```

An `<image>` is one of four slugs: `windows` and `linux` are the desktop guests the e2e
suite runs in, and `windows-builder` and `linux-builder` are where `dist` builds a
release binary. A `<target>` is an operating system, which is what `e2e` and `dist` take,
because each of them picks the image it needs itself.

The host tools the VM commands run go through `host::facts::resolve_tool`, which asks `PATH` and then the places an installer is known to leave a program without putting it on `PATH`: QEMU's and TightVNC's own directories under Program Files, winget's links directory, and scoop's shims directory under `%SCOOP%` or `~\scoop` and `%SCOOP_GLOBAL%` or `%ProgramData%\scoop`. Both package managers append to the *user* `PATH`, so a tool installed in the shell that is now running the xtask is installed and invisible; a lookup that missed it would make `vm setup` plan an install that winget then refuses as redundant, and `vm view` claim a viewer is absent. The VNC viewers are in `facts::VNC_VIEWERS`, executable names rather than package identifiers, and `vm view` of a QEMU guest resolves them the same way `vm doctor` reports them. Two lookups stay on bare `PATH` deliberately and say so where they sit: the Packer ISO tools, because Packer resolves them itself and a fallback location would not help it, and `store::windows_media`'s choice between `curl` and `wget`.

`vm setup` never reboots or signs anyone out; it reports what needs one. `vm doctor` changes nothing. `vm down` is the cheap teardown: it ends the guest and deletes its run state, which the next boot recreates. A command that boots a guest of its own tears it down the same way when it is finished with it, and `vm::run_state_paths` is the one list of what that removes: the record, the overlay, and the three things a boot writes beside them, each named by the `Store` method the writer uses, which are the scratch a job's script was written into (`job_scratch`), the scratch the Windows hand-over launcher is staged in (`handover_scratch`), and the per-VM copy of the firmware's variables a QEMU boot makes (`firmware_vars`). So a green run leaves `vm status` nothing to report. `vm.log` is deliberately outside that list, because a failed boot's message quotes its tail and names its path. `vm purge` is the disk-space one: everything a target has on disk unless `--vm`, `--image`, or `--iso` narrows it, and it asks before deleting unless `-f` is given. `e2e --target host` is what `cargo e2e` does, kept as one command so the manual real-GPU run and the VM runs are the same thing.
**The store holds an image per slug, not an image per target** (`provider::target::Image`).
An image is either a *base*, installed from media, or a *layer*, provisioned over a named
parent:

| slug | kind | what it is for |
|---|---|---|
| `windows` | base | Windows 11 Enterprise evaluation: the e2e guest |
| `linux` | base | Debian 13 with four desktops: the e2e guest |
| `windows-builder` | layer over `windows` | release builds |
| `linux-builder` | base, Ubuntu 22.04 | release builds |

What keys on the image is what belongs to one disk: every path under `images/`, `run/`,
`build/` and `results/`, the VM name, the manifest, the run record, and how much of the
host a guest of it gets (`provider::resources_for`, which both hypervisors read, so a
builder's 8 GiB and eight cores cannot come apart between the two). What keys on the
target is everything about the operating system: the provider matrix, the guest root, the
job scripts, the SSH account, and the device models the Packer templates and the QEMU
command line have to agree about. `Image::target()` is the bridge, and the two desktop
images keep their original slugs so every path they had is the path they have.
The third question is two questions with two names. `Image::has_desktop()` is whether a
session logs on, which the Linux builder alone has none of: it is built from a cloud image
with no desktop in it and writes its readiness marker from a oneshot unit at boot, while the
Windows builder is a differencing child of the desktop image and inherits its autologon.
Every text that offers a console asks it through `Image::console_label()`, so the Linux
builder's `vm view` line says `console:` and every other image's says `desktop:`.
`Image::is_builder()` is the other question, what an image is for, and it is what the texts
about which command uses an image, which command leaves one of its guests behind, and what a
smoke test asks it to prove all read. Asking the first where the second was meant was what
once offered the Windows builder a console four lines above two sentences about its desktop.

A layer is a differencing child, which is what makes the Windows builder five minutes
and a few gigabytes rather than an hour and another fifteen: `commands::build_layer`
creates the child, boots it, runs `toolchain.ps1` and then `finalize.ps1` over SSH, shuts
it down, drops the VM keeping the disk, and moves the disk into the store. The cost is
that the file cannot be read without its parent: Hyper-V refuses to attach a child whose
parent's identifier changed and qcow2 reads garbage silently, so the layer's manifest
records the parent's checksum as the parent's own manifest states it and the inventory
calls a mismatch `ImageCondition::Detached` and blocks the boot. The evaluation clock is
the parent's too, recorded in the same place: provisioning a layer over an eighty-day-old
install does not reset the licence. A purge of a base lists its layers with it, and
`SUNLIT_EARTH_VM_PROVIDER` is refused for a layer, because converting a differencing
child between formats means flattening it through a full copy of its parent.

The other cost is that a differencing child records every block the guest wrote,
including the ones it freed again, so a layer is much larger than what it
installed: the Windows one holds 4.8 GiB of toolchain and came out at 16.5 GiB.
Two steps take that to 13.5, one on each side of the boundary. `finalize.ps1`
deletes the installer caches a build never reads and runs `Optimize-Volume
-ReTrim`, which is the only thing that tells a virtual disk a block is free, and
`build_layer::compact` runs `Optimize-VHD -Mode Full` on the host between the
move into the store and the manifest, so the manifest measures the file as it
will be read. Both are best effort, since a layer that could not be trimmed is a
larger layer rather than a failed build, and the trim runs before `finalize`'s
toolchain checks so that anything it breaks fails that build. The compaction is
the larger half: 2.3 GiB of the 3.0 is blocks the guest had freed and nothing had
told the disk about.

**The builder matrix mirrors the runtime provider matrix**, and `commands::build_image::builder_for` is the one place that decides it:

| | Windows host | Linux host |
|---|---|---|
| Windows image | native Hyper-V (`commands::build_hyperv`) | Packer and QEMU on KVM |
| Linux image | Packer and QEMU on WHPX | Packer and QEMU on KVM |

The one cell that is not Packer is the one QEMU cannot install. QEMU on a Windows host uses WHPX, and a WHPX guest with more than one vCPU does not survive the reboot Windows Setup performs after copying its files: `WHPX: Unexpected VP exit code 4`, unrecoverable. One vCPU survives it and Windows 11 Setup refuses to install on one core; `kernel-irqchip=off` keeps the vCPUs and stops the guest booting at all. So on a Windows host the xtask installs Windows itself, on the hypervisor the guest runs on anyway: it repacks the media without its boot prompt (`store::windows_media::ensure_install_media`, so there is no keypress to inject, keeping a repack only until the build that needed it succeeded, and reusing one a failed build left behind while the record beside it still matches the download it came from), builds the unattend CD with oscdimg from `build_hyperv::CD_FILES`, creates a generation 2 VM, watches the install, runs `finalize.ps1` over SSH, shuts the guest down, keeps its disk and drops the VM, and converts the disk to qcow2 for the QEMU override cell. No Packer, no QEMU process, and no network on the host after the ISO download; the guest's own first logon fetches the Visual C++ runtime, which Windows does not ship and every Rust MSVC binary the suite runs links dynamically, and `finalize.ps1` fails the build if it is not there. Decision 10's intent stands either way: one canonical install in two formats, with only the conversion direction flipping. `docs/vm-setup.md` has the WHPX measurements and a twenty-second recipe for rechecking them against a newer QEMU. The dispatch is deliberately not overridable by `SUNLIT_EARTH_VM_PROVIDER`, which moves a guest rather than a build.

Both builders report on the install while it runs, because neither Packer nor Windows Setup says anything for most of an hour (`commands::build_watch`, whose `Trend` both share). The Packer path holds one QMP connection, presses the installer's boot key with it, and prints a line a minute: what the output disk holds and how fast it is growing, whether the guest's screen is changing, and what QEMU says the machine is doing. A guest QEMU has stopped is reported rather than waited out: the build starts it again once, and if that changes nothing it ends the guest so the build fails now instead of at Packer's two-hour SSH timeout. The build directory keeps `screen.png`, the guest's last screen as QEMU encoded it, and `packer.log`, which is where QEMU's stderr ends up. The native path prints the same shape of line from the signals Hyper-V has: `Get-VM`'s state, the growing VHDX, and the firmware's first boot entry, which is what makes the one unmeasured assumption in it visible (that Setup's own boot entry survives the mid-install reboots). A guest that is not `Running` on two readings in a row ends the build, since a Hyper-V guest stays running through the reboots an install performs; one reading is not enough, because `Start-VM` returns before the guest is `Running` and `Get-VM` says `Starting` in between.

The build VM carries the same `sunlit-e2e-windows` name a runtime guest does and writes a `RunState` with a build reason before it exists, so `vm status`, `vm view`, `vm ssh`, `vm down`, and the one-VM-at-a-time rule all apply to a build. Every failure from the point the create script could have run asks Hyper-V whether the VM is there and says which of the three answers it got, because that script is one process that stops at its first error and most of its statements leave a registered VM behind. A failed build keeps its guest and prints how to reach and remove it; `vm down windows` takes the VM and the unfinished disk together, a `--iso` purge ends a build first because a build holds both DVDs for the whole install, and a build that is still running is refused as something to clear away. Anything that ends a build says so before it does: `StartReason::cost_of_ending` is the one clause, read by the one-VM-at-a-time refusal and by a teardown's listing and question alike, and a purge that ends a build without taking its run state names the record and the disk it leaves.

`e2e --target <guest>` copies three things in: the app, the test harness, and the fixtures, plus the repository's `textures/` when it holds the assets rather than Git LFS pointers (checked by size, since a pointer file exists and cannot be decoded). `test_render_and_exit` samples the globe by color, so without them a guest renders the procedural grid; the job then omits `SUNLIT_EARTH_TEXTURES` rather than naming a directory that is not there, which is the same thing that happens to `cargo e2e` on a host in that state. The generated Windows job also sets `SLINT_BACKEND=winit-software` (`commands::e2e::WINDOWS_SLINT_BACKEND`): a Hyper-V guest's synthetic display adapter offers no OpenGL and Windows ships no software implementation of it, so Slint's default renderer cannot start at all and the app dies with "Could not locate glCreateShader symbol" before its event loop. WARP does not cover that, because WARP is Direct3D and Slint asks for GL. The Linux job sets no backend, because Mesa is a software GL implementation and llvmpipe answers there. Both details live in the generated job, which is per run: neither needs an image rebuild.

`cargo xtask dist` is the release path, and it is the one place decision 8's
"build on the host and copy the binaries in" does not apply. That rule is right for a
debug build of a test harness, where the host's toolchain is the fast path for iterating
on a test; it is wrong for a release, where the point is that the host is not in the
build. So `commands::dist` boots a pristine overlay of the target's builder image, copies
in a `git archive` of `HEAD` without `textures/`, and runs `cargo build --release
--locked -p sunlit-earth` in there with `cargo` named by absolute path. Nothing else of
the host reaches it: no `target/`, no `~/.cargo`, no environment, and the toolchain is
installed by the name `rust-toolchain.toml` pins (`guest::toolchain`).

Two claims a release binary makes cannot be checked by running it, so the builder reads
its own output with the tools it has and the host parses that: `dumpbin /dependents` must
name neither `vcruntime140.dll` nor `msvcp140.dll`, which is `crt-static` proven on the
artifact, and `readelf -d` with `objdump -T` must show a glibc floor of at most 2.35 and
the four libraries the Linux port links. A guest with the Visual C++ redistributable
installed runs a dynamically linked binary perfectly well, which is exactly why running
it proves nothing about that. What running it does prove is the other half, so the binary
is then staged into the *desktop* image of the same target and asked for one 640x360
`render`, whose result is measured from its own PNG header: a render that failed after
opening its output still leaves a file. `--no-verify` skips that boot. The output is
`<target dir>/dist/<target>/`, replaced wholesale on success and untouched on failure,
holding the binary, `build-info.json`, the builder's `output.log` as `build.log`, and the
verification render. A dirty working tree is refused before anything boots, because the
archive is of `HEAD` and a record whose commit does not describe the binary is the one
thing it must not be. `--keep` leaves one guest of the whole run up, not one per target:
the next boot is refused while another guest is registered, so `dist::keeps_guest` narrows
the flag to the last boot the run makes and the closing summary names what is still there.
Which guest that is follows from the verification, so a text offering the builder says
`--keep --no-verify` (`vm::keep_command`): with the verification on, the last boot is the
desktop guest and the flag alone hands back a desktop with no source tree in it.

A build says what it is doing while it does it: `job::OutputTail` reads the guest's
`output.log` on every poll and prints what is new, tracked by how much has been printed
already, because a forty-minute compile that says nothing is indistinguishable from a
wedged one. The whole log is decoded afresh every poll, so a multi-byte character the job
was in the middle of writing arrives as a replacement character and becomes itself a poll
later, which moves everything after it: what a decode had to replace is held back for the
poll that has the whole of it. The e2e path does not take the hook; its suite finishes in
under a minute and its log is printed once at the end.

A guest is also something a person looks at, so staging writes two more things into either one (`guest::handover`): a launcher that starts the app with the environment this boot gave it, and two shortcuts, one for that launcher and one for the guest root. In a Windows guest they are `C:\sunlit-e2e\run-app.cmd`, which sets the same `SLINT_BACKEND` the job sets and `SUNLIT_EARTH_TEXTURES` under the same condition, and two `.lnk` files on the console user's desktop. Per boot for the same reason the job script is per run, and generated rather than shipped in the image because the launcher has to know what this boot staged. A Windows guest also offers one of two consoles, and `handover::enable_enhanced_session` is what decides which. The image ships with Remote Desktop Services disabled, so a guest running a suite offers no enhanced session (`EnhancedSessionModeState` 6 rather than 2) and `vmconnect` opens a basic session that asks for nothing: an enhanced session is RDP, and connecting moves the console session into it, which is where the windowed tests keep their desktop. A guest being handed to a person has nothing of ours running in it, so `vm up` and `e2e --keep` turn the service back on, blank the account's password and clear `LimitBlankPasswordUse`, which is what makes the credential dialog a thing to dismiss rather than fill in. That buys the one thing a basic session cannot do at all: a window that resizes, with the guest's desktop following it. What happened is recorded rather than inferred: `vm::hand_over` writes `RunState::handed_over` from the marker the guest itself printed, and that field, not the start reason, is what `vm view` reads to decide whether to answer the connection dialog and which of the two consoles to describe. The reason is chosen before the boot, so it can answer neither question; `Keep` is written at the same moment, which is what lets `vm status` tell a run in progress from one that is over. Each command's closing text comes from the same facts (`vm::Prepared`), so `vm smoke --keep`, which stages nothing and hands nothing over, is described as the empty desktop it is. A guest whose image predates the Remote Desktop Services disable still offers an enhanced session, and staleness only warns at boot, so the note for a guest nobody handed over says to cancel a credential dialog that appears anyway rather than sign in. The display-configuration dialog in front of an enhanced session is answered by `vm view`, which writes `vmconnect`'s own per-VM settings file (`hyperv::vmconnect_settings`) before starting it, sweeps the ones earlier guests left, and takes the file with the VM when one is destroyed: those settings are filed under the VM's identifier, which is new on every boot, so the dialog's own remember-me checkbox lasts exactly one guest. Runtime is enough for all three, so none of it needs an image rebuild; the disable is an image property only because the service refuses to stop once started, and `finalize.ps1` fails a build whose start type is anything else.

The Linux half of that hand-over is `/var/lib/sunlit-e2e/run-app.sh` and two XDG desktop entries, and what a person there is missing is not an environment variable but the root itself: it sits outside any home directory on purpose, so the closing text names it, the launcher and the shell command, and `vm smoke --keep` promises none of the three. The launcher names the staged textures directory when there is one and sets no backend at all, because Mesa answers GL in the guest; it redirects its own output to `run-app.log` beside it when stdout is not a terminal, since started from an icon there is nowhere else a failure could be read and started from a shell a log would hide it. The entries are written twice, into `~/.local/share/applications` and onto the desktop directory `xdg-user-dir DESKTOP` names, because GNOME draws no desktop icons at all and the menu is the whole hand-over there; both are `Type=Application`, the folder one running `xdg-open`, since a menu shows nothing else and one `xdg-open` finds a file manager in all four sessions (not necessarily that desktop's own; the KDE session opens Thunar). The install script runs over SSH as the console account, so `HOME` is the right home and nothing needs root, and it sources the session's own `session.env` first: the one blessing it writes goes through the session's metadata daemon over the session bus, and without one `gio set` answers that the attribute is not supported. That blessing is `metadata::xfce-exe-checksum`, the file's sha256, which is what xfdesktop's "Mark As Secure And Launch" button writes and the only thing that will make it run a launcher on what it calls an insecure location. Plasma and Nemo were both seen to run an executable entry without asking, which is why there is one attribute and not three. All of it measured one boot per desktop, with the desktop screenshotted each time.

The Linux guest is Debian 13 with four desktops installed side by side, and nothing in the image decides which one a boot logs into. `provider::desktop` is the host half: `Desktop::session` maps the `--desktop` flag onto the `.desktop` basename sddm's `[Autologin] Session=` wants (`plasmax11`, `gnome-xorg`, `xfce`, `cinnamon`, none of which are the names one would guess), and `fw_cfg_args` puts it on QEMU's command line as `-fw_cfg name=opt/sunlit/desktop,string=<session>`. The `opt/` prefix is required; QEMU refuses anything outside it. In the guest, a oneshot unit ordered before `display-manager.service` reads `/sys/firmware/qemu_fw_cfg/by_name/opt/sunlit/desktop/raw`, checks the value against its own allowlist of the same four names, and writes sddm's autologin drop-in. `the_guest_accepts_exactly_the_sessions_the_host_can_ask_for` reads both lists and compares them, since nothing else connects the two: a name on one side and not the other is a boot that silently falls back to Plasma while `vm status` says otherwise. fw_cfg rather than SMBIOS OEM strings, which was the other candidate: the fw_cfg device is ACPI-enumerated so its module loads itself early and the value is a file in sysfs, while mainline exports no per-string sysfs interface for SMBIOS type 11 at all. The chosen desktop goes into `RunState`, so it survives the command that chose it and `vm status` can name it; `--desktop` against the Windows guest is refused rather than ignored, because a run whose flag did nothing is a run whose results are about a desktop nobody chose.

Debian 13 rather than a current Ubuntu, and that choice has a shelf life. It is the only current base where Plasma, GNOME and XFCE all have a first-class X11 session at once: GNOME 50 removed X11 upstream in March 2026 and Ubuntu 25.10 had already dropped the GNOME Xorg session, while trixie froze on GNOME 48 and Plasma 6.3. That insulates the image for trixie's support window (full to 2028-08, LTS to 2030-06) and no longer. The guest stays on X11 because the guest contract needs `DISPLAY` for a process the orchestrator starts over SSH, so a Wayland guest story is real work rather than polish and is on the roadmap as such.

Of Debian's two cloud images the base is `generic`, not the smaller `genericcloud`, and the difference is the only thing that makes a graphical guest possible: `genericcloud` ships `linux-image-cloud-amd64`, built without drivers for physical hardware, and DRM goes with them. No `CONFIG_DRM`, no `virtio_gpu` module, so no `/dev/dri` whatever the display device is, so logind's seat0 is not graphical and sddm waits for a display forever. The guest boots, answers SSH, autologs in as far as its configuration goes, and sits on the text console: a failure that says nothing about its cause, which is why `desktop.sh` checks `modinfo virtio_gpu` before it installs anything and names the image to use.

The disk is attached with `discard=unmap` and `finalize.sh` ends in `fstrim -av`. That is the whole of the size management, because the template also sets `skip_compaction`: Packer's own compaction converts the finished disk and renames the copy over the original, and on this host the rename is refused as long as something still holds the file. Trimming from inside the guest reaches the same end state without a second copy. Zeroing the free space, which is what a build does when compaction *will* run, is the one thing not to do here: it allocates every cluster it writes and there is no convert pass left to drop them.

Two more things about the Linux command line, both in `qemu::Launch::args`. The guest gets a `virtio-tablet-pci`, an absolute pointer, which is what makes a click in a VNC viewer land where the cursor is: VNC's `PointerEvent` carries absolute coordinates, QEMU's implicit PS/2 mouse is relative, and the translation between the two is the textbook cause of offset clicks. And the display is `-device virtio-vga` carrying `xres`/`yres` rather than `-vga virtio`, because only the device form takes properties, and those properties are what set virtio-gpu's preferred mode. `SUNLIT_EARTH_VM_RESOLUTION` therefore applies to both providers now, through the shared `provider::console`; on QEMU it overrides `qemu::DEFAULT_CONSOLE` rather than a size fitted to the host's screen, since a VNC viewer scales and has nothing to fit.

A QEMU guest's three loopback ports (the SSH forward, QMP, VNC) are preferred rather than fixed: `create_from_golden` records the first bindable port at or after 2222, 4444 and 5900, and `start` builds the command line from the record, because on a Windows host WinNAT reserves 100-port blocks for Hyper-V and WSL at moments of its own choosing and QEMU exits over a reserved port before it has built the machine. The walk is bounded by `qemu::PORT_WALK`, chosen so the three ranges can never meet. The waits that follow a boot (`wait_ssh`, the session wait, the job poll) consult `Provider::defunct` after every unanswered probe, so a QEMU that exited fails the command within a poll and quotes the tail of `vm.log` rather than sitting out the ten-minute SSH timeout; on Hyper-V only `Off` and an unregistered VM count as a verdict, because a guest still `Starting` and a failed state query are unknowns, not deaths.

A Hyper-V guest is looked at through a basic `vmconnect` session, which shows the framebuffer as it is: the window is the resolution, and nothing about it can be dragged larger. So the resolution is decided before the guest boots. `hyperv::console_resolution` takes the largest mode from `CONSOLE_MODES` that fits the host's work area, or whatever `SUNLIT_EARTH_VM_RESOLUTION` names, and `hyperv::video_script` puts it in both create scripts as `Set-VMVideo -ResolutionType Single`. Between `New-VM` and `Start-VM` is the only place it can go, because the cmdlet refuses to run against a VM that is on. `Single` rather than `Maximum`: `Maximum` advertises a list and leaves the guest to pick, which it does at 1024x768, the very default this exists to replace. What it costs is the guest's own display settings, which are then offered one mode; what it buys is a console that is the right size from the firmware's first frame, sized from the host, where the screen it has to fit on is.

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
      src/assets/    # texture loading + downscale cache, cloud source + updater, texture mailbox
      src/assets/stars/  # the baked HYG blob and its ATTRIBUTION.md (committed data)
      src/engine/    # the engine thread, injectable clock, wallpaper sink
      src/geometry/  # sphere mesh, procedural grid texture
      src/renderer/  # wgpu pipeline, offscreen render, readback
      src/scene/     # camera, sky state (astronomy FFI), sun reference path, sun occlusion, moon placement, datetime
      src/config.rs  # AppConfig, QualityTier, persistence
      src/memory.rs  # per-OS process counters, the metrics CSV, the budget
      src/memory_report.rs  # the four-section report of where the memory is
      src/params.rs  # SceneParams, ParamsDigest, gamma slider mapping
      tests/         # engine, soak, golden, shading, render_pipeline
    sunlit-app/      # Slint shell: window, tray, IPC, config bridge
      ui/main.slint  # MainWindow and TrayIcon
      tests/         # e2e (desktop-gated), slint_ui
    xtask/           # developer tooling: VM orchestration, the icon bake
  assets/
    icon/            # the mark: SVG master plus 32/24/16 variants, and baked/ (committed)
    linux/           # sunlit-earth.desktop and the user-local install script
  textures/          # local JXL assets, not part of the build: the two 8K Earth maps,
                     # the Moon's 1024x512 surface and the 4096x2048 Milky Way
                     # panorama, with PROVENANCE.md beside them
  vm/                # Templates and guest assets, one directory per image slug
    linux/           # Debian 13, four desktops on Xorg, cloud-init seed
    windows/         # Windows 11 Enterprise eval, autounattend, bootstrap
    linux-builder/   # Ubuntu 22.04, a Rust toolchain, no graphics stack
    windows-builder/ # the two scripts that turn a child of the Windows image into a builder
```

The package inside `crates/sunlit-app` is still named `sunlit-earth`, so the binary, `CARGO_BIN_EXE_sunlit-earth`, and `target/release/sunlit-earth.exe` in the release workflow are unchanged by the directory name.

## Architecture

The organizing principle is **headless first**. The engine runs to completion with no window at all; the settings window is one optional client. Hiding the window removes a client, it does not half-suspend the machinery. This is what removed the tray-mode memory leak class, made soak tests possible, and retired the teardown hacks.

### The engine (`sunlit_core::engine`)

One thread owns the wgpu device, the `Renderer`, the texture mailbox, and the schedule. Clients send `EngineCommand`s and receive `EngineEvent`s.

- **Commands**: `UpdateParams`, `SetPreviewSize`, `SetPreviewEnabled`, `RenderWallpaperNow`, `RenderToFile`, `ExportPixels`, `SetTextureResolution`, `ReportMemory`, `SetAutoRefresh`, `Poke`, `Shutdown`.
- **Events**: `PreviewFrame { rgba, width, height }`, `TexturesReady`, `WallpaperSet(Result)`, `Status(String)`.
- **The loop never sleeps on wall time to decide what is due.** It blocks on the command channel with a 50 ms timeout and, on each wake, asks `clock.elapsed()` what is due: the texture drain (5 s), the sky state refresh (120 s), the cloud poll, the memory metrics sample (600 s), and the auto-refresh export. `Schedule::due` recomputes its deadline from `now` rather than accumulating, so a long stall produces one run and not a burst of catch-up runs.
- **Injected `Clock`.** `SystemClock` in production, `MockClock` in tests. `MockClock` advances UTC too, so simulated days really do rotate the Earth. This is what makes 14 simulated days run in 13 seconds.
- **Injected `CloudSource`.** `HttpCloudSource` in production, fixtures in tests. A dedicated cloud worker thread does network I/O and JPEG decoding and never touches the GPU; it parks frames in the mailbox and pokes the engine, which uploads on its own schedule. A poll skipped because the worker is busy retries on the next tick. Which variant it fetches follows the texture resolution rather than the quality tier; see the Texture resolution section.
- **Injected `WallpaperSink`.** `SystemWallpaper` writes a PNG and calls the Win32 API; `CountingSink` lets the soak test run for simulated weeks without touching the desktop.
- **The preview follows window visibility.** The app sends `SetPreviewEnabled(false)` on every hide and `(true)` on every show, so a hidden window costs no readback. The engine keeps rendering regardless (the wallpaper export depends on it); only delivery stops. Re-showing pays back an "owed" frame from the existing texture, since the dirty check would otherwise suppress a re-render and leave the window blank.
- **Preview frames are pixel buffers**, not shared GPU textures. The engine reads its offscreen target back and hands over RGBA bytes; the app wraps them in `slint::Image::from_rgba8`. Slint therefore needs no wgpu feature and shares no device, which is why teardown is ordinary drop order.

### Parameters (`sunlit_core::params`)

`SceneParams` is the single description of what to draw: camera, texture selection, sample count, lighting, clouds, atmosphere, celestial controls, color correction, and the datetime input. There are exactly two translation points:

1. `ui_callbacks::read_params_from_window` / `apply_params_to_window` in the app.
2. `renderer::render_pass::write_uniforms` in core.

The Earth lens lives in `CameraParams::fov_deg` rather than beside `sky_fov` in `SceneParams`, because that is where `OrbitalCamera` already read it from and where `scene::sun_occlusion` already asks for it: wiring it through cost one assignment in `write_uniforms` and no digest entry, since camera floats compare exactly. The consequence is that it belongs to a preset too, and every entry of `PRESETS` carries `DEFAULT_CAMERA_FOV`, so a preset reproduces the framing its zoom was hand-tuned for instead of inheriting whatever lens was on. `CAMERA_FOV_MIN` and `CAMERA_FOV_MAX` (10 and 170 degrees) are the slider's ends and `AppConfig::sanitize` clamps to them, which is the one clamp there beside the texture resolution: `Mat4::perspective_rh` scales by `1 / tan(fov / 2)`, so 0 and 180 are not an ugly picture but no picture at all, and a config file is a text file.

### Celestial sky

`scene::sky::SkyState` is the one astronomy result for each frame. From the selected datetime it builds the J2000 equatorial to world rotation, rotates a geocentric Astronomy Engine sun vector, computes directions and apparent magnitudes for Mercury, Venus, Mars, Jupiter, and Saturn, and places the Moon: a position in Earth radii from `Astronomy_GeoMoon` and a rotation from `Astronomy_RotationAxis`. The Moon is a position rather than a direction because it is the one thing in the sky near enough for the camera's own displacement to matter. The engine passes the whole state to the renderer and the dirty check compares the sun plus two quantized rotation basis vectors. The older subsolar calculation in `scene::sun` remains as an independent equivalence test.

`cargo xtask bake-stars` reads HYG v4.4, excludes its `Sol` row, propagates proper motion to epoch 2026.0, bakes B minus V color and magnitude, and writes 16 byte records after a 12 byte header. `assets::stars` validates the embedded blob and lends its payload directly to wgpu as the static instance buffer. The blob with 15,597 stars and its `ATTRIBUTION.md` ship together under `crates/sunlit-core/src/assets/stars/`; xtask does not depend on `sunlit-core`. The format contract between the two crates is held by a fixture in `crates/sunlit-core/tests/fixtures/`, a CSV and a BIN checked in side by side: the reader's tests load the BIN, and `the_committed_fixture_matches_a_fresh_bake` in xtask bakes the CSV and compares, so a layout change that lands on one side and not the other fails there rather than in whatever the sky looked like afterwards.

The star pipeline draws over the Milky Way's panorama and under everything else. Four generated triangle strip vertices expand every catalog record into an analytic sprite with a crisp core and independently controlled glow, transformed by the shared sky rotation and camera rotation. The glow is a Gaussian with the value at `star_glow_radius` subtracted and the remainder renormalized, so it reaches zero exactly where the sprite quad ends: a bare Gaussian still carries 4.4% of its peak there, and with brightness, glow strength and glow radius all at their maxima that residual draws the quad's own edge as a straight line and puts bright stars in visible squares. `golden_bright_star_halos` is that corner of the parameter space, and `large_crisp_stars` is the other end of the same axis with no halo at all. Earth retains its own perspective lens, `camera_fov`, defaulting to the 20 degrees the presets were framed at; celestial directions use a separate stereographic lens with its own `sky_fov`. Records are sorted by magnitude, so the runtime submits only the prefix inside the selected limit; the shader keeps the same cutoff as a boundary check. Brightness uses compressed astronomical flux. A second buffer with five records carries the planets through the same pipeline, rewritten by the two calls that move the renderer's stored sky and by neither of them when it did not move: the buffer is what the draw reads and the stored inputs are what a replayed export re-encodes, so the two have to name the same instant. Intensity zero omits both draws. Earth then covers the sky, followed by clouds and the atmosphere shells.

### The Sun

The Sun is two draws, because it is two things at once. Its body is a celestial object, so the clipped white disk at its true 0.267 degree angular radius is drawn with the sky, after the stars and before the Earth, where the opaque globe covers whatever falls inside its painted disc. Its glare forms in the observer rather than in the scene, so it is a second quad drawn last over everything, faded by the fraction of the disk the globe leaves visible rather than by whether the disk survived a depth test. One draw could be occluded or overlaying, not both. Both are four vertices generated from `vertex_index` with no vertex buffer, and `render_pass::Sun::select` chooses them the way `Stars::select` chooses the star draw; `sun_glow` at zero takes the Sun out of the scene entirely, body included.

Both quads are sized through the sky lens by the conformal cone-to-disc formula, which is exact rather than a small-angle approximation: stereographic projection maps circles on the sphere to circles in the plane, so a cone of half-angle alpha about a direction at angle theta images as a disc spanning `tan((theta - alpha) / 2)` to `tan((theta + alpha) / 2)` along the radial direction, the first signed so a cone containing the view axis straddles the origin with no special case. The projected plane reaches pixels by one uniform scale, so that disc is a circle in pixels whatever the aspect ratio is. A draw is skipped when the cone's near edge is past the screen corner's own angle, and drawn fullscreen when the cone reaches the antipode, where its image is the exterior of a circle; there is no behind-the-camera case, because the sky lens has no `w` and maps every direction short of the antipode.

`scene::sun_occlusion` is the CPU half, and it measures against the silhouette the viewer can see rather than the true angular limb. The mixed lens puts the painted globe about four and a half times larger on screen than the sky lens's image of the same silhouette at the two default fields of view, so measuring against the second would fade the glare out while the Sun still sits in visibly empty sky in one framing and leave it burning on top of the painted globe in another. In screen space all three shapes are circles: the disk from the cone formula, the globe at `r / sqrt(d^2 - r^2)` normalized by the camera's own field of view, and the atmosphere shell at the same formula with `RAYLEIGH_RADIUS`. The visible fraction is the disk's area outside the globe circle and the transit factor is the part of it inside the annulus, both by the standard circle-circle lens area. The cost is that the answer does not agree with an ephemeris, which is deliberate and is why `angular_visible_fraction` sits beside it as the ephemeris answer nothing reads: the tests compare the two in the one configuration where the lenses agree about scale, which fixes the aspect ratio once both fields of view are chosen. The functions live where the uniforms are written rather than in `SkyState`, because they are a function of the lens and the viewport as well as of the time.

The composition is the Spencer glare model, per fragment, in angular coordinates: the fragment shader inverts the sky lens analytically to recover its view ray, measures the angle to the Sun, and sums a two-lobe inverse-power bloom with the existing `dither()`, ciliary corona needles from an integer hash, and a lenticular halo near three degrees with a blue inner and red outer edge. Measuring in degrees rather than pixels is what makes the composition hold across the 60 to 180 degree sky slider. One angular window takes every sun-centered term to zero before its quad ends, which is the same lesson `star_halo_profile` carries: a term still carrying anything where its quad stops draws that quad's own edge as a line. The needle hash is a PCG integer hash rather than the usual `fract(sin(...))`, because that one feeds a transcendental a large argument where adapters disagree in the low bits and needles are exactly the high-frequency detail that turns such a disagreement into a golden failure; the needle count also comes down rather than aliasing when a tip would be narrower than two pixels. Angular sizes become pixel sizes as the slider moves, so the core takes the same treatment phase A gave the star core: a minimum radius in pixels and the shared `output_pixel_scale` ramp from 1.0 at 1080p to 2.0 at 4K.

`sun_flare` is camera mode and ships at zero: an aperture starburst and three ghosts along the axis from the Sun through the frame's center, which is a device artifact rather than an eye's and reads as smudges in a still. It is the one thing that makes the glare quad fullscreen, because ghosts land outside the glare cone.

Two goldens pin it, `sun_over_the_night_side` and `sun_grazing_the_limb`, both at 60 degrees of sky rather than the 140 degree default: at the default the whole composition lands inside forty pixels of a 512 pixel frame and a reference that lost the Sun entirely still passes at a mean of 1.11 against a tolerance of 2.00. The engine cases carry the default field of view instead, where a count of painted pixels needs no tolerance.

Two more things about the Sun are true on both sides of the CPU and GPU boundary, and neither is visible to a frame the rest of the suite renders. The density ramp and the sky lens's edge radius exist once in `scene::sun_occlusion` and once in `sphere.wgsl`, and `the_shader_and_the_cpu_agree_on_the_two_shared_rules` appends a compute entry point to the production shaders and reads both back off the GPU, at viewport heights away from 1080p because that is where the ramp clamps to 1.0 and every other frame in the suite is rendered. The pan reaches the Sun through four places with a sign in each, and every other frame in the suite is rendered with no pan at all: `a_pan_slides_the_composite_without_shearing_it` pins the three that place something by rendering a frame panned by an eighth of the width and comparing it against the unpanned one moved by 64 pixels, and `a_pan_past_the_frame_corner_still_draws_the_sun` covers the fourth, the reach a draw culls itself against, at a longitude where the Sun sits more than the glare cone's own 30 degrees past an unpanned frame's corner, which is the only place that reach decides anything.

### The Moon

The Moon is a textured sphere at its true position, distance and orientation, and everything about how it lands on screen follows from measuring it from the eye rather than from the geocenter: parallax is exact at every camera distance, including the ones beyond the Moon's own orbit that the zoom reaches, and so is apparent size, which grows as the camera approaches it. `vs_moon` takes each mesh vertex to world space through one model matrix, subtracts the eye, and maps the resulting direction through the same stereographic sky lens `vs_star` uses; that projection is now one WGSL function (`sky_lens_project`) with two consumers rather than a spelling in each. `fs_moon` is the color texture, a hard terminator (a narrow smoothstep on n dot l against the shared `sun_dir`), an earthshine floor, and a brightness. No water, no fresnel, no night texture.

`scene::moon::place_moon` is the CPU half. It assembles the model matrix from `SkyState`'s position and rotation and the mesh permutation `mesh_from_body`, which is the same one the Earth's own map goes through, so one texture convention serves both bodies: `geometry::sphere` puts the north pole on +Y and the prime meridian on +Z, and `assets::texture_loader::orient`'s flip and quarter shift line an equirectangular map up with that. It also floors the disk in pixels the way `place_sun` floors the Sun's, by inflating the matrix's scale, so the two bodies that subtend the same half degree are the same size on screen wherever the floor is active; `MIN_BODY_DISK_RADIUS_PIXELS` is named for both. Inflating on the CPU keeps the floor to one spelling on one side of the language boundary, which is what the two shared sky-lens rules could not have.

The Moon draws opaque and back-face culled, with no depth write and `CompareFunction::Always`, after the Sun's disk and before the Earth. Opaque because it has to cover the Sun's additive disk; back-face culled because that is exact on a convex sphere and a depth comparison between the sky lens and the Earth's would compare two different projections; before the Earth because the composite's rule is that the painted globe covers whatever falls inside its own disc, which is how a star is hidden too. So clouds and the atmosphere shells sit unambiguously in front of the Moon. Two framings are outside what the composite can show, and they are different kinds of thing. A camera beyond the Moon's orbit, where the Moon is really in front of the Earth, is painted over anyway: that one is given up. A Moon at the antipode of the view axis, which is a camera on the line from the Earth to the Moon at any zoom short of the orbit, is refused instead: the stereographic lens has no finite image of a cone that reaches its antipode, `place_moon` says so by leaving its disc empty, and `write_uniforms` gates the draw on that disc. Without the gate the vertices land thousands of units out in every radial direction at once and the mesh's triangles sweep the frame, opaque and before the Earth; `a_moon_at_the_view_antipode_draws_nothing` is that camera. `vs_moon` carries no offscreen guard of its own, unlike `vs_star`, because a sprite's four vertices share one direction and one verdict moves all of them, where a mesh judged per vertex would keep every triangle that straddles the verdict; the whole mesh is inside the one cone `place_moon` measured, so the CPU gate is exactly sufficient.

It is also a second occluder in `scene::sun_occlusion::visibility`, or the free half of an eclipse would be the wrong half: the Moon covers the Sun's white disk by draw order, and without this the bloom, the corona needles and the halo would go on burning around a dark Moon. Its screen circle comes from the same `sky_lens_disc` at the same floored radius, so unlike the globe comparison this one carries no mixed-lens caveat and agrees with an ephemeris. Where both occluders cover the disk at once the larger hidden area wins rather than their exact union, which is exact everywhere but an eclipse at the limb and errs toward more glare there. A Moon that is not drawn hides nothing, which covers both a switched-off Moon and a missing texture.

Three persisted parameters, all in the Celestial group: `moon_brightness` (zero is the switch, the way `sun_glow` and `star_intensity` are), `moon_size` (1 to 8, a multiplier on the radius and nothing else), and `moon_earthshine` (a floor under the unlit face, 0.05 by default so a new moon does not vanish). The honest way to a larger Moon is a narrower `sky_fov`, which magnifies the Moon and the stars around it together; `moon_size` magnifies the Moon alone.

The surface is the NASA CGI Moon Kit at 1024x512 in slot 3, and it is an overlay like the clouds: excluded from `textures_ready` and `textures_pending`, absent without the file, never delaying a wallpaper export. Unlike the clouds its slot is file-backed, so a resolution switch purges and reloads it, and the load is spawned only while `moon_brightness` is above zero. Tests use a generated fixture map (`tests/support`) rather than the LFS asset: the landmarks in it are asymmetric in all three ways the orientation can be wrong, and generating it rather than committing it is what makes the golden reference's bytes reproducible on any machine.

Adding a shader parameter means: the `.slint` property and slider, the `AppConfig` field, `SceneParams` + its `ParamsDigest`, `Uniforms`, and the WGSL. The bridge functions and the dirty check follow from the struct. `params.rs` has a table-driven test that walks every parameter and asserts it changes the digest, so forgetting the dirty check is a test failure rather than a stale-frame bug.

A setting that is not a shader parameter takes a different route, and `texture_resolution` is the example: it is an `AppConfig` field with a widget, but it stays out of `SceneParams` and the digest because it does not describe what to draw, and acting on it means re-reading files and swapping GPU textures, which `push_params` cannot express. Such a setting gets its own `EngineCommand` and its own callback, and is read back in `read_config_from_window_onto` rather than in `write_to_config`.

`ParamsDigest` is the quantized snapshot used for dirty checking: camera floats compare exactly, everything else is rounded to integer thousandths. `datetime` is deliberately not in the digest; what the shader consumes is the sun direction derived from it, and `FrameState` compares that separately along with the render size.

### The renderer (`sunlit_core::renderer`)

`Renderer` owns every GPU object and renders offscreen into its own texture (`RENDER_ATTACHMENT | TEXTURE_BINDING | COPY_SRC`). It knows nothing about windows. Key methods: `render(&SceneParams, &SkyState) -> RenderOutcome`, `resize`, `drain_texture_updates`, `export_image`, `read_preview_pixels`, `textures_ready`, `loading_text`.

Submodules: `gpu_setup` (construction, pipelines, render targets), `render_pass` (uniform encoding, pass encoding, `Overlays::select`, `Stars::select`, `Sun::select`, `Moon::select`, `read_texture_rgba8`), `textures` (slots, mailbox draining, mipmapped upload, `downsample_2x`), `texture_routing` (which mode draws the globe from which slot), `frame` (`FrameState` dirty check), `uniforms` (the 480-byte `#[repr(C)]` struct).

The texture slots are the grid at 0, then one slot per file-backed path in the order the paths arrive (day 1, night 2, moon 3), then the cloud overlay last, because it comes from the fetcher rather than from a file. `SlotLayout` is the one place that says so: it derives the cloud slot and the mailbox's slot count from the number of paths, and maps the four combo box modes onto slots explicitly. Before phase C the blend mode's combo box index and the cloud slot were both three and one integer stood for both, which was harmless only because the blend branch never used it as a slot.

### The app (`sunlit-app`)

- `main.rs`: CLI (clap), logging, config load, then one of two paths. `run_render` is fully headless: no window, no Slint backend, no event loop; it starts the engine with the preview disabled, waits for `TexturesReady`, calls `render_to_file`, and returns an `ExitCode`. `run_app` creates the window, starts the engine, wires the UI, and runs the event loop. CLI flags: `--mode <tray|window>`, `--tray-start <visible|hidden>`, `--ipc-socket <name>`, `--quality <low|medium|high>`, `--texture-resolution <8192|4096|2048>`, `--software-rendering`, `--textures-dir`, `--log-level`, plus the `render` subcommand. `resolve_texture_paths` names the four file-backed textures in slot order, and it asks how large each file is rather than whether it exists: `textures/**` is Git LFS, a checkout without the objects holds pointer files under the same names, and naming one costs a decode failure and an error line where the same run with no file at all is quiet and draws the same picture. The threshold is the 64 KiB the xtask's guest staging and the engine tests already use.
- `engine_client.rs`: `EngineLink` (send commands, push window state as `SceneParams`) and `event_forwarder` (engine events to the window). Preview frames cross the thread boundary through a latest-value mailbox with a single pending wake-up: the newest frame replaces the parked one and only one `invoke_from_event_loop` closure is ever in flight.
- `ui_callbacks.rs`: callback registration grouped into mouse, change, and action callbacks; every one of them ends in `link.push_params(&window)`. Also the config bridge (`apply_config_to_window`, `read_config_from_window`) and `defer_combobox_indices`.
- `ipc.rs`: opt-in control channel over `interprocess` local sockets. Commands: `quit`, `show-window`, `hide-window`, `export-test`, `query-memory`, `memory-report`, `set-wallpaper`. Fire-and-forget, with `SIGNAL:` lines on stdout as the reply channel. `export-test`, `query-memory` and `memory-report` are answered on the listener thread, so they work while the event loop is idle. `query-memory`'s single `SIGNAL:memory rss_bytes=... peak_rss_bytes=... private_bytes=...` line is a parsing contract the e2e suite depends on and must stay byte-identical; `memory-report` is a separate command for that reason, and brackets its many lines with `SIGNAL:memory_report_begin` and `SIGNAL:memory_report_end` rather than promising a line format.
- `tray.rs`: the baked 32x32 icon bytes, the tray callback wiring, and single-instance enforcement. The tray icon itself is a `SystemTrayIcon` component in `ui/main.slint`, so Slint owns the platform integration.
- `session_end.rs`: Windows only. An invisible top-level window on its own thread that answers `WM_QUERYENDSESSION` and quits the event loop on `WM_ENDSESSION`, so a reboot does not have to wait for Windows to kill the process. winit handles neither message, so without this nothing in the app ever learned the session was ending. The decision is a pure function (`classify`), unit-tested everywhere; the Win32 window is tested by sending it both messages.
- `mouse_math.rs`: pure functions for mouse interaction (globe drag with tilt correction, frame drag, orient drag, tilt drag, zoom scroll). No Slint dependency; unit-tested with `proptest` invariants.

### UI (`ui/main.slint`)

`MainWindow`: resizable split layout, controls panel in a `ScrollView`. Top-level controls are "Set as Wallpaper", "Load Defaults" and "Reset", and a 3x3 grid of camera presets. Below is a collapsible "Advanced" section with `GroupBox`es for Camera Position, Camera Orientation, Framing, Date / Time, Clouds, Celestial, Atmosphere, Lighting, Color Correction, and Rendering. Camera properties are `in-out` with `<=>` slider bindings. A `TouchArea` over the image handles drag and scroll.

Every row in that section is one of three components rather than a hand-written layout: `SettingRow` (label, slider, value), `SettingCheck` and `SettingCombo`. Each carries a `hint`, the one-sentence hover text, and each has a `Tooltip` covering the whole row. That is why their roots are `Rectangle`s with the layout inside: the compiler lowers a `Tooltip` to a full-size sibling area, and inside a layout that area would claim a cell of its own. A row nested under a `SettingHeading` sets `indent`, which shifts the label right and takes the same width off it, so the sliders of a group stay in one column however deep the nesting is. Each instance keeps the element id the row's slider used to carry (`longitude-slider`, `rayleigh-intensity-slider`), since those are what `tests/slint_ui.rs` looks the advanced section up by.

The tooltip's content is the custom-content form and not `Tooltip { text: ... }`, because the built-in content does not wrap and a sentence is wider than any screen. `Hint` is that content: the compiler still wraps it in the built-in `ToolTipImpl`, which is where the background and the border come from, and which sizes the popup from its children's preferred width. A `Text` reports the width it would take unwrapped however narrow the element around it is, so `Hint` declares `min-width`, `max-width` and `preferred-width` rather than a `width`, which the language refuses to have alongside them; the height then follows from the wrap at that width. Measured in the Linux guest under KDE, one boot per revision.

Celestial is subdivided: Sky (the sky lens and the Milky Way), Sun, Moon, Stars. The subsections are what let the labels drop the noun they repeated (`Star brightness` is `Brightness` under Stars), which is what makes one label column wide enough for every group.

`AboutWindow` is an ordinary exported window component. `about::AboutController` creates it on first use, supplies the package version and one attribution list owned by Rust, then reuses the handle. The settings control and tray callback clone the same controller.

`TrayIcon` inherits `SystemTrayIcon`: menu (Open, Refresh Now, checkable Auto-refresh, About, Exit) and `clicked()` to toggle the window. Only properties *declared* on the derived component are exposed to Rust, so the inherited `icon` is bound to a declared `tray-image` property. A `SystemTrayIcon`-rooted component implements `StrongHandle` but not `ComponentHandle`, so there is no `as_weak()`; the handle is kept in an `Rc`.

### The app icon

The mark is four SVGs under `assets/icon/`: a master and three variants for 32, 24 and 16, built on one rule. What cannot survive the raster drops (the dawn band and the warm limb arc at 16, the aurora's soft glow layer below 48) and everything that stays grows in canvas units as the raster shrinks (the outline, the aurora arcs, the glare's reach, the sun core), so no element fades into sub-pixel noise. All four share the master's 64-unit viewBox, so one uniform scale renders any of them. `commands::bake_icon::source_for` is the mapping, and it takes the largest variant at or below the target size rather than the nearest: a variant carries the detail its own size can resolve and no more, so borrowing from above is the resample the variants exist to avoid.

`cargo xtask bake-icon` is the only thing that turns SVG into pixels, and its outputs are committed. A build therefore gains no rasterizer and nothing runs one on a user's machine; the bake reruns when a source SVG changes, and `the_committed_bake_matches_a_fresh_one` fails when one changed without it, comparing the whole tree byte for byte against a fresh render. That is what makes the committed bytes trustworthy as data. `bake` returns the bytes rather than writing them, which is the seam that test uses.

It writes four kinds of thing into `assets/icon/baked/`. `sunlit-earth.ico` carries nine sizes, 16 through 256, as 32-bit BMP below 256 and PNG at 256, which is the layout every Windows icon tool produces and the only encoding the shell reads at the largest size. The hicolor set is seven PNGs laid out as the freedesktop theme spec wants them, ready to be copied into a theme directory. `tray-32.rgba` is raw straight-alpha pixels. `about-256.png` is for the About window the celestial phase A plan introduces.

Each surface consumes one of those, and each choice is about who resamples:

- **The exe**, and through it Explorer, the desktop shortcut, and the taskbar. `crates/sunlit-app/sunlit-earth.rc` names the ICO at ordinal 1, since the shell shows the lowest-ordinal icon resource, and `embed-resource` compiles it in the build script that already existed. `manifest_required()` rather than the optional form: a build that quietly produced an icon-less exe because no `rc.exe` was found is worse than one that stops.
- **The tray**, on both platforms, through Slint's `SystemTrayIcon`. `tray::create_icon` is `include_bytes!` of `tray-32.rgba` wrapped in a `SharedPixelBuffer`. Raw pixels rather than a PNG because this is the app's only image and the bake owns the pixels at exactly the size the tray wants; a const assertion on the length makes a bake at another size a compile error rather than a startup failure in tray mode, which is the one path no headless test reaches.
- **The settings window**, through `Window`'s `icon` property in `ui/main.slint`, bound to the master SVG rather than to a raster. The winit backend renders the icon at 64 logical pixels times the scale factor, so a vector source lands exactly on whatever that comes to, and every size it can ask for is inside the master's range. Slint already carries resvg for `@image-url`, and the Rust build embeds such resources by default, so this costs the app nothing new and the binary reads no path at runtime. Measured on Windows: `WM_GETICON` returns a 64x64 `ICON_SMALL` and no `ICON_BIG`, because winit sets only the small one from `set_window_icon`; the taskbar takes the exe resource, which is the point of having both. What the path costs is one known artifact: the backend's `icon_to_winit` multiplies each channel by alpha to reach the straight-alpha bytes winit wants, and Slint's SVG rasterizer hands it a buffer that is premultiplied already, so every partially transparent pixel arrives darker than it should. That is 7.4% of the 64 px icon, the antialiased outline and the soft rim of the glare, and at the 16 px a title bar draws it comes to a mean of 1.7/255 with seven pixels off by as much as 50. Feeding the baked 64 px PNG instead would not remove it: every encoded image Slint loads lands in the same premultiplied buffer, so the raster route keeps the artifact and additionally gives up the reason the icon is a vector at all, which is that the backend asks for 64 times the scale factor and any fixed raster is then resampled. The one straight-alpha route through Slint's API is `Image::from_rgba8` from Rust, which is why the tray is unaffected: its raw bytes go in as straight alpha and pass through untouched.
- **Linux launchers, docks and app switchers**, through `assets/linux/sunlit-earth.desktop` and the hicolor set. `Icon=sunlit-earth` is a lookup by name and nothing else connects the entry to the files, so `the_desktop_entry_asks_for_the_icon_the_bake_writes` compares the key against the names the bake files under; a mismatch is a generic placeholder with no error raised anywhere. There is no package yet, so `install-user.sh` beside the entry copies both into `$XDG_DATA_HOME`, with `--exec` to rewrite the `Exec` line for a binary that is not on `PATH`. The scalable slot is the master SVG itself, copied rather than baked. `assets/linux` is inside what the shell-script syntax check walks, because that script runs on someone else's machine.

What the Linux guest showed, one boot per desktop: the tray icon and the title bar take the mark, and the taskbar entry takes it only after the install, because a task manager matches a window to a desktop entry by `WM_CLASS` and then draws that entry's icon rather than the window's own. What KDE's launcher search offered was that same running window rather than the installed entry, so nothing seen there says Kickoff indexed the file. A KDE menu reads sycoca rather than the directory, so the install script rebuilds it through whichever of `kbuildsycoca6` and `kbuildsycoca5` is on `PATH`, which in the guest is the first and only that one; the rebuild exits zero over a bare SSH session, and whether Kickoff then lists a fresh entry is still unobserved. Before the install it drew `applications-graphics`, the theme's blue globe, which the hand-over entry names. `WM_CLASS` is `"sunlit-earth", "sunlit-earth"`, which winit derives from the binary name, and that is what makes both the match and `StartupWMClass` work without the app setting anything. `_NET_WM_ICON` is present at 64x64, so a task switcher that prefers the window's own icon gets one too. The desktop-view icon in Plasma kept the old one after the file changed under it, which is a shell cache and not a lookup: the shipped install script writes to `$XDG_DATA_HOME/applications` and never to the desktop directory.

Two things the bake is not. It is not a fidelity guarantee: the mark leans on a radial gradient with a displaced focus and an alpha mask that hides the sun behind the globe, both of which renderers disagree about, so the master was checked against Blink at 256 and the two agree to a mean channel difference of 0.175/255 over the opaque pixels, with 0.55% of channel samples over 8 and all of those on antialiased edges. And it is not a taste check: `bake-icon --review DIR` writes the small rasters and a contact sheet, both shell chromes at 1x and magnified six times with nearest neighbour, and the judgment about whether 16 and 24 read is still a person's.

### The Milky Way

The diffuse band is a panorama of the whole celestial sphere sampled per pixel, drawn first in the pass so everything else in the sky sits on it. `vs_milky_way` is the four-vertex screen quad the sun draws generate from `vertex_index`, and it is always the whole frame: the sky lens has an image of every direction short of the antipode, the antipode is past the frame's corner at every field of view the slider offers, and so there is no region to leave undrawn and nothing here to cull. `MilkyWay::select` is the only gate, on the intensity and on whether the texture has arrived, which is `Stars::select`'s shape and not the Moon's: a fullscreen quad has no geometry that can fail to appear, so nothing about the frame can narrow it.

`fs_milky_way` inverts the chain a star sprite goes through. `milky_way_direction` calls phase B's `sky_lens_direction` on the framebuffer position and then transposes the view matrix and `world_from_eqj`, which makes it the exact inverse of `sky_lens_project` after `view_from_eqj`, the forward composition factored out of `vs_star` when this became its second consumer. That pairing is what puts the panorama at the same scale and orientation as the sprites on top of it, and it is three places a sign can be wrong, so `the_panoramas_reconstruction_inverts_the_projection_it_sits_under` in `tests/render_pipeline.rs` is a compute entry point appended to the production shaders that feeds directions through the forward pair and back, over both ends of the field of view and both signs of pan: the round trip holds to 6.5e-7 of chord distance on `warp` and 1.5e-4 on `lavapipe`, against a tolerance of 1e-3, where dropping either transpose gives 1.229 and 0.546 on both.

`milky_way_uv` is the panorama's own layout, and the one constant in it is `PANORAMA_RIGHT_ASCENSION_ZERO`. The asset is a standard astronomical all-sky map (right ascension zero at the center, increasing to the left, north up, which is the opposite handedness from the Earth's and the Moon's maps because a sphere seen from inside runs the other way round from one seen from outside), and `assets::texture_loader::orient` mirrors and quarter-shifts every equirectangular source it loads. The mirror is what turns right ascension the right way round for this map and the shift is what moves its zero a quarter of the way across, so the shader undoes the shift and nothing else. `textures/PROVENANCE.md` records the measurement that this is the source's layout, against eleven sky positions, because the SVS does not document it and the two readings differ by a mirror that the galactic center alone cannot tell apart.

The wrap is handled with explicit gradients rather than patched later. `atan2` jumps a full turn across its branch cut, so a hardware derivative of `u` there is a whole texture width and the sampler answers that column with the coarsest mip: the average of the entire panorama, drawn as a curve from pole to pole. The direction is continuous across the cut, so `milky_way_uv_gradient` carries its derivative through the map by the chain rule and `textureSampleGrad` takes the result. `the_wrap_column_is_not_a_band_of_the_coarsest_mip` is what holds it, and two things about that case are worth knowing before editing it. Its fixture is bands of declination rather than a gradient, because a linear ramp is a fixed point of the mip chain and a case built on one passed with the gradient sample deleted. And its metric is a second difference over the sky pixels rather than a per-column count, because the cut is a curve on screen and two pixels wide, a derivative being a property of the fragment quad.

What the layer costs is seven transcendentals per pixel over the whole frame, which is negligible on a real adapter and is not on a software one: measured, the panorama adds 0.2 ms to a 1920 by 1080 frame and 0.9 ms to a 4K one on this machine's GPU, and 133 ms and 555 ms on `warp`, against the cloud shell's 16 ms at 1080p. It is not the texture fetch, which the sampler's anisotropy makes no difference to. Nothing in the suite pays it except the two goldens and the panorama engine cases, because every other headless configuration leaves the slot empty; the soak test's fourteen simulated days draw no panorama at all. The plan's departure 6 has the whole table and names the algebraic identity that would remove three of the seven from `sky_lens_direction`, which is not taken because that function is the Sun's too and substituting it moves every reference.

The layer's slot is the fourth file-backed one and the only texture whose source width sits between two of the caps, so `memory::milky_way_texture_bytes` is 42.7 MiB at the 8192 and 4096 settings and 10.7 MiB at 2048, where the halving cache serves the downscale. It is an overlay, like the clouds and the Moon, so `textures_ready` and `textures_pending` exclude it and a checkout without the Git LFS object draws a sky without a band rather than waiting for one.

Two goldens pin it, `panorama_behind_the_stars` and `panorama_at_a_narrow_sky`, the same night-side camera at the two ends of the field-of-view slider, both with the banded fixture rather than the real asset. `base_params` switches the layer off for every other case: it covers the whole frame, so leaving it on would move all eleven other references and bury what each of them is for. What a fixture cannot show is the asset's own layout, and `the_real_panorama_has_the_galactic_plane_where_the_plane_is` in `tests/engine.rs` is where that lives, sampling the rendered sky at the galactic center, both galactic poles and two stretches of the plane and asserting the ordering a mirrored reading inverts. Its sibling `no_bright_star_is_baked_into_the_real_panorama` holds the other property of the file itself, that the layer excludes the bright stars the sprites draw: on the asset as it sits on disk, at the position of every catalog record inside magnitude 1.3, a 3x3 texel core against the 41x41 window around it reads 1.26 at worst against a bound of two, where a star baked into the layer would saturate its texels and read 2.3 to 5. Both skip with a printed reason without the Git LFS object.

### Clouds on the night side

`fs_cloud` shades the shell rather than the ground, and the difference is three things. Its
ramp is centered on the shell's own tangent condition, `sqrt(1 - 1/r^2)`, which at
`CLOUD_SPHERE_RADIUS` is 0.0547 and puts the cloud terminator 3.14 degrees nightward of the
globe's, the 9.6 km of altitude being what keeps a cloud top in the direct beam that much
longer; `fs_rayleigh` computes the same expression for its own limb as `earth_limb_ndotv`. Its
width is `params::CLOUD_TERMINATOR_WIDTH`, a constant at 0.18 rather than a slider, wider than
the globe's 0.1 because twilight goes on lighting a cloud top for several degrees after the
beam has gone. That constant is also the sentinel fix: `write_uniforms` puts -1.0 in
`terminator_width` outside blend mode, as the flag `fs_sphere` reads to ignore the sun, and
`fs_cloud` used to read the same uniform, which reversed its `smoothstep` edges in the three
single-texture modes.

The night value is `cloud_night`, 0.25 by default, and it is an appearance parameter rather
than an irradiance. The plan's own finding is why: the day side is about 18.6 stops brighter
than the night side, by the sun-to-full-moon ratio and independently by the exposure settings
of "Hello, World" against those of "The Blue Marble", which is more than any sensor or eye
holds at once, so the frame is a tone map and the number is a decision about how much of that
gap to compress. It is chosen against the texture it draws over: Black Marble's unlit land
reads 0.137, its Antarctica and a typical city cluster 0.20, the Nile delta 0.36 and its cores
1.0, so 0.25 puts a night cloud above every unlit surface and well below the lights.

`cloud_city_gain`, 0.7 by default, is the light a city throws onto the cloud base over it. It
is why the cloud bind group's binding 3 is the night map rather than the dummy, and the mip
level it samples at is derived from the source's own width, `max(log2(width / 1024), 0)`, so
the blur is a fixed angle instead of one that reaches three times as far at 8192 as at 2048.
Zero gates the sample rather than scaling it, so a run at zero reaches the same pixels a build
without the term does, and `city_light_at_zero_draws_the_frame_a_missing_night_map_draws`
holds that byte for byte; what no case in the suite distinguishes is the gate from a multiply
by zero, which is the plan's departure 4. Which of the night map and the dummy sits on binding
3 is a property of the session and not of the mode, since a load is spawned only for the slot
the current mode draws from and nothing unloads one: a session that has been in blend mode
keeps the map there in every mode afterwards, which is what both of those cases are built on,
and `the_dummy_night_map_lights_no_cloud` is the one that renders a deck against the dummy and
asks what it adds. The cost is that the group spans two slots with different lifetimes:
`purge_file_backed_slots` destroys the night texture without touching the cloud slot, so both
it and `process_decoded_textures` rebuild the group, and a miss is a validation error on the
next draw rather than a wrong pixel. It is an extrapolation rather than a published technique,
and it samples the whole night map rather than only its lights, so it lifts a night deck over
unlit land too.

Three references pin all of it, and not interchangeably: `clouds_across_the_terminator` holds
the floor, `cloud_terminator_close_up` holds the shift and the width, and
`clouds_lit_by_city_light` holds the coupling. They are the suite's first blend-mode goldens,
which is why `check_golden_in` waits for `day_texture` and `night_texture`: nothing spawns
those decodes until a case asks for the mode, and the first one to do so exported the frame
the fallback draws.

### Quality tiers

`QualityTier` (low, medium, high) is persisted in the config and overridable per run with `--quality` (the override is not written back). It has no widget in the settings window, which is why `read_config_from_window` is a read-modify-write against the stored config rather than a fresh `AppConfig::default()`: any persisted setting the UI does not manage has to survive a save untouched. It caps the MSAA sample count (1, 4, unlimited) and the preview width (1280, 1920, unlimited, aspect preserved). Default: low in debug builds, high in release; `EngineConfig::headless` pins low so tests do not depend on the build profile.

The tier does not select the cloud image variant; the texture resolution does. The tier says how much work a frame is allowed to be, and the cloud overlay is texture memory, which is what the other setting is for.

### Texture resolution

The two local surface textures are 8192 wide, and the Moon's is 1024, which is at or below every cap the setting offers, so the halving cache never touches it and it is loaded at its own width whatever the setting says. The Milky Way panorama's 4096 is the one width between two caps: the two upper settings load it as it is and the lowest halves it through the cache, which is what `the_panorama_follows_the_texture_resolution_cap` measures. `AppConfig::texture_resolution` decides what width the two Earth maps are loaded at, from the three in `config::TEXTURE_RESOLUTIONS` (8192, 4096, 2048), and the Rendering group offers them as a combo box. It also selects the cloud image variant, which is the third thing that scales with it; the three offered widths map one to one onto the three variants the upstream service publishes. The default is 4096, so an install whose config predates the setting moves to 4096 and anyone who wants the full width picks it once. `--texture-resolution <8192|4096|2048>` overrides it for one run; clap validates the three values, and a config file holding anything else is repaired to the default by `AppConfig::sanitize` on load, which is where the check belongs since a config file is a text file.

The halving is the same box filter that builds the mip chain, so a 4096 texture is the 8192 texture's first mip level exactly. That is why the default costs so little: on the software adapter the 800x800 render the e2e case checks is byte-identical at 8192 and 4096, and at 2048 the sampled land and ocean pixels move by at most 1/255. On a real adapter it is not quite identical, because anisotropic sampling can ask for a level of detail finer than the narrower texture's level 0 near the limb: measured on this machine's GPU, 8 pixels of 640,000 differ by one channel step. Anything that renders the globe larger than a few hundred pixels across will show the difference properly; the settings window is where to change it back.

Below 8192 the width is reached by halving, and the result is cached on disk: `assets::texture_cache::load_at_resolution` looks for `texture_cache/<stem>.<width>.png` under the same directory the cloud cache uses (so `SUNLIT_EARTH_CACHE_DIR` covers both) and validates it against a sidecar TOML recording the source's size and modification time. On a miss it decodes the source, halves it with the same box filter the mip chain uses, and writes the PNG temp-then-rename, under a temporary name carrying the process id so two writers of one entry cannot truncate each other. The source is stamped before the decode and the stamp re-read after it, because a source replaced during the seconds an 8K decode takes would otherwise be recorded as where the old pixels came from, and that entry would validate forever. Measured on the real assets in a debug build: 3.2 to 3.7 s cold against 0.2 to 0.8 s warm per texture, with the render byte-identical either way. A cached file is a plain downscale in the source's own orientation, which is why the orientation fixes are a separate `texture_loader::orient` rather than part of the decode: reading a cached file back is the same `load` a source goes through. The width is a cap, so a source narrower than the chosen width is loaded as it is.

Changing the setting at runtime is `EngineCommand::SetTextureResolution`, not a params push: it decides which pixels to load rather than what to draw. The renderer clears the bind groups and views of the file-backed slots, calls `Texture::destroy` on the textures they held, and only then lets the reload allocate, in the same nil-before-recreate order as `gpu_setup::replace_render_textures`. `TextureSlot` keeps its `wgpu::Texture` so there is something to destroy, and `last_rendered_index` goes back to the grid, which is the one slot `render` may assume is loaded. `tests/engine.rs` measures the point of all this on the real assets: 1259.7 MiB of private bytes at 8192 against 616.5 MiB at 2048, the Moon's 2.7 MiB resident in both, and it skips with a printed reason where `textures/**` is still Git LFS pointers.

The cloud overlay follows the same command but by a different route, because it comes from the network rather than from disk. `SetTextureResolution` deliberately does not purge the cloud slot: the switch must not depend on the network, and a cloudless globe while a download runs is a worse picture than one at the previous variant. What it does instead is point the fetcher at the new variant and ask for a poll now, and the existing update path replaces texture, view, and bind group together when that poll lands, which the soak test already proves frees the old one. Offline, the old variant stays for as long as the outage lasts, which is intended and logged. The worker reads its target before every poll, inside the retry loop rather than outside it, so a switch made during an outage changes what the next attempt asks for rather than queueing behind an attempt that may be minutes from finishing. The disk cache is keyed by variant (`clouds_cache_<w>x<h>.jpg` and its meta sidecar), so a switch can never be answered with the previous variant's bytes and a switch back finds what it left behind; an entry a run at another resolution wrote is dead weight the current one never reads, which is also what makes the change safe to roll back. The switch posts that entry as it adopts it, which is the step that makes a switch back visible at all: the poll it asks for sends the entry's own `ETag` to the entry's own URL, and inside the upstream refresh window that is a 304, which posts nothing. Nothing in production calls `post_cached` after startup, so without the switch doing it the overlay would sit at the previous variant until upstream published again. A variant with nothing on disk posts nothing, which is the same clause that keeps the old clouds up while a download runs.

`SUNLIT_EARTH_CLOUD_URL` still wins over all of it, and `SUNLIT_EARTH_NO_CLOUDS` is untouched. What the override serves has no variant, so it caches under `clouds_cache_override` rather than under a name claiming a size nobody checked; otherwise a run against a stub would leave that image in `clouds_cache_4096x2048.jpg` and the next ordinary run would put it on screen. With the override in force every resolution resolves to the same URL, and `set_resolution` compares URLs rather than variants, so a switch then moves nothing and keeps the `ETag` it has. `CloudUpdater::new` points the source at the URL its cache entry is named after, so the name and the bytes behind it cannot disagree whatever the caller built the source with.

The frames between the purge and the reload show the procedural grid, which is fine for a preview and not fine for a desktop, so `publish_wallpaper` holds one request back while `Renderer::textures_pending` says a texture the current mode needs is on its way, and makes it at the end of the tick that texture arrives on. That covers every caller that publishes, since they all reach that one function: the button, the tray's "Refresh Now", the IPC `set-wallpaper`, and the auto-refresh schedule. What comes before the wait is the sink's own `check_supported`, so a platform with no wallpaper setter refuses at once rather than after seconds of waiting for textures that were never going to change the answer. `textures_pending` is deliberately not the negation of `textures_ready`: a slot with no file behind it, and one whose decode failed and had its path cleared, are terminal, and waiting on either would be waiting for something that is never going to arrive.

A decode of the old width can still be running when the width changes, so every load carries the `texture_generation` it was spawned in, and two places use it. `process_decoded_textures` discards a post from a superseded generation and touches nothing else: `loading` names the decode that is on its way to a slot, a discarded post is never that decode (the generation moves only in `set_texture_resolution`, which purges every file-backed slot in the same call and clears the flag there), and clearing it again would claim a live load had stopped, which puts a second decode of the same 8K source in flight beside the first. `TextureMailbox::post` refuses to let a stale arrival overwrite a parked message from a newer generation, which is the half that matters: the consumer discards a stale post on sight, so overwriting a fresh one there would destroy the only copy of the texture anyone wants and leave the slot empty for the rest of the session. The cloud fetcher posts no generation at all, and an unstamped message is neither held back nor held onto. Its slot is never purged, so there is nothing for a stamp to protect: a fetch of the old variant that lands after a switch is a cloud layer at the previous width for one poll, which is exactly what the switch deliberately leaves on screen while the new one downloads, and the next poll replaces it. The grid stays procedural at every width. `EngineConfig::mailbox` exists so a test can post that arrival directly, the same way the clock and the cloud source are injected, because the ordering needs a decode still running when the resolution changes and no amount of waiting makes that reliable; its slot count is asserted against the engine's own before the device is opened, since a seam that disagrees either drops posts or hands the consumer an index it does not have. Each of the three properties has a test that fails when the line carrying it is reverted: the mailbox guard, the discard, and the purge's own clear, which is what lets a reload start while the superseded decode is still running.

The setting has a widget, which makes the CLI override awkward in a way `--quality` is not: the window has to show what the engine actually loaded, but a one-run flag must not reach the config file. `EngineLink` therefore carries a flag, set at startup when the argument was given, that makes a save keep the stored width instead of reading the combo box; the combo box's own callback, Reset, and Load Defaults each clear it. That rests on a Slint property set from Rust not counting as a selection, which `test_setting_a_combo_index_is_not_a_selection` pins.

### Sample counts

An MSAA sample count the adapter does not support is not a warning inside wgpu, it is a validation error that kills whichever thread builds the render target. Two layers guard it, and they are not redundant:

1. **The combo box** is built by `renderer::build_aa_options(adapter_supported, tier_cap)`, so the UI only ever offers counts that are both supported and within the tier.
2. **The engine resolves every requested count** through `renderer::resolve_sample_count` in `Engine::new` and again on every `UpdateParams`, and logs a `warn!` when it has to fall back. This is the single source of truth, and it is the one that matters: a config file, a hand-edited value, or a combo box index saved on a machine with a different GPU all arrive as a bare number in `SceneParams` and never go through the combo box.

The rule is "the highest supported count at most the requested one, otherwise the lowest on offer". `tests/engine.rs` starts an engine at the High tier (which does not cap) with `sample_count = 64` and asserts a frame still arrives.

### Shaders

`shaders/blend.wgsl` and `shaders/sphere.wgsl` are concatenated at load time by `renderer/gpu_setup.rs`.

- `blend.wgsl`: `blend_fragment()` (day/night blending with diffuse shading and a per-channel `min(night, day)` clamp), plus `apply_gamma()` and `adjust_saturation()`.
- `sphere.wgsl`: star sprite and sphere vertex transforms, texture sampling, uniforms. Single-texture mode uses `terminator_width < 0` as a sentinel, and in that mode the shader ignores the sun entirely. `schlick_fresnel()` drives both specular modulation and the diffuse color shift on ocean pixels. `fs_cloud` applies the cloud floor and gamma, and shades the shell against its own geometry; see Clouds on the night side. Three concentric atmosphere shells, each with its own vertex/fragment pair: `vs_rayleigh`/`fs_rayleigh` (radius ~1.015), `vs_nightglow_orange`/`fs_nightglow_orange` (~1.014), `vs_nightglow_green`/`fs_nightglow_green` (~1.015). The Sun is `vs_sun_disk`/`fs_sun_disk` for its body and `vs_sun_glare`/`fs_sun_glare` for the observer's glare, both quads generated from `vertex_index` alone. The Moon is `vs_moon`/`fs_moon`, the sphere mesh through one model matrix and then `sky_lens_project`, which is the sky lens's forward projection factored out of `vs_star` when the Moon became its second consumer. The Milky Way is `vs_milky_way`/`fs_milky_way`, a screen quad that is always the whole frame, sampling the panorama through `milky_way_direction`, the inverse of `sky_lens_project` after `view_from_eqj`. Draw order: Milky Way, Stars and Planets, Sun disk, Moon, Earth, Clouds, Rayleigh, Nightglow Orange, Nightglow Green, Sun glare. The pass clears to (0.005, 0.005, 0.01), which is near black because there is a real sky on it now; three engine cases in `tests/engine.rs` hardcode the resulting pixel as `[1, 1, 3, 255]`.

### Wallpaper export

The engine renders at the sink's native resolution using temporary GPU textures with `COPY_SRC`, reads them back through a staging buffer with 256-byte row alignment, encodes PNG, saves to `%LOCALAPPDATA%\SunlitEarth\wallpaper.png`, and applies it with `SystemParametersInfoW`. PNG rather than TIFF because Windows preserves PNG wallpapers losslessly; TIFF wallpapers are JPEG-transcoded at 85% quality and band visibly in smooth gradients.

### Memory reporting

Two things measure memory, and they answer different questions. `memory.rs` is the process-wide one: three counters per platform, a CSV the watchdog appends to, and a soft budget that emits a `warn!` when private bytes cross it. `memory_report.rs` is the where-is-it one: `MemoryReport` in four short sections, assembled on the engine thread because that is where the device is, reachable from a test through `EngineHandle::memory_report`, from a running app through the `memory-report` IPC command, and once per launch as a `debug!` dump the first time the textures are ready.

The four sections are the process counters, wgpu's internal counters (`Device::get_internal_counters`), the backend allocator's live allocations (`Device::generate_allocator_report`), and the renderer's own table of what it believes it owns. The last two next to each other are the point: the day the columns disagree is the day there is a leak. Discipline keeps it readable rather than complete: the allocation section aggregates by GPU label, lists the ten largest groups of at least 1 MiB, and rolls everything else into one line; the expected table lists every texture over the same floor and totals all of them. Only the section names are a contract, and nothing parses the report, unlike `query-memory`'s single line.

Both wgpu queries degrade rather than vanish. `generate_allocator_report` is implemented for D3D12 and Vulkan and returns `None` elsewhere, so on Metal the section says so and names the adapter; the counters need the `counters` cargo feature, which is on workspace-wide, and a backend that does not maintain one reports zero, which D3D12 does for the allocation count. The adapter slug is printed because it is what says whether the GPU bytes overlap the process's private bytes: on WARP and lavapipe they do, on a real GPU they mostly do not.

The budget is a function of the texture resolution rather than one constant (`memory::private_bytes_budget`): a cold start, plus headroom, plus the three textures that width keeps resident, plus the Moon's 6 MiB, which is a constant because its file is narrower than the narrowest cap and is therefore loaded at the same width whatever the setting is. The cold-start half does not shrink with the setting, because a cold downscale cache reads the full-width JXL source whatever width it was asked for. At 8192 it comes to the same 3 GiB the original measurement was taken against. Tests pin both directions: the budget clears a cold-cache first run at every resolution, and stays under twice one, so it is neither a warning nobody reads nor a warning nobody gets. Every resolution is held to the one peak that was actually measured (2.43 GiB, at 8192, in a release build) rather than to a smaller figure derived from the budget's own decomposition, which would move with it and assert nothing. That makes 2048 the binding case, since it gets the smallest resident allowance and has the same 8K decode to pay for.

### Environment knobs

All `SUNLIT_EARTH_*` variables that carry a value go through `sunlit_core::env_override`, which treats unset and blank the same.

| Variable | Effect |
|---|---|
| `SUNLIT_EARTH_CLOUD_URL` | Overrides the cloud image URL. Wins over the variant the texture resolution selects. |
| `SUNLIT_EARTH_CLOUD_POLL_SECS` | Overrides the poll interval. |
| `SUNLIT_EARTH_CACHE_DIR` | Overrides the cache directory, holding both the cloud image and the downscaled surface textures. |
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
| `SUNLIT_EARTH_VM_RESOLUTION` | Either guest console's resolution as `WxH`, read through `provider::console`. Unset means the largest of `hyperv::CONSOLE_MODES` that fits the host's screen for a Hyper-V guest, and `qemu::DEFAULT_CONSOLE` for a QEMU one. |
| `SUNLIT_EARTH_REPO` | The repository root, for running the xtask binary from outside its checkout. Defaults to the compile-time location of the crate. |

### Notable dependencies

- `wgpu`: the `counters` feature is on workspace-wide, which turns its internal byte and object counters from compiled-out no-ops into relaxed atomic adds on resource create and destroy. That is what gives the memory report two totals measured by wgpu rather than only the ones we compute; the cost is expected to be unmeasurable next to a texture upload, and the feature is one line to revert if profiling ever disagrees
- `astronomy-engine-bindings`: C FFI bindings to the Astronomy Engine library (requires `clang` at build time for bindgen)
- `image`: PNG/JPEG encode and decode. `jxl-oxide`: the JPEG XL decoding hook
- `time`: UTC decomposition for astronomy
- `tracing` / `tracing-subscriber` / `tracing-appender`: `max_level_trace` with `release_max_level_warn`; `EnvFilter` respects `RUST_LOG`; non-blocking stderr writer with `FmtSpan::CLOSE`
- `ureq` (rustls): cloud fetching. `crossbeam-channel`: engine command and reply channels
- `interprocess`: local socket IPC. `single-instance`: the OS mutex (app only)
- `windows-sys`: Win32 FFI, `SystemParametersInfoW`, `EnumDisplayMonitors`, `GetMonitorInfoW`, `GetProcessMemoryInfo` in core; `AttachConsole` in the app
- `mach2`: Mach FFI on macOS, for `task_info(TASK_VM_INFO)` in `memory.rs` and nothing else. Declarations only; the `unsafe` call site is ours
- `signal-hook`: Linux only, and only for `session_end`. A signal handler may call almost nothing and quitting a Slint event loop is not on the list, so the delivery has to reach an ordinary thread first; this crate does that with a self-pipe, which is why it is a dependency rather than a scoped `unsafe` around `libc::signal`

## Platform support

Windows is the platform that ships. Linux builds, tests, renders headlessly, and since phase 5 sets a wallpaper and exits cleanly when its session ends. macOS builds, tests, and renders headlessly; what it does not do yet is set a wallpaper.

| | Windows | Linux | macOS |
|---|---|---|---|
| Build, unit, engine, GPU shader, soak | yes | yes (lavapipe) | yes (Metal) |
| Golden images | yes (`warp`) | yes (`lavapipe`) | yes (`metal`) |
| `render` subcommand | yes | yes | yes |
| Settings window | yes | yes, in the test guest | untested |
| Set the desktop wallpaper | yes | yes, per desktop | no |
| Native display query | yes (Win32) | yes (`xrandr`) | no |
| Clean exit when the session ends | yes (`WM_ENDSESSION`) | yes (SIGTERM) | no |
| Desktop e2e (`tests/e2e.rs`) | yes, on the desktop (10 of 11 cases) or in a local VM (all 11) | yes, in a local VM (all 10 under KDE and XFCE, 8 under GNOME and Cinnamon) | compiles, unrun |

Per-OS implementations live in four places, each behind a `cfg` and each documented where it sits:

- `memory::snapshot`: `GetProcessMemoryInfo`, `/proc/self/{status,smaps_rollup}`, `task_info(TASK_VM_INFO)`. Same `MemorySnapshot`, same CSV, so every memory assertion in the suite is live on all three.
- `engine::wallpaper_sink::SystemWallpaper`: Win32 on Windows, a per-desktop command on Linux (see Setting a wallpaper on Linux below), and on macOS `check_supported` returns "not supported on this platform yet" before anything is rendered with `publish` returning the same string if it is reached anyway. Not a stub that pretends to succeed, and not a refusal that arrives after a full-resolution render and readback.
- `config::is_position_on_screen`: Win32 monitor enumeration on Windows and the `xrandr` outputs on Linux, which give the same shape of answer. Where there is no display to ask, and on any platform with no query, a coordinate-range sanity check against X11's INT16 window-position range, which is the coarse portable half of the same question. A run with no display still loads a config, so refusing every saved position there would move a window on the next run that has one.
- `session_end::install`: the Win32 listener on Windows and a SIGTERM listener on Linux, both on their own thread, both quitting the event loop once however many times they are told. macOS returns `None` and says so.

### Setting a wallpaper on Linux

Setting a wallpaper is a desktop-shell operation rather than a display-server one, so there is no call to make and `desktop.rs` is a table instead: `XDG_CURRENT_DESKTOP` selects a row, and the row says what to run. gsettings for the GNOME schema (both `picture-uri` and `picture-uri-dark`, because GNOME picks between them by colour scheme), for Cinnamon's own schema, and for MATE's, whose key takes a path rather than a URI; `plasma-apply-wallpaperimage` for KDE; `xfconf-query` for XFCE; `pcmanfm-qt --set-wallpaper` for LXQt. Budgie has its own row using the GNOME mechanism, so a refusal can name what it found.

The list is walked in the order the session wrote it, not in the table's order, which is what makes `Budgie:GNOME` Budgie and `ubuntu:GNOME` GNOME. Every row works identically in that desktop's X11 and Wayland sessions, because neither the shell nor the setting knows which is running. XFCE is the one row that asks a question first: which backdrop properties exist depends on the monitors and workspaces the session has, and their names are inside the property paths, so `xfconf-query -l` runs and every property ending in `last-image` is set. A session with none is a refusal and not a success, because nothing must report a wallpaper it did not set.

Not the XDG desktop portal, which is what an application would normally reach for: it targets sandboxed applications and puts a confirmation dialog in front of every set, and this wallpaper refreshes on a schedule. Detection and command construction are pure functions tested on every platform against fabricated sessions; only the sink runs anything. `check_supported` does both halves before a frame is rendered: which desktop this is, and whether that desktop's program is on `PATH`. Four of the seven rows have been run, one boot per desktop in the Linux test guest, with the desktop screenshotted afterwards each time; Cinnamon's globe was visible only after its crashed shell was restarted by hand, since its session falls into a fallback dialog. `docs/roadmap.md` names the three rows that have not run and carries that defect.

The screenshot is not ceremony. XFCE's setter exited zero, `test_set_wallpaper` passed, and the desktop went on showing xfdesktop's own default: the property xfdesktop reads is named after the connected monitor and does not exist until something creates it, and the ones the channel file ships under `monitor0` are two major versions old. So the XFCE row takes the connected outputs' names and creates that property, and its fill mode, where the session has neither. The general rule the episode leaves behind is that a setter's exit code is not evidence that a wallpaper changed, which is why on Linux `test_set_wallpaper` asks the session what its wallpaper is once the setter says it set one. It derives the read-back from the same table the write came from, and a desktop that names its settings after its own monitors has to hold the image in one of those: reading back only what the write chose is what would have called the XFCE bug a pass. Plasma's tool and `pcmanfm-qt` have nothing to ask, so those two sessions keep the exit code and say so.

`display.rs` is the other half of the same work. It parses `xrandr --query` into the connected outputs that have a mode assigned, which answers both questions Win32 answers: `target_size` takes the primary output's mode, and `is_position_on_screen` tests the title bar against every output. The parser is separate from the process so it is tested everywhere, not only where xrandr exists. Under a Wayland session the answer comes through XWayland, where display scaling can skew it; the native per-desktop D-Bus query is a roadmap item.

Session end on Linux is SIGTERM and nothing else, through `signal-hook` rather than a scoped `unsafe`: a signal handler may call almost nothing and quitting a Slint event loop is not on that list, so the delivery reaches an ordinary thread through a self-pipe first. SIGHUP is deliberately not treated as the session ending, because the e2e suite starts the app from a process whose controlling terminal is not its own. What this does not do is participate in the question: a desktop asks whether an application is *ready* to close over logind's inhibitor protocol, and SIGTERM arrives after that decision. So this uses the grace period, and the inhibitor half is on the roadmap.

The desktop e2e suite is `#[ignore]`d, not `cfg`-gated: it compiles on all three OSes (which is free coverage for the IPC and process plumbing) and never runs in CI, because hosted runners have no interactive desktop. It runs on the developer's desktop with `cargo e2e`, and in a local VM with `cargo xtask e2e --target <windows|linux>`; see `docs/vm-setup.md`.

Three cases inside it are gated at runtime rather than by `cfg`, following the same convention as `software_adapter_produces_correct_results`. The tray-start-hidden lifecycle and single-instance enforcement need a tray icon, and single-instance is additionally tray-mode-only in the product. Whether there is a tray is a property of the session rather than of the platform: Windows always has one, and on Linux Slint registers its icon over D-Bus through `ksni`, so `tray_supported()` asks `gdbus` who owns `org.kde.StatusNotifierWatcher`. Plasma and xfce4-panel answer, GNOME and Cinnamon do not, so those two cases run under the first two desktops and skip under the other two. `test_set_wallpaper` sets a real desktop wallpaper, so it is opt-in through `SUNLIT_EARTH_E2E_WALLPAPER`, which both generated guest jobs set and nothing else does. All three print why they skipped. Two further cases pick their startup mode by the tray capability, running windowed where there is no tray, which tests the same thing minus the icon. macOS has no VM story: it stays on hosted runners.

What `test_set_wallpaper` can assert about the capability differs by platform, which is why it is three checks rather than one equality. Windows has one setter and every session has it, so a refusal there is a regression. A platform with no setter at all must not claim one. Linux has a setter per desktop, so the answer belongs to the session rather than to the build, and the guarantee is narrower and sharper: a run that opted in to replacing the wallpaper is a run in a guest whose desktop was chosen for having a setter, so on Linux an opted-in run that reports no setter fails rather than skips. Without that clause a backend that went missing would report itself unsupported and the case would quietly pass.

`test_memory_report` is deliberately not gated at all: the report degrades section by section on its own, so a backend with no allocator report produces a shorter line rather than a missing section, and the case asserts the four section names and prints the whole report. Printing is half the point, since the suite runs with `--nocapture` on both paths and that is where a real report from a real GPU or from a guest is captured.

`test_session_end_shuts_down_promptly` is the one exception to "`#[ignore]`d, not `cfg`-gated": it delivers `WM_QUERYENDSESSION` and `WM_ENDSESSION` to the running binary, which are Win32 calls, so the body does not compile elsewhere. It asserts the app exits successfully and in under five seconds, which is the number Windows gives an application before it names it on the shutdown screen.

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
- **A test that needs the real 8K assets skips with a printed reason without them**, rather than failing or passing vacuously: `textures/**` is Git LFS, and a checkout without the objects holds pointer files that exist as far as anything that only asks about existence is concerned, so the check is on size. `lowering_the_resolution_lowers_the_process_footprint` in `tests/engine.rs` is the one such case, and it costs about 25 seconds where the assets are present. Everything else that needs a texture, including every Moon case, the golden suite's Moon and every panorama case but the two that are about the real asset, uses a generated fixture instead.

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

- **`ci.yml`**: `workflow_dispatch` only. It ran on every push and PR until the hosted minutes ran out; this repository is private, so they are billed, and the local VM suite covers what the runners were for. What a manual run still buys is the other two operating systems, so dispatch it before anything that has to hold on all three. A `fmt` job runs `cargo fmt --check` once on Ubuntu, and a `test` matrix runs `cargo test --locked -- --show-output` plus a headless `render` smoke test on `ubuntu-latest`, `windows-latest`, and `macos-latest`, uploading the contact sheet and the smoke render per OS.
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

- `rust-toolchain.toml` pins the channel (`1.94.0`, profile minimal, rustfmt and clippy).
  rustup honors it in this checkout whatever the host's default is, which is intended:
  a release build has to be able to say which compiler made it, and the builder images
  install the channel `guest::toolchain::pinned` reads out of that file. A developer whose
  default is newer sees `rustc -V` differ inside and outside the checkout.
- `.cargo/config.toml` links the C runtime statically on `x86_64-pc-windows-msvc`
  (`-C target-feature=+crt-static`). Every Windows build of this tree agrees, the e2e
  binaries and a local `cargo build --release` included, and the `cc` crate follows it
  with `/MT` for the Astronomy Engine's C. A clean Windows 10 then needs no Visual C++
  redistributable, which is what `dist --target windows` proves on the artifact.
- `unsafe_code = "deny"` in `[workspace.lints.rust]`. It is `deny` and not `forbid` because Slint macros need unsafe internally. `scene/sun.rs`, `scene/sky.rs`, `wallpaper.rs`, `config.rs`, `memory.rs`, and `main.rs` have scoped `#[allow(unsafe_code)]` on individual FFI call sites with `// SAFETY:` comments. New FFI, on any platform, follows that pattern; the macOS `task_info` call in `memory.rs` is the most recent example.
- Slint is pinned to `~1.17` with no wgpu feature. The app does not share a device with Slint, so the wgpu version is independent of the Slint version.
- Render texture size is quantized to 64px boundaries to reduce GPU texture churn during resize, and then capped by the quality tier.
- Zoom is normalized (0.0 to 1.0) with exponential mapping: `distance = 1.5 * (80.0 / 1.5)^t`. Use `zoom_to_distance` / `distance_to_zoom` in `scene/camera.rs`.
- The grid texture uses 16x anisotropic filtering with trilinear mipmaps.
- WGSL `vec3<f32>` has 16-byte alignment, so `#[repr(C)]` structs need an explicit `_pad: f32` after every `[f32; 3]` field. `uniforms.rs` has a compile-time size assertion.
- LF line endings globally.
