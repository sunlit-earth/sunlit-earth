# Plan: Water Appearance (2026-03-17)

## Summary

Add Schlick Fresnel reflectance to the ocean rendering in two ways: (1) modulate the existing specular sun glint by a Fresnel term so it intensifies at grazing angles, and (2) mix the ocean's diffuse base color toward a sky color at grazing angles (Fresnel-driven diffuse color shift), controlled by a new UI slider. Together these make the ocean look like a reflective water surface rather than a painted matte sphere, producing the characteristic "silvery horizon" visible in real photos of Earth from space.

## Stakes Classification

**Level**: Medium
**Rationale**: The change touches multiple files across the codebase (shader, uniforms, frame state, render pass, renderer callback, UI, config, tests) but each individual change is small and follows the exact pattern established by the specular implementation in PR #9. The uniform buffer size does not change (a padding field is repurposed). The feature is additive -- setting the new Fresnel Mix slider to 0.0 and keeping the existing specular as-is produces output identical to the pre-change state. Rollback is straightforward: revert the branch.

## Context

**Research**: [`docs/plans/2026-03-17-water-appearance-research.md`](2026-03-17-water-appearance-research.md)
**Prior work**: [`docs/plans/2026-03-17-water-shading-plan.md`](2026-03-17-water-shading-plan.md) (specular sun glint, PR #9)
**Affected Areas**:

- `shaders/sphere.wgsl` -- add `schlick_fresnel()` function, apply Fresnel to specular, add diffuse color shift
- `src/renderer/uniforms.rs` -- repurpose `_pad3` to add `fresnel_mix` field (no size change, remains 128 bytes)
- `src/renderer/render_pass.rs` -- add `fresnel_mix` to `ShadingParams`, populate in `write_uniforms()`
- `src/renderer/frame.rs` -- add `fresnel_mix` to `FrameState` and `build_frame_state()`
- `src/renderer/mod.rs` -- read `fresnel_mix` from UI, wire through to `ShadingParams` and `build_frame_state()`
- `src/config.rs` -- add `fresnel_mix` to `AppConfig` for persistence
- `src/main.rs` -- read/write `fresnel_mix` in `apply_config_to_window()` and `read_config_from_window()`
- `ui/main.slint` -- add `fresnel-mix` property and slider in Lighting group
- `tests/render_pipeline.rs` -- update local `Uniforms` copy, readback shader, add Fresnel behavioral tests
- `tests/shading.rs` -- no changes needed (tests `blend_fragment()` which is unmodified)

## Success Criteria

- [ ] The specular sun glint intensifies at grazing angles (Fresnel modulation)
- [ ] Ocean pixels near the globe limb shift toward a sky color (diffuse color shift)
- [ ] A "Fresnel Mix" slider in the Lighting group controls the diffuse color shift strength
- [ ] Setting Fresnel Mix to 0.0 produces only the Fresnel-modulated specular (no diffuse shift)
- [ ] Setting both Glint and Fresnel Mix to 0.0 produces output identical to the pre-change state
- [ ] Land pixels are completely unaffected (Fresnel effects are gated by water mask)
- [ ] Single-texture mode (grid, day-only, night-only) is unaffected
- [ ] All existing tests pass with new uniform field set to zero/default
- [ ] New GPU integration tests verify Fresnel behavioral invariants
- [ ] `cargo clippy` passes with no new warnings
- [ ] The new parameter is persisted to config and restored on startup

## Implementation Steps

### Phase 1: Extend Uniform Buffer and Data Plumbing

This phase adds the new `fresnel_mix` field through every layer of the data pipeline without any visual change to the shader. The shader ignores the new field until Phase 2.

#### Step 1.1: Add `fresnel_mix` to the Rust `Uniforms` struct

- **Files**: `src/renderer/uniforms.rs`
- **Action**: Replace the `_pad3: [f32; 2]` field (8 bytes at offset 120) with `fresnel_mix: f32` (4 bytes at offset 120) followed by `_pad3: f32` (4 bytes at offset 124). Total remains 128 bytes. The compile-time size assertion is unchanged.
- **Test cases** (compile-time):
  - Size assertion: `size_of::<Uniforms>() == 128` (unchanged, must still pass)
  - The struct must remain `Pod` and `Zeroable` (compile error if not)
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

#### Step 1.2: Add `fresnel_mix` to the WGSL `Uniforms` struct

- **Files**: `shaders/sphere.wgsl`
- **Action**: Replace `_pad3: vec2<f32>` with `fresnel_mix: f32` followed by `_pad3: f32`. Update the comments to reflect the new offsets.
- **Test cases**: Deferred to Step 1.7 (GPU readback test).
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

#### Step 1.3: Add `fresnel_mix` to `ShadingParams` and `write_uniforms()`

- **Files**: `src/renderer/render_pass.rs`
- **Action**:
  - Add `fresnel_mix: f32` to the `ShadingParams` struct.
  - In `write_uniforms()`, populate `fresnel_mix` from `shading.fresnel_mix` in the `Uniforms` construction. Change `_pad3: [0.0; 2]` to `_pad3: 0.0`.
- **Test cases**: Deferred to Step 1.7 (GPU readback test).
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

#### Step 1.4: Add `fresnel_mix` to `FrameState` and `build_frame_state()`

- **Files**: `src/renderer/frame.rs`
- **Action**:
  - Add `fresnel_mix: i32` to `FrameState` (quantized to integer thousandths, matching the existing pattern).
  - Add `fresnel_mix: f32` parameter to `build_frame_state()`, quantize as `(fresnel_mix * 1000.0) as i32`.
  - Update all existing call sites in the test module to pass `fresnel_mix: 0.0` as the new parameter.
- **Test cases** (unit, in the existing `frame::tests` module):
  - `frame_state_fresnel_mix_quantization`: `build_frame_state(..., fresnel_mix: 0.5)` produces `fresnel_mix == 500`
  - `frame_state_fresnel_mix_triggers_dirty`: changing fresnel_mix from 0.0 to 0.5 produces `state_a != state_b`
  - Existing tests: all existing test calls updated with `fresnel_mix: 0.0` and continue to pass
- **Verify**: `cargo test frame` passes
- **Complexity**: Small

#### Step 1.5: Add `fresnel_mix` to UI and config persistence

- **Files**: `ui/main.slint`, `src/config.rs`, `src/main.rs`
- **Action**:
  - In `ui/main.slint`: add `in-out property <float> fresnel-mix: 0.3;` and a "Fresnel Mix" slider in the Lighting `GroupBox` after the existing "Glint" slider. Range: min 0.0, max 1.0, default 0.3. Triggers `sliders-changed`. Display value as `round(value * 100) / 100`.
  - In `src/config.rs`: add `fresnel_mix: f32` to `AppConfig` with default `0.3`. Add it to the `Default::default()` implementation. (Existing `#[serde(default)]` on the struct ensures backward compatibility with config files that lack the field.)
  - In `src/main.rs`: in `apply_config_to_window()`, add `window.set_fresnel_mix(config.fresnel_mix)`. In `read_config_from_window()`, add `fresnel_mix: window.get_fresnel_mix()`.
- **Test cases**:
  - Manual: run the app, verify the "Fresnel Mix" slider appears in the Lighting group, moving it triggers a re-render
  - Unit (config): existing `serde_round_trip` and `deserialize_missing_fields` tests must pass (missing `fresnel_mix` fills default 0.3)
- **Verify**: `cargo test config` passes; `cargo run` shows the slider
- **Complexity**: Small

#### Step 1.6: Wire `fresnel_mix` through the rendering callback

- **Files**: `src/renderer/mod.rs`
- **Action**:
  - In `rendering_callback()`, read `win.get_fresnel_mix()` alongside the existing specular reads.
  - Pass it to `build_frame_state()` as the new parameter.
  - Pass it to the `ShadingParams` struct construction.
- **Test cases**: No new automated tests (this is UI glue). Verified manually via the slider.
- **Verify**: `cargo build` succeeds; `cargo clippy` passes
- **Complexity**: Small

#### Step 1.7: Update GPU integration tests for new uniform layout

- **Files**: `tests/render_pipeline.rs`
- **Action**:
  - Update the local `Uniforms` copy: replace `_pad3: [f32; 2]` with `fresnel_mix: f32` and `_pad3: f32`. Size assertion remains 128.
  - Update all `Uniforms` construction sites in tests to populate `fresnel_mix: 0.0` (so existing test behavior is unchanged).
  - Update `UNIFORM_READBACK_SHADER`: replace the `_pad3` field with `fresnel_mix: f32` and `_pad3: f32`. Add `output[16] = uniforms.fresnel_mix;` to the readback (increase output array to 17 floats).
  - Add assertion: `values[16] == 0.5` (or whatever test value is written for fresnel_mix).
- **Test cases** (GPU integration):
  - `uniform_buffer_field_offsets_match_wgsl`: extended to read back `fresnel_mix` and assert it matches the written value
  - Existing tests (`sphere_renders_visible_pixels`, `day_side_brighter_than_night_side`, `single_texture_mode_ignores_night`) must still pass with `fresnel_mix = 0.0`
- **Verify**: `cargo test --test render_pipeline` passes
- **Complexity**: Small

### Phase 2: Add Fresnel to the Fragment Shader

This phase adds both visual changes: Fresnel-modulated specular and Fresnel-driven diffuse color shift. It depends on Phase 1 (the `fresnel_mix` uniform field must be in place).

#### Step 2.1: Add `schlick_fresnel()` function and apply Fresnel to specular

- **Files**: `shaders/sphere.wgsl`
- **Action**:
  - Add a helper function before `fs_main()`:

    ```wgsl
    fn schlick_fresnel(n_dot_v: f32) -> f32 {
        let f0 = 0.02;
        return f0 + (1.0 - f0) * pow(1.0 - n_dot_v, 5.0);
    }
    ```

  - In the specular block of `fs_main()`, after computing the view vector `v` and before the final `glint` calculation:
    - Compute `let n_dot_v = max(dot(n, v), 0.0);`
    - Compute `let fresnel = schlick_fresnel(n_dot_v);`
    - Modify the glint calculation to include the Fresnel term: change `spec * max(n_dot_l, 0.0) * result.blend * uniforms.spec_intensity * water` to `spec * fresnel * max(n_dot_l, 0.0) * result.blend * uniforms.spec_intensity * water`
- **Test cases** (GPU integration, in `tests/render_pipeline.rs`):
  - `fresnel_specular_brighter_at_grazing`: render two frames with an all-water texture (alpha=128) and spec_intensity=0.5, sun facing camera (sun_dir=[0,0,1]), at two different eye positions: one head-on (eye at [0,0,3.5]) and one at a grazing angle (eye at [2.5,0,2.5]). The grazing-angle frame should have higher average luminance on the visible water pixels near the limb, since Fresnel increases specular at glancing incidence.
  - `fresnel_specular_zero_intensity_unchanged`: render with spec_intensity=0.0. Output must match the pre-Fresnel baseline pixel-for-pixel (Fresnel term is multiplied by zero, so no change).
  - Existing specular tests (from PR #9): must continue to pass. The Fresnel modulation changes exact pixel values but not behavioral invariants (specular present on water, absent on land, absent at night, absent in single-texture mode).
- **Verify**: `cargo test --test render_pipeline` passes
- **Complexity**: Small

#### Step 2.2: Add Fresnel-driven diffuse color shift for water pixels

- **Files**: `shaders/sphere.wgsl`
- **Action**:
  - In `fs_main()`, after the existing specular block (which computes `fresnel` and `water`), add the diffuse color shift:

    ```wgsl
    // Fresnel-driven diffuse color shift: at grazing angles, mix ocean
    // color toward a sky reflection color, simulating reduced
    // transmission and increased sky reflection on real water.
    if uniforms.fresnel_mix > 0.0 && water > 0.0 {
        let sky_color = vec3<f32>(0.5, 0.7, 0.9);
        let n_dot_v_diffuse = max(dot(n, normalize(uniforms.eye_pos - in.world_normal)), 0.0);
        let fresnel_diffuse = schlick_fresnel(n_dot_v_diffuse);
        color = mix(color, sky_color * result.blend, fresnel_diffuse * uniforms.fresnel_mix * water);
    }
    ```

  - Note: `n_dot_v` and `fresnel` are already computed inside the `if uniforms.spec_intensity > 0.0` block. To reuse them for the diffuse shift (which is controlled independently), the code must either: (a) move the view vector and Fresnel computation outside the specular guard, or (b) recompute them in the diffuse shift block. Option (a) is cleaner since both blocks need the same values. Refactor: move the `water`, `v`, `n_dot_v`, and `fresnel` computations to before the specular `if` block, and guard only the specular-specific code (half vector, spec, glint) inside `if uniforms.spec_intensity > 0.0`.
  - The `sky_color` constant is hardcoded initially. It can be promoted to a uniform later if user control is desired.
  - The `result.blend` factor gates the sky color so it fades to zero on the night side (no sky reflection in darkness).
- **Test cases** (GPU integration, in `tests/render_pipeline.rs`):
  - `fresnel_diffuse_shift_brightens_grazing_water`: render two frames with an all-water texture, fresnel_mix=0.5, same camera but one head-on and one at a grazing angle. The grazing-angle frame should have brighter ocean pixels than the head-on frame (the sky color is lighter than the ocean base).
  - `fresnel_diffuse_shift_zero_is_noop`: render with fresnel_mix=0.0. Output must match the baseline (no diffuse shift applied).
  - `fresnel_diffuse_shift_absent_on_land`: render with an all-land texture (alpha=255), fresnel_mix=0.5. Output must match fresnel_mix=0.0 (land is unaffected).
  - `fresnel_diffuse_shift_absent_at_night`: render with all-water, sun pointing away (sun_dir=[0,0,-1]), fresnel_mix=0.5. Night-side pixels should be unchanged because `result.blend` is 0 on the night side, so `sky_color * result.blend` is zero.
- **Verify**: `cargo test --test render_pipeline` passes
- **Complexity**: Medium

#### Step 2.3: Verify full test suite and clippy

- **Files**: All
- **Action**: Run the full test suite and linter to confirm no regressions.
- **Verify**: `cargo test` passes; `cargo clippy` passes
- **Complexity**: Small

### Phase 3: Tuning and Polish

#### Step 3.1: Visual tuning of default parameters and shader constants

- **Files**: `ui/main.slint` (default value), `shaders/sphere.wgsl` (sky_color constant)
- **Action**: Run the app and visually evaluate the Fresnel effects at multiple zoom levels, sun angles, and globe rotations. Adjust:
  - Default `fresnel-mix` value (currently 0.3) if the effect is too strong or too subtle
  - The hardcoded `sky_color` constant (currently `vec3(0.5, 0.7, 0.9)`) if the color shift looks wrong
  - Verify interaction with existing specular glint at various shininess/intensity settings
- **Manual test cases**:
  - Ocean near the globe limb appears brighter and more silvery than the center
  - The color shift is a gradual transition, not a hard edge
  - Land areas are completely unaffected
  - The specular highlight is more intense at grazing angles than head-on
  - Setting Fresnel Mix to 0.0 removes the diffuse shift (specular Fresnel remains)
  - Setting both Glint and Fresnel Mix to 0.0 matches the pre-change appearance
  - Night side shows no Fresnel effects
  - The terminator transition is smooth with no artifacts
  - Zooming in/out: the effect scales naturally
  - "Set as Wallpaper" export includes the Fresnel effects
- **Verify**: Visual inspection at multiple camera/sun configurations
- **Complexity**: Small

#### Step 3.2: Update CLAUDE.md if needed

- **Files**: `CLAUDE.md`
- **Action**: If any architectural details changed (uniform size, new shader function, new UI controls), update the relevant sections. Specifically:
  - The Uniforms struct comment in the Architecture section should mention `fresnel_mix`
  - The dirty-checking list should include `fresnel_mix`
  - The UI description should mention the Fresnel Mix slider in the Lighting group
- **Verify**: CLAUDE.md accurately describes the current state
- **Complexity**: Small

## Test Strategy

### Automated Tests

| Test Case | Type | Location | Description |
| --- | --- | --- | --- |
| Uniform size assertion | Compile-time | `src/renderer/uniforms.rs` | `size_of::<Uniforms>() == 128` (unchanged) |
| Fresnel mix quantization | Unit | `src/renderer/frame.rs` | `fresnel_mix: 0.5` -> quantized `500` |
| Fresnel mix triggers dirty | Unit | `src/renderer/frame.rs` | Different fresnel_mix -> different FrameState |
| All existing frame tests | Unit | `src/renderer/frame.rs` | Pass with new parameter at default 0.0 |
| Config serde round trip | Unit | `src/config.rs` | fresnel_mix survives serialize/deserialize |
| Config missing field fills default | Unit | `src/config.rs` | Old config without fresnel_mix gets default 0.3 |
| Uniform field offsets (extended) | GPU integration | `tests/render_pipeline.rs` | Read back fresnel_mix via compute shader |
| Existing render tests unchanged | GPU integration | `tests/render_pipeline.rs` | Pass with fresnel_mix=0.0 |
| Fresnel specular brighter at grazing | GPU integration | `tests/render_pipeline.rs` | Grazing-angle water brighter than head-on |
| Fresnel specular zero intensity unchanged | GPU integration | `tests/render_pipeline.rs` | spec_intensity=0 matches baseline |
| Fresnel diffuse shift brightens grazing water | GPU integration | `tests/render_pipeline.rs` | Grazing-angle ocean brighter with fresnel_mix>0 |
| Fresnel diffuse shift zero is noop | GPU integration | `tests/render_pipeline.rs` | fresnel_mix=0 matches baseline |
| Fresnel diffuse shift absent on land | GPU integration | `tests/render_pipeline.rs` | All-land texture unaffected by fresnel_mix |
| Fresnel diffuse shift absent at night | GPU integration | `tests/render_pipeline.rs` | Night-side water unaffected by fresnel_mix |
| All existing shading tests | GPU integration | `tests/shading.rs` | Unchanged (blend_fragment not modified) |

### Manual Verification

- [ ] "Fresnel Mix" slider appears in the Lighting group below "Glint"
- [ ] Moving the slider triggers a re-render in real time
- [ ] Ocean near the globe limb shifts toward silvery/sky-blue at moderate fresnel_mix
- [ ] Specular highlight intensifies at grazing angles compared to pre-change
- [ ] Land areas show zero Fresnel effects at any slider value
- [ ] Night-side ocean shows no Fresnel effects
- [ ] Setting Fresnel Mix to 0.0 removes diffuse shift (Fresnel-modulated specular remains)
- [ ] Setting Glint to 0.0 removes specular (Fresnel diffuse shift remains if fresnel_mix > 0)
- [ ] Setting both to 0.0 produces pre-change appearance
- [ ] Terminator transition is smooth, no artifacts at the day/night boundary
- [ ] "Set as Wallpaper" export includes Fresnel effects
- [ ] Config persistence: change Fresnel Mix, restart app, slider restores the value
- [ ] App launches correctly with a config file that lacks the fresnel_mix field (backward compat)

## Risks and Mitigations

| Risk | Impact | Mitigation |
| --- | --- | --- |
| Uniform buffer alignment mismatch after repurposing `_pad3` | Shader reads wrong value for `fresnel_mix` | GPU readback test (Step 1.7) catches this immediately. The replacement is at the same offset as the old padding field. |
| Fresnel brightening interacts badly with night-side blend near terminator | Bright artifacts at the terminator edge | The diffuse shift is multiplied by `result.blend`, which is 0 on the night side and transitions smoothly via smoothstep. The specular Fresnel is similarly gated by `max(n_dot_l, 0.0) * result.blend`. Both are zero on the night side. |
| Sky color constant looks wrong with current ocean fill color | Unnatural color shift | The sky color `vec3(0.5, 0.7, 0.9)` is a starting point. Phase 3 allows visual tuning. The Fresnel Mix slider gives the user control over intensity. |
| Existing GPU tests break due to Fresnel changing specular intensity | False test failures | All existing tests use `fresnel_mix: 0.0`. Fresnel modulation of specular changes absolute values but existing tests assert behavioral invariants (monotonicity, bounds, visibility), not exact pixel values. Tests with `spec_intensity: 0.0` are completely unaffected since `0 * fresnel = 0`. |
| Backward compatibility: old config files lack `fresnel_mix` | Config load failure or missing default | `AppConfig` uses `#[serde(default)]` at the struct level, so missing fields fill from `Default::default()`. The default is 0.3. Existing config tests verify this pattern. |
| Fresnel effect too subtle to notice at typical viewing distances | Wasted implementation effort | The research documents that Fresnel is one of the most recognizable visual signatures of water from space. The diffuse color shift operates across the entire visible ocean surface (not just the specular hotspot), so it should be clearly visible. The slider default of 0.3 is conservative; users can increase it. |

## Rollback Strategy

All changes are in the `water-appearance` worktree branch. To rollback:

1. Revert the branch or delete it
2. The main branch is untouched
3. No database migrations or external service dependencies

The Fresnel effects can also be "soft disabled" without reverting code:

- Setting Fresnel Mix to 0.0 disables the diffuse color shift
- The Fresnel modulation of specular can be removed by reverting only the shader change in `sphere.wgsl` (the uniform field becomes unused padding, which is harmless)

## Technical Notes

### Uniform buffer layout change

The current 128-byte uniform buffer has 8 bytes of padding at offset 120 (`_pad3: [f32; 2]` / `_pad3: vec2<f32>`). This plan repurposes the first 4 bytes for `fresnel_mix`, keeping the second 4 bytes as `_pad3: f32`. The total size remains 128 bytes, which is already a multiple of 16 (std140 alignment requirement). No pipeline layout, bind group layout, or buffer allocation changes are needed.

Before:

```text
offset 120: _pad3[0]: f32   (padding)
offset 124: _pad3[1]: f32   (padding)
```

After:

```text
offset 120: fresnel_mix: f32
offset 124: _pad3: f32       (padding)
```

### Shader refactoring for shared Fresnel computation

The existing specular block computes the view vector `v` and water mask inside `if uniforms.spec_intensity > 0.0`. The diffuse color shift needs these same values but is controlled independently by `fresnel_mix`. Step 2.2 moves the shared computations (water mask decode, view vector, n_dot_v, Fresnel term) before the specular guard, so both the specular and diffuse shift blocks can use them. The specular-specific code (half vector, Blinn-Phong pow, glint additive term) remains inside the `spec_intensity > 0.0` guard.

This refactoring replaces the current structure:

```wgsl
if uniforms.spec_intensity > 0.0 {
    let water = ...;
    if water > 0.0 {
        let v = ...;
        let h = ...;
        // specular calculation
    }
}
```

With:

```wgsl
let water = ...;
if water > 0.0 {
    let v = ...;
    let n_dot_v = ...;
    let fresnel = schlick_fresnel(n_dot_v);

    // Specular sun glint (Blinn-Phong with Fresnel)
    if uniforms.spec_intensity > 0.0 {
        let h = ...;
        // specular calculation using fresnel
    }

    // Fresnel-driven diffuse color shift
    if uniforms.fresnel_mix > 0.0 {
        // diffuse shift calculation using fresnel
    }
}
```

This ensures that `water == 0.0` (land pixels) short-circuits both effects with a single branch.

## Status

- [x] Plan approved
- [x] Phase 1 complete (uniform buffer + UI + config plumbing)
- [x] Phase 2 complete (Fresnel shader logic + tests)
- [x] Phase 3 complete (visual tuning + CLAUDE.md update)
- [x] Implementation complete
