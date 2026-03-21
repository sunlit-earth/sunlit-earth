# Plan: Cloud Layer Rendering (2026-03-18)

## Summary

Add a semi-transparent cloud layer to the Sunlit Earth globe, rendered as a second sphere at a slightly larger radius than the Earth surface. The cloud texture is loaded from a static file `textures/cloud.png` (an equirectangular grayscale PNG). Clouds are lit by the sun following the same day/night logic as the Earth surface, with a user-controlled opacity slider. This is a first iteration: no live data fetching, no cloud shadows.

## Stakes Classification

**Level**: Medium

**Rationale**: The change touches multiple modules (uniforms, shaders, pipeline setup, render pass, frame state, texture loading, UI, wallpaper export) but follows well-established patterns in the codebase. Each integration point has a clear precedent (the day/night blend mode, Fresnel parameters, composite bind group). Rollback is straightforward since the cloud layer is an additive feature with no changes to existing rendering behavior. The main risk is MSAA mismatch between the two pipelines, which the compile-time size assertion and existing rebuild functions mitigate.

## Context

**Research**: [Cloud Layer Research](2026-03-17-cloud-layer-research.md) (data sources, rendering approach, technical decisions), [Cloud Layer Codebase Research](2026-03-17-cloud-layer-codebase.md) (architecture analysis, integration points, line-level references)

**Affected Areas**: `src/renderer/` (uniforms, frame, render_pass, gpu_setup, textures, texture_routing, mod), `shaders/sphere.wgsl`, `ui/main.slint`, `src/main.rs`, `src/config.rs`, `tests/render_pipeline.rs`

## Key Decisions

Decisions that were unresolved or ambiguous during initial planning, now resolved through codebase research.

### D1: Cloud texture loading is separate from the texture combobox system

The Earth surface texture combobox (Grid / Day / Night / Day+Night Blend) routes through `resolve_textures()` in `texture_routing.rs`, which maps combobox indices to texture slot indices. The cloud texture is not a selectable surface mode -- it is an overlay layer that renders regardless of which Earth texture is active.

**Approach:** Pass `cloud_path: Option<PathBuf>` as a dedicated parameter to `create_gpu_resources`, separate from `texture_paths`. Push the cloud `TextureSlot` after the texture_paths loop, giving it slot index 3. Trigger cloud loading unconditionally in `resolve_textures` via `maybe_spawn_texture_load(res, CLOUDS_SLOT)`, bypassing the combobox routing logic entirely.

**Why not add cloud_path to texture_paths:** Adding it would make `texture_slots[3]` the cloud slot, which shares its numeric value with `BLEND_MODE_INDEX = 3` (the combobox index for Day+Night Blend). Although these namespaces don't technically collide in the current code -- `is_blend_mode` catches `raw_index == 3` before the generic slot lookup -- the coincidence is fragile and confusing. Keeping the cloud path separate makes the intent explicit and avoids coupling the overlay layer to the surface texture selection system.

### D2: CLOUDS_SLOT = 3, clearly documented as a slot index

After the texture_paths loop produces slots [grid(0), day(1), night(2)], the cloud slot is pushed at index 3. `CLOUDS_SLOT: usize = 3` is a texture slot index. `BLEND_MODE_INDEX: usize = 3` is a combobox index. These share a numeric value but operate in different namespaces:

- `BLEND_MODE_INDEX` is compared against `raw_index` (from the UI combobox) in the `is_blend_mode` guard. It never indexes into `texture_slots`.
- `CLOUDS_SLOT` indexes directly into `texture_slots` for loading and bind group creation. It is never used as a combobox value.

A comment on the `CLOUDS_SLOT` constant documents this distinction.

### D3: Cloud shader uses a single smoothstep for day/night transition

The cloud fragment shader computes brightness as:

```wgsl
let brightness = mix(0.05, 1.0, smoothstep(-uniforms.terminator_width, uniforms.terminator_width, n_dot_l));
```

This gives night brightness of 0.05 (faint ambient) and day brightness of 1.0 (fully lit white clouds), with a smooth transition matching the Earth's terminator width. No secondary diffuse ramp is applied to clouds for v1 -- clouds are diffuse scatterers with roughly uniform brightness across the dayside, and the single smoothstep produces a visually correct result. If dayside cloud shading is later desired (darker clouds near the terminator), it can be added as a follow-up.

### D4: RGBA8 texture format for v1, R8Unorm deferred

The cloud PNG is loaded as RGBA8 through the existing `texture_loader::load()` pipeline, which auto-detects format via the `image` crate (PNG is supported natively). The grayscale PNG is expanded to RGBA (r=g=b=gray, a=255) by the image crate, and the cloud fragment shader samples the `r` channel for cloud density.

R8Unorm would save ~25 MB for a 4096x2048 texture, but requires a separate path through `create_mipmapped_texture` / `downsample_2x` (both currently hardcoded for RGBA8). The implementation cost is disproportionate to the memory savings for a single static texture in v1. The shader code (`textureSample().r`) is identical for both formats, so the optimization is purely a texture creation change with no shader impact.

### D5: Cloud bind group reuses the Earth bind group layout

The bind group layout has 4 entries: uniforms (binding 0), primary texture (binding 1), sampler (binding 2), secondary texture (binding 3). For the cloud bind group, binding 1 is the cloud texture view, binding 3 is the dummy 1x1 black texture (unused by `fs_cloud` but required by the layout since the shader module declares all bindings at module scope). This follows the same pattern used by single-texture modes (Grid, Day, Night), which also bind the dummy texture at binding 3.

### D6: Uniform struct padding uses separate f32 fields

The two new fields (`cloud_sphere_radius`, `cloud_opacity`) add 8 bytes at offset 128. The struct must reach 144 bytes (next 16-byte boundary) for std140 alignment. Padding uses `_pad3: f32` and `_pad4: f32` (two separate fields) in both Rust and WGSL, consistent with the existing `_pad: f32` and `_pad2: f32` style in the codebase.

## Success Criteria

- [ ] Cloud sphere renders as a second sphere at radius 1.0015, visible at multiple zoom levels
- [ ] Cloud texture loaded from `textures/cloud.png` as RGBA8 with mipmaps
- [ ] Clouds are lit by the sun: bright on day side, dim/dark on night side, with smooth terminator transition
- [ ] Cloud Opacity slider (0.0--1.0) in the UI controls cloud transparency
- [ ] Cloud opacity is included in dirty-checking; changing the slider triggers a re-render
- [ ] Clouds are skipped gracefully when `textures/cloud.png` is missing (no crash, no error in UI)
- [ ] Cloud pipeline uses alpha blending with depth writes disabled; Earth pipeline is unchanged
- [ ] Both pipelines share the same vertex/index buffer; cloud radius is scaled in the vertex shader
- [ ] Wallpaper export includes clouds at the correct opacity
- [ ] Cloud pipeline matches the Earth pipeline's MSAA sample count at all times
- [ ] Reset All resets cloud opacity to its default value
- [ ] All existing tests continue to pass
- [ ] New unit tests cover cloud opacity quantization in FrameState
- [ ] GPU integration test confirms the cloud pipeline compiles and renders non-transparent output

## Implementation Steps

### Phase 1: Uniforms and Shader

Extend the GPU-side data structures and write the cloud vertex/fragment shaders. This is the foundation that all later phases depend on.

#### Step 1.1: Extend Uniforms struct (Rust side)

- **Files**: `src/renderer/uniforms.rs:1-23`
- **Action**: Add four fields after `fresnel_exp`: `cloud_sphere_radius: f32`, `cloud_opacity: f32`, `_pad3: f32`, `_pad4: f32`. Update the compile-time size assertion from 128 to 144 bytes.
- **Verify**: `cargo build` succeeds with the new struct size
- **Complexity**: Small

#### Step 1.2: Extend Uniforms struct (WGSL side)

- **Files**: `shaders/sphere.wgsl:1-15`
- **Action**: Add `cloud_sphere_radius: f32`, `cloud_opacity: f32`, `_pad3: f32`, `_pad4: f32` to the WGSL `Uniforms` struct after `fresnel_exp`, matching the Rust layout field-by-field.
- **Verify**: Deferred to Step 1.4 (shader compiles)
- **Complexity**: Small

#### Step 1.3: Write cloud vertex and fragment shaders

- **Files**: `shaders/sphere.wgsl` (append after `fs_main`)
- **Action**: Add two new entry points at the end of the file:
  - `vs_cloud`: identical to `vs_main` except `in.position` is multiplied by `uniforms.cloud_sphere_radius` before the MVP transform. `world_normal` is set to `normalize(in.position)` (unscaled, preserving the unit-sphere normal direction). `world_position` is set to the scaled position.
  - `fs_cloud`: samples `sphere_texture` (binding 1) at `in.uv`, extracts the `r` channel as cloud density. Computes `n_dot_l = dot(normalize(in.world_normal), uniforms.sun_dir)`. Computes brightness via `mix(0.05, 1.0, smoothstep(-uniforms.terminator_width, uniforms.terminator_width, n_dot_l))`. Returns `vec4<f32>(brightness, brightness, brightness, cloud_density * uniforms.cloud_opacity)`.

  No Fresnel or specular effects on clouds -- clouds are diffuse scatterers. No secondary diffuse ramp -- the single smoothstep produces uniform dayside brightness (see decision D3).
- **Verify**: Deferred to Step 1.4 (shader compiles)
- **Complexity**: Medium

#### Step 1.4: Create cloud render pipeline

- **Files**: `src/renderer/gpu_setup.rs:241-287` (near `create_pipeline`)
- **Action**: Add a `create_cloud_pipeline` function modeled on `create_pipeline` (line 241) with these differences:
  - `entry_point: Some("vs_cloud")` for vertex
  - `entry_point: Some("fs_cloud")` for fragment
  - `blend: Some(wgpu::BlendState::ALPHA_BLENDING)` in the color target (non-premultiplied straight alpha)
  - `depth_write_enabled: false` in the depth stencil state
  - `depth_compare: wgpu::CompareFunction::Less` (same as Earth)
  - Same `MultisampleState`, `PrimitiveState`, and pipeline layout as the Earth pipeline
- **Verify**: `cargo build` succeeds
- **Complexity**: Medium

#### Step 1.5: Add cloud pipeline to GpuResources and initialization

- **Files**: `src/renderer/mod.rs:172-223` (GpuResources struct), `src/renderer/gpu_setup.rs:206-238` (create_gpu_resources return)
- **Action**:
  - Add three fields to `GpuResources`: `cloud_pipeline: wgpu::RenderPipeline`, `cloud_bind_group: Option<wgpu::BindGroup>`, `cloud_texture_view: Option<wgpu::TextureView>`
  - In `create_gpu_resources`, call `create_cloud_pipeline` after `create_pipeline` and assign to the new field
  - Initialize `cloud_bind_group: None` and `cloud_texture_view: None`
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

#### Step 1.6: Rebuild cloud pipeline on MSAA change

- **Files**: `src/renderer/gpu_setup.rs:372-376` (`rebuild_msaa_resources`)
- **Action**: After recreating `res.pipeline` on line 374, also recreate `res.cloud_pipeline` using `create_cloud_pipeline` with the new `sample_count`. This ensures both pipelines always have matching MSAA state.
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

### Phase 2: Frame State and Shading Params

Wire cloud opacity through the dirty-checking and uniform-writing paths.

#### Step 2.1: Add cloud_opacity to FrameState (RED)

- **Files**: `src/renderer/frame.rs:84-349` (tests section)
- **Action**: Write failing tests for the new `cloud_opacity` field before modifying the struct:
  - `frame_state_cloud_opacity_quantization`: build with `cloud_opacity=0.75`, assert `state.cloud_opacity == 750`
  - `frame_state_cloud_opacity_triggers_dirty`: build two states with different `cloud_opacity`, assert not equal
  - Update `default_frame_state()` helper to pass `cloud_opacity: 0.8`
  - Update all existing test call sites of `build_frame_state` to include the new parameter
- **Verify**: Tests exist and fail (no implementation yet)
- **Complexity**: Small

#### Step 2.2: Implement cloud_opacity in FrameState (GREEN)

- **Files**: `src/renderer/frame.rs:1-82`
- **Action**:
  - Add `pub cloud_opacity: i32` field to `FrameState` (after `fresnel_exp`, line 33)
  - Add `cloud_opacity: f32` parameter to `build_frame_state()` (after `fresnel_exp` on line 53)
  - Quantize: `cloud_opacity: (cloud_opacity * 1000.0) as i32` in the struct initialization
- **Verify**: All tests from Step 2.1 pass, plus all existing frame tests pass with updated call sites
- **Complexity**: Small

#### Step 2.3: Add cloud fields to ShadingParams and write_uniforms

- **Files**: `src/renderer/render_pass.rs:11-24` (ShadingParams), `src/renderer/render_pass.rs:59-100` (write_uniforms)
- **Action**:
  - Add `pub cloud_sphere_radius: f32` and `pub cloud_opacity: f32` to `ShadingParams`
  - In `write_uniforms`, populate the new Uniforms fields: `cloud_sphere_radius: shading.cloud_sphere_radius`, `cloud_opacity: shading.cloud_opacity`, `_pad3: 0.0`, `_pad4: 0.0`
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

#### Step 2.4: Wire cloud_opacity through rendering_callback

- **Files**: `src/renderer/mod.rs:363-378` (build_frame_state call), `src/renderer/mod.rs:410-421` (ShadingParams construction)
- **Action**:
  - Read `cloud_opacity_f` from the window: `let cloud_opacity_f = win.get_cloud_opacity();`
  - Pass `cloud_opacity_f` as the last argument to `build_frame_state` (after `fresnel_exp_f` on line 377)
  - Add `cloud_sphere_radius: 1.0015` and `cloud_opacity: cloud_opacity_f` to the `ShadingParams` construction (after `fresnel_exp` on line 420)
- **Note**: This step depends on the Slint property `cloud-opacity` existing (Phase 3 Step 3.1). If building incrementally, use a temporary hardcoded value `0.8` until the UI property is added.
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

### Phase 3: UI

Add the cloud opacity slider to the Slint UI. This phase can be implemented in parallel with Phase 2 Steps 2.1--2.3, but Step 2.4 depends on the property existing.

#### Step 3.1: Add cloud-opacity property to MainWindow

- **Files**: `ui/main.slint:50-51` (after `fresnel-exp` property)
- **Action**: Add `in-out property <float> cloud-opacity: 0.8;` to the MainWindow property declarations.
- **Verify**: `cargo build` succeeds (Slint generates getter/setter)
- **Complexity**: Small

#### Step 3.2: Add Cloud Opacity slider to UI

- **Files**: `ui/main.slint:493` (after the Lighting GroupBox closing brace, before the Date / Time GroupBox)
- **Action**: Insert a new GroupBox titled "Clouds" containing a single slider row:
  - Label text: "Opacity"
  - Slider: `minimum: 0.0`, `maximum: 1.0`, `value <=> root.cloud-opacity`, `changed => { root.sliders-changed(); }`
  - Display label: `round(root.cloud-opacity * 100) + "%"`
  - Follow the same `HorizontalLayout` pattern used by the Lighting sliders (spacing 4px, label min-width 70px, display min-width 35px)
- **Verify**: `cargo run` shows the slider in the expected position
- **Complexity**: Small

#### Step 3.3: Include cloud_opacity in Reset All

- **Files**: `src/main.rs:233-250` (reset-all callback)
- **Action**: After the existing Fresnel reset (line 242), add `win.set_cloud_opacity(lighting.cloud_opacity);` using the `AppConfig::default()` value, consistent with how all other lighting properties are reset.
- **Verify**: Manual test after full build
- **Complexity**: Small

#### Step 3.4: Persist cloud_opacity in config

- **Files**: `src/config.rs:21-69` (AppConfig struct), `src/config.rs:76-108` (Default impl), `src/main.rs:281-319` (apply_config_to_window), `src/main.rs:321-358` (read_config_from_window)
- **Action**: Follow the same pattern used by `fresnel_mix` and `fresnel_exp`:
  1. Add `pub cloud_opacity: f32` to the `AppConfig` struct (after `fresnel_exp`). The `#[serde(default)]` on the struct ensures config files without this field silently get the default value (forward compatibility for rollback).
  2. In the `Default` impl, set `cloud_opacity: 0.8`.
  3. In `apply_config_to_window`, add `window.set_cloud_opacity(config.cloud_opacity);` after the Fresnel lines.
  4. In `read_config_from_window`, add `cloud_opacity: window.get_cloud_opacity(),` to the `AppConfig` struct literal.
  No additional timer wiring is needed -- the existing `on_sliders_changed` callback already restarts the debounce timer, and the cloud opacity slider fires `sliders-changed` via its `changed` handler (Step 3.2).
- **Test cases**:
  - Config round-trip: save config with `cloud_opacity: 0.6`, reload, verify value persists
  - Missing field: load a config file without `cloud_opacity`, verify default 0.8 is used
  - These are covered by the existing config test patterns in `src/config.rs`
- **Verify**: `cargo test config` passes; manual test: change slider, restart app, slider retains value
- **Complexity**: Small

### Phase 4: Cloud Texture Loading

Load the cloud texture from disk and create the cloud bind group. The cloud texture uses a dedicated loading path, separate from the Earth texture combobox system (see decision D1).

#### Step 4.1: Add CLOUDS_SLOT constant

- **Files**: `src/renderer/mod.rs:33-38` (slot constants)
- **Action**: Add `const CLOUDS_SLOT: usize = 3;` after `BLEND_MODE_INDEX` in `mod.rs`, with a comment:

  ```rust
  /// Texture slot index for the cloud overlay texture.
  /// This shares its numeric value with `BLEND_MODE_INDEX` (a combobox index),
  /// but the two are used in different contexts: `CLOUDS_SLOT` indexes into
  /// `texture_slots` for loading/bind-group creation, while `BLEND_MODE_INDEX`
  /// is compared against the UI combobox value in `resolve_textures`.
  const CLOUDS_SLOT: usize = 3;
  ```
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

#### Step 4.2: Pass cloud_path separately into create_gpu_resources

- **Files**: `src/main.rs:60-70` (texture path setup), `src/renderer/mod.rs` (setup_rendering_notifier signature), `src/renderer/gpu_setup.rs` (create_gpu_resources signature)
- **Action**:
  - In `main.rs`, resolve the cloud texture path separately from `texture_paths`:

    ```rust
    let cloud_path = textures_dir.as_ref()
        .map(|d| d.join("cloud.png"))
        .filter(|p| p.exists());
    ```

    Keep `texture_paths = vec![day_path, night_path]` unchanged.
  - Thread `cloud_path` through `setup_rendering_notifier` to `create_gpu_resources` as a separate `Option<PathBuf>` parameter.
  - In `create_gpu_resources`, after the `texture_paths` loop (which creates slots 0=grid, 1=day, 2=night), push one additional slot for clouds:

    ```rust
    texture_slots.push(TextureSlot {
        bind_group: None,
        source_path: cloud_path,
        loading: false,
    });
    ```

    This creates slot 3 = cloud, matching `CLOUDS_SLOT`.
- **Verify**: `cargo build` succeeds. With `textures/cloud.png` absent, slot 3 has `source_path: None` and is never loaded. With it present, `source_path: Some(...)` is set.
- **Complexity**: Small

#### Step 4.3: Trigger cloud texture loading

- **Files**: `src/renderer/texture_routing.rs:22-49` (resolve_textures)
- **Action**: At the end of `resolve_textures`, after the existing blend mode / slot loading logic, add an unconditional call to start cloud texture loading:

  ```rust
  if CLOUDS_SLOT < res.texture_slots.len() {
      maybe_spawn_texture_load(res, CLOUDS_SLOT);
  }
  ```

  This triggers loading regardless of which Earth texture mode is selected. The existing guard inside `maybe_spawn_texture_load` (checking `source_path.is_some()` and `!loading`) prevents redundant loads and handles the missing-file case.
- **Verify**: `cargo build` succeeds. With cloud.png present, stderr shows the decode message. Without it, no error.
- **Complexity**: Small

#### Step 4.4: Handle decoded cloud texture and create cloud bind group

- **Files**: `src/renderer/textures.rs:28-75` (process_decoded_textures)
- **Action**: In `process_decoded_textures`, after the existing day/night texture view storage (lines 56-64), add cloud-specific handling:

  ```rust
  if msg.slot_index == super::CLOUDS_SLOT {
      res.cloud_texture_view = Some(tex_view.clone());
      maybe_create_cloud_bind_group(res);
  }
  ```

  Add a new function `maybe_create_cloud_bind_group` modeled on `maybe_create_composite_bind_group`:

  ```rust
  fn maybe_create_cloud_bind_group(res: &mut super::GpuResources) {
      if let Some(cloud_view) = &res.cloud_texture_view {
          res.cloud_bind_group = Some(create_bind_group(
              &res.device,
              &res.bind_group_layout,
              &res.uniform_buffer,
              cloud_view,
              &res.sampler,
              &res.dummy_texture_view, // binding 3 unused by fs_cloud
              "cloud_bind_group",
          ));
      }
  }
  ```

  The cloud bind group uses the same layout as all other bind groups. The dummy texture at binding 3 satisfies the layout requirement (the cloud shader does not sample it).

  The `texture_loader::load()` function handles PNG natively via the `image` crate (auto-format detection), and applies the same horizontal flip and 1/4-width shift as for Earth textures. Standard equirectangular cloud maps (including matteason) follow the same convention as NASA Blue Marble textures, so the transforms produce correct alignment. Verify visually during development.
- **Verify**: `cargo build` succeeds. With cloud.png present, `cloud_bind_group` becomes `Some`. Without it, remains `None`.
- **Complexity**: Medium

### Phase 5: Render Pass Integration

Issue the cloud draw call after the Earth draw call in the same render pass.

#### Step 5.1: Extend encode_and_submit to support optional cloud pass

- **Files**: `src/renderer/render_pass.rs:102-154` (encode_and_submit)
- **Action**: Add optional cloud pipeline and bind group parameters to `encode_and_submit`:

  ```rust
  pub(super) fn encode_and_submit(
      device, queue, target, pipeline, bind_group,
      vertex_buffer, index_buffer, index_count,
      cloud_pipeline: Option<&wgpu::RenderPipeline>,
      cloud_bind_group: Option<&wgpu::BindGroup>,
  )
  ```

  After the existing `pass.draw_indexed` call (line 150), add:

  ```rust
  if let (Some(cloud_pipe), Some(cloud_bg)) = (cloud_pipeline, cloud_bind_group) {
      pass.set_pipeline(cloud_pipe);
      pass.set_bind_group(0, cloud_bg, &[]);
      // Vertex and index buffers remain bound from the Earth draw
      pass.draw_indexed(0..index_count, 0, 0..1);
  }
  ```

  Both draws occur within a single render pass. The MSAA resolve fires once when the pass ends, covering both Earth and cloud fragments. The `LoadOp::Clear` fires once at the start. Alpha blending and depth behavior are configured in the cloud pipeline state (Step 1.4), not per-draw-call.
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

#### Step 5.2: Pass cloud resources through execute_render_pass

- **Files**: `src/renderer/render_pass.rs:156-207` (execute_render_pass)
- **Action**: Extend `execute_render_pass` to pass cloud resources to `encode_and_submit`. The cloud draw is issued only when both conditions are met:
  1. `res.cloud_bind_group` is `Some` (cloud texture has loaded)
  2. `shading.cloud_opacity > 0.0` (user has not disabled clouds)

  When either condition fails, pass `None, None` to skip the cloud draw call entirely.
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

#### Step 5.3: Pass cloud resources through export_wallpaper_image

- **Files**: `src/renderer/mod.rs:82-169` (export_wallpaper_image)
- **Action**: In the `encode_and_submit` call within `export_wallpaper_image`, pass the cloud pipeline and cloud bind group using the same logic as the preview path. This ensures the wallpaper export includes clouds at the same opacity as the preview.
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

#### Step 5.4: Update all existing callers of encode_and_submit

- **Files**: Any file calling `encode_and_submit` (currently only `render_pass.rs:execute_render_pass` and `mod.rs:export_wallpaper_image`)
- **Action**: Ensure all call sites pass the two new optional parameters. Call sites that don't need clouds pass `None, None`.
- **Verify**: `cargo build` succeeds with no warnings
- **Complexity**: Small

### Phase 6: Testing

Add automated tests for the new functionality.

#### Step 6.1: Unit tests for cloud_opacity in FrameState

- **Files**: `src/renderer/frame.rs` (tests section)
- **Action**: These tests were written in Step 2.1. Verify they pass after the implementation in Step 2.2:
  - `frame_state_cloud_opacity_quantization`: `cloud_opacity=0.75` produces `state.cloud_opacity == 750`
  - `frame_state_cloud_opacity_triggers_dirty`: different `cloud_opacity` values produce unequal states
  - All existing tests still pass (they were updated with the new parameter in Step 2.1)
- **Verify**: `cargo test frame` passes all frame state tests
- **Complexity**: Small

#### Step 6.2: Update GPU integration test Uniforms struct

- **Files**: `tests/render_pipeline.rs:18-35` (local Uniforms copy)
- **Action**: Add `cloud_sphere_radius: f32`, `cloud_opacity: f32`, `_pad3: f32`, and `_pad4: f32` to the test's local `Uniforms` struct. Update the size assertion from 128 to 144. Update all `Uniforms { ... }` initializations in the test file to include the new fields (use `cloud_sphere_radius: 1.0015`, `cloud_opacity: 0.0`, `_pad3: 0.0`, `_pad4: 0.0` as defaults).
- **Verify**: `cargo test --test render_pipeline` passes (existing tests still work)
- **Complexity**: Small

#### Step 6.3: GPU integration test for cloud pipeline

- **Files**: `tests/render_pipeline.rs` (append new test)
- **Action**: Add a test `cloud_pipeline_renders_with_alpha` that:
  1. Creates the cloud pipeline using the same shader source (blend.wgsl + sphere.wgsl) with `vs_cloud`/`fs_cloud` entry points, alpha blending enabled, depth writes disabled
  2. Creates a 1x1 white texture (R=255, G=255, B=255, A=255) as the cloud texture
  3. Sets `cloud_sphere_radius: 1.0015`, `cloud_opacity: 1.0`
  4. Renders a single frame to a small texture (e.g., 64x64)
  5. Reads back pixels and asserts that at least some pixels have non-zero alpha values or that the output differs from the clear color
  - This test validates that the cloud shader compiles and the cloud pipeline produces visible output.
- **Verify**: `cargo test --test render_pipeline cloud_pipeline` passes
- **Complexity**: Medium

#### Step 6.4: Update shading integration test Uniforms (if applicable)

- **Files**: `tests/shading.rs`
- **Action**: `tests/shading.rs` uses its own `TestCase` / `TestResult` structs for compute shader dispatch against `blend_fragment()`, not the full `Uniforms` struct. It does not have a local copy of `Uniforms`. No changes are needed unless the shading compute harness is modified. Verify by running the test suite.
- **Verify**: `cargo test --test shading` passes
- **Complexity**: Small

### Phase 7: Final Verification

#### Step 7.1: Full test suite

- **Files**: N/A
- **Action**: Run `cargo test` to confirm all unit and integration tests pass. Run `cargo clippy` to confirm no new warnings (pedantic mode is enabled).
- **Verify**: `cargo test` and `cargo clippy` both pass cleanly
- **Complexity**: Small

#### Step 7.2: Manual visual verification

- **Files**: N/A (manual verification)
- **Action**: Run the application and verify:
  - With `textures/cloud.png` present: clouds are visible on the globe
  - Without `textures/cloud.png`: no clouds, no error, Earth renders normally
  - Cloud opacity slider at 0%: no clouds visible
  - Cloud opacity slider at 100%: clouds fully opaque
  - Cloud opacity slider at 50%: clouds partially transparent, Earth visible through thin areas
  - Clouds are bright on the day side, dim/dark on the night side
  - At close zoom: cloud parallax is visible (clouds slightly above Earth surface)
  - At full-globe zoom: clouds appear painted on (parallax sub-pixel, which is correct)
  - MSAA change while clouds are visible: no crash, clouds still render correctly
  - Reset All: cloud opacity returns to 80%
  - "Set as Wallpaper": exported image includes clouds
  - Cloud/Earth alignment: clouds roughly overlay correct geographic areas (verify continents are not shifted)
- **Verify**: All manual checks pass
- **Complexity**: Small

## Test Strategy

### Automated Tests

| Test Case | Type | Input | Expected Output |
| --- | --- | --- | --- |
| cloud_opacity quantization | Unit | `cloud_opacity=0.75` | `state.cloud_opacity == 750` |
| cloud_opacity triggers dirty | Unit | Two states with different `cloud_opacity` | `state_a != state_b` |
| cloud_opacity zero vs nonzero | Unit | 0.0 vs 0.001 | Different FrameState values |
| Existing frame state tests (updated) | Unit | All existing inputs + `cloud_opacity` param | Same outputs as before |
| Uniforms struct size | Compile-time | `size_of::<Uniforms>()` | 144 |
| Cloud pipeline compiles | GPU Integration | blend.wgsl + sphere.wgsl with vs_cloud/fs_cloud | No shader compilation error |
| Cloud pipeline renders output | GPU Integration | White 1x1 texture, opacity 1.0, full draw | Non-zero pixel data in output |
| Existing render_pipeline tests | GPU Integration | Updated Uniforms struct | Same behavioral invariants |
| Existing shading tests | GPU Integration | Unmodified (no Uniforms dependency) | Same behavioral invariants |
| Config round-trip cloud_opacity | Unit | Save with `cloud_opacity: 0.6`, reload | Value persists as 0.6 |
| Config missing cloud_opacity field | Unit | Load config file without field | Default 0.8 used |

### Manual Verification

- [ ] Cloud sphere visible at multiple zoom levels with correct day/night lighting
- [ ] Cloud opacity slider controls transparency smoothly from 0% to 100%
- [ ] Application starts without crash when `textures/cloud.png` is missing
- [ ] MSAA toggle while clouds are visible does not crash
- [ ] Reset All resets cloud opacity to 80%
- [ ] Wallpaper export includes clouds at the current opacity
- [ ] No visual regression in Earth surface rendering (same colors, same lighting)
- [ ] Cloud/Earth geographic alignment is correct (no horizontal shift)
- [ ] Cloud opacity persists across app restart (change slider, quit, relaunch)

## Risks and Mitigations

| Risk | Impact | Mitigation |
| --- | --- | --- |
| MSAA mismatch between Earth and cloud pipelines | wgpu panic at render time | Both pipelines created/rebuilt with the same `sample_count` in `create_gpu_resources` and `rebuild_msaa_resources` |
| Uniform struct alignment broken | GPU data misinterpretation (garbled rendering) | Compile-time `size_of` assertion updated to 144; WGSL struct manually matched field-by-field |
| Cloud texture horizontally misaligned with Earth | Clouds appear shifted relative to continents | Same `texture_loader::load()` applies identical flip and 1/4-width shift; standard equirectangular cloud maps follow the same convention as NASA Blue Marble textures; verify visually |
| `CLOUDS_SLOT` shares numeric value with `BLEND_MODE_INDEX` | Potential confusion in future maintenance | These operate in different namespaces (slot index vs. combobox index); cloud loading path is fully separate from combobox routing; documented with a comment on the constant |
| Cloud draw call when bind group is None | wgpu panic from null bind group | Guard: cloud draw only issued when `cloud_bind_group.is_some() && cloud_opacity > 0.0` |
| Existing tests break from Uniforms size change | CI regression | Update local Uniforms copy in `tests/render_pipeline.rs` in the same PR; `tests/shading.rs` uses its own compute shader structs and is unaffected |

## Rollback Strategy

All changes are additive. To roll back:

1. Revert the commit(s) on the `cloud-layer` branch.
2. No database migrations, config file format changes, or external API contracts are involved.
3. The `cloud_opacity` field in `AppConfig` uses `#[serde(default)]`, so config files written with this field are silently handled by older versions that lack the field (forward-compatible rollback).

## Out of Scope (Documented)

These items are explicitly deferred per the scope constraints:

- **Live cloud data fetching**: No HTTP client, no automatic updates, no caching
- **R8Unorm texture optimization**: The cloud PNG is loaded as RGBA8 through the existing pipeline; R8Unorm saves memory but adds complexity to the mipmap path (see decision D4)
- **Cloud shadows on the Earth surface**: Requires ray-sphere intersection in the Earth fragment shader
- **Post-processing of cloud textures**: The PNG is used as-is; no contrast adjustment, no threshold filtering
- **Limb fade**: An optional `smoothstep(0.0, 0.15, n_dot_v)` to fade cloud alpha at the globe's edge could simulate atmospheric depth. Deferred because the cloud sphere at radius 1.0015 extends only ~0.75 pixels beyond the Earth sphere at typical resolutions, making the hard edge negligible.

## Status

- [ ] Plan approved
- [ ] Implementation started
- [ ] Implementation complete
