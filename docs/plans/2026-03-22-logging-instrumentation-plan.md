# Plan: Structured Logging and Performance Instrumentation (2026-03-22)

## Summary

Add the `tracing` ecosystem to Sunlit Earth, migrate all 32 `eprintln!` calls to structured log macros at appropriate levels, instrument hot code paths (texture loading, rendering, cloud fetching, startup) with spans and timing, add process-level memory tracking via `GetProcessMemoryInfo`, and configure conditional compilation so debug builds emit verbose output while release builds compile out everything below `warn`.

## Stakes Classification

**Level**: Medium
**Rationale**: This change touches every module that currently uses `eprintln!` (6 files) plus adds new code in `main.rs` and a new `src/memory.rs` module. The risk is moderate: the migration is mostly mechanical (find `eprintln!`, replace with `tracing` macro), and the new dependencies (`tracing`, `tracing-subscriber`, `tracing-appender`, `tracing-log`) are mature and widely used. The primary risk is getting the subscriber initialization order wrong (must happen before any log macro fires) or dropping the `WorkerGuard` too early (losing buffered log lines). No shader code, GPU pipeline, or UI layout changes are involved.

## Context

**Research**: [`docs/plans/2026-03-22-testability-observability-research.md`](2026-03-22-testability-observability-research.md) -- consolidated findings from three parallel research efforts covering logging, UI testing, and visual regression. This plan implements "Priority 1: Logging Foundation" from the research recommendations.

**Affected Areas**:

- `Cargo.toml` -- new dependencies
- `src/main.rs` -- logging initialization, `WorkerGuard` lifetime, startup flow logging
- `src/lib.rs` -- new module declaration
- `src/cloud_fetcher.rs` -- 15 `eprintln!` conversions + span instrumentation
- `src/config.rs` -- 9 `eprintln!` conversions
- `src/wgpu_init.rs` -- 2 `eprintln!` conversions + adapter selection logging
- `src/renderer/mod.rs` -- 1 `eprintln!` conversion + frame rendering instrumentation
- `src/renderer/textures.rs` -- 2 `eprintln!` conversions + mipmap generation timing
- `src/renderer/gpu_setup.rs` -- GPU resource creation instrumentation
- `src/renderer/render_pass.rs` -- render pass timing
- `src/texture_loader.rs` -- texture decode timing
- `src/wallpaper.rs` -- wallpaper export flow logging
- `src/memory.rs` -- new module for process memory tracking (Windows)

## Success Criteria

- [x] `cargo build` succeeds with no new warnings
- [x] `cargo build --release` succeeds and compiles out debug/trace/info level log calls
- [x] `cargo test` passes (all existing tests still pass)
- [x] `cargo clippy` passes with no new warnings
- [x] Zero `eprintln!` calls remain in non-test code
- [ ] Running `cargo run` produces timestamped, level-tagged, module-prefixed log output on stderr
- [ ] Running `RUST_LOG=trace cargo run` shows verbose output including wgpu internals
- [ ] Running `cargo run --release` shows only warn/error output
- [ ] Application startup sequence is visible in log output (adapter selection, config load, texture resolution, window creation)
- [ ] Texture decode timings appear in log output with structured fields (path, dimensions, elapsed)
- [ ] Cloud fetcher lifecycle is visible (cache load, freshness check, download, decode)
- [ ] Memory usage (RSS) is logged periodically and at key points (after texture decode, after GPU resource creation)

## Implementation Steps

### Phase 1: Add Dependencies

#### Step 1.1: Add tracing ecosystem to Cargo.toml

- **Files**: `Cargo.toml`
- **Action**: Add the following dependencies:
  - `tracing = { version = "0.1", features = ["max_level_debug", "release_max_level_warn"] }`
  - `tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt"] }`
  - `tracing-appender = "0.2"`
  - `tracing-log = "0.2"`
  - `log = { version = "0.4", features = ["max_level_debug", "release_max_level_warn"] }`
  - Add `Win32_System_ProcessStatus` to the `windows-sys` features list (needed for `GetProcessMemoryInfo`)
- **Test cases**: N/A (dependency addition)
- **Verify**: `cargo build` succeeds. `cargo check` reports no errors. The `Cargo.lock` file is updated with the new crates.
- **Complexity**: Small

### Phase 2: Logging Initialization and Memory Module

#### Step 2.1: Create the logging initialization function

- **Files**: `src/main.rs`
- **Action**: Add an `init_logging()` function near the top of `main.rs` that:
  1. Creates a `tracing_appender::non_blocking(std::io::stderr())` writer, returning `(non_blocking_writer, guard)`
  2. Builds a `tracing_subscriber::fmt` layer using the non-blocking writer, with:
     - `with_env_filter(EnvFilter::from_default_env().add_directive("wgpu_core=warn".parse().unwrap()).add_directive("wgpu_hal=warn".parse().unwrap()).add_directive("naga=warn".parse().unwrap()))` to suppress verbose wgpu output by default while respecting `RUST_LOG`
     - `with_target(true)` for module-level context
     - `with_thread_ids(true)` for multi-thread correlation
     - `with_span_events(FmtSpan::CLOSE)` for automatic span timing on close
  3. Calls `tracing_log::LogTracer::init()` to bridge wgpu's `log` crate output into tracing (note: verify if `tracing-subscriber` handles this automatically; if so, skip explicit init)
  4. Installs the subscriber globally with `tracing::subscriber::set_global_default()`
  5. Returns the `WorkerGuard` so the caller can keep it alive
- **Test cases**:
  - Manual: Run `cargo run` and verify timestamped log output appears on stderr
  - Manual: Run `RUST_LOG=debug cargo run` and verify debug-level output appears
  - Manual: Run `RUST_LOG=trace cargo run` and verify trace-level output appears including wgpu internals
- **Verify**: Application starts and produces formatted log output. The `WorkerGuard` is stored in `main()` as a `let _guard = init_logging();` binding that lives until `std::process::exit(0)`.
- **Complexity**: Medium

#### Step 2.2: Call init_logging() at the start of main()

- **Files**: `src/main.rs`
- **Action**: Insert `let _guard = init_logging();` as the very first line inside `main()`, before `Cli::parse()`. Add a `use tracing::{info, warn, error, debug, trace};` import. Add `info!("sunlit earth v{}", env!("CARGO_PKG_VERSION"));` immediately after logging init to confirm it works.
- **Test cases**:
  - Manual: `cargo run` shows the version line as the first log output
- **Verify**: Version info appears in stderr output on startup.
- **Complexity**: Small

#### Step 2.3: Create memory tracking module

- **Files**: `src/memory.rs` (new file), `src/lib.rs`
- **Action**: Create `src/memory.rs` with:
  1. A `pub fn current_rss_bytes() -> Option<u64>` function that:
     - On Windows (`cfg(windows)`): calls `GetProcessMemoryInfo` via `windows-sys` to read `PROCESS_MEMORY_COUNTERS.WorkingSetSize`. Uses `GetCurrentProcess()` for the process handle. Wraps the FFI call in `#[allow(unsafe_code)]` with a `// SAFETY:` comment consistent with `sun.rs` and `wallpaper.rs` patterns.
     - On non-Windows: returns `None`
  2. A `pub fn log_memory_usage(context: &str)` function that calls `current_rss_bytes()` and emits a `debug!` event with structured fields: `context`, `rss_mb` (formatted to 1 decimal place).
  - Add `pub mod memory;` to `src/lib.rs`.
- **Test cases**:
  - Unit test: `current_rss_bytes()` returns `Some(n)` where `n > 0` on Windows
  - Unit test: `current_rss_bytes()` returns a value less than 10 GB (sanity upper bound)
- **Verify**: `cargo test memory` passes. `cargo clippy` passes.
- **Complexity**: Medium

### Phase 3: Migrate eprintln! Calls

Each step in this phase converts `eprintln!` calls in one module to the appropriate `tracing` macro. The target log level for each call is documented.

#### Step 3.1: Migrate config.rs (9 calls)

- **Files**: `src/config.rs`
- **Action**: Add `use tracing::warn;` import. Convert each `eprintln!` to the appropriate level:
  - `"Warning: could not determine config directory"` -> `warn!("could not determine config directory")`
  - `"Warning: could not read config file {}: {e}"` -> `warn!(path = %path.display(), error = %e, "could not read config file")`
  - `"Warning: could not parse config file {}: {e}"` -> `warn!(path = %path.display(), error = %e, "could not parse config file")`
  - `"Warning: could not determine config directory; config not saved"` -> `warn!("could not determine config directory; config not saved")`
  - `"Warning: could not serialize config: {e}"` -> `warn!(error = %e, "could not serialize config")`
  - `"Warning: could not create config directory {}: {e}"` -> `warn!(path = %parent.display(), error = %e, "could not create config directory")`
  - `"Warning: could not write temporary config file {}: {e}"` -> `warn!(path = %tmp_path.display(), error = %e, "could not write temporary config file")`
  - `"Warning: could not rename config file to {}: {e}"` -> `warn!(path = %path.display(), error = %e, "could not rename config file")`
  - `"Warning: saved window position ({x}, {y}) is off-screen, using OS default"` -> `warn!(x, y, "saved window position is off-screen, using OS default")`
- **Test cases**:
  - Automated: All existing `config.rs` tests pass unchanged (the tests don't assert on stderr output)
  - Manual: Run `cargo run` with a corrupt config file and verify warning appears with structured fields
- **Verify**: Zero `eprintln!` in `config.rs` (excluding test code). `cargo test config` passes. `cargo clippy` passes.
- **Complexity**: Small

#### Step 3.2: Migrate cloud_fetcher.rs (15 calls)

- **Files**: `src/cloud_fetcher.rs`
- **Action**: Add `use tracing::{info, warn, debug};` import. Convert each `eprintln!`:
  - `"Warning: could not serialize cloud cache meta: {e}"` -> `warn!(error = %e, "could not serialize cloud cache meta")`
  - `"Warning: could not create cloud cache directory {}: {e}"` -> `warn!(path = %parent.display(), error = %e, "could not create cloud cache directory")`
  - `"Warning: could not write cloud cache meta {}: {e}"` -> `warn!(path = %tmp_path.display(), error = %e, "could not write cloud cache meta")`
  - `"Warning: could not rename cloud cache meta to {}: {e}"` -> `warn!(path = %path.display(), error = %e, "could not rename cloud cache meta")`
  - `"Loaded cached cloud image (WxH) from path"` -> `info!(width = img.width, height = img.height, path = %path.display(), "loaded cached cloud image")`
  - `"Warning: cached cloud image decode failed: {e}"` -> `warn!(error = %e, "cached cloud image decode failed")`
  - `"Warning: could not read cached cloud image: {e}"` -> `warn!(error = %e, "could not read cached cloud image")`
  - `"Cloud image unchanged (304)"` -> `info!("cloud image unchanged (304 Not Modified)")`
  - `"Cloud image has changed, downloading..."` -> `info!("cloud image has changed, downloading")`
  - `"Warning: {e}"` (freshness check error) -> `warn!(error = %e, "cloud freshness check failed")`
  - `"No cached cloud ETag, downloading..."` -> `info!("no cached cloud ETag, downloading")`
  - `"Downloaded cloud image ({} bytes) in {elapsed:.1}s"` -> `info!(bytes = bytes.len(), elapsed_secs = format_args!("{elapsed:.1}"), "downloaded cloud image")`
  - `"Warning: could not cache cloud image: {e}"` -> `warn!(error = %e, "could not cache cloud image")`
  - `"Decoded cloud image (WxH)"` -> `info!(width = img.width, height = img.height, "decoded cloud image")`
  - `"Warning: cloud image decode failed: {e}"` -> `warn!(error = %e, "cloud image decode failed")`
  - `"Warning: {e} (retrying in Ns)"` -> `warn!(error = %e, retry_delay_secs = retry_delay.as_secs(), "cloud download failed, retrying")`
- **Test cases**:
  - Automated: All existing `cloud_fetcher::tests` pass unchanged
  - Manual: Run `cargo run` and observe cloud fetcher lifecycle in log output
- **Verify**: Zero `eprintln!` in `cloud_fetcher.rs` (excluding test code). `cargo test cloud` passes.
- **Complexity**: Medium

#### Step 3.3: Migrate wgpu_init.rs (2 calls)

- **Files**: `src/wgpu_init.rs`
- **Action**: Add `use tracing::{info, warn};` import. Convert:
  - `"Warning: no software adapter found, using default selection"` -> `warn!("no software adapter found, using default selection")`
  - `"Warning: WGPU_ADAPTER_NAME={name:?} not found, using default selection"` -> `warn!(name = %name, "WGPU_ADAPTER_NAME not found, using default selection")`
  - Also add `info!` events for the adapter selection result: `info!(adapter = %adapter_info, "selected GPU adapter")` after the adapter is chosen and `adapter_info` is formatted.
- **Test cases**:
  - Automated: All existing `wgpu_init::tests` pass unchanged
  - Manual: Run `cargo run` and verify adapter selection appears in log output
- **Verify**: Zero `eprintln!` in `wgpu_init.rs`. `cargo test` passes.
- **Complexity**: Small

#### Step 3.4: Migrate renderer/mod.rs (1 call)

- **Files**: `src/renderer/mod.rs`
- **Action**: Add `use tracing::error;` import. Convert:
  - `"Expected WGPU28 graphics API, got something else"` -> `error!("expected WGPU28 graphics API, got unsupported variant")`
- **Test cases**:
  - Automated: All existing `renderer::tests` pass unchanged
- **Verify**: Zero `eprintln!` in `renderer/mod.rs`. `cargo test` passes.
- **Complexity**: Small

#### Step 3.5: Migrate renderer/textures.rs (2 calls)

- **Files**: `src/renderer/textures.rs`
- **Action**: Add `use tracing::{info, error};` import. Convert:
  - `eprintln!("{e}")` (texture decode error) -> `error!(slot = msg.slot_index, error = %e, "texture decode failed")`
  - `"Decoded texture (WxH) from path in Ns"` -> `info!(width = img.width, height = img.height, path = %path.display(), elapsed_secs = format_args!("{:.2}", start.elapsed().as_secs_f64()), "decoded texture")`
- **Test cases**:
  - Automated: All existing `renderer::textures::tests` pass unchanged
- **Verify**: Zero `eprintln!` in `renderer/textures.rs`. `cargo test` passes.
- **Complexity**: Small

### Phase 4: Add Program Flow Logging

#### Step 4.1: Instrument main.rs startup sequence

- **Files**: `src/main.rs`
- **Action**: Add `tracing` log statements at key points in `main()` to make the startup sequence visible:
  - After CLI parse: `debug!(software_rendering = cli.software_rendering, textures_dir = ?cli.textures_dir, "parsed CLI arguments")`
  - After wgpu init: `info!(adapter = %wgpu_context.adapter_info, sample_counts = ?wgpu_context.supported_sample_counts, "initialized wgpu")`
  - After config load: `debug!("loaded config from disk")`
  - After texture resolution: `info!(textures_dir = ?textures_dir, day = ?day_path, night = ?night_path, "resolved texture paths")`
  - After JXL hook registration: `debug!("registered JXL decoding hook")`
  - After cloud fetcher spawn: `info!("spawned cloud fetcher background thread")`
  - After window.run() returns: `info!("event loop exited, saving config")`
  - Before `std::process::exit(0)`: `debug!("exiting")`
- **Test cases**:
  - Manual: Run `RUST_LOG=debug cargo run` and verify the full startup sequence is visible in order
- **Verify**: Startup log output tells a coherent story from CLI parse through window display.
- **Complexity**: Small

#### Step 4.2: Add span instrumentation to cloud_fetcher.rs

- **Files**: `src/cloud_fetcher.rs`
- **Action**: Add `#[tracing::instrument]` attributes or manual `tracing::info_span!` to key functions:
  - `spawn_cloud_fetcher`: wrap the cache-load section in a `info_span!("cloud_cache_load")` and the background thread loop body in a `info_span!("cloud_poll_cycle")`
  - `download_image`: add `#[tracing::instrument(skip(agent), fields(url = CLOUD_URL))]`
  - `decode_cloud_jpeg`: add `#[tracing::instrument(skip(bytes), fields(bytes_len = bytes.len()))]`
  - `check_freshness`: add `#[tracing::instrument(skip(agent), fields(etag = %etag))]`
- **Test cases**:
  - Automated: All existing tests pass
  - Manual: `RUST_LOG=debug cargo run` shows span context in cloud fetcher log lines
- **Verify**: Cloud fetcher operations include span context (function name, fields) in log output.
- **Complexity**: Small

#### Step 4.3: Add span instrumentation to texture loading

- **Files**: `src/renderer/textures.rs`, `src/texture_loader.rs`
- **Action**:
  - In `textures.rs`: wrap the background thread in `maybe_spawn_texture_load` with a `info_span!("texture_decode", slot = slot_index, path = %path.display())`. Add `tracing::info!` after `create_mipmapped_texture` completes in `process_decoded_textures` reporting the slot index and that the GPU texture was created.
  - In `texture_loader.rs`: add `#[tracing::instrument(skip_all, fields(path = %path.display()))]` to `load()`.
  - In `textures.rs`: add `#[tracing::instrument(skip(device, queue, rgba_pixels), fields(label, width, height, mip_levels))]` to `create_mipmapped_texture`.
- **Test cases**:
  - Automated: All existing tests pass
  - Manual: `RUST_LOG=debug cargo run` shows texture decode timing and mipmap generation in log output
- **Verify**: Texture loading shows span-based timing in log output.
- **Complexity**: Small

#### Step 4.4: Add span instrumentation to rendering pipeline

- **Files**: `src/renderer/mod.rs`, `src/renderer/render_pass.rs`, `src/renderer/gpu_setup.rs`
- **Action**:
  - In `renderer/mod.rs`:
    - Add `trace!("rendering setup")` at the start of the `RenderingSetup` arm
    - Add `trace!("rendering teardown")` at the start of the `RenderingTeardown` arm
    - Add `trace!(received_any, "before rendering")` at the start of the `BeforeRendering` arm, after the GPU_RESOURCES borrow
    - Add `debug!(sample_count = desired, "MSAA sample count changed")` when MSAA rebuild triggers
    - Add `debug!(width = vw, height = vh, "viewport size changed")` when render texture rebuild triggers
    - Add `trace!("frame skipped (unchanged)")` when dirty-check skips a render
  - In `renderer/render_pass.rs`:
    - Add `#[tracing::instrument(skip_all, fields(width = res.render_width, height = res.render_height))]` to `execute_render_pass`
  - In `renderer/gpu_setup.rs`:
    - Add `#[tracing::instrument(skip_all, fields(width, height, sample_count))]` to `create_gpu_resources`
    - Add `debug!("rebuilding MSAA resources")` to `rebuild_msaa_resources`
    - Add `debug!(width, height, "rebuilding render textures")` to `rebuild_render_textures`
- **Test cases**:
  - Automated: All existing tests pass (including `tests/render_pipeline.rs` GPU tests)
  - Manual: `RUST_LOG=trace cargo run` shows per-frame rendering activity
- **Verify**: Frame rendering, dirty-check skips, and GPU resource rebuilds are visible in trace output.
- **Complexity**: Medium

#### Step 4.5: Add memory usage logging at key points

- **Files**: `src/main.rs`, `src/cloud_fetcher.rs`, `src/renderer/textures.rs`, `src/renderer/gpu_setup.rs`
- **Action**: Call `memory::log_memory_usage()` at key allocation points:
  - In `main.rs`: after wgpu init (`"after wgpu init"`), after window creation (`"after window creation"`)
  - In `renderer/gpu_setup.rs`: at the end of `create_gpu_resources` (`"after GPU resource creation"`)
  - In `renderer/textures.rs`: after `create_mipmapped_texture` in `process_decoded_textures` (`"after texture upload"`)
  - In `cloud_fetcher.rs`: after `decode_cloud_jpeg` succeeds (`"after cloud decode"`)
- **Test cases**:
  - Manual: `RUST_LOG=debug cargo run` shows RSS values at each checkpoint
- **Verify**: Memory usage checkpoints appear in debug output.
- **Complexity**: Small

#### Step 4.6: Add wallpaper export flow logging

- **Files**: `src/wallpaper.rs`, `src/main.rs`
- **Action**:
  - In `main.rs` `do_set_wallpaper()`: add `info!("starting wallpaper export")` at entry and `info!(path = %path.display(), "wallpaper set successfully")` on success
  - In `wallpaper.rs`:
    - Add `debug!(width, height, "detected primary monitor resolution")` in `get_primary_monitor_resolution` before returning `Ok`
    - Add `debug!(path = %path.display(), "saving wallpaper PNG")` in `save_wallpaper_image` before encoding
    - Add `debug!("setting Fill wallpaper style")` in `ensure_fill_style`
    - Add `info!(path = %abs_path.display(), "applying wallpaper via SystemParametersInfoW")` in `set_wallpaper`
- **Test cases**:
  - Automated: All existing `wallpaper::tests` pass
  - Manual: Click "Set as Wallpaper" and verify the full export pipeline appears in log output
- **Verify**: Wallpaper export flow is visible in log output from start to finish.
- **Complexity**: Small

### Phase 5: Verification and Cleanup

#### Step 5.1: Verify zero eprintln! in non-test code

- **Files**: All `src/**/*.rs`
- **Action**: Search for remaining `eprintln!` calls outside of `#[cfg(test)]` blocks. Convert any that were missed.
- **Test cases**:
  - Automated: `grep -r "eprintln!" src/ --include="*.rs"` shows results only inside `#[cfg(test)]` modules (if any)
- **Verify**: No `eprintln!` calls remain in production code paths.
- **Complexity**: Small

#### Step 5.2: Run full test suite and clippy

- **Files**: N/A
- **Action**: Run `cargo test`, `cargo clippy`, and `cargo build --release` to verify everything works.
- **Test cases**:
  - Automated: `cargo test` -- all tests pass
  - Automated: `cargo clippy` -- no new warnings
  - Automated: `cargo build --release` -- compiles successfully
- **Verify**: All three commands succeed with zero errors and zero new warnings.
- **Complexity**: Small

#### Step 5.3: Manual end-to-end verification

- **Files**: N/A
- **Action**: Manual verification of the complete logging experience:
  - Run `cargo run` (default log level) and verify:
    - Version info printed at startup
    - Adapter selection visible
    - Texture paths resolved
    - Cloud fetcher cache load visible
    - No spam from wgpu internals
  - Run `RUST_LOG=debug cargo run` and verify:
    - CLI arguments visible
    - Config load details visible
    - Memory usage at key points
    - Texture decode timing
    - Frame rendering activity
  - Run `RUST_LOG=trace cargo run` and verify:
    - Per-frame dirty-check skip/render decisions visible
    - Span close timings visible
  - Run `cargo run --release` and verify:
    - Only warn/error output appears
    - No debug/info/trace output
- **Verify**: All manual checks pass.
- **Complexity**: Small

## Test Strategy

### Automated Tests

| Test Case | Type | Input | Expected Output |
| --- | --- | --- | --- |
| `current_rss_bytes` returns Some on Windows | Unit | None | `Some(n)` where `n > 0` |
| `current_rss_bytes` returns reasonable value | Unit | None | `Some(n)` where `n < 10 * 1024 * 1024 * 1024` |
| All existing config tests pass | Unit | Existing test inputs | Unchanged expected outputs |
| All existing cloud_fetcher tests pass | Unit | Existing test inputs | Unchanged expected outputs |
| All existing wgpu_init tests pass | Unit | Existing test inputs | Unchanged expected outputs |
| All existing renderer tests pass | Unit | Existing test inputs | Unchanged expected outputs |
| All existing texture_loader tests pass | Unit | Existing test inputs | Unchanged expected outputs |
| All existing wallpaper tests pass | Unit | Existing test inputs | Unchanged expected outputs |
| GPU integration tests (shading.rs) pass | Integration | GPU pipeline | Behavioral invariants hold |
| GPU integration tests (render_pipeline.rs) pass | Integration | GPU pipeline | Behavioral invariants hold |

### Manual Verification

- [ ] `cargo run` produces timestamped, level-tagged log output on stderr without wgpu spam
- [ ] `RUST_LOG=debug cargo run` shows verbose startup sequence and memory checkpoints
- [ ] `RUST_LOG=trace cargo run` shows per-frame rendering decisions and span timings
- [ ] `cargo run --release` shows only warn/error output (no info/debug/trace)
- [ ] Cloud fetcher lifecycle is visible: cache load, poll cycle, freshness check result, download/decode timing
- [ ] Texture decode timing appears with path, dimensions, and elapsed seconds
- [ ] Clicking "Set as Wallpaper" shows the full export pipeline in logs
- [ ] No visual or behavioral changes to the application (rendering, UI, wallpaper export)

## Risks and Mitigations

| Risk | Impact | Mitigation |
| --- | --- | --- |
| `WorkerGuard` dropped before `std::process::exit(0)` | Buffered log lines lost at shutdown | Store guard as named binding in `main()` scope; explicit `drop(_guard)` before `exit(0)` is not needed since `exit` terminates immediately. The guard must simply not be dropped *before* the last log statement. |
| Subscriber initialization order wrong | Early log calls lost or panic | Call `init_logging()` as the very first line in `main()`, before `Cli::parse()` |
| `tracing-log` double-initialization | Panic at startup | Check if `tracing-subscriber` auto-initializes `LogTracer`; if so, skip explicit `LogTracer::init()`. The research flagged this as Open Question #5. |
| Span overhead in rendering hot path | Frame rate regression | Use `trace!` level (compiled out in release) for per-frame events in the rendering callback. Use `info!` only for events that happen occasionally (texture loads, resize). The render callback runs at most once per 2-minute timer tick or user interaction, not at 60fps, so overhead is minimal. |
| `GetProcessMemoryInfo` FFI safety | Memory corruption | Follow established `#[allow(unsafe_code)]` + `// SAFETY:` pattern from `sun.rs` and `wallpaper.rs`. The function reads process-level counters and cannot corrupt memory. |
| `windows-sys` feature flag addition | CI build breakage | `Win32_System_ProcessStatus` is a standard feature in `windows-sys 0.59`. Test with `cargo build` before committing. |
| Compile-time level features break test assertions on log output | Tests fail | No existing tests assert on stderr content, so this risk is zero. Future tests that need to capture log output should install a test-specific subscriber layer. |

## Rollback Strategy

All changes are additive (new dependencies, new module, macro replacements). To roll back:

1. Revert the commit(s)
2. Run `cargo build` to confirm the revert is clean

No database migrations, config format changes, or breaking API changes are involved. The `Cargo.lock` changes are the only artifact that would need reverting alongside the source.

## Status

- [x] Plan approved
- [x] Implementation started
- [x] Implementation complete

### Completion Notes

- **Step 1.1**: `tracing-log` omitted (not needed -- `tracing-subscriber`'s `fmt` layer handles `log` bridging automatically via its built-in `tracing-log` integration). `Win32_System_Threading` feature added alongside `Win32_System_ProcessStatus` (needed for `GetCurrentProcess`).
- **Steps 2.1-2.2**: Logging init uses `tracing_subscriber::registry()` with `fmt` layer + `EnvFilter` instead of `set_global_default()` (idiomatic `tracing-subscriber` 0.3 pattern).
- **Step 2.3**: `current_rss_bytes()` uses `&raw mut` instead of `&mut` for the FFI call to avoid the `implicit_borrow_as_raw_pointer` clippy lint.
- **All Phase 3 steps**: Complete. Zero `eprintln!` calls remain in any source file.
- **All Phase 4 steps**: Complete. Span instrumentation added to cloud_fetcher, texture_loader, render_pass, and gpu_setup. Memory checkpoints added at all specified locations.
- **Phase 5**: `cargo build`, `cargo test` (236 tests pass), `cargo clippy` (0 warnings), `cargo build --release` all succeed.
