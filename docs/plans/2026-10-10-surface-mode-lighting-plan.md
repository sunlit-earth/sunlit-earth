# Plan: Uniform Lighting in the Day and Night Modes

## Summary

The Rendering group's "Texture" combo offers Grid, Day, Night and Day/Night Blend. Day and Night are meant to show one map lit evenly, but today they still show a terminator. `fs_main` already draws the surface flat in those modes (it returns early on the `terminator_width < 0` sentinel), yet every shell drawn over the globe shades against the real Sun: `fs_cloud` darkens and thins the clouds on the night side, `fs_rayleigh` draws the blue only on the day side with the orange band at the terminator and the forward-scattering lobe at the limb, and both nightglow shells light only the night side. The result is a flat map with a night hemisphere painted over it by the shells.

After this change:

- Day: day lighting everywhere. Clouds are day clouds over the whole globe, the Rayleigh shell is its day blue all the way round, no orange terminator band, no nightglow.
- Night: night lighting everywhere. Clouds are night clouds over the whole globe, no Rayleigh color (the night side's fade), nightglow all the way round.
- Grid and Day/Night Blend: unchanged, pixel for pixel. The grid keeps its flat surface under shells that follow the Sun, which is what makes it useful for checking the Sun's position and is what the golden suite pictures.
- In Day and Night the Rayleigh shell's forward-scattering (sunrise) lobe is off, since it is a terminator effect. The Sun's disk and glare stay as they are in every mode: they are sky objects. The Moon keeps its own phase lighting in every mode.
- The combo's label becomes "Surface", with a hint rewritten to match. Its four entries keep their names.

## Stakes Classification

Low. The shader change is small and local, Grid and Blend do not change, and no golden reference should move: the one golden case in Day mode, `africa_on_the_z_face`, has the atmosphere off and the clouds at zero opacity (from `base_params`), and no golden case renders Night. If a golden reference does move, that is a finding to investigate, not a reference to re-bless.

## Decisions

### 1. The lighting follows the selected mode, not the resolved bind group

`texture_routing::resolve_textures` can draw something other than the selected mode while cubes load: Blend falls back to the day floor alone until the night floor is resident (with `use_blend` false), and Night and Day draw the grid until their floor arrives. The shells' lighting is chosen from `TextureMode::from_index(params.texture_index)`, the user's selection, so a mode switch reaches the shells at once and the surface catches up as its cubes land. In particular Blend's loading fallback keeps the Sun-driven shells it has today. The surface's own sentinel and `FLAG_NIGHT_ALONE` keep following the resolved group, as now.

### 2. One shader helper decides the lighting for every shell

The lighting choice travels as flag bits in the existing `flags` word (bit 0 is diffuse shading, bit 2 the night drawn alone; check that the bits taken are free, and name the constants on both sides as `FLAG_NIGHT_ALONE` is named). No new uniform field, so the block's layout and size stay as they are. Grid and Blend set neither bit.

`sphere.wgsl` gets one helper, for example `fn light_n_dot_l(n: vec3<f32>) -> f32`, returning `dot(n, uniforms.sun_dir)` under the Sun, `1.0` for uniform day and `-1.0` for uniform night. `fs_cloud`, `fs_rayleigh`, `fs_nightglow_orange` and `fs_nightglow_green` take their `n_dot_l` from it. With those values the existing curves already give the wanted result: at `1.0` the clouds' `sunlit` is 1, Rayleigh's `day_t` is 1, `term_t` is `exp(-125)` and `night_fade` 0, and both nightglow masks are 0; at `-1.0` the clouds' `sunlit` is 0, Rayleigh's `night_fade` is 1, and the nightglow masks are 1 with the midnight `time_mod` (0.5 orange, 1.0 green). Verify each of these in the shader as written rather than trusting this paragraph.

The Rayleigh lobe (`sunrise` in `fs_rayleigh`) is gated off in uniform day and night explicitly, since at `1.0` its own `sunlit` gate would let it through. `fs_main`'s specular and Fresnel terms are unaffected: they sit after the single-texture early return. `fs_moon` keeps reading `uniforms.sun_dir` directly.

The sentinel cannot carry this: Grid sets it too, and Grid keeps the Sun-driven shells.

### 3. The label

`main.slint`'s `SettingCombo` label "Texture" becomes "Surface", and the hint says what the choice now means: the procedural grid under the real Sun, the day map lit everywhere, the night map everywhere, or the two blended across the terminator by the Sun's position. Keep it to the length of the neighboring hints. The four option strings stay. `slots::TEXTURE_LABELS`, the `texture-options`, `texture-index` and `texture-changed` identifiers and the `texture_index` config key keep their names: renaming the key would need a `config/migrate.rs` step for no user-visible gain, and renaming the identifiers is churn across the app and its tests. Internal "texture mode" naming stays.

## Tests

Behavior, not constants (CLAUDE.md, Testing conventions).

- A GPU case in whichever target already drives the shells with explicit uniforms (`tests/render_pipeline.rs` or `tests/shading.rs`, whichever fits): with the Sun on one side, uniform day draws the cloud shell and the Rayleigh shell alike on the sunward and the anti-sunward hemisphere (sample symmetric regions and compare), uniform night likewise, and the Sun lighting still differs between them. Nightglow present on both hemispheres in uniform night and absent in uniform day. The Rayleigh lobe absent in a uniform mode at a camera where the Sun lighting shows it.
- An engine case (`tests/engine/`) that sets `texture_index` and checks the routing end to end, in the style of `night_mode_reads_the_night_half_of_the_page_table`: for example clouds over the night side read as day clouds in Day mode and as night clouds over the day side in Night mode, while Grid still shows both. Reuse `tests/engine/clouds.rs`'s fixture cloud source if it fits.
- A unit test for the function in `render_pass.rs` (or wherever it lands) that maps the mode to the flag bits, including that Grid and Blend set neither, if that logic is factored out.
- Check the `tests/engine/clouds.rs` cases that take a `texture_index`: any that asserts night-side cloud behavior in Day or Night now asserts the old behavior and must move to Blend or Grid, with the reason recorded as a departure.
- The golden target must pass unchanged on WARP (`cargo test -p sunlit-core --test golden`). Do not set `SUNLIT_EARTH_UPDATE_GOLDEN`.

## Documentation

- `docs/rendering.md`: the `sphere.wgsl` bullet (the sentinel sentence gains the shells' lighting), the cloud paragraph that mentions the sentinel, and a short subsection on the modes: what each shell does in Day and Night, why the lobe is off there, why Grid keeps the Sun, why the Sun and Moon are untouched, and decision 1.
- `docs/architecture.md`: wherever it describes the texture mode or `flags`.
- `docs/roadmap.md`: a checked entry under "Bugs and polish" in the style of its neighbors, linking this plan.
- `README.md` is not edited (CLAUDE.md, Workflow). If it names the "Texture" setting, record that under Open items for the orchestrator.

## Gates

Run in the worktree, all green before declaring done:

```bash
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```

The orchestrator dispatches `ci.yml` with `os: all` on the branch at wrap-up, which runs the golden suite on lavapipe and Metal as well.

## Safety

- Work only in `C:\Workspace\rustrover\sunlit-earth\.worktrees\surface-mode-lighting`, on branch `fix/surface-mode-lighting`. Write nothing into the main checkout.
- No VM commands (`cargo xtask vm ...`, `cargo xtask e2e`, `cargo xtask dist`), no elevated commands, no `cargo e2e` (it takes the user's screen over).
- No WSL builds are needed; if one becomes necessary, follow CLAUDE.md's `CARGO_TARGET_DIR=$HOME/sunlit-target-surface-mode-lighting` rule and `cargo sweep` afterwards.
- No pushes, no workflow dispatches, no PRs. Commit progress on the run branch in reasonable chunks; no history rewrite.

## Departures

Numbered, with reasoning, appended here as they happen.

1. `tests/engine/clouds.rs` had `a_dayside_cloud_is_brighter_than_a_night_side_one_in_every_mode`, which walked Grid, Day and Night and asserted that the deck reads brighter at noon than at midnight in each. That is the old behavior in Day and Night, as the Tests section anticipated. It now covers Grid alone, as `a_dayside_cloud_is_brighter_than_a_night_side_one_over_the_grid`, which keeps what it was written for: the grid carries the `terminator_width` sentinel and keeps the Sun-driven shells, so it still proves `fs_cloud` does not read the sentinel. Blend was not added, since its ground changes between the two hours and would confound the reading. Day and Night moved to the new case `day_and_night_light_the_clouds_evenly`.

## Validation rounds

Round 1, over 752a902..f41018a: no MAJOR and no MINOR findings. The validator reran the gates and confirmed that five temporary mutations each fail a new test: the flags dropped from `write_uniforms`, the lobe gate removed, the green nightglow put back on `dot(n, sun_dir)`, the signs in `shell_n_dot_l` swapped, and Blend mapped to the uniform day bit. One observation, not a finding: no test pins decision 1 directly (a uniform mode selected while its floor is still loading); the single call site and the unit test over `shell_lighting_flags` hold it structurally.

## Open items
