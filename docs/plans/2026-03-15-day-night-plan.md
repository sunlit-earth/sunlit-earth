# Plan: Day/Night Rendering (2026-03-15)

## Summary

Add day/night cycle rendering to Sunlit Earth by blending between the day and night textures based on the real-time sun position. The sun position is computed via the Astronomy Engine C library (through the `astronomy-engine-bindings` Rust crate), establishing the FFI integration pattern that future astronomy features (moon, planets, eclipses) will follow. The WGSL shader is extended with a second texture binding, a world-normal varying, and a `smoothstep`-based terminator blend. A periodic Slint timer drives automatic redraws as the sun moves. The UI gains three new controls: a terminator width slider, a diffuse shading toggle, and a "Day/Night Blend" entry in the texture combobox.

## Stakes Classification

**Level**: High
**Rationale**: This plan touches nearly every layer of the application: the shader, the uniform buffer layout, the bind group layout (which forces a pipeline rebuild), the render loop, the texture loading strategy, the dirty-checking system, a new FFI dependency, and the Slint UI. A mistake in the bind group layout or uniform alignment silently produces black frames or panics. The Astronomy Engine FFI introduces `unsafe` code into a codebase that has `unsafe_code = "deny"`. The changes are spread across 8+ files and introduce a new module. Rollback requires reverting multiple coordinated commits.

## Context

**Research**: [`docs/plans/2026-03-15-day-night-research.md`](2026-03-15-day-night-research.md)
**Affected Areas**: `Cargo.toml`, `src/main.rs`, `src/renderer.rs`, `src/sun.rs` (new), `shaders/sphere.wgsl`, `ui/main.slint`, `CLAUDE.md`, `build.rs` (if needed for clang/bindgen)

## Success Criteria

- [ ] `astronomy-engine-bindings` is added as a dependency and builds successfully
- [ ] A new `src/sun.rs` module provides a safe `sun_direction(utc: DateTime) -> glam::Vec3` wrapper around the Astronomy Engine FFI
- [ ] The sun direction is correct: at UTC noon on the March equinox, the subsolar point is near (0N, 0E), producing a sun direction vector near (+1, 0, 0) in the renderer's coordinate frame
- [ ] The uniform buffer is expanded to 96 bytes: MVP (64) + sun_dir (12) + terminator_width (4) + flags (4) + padding (12)
- [ ] The bind group layout has a fourth binding (night texture) and the uniform buffer is visible to both vertex and fragment stages
- [ ] The shader blends day and night textures using `smoothstep(-w, w, NdotL)` where `w` is the terminator width uniform
- [ ] A "Day/Night Blend" option appears in the texture combobox (index 3)
- [ ] Selecting "Day/Night Blend" triggers loading of both day and night textures
- [ ] While either texture is still loading, the renderer falls back to single-texture mode with whatever is available
- [ ] A composite bind group (with both textures) is created as soon as both textures finish loading
- [ ] A terminator width slider appears in the UI and controls the twilight band width in real time
- [ ] A diffuse shading checkbox appears in the UI; when enabled, the day texture is modulated by `max(0, NdotL)`
- [ ] A periodic timer (every 2 minutes) requests redraws so the terminator moves with the sun
- [ ] The sun direction (quantized to the nearest minute) is included in dirty-checking
- [ ] Existing single-texture modes (Grid, Day, Night) continue to work unchanged
- [ ] `cargo build` succeeds with no warnings
- [ ] `cargo clippy` passes
- [ ] `cargo test` passes (existing tests unbroken, new sun module tests pass)
- [ ] The application runs and renders the day/night terminator correctly

## Implementation Steps

### Phase 1: Astronomy Engine Integration

This phase adds the `astronomy-engine-bindings` dependency and creates a safe Rust wrapper module that computes the sun's direction vector in the renderer's coordinate frame.

#### Step 1.1: Add `astronomy-engine-bindings` to `Cargo.toml`

- **Files**: `Cargo.toml`
- **Action**: Add the dependency: `astronomy-engine-bindings = "2.1"`. This crate uses `bindgen` at build time, which requires `clang` to be installed. Verify that the project builds on the development machine. If `clang` is not found, document the requirement.
- **Verify**: `cargo check` succeeds. `cargo tree -i astronomy-engine-bindings` shows the crate.
- **Complexity**: Small

#### Step 1.2: Create the `src/sun.rs` module with safe FFI wrapper

- **Files**: `src/sun.rs` (new), `src/main.rs` (add `mod sun;`)
- **Action**: Create a new module that wraps the Astronomy Engine FFI calls behind a safe Rust API. The module needs a targeted `#[allow(unsafe_code)]` attribute because the project has `unsafe_code = "deny"` globally, but the FFI calls to the C library are inherently unsafe.

  The module provides one public function:

  ```rust
  /// Compute the sun's direction as a unit vector in the renderer's
  /// world-space coordinate frame (Y-up, +X = prime meridian at equator,
  /// -Z = 90 degrees East).
  pub fn sun_direction_now() -> glam::Vec3
  ```

  Implementation outline:
  1. Get the current UTC time via `std::time::SystemTime` / `chrono` or by decomposing into year/month/day/hour/minute/second manually using `time` arithmetic.
  2. Call `Astronomy_MakeTime(year, month, day, hour, minute, second)` to create an `astro_time_t`.
  3. Call `Astronomy_GeoVector(BODY_SUN, time, ABERRATION)` to get the sun's geocentric equatorial J2000 position as an `astro_vector_t` with (x, y, z) in AU.
  4. Convert the J2000 equatorial vector to the renderer's geographic coordinate frame. The J2000 equatorial frame has +X toward the vernal equinox, +Z toward the north celestial pole. The renderer's frame has +X toward 0-lon/0-lat, +Y toward the north pole, -Z toward 90E. The conversion requires accounting for Earth's rotation (Greenwich Sidereal Time) to rotate the equatorial vector into the Earth-fixed frame, then swapping axes to match the renderer's convention.
  5. Normalize the vector and return it as a `glam::Vec3`.

  Alternative simpler approach for the equatorial-to-geographic conversion: instead of manually computing sidereal time and rotating the equatorial vector, use Astronomy Engine's `Astronomy_SunPosition` function, which returns ecliptic longitude/latitude of the sun. Then compute the subsolar point (geographic lat/lon) from the sun's declination and the equation of time (or right ascension + sidereal time), and convert to the renderer's Cartesian frame using the formula from the research document:

  ```text
  sun_x =  cos(phi) * cos(lambda)
  sun_y =  sin(phi)
  sun_z = -cos(phi) * sin(lambda)
  ```

  where phi = subsolar latitude, lambda = subsolar longitude.

  The recommended approach: use `Astronomy_Equator` (which gives right ascension and declination of the sun accounting for nutation and aberration), then compute subsolar latitude = declination, subsolar longitude = (Greenwich Apparent Sidereal Time - right ascension) converted to degrees, and apply the Cartesian formula. Astronomy Engine provides `Astronomy_SiderealTime` for GAST.

  For UTC time decomposition without adding a `chrono` dependency: use the `time` crate or compute year/month/day/hour/minute/second from `SystemTime::now().duration_since(UNIX_EPOCH)` using a helper function. Alternatively, add a lightweight time-decomposition dependency. The plan recommends adding the `time` crate (`time = { version = "0.3", features = ["std"] }`) for reliable UTC decomposition, since manually computing calendar dates from Unix timestamps is error-prone and not the focus of this project.

- **Verify**: `cargo build` succeeds. The module compiles without errors.
- **Complexity**: Large

#### Step 1.3: Write tests for the sun direction computation

- **Files**: `src/sun.rs` (test submodule)
- **Action**: Add unit tests that verify the sun direction against known reference dates. These tests validate the entire pipeline from UTC time to renderer-space direction vector.
- **Test cases**:
  - **March equinox at UTC noon** (2025-03-20 12:00 UTC): The subsolar point is approximately (0N, 0E). Expected sun direction: approximately `(1.0, 0.0, 0.0)` (with tolerance of ~0.05 for Earth's axial tilt and equation of time on that specific date).
  - **June solstice at UTC noon** (2025-06-21 12:00 UTC): The subsolar point is approximately (23.4N, 0E). Expected sun direction: Y component should be ~sin(23.4 deg) = ~0.397, X component should be ~cos(23.4 deg) = ~0.917, Z near 0.
  - **December solstice at UTC noon** (2025-12-21 12:00 UTC): The subsolar point is approximately (23.4S, 0E). Expected sun direction: Y component should be ~-0.397.
  - **UTC midnight** (any equinox, 00:00 UTC): The subsolar point should be near the international date line (180E). Expected sun direction: X near -1, Y near 0, Z near 0.
  - **Normalization**: The returned vector should have unit length (within 1e-4 tolerance).
- **Verify**: `cargo test sun` passes.
- **Complexity**: Medium

#### Step 1.4: Add the `time` crate dependency for UTC decomposition

- **Files**: `Cargo.toml`
- **Action**: Add `time = { version = "0.3", features = ["std"] }` to dependencies. This is used by `src/sun.rs` to decompose `SystemTime::now()` into year/month/day/hour/minute/second components for passing to `Astronomy_MakeTime`. If an alternative approach is preferred (e.g., computing calendar components from `UNIX_EPOCH` duration directly), this step can be skipped, but the `time` crate avoids manual leap-year and calendar arithmetic.
- **Verify**: `cargo check` succeeds.
- **Complexity**: Small

### Phase 2: Shader Extension

This phase modifies the WGSL shader to support two-texture blending with a smooth terminator.

#### Step 2.1: Expand the uniform struct in the shader

- **Files**: `shaders/sphere.wgsl`
- **Action**: Expand the `Uniforms` struct to include the sun direction, terminator width, and a flags field:

  ```wgsl
  struct Uniforms {
      mvp: mat4x4<f32>,          // 64 bytes, offset 0
      sun_dir: vec3<f32>,        // 12 bytes, offset 64
      terminator_width: f32,     // 4 bytes, offset 76
      flags: u32,                // 4 bytes, offset 80 (bit 0: diffuse shading)
      _pad1: f32,                // 4 bytes, offset 84
      _pad2: f32,                // 4 bytes, offset 88
      _pad3: f32,                // 4 bytes, offset 92
  };
  ```

  Total: 96 bytes. The struct must be 16-byte aligned at the end per std140 rules. The `flags` field uses bit 0 for diffuse shading toggle. Padding fields bring the total to a multiple of 16.

  Alternatively, use `vec4<f32>` for sun direction (packing terminator width in .w) and a separate `vec4<u32>` for flags with padding. The approach above is more explicit and readable.

- **Verify**: Shader compiles (verified by `cargo build` since the shader is included via `include_str!`).
- **Complexity**: Small

#### Step 2.2: Add world normal varying and night texture binding

- **Files**: `shaders/sphere.wgsl`
- **Action**: Make three changes to the shader:

  1. Add `world_normal: vec3<f32>` to `VertexOutput` at `@location(1)`.
  2. In `vs_main`, set `out.world_normal = in.position` (vertex position on a unit sphere equals the surface normal).
  3. Add the night texture binding:

     ```wgsl
     @group(0) @binding(3)
     var night_texture: texture_2d<f32>;
     ```

  The existing `sphere_texture` (binding 1) becomes the day texture. The sampler (binding 2) is shared between both textures.

- **Verify**: `cargo build` succeeds (the shader compiles via `include_str!`). Note: the shader will reference bindings that don't exist in the current bind group layout yet -- this is fine because the shader is not validated against the layout until pipeline creation.
- **Complexity**: Small

#### Step 2.3: Implement the blending fragment shader

- **Files**: `shaders/sphere.wgsl`
- **Action**: Replace the `fs_main` function with logic that handles both single-texture and blend modes:

  ```wgsl
  @fragment
  fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
      let day_color = textureSample(sphere_texture, sphere_sampler, in.uv).rgb;

      // If terminator_width is negative, we're in single-texture mode
      // (the night texture binding may be a dummy placeholder)
      if uniforms.terminator_width < 0.0 {
          return vec4<f32>(day_color, 1.0);
      }

      let night_color = textureSample(night_texture, sphere_sampler, in.uv).rgb;
      let n = normalize(in.world_normal);
      let n_dot_l = dot(n, uniforms.sun_dir);
      let w = uniforms.terminator_width;
      let blend = smoothstep(-w, w, n_dot_l);

      var lit_day = day_color;
      // Diffuse shading: modulate day texture by max(0, NdotL)
      if (uniforms.flags & 1u) != 0u {
          lit_day = day_color * max(0.0, n_dot_l);
      }

      let color = mix(night_color, lit_day, blend);
      return vec4<f32>(color, 1.0);
  }
  ```

  The sentinel value `terminator_width < 0.0` distinguishes single-texture mode (existing Grid/Day/Night options) from blend mode. When negative, the shader ignores the night texture entirely, preserving backward compatibility with the existing single-texture bind groups that have no binding 3.

  **Important design note**: Single-texture bind groups (Grid, Day, Night) do not have a night texture at binding 3. To avoid needing two different pipeline layouts, a 1x1 dummy texture is bound at binding 3 for single-texture modes. This keeps one bind group layout and one pipeline for all modes.

- **Verify**: `cargo build` succeeds.
- **Complexity**: Medium

### Phase 3: Renderer Changes -- Uniform Buffer and Bind Group Layout

This phase updates the Rust-side GPU resource setup to match the new shader.

#### Step 3.1: Define the Rust-side uniform struct

- **Files**: `src/renderer.rs`
- **Action**: Add a `#[repr(C)]` uniform struct that matches the WGSL layout:

  ```rust
  #[repr(C)]
  #[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
  struct Uniforms {
      mvp: [f32; 16],           // 64 bytes
      sun_dir: [f32; 3],        // 12 bytes
      terminator_width: f32,    // 4 bytes
      flags: u32,               // 4 bytes
      _pad: [f32; 3],           // 12 bytes
  }
  ```

  Total: 96 bytes. The `mvp` field uses a raw `[f32; 16]` array so the struct can derive `bytemuck::Pod` (glam's `Mat4` doesn't implement `Pod`). The MVP matrix is converted to `[f32; 16]` via `mat4.to_cols_array()` when populating the struct.

  For single-texture mode, `terminator_width` is set to `-1.0` as the sentinel value, and `sun_dir` / `flags` are ignored by the shader.

- **Verify**: `cargo build` succeeds. `std::mem::size_of::<Uniforms>() == 96`.
- **Complexity**: Small

#### Step 3.2: Expand the uniform buffer to 96 bytes

- **Files**: `src/renderer.rs` (in `create_gpu_resources`)
- **Action**: Change the uniform buffer size from `64` to `size_of::<Uniforms>() as u64` (which is 96). Update the `write_buffer` call in the `BeforeRendering` handler to write the full `Uniforms` struct instead of just the MVP matrix.
- **Verify**: `cargo build` succeeds.
- **Complexity**: Small

#### Step 3.3: Expand the bind group layout with night texture binding

- **Files**: `src/renderer.rs` (in `create_gpu_resources`)
- **Action**: Make two changes to the bind group layout:

  1. Change binding 0's visibility from `ShaderStages::VERTEX` to `ShaderStages::VERTEX | ShaderStages::FRAGMENT` (the fragment shader now reads the uniform buffer for sun direction).
  2. Add binding 3: a `texture_2d<f32>` with `FRAGMENT` visibility for the night texture.

  ```rust
  wgpu::BindGroupLayoutEntry {
      binding: 3,
      visibility: wgpu::ShaderStages::FRAGMENT,
      ty: wgpu::BindingType::Texture {
          sample_type: wgpu::TextureSampleType::Float { filterable: true },
          view_dimension: wgpu::TextureViewDimension::D2,
          multisampled: false,
      },
      count: None,
  },
  ```

- **Verify**: `cargo build` succeeds.
- **Complexity**: Small

#### Step 3.4: Create a 1x1 dummy texture for single-texture bind groups

- **Files**: `src/renderer.rs`
- **Action**: In `create_gpu_resources`, create a 1x1 RGBA8 dummy texture (e.g., a single black pixel) and store its `TextureView` in `GpuResources`. This dummy texture is used as the night texture binding (binding 3) in single-texture bind groups (Grid, Day, Night). This avoids needing separate bind group layouts or pipelines for single-texture vs. blend modes.

  Add a `dummy_texture_view: wgpu::TextureView` field to `GpuResources`.

- **Verify**: `cargo build` succeeds.
- **Complexity**: Small

#### Step 3.5: Update `create_bind_group` to accept a night texture view

- **Files**: `src/renderer.rs`
- **Action**: Modify the `create_bind_group` function to accept an additional `night_texture_view: &wgpu::TextureView` parameter. Add a fourth entry for binding 3. Update all call sites:
  - Grid bind group: pass the dummy texture view.
  - In `process_decoded_textures`: when creating bind groups for individual texture slots (Day, Night), pass the dummy texture view.

- **Verify**: `cargo build` succeeds. Existing single-texture rendering still works.
- **Complexity**: Small

### Phase 4: Composite Bind Group and Blend-Mode Texture Loading

This phase implements the two-texture blend mode: eager loading of both textures, creation of a composite bind group, and fallback behavior during loading.

#### Step 4.1: Add blend-mode fields to `GpuResources`

- **Files**: `src/renderer.rs`
- **Action**: Add fields to `GpuResources` to support the composite bind group:

  ```rust
  /// Bind group containing both day and night textures, used in blend mode.
  /// Created once both day and night texture slots have loaded.
  composite_bind_group: Option<wgpu::BindGroup>,
  /// Stored texture views for day and night, needed to build the composite
  /// bind group when both become available.
  day_texture_view: Option<wgpu::TextureView>,
  night_texture_view: Option<wgpu::TextureView>,
  ```

  Initialize all three to `None` in `create_gpu_resources`.

- **Verify**: `cargo build` succeeds.
- **Complexity**: Small

#### Step 4.2: Store texture views when textures finish loading

- **Files**: `src/renderer.rs` (in `process_decoded_textures`)
- **Action**: When a texture finishes decoding and its bind group is created, also store the `TextureView` in the appropriate field (`day_texture_view` for slot 1, `night_texture_view` for slot 2). After storing, check whether both views are now available. If so, create the composite bind group immediately:

  ```rust
  if let (Some(day_view), Some(night_view)) =
      (&res.day_texture_view, &res.night_texture_view)
  {
      res.composite_bind_group = Some(create_composite_bind_group(
          &res.device,
          &res.bind_group_layout,
          &res.uniform_buffer,
          day_view,
          night_view,
          &res.sampler,
      ));
  }
  ```

  The `create_composite_bind_group` function is a new helper that creates a bind group with the day texture at binding 1 and the night texture at binding 3.

- **Verify**: `cargo build` succeeds.
- **Complexity**: Medium

#### Step 4.3: Trigger eager loading of both textures in blend mode

- **Files**: `src/renderer.rs`
- **Action**: Modify the texture loading logic in the `BeforeRendering` handler. Currently, `maybe_spawn_texture_load` is called for the single selected slot. When the texture index indicates blend mode (index 3), call `maybe_spawn_texture_load` for both slot 1 (day) and slot 2 (night):

  ```rust
  if slot_index == BLEND_MODE_INDEX {
      maybe_spawn_texture_load(res, DAY_SLOT);
      maybe_spawn_texture_load(res, NIGHT_SLOT);
  } else {
      maybe_spawn_texture_load(res, slot_index);
  }
  ```

  Define constants: `const DAY_SLOT: usize = 1;`, `const NIGHT_SLOT: usize = 2;`, `const BLEND_MODE_INDEX: usize = 3;`.

- **Verify**: `cargo build` succeeds. Selecting "Day/Night Blend" triggers loading of both textures (observable via stderr timing output).
- **Complexity**: Small

#### Step 4.4: Update render path for blend mode

- **Files**: `src/renderer.rs` (in the `BeforeRendering` handler)
- **Action**: Modify the render pass to use the composite bind group when in blend mode. The logic for resolving which bind group to use:

  1. If texture index is blend mode (3) and `composite_bind_group` is `Some`: use the composite bind group. Write the full `Uniforms` struct with the real sun direction, terminator width from the UI slider, and flags from the UI checkbox.
  2. If texture index is blend mode but `composite_bind_group` is `None` (textures still loading): fall back to single-texture rendering. If day (slot 1) is loaded, use its bind group. Otherwise use `last_rendered_index`. Write the `Uniforms` struct with `terminator_width = -1.0` to signal single-texture mode to the shader.
  3. If texture index is not blend mode (0, 1, or 2): use the per-slot bind group as before. Write `terminator_width = -1.0`.

  Update the loading indicator to show status for blend mode: "Loading Day..." or "Loading Night..." or "Loading Day and Night..." as appropriate.

- **Verify**: `cargo build` succeeds. Manual testing: selecting blend mode shows a fallback texture while the two textures load, then transitions to the blended view.
- **Complexity**: Medium

### Phase 5: Sun Direction and Dirty-Checking

This phase integrates the sun direction computation into the render loop and adds time-driven updates.

#### Step 5.1: Add sun direction to `FrameState`

- **Files**: `src/renderer.rs`
- **Action**: Add `sun_direction: [i32; 3]` to the `FrameState` struct. The sun direction is quantized to integer milliradians (multiply each component by 1000 and cast to i32) to avoid floating-point comparison issues. This means the scene re-renders when the sun moves by ~0.057 degrees, which is roughly once every 15 seconds -- well within the visual threshold for a wallpaper app.

  Also add `terminator_width: i32` (quantized similarly) and `diffuse_shading: bool` to `FrameState` so that changes to these UI controls trigger re-renders.

- **Verify**: `cargo build` succeeds.
- **Complexity**: Small

#### Step 5.2: Compute sun direction in the render loop

- **Files**: `src/renderer.rs` (in `BeforeRendering` handler)
- **Action**: At the start of each `BeforeRendering` callback, call `sun::sun_direction_now()` to get the current sun direction. Store it in the `FrameState` (quantized) and use it to populate the `Uniforms` struct. This computation is cheap (microseconds) and can be done every frame.

- **Verify**: `cargo build` succeeds.
- **Complexity**: Small

#### Step 5.3: Add a periodic redraw timer

- **Files**: `src/main.rs`
- **Action**: After creating the window, create a `slint::Timer` with `TimerMode::Repeated` and a 2-minute interval. The timer callback calls `win.window().request_redraw()`. The timer must be kept alive (stored in a variable that lives until `window.run()` returns).

  ```rust
  let window_weak = window.as_weak();
  let sun_timer = slint::Timer::default();
  sun_timer.start(
      slint::TimerMode::Repeated,
      std::time::Duration::from_secs(120),
      move || {
          if let Some(win) = window_weak.upgrade() {
              win.window().request_redraw();
          }
      },
  );
  ```

  The `sun_timer` variable must remain in scope (not dropped) for the timer to keep firing. Place it before `window.run()` and let it drop naturally when the event loop exits.

- **Verify**: `cargo build` succeeds. The scene updates automatically every 2 minutes without user interaction (observable by watching the terminator slowly shift).
- **Complexity**: Small

### Phase 6: UI Changes

This phase adds the new UI controls: the blend mode combobox entry, the terminator width slider, and the diffuse shading checkbox.

#### Step 6.1: Add "Day/Night Blend" to the texture combobox

- **Files**: `ui/main.slint`, `src/main.rs`
- **Action**: In `main.slint`, change the default `texture-options` from `["Grid", "Day", "Night"]` to `["Grid", "Day", "Night", "Day/Night Blend"]`. In `main.rs`, update the texture labels vector to include the fourth option. Set the default `texture_index` to `3` (blend mode) so the app starts in the most interesting mode.

- **Verify**: `cargo build` succeeds. The combobox shows four options.
- **Complexity**: Small

#### Step 6.2: Add the terminator width slider

- **Files**: `ui/main.slint`, `src/renderer.rs`
- **Action**: In `main.slint`, add a new slider row after the zoom slider:

  ```slint
  HorizontalLayout {
      spacing: 4px;
      Text {
          text: "Terminator";
          vertical-alignment: center;
          min-width: 70px;
      }
      terminator-slider := Slider {
          minimum: 0.01;
          maximum: 0.3;
          value: 0.1;
          changed => { root.sliders-changed(); }
      }
      Text {
          text: round(terminator-slider.value * 1000) / 1000;
          vertical-alignment: center;
          min-width: 35px;
      }
  }
  ```

  Add an `out property <float> terminator-width: terminator-slider.value;` to the root component.

  The slider range 0.01 to 0.3 corresponds to twilight bands from ~1.1 degrees to ~34 degrees of solar elevation. The default of 0.1 (~11.5 degrees) matches the research recommendation.

  In `src/renderer.rs`, read this property via `win.get_terminator_width()` and pass it to the `Uniforms` struct.

- **Verify**: `cargo build` succeeds. Moving the slider changes the terminator band width in real time.
- **Complexity**: Small

#### Step 6.3: Add the diffuse shading checkbox

- **Files**: `ui/main.slint`, `src/renderer.rs`
- **Action**: In `main.slint`, add a checkbox row after the terminator slider. Slint does not have a built-in `CheckBox` in `std-widgets.slint` -- use a `Switch` component or a `TouchArea` with a visual toggle. The simplest approach is to import `CheckBox` from `std-widgets.slint` (it is available in Slint 1.15).

  ```slint
  HorizontalLayout {
      spacing: 4px;
      Text {
          text: "Diffuse";
          vertical-alignment: center;
          min-width: 70px;
      }
      diffuse-check := CheckBox {
          text: "Dayside shading";
          checked: false;
          toggled => { root.sliders-changed(); }
      }
  }
  ```

  Add `out property <bool> diffuse-shading: diffuse-check.checked;` to the root.

  In `src/renderer.rs`, read `win.get_diffuse_shading()` and set `uniforms.flags = if diffuse { 1 } else { 0 }`.

  Import `CheckBox` at the top of `main.slint`: add it to the existing import from `"std-widgets.slint"`.

- **Verify**: `cargo build` succeeds. Toggling the checkbox visually modulates the dayside brightness.
- **Complexity**: Small

#### Step 6.4: Wire up redraw callbacks for new controls

- **Files**: `src/main.rs`
- **Action**: The new controls (terminator slider and diffuse checkbox) both fire the existing `sliders-changed` callback, which already triggers `request_redraw()`. No additional callback wiring is needed. Verify this is the case.
- **Verify**: Moving the terminator slider or toggling the checkbox triggers a re-render.
- **Complexity**: Small

### Phase 7: Integration and Polish

#### Step 7.1: Ensure single-texture modes still work

- **Files**: `src/renderer.rs`
- **Action**: Verify that when the texture index is 0 (Grid), 1 (Day), or 2 (Night), the renderer:
  - Uses the per-slot bind group (with the dummy night texture at binding 3).
  - Writes `terminator_width = -1.0` in the uniforms so the shader skips blend logic.
  - Does not attempt to sample the night texture.

  This should already work from the design in Phase 2-3, but explicitly verify with manual testing.

- **Verify**: Selecting Grid, Day, and Night individually works exactly as before.
- **Complexity**: Small

#### Step 7.2: Update `CLAUDE.md`

- **Files**: `CLAUDE.md`
- **Action**: Update the architecture documentation:
  - Add `sun.rs` to the "Key modules" list: "safe wrapper around Astronomy Engine FFI for sun position computation"
  - Update the shader description to mention day/night blending
  - Update the UI description to mention the terminator slider and diffuse shading checkbox
  - Update the uniform buffer description from 64 bytes to 96 bytes
  - Mention the periodic redraw timer in the rendering lifecycle section
  - Add `astronomy-engine-bindings` and `time` to the notable dependencies
- **Verify**: `CLAUDE.md` accurately reflects the new architecture.
- **Complexity**: Small

### Phase 8: Testing and Verification

#### Step 8.1: Run existing test suite

- **Files**: N/A
- **Action**: Run `cargo test` to confirm all existing tests pass. The camera, sphere, and grid_texture tests should be unaffected. The new sun module tests (from Step 1.3) should pass.
- **Verify**: `cargo test` passes with zero failures.
- **Complexity**: Small

#### Step 8.2: Run clippy

- **Files**: N/A
- **Action**: Run `cargo clippy` and fix any warnings. Pay special attention to:
  - The `#[allow(unsafe_code)]` in `sun.rs` -- ensure it's scoped as narrowly as possible (on the function or block, not the whole module).
  - Any `cast_possible_truncation` warnings from the quantized sun direction.
  - Unused imports or dead code.
- **Verify**: `cargo clippy` exits with zero warnings.
- **Complexity**: Small

#### Step 8.3: Manual end-to-end verification

- **Files**: N/A (manual verification)
- **Action**: Run the application and test all rendering modes and UI controls.
- **Manual test cases**:
  - Application starts in Day/Night Blend mode, shows loading indicator, then renders the blended view
  - The terminator is visible as a smooth transition between day and night hemispheres
  - The terminator position corresponds approximately to the current real-world day/night boundary
  - Moving the terminator width slider makes the twilight band narrower (left) or wider (right)
  - Toggling diffuse shading darkens the dayside near the terminator and brightens the subsolar point
  - Selecting "Grid" shows the grid texture with no blending
  - Selecting "Day" shows only the day texture with no blending
  - Selecting "Night" shows only the night texture with no blending
  - Switching back to "Day/Night Blend" immediately shows the blended view (textures already cached)
  - Camera controls (longitude, latitude, zoom) work in all modes
  - MSAA switching works in all modes
  - Window resize works in all modes
  - After waiting >2 minutes, the terminator has visibly shifted (~0.5 degrees)
  - `cargo run -- --software-rendering` works (software fallback)
- **Verify**: All manual test cases pass.
- **Complexity**: Medium

## Test Strategy

### Automated Tests

| Test Case | Type | Input | Expected Output |
| --- | --- | --- | --- |
| Sun direction at March equinox noon UTC | Unit | 2025-03-20 12:00 UTC | Vec3 ~(1.0, 0.0, 0.0) within 0.05 tolerance |
| Sun direction at June solstice noon UTC | Unit | 2025-06-21 12:00 UTC | Y ~0.40, X ~0.92, Z ~0.0 within 0.05 |
| Sun direction at December solstice noon UTC | Unit | 2025-12-21 12:00 UTC | Y ~-0.40 within 0.05 |
| Sun direction at UTC midnight (equinox) | Unit | 2025-03-20 00:00 UTC | X ~-1.0 within 0.05 |
| Sun direction is unit vector | Unit | Any time | length within 1e-4 of 1.0 |
| Uniforms struct size | Unit | `size_of::<Uniforms>()` | 96 |
| Sphere vertex count | Unit (existing) | `generate_uv_sphere(4, 8)` | 45 vertices |
| Sphere index count | Unit (existing) | `generate_uv_sphere(4, 8)` | 192 indices |
| Camera eye position | Unit (existing) | `OrbitalCamera::new(0, 0, 5)` | (0, 0, 5) |

### Manual Verification

- [ ] Application starts in Day/Night Blend mode and renders correctly after textures load
- [ ] Terminator position matches the real-world day/night boundary (approximately)
- [ ] Terminator width slider changes the twilight band width in real time
- [ ] Diffuse shading checkbox modulates dayside brightness when enabled
- [ ] Single-texture modes (Grid, Day, Night) still work identically to before
- [ ] Texture switching is smooth: blend mode uses fallback while loading, then transitions
- [ ] Periodic timer causes automatic updates (visible after 2+ minutes)
- [ ] All camera, MSAA, and resize interactions work in all modes
- [ ] `cargo build --release` succeeds
- [ ] `cargo clippy` passes with no warnings
- [ ] `cargo test` passes

## Risks and Mitigations

| Risk | Impact | Mitigation |
| --- | --- | --- |
| `astronomy-engine-bindings` requires `clang` for bindgen at build time | Build failure on machines without clang | Document the requirement in CLAUDE.md and README. On Windows, clang can be installed via `winget install LLVM.LLVM` or the Visual Studio build tools. If clang is unavailable on CI, consider vendoring the pre-generated bindings. |
| `#[allow(unsafe_code)]` in `sun.rs` weakens the project's safety posture | Potential UB from incorrect FFI calls | Scope the allow attribute as narrowly as possible (on the unsafe block, not the module). The FFI surface is small (3-4 function calls) and the C library is well-tested. Add comments explaining why each unsafe call is sound. |
| Uniform buffer alignment mismatch between Rust and WGSL | Black screen or garbled rendering | Use `#[repr(C)]` and `bytemuck::Pod` on the Rust struct. Add a compile-time assertion (`const _: () = assert!(size_of::<Uniforms>() == 96);`) to catch size drift. Test with a known sun direction and verify the terminator appears at the expected position. |
| Dual texture memory (~256 MB for two 8K textures) | Out-of-memory on low-VRAM GPUs | This interacts with the existing MSAA memory concern (documented in `docs/notes.md`). Blend mode is opt-in (user selects it). Single-texture modes remain the default escape hatch. Future work could add texture resolution options. |
| Astronomy Engine coordinate frame conversion is wrong | Terminator appears in the wrong position | The automated tests (Step 1.3) verify against known reference dates. If the terminator is off, the conversion logic in `sun.rs` is the first place to debug. The research document provides the expected Cartesian formulas. |
| Composite bind group created before both textures are ready | Crash or missing binding | The code explicitly checks `if let (Some(day), Some(night)) = ...` before creating the composite bind group. Blend mode falls back to single-texture rendering while waiting. |
| Slint `Timer` dropped before event loop starts | Timer never fires | Ensure the `sun_timer` variable is declared before `window.run()` and is not consumed or moved. The timer only needs to live until the event loop exits. |
| `time` crate version conflict with other dependencies | Build failure | The `time` crate is widely used and version 0.3.x is stable. Check `cargo tree -d` for duplicate versions after adding. |

## Rollback Strategy

Revert the commits from this plan. The changes span multiple files but are contained within a clear set:

- Remove `astronomy-engine-bindings` and `time` from `Cargo.toml`
- Delete `src/sun.rs`
- Revert `shaders/sphere.wgsl` to the single-texture version
- Revert `src/renderer.rs` (uniform buffer back to 64 bytes, bind group layout back to 3 entries, remove composite bind group logic)
- Revert `src/main.rs` (remove timer, revert texture labels)
- Revert `ui/main.slint` (remove terminator slider, diffuse checkbox, blend combobox entry)
- Revert `CLAUDE.md`

No data files are modified. The JXL texture files are untracked and remain in `textures/`.

## File Inventory

```text
Cargo.toml                 (modified: add astronomy-engine-bindings, time)
src/main.rs                (modified: add mod sun, timer, texture labels, default index)
src/sun.rs                 (new: Astronomy Engine FFI wrapper, sun direction computation)
src/renderer.rs            (modified: Uniforms struct, 96-byte buffer, bind group layout,
                            composite bind group, blend-mode render path, dummy texture,
                            FrameState expansion, sun direction integration)
shaders/sphere.wgsl        (modified: expanded uniforms, world_normal varying, night
                            texture binding, smoothstep blend logic, diffuse shading)
ui/main.slint              (modified: Day/Night Blend combobox entry, terminator slider,
                            diffuse shading checkbox, new out properties)
CLAUDE.md                  (modified: updated architecture docs)
```

## Status

- [x] Plan approved
- [x] Implementation started
- [x] Implementation complete
