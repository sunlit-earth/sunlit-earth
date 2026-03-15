# Research: Day/Night Rendering (2026-03-15)

## Problem Statement

Sunlit Earth currently renders a single texture on a 3D sphere with no lighting model. To achieve realistic Earth visualization, the renderer must blend between day and night textures based on the sun's real-time position, showing daylight on the sun-facing hemisphere and city lights on the dark side, with a smooth terminator transition between them.

This is a foundational rendering feature that must also be architected to accommodate future layers: city lights modulated by night-side darkness, atmospheric glow at the limb, and cloud overlays.

## Requirements

1. **Real-time sun position** -- compute the sun's direction in world space from the current UTC date/time.
2. **Two-texture blending** -- sample both day and night textures per fragment and blend based on solar illumination.
3. **Smooth terminator** -- the day/night boundary must transition gradually, simulating twilight rather than a hard edge.
4. **Time-driven updates** -- the scene must re-render periodically as the sun moves, not only on user interaction.
5. **Backward compatibility** -- existing single-texture modes (Grid, Day-only, Night-only) must continue to work alongside the new blend mode.
6. **Future-proof architecture** -- the uniform buffer, bind group layout, and shader structure should accommodate additional layers (city lights, atmosphere, clouds) without requiring another full pipeline rebuild.

## Findings

### Sun Position Computation

Two approaches are available for computing the sun's world-space direction:

**Primary approach: Astronomy Engine (recommended).** The project has already selected Astronomy Engine (C library via `astronomy-engine-bindings` Rust crate) as its astronomy backend. It provides sub-arcminute accuracy using VSOP87, handles all coordinate transforms, and will also be needed for future features (moon position, planet positions, eclipses). Using it for sun position establishes the integration pattern that all later astronomy features will follow. The API surface needed is small: given a UTC datetime, compute the sun's geocentric equatorial position and convert to a direction vector in the renderer's coordinate frame.

**Fallback approach: simplified trigonometric formulas.** The external research documented a compact, self-contained formula chain that requires no external dependencies:

1. **Solar declination** from day-of-year: `delta = 23.44 * sin(360/365 * (d - 81))` (error ~1-2 degrees).
2. **Equation of time** correction: `EoT = 9.87*sin(2B) - 7.53*cos(B) - 1.5*sin(B)` where `B = 360/365 * (d - 81)` (error ~0.5 min, shifting the terminator by ~0.1 degrees).
3. **Subsolar point**: latitude equals declination; longitude is `-15 * (utc_hours - 12 + EoT/60)`.
4. **3D direction vector** from subsolar lat/lon using standard spherical-to-Cartesian conversion.

This approach is adequate for visual rendering and could serve as a temporary implementation during development or as a fallback if the Astronomy Engine FFI integration is deferred. However, it would need to be replaced before features like moon rendering or eclipse visualization are added. The recommendation is to use Astronomy Engine from the start to avoid throwaway code.

**Confidence:** High for both approaches. The simplified formulas are well-sourced from multiple independent references (NOAA, PVEducation, PVPMC). Astronomy Engine's accuracy (~1 arcminute) far exceeds rendering needs.

**Gap:** The sign convention for subsolar longitude (east-positive vs. west-positive) in the simplified formula should be verified against a known reference date before implementation. Astronomy Engine handles this internally and does not have this concern.

### Coordinate Frame and World-Space Conventions

The renderer uses a Y-up coordinate system on a unit sphere centered at the origin:

- **+Y** points to the North Pole
- **+X** points toward 0 degrees longitude, 0 degrees latitude (prime meridian at equator)
- **-Z** points toward 90 degrees East (right-hand rule)

The vertex position on the unit sphere equals the surface normal (no separate normal attribute is needed). This is already the case in the current sphere mesh generator (`sphere.rs`), which produces `[f32; 3]` positions on a unit sphere. The vertex shader currently discards world-space position after MVP transformation; for day/night blending, it must pass the world-space position through to the fragment shader as an additional varying.

The sun direction vector formula for this coordinate frame (from subsolar latitude phi and longitude lambda, both in radians):

```text
sun_x =  cos(phi) * cos(lambda)
sun_y =  sin(phi)
sun_z = -cos(phi) * sin(lambda)
```

This produces a unit vector; no normalization is needed.

**Confidence:** High. The coordinate frame was verified in both the codebase research (sphere mesh, UV mapping) and the external research (standard geographic convention).

### Shader Architecture

The current shader (`shaders/sphere.wgsl`) is minimal: one MVP uniform, one texture, one sampler. The fragment shader samples a single texture and outputs it directly with no lighting.

For day/night blending, the shader must be extended with:

1. **Additional varying:** `world_normal: vec3<f32>` passed from vertex to fragment shader (equals the vertex position on a unit sphere).

2. **Additional bindings:**
   - A `sun_direction: vec4<f32>` field in the uniform struct (vec4 rather than vec3 for std140 alignment; the w component can encode terminator width or be reserved).
   - A second texture binding for the night texture.
   - Optionally a second sampler, though the existing sampler (trilinear + 16x anisotropic) can be reused for both textures.

3. **Blending logic** in the fragment shader:

   ```wgsl
   let n_dot_l: f32 = dot(normalize(in.world_normal), uniforms.sun_dir.xyz);
   let blend: f32 = smoothstep(-0.1, 0.1, n_dot_l);
   let color: vec3<f32> = mix(night_color, day_color, blend);
   ```

The `smoothstep` edge values control the twilight band width:

- `(-0.05, 0.05)` gives a narrow band (~5.7 degrees of solar elevation, roughly civil twilight).
- `(-0.1, 0.1)` gives a wider, more visually prominent band (~11.5 degrees).
- These are empirical; the w component of the sun direction vec4 could encode this value as a tunable parameter.

**Alternative terminator functions** found in external research:

- **Multiply-and-clamp:** `clamp(NdotL * 10.0, -1.0, 1.0) * 0.5 + 0.5` -- sharper, good for stylized look.
- **Logistic sigmoid:** `1.0 / (1.0 + exp(-20.0 * NdotL))` -- smoother gradient, but `exp` may not be available in all WGSL versions. `smoothstep` is the safest WGSL choice.
- **Diffuse modulation:** Multiplying the day texture by `max(0.0, NdotL)` adds shading across the dayside, simulating lower sun angles near the terminator.

**Recommendation:** Start with `smoothstep(-0.1, 0.1, NdotL)` for a clearly visible twilight band, with optional diffuse modulation as a follow-up enhancement. The terminator width should be a uniform parameter to allow tuning without shader recompilation.

**Confidence:** High. The `smoothstep` + `mix` pattern is the standard approach used across WebGL, Three.js, and GLSL globe renderers, and all required WGSL built-ins (`smoothstep`, `mix`, `dot`, `normalize`) are confirmed available.

### Uniform Buffer and Bind Group Layout

The current uniform buffer is exactly 64 bytes (one `mat4x4<f32>` for MVP). It must be expanded to include the sun direction.

**Proposed uniform struct (Rust side):**

```rust
#[repr(C)]
struct Uniforms {
    mvp: glam::Mat4,      // 64 bytes, offset 0
    sun_dir: glam::Vec4,  // 16 bytes, offset 64
}
// Total: 80 bytes
```

Using `Vec4` instead of `Vec3` avoids std140 padding complications. The w component is available for the terminator width parameter or future use.

The bind group layout must be extended:

- **Binding 0** (uniform buffer): visibility must change from `VERTEX` only to `VERTEX | FRAGMENT`, since the fragment shader needs the sun direction.
- **Binding 1** (day texture): unchanged.
- **Binding 2** (sampler): unchanged.
- **Binding 3** (night texture): new `texture_2d<f32>` binding, `FRAGMENT` visibility.

A second sampler binding is not needed; the existing sampler can be shared.

**Design consideration for future layers:** When city lights, clouds, and atmosphere are added, each will need its own texture binding. The bind group could grow to 6-8 bindings (uniform, day, night, city lights, clouds, atmosphere, sampler). Alternatively, the design could use multiple bind groups: group 0 for uniforms, group 1 for textures. This is a forward-looking decision that does not need to be finalized now but should be kept in mind. The single-bind-group approach is simpler for the initial implementation.

### Texture Management and Loading

The current `TextureSlot` system loads textures lazily on demand -- only the texture selected in the combobox is loaded. Each slot has its own bind group containing that slot's texture view.

For day/night blending, **both textures must be GPU-resident simultaneously**. This requires changes:

1. **Eager loading:** When blend mode is activated, both day (slot 1) and night (slot 2) textures must be triggered for loading, not just the one selected in the combobox.

2. **Composite bind group:** A new bind group must be created that contains both the day and night texture views, the expanded uniform buffer, and the shared sampler. This bind group is used only in blend mode. The existing per-slot bind groups remain for single-texture preview modes.

3. **Loading coordination:** The composite bind group can only be created once both textures have finished loading. The renderer must handle the intermediate state where one texture is loaded but the other is not (e.g., show a single-texture fallback or a loading indicator).

4. **`resolve_render_index()` modification:** The current logic picks a single slot index. For blend mode, a different code path is needed that uses the composite bind group instead of a per-slot bind group.

**Confidence:** High for the architectural approach. The existing lazy loading mechanism (`mpsc` channel, background threads, `process_decoded_textures`) does not need fundamental changes -- it just needs to be triggered for both textures simultaneously.

### Dirty-Checking and Time-Driven Updates

The current `FrameState` struct captures all render-affecting inputs and skips the render pass when nothing has changed. This is efficient but means the scene only updates on user interaction.

For day/night rendering, two changes are needed:

1. **Sun direction in FrameState:** The sun direction (or a quantized timestamp) must be added to `FrameState` so that changes in sun position trigger re-renders. Comparing `Vec3` values with a small epsilon or comparing quantized timestamps (e.g., rounded to the nearest minute) avoids re-rendering for imperceptible changes.

2. **Periodic redraw timer:** A timer must request redraws at a configurable interval (e.g., every 1-5 minutes). Slint provides timer APIs for this. The timer does not need to be fast -- the sun moves about 0.25 degrees per minute, so even a 5-minute interval produces smooth enough updates for a wallpaper application.

**Confidence:** High. This is a straightforward extension of the existing dirty-checking pattern.

### UI Changes

The texture combobox currently has three options: Grid, Day, Night. A fourth option should be added: "Day/Night Blend" (or "Realistic" or similar). This option activates the two-texture blending pipeline.

For debugging and demonstration, a manual sun-position override would be valuable: a pair of sliders (subsolar latitude and longitude) that, when enabled, override the computed sun position. This allows testing the terminator at arbitrary positions without waiting for real time to change.

## External Research

All external findings are sourced from the codebase and web research documents. Key external sources with confidence assessments:

| Finding | Sources | Confidence |
| --- | --- | --- |
| Declination formula (single-term sine) | PVEducation, PVPMC, NOAA | High -- multiple independent sources agree on constants |
| Equation of time (compact form) | PVEducation, NOAA | High -- error ~0.5 min, shifts terminator ~0.1 degrees |
| Subsolar point formula | HandWiki, Grokipedia | High -- matches physical interpretation |
| `smoothstep` blending pattern | Geeks3D, WebGL Fundamentals, Sangi Lee | High -- multiple working implementations |
| WGSL built-in availability | WebGPU Fundamentals | High -- `smoothstep`, `mix`, `dot` confirmed |
| Smoothstep edge values (-0.05 to 0.05) | Geeks3D, various | Medium -- empirical, varies by preference |
| Longitude sign convention in simplified formula | HandWiki | Medium -- should verify against reference dates |

## Technical Constraints

1. **Astronomy Engine FFI:** The `astronomy-engine-bindings` crate uses bindgen and requires clang at build time. All calls to the C library are `unsafe` and need safe Rust wrappers. The project already has `unsafe_code = "deny"`, so a dedicated module with targeted `#[allow(unsafe_code)]` would be needed for the FFI boundary.

2. **Uniform buffer alignment:** WGSL std140 layout requires vec3 to be padded to 16 bytes. Using vec4 for the sun direction avoids alignment pitfalls.

3. **Bind group recreation:** Adding a texture binding changes the bind group layout, which requires recreating the render pipeline. This is a one-time cost at setup, not a per-frame cost.

4. **Dual texture memory:** Having both day and night textures GPU-resident doubles texture memory usage. At 8192x4096 RGBA8 (the likely resolution for high-quality textures), each texture is ~128 MB uncompressed on the GPU, so both together would be ~256 MB. This interacts with the existing concern about MSAA memory usage at 4K (documented in `docs/notes.md`). Mipmapped textures reduce the effective cost by ~33% (mip chain overhead), but the base allocation is still substantial.

5. **MSAA pipeline independence:** The MSAA system (dynamic sample count, pipeline rebuild on change) is orthogonal to the texture blending feature and should not need modification.

6. **Render-to-texture isolation:** The offscreen rendering approach means all changes are contained within the wgpu render pass. The Slint integration (`Image::try_from(Texture)`) is unaffected.

## Open Questions

1. **Astronomy Engine integration scope:** Should the FFI integration be done as part of this feature, or should the simplified formulas be used initially with Astronomy Engine deferred to a separate task? The recommendation is to integrate Astronomy Engine now, but the simplified formulas provide a viable alternative if the FFI work is complex.

2. **Terminator width tuning:** The smoothstep edge values (-0.1 to 0.1 vs. -0.05 to 0.05) affect visual appearance. Should this be a user-facing setting, a shader uniform, or a compile-time constant? Recommendation: make it a uniform parameter (using the w component of the sun direction vec4) so it can be tuned without recompilation, but do not expose it in the UI initially.

3. **Diffuse shading on the dayside:** Should the day texture be modulated by `max(0.0, NdotL)` to simulate shading, or left at full brightness? Modulation looks more physical but darkens the dayside significantly near the terminator. This is an aesthetic choice best decided visually during implementation.

4. **Composite bind group lifecycle:** When should the composite bind group (containing both day and night texture views) be created? Options: (a) eagerly when both textures finish loading, (b) lazily when blend mode is first selected. Option (a) is simpler and avoids a visible delay when switching to blend mode.

5. **Timer integration with Slint:** What Slint API should be used for periodic redraws? `slint::Timer` with a `TimerMode::Repeated` is the natural choice, but the interaction with the existing `request_redraw` pattern should be verified.

6. **Longitude sign verification:** The simplified formula's subsolar longitude convention (east-positive vs. west-positive) needs verification against a known reference. For example: at UTC noon on the March equinox, the subsolar point should be approximately 0 degrees N, 0 degrees E. This verification is unnecessary if Astronomy Engine is used, as it handles coordinate conventions internally.

## Recommendations

1. **Use Astronomy Engine for sun position.** This aligns with the project's technology decision documented in `docs/project.md` and `docs/tech.md`. The simplified formulas are well-researched and could serve as a temporary implementation, but Astronomy Engine avoids throwaway code and establishes the FFI pattern needed for all future astronomy features (moon, planets, eclipses).

2. **Expand the uniform buffer to 80 bytes** with a `Vec4` sun direction field. Use the w component for terminator width. Design the struct with awareness that future uniforms (e.g., camera position for atmosphere calculations) will be added later.

3. **Use `smoothstep(-0.1, 0.1, NdotL)` for the initial terminator blend.** This is the most portable WGSL approach (no `exp` dependency), produces a visually pleasing twilight band, and matches the standard pattern used across globe renderers. Parameterize the edge values via the uniform buffer for later tuning.

4. **Extend the bind group layout with a night texture binding (binding 3).** Reuse the existing sampler. Change the uniform buffer visibility to `VERTEX | FRAGMENT`. Keep the single-bind-group approach for now; refactor to multiple bind groups only if the binding count becomes unwieldy when clouds and atmosphere are added.

5. **Add a "Day/Night Blend" option to the texture combobox.** Keep the existing Grid, Day, and Night options for debugging and preview. When blend mode is active, eagerly load both day and night textures.

6. **Create a composite bind group** that references both texture views. Build it as soon as both textures finish loading. Fall back to single-texture rendering if only one is available.

7. **Add sun direction to FrameState** and introduce a periodic redraw timer (e.g., every 2 minutes). Quantize the sun direction comparison to avoid re-rendering for sub-pixel changes.

8. **Add a debug sun-position override** (pair of sliders for subsolar lat/lon) to aid development and testing. This can be hidden behind a debug flag or placed in an expandable "Advanced" section of the UI.

## Sources

| Document | Focus Area |
| --- | --- |
| `docs/plans/2026-03-15-day-night-codebase.md` | Current renderer architecture, shader, bind groups, texture management, dirty-checking, and required changes |
| `docs/plans/2026-03-15-day-night-external.md` | Solar position algorithms, terminator geometry, shader blending patterns, WGSL translation, Rust crate survey |
| `docs/project.md` | Project vision, technology stack (Astronomy Engine decision), rendering layer architecture |
| `docs/tech.md` | Astronomy library comparison, Astronomy Engine selection rationale, FFI considerations |
