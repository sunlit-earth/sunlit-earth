# Research: Airglow (2026-03-22)

## Problem Statement

Sunlit Earth renders a 3D globe as a desktop wallpaper but currently has no atmospheric effects. The Earth's sphere renders as a hard-edged disk against a dark background. Real ISS photographs show a distinctive luminous band along the planet's limb, especially on the night side -- this is airglow, a chemiluminescent glow from the upper atmosphere. Adding an airglow effect would significantly improve visual realism.

The roadmap item reads: "Atmosphere glow / Airglow: subtle blue/orange glow at the Earth's limb."

## Requirements

1. **Visible limb glow on the night side** -- a thin, colored band along the Earth's edge, strongest where the atmosphere is viewed tangentially.
2. **Sun-aware** -- the glow should be suppressed on the sunlit hemisphere and most visible on the night-side limb and near the terminator, matching the physical reality that nightglow is invisible against scattered sunlight.
3. **User-controllable** -- at minimum intensity and falloff sliders, with a way to disable the effect entirely (intensity = 0).
4. **Architecturally consistent** -- follows the existing patterns for pipelines, uniforms, UI bindings, dirty checking, and config persistence.
5. **Reasonable implementation scope** -- the effect should be achievable in days, not weeks, given the project's early stage.

## Findings

### Physical Nature of Airglow

Airglow is a faint, continuous luminescence emitted by Earth's upper atmosphere. It is distinct from aurora (which requires energetic solar wind particles). Three mechanisms drive it:

- **Photodissociation and recombination (nightglow).** Daytime solar UV splits O2 molecules. After sunset, atomic oxygen recombines, releasing energy as visible light -- primarily the green OI line at 557.7 nm.
- **Direct photochemical excitation (dayglow).** Solar UV directly excites atmospheric atoms during daytime. Dayglow is ~1000x brighter than nightglow in absolute terms but invisible from the ground because it is overwhelmed by scattered sunlight. From orbit, it is most prominent in UV.
- **Chemiluminescence.** Reactions between OH radicals and ozone produce infrared and red light (OH Meinel bands, 0.6-4.5 um). This is the brightest emission in energy terms but mostly falls outside the visible spectrum.

### Visual Appearance from Space

ISS photographs consistently show airglow as a thin luminous band along the night-side limb with these characteristics:

- **Color:** Predominantly green to yellow-green (OI 557.7 nm), with orange/yellow (sodium D at 589 nm) at similar altitude. A faint reddish upper band (OI 630 nm) sometimes appears.
- **Thickness:** Roughly 1-3% of Earth's visible disk radius. The primary emitting layer sits between 85-105 km altitude, roughly 10-20 km thick.
- **Position:** Above the dark surface limb, below the star field. Confined to the limb where lines of sight graze the emitting altitude.
- **Day-night behavior:** Visible on the night limb only. The dayglow, while intrinsically brighter, is invisible against scattered sunlight.
- **Texture:** Real airglow shows gravity-wave ripples, but a smooth band is an acceptable simplification for a wallpaper.

The dominant visible wavelengths are:

| Emission | Wavelength | Color | Altitude (peak) | Brightness |
| --- | --- | --- | --- | --- |
| OI (green line) | 557.7 nm | Green | 90-100 km | ~250 Rayleighs |
| Na D (sodium) | 589 nm | Yellow-orange | 92 km | 30-100 R |
| OI (red line) | 630 nm | Red | 250-300 km | ~60 R |
| OH Meinel | 600 nm - 4.5 um | Red/IR | 87 km | ~4.5 MR (mostly IR) |

### How Other Projects Render Atmospheric Effects

No surveyed project models chemiluminescent airglow explicitly. All treat the limb glow as an emergent property of atmospheric scattering, producing a blue-tinted result rather than the green/yellow of real nightglow.

**CesiumJS** uses single-scattering (Nishita 1993) on an ellipsoid larger than the globe. It renders a sky atmosphere shell behind the globe for limb glow and a ground atmosphere on the surface for fog. Exposes Rayleigh/Mie coefficients, scale heights, anisotropy, intensity, and hue/saturation/brightness shifts.

**NASA WorldWind** implements Rayleigh + Mie scattering in a sky dome shader with an exposure tone-map. Similar parameter set (Kr, Km, ESun, g). Limb glow emerges from phase function accumulation.

**Google Earth** simulates Rayleigh scattering for a blue limb halo. Implementation is proprietary.

**Unreal Engine 5** uses precomputed lookup tables for multi-scattering atmosphere. Works from ground to space but does not model chemiluminescent airglow.

**Open-source references:** GPU Gems 2 Chapter 16 (Sean O'Neil) provides the canonical real-time scattering tutorial. Bruneton's precomputed atmospheric scattering is the gold standard for quality. JolifantoBambla/webgpu-sky-atmosphere provides a WGSL port of this technique, directly relevant to the wgpu stack.

The key insight is that these projects solve a harder and broader problem (full atmospheric scattering). For Sunlit Earth's narrower goal -- a visible green/yellow airglow band on the night limb -- a simpler approach suffices and may actually look *more* correct than scattering-based methods, which produce blue halos rather than green ones.

### Current Codebase Architecture

**Shader pipeline.** Two WGSL files are concatenated at compile time: `blend.wgsl` (blending functions) then `sphere.wgsl` (vertex/fragment shaders, uniforms). The vertex shader transforms a 64x64 UV sphere by the MVP matrix and passes the position as the world-space normal. The fragment shader samples day/night textures, blends them based on sun direction, and applies water effects.

**No existing atmospheric effect.** The sphere renders as a hard-edged disk against a dark clear color (0.02, 0.02, 0.05). The only limb-adjacent effect is Fresnel reflectance on ocean pixels, which does not extend beyond the sphere silhouette.

**Render pass structure.** A single render pass with two draw calls: (1) Earth sphere, opaque, depth-write on; (2) cloud sphere at radius 1.0015, alpha blending, depth-write off. Both use the same vertex/index buffers and bind group layout.

**Uniform buffer.** 160 bytes, `#[repr(C)]` with `bytemuck::Pod`, compile-time size assertion. Fields must stay in sync between Rust (`src/renderer/uniforms.rs`) and WGSL (`shaders/sphere.wgsl`). New `f32` scalars can be appended in groups of 4 (16 bytes) for alignment. A `vec3<f32>` color field requires 16 bytes (12 + pad).

**Data flow for UI parameters.** Slint slider -> `on_sliders_changed` callback -> `request_redraw()` -> `BeforeRendering` notifier reads properties -> `build_frame_state()` for dirty checking (floats quantized to integer thousandths) -> `write_uniforms()` -> GPU buffer.

### Rendering Approach Options

Five approaches were evaluated, ranging from trivial to production-grade:

| # | Approach | Effort | Quality | Sun-aware | Beyond silhouette |
| --- | --- | --- | --- | --- | --- |
| 1 | Fresnel rim (in `fs_main`) | Hours | Low | No | No |
| 2 | Sun-modulated rim (in `fs_main`) | Hours | Medium | Yes | No |
| 3 | Separate atmosphere shell | 1-2 days | Medium-High | Yes | Yes |
| 4 | Ray-marching scattering | 3-5 days | High | Yes | Yes |
| 5 | Precomputed LUT scattering | 1-2 weeks | Very High | Yes | Yes |

**Approach 1** adds `pow(1.0 - NdotV, power) * color` in the fragment shader. Trivial but not sun-aware and cannot extend beyond the sphere edge.

**Approach 2** extends approach 1 by multiplying the rim term with a `night_factor` derived from `NdotL`. This gives correct day/night behavior but still cannot produce glow *beyond* the sphere silhouette.

**Approach 3** renders a slightly larger concentric sphere (radius ~1.015-1.025) as a third draw call with alpha or additive blending. The cloud layer already demonstrates this exact pattern. The atmosphere shell's fragment shader computes rim glow from the view-normal dot product and modulates by sun direction. This is the standard technique in CesiumJS, WorldWind, and Unity tutorials.

**Approach 4** ray-marches through the atmosphere shell, accumulating Rayleigh/Mie scattering. Produces physically plausible results including blue day-side limb and accurate terminator gradients. Affordable at the app's low redraw rate (2-minute timer) but significantly more code.

**Approach 5** precomputes scattering into LUT textures. O(1) runtime cost per pixel but requires a multi-pass preprocessing step and 2-4 extra textures. Used by Unreal Engine 5 and the Bruneton reference. Overkill for the current scope.

## Technical Constraints

- **Uniform buffer alignment.** Total size must be a multiple of 16 bytes (std140). Any `vec3<f32>` requires a padding `f32` after it. Adding 4 scalar `f32` fields grows the struct from 160 to 176 bytes; adding a `vec3` color field pushes to 192 bytes.
- **Bind group layout.** The existing layout requires texture bindings. The atmosphere shader only needs uniforms (no textures), but the simplest integration reuses the existing bind group with dummy textures rather than creating a separate layout.
- **Draw order.** The atmosphere sphere must render after the Earth (so it overlays the limb) but before the clouds (so clouds occlude the glow where opaque). Order: Earth -> Atmosphere -> Clouds.
- **Depth writes.** Like the cloud pipeline, the atmosphere pipeline must disable depth writes but keep depth testing enabled, so it does not interfere with the cloud layer's depth ordering.
- **`build_frame_state` signature.** Already takes 21+ parameters with `#[allow(clippy::too_many_arguments)]`. Adding more parameters extends this further. A future refactor into a parameter struct may be warranted but is not blocking.
- **`unsafe_code = "deny"`** at crate level. The atmosphere feature is pure Rust + WGSL and does not require any unsafe code.

## Open Questions

1. **Color as uniform or hard-coded?** Exposing airglow color as three `f32` uniforms (R, G, B) allows creative control but costs 16 bytes of uniform space (with padding). Hard-coding a green-yellow color in the shader saves uniform slots and simplifies the UI. A middle ground: hard-code the default color but expose a single "hue shift" slider.

2. **Additive vs. alpha blending?** Additive blending (`src:One + dst:One`) makes the glow naturally brighten whatever is behind it, which is physically correct for an emissive phenomenon. Alpha blending is what the cloud layer uses. Additive is more appropriate for airglow but means the blend state differs from the cloud pipeline pattern. Both approaches are straightforward in wgpu.

3. **Day-side blue Rayleigh limb?** Real Earth shows a blue atmospheric haze on the sunlit limb from Rayleigh scattering, separate from the green nightglow. The current scope covers nightglow only. Adding a blue day-side limb would be a natural follow-up but involves different physics (scattering, not chemiluminescence) and could be done as a second color/intensity channel on the same atmosphere shell.

4. **Atmosphere sphere vertex count.** The cloud layer reuses the 64x64 UV sphere. The atmosphere shell is a smooth gradient with no texture detail, so a much coarser mesh (32x16 or even 16x8) would suffice. However, reusing the existing vertex/index buffers is simpler and the vertex cost is negligible.

5. **Interaction with wallpaper export.** The `export_wallpaper_image()` path creates temporary render textures. The atmosphere pipeline and draw call must be included in the export path so that wallpapers include the glow. This should happen automatically if the atmosphere draw is added to `encode_and_submit()` / `execute_render_pass()`.

6. **Performance at high MSAA.** Adding a third draw call increases fill rate. At MSAA 8x on 4K (already noted as consuming 600+ MB in the roadmap), the atmosphere sphere adds another full-screen alpha pass. This is unlikely to be a problem at the app's low redraw rate but should be monitored.

## Recommendations

### Recommended approach: Separate atmosphere shell (Approach 3)

Approach 3 (separate atmosphere shell draw call) is recommended for the following reasons:

- **Proven pattern.** The cloud layer already demonstrates a concentric overlay sphere with alpha blending, a separate pipeline, and a dedicated vertex shader entry point. The atmosphere shell follows the same pattern exactly.
- **Beyond-silhouette glow.** Unlike approaches 1-2, a slightly larger sphere produces a visible glow halo that extends past the Earth's edge, which is how real airglow appears.
- **Clean separation.** The atmosphere shader is independent of the surface shader. No risk of complicating the already-complex `fs_main` logic.
- **Sun modulation.** The atmosphere fragment shader can modulate glow by `NdotL` to suppress on the day side, matching physical reality.
- **Upgrade path.** The atmosphere shell can later be upgraded to ray-marching (Approach 4) or LUT scattering (Approach 5) without changing the draw-call structure or UI bindings -- only the fragment shader internals change.

### Recommended uniform fields

Append to the existing 160-byte struct (growing to 176 bytes):

| Offset | Field | Type | Default | Purpose |
| --- | --- | --- | --- | --- |
| 160 | `airglow_intensity` | `f32` | 0.3 | Overall brightness (0 = off) |
| 164 | `airglow_falloff` | `f32` | 3.0 | Rim power exponent |
| 168 | `airglow_radius` | `f32` | 1.02 | Atmosphere sphere scale |
| 172 | `_pad3` | `f32` | 0.0 | Alignment padding |

Hard-code the color in the shader as approximately `vec3(0.25, 0.65, 0.35)` (green-yellow). This can be promoted to uniforms later if users want color control.

### Recommended shader design

New entry points in `shaders/sphere.wgsl`:

- **`vs_atmo`**: Identical to `vs_cloud` but scales by `uniforms.airglow_radius` instead of `uniforms.cloud_sphere_radius`.
- **`fs_atmo`**: Computes rim glow from `pow(1.0 - clamp(dot(N, V), 0.0, 1.0), uniforms.airglow_falloff)`, multiplies by a night-side factor derived from `dot(N, sun_dir)`, outputs the glow color with intensity as alpha (or as additive RGB).

### Integration checklist

Files requiring changes, in dependency order:

| File | Change |
| --- | --- |
| `src/renderer/uniforms.rs` | Add `airglow_intensity`, `airglow_falloff`, `airglow_radius`, `_pad3`; update size assertion to 176 |
| `shaders/sphere.wgsl` | Add uniform fields, `vs_atmo` entry point, `fs_atmo` entry point |
| `src/renderer/gpu_setup.rs` | Add `create_atmo_pipeline()`, wire into `create_gpu_resources()` and `rebuild_msaa_resources()` |
| `src/renderer/mod.rs` | Add `atmo_pipeline` to `GpuResources`, read UI properties in `BeforeRendering` |
| `src/renderer/render_pass.rs` | Add atmo fields to `ShadingParams`, atmosphere draw call in `encode_and_submit()` between Earth and clouds |
| `src/renderer/frame.rs` | Add `airglow_intensity`, `airglow_falloff`, `airglow_radius` to `FrameState` and `build_frame_state()` |
| `ui/main.slint` | Add "Atmosphere" GroupBox (between Clouds and Date/Time) with Intensity, Falloff, Radius sliders |
| `src/config.rs` | Add airglow fields to `AppConfig` with defaults |
| `src/main.rs` | Wire `apply_config_to_window`, `read_config_from_window`, `on_reset_all` |

### Draw order

```text
1. Earth sphere     — opaque, depth write ON
2. Atmosphere shell — additive blend, depth write OFF, depth test ON
3. Cloud sphere     — alpha blend, depth write OFF, depth test ON
```

## Sources

| Document | Focus Area |
| --- | --- |
| `docs/plans/2026-03-22-airglow-codebase.md` | Codebase architecture, shader pipeline, uniform layout, UI data flow, integration points, render pass structure |
| `docs/plans/2026-03-22-airglow-external.md` | Airglow physics, ISS visual reference, survey of rendering techniques in CesiumJS / WorldWind / game engines / open-source shaders, parameter design |
