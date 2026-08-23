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

The desktop e2e suite runs in local VMs instead of taking over the developer's desktop. `docs/vm-setup.md` is the human guide; the design is in `docs/plans/2026-08-19-phase3-vm-orchestration-plan.md`, and the Linux guest's overhaul in `docs/plans/2026-08-21-phase5-linux-vm-and-parity-plan.md`.

```bash
cargo xtask vm doctor              # Unelevated, read-only: can this host run the VM suite?
cargo xtask vm setup               # The one command that changes the host. Elevated on Windows.
cargo xtask vm build-image <windows|linux>   # An install, then a manifest. Tens of minutes.
cargo xtask vm up <target> [--desktop <kde|gnome|xfce|cinnamon>]   # An interactive guest, with the current binaries in it
cargo xtask vm ssh <target>        # A shell in the running guest
cargo xtask vm view <target>       # Its desktop (vmconnect for Hyper-V, VNC for QEMU)
cargo xtask vm smoke <target>      # Boot, run a trivial job through the guest contract, take it down
cargo xtask vm status              # Images, media, overlays, running VMs, disk footprint
cargo xtask vm down <windows|linux|all>      # End the guest, keep the golden image
cargo xtask vm purge <windows|linux|all> [--vm] [--image] [--iso] [-f]
cargo xtask e2e --target <host|windows|linux> [--keep] [--allow-expired-image] [--desktop <d>]
```

The host tools the VM commands run go through `host::facts::resolve_tool`, which asks `PATH` and then the places an installer is known to leave a program without putting it on `PATH`: QEMU's and TightVNC's own directories under Program Files, winget's links directory, and scoop's shims directory under `%SCOOP%` or `~\scoop` and `%SCOOP_GLOBAL%` or `%ProgramData%\scoop`. Both package managers append to the *user* `PATH`, so a tool installed in the shell that is now running the xtask is installed and invisible; a lookup that missed it would make `vm setup` plan an install that winget then refuses as redundant, and `vm view` claim a viewer is absent. The VNC viewers are in `facts::VNC_VIEWERS`, executable names rather than package identifiers, and `vm view` of a QEMU guest resolves them the same way `vm doctor` reports them. Two lookups stay on bare `PATH` deliberately and say so where they sit: the Packer ISO tools, because Packer resolves them itself and a fallback location would not help it, and `store::windows_media`'s choice between `curl` and `wget`.

`vm setup` never reboots or signs anyone out; it reports what needs one. `vm doctor` changes nothing. `vm down` is the cheap teardown: it ends the guest and deletes its run state, which the next boot recreates. `vm purge` is the disk-space one: everything a target has on disk unless `--vm`, `--image`, or `--iso` narrows it, and it asks before deleting unless `-f` is given. `e2e --target host` is what `cargo e2e` does, kept as one command so the manual real-GPU run and the VM runs are the same thing.
**The builder matrix mirrors the runtime provider matrix**, and `commands::build_image::builder_for` is the one place that decides it:

| | Windows host | Linux host |
|---|---|---|
| Windows image | native Hyper-V (`commands::build_hyperv`) | Packer and QEMU on KVM |
| Linux image | Packer and QEMU on WHPX | Packer and QEMU on KVM |

The one cell that is not Packer is the one QEMU cannot install. QEMU on a Windows host uses WHPX, and a WHPX guest with more than one vCPU does not survive the reboot Windows Setup performs after copying its files: `WHPX: Unexpected VP exit code 4`, unrecoverable. One vCPU survives it and Windows 11 Setup refuses to install on one core; `kernel-irqchip=off` keeps the vCPUs and stops the guest booting at all. So on a Windows host the xtask installs Windows itself, on the hypervisor the guest runs on anyway: it repacks the media without its boot prompt (`store::windows_media::ensure_install_media`, so there is no keypress to inject, keeping a repack only until the build that needed it succeeded, and reusing one a failed build left behind while the record beside it still matches the download it came from), builds the unattend CD with oscdimg from `build_hyperv::CD_FILES`, creates a generation 2 VM, watches the install, runs `finalize.ps1` over SSH, shuts the guest down, keeps its disk and drops the VM, and converts the disk to qcow2 for the QEMU override cell. No Packer, no QEMU process, and no network on the host after the ISO download; the guest's own first logon fetches the Visual C++ runtime, which Windows does not ship and every Rust MSVC binary the suite runs links dynamically, and `finalize.ps1` fails the build if it is not there. Decision 10's intent stands either way: one canonical install in two formats, with only the conversion direction flipping. `docs/vm-setup.md` has the WHPX measurements and a twenty-second recipe for rechecking them against a newer QEMU. The dispatch is deliberately not overridable by `SUNLIT_EARTH_VM_PROVIDER`, which moves a guest rather than a build.

Both builders report on the install while it runs, because neither Packer nor Windows Setup says anything for most of an hour (`commands::build_watch`, whose `Trend` both share). The Packer path holds one QMP connection, presses the installer's boot key with it, and prints a line a minute: what the output disk holds and how fast it is growing, whether the guest's screen is changing, and what QEMU says the machine is doing. A guest QEMU has stopped is reported rather than waited out: the build starts it again once, and if that changes nothing it ends the guest so the build fails now instead of at Packer's two-hour SSH timeout. The build directory keeps `screen.png`, the guest's last screen as QEMU encoded it, and `packer.log`, which is where QEMU's stderr ends up. The native path prints the same shape of line from the signals Hyper-V has: `Get-VM`'s state, the growing VHDX, and the firmware's first boot entry, which is what makes the one unmeasured assumption in it visible (that Setup's own boot entry survives the mid-install reboots). A guest that is not `Running` on two readings in a row ends the build, since a Hyper-V guest stays running through the reboots an install performs; one reading is not enough, because `Start-VM` returns before the guest is `Running` and `Get-VM` says `Starting` in between.

The build VM carries the same `sunlit-e2e-windows` name a runtime guest does and writes a `RunState` with a build reason before it exists, so `vm status`, `vm view`, `vm ssh`, `vm down`, and the one-VM-at-a-time rule all apply to a build. Every failure from the point the create script could have run asks Hyper-V whether the VM is there and says which of the three answers it got, because that script is one process that stops at its first error and most of its statements leave a registered VM behind. A failed build keeps its guest and prints how to reach and remove it; `vm down windows` takes the VM and the unfinished disk together, a `--iso` purge ends a build first because a build holds both DVDs for the whole install, and a build that is still running is refused as something to clear away. Anything that ends a build says so before it does: `StartReason::cost_of_ending` is the one clause, read by the one-VM-at-a-time refusal and by a teardown's listing and question alike, and a purge that ends a build without taking its run state names the record and the disk it leaves.

`e2e --target <guest>` copies three things in: the app, the test harness, and the fixtures, plus the repository's `textures/` when it holds the assets rather than Git LFS pointers (checked by size, since a pointer file exists and cannot be decoded). `test_render_and_exit` samples the globe by color, so without them a guest renders the procedural grid; the job then omits `SUNLIT_EARTH_TEXTURES` rather than naming a directory that is not there, which is the same thing that happens to `cargo e2e` on a host in that state. The generated Windows job also sets `SLINT_BACKEND=winit-software` (`commands::e2e::WINDOWS_SLINT_BACKEND`): a Hyper-V guest's synthetic display adapter offers no OpenGL and Windows ships no software implementation of it, so Slint's default renderer cannot start at all and the app dies with "Could not locate glCreateShader symbol" before its event loop. WARP does not cover that, because WARP is Direct3D and Slint asks for GL. The Linux job sets no backend, because Mesa is a software GL implementation and llvmpipe answers there. Both details live in the generated job, which is per run: neither needs an image rebuild.

A guest is also something a person looks at, so staging writes two more things into a Windows one (`guest::handover`): `C:\sunlit-e2e\run-app.cmd`, which starts the app with the same `SLINT_BACKEND` the job sets and with `SUNLIT_EARTH_TEXTURES` under the same condition, and two shortcuts on the console user's desktop, one for that launcher and one for the guest root. Per boot for the same reason the job script is per run, and generated rather than shipped in the image because the launcher has to know what this boot staged. A Windows guest also offers one of two consoles, and `handover::enable_enhanced_session` is what decides which. The image ships with Remote Desktop Services disabled, so a guest running a suite offers no enhanced session (`EnhancedSessionModeState` 6 rather than 2) and `vmconnect` opens a basic session that asks for nothing: an enhanced session is RDP, and connecting moves the console session into it, which is where the windowed tests keep their desktop. A guest being handed to a person has nothing of ours running in it, so `vm up` and `e2e --keep` turn the service back on, blank the account's password and clear `LimitBlankPasswordUse`, which is what makes the credential dialog a thing to dismiss rather than fill in. That buys the one thing a basic session cannot do at all: a window that resizes, with the guest's desktop following it. What happened is recorded rather than inferred: `vm::hand_over` writes `RunState::handed_over` from the marker the guest itself printed, and that field, not the start reason, is what `vm view` reads to decide whether to answer the connection dialog and which of the two consoles to describe. The reason is chosen before the boot, so it can answer neither question; `Keep` is written at the same moment, which is what lets `vm status` tell a run in progress from one that is over. Each command's closing text comes from the same facts (`vm::Prepared`), so `vm smoke --keep`, which stages nothing and hands nothing over, is described as the empty desktop it is. A guest whose image predates the Remote Desktop Services disable still offers an enhanced session, and staleness only warns at boot, so the note for a guest nobody handed over says to cancel a credential dialog that appears anyway rather than sign in. The display-configuration dialog in front of an enhanced session is answered by `vm view`, which writes `vmconnect`'s own per-VM settings file (`hyperv::vmconnect_settings`) before starting it, sweeps the ones earlier guests left, and takes the file with the VM when one is destroyed: those settings are filed under the VM's identifier, which is new on every boot, so the dialog's own remember-me checkbox lasts exactly one guest. Runtime is enough for all three, so none of it needs an image rebuild; the disable is an image property only because the service refuses to stop once started, and `finalize.ps1` fails a build whose start type is anything else.

The Linux guest is Debian 13 with four desktops installed side by side, and nothing in the image decides which one a boot logs into. `provider::desktop` is the host half: `Desktop::session` maps the `--desktop` flag onto the `.desktop` basename sddm's `[Autologin] Session=` wants (`plasmax11`, `gnome-xorg`, `xfce`, `cinnamon`, none of which are the names one would guess), and `fw_cfg_args` puts it on QEMU's command line as `-fw_cfg name=opt/sunlit/desktop,string=<session>`. The `opt/` prefix is required; QEMU refuses anything outside it. In the guest, a oneshot unit ordered before `display-manager.service` reads `/sys/firmware/qemu_fw_cfg/by_name/opt/sunlit/desktop/raw`, checks the value against its own allowlist of the same four names, and writes sddm's autologin drop-in. `the_guest_accepts_exactly_the_sessions_the_host_can_ask_for` reads both lists and compares them, since nothing else connects the two: a name on one side and not the other is a boot that silently falls back to Plasma while `vm status` says otherwise. fw_cfg rather than SMBIOS OEM strings, which was the other candidate: the fw_cfg device is ACPI-enumerated so its module loads itself early and the value is a file in sysfs, while mainline exports no per-string sysfs interface for SMBIOS type 11 at all. The chosen desktop goes into `RunState`, so it survives the command that chose it and `vm status` can name it; `--desktop` against the Windows guest is refused rather than ignored, because a run whose flag did nothing is a run whose results are about a desktop nobody chose.

Debian 13 rather than a current Ubuntu, and that choice has a shelf life. It is the only current base where Plasma, GNOME and XFCE all have a first-class X11 session at once: GNOME 50 removed X11 upstream in March 2026 and Ubuntu 25.10 had already dropped the GNOME Xorg session, while trixie froze on GNOME 48 and Plasma 6.3. That insulates the image for trixie's support window (full to 2028-08, LTS to 2030-06) and no longer. The guest stays on X11 because the guest contract needs `DISPLAY` for a process the orchestrator starts over SSH, so a Wayland guest story is real work rather than polish and is on the roadmap as such.

Of Debian's two cloud images the base is `generic`, not the smaller `genericcloud`, and the difference is the only thing that makes a graphical guest possible: `genericcloud` ships `linux-image-cloud-amd64`, built without drivers for physical hardware, and DRM goes with them. No `CONFIG_DRM`, no `virtio_gpu` module, so no `/dev/dri` whatever the display device is, so logind's seat0 is not graphical and sddm waits for a display forever. The guest boots, answers SSH, autologs in as far as its configuration goes, and sits on the text console: a failure that says nothing about its cause, which is why `desktop.sh` checks `modinfo virtio_gpu` before it installs anything and names the image to use.

The disk is attached with `discard=unmap` and `finalize.sh` ends in `fstrim -av`. That is the whole of the size management, because the template also sets `skip_compaction`: Packer's own compaction converts the finished disk and renames the copy over the original, and on this host the rename is refused as long as something still holds the file. Trimming from inside the guest reaches the same end state without a second copy. Zeroing the free space, which is what a build does when compaction *will* run, is the one thing not to do here: it allocates every cluster it writes and there is no convert pass left to drop them.

Two more things about the Linux command line, both in `qemu::Launch::args`. The guest gets a `virtio-tablet-pci`, an absolute pointer, which is what makes a click in a VNC viewer land where the cursor is: VNC's `PointerEvent` carries absolute coordinates, QEMU's implicit PS/2 mouse is relative, and the translation between the two is the textbook cause of offset clicks. And the display is `-device virtio-vga` carrying `xres`/`yres` rather than `-vga virtio`, because only the device form takes properties, and those properties are what set virtio-gpu's preferred mode. `SUNLIT_EARTH_VM_RESOLUTION` therefore applies to both providers now, through the shared `provider::console`; on QEMU it overrides `qemu::DEFAULT_CONSOLE` rather than a size fitted to the host's screen, since a VNC viewer scales and has nothing to fit.

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
      src/engine/    # the engine thread, injectable clock, wallpaper sink
      src/geometry/  # sphere mesh, procedural grid texture
      src/renderer/  # wgpu pipeline, offscreen render, readback
      src/scene/     # camera, sun (astronomy FFI), datetime
      src/config.rs  # AppConfig, QualityTier, persistence
      src/memory.rs  # per-OS process counters, the metrics CSV, the budget
      src/memory_report.rs  # the four-section report of where the memory is
      src/params.rs  # SceneParams, ParamsDigest, gamma slider mapping
      tests/         # engine, soak, golden, shading, render_pipeline
    sunlit-app/      # Slint shell: window, tray, IPC, config bridge
      ui/main.slint  # MainWindow and TrayIcon
      tests/         # e2e (desktop-gated), slint_ui
    xtask/           # developer tooling: VM orchestration for the desktop e2e suite
  textures/          # local 8K JXL assets, not part of the build
  vm/                # Packer templates and guest assets for the test VMs
    linux/           # Debian 13, four desktops on Xorg, cloud-init seed
    windows/         # Windows 11 Enterprise eval, autounattend, bootstrap
```

The package inside `crates/sunlit-app` is still named `sunlit-earth`, so the binary, `CARGO_BIN_EXE_sunlit-earth`, and `target/release/sunlit-earth.exe` in the release workflow are unchanged by the directory name.

## Architecture

The organizing principle is **headless first**. The engine runs to completion with no window at all; the settings window is one optional client. Hiding the window removes a client, it does not half-suspend the machinery. This is what removed the tray-mode memory leak class, made soak tests possible, and retired the teardown hacks.

### The engine (`sunlit_core::engine`)

One thread owns the wgpu device, the `Renderer`, the texture mailbox, and the schedule. Clients send `EngineCommand`s and receive `EngineEvent`s.

- **Commands**: `UpdateParams`, `SetPreviewSize`, `SetPreviewEnabled`, `RenderWallpaperNow`, `RenderToFile`, `ExportPixels`, `SetTextureResolution`, `ReportMemory`, `SetAutoRefresh`, `Poke`, `Shutdown`.
- **Events**: `PreviewFrame { rgba, width, height }`, `TexturesReady`, `WallpaperSet(Result)`, `Status(String)`.
- **The loop never sleeps on wall time to decide what is due.** It blocks on the command channel with a 50 ms timeout and, on each wake, asks `clock.elapsed()` what is due: the texture drain (5 s), the sun-position refresh (120 s), the cloud poll, the memory metrics sample (600 s), and the auto-refresh export. `Schedule::due` recomputes its deadline from `now` rather than accumulating, so a long stall produces one run and not a burst of catch-up runs.
- **Injected `Clock`.** `SystemClock` in production, `MockClock` in tests. `MockClock` advances UTC too, so simulated days really do rotate the Earth. This is what makes 14 simulated days run in 13 seconds.
- **Injected `CloudSource`.** `HttpCloudSource` in production, fixtures in tests. A dedicated cloud worker thread does network I/O and JPEG decoding and never touches the GPU; it parks frames in the mailbox and pokes the engine, which uploads on its own schedule. A poll skipped because the worker is busy retries on the next tick. Which variant it fetches follows the texture resolution rather than the quality tier; see the Texture resolution section.
- **Injected `WallpaperSink`.** `SystemWallpaper` writes a PNG and calls the Win32 API; `CountingSink` lets the soak test run for simulated weeks without touching the desktop.
- **The preview follows window visibility.** The app sends `SetPreviewEnabled(false)` on every hide and `(true)` on every show, so a hidden window costs no readback. The engine keeps rendering regardless (the wallpaper export depends on it); only delivery stops. Re-showing pays back an "owed" frame from the existing texture, since the dirty check would otherwise suppress a re-render and leave the window blank.
- **Preview frames are pixel buffers**, not shared GPU textures. The engine reads its offscreen target back and hands over RGBA bytes; the app wraps them in `slint::Image::from_rgba8`. Slint therefore needs no wgpu feature and shares no device, which is why teardown is ordinary drop order.

### Parameters (`sunlit_core::params`)

`SceneParams` is the single description of what to draw: camera, texture selection, sample count, lighting, clouds, atmosphere, color correction, and the datetime input. There are exactly two translation points:

1. `ui_callbacks::read_params_from_window` / `apply_params_to_window` in the app.
2. `renderer::render_pass::write_uniforms` in core.

Adding a shader parameter means: the `.slint` property and slider, the `AppConfig` field, `SceneParams` + its `ParamsDigest`, `Uniforms`, and the WGSL. The bridge functions and the dirty check follow from the struct. `params.rs` has a table-driven test that walks every parameter and asserts it changes the digest, so forgetting the dirty check is a test failure rather than a stale-frame bug.

A setting that is not a shader parameter takes a different route, and `texture_resolution` is the example: it is an `AppConfig` field with a widget, but it stays out of `SceneParams` and the digest because it does not describe what to draw, and acting on it means re-reading files and swapping GPU textures, which `push_params` cannot express. Such a setting gets its own `EngineCommand` and its own callback, and is read back in `read_config_from_window_onto` rather than in `write_to_config`.

`ParamsDigest` is the quantized snapshot used for dirty checking: camera floats compare exactly, everything else is rounded to integer thousandths. `datetime` is deliberately not in the digest; what the shader consumes is the sun direction derived from it, and `FrameState` compares that separately along with the render size.

### The renderer (`sunlit_core::renderer`)

`Renderer` owns every GPU object and renders offscreen into its own texture (`RENDER_ATTACHMENT | TEXTURE_BINDING | COPY_SRC`). It knows nothing about windows. Key methods: `render(&SceneParams, sun_dir) -> RenderOutcome`, `resize`, `drain_texture_updates`, `export_image`, `read_preview_pixels`, `textures_ready`, `loading_text`.

Submodules: `gpu_setup` (construction, pipelines, render targets), `render_pass` (uniform encoding, pass encoding, `Overlays::select`, `read_texture_rgba8`), `textures` (slots, mailbox draining, mipmapped upload, `downsample_2x`), `texture_routing` (which bind group and blend mode), `frame` (`FrameState` dirty check), `uniforms` (the 192-byte `#[repr(C)]` struct).

### The app (`sunlit-app`)

- `main.rs`: CLI (clap), logging, config load, then one of two paths. `run_render` is fully headless: no window, no Slint backend, no event loop; it starts the engine with the preview disabled, waits for `TexturesReady`, calls `render_to_file`, and returns an `ExitCode`. `run_app` creates the window, starts the engine, wires the UI, and runs the event loop. CLI flags: `--mode <tray|window>`, `--tray-start <visible|hidden>`, `--ipc-socket <name>`, `--quality <low|medium|high>`, `--texture-resolution <8192|4096|2048>`, `--software-rendering`, `--textures-dir`, `--log-level`, plus the `render` subcommand.
- `engine_client.rs`: `EngineLink` (send commands, push window state as `SceneParams`) and `event_forwarder` (engine events to the window). Preview frames cross the thread boundary through a latest-value mailbox with a single pending wake-up: the newest frame replaces the parked one and only one `invoke_from_event_loop` closure is ever in flight.
- `ui_callbacks.rs`: callback registration grouped into mouse, change, and action callbacks; every one of them ends in `link.push_params(&window)`. Also the config bridge (`apply_config_to_window`, `read_config_from_window`) and `defer_combobox_indices`.
- `ipc.rs`: opt-in control channel over `interprocess` local sockets. Commands: `quit`, `show-window`, `hide-window`, `export-test`, `query-memory`, `memory-report`, `set-wallpaper`. Fire-and-forget, with `SIGNAL:` lines on stdout as the reply channel. `export-test`, `query-memory` and `memory-report` are answered on the listener thread, so they work while the event loop is idle. `query-memory`'s single `SIGNAL:memory rss_bytes=... peak_rss_bytes=... private_bytes=...` line is a parsing contract the e2e suite depends on and must stay byte-identical; `memory-report` is a separate command for that reason, and brackets its many lines with `SIGNAL:memory_report_begin` and `SIGNAL:memory_report_end` rather than promising a line format.
- `tray.rs`: the procedurally generated 32x32 icon, the tray callback wiring, and single-instance enforcement. The tray icon itself is a `SystemTrayIcon` component in `ui/main.slint`, so Slint owns the platform integration.
- `session_end.rs`: Windows only. An invisible top-level window on its own thread that answers `WM_QUERYENDSESSION` and quits the event loop on `WM_ENDSESSION`, so a reboot does not have to wait for Windows to kill the process. winit handles neither message, so without this nothing in the app ever learned the session was ending. The decision is a pure function (`classify`), unit-tested everywhere; the Win32 window is tested by sending it both messages.
- `mouse_math.rs`: pure functions for mouse interaction (globe drag with tilt correction, frame drag, orient drag, tilt drag, zoom scroll). No Slint dependency; unit-tested with `proptest` invariants.

### UI (`ui/main.slint`)

`MainWindow`: resizable split layout, controls panel in a `ScrollView`. Top-level controls are "Set as Wallpaper", "Load Defaults" and "Reset", and a 3x3 grid of camera presets. Below is a collapsible "Advanced" section with `GroupBox`es for Camera Position, Camera Orientation, Framing, Date / Time, Clouds, Atmosphere, Lighting, Color Correction, and Rendering. Camera properties are `in-out` with `<=>` slider bindings. A `TouchArea` over the image handles drag and scroll.

`TrayIcon` inherits `SystemTrayIcon`: menu (Open, Refresh Now, checkable Auto-refresh, Exit) and `clicked()` to toggle the window. Only properties *declared* on the derived component are exposed to Rust, so the inherited `icon` is bound to a declared `tray-image` property. A `SystemTrayIcon`-rooted component implements `StrongHandle` but not `ComponentHandle`, so there is no `as_weak()`; the handle is kept in an `Rc`.

### Quality tiers

`QualityTier` (low, medium, high) is persisted in the config and overridable per run with `--quality` (the override is not written back). It has no widget in the settings window, which is why `read_config_from_window` is a read-modify-write against the stored config rather than a fresh `AppConfig::default()`: any persisted setting the UI does not manage has to survive a save untouched. It caps the MSAA sample count (1, 4, unlimited) and the preview width (1280, 1920, unlimited, aspect preserved). Default: low in debug builds, high in release; `EngineConfig::headless` pins low so tests do not depend on the build profile.

The tier does not select the cloud image variant; the texture resolution does. The tier says how much work a frame is allowed to be, and the cloud overlay is texture memory, which is what the other setting is for.

### Texture resolution

The two local surface textures are 8192 wide. `AppConfig::texture_resolution` decides what width they are loaded at, from the three in `config::TEXTURE_RESOLUTIONS` (8192, 4096, 2048), and the Rendering group offers them as a combo box. It also selects the cloud image variant, which is the third thing that scales with it; the three offered widths map one to one onto the three variants the upstream service publishes. The default is 4096, so an install whose config predates the setting moves to 4096 and anyone who wants the full width picks it once. `--texture-resolution <8192|4096|2048>` overrides it for one run; clap validates the three values, and a config file holding anything else is repaired to the default by `AppConfig::sanitize` on load, which is where the check belongs since a config file is a text file.

The halving is the same box filter that builds the mip chain, so a 4096 texture is the 8192 texture's first mip level exactly. That is why the default costs so little: on the software adapter the 800x800 render the e2e case checks is byte-identical at 8192 and 4096, and at 2048 the sampled land and ocean pixels move by at most 1/255. On a real adapter it is not quite identical, because anisotropic sampling can ask for a level of detail finer than the narrower texture's level 0 near the limb: measured on this machine's GPU, 8 pixels of 640,000 differ by one channel step. Anything that renders the globe larger than a few hundred pixels across will show the difference properly; the settings window is where to change it back.

Below 8192 the width is reached by halving, and the result is cached on disk: `assets::texture_cache::load_at_resolution` looks for `texture_cache/<stem>.<width>.png` under the same directory the cloud cache uses (so `SUNLIT_EARTH_CACHE_DIR` covers both) and validates it against a sidecar TOML recording the source's size and modification time. On a miss it decodes the source, halves it with the same box filter the mip chain uses, and writes the PNG temp-then-rename, under a temporary name carrying the process id so two writers of one entry cannot truncate each other. The source is stamped before the decode and the stamp re-read after it, because a source replaced during the seconds an 8K decode takes would otherwise be recorded as where the old pixels came from, and that entry would validate forever. Measured on the real assets in a debug build: 3.2 to 3.7 s cold against 0.2 to 0.8 s warm per texture, with the render byte-identical either way. A cached file is a plain downscale in the source's own orientation, which is why the orientation fixes are a separate `texture_loader::orient` rather than part of the decode: reading a cached file back is the same `load` a source goes through. The width is a cap, so a source narrower than the chosen width is loaded as it is.

Changing the setting at runtime is `EngineCommand::SetTextureResolution`, not a params push: it decides which pixels to load rather than what to draw. The renderer clears the bind groups and views of the file-backed slots, calls `Texture::destroy` on the textures they held, and only then lets the reload allocate, in the same nil-before-recreate order as `gpu_setup::replace_render_textures`. `TextureSlot` keeps its `wgpu::Texture` so there is something to destroy, and `last_rendered_index` goes back to the grid, which is the one slot `render` may assume is loaded. `tests/engine.rs` measures the point of all this on the real assets: 1251.5 MiB of private bytes at 8192 against 610.9 MiB at 2048, and it skips with a printed reason where `textures/**` is still Git LFS pointers.

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
- `sphere.wgsl`: vertex transform, texture sampling, uniforms. Single-texture mode uses `terminator_width < 0` as a sentinel, and in that mode the shader ignores the sun entirely. `schlick_fresnel()` drives both specular modulation and the diffuse color shift on ocean pixels. `fs_cloud` applies the cloud floor and gamma. Three concentric atmosphere shells, each with its own vertex/fragment pair: `vs_rayleigh`/`fs_rayleigh` (radius ~1.015), `vs_nightglow_orange`/`fs_nightglow_orange` (~1.014), `vs_nightglow_green`/`fs_nightglow_green` (~1.015). Draw order: Earth, Clouds, Rayleigh, Nightglow Orange, Nightglow Green.

### Wallpaper export

The engine renders at the sink's native resolution using temporary GPU textures with `COPY_SRC`, reads them back through a staging buffer with 256-byte row alignment, encodes PNG, saves to `%LOCALAPPDATA%\SunlitEarth\wallpaper.png`, and applies it with `SystemParametersInfoW`. PNG rather than TIFF because Windows preserves PNG wallpapers losslessly; TIFF wallpapers are JPEG-transcoded at 85% quality and band visibly in smooth gradients.

### Memory reporting

Two things measure memory, and they answer different questions. `memory.rs` is the process-wide one: three counters per platform, a CSV the watchdog appends to, and a soft budget that emits a `warn!` when private bytes cross it. `memory_report.rs` is the where-is-it one: `MemoryReport` in four short sections, assembled on the engine thread because that is where the device is, reachable from a test through `EngineHandle::memory_report`, from a running app through the `memory-report` IPC command, and once per launch as a `debug!` dump the first time the textures are ready.

The four sections are the process counters, wgpu's internal counters (`Device::get_internal_counters`), the backend allocator's live allocations (`Device::generate_allocator_report`), and the renderer's own table of what it believes it owns. The last two next to each other are the point: the day the columns disagree is the day there is a leak. Discipline keeps it readable rather than complete: the allocation section aggregates by GPU label, lists the ten largest groups of at least 1 MiB, and rolls everything else into one line; the expected table lists every texture over the same floor and totals all of them. Only the section names are a contract, and nothing parses the report, unlike `query-memory`'s single line.

Both wgpu queries degrade rather than vanish. `generate_allocator_report` is implemented for D3D12 and Vulkan and returns `None` elsewhere, so on Metal the section says so and names the adapter; the counters need the `counters` cargo feature, which is on workspace-wide, and a backend that does not maintain one reports zero, which D3D12 does for the allocation count. The adapter slug is printed because it is what says whether the GPU bytes overlap the process's private bytes: on WARP and lavapipe they do, on a real GPU they mostly do not.

The budget is a function of the texture resolution rather than one constant (`memory::private_bytes_budget`): a cold start, plus headroom, plus the three textures that width keeps resident. The cold-start half does not shrink with the setting, because a cold downscale cache reads the full-width JXL source whatever width it was asked for. At 8192 it comes to the same 3 GiB the original measurement was taken against. Tests pin both directions: the budget clears a cold-cache first run at every resolution, and stays under twice one, so it is neither a warning nobody reads nor a warning nobody gets. Every resolution is held to the one peak that was actually measured (2.43 GiB, at 8192, in a release build) rather than to a smaller figure derived from the budget's own decomposition, which would move with it and assert nothing. That makes 2048 the binding case, since it gets the smallest resident allowance and has the same 8K decode to pay for.

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

The screenshot is not ceremony. XFCE's setter exited zero, `test_set_wallpaper` passed, and the desktop went on showing xfdesktop's own default: the property xfdesktop reads is named after the connected monitor and does not exist until something creates it, and the ones the channel file ships under `monitor0` are two major versions old. So the XFCE row takes the connected outputs' names and creates that property, and its fill mode, where the session has neither. The general rule the episode leaves behind is that a setter's exit code is not evidence that a wallpaper changed.

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
- **A test that needs the real 8K assets skips with a printed reason without them**, rather than failing or passing vacuously: `textures/**` is Git LFS, and a checkout without the objects holds pointer files that exist as far as anything that only asks about existence is concerned, so the check is on size. `lowering_the_resolution_lowers_the_process_footprint` in `tests/engine.rs` is the one such case, and it costs about 25 seconds where the assets are present.

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

- `unsafe_code = "deny"` in `[workspace.lints.rust]`. It is `deny` and not `forbid` because Slint macros need unsafe internally. `scene/sun.rs`, `wallpaper.rs`, `config.rs`, `memory.rs`, and `main.rs` have scoped `#[allow(unsafe_code)]` on individual FFI call sites with `// SAFETY:` comments. New FFI, on any platform, follows that pattern; the macOS `task_info` call in `memory.rs` is the most recent example.
- Slint is pinned to `~1.17` with no wgpu feature. The app does not share a device with Slint, so the wgpu version is independent of the Slint version.
- Render texture size is quantized to 64px boundaries to reduce GPU texture churn during resize, and then capped by the quality tier.
- Zoom is normalized (0.0 to 1.0) with exponential mapping: `distance = 1.5 * (80.0 / 1.5)^t`. Use `zoom_to_distance` / `distance_to_zoom` in `scene/camera.rs`.
- The grid texture uses 16x anisotropic filtering with trilinear mipmaps.
- WGSL `vec3<f32>` has 16-byte alignment, so `#[repr(C)]` structs need an explicit `_pad: f32` after every `[f32; 3]` field. `uniforms.rs` has a compile-time size assertion.
- LF line endings globally.
