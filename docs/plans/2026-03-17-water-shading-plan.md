# Plan: Water Shading (2026-03-17)

## Summary

Add specular sun glint and view-dependent reflectance (Fresnel) to ocean pixels on the 3D Earth globe, making water look like water instead of painted matte surface. The implementation uses Blinn-Phong specular with Schlick Fresnel, gated by a water mask stored in the day texture's alpha channel. Two new UI sliders control shininess and glint intensity. The shader already has all the geometric data it needs except the camera eye position, which must be added to the uniform buffer.

## Stakes Classification

**Level**: Medium
**Rationale**: The change touches multiple files across the codebase (uniforms, shaders, UI, frame state, tests, texture pipeline) but each individual change is small and well-isolated. The uniform buffer extension follows a documented pattern. The alpha channel repurposing is safe (confirmed unused). The feature is additive -- existing rendering is unchanged when specular intensity is zero. Rollback is straightforward: revert the branch.

## Context

**Research**: [`docs/plans/2026-03-17-water-shading-research.md`](2026-03-17-water-shading-research.md)
**Affected Areas**:

- `src/renderer/uniforms.rs` -- Rust uniform struct (96 -> 128 bytes)
- `shaders/sphere.wgsl` -- WGSL uniform struct + fragment shader specular logic
- `shaders/blend.wgsl` -- return blend factor alongside color (signature change)
- `src/renderer/render_pass.rs` -- `ShadingParams`, `write_uniforms()`
- `src/renderer/frame.rs` -- `FrameState`, `build_frame_state()`
- `src/renderer/mod.rs` -- wire new slider values through to `ShadingParams` and `build_frame_state()`
- `src/main.rs` -- (no changes needed; sliders auto-trigger redraw via existing `sliders-changed` callback)
- `ui/main.slint` -- two new sliders + two new out-properties in Lighting group
- `tests/shading.rs` -- update compute harness (no uniform struct change needed here, but `blend_fragment` signature changes)
- `tests/render_pipeline.rs` -- update `Uniforms` copy, uniform readback shader, render tests
- Texture pipeline tooling -- encode water mask into day texture alpha channel

## Success Criteria

- [ ] Ocean pixels show a visible specular highlight (sun glint) at default settings
- [ ] The highlight is restricted to water; land pixels have no specular contribution
- [ ] The highlight fades to zero on the night side and through the terminator
- [ ] The highlight intensifies at grazing angles (Fresnel effect)
- [ ] "Ocean Shininess" slider controls highlight tightness in real time
- [ ] "Ocean Glint" slider controls highlight intensity in real time
- [ ] Setting glint to 0.0 produces output identical to the pre-change state
- [ ] All existing tests pass (shading, render_pipeline, unit tests)
- [ ] New GPU integration tests verify specular behavioral invariants
- [ ] `cargo clippy` passes with no new warnings
- [ ] Single-texture mode (grid, day-only, night-only) is unaffected

## Implementation Steps

### Phase 1: Extend Uniform Buffer with Eye Position and Specular Parameters

This phase adds all the new data plumbing without any visual change to the shader. The shader ignores the new fields until Phase 3.

#### Step 1.1: Add new fields to the Rust `Uniforms` struct (RED then GREEN)

- **Files**: `src/renderer/uniforms.rs`
- **Action**: Add `eye_pos: [f32; 3]`, `_pad2: f32`, `spec_shininess: f32`, `spec_intensity: f32`, and `_pad3: [f32; 2]` after the existing `_pad` field. Update the size assertion from 96 to 128.
- **Test cases** (compile-time):
  - Size assertion: `size_of::<Uniforms>() == 128`
  - The struct must remain `Pod` and `Zeroable` (compile error if not)
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

#### Step 1.2: Add matching fields to the WGSL `Uniforms` struct

- **Files**: `shaders/sphere.wgsl`
- **Action**: Add `eye_pos: vec3<f32>`, `_pad2: f32`, `spec_shininess: f32`, `spec_intensity: f32`, and `_pad3: vec2<f32>` to the WGSL `Uniforms` struct, matching the Rust layout byte-for-byte.
- **Test cases**: Deferred to Step 1.6 (GPU readback test).
- **Verify**: `cargo build` succeeds (shader compiles at pipeline creation time, but build confirms no Rust-side issues)
- **Complexity**: Small

#### Step 1.3: Populate new uniform fields in `write_uniforms()`

- **Files**: `src/renderer/render_pass.rs`
- **Action**:
  - Add `spec_shininess: f32` and `spec_intensity: f32` to `ShadingParams`.
  - In `write_uniforms()`, compute eye position from `OrbitalCamera::eye_position()` and populate `eye_pos`, `spec_shininess`, and `spec_intensity` in the `Uniforms` struct. Set `_pad2` and `_pad3` to zero.
- **Test cases**: Deferred to Step 1.6 (GPU readback test).
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

#### Step 1.4: Add specular parameters to `FrameState` and `build_frame_state()`

- **Files**: `src/renderer/frame.rs`
- **Action**:
  - Add `spec_shininess: i32` and `spec_intensity: i32` to `FrameState`.
  - Add `spec_shininess: f32` and `spec_intensity: f32` parameters to `build_frame_state()`, quantized to integer thousandths like the existing diffuse parameters.
  - Update all existing test calls to `build_frame_state()` to pass the two new parameters (use default values: 150.0 and 0.4).
- **Test cases** (unit, in the existing `frame::tests` module):
  - `frame_state_spec_shininess_quantization`: `build_frame_state(..., 150.0, 0.4)` -> `spec_shininess == 150000`, `spec_intensity == 400`
  - `frame_state_spec_shininess_triggers_dirty`: changing shininess from 150.0 to 200.0 produces `state_a != state_b`
  - `frame_state_spec_intensity_triggers_dirty`: changing intensity from 0.4 to 0.5 produces `state_a != state_b`
- **Verify**: `cargo test frame` passes
- **Complexity**: Small

#### Step 1.5: Wire new parameters through the rendering callback

- **Files**: `src/renderer/mod.rs`, `ui/main.slint`
- **Action**:
  - In `ui/main.slint`, add two new `out property` declarations for `spec-shininess` and `spec-intensity`, and add two sliders to the Lighting `GroupBox`:
    - "Shininess" slider: min 10, max 500, default 150, triggers `sliders-changed`
    - "Glint" slider: min 0.0, max 1.0, default 0.4, triggers `sliders-changed`
  - In `rendering_callback()` in `mod.rs`:
    - Read `win.get_spec_shininess()` and `win.get_spec_intensity()` (auto-generated getters from the Slint properties)
    - Pass them to `build_frame_state()`
    - Pass them to `ShadingParams`
- **Test cases** (manual):
  - Run the app, verify the two new sliders appear in the Lighting group
  - Move the sliders, verify the app does not crash (no visual change yet)
- **Verify**: `cargo run` shows the sliders; `cargo clippy` passes
- **Complexity**: Small

#### Step 1.6: Update GPU integration tests for new uniform layout (RED then GREEN)

- **Files**: `tests/render_pipeline.rs`
- **Action**:
  - Update the local `Uniforms` copy to match the new 128-byte layout (add `eye_pos`, `_pad2`, `spec_shininess`, `spec_intensity`, `_pad3`). Update size assertion to 128.
  - Update all `Uniforms` construction sites in tests to populate the new fields (eye_pos from the test camera position `[0, 0, 3.5]`, spec_shininess = 150.0, spec_intensity = 0.0 so behavior is unchanged).
  - Update `UNIFORM_READBACK_SHADER` to read the new fields and assert their values.
- **Test cases** (GPU integration):
  - `uniform_buffer_field_offsets_match_wgsl`: extend to read back `eye_pos.x/y/z`, `spec_shininess`, `spec_intensity` and assert they match the written values
  - Existing tests (`sphere_renders_visible_pixels`, `day_side_brighter_than_night_side`, `single_texture_mode_ignores_night`) must still pass with `spec_intensity = 0.0`
- **Verify**: `cargo test --test render_pipeline` passes
- **Complexity**: Medium

### Phase 2: Encode Water Mask in Day Texture Alpha

This phase is independent of Phase 1 at the code level but depends on Phase 1 being mergeable first to avoid conflicts. The texture pipeline tooling does not currently exist in the repository as committed code (it lives outside the Rust project), so this phase covers only the shader-side readiness and manual texture preparation.

#### Step 2.1: Prepare day texture with water mask in alpha channel

- **Files**: Day texture file (external to the Rust codebase)
- **Action**: Using the existing ocean masking pipeline (which already identifies water pixels), write 255 into the alpha channel for water pixels, 0 for land pixels, and anti-aliased intermediate values at coastlines. Re-export the day texture as JXL. The alpha channel is currently always 255 and is never read by any code path.
- **Test cases** (manual):
  - Load the new texture in an image viewer, inspect the alpha channel: ocean should be white (255), land should be black (0), coastlines should have intermediate values
  - Run the app with the new texture, verify the scene looks identical to before (alpha channel is not yet used by the shader)
- **Verify**: Visual inspection confirms identical rendering
- **Complexity**: Medium (external tooling, not Rust code)

### Phase 3: Add Specular Calculation to Fragment Shader

This phase adds the visual payoff. It depends on Phase 1 (uniform fields) and Phase 2 (water mask in alpha).

#### Step 3.1: Modify `blend_fragment()` to return the blend factor (RED then GREEN)

- **Files**: `shaders/blend.wgsl`, `shaders/sphere.wgsl`
- **Action**: The `blend_fragment()` function currently returns `vec3<f32>` (the blended color). The specular calculation needs the blend factor (`smoothstep(-w, w, n_dot_l)`) to gate the highlight through the terminator. Two options:
  - **Option A**: Change `blend_fragment()` to return a struct with both color and blend factor. This is cleaner but requires updating the compute test harness.
  - **Option B**: Compute the blend factor separately in `fs_main()` before calling `blend_fragment()`. This avoids touching `blend_fragment()` but duplicates the smoothstep call.
  - **Recommended**: Option A. Define a `BlendResult` struct with `color: vec3<f32>` and `blend: f32`. Update `blend_fragment()` to return it. Update `fs_main()` to destructure the result.
- **Test cases**: The existing `tests/shading.rs` compute harness calls `blend_fragment()` and must be updated to match the new return type. All existing shading tests must continue to pass.
- **Verify**: `cargo test --test shading` passes
- **Complexity**: Small

#### Step 3.2: Update the shading compute test harness for `BlendResult`

- **Files**: `tests/shading.rs`
- **Action**: Update the `COMPUTE_HARNESS` WGSL string:
  - Add the `BlendResult` struct definition
  - Update `TestResult` to include the blend factor (or keep it as-is if we only check color)
  - Update the `test_main` function to handle the struct return
- **Test cases**:
  - All existing shading tests must still pass with the same tolerances
  - New test: `blend_factor_zero_on_night_side`: with `n_dot_l = -1.0` and `terminator_width = 0.15`, the blend factor should be 0.0
  - New test: `blend_factor_one_on_day_side`: with `n_dot_l = 1.0` and `terminator_width = 0.15`, the blend factor should be 1.0
  - New test: `blend_factor_monotonic`: sweep n_dot_l from -1 to +1, blend factor should be non-decreasing
- **Verify**: `cargo test --test shading` passes
- **Complexity**: Small

#### Step 3.3: Add Blinn-Phong + Fresnel specular to `fs_main()` (RED then GREEN)

- **Files**: `shaders/sphere.wgsl`
- **Action**: After the `blend_fragment()` call in `fs_main()`, add the specular calculation:
  1. Sample the day texture as `vec4` instead of `.rgb` to get the water mask from `.a`
  2. Compute view direction: `v = normalize(uniforms.eye_pos - in.world_normal)` (world_normal doubles as world position on the unit sphere)
  3. Compute half vector: `h = normalize(uniforms.sun_dir + v)`
  4. Compute Blinn-Phong specular: `spec = pow(max(dot(n, h), 0.0), uniforms.spec_shininess)`
  5. Compute Schlick Fresnel with F0 = 0.02: `fresnel = 0.02 + 0.98 * (1 - max(dot(n, v), 0.0))^5`
  6. Gate by terminator and water mask: `glint = spec * fresnel * max(n_dot_l, 0.0) * blend * uniforms.spec_intensity * water`
  7. Add to blended color: `color = color + vec3(glint)`
  - The specular block is skipped in single-texture mode because of the existing early return when `terminator_width < 0`.
- **Test cases** (GPU integration, in `tests/render_pipeline.rs`):
  - `specular_visible_on_water_pixel`: render with a solid blue day texture (alpha=255, simulating all-water), sun facing camera, spec_intensity=0.5, shininess=150. Center pixel luminance should exceed the diffuse-only luminance from the same setup with spec_intensity=0.0.
  - `specular_absent_on_land_pixel`: render with a solid green day texture (alpha=0, simulating all-land), same setup. Luminance should be identical to spec_intensity=0.0.
  - `specular_absent_on_night_side`: render with all-water texture, sun pointing away from camera (sun_dir=[0,0,-1]), spec_intensity=0.5. Center pixel should show night color, no specular.
  - `specular_zero_intensity_matches_baseline`: render with spec_intensity=0.0. Output should match the pre-specular baseline pixel-for-pixel (within GPU tolerance).
  - `specular_increases_with_intensity`: render with spec_intensity=0.2 and spec_intensity=0.8 on all-water. Higher intensity should produce brighter center.
  - `single_texture_mode_unaffected_by_specular`: render in single-texture mode (terminator_width=-1) with spec_intensity=0.5. Output should match spec_intensity=0.0.
- **Verify**: `cargo test --test render_pipeline` passes
- **Complexity**: Medium

#### Step 3.4: Verify all existing tests pass

- **Files**: All test files
- **Action**: Run the full test suite to confirm no regressions.
- **Verify**: `cargo test` passes; `cargo clippy` passes
- **Complexity**: Small

### Phase 4: Tuning and Polish

#### Step 4.1: Visual tuning of default parameters

- **Files**: `ui/main.slint` (slider defaults), potentially `shaders/sphere.wgsl`
- **Action**: Run the app and visually evaluate the specular effect at multiple zoom levels and sun angles. Adjust default shininess (currently 150) and intensity (currently 0.4) if needed. Verify the effect looks good at:
  - Full globe view (zoom slider near 0.5)
  - Close zoom (zoom slider near 0.0)
  - Sun at various positions (drag the globe to see different sun angles)
  - Terminator zone (the highlight should fade smoothly)
- **Test cases** (manual):
  - Sun glint visible as a bright spot on the ocean
  - No glint on land masses
  - Glint fades at the terminator
  - Glint is brighter at grazing angles (limb of the globe)
  - Setting Glint slider to 0 removes all specular
  - Setting Shininess to max (500) produces a very tight highlight
  - Setting Shininess to min (10) produces a broad, soft highlight
- **Verify**: Visual inspection at multiple camera/sun configurations
- **Complexity**: Small

## Test Strategy

### Automated Tests

| Test Case | Type | Location | Description |
| --- | --- | --- | --- |
| Uniform size assertion | Compile-time | `src/renderer/uniforms.rs` | `size_of::<Uniforms>() == 128` |
| Spec shininess quantization | Unit | `src/renderer/frame.rs` | Shininess 150.0 -> 150000 |
| Spec intensity quantization | Unit | `src/renderer/frame.rs` | Intensity 0.4 -> 400 |
| Spec shininess triggers dirty | Unit | `src/renderer/frame.rs` | Different shininess -> different FrameState |
| Spec intensity triggers dirty | Unit | `src/renderer/frame.rs` | Different intensity -> different FrameState |
| Uniform field offsets (extended) | GPU integration | `tests/render_pipeline.rs` | Read back eye_pos, shininess, intensity via compute shader |
| Blend factor zero on night side | GPU integration | `tests/shading.rs` | n_dot_l=-1 -> blend=0 |
| Blend factor one on day side | GPU integration | `tests/shading.rs` | n_dot_l=1 -> blend=1 |
| Blend factor monotonic | GPU integration | `tests/shading.rs` | Sweep n_dot_l, blend non-decreasing |
| All existing shading tests | GPU integration | `tests/shading.rs` | Unchanged behavior with new return type |
| Specular visible on water | GPU integration | `tests/render_pipeline.rs` | Water pixel brighter with spec than without |
| Specular absent on land | GPU integration | `tests/render_pipeline.rs` | Land pixel unchanged by spec_intensity |
| Specular absent at night | GPU integration | `tests/render_pipeline.rs` | Night-side water pixel unchanged by spec_intensity |
| Specular zero matches baseline | GPU integration | `tests/render_pipeline.rs` | spec_intensity=0 identical to pre-change |
| Specular increases with intensity | GPU integration | `tests/render_pipeline.rs` | Higher intensity -> brighter pixel |
| Single-texture mode unaffected | GPU integration | `tests/render_pipeline.rs` | terminator_width=-1 ignores specular |
| All existing render_pipeline tests | GPU integration | `tests/render_pipeline.rs` | No regressions |

### Manual Verification

- [ ] Two new sliders ("Shininess", "Glint") appear in the Lighting group
- [ ] Moving either slider triggers a re-render
- [ ] Sun glint is visible on ocean areas at default settings
- [ ] No glint on land masses
- [ ] Glint fades smoothly through the terminator
- [ ] Glint is brighter at grazing angles (near globe limb)
- [ ] Setting Glint to 0.0 removes all specular
- [ ] Single-texture modes (Grid, Day, Night) show no specular
- [ ] "Set as Wallpaper" export includes the specular effect
- [ ] App runs without errors on startup

## Risks and Mitigations

| Risk | Impact | Mitigation |
| --- | --- | --- |
| Alpha channel not carried through JXL encode/decode | Specular appears on all pixels or no pixels | Verify alpha roundtrip in the JXL pipeline before writing shader code. The existing day texture uses Rgba8Unorm so the alpha channel exists; confirm jxl-oxide preserves it. |
| Uniform buffer alignment mismatch between Rust and WGSL | Shader reads garbage values for eye_pos or specular params | GPU readback test (Step 1.6) catches this immediately. Follow the existing `_pad` pattern for vec3 alignment. |
| Specular saturates to white at high intensity | Unrealistic blown-out highlights | Acceptable and matches real camera behavior per the research. The user controls intensity via the slider. |
| Existing GPU tests become flaky after shader change | False test failures | Keep spec_intensity=0.0 in all existing tests so the specular code path is a no-op. New tests use spec_intensity>0. |
| `blend_fragment()` signature change breaks the compute test harness | Test compilation failures | Step 3.2 updates the harness before Step 3.3 adds the specular code. |
| Texture pipeline tooling is external to the repo | Water mask texture must be produced manually | Document the alpha-writing step in the texture pipeline. For now, the mask can be baked manually using any image editor. |

## Rollback Strategy

All changes are in the `water-shading` worktree branch. To rollback:

1. Revert the branch or delete it
2. The main branch is untouched
3. No database migrations, no config file changes, no external service dependencies

The specular effect can also be "soft disabled" without reverting code by setting the Glint slider to 0.0 (spec_intensity = 0.0), which makes the specular term multiply to zero.

## Status

- [ ] Plan approved
- [ ] Phase 1 complete (uniform buffer + UI sliders)
- [ ] Phase 2 complete (water mask in texture alpha)
- [ ] Phase 3 complete (specular shader logic + tests)
- [ ] Phase 4 complete (visual tuning)
- [ ] Implementation complete
