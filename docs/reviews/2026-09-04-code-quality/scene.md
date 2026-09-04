<!-- Reviewer notes for docs/reviews/2026-09-04-code-quality-review.md. Line numbers refer to commit 3046327. "The brief" is the shared review instruction; "the maintainer's rules" are the project's comment conventions. Runtime claims here are reasoned from the code; the measured figures are in test-timing.md. -->

# Review: `crates/sunlit-core/src/scene/`

## 1. Summary

The scene modules are in better shape than their line counts suggest. Of the 3,613 lines under `scene/`, 1,984 (55 percent) are `mod tests`, and the two files the maintainer flagged as long are long for opposite reasons: `sun_occlusion.rs` really does carry 730 lines of production code across five separable concerns, while `camera.rs` carries only 279 and is bulked out by 463 lines of low-yield tests. The comments are mostly good: of 52 comment blocks four lines or longer, I would keep 39, move 7 to `docs/`, and delete 6. The exceptions are concentrated in the test doc comments, which repeatedly record measurements and explain why a test exists.

Three findings are worth acting on before release. `SunVisibility::transit_fraction` is computed every frame and read by nothing (grep across both crates, all shaders and all integration tests). `scene::sun`'s own doc comment states a coordinate frame that is the reverse of what its code produces and of what the comment 55 lines below it says. And `sky.rs`'s two full-sweep moon tests each evaluate 5,840 sky states of 15 FFI ephemeris calls apiece when they need 4, which is the heaviest unit-test cost in this area by a wide margin.

## 2. Metrics

| File | Total | Test-mod lines | Prod lines | `#[test]` | Comment blocks 4+ lines (keep / move / delete) | Fns > 80 lines | `pub` items used only in-crate |
|---|---|---|---|---|---|---|---|
| `sun_occlusion.rs` | 1449 | 719 (50%) | 730 | 34 | 25 (20 / 3 / 2) | 1 (`place_sun`, 123) | 8 |
| `camera.rs` | 742 | 463 (62%) | 279 | 31 | 4 (3 / 0 / 1) | 0 | 4 |
| `sky.rs` | 503 | 262 (52%) | 241 | 15 | 7 (4 / 2 / 1) | 0 | 0 |
| `datetime.rs` | 446 | 321 (72%) | 125 | 49 | 6 (5 / 0 / 1) | 0 | 2 |
| `moon.rs` | 279 | 162 (58%) | 117 | 7 | 5 (4 / 1 / 0) | 0 | 1 |
| `sun.rs` | 188 | 57 (30%) | 131 | 5 | 5 (3 / 1 / 1) | 0 | 1 (test-only) |
| `mod.rs` | 6 | 0 | 6 | 0 | 0 | 0 | 0 |
| **Total** | **3613** | **1984 (55%)** | **1629** | **141** | **52 (39 / 7 / 6)** | **1** | **16** |

`pub` items with no reference outside their own file (verified by grepping `crates/sunlit-app`, `crates/sunlit-core/tests`, `crates/xtask` and the rest of `crates/sunlit-core/src`):

- `sun_occlusion.rs`: `SUN_ANGULAR_RADIUS_DEGREES` (31), `EARTH_RADIUS_KM` (51), `limb_air_mass` (76), `limb_hue` (100), `refract` (333), `exposure_gain` (412), `visibility` (649), `angular_visible_fraction` (687).
- `camera.rs`: `ZOOM_DISTANCE_MIN` (166), `ZOOM_DISTANCE_MAX` (168), `distance_to_zoom` (178), `OrbitalCamera::projection_matrix` (266).
- `datetime.rs`: `is_leap_year` (18), `hour_float_to_hm` (82).
- `moon.rs`: `MOON_RADIUS_EARTH_RADII` (24).
- `sun.rs`: `sun_direction_from_time` (25) is `pub` and referenced only from `sky.rs`'s test module, so nothing in a non-test build reaches it.

`SunVisibility` (129) and `SunPlacement` (201) must stay `pub` because they appear in `place_sun`'s signature; `globe_screen_circle`, `sky_lens_disc`, `sky_lens_edge_radius`, `pixel_scale`, `limb_transmission`, `limb_disk_amplitude` and `sky_lens_direction` are all used from `crates/sunlit-app/src/mouse_math.rs` or `crates/sunlit-core/tests/`, so those are fine.

Proptest: no `ProptestConfig` anywhere in the repository, so every property runs the default 256 cases. `datetime.rs` has 6 properties (1,536 cases), `camera.rs` has 1 (256 cases). All of them are pure integer or float arithmetic with no allocation, so the case counts are not a runtime problem.

## 3. Findings

### A. Commentary

**A1 (medium) `sun.rs:22-24` states the wrong coordinate frame.** The doc comment on `sun_direction_from_time` says "Y-up, +X = prime meridian at equator, -Z = 90 degrees East". The code at 87-91 builds `Vec3::new(phi.cos()*lambda.sin(), phi.sin(), phi.cos()*lambda.cos())`, which puts (0N, 0E) on **+Z** and (0N, 90E) on **+X**. The comment at 80-82 in the same function says exactly that, and so does the test doc at 144-145. The public doc comment is the one statement in the file that is wrong, and it is the one a reader of the API sees. Fix the three lines to match 80-82.

**A2 (medium) `datetime.rs:43-45` describes the opposite of what the code does.** "After Feb 28 in a leap year, the cumulative offset is one less than in a non-leap year, so we adjust the day-of-year down." The next four lines *add* one to `cum`; the day-of-year is never adjusted. The behavior is correct (verified by hand for doy 59, 60, 61 in 2024 and 2025); only the comment is inverted. Rewrite as a one-liner about the leap day shifting March onward, or delete it (line 49's `// Feb has 29 days in a leap year` already says the same thing, and that one should go regardless as a restatement).

**A3 (medium) `moon.rs:10-16` claims a shared mechanism that does not exist.** The module doc says the Moon's disk "is floored the way `super::sun_occlusion::place_sun` floors the Sun's" and that "the floor is one rule with one spelling on one side of the language boundary". The two functions do not share a spelling. `place_sun` clamps the projected pixel radius directly (`sun_occlusion.rs:543-546`, `disc.radius = disc.radius.max(floor)`), so the floored disc is not the image of any cone and its center does not move. `place_moon` inflates the true world-space radius by the ratio and re-projects (`moon.rs:94-108`), so its disc stays a real projection and its center shifts slightly. What is actually shared is the constant `MIN_BODY_DISK_RADIUS_PIXELS` and the `pixel_scale` ramp. Both choices are defensible on their own terms (the Moon needs the model matrix scaled, the Sun does not), but the doc should say what is shared rather than claim the implementations match.

**A4 (low) `sky.rs:309-312` narrates code history.** "The branching `time_for_input` does used to sit on the sun path, and these three cases moved here with it." That is a commit message, it is ungrammatical, and it is on the maintainer's explicit delete list. Delete; keep the first line ("A custom date and time selects the instant the widgets name").

**A5 (low) `sun_occlusion.rs:1227-1231` explains why a test exists.** "Every other Moon case here calls `visibility` itself, so this is what says the argument survives the way in: without it the field could be dropped in the one function the renderer actually calls." Also on the delete list. Cut to the first line.

**A6 (low) measurements recorded in test comments.** Three blocks record numbers that belong in `docs/rendering.md`: `sun_occlusion.rs:967-972` ("0.025 of the visible fraction at the true half degree, 0.065 at three times it and 0.121 at eight"), `sun_occlusion.rs:794-796` ("at 13 km the model is 16 percent short of the table's 13 air masses and that comes out as 41 percent more green"), and `sky.rs:458` ("Worst disagreement over these four years: 0.0276 degrees"). Each is useful, and each is a measurement of a past run rather than a statement about the code. Move the numbers to `docs/`, leave a one-line pointer.

**A7 (low) duplicated prose across files.** `sun_occlusion.rs:159-161` and `renderer/render_pass.rs:120-123` carry the same sentence about "a glare fading behind something invisible". Keep it where the type is declared (`sun_occlusion.rs`) and cut the call-site copy to a cross-reference.

**A8 (low) restatements.** `camera.rs:182-185` restates the three field doc comments directly below it; `camera.rs:247`, `252-253` and `260` each restate either the next line or the function's own doc comment at 236-242; `sun.rs:26-27` ("Get the sun's equatorial coordinates") restates the call at 46. Six lines to delete, no information lost.

Comments I want to name as clearly earning their place, so this does not read as a blanket criticism: `sun_occlusion.rs:1-23` (the mixed-lens premise, which is the one thing a reader cannot recover from the code), `229-236` (the sky lens edge derivation), `241-250` (why the circle image is exact rather than a small-angle approximation), `310-318` (provenance of the refraction constants), `404-410` (why a non-physical exposure gain exists), `637-648` (the max-not-union occlusion approximation and the direction of its error); `sky.rs:113-119` (the IAU spin convention) and `137-138` (why an unreachable fallback is there); `moon.rs:60-67` and `95-102`; `sun.rs:29-35` (the 8.8 arcsecond topocentric caveat) and `68-72` (the hour-angle sign); `camera.rs:38-43` and `45-53`; `datetime.rs:27-28` and `55`. All of the `// SAFETY:` blocks (see C4).

### B. Length and structure

**B1 (medium) `sun_occlusion.rs` is five modules.** At 730 production lines the file is over the threshold on its own, and the concerns separate cleanly. The module name covers only one of the five things it does. Proposed seams, in descending order of value:

| New module | Items and line ranges | Lines | Why |
|---|---|---|---|
| `scene/sky_lens.rs` | `ScreenCircle` (119-124), `sky_lens_edge_radius` (229-239), `sky_lens_disc` (241-280), `globe_screen_circle` (282-308), `sky_lens_direction` (352-380), `ndc_to_pixels` (724-729), `pixel_scale` (43-47), `MIN_BODY_DISK_RADIUS_PIXELS` (33-41) | ~110 | Highest value. `moon.rs:20` and `crates/sunlit-app/src/mouse_math.rs:7` both import lens geometry from a module named "sun occlusion", which is a wrong-module import today. `tests/engine.rs` and `tests/render_pipeline.rs` reach into it for the same reason. |
| `scene/limb_extinction.rs` | `EARTH_RADIUS_KM` (49-51), `ATMOSPHERE_SCALE_HEIGHT_KM` (53-58), `HORIZON_AIR_MASS` (60-61), `CHANNEL_TRANSMISSION` (63-68), `limb_air_mass` (70-78), `limb_transmission` (80-94), `limb_hue` (96-103), `limb_disk_amplitude` (105-114), `DISK_FADE_GAIN` (116-117), plus tests 761-853 | ~70 + 90 test | Self-contained, has no dependency on anything else in the file, and is the half mirrored by `sphere.wgsl` that `tests/render_pipeline.rs:1539-1557` checks against. |
| `scene/horizon_band.rs` | `REFRACTION_LIFT_ZONES` (310-314), `REFRACTION_EXPONENT` (316-318), `refract` (320-350), `exposure_gain` (404-415), `smoothstep` (417-420), `horizon_zone_width` (493-508), `SunHorizonParams` (166-197), plus tests 884-958 | ~85 + 75 test | The six horizon controls and everything they drive. |
| `scene/disk_occlusion.rs` | `SunVisibility` (126-136), `visibility` (637-679), `angular_visible_fraction` (681-699), `overlap_area` (701-722), plus tests 1046-1174 | ~85 + 130 test | Pure circle-against-circle area math, no sun-specific state. |
| `sun_occlusion.rs` (what remains) | module doc, `SUN_ANGULAR_RADIUS_DEGREES`, `SunPlacementInputs`, `SunPlacement`, `FLUX_STRIPS`, `integrate_disk`, `henyey_greenstein_asymmetry`, `place_sun` | ~330 | Rename to `scene/sun_placement.rs` if the split lands. |

The split is worth it, but it is not free: `tests/render_pipeline.rs:1524-1557` embeds the path `scene::sun_occlusion::…` in five assertion messages, and `docs/rendering.md` names the module. Doing only the first two rows (`sky_lens.rs` and `limb_extinction.rs`) gets most of the benefit for about a third of the churn and would take `sun_occlusion.rs` from 1449 to roughly 1000.

**B2 (medium) `place_sun` is 123 lines (`sun_occlusion.rs:512-635`) and carries `#[allow(clippy::too_many_lines)]`.** Two seams take it to about 60 without touching anything else:

- 548-570, the antipode early return, becomes `fn off_screen_placement(globe, atmosphere, floor, depth) -> SunPlacement`.
- 572-592, the refraction lift and re-imaging, becomes `fn lift_disc(disc, globe, zone, refraction, …) -> (Vec3, ScreenCircle, f32)`.

Both are self-contained blocks with a single entry and exit, so the risk is low.

**B3 (low) `integrate_disk` takes eight arguments (`sun_occlusion.rs:435-445`) and carries `#[allow(clippy::too_many_arguments)]`.** Five of them (`globe_radius`, `zone_width`, `band_height_km`, `squash`, `reddening`) describe the band rather than the disk, and `limb_distance` is derived from `globe_radius` by the same `moved` closure that the caller applies at 601-609. A `struct BandGeometry { globe_radius, zone_width, band_height_km, squash, reddening }` reduces the call to `integrate_disk(center_distance, disk_radius, limb_distance, &band)` and drops the allow. The return `(Vec3, f32)` should become a named struct at the same time; the caller at 613 has to read the function body to know which is which.

**B4 (low) `camera.rs` is not a long file.** Its 742 lines are 279 of production code and 463 of tests. Splitting the file would be wrong; shrinking the test module is the fix (see F1).

### C. Consistency

**C1 (low) module doc headers.** `camera.rs`, `datetime.rs` and `scene/mod.rs` have no `//!` header while `moon.rs`, `sky.rs`, `sun.rs` and `sun_occlusion.rs` do. This is a crate-wide inconsistency rather than a scene one: 13 of the files under `crates/sunlit-core/src` open without a module doc. Worth one line each in the scene modules, and `scene/mod.rs` in particular should say what the module boundary is, since it is six `pub mod` lines with no text.

**C2 (low) `#[must_use]` is applied to 8 of 17 `pub` functions in `sun_occlusion.rs` and to none anywhere else in `scene/`.** `must_use_candidate` is `allow`ed workspace-wide (`Cargo.toml`), so these are hand-placed and the split is arbitrary: `refract` and `exposure_gain` have it, `place_sun` and `visibility` do not. Either apply it to all pure returns in the module or drop it; the current state reads as an unfinished pass.

**C3 (low) const placement.** `sun_occlusion.rs` puts constants next to their first user (`DISK_FADE_GAIN` at 117 *after* the function that reads it at 112; `REFRACTION_LIFT_ZONES` at 314; `FLUX_STRIPS` at 385), while `camera.rs` groups them near the top and `sky.rs` does both (`PLANETS` at 54, `EARTH_RADII_PER_AU` at 96). The next-to-user convention is defensible and I would keep it, but it should be stated once and applied, and `DISK_FADE_GAIN` should at least move above the function that uses it.

**C4 (no action) `unsafe` sites.** There are 10 `unsafe` blocks in `scene/`: `sun.rs` at 40, 46, 63, 109 and `sky.rs` at 104, 125, 155, 194, 208, 383. Every one has a scoped `#[allow(unsafe_code)]` directly on it and a `// SAFETY:` comment immediately above, and each comment states a real argument rather than a formula: the by-value calls say the function is pure C returning a value type with no retained reference, and the three pointer-passing calls say the pointer is a valid time value the library mutates for internal caching. The project rule is met. One refinement: `sky.rs:152-153` covers a two-call tuple that takes two raw pointers from the same `&mut astro_time_t`. That is sound because the calls are sequenced and neither pointer outlives its call, but the comment does not say so, and that is the only part of the argument a reader would want spelled out.

**C5 (low) `#[allow]` vs `#[expect]`.** `scene/` uses `#[allow]` at all 33 sites. There is exactly one `#[expect]` in the whole of `crates/sunlit-core/src`, so `#[allow]` is the house style and this is consistent. Switching would be an improvement (an allow that stops applying goes unnoticed) but it is a crate-wide decision, not a scene one.

**C6 (low) test-module import order.** `sky.rs:244-248` puts `use crate::…` before `use super::*`; `sun_occlusion.rs:733-736` and `moon.rs:120-125` put it after. One line each.

**C7 (low) error handling.** There are no `Result`s in this area; the only failure surface is the five `assert_eq!(x.status, astro_status_t_ASTRO_SUCCESS, …)` in `sky.rs` (105, 126, 161, 195, 209) and the one in `sun.rs` (55). I read `astronomy.c` for the failure modes of the four functions involved: all of them return a non-success status only for an invalid `astro_body_t`, and every body passed here is a compile-time constant from the `PLANETS` table or `astro_body_t_BODY_SUN`/`_MOON`. The asserts are therefore unreachable in practice and are the right shape for that. No change needed.

### D. Duplication

**D1 (medium) `sky.rs` repeats the same FFI epilogue six times.** Every one of `moon_position` (100-111), `moon_rotation` (121-148), `rotation_world_from_eqj` (151-187), `body_direction` (190-201) and `body_magnitude` (204-214) is: one `unsafe` call, one status assert with the function name in the message, then `as f32` on each field, and the whole function carries `#[allow(clippy::cast_possible_truncation)]` for nothing but those casts. Six of `sky.rs`'s eleven `#[allow]` sites exist for this. Two small helpers collapse it:

```
fn vec3_of(v: astro_vector_t) -> Vec3        // one #[allow(cast_possible_truncation)]
fn checked(status: astro_status_t, what: &str)
```

That removes about 20 lines, five `#[allow]` attributes and six duplicated `as f32` triples, and `rotation_world_from_eqj:166-182` (nine hand-written `as f32` casts to build one `Mat3`) shrinks to three calls. This is the clearest answer to the brief's "could cast-lint allows be replaced by a shared helper": yes, here, five of them.

**D2 (low) two spellings of the same pointer cast.** `sun.rs:47-53` and `sun.rs:63` use `&raw mut time`; `sky.rs:125`, `157` and `158` use `std::ptr::from_mut(time)`. Same operation, same C API, two files. Pick one.

**D3 (low) the longitude/latitude to world-frame formula appears three times.** `camera.rs:229-231`, `sun.rs:87-91` and `moon.rs:151` (test) all build `(cos(lat)·sin(lon), sin(lat), cos(lat)·cos(lon))`. The moon test copy is deliberate and documented at 149-150 ("spelled out rather than borrowed, so a change to it fails here"), so leave it. The two production copies could share one `fn world_from_lon_lat(lon_deg, lat_deg) -> Vec3`, which would also give the frame convention a single place to be documented and would fix A1 by construction.

**D4 (low) the same permutation matrix under two names.** `Mat3::from_cols(Vec3::Z, Vec3::X, Vec3::Y)` is `world_from_earth_fixed` in `sky.rs:184` and `mesh_from_body` in `moon.rs:69`. `moon.rs:60-67` already argues that one convention serves both bodies, so a shared `const BODY_FIXED_TO_WORLD: Mat3` would make the doc and the code agree. Low value, one line.

**D5 (low) test helper duplication in `sky.rs`.** Lines 288-290 and 302-304 both build an EQJ direction from right ascension and declination. A three-line `fn eqj_from_ra_dec(ra_deg, dec_deg) -> Vec3` serves both.

### E. Correctness and resilience

**E1 (medium, confirmed) `SunVisibility::transit_fraction` is dead.** It is computed at `sun_occlusion.rs:677` (and at 672 in the degenerate branch), stored in `SunPlacement.visibility`, and read by nothing: `grep -rn transit_fraction crates/ --include=*.rs --include=*.wgsl` returns hits only inside `sun_occlusion.rs` itself, six of which are its own test assertions. `render_pass.rs:189` takes `visibility.visible_fraction` and nothing else, and no uniform carries it. It costs one `overlap_area` call per frame plus the `atmosphere` argument threaded through `visibility`. Either delete the field, the third `overlap_area` call and the six assertions, or add a doc line saying it is kept for a planned consumer. Note that the `atmosphere` circle itself is still needed: `place_sun:572` uses `atmosphere.radius` for `horizon_zone_width`.

**E2 (low, confirmed) `custom_year` is the one datetime field with no range check anywhere.** `AppConfig::sanitize` (`config.rs:340-350`) clamps `texture_resolution`, `camera_fov` and `sun_flare` and nothing else, and `config.rs:1067` asserts that `custom_year = 0` from a TOML file round-trips unchanged. Its two siblings are clamped downstream: `datetime.rs:82` clamps `custom_hour` to [0, 24] and `datetime.rs:41` clamps the day of year to the year's length. `custom_year` goes straight from the file through `params.rs:198` into `Astronomy_MakeTime`. I read `Astronomy_MakeTime` in the vendored `astronomy.c`: it does its date arithmetic in `int64_t` and cannot overflow or panic for any `i32` year, so the worst case is a nonsense sun direction rather than a crash. The UI restricts the field to the current year ±10 (`datetime.rs:9-12`), so this reaches only a hand-edited config. Adding `self.custom_year = self.custom_year.clamp(start, end)` to `sanitize` is a one-line fix; whether it is worth it is a judgment call.

**E3 (no action, checked)** I looked for the usual suspects and did not find them. `refract`'s Newton loop (`sun_occlusion.rs:343-347`) is exact Newton on `f(a) = a - lift·e^(-k·a) - h`, whose derivative is `1 + slope·falloff >= 1` everywhere, so the 12 iterations cannot diverge for any `refraction` the slider (0 to 2, `main.slint:1042-1043`) or a config file can produce. `overlap_area` (702-722) guards every division and clamps every `acos` argument. `hour_float_to_hm` and `hour_float_to_hms` survive a NaN input without panicking (`f32::clamp` panics only on a NaN bound, and float-to-int casts saturate). `place_sun`'s antipode branch (548-570) returns `visible_fraction: 1.0` with a `view_direction` the sky lens has no finite image for, but `vs_sun_glare` and `vs_sun_disk` in `sphere.wgsl` both guard on their own `disc.on_screen` (`sphere.wgsl:1007-1009`), so nothing downstream reads the impossible position. All `SunPlacement` and `MoonPlacement` fields are consumed by `render_pass.rs:100-207`.

### F. Unit tests

**F1 (high) `camera.rs`: 31 tests, roughly 14 distinct behaviors.** The largest single cluster is five tests that all assert "setting a field to the value the constructor already gave it changes nothing", which tests `OrbitalCamera::new` rather than any matrix math: `mvp_with_zero_offset_unchanged` (343), `view_with_zero_rotations_unchanged` (380), `view_with_zero_yaw_pitch_unchanged` (442), `all_rotations_zero_equals_no_rotation` (589). The fourth subsumes the second and third outright. Also:

- Three tests pin defaults or constants, which `CLAUDE.md` forbids ("changing a preset or a default must not break a test"): `camera_params_default_values` (287) pins latitude 30.0 and distance 8.0; `zoom_to_distance_at_zero` (296) and `zoom_to_distance_at_one` (302) pin 1.5 and 80.0 as literals rather than as `ZOOM_DISTANCE_MIN`/`MAX`. `zoom_range_proptest` (664) already does it the right way.
- `zoom_to_distance_at_half` (305) writes the geometric-mean property as `(1.5 * 80.0).sqrt()`; it should be `zoom_to_distance(0.5).powi(2) == ZOOM_DISTANCE_MIN * ZOOM_DISTANCE_MAX`, which is the property and pins nothing.
- Three determinant tests assert the same non-degeneracy: `view_with_tilt_preserves_determinant` (415), `yaw_pitch_preserves_determinant` (573), `view_matrix_determinant_is_nonzero` (656), and `all_presets_produce_valid_mvp` (720) checks it a fourth time.
- `mvp_is_not_identity` (648) asserts that a non-trivial camera does not produce the identity matrix. It cannot fail for any implementation anyone would write.
- `pitch_90_looks_at_surface` (497) does not test what its name says. Its first assertion is that pitch shifts the view by more than 0.1, which `view_with_pitch_shifts_look_target_up` (455) already covers more sharply; its second (`v_offset.y.abs() > 0.1`, line 521) is about the un-pitched camera and has nothing to do with pitch 90.

On the brief's question about zoom round-trips: only one direction is tested. `distance_to_zoom_roundtrip` (328) checks `zoom_to_distance(distance_to_zoom(d)) == d` over six pinned distances. The direction the docs actually claim, `distance_to_zoom(zoom_to_distance(t)) == t` for `t` in [0, 1], is not tested anywhere; the existing proptest only checks the range of the forward map. Folding that assertion into `zoom_range_proptest` costs one line and covers both maps over 256 inputs.

Remove or merge: `camera_params_default_values`, `zoom_to_distance_at_zero`, `zoom_to_distance_at_one`, `distance_to_zoom_default`, `mvp_with_zero_offset_unchanged`, `view_with_zero_rotations_unchanged`, `view_with_zero_yaw_pitch_unchanged`, `view_with_tilt_preserves_determinant`, `view_matrix_determinant_is_nonzero`, `mvp_is_not_identity`, `pitch_90_looks_at_surface`, and the three `eye_at_*` tests merged into one table. **31 down to about 14, and 463 test lines down to roughly 250.**

**F2 (high) `datetime.rs`: 49 tests, about 10 distinct behaviors.** This is the extreme case in the area, 72 percent of the file. Classification of the 43 non-proptest tests:

| Group | Count | Distinct behaviors | Note |
|---|---|---|---|
| `base_year` / `year_range` | 3 | 1 | `year_range_centered_on_current` (147) subsumes both others; `year_range_spans_21_years` (141) asserts the literal 21, pinning a UI constant |
| `is_leap_year` | 5 | 1 | one table of `(year, expected)` rows |
| `days_in_year` | 3 | 0 new | the function is a two-branch wrapper; the proptest `days_366_iff_leap` already covers the relation |
| `day_of_year_to_month_day` | 10 | 1 | real boundary rows (Jan 1/31, Feb 1/28/29, Mar 1 both ways, Jul 1, Dec 31 both ways) but one table serves all ten |
| `month_day_label` | 6 | 2 | the name lookup and the format; the rest re-tests `day_of_year_to_month_day` through a wrapper |
| `hour_float_to_hm` | 7 | 1 | one table |
| `hour_label` | 4 | 1 | zero-padding is the only behavior the wrapper adds |
| `hour_float_to_hms` | 5 | 1 | one table, mirroring `hour_float_to_hm` |

Collapsing each group to one table-driven test keeps every input that is currently exercised and takes **43 down to 8**, with the test module dropping from 321 lines to roughly 120. Nothing is lost, because none of the removed assertions is unique.

The 6 proptests are cheap and worth keeping, with one exception: `doy_month_in_range` (408) is fully subsumed by `doy_day_within_month` (432), which indexes `days_in_month[(month - 1)]` and so panics on any out-of-range month before it can reach its own assertion. That is 5 properties, 1,280 cases, all trivial arithmetic. Runtime is not the problem in this file; the 43 separate harness entries are.

**F3 (high) `sky.rs` holds the heaviest tests in the area.** `four_years_of_times()` (389-398) yields 4 years x 365 days x 4 hours = **5,840 samples**, and two tests consume every one of them: `the_moon_orbits_between_fifty_five_and_sixty_four_earth_radii` (404) and `the_sub_earth_point_stays_inside_the_libration_bounds` (445). Each sample calls `compute_sky_state_from_time`, which makes 15 top-level FFI calls (`Astronomy_Rotation_EQJ_EQD`, `Astronomy_SiderealTime`, `Astronomy_GeoVector` for the Sun, then `Astronomy_GeoVector` + `Astronomy_Illumination` for each of five planets, then `Astronomy_GeoMoon` and `Astronomy_RotationAxis`), and `Astronomy_Illumination` calls `Astronomy_GeoVector` twice more internally for a planet. So the two tests together issue roughly **175,000 top-level ephemeris calls**, of which the eleven planet-and-sun calls per sample are pure waste: neither test reads `sun_direction` or `planets`.

Two independent cuts, either safe:

1. Add a private `fn moon_state_from_time(time) -> (Mat3, Vec3, Mat3)` that computes only `world_from_eqj`, `moon_position` and `moon_rotation`, and point the five moon tests at it. 15 FFI calls per sample becomes 4, a 3.75x cut.
2. Drop the hour grid from `[0, 6, 12, 18]` to `[0, 12]`. Libration and lunar distance vary over a 27-day cycle, so a twice-daily grid over four years still samples about 50 lunations at every phase; the six-hourly grid buys nothing the test asserts. A further 2x.

Together that is roughly a 7.5x reduction on the two heaviest tests in `scene/`. I did not run the suite, so the wall-clock figure is unverified; the call counts above are from reading `compute_sky_state_from_time` (73-89) and `four_years_of_times` (389-398).

**F4 (medium) `sun_occlusion.rs:973`, `a_squashed_disk_against_the_limb_is_a_round_one_against_a_moved_limb`, runs 3.2 million inner iterations.** Four squash values x five offsets x a 400 x 400 grid, with a `sqrt` per in-disk point, in test code compiled at `opt-level = 0` (the dev profile optimizes dependencies but not the workspace crates). Do **not** shrink the grid: the doc comment at 967-972 records that the identity's own error already reaches 0.025 and the assertion tolerance is 0.03, so halving the sampling density would eat the remaining headroom. Two safe cuts instead: hoist the unit-disk mask out of the double loop (the `u`, `v` values and the `u*u + v*v > 1.0` test at 1006-1011 do not depend on `squash` or `offset`, so they are recomputed 20 times for nothing), and reduce the parameter grid from 4 x 5 to 3 x 3, which still spans the slider's range. Roughly a 2.5x cut with the assertion unchanged.

**F5 (low) minor test redundancy elsewhere.**

- `sun_occlusion.rs`: the five point-visibility tests at 1047-1083 could be one table; `a_sun_behind_the_globe_reports_nothing_visible` (1430) restates `a_sun_inside_the_painted_globe_is_fully_hidden` (1054) at the `place_sun` level, and is also the separation-zero end of `the_screen_space_fraction_matches_the_ephemeris_where_the_lenses_agree` (1193). Marginal; I would keep it as the only end-to-end fully-occluded case.
- `moon.rs`: all seven tests call `sky()` (129), recomputing the same `compute_sky_state_from_time` seven times. A `LazyLock` or one shared fixture removes six redundant sky states. Trivial cost, trivial fix.
- `sun.rs`: five tests, no redundancy. `unit_vector` (184) is the only one that could go, since every other test's assertions on a normalized vector imply it.

The two `#[allow(clippy::float_cmp)]` sites the brief asked about (861, 906) are both justified. 862 asserts exact equality because the golden images depend on nothing rounding in the clear-of-the-band path, and says so; 907 asserts that `refraction = 0` is exactly the identity, which is a stronger and more useful statement than an epsilon comparison. Keep both.

### G. Best practices

**G1 (low) no hot-path problems found.** `place_sun` and `place_moon` run once per frame on `Copy` structs, take their inputs by reference, allocate nothing and clone nothing. `PLANETS.map(...)` at `sky.rs:76` builds a fixed `[PlanetState; 5]` with no intermediate `Vec`. There are no `String` parameters, no `collect` into a discarded container, and no boxing in the area. `SunPlacementInputs` is 9 fields of `Copy` scalars and matrices (about 200 bytes) passed by reference, which is right.

**G2 (low) `pub fn visibility` is a poor name in a module that also exports `SunVisibility` and a `SunPlacement::visibility` field.** Since it has no caller outside the file (see the metrics table), making it private and renaming it `split_disk` or `measure_visibility` costs nothing and removes the collision.

**G3 (low) `DateTimeInput` and `make_time` live in the wrong module.** `sun.rs:12-13` says so in as many words: "production plumbing and live here because this is where they started". Six places import them (`sunlit-app/src/ui_callbacks.rs:17`, `params.rs:12`, `sky.rs:12`, `moon.rs:125`, `tests/engine.rs:2855`, `tests/render_pipeline.rs:1692`), and none of them is about the reference sun direction. `scene::datetime` is exactly the module for UI datetime plumbing and currently has no astronomy types at all. Moving both there leaves `scene::sun` as 70 lines of pure reference implementation, gives `datetime.rs` a reason to exist beyond `ComboBox` label formatting, and removes `sky.rs`'s odd dependency on `sun.rs`.

## 4. Recommended refactors, by value over effort

| # | Change | Effort | Risk | Value |
|---|---|---|---|---|
| 1 | Fix the wrong coordinate-frame doc at `sun.rs:22-24` (A1) and the inverted comment at `datetime.rs:43-45` (A2) | 15 min | none | Two statements in the source are currently false |
| 2 | Collapse `datetime.rs`'s 43 unit tests into 8 table-driven ones (F2) | 1 h | very low | 200 test lines gone, no coverage lost, removes the constant-pinning tests |
| 3 | Cut `sky.rs`'s two full-sweep moon tests with a moon-only helper and a twice-daily grid (F3) | 1 h | low | ~7.5x fewer FFI calls in the heaviest tests in `scene/` |
| 4 | Prune `camera.rs`'s test module from 31 tests to ~14, and add the missing `distance_to_zoom(zoom_to_distance(t))` round trip (F1) | 1.5 h | low | 200 test lines gone, removes three default-pinning tests |
| 5 | Delete or document `SunVisibility::transit_fraction` (E1) | 30 min | low | Removes dead per-frame work and six assertions that protect nothing |
| 6 | Split `scene/sky_lens.rs` out of `sun_occlusion.rs` (B1, first row) | 2 h | low | Fixes the wrong-module import in `moon.rs` and `sunlit-app/src/mouse_math.rs`; touches five assertion strings in `tests/render_pipeline.rs` |
| 7 | Add `vec3_of` and `checked` helpers to `sky.rs` (D1) | 45 min | low | 20 lines and 5 `#[allow]` attributes gone |
| 8 | Split `place_sun` along its two seams and drop `too_many_lines` (B2) | 1 h | low | 123-line function down to ~60 |
| 9 | Comment pass: the 6 deletes and 7 moves in section A | 1 h | none | Aligns the area with the maintainer's own rules |
| 10 | Move `DateTimeInput` and `make_time` to `scene::datetime` (G3) | 1 h | low | 6 call sites; makes both modules coherent and removes `sky.rs`'s dependency on `sun.rs` |
| 11 | Split `scene/limb_extinction.rs` out of `sun_occlusion.rs` (B1, second row) | 1.5 h | low | Isolates the half mirrored by `sphere.wgsl` |
| 12 | Hoist the unit-disk mask and shrink the parameter grid in the squash test (F4) | 30 min | medium (tolerance headroom is thin, do not touch the 400x400 density) | ~2.5x on the second-heaviest test in the area |
| 13 | Tighten `integrate_disk`'s signature with a `BandGeometry` struct (B3) | 45 min | low | Drops `too_many_arguments`, makes the call site readable |
| 14 | Remaining `sun_occlusion.rs` splits (B1, rows 3 and 4) | 3 h | medium | Only worth doing if 6 and 11 land well |

Items 1 through 5 are about five hours and carry the great majority of the value.

## 5. What I could not verify

- Wall-clock test times. The brief forbade running cargo, so every runtime claim in F3 and F4 is derived from counting calls and loop iterations by reading, not from measurement. The call counts themselves are verified.
- The exact behavior of the Astronomy Engine C library for extreme (six-digit) year values. I confirmed `Astronomy_MakeTime` uses `int64_t` arithmetic and cannot overflow or panic for an `i32` year; I did not trace whether the VSOP87 series then produce a NaN that would reach the uniform buffer.
- Whether `transit_fraction` is reserved for planned work. I confirmed it has no reader today; the maintainer may know a reason to keep it.
