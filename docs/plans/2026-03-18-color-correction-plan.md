# Plan: Color Correction (2026-03-18)

## Summary

Add interactive per-texture color correction (gamma and saturation) to the fragment shader, controlled by four new UI sliders organized into a "Color Correction" GroupBox with Day Texture and Night Texture sub-sections. Each parameter is threaded through the full Slint-to-WGSL data pipeline following the established pattern: Slint property, slider, AppConfig, read/write config functions, reset-all handler, BeforeRendering read, FrameState dirty-check, ShadingParams, Uniforms, WGSL. The uniform buffer grows from 128 to 144 bytes. Two new WGSL helper functions (`apply_gamma` and `adjust_saturation`) are added to `blend.wgsl` and called in `fs_main` after texture sampling but before blending and shading. All four parameters default to 1.0 (identity), so existing users see no visual change.

## Stakes Classification

**Level**: Medium
**Rationale**: The change touches many files across the codebase (two shaders, six Rust source files, one Slint file, two test files) but each individual change is small and follows the exact pattern established by `fresnel_mix` and `fresnel_exp` in previous PRs. The uniform buffer grows by 16 bytes (128 to 144), which requires updating the size assertion and the WGSL struct, but the new size is already 16-byte aligned so no extra padding is needed. The feature is additive -- all parameters default to identity (1.0), producing output identical to the pre-change state. Rollback is straightforward: revert the branch.

## Context

**Research**: [`docs/plans/2026-03-18-color-correction-research.md`](2026-03-18-color-correction-research.md)
**Prior work**: [`docs/plans/2026-03-17-water-appearance-plan.md`](2026-03-17-water-appearance-plan.md) (Fresnel, most recent data-plumbing example)
**Affected Areas**:

- `shaders/blend.wgsl` -- add `apply_gamma()` and `adjust_saturation()` helper functions
- `shaders/sphere.wgsl` -- add four fields to WGSL `Uniforms` struct, apply color correction in `fs_main`
- `src/renderer/uniforms.rs` -- add four `f32` fields, update size assertion from 128 to 144
- `src/renderer/render_pass.rs` -- add four fields to `ShadingParams`, populate in `write_uniforms()`
- `src/renderer/frame.rs` -- add four fields to `FrameState` and `build_frame_state()`
- `src/renderer/mod.rs` -- read four properties from UI, wire through to `ShadingParams` and `build_frame_state()`
- `src/config.rs` -- add four fields to `AppConfig` for persistence
- `src/main.rs` -- read/write in `apply_config_to_window()`, `read_config_from_window()`, `on_reset_all()`
- `ui/main.slint` -- add four `in-out property` declarations and a "Color Correction" GroupBox with sliders
- `tests/render_pipeline.rs` -- update local `Uniforms` copy and readback shader, add color correction behavioral tests
- `tests/shading.rs` -- no changes needed (`blend_fragment()` is unmodified)

## Success Criteria

- [ ] Four new sliders (Day Gamma, Day Saturation, Night Gamma, Night Saturation) appear in a "Color Correction" GroupBox
- [ ] Moving any slider triggers a re-render in real time
- [ ] All four parameters default to 1.0 (identity) -- existing users see no visual change
- [ ] Day gamma/saturation affect only the day texture sample, night gamma/saturation affect only the night texture sample
- [ ] Color correction is applied before blending and shading (so diffuse, Fresnel, and specular operate on corrected colors)
- [ ] Single-texture mode (grid, day-only, night-only) applies day correction to the active texture
- [ ] Setting all four sliders to 1.0 produces output identical to the pre-change state
- [ ] Gamma with value > 1.0 visibly brightens midtones; < 1.0 darkens midtones
- [ ] Saturation with value 0.0 produces greyscale; > 1.0 oversaturates
- [ ] The "Reset All" button resets all four parameters to 1.0
- [ ] All four parameters are persisted to config and restored on startup
- [ ] All existing tests pass with new parameters at identity (1.0)
- [ ] New GPU integration tests verify color correction behavioral invariants
- [ ] `cargo clippy` passes with no new warnings
- [ ] `cargo test` passes

## Implementation Steps

### Phase 1: Extend Uniform Buffer and Rust Data Plumbing

This phase adds the four new fields through every layer of the Rust data pipeline without any visual change to the shader. The shader ignores the new fields until Phase 2.

#### Step 1.1: Add four fields to the Rust `Uniforms` struct

- **Files**: `src/renderer/uniforms.rs`
- **Action**: Append four `f32` fields after `fresnel_exp` at offset 128: `day_gamma`, `day_saturation`, `night_gamma`, `night_saturation`. Update the compile-time size assertion from `== 128` to `== 144`. No padding fields are needed because 144 is already a multiple of 16.
- **Test cases** (compile-time):
  - Size assertion: `size_of::<Uniforms>() == 144` (must pass)
  - The struct must remain `Pod` and `Zeroable` (compile error if not)
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

#### Step 1.2: Add four fields to `ShadingParams` and `write_uniforms()`

- **Files**: `src/renderer/render_pass.rs`
- **Action**:
  - Add `day_gamma: f32`, `day_saturation: f32`, `night_gamma: f32`, `night_saturation: f32` to the `ShadingParams` struct.
  - In `write_uniforms()`, populate the four new fields from `shading.*` in the `Uniforms` construction.
- **Test cases**: Deferred to Step 3.1 (GPU readback test).
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

#### Step 1.3: Add four fields to `FrameState` and `build_frame_state()`

- **Files**: `src/renderer/frame.rs`
- **Action**:
  - Add `day_gamma: i32`, `day_saturation: i32`, `night_gamma: i32`, `night_saturation: i32` to `FrameState` (quantized to integer thousandths, matching the existing pattern).
  - Add four `f32` parameters to `build_frame_state()`, quantize each as `(value * 1000.0) as i32`.
  - Update all existing call sites in the test module to pass the four new parameters as `1.0` (identity).
- **Test cases** (unit, in the existing `frame::tests` module):
  - `frame_state_day_gamma_quantization`: `build_frame_state(..., day_gamma: 1.5)` produces `day_gamma == 1500`
  - `frame_state_day_saturation_quantization`: `build_frame_state(..., day_saturation: 0.5)` produces `day_saturation == 500`
  - `frame_state_color_correction_triggers_dirty`: changing each of the four parameters from 1.0 to a different value produces `state_a != state_b` (four separate assertions)
  - All existing tests: updated with the four new parameters at 1.0 and continue to pass
- **Verify**: `cargo test frame` passes
- **Complexity**: Small

#### Step 1.4: Wire the four parameters through the rendering callback

- **Files**: `src/renderer/mod.rs`
- **Action**:
  - In `rendering_callback()` within the `BeforeRendering` arm, read `win.get_day_gamma()`, `win.get_day_saturation()`, `win.get_night_gamma()`, `win.get_night_saturation()` alongside the existing shading reads (around line 342-349).
  - Pass the four values to `build_frame_state()` as the new parameters.
  - Pass the four values to the `ShadingParams` struct construction (around line 410-421).
- **Test cases**: No new automated tests (this is UI glue). Verified manually via the sliders.
- **Verify**: `cargo build` succeeds; `cargo clippy` passes
- **Complexity**: Small

#### Step 1.5: Add four fields to `AppConfig` for persistence

- **Files**: `src/config.rs`
- **Action**:
  - Add `day_gamma: f32`, `day_saturation: f32`, `night_gamma: f32`, `night_saturation: f32` to `AppConfig` under a `// Color correction` comment, with defaults of `1.0` in `Default::default()`.
  - The existing `#[serde(default)]` on the struct ensures backward compatibility with config files that lack these fields.
- **Test cases** (unit, in the existing `config::tests` module):
  - `default_values_color_correction`: `AppConfig::default()` has all four fields equal to `1.0`
  - Existing `serde_round_trip` test: must still pass (new fields are serialized and deserialized)
  - Existing `deserialize_missing_fields` test: must still pass (missing fields fill defaults)
  - `serde_round_trip_non_default` test: update to include non-default color correction values
- **Verify**: `cargo test config` passes
- **Complexity**: Small

#### Step 1.6: Add read/write/reset in `main.rs`

- **Files**: `src/main.rs`
- **Action**:
  - In `apply_config_to_window()` (around line 286-303): add `window.set_day_gamma(config.day_gamma)`, and similarly for the other three parameters.
  - In `read_config_from_window()` (around line 323-357): add `day_gamma: window.get_day_gamma()`, and similarly for the other three parameters.
  - In `on_reset_all()` (around line 220-251): add `win.set_day_gamma(lighting.day_gamma)`, and similarly for the other three parameters, alongside the existing lighting resets.
- **Test cases**: No new automated tests (UI glue). Manual verification in Phase 4.
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

#### Step 1.7: Add UI properties and sliders

- **Files**: `ui/main.slint`
- **Action**:
  - Add four `in-out property <float>` declarations with default 1.0: `day-gamma`, `day-saturation`, `night-gamma`, `night-saturation` (alongside the existing lighting properties, around line 43-50).
  - Add a new "Color Correction" `GroupBox` section between the existing "Lighting" GroupBox and the "Date / Time" GroupBox. Organize with two visual sub-groups using `Text` labels:
    - "Day Texture" label, then Day Gamma slider (min 0.2, max 3.0) and Day Saturation slider (min 0.0, max 2.0)
    - "Night Texture" label, then Night Gamma slider (min 0.2, max 3.0) and Night Saturation slider (min 0.0, max 2.0)
  - Each slider uses the established pattern: `value <=> root.property`, `changed => { root.sliders-changed(); }`, and a display `Text` showing `round(value * 100) / 100`.
- **Test cases**: Manual verification in Phase 4.
- **Verify**: `cargo build` succeeds; `cargo run` shows the four sliders in the new GroupBox
- **Complexity**: Small

### Phase 2: Add Color Correction to the Fragment Shader

This phase adds the visual changes. It depends on Phase 1 (the four uniform fields must be in place).

#### Step 2.1: Add helper functions to `blend.wgsl`

- **Files**: `shaders/blend.wgsl`
- **Action**: Add two helper functions after the existing `BlendResult` struct and before `blend_fragment()`:

  ```wgsl
  fn apply_gamma(color: vec3<f32>, gamma: f32) -> vec3<f32> {
      return pow(max(color, vec3<f32>(0.0)), vec3<f32>(1.0 / gamma));
  }

  fn adjust_saturation(color: vec3<f32>, saturation: f32) -> vec3<f32> {
      let luminance = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
      return mix(vec3<f32>(luminance), color, saturation);
  }
  ```

  The `max(color, vec3(0.0))` guard in `apply_gamma` prevents undefined behavior from `pow` of negative values in WGSL.
- **Test cases**: Deferred to Step 2.3 (GPU integration tests exercise these functions end-to-end).
- **Verify**: `cargo build` succeeds (the shader compiles as part of `include_str!` concatenation)
- **Complexity**: Small

#### Step 2.2: Add four fields to WGSL `Uniforms` struct and apply correction in `fs_main`

- **Files**: `shaders/sphere.wgsl`
- **Action**:
  - Add four fields to the WGSL `Uniforms` struct after `fresnel_exp`, with offset comments:

    ```wgsl
    day_gamma: f32,           // 4 bytes, offset 128
    day_saturation: f32,      // 4 bytes, offset 132
    night_gamma: f32,         // 4 bytes, offset 136
    night_saturation: f32,    // 4 bytes, offset 140
    ```

  - In `fs_main`, after sampling the day texture (line 62) and before the single-texture early return (line 66-68), apply day color correction:

    ```wgsl
    var day_rgb = apply_gamma(day.rgb, uniforms.day_gamma);
    day_rgb = adjust_saturation(day_rgb, uniforms.day_saturation);
    ```

  - Update the single-texture early return to use `day_rgb` instead of `day.rgb`.
  - After sampling the night texture (line 70), apply night color correction:

    ```wgsl
    var night_rgb = apply_gamma(night_color, uniforms.night_gamma);
    night_rgb = adjust_saturation(night_rgb, uniforms.night_saturation);
    ```

  - Update the `blend_fragment` call (line 74-80) to pass `day_rgb` and `night_rgb` instead of `day.rgb` and `night_color`.
  - Note: the `day.a` alpha channel used for the water mask (line 86) must NOT be modified by color correction -- only the RGB channels are corrected, which is already the case since `apply_gamma` and `adjust_saturation` operate on `vec3`.
- **Test cases**: GPU integration tests in Step 2.3.
- **Verify**: `cargo build` succeeds; `cargo run` shows color correction effects when sliders are moved
- **Complexity**: Medium

#### Step 2.3: Verify full test suite

- **Files**: All
- **Action**: Run the full test suite and linter. All existing tests should pass because the new parameters default to identity (1.0), and `pow(x, 1.0) = x` and `mix(grey, color, 1.0) = color` are identity operations.
- **Verify**: `cargo test` passes; `cargo clippy` passes
- **Complexity**: Small

### Phase 3: Update GPU Integration Tests

This phase adds new tests and updates existing test infrastructure for the expanded uniform buffer.

#### Step 3.1: Update `tests/render_pipeline.rs` for new uniform layout

- **Files**: `tests/render_pipeline.rs`
- **Action**:
  - Update the local `Uniforms` struct: add four `f32` fields (`day_gamma`, `day_saturation`, `night_gamma`, `night_saturation`) after `fresnel_exp`. Update the size assertion from `== 128` to `== 144`.
  - Update all `Uniforms` construction sites in existing tests to populate the four new fields with `1.0` (identity, so existing test behavior is unchanged).
  - Update `UNIFORM_READBACK_SHADER`: add the four new fields to the WGSL `Uniforms` struct, add four output assignments (`output[18]` through `output[21]`), increase output array to 22 floats.
  - Update the `uniform_buffer_field_offsets_match_wgsl` test: write known non-identity values for the four new fields, read them back, and assert they match.
- **Test cases** (GPU integration):
  - `uniform_buffer_field_offsets_match_wgsl`: extended to read back all four color correction fields and assert they match the written values (e.g., `day_gamma: 1.5` reads back as `1.5`)
  - All existing render tests (`sphere_renders_visible_pixels`, `day_side_brighter_than_night_side`, `single_texture_mode_ignores_night`, Fresnel tests): must still pass with color correction at identity (1.0)
- **Verify**: `cargo test --test render_pipeline` passes
- **Complexity**: Small

#### Step 3.2: Add GPU integration tests for gamma behavior

- **Files**: `tests/render_pipeline.rs`
- **Action**: Add tests that verify gamma correction behavioral invariants by rendering with different gamma values and comparing luminance.
- **Test cases** (GPU integration):
  - `gamma_above_one_brightens`: render two frames with the same day texture (mid-grey, e.g., `[128, 128, 128, 255]`), one with `day_gamma: 1.0` and one with `day_gamma: 2.0`, in single-texture mode (`terminator_width: -1.0`). The `gamma: 2.0` frame should have higher average luminance on visible pixels. (Gamma > 1.0 raises `pow(x, 0.5)` which brightens midtones.)
  - `gamma_below_one_darkens`: same setup but with `day_gamma: 0.5`. The `gamma: 0.5` frame should have lower average luminance than `gamma: 1.0`.
  - `gamma_identity_unchanged`: render with `day_gamma: 1.0`. Output luminance should match the baseline (no color correction). This is a regression guard.
- **Verify**: `cargo test --test render_pipeline` passes
- **Complexity**: Small

#### Step 3.3: Add GPU integration tests for saturation behavior

- **Files**: `tests/render_pipeline.rs`
- **Action**: Add tests that verify saturation correction behavioral invariants.
- **Test cases** (GPU integration):
  - `saturation_zero_produces_greyscale`: render with a colorful day texture (e.g., `[255, 0, 0, 255]` red) and `day_saturation: 0.0` in single-texture mode. All non-clear pixels should have R == G == B (within GPU float tolerance of ~2 u8 levels), because `mix(vec3(luminance), color, 0.0) = vec3(luminance)`.
  - `saturation_identity_unchanged`: render with `day_saturation: 1.0`. Output should match the baseline rendered with no color correction applied. This is a regression guard.
  - `saturation_above_one_increases_chroma`: render a moderately saturated texture (e.g., `[200, 100, 50, 255]`) with `day_saturation: 1.0` and `day_saturation: 2.0`. Compute average chroma (max(R,G,B) - min(R,G,B)) across visible pixels. The `saturation: 2.0` frame should have higher average chroma than `saturation: 1.0`.
- **Verify**: `cargo test --test render_pipeline` passes
- **Complexity**: Small

#### Step 3.4: Add GPU integration test for night texture correction independence

- **Files**: `tests/render_pipeline.rs`
- **Action**: Add a test that verifies day and night corrections are independent.
- **Test cases** (GPU integration):
  - `night_gamma_does_not_affect_single_texture_mode`: render in single-texture mode (`terminator_width: -1.0`) with `night_gamma: 0.3` (extreme darkening). Output should be pixel-identical to `night_gamma: 1.0`, because single-texture mode uses only the day texture and the night correction should not apply.
  - `day_and_night_corrections_independent`: render in blend mode with sun along +Z, white day texture, mid-grey night texture. Render twice: once with `day_gamma: 2.0, night_gamma: 1.0` and once with `day_gamma: 1.0, night_gamma: 2.0`. The two frames should produce different pixel data, confirming the corrections target different textures.
- **Verify**: `cargo test --test render_pipeline` passes
- **Complexity**: Small

### Phase 4: Visual Tuning and Documentation

#### Step 4.1: Visual verification and tuning

- **Files**: `ui/main.slint` (slider defaults/ranges if needed), `shaders/sphere.wgsl` (if insertion point needs adjustment)
- **Action**: Run the app and visually evaluate the color correction at multiple zoom levels, sun angles, and globe rotations. Verify:
  - Gamma slider above 1.0 visibly brightens, below 1.0 darkens
  - Saturation slider at 0.0 produces greyscale, above 1.0 oversaturates
  - Day and night corrections are visually independent
  - No artifacts at the terminator, limb, or in single-texture modes
  - Interaction with Fresnel, specular glint, and diffuse shading looks correct
  - "Set as Wallpaper" export includes the color correction
- **Manual test cases**:
  - [ ] "Color Correction" GroupBox appears between Lighting and Date/Time
  - [ ] Day Gamma slider: drag to 2.0, day side brightens noticeably
  - [ ] Day Gamma slider: drag to 0.5, day side darkens
  - [ ] Day Saturation slider: drag to 0.0, day side becomes greyscale
  - [ ] Day Saturation slider: drag to 2.0, day side colors become vivid
  - [ ] Night Gamma slider: drag to 2.0, night side (city lights) brightens
  - [ ] Night Saturation slider: drag to 0.0, night side becomes greyscale
  - [ ] Single-texture mode (Grid/Day/Night): only day correction applies
  - [ ] Setting all four sliders to 1.0 matches the pre-change appearance
  - [ ] "Reset All" resets all four sliders to 1.0
  - [ ] Terminator transition is smooth, no artifacts
  - [ ] Water Fresnel and specular glint look correct on corrected colors
  - [ ] "Set as Wallpaper" includes color correction
  - [ ] Config persistence: change values, restart app, sliders restore correctly
  - [ ] App launches with an old config file lacking color correction fields (backward compat)
- **Verify**: Visual inspection at multiple camera/sun configurations
- **Complexity**: Small

#### Step 4.2: Update CLAUDE.md

- **Files**: `CLAUDE.md`
- **Action**: Update the following sections to reflect the new state:
  - **Shader** description: mention `apply_gamma()` and `adjust_saturation()` in `blend.wgsl`
  - **UI** description: add "Color Correction (Day Gamma, Day Saturation, Night Gamma, Night Saturation)" to the GroupBox list
  - **Dirty-checking** list: add `day_gamma`, `day_saturation`, `night_gamma`, `night_saturation`
  - **Uniforms** description if mentioned: update size from 128 to 144 bytes
- **Verify**: CLAUDE.md accurately describes the current state
- **Complexity**: Small

## Test Strategy

### Automated Tests

| Test Case | Type | Location | Description |
| --- | --- | --- | --- |
| Uniform size assertion | Compile-time | `src/renderer/uniforms.rs` | `size_of::<Uniforms>() == 144` |
| Day gamma quantization | Unit | `src/renderer/frame.rs` | `day_gamma: 1.5` -> quantized `1500` |
| Day saturation quantization | Unit | `src/renderer/frame.rs` | `day_saturation: 0.5` -> quantized `500` |
| Color correction triggers dirty | Unit | `src/renderer/frame.rs` | Changing each parameter triggers dirty |
| All existing frame tests | Unit | `src/renderer/frame.rs` | Pass with new parameters at 1.0 |
| Config default values | Unit | `src/config.rs` | All four fields default to 1.0 |
| Config serde round trip | Unit | `src/config.rs` | Color correction fields survive serialize/deserialize |
| Config missing field fills default | Unit | `src/config.rs` | Old config without color correction fields gets 1.0 |
| Uniform field offsets (extended) | GPU integration | `tests/render_pipeline.rs` | Read back all four fields via compute shader |
| Existing render tests unchanged | GPU integration | `tests/render_pipeline.rs` | Pass with color correction at identity (1.0) |
| Gamma above one brightens | GPU integration | `tests/render_pipeline.rs` | Higher gamma -> higher luminance |
| Gamma below one darkens | GPU integration | `tests/render_pipeline.rs` | Lower gamma -> lower luminance |
| Gamma identity unchanged | GPU integration | `tests/render_pipeline.rs` | Gamma 1.0 matches baseline |
| Saturation zero produces greyscale | GPU integration | `tests/render_pipeline.rs` | R == G == B on all visible pixels |
| Saturation identity unchanged | GPU integration | `tests/render_pipeline.rs` | Saturation 1.0 matches baseline |
| Saturation above one increases chroma | GPU integration | `tests/render_pipeline.rs` | Higher saturation -> higher chroma |
| Night gamma independent of single-texture | GPU integration | `tests/render_pipeline.rs` | Night gamma has no effect in single-texture mode |
| Day and night corrections independent | GPU integration | `tests/render_pipeline.rs` | Different corrections produce different output |
| All existing shading tests | GPU integration | `tests/shading.rs` | Unchanged (`blend_fragment` not modified) |

### Manual Verification

- [ ] "Color Correction" GroupBox appears in the expected position in the controls panel
- [ ] All four sliders trigger re-render in real time
- [ ] Gamma above 1.0 brightens midtones, below 1.0 darkens
- [ ] Saturation 0.0 produces greyscale, above 1.0 oversaturates
- [ ] Day and night corrections are visually independent
- [ ] Single-texture mode uses only day correction
- [ ] All four at identity (1.0) matches pre-change appearance
- [ ] Reset All resets all four to 1.0
- [ ] No artifacts at terminator, limb, or with water effects
- [ ] "Set as Wallpaper" includes color correction
- [ ] Config persists across restarts
- [ ] Old config files without color correction fields load correctly

## Risks and Mitigations

| Risk | Impact | Mitigation |
| --- | --- | --- |
| Uniform buffer alignment mismatch between Rust and WGSL | Shader reads wrong values for all four fields | GPU readback test (Step 3.1) catches this immediately. The four new `f32` fields are appended after `fresnel_exp` at offset 128, and 144 bytes is a multiple of 16 (std140 alignment). |
| `pow` of negative value produces NaN in WGSL | Black pixels, corrupted output | The `max(color, vec3(0.0))` guard in `apply_gamma` is non-negotiable (documented in research). The research confirms negative channel values can occur from arithmetic. |
| Gamma/saturation applied after blending instead of before | Cannot independently correct day vs. night; darkened night-side gets incorrectly gamma-lifted | The plan explicitly specifies insertion after `textureSample` and before `blend_fragment`. Code review and GPU tests confirm the correction targets the correct samples. |
| Single-texture early return path misses color correction | Grid/Day/Night texture modes skip correction | Step 2.2 explicitly applies day correction before the early return on line 66-68. The GPU test in Step 3.4 verifies this. |
| `build_frame_state` argument list grows unwieldy (now 18+ parameters) | Clippy warning, readability | The function already has `#[allow(clippy::too_many_arguments)]`. The research notes this and suggests grouping into a struct as future cleanup. This is acceptable for now. |
| Backward compatibility: old config files lack color correction fields | Config load failure or wrong defaults | `AppConfig` uses `#[serde(default)]` at the struct level, so missing fields fill from `Default::default()`. The defaults are 1.0 (identity). Existing config tests verify this pattern. |
| Existing GPU tests break due to new uniform size | Test compilation or assertion failures | Step 3.1 updates the local `Uniforms` copy in `tests/render_pipeline.rs` to 144 bytes. All existing test constructions populate the new fields at 1.0 (identity), so behavioral invariants are preserved. |
| Performance regression from added shader operations | Slower frame rendering | The research estimates ~5-8 ALU operations per texture per fragment, negligible against a wallpaper renderer that draws one frame per 2-minute timer tick. No branching for identity values is needed -- `pow(x, 1.0) = x` and `mix(grey, color, 1.0) = color` are free. |

## Rollback Strategy

All changes are in the `color-correction` worktree branch. To rollback:

1. Revert the branch or delete it
2. The main branch is untouched
3. No database migrations or external service dependencies

The color correction can also be "soft disabled" without reverting code: setting all four sliders to 1.0 produces the identity transformation, making the feature invisible.

## Technical Notes

### Uniform buffer layout change

The current 128-byte uniform buffer is fully occupied. Adding four `f32` fields (16 bytes) extends it to 144 bytes. Both Rust and WGSL structs must be updated in lockstep.

Before (128 bytes):

```text
offset 0:   mvp: mat4x4<f32>      (64 bytes)
offset 64:  sun_dir: vec3<f32>     (12 bytes)
offset 76:  terminator_width: f32  (4 bytes)
offset 80:  flags: u32             (4 bytes)
offset 84:  diffuse_floor: f32     (4 bytes)
offset 88:  diffuse_ramp: f32      (4 bytes)
offset 92:  _pad: f32              (4 bytes)
offset 96:  eye_pos: vec3<f32>     (12 bytes)
offset 108: _pad2: f32             (4 bytes)
offset 112: spec_shininess: f32    (4 bytes)
offset 116: spec_intensity: f32    (4 bytes)
offset 120: fresnel_mix: f32       (4 bytes)
offset 124: fresnel_exp: f32       (4 bytes)
```

After (144 bytes):

```text
offset 128: day_gamma: f32         (4 bytes)
offset 132: day_saturation: f32    (4 bytes)
offset 136: night_gamma: f32       (4 bytes)
offset 140: night_saturation: f32  (4 bytes)
```

144 is a multiple of 16, satisfying std140 alignment. No extra padding needed.

### Shader insertion point

Color correction is applied in `fs_main` in `sphere.wgsl`:

```text
1. textureSample(sphere_texture, ...) -> day
2. **apply_gamma(day.rgb, day_gamma) -> day_rgb**         <-- NEW
3. **adjust_saturation(day_rgb, day_saturation) -> day_rgb** <-- NEW
4. Single-texture early return: return day_rgb             <-- UPDATED
5. textureSample(night_texture, ...) -> night_color
6. **apply_gamma(night_color, night_gamma) -> night_rgb**    <-- NEW
7. **adjust_saturation(night_rgb, night_saturation) -> night_rgb** <-- NEW
8. blend_fragment(day_rgb, night_rgb, ...)                 <-- UPDATED
9. Water effects (Fresnel, specular) on corrected colors
```

This order (gamma first, then saturation) matches the Lightroom/Photoshop convention documented in the research.

### Application order rationale

Gamma first, then saturation. Gamma corrects the tonal curve (brightness of midtones), then saturation adjusts color intensity on the corrected tones. Reversing the order would mean saturation operates on uncorrected tones and then gamma shifts both luminance and the saturation adjustment, which is less intuitive. Both orders produce acceptable results, but gamma-first is the industry convention (see research document, Section 3).

## Status

- [ ] Plan approved
- [ ] Phase 1 complete (uniform buffer + config + UI plumbing)
- [ ] Phase 2 complete (shader helper functions + fragment shader integration)
- [ ] Phase 3 complete (GPU integration tests)
- [ ] Phase 4 complete (visual tuning + CLAUDE.md update)
- [ ] Implementation complete
