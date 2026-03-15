# Test Coverage Assessment (2026-03-15)

## Current Test Inventory

### Unit Tests (inside `src/`)

The project has 19 unit tests across 4 source modules:

**`src/camera.rs` -- 5 tests** (`#[cfg(test)] mod tests`)

| Test | What it verifies |
|---|---|
| `eye_at_zero_longitude_zero_latitude` | Eye position at (lon=0, lat=0, dist=5) is on +Z axis |
| `eye_at_90_longitude` | Eye position at (lon=90, lat=0, dist=5) is on +X axis |
| `eye_at_90_latitude_is_clamped` | 90-degree latitude is clamped to 89.9 degrees (gimbal lock avoidance) |
| `mvp_is_not_identity` | MVP matrix at a non-trivial camera position differs from identity |
| `view_matrix_determinant_is_nonzero` | View matrix is invertible |

**`src/sphere.rs` -- 6 tests** (`#[cfg(test)] mod tests`)

| Test | What it verifies |
|---|---|
| `sphere_has_correct_vertex_count` | Vertex count matches formula: (stacks+1) * (sectors+1) |
| `sphere_has_correct_index_count` | Index count matches formula: stacks * sectors * 6 |
| `all_vertices_on_unit_sphere` | Every vertex position has length 1.0 (within 1e-5) |
| `uv_coordinates_in_range` | All UV coordinates fall within [0, 1] |
| `indices_in_range` | No index exceeds the vertex count |
| `vertex_layout_has_two_attributes` | Buffer layout has 2 attributes with 20-byte stride |

**`src/grid_texture.rs` -- 3 tests** (`#[cfg(test)] mod tests`)

| Test | What it verifies |
|---|---|
| `output_has_correct_size` | Pixel buffer length equals width * height * 4 |
| `all_pixels_are_opaque` | Every pixel has alpha = 255 |
| `contains_grid_lines` | Pixel at (0, 90) matches the major-yellow color (prime meridian at equator) |

**`src/sun.rs` -- 5 tests** (`#[cfg(test)] mod tests`)

| Test | What it verifies |
|---|---|
| `march_equinox_noon` | Sun direction at 2025-03-20 12:00 UTC points roughly toward +Z (prime meridian) |
| `june_solstice_noon` | Sun direction at 2025-06-21 12:00 UTC has positive Y (northern declination) |
| `december_solstice_noon` | Sun direction at 2025-12-21 12:00 UTC has negative Y (southern declination) |
| `march_equinox_midnight` | Sun direction at 2025-03-20 00:00 UTC points roughly toward -Z (date line) |
| `unit_vector` | `sun_direction_now()` always returns a unit-length vector |

### Integration Tests (`tests/` directory)

**`tests/shading.rs` -- 12 tests**

These are GPU compute shader tests that run the actual `blend_fragment()` WGSL function on real GPU hardware. They use a `LazyLock<Mutex<GpuContext>>` to share a single GPU device across parallel test threads.

| Test | What it verifies |
|---|---|
| `ocean_monotonic` | Ocean blend luminance is non-decreasing across 1001 n_dot_l steps |
| `land_monotonic` | Land blend luminance is non-decreasing across 1001 n_dot_l steps |
| `ocean_never_below_night` | Ocean blend luminance never dips below night luminance |
| `per_channel_never_below_night` | No ocean blend channel drops below its night value |
| `fully_lit_matches_day_color` | n_dot_l = 1.0 produces exact day color |
| `fully_dark_matches_night_color` | n_dot_l = -1.0 produces exact night color |
| `diffuse_disabled_matches_pure_blend` | Diffuse off yields exact smoothstep(day, night) blend |
| `nyc_fully_lit_matches_day_color` | NYC at n_dot_l = 1.0 produces exact day color |
| `nyc_day_side_not_dominated_by_city_lights` | On the day side, luminance stays below night luminance (city lights don't leak) |
| `never_below_min_of_night_and_day` | Per-channel floor invariant holds for ocean/land/NYC |
| `never_below_min_across_color_range` | Per-channel floor holds across 6x6 day/night color matrix (18,036 test cases) |
| `software_adapter_produces_correct_results` | Representative subset runs correctly on the CPU/WARP software adapter |

### Test Infrastructure

- **GPU test harness**: `tests/shading.rs` contains a custom compute shader harness that concatenates `shaders/blend.wgsl` with a WGSL compute entry point, dispatches test cases as storage buffer arrays, and reads back results via buffer mapping. Includes a `make_case()` builder and a `sweep()` helper for generating n_dot_l parameter sweeps.
- **Shared GPU context**: `LazyLock<Mutex<GpuContext>>` prevents per-test device creation crashes on Windows; all tests share one device and serialize GPU submissions.
- **Astronomy test helper**: `sun.rs` has a `#[cfg(test)]`-gated `make_time()` function and `sun_dir_at()` helper that construct `astro_time_t` from calendar components, enabling deterministic tests against specific dates.
- **No mocking framework**: No mock objects or test doubles are used anywhere.
- **No test fixtures on disk**: No test data files, golden images, or snapshot tests.
- **No benchmarks**: No `#[bench]` or criterion benchmarks.

### Testing Configuration in `Cargo.toml`

- No `[dev-dependencies]` section. Test dependencies (`wgpu`, `bytemuck`, `pollster`) are all production dependencies that happen to also be used in tests.
- No `[[test]]` entries for custom test targets (the `tests/shading.rs` file is auto-discovered).
- No test-specific feature flags.

## Module-by-Module Coverage Assessment

### `src/camera.rs` -- GOOD coverage

**Tested**: `eye_position()`, `view_matrix()`, `mvp_matrix()`, latitude clamping, `Vertex::buffer_layout()` indirectly via `sphere.rs` tests.

**Quality**: Tests verify meaningful geometric properties (position on correct axis, matrix invertibility, clamping behavior). The tolerance of 1e-5 is appropriate for f32 trigonometry.

**Not tested**:
- `projection_matrix()` is tested only indirectly through `mvp_matrix`.
- No test for negative longitude or distance edge cases (distance=0 or very large distances).
- No test that the view matrix actually looks at the origin (verify that a known world point projects correctly).

### `src/sphere.rs` -- GOOD coverage

**Tested**: Vertex count, index count, unit-sphere constraint, UV range, index validity, buffer layout.

**Quality**: Tests are comprehensive for the geometry generator. The `all_vertices_on_unit_sphere` test at 32x64 resolution is thorough. The structural tests (count formulas, layout) catch regressions in the parametric generation.

**Not tested**:
- Winding order (CCW from outside) -- the test checks index bounds but not that triangles face outward.
- Degenerate cases: stacks=0, sectors=0, stacks=1, sectors=1.
- No test that UV seams are correct (u=0 and u=1 correspond to the same physical location but different vertices).

### `src/grid_texture.rs` -- MINIMAL coverage

**Tested**: Output size, alpha channel, one specific pixel color.

**Quality**: Tests are shallow. The `contains_grid_lines` test checks a single pixel. The `all_pixels_are_opaque` test is useful but trivial. No test verifies the overall grid structure.

**Not tested**:
- Grid line spacing (do lines appear every 15 degrees?).
- `lerp_color` / `lerp_u8` helper functions (no direct tests).
- Boundary behavior at texture edges (first/last row/column).
- Color values for non-grid, non-major areas (ocean/land gradient).
- Any resolution other than the one tested (64x32 in tests vs 2048x1024 in production).

### `src/sun.rs` -- GOOD coverage

**Tested**: Sun direction at four astronomically significant dates/times (equinoxes, solstices, noon, midnight), unit vector invariant, current time.

**Quality**: Tests verify physically meaningful astronomical properties with appropriate tolerances (0.1-0.15 radians). The `unit_vector` test on `sun_direction_now()` provides a live-check. The `make_time` test helper enables deterministic date-based testing.

**Not tested**:
- Longitude accuracy for times other than noon/midnight (e.g., 6AM should have sun roughly at +X or -X).
- Error handling -- `assert_eq!` on status code is tested implicitly but panics on failure; no test for invalid inputs (though the C library is unlikely to fail on valid dates).
- Southern hemisphere dates or intermediate seasons.

### `src/renderer.rs` -- NO unit test coverage

This is the largest and most complex module (1104 lines) with zero tests.

**Key untested functions**:
- `build_aa_options()` -- pure function, easily testable. Takes `&[u32]` of supported sample counts and returns labels, counts, and default index.
- `downsample_2x()` -- pure function, easily testable. Box-filter downsampling of RGBA pixel data.
- `quantized_viewport_size()` -- depends on Slint window, harder to test.
- `lookup_sample_count()` -- depends on Slint window, harder to test.
- `resolve_render_index()` -- logic for fallback texture selection, depends on `GpuResources` struct.
- `Uniforms` struct size assertion -- exists as a compile-time check (`const _: () = assert!(size_of::<Uniforms>() == 96)`), which is good, but not a test.
- Dirty-checking logic in `rendering_callback` -- complex state comparison, completely untested.
- MSAA rebuild, texture recreation, pipeline creation -- all require GPU, but the logic of when to trigger them is not tested.

### `src/wgpu_init.rs` -- NO test coverage

**Key untested functions**:
- `select_adapter()` -- adapter ranking logic (DiscreteGpu > IntegratedGpu > VirtualGpu > Other > Cpu). This is a pure function given a slice of adapters and a boolean.
- `init()` -- full initialization flow. Requires a GPU, harder to unit test.
- `WGPU_ADAPTER_NAME` environment variable handling.
- Software fallback path (`force_software`).

### `src/texture_loader.rs` -- NO test coverage

**Key untested functions**:
- `load()` -- image loading, horizontal flip, conversion to RGBA8. Requires test image files on disk.
- `shift_horizontal()` -- pure function on a pixel buffer. Easily testable without any file I/O.
- `resolve_textures_dir()` -- directory resolution with env var and CLI overrides. Could be tested with temp directories.

### `src/main.rs` -- NO test coverage (expected)

Main entry point with CLI parsing, window creation, and event loop setup. This is inherently hard to unit test and is typically covered by manual/integration testing.

### `shaders/blend.wgsl` -- EXCELLENT coverage (via integration tests)

The blend function is thoroughly tested by 12 GPU compute shader tests with thousands of test cases covering monotonicity, boundary conditions, per-channel invariants, diffuse toggle, city-light regression, and a broad color matrix sweep.

### `shaders/sphere.wgsl` -- NO direct test coverage

The vertex shader (`vs_main`), fragment shader (`fs_main`), single-texture early return (`terminator_width < 0.0`), and texture sampling are not tested. The integration tests only cover `blend_fragment()` from `blend.wgsl`, not the full fragment shader pipeline.

### `build.rs` -- NO test coverage (trivial, expected)

Single-line Slint compilation step.

## Identified Gaps

### High Priority (pure functions, easy to test, significant logic)

1. **`renderer::build_aa_options()`** -- Pure function mapping sample counts to UI labels and default index. Has edge cases: empty input, no 8x support, only 1x support, unsorted counts.

2. **`renderer::downsample_2x()`** -- Pure function implementing box-filter mipmap generation. Could have off-by-one errors in boundary clamping. Used in every texture upload path.

3. **`texture_loader::shift_horizontal()`** -- Pure function that rotates pixel rows. Incorrect shifts would misalign the prime meridian. Easy to test with small buffers.

### Medium Priority (testable with some setup, important logic)

4. **`texture_loader::resolve_textures_dir()`** -- Directory resolution chain with multiple fallback paths. Could be tested with temp directories and env var manipulation.

5. **`wgpu_init::select_adapter()`** -- Adapter ranking logic. Could be tested with mock adapter info, but `wgpu::Adapter` is hard to construct in tests. The ranking closure inside the function could be extracted for testing.

6. **`grid_texture` deeper coverage** -- The grid generation logic (line spacing, colors, boundary behavior) is only superficially tested.

7. **`camera` edge cases** -- Negative longitude, extreme distances, projection matrix properties (frustum planes).

### Lower Priority (requires GPU or Slint, or low-risk code)

8. **`renderer` dirty-checking** -- The `FrameState` equality comparison drives render skipping. Currently only exercised indirectly by running the app. The quantization logic (milliradians, thousandths) is embedded in the rendering callback.

9. **`renderer` MSAA rebuild / texture recreation logic** -- When to trigger rebuilds is based on comparing current vs stored sample_count and viewport dimensions.

10. **`sphere.wgsl` vertex/fragment shader** -- The vertex transform and texture sampling. Would require rendering to a texture and reading back pixels (full render pipeline test).

11. **`texture_loader::load()`** -- Requires test image files. Would need fixture images or in-memory image generation.

## Test Infrastructure Assessment

**Strengths**:
- The GPU compute shader test harness is well-designed: it tests the actual WGSL code rather than a Rust mirror, eliminating drift risk.
- The shared `LazyLock<Mutex<GpuContext>>` pattern is a practical solution for the Windows device creation crash issue.
- The `software_adapter_produces_correct_results` test ensures CI works without a real GPU.
- Compile-time assertions on struct sizes (both `Uniforms` and `TestCase`/`TestResult`) catch layout mismatches at build time.

**Weaknesses**:
- No test data files or golden images for visual regression testing.
- No `[dev-dependencies]` -- no test utility crates (e.g., `proptest` for property testing, `tempfile` for filesystem tests, `approx` for float comparisons).
- No benchmarks for performance-critical paths (mipmap generation, texture decoding).
- No CI configuration visible in the repo (though the software adapter test suggests CI awareness).
- Float comparison uses raw epsilon arithmetic everywhere; a shared `assert_approx_eq!` macro or helper would reduce repetition.

## Key Observations

1. **Test distribution is lopsided**: 4 out of 8 source modules have zero test coverage. The modules with tests are well-tested, but the untested modules contain significant logic.

2. **The biggest gap is `renderer.rs`**: At 1104 lines, it is the largest module and has the most complex control flow (dirty-checking, MSAA rebuild, texture slot resolution, fallback logic). It has zero tests. Some of its functions (`build_aa_options`, `downsample_2x`) are pure and trivially testable.

3. **`texture_loader` has easily testable pure functions**: `shift_horizontal()` operates on byte buffers with no dependencies. `resolve_textures_dir()` could be tested with temp directories.

4. **The shader tests are excellent but narrow**: They thoroughly cover `blend_fragment()` but nothing else about the rendering pipeline (vertex transform, texture sampling, uniform buffer layout matching, single-texture mode sentinel).

5. **No negative/error-path testing**: No test verifies behavior when things go wrong (missing textures, invalid sample counts, zero-size viewports, adapter enumeration failures). The code uses `expect()` and `assert!()` liberally, but those failure modes are untested.

6. **Compile-time assertions are doing heavy lifting**: The `const _: () = assert!(size_of::<T>() == N)` pattern is used effectively for struct layout verification, compensating somewhat for the lack of runtime tests of the GPU uniform layout.
