# Plan: Live Clouds (2026-03-20)

## Summary

Replace the static local cloud PNG loader with a live-updating cloud texture fetched from the matteason/live-cloud-maps HTTP service. The app will download an 8K equirectangular cloud JPEG, cache it to disk, poll for updates every 60 minutes using HEAD+ETag freshness checks, and decode/upload new images in the background. Two new shader uniforms (cloud floor and cloud gamma) will provide real-time contrast tuning via UI sliders. The existing `cloud.png` file-based loading path will be removed entirely.

## Stakes Classification

**Level**: High
**Rationale**: This touches nearly every layer of the application -- new dependency (`ureq`), new background thread with HTTP I/O, disk cache with metadata, shader uniform layout changes, dirty-checking changes, config persistence changes, UI additions, and removal of an existing code path. The uniform buffer layout change affects GPU integration tests. Incorrect implementation could break rendering or cause runtime panics.

## Context

**Research**: `docs/plans/2026-03-20-live-clouds-research.md` (primary), with supplementary findings in `docs/plans/2026-03-20-ureq-304-research.md`, `docs/plans/2026-03-20-head-request-research.md`, `docs/plans/2026-03-20-image-jpeg-research.md`
**Affected Areas**: `Cargo.toml`, `src/lib.rs`, `src/main.rs`, `src/config.rs`, `src/texture_loader.rs`, `src/renderer/mod.rs`, `src/renderer/uniforms.rs`, `src/renderer/frame.rs`, `src/renderer/render_pass.rs`, `src/renderer/gpu_setup.rs`, `src/renderer/texture_routing.rs`, `shaders/sphere.wgsl`, `ui/main.slint`, plus a new module `src/cloud_fetcher.rs`

## Success Criteria

- [ ] App downloads the 8K cloud JPEG from `https://clouds.matteason.co.uk/images/8192x4096/clouds.jpg` on startup
- [ ] Downloaded JPEG is cached to `%LOCALAPPDATA%\SunlitEarth\clouds_cache.jpg` with ETag metadata in `clouds_cache_meta.toml`
- [ ] On startup with a cached image, the cached image loads immediately and a background freshness check runs
- [ ] Background thread polls every 60 minutes using HEAD+`If-None-Match`; full GET only when ETag changes
- [ ] App functions normally with no network access (uses cached image or shows no clouds)
- [ ] Cloud floor and cloud gamma sliders in the UI adjust cloud contrast in real time (GPU-side)
- [ ] Slider values persist across app restarts via `config.toml`
- [ ] The static `cloud.png` file loader is fully removed
- [ ] `cargo test` passes, `cargo clippy` is clean
- [ ] Uniform buffer remains 144 bytes (compile-time assertion preserved)

## Implementation Steps

### Phase 1: Dependencies and Uniform Layout

Foundation changes that all subsequent phases build on.

#### Step 1.1: Add ureq and JPEG feature to Cargo.toml

- **Files**: `Cargo.toml:8-21`
- **Action**: Add `ureq = { version = "3", default-features = false, features = ["rustls"] }` to `[dependencies]`. Add `"jpeg"` to the `image` crate's features list so it reads `features = ["png", "jpeg"]`.
- **Verify**: `cargo check` succeeds. `cargo tree -i ureq` shows ureq 3.x. `cargo tree -i image` shows jpeg-related decoder crates.
- **Complexity**: Small

#### Step 1.2: Repurpose uniform padding fields for cloud_floor and cloud_gamma (Rust)

- **Files**: `src/renderer/uniforms.rs`
- **Action**: Rename `_pad3: f32` to `cloud_floor: f32` and `_pad4: f32` to `cloud_gamma: f32`. Both fields remain `pub(crate)`. The compile-time size assertion (`== 144`) is unchanged because the struct size does not change.
- **Test cases**:
  - Compile-time assertion still holds (struct is 144 bytes)
  - `bytemuck::Pod` and `bytemuck::Zeroable` derive still compiles
- **Verify**: `cargo check` succeeds; size assertion compiles.
- **Complexity**: Small

#### Step 1.3: Repurpose uniform padding fields for cloud_floor and cloud_gamma (WGSL)

- **Files**: `shaders/sphere.wgsl:1-19`
- **Action**: In the WGSL `Uniforms` struct, rename `_pad3: f32` (offset 136) to `cloud_floor: f32` and `_pad4: f32` (offset 140) to `cloud_gamma: f32`. Update the comments accordingly.
- **Verify**: `cargo build` succeeds (shader compiles at runtime, but this validates the Rust side).
- **Complexity**: Small

#### Step 1.4: Apply floor and gamma in fs_cloud shader

- **Files**: `shaders/sphere.wgsl:132-138` (the `fs_cloud` function)
- **Action**: Replace the raw texture sample with floor/gamma post-processing. The current line `let cloud_density = textureSample(sphere_texture, sphere_sampler, in.uv).r;` becomes:

  ```wgsl
  let raw = textureSample(sphere_texture, sphere_sampler, in.uv).r;
  let floored = saturate((raw - uniforms.cloud_floor) / (1.0 - uniforms.cloud_floor));
  let cloud_density = pow(floored, 1.0 / uniforms.cloud_gamma);
  ```

  Guard against `cloud_gamma <= 0` by clamping the exponent: `1.0 / max(uniforms.cloud_gamma, 0.01)`. Guard against `cloud_floor >= 1.0` by using `max(1.0 - uniforms.cloud_floor, 0.001)` as the divisor.
- **Test cases** (manual visual verification):
  - With floor=0.0, gamma=1.0: clouds look identical to current behavior (identity transform)
  - With floor=0.2, gamma=0.3: thin clouds vanish, thick clouds gain contrast
  - With floor=0.0, gamma=0.1: all clouds appear very bright/opaque
  - With floor=0.5, gamma=1.0: only the densest clouds remain visible
- **Verify**: `cargo build` succeeds. Manual visual check deferred to Phase 6.
- **Complexity**: Small

#### Step 1.5: Wire cloud_floor and cloud_gamma through ShadingParams and write_uniforms

- **Files**: `src/renderer/render_pass.rs:12-26` (ShadingParams), `src/renderer/render_pass.rs:63-106` (write_uniforms)
- **Action**: Add `cloud_floor: f32` and `cloud_gamma: f32` fields to `ShadingParams`. In `write_uniforms`, replace `_pad3: 0.0` with `cloud_floor: shading.cloud_floor` and `_pad4: 0.0` with `cloud_gamma: shading.cloud_gamma`.
- **Verify**: `cargo check` succeeds.
- **Complexity**: Small

#### Step 1.6: Add cloud_floor and cloud_gamma to FrameState and build_frame_state

- **Files**: `src/renderer/frame.rs`
- **Action**: Add two new quantized fields to `FrameState`: `cloud_floor: i32` and `cloud_gamma: i32`. Add corresponding parameters to `build_frame_state()` and quantize them as `(value * 1000.0) as i32`.
- **Test cases**:
  - `cloud_floor` quantization: input 0.196 produces 196
  - `cloud_gamma` quantization: input 0.3 produces 300
  - Changing `cloud_floor` triggers dirty (two `FrameState` values with different `cloud_floor` compare unequal)
  - Changing `cloud_gamma` triggers dirty
- **Verify**: `cargo test frame` passes with new tests.
- **Complexity**: Small

#### Step 1.7: Update all build_frame_state call sites

- **Files**: `src/renderer/mod.rs:389-405` (BeforeRendering callback)
- **Action**: The `build_frame_state()` call gains two new arguments: `cloud_floor_f` and `cloud_gamma_f`. Read these from the Slint window properties (to be added in Phase 3). For now, pass hardcoded defaults `0.0` and `1.0` to keep the build passing; Phase 3 will replace these with actual UI property reads.
- **Verify**: `cargo build` succeeds.
- **Complexity**: Small

#### Step 1.8: Update ShadingParams construction in BeforeRendering

- **Files**: `src/renderer/mod.rs:437-450` (where ShadingParams is constructed)
- **Action**: Add `cloud_floor` and `cloud_gamma` fields to the `ShadingParams` literal, using the same float values read from the window (or the hardcoded defaults from step 1.7).
- **Verify**: `cargo build` succeeds. Existing tests pass.
- **Complexity**: Small

### Phase 2: Cloud Fetcher Module

New module for HTTP fetching, disk caching, and background thread management.

#### Step 2.1: Create cloud_fetcher module with cache types and constants

- **Files**: `src/cloud_fetcher.rs` (new file), `src/lib.rs`
- **Action**: Create `src/cloud_fetcher.rs` and add `pub mod cloud_fetcher;` to `src/lib.rs`. Define:
  - `const CLOUD_URL: &str = "https://clouds.matteason.co.uk/images/8192x4096/clouds.jpg";`
  - `const POLL_INTERVAL: Duration = Duration::from_secs(3600);` (60 minutes)
  - `struct CacheMeta { etag: Option<String>, last_modified: Option<String> }` with serde Serialize/Deserialize
  - `fn cache_dir() -> Option<PathBuf>` returning `dirs::data_local_dir()?.join("SunlitEarth")`
  - `fn cache_image_path() -> Option<PathBuf>` returning `cache_dir()?.join("clouds_cache.jpg")`
  - `fn cache_meta_path() -> Option<PathBuf>` returning `cache_dir()?.join("clouds_cache_meta.toml")`
  - `fn load_cache_meta(path: &Path) -> Option<CacheMeta>` (read + toml::from_str, return None on any error)
  - `fn save_cache_meta(meta: &CacheMeta, path: &Path)` (toml::to_string_pretty + atomic write with `~` suffix)
- **Test cases**:
  - `load_cache_meta` on nonexistent file returns `None`
  - `save_cache_meta` + `load_cache_meta` round-trips correctly
  - `save_cache_meta` creates parent directories
  - `CacheMeta` with only `etag` set serializes/deserializes correctly
  - `CacheMeta` with both fields `None` serializes/deserializes correctly
- **Verify**: `cargo test cloud_fetcher` passes.
- **Complexity**: Medium

#### Step 2.2: Implement HTTP freshness check and download functions

- **Files**: `src/cloud_fetcher.rs`
- **Action**: Implement two functions:
  - `fn check_freshness(agent: &ureq::Agent, etag: &str) -> Result<bool, String>`: Sends HEAD request with `If-None-Match` header. Returns `Ok(true)` if 304 (unchanged), `Ok(false)` if 200 (new content available). Maps transport errors and 4xx/5xx to `Err`.
  - `fn download_image(agent: &ureq::Agent) -> Result<(Vec<u8>, CacheMeta), String>`: Sends unconditional GET request. Reads body into `Vec<u8>`. Extracts `ETag` and `Last-Modified` headers into `CacheMeta`. Maps errors to `Err`.

  Key ureq 3.x patterns (from research):
  - 304 arrives as `Ok(response)` with `response.status() == 304`, NOT as `Err`
  - 4xx/5xx arrive as `Err(ureq::Error::StatusCode(code))`
  - Use `response.into_body().read_to_vec()` to read the body
  - Use `response.headers().get("ETag")` for header extraction
- **Test cases**: These functions require a live HTTP server and are not unit-testable without mocking. Testing will be manual:
  - First download with no ETag: returns image bytes and populated CacheMeta
  - Freshness check with current ETag: returns `Ok(true)` (304)
  - Freshness check with stale ETag: returns `Ok(false)` (200 on HEAD means new content)
- **Verify**: `cargo check` succeeds. Manual testing deferred to Phase 6.
- **Complexity**: Medium

#### Step 2.3: Implement JPEG decode function

- **Files**: `src/cloud_fetcher.rs`
- **Action**: Implement `fn decode_cloud_jpeg(bytes: &[u8]) -> Result<DecodedImage, String>` that:
  1. Calls `image::load_from_memory(bytes)` to decode the JPEG
  2. Applies `.fliph()` and `.into_rgba8()` (same transforms as `texture_loader::load`)
  3. Calls the horizontal shift function to align the prime meridian
  4. Returns a `texture_loader::DecodedImage` with pixels, width, height

  The horizontal shift logic is currently private in `texture_loader.rs`. Either make `shift_horizontal` `pub(crate)` or duplicate the logic. Making it `pub(crate)` is preferred to avoid duplication.
- **Test cases**:
  - Decoding a minimal valid JPEG (e.g., a 2x2 pixel JPEG constructed in-memory) returns `Ok` with correct dimensions
  - Decoding invalid bytes returns `Err`
  - Decoded image has width and height matching the source
- **Verify**: `cargo test cloud_fetcher` passes.
- **Complexity**: Small

#### Step 2.4: Make shift_horizontal pub(crate) in texture_loader

- **Files**: `src/texture_loader.rs:53`
- **Action**: Change `fn shift_horizontal` to `pub(crate) fn shift_horizontal`. No other changes needed -- existing tests continue to work.
- **Verify**: `cargo test texture_loader` passes. `cargo check` succeeds.
- **Complexity**: Small

#### Step 2.5: Implement the background polling thread

- **Files**: `src/cloud_fetcher.rs`
- **Action**: Implement `pub fn spawn_cloud_fetcher(tx: mpsc::Sender<DecodedTextureMessage>, window_weak: slint::Weak<MainWindow>, clouds_slot: usize)` that:
  1. On the spawning thread (before `thread::spawn`): load cached JPEG from `cache_image_path()` if it exists, decode it, send as `DecodedTextureMessage` for `clouds_slot` via `tx`, and call `window_weak.upgrade_in_event_loop()` to request a redraw.
  2. Load `CacheMeta` from `cache_meta_path()`.
  3. Spawn a `std::thread::spawn` background thread that loops:
     a. If we have a cached ETag, call `check_freshness(agent, &etag)`. If `Ok(true)` (304), sleep and continue. If `Ok(false)` (new content), proceed to step (b). On error, log to stderr, sleep, and continue.
     b. Call `download_image(agent)`. On success: write JPEG bytes to `cache_image_path()`, save new `CacheMeta` to `cache_meta_path()`, decode the JPEG, send `DecodedTextureMessage` via `tx`, request redraw. On error: log, sleep, continue.
     c. If we have no cached ETag (first run, no cache), skip HEAD and go directly to download.
     d. Sleep `POLL_INTERVAL` between iterations.
  4. The thread owns a `ureq::Agent` for connection pooling.

  **Important**: The initial cached image load (step 1) happens synchronously on the calling thread so the texture is available before the first render frame, avoiding a flash of no-clouds. The background thread (step 3) handles all subsequent network I/O.

  Actually, re-reading the architecture: the initial cache load should also go through the channel, just like existing texture loading. The `process_decoded_textures()` function will pick it up on the next `BeforeRendering` callback. This is consistent with how Day/Night textures load.
- **Test cases**: This function orchestrates I/O and threading, so it is not unit-testable. Manual verification:
  - First run (no cache): app starts with no clouds, then clouds appear after download completes
  - Second run (with cache): clouds appear immediately on startup
  - With network disconnected: cached clouds appear, no errors printed to stderr (or only a single warning)
- **Verify**: `cargo check` succeeds. Manual testing deferred to Phase 6.
- **Complexity**: Large

### Phase 3: UI Sliders for Cloud Floor and Gamma

#### Step 3.1: Add cloud-floor and cloud-gamma properties to MainWindow

- **Files**: `ui/main.slint:51` (after `cloud-opacity`)
- **Action**: Add two new `in-out` properties:

  ```slint
  in-out property <float> cloud-floor: 0.0;
  in-out property <float> cloud-gamma: 1.0;
  ```

  Default `cloud-floor` to 0.0 (no floor, identity) and `cloud-gamma` to 1.0 (identity gamma). These defaults mean unchanged cloud appearance when the user has not tuned them.
- **Verify**: `cargo build` succeeds.
- **Complexity**: Small

#### Step 3.2: Add Floor and Gamma sliders to the Clouds GroupBox

- **Files**: `ui/main.slint:496-521` (the Clouds GroupBox)
- **Action**: Add two new slider rows after the Opacity slider, following the existing pattern:
  - **Floor** slider: min 0.0, max 0.5, bound to `root.cloud-floor`, display as percentage. The max of 0.5 is chosen because values above 0.5 eliminate most visible clouds.
  - **Gamma** slider: min 0.1, max 2.0, bound to `root.cloud-gamma`, display to 2 decimal places. Range allows both contrast boost (< 1.0) and reduction (> 1.0). The research-recommended default of 0.3 can be set later after visual tuning; for now 1.0 (identity) is safer.
  Both sliders call `root.sliders-changed()` on change.
- **Verify**: `cargo build` succeeds. `cargo run` shows the new sliders in the Clouds group.
- **Complexity**: Small

#### Step 3.3: Add cloud_floor and cloud_gamma to AppConfig

- **Files**: `src/config.rs:57-58` (after `cloud_opacity`)
- **Action**: Add two new fields to `AppConfig`:

  ```rust
  pub cloud_floor: f32,
  pub cloud_gamma: f32,
  ```

  Set defaults in `Default::default()`: `cloud_floor: 0.0`, `cloud_gamma: 1.0`. These identity defaults mean existing config files without these fields will behave unchanged (thanks to `#[serde(default)]`).
- **Test cases**:
  - Default `cloud_floor` is 0.0
  - Default `cloud_gamma` is 1.0
  - Serde round-trip preserves non-default values (e.g., floor=0.2, gamma=0.3)
  - Deserializing a TOML string without these fields fills defaults
- **Verify**: `cargo test config` passes.
- **Complexity**: Small

#### Step 3.4: Wire cloud_floor and cloud_gamma through main.rs

- **Files**: `src/main.rs`
- **Action**: Update four functions:
  1. `apply_config_to_window`: Add `window.set_cloud_floor(config.cloud_floor)` and `window.set_cloud_gamma(config.cloud_gamma)`.
  2. `read_config_from_window`: Add `cloud_floor: window.get_cloud_floor()` and `cloud_gamma: window.get_cloud_gamma()` to the `AppConfig` struct literal.
  3. `on_reset_all` callback: Add `win.set_cloud_floor(lighting.cloud_floor)` and `win.set_cloud_gamma(lighting.cloud_gamma)`.
  4. Replace the hardcoded `0.0`/`1.0` from Step 1.7 with `win.get_cloud_floor()` and `win.get_cloud_gamma()`.
- **Verify**: `cargo build` succeeds.
- **Complexity**: Small

#### Step 3.5: Read cloud_floor and cloud_gamma in BeforeRendering and pass through pipeline

- **Files**: `src/renderer/mod.rs` (BeforeRendering callback, around lines 375-450)
- **Action**: Read `cloud_floor_f` and `cloud_gamma_f` from the window (`win.get_cloud_floor()`, `win.get_cloud_gamma()`). Pass them to `build_frame_state()` (replacing the hardcoded defaults from Step 1.7). Include them in the `ShadingParams` struct literal (replacing the hardcoded defaults from Step 1.8).
- **Verify**: `cargo build` succeeds. Moving the Cloud Floor or Gamma slider triggers a re-render.
- **Complexity**: Small

### Phase 4: Remove Static Cloud PNG Loader

#### Step 4.1: Remove cloud_path parameter from setup_rendering_notifier

- **Files**: `src/renderer/mod.rs:247-268`
- **Action**: Remove the `cloud_path: Option<PathBuf>` parameter from `setup_rendering_notifier()`. Remove `cloud_path.as_ref()` from the closure capture and the `rendering_callback` call. Update `rendering_callback` signature to remove the `cloud_path` parameter.
- **Verify**: `cargo check` succeeds (will show errors in main.rs until Step 4.3).
- **Complexity**: Small

#### Step 4.2: Remove cloud_path from create_gpu_resources

- **Files**: `src/renderer/gpu_setup.rs:17-27, 165-182`
- **Action**: Remove the `cloud_path: Option<PathBuf>` parameter from `create_gpu_resources()`. Change the CLOUDS_SLOT `TextureSlot` initialization to use `source_path: None` (it was previously set to `cloud_path`). The slot still exists in `texture_slots` -- it just starts empty, waiting for the cloud fetcher to populate it via the channel.
- **Verify**: `cargo check` succeeds (modulo call-site updates).
- **Complexity**: Small

#### Step 4.3: Remove cloud_path resolution from main.rs and integrate cloud fetcher

- **Files**: `src/main.rs:71-74, 258`
- **Action**:
  1. Remove the `cloud_path` variable and its resolution logic (`textures_dir.as_ref().map(|d| d.join("clouds.png"))...`).
  2. Update the `setup_rendering_notifier` call to remove the `cloud_path` argument.
  3. After `setup_rendering_notifier`, call `cloud_fetcher::spawn_cloud_fetcher(...)`. This requires access to the `texture_tx` channel sender and `window_weak`, which are currently encapsulated inside `GpuResources`. Two approaches:
     - **Option A**: Have `spawn_cloud_fetcher` called inside `create_gpu_resources` (but this couples GPU setup to networking).
     - **Option B**: Return the `texture_tx` clone from `setup_rendering_notifier` so `main.rs` can pass it to the cloud fetcher.
     - **Option C**: Create the `mpsc` channel in `main.rs` and pass it into both `setup_rendering_notifier` and `spawn_cloud_fetcher`.

  **Decision**: Option C is cleanest. Create the `(texture_tx, texture_rx)` channel in `main.rs`, pass `texture_tx.clone()` and `window.as_weak()` to `spawn_cloud_fetcher`, and pass `texture_rx` (and a separate `texture_tx` clone for the existing texture loading threads) into `setup_rendering_notifier` / `create_gpu_resources`. This requires modifying `setup_rendering_notifier` and `create_gpu_resources` to accept `mpsc::Sender` and `mpsc::Receiver` instead of creating them internally.

  Actually, this is a larger refactor than necessary. A simpler approach: have `setup_rendering_notifier` return a `mpsc::Sender<DecodedTextureMessage>` clone and the `slint::Weak<MainWindow>`, so `main.rs` can pass them to `spawn_cloud_fetcher`. But the channel is created inside `create_gpu_resources` which is called from the rendering callback...

  **Simplest approach**: Create the channel in `main.rs` before `setup_rendering_notifier`. Pass the sender and receiver into the rendering notifier setup. The cloud fetcher gets a clone of the sender. This requires threading the channel through `setup_rendering_notifier` -> `rendering_callback` -> `create_gpu_resources`.
- **Verify**: `cargo build` succeeds. The app no longer looks for `clouds.png`.
- **Complexity**: Medium

#### Step 4.4: Remove cloud texture loading from texture_routing

- **Files**: `src/renderer/texture_routing.rs:37-40`
- **Action**: Remove the `maybe_spawn_texture_load(res, CLOUDS_SLOT)` call from `resolve_textures()`. The cloud texture is now populated exclusively by the cloud fetcher thread, not by the file-based texture loading system. The `CLOUDS_SLOT` still exists in `texture_slots` and `process_decoded_textures()` still handles messages for it -- only the file-based spawn trigger is removed.
- **Verify**: `cargo build` succeeds.
- **Complexity**: Small

### Phase 5: Channel Refactor

This phase restructures channel ownership so `main.rs` can share the sender with the cloud fetcher.

#### Step 5.1: Move channel creation from create_gpu_resources to setup_rendering_notifier's caller

- **Files**: `src/renderer/mod.rs`, `src/renderer/gpu_setup.rs`, `src/main.rs`
- **Action**:
  1. In `main.rs`: Create `let (texture_tx, texture_rx) = mpsc::channel::<DecodedTextureMessage>();` before calling `setup_rendering_notifier`.
  2. Add `texture_tx: mpsc::Sender<DecodedTextureMessage>` and `texture_rx: mpsc::Receiver<DecodedTextureMessage>` parameters to `setup_rendering_notifier`.
  3. Thread them through the closure into `rendering_callback` and into `create_gpu_resources`.
  4. Remove the `let (texture_tx, texture_rx) = mpsc::channel();` line from `create_gpu_resources`.
  5. In `main.rs`, after `setup_rendering_notifier`, pass a `texture_tx.clone()` and `window.as_weak()` to `cloud_fetcher::spawn_cloud_fetcher(...)`.

  Note: `mpsc::Receiver` is not `Clone` or `Send`+`Sync` across the closure boundary trivially. Since the rendering callback runs on the UI thread and `Receiver` is `Send`, it can be moved into the closure. However, `set_rendering_notifier` takes `FnMut + 'static`, and the receiver needs to be captured by move. The existing code already creates the channel inside `create_gpu_resources` which is called from the closure, so the receiver lives inside `GpuResources`. The simplest change: pass only the `Sender` to `create_gpu_resources` and still create the `Receiver` there... but that does not help us share the `Sender` with `main.rs`.

  **Revised approach**: Keep the channel creation inside `create_gpu_resources` but add a method or parameter to expose a `Sender` clone. Alternatively, create a `Sender` clone before entering the rendering notifier closure and pass it to both the closure (for `create_gpu_resources`) and `spawn_cloud_fetcher`. Since `mpsc::Sender` is `Clone + Send`, create it in `main.rs`, clone for the closure, clone for the fetcher.

  The `Receiver` must be created at the same time as the `Sender`. So create both in `main.rs`. Move the `Receiver` into the closure (it is `Send`). Pass a `Sender` clone to `spawn_cloud_fetcher`.

  Concrete changes:
  1. `main.rs`: `let (texture_tx, texture_rx) = mpsc::channel();`
  2. `setup_rendering_notifier` gains `texture_tx: mpsc::Sender<DecodedTextureMessage>, texture_rx: mpsc::Receiver<DecodedTextureMessage>` parameters.
  3. The closure captures `texture_tx` and `texture_rx` by move. Since `Receiver` is not `Clone`, it needs `RefCell` or similar wrapping to be used from `FnMut`. Actually, looking at the existing code: `create_gpu_resources` is called once during `RenderingSetup`, and the receiver ends up inside `GpuResources`. We can wrap the receiver in an `Option` and `.take()` it during setup. Use `std::cell::RefCell<Option<mpsc::Receiver<...>>>` in the closure.
  4. `create_gpu_resources` gains `texture_tx: mpsc::Sender<...>, texture_rx: mpsc::Receiver<...>` and uses them instead of creating a new channel.
  5. `main.rs`: `cloud_fetcher::spawn_cloud_fetcher(texture_tx.clone(), window.as_weak(), CLOUDS_SLOT);`

  The `texture_tx` for existing texture loading threads is cloned from the same sender inside `create_gpu_resources`, so all senders (texture loaders and cloud fetcher) feed the same receiver.

  This is the most involved refactoring step in the plan. The `mpsc::Receiver` is not `Sync`, but it only needs to be `Send` to cross the thread boundary into the closure, and then it lives in the thread-local `GpuResources`. The closure is `FnMut + 'static`, and `Receiver` is `Send`, so wrapping in `RefCell<Option<...>>` and taking during setup should work.

  Actually, looking more carefully at the existing code: the rendering notifier closure already captures `window_weak`, `aa_counts`, `texture_paths`, and `cloud_path` by move. Adding `texture_tx` (which is `Clone + Send`) and wrapping `texture_rx` is straightforward. Use `let texture_rx = std::cell::RefCell::new(Some(texture_rx));` before the `move` closure, and inside `RenderingSetup`, do `texture_rx.borrow_mut().take().expect("...")`.
- **Test cases**: No new unit tests (this is a refactor of plumbing). Verified by:
  - Existing texture loading still works (Day/Night textures load in background)
  - Cloud fetcher can send messages through the shared channel
- **Verify**: `cargo build` succeeds. `cargo test` passes. Manual: textures still load.
- **Complexity**: Medium

### Phase 6: Integration and Manual Testing

#### Step 6.1: Verify full pipeline with live network

- **Files**: N/A (manual verification)
- **Action**: Run the app with network access and no cached cloud image. Verify:
  - Clouds appear after initial download (may take a few seconds on first run)
  - `%LOCALAPPDATA%\SunlitEarth\clouds_cache.jpg` is created
  - `%LOCALAPPDATA%\SunlitEarth\clouds_cache_meta.toml` is created with an ETag
  - Console shows download timing message
- **Manual test cases**:
  - First run: clouds appear after download, cache files created
  - Second run: clouds appear immediately (from cache)
  - Moving Cloud Floor slider: thin clouds disappear progressively
  - Moving Cloud Gamma slider: cloud contrast changes in real time
  - Moving Cloud Opacity slider: clouds fade in/out (existing behavior preserved)
  - Set floor=0.0 gamma=1.0: identical to old behavior (identity)
  - Disconnect network, restart app: cached clouds appear, no crash or hang
  - Disconnect network, wait 60+ min: app continues running, no crash (just a logged warning)
- **Verify**: All manual checks pass.
- **Complexity**: Medium

#### Step 6.2: Verify wallpaper export includes cloud adjustments

- **Files**: N/A (manual verification)
- **Action**: Set non-default cloud floor and gamma values, then click "Set as Wallpaper". Verify the exported wallpaper reflects the adjusted cloud contrast.
- **Manual test cases**:
  - Set cloud_floor=0.2, cloud_gamma=0.5, click Set as Wallpaper
  - Verify wallpaper shows adjusted cloud contrast matching the preview
- **Verify**: Wallpaper matches preview.
- **Complexity**: Small

#### Step 6.3: Run full test suite and clippy

- **Files**: N/A
- **Action**: Run `cargo test` and `cargo clippy`. Fix any failures or warnings.
- **Verify**: `cargo test` passes. `cargo clippy` is clean (no warnings).
- **Complexity**: Small

### Phase 7: Update CLAUDE.md

#### Step 7.1: Update CLAUDE.md to reflect new architecture

- **Files**: `CLAUDE.md`
- **Action**: Update the following sections:
  - **Key modules**: Add `cloud_fetcher.rs` description
  - **Architecture / Shader**: Note cloud floor/gamma uniforms in `fs_cloud`
  - **UI**: Update Clouds GroupBox description to mention Floor and Gamma sliders
  - **Dirty-checking**: Add `cloud_floor` and `cloud_gamma` to the field list
  - **Notable dependencies**: Add `ureq` for HTTP
  - **Uniform buffer**: Note that `_pad3`/`_pad4` are now `cloud_floor`/`cloud_gamma`
  - Remove any references to `cloud.png` static file loading
- **Verify**: CLAUDE.md accurately describes the new architecture.
- **Complexity**: Small

## Phasing and Dependencies

```text
Phase 1 (Uniform Layout) ─────────────┐
                                       ├──> Phase 5 (Channel Refactor) ──> Phase 6 (Integration)
Phase 2 (Cloud Fetcher Module) ────────┤                                         │
                                       │                                         v
Phase 3 (UI Sliders) ─────────────────┤                                   Phase 7 (CLAUDE.md)
                                       │
Phase 4 (Remove Static Loader) ───────┘
```

Phases 1, 2, 3 can be worked in parallel to a degree, but Phase 1 must complete before Phase 3 Step 3.5 (which wires the new fields through the renderer). Phase 4 and Phase 5 depend on Phase 2. Phase 6 depends on everything.

Recommended implementation order: Phase 1 -> Phase 2 -> Phase 3 -> Phase 4+5 (interleaved) -> Phase 6 -> Phase 7.

## Test Strategy

### Automated Tests

| Test Case | Type | Location | Input | Expected Output |
| --- | --- | --- | --- | --- |
| Uniform struct size | Compile-time | `src/renderer/uniforms.rs` | N/A | `size_of::<Uniforms>() == 144` |
| CacheMeta round-trip | Unit | `src/cloud_fetcher.rs` | Meta with etag+last_modified | Identical after save+load |
| CacheMeta missing file | Unit | `src/cloud_fetcher.rs` | Nonexistent path | `None` |
| CacheMeta empty fields | Unit | `src/cloud_fetcher.rs` | Both fields `None` | Round-trips correctly |
| cloud_floor quantization | Unit | `src/renderer/frame.rs` | 0.196 | 196 |
| cloud_gamma quantization | Unit | `src/renderer/frame.rs` | 0.3 | 300 |
| cloud_floor dirty trigger | Unit | `src/renderer/frame.rs` | Two states differing only in cloud_floor | `!=` |
| cloud_gamma dirty trigger | Unit | `src/renderer/frame.rs` | Two states differing only in cloud_gamma | `!=` |
| AppConfig cloud defaults | Unit | `src/config.rs` | `AppConfig::default()` | floor=0.0, gamma=1.0 |
| AppConfig serde round-trip | Unit | `src/config.rs` | Non-default floor/gamma | Preserved after serialize+deserialize |
| JPEG decode valid | Unit | `src/cloud_fetcher.rs` | Minimal valid JPEG bytes | `Ok(DecodedImage)` |
| JPEG decode invalid | Unit | `src/cloud_fetcher.rs` | `&[0, 1, 2, 3]` | `Err(...)` |
| Existing tests unbroken | All | `cargo test` | Full suite | All pass |

### Manual Verification

- [ ] App downloads cloud JPEG on first run (no cache)
- [ ] Cached image loads instantly on subsequent runs
- [ ] Cloud Floor slider removes thin clouds progressively
- [ ] Cloud Gamma slider adjusts midtone contrast
- [ ] Cloud Opacity slider fades clouds (unchanged from before)
- [ ] Floor=0 Gamma=1 is visually identical to current behavior
- [ ] App works offline with cached clouds
- [ ] App works offline with no cache (no clouds, no crash)
- [ ] Wallpaper export reflects cloud floor/gamma adjustments
- [ ] After 60+ minutes, app checks for updates (observe in console logs)
- [ ] "Reset All" resets floor and gamma to defaults

## Risks and Mitigations

| Risk | Impact | Mitigation |
| --- | --- | --- |
| ureq 3.x API differs from research | Build failure | Research was conducted against ureq source code; verified 304-as-Ok behavior. Pin `version = "3"` to avoid breaking changes in 4.x |
| 8K JPEG decode time blocks UI | Noticeable delay when cloud texture updates | Decode happens on background thread; UI thread only receives decoded pixels via channel. No blocking. |
| Network timeout stalls background thread | Thread blocks for default timeout (30s) | Configure `ureq::Agent` with a 30-second timeout. Thread is background-only; UI is never blocked. |
| Uniform layout change breaks GPU tests | Test failures in `tests/shading.rs`, `tests/render_pipeline.rs` | Fields are at the same offsets; only the names change. WGSL and Rust structs are updated together in Phase 1. |
| matteason service unavailable | No clouds displayed | Cache provides last-known image. No-cache + no-network = no clouds (graceful degradation). |
| Large `ureq` + `rustls` dependency tree | Increased compile time | `rustls` is well-maintained and avoids OpenSSL system dependency. Compile time increase is a one-time cost. |
| Channel refactor introduces subtle race | Texture messages lost | The `mpsc` channel is unbounded; messages cannot be lost unless the receiver is dropped. `process_decoded_textures` drains all pending messages each frame. |

## Rollback Strategy

All changes are on the `cloud-layer` worktree branch. If the implementation fails:

1. Abandon the branch: `git checkout main` from the main worktree
2. Delete the worktree: `git worktree remove .worktrees/cloud-layer`
3. The main branch remains untouched

Individual phases can be partially rolled back by reverting commits, since each phase produces a buildable state.

## Status

- [ ] Plan approved
- [ ] Implementation started
- [ ] Implementation complete
