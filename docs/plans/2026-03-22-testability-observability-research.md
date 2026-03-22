# Research: Testability and Observability (2026-03-22)

## Problem Statement

Sunlit Earth is a Windows desktop app that renders a 3D Earth via wgpu and displays it in a Slint window. The project has solid unit test coverage for pure functions and GPU shader integration tests, but lacks three capabilities: (1) automated UI/integration tests that exercise the Slint UI layer, (2) structured logging and observability to replace ad-hoc `eprintln!` calls, and (3) a clear path to close testability gaps in the rendering callback glue code that connects UI state to GPU dispatch.

This document consolidates findings from three parallel research efforts into a unified set of recommendations.

## Requirements

1. **Structured logging** with level-based filtering, timestamps, and module context to replace all 32 `eprintln!` calls across the codebase.
2. **Render-thread safety** -- logging must not block the rendering callback with synchronous I/O.
3. **Compile-time log elimination** in release builds to avoid runtime overhead from verbose debug/trace output (especially from wgpu internals).
4. **UI logic testing** -- verify Slint property bindings, callback wiring, reset behavior, and config round-trips without requiring a GPU or display.
5. **Visual regression capability** -- detect unintended rendering changes in CI using the software adapter (WARP on Windows).
6. **Testability of glue code** -- the rendering callback reads Slint UI properties and feeds them to GPU dispatch; this path is currently untestable.
7. **Performance instrumentation** -- timing for key operations (frame render, texture decode, cloud fetch) without custom stopwatch code.
8. **Compatibility with existing constraints** -- `unsafe_code = "deny"`, Slint `~1.15` pinning, Windows-only CI on GitHub Actions free runners with no hardware GPU.

## Findings

### Current Test Infrastructure

The project has approximately 120+ tests across unit test modules and integration test files. Pure functions are well-covered (90-100%) using `approx::assert_relative_eq!` for float comparisons and `proptest` for property-based testing. GPU shader integration tests in `tests/shading.rs` run the actual `blend_fragment()` WGSL on the GPU via a compute shader harness with 18,000+ parameter combinations per run. Full render pipeline tests in `tests/render_pipeline.rs` render to small offscreen textures and assert behavioral invariants (brightness comparisons, center-vs-edge patterns) rather than pixel-exact values.

The shared GPU test infrastructure in `tests/common/mod.rs` uses `LazyLock<Mutex<GpuContext>>` to share a single device across parallel test threads, since per-test device creation crashes on Windows. A software adapter fallback ensures tests pass in CI without a hardware GPU.

Compile-time size assertions (`const _: () = assert!(size_of::<Uniforms>() == 160)`) catch uniform buffer layout mismatches between Rust and WGSL. The `shading.rs` test uses `include_str!` to include the production shader source, preventing shader code drift.

**Key strength:** The existing behavioral invariant testing approach (asserting monotonicity, bounds, and visibility rather than pixel-exact values) is the most robust strategy for cross-hardware GPU testing and should be the primary approach going forward.

### Testability Gaps and Barriers

Three structural barriers limit test coverage:

**1. Thread-local `GpuResources`:** All rendering state lives in a `thread_local! { RefCell<Option<GpuResources>> }` that is only populated during Slint's `RenderingSetup` callback. No unit test can access `GpuResources` without going through the Slint event loop. Functions like `export_wallpaper_image()` access this thread-local directly, making them untestable outside the rendering context. The integration tests in `tests/render_pipeline.rs` work around this by creating their own parallel rendering context.

**2. Rendering callback coupling:** The `rendering_callback()` function reads UI properties via `window_weak.upgrade()`, requiring a live `MainWindow`. While pure logic has been partially extracted (the humble object pattern is already partially applied via `build_frame_state`, `quantize_to_granularity`, `build_aa_options`), the callback still directly reads Slint window properties for frame rendering.

**3. `main.rs` untestability:** The `main()` function (431 lines) blocks on `window.run()`, calls `std::process::exit(0)` at the end, and initializes the Slint backend (which can only happen once per process). These factors make it impossible to call `main()` from a test. However, several helper functions (`apply_config_to_window`, `read_config_from_window`, `update_datetime_labels`) are already extracted and testable.

**Recommended extraction to close gaps:**

- `collect_ui_state(win: &MainWindow, aa_counts: &[u32]) -> RenderParams` -- gathers all window properties into a plain struct, decoupling UI reads from GPU dispatch. Testable with the Slint testing backend.
- `wrap_longitude(lon: f32) -> f32` -- the modular arithmetic currently inline in mouse drag handling.
- `rotate_drag(dx, dy, tilt_rad) -> (f32, f32)` -- tilt-corrected mouse rotation math.
- `drag_sensitivity(zoom: f32) -> f32` -- zoom-dependent drag scaling.

### Logging and Observability

#### Current state: no logging framework

The project has no logging framework. All diagnostic output goes through 32 `eprintln!` calls distributed across `cloud_fetcher.rs` (15), `config.rs` (7), `renderer/textures.rs` (2), `renderer/mod.rs` (1), and `wgpu_init.rs` (2). There is no structured logging, no level filtering, no timestamps, no way to enable/disable verbose output at runtime, and no way for tests to capture and assert on log output.

Error propagation uses three patterns: `Result<T, String>` with early return, `eprintln!` + fallback to default (graceful degradation), and `expect()` / `assert!()` for invariant violations. All error types are `String`, losing error classification and chaining.

#### Recommended stack: `tracing` ecosystem

The Rust ecosystem has converged on `tracing` as the recommended foundation for new applications. It is a strict superset of `log`: it records structured, span-aware events with timing context. The recommended stack:

| Crate | Purpose |
| --- | --- |
| `tracing` | Instrumentation macros, `#[instrument]`, span API |
| `tracing-subscriber` | Composable layer-based subscriber with `EnvFilter` |
| `tracing-appender` | Non-blocking, off-thread file appender with daily rotation |
| `tracing-log` | Bridge to capture wgpu's `log::` calls into the tracing pipeline |
| `log` | Required as a dependency for wgpu integration; configure compile-time level caps |

**Compile-time elimination** is critical. Both `tracing` and `log` support Cargo feature flags that compile out instrumentation below a given level:

```toml
tracing = { version = "0.1", features = ["max_level_debug", "release_max_level_warn"] }
log = { version = "0.4", features = ["max_level_debug", "release_max_level_warn"] }
```

This is especially important because wgpu emits enormous volumes of trace-level output (documented in Bevy issue #14116) that is expensive even just to filter at runtime.

**Render-thread safety:** `tracing-appender`'s `NonBlocking` writer sends log records over an in-process channel to a dedicated background thread. The render thread pays only the cost of a channel send (~100-500ns). Synchronous file writes never block the rendering callback. The `WorkerGuard` returned by `non_blocking()` must be stored for the full process lifetime; dropping it loses buffered log lines.

**Runtime filtering** via `RUST_LOG` environment variable with `EnvFilter`:

```text
RUST_LOG=warn,wgpu_core=warn,wgpu_hal=error,sunlit_earth=debug
```

**Performance instrumentation** comes free with `FmtSpan::CLOSE`: when enabled, the fmt layer emits a synthesized event when a span closes, including `busy` (time executing) and `idle` (time waiting) durations. Combined with `#[tracing::instrument]` on key functions, this provides per-function timing without manual `Instant::now()` + `elapsed()` code.

**Dual output** (stderr for development + rotating file for diagnostics) is supported via layer composition with independent per-layer filters.

#### `eprintln!` migration mapping

The 32 existing `eprintln!` calls should be converted to appropriate levels:

- `error!` -- unexpected failures (wrong graphics API, buffer mapping failure)
- `warn!` -- recoverable errors (config parse failure, cache I/O failure, adapter fallback)
- `info!` -- operational events (texture decoded, cloud image downloaded, cloud unchanged)
- `debug!` -- detailed operational data (download timing, texture dimensions)

#### Why `tracing` over `log`

`log` is simpler but lacks spans, structured fields, and timing context. Sunlit Earth's background thread architecture (cloud fetcher polling every 60 minutes, texture decoder threads, rendering callback) benefits from `tracing`'s span propagation for correlating events across threads. The overhead when no subscriber is installed is a single atomic load. When spans are compiled out via static level features, they add zero binary size.

`env_logger` is the standard backend for `log`-only projects. For `tracing`-based projects, `tracing-subscriber` is the equivalent and no more complex to set up.

### UI Testing with Slint Testing Backend

#### Capabilities

Slint provides `i-slint-backend-testing`, an internal crate that simulates a windowing system without rendering pixels. Three initialization modes are available:

- `init_no_event_loop()` -- property access only, mocked time
- `init_integration_test_with_system_time()` -- event loop with real clock
- `init_integration_test_with_mock_time()` -- event loop with deterministic time advance

The `ElementHandle` API allows locating elements by accessible label or element ID, simulating clicks, and triggering accessible actions.

#### What it can test

Without pixel rendering, Slint UI tests verify logic:

- Property bindings: setting `camera-longitude` updates the slider, and vice versa
- Callback wiring: slider movement triggers `sliders_changed`
- Reset All: restores all properties to defaults
- Config round-trip: `apply_config_to_window()` then `read_config_from_window()` preserves values
- ComboBox model population: AA and texture options appear correctly
- Conditional visibility: Date/Time controls appear only when `use-custom-datetime` is checked

#### What it cannot test

The testing backend produces no pixels. It cannot validate wgpu rendering output. Since Sunlit Earth's rendering happens entirely in wgpu (not in Slint's renderer), the Slint testing backend is limited to the UI layer in isolation.

#### Constraints

- **One initialization per process:** The Slint backend can only be initialized once. Slint UI tests must go in a dedicated test binary (e.g., `tests/slint_ui.rs`), separate from GPU integration tests.
- **Internal crate:** `i-slint-backend-testing` has no semver guarantees. It must be pinned to an exact version matching the `slint` crate (`~1.15`).
- **Open question:** Whether the Slint testing backend and a wgpu render pipeline can coexist in the same process has not been verified. They may require separate test binaries.

### Visual Regression and Snapshot Testing

#### Headless wgpu rendering (already implemented)

The project's offscreen render-to-texture pipeline is exactly what is needed for headless visual testing. No window, surface, or display is required. The existing `read_texture_rgba8` helper performs GPU-to-CPU pixel readback. The wallpaper export pipeline already demonstrates staging buffer readback. The software adapter (WARP on Windows) produces deterministic output for a given wgpu version, making it the most stable baseline for CI snapshot tests.

#### Perceptual similarity comparison

Exact pixel comparison across machines fails due to floating-point variance across GPU drivers/hardware. Perceptual similarity metrics tolerate sub-pixel differences while catching meaningful regressions:

| Crate | Metric | Score Direction | Confidence |
| --- | --- | --- | --- |
| `image-compare` | MSSIM (8x8 windows) | 0.0 (dissimilar) to 1.0 (identical) | High |
| `dssim` / `dssim-core` | Multiscale SSIM | 0.0 (identical), unbounded positive (dissimilar) | High |
| `twenty-twenty` | SSIM with overwrite mode | Configurable minimum | Medium |
| `honeydiff` | CIEDE2000 color difference | Configurable delta-E threshold | Low |
| `img_hash` | Perceptual hash (aHash/dHash/pHash) | Hamming distance | Medium |

`image-compare` is the most practical choice: actively maintained, CPU-based (multi-threaded via rayon), returns per-pixel difference maps for debugging, and a pattern like `assert!(result.score >= 0.97)` is straightforward to integrate.

#### Behavioral invariant testing (preferred for CI)

Property-based visual assertions -- the approach already used in `tests/render_pipeline.rs` -- are more robust than snapshot comparison for CI. They avoid per-platform snapshot management entirely. Examples of visual invariants:

- The sphere occupies pixels at the center of the frame
- Day-side pixels are brighter than night-side pixels
- If a texture is loaded, the globe is not uniformly grey
- If cloud opacity > 0, cloud pixels are brighter than the same region without clouds
- All pixels have full alpha opacity

This approach is recommended by the wgpu team (Discussion #1611) for GPU tests that run across heterogeneous hardware.

#### Reference image workflow (for snapshot tests)

1. Generate reference images on the WARP software adapter for reproducibility
2. Store references in `tests/snapshots/warp/`
3. On CI, render to texture, read back pixels, compare with MSSIM; fail if score < threshold
4. Provide an `UPDATE_SNAPSHOTS=true` env var mode to regenerate references
5. Use MSAA=1 in test renders to eliminate sample-placement variance

#### Approaches not recommended for this project

- **Pixel Eagle** (Bevy's hosted visual regression service) -- operational overhead outweighs benefit for a small project. Requires real GPU hardware on CI runners.
- **Full binary launch tests** via `CARGO_BIN_EXE_<name>` -- the app opens a GUI window, runs an event loop, and has no headless mode. The cost of adding `--headless` + IPC for pixel extraction is not justified when the renderer can be tested directly through the library crate.

### Performance and Memory Instrumentation

**Frame timing:** `#[tracing::instrument]` on the rendering callback + `FmtSpan::CLOSE` provides per-frame `busy` / `idle` duration automatically, replacing manual `Instant::now()` + `elapsed()` patterns.

**Texture load timing:** The two existing `Instant::now()` + `elapsed()` calls in texture loading can be replaced by `#[tracing::instrument]` spans with structured fields (path, dimensions, elapsed).

**GPU timing:** wgpu's `TIMESTAMP_QUERY` feature enables GPU-side timing via `RenderPass::write_timestamp()`, but this is a GPU API concern separate from logging. CPU-side wall time around `queue.submit()` is the simpler option.

**Memory tracking:** Three options at increasing complexity:

1. Process-level RSS via `GetProcessMemoryInfo` (Win32) -- zero new crates, since `windows-sys` is already a dependency
2. `stats_alloc` custom global allocator wrapper -- tracks Rust heap allocations only (not GPU memory)
3. External tools (Task Manager, Windows Performance Toolkit) -- zero code changes

Option 1 is the most pragmatic for the project's current stage. GPU memory monitoring would require DXGI adapter queries.

## External Research

| Finding | Source | Confidence |
| --- | --- | --- |
| `tracing` is the recommended logging framework for new Rust projects (2025/2026) | [LogRocket blog (Oct 2025)](https://blog.logrocket.com/comparing-logging-tracing-rust/), [Shuttle blog](https://www.shuttle.dev/blog/2023/09/20/logging-in-rust) | High |
| `tracing-appender` `NonBlocking` uses a channel + background thread for async writes | [docs.rs/tracing-appender](https://docs.rs/tracing-appender/latest/tracing_appender/) | High |
| wgpu emits expensive trace-level log output; compile-time elimination recommended | [Bevy issue #14116](https://github.com/bevyengine/bevy/issues/14116) | High |
| Slint testing backend simulates windowing without pixels | [docs.rs/i-slint-backend-testing](https://docs.rs/i-slint-backend-testing), [Slint testing.md](https://github.com/slint-ui/slint/blob/master/docs/testing.md) | High |
| WARP software adapter produces deterministic output per wgpu version | [wgpu blog posts](https://gfx-rs.github.io/2021/07/16/release-0.9-future.html), [existing project CI](tests/common/mod.rs) | High |
| `image-compare` provides MSSIM with per-pixel diff maps | [docs.rs/image-compare](https://docs.rs/image-compare/latest/image_compare/), [GitHub](https://github.com/ChrisRega/image-compare) | High |
| Property-based visual assertions recommended by wgpu team for CI | [wgpu Discussion #1611](https://github.com/gfx-rs/wgpu/discussions/1611) | High |
| Bevy uses Pixel Eagle with hardware GPU CI runners | [DeepWiki bevy CI docs](https://deepwiki.com/bevyengine/bevy/9-development-and-ci) | High |
| `tracing-timing` HDR histograms (maintenance status unclear) | [docs.rs/tracing-timing](https://docs.rs/tracing-timing) | Medium-Low |
| `twenty-twenty` SSIM crate (limited recent activity) | [docs.rs/twenty-twenty](https://docs.rs/twenty-twenty/latest/twenty_twenty/) | Medium |

## Technical Constraints

1. **`unsafe_code = "deny"`** -- scoped `#[allow(unsafe_code)]` only on FFI call sites. New code must not introduce unsafe usage. Memory tracking via `GetProcessMemoryInfo` would require a new scoped allow.

2. **Slint `~1.15` pinning** -- the `i-slint-backend-testing` version must exactly match. Use `=x.y.z` version spec in `Cargo.toml`.

3. **One-init-per-process (Slint backend)** -- the Slint backend can only be initialized once. Slint UI tests and GPU integration tests must use separate test binaries.

4. **Thread-local GPU resources** -- `GPU_RESOURCES` is inaccessible from tests without the Slint rendering notifier active. Integration tests must create their own parallel GPU context.

5. **`std::process::exit(0)` at end of `main()`** -- would kill the test process if `main()` were called directly.

6. **Windows-only CI** -- GitHub Actions free runners have no hardware GPU. All GPU tests must pass on the WARP software adapter.

7. **LF line endings globally** -- must be maintained in all new/modified files.

8. **`tracing` + `log` independence** -- compile-time level features for `tracing` and `log` are independent. Both must be configured to cap verbose output in release builds.

9. **`WorkerGuard` lifetime** -- `tracing-appender`'s `NonBlocking` writer requires the guard to survive for the full process lifetime. Dropping it early loses buffered log lines.

10. **WARP rendering differences** -- WARP output is deterministic for a given wgpu version but may differ from hardware GPU output. Snapshot reference images must be generated on the same adapter used in CI.

## Open Questions

1. **Slint + wgpu coexistence in tests:** Can the Slint testing backend and a wgpu render pipeline (using the software adapter) be initialized in the same process? If not, Slint UI tests and render pipeline tests need separate test binaries. This has not been verified.

2. **Exact `i-slint-backend-testing` version:** The exact version to pin for Slint `~1.15` needs confirmation from Slint's release notes or `Cargo.lock`.

3. **MSSIM threshold calibration:** The exact MSSIM threshold that distinguishes acceptable hardware variance from real regressions has not been empirically measured for this renderer. A threshold of 0.95--0.99 is typical in the literature but would require tuning against WARP output.

4. **Error type strategy:** Should the project adopt `thiserror` for typed errors, or define manual error enums? The current `String` pattern is adequate for the project's size but loses error classification. This is a lower priority than logging.

5. **`tracing-subscriber` automatic `tracing-log` initialization:** Documentation suggests `tracing-subscriber` may handle `LogTracer::init()` automatically in some configurations. Verify in practice to avoid double-initialization.

6. **Coverage CI:** Should `cargo-llvm-cov` coverage reporting be added to the CI workflow? The commands are documented in CLAUDE.md but not wired into CI.

7. **Test binary organization:** Adding `tests/slint_ui.rs` creates a fourth test binary alongside `shading.rs`, `render_pipeline.rs`, and `common/mod.rs`. Is this the right structure, or should Slint UI tests go in a `tests/ui/` directory?

8. **WARP known issues with current shaders:** WARP has documented rendering bugs in older wgpu versions (e.g., issue #2503). The current wgpu 28.x behavior on WARP was not verified for the specific shaders used in Sunlit Earth.

## Recommendations

### Priority 1: Logging Foundation (Low Risk, High Value)

Add the `tracing` ecosystem to `Cargo.toml` and write an `init_logging()` function in `main.rs`. Convert all 32 `eprintln!` calls to appropriate log levels (`error!`, `warn!`, `info!`, `debug!`). Configure compile-time level caps for both `tracing` and `log` to eliminate noise in release builds.

This is a mechanical change that touches many modules but carries low risk. It immediately provides structured, filterable, timestamped logging with zero render-thread I/O overhead via `NonBlocking`. Tests can then capture log output for assertions.

**Dependencies to add:**

```toml
[dependencies]
tracing = { version = "0.1", features = ["max_level_debug", "release_max_level_warn"] }
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt"] }
tracing-appender = "0.2"
tracing-log = "0.2"
log = { version = "0.4", features = ["max_level_debug", "release_max_level_warn"] }
```

### Priority 2: Extract Pure Functions from `main.rs` (Low Risk, Immediate Testability)

Extract `wrap_longitude`, `rotate_drag`, and `drag_sensitivity` into testable functions in `lib.rs` or a new `src/app.rs` module. These are small, pure functions with non-trivial math that can be unit tested immediately without any infrastructure changes.

### Priority 3: UI State Collection Function (Medium Risk, Enables UI Testing)

Extract `collect_ui_state(win: &MainWindow, aa_counts: &[u32]) -> RenderParams` from the rendering callback. This creates a clean seam between UI property reads and GPU dispatch. With this function in place, the Slint testing backend can test the full path from UI property values to render parameters without needing a GPU.

### Priority 4: Slint UI Testing (Medium Risk, New Dependency)

Add `i-slint-backend-testing` as a dev-dependency. Create `tests/slint_ui.rs` as a dedicated test binary. Test `apply_config_to_window` / `read_config_from_window` round-trip, callback invocation, reset behavior, and conditional visibility. This requires resolving the open question about Slint testing backend + wgpu coexistence first.

### Priority 5: Expand Render Pipeline Invariant Tests (Low Risk, Incremental)

Add more behavioral invariant tests to `tests/render_pipeline.rs` covering color correction parameters (gamma, saturation), cloud layer blending, and fresnel shading. These are fully portable across hardware and extend the existing, proven testing pattern.

### Priority 6: Snapshot Testing with MSSIM (Medium Risk, New Infrastructure)

Add `image-compare` as a dev-dependency. Pick a handful of high-value rendering configurations (day-only, night-only, terminator visible), generate WARP reference images, and fail CI if MSSIM score drops below a tuned threshold. Store references in `tests/snapshots/warp/`. This provides a safety net for visual regressions that behavioral invariants might miss, but requires threshold calibration and reference image maintenance.

### Deferred: Error Type Reform

Converting `Result<T, String>` to typed error enums (via `thiserror` or manual definitions) would improve error classification and chaining, but the current pattern works adequately and the logging migration provides more immediate value. Revisit after logging is in place.

### Deferred: Memory and GPU Timing Instrumentation

Process-level RSS via `GetProcessMemoryInfo` and GPU timestamp queries are low priority until specific performance or memory concerns arise. The `tracing` `FmtSpan::CLOSE` timing provides sufficient frame-level instrumentation for now.

### Not Recommended

- **Full binary launch tests** -- the app has no headless mode, and adding one plus IPC for pixel extraction is high cost for marginal benefit when the renderer is testable through the library crate.
- **Pixel Eagle or hosted visual regression services** -- operational overhead outweighs benefit for a small project.
- **`tracing-timing` HDR histograms** -- unclear maintenance status. `FmtSpan::CLOSE` provides per-span timing with zero extra crates.
- **Windows ETW / OutputDebugString** -- heavyweight for a desktop app. stderr + rotating file is sufficient.

## Cross-Cutting Themes

Three themes emerged across all three research areas:

**1. The humble object pattern is the unifying testability strategy.** The codebase has already partially applied this pattern by extracting `build_frame_state`, `quantize_to_granularity`, and `build_aa_options` from the rendering callback. The remaining gap is the UI property reads. Extracting `collect_ui_state()` into a pure struct would simultaneously enable Slint UI testing (Priority 4) and make the rendering callback fully decomposed into testable pieces. The same pattern applies to `main.rs`: extracting `wrap_longitude`, `rotate_drag`, and `drag_sensitivity` makes the mouse interaction math testable without a window.

**2. Behavioral invariants over pixel-exact comparison.** This principle appears in three contexts: the existing GPU shader tests assert monotonicity and bounds, the UI testing research confirms that the Slint testing backend cannot produce pixels (so visual assertions are impossible for UI tests), and the visual regression research concludes that MSSIM thresholds are supplementary to -- not a replacement for -- property-based assertions. The project should continue prioritizing invariant tests and use snapshot comparison only as a secondary safety net.

**3. Structured logging enables both observability and testability.** Adding `tracing` is not just an operational improvement. Structured log output can be captured in tests (via `tracing-subscriber`'s `MockSubscriber` or by installing a test-specific layer), enabling assertions on operational behavior ("when the cloud fetcher gets a 304, it logs a cache hit at info level"). This bridges the observability and testability goals into a single infrastructure investment.

## Sources

| Document | Focus Area |
| --- | --- |
| `docs/plans/2026-03-22-testability-observability-codebase.md` | Current test infrastructure, rendering pipeline testability, `main.rs` decomposition, error handling patterns |
| `docs/plans/2026-03-22-testability-observability-ui-testing.md` | Slint testing backend, wgpu headless rendering, visual regression crates, snapshot testing workflows, full binary launch tests |
| `docs/plans/2026-03-22-testability-observability-logging.md` | `tracing` ecosystem, compile-time filtering, render-thread I/O safety, wgpu log integration, performance timing, memory tracking |
