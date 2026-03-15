# Day/Night Rendering: Solar Position Research

**Date:** 2026-03-15
**Purpose:** External research on solar position algorithms and day/night shader patterns for 3D globe rendering.

---

## 1. Executive Summary

Computing the sun's direction for real-time globe rendering requires two stages: (a) computing the sun's angular position in an Earth-relative coordinate frame from a date/time, and (b) using that direction in a fragment shader to blend day and night textures. Both stages have well-established, compact formulas. The simplified declination + hour-angle approach gives visual accuracy within ~1° — more than sufficient for rendering. The standard shader pattern uses a dot product between the surface normal and the sun direction, smoothed with `smoothstep` or a logistic function across a narrow twilight band.

---

## 2. Solar Position Algorithms

### 2.1 Precision spectrum

There is a wide spectrum of solar position algorithms:

- **Full precision (NREL SPA):** Accurate to ±0.0003° over years −2000–6000. Uses VSOP87 planetary theory. Far more complexity than needed for rendering. Source: [NREL SPA](https://midcdmz.nrel.gov/spa/)
- **Medium precision (NOAA / Spencer 1971):** Fourier series for equation of time and declination. Accurate to a few tenths of a degree. Suitable for real applications. Source: [NOAA General Solar Position Equations](https://gml.noaa.gov/grad/solcalc/solareqns.PDF)
- **Simple trigonometric (day-of-year):** Single-term sine/cosine approximation. Error ≤ 1–2°. Perfectly adequate for visual rendering.

For real-time rendering, the simple day-of-year formulas are the correct choice.

### 2.2 Solar declination

The declination (δ) is the latitude of the subsolar point — how far north or south the sun sits relative to the equator. It ranges from −23.44° (December solstice) to +23.44° (June solstice) and is zero at the equinoxes.

**Simplest formula** (single cosine, used widely in solar engineering):

```
δ = −23.44° × cos( 360/365 × (d + 10) )
```

where `d` is the day of year (January 1 = 1). The offset `+10` shifts the zero crossing to match the winter solstice timing. Source: [PVEducation — Declination Angle](https://www.pveducation.org/pvcdrom/properties-of-sunlight/declination-angle)

**Equivalent sine form** (same accuracy, different phase reference):

```
δ = 23.44° × sin( 360/365 × (d − 81) )
```

The `−81` offset places δ = 0 at the March equinox (~day 81). These two forms are algebraically identical; choose based on which phase convention is more natural.

**Spencer (1971) Fourier series** (error < 0.5°, overkill but compact):

```
B = 360/365 × (d − 1)   (degrees)
δ = 0.006918 − 0.399912·cos(B) + 0.070257·sin(B)
              − 0.006758·cos(2B) + 0.000907·sin(2B)
              − 0.002697·cos(3B) + 0.00148·sin(3B)   (radians)
```

Source: [PVPMC Basic Solar Position Models](https://pvpmc.sandia.gov/modeling-guide/1-weather-design-inputs/sun-position/basic-solar-position-models/)

### 2.3 Equation of time

The equation of time (EoT) corrects for the discrepancy between mean solar time (clocks) and apparent solar time (actual sun position), caused by Earth's elliptical orbit and axial tilt. It ranges from about −16 to +14 minutes.

**Compact approximation** (error ≤ ~0.5 minutes, suitable for rendering):

```
B = 360/365 × (d − 81)   (degrees)
EoT = 9.87·sin(2B) − 7.53·cos(B) − 1.5·sin(B)   (minutes)
```

Source: [PVEducation — Solar Time](https://www.pveducation.org/pvcdrom/properties-of-sunlight/solar-time)

**NOAA Fourier form** (slightly higher precision):

```
γ = 2π/365 × (d − 1)   (radians, fractional year)
EoT = 229.18 × ( 0.000075 + 0.001868·cos(γ) − 0.032077·sin(γ)
                            − 0.014615·cos(2γ) − 0.04089·sin(2γ) )   (minutes)
```

Source: [NOAA General Solar Position Equations](https://gml.noaa.gov/grad/solcalc/solareqns.PDF)

For rendering, the EoT correction shifts the sun's longitude by at most ±4° (15°/hour × 16/60 hour). This is physically significant for accurate terminator placement but can be omitted for a first approximation.

### 2.4 Hour angle and solar time

The **solar hour angle** (H) defines where the sun sits longitudinally — it is zero at solar noon and increases westward at 15°/hour (360°/24h).

Steps from UTC to hour angle for a given longitude:

```
LSTM    = 15° × UTC_offset_hours               (Local Standard Time Meridian)
TC       = 4·(longitude_deg − LSTM) + EoT      (time correction, minutes)
LST      = UTC_hours + TC / 60                 (Local Solar Time, decimal hours)
H        = 15° × (LST − 12)                   (hour angle; negative before noon, positive after)
```

For a **world-space sun direction** that does not depend on observer location, the subsolar point is more useful: it is the geographic point directly beneath the sun. Its coordinates are:

```
subsolar_lat  = δ                              (equals declination)
subsolar_lon  = −15 × (UTC_hours − 12 + EoT/60)   (degrees; prime meridian = 0°)
```

Source: [HandWiki — Subsolar Point](https://handwiki.org/wiki/Astronomy:Subsolar_point)

The subsolar longitude formula assumes the sun is at the anti-meridian (−180°/+180°) at UTC midnight and at 0° longitude at UTC noon, then shifts by the EoT correction. The factor −15 converts hours to degrees.

---

## 3. Converting to a 3D Direction Vector

### 3.1 Coordinate frame convention

For a globe renderer with:
- Y axis pointing to the North Pole
- X axis pointing toward 0° longitude / 0° latitude (prime meridian equator)
- Z axis completing a right-hand system (pointing toward 90°W)

the sun direction unit vector from subsolar latitude φ and subsolar longitude λ (both in radians) is:

```
sun_x =  cos(φ) × cos(λ)
sun_y =  sin(φ)
sun_z = −cos(φ) × sin(λ)
```

This is the standard spherical-to-Cartesian conversion for geographic coordinates on a unit sphere. Source: [Hour angle — Wikipedia](https://en.wikipedia.org/wiki/Hour_angle), [Equatorial coordinate system — Wikipedia](https://en.wikipedia.org/wiki/Equatorial_coordinate_system)

### 3.2 Complete formula chain

Given `day_of_year` (integer, 1–365) and `utc_hours` (float, 0.0–24.0):

```
// Declination
B_deg    = 360.0/365.0 * (day_of_year - 81)
decl_deg = 23.44 * sin(to_radians(B_deg))

// Equation of time (minutes)
eot_min  = 9.87 * sin(to_radians(2 * B_deg))
         - 7.53 * cos(to_radians(B_deg))
         - 1.5  * sin(to_radians(B_deg))

// Subsolar coordinates
sub_lat_deg = decl_deg
sub_lon_deg = -15.0 * (utc_hours - 12.0 + eot_min / 60.0)

// 3D sun direction (Y-up, X toward prime meridian equator)
phi = to_radians(sub_lat_deg)
lam = to_radians(sub_lon_deg)

sun_x =  cos(phi) * cos(lam)
sun_y =  sin(phi)
sun_z = -cos(phi) * sin(lam)
// sun_dir = normalize(vec3(sun_x, sun_y, sun_z))  // already unit length
```

The result is a unit vector pointing from Earth toward the sun in world space.

### 3.3 J2000-based low-precision alternative

For slightly better accuracy (~0.01°) without full VSOP87, the following uses Julian centuries T from J2000.0:

```
JD  = Julian Day Number for the datetime
T   = (JD − 2451545.0) / 36525.0           (Julian centuries from J2000.0)
M   = 357.52910 + 35999.05030·T            (mean anomaly, degrees)
L0  = 280.46645 + 36000.76983·T            (geometric mean longitude, degrees)
λ   = L0 + 1.9146·sin(M) + 0.020·sin(2M)  (ecliptic longitude, degrees)
ε   = 23.439 − 0.013·T                     (obliquity, degrees)
δ   = arcsin(sin(ε) × sin(λ))              (declination, degrees)
```

Right ascension α (in degrees) from this λ and ε gives a more accurate subsolar longitude via Greenwich Mean Sidereal Time (GMST). For visual rendering the simpler day-of-year approach is sufficient. Source: [Grokipedia — Subsolar point](https://grokipedia.com/page/Subsolar_point)

---

## 4. Terminator Geometry

The **terminator** is the great circle on Earth's surface where the sun is at the horizon (solar elevation = 0°). It is geometrically the set of points where the dot product of the surface normal with the sun direction equals zero.

For rendering:
- Points with `dot(normal, sun_dir) > 0` are in daylight.
- Points with `dot(normal, sun_dir) < 0` are in night.
- The terminator is `dot(normal, sun_dir) == 0`.

### 4.1 Physical twilight width

The real terminator is not sharp. Twilight zones defined by solar depression angle:
- **Civil twilight:** Sun 0°–6° below horizon (~0–660 km wide band at equator)
- **Nautical twilight:** Sun 6°–12° below horizon
- **Astronomical twilight:** Sun 12°–18° below horizon

The penumbral transition due to Earth's finite angular diameter of the sun (~0.53°) plus atmospheric refraction (~0.5°) adds roughly 55–115 km to the transition zone, corresponding to a dot-product range of roughly ±0.01 in NdotL units.

For rendering, a twilight band spanning `NdotL` from about −0.1 to +0.1 covers civil twilight realistically. Source: [Terminator (solar) — Wikipedia](https://en.wikipedia.org/wiki/Terminator_%28solar%29), [The Geography Hub — Terminator Line](https://thegeographyhub.com/the-terminator-line-earths-twilight-zone/)

---

## 5. Shader Implementation Patterns

### 5.1 Core pattern (dot product + smoothstep)

The standard approach used across WebGL, Three.js, and GLSL globe renderers:

```glsl
// Fragment shader inputs
uniform sampler2D dayTexture;
uniform sampler2D nightTexture;
uniform vec3 sunDirection;    // unit vector in world space

varying vec2 vUv;
varying vec3 vNormal;

void main() {
    vec3 dayColor   = texture2D(dayTexture,   vUv).rgb;
    vec3 nightColor = texture2D(nightTexture, vUv).rgb;

    float NdotL = dot(normalize(vNormal), sunDirection);
    float blend = smoothstep(-0.05, 0.05, NdotL);

    gl_FragColor = vec4(mix(nightColor, dayColor, blend), 1.0);
}
```

Source: [Geeks3D HackLAB — Earth Day to Night Transition](https://www.geeks3d.com/hacklab/20200210/demo-earth-day-to-night-transition/)

The `smoothstep(-0.05, 0.05, NdotL)` creates a transition band spanning ±0.05 in cosine space, corresponding to roughly ±2.9° of solar elevation — a narrow but soft terminator.

### 5.2 Sharpened terminator (multiply before clamp)

An alternative that produces a sharper but still smooth terminator:

```glsl
float NdotL = dot(normalize(vNormal), sunDirection);
float c = clamp(NdotL * 10.0, -1.0, 1.0);  // sharpen by 10×
float blend = c * 0.5 + 0.5;               // remap [−1, 1] → [0, 1]
vec3 color = mix(nightColor, dayColor, blend);
```

The multiplication factor (here 10.0) controls transition width. Source: [WebGL Fundamentals — Day/Night Globe](https://webglfundamentals.org/webgl/lessons/webgl-qna-show-a-night-view-vs-a-day-view-on-a-3d-earth-sphere.html)

### 5.3 Logistic (sigmoid) terminator

A sigmoid (logistic) function is a smooth alternative to smoothstep that gives a softer, more physically motivated gradient:

```glsl
float NdotL = dot(normalize(vNormal), sunDirection);
float blend = 1.0 / (1.0 + exp(-20.0 * NdotL));   // logistic, k=20
vec3 color = mix(nightColor, dayColor, blend);
```

The coefficient (here −20.0) controls steepness; larger magnitude = sharper transition. A gentler value (−7.0) with a slight horizon offset simulates a diffuse twilight hemisphere:

```glsl
float hemisphere = 1.0 / (1.0 + exp(-7.0 * (NdotL + 0.1)));
```

Source: [Sangi Lee — Create a Realistic Earth with Shaders](https://sangillee.com/2024-06-07-create-realistic-earth-with-shaders/)

### 5.4 WGSL translation

WGSL (used in wgpu) does not have `exp` as a built-in in early versions but does have `smoothstep`. The standard WGSL pattern:

```wgsl
let n_dot_l: f32 = dot(normalize(in.world_normal), sun_dir);
let blend: f32 = smoothstep(-0.05, 0.05, n_dot_l);
let color: vec3<f32> = mix(night_color, day_color, blend);
```

`smoothstep`, `mix`, and `dot` are all available as WGSL built-ins. Source: [WebGPU Fundamentals — WGSL Function Reference](https://webgpufundamentals.org/webgpu/lessons/webgpu-wgsl-function-reference.html)

### 5.5 Combining with diffuse lighting

For a more physical look, the day texture can also be modulated by `NdotL` to add shadowing across the dayside:

```wgsl
let n_dot_l: f32 = dot(normalize(in.world_normal), sun_dir);
let diffuse: f32 = max(0.0, n_dot_l);
let blend: f32   = smoothstep(-0.1, 0.1, n_dot_l);
let color: vec3<f32> = mix(night_color, day_color * diffuse, blend);
```

This darkens the day texture toward the terminator edge, simulating the low sun angle at dawn/dusk.

---

## 6. Coordinate Frame Notes for This Renderer

The current renderer uses a UV sphere where:
- The sphere is centered at the origin.
- The UV map is equirectangular: U=0 maps to −180° longitude, U=1 to +180°, V=0 to +90° latitude (north pole), V=1 to −90° (south pole).
- The North Pole maps to the +Y axis.
- 0° longitude / 0° latitude maps to +X.
- 90°E longitude / 0° latitude maps to −Z (right-hand rule with Y-up, X-east).

This is the standard geographic coordinate convention. The sun direction vector formula in Section 3.1 is written for this convention.

The sun direction vector is a **world-space uniform** — it does not change per-vertex. It must be recomputed on the CPU each frame (or when time changes) and uploaded as a uniform to the shader.

---

## 7. Rust Implementation Notes

No external crate is required for the simplified formulas. The computation is a handful of trigonometric operations available in Rust's `f32`/`f64` standard library (`f32::sin`, `f32::cos`, `f32::to_radians`).

For reference, the `sun` crate ([docs.rs/sun](https://docs.rs/sun)) exposes `pos(unixtime, lat, lon)` returning azimuth and altitude angles — it is observer-dependent and returns horizontal coordinates, not a world-space direction vector, so it is not a natural fit for this use case.

The `spa-rs` crate ([GitHub — spa-rs](https://github.com/frehberg/spa-rs)) implements the full NREL SPA algorithm in Rust — accurate but heavyweight for this purpose.

A self-contained function producing a `glam::Vec3` sun direction from a `chrono::DateTime<Utc>` is practical and requires no additional dependencies beyond `chrono` (already likely present) and `glam` (already in the project).

---

## 8. Sources

1. [NREL Solar Position Algorithm (SPA)](https://midcdmz.nrel.gov/spa/) — authoritative high-precision reference
2. [NOAA General Solar Position Equations (PDF)](https://gml.noaa.gov/grad/solcalc/solareqns.PDF) — compact Fourier-series approximations, NOAA Global Monitoring Division
3. [PVEducation — Declination Angle](https://www.pveducation.org/pvcdrom/properties-of-sunlight/declination-angle) — simplified declination formula with derivation
4. [PVEducation — Solar Time](https://www.pveducation.org/pvcdrom/properties-of-sunlight/solar-time) — equation of time compact form, hour angle relationship
5. [PVPMC Basic Solar Position Models](https://pvpmc.sandia.gov/modeling-guide/1-weather-design-inputs/sun-position/basic-solar-position-models/) — Spencer (1971) Fourier series
6. [HandWiki — Subsolar Point](https://handwiki.org/wiki/Astronomy:Subsolar_point) — subsolar latitude/longitude formula with EoT correction
7. [Grokipedia — Subsolar Point](https://grokipedia.com/page/Subsolar_point) — J2000-based low-precision ecliptic longitude approach
8. [Geeks3D HackLAB — Earth Day to Night Transition](https://www.geeks3d.com/hacklab/20200210/demo-earth-day-to-night-transition/) — `smoothstep(-0.05, 0.05, NdotL)` pattern, GLSL source
9. [WebGL Fundamentals — Day/Night Globe Shader](https://webglfundamentals.org/webgl/lessons/webgl-qna-show-a-night-view-vs-a-day-view-on-a-3d-earth-sphere.html) — sharpen-then-remap pattern, full shader source
10. [Sangi Lee — Create a Realistic Earth with Shaders](https://sangillee.com/2024-06-07-create-realistic-earth-with-shaders/) — logistic sigmoid terminator, two-layer approach
11. [WebGPU Fundamentals — WGSL Function Reference](https://webgpufundamentals.org/webgpu/lessons/webgpu-wgsl-function-reference.html) — WGSL built-in availability (smoothstep, mix, dot)
12. [Terminator (solar) — Wikipedia](https://en.wikipedia.org/wiki/Terminator_%28solar%29) — physical twilight zone widths and angular extents
13. [The Geography Hub — Terminator Line](https://thegeographyhub.com/the-terminator-line-earths-twilight-zone/) — penumbral zone width ~55–115 km
14. [Hour Angle — Wikipedia](https://en.wikipedia.org/wiki/Hour_angle) — definition and relationship to solar time
15. [Equatorial Coordinate System — Wikipedia](https://en.wikipedia.org/wiki/Equatorial_coordinate_system) — spherical-to-Cartesian conversion
16. [GitHub — rust-sun](https://github.com/flosse/rust-sun) — Rust sun position crate (observer-centric, not directly applicable)
17. [GitHub — spa-rs](https://github.com/frehberg/spa-rs) — full NREL SPA in Rust (high precision, heavyweight)

---

## 9. Confidence and Gaps

**Confidence: High** for:
- Declination formula (multiple independent sources agree on the same constants)
- Hour angle / solar time relationship (standard astronomical definition)
- Subsolar point formula (well-documented, matches physical interpretation)
- Shader blending pattern (multiple working implementations found)
- WGSL built-in availability

**Confidence: Medium** for:
- The exact smoothstep edge values (−0.05, 0.05) — these are empirical and vary by implementation; ±0.05 is a common choice but not uniquely "correct"
- EoT compact formula constants — accurate to ~0.5 min, which shifts the terminator by ~0.1°

**Gaps / not fully verified:**
- The sign convention for subsolar longitude (east-positive vs. west-positive) should be verified against known reference dates (e.g., UTC noon on March equinox should give subsolar point at 0°N, 0°E)
- Behavior near the poles (gimbal-like issues if `cos(phi)` approaches zero) does not affect the direction vector but may affect texture mapping
- The renderer's current world-space axis orientation should be confirmed before wiring up the direction vector formula
