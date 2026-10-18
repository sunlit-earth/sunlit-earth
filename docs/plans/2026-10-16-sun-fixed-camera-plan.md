# Plan: A Sun-Fixed Camera

## Summary

A second camera mode in which the camera keeps its place relative to the Sun and the Earth turns beneath it once a day. The lighting the camera sees stays where it is: a view of the day side stays a view of the day side, and a sunrise at the limb stays a sunrise. Today's mode, where the camera hangs over one place on the Earth and turns with it, stays the default and is drawn exactly as before.

In the new mode the Longitude row of Camera Position is replaced by a Time of day row, the local solar time beneath the camera, kept in a setting of its own and separate from the Date / Time override. Latitude, zoom, orientation and framing keep their meaning. Switching modes converts between the two so the view does not jump, and a preset applies as of the moment it is clicked.

Nothing in the shader changes. The camera the renderer draws with is resolved from the settings and the sky state of the frame in one function, and that resolved camera is what the uniforms, the tile residency and the dirty check see.

## Stakes Classification

Medium. The diff touches `SceneParams` and `AppConfig`, the camera in `write_uniforms`, every tile output, the dirty key, the engine's sky schedule, the Camera Position group in `main.slint` and the camera callbacks in `ui_callbacks.rs`. The Earth-fixed mode must come out bit for bit as it is, which the unchanged golden suite checks. The likely mistakes are a path that builds a camera from `params.camera` without resolving it, so that the preview, the wallpaper and the wanted tiles disagree about which hemisphere is in view, and a sign error in the solar time conversion. Both have tests below.

## What the camera does today

Checked against `main` at bfce47a:

- The world frame is Earth-fixed: +Y is the north pole, +Z is 0N 0E, +X is 0N 90E (`scene/camera.rs` `eye_position`, `scene/sun.rs`). `OrbitalCamera` puts the eye at a longitude, latitude and distance, with world +Y as up; yaw, pitch and tilt are measured against that.
- Everything in the sky is rotated into this frame for each frame. `SkyState` (`scene/sky.rs`) carries `world_from_eqj` with Greenwich sidereal time in it, the Sun's direction, the planets and the Moon. So in today's mode the globe stands still and the stars turn once per sidereal day and the Sun once per solar day.
- `OrbitalCamera::from_params` is called by `write_uniforms` (`renderer/render_pass.rs:125`) and by the residency's `Lens::new` (`renderer/residency.rs:366`). The residency reads its camera from `Output::camera` only, and the engine builds every `Output` in `tile_output` (`engine/mod.rs:970`), called from `want_tiles` and three times in `engine/publish.rs`.
- The renderer's dirty key, `FrameState` (`renderer/frame.rs`), is the params digest (camera compared exactly), the target size, and the Sun direction and two sky basis vectors quantized to milliradians. One milliradian of the Earth's turn is about 14 seconds.
- With live time the engine marks the scene dirty every `SKY_INTERVAL`, two minutes (`engine/mod.rs:61`). The draw itself is not gated on the preview being shown; only the readback and the `PreviewFrame` event are.
- A wallpaper publish draws once and replays the sky state the renderer then holds for every shot (`prepare_export`), so all screens of one publish are drawn at one instant.
- The tile loader's drag watch (`engine/tile_loader.rs:164`) compares the longitude and latitude of `self.params.camera` between draws and reports a rate when the camera moved within `DRAG_PAUSE`, 100 ms, of the last draw. `residency.rs:792` reads tiles ahead along that rate.
- Camera Position holds Longitude, Latitude and Zoom, each `resettable: false`, and the group has no reset (`main.slint:591`). A preset sets position, orientation and framing from `PRESETS` (`ui_callbacks.rs:170`). The left drag moves longitude and latitude through `mouse_math::apply_globe_drag_at`.
- Turning the Date / Time override on starts it at the current instant (`ui_callbacks.rs:210`), and while it is on the sky schedule redraws nothing.
- The wallpaper's auto-refresh defaults to five minutes.

## Key Design Decisions

### 1. The frame holds the Sun, not the stars

Decided with the user on 2026-10-16. A camera fixed against the stars would see the Earth turn once per sidereal day with the star field still, but the Sun moves about a degree a day against the stars, so a view of the full day side today would show only the night side six months later, all day long. A camera fixed against the Sun keeps the phase it shows forever, and the Earth turns once per solar day, which is the "once per day" asked for. The stars drift by about a degree a day instead, which nobody watching a wallpaper will see.

### 2. The frame turns about the Earth's axis and nothing else

The Sun-fixed frame is equatorial: it is the Earth-fixed frame turned about +Y by the Sun's longitude. The camera's latitude stays a latitude on the Earth, up stays north, and tilt, yaw and pitch mean what they mean today. What moves is the Sun's declination: it swings 23.44 degrees either side of the equator over the year, at most about 0.4 degrees a day, so the terminator slides north and south with the seasons and a Sun placed just behind the limb drifts off it over days rather than hours.

The alternative, an ecliptic frame that holds the Sun exactly still in the picture, makes the Earth's axis nod 23.4 degrees toward and away from the camera over the year and the camera's latitude wander with it, so every orientation setting would change meaning with the season. Not chosen.

### 3. Time of day replaces Longitude, and is stored on its own

As asked by the user: in the Sun-fixed mode the Longitude row is replaced by a Time of day row, and the value lives in a new key rather than reinterpreting `longitude`. Each key then has one meaning: `longitude` is always a geographic longitude and `solar_time` is always a local solar time. The key is not called `hour` or `time` so that it cannot be mistaken for the override's `custom_hour`.

Time of day is the local apparent solar time beneath the camera, in hours from 0 to 24: 12:00 hangs the camera over the meridian the Sun stands over, 18:00 over the evening terminator, 00:00 over the night side. With `λs` the Sun's longitude, `atan2(sun.x, sun.z)` in the world frame:

```
longitude  = wrap180(λs + (solar_time - 12) * 15)
solar_time = wrap24(12 + (longitude - λs) / 15)
```

The row's label is formatted from Rust as `HH:MM`, the way the override's hour label is. Default 12:00, the day side.

Only the active key is read. The inactive one keeps whatever it held at the last switch and is overwritten by the next one (decision 5), so its stale value is never shown or drawn.

### 4. Apparent solar time, not mean

The Sun's actual longitude is used, so the Sun sits at exactly the same hour angle all year. Mean solar time would make the lighting wander by up to 16 minutes, about 4 degrees, over the year with the equation of time, for no gain. A consequence is that one turn of the globe takes one apparent solar day, which differs from 24 hours by at most about half a minute.

### 5. Switching modes keeps the view

Decided with the user on 2026-10-16. Turning the mode on sets `solar_time` from the current longitude at the current instant, and turning it off sets `longitude` from the current time of day, so the frame before and after the switch is the same and from then on either the Sun or the surface stays put. "The current instant" is the override's when it is on and the system clock's otherwise; the engine draws within a tick of the window's reading, and 50 ms of the Earth's turn is a fifth of a thousandth of a degree.

The conversion runs in the app's toggle callback, which computes the sky state with `sky::compute_sky_state` from the window's date and time and pushes the mode and the converted value in one `push_params`. The two conversion functions live in `sunlit-core` next to the camera, so the app has no astronomy of its own.

### 6. A preset applies as of now

Decided with the user on 2026-10-16. A preset clicked in the Sun-fixed mode leaves the mode on, sets latitude, zoom, orientation and framing as today, and sets the time of day so that the preset's longitude is under the camera at the current instant. The place is centered on the click and turns away from there. The preset table stays Earth-fixed and gains no field.

### 7. The globe drag moves the time of day

In the Sun-fixed mode the left drag's horizontal part goes into `solar_time` at 15 degrees an hour, through the same `apply_globe_drag_at` with the held longitude `(solar_time - 12) * 15` in and out, so its gain, its tilt correction and its wrapping are the ones the drag has today. The vertical part moves latitude as before.

### 8. One function resolves the camera

```rust
impl SceneParams {
    /// The camera in the Earth-fixed world frame, at the instant of `sky`.
    pub fn camera_at(&self, sky: &SkyState) -> CameraParams;
    /// The camera as the settings hold it: the held longitude in the Sun-fixed mode.
    pub(crate) fn held_camera(&self) -> CameraParams;
}
```

`camera_at` returns `self.camera` unchanged in the Earth-fixed mode, and with the longitude from decision 3 in the Sun-fixed mode. It replaces `&params.camera` in `write_uniforms`, which gives it the sky the renderer holds, so a publish's replayed shots all resolve at the publish's instant. `tile_output` takes the sky its output is drawn under and builds `Output::camera` from `camera_at`, so `OrbitalCamera::from_params` and everything in the residency see Earth-fixed cameras only and do not change. The callers in `publish.rs` pass the sky the renderer holds for the export, the same one the draw replays.

The two new fields sit in `SceneParams`, not in `CameraParams`: the preset table is a table of `CameraParams` and a solar time means nothing in it, and the digest's mutation table adds 1.0 to every `CameraParams` field, which a bool cannot take. In `scene_digest!`, `sun_fixed` goes in `switched` and `solar_time` in `absent`, with the reason `datetime` gives: the frame key compares the camera it resolves instead (decision 9).

### 9. The dirty key compares the resolved camera

`FrameState` gains `camera: CameraParams`, the resolved camera, compared exactly like the digest's. The Sun direction is in the key already, but only to a milliradian, and at the closest zoom a milliradian of the Earth's turn is about 6 pixels on a 1080 pixel tall frame and takes 14 seconds, so a draw asked for by a shortened sky schedule (decision 11) would be skipped as unchanged. Exact comparison costs nothing extra: a draw is only attempted when something marks the scene dirty, and the schedule decides how often that is. In the Earth-fixed mode the new field equals the digest's camera and the key behaves as today.

### 10. The drag watch sees the camera as held

`surface.drag` and `surface.settle_drag` are given `held_camera()`, not the resolved camera. The Earth's turn is then never a drag, and a hand on the Time of day row or on the globe is one, at a rate in the same degrees and the same sign as the residency's read-ahead expects: a later time of day moves the camera east, as a larger longitude does. The read-ahead adds that rate to the resolved camera's longitude, which is off from the true rate by the Earth's turn, a quarter of a degree a minute against a drag's tens of degrees a second.

### 11. The preview keeps up with the turn while it is shown

At two minutes the preview steps visibly in the Sun-fixed mode. The surface under the camera crosses the frame at about `focal * ω / (d - 1)` pixels a second, with `focal = height / (2 * tan(fov / 2))`, ω the Earth's turn and d the distance in Earth radii. At the default 20 degree lens on a 1080 pixel tall frame:

| Distance | Pixels per second | One pixel every | Step at 2 min | Step at a 5 min refresh |
|---|---|---|---|---|
| 8 (default) | 0.032 | 31 s | 4 px | 10 px |
| 3.2 (Europe) | 0.10 | 10 s | 12 px | 31 px |
| 1.5 (closest) | 0.45 | 2.2 s | 53 px | 134 px |

So in the Sun-fixed mode with live time and the preview shown, the sky schedule's interval becomes the time the preview target takes to move one pixel, by that formula with the target's own height and the camera's own lens and distance, clamped between a floor and `SKY_INTERVAL`. It is recomputed on `UpdateParams`, `SetPreviewSize` and `SetPreviewEnabled`. With the preview hidden, in the Earth-fixed mode, or with the override on, the interval stays two minutes, because a hidden window still draws on that schedule and has nobody to show the frame to.

The floor is a proposal of 5 seconds. Step 3 measures a preview draw on the software adapter and on the desktop's GPU before it is fixed.

Step 3 measured it on 2026-10-18 on the development desktop (Ryzen 7 5800X, Radeon RX 6800 XT), with the engine built at `opt-level = 2`: the time from an `UpdateParams` that moves the time of day to the `PreviewFrame` it produces, so the sky state, the wanted set, the draw and the readback, in blend mode with the Earth fixture's cube and its tiles, the cloud fixture, the real Moon and Milky Way, MSAA as the High tier allows, at the Europe preset, median of 25 frames after 5 to warm up. The Slint side's own upload and paint of the frame is not in it.

| Preview | WARP | Radeon RX 6800 XT (Vulkan) |
|---|---|---|
| 704 by 576, about the default window at 100% scaling | 79 ms | 1.3 ms |
| 1088 by 896 | 172 ms | 2.2 ms |
| 1920 by 1088 | 324 ms | 3.9 ms |

The floor stays at 5 seconds. It only binds at the closest zooms, below about 2.1 radii on a 1080 pixel tall preview and below about 1.6 on the default one, and there it costs WARP between 1.6% of the time at the default preview and 6.5% at a 1080 pixel one. The GPU could take a floor of a second for a fraction of a percent, but at 5 seconds the steps are still at most about 2 pixels at the closest zoom on a preview up to 1080 pixels tall, so one floor for every adapter is kept rather than a second constant.

The wallpaper keeps its own auto-refresh interval: the steps in the last column are what a user of this mode will see on the desktop, and a shorter interval is theirs to choose.

### 12. The override decides the Earth's turn

In the Sun-fixed mode the instant decides which side of the Earth faces the camera, so with the override on, the Hour slider turns the globe under a still light and the Day slider moves the terminator north and south. Turning the override on changes nothing, since it starts at the current instant; turning it off turns the globe to the present. That is the same jump the Earth-fixed mode makes in the light. The override's hint gains that the globe's turn stops with the Sun, the Moon and the stars.

### 13. The mode is a check box

A `SettingCheck` at the top of Camera Position. Proposed text "Fixed toward the Sun", hint "Keeps the camera where it is relative to the Sun, so the Earth turns beneath it once a day and the lighting it shows stays the same." Like the rest of the group it has no reset. Below it, `if` the mode is on the Time of day row stands where Longitude stood, otherwise Longitude does. `CameraPositionSettings` gains `sun-fixed` and `solar-time`, and the check's `toggled` calls a new `camera-frame-toggled()` that runs decision 5's conversion.

The config gains `sun_fixed = false` and `solar_time = 12.0` in the camera position block of `AppConfig`. No migration: both keys are new, and their defaults draw what every existing config draws today.

## Edge cases at a glance

| Situation | Behavior |
|---|---|
| Switching the mode on or off | The view stays; the converted value is written to the other key (decision 5) |
| Preset in the Sun-fixed mode | The place is centered now and turns away; the mode stays on (decision 6) |
| Left drag in the Sun-fixed mode | Moves the time of day and the latitude (decision 7) |
| Override on, Hour slider | Turns the globe under a still Sun (decision 12) |
| Override turned off | The globe turns to the present instant |
| Seasons | The terminator and the Sun slide north and south, at most 0.4 degrees a day (decision 2) |
| Camera near a pole | The globe spins about the view axis; the latitude clamp is unchanged |
| Several screens | One instant per publish, so a spanned picture still meets at the seams |
| Window hidden | No extra draws; the schedule stays at two minutes (decision 11) |
| Load Defaults | Back to the Earth-fixed mode, longitude 0, time of day 12:00 |
| Reset | The saved config, whichever mode it was in; no conversion, since mode and value come together |
| `render`, `displays --out` | Drawn as of the render's instant, from the config like everything else |
| Config without the new keys | The Earth-fixed mode, as today |
| An older build reads the config | Drops both keys and shows the longitude saved at the last switch |
| Moon, planets, clouds | Unchanged: they are in world space, and the clouds turn with the globe |

## Success Criteria

- With `sun_fixed` false every golden reference passes unchanged and no existing test changes.
- In the Sun-fixed mode, frames hours apart on one day show the Sun at the same hour angle from the camera, and the surface moving west to east, left to right with north up.
- A Sun-fixed render at an instant matches an Earth-fixed render at the converted longitude within the golden tolerance.
- Switching the mode, applying a preset and dragging the globe never make the view jump.
- The wanted tiles follow the turn: what comes over the western limb is fetched without a drag and without waiting for a wallpaper's re-export.
- The preview moves in steps of about a pixel while it is shown, and a hidden window draws no more often than today.

## Implementation Steps

### Step 1: Core model

The conversions `subsolar_longitude`, `longitude_of(solar_time, sun)` and `solar_time_of(longitude, sun)` in `scene/camera.rs`; `sun_fixed` and `solar_time` in `SceneParams`, `from_config`, `write_to_config` and `scene_digest!`; the two keys in `AppConfig` with their defaults; `camera_at` and `held_camera`. Unit tests 1 to 7 below.

### Step 2: Wiring

`write_uniforms` takes `params.camera_at(inputs.sky)`; `tile_output` takes the sky and resolves; its callers in `engine/mod.rs` and `publish.rs` pass the sky of their draw; `FrameState` gains the resolved camera; the drag watch gets `held_camera()`. After this step no `params.camera` reaches `OrbitalCamera::from_params` outside `camera_at`, which a grep confirms.

### Step 3: The sky schedule

The interval of decision 11, recomputed on the three commands. Measure one preview draw at the default preview size on the software adapter (a VM guest or `--software-rendering`) and on the desktop GPU, and fix the floor from that.

### Step 4: The window

`main.slint`: the struct fields, the check, the `if` around the two rows, the `solar-time-label` property and the new callback. `ui_callbacks.rs`: `scene_settings`, `read_params_from_window` and `apply_params_to_window`; the toggle with its conversion; the preset branch; the globe drag branch; the label update. The override's hint.

### Step 5: Tests

The engine, render and window tests below.

### Step 6: Documentation

`docs/roadmap.md` gains the item under Camera and controls. `docs/architecture.md` says, next to the two translation points, that the camera the renderer draws with is resolved by `SceneParams::camera_at` from the params and the sky state, and that tile outputs and the dirty key take the resolved camera. `docs/rendering.md` gets the mode in its camera section and decision 11's table. `docs/usage.md` describes the mode, the Time of day row and the step sizes at each auto-refresh interval. README only with the user's permission.

### Step 7: Validation with the user

Run the app with the preview open at the Europe preset in the Sun-fixed mode for several minutes; toggle the mode, apply presets and drag the globe in it; publish a wallpaper across two screens in a Linux guest with `--screens 2`.

## Tests

Core, unit:

1. `longitude_of` and `solar_time_of` round trip over a grid of Sun longitudes and inputs, across both wraps.
2. With the latitude at the Sun's declination, 12:00 puts the eye on the Sun's direction, 00:00 opposite it, and 18:00 a quarter turn east of the subsolar meridian.
3. At three pinned instants six hours apart on one day, the resolved eye's longitude relative to the Sun is the same to well under a degree.
4. One hour later the resolved longitude is about 15 degrees further west.
5. In the Earth-fixed mode `camera_at` returns `params.camera` exactly, under any sky.
6. The frame key changes with `solar_time` in the Sun-fixed mode and not in the Earth-fixed mode, and with `longitude` the other way round. The digest's mutation table covers `sun_fixed` by itself.
7. A config without the new keys loads as the Earth-fixed mode at 12:00, and both keys round trip.

Engine, `tests/engine.rs` on the mock clock:

8. Sun-fixed, live time, preview shown: advancing the clock by the computed interval draws a frame, and the output the tile loader is asked for carries the resolved longitude. Hidden or Earth-fixed, nothing draws before two minutes.
9. Sun-fixed with only the clock moving reports no drag; successive `UpdateParams` changing `solar_time` within `DRAG_PAUSE` report one, with a positive longitude rate for a later time.

Render, in the `golden` target on its shared engine:

10. A Sun-fixed render at a pinned instant and an Earth-fixed render at `longitude_of` that instant match within the golden tolerance. A comparison, not a new reference.

Window, `tests/slint_ui.rs`, with the override on at a pinned instant so that the conversion is deterministic:

11. The check swaps Longitude for Time of day and back, and the resolved camera before and after each toggle agrees to a hundredth of a degree.
12. A preset in the Sun-fixed mode keeps the mode and sets the time of day that puts its longitude under the camera at that instant.
13. A horizontal globe drag in the Sun-fixed mode changes the time of day by the longitude it would have changed over 15, leaves `longitude` alone, and wraps across midnight.
14. Load Defaults returns to the Earth-fixed mode.

## Out of Scope

- A star-fixed mode. Decided against on 2026-10-16; `camera_at` is the one place it would go.
- Time playback, the roadmap item, which in this mode would spin the globe.
- Presets defined against the Sun, such as a Blue Marble that always faces noon.
- A shorter wallpaper refresh for this mode, or an animated wallpaper.
- Keeping `longitude` current while the Sun-fixed mode is on, which only an older build reading the config would notice.

## Risks and Mitigations

- A path that misses the resolution would draw the preview from one hemisphere and fetch tiles for another, or make the wallpaper disagree with the preview. Mitigated by funneling every camera through `camera_at` and `tile_output`, the grep in step 2, and tests 8 and 10.
- A floor too low for a software adapter would keep a CPU busy while the window is open. Step 3 measures before the floor is fixed, and the interval only shortens while the preview is shown.
- Two time controls in one window, the override's Hour and Camera Position's Time of day. They sit in different groups with different labels, and the hints say what each one moves; step 7 is where the user judges whether that is enough.

## Open items

- The check box's text.
- The schedule's floor, after step 3's measurement. Measured and kept at 5 seconds (decision 11); a lower floor on a GPU is the user's call in step 7.

## Departures

1. **The digest copies the held camera.** Test 6 asks that the frame key not change with `longitude` in the Sun-fixed mode, and `ParamsDigest` copied `SceneParams::camera` whole, longitude included, so a change of a key the mode does not read would have drawn a frame. `digest()` takes `held_camera()` instead, which is `camera` itself in the Earth-fixed mode, so nothing changes there. `solar_time` stays in `absent` as decision 8 has it, and its comment says that in the Sun-fixed mode it reaches the digest through the held camera; the frame key's own `camera` field is the resolved one, as decision 9 has it.

2. **The tile waits resolve under the sky the wanted set was computed under.** Decision 8 has the callers in `publish.rs` pass the sky the renderer holds for the export. That is right for an export's cap, which `export_capped` takes for the replay drawn under that sky, and that call does it. The two pending checks, `shots_pending` and `export_pending`, ask `TileLoader::missing_for`, which answers only for an output the set in force contains, compared exactly, and the set can be computed without a draw: `want_now` computes it under a fresh sky the renderer never holds. With a Sun-fixed camera at live time a camera resolved at another instant is a different output, so a check under the renderer's sky would call a wallpaper's tiles pending until the next draw or the end of `TILE_WAIT`. The engine keeps the sky it last computed the set under (`wanted_sky`) and both checks resolve under it. After `prepare_export`, which every publish and export draws before it answers, the two skies are the same one, and in the Earth-fixed mode no sky reaches an output, so nothing changes there.

3. **Three existing tests needed an edit to compile, none to pass.** The success criteria say no existing test changes. `serde_round_trip_non_default` and `save_and_load_round_trip` in `config/mod.rs` spell out every `AppConfig` field and gained `sun_fixed` and `solar_time`, with values other than the defaults so both round trips cover the new keys, and `test_every_preset_button_fires_the_index_it_is_named_for` in `slint_ui.rs` builds a `CameraPositionSettings` of its own and gained `..win.get_camera_position()`. The golden target's waits for a case's textures moved, unchanged and in the same order, into a `render` helper that `check_golden_in` and test 10 share. Every golden reference passes as it is.

4. **Test 9's half about the clock cannot tell the held camera from the resolved one at the engine.** The sky state is computed to the whole second, since `CivilTime` takes the clock's whole seconds, so the resolved camera moves once a second, and `DragWatch` counts a move as part of a drag only within `DRAG_PAUSE`, 100 ms, of the end of the draw of the move before it. The turn alone therefore never reads as a drag whichever camera the watch is given. The engine case asserts no drag over 24 draws 50 ms apart, and a mutation run that gave the watch `camera_at` still passed it. What pins decision 10 is a `DragWatch` unit case, `a_later_time_of_day_is_a_drag_to_the_east`, which gives the watch `held_camera` and checks the rate's sign and unit and that it turns the short way across midnight, together with the engine case's second half, where two moves of the time of day 30 ms apart are a drag.

5. **Test 7 checks what the default draws, not its number.** `docs/testing.md` asks for behavior rather than constants, so the case for a config without the new keys checks that it draws the Earth-fixed camera under two skies, and a separate case checks that the default time of day, switched on, looks at the sunlit side; the existing `a_field_a_file_leaves_out_arrives_at_its_default` already covers that both keys arrive at their defaults.

6. **The Hour row's hint changed too.** Decision 12 names the override's hint. The risk about two time controls in one window is mitigated by the hints saying what each one moves, so the Hour row's hint also says that the hour decides which side of the Earth a camera fixed toward the Sun looks at.

7. **`scene::sky::compute_sky_state_at` is public.** Test 8 compares the tiles a Sun-fixed camera wants on the mock clock with those an Earth-fixed camera at the converted longitude wants, and converting needs the sky at the mock clock's instant, which the integration target could not name while the function was private to the crate.

8. **Four existing engine cases and two of their helpers changed to close races the base has too.** The success criteria say no existing test changes, but the engine target was not reliably green: validator round 1 saw 3 red runs in 8 on this branch, each failing one of three existing cases. Two of the races are reproduced and are not this feature's. `textures::a_switch_to_the_current_resolution_does_nothing` failed 3 times in 3 on this branch and 3 times in 3 on e1fe60c whenever `memory::a_switch_down_leaves_no_tile_array_behind` ran just before it, the order of the red validator run: `groups::surface` restored the resolution and never waited for the cloud variant the restore refetches, which uploaded inside the case's 500 ms window (a probe saw a download and three decodes there and no tile read). `surface` now waits for the third poll that reaches the fixture after the restore, which `FixtureClouds::polls` counts, and the case settles on a set that is not mid-drag. `tiles::tiles_that_cannot_be_read_are_drawn_from_the_floor` asserted on the set of a drag in 6 runs of 6, because on the system clock its first move always came within `DRAG_PAUSE` of the first sight, and failed whenever the drag ended before its last read count; it and `tiles_that_land_are_drawn_into_the_next_preview_frame` now hold the standing clock the camera-move case had, through a `stand_still` helper the three share. The third case, `readiness::the_tile_wait_starts_over_when_the_resolution_changes`, failed once, in the implementer's first run, which took 600 s against the usual 230 and had none of the new cases yet. Its engine has no cloud source and a mock clock, so neither mechanism above reaches it. It passed 20 lone runs and failed 1 in 10 with 16 CPU-bound processes beside it; with temporary diagnostics, a failing run showed the purge taken during the `settle` after the switch and the tile wait restarting only at the next clock move. `Harness::settle` counts on its two round trips bracketing a tick, but under that load the second request can land in the same drain as the first, so no tick runs between them, and the tick that starts the wait over ran after the case had moved the clock, which left the publish it waits for a whole `TILE_WAIT` away on a clock that does not move again. The case now waits for the wanted set the purge empties to be asked for again, which only a tick does, and before that tick's publish check; under the same load it then passed 10 of 10. The same load on e1fe60c gave 5 passes, one run with no result line that the time box left undiagnosed, and 4 runs after the load had stopped, so the base was not shown to fail; the harness and the case are unchanged from the base, so the race is the base's by its code. No assertion was loosened. `docs/testing.md` records the two conventions under Conventions.

## Validation Record

### Round 1

Validator round 1 on e1fe60c..27631bb found one major and two minors.

- Major: `cargo test` was not reliably green, the engine target failing 3 runs in 8 on the branch with one of three existing cases. Fixed in bb47824 and d63252c (departure 8). After bb47824 the engine target passed 8 full runs of 8 in a row, 125 cases each, 204.7 to 210.2 s; d63252c came after those runs and was checked by 10 loaded runs of its case and the full `cargo test` gate.
- Minor: `sun_fixed.rs` left the shared plain engine at live time and zoom 0. Fixed in bb47824: the case puts `test_params()` back before it ends.
- Minor: `docs/architecture.md` said `sun_fixed` and `solar_time` are not shader parameters, which reads as an exception to the rule that keeps such settings out of `SceneParams`. Reworded in 83c50c7 to say they change the uniforms through the camera they decide.
- Open: `Harness::settle` itself still cannot promise a tick between its two replies under heavy load, and every mock-clock case that settles and then moves the clock has the exposure d63252c closed for one case. A fix that holds everywhere needs a command the engine answers at the end of a tick rather than in its drain, a change to the engine's protocol that this round did not make.

### Round 2

Validator round 2 on 27631bb..99107d2 verified the three fixes and found nothing new: no majors, no minors. It found production code unchanged in the round, traced each race fix to its cause in the engine, and reproduced the order-dependent pair `memory::a_switch_down_leaves_no_tile_array_behind` then `textures::a_switch_to_the_current_resolution_does_nothing`: 3 failures in 3 at 27631bb, 4 passes in 4 at 99107d2. The full `cargo test` gate passed and two more engine runs passed 125 of 125, at 210.3 and 207.8 s. The load-dependent race in the tile wait case was not reproduced, since no artificial load was allowed this round, and the open item above stands.
