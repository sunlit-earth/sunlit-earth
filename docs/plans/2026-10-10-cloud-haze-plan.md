# Plan: Haze Removal and Land and Ocean Cloud Settings

## Summary

The cloud map is mostly a temperature map, so cold clear ground reads as a grey veil, and how grey depends on the region: near zero over tropical ocean, around 0.8 over mid-latitude land in October, with real cloud as little as 0.08 above it ([2026-10-10-cloud-haze-research.md](2026-10-10-cloud-haze-research.md), sections 1 to 4). One global floor cannot remove it. This plan does four things:

1. **Haze removal.** The cloud worker estimates the clear-sky level of each map at 512 x 256 from the map itself, and `fs_cloud` subtracts it as a local floor, with a strength the user sets.
2. **Land and ocean settings.** `fs_cloud` reads a soft land weight from the water mask cube, and the floor, the gamma and the haze removal strength each come in a land and an ocean value, blended by that weight.
3. **A switch for the whole layer.** "Show clouds" stops drawing, and while it is off nothing is downloaded or decoded.
4. **A bug fix.** `sunlit-earth render` and `sunlit-earth displays --out` wait for the cloud layer before they save, instead of usually saving a globe without clouds (research section 8).

It is meant to be tried and tuned. The defaults below come from a Python prototype on one map and are expected to move during validation.

## Stakes Classification

Medium. It changes the look of every wallpaper, adds a texture and a mailbox slot, changes the shared bind group layout, adds an engine event, migrates the config, and will move golden references. Nothing in it touches the tile path, the wallpaper sinks or the display plan.

## What it measures to today

`fs_cloud` applies one floor and one gamma to the map:

```wgsl
let floored = saturate((raw - uniforms.cloud_floor) / max(1.0 - uniforms.cloud_floor, 0.001));
let cloud_density = pow(floored, 1.0 / max(uniforms.cloud_gamma, 0.01));
```

What follows `cloud_density` lights the deck: since #72 (`2026-10-10-surface-mode-lighting-plan.md`) `fs_cloud` takes its `n_dot_l` from `shell_n_dot_l`, so in the Day and Night surface modes the deck is lit evenly as day or as night, and in the grid and the blend by the Sun. That part is downstream of everything this plan changes and stays as it is.

Defaults are floor 0.25 and gamma 0.65, day opacity 0.85. A clear pixel of mid-latitude land at 0.79 comes out at an alpha of 0.51 (research section 3). The cloud bind group is built by `flat_bind_group`, which binds dummy cubes at 4, 5 and 6, so the cloud draw cannot see the water mask today. Binding 3 of the shared layout is unused. The cloud worker posts the cached map as soon as it is spawned and polls whenever the engine asks, which it does on the hourly schedule whatever the settings say. `SceneParams::draws_clouds` is the draw gate, true while either opacity is above zero.

The gamma slider's hint says "below one lifts the thin cloud, above one keeps only the dense tops". The code does the opposite: the exponent is `1 / gamma`, so a gamma below one pushes the thin cloud down. The hint is fixed with the rest of the slider rows.

## Key Design Decisions

### 1. The background estimate is computed on the CPU, in the cloud worker, from each map

A new module, `assets::cloud_haze`, holds one pure function from a decoded map to a 512 x 256 one-channel image, and the constants it uses:

| constant | value | meaning |
|---|---|---|
| `GRID` | 512 x 256 | about 78 km a pixel at the equator |
| `PERCENTILE` | 15 | share of a window assumed to be clear sky |
| `WINDOW` | 9 x 9 grid pixels | about 700 km at the equator |
| `SMOOTH_SIGMA` | 3 grid pixels | about 230 km |
| `CAP` | 0.85 | the largest background allowed |

The steps are those of research section 6: box-average reduction to the grid, the percentile over the window, a Gaussian blur, the cap. Both filters wrap across the antimeridian and clamp at the poles. The reduction is an area average that accepts any input size, because `SUNLIT_EARTH_CLOUD_URL` can serve one that is not a published variant.

`CloudUpdater::post` computes it from the decoded map before posting either, so the cached map and every download get one. The worker thread is where the decode already happens, so the engine thread never sees the work. None of these are settings (decision 7).

Not chosen: computing it on the GPU (a percentile filter in a compute or render pass is far more code for a 512 x 256 result), and recomputing it when a setting changes (no setting feeds it).

### 2. It travels in its own mailbox slot and its own texture

`SlotLayout` gains `cloud_background()`, the slot after the clouds, and `count()` grows by one. The worker posts the background first and the map second, both with no generation, like the map today. `process_decoded_textures` treats the new slot like the cloud slot: it creates an `R8Unorm` texture labelled `cloud_background_texture`, keeps its view, and rebuilds the cloud bind group.

The two posts land in the same drain in practice, since they are made microseconds apart. If a drain falls between them, one frame pairs the new background with the previous map, which is a background from an hour earlier and harmless. The background is not counted in `textures_ready` or `textures_pending`, as the map is not.

Not chosen: a second channel in the cloud texture. `RG8Unorm` at 8192 x 4096 would double the 42.7 MiB the overlay costs, to carry what fits in 128 KiB.

### 3. Binding 3 carries the background, and the cloud bind group carries the water mask

The shared bind group layout gains a filterable 2D float texture at binding 3. Every existing bind group passes the 1x1 dummy texture there, which reads zero in red, so a missing background is a background of zero: today's picture.

The cloud bind group becomes a bind group of its own kind: the map at 1, the cloud sampler at 2, the background (or the dummy) at 3, the dummy cubes at 4 and 5, and the water mask at 6 (or the dummy cube). It is rebuilt when the map lands, when the background lands, and in `rebuild_surface_groups`, which is where a newly resident mask is noticed.

### 4. The land weight is the water mask at a coarse level

```wgsl
let mask_levels = log2(f32(textureDimensions(water_cube).x) / 128.0);
let water = textureSampleLevel(water_cube, sphere_sampler, equi_angular(n), max(mask_levels, 0.0)).r;
let land = 1.0 - water;
```

The mask's finest level is 1024 px a face, so the level read is 3, 128 px a face, about 80 km a texel, which is the coastline softness the prototype used. Reading the level from the texture's own size keeps it right for the 8 px mask of the test fixture. `textureSampleLevel` is an explicit level, not an explicit gradient, so it does not reach the `gradient2d` restriction that keeps the globe off `textureSampleGrad` on Metal; risk 2 has what is still to confirm.

Where no mask is resident the dummy cube reads zero, which makes every pixel land. That happens in a session without the cube textures and in the preview before the mask's pack is built. A blend mode export waits for the mask already, since `surface::needs` lists it for blend. Day and Night modes do not list it, so `needs` adds the mask to those two modes while clouds are drawn, which takes the cloud gate as an argument; that the mask is made resident in those modes at all is checked first in the implementation, and if it is not, it is made so.

### 5. The density formula

```wgsl
let floor = mix(uniforms.cloud_floor_ocean, uniforms.cloud_floor_land, land);
let gamma = mix(uniforms.cloud_gamma_ocean, uniforms.cloud_gamma_land, land);
let haze = mix(uniforms.cloud_haze_ocean, uniforms.cloud_haze_land, land);
let background = textureSample(cloud_background, sphere_sampler, in.uv).r;
let local_floor = 1.0 - (1.0 - haze * background) * (1.0 - floor);
let floored = saturate((raw - local_floor) / max(1.0 - local_floor, 0.001));
let cloud_density = pow(floored, 1.0 / max(gamma, 0.01));
```

Subtracting the background and then the floor is one subtraction of `local_floor` (research section 7), so the shader does it once. With both haze strengths at zero and the land and ocean values equal, `local_floor` is the old floor and the result is today's to the bit, which the tests hold. Everything after `cloud_density` (the lighting through `shell_n_dot_l`, the terminator, the night opacity) is unchanged and applies to both surfaces, so in the Night surface mode the haze removal thins the deck drawn at the night opacity just as it thins the day deck.

The background is sampled with the cloud sampler, which repeats in u and clamps in v, so it wraps at the antimeridian as the map does. It is a 2D texture, so the cloud sampler's anisotropy is the one it was made for.

### 6. Settings

| key | label | range | default |
|---|---|---|---|
| `clouds_enabled` | Show clouds (checkbox) | | on |
| `cloud_haze_land` | Land: Haze removal | 0 to 100% | 100% |
| `cloud_floor_land` | Land: Floor | 0 to 50% | 10% |
| `cloud_gamma_land` | Land: Gamma | 0.1 to 2.0 | 0.65 |
| `cloud_haze_ocean` | Ocean: Haze removal | 0 to 100% | 15% |
| `cloud_floor_ocean` | Ocean: Floor | 0 to 50% | 10% |
| `cloud_gamma_ocean` | Ocean: Gamma | 0.1 to 2.0 | 0.65 |

`cloud_floor` and `cloud_gamma` go. `cloud_opacity`, `cloud_opacity_night` and `cloud_night` stay shared. The six numbers are shader parameters and take the fourteen edits each (CLAUDE.md, Architecture in brief), all in the `quantized` digest group. `clouds_enabled` is not a uniform: it is a `SceneParams` field in the `switched` group, like `diffuse_shading`, and `draws_clouds` becomes `clouds_enabled && (cloud_opacity > 0 || cloud_opacity_night > 0)`.

The panel follows the Atmosphere group's pattern: a `SettingCheck` for "Show clouds", and under `if root.clouds-enabled` the three shared sliders, then a `SettingHeading` "Land" with its three, then "Ocean" with its three.

### 7. What stays a constant

The cap, the percentile, the window and the smoothing (decision 1) and the mask level (decision 4). The cap is the most likely to become a setting, because it decides how much a large real deck over land is thinned (research section 7); it is promoted only if validation shows the haze removal strength cannot do that job. The other three define the estimate rather than taste, and changing them would mean recomputing it. A polar fade, which would keep the haze removal off the mirrored fill near the poles, is left out and judged during validation.

### 8. Turning clouds off stops the downloads

The engine does not hand a poll to the worker while `clouds_enabled` is false. The schedule keeps running, so a poll that falls due while the layer is off is owed, and turning the layer on sends it at once.

The worker posts the cached map on its first request rather than at spawn. With clouds on, the first request is the engine's first tick, so nothing moves. With clouds off at start, the session decodes nothing until the layer is turned on.

Turning the layer off does not free a cloud texture that is already resident. The Moon and the Milky Way keep theirs when switched off for the same reason: switching back is instant and costs no decode. Freeing it would mean the worker has to post the cache again on the next enable, which is more state for 42.7 MiB that the user chose to load once.

### 9. Migration 1 to 2

A file at version 1 has `cloud_floor` and `cloud_gamma`. The step reads both, treating a missing key as its default:

- Both at the old defaults (floor 0.25, gamma 0.65): remove the two keys. The six new keys are missing, so serde fills them with the new defaults, haze removal on.
- Anything else: write the old floor to both new floor keys and the old gamma to both new gamma keys, set both haze strengths to 0, and remove the two old keys. The clouds look exactly as they did.

`clouds_enabled` is missing in every version 1 file and gets its default, on. `ChangeLog` records a key going missing as well as one being set, which takes `Change::to` becoming an `Option`. The rule in `docs/architecture.md` on default changes applies to the six new keys from here on: once released, changing one of their defaults takes a step of its own.

### 10. The one-shot commands wait for the cloud layer

`run_render` in `crates/sunlit-app/src/headless.rs` waits for `TexturesReady` and then exports. The clouds are an overlay, outside texture readiness, so with a warm tile cache the export usually lands before the cached map is decoded and uploaded: in the debug build one render of nine had clouds (research section 8). `run_displays` with `--out` sends `RenderWallpaperNow` at once and has the same race. Both are one-shot commands whose only output is the picture, so both wait.

The engine gains `EngineEvent::CloudsSettled { drawn: bool }`, sent once per engine:

- `drawn: true` when the cloud map and its background have both been uploaded. `process_decoded_textures` knows which slots it applied, and the engine asks the renderer after each drain whether the cloud bind group exists.
- `drawn: false` when the worker's first request ends its first attempt with nothing to show: no cached map, and the poll failed. The worker records that in a shared state beside `busy` and `target` (pending, posted, unavailable), set before it sleeps out its backoff, and the engine's tick reads it. The worker keeps retrying as before; the event only says the first answer was no.

`run_render` waits for the event after `TexturesReady`, and `run_displays` before it sends `RenderWallpaperNow`, whose publish already holds for the textures. Both waits are bounded by a timeout of their own, 60 s, which covers an 8192 x 4096 download of about 5 MB on a slow link and its decode in a debug build. A timeout or `drawn: false` logs a warning naming why and exports without clouds, as today. Neither waits when no cloud is drawn: with `SUNLIT_EARTH_NO_CLOUDS` set there is no cloud source, and with `clouds_enabled` off or both opacities at zero `draws_clouds` is false; in either case the event never comes, so the wait is skipped rather than timed out.

The app's own publishes do not change. The engine keeps treating the clouds as an overlay a wallpaper does not wait for, because a session publishes again within the hour and a session offline would otherwise never publish.

Not chosen: making the clouds part of `TexturesReady`. That event gates every publish of the running app, which the paragraph above keeps as it is.

### 11. Not chosen

- A clear-sky composite over time (research section 5). The better estimate, kept for the own pre-processor that is to replace matteason.
- A CPU land mask baked into the processed map. The mask lives on the GPU as cube packs, and baking would fix the land and ocean strengths at decode time instead of leaving them live.
- Settings for the estimate's internals (decision 7).

## Relation to the sea ice plan

`feature/sea-ice` holds a plan of the same date, not yet implemented, that touches three of the same places. Whichever lands second takes the first's into account; none of them is a conflict of design.

- **Slots.** It adds a slot after the Milky Way, moving the clouds from 3 to 4. Decision 2's background slot comes after the clouds wherever they are, which `SlotLayout` derives.
- **Bindings.** It adds binding 9 to every bind group and the water mask to the day-alone group. Decision 3 adds binding 3 and its own cloud bind group; the two lists of bindings merge.
- **The water mask.** Sea ice is water to the mask, so the land weight treats it as ocean and gives frozen sea the ocean's haze removal. Frozen sea is cold, so it reads like cold land to the IR channel, and a winter Arctic may keep a veil. If validation shows it, the land weight can take the ice concentration as land once the sea ice atlas exists; until then it is an open item.

## Success Criteria

1. With both haze strengths at 0 and equal land and ocean values, the cloud layer renders exactly as today's.
2. At the defaults, over the research's two validation views and over Europe and Asia in daylight, the grey veil over clear land is gone and the cloud bands over land remain; the ocean looks as it does today. Judged by the user in the running app.
3. With "Show clouds" off, the engine makes no download and no decode, and draws no cloud; turning it on draws the cached map at once and polls.
4. A version 1 file at the old defaults loads with the new defaults, and one with a customized floor or gamma loads with the same picture as before.
5. The estimate takes under 100 ms on the 8192 x 4096 map in a release build, measured, on the worker thread.
6. The cloud draw's cost on WARP and lavapipe at 1920 x 1080 is measured against today's 16 ms on WARP and recorded; an increase of more than a quarter is a finding to discuss before merging.
7. `sunlit-earth render` with a warm tile cache and a cached cloud map saves a globe with clouds every time; with no cached map and no network it saves one without clouds as soon as the first poll fails, with a warning; with clouds off it does not wait.
8. `cargo test`, `cargo clippy --all-targets` and `cargo fmt --check` pass, with the golden sets regenerated where they moved and the moves recorded.

## Implementation Steps

### Step 1: The estimate

`assets::cloud_haze` with the function and constants of decision 1, and its unit tests (Tests, 1). `CloudUpdater::post` computes it and posts it to the new slot before the map.

### Step 2: The slot, the texture and the bindings

`SlotLayout::cloud_background`, the mailbox count, the texture creation in `process_decoded_textures`, binding 3 in the layout and in every bind group, the cloud bind group of decision 3 and its rebuilds, `slot_label`. The memory report gains the `cloud_background_texture` row by its label.

### Step 3: The shader and the settings

The six uniforms in `uniform_block!` and `sphere.wgsl` in place of `cloud_floor` and `cloud_gamma`, the binding, the land weight and the formula in `fs_cloud`. `clouds_enabled` and the six fields through `AppConfig`, `SceneParams`, `scene_digest!`, `write_uniforms`, the two translation functions and `main.slint`. The gamma hint corrected.

### Step 4: The switch and the polls

The poll gate in the engine's tick, the deferred cached post in the worker, `draws_clouds`, and the mask in `surface::needs` for Day and Night while clouds are drawn (decision 4).

### Step 5: The one-shot commands

`EngineEvent::CloudsSettled`, the worker's shared state and its read in the tick, and the wait in `run_render` and `run_displays` (decision 10).

### Step 6: Migration

`v1_to_v2` in `config/migrate.rs`, the optional `Change::to` and the log line for a removal.

### Step 7: Tests and goldens

Tests below. Golden references regenerated where they moved: WARP on this machine, lavapipe and Metal from the golden workflow, each move recorded in `docs/testing.md` as earlier regenerations were.

### Step 8: Measurement and documentation

The figures of criteria 5 and 6. `docs/rendering.md` gets a section on haze removal and the land weight beside "Clouds on the night side". `docs/architecture.md` gets the slot, the cloud bind group, the worker's behavior while clouds are off, and `CloudsSettled` beside the other events. `docs/testing.md` gets the new cases. `docs/roadmap.md` closes "Make clouds less intense over land" and records the fix of decision 10 among the bugs. README's description of `render` says nothing about clouds, so it needs no change.

### Step 9: Validation with the user

The user runs the app and tunes the defaults. Changed defaults land before the release, so they need no migration step of their own.

## Tests

1. **The estimate, unit tests in `cloud_haze`.** A uniform map gives a background of its value, capped at `CAP`. A haze level with dense cloud over half the map in patches smaller than the window gives a background near the haze level. Rotating the input by half its width rotates the output by half its width, which holds the wrap. Every published variant and a size that is no variant (1000 x 500) give 512 x 256.
2. **The shader, in `tests/render_pipeline.rs` or `tests/shading.rs`, wherever the harness reaches the cloud draw with a mask and a background.** With both haze strengths at 0 and equal land and ocean values, an all-water mask and an all-land mask render the same frame. With the land floor at 1 and the ocean floor at 0, cloud is drawn over the water pixels and none over land. With a background equal to the map, haze removal at 1 removes the cloud and at 0 leaves it. The mutation check: each case fails with `land` and `water` swapped in `fs_cloud`.
3. **The engine, in `tests/engine/clouds.rs`.** Started with clouds off, the fixture source serves no fetch over several hourly schedules on the mock clock and no cloud is drawn; turned on, one fetch follows and the cloud is drawn. Turned off again, no fetch follows when the next poll falls due. The case that uses `FixtureClouds::serve_uniform` for the night opacity sets both haze strengths to 0, since a uniform map is all background.
4. **Migration, unit tests in `config/migrate.rs`.** The default path, the customized path, a file with one of the two customized, a file missing both, and a version 2 file left alone.
5. **The settings.** The digest's mutation table covers the six fields by construction; `draws_clouds` is false with `clouds_enabled` off and either opacity up. `tests/slint_ui.rs`'s round trip lists the new properties.
6. **The soak.** It assumes one decoded frame per download (`made_before` in `tests/soak.rs`, and `wait_for_decoded_frames_to_go`), and each download now makes two, the map and its background. The arithmetic follows; the checks that no frame is alive after each hour stay as they are, which is what catches a background kept by mistake.
7. **Readiness.** In Day mode with clouds drawn, an export waits for the mask; with the cloud layer off it does not (decision 4).
8. **The one-shot commands.** In `tests/engine/clouds.rs`: with the fixture source and no cache, `CloudsSettled { drawn: true }` arrives once and an export made after it has cloud pixels where the bands are; with a source that fails and no cache, `CloudsSettled { drawn: false }` arrives before the first retry's delay has passed; with clouds off, no event arrives and the source is never asked. In the app crate, the wait is a function of the event receiver and the timeout, unit tested for the three outcomes without an engine.
9. **Goldens.** `clouds_across_the_terminator` and `cloud_terminator_close_up` draw the banded fixture, 255 or 0, over the Earth fixture's mask. Inside a band the background reaches the cap, which leaves 255 at full density, so the moves should be at the band edges; whatever moves is regenerated and recorded.

## Out of Scope

- Freeing the cloud texture while clouds are off (decision 8).
- Any change to the cloud source, the poll interval or the cache.

## Risks and Mitigations

1. **Large real decks over land are thinned.** The estimate takes the inside of a deck wider than its window for ground (research section 7). Mitigated by the cap and the land strength; if validation shows it matters, the cap becomes a setting (decision 7).
2. **The mask read on a software adapter or on Metal.** The cloud sampler has 16x anisotropy, and the texture overhaul research found any anisotropy on a cube expensive on lavapipe where the frame crosses a face edge. An explicit level should take derivatives and anisotropy out of the read, but that is not verified for lavapipe or WARP. Criterion 6 measures it. If it costs, the cloud bind group can carry the surface sampler at a binding of its own for the mask. Metal is covered by CI's Metal runs and by `metal_is_never_asked_for_a_cube_gradient`, which still translates the whole shader.
3. **Coastlines.** A deck crossing a coast changes how it is drawn over about 80 km. Judged in validation; the level in decision 4 is the knob.
4. **The poles.** The haze removal may empty the mirrored fill near the poles over Antarctica and Greenland. Judged in validation; a polar fade is the answer if needed (decision 7).
5. **Estimate cost in debug builds.** The tests and `cargo run` use debug builds, where the filters are slower. It runs on the worker thread, so it delays the clouds rather than the frame; criterion 5 measures release, and the debug figure is recorded beside it.

## Rollback Strategy

The change is one feature branch. Before release, reverting it is enough. After release, a version 2 file read by an older build loses its six new keys to serde's defaults (the older build ignores them and has its own `cloud_floor` and `cloud_gamma` defaults), which is the documented behavior for a file from a newer build.

## Departures

1. **The blend and the local floor are written for exactness.** Decision 5 writes `mix(ocean, land, weight)` and `1 - (1 - haze * background) * (1 - floor)` and says that with both strengths at 0 and equal land and ocean values the result is today's to the bit. In floating point neither form guarantees that: `mix` of two equal values at a weight between 0 and 1 can come back an ulp off, and `1 - (1 - floor)` is not `floor` for most floors that are not dyadic. `fs_cloud` therefore blends through `by_land(ocean, land_value, land) = ocean + (land_value - ocean) * land` and computes `local_floor = floor + haze * background * (1 - floor)`. Both are algebraically the plan's expressions, and both return the old floor and gamma exactly in that case, which is what criterion 1 asks for.

2. **The percentile is taken on the map's 8-bit levels.** The prototype runs scipy's `percentile_filter` on the float grid. A selection among 81 floats for each of the 131072 grid pixels was estimated, not measured, to come near the 100 ms bound of criterion 5 on its own, so `cloud_haze::percentile` rounds the reduced grid to 256 levels and slides a histogram along each row, with the rank scipy takes (12 of 81). Rounding is monotonic, so the answer is the float percentile rounded to its level, at most half a level from it before the blur, and the background is stored as `R8Unorm`, so nothing finer would have reached the shader. The estimate takes 11.9 ms in release on the 8192 x 4096 map.

3. **The first request posts the cache through a retarget when there is one.** Decision 8 moves the cached post from spawn to the first request. A resolution switched before that request would then have posted twice, the old variant's entry and then the new one's from `set_resolution`, which is two decodes of up to 8192 px. `CloudUpdater::set_resolution` now returns whether it moved to another cache entry, and the first request posts the cache itself only when it did not. `has_posted` on the updater is what the worker's first answer (decision 10) reads.

4. **Readiness takes the scene, and the loading line counts the mask.** `Renderer::textures_ready`, `textures_pending` and `cubes_waiting` take `&SceneParams` instead of the texture index, so `surface::needs` can be given `draws_clouds` (decision 4). The loading line's `cubes_waiting` counts a waiting mask whenever `needs` lists it, on the day side in Day and Blend and on the night side in Night, so a Night session waiting for the mask says so instead of reading as loaded. The mask is made resident in every mode already (`SurfaceFeed::land` installs it whatever the mode), which decision 4 asked to check first.

5. **One of the three shader cases cannot fail with land and water swapped.** Test 2 asks that each case fail with `land` and `water` swapped in `fs_cloud`. The first case, equal land and ocean settings with no haze removal over an all-water and an all-land mask, is symmetric in the two by construction, so the swap leaves it passing. The other two fail with the swap on WARP, which was checked with a temporary edit of `sphere.wgsl` and recorded in testing.md.

6. **Criterion 6 is measured with a harness of its own, and not on lavapipe.** The 16 ms the plan compares against has no recorded scene or harness, so the cost was measured before and after with the same throwaway test, not committed: Day mode on the cube surface built from `textures/`, the cached 8192 map served by a fixture source, the median of 60 alternating pairs of 1920 x 1080 exports with and without the clouds, in a release test build on `warp`. The old shader measures 26.4 to 33.7 ms in four runs and the new one 30.3 to 36.3 ms in five, about 3 ms or a tenth more, below the quarter that would have been a finding. This harness puts the old shell at about 30 ms rather than 16, so the 16 ms figure is left where it is and not compared. Lavapipe is reached on this machine only through WSL, which this run may not use, so its figure is left to CI or the user.

7. **The real binary was not run for criterion 7.** `logging::file_writer` writes a log file under the app data directory whenever stderr is not a terminal, which it never is from the tools this run has, and its rolling appender also prunes old log files there. That directory is `C:\Users\mcp\AppData\Local\SunlitEarth`, which this run must not touch beyond reading the cloud JPEG. So `sunlit-earth render` was not run here; the behavior criterion 7 describes is held by the engine cases (`CloudsSettled` drawn once after the map and the background land, `drawn: false` before the first retry with no cache and a failing source, no event and no fetch with the layer off) and by the app's unit tests of the wait and of when it is skipped. A run of the binary with a warm tile cache is left to validation.

8. **The switch reaches the worker, and a download that returns while it is off is dropped.** Decision 8 only keeps the engine from handing the worker a poll while the layer is off. That leaves a poll that is already retrying after a failure going on: the worker backs off for up to five minutes between attempts and never saw the setting, so an outage that began with the layer on kept contacting the source after it was switched off, and once the network returned it downloaded, decoded, estimated and posted a map with the layer off. The engine now writes the switch into an atomic beside `busy` and `target` on every tick, and the worker reads it before every attempt and every 100 ms of a backoff. When the layer is off it gives the request up and sets `abandoned`, the engine owes the poll again, and switching the layer on sends it at once. The updater reads the same atomic once more between the download and its decode: `poll_once` decodes before it writes the cache image, the sidecar or the in-memory `ETag`, so a download that comes back with the layer off is dropped there with nothing decoded, estimated, posted or written, the request is given up the same way, and the next poll with the layer on downloads the map again. What still completes while the layer is off is the download itself, when an HTTP transfer was already under way as the switch flipped: interrupting one would need the source to watch the setting mid-transfer, which `CloudSource` has no way to do, and the bytes are dropped as soon as it returns. `a_download_that_returns_after_the_switch_is_not_decoded` holds this with a fixture source that keeps the fetch open until the case releases it.

## Open items

- The defaults of decision 6, from validation.
- Whether the cap becomes a setting and whether a polar fade is needed.
- Whether frozen sea keeps a veil, and the ice concentration as land once the sea ice atlas exists (Relation to the sea ice plan).
- The cloud draw's cost on lavapipe at 1920 x 1080, before and after (criterion 6, departure 6). It is the adapter risk 2 is about, since the texture overhaul research found anisotropic cube reads expensive there, and this run had no way to it without WSL. CI or the user measures it; if the mask read costs, the cloud group can carry the surface sampler at a binding of its own for the mask.
- `sunlit-earth render` with the real binary for criterion 7 (departure 7).

## Validation Record

The validation with the user (step 9) has not been run.

### Review round 1

On aa3a37d..17b48d0, with the gates green: one major finding and four minor ones, all resolved.

1. Major: switching the layer off did not stop a poll that was already retrying after a failure, so the worker went on contacting the source and could post a map with the layer off. Resolved by departure 8, in c4793d4, with the engine case `switching_the_layer_off_ends_a_failing_retry`.
2. Minor: architecture.md gave the background as 128 KiB with its mips; with its full chain it is 174763 bytes, about 171 KiB. Corrected.
3. Minor: the switch case's last check for no fetch waited two round trips, which a worker that had not reached the source yet could pass. Both of its checks for no fetch now watch the source for a second; the worker's own gate (departure 8) means the case fails only with both gates removed, which was checked.
4. Minor: no unit test held `draws_clouds` to the switch. Added `the_cloud_switch_gates_the_draw_whatever_the_opacities`, with the night opacity alone among its cases.
5. Minor: the open items lacked the lavapipe cost left by departure 6. Added, beside criterion 7's binary run.

The validator could not confirm that the shipped mask carries the level `fs_cloud` reads. It does: `assets::tiles::build::chain` cuts every whole face down to 1 px, `Pack::mips` returns the full chain for a whole face, and `cube_from_pack` makes the texture with that many levels, so a 1024 px mask has eleven levels and level 3 is 128 px. A smaller source would be cut to a smaller finest level, and the shader's level follows the texture's own size, so it still reads 128 px faces. The lavapipe and Metal cloud goldens from golden runs 38090094295 and 38090435229 at d3fb9ee were committed in a0976f2.

### Review round 2

On 17b48d0..67054f2, with the gates green and every finding of round 1 verified fixed, the mask level included: no major finding and three minor ones, all resolved.

1. Minor: departure 8 claimed the cache and its `ETag` were written before the decode, which is the wrong order, so a download that returned with the layer off could be dropped before the decode without the 304 trap it named. Implemented: `CloudUpdater::set_gate` and `PollOutcome::Abandoned` drop such a download with nothing written and give the request up, and departure 8, architecture.md, rendering.md and the `clouds_enabled` doc now say that only a transfer already under way completes while the layer is off. The fixture source holds a fetch until released, so the engine case `a_download_that_returns_after_the_switch_is_not_decoded` covers it, beside the updater's `a_download_returning_through_a_closed_gate_leaves_nothing_behind`.
2. Minor: the "Show clouds" hint now reads "While it is off, no new download starts and nothing is decoded.", which is what the code does.
3. Minor: the lavapipe and Metal golden means in testing.md were averages over RGBA; they are now over RGB as `support::compare` takes them (lavapipe 0.318 and 0.103, Metal 0.317 and 0.099), with the counts unchanged.
