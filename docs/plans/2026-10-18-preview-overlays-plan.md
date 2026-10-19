# Plan: Camera Panel and Time Bar on the Preview

## Summary

Two controls drawn on top of the 3D preview, both Slint elements in the image container and so never part of an export or a wallpaper.

The camera panel sits in the top right corner and gives direct access to the camera position, orientation and framing, minus the field of view: a compass ring for the roll with an N that puts north back up, an orbit joystick with step arrows, zoom buttons and a zoom slider with a mark at the default, a look joystick and a frame joystick, each with a ↺ while it differs from its default, and buttons for the shortcut sheet and for hiding the overlays. It is always expanded; while the pointer is elsewhere its background disappears and its controls drop to about a third. It teaches the mouse gestures: every control's hover text names the gesture that does the same on the globe, a gesture on the globe lights up its control, and the first use of a control whose gesture has not been used yet says which one it is.

The time bar sits at the bottom center. Folded, it is a pill with the state and the time; on hover it opens into a bar with play/pause between two speed rulers (backward on the left, forward on the right), skip buttons, the speed on the left, a clock block on the right (the state with the offset, the time with its zone, the date), and two tracks: the time of day, shaded for day and night under the camera, and the date over one year, with the equinoxes and the solstices marked. The labels in front of the tracks are radio buttons choosing what play runs: the clock, or the date at a fixed time of day. A pin keeps the bar open and at full strength.

Behind the time bar the date and time setting changes shape: live time or one instant, in place of the hour, the day of the year and the year in UTC, shown in local time by default. That setting is what the wallpaper uses, and the sidebar's Date / Time group and the time bar's tracks are two views of it. Playback is something else: an excursion of the preview that the engine runs on its own clock, that never touches the wallpaper, and that pauses when the window closes. Keyboard shortcuts for both controls come with a sheet that lists them.

The design was settled on an interactive mockup, [mockups/2026-10-18-preview-overlays/index.html](../mockups/2026-10-18-preview-overlays/index.html), which this plan refers to for layout, behavior and the constants it was tuned with. The mockup has a single time; the split between the setting and the excursion (decision 6), the Preview state that marks an excursion (decision 7) and the sidebar's Date / Time group (decision 8) are this plan's and not in it. The research behind it is [2026-10-18-preview-overlays-research.md](2026-10-18-preview-overlays-research.md).

## Stakes Classification

High. The new time setting changes a persisted setting's shape, so it takes a config migration, and playback changes the engine loop's cadence while it runs. The camera panel touches nothing outside `sunlit-app`. The work is three changes (Implementation Steps), and the camera panel can ship first.

## What the window and the engine do today

Checked against `main` at 2724285, after the Sun-fixed camera (#80) and the ocean colors (#81):

- The preview is an `Image` inside `image-container` in `main.slint`, under one `TouchArea` (`viewport-touch`) that tracks `left-held`, `right-held` and `middle-held` and calls `mouse-drag-globe`, `mouse-drag-frame`, `mouse-drag-orient`, `mouse-drag-tilt` and `mouse-scroll`. `ui_callbacks::register_mouse_callbacks` applies them through `mouse_math.rs` and ends each in `engine.push_params`. The loading pill is the only other element over the image. There are no keyboard shortcuts anywhere in the window, and the field of view has no gesture.
- The date and time setting is `DateTimeSettings { use-custom, hour, day-of-year, year-index }` in the window and `DateTimeInput { use_custom, custom_hour, custom_day_of_year, custom_year }` in `SceneParams`, with the hour in UTC and the year within ten of the current one. The sidebar's Date / Time group shows it as an "Override date/time" switch, Hour and Day sliders and a Year combo box, with a ↺ while the override is on. `CivilTime::of` (`scene/month.rs`) turns it into a civil time, `time_for_input` (`scene/sky.rs`) into an Astronomy Engine time, and live time drops the sub-second part. The config keys are `use_custom_datetime`, `custom_hour`, `custom_day_of_year` and `custom_year`, and the schema is at version 2 (`config/migrate.rs`).
- `datetime` is not in `ParamsDigest`; the engine compares the derived sky state instead. Live time re-renders on `sky_interval` (`engine/mod.rs`): `SKY_INTERVAL`, two minutes, except for a shown Sun-fixed preview at live time, where it is the time the preview takes to move one pixel, at least `SKY_FLOOR`, 5 s. The loop waits on its channel for at most `TICK`, 50 ms.
- The camera has two modes since [2026-10-16-sun-fixed-camera-plan.md](2026-10-16-sun-fixed-camera-plan.md). `SceneParams::sun_fixed` and `solar_time` sit beside `camera`; in the Sun-fixed mode the Longitude row is a Time of day row and the longitude drawn is `camera_at(sky)`, resolved under the sky of each draw, so the instant decides which side of the Earth faces the camera. The globe drag moves the time of day through `camera::held_longitude` and `solar_time_of_held`. Switching the mode and applying a preset convert at `sky_now`, the override's instant or the system clock's (`ui_callbacks.rs`).
- `sync_month` derives the month in force, and on a CPU adapter the month ahead, from the date before every draw. In Blend mode the tiles a frame wants depend on the Sun's direction (`residency::surfaces_for`), and the drag observer (`DragWatch` in `engine/tile_loader.rs`, called through `surface.drag`) coarsens the wanted set while the camera moves. It watches `held_camera()`, so in the Sun-fixed mode the Earth's turn is never a drag.
- `EngineEvent::PreviewFrame` carries the pixels and the size, not the time the frame shows. Hiding the window sends `SetPreviewEnabled(false)`.
- About 150 lines in `sunlit-core`'s sources and tests name `DateTimeInput` or its fields: `month.rs`, `sky.rs`, `sun.rs`, `params.rs`, `config/mod.rs`, `engine/mod.rs` (`sky_interval` among them), the golden, render pipeline and soak targets, and the engine tests' harness, moon, months, panorama, clouds, readiness, cube, sun, sun_fixed, loading and tiles cases. `ui_callbacks.rs` (`sky_now` among them) and `slint_ui.rs` name the window side, and `crates/sunlit-app/tests/fixtures/e2e_config.toml` holds the four keys.
- A config file written by a newer build loads best effort: `migrate.rs` leaves it alone and serde fills what it does not know.
- README's usage section lists the five mouse gestures and mentions "a date and time override" under Advanced.

Slint 1.18.1 behavior the design rests on, read in the compiler, core and renderer sources in the cargo registry:

- Children of `image-container` declared after `viewport-touch` are drawn above it and take pointer events inside their own bounds first.
- `transform-rotation` is accepted on every element, but the software renderer's `rotate()` is an empty `TODO (#6068)` (`i-slint-renderer-software/lib.rs`). The VMs and `--software-rendering` use that renderer. Anything that turns with the roll is therefore placed with computed coordinates or drawn as a `Path` whose coordinates are bindings; `Path` is in the software renderer through its `std` feature.
- `Timer` is a built-in element (`interval`, `running`, `triggered`), and `i_slint_backend_testing::mock_elapsed_time` drives it in tests.
- `KeyBinding { keys: @keys(...); activated => ... }` inside a `FocusScope` matches logical keys, and `KeyboardModifiers.control` is the Command key on macOS, so a binding written with `Control` is ⌘ there with no second binding. `FocusScope` also has `key-pressed` and `key-released`, which held keys need.
- `PointerScrollEvent` carries `modifiers`.
- `DatePickerPopup`, `TimePickerPopup` and `SpinBox` are in std-widgets.

The `time` crate at 0.3.47 answers `UtcOffset::local_offset_at` on Unix through `localtime_r` with no thread check, so it works in this multi-threaded process; only `refresh_tz` is limited to single-threaded processes and macOS. It needs the `local-offset` feature, which the workspace does not enable yet.

## Key Design Decisions

### 1. Both controls are Slint elements over the preview

They live in the window, not in the render: an export, a wallpaper and a golden image never contain them, and nothing in `sunlit-core`'s renderer changes for them. They go in new files under `crates/sunlit-app/ui/overlays/` and are placed in `image-container` after `viewport-touch`. Their look is built from primitives (`Rectangle`, `Text`, `Path`, `Image` of SVG icons, `TouchArea`), not std-widgets, so it is the same on every platform apart from the system font, and it is a dark translucent look whatever the sidebar's light or dark palette is, because it sits on black space.

### 2. The camera panel

Layout and behavior are the mockup's, top right, 12 px from the edges, 92 px wide:

| Part | Controls | Gesture it names | Keys |
|---|---|---|---|
| Compass ring | Drag around the ring to roll; click N for north up (tilt 0) | Left and right buttons held | Q, E; N |
| Orbit joystick inside the ring | Push and hold to travel; four arrows step 10° | Left drag | Arrows |
| Zoom | + and − (click steps 0.05, hold runs), vertical slider with a mark at the default that snaps within 0.03 | Wheel | + and − |
| Look joystick | Push and hold; ↺ while yaw or pitch differs from its default | Middle drag | Shift + arrows |
| Frame joystick | Push and hold; ↺ while an offset differs from its default | Right drag | Ctrl + arrows (⌘ on macOS) |
| Footer | Shortcut sheet, hide overlays, centered | | F1 (⌘? on macOS), H |

- The joysticks are rate controls: the knob's deflection, squared, sets a speed, and the knob springs back on release. All three move the camera the way they are pushed, so the picture slides the other way. The drags on the globe stay as they are: the left and the right drag both grab the picture, which follows the hand.
- In the Sun-fixed mode the orbit joystick, its arrows and the arrow keys move the time of day instead of the longitude, through `held_longitude` and `solar_time_of_held` as the globe drag does, so they keep their speed, their tilt correction and their wrap, and a 10° step is 40 minutes.
- The N, the ticks and the ring turn with the tilt by computed positions (`x: c + r * sin(a)`), never by `transform-rotation` (Slint findings above).
- No lens control. The field of view looks like zoom on the globe and goes wrong in ways zoom does not; it stays in the sidebar's Framing group, and there is no Ctrl + wheel gesture for it.
- The ↺ badges reuse `ui/icons/reset.svg` and compare against `root.defaults`, as the sidebar's do since [2026-10-14-setting-resets-plan.md](2026-10-14-setting-resets-plan.md). N, the ↺ badges and the orbit arrows ease to their target over 300 ms rather than jumping, so the globe visibly turns back.
- Idle: while the pointer is outside the panel and no control of it is held, and 0.9 s after the last use, the panel's fill, border and shadow go and every control drops to 30% opacity with a faint dark drop shadow for contrast over a bright globe. Hovering or using the panel brings it back over 0.4 s.
- The sidebar's camera groups stay as they are. The panel writes the same three window properties, so the two stay in step with no further code.

### 3. Teaching the gestures

- Every control's hover text has a title, one sentence, a mouse glyph with the button highlighted and the gesture's name, and the keys as key caps (mockup). It is an element in the overlay layer, shown after 450 ms of hover, placed left of the camera panel and above the time bar, and it uses the platform's names: Ctrl or ⌘, Shift or ⇧.
- Reverse highlighting: while a gesture is in progress on the globe, the control that does the same lights up at full strength with an accent ring, even while the panel is idle. `image-container` already has `left-held`, `right-held` and `middle-held`; the wheel sets a flag a 350 ms timer clears, and held keys count the same way.
- First use: the first time in a session that an overlay control is used whose mouse gesture has not been used in the session, a one-line tip at the top center names the gesture for 2.8 s. Nothing is persisted.
- The shortcut sheet (the mockup's F1 sheet) lists the mouse gestures and the keys in two columns, in the platform's names. F1 or Ctrl + ? (⌘? on macOS, where the function keys need Fn) opens it, Esc or a click outside closes it.

### 4. Keyboard shortcuts

A `FocusScope` around the preview holds every shortcut; a click on the globe gives it focus, and it takes focus when the window opens. Discrete keys are `KeyBinding`s; held keys (arrows, + and −, Q and E) are tracked through `key-pressed` and `key-released` and applied by a `Timer` while any is held, through the same callbacks as the joysticks.

| Keys | Action |
|---|---|
| Arrows; Shift + arrows; Ctrl + arrows | Orbit; look; move the frame |
| + and −; Q and E; N | Zoom; roll; north up |
| Space; K | Play or pause; pause |
| J and L | Shuttle: from pause start in their direction, then faster along it; against it slower, then stop |
| R | Same speed, other direction |
| , and . (Shift: bigger) | Skip back and forward: 15 min or 1 h, 1 day or 1 week when play runs the date |
| 0 | Back to now |
| H | Hide or show the overlays |
| F1, Ctrl + ? | The shortcut sheet |

The keys avoid `[`, `]` and Home, which need AltGr on many layouts or do not exist on compact keyboards. A slider in the sidebar that has focus keeps the arrows for itself, as it does today.

### 5. The time setting: live or one instant, shown in local time

`DateTimeInput` becomes `SceneTime`, `Live` or `Fixed(OffsetDateTime)` in UTC, and `SceneParams::time` replaces `SceneParams::datetime`. It is the time setting: the wallpaper and every export draw at it, the config saves it, and both the sidebar's Date / Time group and the time bar's tracks, exact entry and Now edit it. It stays outside `ParamsDigest`. `CivilTime::of` and `time_for_input` take the instant directly, with its sub-second part kept, and any instant from 1900 to 2100 is accepted, the edges clamping.

The window shows local time by default and UTC on request, from `UtcOffset::local_offset_at` with the `local-offset` feature, so each instant gets the offset in force at that instant, summer time included. The zone is shown as an offset, "UTC+2" or "UTC", because the system gives no names without a time zone database; the mockup's "MESZ" is the browser's. The displayed zone is a window setting, not part of the time.

The config keys `use_custom_datetime`, `custom_hour`, `custom_day_of_year` and `custom_year` become `scene_time`, `"live"` or an RFC 3339 UTC timestamp, in a `v2_to_v3` migration step that computes the timestamp exactly from the old three numbers and removes them. Three window settings come with the overlays: `time_zone = "local" | "utc"`, `time_bar_pinned` and `overlays_hidden`.

### 6. Playback is an excursion of the preview

The preview can leave the time setting for a while and come back: an excursion, owned by the engine as it owns every schedule, so its `Clock` drives it, a test runs a week of it in milliseconds, and nothing keeps pushing from the window.

- `EngineCommand::Excursion(ExcursionCommand)`: `Play { rate, kind }` starts an excursion at the instant the preview shows, or retunes a running one there, anchored on `clock.elapsed()`; `Pause` freezes it at the instant reached; `Skip(TimeStep)` moves it, running or paused, and starts a paused one at the time setting when there is none; `End` drops it. `kind` is `Clock`, the rate in multiples of real time, or `Date`, the rate in days a second with the time of day kept by whole 24 hour steps.
- The preview draws at the excursion's instant while there is one, running or paused, and at `SceneParams::time` otherwise. Every export, the unattended refresh and Set as Wallpaper included, draws at `SceneParams::time`. Playback never changes the time setting and never reaches a wallpaper.
- The transport (play, pause, the rulers, skip, Space, J, K, L, R and the skip keys) acts on the excursion alone. The tracks, exact entry, Now and the sidebar act on the time setting. A drag on a track starts from the instant on screen and ends the excursion, so the handle does not jump. The engine ends the excursion whenever an `UpdateParams` brings a different `SceneParams::time`; Now, which may write the `Live` already in force, also sends `End`.
- The engine reports the excursion's state, `EngineEvent::Excursion(Option<ExcursionState>)` with running or paused, the rate and the kind, whenever it changes, and `PreviewFrame` gains `instant`, the time the frame shows. The bar's clock shows the frame's time, also when a software adapter is several hundred milliseconds behind; while live with no excursion the window ticks its own label once a second instead, since the engine draws every two minutes.
- Closing the window pauses a running excursion: `SetPreviewEnabled(false)` pauses it in the engine, and reopening shows it paused where it was. The excursion is not saved; a restart starts from the time setting.
- While an excursion runs and the preview is enabled, every tick is dirty and the loop waits at most 33 ms instead of `TICK`, about 30 frames a second where the adapter keeps up; the time comes from the clock, so a slow adapter drops frames and keeps the speed. The latest-value preview mailbox already prevents a backlog.
- Rates are the mockup's: Clock 1, 5, 10, 30, 60, 120, 300, 600, 1800, 3600, 7200 and 21600 times real time (real time to 6 h/s); Date 1, 2, 7, 14, 30, 60 and 90 days a second (up to 3 months/s). They live in `sunlit-core` beside the excursion math, which the window reads for its rulers. The rate and the kind start at 60× and 1 week/s each session and are not saved. Playback is not a setting: the sidebar carries no playback control, and a wallpaper never runs at a warped time.
- The drag observer becomes a motion observer: the Sun's direction moving counts like the camera moving, so Blend mode wants the coarse set while an excursion runs and the full set once it stops. While an excursion runs it watches the camera resolved under the excursion's sky, so in the Sun-fixed mode the read-ahead follows the turn; otherwise it watches `held_camera()` as today.
- In the Sun-fixed mode the preview's camera is resolved under the excursion's sky like everything else drawn at its instant: a Clock run turns the globe once per apparent solar day of the run (at 6 h/s a turn every 4 s), and a Date run keeps the side in view while the terminator slides north and south with the seasons. `sky_interval` treats an excursion like a fixed time: while one runs the 33 ms cadence applies, while one is paused the preview does not move.
- Switching the camera mode and applying a preset keep converting at the time setting's instant (`sky_now` on `SceneParams::time`), since the result is saved and drawn on the wallpaper. During an excursion the preview can therefore show the converted place away from the center until the excursion ends.

### 7. The time bar

Layout and behavior are the mockup's, bottom center, 14 px from the edge, 800 px wide when open:

- Folded: a pill with the state, the date and time on screen, and the zone. The state is ● LIVE, the offset from now for a fixed time, ▶ or ◀ with the speed for a running excursion, or Preview for a paused one. Hover opens the bar, leaving folds it after 0.7 s unless pinned, a popup is open or a track is held; while the pointer is outside the preview it fades to 42%, unless pinned.
- First row: the speed on the left (the rate and what it means, "300× · 5 min/s" or "1 week/s · same time each day"; a click opens every speed in both directions), then skip back, the backward ruler, a 44 px play/pause, the forward ruler and skip forward, centered over the tracks. A ruler has one dot per rate, growing with the rate, the slowest next to play; a click or a drag along it runs at that rate.
- The clock block right of the three rows, behind a divider, 142 px, Live on top: a state pill with the offset from now beside it, the time on screen in large type with seconds while live and the zone chip right after it (a click switches UTC and local), and the date with a calendar icon. The state pill reads "● Live" for live time, "Now" for a fixed time (a click goes live), and "Preview" during an excursion, whose hover text names the time the wallpaper keeps and whose click ends the excursion. A click on the time or the date opens `DatePickerPopup` and `TimePickerPopup` for exact entry, which sets the time setting. The year changes through exact entry, the sidebar and Date playback; there is no stepper in the bar.
- Second and third rows: Time and Date tracks behind radio labels choosing what play runs, the active one in the accent color and its dot a ▶ while running. The time track spans the displayed zone's day and is shaded from the Sun's altitude at the point under the camera, sampled every 30 minutes into a 48 × 1 image the window stretches; the date track spans the displayed year with month ticks and the four season instants from `Astronomy_Seasons`, and the handle snaps to a season's day within three days. A drag on either sets the time setting (decision 6). In the Sun-fixed mode the light under the camera does not change with the hour, so the time track is drawn plain, and its hover text says that the hour turns the Earth beneath the camera.
- Pin in the top right corner: keeps the bar open and at 100% wherever the pointer is.

### 8. The sidebar's Date / Time group stays

Everything that changes the wallpaper stays configurable in the sidebar, so the group stays and moves to the new setting: "Override date/time" switches between `Live` and `Fixed`, turning it on fixes the current instant as today, and below it Time and Day sliders and a Year `SpinBox` from 1900 to 2100 edit the instant. The sliders read in the displayed zone and their labels name it ("16:23 UTC+2", "Sun 18 Oct"). The group's ↺ turns the override off as today, and Time and Day keep `resettable: false`. The override's and the Time row's hints keep what the Sun-fixed camera added to them: with the override on, the time decides which side of the Earth a camera fixed toward the Sun looks at. It shows the time setting, not the preview: during an excursion it keeps showing the time the wallpaper uses. An edit here ends an excursion like any change of the setting.

### 9. Not chosen

- A mode dial for the camera panel (mockup variant B of the first round): one pad and one slider whose meaning changes with a tab hides two thirds of the controls behind a click.
- A panel that expands on hover: it moved under the pointer and hid what the user had just learned to look for.
- Playback driven by a UI timer pushing `SceneParams` 30 times a second: the wrong owner, untestable on the injected clock, and running while the window is hidden.
- Pausing commits the moment on screen as the time setting (the first draft of this plan): closing the window in the middle of a run would have turned a live wallpaper into a fixed one.
- A time bar that moves the preview alone: the preview would no longer show what the wallpaper gets.
- Moving the time setting out of `SceneParams` into its own command: an in-flight `UpdateParams` could undo a time the engine had set, and the golden and engine tests pin their dates through `SceneParams`. The setting stays there with one writer, the window; the excursion stays in the engine with one writer, the engine.
- Speed buttons, a shuttle pair or speed presets beside play (mockup rounds three and four): the rulers show every speed and its direction at once.
- Three other places for the clock block and the speed readout (mockup rounds five and six).

## Success Criteria

1. The camera panel shows in the top right corner whenever the overlays are not hidden, idle until hovered or used, and every control does what decision 2 says, the sidebar following.
2. A drag with each mouse button, the wheel, and each held key lights up the matching control while it lasts.
3. The panel draws correctly on the software renderer, the N and the ticks turning with the tilt, checked with `--software-rendering` and in the Windows guest.
4. Every shortcut in decision 4 works on Windows and Linux, and the sheet lists them in the platform's names.
5. The time bar folds, opens and pins as decision 7 says; play, the rulers, skip, J K L R, the radio labels, the tracks, the zone chip, exact entry, Now and the Preview pill all do what they say.
6. During an excursion the preview moves at the chosen speed measured against the wall clock, the clock block shows the frame's time and Preview, and the sidebar, an unattended refresh and Set as Wallpaper all keep the time setting.
7. Closing the window during an excursion pauses it, reopening shows it paused, and the time setting is unchanged.
8. In the Sun-fixed mode the orbit controls move the time of day and leave the longitude alone, and a Clock excursion turns the globe at the chosen speed with the tiles following.
9. The sidebar's Date / Time group and the time bar's tracks show and edit the same time setting, in the displayed zone.
10. A config written by 0.4.0 with an override loads to the same instant, and one without loads to live time.
11. Nothing of either control appears in an export, a wallpaper or a golden image.
12. `cargo test`, `cargo clippy --all-targets` and `cargo fmt --check` pass.

## Implementation Steps

Three changes: A, the camera panel, depends on nothing else here; B, the time setting and the excursion in the engine, stands on its own with the sidebar moved to the new setting; C, the time bar, needs B.

### Part A: The camera panel, the gestures and the keys

#### Step A1: Camera actions in Rust

A pure `camera_controls.rs` beside `mouse_math.rs`: the joystick rates (orbit 70°/s at distance 8, scaled by distance and capped at 120°/s, tilt corrected the way `apply_globe_drag_at` is; look 45°/s; frame 1.4/s), the orbit step, zoom steps and rates (0.05 a click, 0.3/s held), roll by angle, the 300 ms ease, and the clamps and wraps the sliders use. The orbit takes the held longitude in and gives it back, so the Sun-fixed mode goes through `held_longitude` and `solar_time_of_held` at the one call site, as the globe drag does. Elapsed time between joystick ticks is measured with `Instant` in Rust per stick, so a late Slint timer does not slow the camera.

#### Step A2: Overlay widgets

In `ui/overlays/common.slint`: `Joystick` (pad, knob, `Timer`, `held` and the deflection out), `HoldButton`, `RollRing`, `VZoomSlider` with the default mark, `ResetBadge`, `MouseGlyph` over five SVGs (`ui/icons/mouse-left.svg`, `-middle`, `-right`, `-both`, `-wheel`), `KeyCap`, and `OverlayHint` with its delay.

#### Step A3: The panel

`ui/overlays/camera-panel.slint`, placed in `image-container`. Callbacks `camera-stick(CameraStick, float, float)`, `camera-stick-released(CameraStick)`, `camera-orbit-step(int, int)`, `camera-roll(float)`, `camera-zoom-step(int)`, `camera-zoom-rate(int)` and `camera-reset(CameraReset)` registered in `ui_callbacks.rs` like the mouse callbacks; the zoom slider writes `camera-position.zoom` through a two-way binding and snaps in its `changed`. The idle state, the reverse highlight from `left-held`, `right-held`, `middle-held`, the wheel flag and the held keys, and the first-use tip.

#### Step A4: Keys and the sheet

The `FocusScope` and its bindings of decision 4 (the time keys arrive with Part C), the held-key timer, an `is-macos` property set from Rust for the labels, and `ui/overlays/shortcut-sheet.slint`. H hides the overlays and leaves a small button in the corner to bring them back; `overlays_hidden` is saved with the window's other settings.

### Part B: The time setting and the excursion

#### Step B1: `SceneTime`

`SceneTime` in `scene/` replacing `DateTimeInput`, `SceneParams::time`, `CivilTime::of` and `time_for_input` on the instant, `month_in_force` and `month_ahead` likewise; `sky_interval` and the window's `sky_now` read `SceneTime`. The `v2_to_v3` migration with its log entries, and `AppConfig`'s `scene_time` with serde as the string forms of decision 5, `time_zone`, `time_bar_pinned` and `overlays_hidden`. The tests that pin a date change mechanically to `SceneTime::Fixed(datetime!(...))`, and `e2e_config.toml` to `scene_time`. The `local-offset` feature on `time`.

#### Step B2: The sidebar group on the new setting

Decision 8: `DateTimeSettings` becomes the switch and the instant in the displayed zone, the Year combo box a `SpinBox`, and the label helpers in `scene/datetime.rs` take a zone. The conversions between the sliders and the instant are pure functions in `ui_callbacks.rs`, round trips tested across a leap year and both summer time changes.

#### Step B3: The excursion in the engine

A pure `engine/excursion.rs`: the commands' state machine, the rate ladders, the instant at an elapsed time for both kinds, clamped to 1900 to 2100 with a running excursion pausing at the edge. The engine handles `EngineCommand::Excursion`, ends the excursion on a changed `SceneParams::time`, draws the preview at the excursion's instant and exports at `SceneParams::time`, pauses on `SetPreviewEnabled(false)`, shortens the wait to 33 ms while running, emits `EngineEvent::Excursion`, and puts `instant` into `PreviewFrame`. `sync_month` and `want_tiles` take the instant being drawn, `tile_output` resolves the camera under the excursion's sky for the preview, `sky_interval` counts an excursion as a fixed time, and `DragWatch` watches the Sun's direction as well as the camera, the resolved camera while an excursion runs (decision 6).

#### Step B4: Astronomy helpers

`scene::sky::sun_direction_at(OffsetDateTime)`, the Sun alone, for the track shading, and `scene::seasons(year) -> [OffsetDateTime; 4]` through `Astronomy_Seasons`, an FFI call with its scoped `#[allow(unsafe_code)]` and `// SAFETY:` comment like the others in `scene/`.

### Part C: The time bar

#### Step C1: The bridge

A `time_bar.rs` in `sunlit-app` that mirrors the excursion from `EngineEvent::Excursion` and the last frame's `instant`, turns the transport's callbacks into `EngineCommand::Excursion` and the tracks', exact entry's and Now's into `SceneParams` pushes as decision 6 says, formats the texts (time, seconds, zone, date, offset, state, speed and its meaning) as pure functions, computes the shading image when the displayed day or the point under the camera moves by a degree, and a plain one in the Sun-fixed mode, and runs the one second label timer while live with no excursion. `engine_client.rs` hands each frame's `instant` and each excursion event to it.

#### Step C2: The bar

`ui/overlays/time-bar.slint`: the pill, the bar, the speed readout, the rulers over a model of the ladder, the clock block with its three states, the radio labels, the two tracks with their drags, hover times and season diamonds, the pin, the zone chip, and the two std-widgets pickers. The time keys of decision 4 join the `FocusScope`.

### Step D: Documentation

- `docs/architecture.md`: the overlay layer and its files; the camera panel, the reverse highlight and the keyboard scope; `SceneTime` as the time setting, the excursion commands and event, which frames draw at which instant, the 33 ms cadence and the motion observer; the reworked Date / Time group; the new config keys and the migration.
- `docs/rendering.md`: nothing, the renderer does not change.
- `docs/testing.md`: the excursion tests on the mock clock and the overlay tests' use of `mock_elapsed_time` and dispatched key events.
- `CLAUDE.md`: the architecture lines that name `datetime` say `time`, and the list of things with their own `EngineCommand` gains the excursion.
- `docs/roadmap.md`: "Time playback" checked off with a link to this plan, and an entry for the camera panel and the keyboard shortcuts.
- README, with the user's permission only: the mouse list gains the keyboard sheet and the camera panel, and the time bar gets a sentence.

### Step E: Validation with the user

The user tries each part in the app. Part A also in the Windows guest (`cargo xtask vm up windows`, software renderer) for criterion 3 and in the Linux guest under KDE and GNOME on Wayland for the keyboard focus. Part C with excursions at 3600× and at 3 months/s in both camera modes, timed against a wall clock, on this machine's GPU and with `--software-rendering`, and with the tile report (`ReportTiles`) taken before, during and after a run in Blend mode. macOS is exercised only on CI's runners; the ⌘ bindings rest on Slint's documented mapping.

## Tests

- `camera_controls.rs` unit tests: a full deflection for one second turns the globe by the stated rate at distance 8; the tilt correction makes "up" screen up at any roll; clamps and wraps; the ease reaches its target exactly; the zoom snap; in the Sun-fixed mode an orbit step east is a later time of day and wraps across midnight.
- `excursion.rs` unit tests: Clock and Date instants for positive and negative rates, Date keeping the UTC time of day, a retune keeping the instant continuous, Pause freezing it, Skip with and without an excursion, the 1900 and 2100 edges, every ladder rate distinct and sorted.
- Config migration tests: an override becomes the exact instant of its year, day and hour; no override becomes `"live"`; the four keys are gone and logged; `v0.2.2.toml` still loads.
- Engine tests on `MockClock`, a new `tests/engine/excursion.rs`: a run at 3600× for one mock second moves the Sun 15° in the preview frames and `PreviewFrame::instant` matches; a wallpaper published during the run is drawn at `SceneParams::time`; a changed `SceneParams::time` ends the excursion and an unchanged one, as every camera drag sends, does not; `SetPreviewEnabled(false)` pauses it and re-enabling shows the paused instant; a Date run across the middle of a month changes the month in force; the motion observer reports motion while the Sun moves and settles after it stops; a Sun-fixed Clock run turns the resolved camera at the run's rate, the wanted tiles follow it and the drag's rate points the way the camera moves; `sky_interval` gives `SKY_INTERVAL` for a fixed time and during an excursion.
- Sidebar conversion tests: slider values to the instant and back in local time and UTC, across a leap day and both summer time changes.
- Formatting tests in `time_bar.rs`: offsets ("now", "+5 min", "−2 d 3 h", years), zone labels for whole and half hour offsets, speed texts for every rate, the three state texts.
- `slint_ui.rs`: N after a tilt brings it back to 0 once `mock_elapsed_time` has passed the ease; a held orbit joystick moves the longitude monotonically, and in the Sun-fixed mode the time of day with the longitude untouched; the look and frame ↺ appear exactly while their values differ from `defaults` and clear them; the zoom slider snaps to the mark; F1 opens the sheet and Esc closes it; H hides and shows the overlays; a click on a forward ruler dot sends that rate and leaves the time setting alone; the radio labels switch the rulers' ladder; a track drag sets the time setting; Now sends `Live` and `End`; the Preview pill ends the excursion; the pin keeps the bar open with the pointer outside; the sidebar group and the bar show the same instant. Dispatched pointer and key events, as the existing cases do.
- The golden and render tests change only in how they pin their date, and their references stay as they are; a changed reference is a defect in Step B1.

## Out of Scope

- A lens control on the preview, and the Ctrl + wheel gesture for it.
- Persisting the playback rate and kind, or the excursion itself.
- Sunrise and sunset times as text, jumps to moon phases or eclipses, and time scrubbing by a gesture on the globe.
- The clouds of the instant shown: the cloud map stays the latest observed one, as it is for a fixed time today.
- A live time with an offset ("always six hours ahead").
- The `render` subcommand's own time flag.
- The planned rework of the sidebar.

## Risks and Mitigations

| Risk | Mitigation |
|---|---|
| Thirty preview readbacks a second are heavy on a big preview: 8 MB a frame at the Medium tier's 1920 px, copied into a Slint image. | The 33 ms wait is a ceiling, not a promise; the mailbox drops what the UI has not taken, and the speed comes from the clock. Step E measures it; if needed the cadence drops to 20 frames a second while running. |
| Blend mode keeps asking for tiles along the moving terminator. | The motion observer of Step B3, and the tile report in Step E. |
| A fast Clock run in the Sun-fixed mode turns the globe up to 90° a second, and the tiles never catch up. | The motion observer watches the resolved camera during a run, so the coarse set is wanted with read-ahead along the turn; the floor is drawn where tiles are missing, and Step E watches the tile report at 6 h/s. |
| A Date run on a CPU adapter outruns the month packs, which load the month in force and the month ahead only. | The floor already in use is drawn until the next lands, which is a visible step at mid month and no stall; Step E watches it. If it stalls, Date playback on a CPU adapter caps at 1 month/s. |
| The preview shows an excursion and someone takes it for the wallpaper's time. | The Preview state in the pill and the clock block, its hover text naming the time the wallpaper keeps, and the sidebar group showing the setting. |
| Something drawn with `transform-rotation` slips in and does not turn on the software renderer. | Decision 2 forbids it, and Step E checks in the Windows guest. |
| The keyboard scope loses focus to the sidebar and the shortcuts seem dead. | A click on the globe takes it back, the scope takes it on open, and the sheet says the shortcuts act on the preview. |
| A time zone change while the app runs is not seen on Linux until restart, since `tzset` is not called in a multi-threaded process. | Accepted; the label is correct again after a restart. |
| Date playback steps 24 hours of UTC, so its local clock jumps an hour at a summer time change. | Accepted and named in the Date label's hover text. |
| The migration or the sidebar's conversion lands an instant an hour or a day off. | Exact tests over a leap year, day 366, both summer time changes and the fractional hours the old slider produced. |
| Joystick timers drift and the camera speed varies with load. | Elapsed time measured in Rust per stick (Step A1). |

## Rollback Strategy

Revert the change. Part A leaves `overlays_hidden` in the config, which an older build ignores. Part B leaves a version 3 file, which an older build loads best effort: it ignores `scene_time` and the window settings, finds none of the four old keys, and starts in live time. A fixed time set after the upgrade is the one thing a downgrade loses.

## Departures

None yet.

## Open items

The user settled the questions of the first draft on 2026-10-18: the drags on the globe stay as they are, the bar has no year stepper, the sidebar keeps its Date / Time group (decision 8), playback never reaches the wallpaper and pauses when the window closes (decision 6), and the bar's edits are the time setting (decisions 5 and 7).

Two proposals from the rebase onto the Sun-fixed camera, confirmed by the user on 2026-10-18:

1. The time track is drawn plain in the Sun-fixed mode (decision 7), since its shading would be one color all day.
2. A mode switch or a preset during an excursion converts at the time setting's instant, not at the instant the preview shows (decision 6), so the wallpaper gets the view the click asked for and the preview may not.

## Validation Record

None yet.
