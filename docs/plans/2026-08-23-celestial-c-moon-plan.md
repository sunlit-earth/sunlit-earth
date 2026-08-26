# Plan: Celestial C, the Moon

Phase C of [2026-08-23-celestial-bodies-research.md](2026-08-23-celestial-bodies-research.md), section 6. Builds on phase A's `SkyState` ([2026-08-23-celestial-a-stars-plan.md](2026-08-23-celestial-a-stars-plan.md)); independent of phase B.

## Summary

The Moon as a textured sphere at its true position, distance, and orientation, projected through phase A's stereographic sky lens: position from `Astronomy_GeoMoon`, phase falling out of lighting the sphere with the existing sun direction, orientation from `Astronomy_RotationAxis`, surface from the NASA CGI Moon Kit at 1024x512 through the existing async texture slot machinery. The far plane does not change. Two artistic controls: a size factor that scales only the radius, and an earthshine floor. Includes one load-bearing refactor: the texture slot layout stops punning the clouds slot against the blend-mode combobox index, because the Moon is the first new file-backed slot since that pun was written.

## Stakes Classification

Medium. The slot refactor touches the mapping every texture mode flows through, so a mistake shows as the wrong texture in a mode, a wedged "Loading..." state, or a mailbox index panic; all covered by existing engine tests plus new ones, none destructive. The rest is additive rendering plus one new LFS asset, and nothing outside the Moon's own draw changes at all now that the far plane stays where it is.

## Research

Recorded from code reconnaissance on 2026-08-23, references to the tree at that date.

The slot layout is the hazard. Slots are grid 0, day 1, night 2, clouds 3; `CLOUDS_SLOT = 3` shares its value with `BLEND_MODE_INDEX = 3`, which is a combobox index, and the code comment documents the pun (`renderer/mod.rs:39-53`); `slot_of` maps combobox index to slot by identity (`renderer/mod.rs:594-597`), which works only because the first three modes equal their slots and blend is special-cased before use (`renderer/mod.rs:397-406`, `texture_routing`). The engine computes `slot_count = texture_paths.len() + 2` (grid plus clouds, `engine/mod.rs:513`) and asserts the injected mailbox agrees (`engine/mod.rs:513-520`); production passes two paths (`resolve_texture_paths`, `sunlit-app/src/main.rs:228`), the default config passes `vec![None, None]` (`engine/mod.rs:175`). `SLOT_LABELS` names four slots for the memory report (`renderer/mod.rs:62-67`). `purge_file_backed_slots` purges slots 1 through `texture_paths.len()` on a resolution switch (`textures.rs:138`); `textures_ready`/`textures_pending` special-case blend mode and otherwise index by slot (`renderer/mod.rs:397-434`); clouds are deliberately excluded from readiness. The far plane is a literal 100.0 in `projection_matrix` (`camera.rs:238-240`); the worst-case eye-to-Moon distance is about 144 Earth radii (research document, section 6.1). The budget is `private_bytes_budget` = cold start + headroom + resident textures by resolution (`memory.rs:79-83`). Texture prep applies a horizontal flip and a quarter shift (`texture_loader::orient`, `assets/texture_loader.rs:61-65`), so a new equirectangular source goes through the same offline orientation as the Earth maps and reads back through the same loader. `Astronomy_GeoMoon` returns geocentric EQJ in AU, `Astronomy_RotationAxis` returns the pole's J2000 RA/Dec, a spin angle for the prime meridian, and the pole unit vector (`astronomy.h:1180-1185, 1323`, struct at `astronomy.h:1085-1093`); `Astronomy_Libration` provides distance and apparent diameter for tests. The CGI Moon Kit facts (LROC WAC natural color, equirectangular centered on the near side, Mean Earth frame, 2048x1024 at 3.2 MB in the 2019 set, public domain with an SVS credit) are in the research document. Uniform growth has room: `Uniforms` is 208 bytes with a size assertion (`uniforms.rs:42`).

## Key Design Decisions

1. **The slot pun ends before the Moon lands: file-backed slots stay contiguous, clouds move to the end, and mode-to-slot becomes an explicit map.** New layout: grid 0, day 1, night 2, moon 3, clouds last at `texture_paths.len() + 1`. `CLOUDS_SLOT` stops being a constant equal to 3 and is derived; `slot_of`'s identity mapping is replaced by a function that maps the four combobox modes to their slots and is unit-tested against every mode, and `BLEND_MODE_INDEX` remains purely a combobox concept that never indexes `texture_slots`. `SLOT_LABELS` and the default `texture_paths` grow together with the engine's slot-count assertion. This is a prelude commit sequence with all existing engine and texture tests green before any moon rendering exists, because it is the step with real regression surface.

2. **The Moon's placement, scale, and orientation are a model matrix, and the sky lens turns it into pixels.** *(Revised after phase A; see Revisions 1.)* Position: `GeoMoon` in AU times 23,455 (Earth radii per AU) through `R_world_from_eqj`. Rotation: the IAU axis from `RotationAxis` (pole plus prime meridian spin) composed with `R_world_from_eqj`. Scale: 0.2724 times the `moon_size` parameter. Those three are a model matrix in the shared uniform buffer, as first planned. What changed is what happens next: `vs_moon` takes each mesh vertex to world space through that matrix, subtracts the eye position, and maps the resulting direction through the stereographic sky lens, exactly as `vs_star` maps a catalog direction. It is a model matrix followed by the sky lens, not a second MVP.

   Taking the direction from the eye rather than from the geocenter is what keeps the two things true scale placement was for. Parallax is exact at every camera distance, including the distances beyond the Moon's own orbit the zoom control reaches, and so is apparent size, which grows as the camera approaches the Moon and shrinks as it recedes. Neither needs a calculation of its own; both fall out of measuring the direction from where the camera is. Unit tests as first planned: distance within 55 to 64 Earth radii across a year sweep, one or two positions against JPL Horizons values, and the sub-Earth longitude of the oriented sphere within libration bounds of zero (the tidal-lock sanity check that catches a transposed rotation or a flipped spin sign).

3. **The far plane does not change.** *(Revised after phase A; see Revisions 1.)* The Moon reaches the screen as a direction, so nothing in the scene is farther away than the Earth and the literal 100 in `projection_matrix` is still correct. This decision existed to make a worst-case eye-to-Moon distance of 144 fit inside the frustum, and there is no frustum in the Moon's path any more. Dropping it takes the one step in this phase with repo-wide blast radius out of it: no reference image can move because of the Moon's projection, which is what makes criterion 5 a real check rather than a hoped-for one.

4. **The moon draws opaque and back-face culled, with the sky, after the stars (and after phase B's sun disk, where that has landed) and before the Earth.** *(Revised after phase A; see Revisions 2.)* Occlusion is the composite's rule rather than depth testing: everything celestial is drawn first and the opaque globe covers whatever falls inside its painted disc, which is how phase A hides a star behind the Earth and is the only rule that produces a coherent picture when the globe is drawn four and a half times larger than the sky lens's image of its own silhouette. A depth comparison between the two lenses would mean nothing, so the Moon does not make one; the pipeline writes no depth and its self-occlusion comes from back-face culling instead, which is exact on a convex sphere and needs nothing else. Clouds and the atmosphere shells then sit unambiguously in front of the Moon, which is correct for a Moon anywhere beyond the limb.

   The fragment shader is unchanged in substance: sample the color texture, apply a hard terminator (narrow smoothstep on n dot l with the existing `sun_dir`), an earthshine floor, and a fixed brightness; no water, no fresnel, no night texture. The mesh is the existing UV sphere.

   What this gives up is the one configuration depth testing bought: a camera beyond the Moon's orbit, where the Moon is genuinely in front of the Earth and will be painted over anyway. The composite has no way to show that correctly, since the globe it would have to be in front of is drawn in a different lens from the sky the Moon is in. It is reachable (the camera goes to 80 Earth radii and the Moon sits between 56 and 64), so it is a stated limitation rather than an unreachable corner.

5. **The asset is the 2019 kit's 1024x512, prepared offline like the Earth maps.** *(Revised after phase A; see Revisions 3.)* Downloaded once by a maintainer, oriented (the same flip and quarter-shift convention `texture_loader::orient` expects, verified by the landmark check below), encoded to JXL with cjxl (the repo decodes JXL but does not encode it, so prep stays external, as it was for the Earth sources), committed to `textures/` under LFS with provenance and the SVS credit line recorded beside it and in the About window's attribution list. The size follows from what the sky lens can put on screen. The disk is 13 pixels across at the default field of view on a 4K render, and 260 at the far end of both controls together, the narrowest 60 degree sky at the eight times size decision 6 now offers. A visible hemisphere spans 180 degrees of longitude, so 260 pixels asks for 1.4 screen pixels per degree against the 2.8 texels per degree a 1024-wide map carries. 2048 was sized for a Moon that would have been 57 pixels at 1x under the Earth's own lens, and would now be four times the memory (11.2 MiB against 2.8 MiB of VRAM with mips) for resolution nothing on screen can ask for. One framing does out-ask it, and it is worth naming rather than rounding away: the camera at 80 Earth radii with the Moon on the same side is 24 radii from it, where the disk reaches 648 pixels and 1024 falls to about 0.8 texels per pixel. Three controls at their extremes at once, soft rather than broken; if a screenshot disagrees, 2048 is a one-file swap. At 1024 wide it sits at or below every resolution cap, so the halving cache never touches it. The Moon is an overlay like clouds: excluded from `textures_ready`/`textures_pending`, absent without the file, never delaying a wallpaper export; unlike clouds its slot is file-backed, so a resolution switch purges and reloads it, which at 1024 is cheap and correct.

6. **Two scene parameters, full digest treatment: `moon_size` and `moon_earthshine`.** Size 1.0 to 8.0, default 1.0 (realistic by default; the research document records the reasoning for scaling radius rather than distance). The range doubles because the sky lens made the honest Moon four and a half times smaller than the range was chosen against: 13 pixels on a 4K wallpaper and about 6 on a 1080p preview. It is worth saying in the same breath that `moon_size` is the dishonest way to get a bigger Moon and phase A already shipped the honest one, since narrowing the sky field of view magnifies the Moon and the stars around it together; the slider is there for the user who wants the Moon larger than its neighbors, not as the recommended route. Earthshine as a small floor, default subtly nonzero so a new moon does not vanish. Both in the Celestial group, config, digest rows, uniforms. No separate enable: size cannot reach zero, but a missing asset or the existing texture modes are unaffected, and if screenshots argue for an off switch, earthshine plus a zero-size floor is not it; add a `moon_enabled` bool then and record it as a departure.

7. **The budget grows by a constant.** `private_bytes_budget` gains the moon texture's resident footprint (about 2.8 MiB plus decode transients, per decision 5) as a resolution-independent term; the budget tests are re-measured and re-pinned per the phase 4.1 convention.

## Success Criteria

1. The slot refactor lands first and green: every texture mode renders what it did before, blend mode included, the mailbox assertion still fires on a mismatched seam, and the mode-to-slot map has exhaustive unit coverage.
2. Position and orientation are pinned: the year-sweep distance test, at least one Horizons cross-check, and the sub-Earth longitude test all pass on all three platforms.
3. Phase is right by construction and by measurement: the GPU invariant test's lit-pixel fraction tracks `Astronomy_Illumination`'s illuminated fraction within tolerance at three pinned datetimes (crescent, quarter, gibbous). The test narrows the sky field of view and raises `moon_size` so the disk is large enough for a pixel fraction to mean anything; at the defaults it is 13 pixels across and the measurement would be quantization noise.
4. A human check against a planetarium view confirms the visible face and crescent orientation once at a pinned datetime, at a framing chosen so the disk is big enough to judge; a moon-crescent golden at that framing then pins it forever.
5. Existing goldens pass unchanged, with nothing in this phase able to move them now that the far plane stays put and the Moon draws only where it is; the new golden is stable across three consecutive runs on the local adapters.
6. Without the LFS object the app runs exactly as today plus stars (and sun, if phase B landed): no moon, no error, no delayed export; the e2e staging carries the new texture along with the existing ones.
7. Budget tests re-pinned; `cargo test`, `cargo clippy --all-targets`, `cargo fmt --check` green on Windows and in WSL; CLAUDE.md and `docs/roadmap.md` updated.

## Implementation Steps

### Step 1: The slot layout refactor

Decision 1 in full, as its own reviewable commit sequence, with the mode-to-slot unit tests and all existing engine, texture, and UI tests green. No moon anywhere yet.

### Step 2: `SkyState` moon fields

The model matrix inputs (position, IAU orientation, scale) and the distance, Horizons, and sub-Earth tests. No far plane work: decision 3 retired it.

### Step 3: The moon pipeline

`vs_moon`/`fs_moon`, the model matrix and moon uniforms, the sky-lens projection of eye-relative vertex directions, back-face culling with no depth write, draw placement after the stars and before the Earth, the slot wired through the engine config, and an engine test that a frame arrives with a moon path configured against a small fixture texture.

### Step 4: The asset

Offline prep (download, orient, cjxl), the LFS commit with provenance, the landmark orientation check against a planetarium view, the About window attribution line, e2e staging inclusion.

### Step 5: Parameters, phase test, budget

`moon_size` and `moon_earthshine` through the whole bridge with digest rows and UI; the illuminated-fraction GPU test; the budget term and re-pinned tests.

### Step 6: Goldens and documentation

The moon-crescent golden, CLAUDE.md, roadmap.

## Risks and Mitigations

- The slot refactor is the concentrated regression risk, which is why it is Step 1, isolated, exhaustively unit-tested, and lands with no behavior change to hide behind.
- Texture orientation has four plausible wrong answers (flip, shift, spin sign, pole sign). The sub-Earth longitude test catches the rotation half analytically; the landmark check against a planetarium view catches the texture-prep half once, and the golden keeps it caught.
- The composite hides the Moon wherever it overlaps the painted globe, including places the Moon is not actually behind the Earth. That is phase A's rule applied consistently, and a Moon a few degrees off the limb will disappear earlier than an ephemeris says it should. It is the same trade the stars already make, at a size where it is more noticeable; if a screenshot makes it look wrong, the finding is about the mixed lens and not about this phase.
- A camera beyond the Moon's orbit shows no Moon in front of the Earth, per decision 4. Reachable but rare, and stated rather than fixed.
- `RotationAxis` correctness for the Moon is trusted from the library's IAU model; even a few degrees of error is invisible on a disk of a dozen pixels, and the tests bound gross errors.
- A fixture moon texture is needed so engine tests do not depend on the LFS asset, following the existing skip-with-reason convention for tests that need real assets.

## Revisions after phase A

Phase A landed with a design that moved during implementation: celestial content is drawn through a stereographic sky lens with its own field of view (60 to 180 degrees, default 140) rather than through the Earth camera's 20 degree perspective lens. That is recorded in phase A's own Departures. This plan was written before that change and never updated for it, and read against the shipped tree on 2026-08-24 it was the phase most affected: the Moon was to be an ordinary depth-tested object in the Earth's lens, which is now the one place it cannot be.

1. **The Moon moves onto the sky lens, and the far plane change goes with it.** Under a 20 degree lens the Moon is inside the frame only when it sits within 10 degrees of the view axis, which is under one percent of the sky, so a Moon drawn there would almost never be visible; and when it was, it would be four and a half times larger than the sky lens puts the stars around it, and in the wrong place among them. Neither is acceptable for the phase whose whole point is that the Moon is up there. So `vs_moon` projects eye-relative vertex directions through the sky lens, which keeps parallax and apparent size exact for free and leaves nothing in the scene farther away than the Earth. The far plane stays at 100, and decision 3 is now a decision not to touch it.

2. **Occlusion is draw order, not depth.** A depth comparison between two different lenses means nothing, so the Moon draws with the sky, before the Earth, and the painted globe covers it exactly as it covers a star. Back-face culling replaces depth testing for the Moon's own front-to-back. The configuration this gives up is a camera beyond the Moon's orbit, where the Moon is really in front of the Earth; the composite cannot show that, and decision 4 says so rather than leaving it to be discovered.

3. **The asset halves twice.** 2048x1024 was sized against a 57-pixel disk under the Earth's lens. Through the sky lens the largest disk any ordinary framing produces is 260 pixels, which 1024x512 serves at two texels per screen pixel for a quarter of the memory. Only the camera-beyond-the-Moon framing at both controls' extremes out-asks it, and decision 5 records that rather than sizing the whole asset for it.

4. **The size slider's range doubles, to 1.0 through 8.0.** Same cause: the honest Moon is now 13 pixels on a 4K wallpaper. The default stays at 1.0, and decision 6 records that narrowing the sky field of view is the better way to get a bigger Moon, because it magnifies the sky with it.

Unchanged by any of this: decision 1's slot refactor, which is the step with the real regression surface and has nothing to do with projection; the astronomy (position, IAU orientation, libration); the phase falling out of the shared sun direction; the earthshine floor; and the overlay semantics that keep a missing asset from delaying anything.

## Rollback Strategy

Feature branch. Step 1 is intentionally separable: if later steps revert, the slot refactor can stay, since it is a correctness improvement on its own. New config fields are serde-ignored by older binaries, and the LFS asset is inert data. There is no far plane constant to revert any more, which is one fewer global thing this phase can leave behind.
