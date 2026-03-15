# Plan: Test Coverage and Renderer Refactoring (2026-03-15)

## Summary

Close test coverage gaps in Sunlit Earth by adding tests for four untested modules, splitting the monolithic `renderer.rs` into focused submodules, extracting decision logic from the rendering callback into pure testable functions, and establishing coverage tooling and testing standards. Organized into four incremental phases, each independently valuable with a passing test suite at every checkpoint.

## Stakes Classification

**Level**: Medium
**Rationale**: Changes span multiple source files and introduce new dev-dependencies, but each phase is isolated and reversible. No changes to observable production behavior. The existing 31 tests serve as a regression safety net throughout.

## Context

**Research**: `docs/plans/2026-03-15-test-coverage-research.md` (synthesized findings)
**Source research**: `docs/plans/2026-03-15-test-coverage-codebase.md`, `docs/plans/2026-03-15-test-coverage-external.md`, `docs/plans/2026-03-15-test-coverage-desktop-testing.md`

## Success Criteria

- [ ] cargo-llvm-cov installed and producing an HTML coverage report with baseline recorded
- [ ] `build_aa_options()`, `downsample_2x()`, and `shift_horizontal()` have unit tests
- [ ] `approx` crate used for float assertions in new and existing tests
- [ ] `quantize_to_granularity()` extracted from `renderer.rs` and tested
- [ ] `adapter_type_rank()` extracted from `wgpu_init.rs` and tested
- [ ] `grid_texture` coverage deepened with grid-line pixel sampling
- [ ] Proptest properties for `shift_horizontal` and `downsample_2x`
- [ ] `renderer.rs` split into `renderer/` submodule
- [ ] `build_frame_state()` extracted as a pure function and tested
- [ ] Dirty-check coverage verified for all render-affecting fields
- [ ] Full render pipeline integration test (sphere.wgsl vertex + fragment) passing
- [ ] Single-texture mode sentinel (`terminator_width < 0`) tested on GPU
- [ ] Uniform buffer field offset test catching padding mismatches
- [ ] Overall coverage at or above 60%
- [ ] `CLAUDE.md` updated with testing standards and coverage commands

## Implementation Steps

### Phase 1: Baseline Coverage and Easy Wins

Goal: Install tooling, measure current state, test pure functions that require zero refactoring.

#### Step 1.1: Add dev-dependencies to Cargo.toml

- **Files**: `Cargo.toml`
- **Action**: Add a `[dev-dependencies]` section:
  ```toml
  [dev-dependencies]
  approx = "0.5"
  proptest = "1"
  ```
- **Verify**: `cargo check --tests` compiles without errors
- **Complexity**: Small

#### Step 1.2: Install cargo-llvm-cov and measure baseline

- **Files**: None (tooling only)
- **Action**: Run `cargo install cargo-llvm-cov`, then `cargo llvm-cov --html` to generate a baseline report. Record the overall line coverage percentage in the commit message.
- **Verify**: `target/llvm-cov/html/index.html` exists and shows per-file coverage
- **Complexity**: Small

#### Step 1.3: Write unit tests for `build_aa_options`

- **Files**: `src/renderer.rs` (append `#[cfg(test)] mod tests` block)
- **Action**: Test `build_aa_options()`. This function is `pub`, pure, and takes `&[u32]` of supported sample counts. It always pushes "None"/1 first, then adds `"MSAA {n}\u{d7}"` for each count > 1. The default index targets 8x, falling back to the last entry.
- **Test cases**:
  - `&[1]` -> labels `["None"]`, counts `[1]`, default `0`
  - `&[1, 2, 4, 8]` -> labels `["None", "MSAA 2\u{d7}", "MSAA 4\u{d7}", "MSAA 8\u{d7}"]`, counts `[1, 2, 4, 8]`, default `3`
  - `&[1, 2, 4]` (no 8x) -> default `2` (last entry = highest available)
  - `&[1, 8]` (skip intermediates) -> labels `["None", "MSAA 8\u{d7}"]`, counts `[1, 8]`, default `1`
  - `&[]` (empty, only "None" added) -> labels `["None"]`, counts `[1]`, default `0`
- **Verify**: All tests pass
- **Complexity**: Small

#### Step 1.4: Write unit tests for `downsample_2x`

- **Files**: `src/renderer.rs` (append to test module)
- **Action**: Test `downsample_2x()`. This is a private function (accessible from the same file's `#[cfg(test)]` module) that does box-filter 2x downsampling of RGBA8 data with rounding: `(tl + tr + bl + br + 2) / 4`. Boundary pixels clamp neighbor coordinates to stay in bounds.
- **Test cases**:
  - 2x2 uniform red `[255,0,0,255]` x4 -> 1x1 `[255,0,0,255]`
  - 2x2 checkerboard (RGBA values `[0,0,0,255]`, `[100,0,0,255]`, `[0,100,0,255]`, `[0,0,100,255]`) -> 1x1 `[25,25,25,255]`
  - 4x4 uniform white -> 2x2 all `[255,255,255,255]`
  - Output length: for input `(w, h)`, output length is `max(w/2,1) * max(h/2,1) * 4`
- **Verify**: All tests pass with `assert_eq!` (integer arithmetic, no float tolerance)
- **Complexity**: Small

#### Step 1.5: Write unit tests for `shift_horizontal`

- **Files**: `src/texture_loader.rs` (add `#[cfg(test)] mod tests` block)
- **Action**: Test `shift_horizontal()`. This private function rotates each row right by `width * 3` bytes (i.e., 3/4 of the row width in pixels). It mutates the buffer in place.
- **Test cases**:
  - 4-pixel-wide, 1-row image with distinct pixels `[A, B, C, D]` -> after shift (rotate_right by 3 pixels), result is `[B, C, D, A]`
  - 4-pixel-wide, 2-row image -> verify each row is shifted independently
  - Uniform-color row -> shift produces same row (identity for uniform data)
  - 8-pixel-wide, 1-row image -> shift by 6 pixels wraps correctly
- **Verify**: All tests pass with `assert_eq!` on pixel byte arrays
- **Complexity**: Small

#### Step 1.6: Replace raw epsilon comparisons with `approx` in existing tests

- **Files**: `src/camera.rs`, `src/sun.rs`
- **Action**: Add `use approx::assert_relative_eq;` to each test module. Replace `assert!((val - expected).abs() < eps, ...)` patterns with `assert_relative_eq!(val, expected, epsilon = eps)`. Leave `tests/shading.rs` unchanged (its GPU tolerance pattern works well as-is).
- **Verify**: `cargo test camera sun` passes with clearer assertion messages
- **Complexity**: Small

**Phase 1 checkpoint**: `cargo test` passes (all 31 original + new tests). `cargo llvm-cov --html` shows coverage increase from baseline.

### Phase 2: Extract and Test Hidden Logic

Goal: Make currently-untestable logic testable through minor refactoring, add deeper tests and property-based tests.

#### Step 2.1: Extract `quantize_to_granularity` as a pure function

- **Files**: `src/renderer.rs`
- **Action**: Extract the quantization arithmetic from `quantized_viewport_size()` into a new `pub(crate)` free function:
  ```rust
  pub(crate) fn quantize_to_granularity(w: u32, h: u32) -> (u32, u32) {
      let qw = (w / SIZE_GRANULARITY).max(1) * SIZE_GRANULARITY;
      let qh = (h / SIZE_GRANULARITY).max(1) * SIZE_GRANULARITY;
      (qw, qh)
  }
  ```
  Update `quantized_viewport_size` to call this after reading from the Slint window.
- **Verify**: `cargo build` succeeds, behavior unchanged
- **Complexity**: Small

#### Step 2.2: Test `quantize_to_granularity`

- **Files**: `src/renderer.rs` (test module)
- **Test cases**:
  - `(0, 0)` -> `(64, 64)` (0/64=0, max(1)=1, *64=64)
  - `(63, 63)` -> `(64, 64)`
  - `(64, 64)` -> `(64, 64)` (exact boundary)
  - `(65, 65)` -> `(64, 64)` (65/64=1, *64=64)
  - `(128, 128)` -> `(128, 128)`
  - `(129, 200)` -> `(128, 192)`
  - `(1920, 1080)` -> `(1920, 1024)`
- **Verify**: All tests pass
- **Complexity**: Small

#### Step 2.3: Extract adapter ranking from `wgpu_init.rs`

- **Files**: `src/wgpu_init.rs`
- **Action**: Extract the `rank` closure inside `select_adapter()` into a standalone function:
  ```rust
  pub(crate) fn adapter_type_rank(device_type: wgpu::DeviceType) -> u32 {
      match device_type {
          wgpu::DeviceType::DiscreteGpu => 0,
          wgpu::DeviceType::IntegratedGpu => 1,
          wgpu::DeviceType::VirtualGpu => 2,
          wgpu::DeviceType::Other => 3,
          wgpu::DeviceType::Cpu => 4,
      }
  }
  ```
  Update `select_adapter` to use `adapter_type_rank(a.get_info().device_type)` in the `min_by_key`.
- **Verify**: `cargo build` succeeds, behavior unchanged
- **Complexity**: Small

#### Step 2.4: Test `adapter_type_rank`

- **Files**: `src/wgpu_init.rs` (add `#[cfg(test)] mod tests` block)
- **Test cases**:
  - `DiscreteGpu` ranks lower (better) than all other types
  - `Cpu` ranks highest (worst) of all types
  - Full ordering: `DiscreteGpu < IntegratedGpu < VirtualGpu < Other < Cpu`
- **Verify**: All tests pass
- **Complexity**: Small

#### Step 2.5: Deepen `grid_texture` test coverage

- **Files**: `src/grid_texture.rs` (existing test module)
- **Action**: Add tests sampling specific pixel positions to verify grid line placement.
- **Test cases** (at a resolution where pixel-to-degree mapping is exact, e.g., 360x180):
  - Equator pixel is `MAJOR_YELLOW` (major grid line)
  - Prime meridian pixel is `MAJOR_YELLOW`
  - 15-degree minor grid line pixel is `GRID_WHITE`
  - Pixel between grid lines is base color (ocean or land)
- **Verify**: Tests pass with exact pixel color assertions
- **Complexity**: Small

#### Step 2.6: Add proptest for `shift_horizontal`

- **Files**: `src/texture_loader.rs` (test module)
- **Action**: Add a proptest verifying the identity-after-four-shifts property. Since `shift_horizontal` always rotates by 3/4 width, calling it 4 times shifts by 3 full widths, which is equivalent to a full rotation back to the original.
- **Property**: For any `width` in `1..=64` and `height` in `1..=16`, and any RGBA pixel data of length `width * height * 4`: calling `shift_horizontal` four times returns the original buffer.
- **Verify**: `cargo test texture_loader` passes
- **Complexity**: Small

#### Step 2.7: Add proptest for `downsample_2x`

- **Files**: `src/renderer.rs` (test module)
- **Action**: Add a proptest verifying the output size invariant.
- **Property**: For any even `width` in `2..=128` and even `height` in `2..=128`, and any RGBA pixel data of the correct length: `downsample_2x` output has length `(width/2) * (height/2) * 4` and does not panic.
- **Verify**: `cargo test renderer` passes
- **Complexity**: Small

**Phase 2 checkpoint**: `cargo test` passes. `cargo llvm-cov --html` shows coverage increase. `renderer.rs`, `wgpu_init.rs`, and `texture_loader.rs` now show partial coverage.

### Phase 3: Renderer Module Split and Humble Object Refactoring

Goal: Split the 1104-line `renderer.rs` into focused submodules, then extract all decision logic from the rendering callback into pure, GPU-free functions that are testable without a Slint window or GPU device.

**Motivation**: `renderer.rs` mixes four concerns: GPU resource setup, texture loading/management, mipmap utilities, and the rendering callback with interleaved decision logic. Splitting along natural seams makes each file focused, imports cleaner for integration tests, and positions the codebase for unit-testing the rendering decisions.

#### Step 3.1: Split `renderer.rs` into `renderer/` submodule

- **Files**: Delete `src/renderer.rs`, create `src/renderer/mod.rs`, `src/renderer/gpu_setup.rs`, `src/renderer/textures.rs`, `src/renderer/uniforms.rs`
- **Action**: Convert into a directory module. This is a pure mechanical move with no logic changes.

  | File | Contents | Approx lines |
  |------|----------|--------------|
  | `mod.rs` | Public API (`setup_rendering_notifier`, `build_aa_options`), `rendering_callback`, `FrameState`, `lookup_sample_count`, `quantized_viewport_size`, `quantize_to_granularity`, constants, thread-local `GPU_RESOURCES` | ~350 |
  | `gpu_setup.rs` | `GpuResources` struct, `create_gpu_resources()`, `create_pipeline()`, `create_render_textures()`, `rebuild_msaa_resources()`, `rebuild_render_textures()`, `replace_render_textures()` | ~400 |
  | `textures.rs` | `TextureSlot`, `DecodedTextureMessage`, `process_decoded_textures()`, `maybe_create_composite_bind_group()`, `maybe_spawn_texture_load()`, `resolve_render_index()`, `create_mipmapped_texture()`, `create_bind_group()`, `upload_mip()`, `downsample_2x()` | ~300 |
  | `uniforms.rs` | `Uniforms` struct with `#[repr(C)]`, `size_of` compile-time assertion | ~30 |

  Cross-module visibility: functions called from `mod.rs` become `pub(super)` in their submodule. `Uniforms` becomes `pub(crate)` so integration tests can import it.

- **Verify**: `cargo build` succeeds. `cargo test` passes. `cargo clippy` clean. No changes to `use` statements outside `src/renderer/`. The public API (`renderer::setup_rendering_notifier`, `renderer::build_aa_options`) is unchanged.
- **Complexity**: Medium (mechanical but touches many lines; risk is misplaced imports, not logic errors)

#### Step 3.2: Relocate Phase 1/2 tests to their new homes

- **Files**: `src/renderer/mod.rs`, `src/renderer/textures.rs`
- **Action**: Move `#[cfg(test)] mod tests` blocks written in Phases 1 and 2:
  - `build_aa_options` tests and `quantize_to_granularity` tests -> `mod.rs`
  - `downsample_2x` tests (including proptest) -> `textures.rs`
- **Verify**: `cargo test` passes, all previously-written tests still run
- **Complexity**: Small

#### Step 3.3: Extract `build_frame_state()` as a pure function

- **Files**: `src/renderer/mod.rs`
- **Action**: Create a `pub(crate)` function that builds a `FrameState` from raw values (no Slint or wgpu types):
  ```rust
  pub(crate) fn build_frame_state(
      longitude: f32, latitude: f32, zoom: f32,
      sample_count: u32, texture_index: i32,
      render_width: u32, render_height: u32,
      sun_dir: glam::Vec3,
      terminator_width: f32, diffuse_shading: bool,
      diffuse_floor: f32, diffuse_ramp: f32,
  ) -> FrameState { ... }
  ```
  The quantization logic (multiplying by 1000 and casting to `i32`) moves inside this function. The rendering callback calls `build_frame_state(win.get_camera_longitude(), ..., res.sample_count, ...)` instead of constructing the struct inline. `FrameState` already derives `PartialEq`, so the dirty-check comparison (`res.last_state.as_ref() == Some(&current_state)`) continues to work unchanged.
- **Verify**: `cargo test` passes. `cargo clippy` clean. Visual spot-check that the app renders identically.
- **Complexity**: Small

#### Step 3.4: Test `build_frame_state` quantization and dirty-checking

- **Files**: `src/renderer/mod.rs` (test module)
- **Action**: Write unit tests for the extracted function, focusing on the quantization behavior that drives dirty-checking.
- **Test cases**:
  - Sun direction `(0.1234, -0.5678, 0.9012)` quantizes to `[123, -567, 901]` (truncation toward zero)
  - Terminator width `0.15` quantizes to `150`
  - Diffuse floor/ramp quantize correctly
  - Two states with sun directions differing by less than 0.001 (below quantization threshold) compare as equal via `PartialEq`, confirming the dirty-check correctly skips renders for sub-milliradian changes
  - Two states with sun directions differing by exactly 0.001 compare as different
  - Changing each field individually produces a different `FrameState` (ensures no field is accidentally omitted from dirty-checking). Test all 12 fields: longitude, latitude, zoom, sample_count, texture_index, width, height, sun_direction, terminator_width, diffuse_shading, diffuse_floor, diffuse_ramp
- **Verify**: All tests pass
- **Complexity**: Small

**Phase 3 checkpoint**: `cargo test` passes. `cargo clippy` clean. Run the app and visually confirm rendering is identical. `cargo llvm-cov --html` shows increased `renderer/` coverage. No file in `src/renderer/` exceeds ~400 lines.

### Phase 4: GPU Render Pipeline Integration Tests

Goal: Test the vertex transform, full fragment shader pipeline, single-texture mode sentinel, and uniform buffer layout on the GPU.

#### Step 4.1: Create shared GPU test infrastructure in `tests/common/mod.rs`

- **Files**: `tests/common/mod.rs` (new file)
- **Action**: Extract a reusable `GpuContext` struct and `create_gpu_context()` function from `tests/shading.rs` into a shared module. Provides: device/queue creation (with software fallback), buffer readback helper, and the `LazyLock<Mutex<GpuContext>>` pattern. Update `tests/shading.rs` to import from `tests/common/mod.rs` instead of defining its own context.
- **Verify**: `cargo test --test shading` still passes after the refactor
- **Complexity**: Medium

#### Step 4.2: Write a full render pipeline integration test

- **Files**: `tests/render_pipeline.rs` (new file)
- **Action**: Create an integration test that renders a small frame (64x64 or 128x128) using the production vertex and fragment shaders (`blend.wgsl` + `sphere.wgsl`) on the software adapter. Uses the same render pipeline configuration as production. Asserts behavioral invariants (not pixel-exact values, due to cross-hardware float variance).
- **Test cases**:
  - `sphere_renders_visible_pixels` -- render with a known texture and `terminator_width = -1.0` (single-texture mode). Assert that not all pixels are the clear color, confirming the sphere is visible.
  - `day_side_brighter_than_night_side` -- render with day=white, night=dark gray textures in blend mode with sun pointing along +Z. The hemisphere facing the sun should have higher average luminance than the opposite hemisphere.
  - `single_texture_mode_ignores_night` -- render with `terminator_width = -1.0` and a red day texture + green night texture. Assert that no green pixels appear in the output, confirming the sentinel bypasses night texture sampling.
- **Verify**: All tests pass on the software adapter
- **Complexity**: Large

#### Step 4.3: Write a uniform buffer field offset test

- **Files**: `tests/render_pipeline.rs` (append)
- **Action**: Write a compute shader test that reads specific fields from a `Uniforms` buffer written from Rust and asserts their values. This catches byte-offset mismatches between the Rust `#[repr(C)]` struct and the WGSL `Uniforms` struct that the compile-time `size_of` assertion cannot detect.
- **Test cases**:
  - Write a `Uniforms` struct with known values (identity MVP, sun_dir `[0.0, 1.0, 0.0]`, terminator_width `0.15`, flags `1`, diffuse_floor `0.5`, diffuse_ramp `0.25`). A compute shader reads each field and writes to an output buffer. Assert output matches input for every field.
- **Verify**: Test passes. Failure message indicates which field has the wrong offset.
- **Complexity**: Medium

**Phase 4 checkpoint**: `cargo test` passes, including new render pipeline tests. `sphere.wgsl` is now tested end-to-end.

### Phase 5: Documentation

Goal: Update project documentation with testing standards and coverage commands.

#### Step 5.1: Update CLAUDE.md

- **Files**: `CLAUDE.md`
- **Action**: Add a `## Testing` section documenting:
  - Coverage commands: `cargo llvm-cov --html` (local), `cargo llvm-cov --lcov` (CI)
  - Coverage targets: 90-100% for pure functions, 80-90% for business logic, 20-40% for GPU pipeline, <20% for main/UI; overall target 60-70%
  - Conventions: `approx::assert_relative_eq!` for float comparisons, behavioral invariants (not pixel-exact) for GPU tests, `LazyLock<Mutex<GpuContext>>` for shared GPU device, shared helpers in `tests/common/mod.rs`
  - Dev-dependencies: `approx`, `proptest` and their purposes
  - Module structure: `src/renderer/` submodule layout
  - Add coverage commands to the Build Commands section

- **Verify**: CLAUDE.md renders correctly. Commands listed actually work.
- **Complexity**: Small

## Out of Scope

The following were investigated in the research phase but are not included in this plan:

**Slint UI tests** (`i-slint-backend-testing`): Unresolved whether the Slint testing backend and wgpu can coexist in the same process. The exact `i-slint-backend-testing` version to pin for Slint `~1.15` needs confirmation. Should be scoped as a separate feature with its own research spike.

**Golden image / visual regression tests**: Depend on the render pipeline test (Phase 4) existing first. Software-adapter-only baselines need to be established. SSIM thresholds and golden file management workflow should be designed separately.

**cargo-nextest**: Worth adopting for faster test runs and per-process isolation, but orthogonal to coverage improvement. Can be added independently at any time.

**cargo-mutants**: Useful for validating test quality on pure functions after coverage improves. Best run as a one-off audit after Phases 1-3 complete.

## Test Strategy

### New Automated Tests

| Test | Type | Location | Phase |
|---|---|---|---|
| `build_aa_options` with various sample count inputs | Unit | `src/renderer/mod.rs` | 1 |
| `downsample_2x` with uniform/checkerboard images | Unit | `src/renderer/textures.rs` | 1 |
| `shift_horizontal` with known pixel patterns | Unit | `src/texture_loader.rs` | 1 |
| Float assertions migrated to `assert_relative_eq!` | Unit | `src/camera.rs`, `src/sun.rs` | 1 |
| `quantize_to_granularity` boundary cases | Unit | `src/renderer/mod.rs` | 2 |
| `adapter_type_rank` ordering | Unit | `src/wgpu_init.rs` | 2 |
| Grid texture pixel sampling at known positions | Unit | `src/grid_texture.rs` | 2 |
| `shift_horizontal` four-shift identity (proptest) | Property | `src/texture_loader.rs` | 2 |
| `downsample_2x` output size invariant (proptest) | Property | `src/renderer/textures.rs` | 2 |
| `build_frame_state` quantization behavior | Unit | `src/renderer/mod.rs` | 3 |
| `build_frame_state` per-field dirty-check coverage | Unit | `src/renderer/mod.rs` | 3 |
| Sphere renders visible pixels | GPU Integration | `tests/render_pipeline.rs` | 4 |
| Day side brighter than night side | GPU Integration | `tests/render_pipeline.rs` | 4 |
| Single-texture mode ignores night texture | GPU Integration | `tests/render_pipeline.rs` | 4 |
| Uniform buffer field offsets match WGSL | GPU Integration | `tests/render_pipeline.rs` | 4 |

### Manual Verification

- Run `cargo llvm-cov --html` and verify coverage percentages per module
- Run `cargo test` on a machine with a discrete GPU to verify GPU tests pass on hardware
- Run `cargo clippy` to verify no new warnings from refactored code
- Visual spot-check the app after the renderer split to confirm identical rendering

## Risks and Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Software adapter unavailable in CI | Render pipeline tests fail | `force_fallback_adapter: true` with skip annotation if no adapter; WARP pre-installed on Windows CI |
| Proptest finds edge cases in `downsample_2x` or `shift_horizontal` | Reveals bugs in production code | Fix the bugs. This is a feature, not a risk. |
| cargo-llvm-cov branch coverage is unstable | CI coverage gate flaky | Use line coverage for thresholds. Branch coverage is informational only (marked experimental). |
| Renderer split causes merge conflicts with parallel work | Block on parallel renderer changes | Do the split as a standalone commit. No logic changes in the split commit. |
| `build_frame_state` extraction changes rendering behavior | Rendering regression | The same quantization arithmetic moves to a function. `FrameState` derives `PartialEq` unchanged. Visual spot-check after extraction. |
| GPU device creation crashes on Windows | Render pipeline tests crash | Reuse `LazyLock<Mutex<GpuContext>>` from `tests/shading.rs` via `tests/common/mod.rs` |

## Rollback Strategy

Each phase is an additive, independently revertible set of commits:

- **Phase 1**: Revert dev-dependencies and new test functions. Zero production code impact.
- **Phase 2**: Revert extracted functions. Original inline code restored.
- **Phase 3**: The module split (step 3.1) is the hardest to revert. Commit it standalone so `git revert` works cleanly. The humble object extractions (steps 3.3-3.4) are independently revertible.
- **Phase 4**: Revert new integration test files. No production code changes.
- **Phase 5**: Revert documentation changes.

## Dependencies Between Phases

```
Phase 1 (tooling + easy wins)
  |
  v
Phase 2 (extractions + proptest) -- uses approx/proptest from Phase 1
  |
  v
Phase 3 (renderer split + humble object) -- builds on Phase 2 extractions
  |
  v
Phase 4 (GPU integration tests) -- imports from renderer/ and tests/common/
  |
  v
Phase 5 (documentation) -- documents final state
```

Phases must be executed sequentially. Phase 3 modifies `renderer.rs` extensively, so it should not overlap with Phase 4 which imports from `renderer/`.

## Recommended `[dev-dependencies]`

```toml
[dev-dependencies]
approx = "0.5"          # assert_relative_eq! for float comparisons
proptest = "1"           # property-based testing for pure functions
# Future phases (not in this plan):
# tempfile = "3"         # temp directories for texture_loader tests
# i-slint-backend-testing = "~1.15"  # must match slint version exactly
# image-compare = "0.4"  # SSIM-based golden image comparison
```

## Status

- [ ] Plan approved
- [ ] Phase 1 complete (baseline + easy wins)
- [ ] Phase 2 complete (extract + test hidden logic)
- [ ] Phase 3 complete (renderer split + humble object)
- [ ] Phase 4 complete (GPU integration tests)
- [ ] Phase 5 complete (documentation)
- [ ] Implementation complete
