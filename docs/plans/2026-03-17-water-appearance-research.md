# Consolidated Research: Water Appearance Improvement (2026-03-17)

## Purpose

This document synthesizes the codebase analysis (`2026-03-17-water-appearance-codebase.md`) and external research (`2026-03-17-water-appearance-external.md`) into a unified assessment of why the ocean looks artificial and what to do about it. It is the basis for the implementation plan.

---

## The Problem

The ocean in Sunlit Earth looks like a painted solid surface rather than water. This is visible at every viewing distance and dominates the overall impression of the render. The roadmap acknowledges the problem explicitly: "Water still looks pretty solid, need to investigate what kind of effects can improve this."

The specular sun glint added in PR #9 was a significant first step -- it introduced a Blinn-Phong highlight gated by the water mask in the alpha channel. But the glint covers only a small angular region near the sun reflection point. The vast majority of the ocean surface remains a uniform dark gradient with zero visual complexity.

---

## Root Causes (Ranked by Visual Impact)

### 1. Uniform fill color with zero spatial variation -- the dominant cause

Every ocean pixel in the day texture is set to exactly RGB (10, 40, 80) by the texture pipeline's `apply_ocean_mask()` function. After diffuse shading, this produces a perfectly smooth navy gradient from the subsolar point to the terminator. There is no depth variation, no coastal brightening, no regional color zones -- nothing to break up the uniformity.

Real ocean viewed from space shows dramatic spatial variation: deep saturated blue in the open Pacific, turquoise over shallow tropical reefs, green-grey in nutrient-rich coastal zones, tan-brown at river outflows. This variation spans 3-4x per RGB channel between the brightest shallows and darkest deeps. The fill color eliminates all of it.

Every production globe renderer that looks convincing -- Cesium, Google Earth, Three.js Earth demos, PlanetPixelEmporium-based renderers -- relies on spatially varying ocean color as its foundation. None achieve a convincing ocean from a uniform fill plus specular. The specular and Fresnel effects are enhancements layered on top of a base that already has spatial color information.

**Codebase location**: `tools/texture-pipeline/src/texture_pipeline/ocean_masking.py`, line 62 (default color parameter) and lines 78-79 (uniform assignment to all ocean pixels).

**Why the fill was introduced**: The roadmap noted original NASA Blue Marble ocean pixels were "too dark and sometimes contain satellite imagery artifacts." The uniform fill was a blunt fix. The original motivation is valid, but the cure is worse than the disease -- artifact correction would be better than total color replacement.

### 2. Missing Fresnel effect -- water should brighten at grazing angles

The water shading research for PR #9 recommended Schlick Fresnel (F0 = 0.02 for water), but it was not implemented. The specular in the current shader is `pow(n_dot_h, shininess) * n_dot_l * blend * intensity * water` -- a constant-intensity Blinn-Phong with no viewing-angle dependence.

Without Fresnel, the ocean appears equally matte at all viewing angles. Real water is strongly reflective at grazing angles: at the limb of the globe, the ocean should appear brighter and more silvery, shifting from its intrinsic blue toward reflecting the sky. This "silvery horizon" is one of the most recognizable visual signatures of water in real photos of Earth from space. Its absence makes the ocean look like a painted sphere.

The Fresnel effect operates through two channels:
1. **Specular modulation**: The specular highlight should intensify at grazing angles (replacing constant `spec_intensity` with `fresnel * spec_intensity`).
2. **Diffuse color shift**: At grazing angles, less light from within the water reaches the camera and more sky is reflected. This can be modeled by mixing the intrinsic ocean color toward a sky color based on the Fresnel term.

**Codebase location**: `shaders/sphere.wgsl`, lines 73-87 (specular calculation, no Fresnel term present). The `eye_pos` uniform needed for the view vector is already present (added in PR #9).

**Implementation cost**: Very low -- a 5-line `schlick_fresnel()` function plus minor changes to the specular and diffuse paths. No new uniforms, textures, or pipeline changes required.

### 3. Fill color is too dark

RGB (10, 40, 80) in sRGB is very dark navy. Even at full illumination (`shading = 1.0`), the linear values are approximately (0.039, 0.157, 0.314). Real deep ocean viewed from space is significantly brighter and more saturated, with approximate sRGB values around RGB (15-30, 50-80, 120-160) for the mid-Pacific. The current fill is roughly half the brightness of real open ocean.

This compounds with the diffuse shading model: at the terminator (`diffuse_floor = 0.50`), the fill darkens to approximately (5, 20, 40), which is functionally black. The diffuse floor designed for land (modulating bright, varied satellite imagery) produces an excessively dark result on the already-dark fill.

**Note**: If spatial variation is restored (root cause #1), this issue is subsumed. As a standalone fix it is trivial (change a single constant) but is a stopgap.

### 4. No latitude, depth, or proximity-to-land color variation

Real ocean color varies systematically with geography:
- Continental shelves are visibly lighter and greener than open ocean (the shelf-edge contrast is one of the strongest on Earth)
- Tropical waters are brighter turquoise; polar waters are dark grey-green
- Major river outflows (Amazon, Congo, Yellow River) produce visible tan-brown plumes
- Phytoplankton blooms create green-blue patches in productive zones

At full-globe view (1.5-5 Earth radii), the main ocean color signals visible are: open-ocean vs coastal contrast, tropical shallow areas (Bahamas Bank, Persian Gulf), and polar darkening. These features are present in the original NASA Blue Marble source imagery but were discarded by the uniform fill.

### 5. Specular highlight too tight and localized

With shininess = 150 and no Fresnel broadening, the sun glint is a small bright dot on a vast dark surface. Real ocean sun glint has a characteristic elongated shape that varies with viewing geometry, and is visually softer because Fresnel intensifies it progressively toward grazing angles rather than concentrating it at a single peak.

### 6. Diffuse model darkens the already-dark fill at the terminator

The shared `diffuse_floor = 0.50` appropriate for land textures produces near-black output when applied to the low-value ocean fill. A water-specific diffuse floor would prevent ocean pixels near the terminator from appearing indistinguishable from the night side.

---

## Two Fundamental Approaches to Ocean Color

The research identifies two distinct strategies for providing spatial color variation. They are not mutually exclusive but have different tradeoffs.

### Approach A: Use natural satellite ocean color ("restore what was discarded")

The NASA Blue Marble source texture (`world.topo.200405`) already contains satellite-derived ocean color: depth gradients, coastal brightening, chlorophyll-based regional color, river plume features. The texture pipeline discards this by overwriting all ocean pixels with a uniform fill. Restoring the original ocean pixels is the simplest path to spatial variation.

**What exists in the codebase**: The unmodified original texture is preserved at `textures/world.topo.200405.original.jxl`. The texture pipeline's `apply_ocean_mask()` is the single function that replaces ocean color with the fill.

**How to implement**: Modify the texture pipeline to skip the fill step for ocean pixels, or apply a lighter treatment (brightness/contrast correction) that preserves the satellite spatial variation while addressing the original "too dark" complaint. The alpha channel water mask encoding would be preserved unchanged (it is computed from the shapefile mask, not from the pixel colors).

**Tradeoffs**:
- Pro: Highest fidelity; the spatial variation is real satellite data
- Pro: Zero shader changes needed for the base color (specular/Fresnel improvements are orthogonal)
- Pro: No new textures or GPU pipeline changes
- Con: The original NASA Blue Marble deep ocean is itself a flat blue (NASA's own product used a uniform fill for deep ocean), so variation is strongest in coastal and shallow areas
- Con: Some satellite imagery artifacts in the original (the reason the fill was introduced)
- Con: The "lighter-water.jxl" variant in the main repo suggests prior experimentation with this approach, possibly indicating aesthetic challenges

### Approach B: Synthesize ocean color in the shader ("build it from data")

Instead of relying on the source texture's ocean pixels, compute ocean color procedurally in the shader using additional data (bathymetry depth map, latitude, or distance-to-coast). The PlanetPixelEmporium textures use this approach: GEBCO bathymetric data drives a depth-based color ramp.

**How to implement**: Add a bathymetry (GEBCO/ETOPO) grayscale texture as a new texture slot. In the shader, sample the depth value and interpolate between a shallow color (lighter, blue-green) and a deep color (darker navy). The water mask already in the alpha channel gates which pixels receive the synthetic color.

**Tradeoffs**:
- Pro: Full artistic control over the color ramp; easy to tune
- Pro: Clean, artifact-free; no satellite imagery issues
- Pro: Depth data is high quality and freely available (GEBCO 2025 Grid)
- Con: Requires a new texture slot, new bind group entry, new shader binding, and texture loading path (medium implementation complexity)
- Con: No surface color variation from chlorophyll, sediment, or weather -- only depth gradient
- Con: Can look "synthetic" or "relief map-like" without careful tuning

### Approach C: Hybrid (satellite color + bathymetric enhancement)

Preserve the satellite ocean color from Blue Marble, then use bathymetry data to modulate brightness and add depth detail in regions where the satellite source is flat. This combines the accuracy of satellite data with the detail of bathymetric depth.

This is the medium-effort path that external research identifies as the best quality option, but it requires both the texture pipeline modifications and the new bathymetry texture slot.

### Recommendation

**Approach A (restore satellite ocean color)** is the clear first choice. It provides the highest impact for the lowest effort, requires no shader changes for the base color, and no new GPU infrastructure. The satellite imagery artifacts that motivated the original fill can be addressed with targeted corrections (brightness boost, localized masking) rather than wholesale replacement.

Approach B (bathymetry shader) is valuable as a future enhancement but adds implementation complexity that is not justified until the simpler path has been tried and found insufficient.

---

## Improvement Options (Ranked by Impact vs Effort)

These are ordered by the product of visual impact and implementation ease. Each option is independent -- they can be implemented in any combination.

### Tier 1: High impact, low effort

**1. Restore satellite ocean color in the texture pipeline**

Replace the uniform fill with preserved/corrected NASA Blue Marble ocean pixels. This is the single most impactful change.

- Impact: Very High -- transforms ocean from painted solid to geographically realistic
- Effort: Low -- modify `apply_ocean_mask()` to skip or lighten the fill; re-run the texture pipeline
- Codebase support: Full. The original texture exists (`textures/world.topo.200405.original.jxl`), the texture pipeline is already set up, and the alpha channel water mask encoding is independent of the RGB fill
- Files: `tools/texture-pipeline/src/texture_pipeline/ocean_masking.py`, `tools/texture-pipeline/src/texture_pipeline/main.py`
- New infrastructure: None

**2. Add Schlick Fresnel to the specular path**

Replace the constant specular intensity with a Fresnel-modulated term. This makes the sun glint intensify at grazing angles and creates the characteristic ocean-limb brightening.

- Impact: High -- produces the "water brightens at the horizon" effect visible in real Earth photos
- Effort: Very Low -- 5-line function addition to `sphere.wgsl`, minor change to the specular calculation
- Codebase support: Full. The `eye_pos` uniform and view vector `v` are already computed for the existing Blinn-Phong specular. The `dot(n, v)` needed for Fresnel is trivial to add.
- Files: `shaders/sphere.wgsl` (add `schlick_fresnel()` function, modify specular calculation)
- New infrastructure: None

### Tier 2: Medium impact, low-medium effort

**3. Fresnel-driven diffuse color shift toward limb**

Apply the Fresnel term to the diffuse base color (not just specular). At grazing angles, mix the ocean color toward a sky-blue constant, simulating the real-world phenomenon where ocean near the horizon reflects sky rather than showing its intrinsic color.

```wgsl
let sky_color   = vec3<f32>(0.5, 0.7, 0.9);
let water_color = mix(base_ocean_color, sky_color, fresnel * water);
```

- Impact: Medium -- visible horizon brightening on the ocean, even away from the specular hotspot
- Effort: Low (if Fresnel is already computed for option 2)
- Codebase support: Full. Uses the same `eye_pos`, view vector, and water mask already present.
- Files: `shaders/sphere.wgsl` (add 2-3 lines after the diffuse calculation)
- New infrastructure: None. A `sky_color` uniform could be added later for tunability, but a hardcoded constant is fine for the first implementation.

**4. Brighter base ocean fill (stopgap if option 1 is deferred)**

Change the fill color from RGB (10, 40, 80) to approximately RGB (20, 65, 130). This is a one-line change that immediately makes the ocean look less funereal.

- Impact: Medium -- brighter ocean everywhere, but still uniform
- Effort: Trivial -- change a single constant
- Files: `tools/texture-pipeline/src/texture_pipeline/ocean_masking.py` (line 62)
- Note: Superseded by option 1 if that is implemented. The existence of `textures/world.topo.200405.lighter-water.jxl` in the main repo suggests this kind of experiment has been tried before.

**5. Water-specific diffuse floor**

Raise the minimum diffuse shading factor for ocean pixels to prevent them from going near-black at the terminator. The water mask from the alpha channel selects which pixels use the higher floor.

- Impact: Low-Medium -- prevents the terminator region from looking wrong, but does not improve the majority of ocean
- Effort: Low -- add an `ocean_diffuse_floor` uniform or a hardcoded higher floor in `blend.wgsl` when the water mask is high
- Files: `shaders/blend.wgsl` or `shaders/sphere.wgsl`
- New infrastructure: Possibly one new uniform field; or could be accomplished by passing the water mask into `blend_fragment()`

### Tier 3: Medium impact, medium-high effort

**6. Bathymetry-driven depth color gradient**

Add a GEBCO/ETOPO bathymetry texture as a new texture slot. In the shader, sample depth and interpolate between shallow and deep ocean colors.

- Impact: Medium -- visible continental shelf edges, depth gradients
- Effort: Medium -- new texture slot, new bind group entry, new shader binding, texture loading path, texture pipeline step to prepare the depth texture
- Codebase support: Partial. The texture loading infrastructure exists (`TextureSlot`, `create_mipmapped_texture()`), but adding a third texture slot requires bind group layout changes.
- Files: `src/renderer/textures.rs`, `src/renderer/gpu_setup.rs`, `shaders/sphere.wgsl`, texture pipeline tooling
- Note: Lower priority than option 1 because the NASA Blue Marble source already encodes depth-related color variation (coastal areas are genuinely lighter). Bathymetry adds the most value for deep-ocean regions where the Blue Marble source itself uses a flat fill.

---

## What the Codebase Already Supports vs What Needs New Infrastructure

### Already present (no new infrastructure needed)

| Capability | Location |
|---|---|
| Water mask in alpha channel (ocean=128, land=255) | Day texture alpha, decoded in `sphere.wgsl` lines 74-77 |
| Water mask decode in shader | `let water = saturate((1.0 - day.a) * 2.0)` |
| Eye position uniform for view vector | `uniforms.eye_pos` in `sphere.wgsl`, populated from `OrbitalCamera::eye_position()` |
| View vector computation | `let v = normalize(uniforms.eye_pos - in.world_normal)` in specular block |
| Blinn-Phong half-vector and specular term | `sphere.wgsl` lines 78-84 |
| Specular intensity and shininess uniforms | `uniforms.spec_intensity`, `uniforms.spec_shininess` |
| Specular UI sliders | `ui/main.slint` lines 403-433 |
| Dirty-check for specular params | `frame.rs` lines 27-29 |
| Original unmasked texture | `textures/world.topo.200405.original.jxl` |
| Texture pipeline with ocean masking | `tools/texture-pipeline/` |
| Texture pipeline CLI with `--ocean-color` param | `main.py` line 297 |

### Needs implementation (but no new infrastructure)

| Change | Complexity |
|---|---|
| `schlick_fresnel()` function in WGSL | 5 lines |
| Fresnel modulation of specular intensity | 2-line modification |
| Fresnel diffuse color mixing toward sky | 3 lines |
| Texture pipeline change to preserve satellite ocean color | Modify `apply_ocean_mask()` |
| Re-run texture pipeline to produce new day texture | CLI invocation |

### Needs new infrastructure

| Change | What is needed |
|---|---|
| Bathymetry texture slot | New `TextureSlot` variant, bind group layout change, shader binding, texture loading |
| Per-water diffuse floor uniform | New uniform field (Rust + WGSL), UI slider, dirty-check field |
| Sky color uniform for Fresnel mixing | New uniform field (or hardcode initially) |

---

## Key Findings

1. **The uniform fill is the root cause.** Every other deficiency (no Fresnel, dark fill, no depth variation) is secondary. Restoring spatial variation in the base ocean color is the single highest-impact change.

2. **Fresnel was planned but not implemented.** The water shading research for PR #9 specified Schlick Fresnel with F0 = 0.02, including WGSL code. The implementation delivered Blinn-Phong specular without the Fresnel term. All the infrastructure needed for Fresnel (eye position, view vector) was added in that PR and is ready to use.

3. **No renderer achieves a good ocean from uniform fill + specular.** This is the most consistent finding across all external sources. Cesium, Google Earth, Three.js demos, and PlanetPixelEmporium all start from spatially varying ocean color. The specular highlight is a polish layer, not a foundation.

4. **The original NASA Blue Marble texture has the spatial variation we need.** It is already in the repo (`textures/world.topo.200405.original.jxl`). The texture pipeline discarded it. The path of least resistance is to stop discarding it.

5. **NASA Blue Marble itself uses a flat fill for deep ocean.** The spatial variation in the Blue Marble source is strongest for coastal and shallow areas; deep open ocean was filled by NASA, not satellite-derived. This means restoring the original texture improves coastal areas dramatically but leaves deep ocean relatively flat. Bathymetry enhancement could address this later.

6. **The lighter-water.jxl variant suggests prior experimentation.** An uncommitted `textures/world.topo.200405.lighter-water.jxl` exists in the main repo, indicating the user has already tried adjusting the ocean fill brightness. This supports the finding that the current fill is too dark and the user is already aware of it.

7. **Clouds and atmosphere are not prerequisites for good water.** The NASA Blue Marble is specifically a cloudless composite, and its ocean is recognizable as water. Spatial color variation and Fresnel do the work; clouds add context but are not required.

---

## Confidence Assessment

| Finding | Confidence | Basis |
|---|---|---|
| Uniform fill is the dominant cause of artificial appearance | Very High | Codebase trace + every external renderer comparison |
| Restoring satellite ocean color is highest-impact change | High | Universal pattern across production renderers; no counterexample found |
| Schlick Fresnel F0=0.02 is correct for water | Very High | Multiple physics and rendering sources agree |
| Fresnel was planned but not implemented in PR #9 | Confirmed | Codebase analysis: `sphere.wgsl` has no Fresnel function; water shading research specified it |
| NASA Blue Marble deep ocean uses a uniform fill | High | NASA documentation explicitly states this |
| RGB (10, 40, 80) is darker than real ocean from space | High | Comparison with NASA satellite imagery values |
| Bathymetry texture would add visible depth gradients | High | PlanetPixelEmporium widely used; Cesium bathymetry mode |
| Clouds not needed before water improvement | Medium-High | Blue Marble is cloudless and ocean looks like water |

---

## Knowledge Gaps

- **Exact visual quality of the original Blue Marble ocean pixels at render resolution**: The original texture's ocean color has not been rendered through the pipeline since the fill was introduced. It may need brightness/contrast correction to look good under the current diffuse shading model.
- **What specific artifacts motivated the original uniform fill**: The roadmap says "too dark and sometimes contain satellite imagery" but does not identify specific problem regions. Understanding these would inform targeted correction vs wholesale fill.
- **Interaction between Fresnel diffuse mixing and the night-side blend**: The `blend_fragment()` function has a specific min-clamp that prevents darkening below the night texture. Fresnel brightening at the limb could interact with this near the terminator in ways that need visual testing.
- **Optimal sky color constant for Fresnel diffuse mixing**: The research suggests approximately (0.4-0.5, 0.6-0.7, 0.85-0.9) but the exact value depends on aesthetic preference and interaction with the base ocean color.

---

## Sources

### Codebase analysis
- `docs/plans/2026-03-17-water-appearance-codebase.md` -- full render pipeline trace for water pixels

### External research
- `docs/plans/2026-03-17-water-appearance-external.md` -- ocean color science, globe renderer survey, shader techniques

### Prior water shading research (PR #9 background)
- `docs/plans/2026-03-17-water-shading-research.md` -- consolidated research that led to the specular sun glint implementation
- `docs/plans/2026-03-17-water-shading-plan.md` -- implementation plan for specular (note: Fresnel was specified but not implemented)

### Key external sources (selected from external research document)
- [NASA PACE: Light and the Ocean](https://pace.oceansciences.org/ocean_light.cgi) -- physical basis for ocean color
- [Ocean Details -- Cesium Wiki](https://github.com/CesiumGS/cesium/wiki/Ocean-Details) -- production globe renderer water approach
- [PlanetPixelEmporium Earth Textures](https://planetpixelemporium.com/earth8081.html) -- bathymetry-based ocean color approach
- [GPU Gems Ch. 1: Effective Water Simulation](https://developer.nvidia.com/gpugems/gpugems/part-i-natural-effects/chapter-1-effective-water-simulation-physical-models) -- Fresnel and water shading reference
- [Schlick's Approximation -- Wikipedia](https://en.wikipedia.org/wiki/Schlick%27s_approximation) -- Fresnel formula reference
