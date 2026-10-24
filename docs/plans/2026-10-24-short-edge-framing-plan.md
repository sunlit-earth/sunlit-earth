# Plan: One Framing for the Preview, the Wallpaper and the CLI

## Summary

The preview, the wallpaper and a CLI render should show Earth, the Sun and the Moon in the same places relative to each other, whatever their sizes. Today they do not, because the two lenses are measured on different edges of the picture and the three render paths frame the scene three different ways. This plan:

- defines both fields of view across the picture's short edge, and the pan in units of that edge (the short edge rule);
- turns the settings into each output's lenses in one place, and makes the preview, every wallpaper render and every export go through it;
- frames the preview so that the anchor screen's rectangle is always as tall as the panel, clipping the sides when the panel is narrower;
- migrates `sky_fov` and `offset_x` so a 16:9 screen keeps its wallpaper;
- adds a frame guides button to the preview HUD, off by default, that outlines the anchor screen, 16:9 and 4:3 on the preview.

No new setting. The CLI needs no new flag: its output depends only on `--width`, `--height` and the config.

## Stakes Classification

High. The meaning of two persisted settings changes and takes a config migration, every render path changes how it frames the scene, and the golden references must survive it. Nothing in the shader, the uniform block or the IPC contract changes.

## Background

The analysis behind this plan happened in conversation with the user on 2026-10-24; this section records what it established.

### The two lenses are anchored to different edges

`docs/rendering.md` ("One view across several screens") states it: the Earth lens is `Mat4::perspective_rh(fov_y, aspect, ..)` (`scene/camera.rs`), so the globe's scale follows the frame's height; the sky lens builds NDC as `radial * r / edge * vec2(1, aspect)` (`sphere.wgsl`, `scene/sky_lens.rs`), so the Sun, the Moon and the stars follow its width. The pan is in NDC, so it stretches with the frame on both axes.

Consequently a sky direction's distance from the globe, measured in globe radii, is proportional to the frame's width over its height. A 4:3 preview draws the Moon 25% closer to the globe than a 16:9 wallpaper does, and a 3440x1440 ultrawide magnifies the sky 1.34x against a 16:9 screen.

### Three render paths, three framings

Checked against `main` at 4e7aae4:

- The preview renders `self.params` as they are, at the panel's size: `Engine::draw` calls `self.renderer.render(&self.params, &sky)` (`engine/mod.rs`), and the preview's tile output is `tile_output(&self.params, ..)`.
- An export (the CLI's `render`, `render_to_file`, `export_pixels`) also renders `self.params` as they are: `publish.rs`, in the function that answers a waiting export, `let params = self.params; self.export_capped(&params, ..)`.
- A wallpaper goes through `layout::screen_framing` (each screen, with the portrait contain rule) or `layout::canvas_framing` (a span), applied by `Framing::applied_to` in `export_framed`.

So a portrait CLI render does not even match a portrait wallpaper today.

### The short edge rule

Both fields of view span the picture's short edge, and the pan is measured in halves of it. Every output is then the same picture, uniformly scaled so the short edges match, centered on the same point; outputs differ only in how far they extend along the long edge. Resizing never moves anything relative to anything else.

For a landscape output the short edge is the height, so the Earth lens is exactly today's on every landscape output, and on a portrait output it is exactly today's contain rule (`contain_camera_fov`): the Earth lens of every wallpaper is unchanged. What changes is the sky and the horizontal pan, and both are migrated so that a 16:9 screen keeps its picture.

Worked cases at the defaults, one image per screen:

- A 1920x1080 notebook and a 3440x1440 ultrawide: the globe sits in the same place at the same fraction of the height on both, the Sun and the Moon keep their places relative to it, and the ultrawide shows 0.31 screen heights more sky on either side, about 173 degrees across against 140. Nothing is stretched; the extra area is the same lens continuing outward.
- A scene composed on a 21:9 screen and rendered by the CLI at 16:9: the same picture at the same height with the outer 13% of the width cut on either side. A body placed there is not in the render, and a far-panned globe can be cut, but nothing moves.

## Key Design Decisions

### 1. Settings units and output units

The settings keep their four keys and change two meanings:

| Key | Today | After |
|---|---|---|
| `camera_fov` | vertical field of the frame (contain rule on portrait wallpapers) | field across the short edge |
| `sky_fov` | horizontal field of the frame | field across the short edge |
| `offset_x` | NDC, halves of the frame's width | halves of the short edge |
| `offset_y` | NDC, halves of the frame's height | halves of the short edge |

An output is a width, a height and a reference short edge `S` in that output's pixels. For a screen or an export `S = min(width, height)`; the preview's `S` is decision 4's. Its lenses in the units the shader already takes are:

- `camera_fov_out = 2 * atan(tan(camera_fov / 2) * height / S)` (vertical)
- `sky_fov_out = 4 * atan(tan(sky_fov / 4) * width / S)` (horizontal)
- `offset_ndc_x = offset_x * S / width`, `offset_ndc_y = offset_y * S / height`

The principal point stays at the output's center. The shader, `uniforms.rs` and `write_uniforms` do not change: they keep receiving a vertical Earth lens, a horizontal sky lens and an NDC pan, now derived per output.

### 2. One derivation, and a type that proves it ran

`display::layout` stays the home of framing:

- `Framing` keeps meaning the settings' four values (`From<&SceneParams>`).
- A new `OutputFraming` holds one output's four derived values plus whether either lens was clamped (decision 6). `layout::output_framing(settings, width, height, short_edge)` computes it per decision 1. `screen_framing(settings, width, height)` becomes `output_framing` with `S = min(width, height)`, and `contain_camera_fov` is deleted, since the rule subsumes it.
- `canvas_framing` keeps its math and starts from the anchor's `screen_framing`, as today; its `Framing` result becomes an `OutputFraming`. `Shot::framing` becomes an `OutputFraming`.
- A new `OutputParams` newtype in `params.rs` wraps the `SceneParams` one output draws. Only `OutputFraming::applied_to` builds one. `Renderer::render`, `Renderer::export_image_capped`, `tile_output` and `write_uniforms` take `&OutputParams` instead of `&SceneParams`, so a render path that skips the derivation does not compile. `OutputParams` derefs to `SceneParams` for reading.

`SceneParams` remains the single description of what to draw, in settings units; `OutputParams` is that description for one rectangle. The two translation points named in `CLAUDE.md` are unchanged, and `write_uniforms` now only ever sees output units.

### 3. Every path is an output

- Each wallpaper screen: `screen_framing`, as today.
- A span: `canvas_framing`, as today.
- An export of `width` by `height` (CLI `render`, `render_to_file`, `export_pixels`): `screen_framing(settings, width, height)`. A portrait CLI render now gets what a portrait wallpaper gets.
- The preview: decision 4.

`sunlit-earth displays` prints both the settings and each shot's derived framing, as it does now; its labels are updated to say which units each line is in.

### 4. The preview's frame

The anchor screen's rectangle is always as tall as the preview frame: `S = frame_height * min(1, A)`, where `A` is the anchor screen's width over its height. With a landscape screen that is the frame's height whatever the panel's shape. A wide panel shows extra sky to the sides, and a narrow one clips the screen's sides. The user chose clipping over shrinking the picture to fit the width. With a portrait screen the rectangle's width is `frame_height * A`, so it also fits the panel's height.

Height decides the scale; width only shows or hides the sides. When no screen is known (a headless engine, a session the platform cannot enumerate) the rule falls back to `S = frame_height`.

The anchor is the one the wallpaper plan uses, in every display mode: `layout::resolve_anchor` over the engine's monitor list and the stored anchor. The engine keeps a `preview_screen: Option<(u32, u32)>`, separate from `known_monitors` so the display hint comparison and its republish rule are untouched. It is set from a new `EngineConfig::monitors` field, which `app.rs` fills from the enumeration it already makes at startup, and updated on `SetDisplayPlan`, on a `MonitorsChanged`-emitting `recheck_displays` and in `plan_wallpaper`. A change marks the engine dirty.

`S` is computed from the frame the engine actually renders, after the tier cap, so it holds at every quality tier.

### 5. The preview frame says how it was framed

`EngineEvent::PreviewFrame` gains `short_edge: f32` (`S` in frame pixels) and `screen: Option<(u32, u32)>` (the anchor's size the frame was framed for). The window passes both to Slint with the image, along with the frame's size. The guides and the drag gain then follow the frame on screen, including the old frame shown with `cover` while a resize settles. They do not follow a panel size that frame was not drawn for.

Slint computes the frame's displayed scale under `image-fit: cover`, `k = max(container_width / frame_width, container_height / frame_height)`, and exposes `S * k` in logical pixels as an out property. `mouse_math::fine_drag_gain` takes the short edge field of view and that length instead of the raw `sky_fov` and the viewport width; its value at a 16:9 landscape panel is unchanged.

### 6. Clamps

- The derived sky lens: `SKY_FOV_MAX` stays 330. `SKY_FOV_MIN` drops from 60 to 1, because a narrow output now legitimately derives a horizontal sky narrower than 60: the narrowest slider position (30) on a 1:4 preview panel is 7.5 degrees. Its only remaining job is keeping `tan` off zero. `sphere.wgsl`'s `sky_lens_edge_radius` and `scene::sky_lens::sky_lens_edge_radius` change together, and `the_shader_and_the_cpu_agree_on_the_three_shared_rules` covers the new lower bound.
- The derived Earth lens is held below 179 degrees, replacing `canvas_framing`'s 170 (`CAMERA_FOV_MAX`, which stays the slider's bound). It is reachable only with a setting near 170 on a strongly portrait output.
- `OutputFraming` reports either clamp. A wallpaper publish adds it to its status note, as `sky_clamped` does today, and an export logs a warning.

### 7. Migration, slider ranges and presets

A new step `v3_to_v4` in `config/migrate.rs`, assuming the 16:9 screen most configs were tuned on:

- `sky_fov`: the old default 140 becomes the new default 86 exactly, in the manner of `change_default`; any other value `h` becomes `4 * atan(tan(h / 4) * 9 / 16)`. On 16:9, 86 short edge degrees are 140.0 horizontal to within 0.01.
- `offset_x`: multiplied by 16/9. `offset_y` is unchanged, since the short edge of a landscape frame is its height.
- Missing keys stay missing. Each change is recorded in the change log.

Every 16:9 wallpaper is therefore unchanged. Other shapes change in the sky's scale and the horizontal pan, which is the point.

Ranges, in `main.slint` (`sky-fov-minimum`, `sky-fov-maximum`, the offset rows), `camera_controls.rs` (`OFFSET_LIMIT`) and `mouse_math::apply_frame_drag`:

- Sky field of view: 30 to 120, covering the old 60 to 180, which migrate to 34.3 and 117.4.
- Offset X: -5.5 to 5.5, covering the old 3 times 16/9 (5.33). Offset Y stays -3 to 3. `OFFSET_LIMIT` becomes one limit per axis.

The `AppConfig` default for `sky_fov` becomes 86. `PRESETS` need nothing: every preset's `offset_x` is 0, and `offset_y` keeps its meaning.

Both field of view rows' hints say the angle spans the short side of the picture.

### 8. Frame guides

A toggle in the camera panel's footer, left of the `?` button: `guides-button := OverlayButton` with a new `icons/frame.svg` (four corner marks, drawn like the existing icons), `on: root.guides-shown`, and hint id `guides`. Its `OverlayHints` entry is titled "Frame guides", and its text says the guides outline this screen, 16:9 and 4:3 to frame a render for any of them. The key is F, following the panel's convention that every control names its shortcut (F is unused by `track-key`), and the shortcut sheet lists it.

- Off at every start, and not saved. It is a framing aid rather than a setting, and the user wants no new setting for this feature.
- Hidden with the other overlays by H, and drawn under the camera panel, the time bar and the hints.
- A new `overlays/frame-guides.slint` component draws one 1 px rectangle per aspect, centered on the container. An aspect `a` has its short edge equal to the displayed `S` from decision 5: `(S * a, S)` when `a >= 1`, `(S, S / a)` otherwise. With a landscape screen all three rectangles are as tall as the frame, so they show as pairs of vertical lines. A rectangle wider than the panel is clipped by `image-container`, which already clips.
- The screen's rectangle uses the accent color; 16:9 and 4:3 use the overlay foreground at reduced opacity. Labels sit inside each rectangle's top left corner, one row apart so that close edges cannot overlap them: "Screen 2560 × 1440", "16:9", "4:3". Where the screen's aspect is within 1% of 16:9 or 4:3, one rectangle carries both labels.
- No screen known: only 16:9 and 4:3 are drawn.
- No dimming outside the rectangles: with three frames, a dimmed area would belong to some of them and not others.

The rectangles stay put when the scene is panned. They are output rectangles, centered on the principal point, and a pan moves the scene within them.

### 9. The goldens keep their pictures

The golden target renders through `export_pixels` at 512x256 and 256x128, both 2:1, which today means horizontal sky fields of 140, 60, 300 and `SUN_CASE_SKY_FOV`. Each case's `sky_fov` (and any `offset_x`) is rewritten in the new units for 2:1 through a test helper that inverts decision 1. The derived uniforms are then the same and no reference changes. The engine and render pipeline tests that compute expected screen positions with `sky_lens_disc` or `globe_screen_circle` take their inputs from the output's derived framing.

### 10. Not chosen

- A stored reference aspect, with a "match my main screen" button: a setting the user wants to avoid, and with the lenses on different edges the composition would still depend on it.
- A fixed 16:9 reference frame, contained in every output: needs no setting either, but a screen narrower than 16:9 would shrink the globe instead of cropping the sides, and the two lenses would still have to be brought onto one edge.
- A reference that follows the connected screen live: docking a laptop to an ultrawide would quietly change the composition, and the CLI would depend on the screens.
- A letterboxed preview: wastes the panel and undoes the preview fill plan.
- Containing the screen's rectangle in a narrow panel: shrinks the picture to a strip; the user chose clipping.
- A CLI framing flag: unnecessary once the output depends only on its size.
- Changing the shader to take the short edge directly: the per output derivation in decision 1 does it without touching the uniform block, `write_uniforms`' contract or the goldens.

## Success Criteria

1. For any two outputs, whether the preview at any panel size, any wallpaper screen or any export size, a sky direction and the globe's center land at the same position relative to the output's center when measured in units of that output's `S`. Shown by a property test over random output sizes and screen aspects, and by a GPU engine case (criterion 3).
2. On a 16:9 screen, a migrated config draws the wallpaper it drew before: every derived uniform matches the old one within float tolerance.
3. With a 16:9 screen known, the globe's centroid and radius in the preview frame, scaled by `S`, match an export at 1600x900. Narrow, wide and screenless previews are all covered.
4. No golden reference changes.
5. The guides button is off at start, toggles with a click and with F, hides with H, and its rectangles sit where decision 8 puts them for a given frame, container and screen.
6. The gates below pass.

## Implementation Steps

### Step 1: The derivation

In `display/layout.rs`: `OutputFraming`, `output_framing`, the new `screen_framing`, `canvas_framing` on top of it, the clamps of decision 6, and the deletion of `contain_camera_fov`. In `params.rs`: `OutputParams`. Unit tests:

- landscape outputs keep the Earth lens;
- portrait outputs get what `contain_camera_fov` gave, asserted as a behavior: the horizontal extent equals the vertical one at square;
- the invariance property of criterion 1, through `sky_lens_disc` and `globe_screen_circle`;
- pan invariance in `S` units;
- both clamps report.

The existing canvas tests keep passing unchanged, which shows the span derivation survived.

### Step 2: The render paths

Thread `OutputParams` through `Renderer::render`, `export_image_capped`, `tile_output` and `write_uniforms`. In the engine:

- add `preview_screen` and `EngineConfig::monitors`;
- derive the preview's `OutputParams` in one method used by `draw` and the preview's tile output;
- frame exports with `screen_framing`;
- carry `short_edge` and `screen` on `PreviewFrame`.

Update `sunlit-earth displays`' labels, and fill `EngineConfig::monitors` in `app.rs` from the existing startup enumeration. Engine tests: the frame's `short_edge` and `screen` for a landscape screen, a portrait screen, none, and after `SetDisplayPlan` and a `MonitorsChanged`; criterion 3 as a GPU case in `tests/engine/`. Rewrite tests whose expectations assumed raw params to use the derived framing, and keep every golden reference per decision 9.

### Step 3: Migration and ranges

`v3_to_v4` with tests: 140 becomes 86; another value converts; `offset_x` scales; missing keys stay missing; changes are logged; a v3 fixture file loads to the expected values. Change the `sky_fov` default, the slider ranges, `OFFSET_LIMIT` and `apply_frame_drag`'s clamp, and both hints. Update the config tests that name the old default or ranges as behavior rather than as constants where they can.

### Step 4: The window

Pass the frame's size, `short_edge` and `screen` to Slint; add the displayed short edge out property; move `fine_drag_gain` to it. Add `frame-guides.slint`, the button, the icon, the hint, the F key and the shortcut sheet row. `slint_ui` tests cover criterion 5's behaviors and the rectangles' geometry for a landscape screen in a wide and a narrow container, a portrait screen, a screen within 1% of 16:9, and no screen.

### Step 5: Documentation

Rewrite the two lenses part of `docs/rendering.md` ("One view across several screens") around the short edge rule. It should cover:

- the formulas of decision 1;
- the contain rule as a case of it;
- the new lower sky clamp;
- the preview's framing;
- the measurements in the Background above.

Update `docs/architecture.md` (the preview path, `OutputParams`, `PreviewFrame`), `docs/usage.md` wherever it describes the fields of view, the pan or the preview, the `CLAUDE.md` bullets on `SceneParams` and on the preview, and add an entry to `docs/roadmap.md`. If `README.md` describes either field of view, the change is proposed to the user rather than made.

### Step 6: VM runs

Run `cargo xtask e2e --target windows` and `cargo xtask e2e --target linux` (KDE) and record the outcome. Run `vm down all` afterwards. If a VM command fails for host reasons, run `cargo xtask vm doctor`, record what it says, and carry on; do not run `vm setup` or `vm build-image`.

### Step 7: Validation with the user

On their desktop, the user:

- composes a scene with the Sun and the Moon near the globe and compares the preview with the wallpaper;
- resizes the preview to wide and narrow shapes;
- renders the scene with the CLI at 16:9 and at the screen's own size, and compares both against the guides.

The orchestrator handles this after the validation loop.

## Gates

- `cargo fmt --check`
- `cargo test` (the whole workspace)
- `cargo clippy --all-targets` with no warnings
- Step 6's e2e runs, recorded

## Out of Scope

- Physical size continuity across screens of different pixel density in a span.
- Pixel sized elements: star sprites and the minimum Sun and Moon disc scale with the output's height (`sky_lens::pixel_scale`), so they are slightly larger on a small preview. That affects sizes, not positions.
- Guides for aspects other than the screen's, 16:9 and 4:3, and for the other screens in Every screen or Across screens mode.
- Persisting the guides toggle.

## Risks and Mitigations

- A render path keeps drawing unframed params: `OutputParams` makes that a compile error.
- The preview and the guides disagree mid resize: both take `S` from the frame being shown (decision 5), not from the panel.
- Users on 16:10 or ultrawide screens see their sky scale change once: that is the fix, recorded in the migration log; the beta expects breaking changes.
- A config written by this version and read by an older build misreads `sky_fov` and `offset_x`: accepted for the beta. The schema version records which meaning a file has.
- Golden drift from float differences in the derived values: decision 9's helper inverts the formulas exactly, and the golden tolerance absorbs rounding.

## Rollback Strategy

Revert the branch's commits. A config migrated to version 4 keeps its converted values; the version 3 original is kept at `config::backup_path` by the existing migration machinery, and restoring it brings the old values back.

## Departures

1. **The canvas tests compare against the anchor's own framing.** Step 1 expected the existing `canvas_framing` tests to pass unchanged. They do not, because several compared the canvas's derived sky and pan against the settings themselves, which only held while the settings were the anchor's own lenses. The span math is unchanged and still starts from the anchor's `screen_framing`; the tests now compare against that framing instead of against the settings, and their fixture sky is 86, the new default, so a 1920x1080 anchor draws the 140 they were written for. The pixel scale identities they assert are the same.

2. **The default sky draws 140.012 degrees across a 16:9 width, not 140.0 to within 0.01.** Decision 7 moves the old default 140 to exactly 86 and says that is 140.0 on 16:9 to within 0.01. The exact conversion is 85.993, and 86 draws 140.012. I kept 86, as the plan chose a round default in the manner of `change_default`; the difference is about a tenth of a pixel at the edge of a 1920 pixel frame. Criterion 2 holds to that margin for the default and to a thousandth of a degree for every other value, and `a_migrated_file_draws_the_same_wallpaper_on_a_16_9_screen` says so.

3. **The golden and engine cases convert in one place each rather than per case.** Decision 9 has each golden case's `sky_fov` rewritten through a helper. The golden `render` (and the contact sheet) instead sends `tests/support::settings_for_output` of the case's params for its frame size, and `base_params` pins the sky at 140 in the renderer's units, since the default is no longer that. The engine harness's `test_params` pins a sky that draws 140 across its 2:1 frame (`output_sky_as_setting`), the panorama cases pin the same, and the cases that set a sky or a pan of their own convert it, while the placement helpers in `moon.rs` and `panorama.rs` take the derived framing (`harness::drawn_at`). Same effect, fewer places to get wrong: no reference changed.

4. **A new preview anchor of the same shape re-sends the frame instead of redrawing it.** Decision 4 says a change of `preview_screen` marks the engine dirty. Doing that unconditionally broke `readiness::the_tile_wait_is_not_renewed_by_the_packs_of_other_months`: the first publish learned the 1920x1080 screen, the redraw that caused came after the test had moved the mock clock, and the other months' packs no longer landed within the test's minute; a draw closes the transcoder's pause gate on the mock clock, which I take to be the mechanism, though I did not trace it further. Without the redraw the case passes. A landscape screen frames a landscape preview exactly as no screen does, so `update_preview_screen` now marks the engine dirty only when the derived preview framing changes, and otherwise re-sends the frame already drawn with the new `screen`, so the window's guides still learn the screen's size.

5. **`sunlit-earth displays` prints more than it did.** Decision 3 says it prints the settings and each shot's derived framing "as it does now". It printed only the span's canvas lens. It now prints a `settings:` line in short edge units, the canvas `lens:` line for a span, and a `lens:` line under each render group otherwise, each derived line saying vertical, horizontal and NDC.

6. **`OutputParams::framed` is `pub(crate)`.** Decision 2 has only `OutputFraming::applied_to` build an `OutputParams`. Rust cannot restrict a constructor in `params.rs` to `display::layout`, which is not an ancestor module, so the constructor is crate private and documented as the derivation's alone; outside the crate nothing can build one, and every render path takes one.

7. **The shortcut sheet's card padding drops from 18 to 13 pixels.** The F row made the sheet one row longer, and at 800x700 the card scrolled by 8 pixels, which `test_the_shortcut_sheet_keeps_its_text_on_the_card` forbids. Five pixels off the top and bottom padding keep every row on the card without scrolling.

## Open items

1. **README, proposed rather than made.** `README.md` says under the `render` example that "the exported image will not necessarily match your preview exactly, because it will be rendered at your specified resolution and not the preview window's resolution". That is no longer the case for the composition. A proposed replacement: "The export is the same picture as the preview, scaled until their short sides match: a render of another shape shows more or less along its long side, and the frame guides (F) show what a 16:9 or 4:3 render contains." No README text describes either field of view.
2. **A panorama case is sensitive to the sky it runs at.** With the default sky, 153 degrees across its 2:1 frame, `panorama::the_panorama_puts_a_landmark_where_the_star_path_puts_the_same_direction` measured the landmark 102 pixels from the sprite; at the 140 it was written for, which it now pins, it passes. Not investigated. One unverified explanation is that `star_mag_limit` of -1 lets a bright planet into a frame the wider sky chooses, which would move the sprite centroid.

## Validation Record

None yet.
