# Plan: Resetting Settings to Their Defaults

## Summary

Every slider row in the Advanced section gets a ↺ in a slot after its value, shown only while the value differs from its default; clicking it puts the default back. Every subsection heading (Sun, Haze removal, Rayleigh, ...) and every group header except Rendering gets the same ↺, shown while anything under it differs, switches included, and resetting all of it.

The defaults come from `AppConfig::default()` and nowhere else. The window's settings become one Slint struct per group, nested by subsection, with no initial values in the `.slint` file, and Rust writes both the values and a `defaults` struct of the same shape. The nesting is the only place that says which settings belong to which group, so a group's ↺ is shown by `root.clouds != root.defaults.clouds` and its reset is one assignment. The duplicated defaults in `main.slint` and the test that kept them in step go away.

The top-level "Load Defaults" and "Reset" buttons keep their names and their behavior.

## Stakes Classification

Medium. The diff touches every row of `main.slint`, the two bridge functions in `ui_callbacks.rs` and most of `tests/slint_ui.rs`, but it changes nothing persisted and nothing in `sunlit-core`: the config file, `SceneParams`, the engine and the renderer stay as they are. The likely mistake is a row bound to the wrong field or the wrong default, which the tests below are built to find without naming any field.

## What the window does today

Checked against `main` at 1245e82:

- `MainWindow` has one `in-out property` per setting, 67 of them behind the Advanced toggle, each with an initial value in `main.slint`. Those values restate `AppConfig::default()`. `test_the_window_and_the_config_start_from_the_same_defaults` holds them to it, with `SLINT_LITERAL_PRECISION` for zoom, which reads 0.421 in the file and 0.42096078 from `distance_to_zoom(8.0)`.
- The running app never shows those initial values. `init_ui` writes the loaded config into the window (`app.rs:173`), the first push follows (`app.rs:393`), and only then is the window shown (`app.rs:681`). The engine starts from the config, not from the window. The initial values matter to `tests/slint_ui.rs`, whose `create_window` returns a bare window, and to Slint's live preview.
- Only `ui_callbacks.rs` (194 accessor calls) and `tests/slint_ui.rs` (143) touch the setting properties. `app.rs`, `engine_client.rs`, `tray.rs`, `displays.rs` and `about.rs` use other properties only: auto-refresh, the viewport, the derived labels, the display models. `e2e.rs` and `ipc.rs` use none.
- "Load Defaults" applies `AppConfig::default()` to everything through `apply_whole_config`, and "Reset" applies `load_config()`. The config file is written by Set as Wallpaper and, as a side effect, by any change of the display plan or of auto-refresh, which save the whole window (`ui_callbacks.rs:353`, `app.rs:233`). README describes Reset as reloading "the last saved configuration".
- The fluent `GroupBox` draws its title as a plain `Text` with nothing beside it: a `VerticalLayout` with 16px top padding, 8px spacing and 8px bottom padding, the title semi-bold at the default size, and the content in a `GridLayout` with no horizontal padding.

Slint 1.18.1 behavior the design rests on, read in the compiler and runtime sources in the cargo registry:

- A two-way binding can target a field path of a struct property. The resolver collects the path into `TwoWayBinding::Property { field_access }` (`passes/resolving.rs`), and the runtime links the two sides with `Property::link_two_way_with_map` (`i-slint-core/properties/two_way_binding.rs`).
- `==` and `!=` accept any type, structs included; only the ordering comparisons are restricted (`from_binary_expression_node`).
- Assigning to a struct field is allowed: `try_set_rw` passes a `StructFieldAccess` through to its base property.
- An `if` whose condition re-evaluates to the same `true` keeps its instance. `Conditional::ensure_updated` creates the instance only when there is none and drops it only on `false`, so a slider inside `if root.clouds.enabled` survives every write to `clouds` in the middle of its own drag.
- Generated Rust structs have only public fields and no `#[non_exhaustive]` (`generator/rust.rs`), so a struct literal that leaves a field out does not compile.
- The fluent `Slider`'s `accessible-action-set-value` clamps to the range, sets the value and fires `changed`, which lets a test move a slider the way a user does.
- An SVG `@image-url` already renders inside a window of this app (`about.slint:61`).

None of this has been compiled in this repository yet; Step 1 is where the first item would fail if the reading is wrong.

## Key Design Decisions

### 1. A reset goes to the default

A ↺ puts back `AppConfig::default()`'s value, as agreed with the user on 2026-10-14. Going back to the last saved value may become an option later. The design keeps that cheap: every ↺ goes to whatever Rust writes into `defaults` (decision 5), so the option would write the saved config there after each save, and the property would be renamed then.

### 2. One ↺ per slider row, in a reserved slot

`SettingRow` gains a 16px slot after the value, 4px from it. While the value differs from the row's default the slot holds the ↺, and otherwise it is empty, so neither the slider nor the value moves when it appears. Clicking it sets the value to the default and calls the row's `changed`, the same path a drag takes, including the snap handlers of tilt, yaw, pitch and the two offsets.

```
Clouds                                        ↺
 ☑ Show clouds
   Day opacity        [======o----]  70%  ↺
   Night opacity      [===o-------]  45%
   Night brightness   [==o--------]  20%
 Haze removal                                 ↺
   Toward the equator [=====o---]  120%  ↺
   Toward the poles   [======o--]  130%
```

The ↺ is the modified marker; there is no second one. The comparison is exact, `value != default-value`: a slider dragged back close to its default still offers the reset, which is true, and a reset lands on the default exactly, since both sides hold the f32 Rust wrote.

The slot costs the slider 20px. At the panel's default 360px, the slider of a row with a 45px value column goes from 189px to 169px, and at the 262px floor from 91px to 71px. The row's own floor becomes 161px, under the 232px the panel's content has at its floor, so the floor stays set by `SettingCombo` and `test_nothing_in_the_panel_asks_for_more_than_the_sidebar_floor` holds unchanged.

Checkboxes and combo boxes get no slot. One click on a checkbox is its own reset, and the only combos in reach are Rendering's, which decision 3 leaves out, and the Date / Time year. The header above covers their settings. Hour and Day set `resettable: false`, since a default of noon on January 1 means nothing; Date / Time has its own rule in decision 3.

The ↺ has `accessible-role: button`, the accessible label "Reset " followed by the row's label, and its default action is the reset. It has no tooltip of its own: the row's hint covers the whole row, the slot included.

### 3. One ↺ per subsection and per group

`SettingHeading` gains the same slot beside its text and becomes the reset of its subsection: Haze removal, Land, Ocean, Sky, Sun, Moon, Stars, Rayleigh, Sunrise band, Nightglow, Day Texture and Night Texture.

The group header gets it too, which needs a component of our own, `SettingGroup`, in place of the fluent `GroupBox`. It takes the fluent numbers (16px top padding, 8px spacing, 8px bottom padding, a semi-bold title, no horizontal padding) and puts the title and the slot in one row, so the group's ↺ lines up with the rows' ↺ column. Every group in Advanced moves to `SettingGroup`, Rendering included, so they keep looking alike.

| Group | ↺ shown while | Notes |
|---|---|---|
| Rendering | never | Surface, resolution and anti-aliasing are choices made once. The resolution goes through `SetTextureResolution`, and the anti-aliasing rows depend on the adapter. Load Defaults still covers them. |
| Camera Position, Camera Orientation, Framing | any of its three values differs | Camera Position will show its ↺ almost always, since every drag of the globe moves it. That is accurate, and its reset works as a "home" button. |
| Date / Time | the override is on | The reset turns it off, which is live time. The hour, day and year it held are put back too but do not count toward showing the ↺, since turning the override on writes the current instant into them every time. |
| Clouds, Atmosphere | anything differs, the switch included | The reset turns "Show clouds" or "Atmosphere glow" back on along with the rest. |
| Celestial, Lighting, Color Correction | anything differs | "Dayside shading" counts toward Lighting. |

Displays, the presets and auto-refresh sit outside Advanced and get nothing.

### 4. The settings are one Slint struct per group, nested by subsection

The 67 properties become nine, one per group, of struct types that nest the way the panel does:

```slint
export struct CameraPositionSettings { longitude: float, latitude: float, zoom: float }
export struct CameraOrientationSettings { tilt: float, yaw: float, pitch: float }
export struct FramingSettings { fov: float, offset-x: float, offset-y: float }
export struct DateTimeSettings { use-custom: bool, hour: float, day-of-year: float, year-index: int }

export struct HazeSettings { equator: float, pole: float, latitude: float, band: float }
export struct CloudCoverSettings { floor: float, gamma: float }
export struct CloudSettings {
    enabled: bool, opacity: float, opacity-night: float, night-brightness: float,
    haze: HazeSettings, land: CloudCoverSettings, ocean: CloudCoverSettings,
}

export struct SkySettings { fov: float, milky-way: float }
export struct SunSettings {
    glow: float, rays: float, size: float, halo-radius: float, flare: float,
    horizon-boost: float, horizon-reach: float, horizon-depth: float,
    reddening: float, refraction: float,
}
export struct MoonSettings { brightness: float, size: float, earthshine: float }
export struct StarSettings {
    intensity: float, size: float, glow-strength: float, glow-radius: float,
    contrast: float, mag-limit: float,
}
export struct CelestialSettings { sky: SkySettings, sun: SunSettings, moon: MoonSettings, stars: StarSettings }

export struct RayleighSettings { intensity: float, sharpness: float, haze: float }
export struct SunriseBandSettings { glow: float, width: float }
export struct NightglowSettings { intensity: float, balance: float, falloff: float }
export struct AtmosphereSettings {
    enabled: bool, rayleigh: RayleighSettings, sunrise: SunriseBandSettings,
    nightglow: NightglowSettings,
}

export struct LightingSettings {
    terminator-width: float, diffuse-shading: bool, diffuse-floor: float, diffuse-ramp: float,
    spec-shininess: float, spec-intensity: float, fresnel-mix: float, fresnel-exp: float,
}

// `gamma` is the slider's position on the curve, as `day-gamma` is today.
export struct TextureColorSettings { gamma: float, saturation: float }
export struct ColorCorrectionSettings { day: TextureColorSettings, night: TextureColorSettings }

export struct SceneSettings {
    camera-position: CameraPositionSettings, camera-orientation: CameraOrientationSettings,
    framing: FramingSettings, datetime: DateTimeSettings, clouds: CloudSettings,
    celestial: CelestialSettings, atmosphere: AtmosphereSettings,
    lighting: LightingSettings, color-correction: ColorCorrectionSettings,
}
```

```slint
in-out property <CameraPositionSettings> camera-position;
// one per group, as named in SceneSettings, none with an initializer
in property <SceneSettings> defaults;
```

A row binds its field and names its default:

```slint
cloud-opacity-slider := SettingRow {
    label: "Day opacity";
    // hint, indent and range as today
    value <=> root.clouds.opacity;
    default-value: root.defaults.clouds.opacity;
    value-text: round(root.clouds.opacity * 100) + "%";
    changed => { root.sliders-changed(); }
}
```

A subsection or a group compares and assigns the struct it stands for:

```slint
SettingHeading {
    text: "Haze removal";
    modified: root.clouds.haze != root.defaults.clouds.haze;
    reset => {
        root.clouds.haze = root.defaults.clouds.haze;
        root.sliders-changed();
    }
}
```

The nesting is the one statement of which setting belongs where. No list in Rust or in Slint names a group's members, so adding a parameter to a subsection means adding it to that struct, and its heading and its group cover it with no further edit.

There is one property per group rather than one `scene` property for everything, because a write to a struct property dirties every binding that reads any part of it. With one `scene`, each slider tick would re-evaluate the `value-text` and `modified` of all 67 rows, the `if` conditions and the layout over them. One property per group bounds that to the group being dragged. `defaults` is a single struct because it is written once per config apply and never during a drag.

The rows keep their element ids (`cloud-opacity-slider`), since the tests find rows by them.

### 5. The defaults live in `AppConfig::default()` alone

None of the nine properties has an initializer, and neither does `defaults`. `apply_config_to_window`, which startup, Load Defaults and Reset all run, writes `defaults` from `SceneParams::default()`, which is `SceneParams::from_config(&AppConfig::default())`, along with the values. Any window that has been given a config therefore knows what its resets go back to, and no path fills one without the other.

The same rule applies to the other properties whose initial value restates the config or is derived from it. They lose their literals: `aa-index: 1`, `texture-index: 3`, `texture-resolution-index: 0`, `display-mode-index: 1`, `auto-refresh-enabled: false`, `auto-refresh-interval: 5.0`, and the derived `zoom-display-distance`, `day-gamma-value`, `night-gamma-value`, `hour-label`, `day-label` and `max-day-of-year`. Startup writes all of them through `set_combobox_indices`, `apply_config_to_window` and the first `push_params` before the window is shown. Literals stay where they are UI state that no config holds: `advanced-open`, `renderer-info`'s "Initializing...", the option lists, `sky-fov-minimum` and `sky-fov-maximum`, and the panel widths.

`test_the_window_and_the_config_start_from_the_same_defaults` and `SLINT_LITERAL_PRECISION` go, since nothing is left to compare. `create_window` in `tests/slint_ui.rs` applies `AppConfig::default()` the way startup does, so every test sees the window the app shows rather than a zeroed one. Without that, the Clouds and Atmosphere rows would be missing when the floor tests measure the panel, and those tests would pass for the wrong reason.

Two costs are accepted. Slint's live preview shows every slider at zero and the Clouds and Atmosphere rows hidden. And Slint derives `Default` for the generated structs, so `SceneSettings::default()` in Rust is all zeros: the code always goes through `scene_settings(&SceneParams::default())`, and the doc comment on `scene_settings` says why.

### 6. The bridge stays two functions, now checked by the compiler

`apply_params_to_window` and `read_params_from_window` keep their names and remain the app's translation point for `SceneParams`. Their bodies change shape:

- `scene_settings(&SceneParams) -> SceneSettings` builds the whole struct as one literal, with the gamma mapping (`gamma_value_to_slider`) and the year index as today. `apply_params_to_window` destructures its result without `..` and sets the nine properties, so a group added to `SceneSettings` and never written does not compile.
- `read_params_from_window` reads the nine properties into a `SceneSettings` literal and builds `SceneParams` from it as one literal, as it does now.

Today a forgotten `set_sun_glow` compiles, and only the defaults test notices, and only if that default is not zero. After this change, a field missing on either side of the bridge is a compile error. What remains unchecked by the compiler is the `.slint` side, a row bound to the wrong field, which the tests cover.

Every other writer reads its group, changes it and writes it back: the globe, frame, orient and tilt drags and the scroll (`camera-position`, `camera-orientation`, `framing`), the presets (all three), and `datetime_override_toggled` and `update_datetime_labels` (`datetime`). The globe drag's gain reads `celestial.sky.fov`, `camera-orientation.tilt` and `camera-position.zoom`.

### 7. The ↺ is an SVG, not a glyph

`crates/sunlit-app/ui/icons/reset.svg`: a counterclockwise arrow on a 16-unit view box, drawn for this, shown with `colorize: Palette.foreground` so it follows the light and dark palettes. Whether U+21BA is in the font Slint picks on each desktop is not something this repository can check, and a missing glyph renders as a box. An SVG renders the same everywhere, as the About window's mark does.

The button gets a rounded background on hover and `mouse-cursor: pointer`.

### 8. Not chosen

- A right-click menu with "Reset to default". Slint 1.18 has `ContextMenuArea`, but with one target the menu repeats the ↺ where nobody sees it. It becomes worth adding if "revert to saved" arrives as a second entry.
- Double-clicking a label to reset. Nobody would find it.
- A ↺ on the Advanced toggle. Load Defaults already does that, one click away.
- One `scene` struct for everything (decision 4), or a group table in Rust with one reset callback, which would name each group's members a second time.
- Generating a `.slint` file of defaults in the build script. `sunlit-app`'s build would have to compile `sunlit-core`, wgpu included, as a build dependency, to evaluate a function the app can call at runtime.

## Success Criteria

1. With the default config applied, the Advanced section shows no ↺ anywhere, with every group's switch on.
2. Moving a slider shows the ↺ on its row, on its subsection heading if it has one, and on its group header, and nowhere else. Neither the slider nor the value column moves when it appears.
3. Clicking a row's ↺ puts that setting's default back, the preview re-renders, and no other setting changes.
4. Clicking a subsection's or a group's ↺ puts back every setting under it, switches included, and nothing outside it.
5. Date / Time shows its ↺ while the override is on, and clicking it returns to live time.
6. Rendering, Displays, the presets, auto-refresh, Load Defaults and Reset behave as before.
7. No literal in `main.slint` restates a config default or a value derived from one; `AppConfig::default()` is the only place a default is written.
8. The panel's floor and the window's minimum width are unchanged, and the Advanced section looks as it did apart from the ↺s, compared in screenshots before and after.
9. Dragging a slider is no slower than on `main`, measured as in Step 7.
10. `cargo test`, `cargo clippy --all-targets` and `cargo fmt --check` pass.

## Implementation Steps

The conversion does not compile halfway, so Steps 1 to 4 land as one change.

### Step 1: Structs and properties

In `main.slint`: the struct declarations of decision 4 above `MainWindow`, exported the way `MonitorTile` is; the nine group properties and `defaults` in place of the 67; and the literals decision 5 removes.

### Step 2: Widgets

In `widgets.slint`: a `ResetSlot` (the 16px slot, its `if`-gated button with the icon, the hover and the accessibility of decisions 2 and 7). `SettingRow` uses it and gains `default-value`, `resettable` (true by default) and an `out property <bool> modified`. `SettingHeading` becomes a row of its text and the slot, with `modified` and a `reset` callback. The new `SettingGroup` of decision 3. The icon file.

### Step 3: Rebind the panel

Every `SettingRow` binds its group's field and its default. Every `SettingCheck` binds its field. The `if` switches read `root.clouds.enabled` and `root.atmosphere.enabled`. The snap handlers of tilt, yaw, pitch and the offsets assign into their group. Every `GroupBox` becomes a `SettingGroup` with the `modified` and `reset` of decision 3's table, and every `SettingHeading` gets its subsection's. Rendering's `SettingGroup` sets no `modified`.

### Step 4: The bridge

In `ui_callbacks.rs`: `scene_settings`, the two bridge functions, `apply_config_to_window` writing `defaults`, and every drag, preset and datetime callback of decision 6. `engine_client.rs` needs no change, since `push_params` goes through `read_params_from_window`.

### Step 5: Tests

As listed under Tests.

### Step 6: Documentation

- `docs/architecture.md`, the UI section: `SettingGroup` in place of the `GroupBox`es; the ↺ on rows, headings and groups and what each covers; the settings structs and why there is one per group; and that `apply_config_to_window` writes `defaults` from `AppConfig::default()`, the only place defaults are written. The sentence "Camera properties are `in-out` with `<=>` slider bindings" goes.
- `docs/rendering.md`, the shader parameter recipe: the `ui/main.slint` row becomes "the field in its group's settings struct, and its `SettingRow` block with `default-value`", and the paragraph on the bridge functions says that the struct literals make a missing field a compile error, in place of the argument about grepping `set_sun_glow`. The count stays fourteen edits in seven files.
- `CLAUDE.md`, the same recipe line.
- `docs/testing.md`: a sentence on the reset tests, which move sliders through the accessibility action so that they name no field.
- `docs/roadmap.md`, under Camera and controls: an entry linking this plan.
- README is not edited.

### Step 7: Validation with the user

Screenshots of the Advanced section before and after, at the default panel width and at the floor, on Windows and in the Linux guest (`cargo xtask vm up linux`, then `vm view`). A ten-second slider drag in Celestial with `SLINT_DEBUG_PERFORMANCE=refresh_lazy,console`, on `main` and on the branch, in a release build with and without `--software-rendering`. Then the user tries it.

## Tests

In `tests/slint_ui.rs`, with `create_window` applying `AppConfig::default()` (decision 5):

- The existing round-trip, apply and load-defaults tests rewritten from the per-property accessors to the group structs. `test_the_cloud_sliders_follow_the_cloud_switch` and its atmosphere twin switch through the structs.
- `test_the_window_and_the_config_start_from_the_same_defaults` and `SLINT_LITERAL_PRECISION` deleted.
- `test_the_defaults_offer_no_reset`: Advanced open, the default config applied, no element in the tree whose accessible label starts with "Reset ". Changing the Rendering combos leaves it that way.
- `test_moving_one_slider_offers_three_resets`: one slider inside a subsection moved through `set_accessible_value`; exactly its row's, its heading's and its group's ↺ appear.
- `test_every_row_reset_returns_its_default`: every slider in the tree moved through `set_accessible_value` to a point inside its own range that differs from where it was, computed from `accessible_value_minimum` and `accessible_value_maximum` so no field is named. Then the ↺ inside each `SettingRow` is pressed, and each is gone after its own press. `read_params_from_window` then equals `SceneParams::default()`. A row bound to one field and defaulting from another ends on the wrong value here.
- `test_every_heading_and_group_reset_clears_what_it_covers`: every slider moved the same way, then every `SettingHeading`'s ↺ pressed, each gone after its own press, then every `SettingGroup`'s. Everything then equals `SceneParams::default()`. A heading that compares one struct and assigns another keeps its ↺ after the press and fails.
- `test_a_group_reset_turns_its_switch_back_on`: clouds, the atmosphere and dayside shading switched off through the structs; each group offers its ↺; after the press the switches are on and the Clouds and Atmosphere rows are back in the tree.
- `test_date_time_offers_a_reset_while_overridden`: with the override on, "Reset Date / Time" is in the tree and Hour and Day offer no ↺ whatever their values; after the press the override is off.
- `test_a_reset_pushes_the_scene`: a handler on `sliders-changed` counts calls; a row's, a heading's and a group's ↺ each fire it.

The floor and panel position tests stay as they are and must pass. With `create_window` applying the config, they now measure the panel with its Clouds and Atmosphere rows in it, as the app shows it.

## Out of Scope

- Resetting to the last saved value, per row or as an option (decision 1).
- Resets for Rendering, Displays, auto-refresh and the presets.
- An undo history.
- The side-effect saves on a display plan or auto-refresh change.
- Any change to a default.
- The names and behavior of Load Defaults and Reset, which stay as they are until the planned overhaul of how settings are managed.

## Risks and Mitigations

| Risk | Mitigation |
|---|---|
| A slider tick now re-evaluates its whole group's bindings and the layout over them, which could make drags slower, most of all in a debug build on the software renderer. | One property per group (decision 4) bounds it to the largest group, Celestial's 21 rows, and Step 7 measures it against `main`. If it is still noticeable, the subsections become properties of their own. |
| A row bound to one field and defaulting from another, or a heading that compares one struct and resets another. | The two exhaustive tests move every slider without naming one, so a row added later is covered the day it is added. |
| A row that leaves out `default-value` reads 0. | `test_the_defaults_offer_no_reset` catches it for every row whose default is not zero. The six rows whose default is zero (longitude, tilt, yaw, pitch and the two offsets) would reset correctly until that default changed; review checks those six by eye. |
| A two-way binding to a struct field behaves differently from one to a plain property in a corner the source reading missed. | The tests drive real sliders through their accessibility action, which is the user's path through the widget, and Step 7 is a manual pass in the app. |
| A path that reads the window before a config is applied now sees zeros. | None exists today (see What the window does today). `create_window` applies the defaults so the tests cannot drift into one, and a window without `defaults` shows a ↺ on almost every row, which is hard to miss. |
| `SettingGroup`'s header looks different from the fluent `GroupBox`. | It is built from the fluent source's numbers, and Step 7 compares screenshots. |
| Slint's live preview shows zeros. | Accepted (decision 5). |

## Rollback Strategy

Revert the change. Nothing persisted is touched, so a revert leaves no trace on users' machines.

## Departures

1. `display-anchor-index: 0` loses its literal too, although decision 5's list leaves it out. Row 0 is the automatic anchor, which is what `AppConfig::default()`'s empty `anchor_monitor` comes to, so the literal restated a config default the way `display-mode-index: 1` did, and success criterion 7 rules it out. An `int` without an initializer is 0, so nothing changes for a window that has not been given a config, and startup writes the index through `set_combobox_indices` before the window is shown.

2. `test_every_heading_and_group_reset_clears_what_it_covers` moves every slider a second time between the headings and the groups. Celestial and Color Correction have every row under a heading, so once the headings have been pressed those two groups have nothing left to reset and their own ↺ would never be exercised. With the second round of moves every group that holds a slider offers its ↺ and is pressed, and the case asserts that a group offers one exactly when it holds a slider, which is every group but Rendering and, with the override off, Date / Time.

3. `docs/rendering.md`'s `ui_callbacks.rs` row now names `scene_settings` and `read_params_from_window`, and `CLAUDE.md`'s recipe line says the same. Step 6 named only the `ui/main.slint` row, but adding a field to an existing group no longer touches `apply_params_to_window`, which only distributes the nine groups; the field goes into the literal in `scene_settings`. The count stays fourteen edits in seven files.

## Open items

None.

## Validation Record

Round 1: 0 major, 1 minor.

1. `test_every_heading_and_group_reset_clears_what_it_covers` presses the headings and groups in tree order and passes a press once its own button is gone, so a reset that also puts back something an earlier press had already put back went unseen: the Stars heading also assigning `celestial.moon`, or the Color Correction group also assigning `lighting`, passed every test. Success criterion 4's "nothing outside it" was tested in one direction only. Added `test_each_heading_and_group_reset_changes_exactly_what_it_covers`, which presses each heading's and each group's reset alone in a freshly moved window and holds every row to having lost its reset exactly when the pressed one covers it, with what a heading or a group covers read off the tree rather than named. Each of the two over-reaches, applied to `main.slint` by hand, failed that case and nothing else.
