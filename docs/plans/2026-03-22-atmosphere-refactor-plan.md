# Plan: Atmosphere Effect Refactor -- Separate Rayleigh and Nightglow Layers (2026-03-22)

## Summary

Refactor the existing single-shell atmosphere effect into a physically-based multi-layer system with three concentric shells: (1) a low-altitude Rayleigh scattering shell for day-side blue limb glow with terminator orange, (2) a mid-altitude orange/yellow nightglow shell for sodium D-line and FeO emission, and (3) a slightly higher green nightglow shell for OI 557.7nm emission. Each shell is a separate draw call with additive blending, replacing the current unified `vs_atmo`/`fs_atmo` shader. The user-facing controls are reorganized into two sub-sections (Rayleigh and Nightglow) within the existing Atmosphere GroupBox, with five sliders replacing the current two.

## Stakes Classification

**Level**: Medium

**Rationale**: This is a refactor of an existing working feature, not a greenfield addition. The change spans 9 files (same files as the original atmosphere implementation) and follows the established pattern of the current atmosphere shell exactly -- each new shell is a clone of the existing shell with different shader logic and uniform values. The uniform struct grows by 16 bytes (176 to 192), which is a minor but irreversible GPU interface change. No new dependencies are introduced. The feature can be disabled by setting intensities to zero. Rollback is straightforward via `git revert`. The main complexity is in the shader logic for physically-motivated nightglow modulation (time-of-night and latitude variation), which is new code without a direct precedent in the codebase.

## Context

**Research**:

- `docs/plans/2026-03-22-atmosphere-plan.md` -- the original atmosphere implementation plan (completed)
- `docs/plans/2026-03-22-rayleigh-scattering-external.md` -- Rayleigh scattering physics, ISS visual reference, rendering techniques
- `docs/plans/2026-03-22-nightglow-colors.md` -- nightglow emission layers, altitude profiles, color variation by time-of-night and latitude
- `docs/plans/2026-03-22-atmosphere-visual-reference.md` -- visual appearance from various orbital distances

**Affected Areas**: WGSL shader, Rust uniform struct, GPU pipeline setup, render pass encoding, dirty checking, Slint UI, config persistence, main.rs glue code, wallpaper export path, GPU integration tests

**Approach**: Replace the single atmosphere shell (radius 1.02) with three concentric shells at physically-motivated radii. Each shell has its own vertex/fragment shader entry points and pipeline, all sharing additive blending and the same bind group layout. The Rayleigh shell sits closest to Earth's surface (~1.003); the two nightglow shells sit higher (~1.014 and ~1.015). User-facing controls are split into Rayleigh (intensity, falloff) and Nightglow (intensity, balance, falloff) groups. Shell radii are hardcoded constants, not user-facing.

## Success Criteria

- [ ] Three distinct atmosphere layers are visually distinguishable: blue Rayleigh at the day-side limb, orange/yellow nightglow near the terminator on the night side, green nightglow deeper into the night side
- [ ] Rayleigh shell shows blue on the day side, orange/red at the terminator, and fades out on the night side
- [ ] Orange nightglow shell is only visible on the night side, with intensity that peaks near the terminator (post-sunset region) and diminishes toward midnight
- [ ] Green nightglow shell is only visible on the night side, with intensity that peaks at midnight and is weaker near the terminator (opposite profile to orange)
- [ ] Both nightglow shells show latitude modulation: enhanced near +/-15-30 degrees
- [ ] The Atmosphere GroupBox has an enable/disable checkbox, two Rayleigh sliders (Intensity, Falloff), and three Nightglow sliders (Intensity, Balance, Falloff)
- [ ] When the checkbox is unchecked (or all intensities are 0), no atmosphere draw calls are issued
- [ ] The atmosphere effect is included in wallpaper exports
- [ ] All atmosphere settings persist across app restarts via config.toml
- [ ] "Reset All" restores all atmosphere parameters to defaults
- [ ] Changing any atmosphere slider triggers a re-render (dirty checking works)
- [ ] `cargo test` passes (including updated GPU integration tests)
- [ ] `cargo clippy` passes with no new warnings
- [ ] The uniform struct is exactly 192 bytes with a compile-time assertion
- [ ] Draw order is: Earth, Rayleigh, Nightglow Orange, Nightglow Green, Clouds

## Implementation Steps

### Phase 1: Uniform Struct and WGSL Interface

Extend the GPU-side data interface to carry the new atmosphere parameters. The Rust uniform struct and WGSL struct must stay in lockstep.

#### Step 1.1: Extend Rust `Uniforms` struct

- **Files**: `src/renderer/uniforms.rs`
- **Action**: Replace the four existing atmosphere fields (`atmo_intensity`, `atmo_falloff`, `atmo_radius`, `_pad3`) with eight new fields: `rayleigh_intensity: f32`, `rayleigh_falloff: f32`, `nightglow_intensity: f32`, `nightglow_falloff: f32`, `nightglow_balance: f32`, `rayleigh_radius: f32`, `nightglow_orange_radius: f32`, `nightglow_green_radius: f32`. Update the compile-time size assertion from 176 to 192.
- **Test cases**:
  - Compile-time assertion `size_of::<Uniforms>() == 192` passes
  - `bytemuck::Pod` derivation still compiles
- **Verify**: `cargo build` succeeds, size assertion is 192
- **Complexity**: Small

#### Step 1.2: Extend WGSL `Uniforms` struct

- **Files**: `shaders/sphere.wgsl` (lines 1-27, the `struct Uniforms` block)
- **Action**: Replace the four existing atmosphere fields with eight new fields matching the Rust side: `rayleigh_intensity: f32` (offset 160), `rayleigh_falloff: f32` (offset 164), `nightglow_intensity: f32` (offset 168), `nightglow_falloff: f32` (offset 172), `nightglow_balance: f32` (offset 176), `rayleigh_radius: f32` (offset 180), `nightglow_orange_radius: f32` (offset 184), `nightglow_green_radius: f32` (offset 188). Add offset comments matching the Rust side. Remove the old `_pad3` field.
- **Verify**: `cargo build` succeeds (WGSL is validated at runtime pipeline creation)
- **Complexity**: Small

#### Step 1.3: Extend `ShadingParams` and `write_uniforms`

- **Files**: `src/renderer/render_pass.rs` (lines 12-35 for `ShadingParams`, lines 72-123 for `write_uniforms`)
- **Action**: Replace the three existing atmosphere fields in `ShadingParams` (`atmo_intensity`, `atmo_falloff`, `atmo_radius`) with: `rayleigh_intensity: f32`, `rayleigh_falloff: f32`, `nightglow_intensity: f32`, `nightglow_falloff: f32`, `nightglow_balance: f32`, `rayleigh_radius: f32`, `nightglow_orange_radius: f32`, `nightglow_green_radius: f32`. Wire them into the `Uniforms` struct construction in `write_uniforms()`.
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

### Phase 2: WGSL Shader Entry Points

Replace the existing `vs_atmo`/`fs_atmo` with six new entry points: one vertex/fragment pair per shell.

#### Step 2.1: Replace `vs_atmo` with three vertex shaders

- **Files**: `shaders/sphere.wgsl` (lines 159-168, after `fs_cloud`)
- **Action**: Remove the existing `vs_atmo` function. Add three vertex shaders, each identical except for which radius uniform they use to scale the sphere:
  - `vs_rayleigh`: scales by `uniforms.rayleigh_radius`
  - `vs_nightglow_orange`: scales by `uniforms.nightglow_orange_radius`
  - `vs_nightglow_green`: scales by `uniforms.nightglow_green_radius`

  Each follows the `vs_cloud` pattern: scale `in.position` by the radius, transform by MVP, pass through the unscaled unit-sphere direction as `world_normal`.
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

#### Step 2.2: Replace `fs_atmo` with `fs_rayleigh`

- **Files**: `shaders/sphere.wgsl` (after the vertex shaders)
- **Action**: Remove the existing `fs_atmo` function. Add `@fragment fn fs_rayleigh` implementing the Rayleigh scattering shell:
  1. Compute `N = normalize(in.world_normal)` and `V = normalize(uniforms.eye_pos - in.world_normal)`
  2. Compute `rim = pow(1.0 - clamp(dot(N, V), 0.0, 1.0), uniforms.rayleigh_falloff)`
  3. Compute `NdotL = dot(N, uniforms.sun_dir)`
  4. Day-side blue: `day_t = smoothstep(-0.1, 0.2, NdotL)` controls transition from night to day
  5. Terminator orange: `term_t = exp(-NdotL * NdotL / 0.02)` Gaussian peak at terminator
  6. `day_color = vec3(0.4, 0.6, 1.0)` (blue Rayleigh)
  7. `term_color = vec3(1.0, 0.5, 0.2)` (orange spectral depletion)
  8. Night-side fadeout: `night_fade = smoothstep(0.0, -0.3, NdotL)` fades Rayleigh to zero on the night side
  9. `color = mix(term_color * term_t * 0.5, day_color, day_t) * (1.0 - night_fade)`
  10. Return `vec4(color * rim * uniforms.rayleigh_intensity, 0.0)`
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

#### Step 2.3: Add `fs_nightglow_orange`

- **Files**: `shaders/sphere.wgsl` (after `fs_rayleigh`)
- **Action**: Add `@fragment fn fs_nightglow_orange` implementing the lower nightglow shell (sodium D + FeO):
  1. Compute `N`, `V`, `rim` using `uniforms.nightglow_falloff`
  2. Compute `NdotL = dot(N, uniforms.sun_dir)`
  3. Night-side mask: `night_mask = smoothstep(0.1, -0.1, NdotL)` -- only visible on night side
  4. Time-of-night modulation: `depth = clamp(-NdotL, 0.0, 1.0)`. Orange is STRONGER near the terminator (low depth) and weaker at midnight (high depth). Use `time_mod = 1.0 - 0.5 * depth` so intensity at terminator edge is 1.0 and at midnight is 0.5.
  5. Latitude modulation: `lat = asin(clamp(N.y, -1.0, 1.0))`. Enhanced near +/-15-30 degrees geomagnetic latitude. Use `lat_mod = 0.7 + 0.3 * exp(-pow((abs(lat) - 0.4) / 0.2, 2.0))` where 0.4 rad is approx 23 degrees.
  6. `color = vec3(1.0, 0.7, 0.2)` (warm orange-yellow for sodium D + FeO)
  7. `intensity = uniforms.nightglow_intensity * (1.0 - uniforms.nightglow_balance)`
  8. Return `vec4(color * rim * intensity * night_mask * time_mod * lat_mod, 0.0)`
- **Verify**: `cargo build` succeeds
- **Complexity**: Medium

#### Step 2.4: Add `fs_nightglow_green`

- **Files**: `shaders/sphere.wgsl` (after `fs_nightglow_orange`)
- **Action**: Add `@fragment fn fs_nightglow_green` implementing the upper nightglow shell (OI 557.7nm):
  1. Same rim and night_mask computation as `fs_nightglow_orange`
  2. Time-of-night modulation: opposite to orange. `depth = clamp(-NdotL, 0.0, 1.0)`. Green is WEAKER near the terminator and STRONGER at midnight. Use `time_mod = 0.5 + 0.5 * depth`.
  3. Same latitude modulation as orange
  4. `color = vec3(0.2, 1.0, 0.3)` (green OI 557.7nm)
  5. `intensity = uniforms.nightglow_intensity * uniforms.nightglow_balance`
  6. Return `vec4(color * rim * intensity * night_mask * time_mod * lat_mod, 0.0)`
- **Verify**: `cargo build` succeeds
- **Complexity**: Small (mirrors Step 2.3 with different constants)

### Phase 3: GPU Pipelines

Create three pipelines (one per shell), replacing the single `atmo_pipeline`.

#### Step 3.1: Replace `create_atmo_pipeline` with three pipeline constructors

- **Files**: `src/renderer/gpu_setup.rs` (lines 349-402)
- **Action**: Remove `create_atmo_pipeline`. Add three new functions: `create_rayleigh_pipeline`, `create_nightglow_orange_pipeline`, `create_nightglow_green_pipeline`. Each is identical to the removed function except for the vertex/fragment entry point names:
  - `create_rayleigh_pipeline`: `"vs_rayleigh"` / `"fs_rayleigh"`, label `"rayleigh_pipeline"`
  - `create_nightglow_orange_pipeline`: `"vs_nightglow_orange"` / `"fs_nightglow_orange"`, label `"nightglow_orange_pipeline"`
  - `create_nightglow_green_pipeline`: `"vs_nightglow_green"` / `"fs_nightglow_green"`, label `"nightglow_green_pipeline"`

  All three share the same additive blend state (`src: One, dst: One, op: Add` for color; `OVER` for alpha), depth settings (`depth_write_enabled: false`, `CompareFunction::Less`), and back-face culling as the current `create_atmo_pipeline`.
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

#### Step 3.2: Wire new pipelines into `GpuResources` and lifecycle

- **Files**: `src/renderer/gpu_setup.rs` (lines 210-251 in `create_gpu_resources`, lines 487-495 in `rebuild_msaa_resources`), `src/renderer/mod.rs` (lines 221-280 in `struct GpuResources`)
- **Action**:
  1. Replace `atmo_pipeline: wgpu::RenderPipeline` in `GpuResources` with three fields: `rayleigh_pipeline`, `nightglow_orange_pipeline`, `nightglow_green_pipeline`
  2. In `create_gpu_resources()`: call all three constructors and assign to the new fields
  3. In `rebuild_msaa_resources()`: rebuild all three pipelines when MSAA sample count changes
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

### Phase 4: Render Pass Integration

Replace the single atmosphere draw call with three draw calls in the correct order.

#### Step 4.1: Update `encode_and_submit` for three atmosphere passes

- **Files**: `src/renderer/render_pass.rs` (lines 130-201)
- **Action**: Replace the two `atmo_pipeline`/`atmo_bind_group` parameters with six parameters: `rayleigh_pipeline`, `rayleigh_bind_group`, `nightglow_orange_pipeline`, `nightglow_orange_bind_group`, `nightglow_green_pipeline`, `nightglow_green_bind_group` (all `Option<&...>`). Update the draw order within the render pass:

  ```text
  // Earth draw (existing)
  pass.draw_indexed(0..index_count, 0, 0..1);

  // Rayleigh scattering (closest to surface)
  if let (Some(pipe), Some(bg)) = (rayleigh_pipeline, rayleigh_bind_group) { ... }

  // Nightglow orange (sodium + FeO, ~1.014 radius)
  if let (Some(pipe), Some(bg)) = (nightglow_orange_pipeline, nightglow_orange_bind_group) { ... }

  // Nightglow green (OI 557.7nm, ~1.015 radius)
  if let (Some(pipe), Some(bg)) = (nightglow_green_pipeline, nightglow_green_bind_group) { ... }

  // Cloud overlay (existing, drawn last)
  if let (Some(cloud_pipe), Some(cloud_bg)) = (...) { ... }
  ```

- **Verify**: `cargo build` succeeds
- **Complexity**: Small

#### Step 4.2: Update `execute_render_pass` for three atmosphere layers

- **Files**: `src/renderer/render_pass.rs` (lines 205-276)
- **Action**: Replace the atmosphere pipeline/bind group resolution logic. The Rayleigh shell draws when `shading.rayleigh_intensity > 0.0`. The nightglow shells draw when `shading.nightglow_intensity > 0.0` (both shells are gated by the same master intensity). All three reuse the Earth's bind group (texture bindings are present but ignored). Pass the six resolved `Option` values to `encode_and_submit`.
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

#### Step 4.3: Update `export_wallpaper_image` for three atmosphere layers

- **Files**: `src/renderer/mod.rs` (lines 114-218)
- **Action**: Apply the same three-way atmosphere pipeline/bind group resolution as in `execute_render_pass`. Pass all six atmosphere parameters to `encode_and_submit`.
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

### Phase 5: Dirty Checking and Frame State

Ensure all new atmosphere parameters trigger re-renders when changed.

#### Step 5.1: Update `FrameState` struct and `build_frame_state` (RED then GREEN)

- **Files**: `src/renderer/frame.rs`
- **Action**: Replace the three existing atmosphere fields (`atmo_intensity`, `atmo_falloff`, `atmo_radius`) with five new fields: `rayleigh_intensity: i32`, `rayleigh_falloff: i32`, `nightglow_intensity: i32`, `nightglow_falloff: i32`, `nightglow_balance: i32`. All use `(value * 1000.0) as i32` quantization. Update `build_frame_state()` to accept the five new parameters instead of the three old ones. Update the existing `default_frame_state()` test helper.
- **Test cases** (add to `src/renderer/frame.rs` `mod tests`):
  - `frame_state_rayleigh_intensity_quantization`: Build with `rayleigh_intensity = 0.3`, assert field equals `300`
  - `frame_state_rayleigh_falloff_quantization`: Build with `rayleigh_falloff = 6.0`, assert field equals `6000`
  - `frame_state_nightglow_intensity_quantization`: Build with `nightglow_intensity = 0.3`, assert field equals `300`
  - `frame_state_nightglow_falloff_quantization`: Build with `nightglow_falloff = 4.0`, assert field equals `4000`
  - `frame_state_nightglow_balance_quantization`: Build with `nightglow_balance = 0.5`, assert field equals `500`
  - `frame_state_rayleigh_intensity_triggers_dirty`: Change from 0.3 to 0.5, assert `state_a != state_b`
  - `frame_state_rayleigh_falloff_triggers_dirty`: Change from 6.0 to 8.0, assert `state_a != state_b`
  - `frame_state_nightglow_intensity_triggers_dirty`: Change from 0.3 to 0.5, assert `state_a != state_b`
  - `frame_state_nightglow_falloff_triggers_dirty`: Change from 4.0 to 6.0, assert `state_a != state_b`
  - `frame_state_nightglow_balance_triggers_dirty`: Change from 0.5 to 0.8, assert `state_a != state_b`
- **Verify**: All new and existing frame tests pass. The old `atmo_*` tests are removed/replaced.
- **Complexity**: Medium (must update all existing test call sites that call `build_frame_state`)

#### Step 5.2: Wire new parameters into `BeforeRendering` frame state construction

- **Files**: `src/renderer/mod.rs` (lines 440-475 in `BeforeRendering`)
- **Action**: Replace the `atmo_enabled` / `atmo_intensity` / `atmo_falloff` / `atmo_radius` reads with reads of the new properties: `rayleigh_intensity`, `rayleigh_falloff`, `nightglow_intensity`, `nightglow_falloff`, `nightglow_balance`. When `atmo_enabled` is false, set both `rayleigh_intensity` and `nightglow_intensity` to 0.0. Pass the five user-facing values plus the three constant radii (1.003, 1.014, 1.015) to `build_frame_state()` and `ShadingParams`.
- **Verify**: `cargo build` succeeds (requires Phase 6 UI properties)
- **Complexity**: Small

### Phase 6: Slint UI

Reorganize the Atmosphere GroupBox with sub-sections.

#### Step 6.1: Replace atmosphere properties and GroupBox layout

- **Files**: `ui/main.slint` (lines 58-60 for properties, lines 669-732 for GroupBox)
- **Action**:
  1. Replace the three `in-out property` declarations (`atmo-enabled`, `atmo-intensity`, `atmo-falloff`) with six:
     - `in-out property <bool> atmo-enabled: true;`
     - `in-out property <float> rayleigh-intensity: 0.3;`
     - `in-out property <float> rayleigh-falloff: 6.0;`
     - `in-out property <float> nightglow-intensity: 0.3;`
     - `in-out property <float> nightglow-balance: 0.5;`
     - `in-out property <float> nightglow-falloff: 4.0;`
  2. Replace the Atmosphere GroupBox contents with the new layout:
     - Enable checkbox (same as before)
     - When enabled, two sub-sections with bold labels:
       - **Rayleigh**: Intensity slider (0.0-1.0, display as percentage), Falloff slider (1.0-10.0, display raw value)
       - **Nightglow**: Intensity slider (0.0-1.0, display as percentage), Balance slider (0.0-1.0, display "orange" at 0 and "green" at 1), Falloff slider (1.0-10.0, display raw value)
  3. All sliders trigger `root.sliders-changed()`. Sliders are visible only when `root.atmo-enabled` is true.
- **Manual verification**:
  - App launches, reorganized Atmosphere GroupBox is visible
  - Unchecking the checkbox hides all five sliders
  - Moving any slider triggers a redraw
- **Verify**: `cargo build` succeeds
- **Complexity**: Medium

### Phase 7: Config Persistence and Main Wiring

Wire the new atmosphere parameters through config save/load and the reset callback.

#### Step 7.1: Update `AppConfig` atmosphere fields

- **Files**: `src/config.rs` (lines 62-65 for fields, lines 117-119 for defaults)
- **Action**: Replace `atmo_intensity: f32` and `atmo_falloff: f32` with: `rayleigh_intensity: f32`, `rayleigh_falloff: f32`, `nightglow_intensity: f32`, `nightglow_falloff: f32`, `nightglow_balance: f32`. Keep `atmo_enabled: bool` unchanged. Update `impl Default` to set: `rayleigh_intensity: 0.3`, `rayleigh_falloff: 6.0`, `nightglow_intensity: 0.3`, `nightglow_falloff: 4.0`, `nightglow_balance: 0.5`.
- **Test cases** (update existing and add new in `src/config.rs` `mod tests`):
  - `default_rayleigh_intensity`: Assert `config.rayleigh_intensity == 0.3`
  - `default_rayleigh_falloff`: Assert `config.rayleigh_falloff == 6.0`
  - `default_nightglow_intensity`: Assert `config.nightglow_intensity == 0.3`
  - `default_nightglow_falloff`: Assert `config.nightglow_falloff == 4.0`
  - `default_nightglow_balance`: Assert `config.nightglow_balance == 0.5`
  - `deserialize_missing_atmo_fields_fills_defaults`: Parse TOML without new fields, assert all five are at defaults
  - Update `serde_round_trip_non_default` to include all five new fields with non-default values
  - Update `save_and_load_round_trip` to include all five new fields
- **Verify**: All config tests pass
- **Complexity**: Small

#### Step 7.2: Wire `apply_config_to_window`, `read_config_from_window`, `on_reset_all`

- **Files**: `src/main.rs` (lines 314-357 for apply, lines 361-406 for read, lines 220-261 for reset)
- **Action**:
  1. In `apply_config_to_window()`: Replace `set_atmo_intensity`/`set_atmo_falloff` with calls to set all five new properties from config
  2. In `read_config_from_window()`: Replace `get_atmo_intensity`/`get_atmo_falloff` with reads of all five new properties
  3. In `on_reset_all()`: Replace the atmo reset lines with resets for all five new properties plus `atmo_enabled`
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

### Phase 8: Integration and Verification

#### Step 8.1: End-to-end integration in `BeforeRendering`

- **Files**: `src/renderer/mod.rs` (BeforeRendering arm)
- **Action**: Complete the wiring from Phase 5 Step 5.2. Ensure:
  1. `atmo_enabled` gates both `rayleigh_intensity` and `nightglow_intensity` (set to 0.0 when disabled)
  2. The three constant radii (`1.003_f32`, `1.014_f32`, `1.015_f32`) are passed to `ShadingParams`
  3. `ShadingParams` is fully populated with all eight atmosphere fields
- **Verify**: App compiles and runs. Three atmosphere layers are visible with correct spatial separation.
- **Complexity**: Small

#### Step 8.2: Update GPU integration tests

- **Files**: `tests/render_pipeline.rs`
- **Action**: Update the test `Uniforms` struct to match the new 192-byte layout: replace `atmo_intensity`, `atmo_falloff`, `atmo_radius`, `_pad3` with `rayleigh_intensity`, `rayleigh_falloff`, `nightglow_intensity`, `nightglow_falloff`, `nightglow_balance`, `rayleigh_radius`, `nightglow_orange_radius`, `nightglow_green_radius`. Update the compile-time size assertion to 192. Update `default_test_uniforms()` to set the new fields (all atmosphere intensities to 0.0 to match existing test behavior). Update the WGSL `Uniforms` struct in the `uniform_buffer_field_offsets_match_wgsl` test and its readback assertions to validate all eight new fields. Update the `uniforms` value in the test to set `rayleigh_intensity: 0.3`, `rayleigh_falloff: 6.0`, `nightglow_intensity: 0.3`, `nightglow_falloff: 4.0`, `nightglow_balance: 0.5`, `rayleigh_radius: 1.003`, `nightglow_orange_radius: 1.014`, `nightglow_green_radius: 1.015` and assert each readback value matches.
- **Test cases**:
  - `uniform_buffer_field_offsets_match_wgsl`: All eight new atmosphere uniforms read back correctly from the GPU
  - All existing render tests pass unchanged (they set atmosphere intensity to 0.0)
- **Verify**: `cargo test --test render_pipeline` passes
- **Complexity**: Medium

#### Step 8.3: Visual verification

- **Files**: N/A (manual testing)
- **Action**: Manual verification of the complete refactored feature.
- **Manual test cases**:
  - Launch app with default settings. Verify blue Rayleigh glow on the day-side limb, orange/yellow nightglow near the terminator on the night side, green nightglow deeper into the night side.
  - Verify the three shells are at visually distinct radii (Rayleigh hugs the surface, nightglow sits slightly higher).
  - Set Rayleigh intensity to 0. Verify only nightglow is visible.
  - Set Nightglow intensity to 0. Verify only Rayleigh is visible.
  - Slide the Balance slider fully to orange (0.0). Verify no green nightglow, only orange.
  - Slide the Balance slider fully to green (1.0). Verify no orange nightglow, only green.
  - Slide the Balance to 0.5. Verify both orange and green are visible.
  - Adjust Rayleigh falloff from 1.0 to 10.0. Verify the Rayleigh glow width changes from wide halo to tight rim.
  - Adjust Nightglow falloff from 1.0 to 10.0. Verify the nightglow glow width changes.
  - Uncheck the Atmosphere enable checkbox. Verify all atmosphere layers disappear. Re-check it. Verify they return.
  - Enable clouds. Verify clouds render on top of all atmosphere layers (correct draw order).
  - Click "Set as Wallpaper". Verify the exported wallpaper includes all three atmosphere layers.
  - Click "Reset All". Verify all atmosphere parameters return to defaults.
  - Close and reopen the app. Verify atmosphere settings persisted from the config file.
  - Rotate the globe (drag). Verify the atmosphere layers rotate with the globe and the color distribution follows the sun direction.
  - Change MSAA setting. Verify all atmosphere layers still render correctly after pipeline rebuild.
  - Verify nightglow is brighter near +/-20 degrees latitude than at the poles or equator.
  - Verify orange nightglow is brighter near the terminator, green nightglow is brighter at midnight.
- **Verify**: All manual checks pass
- **Complexity**: Small

#### Step 8.4: Run full test suite

- **Files**: N/A
- **Action**: Run `cargo test` and `cargo clippy`. Fix any failures introduced by the refactored parameters.
- **Verify**: `cargo test` passes, `cargo clippy` passes
- **Complexity**: Small

#### Step 8.5: Update CLAUDE.md

- **Files**: `CLAUDE.md`
- **Action**: Update the renderer/uniforms description to reflect the new 192-byte struct with the eight atmosphere fields. Update the shader description to mention the three atmosphere shell entry points (`vs_rayleigh`/`fs_rayleigh`, `vs_nightglow_orange`/`fs_nightglow_orange`, `vs_nightglow_green`/`fs_nightglow_green`). Update the UI description to mention the reorganized Atmosphere GroupBox with Rayleigh and Nightglow sub-sections. Update the dirty-checking field list to include the five new user-facing parameters.
- **Verify**: CLAUDE.md accurately describes the new architecture
- **Complexity**: Small

## Test Strategy

### Automated Tests

| Test Case | Type | Input | Expected Output |
| --- | --- | --- | --- |
| `Uniforms` size assertion | Compile-time | `size_of::<Uniforms>()` | 192 |
| `rayleigh_intensity` quantization | Unit | `rayleigh_intensity = 0.3` | `FrameState.rayleigh_intensity == 300` |
| `rayleigh_falloff` quantization | Unit | `rayleigh_falloff = 6.0` | `FrameState.rayleigh_falloff == 6000` |
| `nightglow_intensity` quantization | Unit | `nightglow_intensity = 0.3` | `FrameState.nightglow_intensity == 300` |
| `nightglow_falloff` quantization | Unit | `nightglow_falloff = 4.0` | `FrameState.nightglow_falloff == 4000` |
| `nightglow_balance` quantization | Unit | `nightglow_balance = 0.5` | `FrameState.nightglow_balance == 500` |
| `rayleigh_intensity` triggers dirty | Unit | Change 0.3 to 0.5 | `state_a != state_b` |
| `rayleigh_falloff` triggers dirty | Unit | Change 6.0 to 8.0 | `state_a != state_b` |
| `nightglow_intensity` triggers dirty | Unit | Change 0.3 to 0.5 | `state_a != state_b` |
| `nightglow_falloff` triggers dirty | Unit | Change 4.0 to 6.0 | `state_a != state_b` |
| `nightglow_balance` triggers dirty | Unit | Change 0.5 to 0.8 | `state_a != state_b` |
| `AppConfig` default `rayleigh_intensity` | Unit | `AppConfig::default()` | `0.3` |
| `AppConfig` default `rayleigh_falloff` | Unit | `AppConfig::default()` | `6.0` |
| `AppConfig` default `nightglow_intensity` | Unit | `AppConfig::default()` | `0.3` |
| `AppConfig` default `nightglow_falloff` | Unit | `AppConfig::default()` | `4.0` |
| `AppConfig` default `nightglow_balance` | Unit | `AppConfig::default()` | `0.5` |
| Missing atmosphere fields deserialize to defaults | Unit | TOML without atmo fields | Defaults filled |
| Config serde round-trip with new fields | Unit | Serialize then deserialize | Fields preserved |
| Uniform buffer field offsets match WGSL | GPU integration | Known values written | Correct values read back |
| Existing GPU render tests | GPU integration | Existing inputs | Pass unchanged |

### Manual Verification

- [ ] Three atmosphere layers visually distinguishable: blue Rayleigh (day), orange nightglow (post-sunset night), green nightglow (deep night)
- [ ] Rayleigh glow extends beyond the sphere silhouette, hugging the surface at day-side limb
- [ ] Nightglow shells sit visibly higher than Rayleigh
- [ ] Orange nightglow is brighter near the terminator, green is brighter at midnight
- [ ] Nightglow shows latitude enhancement near +/-20 degrees
- [ ] Balance slider shifts relative weight between orange and green nightglow
- [ ] Rayleigh intensity slider scales brightness from invisible (0) to prominent (1)
- [ ] Nightglow intensity slider scales brightness for both layers simultaneously
- [ ] Falloff sliders adjust glow width from wide halo (1) to tight rim (10)
- [ ] Enable/disable checkbox works, collapsing all sliders when unchecked
- [ ] Clouds render on top of all atmosphere layers (correct draw order)
- [ ] Wallpaper export includes all three atmosphere layers
- [ ] Reset All restores all atmosphere defaults
- [ ] Settings persist across app restart
- [ ] MSAA change does not break atmosphere rendering
- [ ] Config files with old `atmo_intensity`/`atmo_falloff` fields are silently ignored (forward compatibility via `#[serde(default)]`)

## Risks and Mitigations

| Risk | Impact | Mitigation |
| --- | --- | --- |
| WGSL syntax error in new fragment shaders | App crashes at pipeline creation | Test by running the app immediately after adding shader code; WGSL errors produce clear messages |
| Three additive blending passes cause brightness overflow | Atmosphere glow clips to white at the limb | Default intensities are conservative (0.3 each); the user can reduce them. The `vec4(atmo, 0.0)` return with zero alpha prevents alpha accumulation issues. |
| Nightglow latitude modulation looks artificial | Unnatural banding at +/-20 degrees | The Gaussian modulation function (`exp(-pow(...))`) provides a smooth gradient, not a hard edge. Tuning the width parameter (0.2 radians FWHM) during visual verification. |
| Old config files have `atmo_intensity`/`atmo_falloff` but not the new fields | Config load silently fills new fields with defaults thanks to `#[serde(default)]` | Verified by existing `deserialize_missing_fields_fills_defaults` test pattern |
| `build_frame_state` parameter count grows further (now 26 arguments) | Clippy warning | Already suppressed with `#[allow(clippy::too_many_arguments)]`; a future refactor to a parameter struct is noted in the codebase docs |
| Three draw calls instead of one increases fill rate cost | Increased fill rate | Negligible at the app's 2-minute redraw interval; each is a single indexed draw of the same sphere mesh with no texture sampling |
| GPU integration tests fail due to uniform buffer size change | The uniform buffer size is allocated from the Rust struct size, so it grows automatically. Test `Uniforms` instances need the new fields. | Addressed in Step 8.2 |
| Performance regression at high MSAA on 4K | Three additional full-screen alpha passes | At the app's 2-minute redraw interval this is negligible. The atmosphere can be disabled if needed. |

## Rollback Strategy

All changes are contained in the `feature-airglow` branch. If issues arise:

1. `git revert` the refactor commit(s) to restore the single-shell atmosphere
2. The uniform struct reverts to 176 bytes with the original four atmosphere fields
3. Config files with unknown `rayleigh_*`/`nightglow_*` fields are silently ignored by serde `#[serde(default)]`
4. No database migrations, no external API changes, no persistent side effects beyond the config file

## File Change Summary

| File | Change | Lines (est.) |
| --- | --- | --- |
| `src/renderer/uniforms.rs` | Replace 4 atmo fields with 8 new fields, update size assertion | +5, -4 |
| `shaders/sphere.wgsl` | Replace 4 uniform fields with 8, replace `vs_atmo`/`fs_atmo` with 6 entry points | +100, -30 |
| `src/renderer/gpu_setup.rs` | Replace `create_atmo_pipeline` with 3 constructors, update lifecycle | +80, -40 |
| `src/renderer/mod.rs` | Replace `atmo_pipeline` with 3 fields, update BeforeRendering and export | +30, -15 |
| `src/renderer/render_pass.rs` | Replace 3 atmo fields in ShadingParams with 8, update encode/execute | +30, -15 |
| `src/renderer/frame.rs` | Replace 3 FrameState fields with 5, update build function, update tests | +80, -60 |
| `ui/main.slint` | Replace 2 atmo sliders with 5 sliders in 2 sub-sections | +50, -20 |
| `src/config.rs` | Replace 2 atmo fields with 5, update defaults and tests | +40, -20 |
| `src/main.rs` | Update config apply/read/reset for 5 new properties | +15, -10 |
| `tests/render_pipeline.rs` | Update Uniforms struct, size assertion, readback test | +30, -15 |
| `CLAUDE.md` | Update architecture descriptions | +10, -5 |

**Estimated total**: ~470 lines added, ~234 lines removed, net ~236 lines across 11 files.

## Status

- [ ] Plan approved
- [ ] Implementation started
- [ ] Implementation complete
