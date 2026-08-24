# Plan: Celestial B, the Sun

Phase B of [2026-08-23-celestial-bodies-research.md](2026-08-23-celestial-bodies-research.md), section 5. Builds on phase A's `SkyState` ([2026-08-23-celestial-a-stars-plan.md](2026-08-23-celestial-a-stars-plan.md)) and does not start until it has landed.

## Summary

The Sun as a visible object: a procedural glare composite (clipped white core disk at the true angular size, inverse-power bloom, ciliary-corona needles, lenticular halo ring, an optional off-by-default camera mode with aperture spikes and ghosts) drawn as one screen-aligned quad, last in the pass, with occlusion by the Earth and the atmosphere transit computed analytically on the CPU. Three new scene parameters. No assets, no new passes, no memory change.

## Stakes Classification

Low-medium. Everything is additive: one pipeline, one draw, pure shader math, CPU-side geometry that is unit-testable without a GPU. The failure modes are visual (a glare that looks wrong, banding, a flare that leaks through the Earth), all caught by goldens and unit tests, none destructive. The largest real risk is taste, which screenshots settle, not code.

## Research

Recorded from code reconnaissance on 2026-08-23, references to the tree at that date.

The render pass draws Earth, clouds, Rayleigh, and two nightglow shells and ends there (`render_pass.rs:126-216`); `Overlays::select` (`render_pass.rs:265-313`) is the existing mechanism that decides per frame which optional draws happen and with which bind group, shared by the preview and export paths so they cannot drift, and `encode_and_submit`'s argument list is already eight optional pairs long (`render_pass.rs:126-143`), which this phase should not make worse. The additive, no-depth-write nightglow pipelines are the blend/depth template (`gpu_setup.rs:387-434`). `dither()` in `sphere.wgsl:216-219` is the in-tree fix for 8-bit banding in faint gradients and is exactly what the bloom tail needs. The camera's eye position and view matrix come from `OrbitalCamera` (`camera.rs:197-235`); the post-projection screen offset is applied in `mvp_matrix` (`camera.rs:245-249`) and phase A already routes it to sky content. The atmosphere shell radius that defines the transit band is `RAYLEIGH_RADIUS = 1.015` (`params.rs:17`). Phase A's `SkyState` owns per-frame astronomy and `FrameInputs` carries it into the renderer; the sun direction is already there. Angular geometry for occlusion: from an eye at distance d the Earth's angular radius is asin(1/d), the atmosphere's asin(1.015/d), and the sun's disk is a constant 0.267 degrees of angular radius, so the visible fraction and the transit factor are closed-form functions of the angle between the sun direction and the eye-to-origin direction. Golden cases pin a custom datetime (`tests/golden.rs:109-125`), and the digest table test is `params.rs:420`.

## Key Design Decisions

1. **The glare is one screen-aligned quad, drawn last, depth ignored, additive.** Glare forms in the observer, so it layers over everything including the atmosphere shells. The quad is sized in clip space to the glare's angular extent (bloom reach plus margin) around the sun's projected position; the draw is skipped when `sun_glow` is 0, when the disk is fully occluded, or when the sun projects behind the camera (clip w not positive). It joins `Overlays` as a fifth optional entry rather than growing `encode_and_submit`'s raw argument list; if that list is refactored to take `Overlays` wholesale, do it here as a mechanical prelude commit.

2. **Occlusion and transit are CPU-side pure functions in `scene::sky`, not GPU queries.** `sun_glare(eye, sun_dir)` returns the visible-disk fraction (smoothstepped over the sun's angular diameter as its center crosses the Earth's angular limb) and the transit factor (where the line of sight passes between the solid limb and the Rayleigh radius). Both land in `SkyState`, reach the shader as two scalars, and scale every glare term; the transit factor additionally shifts the bloom and corona toward the warm sunrise tint. Unit tests cover the four regimes: fully visible, fully hidden, grazing the limb, inside the atmosphere band, plus the symmetry that the fraction is monotonic along a limb crossing.

3. **The composition is the Spencer glare model, procedurally, per fragment, in angular coordinates.** The fragment shader reconstructs the view ray by analytically inverting phase A's stereographic sky lens, measures the angle to the sun center (so every falloff is FOV-correct, not pixel-fixed), and sums: the clipped white core at 0.267 degrees angular radius with a soft edge; an inverse-power bloom reaching past 10 degrees at low alpha with an optional warm bias; ciliary-corona needles as deterministic angular hash noise modulating the inner few degrees (dozens of thin lobes, varying length, low contrast, lobes at least 2 pixels wide at their tips for adapter stability); and a lenticular halo ring near 3 degrees, blue-tinged inner edge, red-tinged outer edge, very low alpha, folded under the glow parameter rather than getting its own slider unless screenshots demand one. The bloom tail gets `dither()`. Every term multiplies the occlusion fraction; the warm shift follows the transit factor.

4. **Camera mode is a separate parameter and ships off.** `sun_flare` adds an N-point aperture starburst (default six spikes; screen-fixed, because spikes belong to the imaging device and the tilt control rolls that device, so a device-attached pattern does not rotate in frame) and two or three ghost blobs along the axis from the sun's screen position through screen center, the classic sprite-flare geometry, procedural like everything else. Default 0 because lens ghosts in a still wallpaper read as smudges to some viewers, and the eye-glare default cannot produce ghosts.

5. **Three scene parameters, full digest treatment: `sun_glow`, `sun_rays`, `sun_flare`.** Config fields with sliders in the Celestial group, digest entries and table rows, uniforms. `sun_glow` is the master (0 skips the draw); `sun_rays` scales the corona needles; `sun_flare` enables and scales camera mode. Defaults: glow on at a modest value, rays on low, flare 0.

6. **The optional Rayleigh forward-scatter boost is a separate, droppable step.** A small term in `fs_rayleigh` raising the existing limb glow near the sun's azimuth (a rim-term proxy for forward scattering, per the research), tried with screenshots after the glare itself works. If it does not visibly improve the limb-sunrise composition it is dropped without ceremony, and the research document's backlog entry for it is updated either way.

7. **No exposure model, no HDR.** Star and glare brightness stay artistically mapped into the LDR target; the glare's bloom is what makes the sun visually dominate, not any scene-wide adjustment. An HDR intermediate with convolution bloom stays on the backlog and would subsume parts of this shader if it ever lands.

## Success Criteria

1. Unit tests pin the occlusion and transit function at its four regimes and its monotonic limb crossing, all without a GPU.
2. A GPU engine test asserts a fully occluded sun contributes no pixels (render with the sun behind the Earth, compare against glow 0).
3. Two new goldens at pinned datetimes: the sun in frame over the night side, and the sun grazing the limb with the warm transit tint; both stable across three consecutive runs on the local adapters, existing goldens untouched.
4. The three parameters round-trip through config and the Slint bridge, have digest rows, and `sun_glow = 0` demonstrably skips the draw.
5. The sun visibly outshines everything: in the sun-in-frame golden, the brightest non-sun pixel (Venus or Sirius) stays below the clipped core, checked once by inspection and then protected by the golden.
6. No banding visible in the bloom tail on an 8-bit screenshot at default settings (inspection during implementation; the dither is the mechanism).
7. `cargo test`, `cargo clippy --all-targets`, `cargo fmt --check` green on Windows and in WSL; CLAUDE.md and `docs/roadmap.md` updated.

## Implementation Steps

### Step 1: Occlusion and transit in `scene::sky`

The pure functions, their `SkyState` fields, the four-regime and monotonicity unit tests, and the uniform plumbing for the two scalars. No visible change yet.

### Step 2: The glare pipeline and the core composition

The quad pipeline (additive, depth ignored, drawn last through `Overlays`, with the argument-list refactor as a prelude if taken), the WGSL entry points, core disk plus bloom with dither, occlusion applied, skip conditions. First screenshots.

### Step 3: Corona, halo, and the transit tint

The angular hash needles, the halo ring, the warm shift from the transit factor. Screenshot iteration on the defaults; this is where taste is settled.

### Step 4: Camera mode

Spikes and ghosts behind `sun_flare`, default 0.

### Step 5: Parameters, config, UI

The three fields through the whole bridge, digest rows, Celestial group sliders, `slint_ui.rs` coverage.

### Step 6: The optional Rayleigh boost, goldens, documentation

The `fs_rayleigh` term evaluated against screenshots and kept or dropped; the two goldens; CLAUDE.md and roadmap; the research document's backlog note updated for whichever way the boost went.

## Risks and Mitigations

- Taste. The composition has four coupled falloffs and the defaults will take iteration; Step 3 exists as the dedicated screenshot loop, and every knob that survives is a parameter, so a bad default is recoverable by the user.
- Cross-adapter golden stability of the needle noise. Pure ALU and deterministic, but trigonometric precision differs per adapter; mitigated by keeping needles low-contrast and at least 2 pixels wide, and by the existing tolerance. If one adapter still disagrees, coarsen the needle frequency rather than the tolerance.
- Additive saturation where the glare overlaps the bright limb or clouds. Expected and physically sensible (that is what veiling glare does); checked by the grazing golden for gross ugliness.
- The quad's clip-space sizing near the screen edge or at extreme offsets could clip the bloom tail. Size with margin and verify with an off-center framing during Step 2.
- The transit band depends on `RAYLEIGH_RADIUS` staying 1.015; if the atmosphere work ever moves it, the transit factor follows automatically because it reads the same constant, which is why the function takes the radius as an argument rather than embedding it.

## Rollback Strategy

Feature branch, plain revert. All three config fields are ignored by older binaries; no assets, no persisted state, no schema change anywhere. Dropping Step 6's Rayleigh term is a one-line revert independent of the rest.
