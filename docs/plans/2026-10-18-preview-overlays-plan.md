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

1. The time between joystick ticks is measured in Slint on the animation clock (`animation-tick()`) rather than with `Instant` in Rust (Step A1), and the callbacks carry it: `camera-stick(CameraStick, float, float, float)` and `camera-zoom-rate(int, float)` end in the seconds since the previous report, and the eases run on a `camera-ease-tick(float)` timer that Rust starts through a `camera-easing` property. The guarantee is the same, a late timer moves the camera further rather than slower, because the elapsed time is measured and not assumed. What changes is the clock: the animation clock is the system clock in the app and the mock clock under the testing backend, so `mock_elapsed_time` drives the sticks and the 300 ms ease in `slint_ui.rs` exactly, which an `Instant` would not. With nothing to measure in Rust, `camera-stick-released` has no work and is left out; a stick, the roll ring or a held zoom button cancels any ease on the values it moves instead.

2. An idle control drops to 30% opacity without the faint dark drop shadow decision 2 asks for. Slint draws a drop shadow for a rectangle's shape only, not for a glyph, an SVG icon or a path, and the controls that most need contrast over a bright globe (the N, the ticks, the knobs' rims) are those. The fade alone is what the panel does when idle; Step E will show whether it needs more over a bright day side.

3. H, which toggles, is read in the `FocusScope`'s `key-pressed` rather than declared as a `KeyBinding` (decision 4). A `KeyBinding` fires on every auto-repeat and does not see the event, so a held H would flicker the overlays; `key-pressed` sees `repeat` and toggles once per press. N, F1, Ctrl + ? and Esc stay `KeyBinding`s, since repeating them does no harm.

4. The `time` crate gets three features besides `local-offset`: `formatting` and `parsing` for the RFC 3339 spelling of `scene_time` in the config, and `macros` for `datetime!`, which the tests that pin a date are written in (Step B1) and which gives `SceneTime`'s range its two constants. None of the three adds a crate (`time-macros` and `itoa` were in `Cargo.lock` already). `local-offset` does add one, `num_threads`, which `time` uses on some Unix targets, so `cargo xtask bake licenses` was rerun and its two outputs committed with Step B1. The migration step also keeps a `scene_time` a file already names when it holds no override, which only a file written for version 3 by hand does; without that, the unstamped `e2e_config.toml`, which now names its instant directly, would have been migrated to live time.

5. The seasons are `scene::sky::seasons(year) -> Option<[OffsetDateTime; 4]>` rather than `scene::seasons(year) -> [OffsetDateTime; 4]` (Step B4). They sit in `sky` beside `sun_direction_at`, where the Astronomy Engine calls and the conversion of its times already are, and the `Option` carries the status `Astronomy_Seasons` returns, which a caller drawing the date track can answer by leaving the diamonds out instead of the crate panicking.

6. The skip keys are read from the text Slint delivers rather than from the key (decision 4). Slint's key events carry the text a key gives with the modifiers applied and no physical key, and Shift + comma gives "<" on a US or UK layout and ";" on a German one, so "Shift: bigger" cannot be written as Shift plus the comma key. The keyboard handler takes "," and "." alone for the small steps, and with Shift held "<" and ">", ";" and ":", or "," and "." themselves for the big ones, which covers the US, UK, German and most other Latin layouts. A layout whose comma or period key gives something else under Shift gets the big step from Shift + a click on the skip buttons, which works everywhere. The cost is that Shift + the semicolon key on a US layout, which gives ":", also skips forward by the big step.

7. Outside an excursion the bar shows the time setting itself rather than the last frame's instant (decision 6 says the bar's clock shows the frame's time). The two are the same once the engine has drawn, and the setting is there at once, so a drag on a track moves its handle with the pointer instead of a frame behind it. During an excursion the bar shows the frame's instant as planned, and a track press or exact entry during one takes the rest of the instant from the frame on screen, sets the setting, sends `End` and shows the setting from then on without waiting for the engine's report.

8. While a wallpaper or an export waits for its tiles, the month in force follows the time setting, not the instant drawn (Step B3 says `sync_month` takes the instant being drawn). The wanted set and the page table have one month, and the wallpaper's tiles are the setting's month's, so with an excursion in another month every preview draw put the excursion's month in force and every owed publish the setting's again. Each change counted as a renewal of the tile wait, so a deferred wallpaper waited until its tiles landed rather than `TILE_WAIT` at the most, the publish's tile checks judged whichever month the last draw left in force, and the engine redrew the preview every tick, shown or hidden, to flip the month back. `Engine::month_instant` now gives the setting's instant to `sync_month` in every draw while `wallpaper_shots` or `exports` is not empty, which is from the plan of a publish until it went out with every tile, and an export's wait at most. Meanwhile a preview of an excursion in another month is drawn at its own instant from the setting's month's floor and tiles, which shows as the floor holding still for a few seconds of a Date run or of a paused excursion's preview; the month comes back with the first draw after the wallpaper is made. Keeping two months in the set at once would avoid that, at the cost of a second month in the tile array, the page table and the readiness checks, for a state that lasts `TILE_WAIT` at the most outside the made again case.

9. While a wallpaper or an export waits for its tiles, the Sun by which Blend mode's wanted set chooses day or night tiles is the time setting's, not the preview's, for the reason of departure 8: the set has one Sun as it has one month. With the preview's Sun, a wallpaper made while the preview shows an excursion twelve hours away was wanted and judged with the night where its own day is lit, found nothing missing, and went out with its lit side from the floor, never to be made again; a Date excursion's change of declination did the same along the terminator, and an export likewise. `Engine::want_tiles` now takes the setting's Sun for the set's surfaces while `wallpaper_shots` or `exports` is not empty, in every path that computes the set, a draw of the preview or of an export, `want_now` and the made again case, and the preview's Sun otherwise. The motion observer still watches the preview's camera and Sun, so an export's draw is not a move. Meanwhile the preview of such an excursion is drawn under its own Sun from tiles chosen by the setting's, which shows as floor along the part of its day the setting's night covers and the part of its night the setting's day covers, for `TILE_WAIT` at the most outside the made again case. Giving each output its own surfaces would avoid that, at the cost of a per output Sun through the residency's request, its ranking and `missing_for`.

10. The transport row of decision 7 keeps its full size only where it fits centered over the tracks clear of the speed readout; the user chose compact rulers for the rest (Step E). The full row with the time's twelve speeds is 400 px, and centered over the 352 px of track a bar gets in the app's default 1000 px window it ran over the readout and the clock column. The readout in words starts at the left column, above the radio labels, rather than at the tracks' column as in the mockup: with the mockup's placement and play centered, the full row leaves the readout 70 px with the time's speeds and 125 px with the date's, and the widest readouts take 88 and 123 px in the app's font (in the mockup's own browser rendering 81 and 112, so the mockup ran 1800× to 21600× over skip back by up to 12 px). From the left column the full 800 px bar leaves 40 px between the widest time readout and skip back and 60 px with the date's. Where the full row would reach the widest readout of the ladder's speeds, which the bar measures out of sight from the tracks' width and the readout's text rather than from a window size or the speed in force, the readout folds to its arrow, a 28 px button in the left column that still opens the list of every speed and whose hint names the speed ("Speed: 60× · 1 min/s"), and the rulers pack their dots closer and smaller, from 11 px apart down to 7, so the row fits the tracks' column with play over its middle; a press maps to the dot under it at whatever pitch. The readout folds below a bar of about 732 px with the time's twelve speeds and 692 px with the date's seven, a window of 1120 and 1080 px at the default panel, and a change of speed alone never folds or unfolds it; between 660 and 732 px with the time's speeds and between 550 and 692 px with the date's the dots keep their full 11 px, since only the readout needs the room there. The narrowest bar that holds the compact row is 564 px with the time's twelve speeds and 494 px with the date's seven, which at the default 360 px panel is a window of 952 and 882 px. Below that the row used to spill past the tracks' column on both sides, and in the narrowest windows the bar's fixed parts (the left column, the 142 px clock column and the margins, 260 px) left the tracks no room at all: at the app's 520 px minimum window the bar got about 212 px, and parts of it overlapped and reached into the sidebar. The user chose in Step E that the open bar never gets narrower than 564 px, for both kinds of play: the narrowest bar that holds the compact row of the longest ladder, the time's twelve speeds, derived in the bar from the fixed columns, the transport's fixed parts and the ladder's length at 7 px, which Rust sets once as `time-longest-ladder`, so a switch between Time and Date never changes the width; with the date's seven speeds the dots keep their full 11 px there. In a preview narrower than that, below a window of 952 px at the default panel, the bar keeps that width and that layout, centered on the preview, and `image-container` clips it and everything else drawn over the preview but the date and time pickers, so both ends are cut equally and nothing else reaches into the sidebar or takes a press there. The pickers are popups, which no clip reaches, and while one is open the pointer reaches nothing else. Each opens above the bar inside the preview, 6 px from its edges, where it fits; where it does not, the date picker below a window of about 710 px at the default panel and the time picker below about 640, it stands against the window's right edge and covers the sidebar by the width it has over the preview's, the least a popup can cover, since Slint keeps a popup inside the window. At the 520 px minimum window the preview is 236 px and shows the tracks' column with play and a sliver of the clock column. Play stands over the tracks' middle, which is 60 px left of the bar's because the left column is narrower than the clock column, so in a cut bar it stands 60 px left of the preview's middle, as it does in every wider window. Slint 1.18 blurs only shadows, not content, so the cut edges fade instead: where the bar, open or folded, is wider than the preview, a 28 px strip at each edge of the preview over the bar's height runs from transparent to the bar's background at 90%, at the bar's own opacity. The folded pill is centered and cut the same way. A hover hint is never wider than the preview less 6 px each side, and the camera panel's hint stays 6 px inside the preview's left edge, over the panel in the narrowest windows, rather than running out of it. The first-use tip is never wider than the preview less 6 px each side either, its text wrapping to that width and the tip growing around the lines.

11. The compass ring's N rolls the picture when dragged, besides putting north up on a click (decision 2 rolls by a drag around the ring and puts north up by a click on the N, which made the N the one part of the ring that could not be dragged). The user asked for it in Step E: reaching to turn the view, the hand goes for the N. A press on the N that moves more than 4 px from where it went down, along either axis, rolls from the angle of the press point exactly as a drag on the ring from that point does, through the same `rolled` callback, and holds the ring as a ring drag does, so the panel stays awake, the ring's border lights and the roll's first-use tip can show; a release after less is a click and puts north up, so a press that trembles by a pixel or two still clicks. The overlays had no drag threshold, and Slint's own is not reachable from `.slint` code, so the 4 px is `RollRing`'s `drag-threshold`. The N's hint names both.

## Open items

The user settled the questions of the first draft on 2026-10-18: the drags on the globe stay as they are, the bar has no year stepper, the sidebar keeps its Date / Time group (decision 8), playback never reaches the wallpaper and pauses when the window closes (decision 6), and the bar's edits are the time setting (decisions 5 and 7).

Two proposals from the rebase onto the Sun-fixed camera, confirmed by the user on 2026-10-18:

1. The time track is drawn plain in the Sun-fixed mode (decision 7), since its shading would be one color all day.
2. A mode switch or a preset during an excursion converts at the time setting's instant, not at the instant the preview shows (decision 6), so the wallpaper gets the view the click asked for and the preview may not.

## Validation Record

### Round 1

Validator round 1 ran on 2aa2eca..88f07ec in two halves: Part A found no major and 6 minor findings; Parts B to D found 1 major and 3 minor, with the gates green there (core lib 680, bc7 4, engine 135, golden 25, render_pipeline 51, shading 7, soak 1, app lib 115, slint_ui 78, xtask 693). The fixes of M1, m1, m2, m4 and the first half of m8 each have a case that failed with the fix taken out; m3, m5, m6 and m7 changed only comments and docs, and m8's second half, the export draw's sky, had no case.

- Major, an excursion in another month broke a deferred wallpaper's `TILE_WAIT` bound: the preview's draws and the owed publish put their months in force in turn, each change renewing the wait, and the publish's tile checks judged whichever month the last draw left. Fixed in 9bd589a, departure 8, with two cases in `tests/engine/readiness.rs`, the preview shown and hidden.
- Minor m1, a running 300 ms ease overwrote every writer of its values but the sticks, the roll ring and the held zoom button, worst after the Sun-fixed switch. Fixed in 656efe6: an ease lets go of a field that no longer holds what its last tick left there, and of the held longitude when the switch changes what it means; slint_ui cases for the switch and a globe drag.
- Minor m2, the first-use tip counted the keys and a bare click on the globe as uses of a mouse gesture. Fixed in 656efe6: a drag on the globe counts once it has moved, and the wheel; a slint_ui case.
- Minor m3, three comments gave the roll's sign as clockwise. Fixed in e423f2e; the code was right.
- Minor m4, the sheet the panel's ? button opened did not close on Esc while a sidebar control had the keyboard. Fixed in 656efe6: the button focuses the preview's `FocusScope`; a slint_ui case.
- Minor m5, `docs/usage.md` said the keys work once the globe has been clicked. Fixed in e423f2e.
- Minor m6, `TimeZoneChoice` sat between `QualityTier` and its doc comment. Fixed in e423f2e.
- Minor m7, the custom date's reasoning in `scene/month.rs` and in two docs. Fixed in e423f2e. `PREFETCH_WINDOW` now argues from a fixed time and an excursion: a day holds for the slider and for a Clock run, while a jump, a skip and a Date run can outrun it and draw the new month's floor until its tiles land, which the plan's Risks already accept and Step E watches; the behavior is unchanged.
- Minor m8, the motion observer took a run's pause, where it hands over from the resolved camera and the Sun to the held camera, for a move at thousands of degrees a second, and an export drawn during a run showed it the setting's camera and Sun. Fixed in 07aa5e4: a change of what is watched rebases the watch, and an export's draw computes the wanted set under the preview's sky; a `DragWatch` unit case.
- Declined by the orchestrator: `overlays_hidden`, `time_bar_pinned` and `time_zone` are saved only at the window's existing save points, like every other setting of the window: decision 5 makes them window settings and asks for no save of their own.

### Round 2

Validator round 2 ran on 88f07ec..cba3157 and verified M1 and m1 to m8: m8's rebase of the watch is right, and its export draw's sky regressed into R2-1. It found 1 major and 1 minor, with the gates green at cba3157 (core lib 681, bc7 4, engine 137, golden 25, render_pipeline 51, shading 7, soak 1, app lib 116, slint_ui 82, xtask 693).

- Major R2-1, in Blend mode a wallpaper or an export made during an excursion was wanted and judged with the excursion's Sun for its day and night, so with that Sun far from the setting's it went out at once with its lit side from the floor and was never made again. Fixed in ef1092c, departure 9, with a readiness case in Blend mode that publishes with the preview twelve hours away and again after `End`; the two differed with the fix taken out.
- Minor R2-2, the Round 1 record claimed a failing case for every fix. Corrected in this record.

### Round 3

Validator round 3 ran on cba3157..b26c80b, verified R2-1 (it took the fix out and saw the new readiness case fail) and R2-2, and found no major and 2 minors, with the gates green at b26c80b (core lib 681, bc7 4, engine 138, golden 25, render_pipeline 51, shading 7, soak 1, app lib 116, slint_ui 82, xtask 693). The orchestrator fixed both.

- Minor R3-1, the doc comment on `Engine::draw` said the wanted set is computed under the preview's sky whichever is drawn, which since ef1092c leaves out the setting's month and Sun while outputs wait; departure 9 also named only the preview's day under the setting's night. Both corrected.
- Minor R3-2, a publish owed during an excursion that ends in `report_wallpaper(Err)`, from `check_supported` or `plan_wallpaper` after its wait, cleared the shots without marking the frame dirty, so a paused excursion kept the setting's month and Sun in its wanted set until something else dirtied it. `report_wallpaper` now marks the frame dirty while there is an excursion. No case: the path needs a sink that stops being supported between planning a publish and paying for it.

### Step E, round 1

The user's first look at the overlays found three layout bugs; a sweep of every hover hint in both platforms' key names and of the shortcut sheet with an offscreen render found two more of the same kind. Each fix has a slint_ui case that failed with the fix taken out except the `KeyCap` that no longer narrows below its label, which had none until cc5a9e2, and the gates were green at 554e1bd (core lib 681, bc7 4, engine 138, golden 25, render_pipeline 51, shading 7, soak 1, app lib 116, slint_ui 86, xtask 693).

- The second line of a wrapping gesture ("Drag the globe with the left button") ran over the hint's bottom border, in the orbit, look and frame hints. Slint 1.18 measures the gesture section, made by an `if`, at its own preferred width, so the text was measured on one line. Fixed in 9ced79d: the row places the glyph, the text and the keys by hand and takes the text's height at the width it has.
- Key caps overlapped, Ctrl (⌘) and the arrows of the frame hint: the row squeezed them to 18 px to make room for the text. Fixed in 9ced79d: the keys keep their full width on one line at the right, the gesture text wraps beside them, and a `KeyCap` no longer narrows below its label.
- Play and everything around it stood 200 px right of the tracks' center and the forward ruler ran under the Live pill: the transport row had no width, so its centering spread over the whole bar. Fixed in 2bd7852.
- Found in the sweep: in the help hint (F1, or ⌘ and ? on macOS), which has keys and no gesture, and in the zoom hint the caps stretched across the row. Fixed with the hint's row in 9ced79d.
- Found in the sweep: in a narrow window the shortcut sheet's keyboard descriptions ran past the card's right edge. Fixed in 9a2a270: the rows wrap their text. The columns were not equal, which the validation below found and f271623 fixed.

### Step E, round 1 validation

The validator found 2 majors and 3 minors at 3b91588, with the gates green there (core lib 681, bc7 4, engine 138, golden 25, render_pipeline 51, shading 7, soak 1, app lib 116, slint_ui 86, xtask 693).

- Major M1, at the app's default 1000 by 600 window the transport row, 400 px wide over 352 px of track, ran over the speed readout and onto the clock column; only a bar of about 790 px was clean. The user chose compact rulers. Fixed in 6657638, departure 10: where the full row would reach the readout, the readout folds to its arrow in the left column, its hint naming the speed, and the rulers pack their dots from 11 px apart down to 7. The narrowest bar that holds the compact row is 564 px with the time's speeds (a 952 px window at the default panel) and 494 px with the date's; below it the row spills past the tracks' column, and at the 520 px minimum window the bar's fixed parts leave the tracks no room, which goes to the user. Three slint_ui cases: the transport inside the tracks' column and clear of the readout and the radio labels with play centered at seven window widths for both kinds, each dot of a compact ruler running at its own rate, and the speed in the readout's hint; all three failed on the old layout.
- Major M2, the shortcut sheet's columns were unequal, so the keyboard's descriptions broke inside words and the card ran off an 800 by 700 window. Fixed in f271623: two equal columns where each leaves its text 110 px, one under the other where not, the platform under the title when both do not fit, and the card scrolls when taller than the preview, which only the narrowest windows need. The sheet's case now runs six window sizes on both platforms, checks equal columns, the card inside the preview, no scrolling at 1000 by 600 and 800 by 700, and every description at least as wide as its longest word; it failed on the old sheet.
- Minor m1, no case covered the `KeyCap`'s minimum width. Fixed in cc5a9e2 with a case that squeezes a cap in a 24 px row; it failed with the old 18 px minimum.
- Minor m2, the Step E record claimed equal columns and a failing case for every fix. Corrected above.
- Minor m3, a hint's keys wider than its room ran left past the padding. Fixed in cc5a9e2: keys that would leave the gesture text less than 80 px take a line of their own. No case, since no hint comes near it; a temporary six-key hint rendered offscreen showed the keys on their own line.

### Step E, round 2 validation

The validator found 1 major and 1 minor at 353be24, with the gates green there.

- Major M1, the readout folded from the width of the speed in force rather than the bar's: on a full 800 px bar the readout showed only its arrow for every date speed ("same time each day" needs about 130 px where the row left 125) and for the time's 1× and 120× to 21600×, and L at 60× in a 1400 px window folded it, J unfolded it. Fixed in f66651c, departure 10: the bar folds the readout when the widest readout of the ladder's speeds would reach the transport, and the readout in words starts at the left column, so the full bar names every speed of both kinds in words and the fold sits at a bar of about 732 px with the time's speeds and 692 with the date's; play stays centered over the tracks and the compact row below the fold, its 564 and 494 px floors included, is unchanged. Three slint_ui cases: every speed of both kinds in words and clear of the transport on an 800 px bar, the same fold at every speed from the 952 px window to the widest bar in 32 px steps and a pixel either side of the fold, and the speed's hint following a change of speed under a resting pointer at 1000 and 1400 px; all three failed on the old layout.
- Minor m1, the shortcut sheet's mouse column puts its descriptions 104 px in beside glyphs of 14 to 20 px, copied from the mockup's 96 px minimum width and 8 px gap, which makes needless wraps in a 520 px window. Left open for the user's call.

### Step E, round 3 validation

The validator found no major and no minor at 459500e, with the gates green there. The fold no longer reads the speed in force: a probe over every rate of both ladders in both directions, at windows from 952 to 1400 px and a pixel either side of each threshold, gave one fold state per width, with the readout speaking from a 732 px bar for the time's speeds and 692 for the date's, and the floors still at 564 and 494. The hidden sizers cover every label the readout can show and take no room, no hover and no place in the bar's height. Moving the readout to the left column changes decision 7's placement and waits for the user's look with the rest of Step E, as does the shortcut sheet's 104 px mouse column.

### Step E, round 4 validation

The validator found no major and 2 minors at 6a76a09, with the gates green there; both were fixed in 15f3d39, with the gates green at that commit (core lib 681, bc7 4, engine 138, golden 25, render_pipeline 51, shading 7, soak 1, app lib 116, slint_ui 99, xtask 693).

- Minor m1, the date and time pickers are popups, which `image-container`'s clip does not reach, so in narrow windows they covered part of the sidebar while departure 10 and `docs/architecture.md` said nothing over the preview reaches it. Each picker now opens above the bar inside the preview, 6 px from its edges, where it fits, and where it is wider than the preview it stands against the window's right edge and covers the sidebar by its excess width only; the docs name the pickers as the one exception. Slint 1.18.1 keeps a popup inside the window, which had already moved the old picker, centered on the bar, to the same rectangle, so the new slint_ui case, which opens both pickers at five window widths and proves each one's place with a click on its Cancel button, passes on the old placement too. Offscreen renders at 520, 600, 700, 800 and 1000 px show the date picker over the sidebar at 520 and 600 px by 108 px and at 700 px by 8 px, the time picker at 520 and 600 px by 36 px, and both inside the preview from 800 px, the time picker from 700.
- Minor m2, the first-use tip kept its text on one line and was cut at narrow previews: at a 520 px window the roll tip was 281 px over a 236 px preview. The tip is now never wider than the preview less 6 px each side, and its text wraps, placed by hand so its height is measured at the width it gets. A slint_ui case checks the tip inside and centered on the preview, its parts on the tip and its text on more than one line at a 520 px window; it failed on the old tip.
