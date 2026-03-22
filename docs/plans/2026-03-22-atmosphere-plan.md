# Plan: Unified Atmosphere Effect (2026-03-22)

## Summary

Add a unified atmosphere shell that renders three distinct atmospheric phenomena on a single concentric sphere slightly larger than Earth: blue Rayleigh scattering on the day-side limb, orange/red spectral depletion at the terminator, and green/yellow chemiluminescent airglow on the night-side limb. The atmosphere renders as a third draw call (between Earth and clouds) using additive blending, following the existing cloud layer architecture exactly. Two user-facing sliders (Intensity and Falloff) control the effect from a new "Atmosphere" GroupBox in the Slint controls panel.

## Stakes Classification

**Level**: Medium

**Rationale**: The change spans 9 files across shader, renderer, UI, and config layers, but follows a well-established pattern (the cloud layer). Every integration point has a direct precedent in the existing cloud pipeline code. No architectural changes are needed, no new dependencies are introduced, and the feature can be disabled by setting intensity to zero. The uniform struct grows by 16 bytes (160 to 176), which is a minor but irreversible change to the GPU interface. Rollback is straightforward via `git revert`.

## Context

**Research**:

- `docs/plans/2026-03-22-airglow-research.md` -- synthesis of codebase analysis and external research, recommended approach, integration checklist
- `docs/plans/2026-03-22-rayleigh-scattering-external.md` -- Rayleigh scattering physics, ISS visual reference, rendering technique survey (7 approaches), WGSL shader pseudocode
- `docs/plans/2026-03-22-airglow-codebase.md` -- detailed codebase analysis: uniform layout, UI data flow, render pass structure, pipeline creation, dirty checking
- `docs/plans/2026-03-22-airglow-external.md` -- airglow physics, emission wavelengths, altitude ranges, visual appearance from space, survey of CesiumJS/WorldWind/game engine implementations

**Affected Areas**: WGSL shader, Rust uniform struct, GPU pipeline setup, render pass encoding, dirty checking, Slint UI, config persistence, main.rs glue code, wallpaper export path

**Approach**: Separate atmosphere shell (Approach 3 from research). A concentric sphere at configurable radius (default 1.02) rendered as a third draw call with additive blending. The fragment shader uses `dot(N, sun_dir)` to select between three color regimes -- blue (day), orange (terminator), green (night) -- modulated by a rim falloff term `pow(1 - dot(N, V), falloff)`. Colors are hard-coded in the shader; only intensity, falloff, and radius are exposed as uniforms. This matches the recommendation from all four research documents and provides an upgrade path to ray-marched scattering in the future.

## Success Criteria

- [ ] A visible colored atmosphere glow appears around the Earth's limb, extending beyond the sphere silhouette into the dark background
- [ ] The glow is blue on the day-side limb, transitions to orange/red at the terminator, and shows green/yellow on the night-side limb
- [ ] The Atmosphere GroupBox in the Slint controls panel has an enable/disable checkbox and Intensity/Falloff sliders
- [ ] When the checkbox is unchecked (or intensity is 0), no atmosphere draw call is issued and no visual effect is present
- [ ] The atmosphere effect is included in wallpaper exports
- [ ] Atmosphere settings persist across app restarts via config.toml
- [ ] "Reset All" restores atmosphere parameters to defaults
- [ ] Changing any atmosphere slider triggers a re-render (dirty checking works)
- [ ] `cargo test` passes (including existing GPU integration tests in `tests/shading.rs` and `tests/render_pipeline.rs`)
- [ ] `cargo clippy` passes with no new warnings
- [ ] The uniform struct is exactly 176 bytes with a compile-time assertion

## Implementation Steps

### Phase 1: Uniform Struct and WGSL Interface

Extend the GPU-side data interface to carry atmosphere parameters. This phase modifies the two files that must stay in lockstep (Rust uniform struct and WGSL uniform struct) plus the `ShadingParams` bridge and `write_uniforms` function.

#### Step 1.1: Extend Rust `Uniforms` struct

- **Files**: `src/renderer/uniforms.rs`
- **Action**: Append four `f32` fields after `cloud_gamma` (offset 160): `atmo_intensity`, `atmo_falloff`, `atmo_radius`, `_pad3`. Update the compile-time size assertion from 160 to 176.
- **Test cases**:
  - Compile-time assertion `size_of::<Uniforms>() == 176` passes
  - `bytemuck::Pod` derivation still compiles (all fields are `f32` or `u32`, struct is `#[repr(C)]`)
- **Verify**: `cargo build` succeeds, size assertion is 176
- **Complexity**: Small

#### Step 1.2: Extend WGSL `Uniforms` struct

- **Files**: `shaders/sphere.wgsl` (lines 1-23, the `struct Uniforms` block)
- **Action**: Append three fields after `cloud_gamma`: `atmo_intensity: f32` (offset 160), `atmo_falloff: f32` (offset 164), `atmo_radius: f32` (offset 168), `_pad3: f32` (offset 172). Add offset comments matching the Rust side.
- **Verify**: `cargo build` succeeds (shader compilation happens at runtime, but the Rust side must compile). Full verification deferred to Phase 2 when the shader is actually used.
- **Complexity**: Small

#### Step 1.3: Extend `ShadingParams` and `write_uniforms`

- **Files**: `src/renderer/render_pass.rs` (lines 12-32 for `ShadingParams`, lines 67-116 for `write_uniforms`)
- **Action**: Add `atmo_intensity: f32`, `atmo_falloff: f32`, `atmo_radius: f32` fields to `ShadingParams`. Wire them into the `Uniforms` struct construction in `write_uniforms()`, mapping to the new fields. Set `_pad3: 0.0`.
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

### Phase 2: WGSL Shader Entry Points

Add the atmosphere vertex and fragment shaders. These follow the `vs_cloud`/`fs_cloud` pattern directly.

#### Step 2.1: Add `vs_atmo` vertex shader

- **Files**: `shaders/sphere.wgsl` (after `fs_cloud`, around line 153)
- **Action**: Add a `@vertex fn vs_atmo` entry point. Identical to `vs_cloud` but scales `in.position` by `uniforms.atmo_radius` instead of `uniforms.cloud_sphere_radius`. The normal output remains the unscaled unit-sphere direction.
- **Verify**: `cargo build` succeeds (shader is included via `include_str!` at compile time but only validated at runtime pipeline creation)
- **Complexity**: Small

#### Step 2.2: Add `fs_atmo` fragment shader

- **Files**: `shaders/sphere.wgsl` (after `vs_atmo`)
- **Action**: Add a `@fragment fn fs_atmo` entry point implementing the unified atmosphere color selection. The shader:
  1. Computes `N = normalize(in.world_normal)` and `V = normalize(uniforms.eye_pos - in.world_normal)`
  2. Computes `rim = pow(1.0 - clamp(dot(N, V), 0.0, 1.0), uniforms.atmo_falloff)`
  3. Computes `NdotL = dot(N, uniforms.sun_dir)`
  4. Selects color based on sun angle:
     - `day_t = smoothstep(-0.1, 0.2, NdotL)` -- transition from night to day
     - `term_t = exp(-NdotL * NdotL / 0.02)` -- Gaussian peak at terminator
     - `day_color = vec3(0.4, 0.6, 1.0)` (blue Rayleigh scattering)
     - `term_color = vec3(1.0, 0.5, 0.2)` (orange spectral depletion)
     - `night_color = vec3(0.3, 1.0, 0.4)` (green OI 557.7nm airglow)
     - `color = mix(night_color, day_color, day_t) + term_color * term_t * 0.3`
  5. Computes `atmo = color * rim * uniforms.atmo_intensity`
  6. Returns `vec4(atmo, 0.0)` -- RGB additive contribution, alpha zero for additive blending
- **Verify**: Shader compiles at runtime when the atmosphere pipeline is created in Phase 3. Any WGSL syntax errors will surface as pipeline creation panics.
- **Complexity**: Small

### Phase 3: GPU Pipeline

Create the atmosphere render pipeline and integrate it into the resource lifecycle.

#### Step 3.1: Create `create_atmo_pipeline` function

- **Files**: `src/renderer/gpu_setup.rs` (after `create_cloud_pipeline`, around line 345)
- **Action**: Add a `pub(super) fn create_atmo_pipeline()` function following the `create_cloud_pipeline` pattern. Key differences from the cloud pipeline:
  - Vertex entry point: `"vs_atmo"` (not `"vs_cloud"`)
  - Fragment entry point: `"fs_atmo"` (not `"fs_cloud"`)
  - Blend state: additive (`src_factor: One, dst_factor: One, operation: Add` for color; `OVER` for alpha) instead of `ALPHA_BLENDING`
  - Label: `"atmo_pipeline"`
  - All other settings identical to cloud pipeline: same vertex buffer layout, same `TriangleList` topology, `Ccw` front face, back-face culling, `Depth32Float` depth with `depth_write_enabled: false` and `CompareFunction::Less`, same multisample state
- **Test cases**:
  - Pipeline creation succeeds at runtime (verified by running the app)
  - The additive blend state is correctly configured (visual verification: atmosphere glow adds to the background without obscuring it)
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

#### Step 3.2: Wire atmosphere pipeline into `GpuResources` and lifecycle

- **Files**: `src/renderer/gpu_setup.rs` (lines 210-248 in `create_gpu_resources`, lines 430-436 in `rebuild_msaa_resources`), `src/renderer/mod.rs` (lines 213-270 in `struct GpuResources`)
- **Action**:
  1. Add `atmo_pipeline: wgpu::RenderPipeline` field to `GpuResources` (after `cloud_pipeline`)
  2. In `create_gpu_resources()`: call `create_atmo_pipeline(&device, &pipeline_layout, &shader, sample_count)` and assign to the new field
  3. In `rebuild_msaa_resources()`: rebuild `res.atmo_pipeline` alongside the cloud pipeline when MSAA sample count changes
- **Verify**: `cargo build` succeeds, app launches without panic (pipeline creation validates the WGSL entry points)
- **Complexity**: Small

### Phase 4: Render Pass Integration

Add the atmosphere draw call between Earth and clouds.

#### Step 4.1: Add atmosphere parameters to `encode_and_submit`

- **Files**: `src/renderer/render_pass.rs` (lines 123-184, `encode_and_submit` function)
- **Action**: Add `atmo_pipeline: Option<&wgpu::RenderPipeline>` and `atmo_bind_group: Option<&wgpu::BindGroup>` parameters (after the existing cloud parameters). Insert an atmosphere draw call between the Earth draw and the cloud draw. The atmosphere reuses the same vertex and index buffers already bound by the Earth draw. Draw order within the render pass:

  ```rust
  // Earth draw (existing)
  pass.draw_indexed(0..index_count, 0, 0..1);

  // Atmosphere overlay (new -- between Earth and clouds)
  if let (Some(atmo_pipe), Some(atmo_bg)) =
      (atmo_pipeline, atmo_bind_group)
  {
      pass.set_pipeline(atmo_pipe);
      pass.set_bind_group(0, atmo_bg, &[]);
      pass.draw_indexed(0..index_count, 0, 0..1);
  }

  // Cloud overlay (existing)
  if let (Some(cloud_pipe), Some(cloud_bg)) =
      (cloud_pipeline, cloud_bind_group)
  {
      ...
  }
  ```

- **Verify**: `cargo build` succeeds
- **Complexity**: Small

#### Step 4.2: Wire atmosphere into `execute_render_pass`

- **Files**: `src/renderer/render_pass.rs` (lines 186-248, `execute_render_pass` function)
- **Action**: Add atmosphere pipeline/bind group resolution logic, matching the existing cloud pattern. The atmosphere uses the Earth's bind group (it needs only the uniform buffer; the texture bindings are present but ignored by `fs_atmo`). Condition: `shading.atmo_intensity > 0.0`. Pass the resolved `atmo_pipe` and `atmo_bg` to `encode_and_submit`.
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

#### Step 4.3: Wire atmosphere into `export_wallpaper_image`

- **Files**: `src/renderer/mod.rs` (lines 114-210, `export_wallpaper_image` function)
- **Action**: Add atmosphere pipeline/bind group resolution matching the pattern in `execute_render_pass`. Pass `atmo_pipe` and `atmo_bg` to `encode_and_submit` so that wallpaper exports include the atmosphere glow. The bind group is the same one used for the Earth draw (already available as `bind_group`).
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

### Phase 5: Dirty Checking and Frame State

Ensure atmosphere parameter changes trigger re-renders.

#### Step 5.1: Add atmosphere fields to `FrameState` (RED)

- **Files**: `src/renderer/frame.rs`
- **Action**: Write tests first, then add `atmo_intensity: i32`, `atmo_falloff: i32`, `atmo_radius: i32` fields to the `FrameState` struct. Add corresponding parameters to `build_frame_state()` with `(value * 1000.0) as i32` quantization.
- **Test cases** (add to `src/renderer/frame.rs` `mod tests`):
  - `frame_state_atmo_intensity_quantization`: Build with `atmo_intensity = 0.3`, assert field equals `300`
  - `frame_state_atmo_falloff_quantization`: Build with `atmo_falloff = 4.5`, assert field equals `4500`
  - `frame_state_atmo_radius_quantization`: Build with `atmo_radius = 1.02`, assert field equals `1020`
  - `frame_state_atmo_intensity_triggers_dirty`: Change `atmo_intensity` from 0.3 to 0.5, assert `state_a != state_b`
  - `frame_state_atmo_falloff_triggers_dirty`: Change `atmo_falloff` from 4.0 to 5.0, assert `state_a != state_b`
  - `frame_state_atmo_radius_triggers_dirty`: Change `atmo_radius` from 1.02 to 1.03, assert `state_a != state_b`
- **Verify**: All new tests pass. All existing `frame_state_*` tests still pass (the `build_frame_state` calls in existing tests need the three new parameters appended).
- **Complexity**: Medium (must update all existing test call sites that call `build_frame_state`)

#### Step 5.2: Wire atmosphere into `BeforeRendering` frame state construction

- **Files**: `src/renderer/mod.rs` (lines 410-452 in `BeforeRendering`)
- **Action**: Read `atmo_intensity`, `atmo_falloff`, and `atmo_radius` from the Slint window properties (via `win.get_atmo_intensity()`, etc.). Pass them to `build_frame_state()` and include them in the `ShadingParams` construction.
- **Verify**: `cargo build` succeeds (requires Phase 6 UI properties to exist first, or use temporary hardcoded values)
- **Complexity**: Small

### Phase 6: Slint UI

Add the Atmosphere GroupBox to the controls panel.

#### Step 6.1: Add Atmosphere properties and GroupBox

- **Files**: `ui/main.slint`
- **Action**:
  1. Add three `in-out property` declarations on `MainWindow` (after the cloud properties, around line 57):
     - `in-out property <bool> atmo-enabled: true;`
     - `in-out property <float> atmo-intensity: 0.3;`
     - `in-out property <float> atmo-falloff: 4.0;`
  2. Add a new GroupBox between "Clouds" (line 599) and "Date / Time" (line 666) titled "Atmosphere":
     - An enable/disable checkbox bound to `root.atmo-enabled`, triggering `root.sliders-changed()` on toggle
     - An Intensity slider (0.0 to 1.0) bound to `root.atmo-intensity`, with display text showing percentage. Slider triggers `root.sliders-changed()`. Slider and label are only visible when `root.atmo-enabled` is true.
     - A Falloff slider (1.0 to 10.0) bound to `root.atmo-falloff`, with display text showing the raw value rounded to one decimal. Slider triggers `root.sliders-changed()`. Slider and label are only visible when `root.atmo-enabled` is true.
  3. Follow the existing Clouds GroupBox layout pattern exactly: `VerticalLayout` with `spacing: 4px`, each row is a `HorizontalLayout` with label `Text` (min-width 70px), `Slider`, and value `Text` (min-width 35px).
- **Manual verification**:
  - App launches, the Atmosphere GroupBox appears between Clouds and Date/Time
  - Unchecking the checkbox hides the sliders
  - Moving sliders triggers a redraw
- **Verify**: `cargo build` succeeds
- **Complexity**: Medium

### Phase 7: Config Persistence and Main Wiring

Wire the atmosphere parameters through config save/load and the reset callback.

#### Step 7.1: Add atmosphere fields to `AppConfig`

- **Files**: `src/config.rs`
- **Action**: Add `atmo_enabled: bool`, `atmo_intensity: f32`, `atmo_falloff: f32` fields to `AppConfig` (after cloud fields). Set defaults in `impl Default`: `atmo_enabled: true`, `atmo_intensity: 0.3`, `atmo_falloff: 4.0`. The `atmo_radius` is intentionally NOT exposed in config -- it is a constant (`1.02`) set in the renderer, not user-facing.
- **Test cases** (add to `src/config.rs` `mod tests`):
  - `default_atmo_enabled`: Assert `config.atmo_enabled == true`
  - `default_atmo_intensity`: Assert `config.atmo_intensity == 0.3` (use `assert_relative_eq!`)
  - `default_atmo_falloff`: Assert `config.atmo_falloff == 4.0` (use `assert_relative_eq!`)
  - `deserialize_missing_atmo_fields_fills_defaults`: Parse `"cloud_opacity = 0.5"`, assert atmo fields are at defaults
  - Existing `serde_round_trip` and `serde_round_trip_non_default` tests must be updated to include atmo fields
- **Verify**: All config tests pass, `cargo test config` succeeds
- **Complexity**: Small

#### Step 7.2: Wire `apply_config_to_window`, `read_config_from_window`, `on_reset_all`

- **Files**: `src/main.rs`
- **Action**:
  1. In `apply_config_to_window()`: Add `window.set_atmo_enabled(config.atmo_enabled)`, `window.set_atmo_intensity(config.atmo_intensity)`, `window.set_atmo_falloff(config.atmo_falloff)`
  2. In `read_config_from_window()`: Add `atmo_enabled: window.get_atmo_enabled()`, `atmo_intensity: window.get_atmo_intensity()`, `atmo_falloff: window.get_atmo_falloff()`
  3. In `on_reset_all()`: Add `win.set_atmo_enabled(lighting.atmo_enabled)`, `win.set_atmo_intensity(lighting.atmo_intensity)`, `win.set_atmo_falloff(lighting.atmo_falloff)` (where `lighting` is `AppConfig::default()`)
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

### Phase 8: Integration and Verification

#### Step 8.1: End-to-end integration in `BeforeRendering`

- **Files**: `src/renderer/mod.rs` (BeforeRendering arm)
- **Action**: Complete the wiring from Phase 5 Step 5.2:
  1. Read `let atmo_enabled = win.get_atmo_enabled()` and the intensity/falloff properties
  2. Compute effective intensity: `let atmo_intensity_f = if atmo_enabled { win.get_atmo_intensity() } else { 0.0 };`
  3. Pass `atmo_intensity_f`, `win.get_atmo_falloff()`, and the constant `1.02_f32` (atmo radius) to `build_frame_state()`
  4. Include them in `ShadingParams`: `atmo_intensity: atmo_intensity_f`, `atmo_falloff: win.get_atmo_falloff()`, `atmo_radius: 1.02`
- **Verify**: App compiles and runs. Atmosphere glow is visible around the Earth's limb.
- **Complexity**: Small

#### Step 8.2: Visual verification

- **Files**: N/A (manual testing)
- **Action**: Manual verification of the complete feature:
- **Manual test cases**:
  - Launch app with default settings. Verify blue glow on the day-side limb, green glow on the night-side limb, orange transition at the terminator.
  - Set atmo intensity to 0. Verify no visible atmosphere glow.
  - Set atmo intensity to 1.0. Verify strong glow that extends visibly beyond the sphere edge.
  - Adjust falloff from 1.0 (wide, diffuse halo) to 10.0 (tight rim). Verify the glow width changes.
  - Uncheck the Atmosphere enable checkbox. Verify glow disappears. Re-check it. Verify glow returns.
  - Enable clouds. Verify clouds render on top of the atmosphere glow (correct draw order).
  - Click "Set as Wallpaper". Verify the exported wallpaper includes the atmosphere glow.
  - Click "Reset All". Verify atmosphere parameters return to defaults (enabled, intensity 0.3, falloff 4.0).
  - Close and reopen the app. Verify atmosphere settings persisted from the config file.
  - Rotate the globe (drag). Verify the atmosphere glow rotates with the globe and the day/night color distribution follows the sun direction.
  - Change MSAA setting. Verify the atmosphere glow still renders correctly after pipeline rebuild.
- **Verify**: All manual checks pass
- **Complexity**: Small

#### Step 8.3: Run full test suite

- **Files**: N/A
- **Action**: Run `cargo test` and `cargo clippy`. Fix any failures introduced by the new parameters (primarily: existing `build_frame_state` call sites in tests that need the three new arguments, and any clippy warnings about the extended parameter list).
- **Verify**: `cargo test` passes, `cargo clippy` passes
- **Complexity**: Small

## Test Strategy

### Automated Tests

| Test Case | Type | Input | Expected Output |
| --- | --- | --- | --- |
| `Uniforms` size assertion | Compile-time | `size_of::<Uniforms>()` | 176 |
| `atmo_intensity` quantization | Unit | `atmo_intensity = 0.3` | `FrameState.atmo_intensity == 300` |
| `atmo_falloff` quantization | Unit | `atmo_falloff = 4.5` | `FrameState.atmo_falloff == 4500` |
| `atmo_radius` quantization | Unit | `atmo_radius = 1.02` | `FrameState.atmo_radius == 1020` |
| `atmo_intensity` triggers dirty | Unit | Change 0.3 to 0.5 | `state_a != state_b` |
| `atmo_falloff` triggers dirty | Unit | Change 4.0 to 5.0 | `state_a != state_b` |
| `atmo_radius` triggers dirty | Unit | Change 1.02 to 1.03 | `state_a != state_b` |
| `AppConfig` default `atmo_enabled` | Unit | `AppConfig::default()` | `true` |
| `AppConfig` default `atmo_intensity` | Unit | `AppConfig::default()` | `0.3` |
| `AppConfig` default `atmo_falloff` | Unit | `AppConfig::default()` | `4.0` |
| Missing atmo fields deserialize to defaults | Unit | TOML without atmo fields | Defaults filled |
| Config serde round-trip with atmo | Unit | Serialize then deserialize | Fields preserved |
| Existing GPU integration tests | Integration | Existing test inputs | Pass unchanged |

### Manual Verification

- [ ] Atmosphere glow visible at Earth's limb with correct day (blue) / terminator (orange) / night (green) coloring
- [ ] Glow extends beyond the sphere silhouette into the dark background
- [ ] Intensity slider scales brightness from invisible (0) to prominent (1)
- [ ] Falloff slider adjusts glow width from wide halo (1) to tight rim (10)
- [ ] Enable/disable checkbox works, collapsing sliders when unchecked
- [ ] Clouds render on top of atmosphere (correct draw order)
- [ ] Wallpaper export includes atmosphere glow
- [ ] Reset All restores atmosphere defaults
- [ ] Settings persist across app restart
- [ ] MSAA change does not break atmosphere rendering

## Risks and Mitigations

| Risk | Impact | Mitigation |
| --- | --- | --- |
| WGSL shader syntax error in `fs_atmo` | App crashes at pipeline creation | Test by running the app immediately after adding shader code; WGSL errors produce clear error messages |
| Additive blending interacts poorly with MSAA resolve | Bright fringing at the sphere edge | The cloud layer already uses blending with MSAA without issues; additive blending is simpler than alpha blending in this regard |
| Atmosphere glow too bright/faint with default values | Poor first impression | Defaults (intensity 0.3, falloff 4.0) are based on research survey of comparable implementations; manual tuning during visual verification step |
| `build_frame_state` parameter count grows further (now 24 arguments) | Clippy warning | Already suppressed with `#[allow(clippy::too_many_arguments)]`; a future refactor to a parameter struct is noted in the codebase docs |
| Existing GPU integration tests fail due to uniform buffer size change | Test failures | The uniform buffer size is allocated from the Rust struct size, so the GPU-side buffer automatically grows. Shader tests that create a `Uniforms` instance need the new fields set to `0.0`. |
| Performance regression at high MSAA on 4K | Fill rate increase | One additional full-screen alpha pass. At the app's 2-minute redraw interval, this is negligible. The atmosphere can be disabled if needed. |

## Rollback Strategy

All changes are contained in a single feature branch (`feature-airglow`). If issues arise:

1. `git revert` the merge commit to remove all atmosphere code
2. The uniform struct reverts to 160 bytes, WGSL reverts to 23-field struct
3. Config files with unknown `atmo_*` fields are silently ignored by serde `#[serde(default)]`
4. No database migrations, no external API changes, no persistent side effects beyond the config file

## File Change Summary

| File | Change | Lines (est.) |
| --- | --- | --- |
| `src/renderer/uniforms.rs` | Add 4 fields, update size assertion | +5 |
| `shaders/sphere.wgsl` | Add 4 uniform fields, `vs_atmo`, `fs_atmo` | +40 |
| `src/renderer/gpu_setup.rs` | Add `create_atmo_pipeline()`, wire into lifecycle | +50 |
| `src/renderer/mod.rs` | Add `atmo_pipeline` field, read UI properties, wire `BeforeRendering` and export | +20 |
| `src/renderer/render_pass.rs` | Add atmo to `ShadingParams`, `write_uniforms`, `encode_and_submit`, `execute_render_pass` | +25 |
| `src/renderer/frame.rs` | Add 3 fields to `FrameState`, extend `build_frame_state`, add tests | +80 |
| `ui/main.slint` | Add properties and Atmosphere GroupBox | +40 |
| `src/config.rs` | Add 3 fields to `AppConfig`, defaults, tests | +30 |
| `src/main.rs` | Wire config, reset, callbacks | +10 |

**Estimated total**: ~300 lines added across 9 files.

## Status

- [ ] Plan approved
- [ ] Implementation started
- [ ] Implementation complete
