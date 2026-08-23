# Plan: Celestial C, the Moon

Phase C of [2026-08-23-celestial-bodies-research.md](2026-08-23-celestial-bodies-research.md), section 6. Builds on phase A's `SkyState` ([2026-08-23-celestial-a-stars-plan.md](2026-08-23-celestial-a-stars-plan.md)); independent of phase B.

## Summary

The Moon as a textured sphere at its true position, distance, and orientation: position from `Astronomy_GeoMoon`, phase falling out of lighting the sphere with the existing sun direction, orientation from `Astronomy_RotationAxis`, surface from the NASA CGI Moon Kit at 2048x1024 through the existing async texture slot machinery. The far plane grows to 250. Two artistic controls: a size factor that scales only the radius, and an earthshine floor. Includes one load-bearing refactor: the texture slot layout stops punning the clouds slot against the blend-mode combobox index, because the Moon is the first new file-backed slot since that pun was written.

## Stakes Classification

Medium. The slot refactor touches the mapping every texture mode flows through, so a mistake shows as the wrong texture in a mode, a wedged "Loading..." state, or a mailbox index panic; all covered by existing engine tests plus new ones, none destructive. The rest is additive rendering plus one new LFS asset. The far plane change is global but bounded by golden coverage.

## Research

Recorded from code reconnaissance on 2026-08-23, references to the tree at that date.

The slot layout is the hazard. Slots are grid 0, day 1, night 2, clouds 3; `CLOUDS_SLOT = 3` shares its value with `BLEND_MODE_INDEX = 3`, which is a combobox index, and the code comment documents the pun (`renderer/mod.rs:39-53`); `slot_of` maps combobox index to slot by identity (`renderer/mod.rs:594-597`), which works only because the first three modes equal their slots and blend is special-cased before use (`renderer/mod.rs:397-406`, `texture_routing`). The engine computes `slot_count = texture_paths.len() + 2` (grid plus clouds, `engine/mod.rs:513`) and asserts the injected mailbox agrees (`engine/mod.rs:513-520`); production passes two paths (`resolve_texture_paths`, `sunlit-app/src/main.rs:228`), the default config passes `vec![None, None]` (`engine/mod.rs:175`). `SLOT_LABELS` names four slots for the memory report (`renderer/mod.rs:62-67`). `purge_file_backed_slots` purges slots 1 through `texture_paths.len()` on a resolution switch (`textures.rs:138`); `textures_ready`/`textures_pending` special-case blend mode and otherwise index by slot (`renderer/mod.rs:397-434`); clouds are deliberately excluded from readiness. The far plane is a literal 100.0 in `projection_matrix` (`camera.rs:238-240`); the worst-case eye-to-Moon distance is about 144 Earth radii (research document, section 6.1). The budget is `private_bytes_budget` = cold start + headroom + resident textures by resolution (`memory.rs:79-83`). Texture prep applies a horizontal flip and a quarter shift (`texture_loader::orient`, `assets/texture_loader.rs:61-65`), so a new equirectangular source goes through the same offline orientation as the Earth maps and reads back through the same loader. `Astronomy_GeoMoon` returns geocentric EQJ in AU, `Astronomy_RotationAxis` returns the pole's J2000 RA/Dec, a spin angle for the prime meridian, and the pole unit vector (`astronomy.h:1180-1185, 1323`, struct at `astronomy.h:1085-1093`); `Astronomy_Libration` provides distance and apparent diameter for tests. The CGI Moon Kit facts (LROC WAC natural color, equirectangular centered on the near side, Mean Earth frame, 2048x1024 at 3.2 MB in the 2019 set, public domain with an SVS credit) are in the research document. Uniform growth has room: `Uniforms` is 208 bytes with a size assertion (`uniforms.rs:42`).

## Key Design Decisions

1. **The slot pun ends before the Moon lands: file-backed slots stay contiguous, clouds move to the end, and mode-to-slot becomes an explicit map.** New layout: grid 0, day 1, night 2, moon 3, clouds last at `texture_paths.len() + 1`. `CLOUDS_SLOT` stops being a constant equal to 3 and is derived; `slot_of`'s identity mapping is replaced by a function that maps the four combobox modes to their slots and is unit-tested against every mode, and `BLEND_MODE_INDEX` remains purely a combobox concept that never indexes `texture_slots`. `SLOT_LABELS` and the default `texture_paths` grow together with the engine's slot-count assertion. This is a prelude commit sequence with all existing engine and texture tests green before any moon rendering exists, because it is the step with real regression surface.

2. **The Moon's placement, scale, and orientation are a model matrix computed in `SkyState`.** Position: `GeoMoon` in AU times 23,455 (Earth radii per AU) through `R_world_from_eqj`. Rotation: the IAU axis from `RotationAxis` (pole plus prime meridian spin) composed with `R_world_from_eqj`. Scale: 0.2724 times the `moon_size` parameter. The matrix reaches the shader as a second MVP in the shared uniform buffer. Unit tests: distance within 55 to 64 Earth radii across a year sweep, one or two positions against JPL Horizons values, and the sub-Earth longitude of the oriented sphere within libration bounds of zero (the tidal-lock sanity check that catches a transposed rotation or a flipped spin sign).

3. **The far plane becomes a named constant at 250.** One literal in `projection_matrix` becomes `FAR_PLANE`, sized for the worst case (144) with margin for the size slider and future zoom changes. `Depth32Float` with near 0.1 loses nothing measurable. Goldens are the regression net: existing references must still pass, since the projection matrix change affects only depth values, not coverage.

4. **The moon draws opaque with depth write, right after the Earth, through its own `vs_moon`/`fs_moon`.** Depth testing settles occlusion in both directions (Moon behind the limb, and Moon in front of the Earth when the camera is beyond it); the shells drawn later already depth-test, so the atmosphere overdraws a Moon behind the limb and skips a Moon in front, both correct (research document, section 9). The fragment shader samples the color texture, applies a hard terminator (narrow smoothstep on n dot l with the existing `sun_dir`), an earthshine floor, and a fixed brightness; no water, no fresnel, no night texture. The mesh is the existing UV sphere.

5. **The asset is the 2019 kit's 2048x1024, prepared offline like the Earth maps.** Downloaded once by a maintainer, oriented (the same flip and quarter-shift convention `texture_loader::orient` expects, verified by the landmark check below), encoded to JXL with cjxl (the repo decodes JXL but does not encode it, so prep stays external, as it was for the Earth sources), committed to `textures/` under LFS with provenance and the SVS credit line recorded beside it and in the About window's attribution list. At 2048 wide it sits at or below every resolution cap, so the halving cache never touches it. The Moon is an overlay like clouds: excluded from `textures_ready`/`textures_pending`, absent without the file, never delaying a wallpaper export; unlike clouds its slot is file-backed, so a resolution switch purges and reloads it, which at 2048 is cheap and correct.

6. **Two scene parameters, full digest treatment: `moon_size` and `moon_earthshine`.** Size 1.0 to 4.0, default 1.0 (realistic by default; the research document records the reasoning for scaling radius rather than distance). Earthshine as a small floor, default subtly nonzero so a new moon does not vanish. Both in the Celestial group, config, digest rows, uniforms. No separate enable: size cannot reach zero, but a missing asset or the existing texture modes are unaffected, and if screenshots argue for an off switch, earthshine plus a zero-size floor is not it; add a `moon_enabled` bool then and record it as a departure.

7. **The budget grows by a constant.** `private_bytes_budget` gains the moon texture's resident footprint (about 11 MiB plus decode transients) as a resolution-independent term; the budget tests are re-measured and re-pinned per the phase 4.1 convention.

## Success Criteria

1. The slot refactor lands first and green: every texture mode renders what it did before, blend mode included, the mailbox assertion still fires on a mismatched seam, and the mode-to-slot map has exhaustive unit coverage.
2. Position and orientation are pinned: the year-sweep distance test, at least one Horizons cross-check, and the sub-Earth longitude test all pass on all three platforms.
3. Phase is right by construction and by measurement: the GPU invariant test's lit-pixel fraction tracks `Astronomy_Illumination`'s illuminated fraction within tolerance at three pinned datetimes (crescent, quarter, gibbous).
4. A human check against a planetarium view confirms the visible face and crescent orientation once at a pinned datetime; a moon-crescent golden then pins it forever.
5. Existing goldens pass unchanged after the far plane change; the new golden is stable across three consecutive runs on the local adapters.
6. Without the LFS object the app runs exactly as today plus stars (and sun, if phase B landed): no moon, no error, no delayed export; the e2e staging carries the new texture along with the existing ones.
7. Budget tests re-pinned; `cargo test`, `cargo clippy --all-targets`, `cargo fmt --check` green on Windows and in WSL; CLAUDE.md and `docs/roadmap.md` updated.

## Implementation Steps

### Step 1: The slot layout refactor

Decision 1 in full, as its own reviewable commit sequence, with the mode-to-slot unit tests and all existing engine, texture, and UI tests green. No moon anywhere yet.

### Step 2: `SkyState` moon fields and the far plane

The model matrix inputs (position, IAU orientation, scale), the distance, Horizons, and sub-Earth tests, `FAR_PLANE`, and a golden re-run proving nothing moved.

### Step 3: The moon pipeline

`vs_moon`/`fs_moon`, the second MVP and moon uniforms, draw placement after the Earth, the slot wired through the engine config, and an engine test that a frame arrives with a moon path configured against a small fixture texture.

### Step 4: The asset

Offline prep (download, orient, cjxl), the LFS commit with provenance, the landmark orientation check against a planetarium view, the About window attribution line, e2e staging inclusion.

### Step 5: Parameters, phase test, budget

`moon_size` and `moon_earthshine` through the whole bridge with digest rows and UI; the illuminated-fraction GPU test; the budget term and re-pinned tests.

### Step 6: Goldens and documentation

The moon-crescent golden, CLAUDE.md, roadmap.

## Risks and Mitigations

- The slot refactor is the concentrated regression risk, which is why it is Step 1, isolated, exhaustively unit-tested, and lands with no behavior change to hide behind.
- Texture orientation has four plausible wrong answers (flip, shift, spin sign, pole sign). The sub-Earth longitude test catches the rotation half analytically; the landmark check against a planetarium view catches the texture-prep half once, and the golden keeps it caught.
- The far plane change touches every draw's depth. Goldens are the net; if any reference moves past tolerance, that is a finding to understand, not to regenerate over.
- `RotationAxis` correctness for the Moon is trusted from the library's IAU model; even a few degrees of error is invisible on a 56-pixel disk, and the tests bound gross errors.
- A fixture moon texture is needed so engine tests do not depend on the LFS asset, following the existing skip-with-reason convention for tests that need real assets.

## Rollback Strategy

Feature branch. Step 1 is intentionally separable: if later steps revert, the slot refactor can stay, since it is a correctness improvement on its own. New config fields are serde-ignored by older binaries; the LFS asset is inert data; the far plane constant reverts with the code that needed it.
