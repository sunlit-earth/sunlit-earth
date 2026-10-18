# Plan: An Adjustable Ocean Color

## Summary

Two new settings, the ocean's base color on the day side and on the night side, each set with a color picker in the settings panel. The renderer keeps every texture as it is and moves the open water's share of each texel from the color the bake gave it to the chosen one, in the shader, by the water mask. Sea ice, the glint, the Fresnel shift, the night's sea ice, lights at sea and the cloud layer's land weight read what they read today.

The defaults are the colors the shipped textures carry, (10, 30, 60) by day and (5, 5, 15) by night. At a color equal to the one its texture carries, the shift is exactly zero and the shader takes a branch that leaves the old arithmetic untouched, so every golden render is byte for byte the one before the change when the colors are set to the active values.

Slint 1.18 has no color picker widget, so the picker is a component of this repository: a saturation and value square, a hue strip and a hex field in a popup, opened from a swatch in a settings row.

## Stakes Classification

Medium. The change reaches the shader, the uniform block, the surface residency, `SceneParams`, the config file and a new widget, but every part is additive, and the property that matters most, nothing moves at the default, is checked byte for byte rather than within a tolerance. The likely mistakes are a recolor that leaks onto ice or land, which the shader tests below are built to catch, and a shader edit that changes floating point code generation at the default, which the identity check catches.

## What the code does today

Checked against `main` at bfce47a on 2026-10-18:

- The fills are baked. The day faces carry a flattened ocean fill with the sea ice composited over it, and the Black Marble draws every sea at night as one flat color (`docs/rendering.md`, Sea ice in the day faces). When a pack is built, `ocean_color` (`crates/sunlit-core/src/assets/tiles/build.rs:384`) takes the per channel median of the texels the mask calls open water, and the pack stores it in header bytes 36 to 39 (`pack.rs`). Read from the pack caches on this machine: every shipped day pack holds (10, 30, 60, 255) and the night pack (5, 5, 15, 255); every day pack of the golden suite's Earth fixture holds (11, 30, 60, 255) and its night pack (5, 5, 15, 255), from a fresh golden run today. The mask pack holds zero.
- Two uniforms carry the fills. `day_ocean` and `night_ocean` (`renderer/render_pass.rs:260`) come from `SurfaceTiles::ocean_colors` (`renderer/tiles.rs:670`): the month in force's day pack and the night's, zero for a pack not yet handed over. `refined` (`sphere.wgsl:487`) draws them for page table cells flagged constant ocean, and `night_with_ice` (`sphere.wgsl:413`) uses the night's to put ice in place of the water's share of the fill. The floors and the stored tiles hold the fill in their own texels; the Earth fixture's floor holds (10, 30, 60) where its pack's median is (11, 30, 60) (`docs/testing.md`).
- The mask is read in blend mode alone. `globe_surface` (`sphere.wgsl:513`) samples `water_cube` only when `terminator_width` is not negative. The Day and Night bind groups bind the dummy cube in the mask's place (`renderer/mod.rs:609`), which reads (0, 0, 0, 255), so all land. `surface::needs` (`renderer/surface.rs:131`) asks for the mask in Day and Night mode only while clouds are drawn, for the cloud layer's land weight.
- The ice term is blend mode's. `fs_main` returns the corrected day texel before it computes `ice`, so Day mode has no notion of ice. Ice is `smoothstep` on the day texel's luminance (52 to 76 of 255) times `smoothstep` on latitude (30 to 35 degrees), and `surface.water * (1.0 - ice)` is the water term the glint and the Fresnel shift read.
- Values are stored sRGB. No surface format is sRGB (`no_surface_format_is_srgb`, `renderer/surface.rs:591`), so the shader works on the stored values and an 8 bit color chosen in the UI is the value the texel takes.
- The uniform block is 576 bytes, `_pad11` at offset 572 last (`renderer/uniforms.rs`).
- The goldens. Every case starts from `base_params()` (`tests/golden.rs:184`). `africa_on_the_z_face`, `arctic_in_march`, `arctic_in_september` and `antarctic_in_september` draw the Earth fixture in Day mode, the two cloud cases in blend mode, the rest the grid. No case draws Night mode. The comparison has a tolerance, so a passing suite does not show that nothing moved. All 24 pass at bfce47a on WARP here. A precedent for an exact check exists: the night ice's WARP set was regenerated and found byte for byte the one before (`docs/testing.md`).
- The config. A missing key takes its serde default; a value of the wrong type fails the whole file, which then loads as the defaults (`config/mod.rs:561`); a key an older build does not know is dropped on read. Adding keys needs no migration step.
- Shipped geometry. Faces 2048, floor 512, mask 1024, two tile levels (`assets/tiles/mod.rs`). A mask texel is about 10 km, twice the finest day texel.

Slint 1.18.1, read in the cargo registry sources:

- The std widgets export no color picker or color dialog (`i-slint-compiler-1.18.1/widgets/fluent/std-widgets.slint`), and the documentation for the next release lists none either. Slint 1.11 added color pickers to the Live Preview, which is editor tooling and not a runtime widget.
- `color` holds 8 bit channels unless the `32-bit-color` feature is on (`i-slint-core-1.18.1/graphics/color.rs:33`), and `cargo tree` shows it off here, so (10, 30, 60) set from Rust reads back exactly.
- `hsv()` and `oklch()` construct colors, and `.to-hsv()`, `.to-oklch()`, `.red`, `.green`, `.blue` take them apart (`builtin_macros.rs` and `lookup.rs` in the compiler). Nothing in the language formats or parses a hex string.
- `PopupWindow` takes `PopupClosePolicy.close-on-click-outside`.
- The fluent `LineEdit` sets its text and fires `edited` from `accessible-action-set-value`, and `ElementHandle::set_accessible_value` reaches it from a test.

## Key Design Decisions

### 1. Recolor in the shader by the mask, relative to the pack's own fill

A texel over water is the water's share of the fill plus whatever else is in it: land at a coast, ice, a ship's lights. Moving the fill's share to the chosen color is one addition:

```
texel + open_water * (chosen - fill)
```

where `open_water` is the mask's share of water less the share the ice term takes. This is the arithmetic `night_with_ice` already uses to put ice in place of the night's fill, so the two compose: on the night side the water's share becomes ice by `ice` and the chosen color by `1 - ice`. A coast texel half over water moves halfway; a light at sea keeps its excess over the fill; ice keeps its color. At `chosen == fill` the difference is zero and nothing moves.

`fill` is the pack's median, which the constant ocean cells draw exactly, so those cells end exactly at the chosen color and the floor's texels within the codec's noise of it.

### 2. The CPU computes the shift, and a zero shift takes the old path

`write_uniforms` computes `(chosen - fill) / 255` per channel for each side and writes two new `vec3<f32>` uniforms, `day_ocean_shift` and `night_ocean_shift`. The fill is the ocean color of the pack the drawn floor came from, which `install_surface` (`renderer/mod.rs:427`) records beside the cube from `Pack::ocean()`; the floor and its pack's tile index arrive in the same call, so whenever a floor is drawn its fill is known. That avoids the window in which `ocean_colors` reads zero, from a month change until the new month's pack is handed over to the tiles, during which a fill taken from it would recolor by the whole chosen color. The shift is zero when no floor is drawn, when the fill's alpha is zero, in Grid mode, and when the chosen color equals the fill. A zero computed this way is exactly `0.0`.

The shader applies the shift inside a branch on the uniform being non zero, so at the default the fragment runs the arithmetic it runs today, and the recolor ends in a clamp to the unit cube only where it runs, since `apply_gamma` takes a power. Both branches depend on uniforms alone, so no sample moves out of uniform control flow; the samples stay where they are.

### 3. The mask is resident in every cube mode

Day and Night mode need the mask to know where the water is. `surface::needs` asks for it in Day, Night and Blend, the Day and Night bind groups bind it in place of the dummy, and `globe_surface` samples it in every mode. Grid keeps its own group with the dummy mask. The cost is the mask's cube, about 4 MiB in BC4 at 1024 px with its chain, and a Day or Night publish now waits for it as a blend one does. Day mode computes the ice term too, the same expression blend mode uses, so the picker does not tint the ice composited in the day faces. Night mode has no day texel and so no ice, as today: the Black Marble's frozen seas are the same flat fill, and they take the chosen night color.

### 4. Defaults are the shipped fills, stored as hex

`AppConfig` gains `day_ocean_color = "#0a1e3c"` and `night_ocean_color = "#05050f"`. A small `Rgb8` type in `sunlit-core` serializes as `#rrggbb`, reads it with or without the `#` and in either case, and is the type of both the config fields and the `SceneParams` fields. A malformed value fails the file like any other value of the wrong type, which keeps one rule for the config. A test that needs the real assets holds every shipped pack's ocean color to these defaults, so a rebake that moves the fill fails it, and the step that follows is `change_default` in `config/migrate.rs` for users who kept the old default.

### 5. The two colors join Color Correction

The rows go first under the existing Day Texture and Night Texture headings, above Gamma and Saturation, because they are the base the corrections then act on. `TextureColorSettings` gains `ocean: color`, so the headings' and the group's resets cover the new rows with no new code, and `root.defaults` carries the defaults from `AppConfig::default()` as it carries everything else.

```
Color Correction                                 ↺
 Day Texture                                     ↺
   Ocean          [████]  #0a1e3c                ↺
   Gamma          [=====o-----]  1.00
   Saturation     [=====o-----]  1.00
 Night Texture
   Ocean          [████]  #05050f
   Gamma          [=====o-----]  1.00
   Saturation     [====o------]  0.85
```

Hints: by day, "Base color of open water on the day map, before gamma, saturation and lighting. Sea ice keeps its own color." By night, "Base color of open water on the night map, before gamma and saturation. Lights at sea and sea ice keep their own colors."

### 6. The picker

`SettingColor` in `widgets.slint` is a row on `SettingRow`'s label column: the label, a swatch button, the hex text and a `ResetSlot`. It has `in-out property <color> value`, `in property <color> default-value`, `out property <bool> modified` and `callback changed()`, the same contract as `SettingRow`, so the panel binds it the same way. The swatch's accessible value is the hex string and its `accessible-action-set-value` parses one, which serves screen readers and lets the tests move the row the way they move a slider.

Clicking the swatch opens `ColorPicker` in a `PopupWindow` that closes on a click outside:

```
┌────────────────────────────┐
│ ┌────────────────────────┐ │
│ │ saturation →           │ │  square, 160 x 120
│ │ value ↓       ○        │ │
│ └────────────────────────┘ │
│ [====== hue strip ==|====] │
│ Hex [#0a1e3c            ]  │
└────────────────────────────┘
```

- The square is three layers: the hue at full saturation and value, a horizontal gradient from white to transparent, and a vertical gradient from transparent to black. Its vertical axis is the square root of value, drawn with enough gradient stops to follow that curve, because both defaults are dark: value 0.24 by day and 0.06 by night, which a linear axis puts in the square's bottom quarter and bottom sixteenth, and the square root axis at about a half and a quarter.
- The hue strip is a gradient through the six primaries and secondaries and an accessible slider from 0 to 360.
- The hex field accepts `#rrggbb` or `rrggbb` on Enter; anything else is ignored and the field shows the color again when it loses focus.
- The color is the only state that persists. The popup takes hue, saturation and value from `value.to-hsv()` when it opens, keeps its own hue while saturation or value is zero so a gray does not lose it, and writes `value` from its pointer and edit handlers alone, never from a binding. A color that nobody touches therefore never passes through a float round trip, and (10, 30, 60) stays (10, 30, 60).
- Formatting and parsing hex goes through an exported global, `ColorText`, with two pure callbacks that the app implements by calling `Rgb8`'s own functions, so the config and the UI share one parser. The app and the UI tests' `create_window` install it through the same function.

### 7. Not chosen

- A default that follows the texture. An unset color meaning "the pack's own" would keep any future bake's fill without a migration, but the picker needs a concrete color to show and to reset to, which would have to come back from the engine, and the request is a base color the user defines. The real asset test in decision 4 covers the coupling instead.
- Recoloring by closeness to the fill without the mask. The night's unlit land is as dark as its sea, so a color key would recolor land at night, and a coast texel's mix would be keyed by its color rather than its share of water.
- Rebaking the faces with the chosen color. A bake takes minutes per month and needs the texture pipeline at runtime.
- The chosen colors as packed RGBA8 with the fill read in the shader from `day_ocean`. It is the pattern the block already has, but `day_ocean` is zero until the month in force's pack is handed over to the tiles, and the shift would then be the whole color.
- Loading the mask in Day and Night mode only while a color differs from its fill. It keeps the default's residency as it is, but the fill is known only once a floor is resident, so the mask would be asked for in a second phase, and readiness would change with a setting. The mask is small and the blend default loads it already.
- OKLCH axes for the square. Better perceptual spacing, but most of the lightness and chroma plane is outside sRGB at most hues and needs clipping in the drawing and the picking. The square root value axis and the hex field cover the dark end.
- A third party Slint picker. None was found for 1.18.

## Success Criteria

1. With the colors set to the fills the textures carry, every golden case renders byte for byte as it does on `main`, on WARP here, on lavapipe in WSL, and on Metal and CI's lavapipe through `golden.yml`. The committed references are not regenerated.
2. With the shipped textures and the default config, renders of Blend, Day and Night mode at fixed instants are byte for byte the ones `main` renders, on this machine's GPU.
3. At gamma 1 and saturation 1, open water draws the chosen day color in Day mode and the chosen night color in Night mode within one level, constant ocean cells and floor texels alike.
4. Sea ice in the day faces, the night's sea ice, the glint, the Fresnel shift, lights at sea and the cloud layer's land weight behave as before at any chosen color, each held by a test below.
5. The picker sets a color from the square, the hue strip and the hex field; a row, a heading and the group reset it; the color persists through a save as hex; opening and closing the picker without touching it changes nothing.
6. `cargo test`, `cargo clippy --all-targets` and `cargo fmt --check` pass, and the e2e suite passes in the Windows guest and in the Linux guest on KDE.

## Implementation Steps

Work on `feature/ocean-color` in `.worktrees/ocean-color`.

### Step 0: Baselines and spikes

1. In a worktree at `main`'s commit, run `SUNLIT_EARTH_UPDATE_GOLDEN=1 cargo test -p sunlit-core --test golden` and copy `crates/sunlit-core/tests/golden/warp/` to the scratch directory as the WARP baseline. Do the same in WSL for lavapipe, with `CARGO_TARGET_DIR=$HOME/sunlit-target-<worktree>`. Remove the worktree.
2. Write scratch configs for Blend, Day and Night mode with a custom date and time and three views: the default view, the Arctic in March, and a close coast with lights at sea on the night side (the Persian Gulf works). Render each with `SUNLIT_EARTH_CONFIG=<config> SUNLIT_EARTH_NO_CLOUDS=1 cargo app --release -- render --output <png> --width 1920 --height 1080`, twice; if the two renders of a case differ, this GPU is not deterministic and criterion 2 falls back to the software adapters.
3. Spike the popup: a `PopupWindow` with the three layer square opened from a row inside the sidebar's `ScrollView`, on the Windows desktop, in the Windows guest (`winit-software`), and in the Linux guest on KDE under X11 and Wayland. Check it is not clipped by the sidebar, closes on a click outside, and that a color picked from the square's middle matches the swatch it sets within a level or two. Record what was seen.

### Step 1: Types, config and parameters

`Rgb8` with its hex functions and serde; the two `AppConfig` fields and defaults; the two `SceneParams` fields, `from_config` and `write_to_config`; a `colored` group in `scene_digest!`, compared exactly, whose mutation row changes the red channel by one.

### Step 2: Renderer

`ResidentCube` carries its pack's ocean color, set in `install_surface`. A pure `ocean_shift(chosen, fill) -> [f32; 3]` with the rules of decision 2. Two `vec3` uniforms with their padding after `_pad11`, the block going to 608 bytes, and its size assertion and doc comment. `write_uniforms` fills them, zero in Grid mode. `surface::needs` asks for the mask in every cube mode, and `rebuild_surface_groups` binds it in the Day and Night groups.

### Step 3: Shader

The two `Uniforms` fields with offset comments. `globe_surface` samples the mask in every cube mode. `fs_main` computes the latitude, the day luminance and `ice` before the single surface return, with `ice` zero in Night mode, and applies the shift to the day texel (the night texel in Night mode, by the night shift) and, in blend mode, to the night texel after `night_with_ice`, each in its branch. The water term keeps its expression. The constant ocean cells, the ice gate, `NIGHT_ICE`, the Fresnel sky color and `fs_cloud` are not touched.

```wgsl
/// `texel` with the open water's share of its pack's fill moved by `shift`,
/// the chosen color less the fill, clamped for `apply_gamma`.
fn recolor_ocean(texel: vec3<f32>, open_water: f32, shift: vec3<f32>) -> vec3<f32> {
    return clamp(texel + open_water * shift, vec3<f32>(0.0), vec3<f32>(1.0));
}
```

### Step 4: The identity check

Set the golden suite's `base_params` to the Earth fixture's fills, (11, 30, 60) and (5, 5, 15), and have the golden engine assert, when it builds the packs, that every day pack and the night pack hold exactly those. `check_golden_in` waits for the mask in Day mode as well. Then regenerate WARP and lavapipe on the branch as in step 0 and compare every file with `cmp` against the baselines, and render the step 0 configs and compare them. Every file has to match. If any differs, stop: build a shader with the uniform fields and no recolor to tell a code generation difference from a logic one, and settle it before going on.

### Step 5: The UI

`ColorText` and its Rust side. `SettingColor` and `ColorPicker` in `widgets.slint`. `ocean` in `TextureColorSettings`, the two rows, `scene_settings` and `read_params_from_window`. The UI tests' helpers learn the new row (see Tests).

### Step 6: Tests

As listed below; the full suite and clippy.

### Step 7: Documentation

- `docs/rendering.md`: a section on the ocean color, the arithmetic, where the fill comes from, the coast resolution and the Night mode's frozen seas; update the sentences that say Night mode reads no mask and that single surface modes need the mask only for clouds.
- `docs/architecture.md`: the mask in every cube mode, the `colored` digest group, and a color parameter's path through the files beside the float one.
- `docs/testing.md`: the new tests and the identity record with each adapter's result.
- `docs/roadmap.md`: the item, checked once validated.
- README is not edited without asking; if a config key table there should list the new keys, ask.

### Step 8: Validation with the user

The user picks colors in the running app and looks at: open sea at default zoom and closest zoom, coasts at closest zoom for the fringe, the sea ice edge in March and September, inland water (the Caspian, the Great Lakes, Baikal), the terminator, lights at sea at night, and all three cube modes. Then the e2e suite in both guests, and, after pushing the branch with the user's permission, `golden.yml` on `macos-latest` and `ubuntu-latest` for `main` and for the branch, with the artifacts compared byte for byte.

## Tests

`sunlit-core` unit tests:

- `Rgb8`: hex round trip; with and without `#`; upper and lower case; rejects wrong lengths and non hex digits.
- Config: the two keys round trip through a save; a file without them loads the defaults; a malformed one loads as a malformed float does.
- `ocean_shift`: exactly `[0.0; 3]` when the chosen color equals the fill; zero with no floor, with a fill of alpha zero and in Grid mode; the expected signed fraction otherwise.
- `uniforms.rs` and `params.rs`: covered by the existing layout tests and the generated mutation table once the fields exist.
- `surface::needs`: the mask in Day, Night and Blend whatever the clouds; nothing in Grid. This replaces `drawn_clouds_add_the_mask_to_every_surface_mode` and the two assertions after it.

`tests/render_pipeline.rs`, with cubes of one color as the sea ice cases use, through the production bind groups:

- A zero shift over a mask of all water draws the same pixels as over the dummy mask, in Day and Night mode: the mask has no effect at the default.
- Open water takes the chosen color within one level in Day and Night mode, for a floor texel of the fill and for a constant ocean cell.
- A coast texel half over water, half land and half fill, ends at half land and half the chosen color within one level.
- The winter pack (240, 243, 247) and the darkest full ice (80, 80, 80) over water poleward of 36 degrees are unchanged at a chosen color far from the fill, in Day and blend mode; a coast texel equatorward of 29 degrees is recolored by its share.
- The night's sea ice keeps the color `sea_ice_on_the_night_side_is_drawn_by_its_day_luminance` holds it to at a night color far from the fill, and lights of (200, 160, 100) over open water keep their excess over the fill.
- The glint and the Fresnel shift still change pixels over recolored water by the amount they change them over the fill.
- Grid mode draws the same pixels with any chosen colors.

Engine tests:

- A Day publish waits for the mask whatever the clouds: `a_drawn_cloud_layer_holds_a_day_publish_for_the_mask` loses its "clouds off goes out" half.
- In Day mode over the Earth fixture, a new day color changes pixels where the mask is water and leaves every pixel where it is land as it was.
- With the real assets, every shipped day pack's ocean color is the day default and the night pack's the night default, beside `the_real_day_packs_store_the_sea_ice_and_flag_the_open_water`; it skips without the LFS objects like its neighbors.
- Tests that render the Earth fixture at default parameters now see one level of red over its water, (11 against 10). Where a test fails on that alone, it sets the fixture's fills from one constant in `test_support` rather than loosening its assertion; each such case goes in the departures.

`tests/slint_ui.rs`:

- `part_of` and `resettable_rows` know `SettingColor`, and a color row is moved off its default through the swatch's accessible value, so the default, row, heading and group reset tests cover the new rows unchanged in intent.
- The two colors round trip from `SceneParams` to the window and back exactly.
- Opening the picker and closing it changes neither color.
- A hex entered in the popup's field sets the color; an invalid one leaves it.
- The hue strip's accessible value and a pointer press in the square set `hsv(h, s, v)` of that place.
- `test_nothing_in_the_panel_asks_for_more_than_the_sidebar_floor` passes with the new rows.

## Out of Scope

- Grid mode, which draws its own generated colors.
- A color per month, or for lakes apart from the sea.
- The color of sea ice, of the night's ice, of lights, and the Fresnel shift's sky color.
- A finer mask; the coast fringe is documented, not fixed.
- Changing the bake or the packs; `FORMAT_VERSION` does not move.

## Risks and Mitigations

- A shader edit changes code generation at the default. Adding uniforms and a branch can, in principle, change how a compiler contracts unrelated expressions, which would move a pixel by a level somewhere. Mitigation: the existing expressions stay as they are, the recolor sits in a uniform branch, and step 4 compares bytes and isolates the cause before anything else is built on it.
- A fringe at coasts. The mask is half the finest day tile's resolution, so at the closest zoom a band about one mask texel wide along a coast is recolored by the mask's share rather than the texel's: land takes a little of the chosen color, water keeps a little of the fill. It is invisible at the default colors and grows with the distance between the chosen color and the fill. Mitigation: document it, look at it in step 8, and leave a finer mask to the roadmap.
- Bright coasts poleward of 30 degrees. The ice term takes some of their water for ice, as it does for the glint today, so their water's share is recolored less. Documented with the existing limitation.
- Inland water. Water the mask marks but the bake did not flatten keeps its own texture, moved by the same shift. Step 8 looks at the large lakes; whether any are affected is not known yet.
- The Earth fixture's fill is not the shipped one. Tests at default parameters see a shift of one level. Mitigation: the fixture's fills in one constant, and the golden engine asserts them.
- The popup on some backend. Clipping or placement in the sidebar, or behavior under Wayland or `winit-software`, is unknown until the step 0 spike.
- Precision at the dark end. Both defaults are dark. Mitigation: the square root value axis and the hex field; the user judges it in step 8.

## Rollback Strategy

Revert the branch's commits. A config written by the new build keeps working in an older one, which drops the two keys it does not know. No pack, cache or reference set changes.

## Open items

Settled with the user on 2026-10-18, each with the recommended answer.

1. The rows' place: first under Color Correction's Day Texture and Night Texture headings.
2. The mask resident in Day and Night mode at all times.
3. Grid mode left alone.
4. The picker's controls: the square, the hue strip and the hex field, with no red, green and blue fields.

## Departures

1. Step 0.1's baselines were generated in the branch's own worktree before any code changed, rather than in a worktree at `main`'s commit. The branch's HEAD was `main` (bfce47a) plus the plan document, so the code was `main`'s, and a second worktree and its build would have produced the same files. Both baselines were regenerated a second time from the same code and came out byte for byte the same, so WARP and lavapipe are deterministic run to run here, which is what makes a byte comparison with the branch meaningful.

2. Step 0.2's configs switch the Moon and the Milky Way off. Of two renders of the Night mode's Arctic view from the same binary, one had the Milky Way and the other did not, 901704 pixels apart by up to 51 levels: the panorama is an overlay that no export waits for, so whether it is in a render is a race, not a property of the GPU. With both overlays off, three renders of each of the nine scenes from `main`'s binary were byte for byte the same, so this machine's GPU is deterministic for these renders and success criterion 2 is held on it rather than falling back to the software adapters.

3. Step 0.3's spike was rendered offscreen only, through Slint's software renderer with dispatched pointer events, and not on the desktop or in the guests: this run was not to launch the GUI on the desktop, and a guest's desktop needs eyes. Slint 1.18's winit backend does not open a popup as a window of its own ("We don't render popups as separate windows yet", `winitwindowadapter.rs`); it places a `PopupWindow` inside the main window through the core's `place_popup`, the same code the software window goes through, so placement and clipping do not depend on the backend. What does is how each renderer draws the gradients, which step 8's look covers.

4. Night mode's recolor uses the mask's whole share of water rather than an `ice` set to zero in Night mode. `fs_main` computes `ice` with the expression it had, branches on `FLAG_NIGHT_ALONE`, and passes `surface.water` to `recolor_ocean` there and `surface.water * (1 - ice)` otherwise. A `select` on `ice` would have sat between the ice expression and every use of it in blend mode, where a compiler may contract `1 - s1 * s2` into a fused multiply and add; the branch leaves blend mode's code as it was. The result is the same as an `ice` of zero.

5. `Grid mode draws the same pixels with any chosen colors` is an engine test, `ocean::the_grid_draws_the_same_pixels_with_any_ocean_colors`, rather than a case in `tests/render_pipeline.rs`, and what it holds is that the grid's pixels do not depend on the colors: the grid's group binds the dummy mask, which reads no water, so no shift moves anything there. That the shift is zero on the grid in the first place is held on the CPU side by the unit test `surface::tests::a_frame_measures_its_ocean_colors_from_the_floors_it_draws`, through `drawn_fills`, which gives the grid no fill.

6. A color parameter's path through the files is documented in `docs/rendering.md`, beside the table of a float parameter's fourteen edits, which is where that table lives, and `docs/architecture.md` points to it from the paragraph on `ParamsDigest` and its groups, which it gains.

## Validation Record

Step 4, the identity check, on 2026-10-18. Every golden reference regenerated on the branch with `base_params` at the Earth fixture's fills, which the golden engine asserts of every pack, was compared with `cmp` against the set regenerated from `main`'s code: WARP 22 of 22 files the same byte for byte, and lavapipe (llvmpipe, LLVM 15.0.7, in WSL) 22 of 22. Each adapter gave the same bytes from the same code in two runs. Through `golden.yml`, with the artifacts of `main` (bfce47a) and of the branch (e724e04) compared with `cmp`: Metal on `macos-latest` 22 of 22 the same (runs 39133168228 and 39133908124), and lavapipe on `ubuntu-latest` 22 of 22 (runs 39133500680 and 39133997565). Nine renders of the shipped cube at 1920 by 1080 (Blend, Day and Night mode, each over the default view at 17:00 UTC on 21 June, over the Arctic from 80 N on 20 March, and over the Persian Gulf at the closest zoom at night) from release builds of `main` and of the branch, the default config with clouds, the Moon and the Milky Way off, were the same byte for byte on this machine's GPU, which gave the same bytes in three renders of each from `main`'s build (departure 2). No build of a shader without the recolor was needed, since nothing differed.

Step 0.3, the picker spike, offscreen through Slint's software renderer (departure 3): a popup opened from a row inside a sidebar `ScrollView` and placed across the sidebar's edge was drawn whole, closed on a click outside, and a press at the square's middle set (32, 46, 64) where the square draws (31, 45, 64); four more places agreed within two levels. The look on the Windows desktop, in the Windows guest under `winit-software` and in the Linux guest under X11 and Wayland is left for step 8.

Steps 5 to 7, the gates, on the branch at 14df9bd: `cargo fmt --check` clean; `cargo clippy --all-targets` with no warnings; `cargo test` with `SUNLIT_EARTH_CLOUD_URL` unset passed every target, 649 core unit tests, 126 engine, 24 golden, 49 render pipeline, 7 shading, 4 bc7, 1 soak, 87 and 693 in the app and the xtask, 55 Slint UI, and the 18 e2e cases ignored as they are outside a desktop. `textures::the_real_packs_carry_the_default_ocean_colors` ran against the real packs rather than skipping.

Step 8, the e2e suite, on the branch's code at 71fadd7: `cargo xtask e2e --target linux` in the Linux guest under KDE Plasma on X11, 17 passed and 0 failed; `cargo xtask e2e --target windows` in the Windows guest, 18 passed and 0 failed, three of them skipping by their own runtime gates (no second output for a layout change, the setter line being Linux's, and Plasma's file watch). The user's look in the app is for after this run.

Validation round 1 found 0 MAJOR and 5 MINOR, all fixed in 4bde21d: no test held the day recolor in blend mode, the default, so `open_water_takes_the_chosen_colors_in_blend_mode` was added and the glint and Fresnel case now asserts the water was recolored, both failing under a temporary shader edit that limited the day recolor to single mode; three functions in `ui_callbacks.rs` had been placed inside `apply_config_to_window`'s doc comment; the roadmap still said the Night mode reads no mask; the grid test's doc comment and departure 5 now say it holds the grid's pixels through the dummy mask, with the zero shift held by the `drawn_fills` unit test; and a test comment that told history was reworded. No shader or renderer code changed. The gates after the fixes: `cargo fmt --check` clean, `cargo clippy --all-targets` with no warnings, and `cargo test` passing every target, render pipeline now 50.
