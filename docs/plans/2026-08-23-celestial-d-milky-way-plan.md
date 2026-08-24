# Plan: Celestial D, the Milky Way

Phase D of [2026-08-23-celestial-bodies-research.md](2026-08-23-celestial-bodies-research.md), section 8. Builds on phase A's `SkyState` and star sprites ([2026-08-23-celestial-a-stars-plan.md](2026-08-23-celestial-a-stars-plan.md)) and on phase C's slot layout ([2026-08-23-celestial-c-moon-plan.md](2026-08-23-celestial-c-moon-plan.md)).

## Summary

The diffuse Milky Way as a background layer: NASA SVS Deep Star Maps 2020's Milky-Way-only layer (faint Gaia starlight with the bright stars excluded, so nothing is drawn twice above phase A's sprites), equatorial J2000 variant, tone-mapped offline from the 4k EXR to an 8-bit JXL, rendered by a fullscreen triangle that reconstructs the view ray per pixel and samples the panorama through the transposed sky rotation. One intensity parameter. The layer follows the texture resolution cap, adds a resolution-dependent budget term, and the pass's clear color moves to near black.

## Stakes Classification

Low-medium. Additive rendering plus one LFS asset plus a budget bump. The failure modes are visual (a misrotated band, a mip seam at the panorama wrap, a tone-map that reads wrong) and one memory-accounting update; nothing destructive, nothing persisted beyond a config field. The clear-color change touches every existing golden, which makes it the one step with repo-wide blast radius, handled as its own step with a full regeneration.

## Research

Recorded from code reconnaissance on 2026-08-23, references to the tree at that date.

There is no fullscreen pass in the tree; every pipeline draws the shared UV sphere (`render_pass.rs:126-216`), and the pass clears to (0.02, 0.02, 0.05) (`render_pass.rs:156-161`), which today is the entire sky and after this phase is dead weight under real content. The sky rotation and its transpose exist since phase A (`scene::sky`); the screen offset that phase A's star path already compensates applies here in reverse when reconstructing rays. The slot machinery is post-phase-C: file-backed slots contiguous, clouds derived, the mode-to-slot map explicit; this layer takes the next file-backed slot and, unlike the moon's 2048 source, its 4096 source is wider than the smallest cap, so the existing halving cache (`assets/texture_cache`, keyed under the shared cache dir with source-stamp validation) downscales it for the 2048 setting exactly as it does the Earth maps. The budget is cold start + headroom + `resident_texture_bytes(texture_resolution)` (`memory.rs:79-83`), so this layer's term belongs inside the resolution-dependent function, not beside it. The repo decodes JXL but cannot encode it (phase 4 research, `2026-08-21-phase4-texture-resolution-plan.md`), so the EXR tone-map happens in maintainer-run offline prep like every other source asset. WGSL's `textureSampleGrad` is the standard fix for the derivative discontinuity at an equirectangular wrap column. Asset facts (the `milkyway_2020` layer's contents and exclusions, equatorial and galactic variants, 4k EXR at 34.3 MB, public domain with the "NASA/GSFC/SVS" and "ESA/Gaia/DPAC" credit) are in the research document; the equatorial variant needs no galactic rotation at runtime, though `Astronomy_Rotation_GAL_EQJ` exists if the choice ever changes. Golden tolerance and the pinned datetime are per `tests/golden.rs:109-180`.

## Key Design Decisions

1. **A fullscreen triangle, drawn first, no depth interaction.** The vertex shader emits the standard oversized triangle; the fragment shader takes the NDC position, undoes the screen offset, analytically inverts phase A's stereographic sky projection, rotates by the transposed view rotation and then by the transposed `R_world_from_eqj` into EQJ, and converts the direction to equirectangular UV. This is exact at every zoom and immune to the far plane, which is why it wins over an inside-out sphere whose radius would have to thread between the maximum camera distance and the far plane forever (research document, section 8.2). Stars draw second and sit on top; the Earth and everything after overdraw it by depth or order as today.

2. **The wrap seam is handled with explicit gradients from the start.** The atan2-derived U coordinate jumps a full texture width at the wrap column, which makes hardware mip selection pick the smallest level for one pixel column; the shader computes UV gradients from the direction's derivatives (or equivalently samples with `textureSampleGrad` using gradients built from a continuous auxiliary coordinate) rather than shipping the artifact and patching it later. One golden is framed to cross the seam so the fix is pinned, not asserted.

3. **The asset is the equatorial 4k `milkyway_2020` layer, tone-mapped offline, and the exposure is decided once with screenshots.** Maintainer prep: download the 4096x2048 EXR, tone-map the linear half-float data to 8-bit (the exposure and curve chosen against night-side screenshots during Step 1, then recorded in the provenance note), orient to the loader's convention, encode with cjxl, commit to `textures/` under LFS with provenance and the credit line, which also joins the About window's attribution list. The in-app intensity slider covers taste at runtime; the baked exposure only has to be a sensible neutral.

4. **The layer is a file-backed slot under the resolution cap.** Next slot after the moon; loads async through the mailbox like every other slot; excluded from `textures_ready`/`textures_pending` (an overlay, like clouds and the moon); absent without the file. At the 8192 and 4096 settings the 4096 source loads as is; at 2048 the halving cache serves the downscale (10.7 MiB resident instead of 42.7 MiB). A resolution switch purges and reloads it with the other file-backed slots, and the cache makes the reload cheap.

5. **One scene parameter, `milky_way_intensity`, defaulting on at a modest value on every tier.** Full digest treatment, Celestial group slider, 0 skips the draw. The research document's open question about the low tier resolves here: the memory cost follows the resolution setting rather than the tier (the tier caps MSAA and preview width, not textures), so there is no tier-shaped reason to default it off; if the soak or budget numbers disagree during Step 4, that is a departure to record.

6. **The clear color moves to near black as its own step, with a full golden regeneration.** With a real sky behind everything, the blue-tinted clear reads as haze on top of the panorama and under the stars. Move it to (0.005, 0.005, 0.01) unconditionally (not conditional on the layer, so frames without the asset do not change color when it arrives), regenerate all goldens on the local adapters in one commit that contains nothing else, and dispatch `golden.yml` for the Metal set. Kept last so every other step validates against unmoved references.

7. **The budget term is resolution-dependent.** `resident_texture_bytes` gains the panorama at the width the cap yields; tests re-measured and re-pinned per the phase 4.1 convention, including the both-directions property (the budget clears a cold run at every resolution and stays under twice one).

## Success Criteria

1. At a pinned datetime the band lies where it belongs: the galactic center region renders toward Sagittarius, checked once against a planetarium view, then pinned by a golden.
2. No double stars: a framing containing Orion shows the belt as phase A sprites with no smeared duplicates beneath (the layer excludes bright stars by construction; the golden keeps the property if the asset is ever regenerated).
3. The seam golden shows no visible column at the panorama wrap.
4. `milky_way_intensity` round-trips through config and the bridge, has a digest row, and 0 skips the draw.
5. At the 2048 setting the layer loads through the halving cache (log-verified warm start) and the resident footprint drops accordingly; budget tests re-pinned and green at all three settings.
6. Render time on the software adapters stays acceptable: the traced render span grows by no more than the cloud draw's cost at the same resolution (the research document's stated bound).
7. Without the LFS object the app renders exactly as post-phase-A/B/C; existing goldens pass untouched through Steps 1 to 4 and are regenerated exactly once, in the clear-color step.
8. `cargo test`, `cargo clippy --all-targets`, `cargo fmt --check` green on Windows and in WSL; CLAUDE.md and `docs/roadmap.md` updated.

## Implementation Steps

### Step 1: The asset

Offline prep with the exposure decided against screenshots, provenance and credit recorded, LFS commit, About window attribution line, e2e staging inclusion.

### Step 2: The fullscreen pipeline

The triangle, inverse stereographic ray reconstruction with offset compensation, the EQJ rotation, equirect sampling with explicit gradients, draw placement before the stars, and an engine test that a frame arrives with the layer configured against a small fixture panorama.

### Step 3: Slot, cap, and parameter

The slot wiring, cap and cache behavior verified at 2048 against the fixture, `milky_way_intensity` through the whole bridge with its digest row and slider.

### Step 4: Budget and performance

The resolution-dependent term, re-pinned budget tests, the render-span measurement on the software adapter.

### Step 5: Goldens

The band golden, the Orion no-double-stars golden, the seam golden, all against the unchanged clear color.

### Step 6: The clear color and documentation

Decision 6's regeneration commit, CLAUDE.md, roadmap, and the research document's open question 3 marked resolved.

## Risks and Mitigations

- The tone-map is a one-shot artistic decision baked into the asset. Mitigated by deciding it with screenshots before the LFS commit and by the runtime slider; a re-bake is a one-file LFS replacement if taste moves later.
- Ray reconstruction has three transposes and an offset to get right, and each wrong sign yields a plausible-looking but misrotated sky. The Sagittarius check catches gross errors; a unit test on the CPU-side matrix pipeline (a known NDC point maps to a known EQJ direction at a pinned camera) catches the rest without a GPU.
- The clear-color regeneration invalidates every reference at once; done as an isolated commit with nothing else in it, disagreement between adapters surfaces as exactly one reviewable diff set, and `golden.yml` covers Metal once this is on the default branch.
- The seam fix depends on derivative behavior that differs subtly across adapters; the seam golden pins it on each adapter set rather than assuming uniformity.
- Fixture panoramas keep the engine tests independent of the LFS asset, following the existing skip-with-reason convention for the cases that truly need the real asset.

## Rollback Strategy

Feature branch. The config field is serde-ignored by older binaries, the asset is inert LFS data, and the clear-color commit reverts together with its regenerated goldens as one unit. Steps 1 through 5 leave existing references untouched, so a partial landing is safe at any step boundary before Step 6.
