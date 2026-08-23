# Celestial Bodies Research: Stars, Sun, Moon, Planets, Milky Way

**Date:** 2026-08-23
**Status:** Research complete, recommendations made. Revised 2026-08-23 after review: the Sun expanded from a sprite footnote into its own researched section (5), phasing reordered to put the Sun second, the HYG catalog choice confirmed, and attribution assigned to a new About window. Plan documents exist for all four phases; see section 11.

This document assesses how to render the sky behind the Earth: an astronomically correct star field, the Sun as a visible object with a perceptually grounded glare, the Moon at its true position and phase, the naked-eye planets, and the Milky Way as a diffuse background. It covers what the codebase already provides, what the hard problems are, which options were considered, and what is recommended. External facts (catalog contents, asset resolutions, licenses, rendering technique references) were researched against the primary sources on 2026-08-23; the Astronomy Engine API claims were verified against the vendored C header in `astronomy-engine-bindings` 2.1.19.

## 1. Scope and constraints

In scope: background stars, the Sun with glare, the Moon (position, phase, orientation), the five naked-eye planets, and the Milky Way. Eclipse visualization is sketched but deferred to the backlog.

Constraints, from the project goals:

1. Real-time rendering in the desktop app must remain performant.
2. Initial scene loading must remain fast.
3. Memory footprint must remain within reasonable bounds.
4. Positions must be correct, but not absolutely precise. Arcminutes are fine; degrees are not.
5. Realistic by default, with deliberate artistic license: stars and the Moon stay visible even when a physically exposed camera would lose them against the day side, the same way the night side is already rendered far brighter than reality.

## 2. What we already have

### 2.1 The world frame, and the one thing the sky needs that it lacks

The renderer's world frame is Earth-fixed: +Y is the north pole, +Z pierces the equator at the prime meridian, +X at 90 degrees East. The Earth mesh never rotates; time of day enters the scene only through the sun direction. `scene::sun` computes that direction by asking Astronomy Engine for the sun's right ascension and declination referred to the equator of date, subtracting Greenwich apparent sidereal time to get the subsolar point, and converting to a unit vector. This is the correct architecture to extend: celestial objects should also be positioned by converting out of an inertial frame into this Earth-fixed frame, once per refresh, on the CPU.

One detail found during this assessment: `sun_direction_from_time` builds its observer with `Astronomy_MakeObserver(0.0, 0.0, 0.0)`, and the comment calls that geocentric. It is actually a topocentric observer on the surface at 0N 0E. For the sun the difference is at most 8.8 arcseconds of parallax, which is irrelevant. It would not be irrelevant for the Moon, where surface parallax reaches about a degree, so the Moon must not be positioned through the same `Astronomy_Equator` call pattern. The geocentric functions (`Astronomy_GeoMoon`, `Astronomy_GeoVector`) are the right tools, and they exist in the bindings.

### 2.2 The renderer

One render pass draws, in order: the Earth sphere (opaque, writes depth), the cloud shell (alpha blended, tests depth, no write), the Rayleigh shell (premultiplied alpha, simulating both in-scattering and extinction), and two additive nightglow shells. All five draws share one 64x64 UV sphere mesh, one 208-byte uniform buffer, and a bind group with one uniform binding, two texture bindings, and a sampler. The pass clears to (0.02, 0.02, 0.05), which is currently the entire sky. The target is `Rgba8Unorm` with a `Depth32Float` buffer and optional MSAA. Dirty checking compares a quantized `ParamsDigest` plus the render size and the sun direction quantized to milliradians; the sun moves enough to trip that on every 120-second refresh, so a live view already re-renders about every two minutes and no more often.

### 2.3 The camera, and two numbers that matter

The orbital camera sits 1.5 to 80 Earth radii from the origin with a 20 degree vertical field of view, near plane 0.1, far plane 100. Two consequences for this work:

- At 20 degrees over a 2160-pixel-tall render, one pixel is 33 arcseconds. This number decides the star rendering approach (section 4.1).
- The camera's maximum distance (80) is beyond the Moon's true distance (55.9 to 63.8 Earth radii), and the far plane (100) is closer than the worst-case eye-to-Moon distance (about 144). Placing the Moon at true scale requires raising the far plane (section 6.1).

### 2.4 Asset and parameter machinery that carries over directly

Everything a new celestial layer needs already has a proven pattern:

- Texture slots with async decode through the latest-value mailbox, generation stamping, and per-slot GPU labels feeding the memory report. Adding a slot means growing `texture_paths`, `SLOT_LABELS`, and the mailbox slot count together (the engine asserts the counts agree).
- The texture resolution setting acts as a width cap with a disk cache of halved copies, so a new source texture participates in memory management for free.
- `SceneParams` plus `ParamsDigest` plus the table-driven digest test make new shader parameters mechanical: `.slint` property, config field, params field, digest entry, uniform field, WGSL.
- Golden tests pin a fixed custom datetime, so sky content is exactly as deterministic as the terminator already is.
- The injected clock means the soak test's fourteen simulated days would rotate the star field and orbit the Moon realistically, for free.

### 2.5 The astronomy library covers everything

The vendored Astronomy Engine 2.1.19 header (the bindings crate runs bindgen over all of it) provides every computation this document needs, so no new dependency is required:

| Need | Function |
|---|---|
| Moon position, geocentric J2000, AU | `Astronomy_GeoMoon` (also `Astronomy_Libration` for distance and apparent diameter) |
| Planet positions, geocentric J2000, light-time corrected | `Astronomy_GeoVector` with aberration |
| Planet and Moon apparent magnitude | `Astronomy_Illumination` (Saturn includes ring tilt) |
| Moon orientation (north pole + prime meridian angle, IAU model) | `Astronomy_RotationAxis` |
| Precession + nutation matrix, J2000 to equator-of-date | `Astronomy_Rotation_EQJ_EQD` |
| Earth rotation angle | `Astronomy_SiderealTime` (already in use) |
| Galactic to equatorial, if a galactic-frame asset is ever used | `Astronomy_Rotation_GAL_EQJ` |
| Eclipse predictions, for later | `Astronomy_SearchLunarEclipse`, `Astronomy_SearchGlobalSolarEclipse` |

Stars need no ephemeris at all: they are fixed directions in the J2000 frame, which is the point of section 3. (`Astronomy_DefineStar` exists but supports only eight user stars; it is for ephemeris-style queries and is not the tool for a catalog.)

## 3. The one architectural addition: a sky frame transform

Everything in the sky is naturally expressed in J2000 equatorial coordinates (EQJ): star catalogs publish J2000 positions, and `Astronomy_GeoMoon` / `Astronomy_GeoVector` return EQJ vectors. The Earth-fixed world frame differs from EQJ by precession, nutation, and the sidereal rotation. So the core of this whole feature set is one 3x3 rotation matrix, recomputed whenever the sun direction is:

```
R_world_from_eqj = P_axes * R_y(GAST) * M_eqd_from_eqj
```

where `M_eqd_from_eqj` comes from `Astronomy_Rotation_EQJ_EQD`, the sidereal rotation uses the same `Astronomy_SiderealTime` value the sun path already computes, and `P_axes` is the fixed permutation onto the renderer's axes (equatorial x to world +Z, equatorial y to world +X, pole to world +Y, matching how `sun.rs` converts the subsolar point today).

Everything follows from this matrix:

- Star vertex data stays static in EQJ forever; the matrix is a uniform and the GPU does the rest. Zero per-star CPU work per frame.
- The Moon and planet positions are a handful of CPU vector rotations per refresh.
- A Milky Way panorama in the equatorial frame uses the same matrix.

This slots into the engine as a `SkyState` computed next to the sun direction (a pure function of `astro_time_t` in `scene`, unit-testable on every platform without a GPU): the rotation matrix, the moon position in world units and its orientation, per-planet direction and magnitude, and the sun's screen-space glare inputs (section 5.4). Cost is a few dozen trigonometric series evaluations, well under a millisecond, once per 120-second tick and once per parameter push, which is the cadence the sun already has.

Correctness is cheap to verify because the sun provides a cross-check: `R_world_from_eqj` applied to the sun's geocentric EQJ unit vector must agree with the existing `sun_direction_from_time` to within a few hundredths of a degree (the residual is the topocentric offset noted in 2.1). A second test: Polaris (RA 2h31m, Dec +89.26 degrees) must land within about a degree of world +Y. These two tests pin the matrix's handedness, its permutation, and its sidereal term, which is where sign mistakes actually happen.

Dirty checking extends naturally: `FrameState` gains the quantized moon direction and a quantized sidereal angle (or the matrix's quantized basis vectors). The sky rotates 15 degrees per hour, which trips a milliradian quantization every 14 seconds of real time; since the engine only evaluates on its 120-second sun schedule and on pushes, the render cadence does not change.

## 4. The star field

### 4.1 Why stars must be geometry, not a texture

The tempting shortcut is a prebaked full-sky texture with the stars already in it. The pixel math rules it out. At our 20 degree field of view a 4K render resolves 33 arcseconds per pixel. An equirectangular sky texture spans 360 degrees, so at the equator it delivers 158 arcsec/px at 8k width, 79 at 16k, and 40 at 32k. Even the 32k texture is coarser than the screen, so every star becomes a resampled smudge a few pixels wide, and the 32k source (NASA's, section 8) is a 1.4 GB EXR that would cost about 2.7 GiB of VRAM as RGBA8 with mips. A texture can never make stars look like points at this field of view, and the memory constraint kills it independently.

Procedural stars (hash noise) are cheap but fail the correctness requirement outright.

So: resolvable stars are analytic geometry from a real catalog, rendered as small screen-space sprites. This is what planetarium renderers do, it is trivially cheap at our star counts, and it is exact at any resolution and field of view. The diffuse Milky Way, which is the one part of the sky that really is a low-frequency image, stays a texture (section 8).

### 4.2 Catalog choice

Four candidates, facts verified against the actual catalog files:

| | Yale BSC5 | Hipparcos | HYG v4.4 | Tycho-2 |
|---|---|---|---|---|
| Stars total | 9,110 | 118,218 | 119,614 | 2,539,913 |
| Stars mag <= 6.5 | 8,404 | 8,874 | 8,921 | n/a |
| Stars mag <= 7.0 | 9,050 | 15,544 | 15,598 | n/a |
| Epoch/frame | J2000 | ICRS at J1991.25 | J2000 | ICRS ~J1991.25 |
| B-V color | 96.5% | 98.9% | 98.4% | BT/VT proxy |
| Raw size | 1.6 MiB | 51 MiB | 32 MiB CSV | ~160 MB gz |
| License | informal, citation-based, no formal grant | ESA archive terms; commercial use gated behind an ESA authorization request | CC BY-SA 4.0, explicit | same CDS terms as Hipparcos |

Recommendation, confirmed in review on 2026-08-23: HYG v4.4 (now hosted on Codeberg, `astronexus/hyg`), truncated at magnitude 7.0 into a compact binary blob at build-preparation time. HYG is the only candidate with an explicit formal license; it is already at J2000; its B-V coverage is essentially complete; and it includes proper motions, so the bake step can propagate positions to a current fixed epoch (this matters at the margin: Alpha Centauri has moved about 1.6 arcminutes since J2000, which is 3 pixels at 4K). Baking to magnitude 7.0 (15,598 stars) rather than the naked-eye 6.5 costs nothing (about 250 KiB at 16 bytes per star: a unit vector as 3x f32, magnitude and B-V quantized to a byte each, two spare bytes) and leaves the display cutoff a runtime choice.

The CC BY-SA 4.0 share-alike condition applies to the derived data blob, not to the GPL code that reads it. The handling: keep the generator (an offline `xtask` subcommand or script) in the repo, ship the blob with a sidecar note naming HYG v4.4 and CC BY-SA 4.0, and surface the attribution in the app through the About window (section 9), which this feature set introduces. Combining CC BY-SA data with GPLv3 code in one distribution is common practice but not a formally settled question; the fallback, if it ever needed to be exercised, is Yale BSC5 on its informal citation-based terms.

The blob should be embedded with `include_bytes!`: 250 KiB of binary growth, zero I/O, zero failure modes, stars present from the first frame.

### 4.3 Rendering technique

wgpu's point primitive rasterizes exactly one pixel (WGSL has no point-size output), so stars are instanced quads: one instance per star, four vertices expanded in clip space by a pixel-sized offset. The vertex shader takes the star's EQJ direction, applies `R_world_from_eqj` and the view rotation with w = 0 (directions at infinity need no translation and are immune to camera position), projects, then offsets the corners. The same post-projection screen offset the globe uses must be applied so the sky pans with the framing. Depth is pinned to the far plane and the draw is depth-tested but happens first, so the Earth simply covers stars behind it, and the atmosphere shells, drawn later with their existing blending, tint and dim stars at the limb exactly the way they should (the Rayleigh shell's extinction term already darkens what is behind the haze).

Brightness is where realism needs the project's usual artistic hand. True flux spans a factor of about 2,400 from Sirius to magnitude 7, which an 8-bit target cannot represent. The standard planetarium approach works here: render every star as a small Gaussian point-spread function of fixed size (2 to 3 pixels sigma) whose amplitude comes from a compressed flux mapping (a power law on `10^(-0.4 mag)` with a user gain), and let only the brightest few stars grow slightly in radius. Two practical reasons for the 2-plus-pixel PSF beyond looks: it keeps golden images stable across adapters (single-pixel geometry is exactly what cross-adapter rasterization differences chew on), and it prevents twinkling between wallpaper re-renders as the sky rotation shifts sprite centers across pixel boundaries. Blending is additive so overlapping PSFs sum.

Color comes from B-V at bake time: convert to an effective temperature with the standard approximation, then to sRGB, then desaturate toward white (fully saturated blackbody colors look wrong at point sizes). Store the resulting RGB per star in the blob so the shader does no color math.

Cost: one draw call, ~15k instances of a handful of covered pixels each. Negligible on real GPUs and cheap even on WARP and lavapipe, where the covered-pixel total (a few hundred thousand fragments at most) is small next to the Earth plus four full overlay spheres the pass already rasterizes.

Parameters: a star intensity slider (0 disables and skips the draw) and optionally a magnitude-limit slider. Both are ordinary shader parameters and follow the digest table convention. Stars default on, per the artistic constraint that the sky is visible even on day-side views; no exposure model is attempted.

## 5. The Sun

### 5.1 What the sun actually looks like from space, and why that frames the design

In vacuum there is nothing to scatter light, so a physically honest sun is a clipped white disk 0.53 degrees across (about 56 pixels in a 4K render at our field of view) on a black sky. Everything beyond the disk in real space photography is the camera: aperture-blade diffraction spikes (the five straight blades of the Apollo Hasselblads' Zeiss lenses produce the ten-point stars in those frames), internal-reflection ghosts, sensor blooming. A human eye substitutes its own artifacts: perceptual glare. This reframes the question usefully. There is no scene-side "correct" sun rendering to approximate; there is only a choice of simulated observer, which is exactly the artistic license the constraints grant. A bare 56-pixel white circle would read as a second, wrong moon; what makes a sun read as the sun is the observer's glare around it.

What human glare consists of is settled perceptual literature. Spencer, Shirley, Zimmerman, and Greenberg (SIGGRAPH 1995), still the standard reference, decompose it into three components: bloom or veiling luminance, a broad wavelength-neutral haze from intraocular scattering that extends tens of degrees and causes the contrast loss around a bright source; the ciliary corona, the fine radiating needles people mean when they say "rays", caused by diffraction in the ocular media and extending out to several degrees (up to about 8); and the lenticular halo, a subtle colored ring from the lens fibers acting as a radial diffraction grating, sitting at roughly 2.5 to 3.5 degrees with a blue inner and red outer edge. A property of that model worth exploiting: glare geometry depends only on the source's apparent brightness and screen position, never on scene distance, so the whole effect legitimately lives in screen space.

### 5.2 Technique options

Three families were evaluated against our architecture: a single forward pass into an LDR `Rgba8Unorm` target, no post-processing chain, one bright source whose position and occlusion are known analytically, software adapters that must keep up, and a still image as the product.

(a) Screen-space post-processing. This covers bright-pass Gaussian bloom, John Chapman's pseudo lens flare (downsample, threshold, sample ghosts by center-mirrored UVs plus a halo ring and per-channel chromatic offsets), and FFT convolution bloom against a real aperture PSF (Unreal's convolution bloom; the physically serious end). All of them operate on the rendered frame, need a downsample-blur-threshold chain, and want HDR input to threshold meaningfully; in our LDR target the sun disk and a bright cloud both saturate at 255 and would flare identically. Their real selling point is generality: every bright pixel flares, which is why SpaceEngine chose screen-space (its flares "inherit the shape of the light source"). We have exactly one object needing the effect and no post-processing infrastructure to amortize; buying the architecture for one light is the wrong trade. Rejected for this feature. If an HDR intermediate ever lands (backlog), convolution bloom would subsume parts of this section.

(b) Billboard composite, textured. The classic sprite flare (Kilgard's OpenGL lens flare, the ThinMatrix tutorial lineage, and what planetarium software ships: Stellarium composites its sun from a core disk plus halo textures, SpaceEngine layers a star-rays texture over its screen-space ghosts): draw screen-aligned quads at the light's projected position, additively blended, intensity faded by an occlusion factor. Fits our architecture, but the textures are one more set of shipped assets, and at our quality bar they would need authoring iterations.

(c) Billboard composite, procedural. Same draw structure as (b), one large screen-aligned quad drawn last with depth ignored, but the fragment shader evaluates the glare analytically as a radial profile in angular coordinates instead of sampling textures. Every component of the Spencer model is one term with one falloff; the needle structure comes from deterministic angular hash noise; no assets, no passes, fully deterministic for golden tests, and each term maps onto a slider. Recommended.

### 5.3 Recommended composition

All terms are functions of the angular distance from the sun's center, computed per fragment from the view ray so the profile is field-of-view-correct rather than pixel-fixed, and of the angle around it. Layered from the inside out:

1. Core disk: clipped pure white at the true 0.53 degree diameter, edge softened over a pixel or two. The only fully saturated white object in the scene, which by itself separates the sun from Venus and Sirius.
2. Bloom (veiling glare): a broad radial falloff, inverse-power rather than Gaussian so it stays bright close in but keeps a long faint tail, reaching ten degrees or more at low alpha, near-white with an optional slight warm bias. This term does most of the "outshines everything" work: nearby stars and the Milky Way visually drown in it without any exposure model, and it sells brightness the way human vision reports it.
3. Ciliary corona: fine radial needles modulating the inner few degrees, dozens of thin lobes of varying length generated from deterministic angular hash noise, low contrast. This is the component people describe as "rays" when looking toward the sun, and the research is clear it belongs to the eye, not the camera, so it rightly appears in the default look.
4. Lenticular halo: a faint ring at about 3 degrees, blue-tinged inner edge, red-tinged outer edge, very low alpha. One smoothstep band with a radial color ramp, and it is the detail that makes the composite read as an observer's glare rather than a fog sprite.
5. Camera mode, optional and off by default: an N-point aperture starburst (even blade counts give N spikes, odd give 2N; five blades and their ten spikes are the Apollo look) and two or three ghost blobs placed along the axis from the sun through the screen center, the classic flare geometry. Spikes stay screen-fixed: they belong to the imaging device, and our tilt control rolls the camera, so a device-attached pattern is precisely one that does not rotate in frame. Ghosts are the reason this ships as a toggle rather than a default: in a still wallpaper they read to some eyes as smudges on the display, and the eye-glare default cannot produce them (eyes have no lens ghosts).

The wide faint gradients of term 2 are the textbook 8-bit banding case; the existing `dither()` in `sphere.wgsl` already solves exactly this for the nightglow shells and applies verbatim.

Parameters: `sun_glow` (master intensity, 0 skips the draw entirely), `sun_rays` (corona strength), `sun_flare` (camera mode strength, default 0). Whether the halo deserves its own slider is a screenshots-in-hand decision. All three follow the digest table convention. Together with the stars' capped brightness this settles the hierarchy the review asked for: the sun is the only object with a saturated core and a structured multi-degree glare, Venus is the brightest point sprite, and everything else falls in line by magnitude.

### 5.4 Occlusion, and the limb-sunrise moment

Game engines fade flares with hardware occlusion queries or depth-buffer sampling around the light's screen position because their occluders are arbitrary meshes, and screen-space variants famously pop when the occluder leaves the frame. Our only occluder is one analytic sphere at the origin, so the CPU computes the exact answer with no queries, no readbacks, and no edge cases: the angular separation between the sun direction and the Earth's limb, seen from the eye, gives the visible fraction of the disk, and every glare term scales with a smooth function of it. It is testable without a GPU, and it is strictly better than what the surveyed engines do (SpaceEngine's devlog documents upgrading toward exactly this behavior: smooth fading plus absorption by translucent occluders).

The transit band is where the analytic approach pays off visibly. While the line of sight to the sun passes through the atmosphere shell (between the solid limb and the Rayleigh radius), the same geometry yields a transit factor that shifts the glare warm and dims it: the ISS-sunrise look, a warm glare kindling at the limb before the white disk breaks free. The photographic reference is well documented: at grazing incidence the light path through the lower atmosphere is long, blue is scattered out, and what remains is a concentrated orange band with a thin blue line above it. An optional second-order polish, worth trying with screenshots: a forward-scattering boost in `fs_rayleigh` that raises the existing limb glow near the sun's azimuth, which the research shows is the standard cheap rim-term proxy for real scattering. The two effects are independent; the second can land later or never.

### 5.5 Engineering fit

One new pipeline: a screen-aligned quad sized to the glare extent, additive blend, depth test and write off, drawn last so the glare overlays the atmosphere shells (glare forms in the observer, so it layers over everything in the scene). `SkyState` grows the sun's projected position, the visible-disk fraction, and the transit factor; the shared uniform buffer grows those scalars plus the three parameters. The draw is skipped when `sun_glow` is 0, when the disk is fully occluded, and when the sun is behind the camera (projection w non-positive). No textures, no new passes, no slots, no budget change, no loading-time change. On the software adapters the cost is the quad's covered pixels once, with a handful of transcendentals per fragment, comparable to one atmosphere shell.

Tests: unit tests for the occlusion and transit function (fully visible, fully hidden, grazing, in-band); digest rows for the three parameters; two new goldens at the pinned datetime (sun in frame over the night side, sun grazing the limb). The composite is deterministic by construction, hash noise included.

## 6. The Moon

### 6.1 Position, scale, and the far plane

`Astronomy_GeoMoon` gives the geocentric EQJ position in AU; multiplying by 23,455 (Earth radii per AU) and rotating by `R_world_from_eqj` places the Moon in world units at its true distance of 55.9 to 63.8 Earth radii, with radius 0.2724. Placing it at true scale, rather than as a billboard, buys three things at once: the apparent size is automatically right from any camera distance, parallax is automatically right (the camera can be closer to the Moon than to the Earth), and occlusion in both directions is plain depth testing.

Two engine-level consequences. First, the far plane must grow from 100 to at least 200 (worst case eye-to-Moon distance is about 144); with a `Depth32Float` buffer and a 0.1 near plane this costs nothing measurable in precision. Second, the true-scale Moon subtends 0.52 degrees from Earth, about 56 pixels tall in a 4K render at our 20 degree FOV: real and recognizable, but small. The recommended artistic control is a moon size factor that scales only the radius (1x to about 4x), keeping direction, distance, phase geometry, and occlusion honest; at 4x the disk is about 220 pixels and cannot collide with the Earth (a 1.1 radius sphere 56 radii away clears it by a wide margin). Default 1x, per realistic-by-default. Scaling radius is preferred over pulling the Moon closer, which would corrupt parallax and occlusion for cameras beyond it.

### 6.2 Phase

Phase needs no phase computation at all: light the Moon sphere with the same `sun_dir` the Earth uses and the phase, its orientation on the disk, and its evolution over the month all come out correct, because the geometry is correct. The error from reusing Earth's sun direction at the Moon's position is at most 0.15 degrees (the Earth-Moon distance seen from 1 AU), invisible. The lunar terminator is much harder than Earth's (no atmosphere), so the moon fragment shader applies a narrow smoothstep on n dot l rather than reusing Earth's terminator width. A small "earthshine" floor keeps the dark portion faintly visible instead of letting a new moon vanish into black; this is both the artistic choice the constraints ask for and a real phenomenon.

A dedicated `fs_moon` entry point (in the same concatenated WGSL module) is cleaner than contorting `fs_main`: sample the color texture, apply the hard terminator, the earthshine floor, and a brightness control. No water, no fresnel, no night texture. The moon draw needs its own model matrix (position, scale, orientation), which becomes a second MVP in the shared uniform buffer alongside the fields that are already there.

### 6.3 Orientation

The Moon is tidally locked but librates by up to about 8 degrees. `Astronomy_RotationAxis(BODY_MOON)` returns the north pole direction (EQJ) and the prime meridian angle from the IAU/WGCCRE model, which is exactly the model matrix's rotation part; composed with `R_world_from_eqj` it orients the texture correctly, librations included. Even if this were a few degrees off it would be invisible on a 56-pixel disk, which makes it a low-risk piece of correctness that is cheaper to do right than to approximate.

### 6.4 Texture

The NASA CGI Moon Kit (SVS 4720) is the settled choice, and is what Celestia's current moon texture already derives from: an LROC WAC three-band natural-color composite, equirectangular, centered on the near side in the Mean Earth frame (so texture center faces Earth, matching how `Astronomy_RotationAxis` orients the prime meridian), public domain with a requested credit to NASA's Scientific Visualization Studio. Available from 1024x512 up to 27360x13680; the 2025 revision adds 16-bit sRGB TIFFs and float EXRs from 4096x2048 up.

Ship 2048x1024. The disk is at most a few hundred pixels on screen, and a 2048-wide equirectangular map delivers more than 5 texels per screen pixel even at moon scale 4x, so 4096 would be pure memory cost (43 MiB versus 10.7 MiB of VRAM with mips). The source goes through the same offline prep as the Earth textures (orient, encode as JXL, into `textures/` under Git LFS) and loads through a new texture slot with async decode, exactly like the day and night maps. Its 2048 width is at or below every value of the resolution setting, so the cap semantics never downscale it and it needs no special casing. Like clouds, the Moon is an overlay, not a requirement: it appears when decoded, is excluded from `textures_ready`/`textures_pending`, and never delays a wallpaper export.

### 6.5 Eclipses (deferred)

Both eclipse kinds become tractable once the Moon exists, and both are deferred to the backlog. Lunar eclipse: the moon shader tests whether the fragment-to-sun ray passes through the Earth (one sphere-ray test against the origin) and blends toward umbral red; cheap, self-contained. Solar eclipse on Earth: the Earth shader receives the Moon's world position and darkens fragments whose view of the sun is occluded, giving the traveling shadow spot. Astronomy Engine's eclipse search functions can later drive UI affordances (jump to the next eclipse).

## 7. Planets

The five naked-eye planets are directions plus magnitudes, refreshed with the sky state: `Astronomy_GeoVector` (light-time corrected, with aberration) rotated into the world frame, `Astronomy_Illumination` for apparent magnitude. They render through the star pipeline as extra instances appended after the catalog block, with fixed per-planet tints (Venus white, Mars salmon, Jupiter cream, Saturn pale yellow, Mercury gray) and brightness through the same compressed flux mapping; Venus at magnitude -4.9 simply lands at the mapping's bright end. Their true angular sizes are at most about an arcminute (Venus at closest), i.e. one to two pixels, so sprites are not an approximation but the correct rendering.

They are drawn as directions at infinity like the stars. The alternative, true scaled positions, would put Saturn 300,000 units out, far beyond any workable far plane. The cost of the infinity treatment is that planet directions are geocentric rather than camera-relative; the worst case (Venus at closest approach, camera 80 Earth radii off axis) is 0.7 degrees, typically far less, which the "correct, not absolutely precise" constraint explicitly tolerates. Update cadence is the sky refresh; planets move against the stars at most about a degree per day, so 120 seconds is absurdly sufficient.

## 8. The Milky Way background

### 8.1 Source

NASA SVS Deep Star Maps 2020 (SVS 4851) is the unambiguous winner, for a reason beyond license: it is the only free full-sky product that separates the diffuse Milky Way from the bright stars into distinct layers. Its `milkyway_2020` layer is built from Gaia DR2's faint unresolved starlight with the Hipparcos/Tycho bright stars deliberately excluded, and the companion `hiptyc_2020` layer holds only those bright stars. Since our bright stars come from the catalog as sprites, using the `milkyway_2020` layer means nothing is drawn twice, which was the main quality risk of any photographic panorama (ESO's Brunier panorama and the Gaia flux maps bake everything into one image; Stellarium's Mellinger panorama has bespoke, unconfirmed redistribution terms; Gaia's maps are CC BY-SA 3.0 IGO which drags a share-alike condition onto a mere background). The SVS maps are US public domain with a requested credit line ("NASA/GSFC/SVS" and "ESA/Gaia/DPAC"), and are published in both equatorial J2000 and galactic frames. Take the equatorial variant: it uses the same `R_world_from_eqj` matrix as everything else and needs no galactic rotation at runtime.

Preparation: the 4k (4096x2048) EXR is 34.3 MB, linear half-float HDR. An offline step (the same category of maintainer-run prep the Blue Marble textures already went through) tone-maps it to 8-bit and encodes JXL into `textures/`. The exposure choice made in that step is an artistic decision made once, with the in-app intensity slider covering taste at runtime.

### 8.2 Rendering

Two workable approaches:

- Reuse the UV sphere mesh, drawn from inside at a large radius with a dedicated pipeline. Familiar plumbing, but the radius has to thread between the maximum camera distance (80) and the far plane, and every future change to the zoom range or far plane has to re-check it. Viewed from inside, it also needs its cull mode flipped.
- A fullscreen triangle drawn first: reconstruct the view ray per pixel from the inverse projection and transposed view rotation (undoing the screen offset), rotate into EQJ by the transpose of `R_world_from_eqj`, convert direction to equirectangular UV, sample. Exact at every zoom, no geometry, no far-plane coupling.

The fullscreen triangle is recommended. The one known trap is the mip seam at the UV wrap column (the atan2 derivative discontinuity makes the hardware pick the smallest mip for one pixel column), fixed with the standard `textureSampleGrad` treatment; this is a well-understood implementation detail, not a risk.

The layer is optional exactly like clouds: a `milky_way_intensity` parameter (0 skips the draw), a new file-backed texture slot, async decode. At 4096x2048 it costs 42.7 MiB of VRAM with mips; the resolution cap applies through the existing halving cache, so the 2048 setting drops it to 10.7 MiB. With the sky populated by real content, the hardcoded clear color should move to near black, with the current faint blue becoming unnecessary; that is a one-line artistic decision to make with screenshots in hand.

On the software adapters the fullscreen sample is the most expensive single addition in this document (one texture fetch per output pixel), but it is strictly less work than the existing cloud shell draw, which rasterizes comparable coverage with blending on top of the Earth. If profiling ever disagrees, the layer is skippable by its own parameter.

## 9. Cross-cutting engineering

**Engine and params.** `SkyState` is computed in `scene` beside the sun direction, on the same schedule, from the same injected clock. New user-facing parameters (star intensity, magnitude limit if offered, sun glow, sun rays, sun flare, milky way intensity, moon size, moon brightness/earthshine) are ordinary `SceneParams` fields with the full digest-table treatment; the derived sky quantities (rotation matrix, moon position and orientation, planet directions, sun glare inputs) parallel the sun direction: not in the digest, compared in `FrameState` in quantized form.

**Uniforms.** The shared uniform struct grows by roughly a moon MVP (64 bytes), the sky rotation for stars and background (48 to 64 bytes), and the sun glare scalars plus the new parameters: from 208 bytes toward about 400, far below any limit that matters. Alignment discipline (`_pad` after every vec3) continues to apply.

**Pipelines and draw order.** Four new pipelines in the existing shader module, in pass order: milky way (fullscreen, drawn first, no depth), stars (instanced quads, additive, depth test against far, drawn second), moon (opaque, depth write, drawn beside the Earth), and the sun glare quad (additive, depth ignored, drawn last, occlusion applied analytically). Clouds and atmosphere shells keep their positions and their existing depth tests already do the right thing against the Moon in every configuration checked (Moon behind the limb: shells overdraw it, correct, the atmosphere is in front; Moon in front of Earth with the camera beyond it: shell fragments fail the depth test against the Moon, correct).

**Attribution surface.** The tray menu (Open, Refresh Now, Auto-refresh, Exit) gains an About entry opening a small window with the version and the credits this work introduces: the star catalog (HYG v4.4, CC BY-SA 4.0), the NASA SVS credit lines for the Moon kit and the star maps ("NASA's Scientific Visualization Studio", "NASA/GSFC/SVS", "ESA/Gaia/DPAC"), Astronomy Engine (MIT), and the existing asset sources (NASA Blue Marble and Black Marble, the live cloud composite). The settings window should reach the same About window through a small control, since windowed mode has no tray. This ships with phase A, because the star blob is the first asset whose license asks for visible attribution.

**Texture slots.** Two new file-backed slots (moon, milky way). The mailbox slot count, `SLOT_LABELS`, and the `EngineConfig::mailbox` assertion move together; the memory report's expected-texture table picks the new slots up automatically.

**Memory budget.** At defaults the additions are about 11 MiB (moon) + 43 MiB (milky way) of GPU memory plus transient decode buffers, against a measured budget of several GiB; `memory::private_bytes_budget` gains the milky way as a resolution-dependent term and the moon as a constant, and the budget tests get re-measured and re-pinned. Stars and the sun glare add no textures at all.

**Loading time.** Stars are embedded bytes: zero I/O, present on the first frame, and the sun glare is pure shader math. Moon and milky way load like clouds: asynchronously, appearing when decoded, never blocking the first frame or a wallpaper export.

**Far plane.** 100 to at least 200, one constant, required by the true-scale Moon.

**Missing assets.** A checkout without the LFS objects, or a user without the textures, gets exactly today's behavior plus stars and sun: sky layers that need files are simply absent, the same terminal-slot semantics clouds already have.

## 10. Testing

- Unit (all platforms, no GPU): matrix orthonormality and determinant; the sun cross-check against `sun_direction_from_time` (tolerance covers the topocentric residual); Polaris within a degree of +Y; moon distance within 55 to 64 Earth radii across a year sweep; one or two hardcoded moon positions checked against JPL Horizons values; catalog blob invariants (count, unit-length directions, magnitude range, no NaN); Venus magnitude brighter than -3 at a known date; the sun occlusion and transit function at its four regimes (visible, hidden, grazing, in the atmosphere band).
- GPU invariants: lit fraction of moon disk pixels tracks the illuminated fraction from `Astronomy_Illumination` within tolerance; a star known to be behind the Earth contributes nothing; a fully occluded sun contributes nothing.
- Golden: new scenes at the pinned datetime (night side with stars, sun in frame, sun grazing the limb, moon crescent, milky way band), which the fixed custom datetime makes fully deterministic; the pair-distinguishability test covers the new references automatically; the 2-plus-pixel star PSF and the sun's smooth analytic profile are what keep these stable across WARP, lavapipe, and Metal.
- Digest table: one row per new parameter, enforced by the existing test shape.
- The soak test needs no changes but gains meaning: fourteen simulated days now exercise a rotating sky, a moving Moon, and sunrise transits through the same dirty-check path.

## 11. Recommended phasing

Order confirmed in review on 2026-08-23. Each phase is independently shippable and visibly improves the wallpaper.

- **Phase A: sky transform, stars, planets, About window.** `SkyState`, `R_world_from_eqj`, the catalog bake tool and blob, the star pipeline, planets as appended instances (they share every mechanism), parameters and UI, and the About window carrying the catalog attribution. No new image assets, no LFS, no budget changes. Delivers the "star field" and "visible planets" roadmap items and transforms night-side framings. Plan: [2026-08-23-celestial-a-stars-plan.md](2026-08-23-celestial-a-stars-plan.md).
- **Phase B: the Sun.** The glare pipeline and composition of section 5, the analytic occlusion and transit factors in `SkyState`, the three parameters, and optionally the `fs_rayleigh` forward-scatter boost if screenshots justify it. No assets. Plan: [2026-08-23-celestial-b-sun-plan.md](2026-08-23-celestial-b-sun-plan.md).
- **Phase C: the Moon.** Asset prep, the moon slot and pipeline, orientation, the far plane change, the size and earthshine controls, and the slot-layout refactor that retires the clouds-slot pun. Delivers the "moon at correct position and phase" roadmap item. Plan: [2026-08-23-celestial-c-moon-plan.md](2026-08-23-celestial-c-moon-plan.md).
- **Phase D: Milky Way.** Asset prep from the SVS EXR, the fullscreen background pass, its intensity control, budget update, clear-color decision. Plan: [2026-08-23-celestial-d-milky-way-plan.md](2026-08-23-celestial-d-milky-way-plan.md).
- **Backlog, deliberately unplanned:** eclipse rendering (section 6.5); an HDR intermediate target with convolution bloom, which would make star and sun brightness physically grounded instead of artistically mapped and subsume parts of section 5; the Rayleigh forward-scatter boost if it does not land with phase B. These need further refinement before any of them gets a plan.

## 12. Open questions

1. Default intensities are taste decisions to make with rendered screenshots during implementation: the star magnitude default (6.5 versus deeper), the sun glare defaults, and whether the lenticular halo gets its own slider.
2. Resolved 2026-08-23: HYG v4.4 is the catalog, with attribution carried by the new About window and the blob's sidecar note; Yale BSC5 remains the documented fallback.
3. Whether the milky way layer defaults on at the low quality tier, where its 43 MiB is proportionally most noticeable.
4. Whether the moon default scale stays at the honest 1x or ships slightly enlarged; this document recommends 1x with the slider present.

## 13. Sources

- Astronomy Engine C API: vendored header in `astronomy-engine-bindings` 2.1.19 (verified locally).
- HYG v4.4: https://codeberg.org/astronexus/hyg (CC BY-SA 4.0; counts measured from the current CSV).
- Yale BSC5: https://cdsarc.cds.unistra.fr/viz-bin/cat/V/50 ; Hipparcos: https://cdsarc.cds.unistra.fr/viz-bin/cat/I/239 and https://www.cosmos.esa.int/web/esdc/terms-and-conditions
- NASA SVS Deep Star Maps 2020: https://svs.gsfc.nasa.gov/4851 (layers, frames, sizes, credit line).
- NASA CGI Moon Kit: https://svs.gsfc.nasa.gov/4720 (maps, resolutions, projection, credit line).
- ESO Brunier panorama (evaluated, not chosen): https://www.eso.org/public/images/eso0932a/ ; Gaia sky maps (evaluated, not chosen): https://sci.esa.int/web/gaia/-/the-colour-of-the-sky-from-gaia-s-early-data-release-3-equirectangular-projection
- Spencer, Shirley, Zimmerman, Greenberg, "Physically-Based Glare Effects for Digital Images", SIGGRAPH 1995: https://dl.acm.org/doi/10.1145/218380.218466 ; ciliary corona and lenticular halo angular data: https://journals.lww.com/jcrs/fulltext/2001/01000/ciliary_corona_and_lenticular_halo.11.aspx
- Ritschel et al., "Temporal Glare: Real-Time Dynamic Simulation of the Scattering in the Human Eye" (real-time approximations of the Spencer model): http://people.compute.dtu.dk/jerf/papers/TemporalGlare.pdf
- Kilgard, "Fast OpenGL-rendering of Lens Flares" (the classic sprite composite): https://www.opengl.org/archives/resources/features/KilgardTechniques/LensFlare/
- Chapman, "Pseudo Lens Flare" (screen-space alternative, evaluated and rejected): https://john-chapman.github.io/2017/11/05/pseudo-lens-flare.html
- SpaceEngine devlogs on its lens flare and smooth occlusion (comparative reference): https://spaceengine.org/news/blog170317/ and https://spaceengine.org/news/blog170415/
- Aperture diffraction spikes (blade-count geometry, Apollo Hasselblad example): https://en.wikipedia.org/wiki/Diffraction_spike
- ISS sunrise limb photography (transit-band color reference): https://science.nasa.gov/earth/earth-observatory/sunrise-from-the-station-152259/
