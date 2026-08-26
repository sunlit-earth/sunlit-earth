# Plan: Celestial D, the Milky Way

Phase D of [2026-08-23-celestial-bodies-research.md](2026-08-23-celestial-bodies-research.md), section 8. Builds on phase A's `SkyState` and star sprites ([2026-08-23-celestial-a-stars-plan.md](2026-08-23-celestial-a-stars-plan.md)), on phase B's inverse of the sky lens, `sky_lens_direction` in `sphere.wgsl` ([2026-08-23-celestial-b-sun-plan.md](2026-08-23-celestial-b-sun-plan.md)), and on phase C's slot layout ([2026-08-23-celestial-c-moon-plan.md](2026-08-23-celestial-c-moon-plan.md)).

## Summary

The diffuse Milky Way as a background layer: NASA SVS Deep Star Maps 2020's Milky-Way-only layer (faint Gaia starlight with the bright stars excluded, so nothing is drawn twice above phase A's sprites), equatorial J2000 variant, tone-mapped offline from the 4k EXR to an 8-bit JXL, rendered by a fullscreen draw that reconstructs the view ray per pixel through the inversion the Sun's glare already uses and samples the panorama through the transposed sky rotation. One intensity parameter. The layer follows the texture resolution cap, adds a resolution-dependent budget term, and the pass's clear color moves to near black.

## Stakes Classification

Low-medium. Additive rendering plus one LFS asset plus a budget bump. The failure modes are visual (a misrotated band, a mip seam at the panorama wrap, a tone-map that reads wrong) and one memory-accounting update; nothing destructive, nothing persisted beyond a config field. The clear-color change touches every existing golden, which makes it the one step with repo-wide blast radius, handled as its own step with a full regeneration.

## Research

Recorded from code reconnaissance on 2026-08-23, references to the tree at that date.

There is no fullscreen pass in the tree; every pipeline draws the shared UV sphere (`render_pass.rs:126-216`), and the pass clears to (0.02, 0.02, 0.05) (`render_pass.rs:156-161`), which at the time of writing was the entire sky. Phase A has since put stars and planets on it, and after this phase it is dead weight under real content. The sky rotation and its transpose exist since phase A (`scene::sky`); the screen offset that phase A's star path already compensates applies here in reverse when reconstructing rays. The slot machinery is post-phase-C: file-backed slots contiguous, clouds derived, the mode-to-slot map explicit; this layer takes the next file-backed slot and, unlike the moon's 1024 source, its 4096 source is wider than the smallest cap, so the existing halving cache (`assets/texture_cache`, keyed under the shared cache dir with source-stamp validation) downscales it for the 2048 setting exactly as it does the Earth maps. The budget is cold start + headroom + `resident_texture_bytes(texture_resolution)` (`memory.rs:79-83`), so this layer's term belongs inside the resolution-dependent function, not beside it. The repo decodes JXL but cannot encode it (phase 4 research, `2026-08-21-phase4-texture-resolution-plan.md`), so the EXR tone-map happens in maintainer-run offline prep like every other source asset. WGSL's `textureSampleGrad` is the standard fix for the derivative discontinuity at an equirectangular wrap column. Asset facts (the `milkyway_2020` layer's contents and exclusions, equatorial and galactic variants, 4k EXR at 34.3 MB, public domain with the "NASA/GSFC/SVS" and "ESA/Gaia/DPAC" credit) are in the research document; the equatorial variant needs no galactic rotation at runtime, though `Astronomy_Rotation_GAL_EQJ` exists if the choice ever changes. Golden tolerance and the pinned datetime are per `tests/golden.rs:109-180`.

Read again on 2026-08-26, after phase B. The pass still clears to (0.02, 0.02, 0.05) (`render_pass.rs:202-207`), and three engine cases hardcode that as `CLEAR: [u8; 4] = [5, 5, 13, 255]` (`tests/engine.rs:142`, `184`, `215`), so decision 6's move takes them with it; `tests/render_pipeline.rs` has a `CLEAR_COLOR` of its own for passes it encodes itself and is unaffected. There are now draws with no vertex buffer: the two sun quads are four vertices generated from `vertex_index` (`sun_quad_corner`, `sphere.wgsl:701-709`), built by `create_sun_pipeline` with `buffers: &[]`, additive blend, no depth write and `CompareFunction::Always` (`gpu_setup.rs:331-388`), and `vs_sun_glare` already emits a fullscreen quad when the cone reaches the antipode or camera mode is on. The inverse of the sky lens exists: `sky_lens_direction(position)` (`sphere.wgsl:589-604`) takes a framebuffer position to a view-space direction, pan included, and `fs_sun_glare` measures every glare term through it. Its pan sign is pinned by `a_pan_slides_the_composite_without_shearing_it` in `tests/engine.rs`, and phase B's departure 8 records what each wrong sign in that chain looks like. `the_shader_and_the_cpu_agree_on_the_two_shared_rules` (`tests/render_pipeline.rs:1211-1330`) appends a compute entry point to the production shaders and reads WGSL functions back off the GPU, which is the technique for testing production shader math rather than a CPU re-derivation of it. The draw order is stars and planets, the Sun's disk, the Earth, clouds, Rayleigh, the two nightglow shells, and the Sun's glare (`render_pass.rs:161-168`), with the Moon due between the disk and the Earth; the selection structs are `Stars::select` and `Sun::select` (`render_pass.rs:344-394`). Phase B's two goldens, `sun_over_the_night_side` and `sun_grazing_the_limb`, run at 60 degrees of sky because at the default the whole composition fits in forty pixels of a 512 by 256 frame and a reference that had lost the Sun still passed; that measurement is departure 2 of its plan.

## Key Design Decisions

1. **A fullscreen draw, first in the pass, no depth interaction, through the inversion that already exists.** *(Revised after phase B; see Revisions after phase B, 1.)* The fragment shader calls `sky_lens_direction` on its framebuffer position, which undoes the pan and inverts the sky lens exactly as the Sun's glare does, then rotates by the transposed view rotation and by the transposed `R_world_from_eqj` into EQJ, and converts the direction to equirectangular UV. No second inversion is written: the one in the tree has its pan sign pinned and shares its edge-radius clamp with the forward map, and a copy would be a rule spelled twice, which phase B's departure 7 is about. The vertex side is the four-vertex quad the sun draws generate from `vertex_index`, through the same pipeline template with `buffers: &[]`, rather than an oversized triangle; the two cover the same pixels and one of them exists. This is exact at every zoom and immune to the far plane, which is why it wins over an inside-out sphere whose radius would have to thread between the maximum camera distance and the far plane forever (research document, section 8.2). Stars draw second and sit on top; the Sun's disk, the Moon, the Earth and everything after overdraw it by order, and the Sun's glare, drawn last, veils it as it veils everything under it.

   The inversion is total, which is what makes a fullscreen primitive exactly right rather than one that needs a guard. The lens normalizes the horizontal half field of view to the screen edge, so a pixel at NDC (x, y) inverts through `theta = 2 * atan(length(vec2(x, y / aspect)) * edge_radius)`, and the largest value that can produce is at the screen corner: 77 degrees at the default 140 degree sky on a 16:9 frame, 98 at the widest setting of 180. Both are short of the 180 degrees where the projection diverges, so every pixel of the frame has a direction and there is no region outside the field of view to leave undrawn. The shader's `sky_corner_angle` is that number computed per frame, pan included, for the sun draws' culls.

2. **The wrap seam is handled with explicit gradients from the start.** The atan2-derived U coordinate jumps a full texture width at the wrap column, which makes hardware mip selection pick the smallest level for one pixel column; the shader computes UV gradients from the direction's derivatives (or equivalently samples with `textureSampleGrad` using gradients built from a continuous auxiliary coordinate) rather than shipping the artifact and patching it later. *(Revised after phase B; see Revisions after phase B, 2.)* The fix is pinned by an engine case with a per-column assertion, not by a golden. A wrap column is one pixel wide: in a 512 by 256 frame that is 256 pixels, 0.2 percent of the frame, under the one percent of outliers a golden tolerates and nothing at all in its mean, so a seam golden would pass with the seam in it, which is the failure phase B's departure 2 measured for a Sun at the default sky. The case renders the fixture panorama with the wrap in frame and asserts that no column differs from both its neighbors by more than a step, with the fixture's content smooth across the wrap so a mip drop shows as a distinct band; it has to fail when the gradient sample is replaced by a plain `textureSample`, which is the fault it exists to catch. A golden may still be framed across the seam for the picture, but it is not the test.

3. **The asset is the equatorial 4k `milkyway_2020` layer, tone-mapped offline, and the exposure is decided once with screenshots.** Maintainer prep: download the 4096x2048 EXR, tone-map the linear half-float data to 8-bit (the exposure and curve chosen against night-side screenshots during Step 1, then recorded in the provenance note), orient to the loader's convention, encode with cjxl, commit to `textures/` under LFS with provenance and the credit line, which also joins the About window's attribution list. The in-app intensity slider covers taste at runtime; the baked exposure only has to be a sensible neutral.

   What Step 1's screenshots also have to settle is whether 4096 is enough resolution, which phase A's field-of-view control turned into a question. The panorama carries 11.4 texels per degree; a 4K render shows 23.9 screen pixels per degree at the default 140 degree sky and 62.5 at the narrowest 60, so the layer is magnified between two and five and a half times depending on where the user leaves the slider. A diffuse band is the kind of content that tolerates magnification, which is why this plan still specifies 4096, but it is a judgment to make with a night-side screenshot at both ends of the slider and not an assumption to ship. The 8192 variant would match the default exactly and quadruple the budget term; if it is taken, record it as a departure and re-measure decision 7.

4. **The layer is a file-backed slot under the resolution cap.** Next slot after the moon; loads async through the mailbox like every other slot; excluded from `textures_ready`/`textures_pending` (an overlay, like clouds and the moon); absent without the file. At the 8192 and 4096 settings the 4096 source loads as is; at 2048 the halving cache serves the downscale (10.7 MiB resident instead of 42.7 MiB). A resolution switch purges and reloads it with the other file-backed slots, and the cache makes the reload cheap.

5. **One scene parameter, `milky_way_intensity`, defaulting on at a modest value on every tier.** The layer reads phase A's `sky_fov` through the same inversion the stars use, so it rescales with them when that slider moves and needs no field of view of its own; a test that the band's screen extent tracks `sky_fov` while the Earth's does not is what keeps the two lenses from drifting apart. Full digest treatment, Celestial group slider, 0 skips the draw through a `MilkyWay::select` in the shape of `Stars::select` and `Sun::select`. The research document's open question about the low tier resolves here: the memory cost follows the resolution setting rather than the tier (the tier caps MSAA and preview width, not textures), so there is no tier-shaped reason to default it off; if the soak or budget numbers disagree during Step 4, that is a departure to record.

6. **The clear color moves to near black as its own step, with a full golden regeneration.** With a real sky behind everything, the blue-tinted clear reads as haze under the panorama and, where the panorama is dark, as a floor the stars sit on. Move it to (0.005, 0.005, 0.01) unconditionally (not conditional on the layer, so frames without the asset do not change color when it arrives), regenerate all goldens on the local adapters in one commit that contains nothing else, and dispatch `golden.yml` for the Metal set. Kept last so every other step validates against unmoved references. The move takes the three engine cases that hardcode the clear color with it (`tests/engine.rs`), and the set it regenerates is the nine cases the suite holds after phase B plus whatever phase C added. The `metal` set is already behind by phases A and B and no machine here can regenerate it, so the dispatch is for the whole backlog and not for this step alone.

7. **The budget term is resolution-dependent.** `resident_texture_bytes` gains the panorama at the width the cap yields; tests re-measured and re-pinned per the phase 4.1 convention, including the both-directions property (the budget clears a cold run at every resolution and stays under twice one).

## Success Criteria

1. At a pinned datetime the band lies where it belongs: the galactic center region renders toward Sagittarius, checked once against a planetarium view, then pinned by a golden. Every new golden in this phase has its teeth measured the way phase B measured its two: the framing is rendered with the layer switched off and the comparison has to fail.
2. No double stars: a framing containing Orion shows the belt as phase A sprites with no smeared duplicates beneath (the layer excludes bright stars by construction; the golden keeps the property if the asset is ever regenerated).
3. The seam case of decision 2 passes, and fails when the gradient sample is replaced by a plain one.
4. `milky_way_intensity` round-trips through config and the bridge, has a digest row, and 0 skips the draw.
5. At the 2048 setting the layer loads through the halving cache (log-verified warm start) and the resident footprint drops accordingly; budget tests re-pinned and green at all three settings.
6. Render time on the software adapters stays acceptable: the traced render span grows by no more than the cloud draw's cost at the same resolution (the research document's stated bound).
7. Without the LFS object the app renders exactly as post-phase-A/B/C; existing goldens pass untouched through Steps 1 to 5 and are regenerated exactly once, in the clear-color step. By then the set includes phase A's three star cases, phase B's two sun cases and whatever C added, so the regeneration commit is larger than this plan was written against and is still one commit.
8. `cargo test`, `cargo clippy --all-targets`, `cargo fmt --check` green on Windows and in WSL; CLAUDE.md and `docs/roadmap.md` updated.

## Implementation Steps

### Step 1: The asset

Offline prep with the exposure decided against screenshots, provenance and credit recorded, LFS commit, About window attribution line, e2e staging inclusion.

### Step 2: The fullscreen pipeline

The fullscreen quad through the sun pipelines' template, ray reconstruction through `sky_lens_direction`, the EQJ rotation, equirect sampling with explicit gradients, the seam engine case, `MilkyWay::select`, draw placement before the stars, the round-trip compute probe from the risks, and an engine test that a frame arrives with the layer configured against a small fixture panorama.

### Step 3: Slot, cap, and parameter

The slot wiring, cap and cache behavior verified at 2048 against the fixture, `milky_way_intensity` through the whole bridge with its digest row and slider.

### Step 4: Budget and performance

The resolution-dependent term, re-pinned budget tests, the render-span measurement on the software adapter.

### Step 5: Goldens

The band golden and the Orion no-double-stars golden with their teeth measured, all against the unchanged clear color.

### Step 6: The clear color and documentation

Decision 6's regeneration commit with the engine cases' clear constants, CLAUDE.md, roadmap, and the research document's open question 3 marked resolved.

## Risks and Mitigations

- The tone-map is a one-shot artistic decision baked into the asset. Mitigated by deciding it with screenshots before the LFS commit and by the runtime slider; a re-bake is a one-file LFS replacement if taste moves later.
- Ray reconstruction has two transposes to get right after the inversion, and each wrong sign yields a plausible-looking but misrotated sky. The inversion itself is phase B's and is pinned; the Sagittarius check catches gross errors in what this phase adds after it. The round-trip guard is not a CPU re-derivation of the lens, which would be a third spelling of the chain to keep in step: it is a compute probe in the shape of `the_shader_and_the_cpu_agree_on_the_two_shared_rules`, feeding directions through the production forward projection (the function phase C factors out of `vs_star`, or factored here if C has not) and back through `sky_lens_direction`, asserting they return, over the field of view's whole range and both signs of pan. Phase B's pan case can also be run with the fixture panorama on, since the band moving with the stars under a pan is the same property for one more consumer.
- The clear-color regeneration invalidates every reference at once; done as an isolated commit with nothing else in it, disagreement between adapters surfaces as exactly one reviewable diff set, and `golden.yml` covers Metal once this is on the default branch.
- The seam fix depends on derivative behavior that differs subtly across adapters; the seam case runs on every adapter the suite runs on rather than assuming uniformity.
- Fixture panoramas keep the engine tests independent of the LFS asset, following the existing skip-with-reason convention for the cases that truly need the real asset.

## Revisions after phase A

Phase A landed with a design that moved during implementation: celestial content is drawn through a stereographic sky lens with its own field of view (60 to 180 degrees, default 140) rather than through the Earth camera's 20 degree perspective lens. That is recorded in phase A's own Departures. This plan was written against the perspective lens and had its ray-reconstruction sentence updated when the change landed; read against the shipped tree on 2026-08-24, three further things follow.

1. **The inversion is total, so the fullscreen triangle needs no guard.** Every pixel of the frame, corners included, maps back to a direction short of the antipode: 77 degrees off the view axis at the screen corner at the default sky, 98 at the widest. Decision 1 now records the formula and the two numbers, because "invert the projection" over a lens with a bounded field of view invites a guard for pixels outside it, and there are none.

2. **The layer's on-screen scale is a user control now.** `sky_fov` magnifies the panorama along with the stars, which is right and is the point, but it means the resolution question has a range rather than an answer: the 4096 source is magnified about two times at the default and five and a half at the narrowest setting on a 4K render. Decision 3 keeps 4096 and makes the check explicit in Step 1 instead of assuming it; decision 5 adds the test that the band tracks `sky_fov` while the Earth does not.

3. **The forward map is now something to be the exact inverse of.** Phase A's `vs_star` clamps the field of view, divides by `edge_radius`, and applies an aspect factor and the screen offset in a particular order. A reconstruction that differs anywhere in that chain puts the band at a slightly different scale from the stars on top of it, which reads as a misalignment rather than as a bug. The risks section asks for a round-trip test rather than a re-derivation.

Unchanged by any of this: the asset choice and its offline tone-map, the seam handling, the slot and cap behavior, the budget term, and the clear-color step, which is still last and still its own commit.

## Revisions after phase B

Phase B landed on 2026-08-26 and this plan was read against it the same day. Three things follow.

1. **The inversion this plan was going to write exists, and is used rather than duplicated.** `sky_lens_direction` in `sphere.wgsl` is the analytic inverse of the sky lens, pan included, and the Sun's glare measures every one of its terms through it. Decision 1 calls it. Phase B's departure 7 is the standing rule: a rule spelled twice across the composite is a divergence waiting for the frame that shows it, and its departure 8 records that the pan reaches this chain with a sign that nothing but a panned frame can check. The risk section's round-trip test moves onto the GPU accordingly, in the shape of the compute probe phase B added, instead of the CPU-side re-derivation that would have been a third spelling.

2. **The seam golden had no teeth, so the seam gets an engine case.** Phase B measured that a golden at the default sky passes with the Sun deleted. A wrap seam is one column, 0.2 percent of a golden frame, inside its outlier allowance and absent from its mean. Decision 2 and criterion 3 now ask for a per-column assertion against the fixture panorama, and for the case to fail with a plain `textureSample` in place of the gradient one.

3. **New goldens are measured for teeth, and the clear-color step knows what else it moves.** Every new reference is rendered once without the layer to confirm the comparison fails. Decision 6's regeneration now also names the three engine cases that hardcode the clear color, and the set it regenerates is two cases larger than it was, both of phase B's at 60 degrees of sky.

Noted without a decision to make: the fullscreen primitive is the sun draws' `vertex_index` quad through their pipeline template, since that exists and an oversized triangle would be new code for the same pixels; and the Sun's glare, drawn last, veils the band like everything under it, which is what veiling glare does.

Unchanged by any of this: the asset choice and its offline tone-map, the slot and cap behavior, the budget term, the `sky_fov` tracking test, and the clear-color step as the last and isolated commit.

## Rollback Strategy

Feature branch. The config field is serde-ignored by older binaries, the asset is inert LFS data, and the clear-color commit reverts together with its regenerated goldens as one unit. Steps 1 through 5 leave existing references untouched, so a partial landing is safe at any step boundary before Step 6.
