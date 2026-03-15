# Desktop Rendering App Testing: Research Findings (2026-03-15)

## Executive Summary

Testing desktop rendering applications requires a layered strategy: pure-logic unit tests form the base, GPU compute shader tests verify shader correctness without a full pipeline, render-to-texture integration tests catch full-pipeline regressions, and Slint's testing backend enables headless UI verification. Sunlit Earth already has a strong foundation in layers one and two. The primary gaps are (a) several pure functions in `renderer.rs` and `texture_loader.rs` that have no tests despite being trivially testable, and (b) the absence of any visual regression or render pipeline integration test. The Slint UI is currently untouched by any automated test.

---

## 1. Architecture for Testability in Rendering Applications

### The Humble Object Pattern

The humble object pattern is the central idea behind making rendering code testable. The key insight: the GPU or UI framework is the "hard-to-test" dependency. Extract all decision logic out of callbacks and into plain Rust functions and structs, leaving the rendering callback as a thin shell that merely dispatches pre-computed values to the GPU.

Applied to Sunlit Earth, `renderer.rs` has a 1104-line `rendering_callback` function that mixes logic (dirty-checking, quantization, state comparison) with GPU submission. The humble object strategy says: extract the logic into a testable struct, leave only the wgpu calls in the callback.

**Before (current):** all state comparison, quantization, and dirty-check logic lives directly inside the `RenderingSetup`/`BeforeRendering` match arms, where it cannot be called without a live Slint event loop and GPU device.

**After (humble object approach):**

```rust
/// Pure, GPU-free state that controls what a frame should render.
/// No wgpu types, no Slint types — fully constructable in a unit test.
#[derive(Debug, PartialEq)]
pub struct FrameDescriptor {
    pub viewport: (u32, u32),    // quantized to 64px
    pub mvp: [f32; 16],
    pub sun_dir: [f32; 3],
    pub terminator_width: f32,
    pub sample_count: u32,
    pub texture_index: usize,
    pub diffuse_shading: bool,
}

impl FrameDescriptor {
    /// Construct from raw UI values. Pure function, no GPU required.
    pub fn from_ui_state(
        raw_width: u32, raw_height: u32,
        lon: f32, lat: f32, zoom: f32,
        sun_dir: [f32; 3],
        terminator_width: f32,
        sample_count: u32,
        texture_index: usize,
        diffuse_shading: bool,
    ) -> Self { ... }
}
```

The rendering callback then:
1. Calls `FrameDescriptor::from_ui_state(...)` — pure, testable
2. Compares the new descriptor to the last rendered one — pure, testable
3. If changed, submits GPU commands using the descriptor's values

This gives you unit tests for dirty-checking and quantization with zero GPU involvement.

### Dependency Injection in Rust

Rust provides three mechanisms for injecting test doubles, in order of invasiveness:

**1. Generics (zero-cost, compile-time):** Best when the abstraction has a small, stable interface.

```rust
trait SunProvider {
    fn sun_direction(&self) -> glam::Vec3;
}

struct LiveSunProvider;
impl SunProvider for LiveSunProvider {
    fn sun_direction(&self) -> glam::Vec3 { sun::sun_direction_now() }
}

// In tests:
struct FixedSunProvider(glam::Vec3);
impl SunProvider for FixedSunProvider {
    fn sun_direction(&self) -> glam::Vec3 { self.0 }
}
```

**2. `cfg(test)` type substitution:** For simple cases where one mock covers all tests. Avoids trait overhead entirely.

```rust
#[cfg(test)]
use mock_clock::MockClock as Clock;
#[cfg(not(test))]
use std::time::SystemTime as Clock;
```

Drawback: all tests share one mock implementation — no per-test customization.

**3. Trait objects (`Box<dyn Trait>` / `Arc<dyn Trait>`):** For runtime pluggability. Adds a heap allocation and vtable indirection, but enables mock injection in integration tests without recompilation.

For Sunlit Earth, option 1 (generics) is most appropriate. The codebase has no existing DI infrastructure, and lightweight generic parameters on a few helper functions would cover the main testability gaps without architectural upheaval.

### Render-to-Texture Pipeline Design

The render-to-texture architecture Sunlit Earth already uses (offscreen wgpu texture → `Image::try_from(Texture)` → Slint) is actually well-suited for testing. The pipeline already terminates at a plain pixel buffer. The key insight from wgpu documentation: rendering to a texture and reading it back requires only that the texture be created with `COPY_SRC` alongside `RENDER_ATTACHMENT`:

```rust
let texture = device.create_texture(&wgpu::TextureDescriptor {
    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
    // ...
});
// after rendering:
encoder.copy_texture_to_buffer(
    wgpu::TexelCopyTextureInfo { texture: &texture, .. },
    wgpu::TexelCopyBufferInfo { buffer: &staging_buf, .. },
    extent,
);
queue.submit([encoder.finish()]);
device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
let mapped = staging_buf.slice(..).get_mapped_range();
// pixel bytes available here
```

The production texture already has `RENDER_ATTACHMENT` — adding `COPY_SRC` to a test configuration is the only change needed to enable pixel readback in tests.

---

## 2. Testing wgpu Applications

### What wgpu Itself Does

The `wgpu` project uses a "defense in depth" model:
- `wgpu_test` crate with a `#[gpu_test]` attribute macro that runs a test on all GPU adapters found on the system
- `.gpuconfig` file generated in advance so adapter enumeration does not happen at test collection time (important: device creation during test enumeration crashes on Windows and slows test runs everywhere)
- A separate software-adapter test path for CI environments without hardware

The key design decision documented by the wgpu maintainers: **do not create GPU devices during test enumeration**. Sunlit Earth's `tests/shading.rs` already follows this correctly by using `LazyLock<Mutex<GpuContext>>` — the device is created on first use, not when the test binary initializes.

### Compute Shader Testing Strategy

Sunlit Earth already implements the optimal strategy for shader logic testing: upload test cases as storage buffers, dispatch a compute shader wrapping the production shader function, and read back results. This is superior to mirroring shader logic in Rust because:

- Tests cannot drift out of sync with the shader
- Floating-point behavior matches the production code exactly
- The same test cases run against both hardware and the software adapter

The `rust-gpu` project uses a complementary approach they call "differential testing": run Rust and WGSL shaders side-by-side and compare outputs. This is worth considering for future shaders where a Rust reference implementation is natural to write.

### GPU Device Sharing Pattern

The pattern used in `tests/shading.rs` is the correct one for Windows:

```rust
static GPU: LazyLock<Mutex<GpuContext>> = LazyLock::new(|| {
    Mutex::new(create_gpu_context(false))
});
```

The `Mutex` serializes GPU command submissions so parallel `cargo test` threads do not race on the device. `LazyLock` ensures exactly one device creation event. This matches what the wgpu project documents as the reason for crashes on Windows: multiple simultaneous device creation calls from parallel test threads.

### Software Adapter for CI

`RequestAdapterOptions { force_fallback_adapter: true, .. }` selects the software (CPU) adapter. On Windows, this routes through DX12's software rasterizer. On Linux, it requires LavaPipe (Mesa) or SwiftShader to be installed. The wgpu-py project documents this directly: "On Windows this (probably) just works via DX12."

The `software_adapter_produces_correct_results` test in `tests/shading.rs` already validates this path. The important constraint from cross-platform experience: **do not do pixel-exact comparisons across software adapters and hardware**. Software adapters produce slightly different floating-point results. Use behavioral invariants (monotonicity, boundary conditions, per-channel floors) rather than exact pixel equality.

### Full Render Pipeline Test (Currently Missing)

There is currently no test that exercises `sphere.wgsl`'s vertex and fragment shaders. A full pipeline test would:

1. Create a wgpu device (using the shared `LazyLock` pattern)
2. Build the full render pipeline (vertex + fragment shaders, bind groups, MSAA resolve)
3. Render a small frame (e.g., 64×64 pixels) to an offscreen texture
4. Copy the texture to a staging buffer and read back pixels
5. Assert behavioral invariants: sum of pixels > 0 (sphere is visible), no pure magenta pixels (no uninitialized fragments), day-side pixels are brighter than night-side pixels at a known sun direction

This does not require golden images. Behavioral assertions are robust across hardware.

---

## 3. Testing Slint UI Applications

### Slint's Testing Backend

Slint provides `i-slint-backend-testing`, an official testing backend that simulates a windowing system without rendering any pixels. Key properties:
- No display required — works in headless CI environments
- Text is measured by fixed font sizes, not system fonts
- Three initialization modes:
  - `init_no_event_loop()` — for property/state tests, no async, system time mocked
  - `init_integration_test_with_mock_time()` — for tests involving animations/timers; use `mock_elapsed_time()` to advance time
  - `init_integration_test_with_system_time()` — for tests that spawn threads and use `invoke_from_event_loop()`

**Critical warning from Slint docs:** `i-slint-backend-testing` is an internal crate. It does not follow semver. It must be pinned to an exact version matching the `slint` crate version. In Sunlit Earth's `Cargo.toml` this means adding it as a dev-dependency pinned to exactly `~1.15` (matching the production dependency).

### What Can Be Tested

The `ElementHandle` API provides:
- `find_by_accessible_label(label)` — locate elements by accessibility label
- `find_by_element_id(component, id)` — locate by element ID
- `invoke_accessible_default_action()` — trigger button clicks etc.
- `single_click()` / `double_click()` — simulate mouse input (async)

This enables testing:
- ComboBox selection changes trigger the correct Rust callback
- Slider movements update property bindings
- The texture combobox enables/disables the terminator width slider when switching between Day and Day/Night Blend modes

### What Cannot Be Tested with the Testing Backend

The testing backend does not render pixels. Tests cannot verify visual output — only state and callbacks. For visual output, the render-to-texture pipeline must be tested separately through the wgpu layer (see section 2).

### Practical Slint Test Setup

```rust
// In Cargo.toml [dev-dependencies]:
// i-slint-backend-testing = { version = "=1.15.x", features = ["testing"] }

#[cfg(test)]
mod slint_tests {
    use i_slint_backend_testing as slint_testing;

    #[test]
    fn terminator_slider_hidden_in_single_texture_mode() {
        slint_testing::init_no_event_loop();
        let window = crate::MainWindow::new().unwrap();
        // Set combobox to "Day" (single texture mode)
        // Assert that the terminator width slider is not visible
        // (or has opacity 0, depending on the .slint implementation)
    }
}
```

The key limitation: because `renderer.rs` registers the rendering notifier in `main.rs` and requires a live GPU, UI-only tests should use a variant of the window that does not register the rendering notifier. This may require extracting the window setup into a testable factory function.

---

## 4. Visual Regression and Golden File Testing

### When to Use Visual Regression Testing

Visual regression tests are appropriate when:
- The rendered output is the primary deliverable (it is, for a wallpaper app)
- You have a stable reference render to compare against
- The comparison can tolerate hardware-specific floating-point variance

They are inappropriate as the primary testing strategy because:
- GPU float variance between hardware means the same code produces different pixel values on different machines
- Software adapters produce different outputs than hardware adapters
- Every intentional visual change requires updating golden files

For Sunlit Earth, behavioral invariant tests (see section 2) should be the primary strategy. Visual regression tests are a secondary layer.

### Image Comparison Crates for Rust

Two well-maintained options:

**`image-compare` crate**: Provides SSIM (Structural Similarity Index) and RMS comparison. Returns a score from 0.0 (dissimilar) to 1.0 (identical), plus a per-pixel difference map. Suitable for asserting that two renders of the same scene are "95% similar."

**`dssim-core` crate**: Multi-scale SSIM using L\*a\*b\* color space, which matches human perception better than RGB-based SSIM. More accurate for perceptual comparison but slower.

For CI golden tests without access to the same GPU, generate reference images with the software adapter (LavaPipe/WARP) and compare only software-adapter outputs against software-adapter golden files. Never compare software-adapter output to hardware-GPU golden files.

### Tolerance Strategy

Two complementary approaches:

**Pixel-level tolerance**: Assert that no more than N% of pixels differ by more than threshold T. Example: `assert!(mismatch_fraction < 0.01)` with per-pixel tolerance of 2/255 per channel. This catches large errors (sphere disappeared, wrong color) while ignoring sub-pixel anti-aliasing differences.

**Structural similarity (SSIM)**: More robust to small positional offsets and blur. A threshold of 0.95 SSIM is typical for rendered output comparison.

### Golden File Workflow

1. Generate golden files with a known-good commit using the software adapter
2. Store them in `tests/fixtures/golden/` (small resolution, e.g., 128×128)
3. In CI, render the same scene with the same software adapter and compare
4. When intentionally changing the renderer, update golden files with a dedicated `cargo test -- --update-golden` flag or a separate binary

### CI Considerations

GitHub Actions standard runners (ubuntu-latest, windows-latest) do not have GPUs. For wgpu tests:
- **Windows**: DX12 software adapter works without any extra setup (`force_fallback_adapter: true`)
- **Linux**: Requires LavaPipe (`apt install mesa-vulkan-drivers`) or a step to install SwiftShader
- **Conditional skip**: Gate GPU-dependent tests behind an environment variable (`RUN_GPU_TESTS=1`) so that CI without software adapter setup does not fail

The existing `software_adapter_produces_correct_results` test in `tests/shading.rs` already demonstrates the correct pattern for CI compatibility.

---

## 5. Refactoring for Testability: Priority Ordering

### Principle: Highest Return for Least Churn

When adding tests to an existing codebase, start with pure functions that have no external dependencies. They require no refactoring — just write the test. Then move to functions that can be made testable with small, localized extractions.

### Priority 1: Pure Functions (No Refactoring Required)

These functions exist today, are pure, and have zero tests. Add tests directly:

**`renderer::build_aa_options(&[u32]) -> (Vec<SharedString>, Vec<u32>, i32)`**
Tests needed: empty input, only `[1]`, `[1, 2, 4, 8]`, `[1, 2, 4]` (no 8x), unsorted input.

**`renderer::downsample_2x(pixels, width, height) -> Vec<u8>`**
Tests needed: 2×2 uniform image stays same color, 4×4 checkerboard averages correctly, single-pixel image. This function is called in every texture upload path — an off-by-one could corrupt every mipmap.

**`texture_loader::shift_horizontal(pixels, width, height)`**
Tests needed: shift of width=4 by 1 quarter moves content correctly, shift of a known pattern produces known output, double-shift returns to original.

**`grid_texture::lerp_u8`** (currently private helper)
If exported as `pub(crate)`, can be tested directly. Or test through `generate()` with a pixel sampled at a known non-grid latitude.

**`wgpu_init::select_adapter` ranking closure**
The actual ranking comparison (`DiscreteGpu` > `IntegratedGpu` > ...) is embedded inside a closure passed to `sort_by_key`. Extract it as a standalone `fn adapter_rank(kind: DeviceType) -> u32 -> u32` and test the ordering directly.

### Priority 2: Extractable Logic (Minimal Refactoring)

**`renderer` dirty-checking logic**

Currently the dirty check is a large conditional in `BeforeRendering`. Extract into:

```rust
fn frame_changed(prev: &FrameDescriptor, next: &FrameDescriptor) -> bool
```

This is a one-line refactor (move the `if prev == next { return; }` check to operate on structs) that enables testing all dirty-check permutations without a GPU.

**`renderer` quantization logic**

`quantized_viewport_size(raw_w, raw_h) -> (u32, u32)` is currently inline. Extract as a free function. Tests: 0→64, 63→64, 64→64, 65→128, 0×0 edge case.

**`texture_loader::resolve_textures_dir()`**

Accepts optional path overrides and falls back to executable-relative directories. Can be tested by creating a `tempfile::TempDir`, setting environment variables via `std::env::set_var`, and asserting the correct directory is returned. Requires adding `tempfile` as a dev-dependency.

### Priority 3: Integration Tests (Require GPU)

**Full render pipeline test for `sphere.wgsl`**

Extend `tests/shading.rs` (or create `tests/render_pipeline.rs`) with a test that:
1. Uses the shared `LazyLock<Mutex<GpuContext>>` device
2. Builds the full render pipeline from `sphere.wgsl` + `blend.wgsl`
3. Renders a 128×128 frame to a `COPY_SRC` texture
4. Reads back pixels and asserts behavioral invariants

This is the highest-value gap currently: `sphere.wgsl`'s vertex transform and single-texture mode sentinel (`terminator_width < 0`) are completely untested.

**Uniform buffer layout test**

The `Uniforms` struct has a compile-time size assertion but no test that the field offsets match the WGSL layout. A compute shader test that reads specific fields from a known `Uniforms` buffer and asserts their values would catch padding mistakes that the size assertion cannot catch.

### Priority 4: Slint UI Tests (Require Backend Setup)

Add `i-slint-backend-testing` as a dev-dependency and write tests for:
- ComboBox index changes fire the correct callback and update internal state
- Terminator width slider is visible only in Day/Night Blend mode
- MSAA combobox labels match the counts array (the `build_aa_options` result)

These tests require extracting the window construction from `main.rs` into a factory function so tests can create a `MainWindow` without also starting the render pipeline.

---

## 6. Practical Recommendations Specific to Sunlit Earth

### Immediate Actions (No Refactoring)

1. Add `#[cfg(test)]` modules to `renderer.rs` with tests for `build_aa_options` and `downsample_2x`. These are self-contained — no GPU, no Slint, no external files.

2. Add a `#[cfg(test)]` module to `texture_loader.rs` with tests for `shift_horizontal`. Create a 4-pixel-wide, 1-row RGBA8 test buffer, shift by one quarter, assert the result.

3. Add tests to `grid_texture.rs` for the grid spacing property: sample pixels at known grid-line longitudes/latitudes and assert they are the grid color; sample pixels between lines and assert they are the base color.

4. Extract `adapter_rank` from `wgpu_init.rs` as a `pub(crate)` function and add an ordering test.

### Short-term (Minor Refactoring)

5. Extract `quantized_viewport_size` from `renderer.rs` as a free function and test the 64px quantization boundary.

6. Introduce a `FrameDescriptor` struct (or equivalent) to hold the inputs to a frame render. The dirty-check comparison then operates on this struct, enabling unit tests without GPU.

7. Add `tempfile` as a dev-dependency and test `resolve_textures_dir` with known temp directories.

### Medium-term (Integration Tests)

8. Create `tests/render_pipeline.rs` using the same `LazyLock<Mutex<GpuContext>>` pattern as `tests/shading.rs`. Build the full sphere pipeline and render a small frame. Assert that (a) pixels are not all black, (b) on the day side (sun_dir pointing toward camera) pixels are brighter than on the night side.

9. Test the single-texture mode sentinel: render with `terminator_width = -1.0` and assert behavior matches the single-texture code path in `sphere.wgsl`.

### Longer-term (Visual Regression)

10. Once the render pipeline test exists, consider adding a golden image at 128×128 rendered by the software adapter. Store it in `tests/fixtures/golden/`. Compare with `image-compare`'s SSIM at a threshold of 0.97.

### Dev-Dependencies to Add

```toml
[dev-dependencies]
tempfile = "3"                           # for texture_loader directory tests
approx = "0.5"                           # for assert_relative_eq! on f32
image-compare = "0.4"                    # for SSIM-based golden tests (when needed)
# i-slint-backend-testing pinned exactly to match slint version when Slint UI tests are added
```

`approx` is worth adding immediately — it replaces the recurring pattern of `assert!((got - expected).abs() < EPS, ...)` with the cleaner `assert_relative_eq!(got, expected, epsilon = 1e-5)`.

---

## Confidence Assessment

| Area | Confidence | Notes |
|---|---|---|
| Humble object / FrameDescriptor pattern | High | Well-established; directly applicable |
| Rust DI via generics | High | Standard Rust idiom |
| wgpu compute shader testing | High | Already working in the codebase |
| Software adapter on Windows CI | High | Documented by wgpu-py; confirmed by existing test |
| Slint testing backend | Medium | API is internal/unstable; exact version pinning required |
| Full render pipeline test via texture readback | High | Well-documented wgpu pattern |
| SSIM golden tests | Medium | Cross-platform float variance requires software-adapter-only baselines |
| Linux CI GPU testing | Medium | LavaPipe reliability varies; SwiftShader is slower but more portable |

## Knowledge Gaps

- The exact `i-slint-backend-testing` version string for Slint 1.15 was not confirmed. Check `slint`'s own `Cargo.lock` or release notes before adding as a dev-dependency.
- Whether Slint's testing backend can be initialized in the same process as a wgpu render pipeline (both may try to initialize graphics backends) has not been verified. May require separate test binaries.
- LavaPipe behavior on GitHub Actions `ubuntu-latest` images: some reports indicate it works out of the box on recent Ubuntu runner images; others require explicit installation. This should be validated empirically.

## Sources

Primary sources consulted:

- [Humble Object pattern — martinfowler.com](https://martinfowler.com/bliki/HumbleObject.html)
- [Humble Object explained — learning-notes.mistermicheels.com](https://learning-notes.mistermicheels.com/architecture-design/humble-object-pattern/)
- [wgpu_test crate documentation](https://wgpu.rs/doc/wgpu_test/index.html)
- [wgpu Testing Discussion #1611](https://github.com/gfx-rs/wgpu/discussions/1611)
- [wgpu Benchmark Device Creation Issue #6808](https://github.com/gfx-rs/wgpu/issues/6808)
- [Windowless wgpu rendering — Learn Wgpu](https://sotrh.github.io/learn-wgpu/showcase/windowless/)
- [i-slint-backend-testing — lib.rs](https://lib.rs/crates/i-slint-backend-testing)
- [Slint testing.md — github.com/slint-ui/slint](https://github.com/slint-ui/slint/blob/master/docs/testing.md)
- [image-compare crate — crates.io](https://crates.io/crates/image-compare)
- [dssim-core crate — crates.io](https://crates.io/crates/dssim-core)
- [Mocking in Rust with conditional compilation — klau.si](https://klau.si/blog/mocking-in-rust-with-conditional-compilation/)
- [Make Rust code testable with dependency inversion — worldwithouteng.com](https://worldwithouteng.com/articles/make-your-rust-code-unit-testable-with-dependency-inversion/)
- [wgpu-py: running without a GPU — wgpu-py docs](https://wgpu-py.readthedocs.io/en/stable/start.html)
- [rasterizers (LavaPipe/SwiftShader prebuilt for CI) — github.com/jakoch](https://github.com/jakoch/rasterizers)
- [Bevy automated testing — chadnauseam.com](https://chadnauseam.com/coding/gamedev/automated-testing-in-bevy/)
- [TDD in Bevy — edgardocarreras.com](https://edgardocarreras.com/blog/tdd-in-rust-game-engine-bevy/)
- [Rust traits and dependency injection — jmmv.dev](https://jmmv.dev/2022/04/rust-traits-and-dependency-injection.html)
- [wgpu multithreading discussion #4814](https://github.com/gfx-rs/wgpu/discussions/4814)
- [rust-gpu testing guide](https://rust-gpu.github.io/rust-gpu/book/testing.html)
