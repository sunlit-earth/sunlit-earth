# Consolidated Research: Water Shading for Sunlit Earth (2026-03-17)

## Problem Statement

Water on the day side of the 3D Earth globe looks matte, like a painted solid surface rather than actual water. The goal is to add specular sun glint and view-dependent reflectance to ocean pixels so they look realistic, while keeping the implementation lightweight enough for a wallpaper app that renders static frames on demand.

This document synthesizes findings from two research tracks: a codebase analysis (what the current pipeline supports and what needs to change) and an external techniques survey (how other globe renderers solve the same problem). The recommendations at the end form a concrete, actionable plan.

---

## Current Pipeline State

### What the Shader Does Today

The fragment shader (`sphere.wgsl`) samples a day texture and, in blend mode, a night texture. It computes a single `n_dot_l = dot(normalize(world_normal), sun_dir)` value and passes it to `blend_fragment()` (`blend.wgsl`), which applies:

1. A `smoothstep(-w, w, n_dot_l)` day/night blend across the terminator.
2. Optional Lambertian diffuse shading on the day side, controlled by `diffuse_floor` and `diffuse_ramp`.
3. A per-channel clamp `max(day * shading, min(night, day))` to prevent city-light artifacts.

**The shading model is purely diffuse.** There is no specular component. Water and land receive identical treatment.

### What the Shader Has Access To

| Available | Not Available |
|---|---|
| `world_normal` (= vertex position on unit sphere) | Camera eye position / view direction |
| `sun_dir` (uniform) | Per-pixel material properties (water mask, roughness) |
| Day texture RGB (binding 1) | Any specular parameters |
| Night texture RGB (binding 3) | |
| Diffuse shading parameters (uniforms) | |

The **day texture alpha channel is always 255** and is never read or used for transparency. The render pipeline uses `Rgba8Unorm` with no alpha blending, so repurposing the alpha channel is safe.

### Uniform Buffer Layout

The current buffer is 96 bytes (6 x 16-byte rows). The Rust `Uniforms` struct and the WGSL `Uniforms` struct are kept in sync with a compile-time size assertion. Adding fields requires updating both sides, plus the dirty-check `FrameState`, plus the `write_uniforms()` function, plus the test harnesses in `tests/shading.rs` and `tests/render_pipeline.rs`.

### Pre-Existing Ocean Masking Pipeline

The `tools/texture-pipeline/` tooling rasterizes the Natural Earth 10m ocean shapefile into a pixel mask and replaces ocean pixels with a uniform fill color (~RGB 10, 40, 80), preserving polar ice. The current day texture has already been processed through this pipeline, meaning the pipeline knows exactly which pixels are ocean. This knowledge can be exported as a water mask with no additional data source.

---

## Techniques: What Works for Globe-Scale Water

### Blinn-Phong Specular (Sun Glint)

The consensus across real-time globe renderers (Cesium, WebGL Earth, Three.js Earth tutorials, UPenn CIS 565 WebGL Globe) is Blinn-Phong specular restricted to water pixels via a mask. The core pattern:

```wgsl
let v       = normalize(eye_pos - world_pos);
let h       = normalize(sun_dir + v);
let n_dot_h = max(dot(n, h), 0.0);
let spec    = pow(n_dot_h, shininess) * spec_intensity;
```

The result is **added** (not multiplied) to the diffuse color. Blinn-Phong is preferred over Phong because it is cheaper (no reflection vector) and more physically accurate at grazing angles.

**Shininess parameter**: Water is nearly mirror-like. The WebGPU Ocean Simulation (Barth Cave, 2024) uses a specular exponent of 720. The GameDev community recommends 128-256 for ocean. A practical range for this project is 50-500, with a default around 150. This produces a tight, bright sun glint at full-globe view that softens as the user zooms in.

**Intensity parameter**: A separate `spec_intensity` scalar (0.0-1.0, default ~0.4) controls highlight brightness. The highlight is additive, so values near 1.0 will saturate to white in the Rgba8Unorm render target. This matches how real cameras saturate on sun glint.

### Schlick Fresnel

The Fresnel effect makes water more reflective at grazing angles (near the Earth's limb) and less reflective when viewed head-on. This is the single most important visual cue that distinguishes water from a painted surface.

The industry-standard real-time approximation is Schlick's formula:

```
F(theta) = F0 + (1 - F0) * (1 - cos(theta))^5
```

For water, F0 = 0.02 (2% reflectance at normal incidence). At grazing incidence, reflectance approaches 100%. In WGSL:

```wgsl
fn schlick_fresnel(cos_theta: f32, f0: f32) -> f32 {
    let inv = 1.0 - clamp(cos_theta, 0.0, 1.0);
    return f0 + (1.0 - f0) * inv * inv * inv * inv * inv;
}
```

The Fresnel factor modulates the specular intensity, replacing a constant `spec_intensity` with `fresnel * spec_intensity`. This is confirmed by the WebGPU Ocean Simulation article, the Godot Shaders documentation, the lettier.github.io 3D Game Shaders tutorial, and the Scratchapixel Fresnel article.

At full-globe view, the Fresnel effect is subtle because most ocean pixels are viewed from moderate angles. At high zoom it becomes prominent, making water near the viewport edges look silvery.

### Gating by the Terminator

The specular term must fade to zero on the night side. Two gates accomplish this:

1. **`n_dot_l` gate**: Multiplying the specular by `max(dot(n, sun_dir), 0.0)` ensures no glint where the sun is below the horizon.
2. **Blend factor gate**: Multiplying by the same `smoothstep(-w, w, n_dot_l)` used for day/night blending ensures the glint fades smoothly through the terminator zone, matching the diffuse shading behavior.

### Procedural Normal Perturbation (Optional)

At globe scale, individual ocean waves are invisible, but their aggregate effect scatters the specular highlight into a softer, wider pattern. This can be approximated by perturbing the surface normal with hash-based procedural noise before computing the specular term. This is cheap (~10 ALU ops, no texture sample) and produces a subtly uneven highlight that breaks up the "perfect mirror" appearance.

For a static wallpaper renderer, the perturbation would be fixed (not animated). This is acceptable and consistent with the project's rendering model. Animation would require a `time` uniform and is future work.

### Techniques Not Warranted

**Wave geometry**: At the project's minimum orbital distance (1.5 Earth radii), a single pixel covers many km of surface. Wave geometry (0-30 meters) is tens of orders of magnitude below rendering resolution.

**Screen-space reflections / environment maps**: These are designed for close-up water rendering and add substantial complexity. For a globe wallpaper app, Blinn-Phong + Fresnel achieves the target visual quality.

**PBR (GGX/Cook-Torrance)**: More physically accurate than Blinn-Phong but significantly more complex. The visual difference at globe scale is minimal. Blinn-Phong is sufficient.

---

## Water Mask: How to Distinguish Water from Land

The shader needs a per-pixel water mask to restrict specular highlights to ocean areas. Four approaches were evaluated:

### Recommended: Alpha Channel of Day Texture

Encode the water mask in the alpha channel of the pre-processed day texture (0 = land, 255 = water, with anti-aliased intermediate values at coastlines).

**Advantages:**
- Zero additional memory bandwidth (the day texture is already sampled; reading `.a` is free)
- No new texture binding or bind group layout change
- The ocean masking pipeline already identifies water pixels; writing the mask into the alpha channel is a small addition
- Anti-aliased coastline transitions are straightforward (intermediate alpha values)

**Implementation:** In the shader, after sampling the day texture:
```wgsl
let day_sample = textureSample(sphere_texture, sphere_sampler, in.uv);
let day_color  = day_sample.rgb;
let water_mask = day_sample.a;   // 0.0 = land, 1.0 = water
```

**Risk assessment:** The alpha channel is currently always 255. The render pipeline uses `Rgba8Unorm` with no alpha blending, so alpha is never used for transparency. A search of the codebase confirms no code path reads or depends on alpha = 1.0. The risk is low.

### Alternative: Separate Binary Texture

A grayscale water mask texture bound at a new binding (binding 4). This is the most robust approach and the industry standard (Cesium, Three.js Earth, WebGL Earth, planetpixelemporium.com specular maps).

**Advantages:** Cleanly separated from the color texture. Can be at a different resolution (even 1K is sufficient for a binary classifier). Can be updated independently.

**Disadvantages:** Requires a new `BindGroupLayoutEntry`, a new `@binding(4)` in the shader, updates to all bind group creation call sites (grid, single-texture, composite), and an additional texture sample per fragment (~50% increase in texture bandwidth).

### Not Recommended: Color-Based Detection

Detecting water by comparing sampled day color against the known ocean fill color (~RGB 10, 40, 80). This is fragile due to trilinear filtering at coastlines, potential fill color changes, and ice/deep-ocean color similarity. Useful only as a temporary fallback during development.

### Future Option: Channel-Packed Material Texture

A single auxiliary texture with R = water mask, G = ice mask, B = roughness variation. This provides rich per-pixel material data at the cost of one additional texture sample. Worth considering if the project later needs ice masking or variable roughness, but overkill for the initial water shading feature.

---

## Required Pipeline Changes

### 1. Add Eye Position Uniform

**What:** Add `eye_pos: vec3<f32>` + `_pad2: f32` (16 bytes) to the `Uniforms` struct, bringing it from 96 to 112 bytes.

**Where to change:**
- `src/renderer/uniforms.rs` — add field, update size assertion to 112
- `shaders/sphere.wgsl` — add field to WGSL `Uniforms` struct
- `src/renderer/render_pass.rs` — populate `eye_pos` from `OrbitalCamera::eye_position()` in `write_uniforms()`
- `src/renderer/frame.rs` — no change needed (eye position is derived from longitude/latitude/zoom, which are already tracked)
- `tests/shading.rs` — update compute shader uniform struct
- `tests/render_pipeline.rs` — update uniform buffer size and contents

**Source of the value:** `OrbitalCamera::eye_position()` in `src/scene/camera.rs` already computes the camera's world-space position from longitude, latitude, and distance. It is currently used only to build the view matrix. The same value can be passed through as a uniform.

### 2. Add Specular Parameters to Uniforms

**What:** Add `spec_shininess: f32` and `spec_intensity: f32` (8 bytes) to the `Uniforms` struct. These can replace the existing `_pad` field (4 bytes) and use one more float, or be appended after the eye position block. The total must remain a multiple of 16.

With the eye position (16 bytes) and two specular floats (8 bytes), the uniform buffer grows from 96 to 96 + 16 + 8 = 120 bytes. Adding 8 bytes of padding brings it to 128 bytes (8 x 16), a clean multiple.

**Where to change:** Same files as the eye position, plus:
- `src/renderer/render_pass.rs` — add `ShadingParams` fields for shininess and intensity
- `src/renderer/frame.rs` — add quantized specular fields to `FrameState` and `build_frame_state()`
- `src/main.rs` — read slider values and pass through
- `ui/main.slint` — add two sliders to the Lighting group

### 3. Encode Water Mask in Day Texture Alpha

**What:** Modify the ocean masking pipeline (`tools/texture-pipeline/`) to write 255 in the alpha channel for water pixels and 0 for land pixels when producing the processed day texture. Anti-alias coastlines with intermediate values.

**Where to change:**
- `tools/texture-pipeline/` — add alpha channel writing step after the ocean fill step
- Re-run the pipeline to produce an updated `world.topo.200405.jxl`

**No shader binding changes are needed.** The day texture is already `Rgba8Unorm` and its alpha channel is already sampled (but currently discarded). Reading `.a` in the shader is free.

### 4. Add Specular Calculation to Fragment Shader

**What:** After the existing `blend_fragment()` call, compute a Blinn-Phong specular term with Schlick Fresnel, gated by the water mask and the day/night blend factor. Add the result to the blended color.

**Where to change:**
- `shaders/sphere.wgsl` — add specular logic in `fs_main()` after the `blend_fragment()` call
- Optionally extract specular as a pure function in `shaders/blend.wgsl` for testability via the compute shader harness

### 5. Add UI Sliders

**What:** Two new sliders in the Lighting `GroupBox`:
- "Ocean Shininess" — range 10 to 500, default 150
- "Ocean Glint" — range 0.0 to 1.0, default 0.4

**Where to change:**
- `ui/main.slint` — add sliders with bidirectional `<=>` bindings
- `src/main.rs` — read values from UI and pass to `ShadingParams`

---

## Proposed Shader Logic

The following integrates into the existing `fs_main()` in `sphere.wgsl`. It is placed after the `blend_fragment()` call and before the final `return`:

```wgsl
// --- Water specular (sun glint) ---
let water      = textureSample(sphere_texture, sphere_sampler, in.uv).a;
let v          = normalize(uniforms.eye_pos - in.world_normal);
let h          = normalize(uniforms.sun_dir + v);
let n_dot_h    = max(dot(n, h), 0.0);
let spec       = pow(n_dot_h, uniforms.spec_shininess);

// Schlick Fresnel: F0 = 0.02 for water
let cos_v      = max(dot(n, v), 0.0);
let inv        = 1.0 - cos_v;
let fresnel    = 0.02 + 0.98 * inv * inv * inv * inv * inv;

// Gate: no glint on night side, fade through terminator
let glint      = spec * fresnel * max(n_dot_l, 0.0) * blend * uniforms.spec_intensity * water;

let color      = color + vec3<f32>(glint);
```

Notes on this logic:
- `in.world_normal` doubles as the world position because the mesh is a unit sphere centered at the origin.
- The `water` value comes from the alpha channel of the day texture, which is already sampled. In practice, the existing `textureSample` call would be changed from `.rgb` to a full `vec4` sample, with `.rgb` used for day color and `.a` for the water mask.
- `n` and `n_dot_l` are already computed before the `blend_fragment()` call.
- `blend` is the same `smoothstep(-w, w, n_dot_l)` value used for day/night blending. It would need to be computed before the `blend_fragment()` call and passed separately, or the `blend_fragment()` function signature would need to return it alongside the color.
- The Fresnel pow5 is expanded as five multiplications to avoid the cost of a general `pow()` call.
- In single-texture mode (`terminator_width < 0`), the early return before this code block means specular is automatically skipped.

---

## Performance Assessment

The rendering model is event-driven: a frame renders once when parameters change, at most once every 2 minutes for sun updates. Per-frame GPU cost is not a concern. The only constraint is that a single frame completes within ~16ms to avoid visible stutter.

| Addition | Cost per Fragment | Impact |
|---|---|---|
| Blinn-Phong specular | ~5 ALU ops | Negligible |
| Schlick Fresnel (pow5 expanded) | ~6 ALU ops | Negligible |
| Water mask from alpha channel | 0 extra samples | Free |
| Normal perturbation (optional) | ~10 ALU ops | Negligible |
| New uniform data (eye_pos + 2 floats) | 0 (uniform read) | Negligible |

Total additional cost: ~11-21 ALU operations per fragment, zero additional texture samples. At 4K resolution (8.3M pixels), this is well within the budget of any GPU from the last decade.

---

## Implementation Order

The changes are ordered to produce testable intermediate states:

1. **Eye position uniform** — Add `eye_pos` to `Uniforms` on both Rust and WGSL sides. Populate from `OrbitalCamera::eye_position()`. Update test harnesses. This is a prerequisite for all specular work and has no visual effect on its own.

2. **Water mask in alpha channel** — Modify the texture pipeline to write the ocean mask into the alpha channel. Re-export the day texture. Verify the mask is correct by temporarily visualizing it (e.g., `return vec4(water, water, water, 1.0)` in the shader).

3. **Specular parameters and UI sliders** — Add `spec_shininess` and `spec_intensity` to uniforms. Add UI sliders. Wire them through `ShadingParams`, `write_uniforms()`, and `FrameState`.

4. **Blinn-Phong + Fresnel in shader** — Add the specular calculation to `fs_main()`. This is where the visual payoff happens.

5. **Tuning** — Adjust default shininess and intensity values based on visual evaluation at multiple zoom levels and sun angles.

6. **Optional: procedural normal perturbation** — Add hash-based normal perturbation for subtle glint breakup. Evaluate whether the visual improvement at globe scale justifies the code complexity.

---

## Confidence Assessment

| Finding | Confidence | Basis |
|---|---|---|
| Blinn-Phong is the right specular model | High | Confirmed by Cesium, WebGL Earth, Three.js Earth, UPenn CIS 565, multiple tutorials |
| Shininess 100-300 is the right range for ocean | High | WebGPU ocean sim (720), GameDev thread (128-256), physics of ocean |
| Schlick Fresnel with F0=0.02 is correct for water | High | Scratchapixel, Wikipedia, WebGPU ocean sim, 3D Game Shaders tutorial |
| Alpha channel for water mask is viable and safe | High | Codebase audit: alpha is always 255, never read, no blending enabled |
| Eye position can be added without breaking existing tests | High | Uniform extension pattern is documented in codebase analysis |
| Performance impact is negligible | High | ALU cost is ~15 ops/fragment, zero new texture samples |
| Procedural normal perturbation is barely visible at full-globe | Medium | No direct reference for globe-scale evaluation; likely visible only at high zoom |
| PBR (GGX) is not warranted | Medium | Blinn-Phong is sufficient for a wallpaper app; PBR difference is minimal at globe scale |

---

## Sources

### Codebase

| File | Relevance |
|---|---|
| `shaders/blend.wgsl` | Current day/night blend function (insertion point for specular) |
| `shaders/sphere.wgsl` | Current vertex/fragment shaders, uniforms, texture sampling |
| `src/renderer/uniforms.rs` | Uniform buffer layout (must be extended) |
| `src/renderer/render_pass.rs` | `write_uniforms()` and `ShadingParams` (must add eye_pos and specular params) |
| `src/renderer/frame.rs` | `FrameState` dirty-checking (must add specular params) |
| `src/renderer/gpu_setup.rs` | Pipeline and bind group layout (unchanged unless using separate mask texture) |
| `src/renderer/textures.rs` | Texture slot system, bind group creation |
| `src/scene/camera.rs` | `OrbitalCamera::eye_position()` — source of the eye position uniform |
| `tools/texture-pipeline/` | Ocean masking pipeline (must add alpha channel writing) |
| `tests/shading.rs` | Compute shader test harness for `blend_fragment()` (must update uniform struct) |
| `tests/render_pipeline.rs` | Full pipeline GPU tests (must update uniform struct) |
| `ui/main.slint` | UI layout (must add two sliders) |
| `src/main.rs` | Callback wiring (must add slider readout) |

### External

1. [Working with Lights - Learn wgpu](https://sotrh.github.io/learn-wgpu/intermediate/tutorial10-lighting/) -- WGSL Blinn-Phong code
2. [Advanced Lighting (Blinn-Phong) - LearnOpenGL](https://learnopengl.com/Advanced-Lighting/Advanced-Lighting) -- Shininess guidance, half-vector
3. [Lighting Maps - LearnOpenGL](https://learnopengl.com/Lighting/Lighting-maps) -- Specular map sampling
4. [Fresnel Factor - 3D Game Shaders for Beginners](https://lettier.github.io/3d-game-shaders-for-beginners/fresnel-factor.html) -- Schlick approximation
5. [Schlick's Approximation - Wikipedia](https://en.wikipedia.org/wiki/Schlick%27s_approximation) -- F0 values
6. [Ocean Simulation - Barth Cave](https://barthpaleologue.github.io/Blog/posts/ocean-simulation-webgpu/) -- Specular exponent 720, Fresnel formula
7. [Ocean Details - Cesium Wiki](https://github.com/CesiumGS/cesium/wiki/Ocean-Details) -- Production globe renderer ocean handling
8. [WebGL Earth - Buschnick](https://blog.buschnick.net/2014/02/webgl-earth.html) -- Binary specular map on globe
9. [Three.js Earth - Mastermaps](https://blog.mastermaps.com/2013/09/creating-webgl-earth-with-threejs.html) -- MeshPhongMaterial with specular map
10. [WebGL Globe - UPenn CIS 565](https://github.com/rarietta/WebGL) -- Luminance-threshold specular masking
11. [Planet Earth Textures - planetpixelemporium](https://planetpixelemporium.com/earth.html) -- Dedicated specular map
12. [Simple Water - Inigo Quilez](https://iquilezles.org/articles/simplewater/) -- FBM noise for normal perturbation
13. [Very Fast Procedural Ocean - Shadertoy](https://www.shadertoy.com/view/MdXyzX) -- Hash-based wave generation
14. [Introduction to Shading: Fresnel - Scratchapixel](https://www.scratchapixel.com/lessons/3d-basic-rendering/introduction-to-shading/reflection-refraction-fresnel.html) -- Full Fresnel equations, F0 for water ~0.02
15. [Natural Earth III Extra Data - shadedrelief.com](https://www.shadedrelief.com/natural3/pages/extra.html) -- Land/water mask textures
16. [Specularity - Learn WebGPU for C++](https://eliemichel.github.io/LearnWebGPU/basic-3d-rendering/lighting-and-material/specular.html) -- WGSL specular patterns
17. [Specular mask in alpha channel - Polycount](https://polycount.com/discussion/52917/specular-mask-in-alpha-channel) -- Channel packing tradeoffs
18. [Glittering Light on Water - NOAA](https://psl.noaa.gov/outreach/education/science/glitter/) -- Real-world physics of sun glitter
19. [Apply displacement/specular maps - WebGL Fundamentals](https://webglfundamentals.org/webgl/lessons/webgl-qna-apply-a-displacement-map-and-specular-map.html) -- "Add not multiply" for specular
