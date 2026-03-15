# Plan: GPU Compute Shader Tests for WGSL Shading Logic (2026-03-15)

## Summary

Replace the Rust-mirror shading tests (`src/shading.rs`) with GPU compute
shader tests that execute the actual WGSL blending logic from `sphere.wgsl`.
The blend/shading math is extracted into a standalone `shaders/blend.wgsl`
file, which is concatenated into both the production fragment shader and a
test compute shader. This eliminates the maintenance risk of the Rust mirror
drifting out of sync with the shader.

## Stakes Classification

**Level**: Medium
**Rationale**: Touches the shader source (split into two files) and the
renderer's shader loading (string concatenation). The rendering pipeline
itself is unchanged — same WGSL logic, same uniform layout, same entry
points. Rollback is straightforward (recombine the files). Tests run on GPU
but failure only affects the test suite, not production.

## Context

**Affected Areas**:

- `shaders/sphere.wgsl` — inline blend logic extracted into a function call
- `shaders/blend.wgsl` — new file, pure function, no bindings
- `src/renderer.rs` — shader loading changes from single `include_str!` to
  concatenation of two files
- `src/shading.rs` — deleted (Rust mirror)
- `src/main.rs` — remove `#[cfg(test)] mod shading;` declaration
- `tests/shading.rs` — new integration test with GPU test harness

## Success Criteria

- [ ] `blend_fragment()` lives in `shaders/blend.wgsl` and is the single
  source of truth for both production rendering and tests
- [ ] `src/shading.rs` is deleted; no Rust reimplementations of shader math
  exist anywhere
- [ ] All existing test properties (monotonicity, per-channel floor, boundary
  conditions, NYC regression) are verified by running the real WGSL on the GPU
- [ ] `cargo test` passes (including the new GPU tests)
- [ ] `cargo clippy` is clean
- [ ] Visual rendering is unchanged (manual check)

## Implementation Steps

### Phase 1: Extract Blend Function

#### Step 1.1: Create `shaders/blend.wgsl`

- **Files**: `shaders/blend.wgsl` (new)
- **Action**: Create a new file containing a single pure function extracted
  from `fs_main`:

  ```wgsl
  fn blend_fragment(
      day_color: vec3<f32>,
      night_color: vec3<f32>,
      n_dot_l: f32,
      terminator_width: f32,
      diffuse_enabled: bool,
      diffuse_floor: f32,
      diffuse_ramp: f32,
  ) -> vec3<f32> { ... }
  ```

  The function takes all inputs as parameters (no access to uniforms or
  textures) and returns the blended RGB color. The body is the existing
  smoothstep/mix/max/min logic currently inline in `fs_main`.
- **Verify**: File exists, contains only the function, no `@group`/`@binding`
  or entry point annotations.
- **Complexity**: Small

#### Step 1.2: Update `sphere.wgsl` to call `blend_fragment`

- **Files**: `shaders/sphere.wgsl`
- **Action**: Replace the inline blend logic in `fs_main` (lines 58--70)
  with a call to `blend_fragment(day_color, night_color, n_dot_l,
  uniforms.terminator_width, (uniforms.flags & 1u) != 0u,
  uniforms.diffuse_floor, uniforms.diffuse_ramp)`. The Uniforms struct,
  texture bindings, vertex shader, and single-texture early return all remain
  in this file unchanged.
- **Verify**: `sphere.wgsl` no longer contains `smoothstep`, `diffuse_floor`,
  or `min(night_color, day_color)` directly — all blend math is delegated.
- **Complexity**: Small

#### Step 1.3: Update `renderer.rs` shader loading

- **Files**: `src/renderer.rs`
- **Action**: Change the `include_str!("../shaders/sphere.wgsl")` call to
  concatenate `blend.wgsl` before `sphere.wgsl`:

  ```rust
  let wgsl_source = format!(
      "{}\n{}",
      include_str!("../shaders/blend.wgsl"),
      include_str!("../shaders/sphere.wgsl"),
  );
  ```

  Then pass `wgsl_source.into()` to `ShaderSource::Wgsl`.
- **Verify**: `cargo build` succeeds. `cargo run` renders identically to
  before (manual visual check).
- **Complexity**: Small

### Phase 2: GPU Test Harness

#### Step 2.1: Define WGSL/Rust struct layout for test cases

- **Files**: `tests/shading.rs`
- **Action**: Define the `TestCase` and `TestResult` structs in both WGSL
  (inline string for the compute harness) and Rust (`#[repr(C)]` with
  `bytemuck::Pod`). Must respect WGSL storage buffer alignment:

  WGSL struct:

  ```text
  struct TestCase {
      day: vec3<f32>,           // offset 0  (align 16)
      _pad0: f32,               // offset 12
      night: vec3<f32>,         // offset 16 (align 16)
      _pad1: f32,               // offset 28
      n_dot_l: f32,             // offset 32
      terminator_width: f32,    // offset 36
      diffuse_floor: f32,       // offset 40
      diffuse_ramp: f32,        // offset 44
      diffuse_enabled: u32,     // offset 48 (bool not host-shareable)
      _pad2: u32,               // offset 52
      _pad3: u32,               // offset 56
      _pad4: u32,               // offset 60
  }  // total: 64 bytes (multiple of 16)

  struct TestResult {
      color: vec3<f32>,         // offset 0  (align 16)
      _pad: f32,                // offset 12
  }  // total: 16 bytes
  ```

  Rust structs must be identical in layout with `#[repr(C)]`. Add
  compile-time size assertions.
- **Verify**: `assert!(size_of::<TestCase>() == 64)`,
  `assert!(size_of::<TestResult>() == 16)`.
- **Complexity**: Small

#### Step 2.2: Write compute shader entry point

- **Files**: `tests/shading.rs`
- **Action**: Define a `COMPUTE_HARNESS` constant containing the WGSL compute
  shader that reads from an input storage buffer, calls `blend_fragment`,
  and writes to an output storage buffer:

  ```wgsl
  @group(0) @binding(0) var<storage, read> inputs: array<TestCase>;
  @group(0) @binding(1) var<storage, read_write> outputs: array<TestResult>;

  @compute @workgroup_size(64)
  fn test_main(@builtin(global_invocation_id) id: vec3<u32>) {
      let i = id.x;
      if i >= arrayLength(&inputs) { return; }
      let tc = inputs[i];
      let color = blend_fragment(
          tc.day, tc.night, tc.n_dot_l, tc.terminator_width,
          tc.diffuse_enabled != 0u, tc.diffuse_floor, tc.diffuse_ramp,
      );
      outputs[i] = TestResult(color, 0.0);
  }
  ```

  The full test shader is built by concatenating `blend.wgsl` + this harness.
- **Verify**: `device.create_shader_module(...)` with the concatenated source
  succeeds without validation errors.
- **Complexity**: Small

#### Step 2.3: Implement GPU dispatch and readback helper

- **Files**: `tests/shading.rs`
- **Action**: Create a helper function
  `fn run_on_gpu(cases: &[TestCase]) -> Vec<TestResult>` that:
    1. Creates a headless wgpu device (`compatible_surface: None`,
       `pollster::block_on`). If no adapter is available, panics with a clear
       message.
    2. Compiles the concatenated shader (`blend.wgsl` + harness).
    3. Creates a compute pipeline with `layout: None` (auto-inferred).
    4. Creates input storage buffer (from `cases`), output storage buffer,
       and staging buffer (MAP_READ + COPY_DST).
    5. Creates bind group, dispatches
       `((cases.len() + 63) / 64, 1, 1)` workgroups.
    6. Copies output buffer to staging, maps staging, reads back `TestResult`
       slice via `bytemuck::cast_slice`.
    7. Returns the results.

  Uses `device.poll(wgpu::PollType::wait_indefinitely()).unwrap()` for the
  buffer map synchronization.
- **Verify**: Helper compiles. A trivial round-trip test (one case with known
  output) passes.
- **Complexity**: Medium

#### Step 2.4: Port test cases to GPU execution

- **Files**: `tests/shading.rs`
- **Action**: Rewrite each test to build a `Vec<TestCase>`, call
  `run_on_gpu`, and assert on the returned `TestResult` values. Port these
  test properties from the existing Rust-mirror tests:

  **Monotonicity tests** (sweep n_dot_l -1..+1 in 1000 steps, check
  luminance non-decreasing):
  - Ocean texel: day `[0.05, 0.08, 0.30]`, night `[0.01, 0.01, 0.04]`
  - Land texel: day `[0.30, 0.25, 0.15]`, night `[0.03, 0.02, 0.01]`

  **Per-channel floor tests** (no channel below `min(night, day)` for any
  n_dot_l):
  - Ocean, land, and NYC params, 1000 steps each

  **Boundary conditions**:
  - n_dot_l = 1.0 (fully lit) produces day_color exactly
  - n_dot_l = -1.0 (fully dark) produces night_color exactly

  **Diffuse toggle**:
  - With `diffuse_enabled = false`, result matches pure day/night blend

  **NYC city-light regression**:
  - At n_dot_l in {0.3, 0.5, 0.7, 1.0}: luminance <= day luminance (no
    city light leakage)

  **Stress test** (6 day colors x 6 night colors x 500 n_dot_l steps):
  - No channel below `min(night, day)` for any combination

  All test cases can be batched into a single `run_on_gpu` call per test
  for efficiency.
- **Verify**: `cargo test shading` passes with all tests executing on GPU.
- **Complexity**: Medium

### Phase 3: Cleanup

#### Step 3.1: Delete Rust mirror

- **Files**: `src/shading.rs` (delete), `src/main.rs`
- **Action**: Delete `src/shading.rs` entirely. Remove the
  `#[cfg(test)] mod shading;` line from `src/main.rs`. The `luminance`
  helper needed for assertions now lives in `tests/shading.rs` (already
  added in Step 2.4). The `buggy_*` tests are dropped — they tested the
  old code and served their purpose.
- **Verify**: `src/shading.rs` does not exist. No Rust reimplementation of
  `smoothstep`, `mix`, or blend logic remains anywhere. `cargo test` passes.
  `cargo clippy` is clean.
- **Complexity**: Small

## Risks and Mitigations

**WGSL struct alignment mismatch between Rust and GPU**
Impact: tests pass/fail incorrectly due to garbled data.
Mitigation: compile-time `size_of` assertions on both structs. First test
case uses known-good values and asserts exact output.

**No GPU adapter available (headless CI)**
Impact: tests panic.
Mitigation: on Windows (this project's target), DX12 WARP is always
available. Use `force_fallback_adapter: true` as fallback if primary
request fails.

**Shader concatenation breaks naga validation**
Impact: `cargo test` fails to compile shader.
Mitigation: caught immediately at `create_shader_module`. Keep `blend.wgsl`
free of bindings/entry points so it composes cleanly.

**f32 precision differences between CPU and GPU**
Impact: assertion tolerances too tight.
Mitigation: use 1e-5 epsilon for GPU comparisons (vs 1e-6 for the Rust
mirror). GPU f32 ops may differ in last ULP.

## Rollback Strategy

Low-risk rollback: revert the `blend.wgsl` extraction by moving the function
body back inline into `sphere.wgsl`, restore the single `include_str!` in
`renderer.rs`, restore `src/shading.rs` and its `main.rs` declaration from
git history, and delete `tests/shading.rs`.

## Status

- [ ] Plan approved
- [ ] Implementation started
- [ ] Implementation complete
