# Prototype Retrospective and Next Iteration Plan

Date: 2026-08-15. Status: analysis complete, plan proposed, nothing implemented yet. Sections 8.3, 8.4, and 11 were revised the same day after the test-infrastructure design was settled in discussion and the open research questions were answered.

This document reviews the Sunlit Earth prototype (186 commits, 2026-03-08 to 2026-03-30, ~13,500 lines of Rust/WGSL/Slint), diagnoses the problems that stalled development, and proposes an architecture and process for the next iteration. The four stated goals for the next iteration are: an isolated, automated test setup including UI tests; cross-platform development with test VMs and an orchestration layer; systematic prevention and detection of memory leaks; and a performance-first build that treats high-resolution assets as an optional upgrade.

## 1. Summary

The prototype validated every major technology bet: Rust + wgpu + Slint render-to-texture works, the astronomy FFI works, the shader stack produces good images, and the wallpaper export pipeline works on Windows. What failed is not the rendering core but the orchestration layer around it: resource lifecycles are coupled to window visibility, heavy CPU work runs on the UI thread, the same 30 parameters are hand-plumbed through eight layers, and the code paths that matter most in production (hidden window, live cloud updates, multi-day uptime) are exactly the paths excluded from every automated test.

The 7.3 GB memory consumption has a specific, well-evidenced explanation: decoded 8K cloud textures (about 134 MB each) accumulate in an unbounded channel that is only drained when the window renders a frame, and rendering stops when the window is hidden to the tray. Section 4.1 has the full analysis and section 8.2 the verification procedure.

The recommendation is a structured refactor into a headless-first workspace, not a from-scratch rewrite. Roughly half the code (shaders, camera and sun math, texture processing, the test suites) is validated and worth keeping. The half that needs rewriting (lifecycle, channels, parameter plumbing, main.rs orchestration) gets rewritten inside the refactor anyway. Section 6 covers this decision, sections 7 to 10 the plan.

## 2. Where the prototype stands

Working features: 3D Earth with day/night blending, water shading (specular + Fresnel), three-shell atmosphere, live cloud overlay with disk caching and ETag polling, color correction, full camera control (mouse + sliders + presets), custom date/time, config persistence, wallpaper export at native resolution, auto-refresh scheduler, tray mode with single-instance enforcement, IPC control channel, render-to-PNG subcommand, software rendering fallback.

Test assets: 130+ unit tests including proptest properties, GPU integration tests that run real WGSL on the software adapter in CI, headless Slint UI tests via `i-slint-backend-testing`, and desktop-gated e2e tests driving the real binary over IPC. CI runs the full non-ignored suite on Windows.

Known defects at time of writing: the memory leak (7.3 GB RSS after 10 days in tray mode), slow and UI-blocking startup in debug builds, window unresponsive during texture loads, and the teardown hacks (`std::mem::forget` on timers, `std::process::exit(0)` to dodge a wgpu TLS destruction panic, `src/main.rs:517-530`).

## 3. What worked well

These should be carried into the next iteration deliberately, not rediscovered.

- **The stack choice.** wgpu 28 + Slint render-to-texture via `Image::try_from(Texture)` proved out, including MSAA, resize handling, and coexistence with a wallpaper export path at a different resolution. The software adapter (WARP) made GPU tests runnable in CI with no hardware.
- **Research-plan-implement discipline.** The 80+ documents in `../plans` repeatedly paid off. The tray-mode debugging in late March (minimal reproduction project, hypothesis verification docs) is the reason we know, with experimental confirmation, that `BeforeRendering` stops firing when the window is hidden (`../plans/2026-03-30-fresh-astro-state-plan.md`). That finding is the key to the leak diagnosis below.
- **Pure-function extraction.** `mouse_math.rs`, `scene/datetime.rs`, `scene/camera.rs`, `quantize_to_granularity`, `downsample_2x`: all fully unit-tested, some property-tested, none of them regressed during the UI churn.
- **Behavioral GPU tests.** Asserting invariants (monotonicity, bounds, visibility) instead of exact pixels made `tests/shading.rs` and `tests/render_pipeline.rs` portable across adapters. This convention scales to cross-platform CI unchanged.
- **The IPC + stdout signal protocol for e2e.** `--ipc-socket` with `SIGNAL:` lines on stdout is a clean, debuggable way to drive a GUI binary from a test harness. This is the seed of the future automated UI test setup and should be extended, not replaced.
- **The `render` subcommand.** Render-to-PNG-and-exit is the seed of the headless core.
- **Memory instrumentation exists.** `memory.rs` snapshots RSS via `GetProcessMemoryInfo`, and `tests/e2e.rs:536-568` already asserts RSS budgets (under 300 MB at startup checkpoints, under 1 GB overall) for the render subcommand. The instrument existed; it just was not pointed at the scenario that mattered (see 5.).

## 4. What did not work

### 4.1 The memory leak

Everything below is established from code reading and from the March experiments; the runtime confirmation step is in section 8.2. The evidence chain:

1. The cloud fetcher downloads an 8192x4096 JPEG (about 15 MB), decodes it to RGBA8 (8192 x 4096 x 4 = about 134 MB), and sends the decoded pixels as a `DecodedTextureMessage` into a channel (`src/cloud_fetcher.rs:266-277`).
2. That channel is `std::sync::mpsc::channel`, which is unbounded (`src/main.rs:222`).
3. The only consumer is `process_decoded_textures`, called exclusively from the `BeforeRendering` rendering callback (`src/renderer/mod.rs:432`, `src/renderer/textures.rs:29`).
4. `BeforeRendering` stops firing when the window is hidden to the tray. This was confirmed experimentally in March (`../plans/2026-03-30-fresh-astro-state-plan.md`, experiment `exp9_stale_render_state_after_hide`). The `request_redraw()` calls issued by the fetcher and the sun timer do not change this.
5. The upstream service publishes new images every 3 hours, 8 per day (verified against the live-cloud-maps documentation). The fetcher polls hourly and downloads on every ETag change, so a hidden instance accumulates up to 8 undrained 134 MB messages per day.

Arithmetic check: 10 days at 8 updates/day is a ceiling of 80 accumulated frames, about 10.7 GB. The observed 7.3 GB corresponds to roughly 54 frames, consistent with the machine sleeping part of each day. No other candidate found in the codebase produces growth of this magnitude at this rate. Secondary suspects worth watching during soak tests, but not capable of explaining 7.3 GB: per-export temporary GPU textures in `export_wallpaper_image` (transient, dropped after readback), wgpu allocator fragmentation across auto-refresh cycles, and Slint image churn from the 2-minute redraw timer.

Three compounding design mistakes, each a lesson for the next iteration:

- **Unbounded producer, conditional consumer.** The producer sends unconditionally forever; the consumer runs only under a UI condition. Safe Rust prevents use-after-free, but unbounded growth of reachable memory is exactly the kind of leak the borrow checker cannot see. Rule for the next iteration: every producer/consumer boundary uses either a bounded channel or latest-value (mailbox) semantics, chosen explicitly. For textures, only the latest frame per slot is ever useful, so a single-slot replacing mailbox is correct and caps the cost at one frame.
- **Payloads decoded eagerly at the wrong end.** The channel carries 134 MB of decoded pixels when it could carry the 15 MB JPEG (or nothing at all, since the image is already cached on disk) and decode at consumption time. Rule: keep data compressed until the last responsible moment; never park decoded pixel buffers in queues.
- **Consumption tied to visibility.** The renderer only observes the world when the window is visible, but the app's whole purpose is background operation. This is the same root cause as the stale-sun-direction bug fixed in the last commit before the hiatus. Patching parameter-by-parameter (as that commit did for sun direction) treats symptoms; the architecture in section 7 removes the cause.

Additional finding: the leak was invisible because release builds compile out all instrumentation. `tracing` is built with `release_max_level_warn` (Cargo.toml:23), so `log_memory_usage` at debug level produced zero telemetry from the process that leaked for 10 days. And every e2e test sets `SUNLIT_EARTH_NO_CLOUDS=1`, so the leaking code path was excluded from the entire automated test surface.

### 4.2 Slow startup and a frozen UI in debug builds

Three verified causes, all on the UI thread:

1. **Synchronous 8K JPEG decode before the event loop starts.** `spawn_cloud_fetcher` decodes the cached cloud image on the calling thread so clouds appear on the first frame (`src/cloud_fetcher.rs:169-201`). In a debug build that is a multi-second stall before the window exists.
2. **CPU mipmap generation on the UI thread.** `create_mipmapped_texture` runs inside `BeforeRendering` when a decode message arrives (`src/renderer/textures.rs:34-46`). For an 8K texture that is a full box-filter pyramid (`downsample_2x`, ~45 million output pixels across levels) executed in the unoptimized app crate. The dev profile sets `opt-level = 2` for dependencies (Cargo.toml:67-69), but `downsample_2x`, `grid_texture::generate`, and `shift_horizontal` live in the app crate at opt-level 0 with bounds checks and no vectorization. This is also the confirmed cause of the roadmap bug "main window is unresponsive while textures load": decoding is on a background thread, but mipmapping and upload are not.
3. **Grid texture generated during `RenderingSetup`.** 2048x1024 procedural generation plus its mip chain, also on the UI thread (`src/renderer/gpu_setup.rs:147-155`).

These are all fixable within the current architecture (move mip generation to the decode thread or the GPU, make the cached cloud load async, raise app-crate opt-level in dev), but the deeper fix is the asset pipeline in section 9: mipmaps do not need to be computed at runtime at all.

### 4.3 Automated testing stops at the desktop boundary

The test pyramid is solid up to the point where a real window, GPU present, tray icon, or wallpaper API is involved, and then everything is `#[ignore]` and runs only on the development desktop, by hand, interleaved with the production instance of the app. Consequences:

- The e2e suite is not in CI, so it rots silently and catches nothing automatically.
- Tests that mutate the desktop (wallpaper, registry style keys) are unsafe to run on the machine you work on, so they get skipped.
- The scenarios that matter for a background app (hidden window for hours, cloud updates arriving, sleep/resume, multi-day uptime) have no test at all. They are also awkward to compress into test time because the app reads the real clock and the real network: nothing injects time or data.
- Visual quality changes (shading, atmosphere, color) required a human looking at the window for every iteration, which is the "lots of user feedback needed" problem. There is no golden-image or contact-sheet mechanism even though the render subcommand can already produce deterministic PNGs with a fixed `--config` and custom datetime.

### 4.4 Code structure: parameter amplification and lifecycle coupling

The module layout is actually in decent shape (the March refactorings did their job), and the "messy" feeling has two specific sources:

- **Parameter plumbing amplification.** One new shader parameter currently touches about eight places: `ui/main.slint` (property + slider), `config.rs` (field, default, serde), `ui_callbacks.rs` (apply + read), `renderer/mod.rs` (UI read + `build_frame_state` argument), `renderer/frame.rs` (FrameState field + quantization), `render_pass.rs` (`ShadingParams` field + `write_uniforms`), `renderer/uniforms.rs` (layout + padding), and the WGSL. The 27-argument `build_frame_state` call and the 30-field `GpuResources` struct are symptoms. The fix is a single `SceneParams` struct that flows through all layers, with the Slint bridge and the uniform encoding as the only two translation points, plus grouped structs (macro-derived or code-generated bridge if needed).
- **Lifecycle coupling.** GPU state lives in a `thread_local` owned by a rendering callback whose invocation depends on window visibility; teardown order between Slint, winit, and wgpu is unresolved (hence `process::exit(0)` and leaked timers, `src/main.rs:517-530`); `run_event_loop` in main.rs mixes timers, tray, IPC, scheduler, and mode branching, with the auto-refresh timer body duplicated twice (`src/main.rs:296-314` and `341-359`). Every tray-mode bug in late March traces back to this coupling.

### 4.5 Cross-platform is aspirational

Everything OS-facing is `cfg(windows)` with no Linux/macOS counterpart: wallpaper setting, tray message pump, memory snapshots, single instance. CI builds and tests on Windows only. Nothing has ever compiled or run on Linux or macOS, so the "cross-platform" goal is currently untested against reality. Notably, Slint 1.17 (June 2026) shipped a built-in `SystemTrayIcon` element for macOS, Windows, and Linux with menu and activation callbacks, which could replace most of `tray.rs` (310 lines of Win32 message pump) if the upgrade to Slint 1.17 / wgpu 29 is taken. Whether it covers checkable menu items and left-click toggling needs evaluation.

## 5. Methodology gaps

What the process was missing, independent of any single bug:

1. **No production telemetry.** Release builds strip all instrumentation below warn level. A background app that runs for weeks needs an always-on, cheap signal: periodic RSS samples to a rotating log file, and a warn-level alert when RSS crosses a budget. The leak would have been visible in a log after day one instead of in Task Manager after day ten.
2. **No soak testing, and no way to do it.** Time (`OffsetDateTime::now_utc`, `Instant`, timer intervals) and the network (cloud URL) are hard-wired. Without injectable time and a fake cloud server, "10 days of tray mode" cannot be compressed into a 30-second test. This is the single highest-leverage testability investment.
3. **Test hermeticity achieved by amputation.** `SUNLIT_EARTH_NO_CLOUDS=1` made tests hermetic by deleting the riskiest subsystem from test coverage. The hermetic version of the cloud fetcher should be a local HTTP stub serving fixture JPEGs with controllable ETags, so the full pipeline stays under test.
4. **Resource-flow review was never a checklist item.** Channels, caches, and retained buffers each need an explicit answer to "what bounds this?" at review time. One rule catches the whole class: no unbounded queue may cross a thread boundary.
5. **Coverage targets measured lines, not scenarios.** 60-70% line coverage was met while the highest-risk scenario (hidden + clouds + days of uptime) had zero coverage. The next iteration should enumerate operating modes (visible/hidden x clouds on/off x hours of simulated time) and require a test per mode.
6. **Docs recorded decisions but not invariants.** `../plans` is an excellent archive of research, but operational invariants ("BeforeRendering does not fire when hidden", "all texture messages must be drained somewhere visibility-independent") live buried in dated files. A short `docs/invariants.md` that accumulates such hard-won facts would have prevented the leak from being designed in three weeks after the tray research established the key fact.

## 6. Refactor or rewrite

**Recommendation: incremental restructure into a workspace, not a rewrite.** Reasoning:

- The parts that would survive any rewrite verbatim are large and validated: both WGSL shaders, camera/orbital math, sun position FFI wrapper, datetime logic, texture transforms, mouse math, wallpaper/Win32 code, the config format, and all three test suites. That is roughly half the codebase and most of its accumulated correctness.
- The parts that caused the pain (lifecycle, channels, plumbing, main.rs) need rewriting either way. Doing it inside the existing repo keeps the tests green throughout and keeps the git history and docs attached to the code they describe.
- A greenfield rewrite would re-expose the project to the category of bugs that took three weeks of March to fix (tray/event-loop/teardown), with no guarantee the second system avoids new ones.

A rewrite would only be justified if the stack itself were being replaced (e.g. dropping Slint), and nothing in this retrospective points that way. The stack held; the architecture between the stack components did not.

## 7. Target architecture for the next iteration

The organizing principle: **headless first**. The wallpaper engine must run to completion with no window at all; the settings window is one optional client among several. This single decision resolves the leak class (no visibility-coupled consumption), the testability problem (the engine runs in CI on a software adapter), the tray-mode fragility (hiding the window removes a client instead of half-suspending the engine), and unblocks Linux/macOS daemon operation.

Proposed workspace layout:

```
sunlit-earth/
  crates/
    sunlit-core/      # no Slint dependency, no window
      scene/          # camera, sun, datetime (as today)
      assets/         # texture sources, tiers, decode, mips, caching, cloud fetch
      render/         # wgpu pipeline, offscreen render, readback (as today, minus Slint)
      engine/         # the state machine: owns SceneParams, textures, scheduler
    sunlit-app/       # Slint UI shell: preview window, tray, IPC, config bridge
    sunlit-cli/       # render subcommand, soak runner, benchmark harness (optional split)
```

Key mechanics inside `sunlit-core`:

- **One `SceneParams` struct** replaces the FrameState/ShadingParams/CameraParams/config/UI-property quintuplication. The Slint bridge converts window properties to `SceneParams` in one place; the uniform encoder converts `SceneParams` to GPU bytes in one place. Dirty-checking compares quantized `SceneParams` directly.
- **The engine owns its thread and its resources.** Clients (UI, IPC, scheduler, tests) talk to it over a command channel (bounded) and receive frames or events back. No `thread_local`, no rendering-notifier ownership of GPU state. The Slint preview becomes: engine renders to texture, app converts to `slint::Image`. Teardown becomes ordinary drop order on one thread, which should also retire the `process::exit(0)` hack.
- **Latest-value mailboxes for all asset updates**, one slot per texture, replacement semantics, drained by the engine loop on its own schedule regardless of any window. Compressed bytes or file paths in the mailbox, never decoded pixel buffers.
- **Injected clock and injected asset sources.** The engine takes a `Clock` trait and the fetcher takes a base URL. Production wires the real ones; tests wire a mock clock (hours per test-second) and a local fixture server. This is what makes soak tests possible.
- **Quality tiers as a first-class config** (see section 9), with the low tier as the default for dev and tests.

The existing `--ipc-socket` protocol moves to `sunlit-app` and grows a few commands (`set-param`, `render-to-file`, `query-memory`, `click`/`drag`/`scroll` dispatched as Slint pointer events, `advance-mock-clock` in test builds) to become the automated UI test transport.

## 8. Test and infrastructure plan

### 8.1 Test layers

| Layer | What | Where it runs |
|---|---|---|
| Unit + property | pure functions, as today | every OS, every push |
| Engine integration | headless engine on software adapter: render, export, texture swap, scheduler cycles | every OS, every push |
| Golden images | render fixed configs (fixed datetime, fixed textures, no MSAA) and compare with perceptual tolerance; plus a "contact sheet" artifact (grid of presets) uploaded from CI for human review of visual changes | Linux (lavapipe) + Windows (WARP) every push; macOS (Metal) after the probe job |
| Soak / leak | mock clock + fixture cloud server, simulate 14 days in under a minute, assert RSS and allocation deltas bounded | Linux + Windows, nightly and pre-release |
| UI logic | `i-slint-backend-testing` as today, expanded | every OS, every push |
| Desktop e2e | real binary, IPC-driven, tray/window lifecycle, wallpaper set | local VMs via `cargo xtask e2e`, never the dev desktop |

Golden-image caveat: cross-adapter float variance means goldens should be per-adapter (one set for lavapipe, one for WARP) or compared with a perceptual metric and a tolerance, extending the existing behavioral-invariant convention rather than exact pixels.

### 8.2 Memory: verification, prevention, detection

Immediate verification of the leak diagnosis, before any refactor (also serves as the regression test afterward):

1. Build with memory logging enabled in release, or temporarily log RSS at info level. Run `--mode tray`, hide the window, and point the fetcher at a local stub server that serves a new ETag + JPEG every few seconds. RSS should climb by one decoded-frame quantum (about 134 MB at 8K, about 2 MB with a 1024x512 fixture) per update while hidden, and flush when the window is shown again and a frame renders.
2. The same scenario as an automated soak test is the permanent guard.

Prevention rules (encode in CLAUDE.md / review checklist for the next iteration):

- No unbounded channel crosses a thread boundary. Every channel is bounded or latest-value, and the choice is written down at the declaration site.
- Decoded pixel buffers are never stored in queues, caches, or long-lived structs; only compressed bytes or GPU textures.
- Every background producer names its consumer and the condition under which the consumer runs; if the condition is not "always", the design is wrong.
- Every long-lived process ships an RSS watchdog: sample every few minutes, warn-level log past a soft budget.

Detection tooling, by platform:

- **In tests (all platforms):** a counting `GlobalAlloc` wrapper (hand-rolled or `stats_alloc`) to assert per-cycle heap deltas in soak tests; `dhat-rs` for allocation-site profiles when a delta assertion fails. Both are pure Rust and work on Windows.
- **Linux (the deep-analysis platform):** heaptrack or bytehound for allocation flame graphs, valgrind massif for growth over time, and LeakSanitizer (`-Zsanitizer=address`, nightly) for unreachable leaks. Note that Rust leaks are usually reachable growth (like this one), which LSAN cannot see; the RSS-trend soak test is the primary instrument, sanitizers are secondary.
- **Windows (the production platform):** the built-in `memory.rs` snapshots as the always-on signal; VMMap snapshot diffs and umdh allocation-stack diffs for interactive hunts.
- **GPU side:** wgpu exposes internal allocator/hub reports (`Device::generate_allocator_report`; exact API surface on wgpu 28/29 to be verified) to distinguish driver/GPU memory from heap when RSS and heap counters diverge.

### 8.3 Cross-platform test execution: GitHub Actions matrix plus local VMs

Decisions settled on 2026-08-15: the full three-OS matrix runs on GitHub Actions within hosted-runner limits; local desktop e2e runs in VMs orchestrated by a `cargo xtask` layer; QEMU is the standard hypervisor with one exception (Hyper-V for the Windows guest when the host is Windows); no self-hosted GitHub runners; no macOS hardware, so macOS coverage lives exclusively on GitHub-hosted runners; real-GPU runs stay manual on the developer host for now.

**The GitHub Actions matrix.** Extend `ci.yml` to `ubuntu-latest`, `windows-latest`, and `macos-latest`, and fix the two commented-out gates (`cargo fmt`, `-D warnings`) first, since a three-OS matrix multiplies the cost of warning debt. What each hosted platform can and cannot do:

- **Linux** is the most capable hosted platform for windowed tests: `xvfb-run` (or headless Weston for the Wayland path) provides a real display server, winit opens windows, lavapipe runs the wgpu pipeline, and OS-level input injection via `xdotool` plus screenshots work. Windowed click-through tests run here on every push.
- **Windows** covers everything up to window creation and WARP rendering (the current CI already proves the GPU layer), but hosted agents have no interactive desktop session; Microsoft documents visible-UI desktop testing as unsupported on hosted agents ([UI testing considerations](https://learn.microsoft.com/en-us/azure/devops/pipelines/test/ui-testing-considerations?view=azure-devops)). OS-level input injection and tray tests are out of scope here and live in the local Windows VM.
- **macOS** (Apple Silicon runners) exposes a paravirtualized GPU with Metal, and wgpu's own CI runs its GPU test suite on plain `macos-14` runners (section 11, question 1), so the engine and render layers are expected to work there, pending a one-time probe job. Desktop-session automation is constrained by macOS permission prompts (TCC); treat macOS as a build, unit, engine, and render platform.

**Input injection principle.** Click-through tests inject input at the Slint layer, not the OS layer: `window().dispatch_event(WindowEvent::PointerPressed { .. })` and friends, driven over the IPC channel (commands like `click x y`, `drag`, `scroll`). Tests written this way run identically anywhere a window can be created: under xvfb on Linux runners, on hosted Windows runners, and in the local VMs, because they do not depend on an interactive desktop or OS input APIs. OS-level tests (a real click on the tray icon, the wallpaper visibly changing) are a thin layer that runs only in the local VMs.

**Local VMs, orchestrated by cargo xtask.** A small `xtask` crate in the workspace is the single entry point for humans and any CI: `cargo xtask e2e --target windows|linux [--keep]`, `cargo xtask vm build-image <target>`, `cargo xtask vm ssh <target>`. Test tiers (unit, engine, windowed, desktop-e2e) are defined once as xtask commands, and the GitHub workflows call the same commands, so the CI YAML stays a thin scheduling shell with no orchestration knowledge of its own. The provider matrix:

| | Windows guest | Linux/GNOME guest |
|---|---|---|
| Windows host | Hyper-V (differencing VHDX) | QEMU + WHPX (qcow2 overlay) |
| Linux host | QEMU + KVM (qcow2 overlay) | QEMU + KVM (qcow2 overlay) |

Implementation notes:

- **One provider trait, two implementations.** `create_from_golden`, `start`, `wait_ssh`, `exec`, `copy_in`, `collect_results`, `destroy`; implemented for Hyper-V (PowerShell cmdlets) and for QEMU (process spawn plus QMP socket). Provider selection is host OS plus guest OS with a config override. QEMU on Windows uses WHPX acceleration, which runs on top of the Hyper-V hypervisor, so the two providers coexist on the same host by design (the "Windows Hypervisor Platform" optional feature must be enabled alongside Hyper-V).
- **Clean state via throwaway overlays, not checkpoints.** Golden images are read-only; each run creates a disposable child (qcow2 backing file for QEMU, differencing VHDX for Hyper-V) and deletes it afterward. Every run starts pristine and there is no snapshot tree to manage.
- **One guest contract regardless of provider.** Autologon, an OpenSSH server, and a results directory the xtask pulls back. On Windows, processes started over SSH have no access to the interactive desktop, so the golden image pre-registers a scheduled task ("run only when user is logged on") that executes the test job in the console session; the orchestrator drops a job file over SSH and fires `schtasks /run`, then polls for results. On Linux, the image autologs into a GNOME session and tests run against that session's display; disable GNOME animations in the golden image (`gsettings set org.gnome.desktop.interface enable-animations false`), since the shell composites via llvmpipe.
- **Image strategy.** The Linux guest is a single qcow2 file used unchanged on both hosts. The Windows guest is one canonical golden image with virtio drivers pre-installed (Hyper-V integration services are built into Windows), converted between VHDX and qcow2 with `qemu-img convert` so both providers boot the same image. Build images with Packer's QEMU builder (Windows Enterprise evaluation ISO plus `autounattend.xml`; a GNOME distribution via cloud-init or preseed); templates live in the repo, the multi-GB images live outside it with a checksum manifest in the repo so the xtask can detect a missing or stale image and explain how to rebuild it.
- **Build on the host, copy artifacts in.** Both guests are x86_64; the Windows host builds Windows binaries natively and Linux binaries via WSL2 (matching the guest's base distro for glibc compatibility), and `cargo test --no-run --message-format=json` yields the test executable paths to copy. One harness change is required: `tests/e2e.rs:23` locates the app binary via `env!("CARGO_BIN_EXE_sunlit-earth")`, a compile-time host path; add a `SUNLIT_EARTH_BIN` environment override with the compile-time value as fallback.

The e2e harness itself already has the right shape (spawn binary, IPC commands, SIGNAL lines, log assertions); the remaining work is porting its environment assumptions (paths, `%LOCALAPPDATA%`, tray) per OS.

**Considered and rejected.** Self-hosted GitHub runners would have provided the orchestration for free, but they execute workflow code on private hardware; the repo is private today and will be published later, and self-hosted runners on public repos are a standing security risk (pull requests can run arbitrary code on the host). Vagrant abstracts the same hypervisors but adds a BUSL-licensed dependency with slowed development and would still leave the Windows interactive-session problem unsolved. A local Xvfb container as a fast Linux inner loop is redundant with the CI xvfb layer plus the GNOME VM; add it later only if VM boot time becomes an annoyance. Mac hardware (which would enable Tart VMs with paravirtual Metal) is not planned; macOS stays on hosted runners.

### 8.4 GPU coverage: what software adapters do and do not test

Accepting software rendering in VMs and CI is safer than it sounds, for an architectural reason: lavapipe is a conformant Vulkan implementation and WARP a conformant D3D12 implementation, so wgpu runs its complete Vulkan backend (naga to SPIR-V, barriers, allocator) against lavapipe and its complete DX12 backend (naga to HLSL/DXIL) against WARP. All application code and all wgpu code is exercised; the untested layer is the vendor kernel driver and the silicon. The software path is also a shipped configuration (`--software-rendering`), so this is coverage of a real production mode, not merely a compromise. With the macOS runners added, all three shader translation targets (SPIR-V, HLSL, MSL) are covered in CI.

The residual risks and their mitigations:

1. **Vendor driver and shader compiler bugs**, the largest real-world risk for an app running on arbitrary consumer machines: mitigated by a real-GPU e2e run on the developer host, manual for now, wrapped as `cargo xtask e2e --target host` so it is the same command and can be automated later. Full driver-matrix coverage lives on user machines by necessity, so design for diagnosability: the adapter is already logged; add device-lost handling with automatic fallback to software rendering and a clear log line.
2. **Precision and filtering differences** (mip selection, MSAA resolve, rounding): absorbed by the existing behavioral-invariant convention and per-adapter golden images with perceptual tolerance.
3. **Timing-dependent behavior**: real GPUs are asynchronous in a different ratio to CPU work, and software adapters hide missing throttling. The soak tests and the host real-GPU run are the guard.
4. **Memory pressure and limits**: an 8K mip chain that fits in system RAM may not fit a 2 GB iGPU. Add a limits-emulation CI job that requests artificially low limits (for example a small `max_texture_dimension_2d`) on the software adapter to exercise the tier-degradation paths without owning weak hardware.
5. **Adapter selection**: the discrete/integrated/CPU ranking never sees a hybrid-GPU laptop in CI; it stays unit-tested (`adapter_type_rank`) and diagnosable via the adapter log line.

## 9. Performance plan: fast by default, pretty by choice

Invert the current asset strategy. Today the app decodes 8K JXL/JPEG assets and builds their mip pyramids on the CPU at every startup; the next iteration starts from a low tier and treats resolution as an upgrade:

- **Quality tiers** in config: `low` (1024x512 or 2048x1024 textures, no MSAA, clouds optional), `medium` (4K, MSAA 4x), `high` (8K, MSAA 8x). Dev builds and all tests default to `low`; the cloud service already publishes 1024/2048/4096/8192 variants, so the low tier needs no new asset work. Startup always shows a low-tier frame first and upgrades in the background (progressive enhancement), which also fixes perceived startup time in release.
- **Offline asset pipeline** (a small `xtask` or build tool): pre-flip, pre-shift, and pre-generate mipmaps into a container format (KTX2 is the natural fit; the `ktx2` crate is proven, see section 11, question 3), so runtime texture load becomes read + upload. This deletes `downsample_2x` and `shift_horizontal` from the runtime entirely and with them the UI-thread stalls of section 4.2. Optionally evaluate BC7 compression (about 4x VRAM saving, decode-free sampling); support on the relevant adapters checks out (section 11, question 3), but gate on wgpu's `TEXTURE_COMPRESSION_BC` feature at runtime with an uncompressed fallback.
- **Whatever decode remains happens off the UI thread, always**, including the cached cloud image at startup and mip upload.
- **Cheap wins to take regardless:** the equirectangular flip + quarter-shift is a UV affine transform and can move into the shader or the offline pipeline for free; the app crate can get `opt-level = 1` or 2 in the dev profile if debug ergonomics allow.
- **Budgets, measured in CI:** startup to visible window, startup to first frame, engine render time per tier, RSS baseline per tier. The tracing spans (`FmtSpan::CLOSE`) already produce the timings; a benchmark job records them per commit so regressions are visible as diffs instead of vibes.

## 10. Phased roadmap

Revised 2026-08-15 after discussion. Phases 0 to 3 are commitments in this order; what comes after is direction, to be refined when we get there.

Phase 0, stop the bleeding, with proof (Windows only, ships as 0.1.1). The reproduction test comes first so the fix has a red test to turn green. Detailed plan: `../plans/2026-08-15-phase0-memory-leak-plan.md`. Items 1 to 4 are implemented; the diagnosis in section 4.1 was confirmed exactly (120.4 MiB of private-bytes growth across 15 hidden cloud updates with a 2048x1024 fixture, 8.03 MiB per update, and no GPU texture created between hide and show), and the fix brings that to 1.8 MiB. Only the multi-day validation from section 8.2 and the 0.1.1 tag remain. The optional compressed-payload hardening in item 3 was not needed and stays optional: with a five-second drain a frame is parked for at most one timer period.
1. Test knobs: environment overrides for the cloud URL, poll interval, and cache directory (all currently hardcoded in `cloud_fetcher.rs`), plus a `query-memory` IPC command, so the leak reproduces in minutes instead of days and memory can be sampled deterministically.
2. Leak regression test: a Windows e2e test that runs the real binary with the window hidden via IPC, serves fixture cloud JPEGs with rotating ETags from a local HTTP stub, and asserts bounded memory across many updates. Red against current code (growth of one decoded frame per update), green after the fix.
3. The fix: per-slot latest-value mailboxes with replacement semantics, drained by an event-loop timer that runs regardless of window visibility and processes updates, so wallpaper exports keep getting fresh clouds. Optional hardening: carry compressed bytes or a cache-file notification instead of decoded pixels to shrink the parked worst case.
4. Production memory telemetry that survives release builds: periodic samples to a small metrics file plus a warn-level budget alert; manual verification on the real binary per section 8.2.

Phase 1, the restructure (section 7), with nothing else mixed in. **Implemented**; see `../plans/2026-08-15-phase1-restructure-plan.md` for the step-by-step record and the measurements. All four items landed, including the descopeable Slint 1.17 upgrade and the `SystemTrayIcon` replacement. The `process::exit(0)` and `mem::forget` teardown hacks are gone: with the engine owning the wgpu device, Slint holds no wgpu object and there is no cross-library destruction order to get wrong.
1. Extract `sunlit-core`: engine owns its thread and resources, `SceneParams` unification, injected clock and asset sources; retire the thread-local and the exit hacks.
2. The tests the restructure enables, as its acceptance criteria: mock-clock soak test (14 simulated days in seconds), engine integration tests, golden-image + contact-sheet job on the existing Windows CI (WARP).
3. Low quality tier as the default for dev and tests (config plus small texture variants; the cloud service already publishes them).
4. Port `sunlit-app` onto the engine; upgrade to Slint 1.17 + wgpu 29 (`SystemTrayIcon` replaces `tray.rs`; test whether the `process::exit(0)` workaround is still needed). Splits into its own step if the upgrade fights back.

Phase 2, basic cross-platform support:
1. Build and run on Linux and macOS: cfg gates, paths, per-OS memory snapshots; the engine and render layers pass on lavapipe and Metal.
2. Then expand CI to the three-OS matrix (build/unit/engine everywhere, windowed tests under xvfb on Linux, the Metal probe job on macOS); re-enable `fmt` and `-D warnings`; per-adapter golden images.

Phase 3, VM orchestration (section 8.3):
1. The xtask crate with the provider trait (Hyper-V and QEMU), golden images, and the guest contract.
2. Desktop e2e moves onto the VMs, off the dev desktop.

Later, direction rather than commitment, refined when we get there: full Linux wallpaper setters (GNOME/KDE) and Linux tray via StatusNotifier; macOS wallpaper and tray against hosted runners; multi-monitor via `IDesktopWallpaper` (section 11, question 5); the offline KTX2 asset pipeline, BC7, progressive startup, and benchmark budgets in CI (section 9).

## 11. Research questions: answers (researched 2026-08-15)

The open questions from the first version of this document, now answered. What remains open is listed at the end.

**1. Do GitHub-hosted macOS runners expose a usable GPU for wgpu?** Yes. The Apple Silicon runners (`macos-14` and later) run under Apple's Virtualization framework with a paravirtualized GPU that exposes Metal, and GitHub's Apple Silicon runner announcement states GPU acceleration is enabled. The strongest evidence is that [wgpu's own CI](https://github.com/gfx-rs/wgpu/blob/trunk/.github/workflows/ci.yml) executes its GPU test suite on plain `macos-14` hosted runners. Consequence: macOS can run the engine, render, and golden-image layers in CI, not just build and unit tests, and this also completes shader-translation coverage across all three naga targets (SPIR-V via lavapipe, HLSL/DXIL via WARP, MSL via Metal). Remaining action: a one-time probe job confirming our specific pipeline (MSAA, mipmapped 8K textures, buffer readback) on the paravirtual device.

**2. Does Slint 1.17's SystemTrayIcon cover what tray.rs does?** Yes on Windows, with one macOS caveat. The [`SystemTrayIcon` element](https://docs.slint.dev/latest/docs/slint/reference/window/systemtrayicon/) (macOS, Windows, Linux) has `icon`, `tooltip`, `visible`, and `title` properties, exactly one child `Menu`, and a `clicked()` callback. On Windows, left-click fires `clicked()` and right-click opens the menu, which matches the current behavior (left-click toggles the window, context menu for actions), and `MenuItem` supports `checkable`/`checked` with automatic toggling on activation, which covers the Auto-refresh checkmark. Adopting it removes the Win32 message pump, the crossbeam sync channel, and most of the 310 lines of `tray.rs`. Caveats: on macOS `clicked()` only fires when no menu is attached, so the show/hide toggle must become a menu item there; keyboard shortcuts in tray menus are ignored; on Linux a StatusNotifierItem-capable desktop is required (stock GNOME needs the AppIndicator extension, which Ubuntu ships by default). The upgrade brings wgpu 29 along via the `unstable-wgpu-29` feature with `slint = "~1.17"`. Whether the `WGPUConfiguration::Manual` teardown panic (the reason for the `process::exit(0)` workaround, `src/main.rs:522-530`) still reproduces on 1.17/wgpu 29 could not be determined from public sources and must be tested during the upgrade; the engine restructure of section 7 removes the hazard independently by taking GPU resources out of Slint's thread-local state.

**3. Is the KTX2 + BC7 asset pipeline feasible?** Yes. The pure-Rust [`ktx2` crate](https://github.com/BVE-Reborn/ktx2) parses KTX2 containers with an iterator over mip levels and supercompression support, and it is what Bevy uses in production for exactly this purpose (loading pre-generated mip chains into wgpu), so the offline-mips plan in section 9 stands on proven ground. On BC7: it is a mandatory Direct3D 12 format, so WARP supports it; Mesa's software stack has shipped BPTC/BC support for years and lavapipe runs D3D12-over-Vulkan titles that require BC formats; Apple Silicon Metal supports BC as well. Regardless, the correct implementation gates on wgpu's `TEXTURE_COMPRESSION_BC` feature at adapter init and falls back to uncompressed textures, which turns any adapter without BC into a handled case instead of a bug.

**4. Has the Slint testing backend grown pixel capabilities since 1.15?** Yes. As of 1.17, [`i-slint-backend-testing`](https://docs.rs/i-slint-backend-testing) embeds an MCP server exposing a `take_screenshot` tool (base64 PNG) along with click, drag, and type tools, so the testing backend can now render pixels and simulate richer input than the 1.15 API surveyed in `../plans/2026-03-24-slint-testing-research.md`. It is aimed at interactive, agent-assisted development rather than CI determinism, so the golden-image layer stays on the wgpu render path, but it is worth adopting for local UI debugging and exploratory testing.

**5. Multi-monitor wallpaper.** The OS API exists and shapes the export design: Windows 8+ provides the `IDesktopWallpaper` COM interface with [`SetWallpaper(monitor_id, path)`](https://learn.microsoft.com/en-us/windows/desktop/api/shobjidl_core/nf-shobjidl_core-idesktopwallpaper-setwallpaper) plus `GetMonitorDevicePathCount`/`GetMonitorDevicePathAt` for per-monitor wallpapers, superseding the current single-wallpaper `SystemParametersInfoW` call. The practical binding is the `windows` crate (`Win32_UI_Shell` feature); `windows-sys` exposes COM only as raw vtables. The alternative without per-monitor COM is composing one spanned image across the virtual desktop with `WallpaperStyle=22`. Either way the headless engine design fits naturally: render one frame per monitor (or one spanned frame) per refresh. The [`more-wallpapers`](https://github.com/LuckyTurtleDev/more-wallpapers) crate demonstrates per-screen wallpaper setting across platforms and is a useful reference for the future Linux and macOS setters.

**Still open** (small, deferred to the phases where they matter):

- ~~Does the `process::exit(0)` teardown workaround remain necessary on Slint 1.17 / wgpu 29?~~ Answered in Phase 1: no. The workaround was removed on Slint 1.17 with wgpu 28, and the reason it is no longer needed is structural rather than a version fix. Slint no longer receives a wgpu device at all (the `unstable-wgpu-28` feature is gone), so the thread-local destruction ordering between Slint's backend state and wgpu's `Queue::drop` cannot arise. The wgpu 29 bump is now independent of the Slint version and is not needed.
- Does our render pipeline pass on the macOS paravirtual GPU? One probe job in Phase 2.
- Which StatusNotifier setup the Linux VM image needs for tray e2e (GNOME plus extension vs KDE)? Decide when Linux tray work starts, after Phase 3.
