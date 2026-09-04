# Plan: Phase 1, Restructure into a Headless-First Workspace

## Summary

Restructure the crate into a Cargo workspace with a headless `sunlit-core` (scene, assets, renderer, engine) and a thin `sunlit-app` (Slint UI shell). The engine owns its own thread, wgpu device, and resources, consumes a single unified `SceneParams` struct, and takes an injected clock and asset source so multi-day behavior can be simulated in seconds. The Slint UI becomes one client of the engine; the preview is delivered as pixel buffers instead of shared GPU textures, which removes the `WGPUConfiguration::Manual` coupling, the GPU thread-local, and (expected) the `process::exit(0)` teardown hack. Acceptance is defined by new tests: a mock-clock soak test, engine integration tests, and golden images on the existing Windows CI. Also in scope: quality tiers with a low default for dev and tests, and the Slint 1.17 upgrade with `SystemTrayIcon` replacing `tray.rs`. This is Phase 1 of `../reviews/2026-08-15-retrospective.md` section 10; the architecture is section 7.

## Stakes Classification

**Level**: High

**Rationale**: This touches every module and changes the app's threading and rendering data flow. Mitigations: the work is staged so the build and test suite stay green after every step (strangler pattern: the engine is built alongside the old path, then the app is switched over, then the old path is deleted); the Phase 0 regression test and the existing e2e suite run unchanged as black-box guards; the riskiest steps (Slint 1.17 upgrade, tray replacement) are explicitly descopeable.

## Research

- `../reviews/2026-08-15-retrospective.md` sections 4.4 (parameter amplification, lifecycle coupling), 7 (target architecture), 8.1/8.2 (test layers, soak tests), 10 Phase 1, 11 questions 1-4
- `2026-03-25-slint-shutdown-research.md` and the `process::exit(0)` comment in `src/main.rs` (teardown hazard being removed)
- Slint 1.17 release notes and `SystemTrayIcon` docs (retrospective section 11, question 2)

## Key Design Decisions

**D1: The engine gets its own wgpu device on its own thread; the preview becomes pixel buffers.** Today the wgpu device is shared with Slint via `WGPUConfiguration::Manual` and the preview is a zero-copy `Image::try_from(Texture)`. Instead, the engine creates its own device (moving `wgpu_init`), renders offscreen exactly as today, and sends preview frames to the app as RGBA byte buffers; the app wraps them in `slint::Image::from_rgba8(SharedPixelBuffer)`. Consequences, all intended: Slint no longer needs the `unstable-wgpu-28` feature or the wgpu backend selector (its own UI renders with the default renderer); the wgpu 29 bump stops being coupled to the Slint version (we keep wgpu 28 in this phase); the thread-local `GPU_RESOURCES` and the TLS teardown ordering problem disappear structurally. Cost: one CPU copy per preview frame. At quantized preview sizes (about 2 MB per frame) this is well within budget even during mouse drags; the readback path (`read_texture_rgba8`) already exists and is tested.

**D2: The engine never sleeps on wall time for scheduling decisions.** The engine loop blocks on its command channel with a short real timeout, and on each wake computes due work (sun-position refresh, auto-refresh export, cloud poll) from `clock.now()`. Production wires `SystemClock`; tests wire a `MockClock` and drive determinism by advancing it and sending a `Poke` command. This is what makes the 14-simulated-days soak test possible.

**D3: Asset fetching goes behind a trait.** `CloudSource` (check freshness, fetch bytes) with an HTTP implementation extracted from today's `cloud_fetcher` and a fixture implementation for tests. Decode and mip generation run on a worker thread owned by the engine, never on the UI thread (this also fixes the section 4.2 UI stalls as a side effect). The Phase 0 mailbox and env knobs carry over; the soak test uses the fixture source directly, no HTTP.

**D4: One `SceneParams` struct.** Defined in core, it replaces the parameter quintuplication (Slint properties, `AppConfig`, `FrameState`, `ShadingParams`, `Uniforms` construction). Exactly two translation points remain: the Slint bridge in the app (window properties to `SceneParams` and back) and the uniform encoder in core (`SceneParams` to GPU bytes). Dirty checking compares quantized `SceneParams`.

**D5: Package name and binary name stay `sunlit-earth`.** The binary crate is `crates/sunlit-app` but keeps `name = "sunlit-earth"` in its manifest so `CARGO_BIN_EXE_sunlit-earth`, the e2e suite, CI, and the release workflow keep working.

## Success Criteria

- [x] Workspace builds; `cargo test` green at every step boundary (commit per step)
- [x] `render` subcommand produces a PNG with no window and no Slint backend involvement
- [x] Phase 0 regression test passes unchanged (black box), and the whole Phase 0 surface keeps working: all `SUNLIT_EARTH_*` env knobs, the `query-memory` IPC command, the memory metrics CSV, and the drain-while-hidden semantics (the Phase 0 mailbox and drain timer become engine-internal, but the observable behavior is identical)
- [x] Existing e2e suite passes (adapted only where paths or startup logs changed)
- [x] New engine integration tests pass headlessly on the software adapter
- [x] Mock-clock soak test: 14 simulated days of cloud updates and auto-refresh in under a minute, bounded private bytes
- [x] Golden-image test with tolerance, plus contact-sheet artifact job in CI
- [x] Quality tiers exist; dev/test default is low; release default unchanged in output quality
- [x] Slint 1.17: tray via `SystemTrayIcon`, `tray.rs` message pump deleted; teardown without `process::exit(0)` attempted and outcome documented
- [x] `cargo clippy` no new warnings (the pre-existing pedantic debt is untouched, see Results); CLAUDE.md rewritten for the new layout

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

**Files**: `CLAUDE.md`, `.github/workflows/ci.yml`, `../reviews/2026-08-15-retrospective.md`, `docs/roadmap.md`

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
- [x] Steps 3-4: SceneParams + engine
- [x] Step 5: app switched, old path deleted
- [x] Steps 6-7: tiers + new test layers
- [x] Step 8: Slint 1.17 + tray (or descoped with findings)
- [x] Step 9: docs + CI

## Deviations

Recorded as they happened, smallest change that kept the plan's intent.

1. **Step 4 split into two commits, and the old path was not kept in dual operation.** The plan
   allowed for this ("if dual operation is more work than it saves"). Moving the renderer to core
   while also keeping a working app-side renderer would have meant maintaining two copies of it.
   Instead 4a moved the renderer to core and left the app's rendering notifier driving it through
   the new API, so behavior was unchanged and the tree stayed green; 4b built the engine beside
   that; Step 5 switched the app over and deleted the notifier.

2. **`TextureMailbox` moved to core in Step 2, not Step 4.** The plan kept the renderer (and with
   it the mailbox) app-side until Step 4, but `cloud_fetcher` moved to core in Step 2 and needs the
   mailbox. It went along, into `assets::mailbox`, and the app re-exported it so call sites did not
   change.

3. **`build_aa_options` changed signature twice.** It returned `Vec<slint::SharedString>`, so it had
   to become `Vec<String>` to move to core (Step 4a), and gained a `max_samples` cap in Step 6 so
   the anti-aliasing combo box never offers a setting the quality tier would ignore.

4. **`scene::sun::compute_sun_direction_at` added.** The engine's injected clock has to reach the
   astronomy path, or a mock clock advancing fourteen days would leave the sun where it was.
   `compute_sun_direction` now delegates to it with the real clock, so existing callers are
   unaffected.

5. **`enforce_single_instance` became `acquire_single_instance` and no longer exits in place.** The
   e2e single-instance test started failing because the "another instance is already running" line
   was lost in the non-blocking writer's buffer when `process::exit(0)` ran immediately after it.
   The check moved into `main`, which returns instead, so the logging guard drops and flushes. It
   also now runs before the window and the GPU device exist, which is where it belonged anyway.

6. **Viewport resizes are polled every 200 ms rather than event-driven.** Slint exposes the preview
   size as an `out property` with no size-changed callback bindable from Rust here. The poll sends
   `SetPreviewSize` only when the value changes, and the engine quantizes it, so a drag produces a
   handful of commands.

7. **The preview stayed enabled while the window was hidden** (corrected in review). As first
   written the windowed app never sent `SetPreviewEnabled`, so the command was reachable only
   through the `preview_enabled` field of `EngineConfig`, which is what the `render` subcommand
   sets. That left a hidden High-tier window doing a full 4K readback and `SharedPixelBuffer` copy
   on every sun tick for nobody. The app now sends `SetPreviewEnabled(false)` on every hide (IPC,
   tray toggle, the close handler, and the deferred `--tray-start hidden`) and `(true)` on every
   show. The engine's owed-frame logic delivers a frame on re-show, and it had to be hardened
   first: the debt was being cleared even when there was no frame to pay it with, which is exactly
   the `--tray-start hidden` case.

8. **The quality tier does not yet skip the 8K textures.** The plan's low tier says "skip the 8K JXL
   textures unless explicitly selected", but the only Earth textures that exist are those 8K files,
   and the texture combo box *is* the explicit selection, so the rule would be a no-op. The tier
   caps MSAA, the preview size, and the cloud variant today; smaller Earth textures belong with the
   offline asset pipeline in retrospective section 9.

9. **The soak test's wall-clock bound is 120 seconds, not 60.** It runs in 12.5 seconds on the
   development desktop; the headroom is for the software adapter on CI, where 672 exports and 112
   JPEG decodes are considerably slower. The assertion exists to catch "the compression stopped
   working", not to benchmark.

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

### Step 3: SceneParams

`sunlit_core::params` now holds `SceneParams`, its quantized `ParamsDigest`, the four shell radii,
the gamma slider mapping, and `AppConfig` conversions in both directions. The 27-argument
`build_frame_state` is gone: `FrameState` is now the digest plus width, height, and the quantized
sun direction. `ShadingParams` is gone too: `write_uniforms` takes `&SceneParams` plus a small
`FrameInputs` (sun direction and the blend flag, neither of which is a scene parameter). The
overlay-selection logic that was copy-pasted between the preview pass and the wallpaper export is
now `Overlays::select`, so the two paths cannot drift.

The Slint bridge gained `read_params_from_window` and `apply_params_to_window`;
`read_config_from_window` and `apply_config_to_window` are now thin wrappers that add window
geometry and the auto-refresh fields. The `BeforeRendering` callback reads the window exactly once
into a `SceneParams` and derives everything from it, including the MSAA rebuild check.

The 40-odd positional `build_frame_state` dirty-check tests were rewritten as a single table-driven
test in `params.rs` that walks every shader parameter, so adding a knob without wiring the dirty
check now fails a test instead of producing stale frames.

Verification: `cargo test` 304 pass (196 core, 61 app, 19 render_pipeline, 12 shading, 16
slint_ui), `cargo clippy --all-targets` 21 warnings, all pre-existing kinds. Full e2e suite 8 pass.

### Step 4: the renderer moves to core, the engine appears

Landed as two commits.

**4a, the renderer.** `GpuResources` became `sunlit_core::renderer::Renderer` with methods instead
of free functions reaching into a thread-local. It renders into its own offscreen texture (now
carrying `COPY_SRC` as well as `TEXTURE_BINDING`, so both binding it and reading it back work) and
returns a `RenderOutcome` rather than a `slint::Image`. Background decode threads wake their
consumer through the same `NotifyFn` the cloud fetcher uses. The shaders and the two GPU
integration suites moved with it. The app kept a thin `renderer` module holding the Slint
rendering notifier and the thread-local, so nothing changed behaviorally at this point.

**4b, the engine.** `sunlit_core::engine` blocks on its command channel with a 50 ms timeout and
computes due work from `clock.elapsed()` on every wake (D2). Cloud fetching went behind
`CloudSource` (D3) with `HttpCloudSource` for production, and `CloudUpdater` owns the disk cache and
the decode. Wallpaper publishing went behind `WallpaperSink` so tests never repaint the desktop.
A dedicated cloud worker thread does network I/O and JPEG decoding and never touches the GPU; it
pokes the engine, which uploads on its own schedule.

Two design details worth recording:

- `Schedule::due` recomputes its deadline from `now` instead of accumulating. Without that, one
  jump of a simulated day would fire 17 280 catch-up drains rather than one. There is a test for it.
- Re-enabling the preview has to deliver a frame even though the scene did not change, so
  `PreviewState` tracks "owed" separately from "enabled". A window that was hidden and shown again
  would otherwise sit on a stale image until the user touched something.

Verification: `cargo test` 313 pass including nine new engine integration tests, clippy has no new
warnings, full e2e suite 8 pass.

### Step 5: the app switched to the engine, the old path deleted

`main.rs` no longer calls `wgpu_init` or `BackendSelector::require_wgpu_28`, and the `slint`
dependency lost the `unstable-wgpu-28` feature, which also let the app crate drop `wgpu`, `glam`,
`bytemuck`, `pollster`, `ureq`, `jxl-oxide`, `astronomy-engine-bindings`, `serde`, `toml`, `dirs`,
`image`, and `winreg` from its manifest. The app is now Slint, clap, tracing, the tray, IPC, and
`sunlit-core`.

Preview frames cross the thread boundary through a latest-value mailbox with a single pending
wake-up: the engine parks the newest frame and queues one `invoke_from_event_loop` closure, which
takes whatever is parked. Queueing 2 MB buffers is precisely the failure mode Phase 0 removed from
the texture path, so it is not reintroduced for frames.

The `render` subcommand is fully headless: no window, no Slint backend, no event loop. It starts
the engine with `preview_enabled: false`, waits for `TexturesReady`, calls `render_to_file`, and
returns an exit code.

The sun timer, drain timer, memory watchdog, and both copies of the auto-refresh timer body are
gone from `main.rs`; all four are engine schedules now. What remains in the app is the viewport
size poll (200 ms, sends `SetPreviewSize` only when the quantized size changes) and the one-shot
startup wallpaper refresh.

Two e2e adaptations were needed, both because behavior moved rather than changed:

- `first frame rendered` is now logged by the engine (the render subcommand has no window to
  render into), while `SIGNAL:first_frame_rendered` is still printed by the app when it displays
  its first preview frame.
- `enforce_single_instance` became `acquire_single_instance` and no longer calls `process::exit`
  itself. The check moved into `main`, before the window and the GPU device exist, and the
  "already running" path returns from `main` so the logging guard drops and flushes the message.
  Exiting in place lost it to the non-blocking writer's buffer.

**Teardown outcome: the hacks are gone.** `process::exit(0)` and all four `std::mem::forget` calls
were removed. `main` returns `ExitCode`, the engine is shut down with an ordinary thread join, and
the timers are dropped normally. The wgpu thread-local destruction panic does not reproduce,
which is the expected result of D1: Slint no longer holds any wgpu object, so there is no
cross-library destruction order to get wrong. Verified by running the three shutdown-sensitive e2e
tests four times in a row plus two full suite runs, all exit code 0 with no panic on stderr.

Verification: `cargo test` 332 pass, clippy no new warnings, full e2e suite 8 pass in 42 seconds
(including the Phase 0 regression test, unchanged).

### Step 6: quality tiers

`QualityTier` (low, medium, high) lives in `config` and is persisted with the rest of the settings,
overridable per run with `--quality`. (Persistence was in fact broken as first written and was fixed
in review; see Post-review fixes below.) It caps three things: the MSAA sample count (1, 4,
unlimited), the preview width (1280, 1920, unlimited, aspect ratio preserved), and which cloud
image variant is downloaded (2048x1024, 4096x2048, 8192x4096, all published upstream already). The
default is low in debug builds and high in release, and `EngineConfig::headless` pins low so tests
never depend on the build profile.

The sample-count cap is applied in two layers: the anti-aliasing combo box only offers counts the
tier allows, and since the post-review fixes the engine also resolves every requested count against
the adapter's supported list (see Post-review fixes below; this paragraph originally claimed the
combo box filter was the only layer, which left config-supplied counts unvalidated). The
cloud URL composes with the Phase 0 environment override, and the override wins: a test pointing
at a local stub is not second-guessed by the tier.

Verification: `cargo test` 344 pass, clippy no new warnings, full e2e suite 8 pass. The e2e render
test now runs at the low tier (debug build) and its pixel assertions still hold, which is the
evidence that dropping MSAA does not change the image where it matters.

### Step 7: the new test layers

**Engine integration tests** (`crates/sunlit-core/tests/engine.rs`, 10 tests) landed with the
engine in Step 4b and gained a texture-swap case here. They cover the first frame, dirty-check
suppression, resize quantization, preview toggling, render-to-file with and without a preview,
wallpaper publishing through a stub sink, and the textures-ready event.

**Mock-clock soak** (`crates/sunlit-core/tests/soak.rs`). 14 simulated days: 672 wallpaper exports
(one per simulated 30 minutes) and 112 cloud publications (one per simulated 3 hours) against a
fixture `CloudSource` and a `CountingSink`, with the preview disabled, which is the hidden-window
scenario the old architecture stopped servicing. Result on the development desktop:

```
14 simulated days in 12.5s: 672 exports, 113 cloud fetches (of 112 publications)
private bytes: startup 304.0 MiB, after warm-up 388.5 MiB (+84.4),
               end 388.4 MiB (+0.0 over 12 simulated days)
```

The samples printed every 84 steps are flat to within 1 MiB from step 84 to step 672, so the
baseline is taken after warm-up and the assertion is on the remaining 12 simulated days (limit
16 MiB, two decoded frames). Warm-up itself, about 84 MiB, is the first cloud texture, its mip
chain, and wgpu's allocator pools, and gets its own generous ceiling so a gross regression there
is still caught. For scale: the unfixed Phase 0 behavior would have parked one decoded 2048x1024
frame per update, about 900 MiB across this run.

The soak test found one real defect: a cloud poll skipped because the worker was still busy used
to wait out a whole interval before retrying. It now retries on the next tick.

**Golden images** (`crates/sunlit-core/tests/golden.rs`), four cases at 512x256 with the
procedural grid texture and the software adapter forced, so a developer machine with a discrete
GPU and a CI runner with WARP compare against the same references. Tolerance is a mean channel
difference under 2/255 plus at most 1% of pixels differing by more than 24. Regenerate with
`SUNLIT_EARTH_UPDATE_GOLDEN=1`. The four references total 284 KB.

A fifth test asserts that every pair of references is distinguishable under that tolerance, which
is what keeps the others from being vacuous. It immediately earned its place: the first attempt at
a night-side case compared equal to the daytime one, because the grid texture renders in
single-texture mode where the shader takes the `terminator_width < 0` path and ignores the sun
entirely. Only the atmosphere shells are sun-dependent without real textures, so the cases became
default, strong Rayleigh, strong nightglow at midnight, and a close-up. The base camera also had
to be zoomed in: at the default zoom the globe is small enough that limb effects land on a handful
of pixels and no tolerance can separate them from noise. Day/night shading itself stays covered by
`tests/shading.rs` and `tests/render_pipeline.rs`, which run the blend function on the GPU
directly.

**Contact sheet**: all nine camera presets rendered into one PNG at `target/contact-sheet.png`,
uploaded by CI as an artifact. It asserts nothing; it exists so a human can glance at a shading
change.

Verification: `cargo test` 352 pass, clippy no new warnings.

### Step 8: Slint 1.17 and the tray

Not descoped. The upgrade to Slint 1.17.1 needed no source changes at all: with the wgpu feature
already gone in Step 5, the app only touches the stable window, model, and image APIs. The testing
backend pin moved to `=1.17.1`.

`SystemTrayIcon` replaced `tray.rs`'s Win32 message pump. The icon, its menu (Open, Refresh Now,
checkable Auto-refresh, Exit) and the `clicked()` toggle are declared in `ui/main.slint` as a
component inheriting `SystemTrayIcon`; `tray.rs` shrank from 306 lines to 141 and now holds only
the procedurally generated icon, the callback wiring, and the single-instance mutex. Deleted with
the pump: the Win32 `GetMessageW` loop, the tray thread, the `crossbeam` sync channel and its
`OnceLock`, the mirrored `AtomicBool` for the checkmark, and the `tray-icon` dependency. The app's
`windows-sys` features are down to `Win32_System_Console` for `AttachConsole`.

Two API details worth knowing for anyone touching this again:

- Only properties and callbacks *declared* on the derived component are exposed to Rust;
  the inherited `icon`, `tooltip`, `title`, `visible`, and `clicked` are not. The icon is fed
  through a declared `tray-image` property that the inherited `icon` binds to.
- A `SystemTrayIcon`-rooted component implements `StrongHandle` but not `ComponentHandle`, so
  there is no `as_weak()`. The handle is kept in an `Rc` and cloned into the auto-refresh callback.

Verification: `cargo test` 352 pass, clippy no new warnings, full e2e suite 8 pass. The tray-mode
e2e tests exercise the new code path (they run `--mode tray`, which now constructs and shows the
Slint tray) but drive the app over IPC rather than through the tray menu, so whether the icon
renders correctly and the menu entries look right still needs a human eye.

### Step 9: docs and CI

CLAUDE.md rewritten for the workspace: layout, the engine and its injected clock, source, and sink,
the two `SceneParams` translation points, the renderer API, the app modules, quality tiers, the
shader entry points and their real draw order (the old file had clouds drawn last; they are drawn
first, before the atmosphere shells), a table of every `SUNLIT_EARTH_*` knob, the test layers with
their commands, and the resource-flow rules from retrospective section 8.2.

CI already ran `cargo test --locked` from the workspace root, which covers both crates including
the engine, soak, and golden suites; the added step uploads `target/contact-sheet.png` as an
artifact. `release.yml` needed no change: the package is still named `sunlit-earth`, so
`target/release/sunlit-earth.exe` is still where the zip step looks. `Cargo.lock` stays at the
workspace root and `--locked` keeps working.

Roadmap: "non-blocking texture loading" checked off (decode and mip generation are on
engine-owned threads now), "system tray extended features" checked off apart from a true daemon
mode, and the memory-budget entry now points at the quality tiers for the part they cover.
Retrospective: Phase 1 marked implemented, and the open question about the `process::exit(0)`
workaround answered.

Two pieces of cleanup landed here as well. `cloud_fetcher::spawn_cloud_fetcher`, the
standalone-thread wrapper, became dead once the engine owned the cloud worker and was deleted
along with `Renderer::device`, `queue`, `sample_count`, and `EngineLink::push`. Deleting it
exposed a regression: the old fetcher retried a failed download with exponential backoff from 15
seconds up to 5 minutes, while the engine's worker just waited out the next poll, which in
production is an hour. The worker now carries that backoff itself and bails out if the engine
disconnects while it is sleeping.

`cargo build --release --locked` was run end to end (4m20s, 27 MB binary at
`target/release/sunlit-earth.exe`) to confirm the release workflow's path assumption still holds.

### Post-review fixes

A validation pass found three major issues and six minor ones. All were fixed on this branch.

**Sample counts were never validated against the adapter.** The quality tier capped them, but the
High tier (the release default) caps nothing, so a config asking for an unsupported count reached
`create_render_textures` and killed the engine thread with a wgpu validation error. The symptom was
the nastiest kind: the window came up, IPC answered, quit exited 0, and no frame ever appeared.
`renderer::resolve_sample_count` now takes the highest supported count at most the requested one,
and the engine applies it in `Engine::new` and on every `UpdateParams`, warning once per distinct
request. The engine is the single source of truth here, because a bare number in `SceneParams` can
come from a config file or a combo box index saved on a machine with a different GPU, neither of
which passes through the UI's option list. Two engine tests cover it, deliberately at the High tier:
at the Low tier the cap masks the bug and the tests prove nothing. Verified red (with the reported
validation error) against a stubbed-out resolver.

A related gap: an engine thread that died was silent. `EngineHandle::send` now logs at error level
when the channel is dead, and the join reports the panic payload.

**The quality tier was not persisted.** `read_config_from_window` built the saved config from
`AppConfig::default()`, so every save overwrote `quality_tier` with the build default: a debug-build
save permanently downgraded a release install to low. Saves are now a read-modify-write against the
stored config. Enumerating fields to preserve would rot on the next non-UI setting, so the
UI-managed fields are written onto what is on disk instead. A side effect that is desirable:
`--quality` stays a per-run flag rather than being written back.

**The engine command channel was unbounded** while the docs claimed every crossing was a mailbox.
Kept unbounded, with the argument now attached to the declaration: an unconditional consumer that
drains every 50 ms, human-rate producers, and payloads that carry no pixels (pinned by a test on
`size_of::<EngineCommand>()`). A bound was considered and rejected in writing: a blocking send from
the UI thread deadlocks against a mid-export engine, and a `try_send` that drops `UpdateParams` can
drop the last one and leave the window and the engine permanently disagreeing. CLAUDE.md now names
all three crossings honestly.

The minor fixes: a test for the cloud worker's failure path (backoff extracted into `RetryBackoff`
so its schedule is testable without sleeping through it, plus a scripted source that fails on
demand); the cloud schedule no longer drags its deadline back and forth while the worker is busy;
`WgpuContext` lost its dead `instance` and `adapter` fields and two stale doc comments were
corrected; `SetPreviewEnabled` is now actually wired to window visibility, which required hardening
the owed-frame logic first (it cleared the debt even when there was no frame to pay it with, the
`--tray-start hidden` case exactly); `EngineConfig::headless` forces the software adapter so tests
behave the same locally as on CI; and `PRIVATE_BYTES_BUDGET` went from 2 GiB to 3 GiB, because the
observed 2.43 GiB startup peak at the High tier made the old budget warn about normal operation. A
tier-aware budget belongs with the future texture-tier work.

Cost of moving the tests onto the software adapter, measured on the development desktop: the engine
suite goes from 9 s to 16 s, and the soak from 12.5 s to 50 s against its 120 s bound. That is a
2.4x margin, thin enough that a much slower CI runner could make it flaky; the note in the test says
the lever is halving the export cadence rather than raising the bound.

### Final state

`cargo test`: 371 tests pass (264 core unit, 14 engine, 6 golden, 19 render_pipeline, 12 shading,
1 soak, 38 app unit, 17 slint_ui), plus 8 desktop e2e tests behind `--ignored`.

Post-review verification, all on the development desktop: full suite green; `cargo clippy
--all-targets` 21 warnings, unchanged from the pre-Phase-1 baseline; full e2e suite 8 pass in 42 s
including the hide and show tests that now exercise `SetPreviewEnabled`; soak run twice on the
software adapter at 49.8 s and 49.6 s with 1.6 MiB and 1.1 MiB of growth after warm-up; golden
suite 6 pass with every pair of references still distinguishable.
`cargo clippy --all-targets`: 21 warnings, every one of them a pre-existing pedantic lint that was
already present before this work (`manual RangeInclusive::contains` in the scene math, field
assignment after `Default::default()` in the config tests, two long functions in
`render_pipeline.rs`, and three casts). No new warnings were introduced at any step, including the
review fixes.
