# Research: Test Coverage Strategy (2026-03-15)

## Problem Statement

Sunlit Earth has 31 tests across 5 test locations (4 unit test modules, 1 integration test file), but 4 of 8 source modules have zero test coverage. The largest and most complex module (`renderer.rs`, 1104 lines) is entirely untested. Several pure functions in `renderer.rs` and `texture_loader.rs` are trivially testable but have no tests. The shader integration tests are excellent but cover only `blend_fragment()` from `blend.wgsl` -- the vertex transform, texture sampling, and single-texture mode sentinel in `sphere.wgsl` are untested. No Slint UI behavior is tested. No coverage tooling is in place.

The goal of this research is to determine the right testing strategy, tools, coverage targets, and prioritized action plan for closing these gaps in a desktop rendering application where significant surface area (GPU pipeline, UI event loop) is inherently hard to test.

## Current State

### Test Inventory

The project has 31 tests total:

| Location | Count | Quality |
|---|---|---|
| `src/camera.rs` | 5 | Good -- geometric properties, tolerances, clamping |
| `src/sphere.rs` | 6 | Good -- vertex count, index validity, unit sphere constraint |
| `src/grid_texture.rs` | 3 | Minimal -- output size, alpha, one pixel color |
| `src/sun.rs` | 5 | Good -- astronomically meaningful dates, unit vector invariant |
| `tests/shading.rs` | 12 | Excellent -- GPU compute tests on real WGSL, 18,000+ parameter combinations |

### Untested Modules

| Module | Lines | Barrier to Testing |
|---|---|---|
| `src/renderer.rs` | 1104 | Contains pure functions (`build_aa_options`, `downsample_2x`) mixed with GPU code |
| `src/texture_loader.rs` | ~100 | Contains pure functions (`shift_horizontal`) mixed with file I/O |
| `src/wgpu_init.rs` | ~80 | Adapter ranking logic is embedded in a closure |
| `src/main.rs` | ~200 | Entry point, inherently hard to unit test (expected) |

### Test Infrastructure Strengths

- GPU compute shader harness tests actual WGSL, eliminating drift between tested and production logic.
- `LazyLock<Mutex<GpuContext>>` pattern prevents per-test device creation crashes on Windows.
- Software adapter test (`software_adapter_produces_correct_results`) ensures CI works without a GPU.
- Compile-time `const _: () = assert!(size_of::<T>() == N)` assertions catch struct layout mismatches.
- Astronomy test helpers (`make_time`, `sun_dir_at`) enable deterministic date-based tests.

### Test Infrastructure Weaknesses

- No `[dev-dependencies]` section -- no test utility crates.
- No test data files, golden images, or snapshot tests.
- No benchmarks.
- No coverage tooling or CI coverage reporting.
- Float comparison uses raw epsilon arithmetic everywhere; no shared assertion helper.

## Findings

### Testing Strategy for Desktop Rendering Applications

Desktop rendering apps require a layered testing strategy because different layers have fundamentally different testability characteristics:

**Layer 1 -- Pure functions (unit tests, no barriers).** Camera math, sphere generation, sun direction, grid texture generation, mipmap downsampling, pixel buffer manipulation. These are fully testable with standard `#[cfg(test)]` modules. The existing tests at this layer are well-written.

**Layer 2 -- Shader logic (GPU compute tests).** The `blend_fragment()` function is thoroughly tested via the compute shader harness. This layer validates shader correctness without needing a full render pipeline.

**Layer 3 -- Render pipeline (GPU integration tests).** Vertex transform, texture sampling, uniform buffer layout, single-texture mode sentinel. Requires rendering to a texture, reading pixels back, and asserting behavioral invariants (not pixel-exact values, due to cross-hardware float variance).

**Layer 4 -- UI behavior (Slint testing backend).** Callback wiring, property bindings, slider-to-render state propagation. Requires `i-slint-backend-testing` but no GPU.

**Layer 5 -- Full application (manual testing).** Window management, wallpaper integration, end-to-end visual quality. Not automatable.

The primary gaps in Sunlit Earth are at layers 1 (pure functions in `renderer.rs` and `texture_loader.rs`), 3 (no render pipeline test), and 4 (no Slint UI tests).

### The Humble Object Pattern

The central strategy for making rendering code testable is the humble object pattern: extract all decision logic out of GPU/UI callbacks into plain Rust functions and structs, leaving the callback as a thin dispatcher.

Applied to `renderer.rs`: the dirty-checking, viewport quantization, state comparison, and MSAA rebuild logic are currently embedded in the `rendering_callback` function alongside GPU submission code. Extracting these into a `FrameDescriptor` struct with pure construction and comparison methods would make all that logic testable without a GPU:

```rust
#[derive(Debug, PartialEq)]
pub struct FrameDescriptor {
    pub viewport: (u32, u32),
    pub mvp: [f32; 16],
    pub sun_dir: [f32; 3],
    pub terminator_width: f32,
    pub sample_count: u32,
    pub texture_index: usize,
    pub diffuse_shading: bool,
}
```

The rendering callback then: (1) constructs a `FrameDescriptor` from UI state (pure, testable), (2) compares it to the previous frame's descriptor (pure, testable), (3) if changed, submits GPU commands using the descriptor's values.

**Confidence:** High. This is a well-established pattern (Martin Fowler's "Humble Object") used across game engines and rendering applications.

### Rust Testing Best Practices

**Test organization.** The canonical approach (`#[cfg(test)] mod tests` at the bottom of each file) is already used correctly. For integration tests, shared helpers should go in `tests/common/mod.rs` (not `tests/common.rs`, which Cargo treats as a separate test target).

**Naming.** The project already follows Rust convention: snake_case names that read as sentences, no `test_` prefix. Good examples from the codebase: `eye_at_zero_longitude_zero_latitude`, `fully_lit_matches_day_color`.

**Unit vs. integration tests.** Pure functions and private helpers get unit tests. Cross-module interactions, GPU shader execution, and UI callbacks get integration tests. This maps cleanly to the layered strategy above.

**Mocking.** `wgpu::Device` and `wgpu::Queue` are concrete types with no trait abstraction, making them inherently unmockable. The GPU integration test approach (running on a real or software adapter) is the practical alternative. For non-GPU code, trait-based injection via generics is the most appropriate Rust mechanism -- lightweight, zero-cost, and compatible with the existing architecture. Mockall would only be needed if new trait-based abstractions are introduced.

**Property-based testing.** Proptest is the leading library for Rust. Good candidates in Sunlit Earth:
- `shift_horizontal()`: invertibility (shift by N then shift by width-N returns original), idempotency at shift=0 and shift=width
- `downsample_2x()`: output size formula always holds, all output pixels in valid RGBA range
- `build_aa_options()`: returned counts are a subset of input, default index is always in bounds

**Snapshot testing.** Insta captures output to `.snap` files and provides `cargo insta review` for interactive approval. Useful for: grid texture pixel output at small resolution, concatenated WGSL shader source, sphere vertex/index buffers. The `.snap` files must be checked into the repository.

### Testing wgpu and GPU Pipelines

**Windowless rendering.** Request an adapter with `compatible_surface: None`, render to a texture with `RENDER_ATTACHMENT | COPY_SRC`, copy to a `MAP_READ` buffer, and read back pixels. The production code already renders to an offscreen texture -- adding `COPY_SRC` to a test configuration is the only change needed.

**Software adapters for CI.** `force_fallback_adapter: true` selects WARP on Windows (works without extra setup) or LavaPipe on Linux (requires Mesa). The existing `software_adapter_produces_correct_results` test validates this path. Pin to the Ubuntu LTS mesa package in CI, not rolling PPAs, to avoid LavaPipe stability issues.

**The Noop backend.** wgpu's `Backends::NOOP` accepts API calls but performs no computation. Useful for testing resource management logic without any GPU or software renderer. Not useful for shader correctness.

**Cross-hardware tolerance.** Never do pixel-exact comparisons across different GPU adapters. Software adapters produce slightly different floating-point results from hardware GPUs. Use behavioral invariants (monotonicity, boundary conditions, per-channel floors) or a per-pixel threshold (max difference of 2/255 per channel, less than 0.1% of pixels differ).

**Full render pipeline test (currently missing).** The highest-value integration test gap: `sphere.wgsl`'s vertex transform and single-texture mode sentinel (`terminator_width < 0`) are completely untested. A test rendering a small frame (64x64 or 128x128) to an offscreen texture and asserting behavioral invariants (sphere visible, day side brighter than night side, no uninitialized fragments) would close this gap.

### Testing Slint UI

Slint provides `i-slint-backend-testing` with three initialization modes:
- `init_no_event_loop()` -- for property/state tests, no async
- `init_integration_test_with_mock_time()` -- for animations/timers, controlled time
- `init_integration_test_with_system_time()` -- for thread-spawning tests

**Can test:** callback invocations, property updates, timer advancement, element visibility.
**Cannot test:** actual pixel rendering, wgpu output, platform window chrome.

**Critical constraint:** The testing backend can only be initialized once per process. Integration tests involving the event loop must be grouped in a single `#[test]` function or use separate test binaries.

**Version pinning:** The crate is internal and does not follow semver. It must be pinned to the exact Slint version (`~1.15`).

**Open question:** Whether the Slint testing backend and a wgpu render pipeline can coexist in the same process has not been verified. May require separate test binaries.

### Coverage Tools

| Tool | Platform | Method | Branch Coverage | Recommendation |
|---|---|---|---|---|
| `cargo-llvm-cov` | Linux, macOS, Windows | LLVM instrumentation | Yes (experimental) | **Recommended** |
| `cargo-tarpaulin` | Linux only | LLVM or ptrace | No | Not viable (no Windows) |
| `grcov` | Cross-platform | LLVM / gcov aggregation | Limited | Unneeded complexity |

**cargo-llvm-cov** is the right choice. It works on Windows (critical for this project), supports `cargo-nextest`, and produces line, region, and branch coverage. The `--no-report` / `report` split allows merging coverage from unit tests and GPU integration tests that need different environment variables.

CI setup:

```yaml
- name: Install cargo-llvm-cov
  uses: taiki-e/install-action@cargo-llvm-cov

- name: Coverage (unit tests)
  run: cargo llvm-cov --no-report --lib

- name: Coverage (integration tests)
  run: cargo llvm-cov --no-report --test shading

- name: Generate coverage report
  run: cargo llvm-cov report --lcov --output-path lcov.info
```

### Coverage Targets

For a desktop rendering application, coverage targets must account for the inherent untestability of the GPU pipeline and UI event loop. A tiered approach:

| Layer | Target | Rationale |
|---|---|---|
| Pure functions (camera, sphere, grid, math helpers) | 90-100% | No barriers to testing; regressions are silent |
| Business logic (sun, dirty-checking, AA options, shift) | 80-90% | Some setup cost but high value |
| GPU pipeline (renderer.rs core loop) | 20-40% | Only GPU integration tests are practical |
| Main / event handlers | <20% | Inherently UI-driven, test manually |

An **overall target of 60-70%** is realistic. Enforcing 80%+ (the standard for a pure library crate) against a binary with significant GPU and UI surface area would produce coverage inflation through superficial tests rather than meaningful quality improvement.

**Branch coverage** is more valuable than line coverage for the dirty-checking logic, where many `if old != new` conditions exist. Line coverage could show 100% if every condition is always true in tests.

### Additional Tools

**cargo-nextest.** Faster test runner with per-process isolation. Directly compatible with the `LazyLock<Mutex<GpuContext>>` pattern -- each test gets its own process, so the static is freshly initialized. `cargo nextest run` is a drop-in replacement for `cargo test`.

**cargo-mutants.** Mutation testing: modifies source code and checks whether tests catch the change. Most useful for validating test quality on `camera.rs`, `sphere.rs`, and `sun.rs` before expanding coverage elsewhere.

**approx.** Replaces the recurring `assert!((got - expected).abs() < EPS)` pattern with `assert_relative_eq!(got, expected, epsilon = 1e-5)`. Worth adding immediately.

## Technical Constraints

1. **Windows GPU device creation.** Multiple simultaneous `wgpu::Device` creation calls from parallel test threads crash on Windows. All GPU tests must share a single device via `LazyLock<Mutex<GpuContext>>`.

2. **Slint one-initialization-per-process.** The testing backend can only be initialized once. Slint UI tests that need the event loop must be consolidated or use separate test binaries.

3. **`unsafe_code = "deny"`.** Test code must also obey this. GPU integration tests that use `bytemuck::cast_slice` or similar do not require unsafe because bytemuck provides safe wrappers for `#[repr(C)]` types with `Pod + Zeroable`.

4. **Cross-platform float variance.** Shader outputs differ by up to 1-2/255 per channel between hardware GPUs, WARP, and LavaPipe. All GPU assertions must use behavioral invariants or per-pixel thresholds, not exact equality.

5. **LavaPipe CI stability.** Pin to Ubuntu LTS mesa packages, not rolling PPAs. GitHub Actions `ubuntu-24.04` includes LavaPipe by default.

6. **Slint `i-slint-backend-testing` versioning.** Internal crate, no semver guarantees. Must be pinned to the exact Slint version.

## Recommendations

Ordered by impact-to-effort ratio:

### Immediate (No Refactoring Required)

1. **Install cargo-llvm-cov and measure current coverage.** This provides immediate visibility into gaps and establishes a baseline before adding tests. Use `cargo llvm-cov --html` for a local report.

2. **Test `renderer::build_aa_options()`.** Pure function mapping sample counts to UI labels and default index. Test cases: empty input, `[1]` only, `[1, 2, 4, 8]`, `[1, 2, 4]` (no 8x), unsorted counts.

3. **Test `renderer::downsample_2x()`.** Pure box-filter mipmap function. Test cases: 2x2 uniform image stays same color, 4x4 checkerboard averages correctly, single-pixel image.

4. **Test `texture_loader::shift_horizontal()`.** Pure pixel buffer rotation. Test cases: 4-pixel-wide 1-row buffer shifted by 1, shift by 0 is identity, shift by width is identity, double-shift returns to original.

5. **Add `approx` to `[dev-dependencies]`** and replace raw epsilon comparisons in existing tests with `assert_relative_eq!`.

### Short-term (Minor Refactoring)

6. **Extract `quantized_viewport_size()` from `renderer.rs` as a free function.** Test the 64px quantization boundary: 0 maps to 64, 63 maps to 64, 64 maps to 64, 65 maps to 128, 0x0 edge case.

7. **Extract adapter ranking from `wgpu_init::select_adapter()`.** The `DiscreteGpu > IntegratedGpu > VirtualGpu > Other > Cpu` comparison is embedded in a closure. Extract as `pub(crate) fn adapter_rank(kind: DeviceType) -> u32` and test the ordering.

8. **Add proptest for `shift_horizontal` and `downsample_2x`.** The invertibility property (`shift(shift(buf, n), width - n) == buf`) and size invariant (`output.len() == width/2 * height/2 * 4`) are natural property-based tests.

9. **Deepen `grid_texture` coverage.** Sample pixels at known grid-line positions (every 15 degrees) and assert grid color; sample between lines and assert base color. Test boundary behavior at texture edges.

### Medium-term (Integration Tests)

10. **Create a full render pipeline test.** Render a small frame (64x64) using the production vertex + fragment shaders on the software adapter. Assert behavioral invariants: pixels are not all black, day side is brighter than night side at a known sun direction, no uninitialized fragments.

11. **Test the single-texture mode sentinel.** Render with `terminator_width = -1.0` and verify the single-texture code path in `sphere.wgsl` produces the expected output.

12. **Test uniform buffer field offsets.** A compute shader test that reads specific fields from a known `Uniforms` buffer and asserts their values would catch padding mistakes that the compile-time size assertion cannot detect.

### Longer-term (UI and Visual Regression)

13. **Add Slint UI tests.** Use `i-slint-backend-testing` to verify: combobox selection fires correct callback, terminator slider visibility changes with texture mode, MSAA combobox labels match the `build_aa_options` output.

14. **Add golden image tests.** Generate 128x128 reference images with the software adapter, store in `tests/fixtures/golden/`, compare using `image-compare`'s SSIM at a threshold of 0.97. Only compare software-adapter outputs against software-adapter golden files.

### Recommended `[dev-dependencies]`

```toml
[dev-dependencies]
approx = "0.5"          # assert_relative_eq! for float comparisons
proptest = "1"           # property-based testing for pure functions
tempfile = "3"           # temp directories for texture_loader tests
insta = "1"              # snapshot testing for grid texture, WGSL source
# When Slint UI tests are added:
# i-slint-backend-testing = "~1.15"  # must match slint version exactly
# When visual regression tests are added:
# image-compare = "0.4"  # SSIM-based golden image comparison
```

## Open Questions

1. **Slint + wgpu coexistence in tests.** Can the Slint testing backend and a wgpu render pipeline coexist in the same process? If not, Slint UI tests and render pipeline tests must be in separate test binaries.

2. **Exact `i-slint-backend-testing` version.** The version string for Slint 1.15 needs confirmation from Slint's release notes or `Cargo.lock` before adding as a dev-dependency.

3. **LavaPipe on GitHub Actions.** Whether `ubuntu-latest` images include LavaPipe out of the box needs empirical validation. WARP on Windows is confirmed to work.

4. **Branch coverage stability.** cargo-llvm-cov's branch coverage is marked experimental. Verify it produces stable results before gating CI on branch coverage thresholds.

## Sources

| Document | Focus Area |
|---|---|
| `docs/plans/2026-03-15-test-coverage-codebase.md` | Current test inventory, module-by-module assessment, identified gaps, test infrastructure |
| `docs/plans/2026-03-15-test-coverage-external.md` | Rust testing practices, coverage tools, GPU testing, Slint testing, FFI testing, coverage standards |
| `docs/plans/2026-03-15-test-coverage-desktop-testing.md` | Humble object pattern, wgpu render pipeline testing, Slint UI testing, visual regression, refactoring priorities |
