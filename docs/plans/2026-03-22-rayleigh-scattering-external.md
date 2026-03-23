# Rayleigh Scattering and the Blue Atmospheric Limb: Physics and Rendering Research

*Research conducted 2026-03-22 for the Sunlit Earth project. Companion to `2026-03-22-airglow-external.md` (nightglow chemiluminescence). Covers the distinct day-side phenomenon: the blue atmospheric limb glow from Rayleigh scattering.*

---

## 1. Physical Rayleigh Scattering

### What Is Rayleigh Scattering?

Rayleigh scattering is the elastic scattering of electromagnetic radiation by particles much smaller than the wavelength of the radiation — specifically, with particle radius less than roughly one-tenth of the wavelength. In Earth's atmosphere the relevant particles are nitrogen (N2) and oxygen (O2) molecules, which are on the order of 0.3–0.4 nm in diameter, far smaller than visible light wavelengths (400–700 nm).

The defining characteristic of Rayleigh scattering is its strong inverse fourth-power dependence on wavelength:

```
beta(lambda) proportional to lambda^(-4)
```

This means blue light (440 nm) is scattered roughly 5.5 to 9.4 times more strongly than red light (680–700 nm), depending on which wavelengths you compare. Quantitatively, representative Rayleigh scattering coefficients at sea level for Earth's atmosphere are approximately:

- Red (680 nm): beta = 5.2e-6 m^-1
- Green (550 nm): beta = 1.2e-5 m^-1
- Blue (440 nm): beta = 3.0e-5 m^-1

(Values from the Alan Zucconi atmospheric scattering series and confirmed by CesiumJS's default coefficients of 5.5e-6 / 13.0e-6 / 28.4e-6 per RGB channel in 1/m units.)

The Rayleigh phase function describes how strongly light is scattered in each direction relative to the incident ray. For scattering angle theta (angle between incoming and outgoing ray directions):

```
P(theta) = (3 / 16*pi) * (1 + cos^2(theta))
```

This is symmetric: scattering is strongest in the forward direction (theta = 0) and backward direction (theta = 180 degrees), and weakest at 90 degrees. The forward/backward peaks are a factor of 1.5 stronger than the 90-degree minimum. This symmetry means the sky is brighter near the sun and in the antisolar direction, and somewhat dimmer at 90 degrees from the sun — a subtle but physically real effect.

### Why the Sky Is Blue

When sunlight enters the atmosphere, it scatters in all directions. An observer on the ground looking at any part of the sky (away from the direct sun) sees photons that have been redirected toward them by scattering events. Because blue photons scatter more frequently than red, the diffuse sky light is strongly blue-shifted relative to the incident solar spectrum. Red photons, scattering less, tend to travel in more nearly straight lines and are depleted less from the direct beam, leaving the sun itself appearing somewhat yellowish-white.

At sunset and sunrise, the direct sunbeam passes through a much longer atmospheric path — up to 40 times longer than at zenith. Over that path, blue photons are progressively scattered out of the direct beam, leaving only the longer red and orange wavelengths, which is why sunsets are red and orange.

### Why the Atmosphere Appears Blue from Space

When viewed from orbit, the atmosphere is not seen from the inside (looking up through a scattering volume) but from the outside (looking at the atmosphere edge-on at the limb). The physics is the same, but the geometry is different.

At the limb, a ray from the observer's eye grazes the top of the dense troposphere. The path length through the scattering volume is at a maximum because the ray is nearly tangent to the spherical shell. This is the path-length integration effect, often called limb brightening: the same thin layer of atmosphere that appears invisible when looking straight down (nadir view) appears bright when the ray path through it is very long.

Because the scattering is wavelength-dependent (blue scatters more), the integrated light scattered toward the observer at the limb is blue-dominated. The shorter the wavelength, the greater the accumulated scattering along the long tangential path. The result is a blue atmospheric halo at the day-side limb.

This effect is physically distinct from nightglow (airglow). It does not require chemical excitation or recombination. It is passive sunlight scattering: the atmosphere glows blue at the limb because blue sunlight is deflected toward the observer as it passes through the dense lower atmosphere at grazing angles.

### Typical Visual Thickness of the Blue Limb

From the ISS at approximately 400 km altitude, the visually prominent blue haze layer extends across the troposphere and lower stratosphere, roughly the lowest 15–50 km of the atmosphere. However, the optically dense scattering layer (troposphere) is only about 10–15 km thick. On Earth's 6371 km radius sphere, this is:

- Geometric fraction: 10–15 km / 6371 km ≈ 0.16–0.24% of Earth's radius
- As a fraction of Earth's visible disk radius: roughly 0.2–0.5% for the densest layer, up to 1–2% for the diffuse gradient

In ISS photographs, the blue limb typically appears as a thin but clearly visible crescent of saturated sky-blue color, visibly thinner than the white cloud layer below it. NASA's "Viewing Earth's Limb" photographs confirm the stratified structure: orange-brown at the lowest level (dust, aerosols), merging into pure blue, then deepening to dark blue/violet near the exosphere. The exact apparent thickness depends strongly on camera exposure settings — overexposing shows a thicker haze.

For the purposes of a space-view renderer, the effective scattering scale height — the altitude at which atmospheric density falls to 1/e of the surface value — is approximately 8.5 km for Rayleigh scattering. The commonly used value in real-time renderers is 7.994–8.5 km.

### Variation with Sun Angle and Terminator Proximity

The blue limb glow is only present on the day side where direct sunlight illuminates the atmosphere. On the night side the atmosphere is dark (no incident sunlight to scatter). This gives the blue limb a clear day/night behavior:

- **Deep day side (facing sun):** The limb glows blue with maximum intensity. The atmosphere is fully illuminated.
- **Near the terminator:** As the atmosphere transitions from day to night, the limb color shifts dramatically. The sun is at low angle, so light passing through the atmosphere at the surface travels a very long path (analogous to a sunset). Blue wavelengths are progressively removed, leaving orange and then red at the limb near the shadow boundary. The result is a gradient from blue (deep day side) to orange/reddish at the very edge of the terminator.
- **On the night side:** No Rayleigh-scattered limb glow. Only the green/yellow nightglow chemiluminescence remains (which is a separate phenomenon, covered in `2026-03-22-airglow-external.md`).

The terminator transition color (orange/reddish atmospheric arc) is one of the most visually striking features of Earth from space and corresponds physically to very high solar zenith angles creating sunset-like spectral depletion of blue light within the atmosphere. ISS photographs show this clearly as a narrow orange or salmon-colored arc along the shadow boundary.

### Limb Brightening: Viewing Angle and Scattering Intensity

The intensity of the scattered light at the limb varies dramatically with the angle between the view ray and the surface normal. At nadir (looking straight down), the ray passes through perhaps 10–15 km of atmosphere. At the limb (grazing angle), the same ray can pass through hundreds of kilometers of atmosphere at altitudes where density is still appreciable.

Quantitatively, optical depth at nadir is approximately 0.1 for the full atmosphere at 500 nm wavelength. At the limb (grazing through a spherical shell), the path length multiplier is roughly:

```
path_multiplier = sqrt(2 * pi * R_earth / H_scale)
                = sqrt(2 * pi * 6371 / 8.5)
                ≈ 69
```

where R_earth is Earth's radius and H_scale is the Rayleigh scale height. This means limb optical depth can be 69 times higher than nadir optical depth at the same altitude band. This is why the limb "glows" visibly even though the straight-down view through the same atmosphere appears transparent.

In shader terms this is captured naturally by any ray-marching integration through an atmosphere shell: rays that graze the top of the sphere accumulate much more scattering than rays heading toward the center. For the simplified Fresnel-based approaches, it is approximated by `pow(1 - dot(N, V), power)`, where `dot(N, V)` approaches zero at the limb (grazing angle), driving the glow term to full intensity. This is a reasonable approximation of the path-length effect.

---

## 2. Visual Appearance from Space

### Color

The dominant color of the day-side limb is a saturated sky blue, approximately sRGB (0.3, 0.6, 1.0) or similar, corresponding to the preferential scattering of 400–500 nm wavelengths. The exact hue shifts toward cyan or white near the brightest parts of the limb (where the dense lower troposphere scatters even red somewhat), and shifts toward deeper indigo/violet at higher altitudes where the column is thinner and blue dominates more.

Near the terminator the limb transitions through white → orange → red as solar zenith increases and path lengths through the lower dense atmosphere increase.

### Thickness

From a typical ISS photograph:
- The blue haze band occupies roughly 1–3% of Earth's disk radius
- The most optically dense blue layer (troposphere) appears as a thinner inner band of approximately 0.5–1% of disk radius
- The diffuse outer halo can extend to 2–5% of disk radius at long exposures

For a renderer using a normalized unit sphere (radius = 1.0), the atmosphere shell should be placed at approximately 1.015 to 1.025 (1.5–2.5% above the surface), with the visible glow confined to a thin rim via a high-exponent falloff function.

### Brightness and Sun Modulation

The blue limb glow is:
- Maximum intensity on the day-side limb directly opposite the sun from the observer's viewpoint
- Reduced toward zero approaching the night-side terminator
- Zero on the night side (no incident sunlight to scatter)

The glow is NOT simply proportional to `dot(N, sun_dir)` — it is actually the atmospheric column that is sun-illuminated, and the observer may or may not be looking at the illuminated limb. In a simplified model, modulating by `max(0, dot(N, sun_dir))` is a reasonable first approximation for whether the atmosphere at that point is lit. More physically accurate approaches integrate the illumination along the ray path.

---

## 3. Real-Time Rendering Approaches

### Approach 1: Simple Fresnel/Rim Glow

The most basic approximation uses only the view-normal angle to produce a rim glow term, then tints it blue:

```wgsl
let NdotV = clamp(dot(normalize(world_normal), normalize(view_dir)), 0.0, 1.0);
let rim = pow(1.0 - NdotV, atmo_power);          // e.g. power = 4.0-8.0
let atmo_color = vec3<f32>(0.25, 0.55, 1.0);     // blue
let atmo = rim * atmo_color * atmo_intensity;
```

This produces a blue glow around the entire sphere silhouette regardless of sun position.

**Quality assessment:** Recognizably atmospheric in color and location. Looks like a "space game" atmosphere. Does not reproduce the day/night asymmetry (both sides glow equally), does not extend beyond the sphere edge into space, and does not reproduce the terminator color shift to orange/red. Suitable for a quick prototype.

**Parameters:** 1 intensity scalar, 1 power exponent. Optionally 3 color channels if the color is made configurable.

### Approach 2: Sun-Modulated Fresnel Rim

Extends Approach 1 by suppressing the glow on the night side and at the limb where the surface faces away from the sun:

```wgsl
let NdotV = clamp(dot(normalize(world_normal), normalize(view_dir)), 0.0, 1.0);
let NdotL = dot(normalize(world_normal), sun_dir);
let rim = pow(1.0 - NdotV, atmo_power);

// Blue glow only where sun illuminates the atmosphere
// Smooth transition rather than hard cutoff
let day_factor = clamp(NdotL * 2.0 + 0.5, 0.0, 1.0);
let atmo_color = vec3<f32>(0.25, 0.55, 1.0);
let atmo = rim * day_factor * atmo_color * atmo_intensity;
```

An optional enhancement adds a terminator-zone color shift from blue to orange/reddish:

```wgsl
// Near-terminator atmosphere: sun angle drives a blue-to-red shift
let sun_horizon_factor = clamp(1.0 - NdotL * 4.0, 0.0, 1.0); // 1 near terminator, 0 in full sun
let terminator_color = mix(
    vec3<f32>(0.25, 0.55, 1.0),   // blue (full day side)
    vec3<f32>(1.0, 0.4, 0.1),     // orange (near terminator)
    sun_horizon_factor * 0.6       // blend strength
);
let atmo = rim * day_factor * terminator_color * atmo_intensity;
```

**Quality assessment:** Substantially better than Approach 1. The day/night asymmetry is now correct. The terminator color shift adds significant visual realism. Still cannot produce glow beyond the sphere silhouette (i.e., no actual halo extending into space), and the mathematical relationship between NdotL and the color gradient is empirical rather than physical. For a fixed-viewpoint wallpaper this is often acceptable.

**Parameters:** 2 uniforms (intensity, power) if color is hard-coded. 3 additional uniforms if terminator color shift parameters are exposed. Total: 2–5 user-facing parameters.

### Approach 3: Separate Atmosphere Shell Geometry

Renders a concentric sphere slightly larger than Earth (scaled by e.g. 1.015–1.025) as a separate draw call with additive blending. The atmosphere shell's fragment shader computes the Fresnel/rim term independently from the surface shader:

```wgsl
// Fragment shader for atmosphere shell (fs_atmo)
// Input: world_normal interpolated from vs_atmo vertex shader
//        (for a sphere, this equals the normalized vertex position)

@fragment
fn fs_atmo(in: VertexOutput) -> @location(0) vec4<f32> {
    let N = normalize(in.world_pos);
    let V = normalize(uniforms.camera_pos - in.world_pos);
    let NdotV = clamp(dot(N, V), 0.0, 1.0);
    let NdotL = dot(N, uniforms.sun_dir);

    // Rim falloff
    let rim = pow(1.0 - NdotV, uniforms.atmo_falloff);

    // Day-side modulation
    let day_factor = clamp(NdotL * 2.0 + 0.5, 0.0, 1.0);

    // Color shifts from blue (day) to orange (terminator)
    let blue  = vec3<f32>(0.25, 0.55, 1.0);
    let orange = vec3<f32>(1.0, 0.4, 0.1);
    let t = clamp(1.0 - NdotL * 3.0, 0.0, 1.0);
    let color = mix(blue, orange, t * t * 0.5);

    let intensity = rim * day_factor * uniforms.atmo_intensity;
    return vec4<f32>(color * intensity, intensity);
}
```

With additive blend state in wgpu (`src: One, dst: One`), this layer adds its color to whatever is behind it, naturally producing a halo visible beyond the sphere edge.

**Key advantage over approaches 1–2:** The glow extends into space beyond the sphere silhouette, which is how real atmospheric halos look. The shell sphere's geometry guarantees that at viewing angles that see the dark space behind the Earth's edge, the atmosphere shader still runs and produces color.

**Draw order for this approach:**
1. Earth sphere (opaque, depth write on)
2. Atmosphere shell (additive blend, depth write off, depth test on)
3. Cloud sphere (alpha blend, depth write off, depth test on)
4. Nightglow shell (additive blend, depth write off, depth test on) — if the airglow feature is also present

**Quality assessment:** Significantly better than approaches 1–2 for space-view rendering. The beyond-silhouette glow is a major visual improvement. Color shift at terminator can be added. Still not physically derived — the shape of the glow does not exactly match Rayleigh path-length accumulation, but the result is convincing for most viewers.

**Parameters:** intensity, falloff exponent, shell radius. Optionally: separate day color and terminator color vectors (6 more floats).

This is the technique used by CesiumJS (sky atmosphere ellipsoid), NASA WorldWind (sky dome geometry), and Unity Earth tutorials.

### Approach 4: Single-Scattering Ray Marching (Sean O'Neil / GPU Gems 2)

Sean O'Neil's 2004 chapter "Accurate Atmospheric Scattering" (GPU Gems 2, Chapter 16) presents the canonical real-time implementation of single-scattering volumetric atmosphere. The algorithm:

1. For each pixel, cast a ray from the camera through the atmosphere shell
2. Divide the ray into N sample points (typically 5–20 for space view, more for ground view)
3. At each sample point, evaluate the optical depth integral along both the view ray and the sun ray (out-scattering and in-scattering)
4. The original CPU implementation required ~3000 operations per vertex. The GPU version replaces the 2D lookup tables with polynomial approximations, reducing this to ~60 operations per vertex

Key equations:

```glsl
// Rayleigh phase function (cosine of angle between view and sun)
float rayleighPhase = 0.75 * (1.0 + mu * mu);  // mu = dot(view_dir, sun_dir)

// Mie phase function
float miePhase = 1.5 * ((1.0 - g2) / (2.0 + g2)) *
    (1.0 + mu*mu) / pow(1.0 + g2 - 2.0*g*mu, 1.5);

// Exponential density falloff with altitude h
float density = exp(-h / scale_height);

// Per-step accumulation (simplified)
vec3 scatter = density * (kRayleigh * wavelength_factors + kMie);
transmittance *= exp(-scatter * step_size);
color += transmittance * scatter * sun_intensity * step_size;
```

Where `wavelength_factors = vec3(5.8e-6, 13.5e-6, 33.1e-6)` for Rayleigh (RGB, normalized by wavelength^4 relative to some reference) and `kMie = vec3(21e-6)` approximately.

The implementation uses two separate shader programs: `SkyFromSpace` (camera outside atmosphere) and `SkyFromAtmosphere` (camera inside). For Sunlit Earth the camera is always in space, so only `SkyFromSpace` is needed.

**Key outputs:** The algorithm naturally produces:
- Blue day-side limb from Rayleigh dominance
- Orange/red near-terminator color from path-length depletion of blue
- Correct brightness falloff (stronger at limb than center)
- Mie forward scattering glow around the sun disk (visible in sky when camera is inside atmosphere, less so from space)
- HDR output requiring tone mapping: `1.0 - exp(-exposure * color)`

**Shader structure (pseudocode):**
```
SkyFromSpace vertex shader:
  - Ray-sphere intersect against outer atmosphere shell
  - For each of N primary ray steps:
      - Sample Rayleigh + Mie optical depth accumulator
      - For each step, compute secondary optical depth toward sun
  - Output: accumulated Rayleigh color, accumulated Mie color

SkyFromSpace fragment shader:
  - Apply Rayleigh phase function * accumulated Rayleigh color
  - Apply Mie phase function * accumulated Mie color
  - Apply HDR tone-map
```

**Libraries implementing this technique:**
- `wwwtyro/glsl-atmosphere` (MIT, ~200 lines GLSL) — clean modern implementation with ray-sphere intersection. Signature:
  ```glsl
  vec3 atmosphere(
      vec3 r,         // ray direction (normalized)
      vec3 r0,        // ray origin (camera position, world space)
      vec3 pSun,      // sun direction (normalized)
      float iSun,     // sun intensity (e.g. 22.0)
      float rPlanet,  // planet radius (e.g. 6371e3)
      float rAtmos,   // atmosphere radius (e.g. 6471e3 = 100km above surface)
      vec3 kRlh,      // Rayleigh scattering coefficients (e.g. vec3(5.5e-6, 13.0e-6, 22.4e-6))
      float kMie,     // Mie scattering coefficient (e.g. 21e-6)
      float shRlh,    // Rayleigh scale height (e.g. 8e3)
      float shMie,    // Mie scale height (e.g. 1.2e3)
      float g         // Mie anisotropy (e.g. 0.758)
  );
  ```

- `github.com/dpasca/oneil_scattering_demo_udate` — updated version of O'Neil's original demo (C++/GLSL)
- `Shadertoy/4tjGRh` ("Planet") — full planet-from-space shader in GLSL, self-contained Shadertoy

**Quality:** Very good. The blue limb is physically motivated. Color transitions near the terminator are correct. The glow extends into space naturally from the ray-marching geometry.

**Cost for Sunlit Earth:** At 16 primary ray steps and 8 secondary steps, this is 128 texture-free float operations per fragment plus the phase functions. For a desktop wallpaper with 2-minute redraw intervals, this is trivially fast even on integrated GPU. The implementation cost (porting from GLSL to WGSL, setting up the sphere geometry, ray-sphere intersect) is estimated at 3–5 days.

**WGSL porting notes:** The `wwwtyro/glsl-atmosphere` shader is GLSL with no version directives and no texture dependencies — it is straightforward to port to WGSL. The main changes are:
- `vec3` -> `vec3<f32>`
- `float` -> `f32`
- `max(a, b)` -> `max(a, b)` (same)
- `exp()`, `pow()`, `dot()`, `sqrt()` -> identical names in WGSL
- Loop syntax: `for (int i = 0; i < N; i++)` -> `for (var i: i32 = 0; i < N; i++)`
- No `discard` needed; the atmosphere function returns vec3 that is added to background

### Approach 5: Precomputed Atmospheric Scattering (Bruneton/Neyret 2008, Updated 2017)

Eric Bruneton and Fabrice Neyret's 2008 EGSR paper "Precomputed Atmospheric Scattering" precomputes all scattering integrals into lookup tables, enabling O(1) per-pixel runtime cost with support for multiple scattering, ozone absorption, and accurate shadow casting.

**Lookup tables computed at initialization:**
1. **Transmittance LUT** (2D texture, ~256x64): For each (altitude h, sun zenith cos theta) pair, stores the RGB transmittance — how much light survives the path from altitude h to the top of the atmosphere in the sun's direction. Parameterized by two scalars.
2. **Scattering LUT** (3D or 4D texture, ~256x128x32 or similar): Stores single Rayleigh scattering plus multiple scattering contributions, parameterized by (altitude, sun zenith, view zenith, view-sun azimuth). This is the large table that Hillaire's 2020 work reduced.
3. **Irradiance LUT** (2D texture): Ground irradiance from multiple scattering at each altitude and sun angle.
4. **Multiple Scattering LUT**: Separate table for the higher-order scattering terms.

**Runtime usage (per pixel):**
```glsl
// Evaluate sky color at a point on the atmosphere shell
vec3 radiance = GetSkyRadiance(camera, view_ray, sun_direction, transmittance);
```
This is one function call that does a few texture lookups and some arithmetic. No loop, no ray marching. Cost is comparable to a single texture sample.

**The 2017 update** added:
- Ozone absorption (creates the characteristic blue tint at the limb even without Rayleigh)
- Custom density profiles for aerosols
- Correct spectral-to-RGB conversion via CIE color matching functions

**WGSL availability:** `JolifantoBambla/webgpu-sky-atmosphere` implements Hillaire's 2020 variant (which uses the same LUT approach but with smaller, faster tables) in WGSL for WebGPU. The library uses compute shaders to build the LUTs and a render pass to evaluate the sky. It supports both ground and space views.

However, integrating this into Sunlit Earth would require:
- Several compute passes at startup to build LUTs
- 2–4 additional textures and bind group entries
- Restructuring the rendering notifier to include a preprocessing phase
- The library is designed as an npm package for WebGPU (browser), not for native wgpu (Rust) — porting requires understanding its internal structure

Implementation estimate: 1–2 weeks to integrate properly.

**Quality:** Near-reference quality. Multiple scattering produces the subtle second-order effects (sky is not perfectly black where it faces away from the sun — there is still diffuse sky light). Ozone layer gives the correct deep blue at the limb. Terminator gradient is physically exact.

### Approach 6: Hillaire 2020 — Scalable Production Ready Technique

Sébastien Hillaire's 2020 EGSR paper and its Unreal Engine 5 implementation refine Bruneton's approach to avoid the large 4D scattering texture. Instead, it uses:

1. **Transmittance LUT** (same as Bruneton): 256x64, static per atmosphere definition.
2. **Multiple Scattering LUT** (2D, ~32x32): New approximation. Stores a single value representing the average multiple-scattering contribution per sample point. Much smaller than Bruneton's full scattering table, with small visual accuracy tradeoff.
3. **Sky-View LUT** (2D, ~200x100 or 256x128): Parameterized by latitude (view elevation angle) and sun angle. Rendered each frame. Non-linear parameterization concentrates detail near the horizon. This is the primary table enabling cheap per-pixel evaluation.
4. **Aerial Perspective LUT** (3D volume texture, ~32x32x32): View-frustum-space volume storing in-scattering and transmittance for terrain/cloud aerial perspective.

For a space view, only the Transmittance LUT, Multiple Scattering LUT, and Sky-View LUT are strictly needed. The Sky-View LUT is re-evaluated each frame when the sun moves (in Sunlit Earth, every 2-minute timer tick).

**Key advantages over Bruneton 2008:**
- No large 4D texture (the main bottleneck)
- Atmosphere parameters can be changed at runtime without full LUT recompute
- Works at mobile GPU performance levels (the Sky-View LUT is small)
- Multiple scattering is approximated but visually very close to the ground truth

**WebGPU WGSL implementation:** `JolifantoBambla/webgpu-sky-atmosphere` on GitHub. This is the only known WGSL implementation of this technique as of early 2026. It demonstrates a `SkyAtmosphereComputeRenderer` class that manages LUT builds and sky rendering as post-process passes.

### Approach 7: Closed-Form Analytical Approximations

Several implementations avoid ray marching entirely by solving simplified forms of the scattering integral analytically, making assumptions that reduce the problem to a closed-form expression.

The approach from `rhept.org` (and similar work) makes these assumptions:
- Single scattering only
- Parallel sun rays (directional light)
- Flat Earth (density varies only with vertical coordinate)
- Exponentially decaying density profile with infinite upper boundary

Under these assumptions the double integral (view ray integral of [sun ray integral]) collapses to a product of exponential terms:

```glsl
// Full Rayleigh + Mie analytical sky (from rhept.org)
const float I0 = 11.0;
const vec3 sigma_r = vec3(0.33, 0.78, 1.89);  // Rayleigh coefficients (blue >> red)
const float S_r = 0.17;
const float S_m = 0.05;

vec3 sky_full(vec3 vd, vec3 sd) {  // vd = view dir, sd = sun dir
    float mu = dot(vd, sd);
    float phase_r = 0.06 * (1.0 + mu * mu);
    float phase_m = 0.3 * pow(0.25 / (1.25 + mu), 2.0);

    vec3 sigma_sum = S_r * sigma_r + S_m;
    vec3 phase_sum = S_r * sigma_r * phase_r + S_m * phase_m;

    // The core analytical formula: product of two exponentials
    return I0 * sd.y / (sd.y + vd.y) * (phase_sum / sigma_sum) *
           (exp(sigma_sum / sd.y) - exp(-sigma_sum / vd.y));
}
```

**Limitation for space view:** This formulation assumes the observer is inside the atmosphere (`vd.y` is the vertical component of the view direction). From space, `vd.y` is zero or negative for most view rays, which causes division-by-zero or nonsensical output. The formula must be adapted for the space-view case, typically by providing the tangent altitude from sphere-ray intersection geometry.

The Shadertoy shader `lcfSRl` ("Fast Atmosphere") extends this to work from space by computing the apparent view elevation at the tangent point rather than using the Cartesian view direction directly.

For Sunlit Earth, the closed-form approach would require either restricting to the on-sphere case (and thus losing the beyond-silhouette halo) or deriving the tangent-point parameterization — which is essentially the same cost as simple ray marching.

### What CesiumJS, Google Earth, and NASA WorldWind Use

**CesiumJS (2022 Improved Atmosphere):** Single-scattering Nishita 1993 algorithm. Two rendered objects:
1. Sky atmosphere shell (ellipsoid larger than Earth) — GLSL fragment shader with ray marching through the shell, computing Rayleigh + Mie accumulation. This is what produces the blue limb glow.
2. Ground atmosphere (rendered on globe surface at distance, fog when close).
Implementation in `AtmosphereCommon.glsl` in the CesiumGS/cesium repository. Rayleigh coefficients: `vec3(5.5e-6, 13.0e-6, 28.4e-6)` per RGB channel. Single scattering, HDR tone-map with exposure parameter. Exposed parameters: rayleighCoefficient, mieCoefficient, mieAnisotropy, rayleighScaleHeight, mieScaleHeight, lightIntensity, hueShift, saturationShift, brightnessShift.

**NASA WorldWind:** Rayleigh + Mie in a sky dome shader (vertex shader scattering). Phase functions implemented directly. Parameters: Kr = 0.0025, Km = 0.0015, ESun = 15.0, rayleighScaleDepth = 0.25, g = -0.95. Exposure tone-map applied. The limb glow emerges from the scattering accumulation.

**Google Earth:** Uses Rayleigh atmospheric scattering for the blue limb halo. Implementation details proprietary. The 2013 "photorealistic atmosphere" update added the visible blue atmospheric limb for space-view scenes. Visually consistent with single-scattering Nishita/O'Neil quality.

---

## 4. Practical Assessment for Sunlit Earth

### The Core Visual Goal

The target phenomenon — blue atmospheric limb on the day side, visible as a thin halo around the planet in a space-view wallpaper — is dominated by three visual elements:
1. The glow is blue/cyan (Rayleigh scattering favors short wavelengths)
2. The glow is brightest at the rim (limb path-length effect)
3. The glow is only on the sunlit side, transitioning to orange/reddish at the terminator

A fourth desirable element is that the glow extends slightly beyond the planet edge into the "space" pixels around the sphere. This requires either a shell geometry draw call or a screenspace post-process pass.

### Which Approach for Sunlit Earth?

Given the constraints (single draw call preferred, no compute shaders or LUTs currently, WGSL, uniforms-based parameters), the following ranking applies:

**Best fit: Approach 3 (Separate Atmosphere Shell) + sun-modulated color shift**

This matches the existing cloud layer architecture exactly. The cloud sphere is already an example of a separate concentric draw call with alpha blend and depth settings. Swapping to additive blend and writing a simple rim shader with day-side modulation is straightforward.

The key additions over a pure Fresnel rim that justify the extra draw call:
- Glow extends visibly into space beyond the planet edge — the most important visual improvement
- Correct day/night side asymmetry
- Terminator color shift from blue to orange adds significant realism at low cost

Estimated uniform additions: 2 scalars (atmo_intensity, atmo_falloff). Shell radius can reuse or mirror the cloud sphere radius pattern. Color can be hard-coded in the shader with an optional hue parameter.

**Second option: Approach 4 (Ray marching, wwwtyro/glsl-atmosphere)**

If higher visual quality is desired and the 3–5 day implementation investment is acceptable, porting `wwwtyro/glsl-atmosphere` to WGSL would be the next step. The physics-derived output is substantially better near the terminator (correct color gradient, accurate brightening vs. the approximation in Approach 3). The computational cost is negligible for a 2-minute-redraw wallpaper.

The implementation structure for Sunlit Earth would be:
- Separate atmosphere shell sphere (same geometry approach as Approach 3)
- The fragment shader replaces the simple rim calculation with the full ray-marching integral
- Camera position must be passed as a uniform (currently not needed but straightforward to add)
- Rayleigh coefficients, scale heights, atmosphere radius as additional uniforms

**Do not pursue for initial implementation: Approaches 5/6 (LUT-based)**

The LUT precomputation requires compute shader support and significant architectural changes. The visual improvement over a good ray-marching implementation is subtle for a space-view (most of the multiple-scattering gains are visible from ground level, not from orbit). The 1–2 week cost is disproportionate to the visual gain for this use case.

### Coexistence with Nightglow

The blue day-side Rayleigh limb and the green/yellow night-side airglow (nightglow, covered in `2026-03-22-airglow-external.md`) are two distinct physical phenomena that occupy different sides of the planet. In a renderer:

- Blue atmospheric limb: day-side shell, modulated by `max(0, NdotL)` or equivalent
- Green/yellow nightglow: night-side shell, modulated by `max(0, -NdotL)` or similar

Both can be rendered as separate draw calls on the same atmosphere shell sphere with different shaders, or as a single draw call with the shader selecting the correct color based on NdotL. If the blue limb and nightglow share an atmosphere shell draw call, the fragment shader would blend between the two:

```wgsl
let NdotL = dot(N, sun_dir);
let day_weight   = clamp(NdotL * 4.0, 0.0, 1.0);
let night_weight = clamp(-NdotL * 4.0, 0.0, 1.0);
let atmo = rim * (blue_color * day_weight + green_color * night_weight) * intensity;
```

This combined approach would fit in a single draw call and a single pair of uniform fields (day intensity, night intensity).

### Parameter Budget for Uniforms

Keeping the uniform struct at 16-byte multiples:

Minimal Rayleigh blue limb (hard-coded color in shader, shell radius shared with or offset from airglow radius):
- `atmo_day_intensity: f32` — day-side Rayleigh glow brightness (0.0 = off)
- `atmo_day_falloff: f32` — rim exponent for day-side glow (4.0–10.0)
- (padding or additional fields)

If the blue limb and nightglow share a shell:
- `atmo_day_intensity: f32`
- `atmo_day_falloff: f32`
- `atmo_night_intensity: f32`
- `atmo_night_falloff: f32`

This is a clean 16-byte addition. The shell radius uniform (`airglow_radius`) already covers both phenomena if they share the same shell.

### Visual Comparison: Simple vs. Physical

| Feature | Approach 2 (Fresnel in sphere shader) | Approach 3 (Shell + simple rim) | Approach 4 (Ray marching) |
|---|---|---|---|
| Blue day-side glow | Yes | Yes | Yes |
| Glow extends into space beyond sphere edge | No | Yes | Yes |
| Day/night asymmetry | Yes | Yes | Yes |
| Terminator color shift (blue -> orange) | Approximate | Approximate | Physical |
| Limb brightness vs. center ratio | Approximate | Approximate | Physical |
| Multiple scattering (haze on day side) | No | No | No (single scattering) |
| Implementation cost | Hours | 1–2 days | 3–5 days |
| Uniform count added | 2 | 3–4 | 5–7 |
| WGSL shader lines added | ~15 | ~40 | ~120 |

---

## 5. Key Shader Code Reference

### WGSL Fragment Shader for Approach 3 (Atmosphere Shell, Sun-Modulated Rim)

This is pseudocode for the atmosphere shell fragment shader entry point, intended to be added to `shaders/sphere.wgsl` alongside the existing `vs_cloud` / `fs_cloud` entry points:

```wgsl
@fragment
fn fs_atmo(in: AtmoVertexOutput) -> @location(0) vec4<f32> {
    let N = normalize(in.world_pos);
    // Camera position from inverse MVP or passed as uniform
    // For a unit sphere this simplifies: V = -normalize(in.world_pos) when cam is far
    // Better: pass camera_world_pos as a uniform
    let V = normalize(uniforms.camera_world_pos - in.world_pos);
    let NdotV = clamp(dot(N, V), 0.0, 1.0);
    let NdotL = dot(N, uniforms.sun_dir);

    // Rim falloff: high exponent = tight band at the edge
    let rim = pow(1.0 - NdotV, uniforms.atmo_day_falloff);

    // Day-side modulation: suppress on night side
    let day_factor = clamp(NdotL * 3.0 + 0.3, 0.0, 1.0);

    // Color: blue on deep day side, orange near terminator
    let blue   = vec3<f32>(0.3, 0.6, 1.0);
    let orange = vec3<f32>(1.0, 0.45, 0.1);
    // Normalized sun horizon factor: 1 when sun is on horizon from this point
    let t = clamp(1.0 - NdotL * 4.0, 0.0, 1.0);
    let color = mix(blue, orange, t * t * 0.5);

    let intensity = rim * day_factor * uniforms.atmo_day_intensity;

    // Additive output: alpha = 0 so depth blend works correctly
    return vec4<f32>(color * intensity, 0.0);
}
```

The vertex shader (`vs_atmo`) is identical to `vs_cloud` but uses `uniforms.atmo_shell_radius` as the scale factor.

### GLSL Atmosphere Function (wwwtyro/glsl-atmosphere, for WGSL adaptation)

The key function signature and loop structure, suitable for adaptation to WGSL for Approach 4:

```glsl
// Original GLSL from wwwtyro/glsl-atmosphere (MIT license)
#define PI 3.141592
#define iSteps 16   // primary ray samples
#define jSteps 8    // sun ray samples

// Ray-sphere intersection helper
vec2 rsi(vec3 r0, vec3 rd, float sr) {
    float a = dot(rd, rd);
    float b = 2.0 * dot(rd, r0);
    float c = dot(r0, r0) - (sr * sr);
    float d = (b*b) - 4.0*a*c;
    if (d < 0.0) return vec2(1e5, -1e5);
    return vec2((-b - sqrt(d))/(2.0*a), (-b + sqrt(d))/(2.0*a));
}

vec3 atmosphere(
    vec3 r,       // normalized ray direction
    vec3 r0,      // ray origin (camera world position)
    vec3 pSun,    // sun direction (normalized)
    float iSun,   // sun intensity (22.0 is a good default)
    float rPlanet,// planet radius in same units as r0
    float rAtmos, // atmosphere shell radius
    vec3 kRlh,    // Rayleigh coefficients (e.g. vec3(5.5e-6, 13.0e-6, 22.4e-6))
    float kMie,   // Mie coefficient (e.g. 21e-6)
    float shRlh,  // Rayleigh scale height (e.g. 8e3)
    float shMie,  // Mie scale height (e.g. 1.2e3)
    float g       // Mie anisotropy g-factor (e.g. 0.758)
) {
    // ... ray-sphere intersect, loop, accumulate ...
    // Returns vec3 RGB sky color (may need HDR tone-mapping: 1 - exp(-color))
}
```

WGSL translation would change type names and loop syntax but the math is identical.

---

## 6. Sources

- [Rayleigh scattering — Wikipedia](https://en.wikipedia.org/wiki/Rayleigh_scattering)
- [The Mathematics of Rayleigh Scattering — Alan Zucconi](https://www.alanzucconi.com/2017/10/10/atmospheric-scattering-3/)
- [Rayleigh Phase Function — PDS Atmospheres, NMSU](https://pds-atmospheres.nmsu.edu/education_and_outreach/encyclopedia/rayleigh_phase.htm)
- [Simulating the Colors of the Sky — Scratchapixel](https://www.scratchapixel.com/lessons/procedural-generation-virtual-worlds/simulating-sky/simulating-colors-of-the-sky.html)
- [Blue Sky and Rayleigh Scattering — HyperPhysics, Georgia State University](http://hyperphysics.phy-astr.gsu.edu/hbase/atmos/blusky.html)
- [Viewing Earth's Limb — NASA Earth Observatory](https://www.earthobservatory.nasa.gov/images/3338/viewing-earths-limb)
- [Earth's Limb with a Crescent Moon — NASA Science](https://science.nasa.gov/earth/earth-observatory/earths-limb-with-a-crescent-moon-150240/)
- [Earth's Colorful Atmospheric Layers Photographed from Space — Space.com](https://www.space.com/8596-earth-colorful-atmospheric-layers-photographed-space.html)
- [Crepuscular Rays and Light Scattering — NASA Science](https://science.nasa.gov/earth/earth-observatory/crepuscular-rays-and-light-scattering-150090/)
- [Accurate Atmospheric Scattering — GPU Gems 2, Ch. 16, Sean O'Neil, NVIDIA](https://developer.nvidia.com/gpugems/gpugems2/part-ii-shading-lighting-and-shadows/chapter-16-accurate-atmospheric-scattering)
- [Precomputed Atmospheric Scattering (2008/2017) — Eric Bruneton](https://ebruneton.github.io/precomputed_atmospheric_scattering/)
- [Precomputed Atmospheric Scattering — ebruneton, GitHub](https://github.com/ebruneton/precomputed_atmospheric_scattering)
- [A Scalable and Production Ready Sky and Atmosphere Rendering Technique (2020) — Sébastien Hillaire, Wiley CGF](https://onlinelibrary.wiley.com/doi/abs/10.1111/cgf.14050)
- [Production Ready Atmosphere Rendering — Hillaire 2020 PDF (egsr2020)](https://sebh.github.io/publications/egsr2020.pdf)
- [webgpu-sky-atmosphere (WGSL Hillaire implementation) — JolifantoBambla, GitHub](https://github.com/JolifantoBambla/webgpu-sky-atmosphere)
- [glsl-atmosphere (MIT, Rayleigh+Mie, space view) — wwwtyro, GitHub](https://github.com/wwwtyro/glsl-atmosphere)
- [Improved Atmosphere in CesiumJS — Cesium Blog, 2022](https://cesium.com/blog/2022/05/26/improved-atmosphere-in-cesiumjs/)
- [SkyAtmosphere API — CesiumJS Documentation](https://cesium.com/learn/cesiumjs/ref-doc/SkyAtmosphere.html)
- [WorldWind SkyProgram GLSL source — NASA WorldWind](https://nasaworldwind.github.io/WebWorldWind/shaders_SkyProgram.js.html)
- [Analytical Sky Shader — rhept.org](https://rhept.org/posts/scattering/)
- [A Shader for the Atmospheric Sphere — Alan Zucconi](https://www.alanzucconi.com/2017/10/10/shader-atmospheric-sphere/)
- [Atmosphere Rendering: A literature review — trist.am, 2024](https://www.trist.am/blog/2024/atmosphere-rendering/)
- [TerrainView7: Planet rendering in WebGPU with Bruneton scattering — DEV Community](https://dev.to/the_lone_engineer/terrainview7-full-scale-planet-rendering-in-webgpu-emscripten-now-with-precomputed-atmospheric-18ik)
- [Sky & Atmosphere rendering resource list — vterrain.org](http://vterrain.org/Atmosphere/)
- [Unofficial update of O'Neil's atmospheric scattering demo — dpasca, GitHub](https://github.com/dpasca/oneil_scattering_demo_udate)
- [Efficient and Dynamic Atmospheric Scattering — Elek 2009, Chalmers University (PDF)](https://publications.lib.chalmers.se/records/fulltext/203057/203057.pdf)
