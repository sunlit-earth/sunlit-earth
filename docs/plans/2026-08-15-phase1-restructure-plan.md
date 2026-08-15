# Plan: Phase 1, Restructure into a Headless-First Workspace

## Summary

Restructure the crate into a Cargo workspace with a headless `sunlit-core` (scene, assets, renderer, engine) and a thin `sunlit-app` (Slint UI shell). The engine owns its own thread, wgpu device, and resources, consumes a single unified `SceneParams` struct, and takes an injected clock and asset source so multi-day behavior can be simulated in seconds. The Slint UI becomes one client of the engine; the preview is delivered as pixel buffers instead of shared GPU textures, which removes the `WGPUConfiguration::Manual` coupling, the GPU thread-local, and (expected) the `process::exit(0)` teardown hack. Acceptance is defined by new tests: a mock-clock soak test, engine integration tests, and golden images on the existing Windows CI. Also in scope: quality tiers with a low default for dev and tests, and the Slint 1.17 upgrade with `SystemTrayIcon` replacing `tray.rs`. This is Phase 1 of `../retrospective-2026-08.md` section 10; the architecture is section 7.

## Stakes Classification

**Level**: High

**Rationale**: This touches every module and changes the app's threading and rendering data flow. Mitigations: the work is staged so the build and test suite stay green after every step (strangler pattern: the engine is built alongside the old path, then the app is switched over, then the old path is deleted); the Phase 0 regression test and the existing e2e suite run unchanged as black-box guards; the riskiest steps (Slint 1.17 upgrade, tray replacement) are explicitly descopeable.

## Research

- `../retrospective-2026-08.md` sections 4.4 (parameter amplification, lifecycle coupling), 7 (target architecture), 8.1/8.2 (test layers, soak tests), 10 Phase 1, 11 questions 1-4
- `2026-03-25-slint-shutdown-research.md` and the `process::exit(0)` comment in `src/main.rs` (teardown hazard being removed)
- Slint 1.17 release notes and `SystemTrayIcon` docs (retrospective section 11, question 2)

## Key Design Decisions

**D1: The engine gets its own wgpu device on its own thread; the preview becomes pixel buffers.** Today the wgpu device is shared with Slint via `WGPUConfiguration::Manual` and the preview is a zero-copy `Image::try_from(Texture)`. Instead, the engine creates its own device (moving `wgpu_init`), renders offscreen exactly as today, and sends preview frames to the app as RGBA byte buffers; the app wraps them in `slint::Image::from_rgba8(SharedPixelBuffer)`. Consequences, all intended: Slint no longer needs the `unstable-wgpu-28` feature or the wgpu backend selector (its own UI renders with the default renderer); the wgpu 29 bump stops being coupled to the Slint version (we keep wgpu 28 in this phase); the thread-local `GPU_RESOURCES` and the TLS teardown ordering problem disappear structurally. Cost: one CPU copy per preview frame. At quantized preview sizes (about 2 MB per frame) this is well within budget even during mouse drags; the readback path (`read_texture_rgba8`) already exists and is tested.

**D2: The engine never sleeps on wall time for scheduling decisions.** The engine loop blocks on its command channel with a short real timeout, and on each wake computes due work (sun-position refresh, auto-refresh export, cloud poll) from `clock.now()`. Production wires `SystemClock`; tests wire a `MockClock` and drive determinism by advancing it and sending a `Poke` command. This is what makes the 14-simulated-days soak test possible.

**D3: Asset fetching goes behind a trait.** `CloudSource` (check freshness, fetch bytes) with an HTTP implementation extracted from today's `cloud_fetcher` and a fixture implementation for tests. Decode and mip generation run on a worker thread owned by the engine, never on the UI thread (this also fixes the section 4.2 UI stalls as a side effect). The Phase 0 mailbox and env knobs carry over; the soak test uses the fixture source directly, no HTTP.

**D4: One `SceneParams` struct.** Defined in core, it replaces the parameter quintuplication (Slint properties, `AppConfig`, `FrameState`, `ShadingParams`, `Uniforms` construction). Exactly two translation points remain: the Slint bridge in the app (window properties to `SceneParams` and back) and the uniform encoder in core (`SceneParams` to GPU bytes). Dirty checking compares quantized `SceneParams`.

**D5: Package name and binary name stay `sunlit-earth`.** The binary crate is `crates/sunlit-app` but keeps `name = "sunlit-earth"` in its manifest so `CARGO_BIN_EXE_sunlit-earth`, the e2e suite, CI, and the release workflow keep working.

## Success Criteria

- [ ] Workspace builds; `cargo test` green at every step boundary (commit per step)
- [ ] `render` subcommand produces a PNG with no window and no Slint backend involvement
- [ ] Phase 0 regression test passes unchanged (black box), and the whole Phase 0 surface keeps working: all `SUNLIT_EARTH_*` env knobs, the `query-memory` IPC command, the memory metrics CSV, and the drain-while-hidden semantics (the Phase 0 mailbox and drain timer become engine-internal, but the observable behavior is identical)
- [ ] Existing e2e suite passes (adapted only where paths or startup logs changed)
- [ ] New engine integration tests pass headlessly on the software adapter
- [ ] Mock-clock soak test: 14 simulated days of cloud updates and auto-refresh in under a minute, bounded private bytes
- [ ] Golden-image test with tolerance, plus contact-sheet artifact job in CI
- [ ] Quality tiers exist; dev/test default is low; release default unchanged in output quality
- [ ] Slint 1.17: tray via `SystemTrayIcon`, `tray.rs` message pump deleted; teardown without `process::exit(0)` attempted and outcome documented
- [ ] `cargo clippy` clean; CLAUDE.md rewritten for the new layout

## Implementation Steps

Each step ends with a commit and a green `cargo test` + `cargo clippy`.

### Step 1: Workspace scaffolding, everything moves, nothing changes

**Files**: root `Cargo.toml` (becomes virtual workspace), `crates/sunlit-app/**` (git mv of the entire package: `src/`, `ui/`, `shaders/`, `tests/`, `build.rs`, package manifest)

Root manifest becomes a `[workspace]` with `members = ["crates/*"]` and shared `[workspace.lints]`/`[workspace.dependencies]` where convenient. The package keeps `name = "sunlit-earth"` (D5). `textures/` stays at the repo root (the resolution walk-up from the exe and the cwd-relative lookup both still find it). Update `.github/workflows/*.yml` only if a path assumption breaks (`cargo test --locked` at the root works on a workspace). Verify e2e still runs (`CARGO_BIN_EXE_sunlit-earth` resolves).

### Step 2: Create `sunlit-core`, move the leaf modules

**Files**: `crates/sunlit-core/**`; moves: `scene/`, `geometry/`, `texture_loader.rs`, `memory.rs`, `wallpaper.rs`, `config.rs`, `wgpu_init.rs`; `cloud_fetcher.rs` moves with one decoupling change

`sunlit-core` has no Slint dependency (enforced by its manifest). The only source-level decoupling needed at this step: `cloud_fetcher` currently takes `slint::Weak<MainWindow>` for redraw nudges; replace with a generic `notify: Arc<dyn Fn() + Send + Sync>` the caller provides. The `env_override` helper (Phase 0, currently `pub(crate)` in `lib.rs`) moves to core and is re-exported, since the knobs it serves (`SUNLIT_EARTH_CLOUD_URL`, `SUNLIT_EARTH_CLOUD_POLL_SECS`, `SUNLIT_EARTH_CACHE_DIR`, `SUNLIT_EARTH_CONFIG`, `SUNLIT_EARTH_METRICS_DIR`) all belong to modules moving here. `mouse_math.rs`, `ui_callbacks.rs`, `ipc.rs`, `tray.rs`, `main.rs`, and `renderer/` stay in the app for now.

### Step 3: `SceneParams` in core

**Files**: `crates/sunlit-core/src/params.rs`, touched call sites in renderer/app

Define `SceneParams` (camera, shading, clouds, atmosphere, color correction, datetime input, texture selection, sample count) with `Default` matching `AppConfig::default()`, quantized comparison (the `FrameState` thousandths convention), and conversions: `AppConfig <-> SceneParams`. Rework `renderer::frame::FrameState` and `render_pass::ShadingParams` construction to be derived from `SceneParams` so the struct is proven before the engine exists. The Slint bridge (`read_params_from_window`, `apply_params_to_window`) lands in the app next to the existing config bridge, which shrinks to geometry-and-persistence concerns.

### Step 4: The engine in core, built alongside the old path

**Files**: `crates/sunlit-core/src/engine/` (new), `renderer/` moves into core in this step with its Slint touchpoints removed

Engine surface (keep it minimal):

- `Engine::start(EngineConfig) -> EngineHandle`, where `EngineConfig` carries the adapter preference, texture paths, quality tier, `Arc<dyn Clock>`, `Arc<dyn CloudSource>`, and an event callback `Arc<dyn Fn(EngineEvent) + Send + Sync>`.
- Commands (channel): `UpdateParams(SceneParams)`, `SetPreviewSize(u32, u32)`, `RenderWallpaperNow`, `RenderToFile { path, w, h }`, `SetAutoRefresh { enabled, interval }`, `Poke`, `Shutdown`.
- Events: `PreviewFrame { rgba: Vec<u8>, w, h }`, `TexturesReady`, `WallpaperSet(Result)`, `Status(String)` for the loading text.
- Loop per D2: wake on command or short timeout; compute due work from `clock.now()`; dirty-check `SceneParams` (plus sun direction recomputed from the clock) before rendering; render offscreen and read back for preview; run wallpaper export at native resolution on demand or on schedule.

The renderer moves to core here: `execute_render_pass` returns the texture/pixels instead of a `slint::Image`; `create_gpu_resources` takes the engine's own device (from `wgpu_init`, moved in Step 2); the mailbox drain happens inside the engine loop (the Phase 0 timer becomes engine-internal). The old app-side rendering notifier keeps working during this step by delegating where practical; if dual operation is more work than it saves, keep the old path compiling but inert behind the existing code path and switch in Step 5.

### Step 5: Switch the app to the engine, delete the old path

**Files**: `crates/sunlit-app/src/main.rs`, `ui_callbacks.rs`, `ipc.rs`, deletion of the app-side renderer wiring

- Startup: no `BackendSelector::require_wgpu_28`; create `MainWindow` with the default backend; start the engine; register the event callback that forwards `PreviewFrame` via `invoke_from_event_loop` into a `slint::Image`.
- UI callbacks: every change callback reads the window into `SceneParams` and sends `UpdateParams`; mouse handlers unchanged except they no longer call `request_redraw` (the engine replies with a frame).
- The `render` subcommand becomes fully headless: start engine, `RenderToFile`, wait, exit, no window created at all.
- IPC: `show-window`/`hide-window`/`quit` unchanged; `export-test` and `query-memory` forward to the engine or stay as is.
- Remove `GPU_RESOURCES`, the rendering notifier, the sun timer, the drain timer, and the auto-refresh timers from `main.rs` (all engine-internal now). Attempt removal of `process::exit(0)` and the `mem::forget` calls; keep whichever proves still necessary and document why in this plan's Results.

### Step 6: Quality tiers

**Files**: `crates/sunlit-core/src/config.rs`, engine, cloud source

`quality_tier: low | medium | high` in config and `EngineConfig`. Low: preview capped (about 1280 wide), MSAA off, cloud fetch at the 2048x1024 URL variant, skip the 8K JXL textures unless explicitly selected. High: current behavior. Default: low in debug builds, high in release (`cfg!(debug_assertions)`); tests always low. The cloud URL resolution per tier composes with the Phase 0 env override (env wins).

### Step 7: The new test layers

**Files**: `crates/sunlit-core/tests/engine.rs`, `crates/sunlit-core/tests/soak.rs`, `crates/sunlit-core/tests/golden.rs`, `tests/common` reuse, `.github/workflows/ci.yml`

1. Engine integration tests (software adapter, shared device per existing convention): submit params, receive a preview frame, assert invariants (dimensions, non-black, dirty-check suppresses identical re-renders, texture swap produces a new frame).
2. Mock-clock soak test: fixture `CloudSource` publishing a new 2048x1024 frame every simulated 3 hours, auto-refresh every simulated 30 minutes with the wallpaper-setting call stubbed to export-only; advance 14 simulated days; assert private bytes (via `memory::snapshot`) bounded within one frame of baseline and that exports happened on schedule.
3. Golden images: fixed `SceneParams` sets rendered at 512x288 with the grid texture (deterministic, no external assets), compared to checked-in references with a perceptual/percentage tolerance; regenerate via env flag. Contact sheet: presets grid rendered into one PNG, uploaded as a CI artifact for human review.

### Step 8: Slint 1.17 and the tray (descopeable)

**Files**: `crates/sunlit-app/Cargo.toml` (`slint = "~1.17"`, no wgpu feature), `ui/main.slint`, deletion of most of `tray.rs`

Upgrade Slint; fix any DSL/API breakage. Replace the tray thread with `SystemTrayIcon` in the `.slint` file: menu (Open, Refresh Now, checkable Auto-refresh, Exit), `clicked()` toggles window visibility (Windows behavior per retrospective section 11 question 2). Keep `single-instance` and the icon generation (feed the generated icon to the element). Delete the Win32 message pump, the crossbeam sync channel, and the tray thread. Adapt the tray-related e2e tests (they drive IPC, not the tray, so changes should be small). If the upgrade or the tray element fights back after honest effort, descope this step to a follow-up: record the findings in this plan, keep `tray.rs`, and continue.

### Step 9: Docs and CI

**Files**: `CLAUDE.md`, `.github/workflows/ci.yml`, `docs/retrospective-2026-08.md`, `docs/roadmap.md`

Rewrite CLAUDE.md for the workspace layout, engine architecture, new test commands, and tier defaults. CI: ensure workspace-wide test invocation, add the golden/contact-sheet steps. Note Phase 1 completion status in the retrospective; update roadmap entries that this phase resolved (non-blocking texture loading; memory budget entry gets a pointer to tiers).

## Risks and Mitigations

**Preview latency or jank from pixel-buffer round trips during mouse drags.** Mitigation: preview sizes are quantized and small; coalesce `UpdateParams` (engine renders the latest params, skipping stale intermediates, which the mailbox pattern already models). If still visible, reduce preview readback size at low tier.

**Slint 1.17 upgrade breakage.** Isolated in Step 8, explicitly descopeable; everything before it runs on Slint 1.15.

**The teardown panic persists even without Manual wgpu config.** Then keep `process::exit(0)` with an updated comment; the structural win (engine-owned device) stands regardless.

**e2e timing changes** (first frame now arrives via engine round trip). Mitigation: the `SIGNAL:first_frame_rendered` marker moves to the app's first `PreviewFrame` handling; tests key on signals, not timing.

**Scope blowout.** The strangler staging keeps every commit green; Steps 6 and 8 are the designated descope levers, and the contact sheet inside Step 7 is optional. Core acceptance is Steps 1-5 and 7.

## Rollback Strategy

Every step is a separate commit on `feat/phase1-restructure`, stacked on the Phase 0 branch; revert individual steps or drop the branch. No force pushes.

## Status

- [x] Plan approved
- [x] Steps 1-2: workspace + core extraction
- [ ] Steps 3-4: SceneParams + engine
- [ ] Step 5: app switched, old path deleted
- [ ] Steps 6-7: tiers + new test layers
- [ ] Step 8: Slint 1.17 + tray (or descoped with findings)
- [ ] Step 9: docs + CI

## Deviations

Recorded as they happened, smallest change that kept the plan's intent.

## Results

Progress log, filled in as each step lands.

### Baseline before Step 1

`cargo test`: 319 tests pass (272 lib, 19 render_pipeline, 12 shading, 16 slint_ui), 8 e2e ignored.
`cargo clippy --all-targets`: 33 warnings, all pre-existing pedantic lints. Full e2e suite passes
in 43 seconds on the development desktop.

### Step 1: workspace scaffolding

The whole package moved to `crates/sunlit-app` with `git mv`, keeping `name = "sunlit-earth"` so
the binary path, `CARGO_BIN_EXE_sunlit-earth`, and the release workflow's `target/release/
sunlit-earth.exe` all stay valid. The root manifest became a virtual workspace with
`resolver = "3"`, shared `[workspace.dependencies]`, and shared `[workspace.lints]`. `textures/`
stayed at the repo root: the e2e-spawned binary no longer finds it through the cwd-relative lookup
(cwd is now the crate directory) but the walk-up from the executable in `target/debug` still does.
`Cargo.lock` did not change, so `--locked` keeps working. No workflow file needed edits.

Verification: `cargo test` 319 pass, `cargo clippy --all-targets` 33 warnings (unchanged),
`cargo test --test e2e -- --ignored` 8 pass in 43 seconds.

### Step 2: sunlit-core extracted

`crates/sunlit-core` now owns `config`, `memory`, `wallpaper`, `wgpu_init`, `scene/`, `geometry/`,
and a new `assets/` module holding `texture_loader`, `cloud_fetcher`, and `mailbox`. It has no
Slint dependency. Three decoupling changes were needed:

- `wgpu_init::WgpuContext` no longer carries a `slint::wgpu_28::WGPUConfiguration`. It returns the
  instance, adapter, device, and queue, and the app assembles `WGPUConfiguration::Manual` itself.
  Step 5 deletes that assembly.
- `cloud_fetcher::spawn_cloud_fetcher` takes `NotifyFn = Arc<dyn Fn() + Send + Sync>` instead of a
  `slint::Weak<MainWindow>`. The app passes a closure that hops onto the event loop and requests a
  redraw, so the observable behavior is unchanged.
- `TextureMailbox` and `DecodedTextureMessage` moved from `renderer/textures.rs` to
  `assets::mailbox` with their four unit tests, because the cloud fetcher needs them and the
  renderer is still app-side. `renderer` re-exports both, so `renderer::TextureMailbox` still
  resolves for existing call sites.

Verification: `cargo test` 319 pass (181 core, 91 app, 19 render_pipeline, 12 shading, 16
slint_ui), `cargo clippy --all-targets` 31 warnings, all present in the Step 1 baseline (the count
dropped because lib and lib-test no longer double-report the same lints across one crate).
Full e2e suite 8 pass in 43 seconds.
