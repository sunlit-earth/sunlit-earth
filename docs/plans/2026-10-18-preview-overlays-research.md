# Preview overlays: camera control and time control (research)

Two controls drawn on top of the 3D preview rather than in the sidebar: a camera control in the top right corner that gives direct access to every camera parameter and teaches the mouse gestures, and a time control that shows the scene's date and time, moves it back and forth, and plays it at a multiple of real time. This document collects what exists today, what Slint 1.18.1 can and cannot do for it, design options with a recommendation for each, and the open questions. The design that came out of it was settled on an interactive mockup, [mockups/2026-10-18-preview-overlays/index.html](../mockups/2026-10-18-preview-overlays/index.html), and the plan is [2026-10-18-preview-overlays-plan.md](2026-10-18-preview-overlays-plan.md); where the two disagree with this document, they are the later word.

## What exists today

### Camera

Nine parameters in three Slint structs (`crates/sunlit-app/ui/main.slint`), each its own sidebar group:

| Group | Parameter | Range | Mouse | Reset |
|---|---|---|---|---|
| Camera Position | longitude | -180..180°, wraps | left drag | none (not resettable) |
| | latitude | -89..89° | left drag | |
| | zoom | 0..1, exponential distance | wheel | |
| Camera Orientation | tilt (roll about the view axis) | -180..180°, wraps | left + right drag, horizontal | group and row |
| | yaw | -90..90° | middle drag | group and row |
| | pitch | -90..90° | middle drag | group and row |
| Framing | fov | 10..170° | none | group and row |
| | offset x, offset y | -3..3 | right drag | group and row |

The gestures live in one `TouchArea` (`viewport-touch`) over the preview `Image`; it tracks `left-held`, `right-held` and `middle-held` and calls `mouse-drag-globe`, `mouse-drag-frame`, `mouse-drag-orient`, `mouse-drag-tilt` and `mouse-scroll`. The arithmetic is in `crates/sunlit-app/src/mouse_math.rs`, the registration in `ui_callbacks::register_mouse_callbacks`, and every callback ends in `engine.push_params(&win)`, which the engine coalesces. The left drag blends a fine and a coarse gain by cursor speed (`DragSpeed`), so synthetic pixel deltas fed into it would be misread as a slow or fast hand.

Gaps: the field of view has no gesture, there are no keyboard shortcuts anywhere in the window, and nothing in the UI says that the mouse gestures exist.

### Date and time

`DateTimeSettings { use-custom, hour, day-of-year, year-index }` in Slint, `DateTimeInput { use_custom, custom_hour: f32, custom_day_of_year: u16, custom_year: i32 }` in core (`scene/sun.rs`). The hour is UTC. The year combo covers the current year ±10 (`scene/datetime.rs`). Live mode re-renders on `SKY_INTERVAL`, 2 minutes (`engine/mod.rs`); a custom date freezes the scene. `datetime` is in `SceneParams` but deliberately not in the digest: the engine compares the derived sky state instead. The month in force, and on a CPU adapter the month ahead, is derived from the date (`scene/month.rs`, `Engine::sync_month`): a GPU keeps every month's day floor resident, a CPU adapter keeps two. The floor switches at mid month, so it is a discrete change.

The roadmap already carries "Time playback" (Camera and controls), asked for on 2026-08-29: 30 to 60 times real time is the ballpark for watching a sunrise, since the Sun's disk crosses its own diameter in about two minutes.

## Slint 1.18.1 findings

Read from the vendored sources under `~/.cargo/registry/src/.../i-slint-*-1.18.1`.

- Overlay placement. Children of `image-container` declared after `viewport-touch` are drawn above it and receive pointer events first inside their own bounds; everything outside them still reaches the globe. Because the overlay is Slint and not wgpu, it never appears in an export or a wallpaper, and `SceneParams` is untouched by its existence.
- Rotation does not work everywhere. `transform-rotation` is a reserved property on every element, but the software renderer's `rotate()` is an empty `// TODO (#6068)` (`i-slint-renderer-software-1.18.1/lib.rs:3096`). The app uses femtovg on a normal desktop and the software renderer with `SLINT_BACKEND=winit-software`, which is what the VMs run. Anything that has to turn with the tilt (a north marker, a compass ring) should therefore be positioned with computed `x`/`y` (`r * sin(θ)`, `r * cos(θ)`) or drawn as a `Path` whose coordinates are bindings, never rotated as an element.
- `Path` is available in the software renderer: its `std` feature enables `path`, and `slint`'s default `renderer-software` pulls `std`.
- `Timer` is a built-in element (`running`, `interval`, `triggered`), which is what a press-and-hold joystick or an auto-repeating button needs. The testing backend's `mock_elapsed_time` drives it, and `slint_ui.rs` already uses it.
- `KeyBinding` with the `@keys(...)` literal is new and not experimental: `KeyBinding { keys: @keys(Control + Plus); activated => {...} }` placed inside a `FocusScope`. It matches logical keys.
- `PointerScrollEvent` carries `modifiers`, so Ctrl+wheel and Shift+wheel can be told apart from the plain wheel in the existing `scroll-event` handler.
- `DatePickerPopup` and `TimePickerPopup` exist in std-widgets (`date`/`time` in, `accepted(Date)`/`accepted(Time)` out). They are modal dialogs styled like the rest of the fluent widgets, good for "type an exact date" and too heavy for scrubbing.
- Drop shadows and inner shadows exist; backdrop blur does not. The overlay's panels are a translucent dark fill, as the existing loading pill (`#000000aa`) already is.
- `TooltipArea` is a built-in; the app has its own `Tooltip`/`Hint` pair in `widgets.slint`, which the overlay should reuse for consistency.

## Component 1: the camera control

### Options

A. A stacked control in the style of the classic Google Earth navigator: a compass ring around an orbit pad, below it a look pad, below that a pan pad, and a zoom slider. Everything is visible at once; the column is about 64 px wide and 300 px tall.

B. One dial with a mode switch: a single 2D pad and a single 1D control whose meaning changes with the selected mode (Position, Orientation, Framing). Compact, but it hides two thirds of the controls behind a click, which works against "direct access to all controls".

Recommended: A, collapsed by default to its first row and expanding on hover or click. The three rows line up with the three sidebar groups and with the three mouse buttons, which is what makes it teach:

| Row | 2D control | 1D control | Mouse equivalent shown |
|---|---|---|---|
| Position | orbit pad: longitude and latitude | zoom slider with + and − | left drag, wheel |
| Orientation | look pad: yaw and pitch | roll ring around the orbit pad: tilt | middle drag, left + right drag |
| Framing | pan pad: offset x and y | lens slider: fov | right drag, Ctrl + wheel (new) |

The roll ring sits around the orbit pad rather than the look pad, because what tilt does on screen is turn north away from up, which is what a compass ring around the globe says without words. Its N marker is placed by computed coordinates, per the rotation finding above.

```
collapsed (default)        expanded (hover or click)

     ╭───N───╮                  ╭───N───╮
    │  ╭───╮  │                │  ╭───╮  │   roll ring: drag to tilt, click N for north up
    │  │ ✥ │  │                │  │ ✥ │  │   orbit pad
    │  ╰───╯  │                │  ╰───╯  │
     ╰───────╯                  ╰───────╯
       [+]                        [+]
        ┃                          ┃         zoom
       [−]                        [−]
       [⋯]                       ╭─────╮
                                 │  ◉  │     look pad: yaw and pitch, click center to level
                                 ╰─────╯
                                 ╭─────╮
                                 │  ✥  │     pan pad: offset, click center to recenter
                                 ╰─────╯
                                  [◐]━━━    lens: field of view
                                  [?]        gesture cheat sheet
```

### Behavior

- Pads are joysticks, not trackballs. The pads are about 48 px across, too small for a drag across them to map onto 180° of globe directly; a deflection from the center sets a rate instead, applied by a `Timer` while held, and the knob springs back on release. Rate grows with deflection (quadratic, so a small push is fine control). Clicking an edge arrow steps by a fixed amount (for example 15° of longitude).
- The orbit rate scales with the camera's distance, through `coarse_drag_gain`, so a full push feels the same at every zoom.
- Zoom, lens and roll auto-repeat while a button is held.
- Resets mirror the sidebar: clicking N sets tilt to 0, clicking the look pad's center sets yaw and pitch to 0, clicking the pan pad's center sets both offsets to 0, double-clicking the lens returns fov to its default. Camera Position has no reset in the sidebar and gets none here; the presets are the way back.
- Opacity follows attention: about 40% while the pointer is elsewhere, full over the preview, so the control does not spoil judging a composition. A key (H) and a small button hide all overlays.

### Teaching the gestures

1. Every control's hover hint names the gesture next to the action, with a small mouse glyph whose button is highlighted: "Orbit the globe. Faster: drag it with the left button." The glyphs are four SVGs (left, middle, right, wheel) under `ui/`, colorized.
2. Reverse highlighting: while a gesture is in progress on the globe, the row that does the same thing lights up. `image-container` already has `left-held`, `right-held` and `middle-held`, so this is a binding, not new state. It is the cheapest and probably the most effective teacher: people discover that the widget and the drag are the same thing by doing either.
3. A `?` button opens a cheat sheet over the preview listing every gesture and shortcut. It is also the only place keyboard shortcuts would be documented in the UI.
4. Optional: a one-time hint on first launch pointing at the control. It needs a persisted "seen" flag in `AppConfig`, which is not a shader parameter and stays out of `SceneParams`.

### New input that the control would introduce

- Ctrl + wheel for the field of view, so the third row has a mouse equivalent like the other two. `PointerScrollEvent.modifiers` makes this a branch in the existing handler.
- Keyboard shortcuts, through a `FocusScope` around the preview with `KeyBinding` children: arrows orbit, + and − zoom, Q and E roll, N north up, and the time control's keys below. A click on the preview has to give the scope focus (`focus-on-click`), and the sidebar's sliders take it away again, which is expected. Not required for the control itself; it is a separate decision.

### Implementation sketch

- `ui/overlays.slint` (new): `CameraControl` and its parts (`Joystick`, `RollRing`, `HoldButton`), imported by `main.slint` and placed inside `image-container` after `viewport-touch`, anchored to the top right with a margin.
- One new callback instead of nine: `camera-nudge(CameraAxis, float)` with an exported Slint enum `CameraAxis { orbit-x, orbit-y, zoom, roll, yaw, pitch, offset-x, offset-y, fov }`, plus `camera-reset(CameraAxis)`. The amount is in control units per tick, not pixels, so `DragSpeed` stays out of it.
- Rust: a pure `camera_controls` module beside `mouse_math.rs` that applies a nudge to the three settings structs with the same clamps and wraps the sliders use, unit-tested like `mouse_math`. The callback writes the struct back to the window and calls `engine.push_params`, exactly like the mouse callbacks, so sidebar sliders and overlay stay in sync for free and the engine's drag observer treats a held joystick like a drag.
- Tests: the pure math as unit tests; in `slint_ui.rs`, `find_by_element_id` plus `single_click` for steps and resets, and `mock_elapsed_time` for a held joystick moving the value monotonically.

## Component 2: the time control

### Concept

A bar centered along the bottom edge of the preview. Collapsed, it is a small pill that says what time the scene shows; expanded, it carries the date, the time of day and the transport.

```
collapsed, live            ╭──────────────────────────────╮
                           │ ● Live   Sat 18 Oct · 14:32 UTC │
                           ╰──────────────────────────────╯

expanded, paused at a chosen time
╭─────────────────────────────────────────────────────────────────────────────╮
│  ◀◀   ▶   ▶▶    60×  ▾      Sat 18 Oct 2026   14:32 UTC (16:32 local)   [Now] │
│                                                                               │
│  time  00 ─────────────▓▓▓▓▓▓▓▓▓▓●▓▓▓▓▓▓▓▓▓▓▓───────────── 24   ☀ day at view center │
│  date  J   F   M◆  A   M   J◆  J   A   S◆  O●  N   D◆        ◆ equinox / solstice │
╰─────────────────────────────────────────────────────────────────────────────╯
```

### Controls

- Time of day track, 00 to 24. Dragging it changes the hour and leaves the date alone. The track can be shaded with day and night at the point under the view's center, so that "sunrise over the place I'm looking at" is something to aim for rather than search for. Sunrise and sunset come from Astronomy Engine (`Astronomy_SearchRiseSet`), which the core already binds; this is the stretch item of the list.
- Date track, one year with month ticks. Dragging it changes the day and keeps the time of day. Snap points at the two equinoxes and the two solstices, because the shape of the terminator is what a date changes in this app. A year field (or arrows) beside it replaces the ±10 combo; clicking the date opens `DatePickerPopup` for an exact entry, clicking the time opens `TimePickerPopup`.
- Transport: step back, play/pause, step forward, and a rate selector. Steps are a minute, an hour or a day depending on the rate in force.
- Now: returns to live time and closes the gap. While the scene is not live, the pill says how far away it is ("+3 h 20 min").
- Keyboard: Space play/pause, J and L for reverse and forward with repeated presses speeding up (the convention of video editors), K pause, comma and period to step, Home for now.

### Playback semantics

- Time based, never frame based: the scene's instant is `anchor + rate × (elapsed − anchor_elapsed)`. A slow adapter drops frames and the clock still runs at the chosen speed.
- Two kinds of playback, matching the two tracks:
  - Continuous: the clock runs at a rate. Useful range 1× to about 3600×. At 20 frames a second (the engine's current tick), 3600× moves 3 minutes per frame, 0.75° of rotation; at 86 400× (a day per second) it is 18° per frame, and the globe strobes. Rates above a few thousand are not worth offering in this mode.
  - Daily: the date advances and the time of day stays, at a day, a week or a month per second. The subsolar longitude barely moves and the terminator swings through the seasons, which is the view a seasonal wallpaper is about. With UTC held fixed the Sun traces the analemma, a nice side effect rather than a goal.
- A suggested ladder: −3600×, −600×, −60×, −10×, 1×, 10×, 60×, 600×, 3600×; and for Daily, 1, 7 and 30 days per second, both directions.
- Grabbing either track pauses playback; letting go resumes it if it was running.

### Where time advances: engine or UI

The UI could drive playback with a Slint `Timer` that recomputes the instant and pushes `SceneParams` thirty times a second. It is the smaller change and it is the wrong owner: the engine is headless first and owns every schedule, its `Clock` is injected so a test can run a week of playback in milliseconds, and a UI timer keeps pushing while the window is hidden in the tray.

Recommended: the engine owns a timeline.

- A time state outside `SceneParams`, the way `texture_resolution` and the display plan are: `Live`, `Fixed(instant)`, `Playing { anchor_instant, anchor_elapsed, rate, kind }`, sent with a new `EngineCommand::SetTime`. `datetime` is already outside the digest and compared through the derived sky state, so moving it out of `SceneParams` matches how it is treated today. The 13 non-test references to `params.datetime` and the `DateTimeInput` uses in `sky`, `month` and the engine are what moves.
- One representation for an instant (UTC seconds as `f64`, or an `OffsetDateTime`) instead of hour, day of year and year. The tracks are views of that one value. The config keeps the current four keys until a migration step in `config/migrate.rs` replaces them; the old ones translate exactly.
- `EngineEvent::PreviewFrame` carries the instant the frame was drawn at, so the label under the picture always matches the picture, also when the software adapter is several hundred milliseconds behind.
- Cadence: while playing, the engine marks the frame dirty every tick and shortens `recv_timeout` so it can reach about 30 frames a second on a GPU; the latest-value preview mailbox already prevents a backlog. While live, nothing changes: the 2 minute `SKY_INTERVAL` stays, and a UI `Timer` ticks the label once a second without involving the engine.

### Interactions to decide

- Wallpaper. Today a custom date applies to the preview and to every wallpaper the engine publishes, so an auto-refresh during playback would publish whatever instant it lands on. Options: (a) keep that, one time for everything, consistent with how camera fiddling already affects the next refresh; (b) playback is a preview-only state and a publish uses the persisted Live or Fixed time. Recommendation (b): playback is an exploration tool, and a wallpaper of a random instant mid-warp is a surprise nobody wants.
- Persistence. Playback is never saved. Closing the window while playing saves the instant on screen as a Fixed time, or Live if the user had pressed Now; hiding the window to the tray pauses playback.
- Textures across months. Continuous or Daily playback across the middle of a month swaps the day floor, a visible step in the surface colors; on a CPU adapter it also opens another month's pack, whose cost under playback has not been measured. The tile wants depend on the sky state (`want_tiles(sky)`), so a moving terminator may keep the loader busy in blend mode; this needs a measurement, and the drag observer's throttling is a candidate to extend to playback.
- Clouds. The cloud map is the latest observed one whatever the scene's time, as it already is for a custom date. A note in the pill ("clouds: today") when far from now would be honest.
- Local time. The hour is UTC today. Showing the user's local time beside it needs the local offset, which the `time` crate already gives with its `local-offset` feature: at 0.3.47, `UtcOffset::local_offset_at` on Unix calls `localtime_r` with no thread check, and only `refresh_tz`, which rereads `TZ`, is limited to single-threaded processes and macOS (`time/src/sys/refresh_tz/unix.rs`). A first version of this paragraph said the call refuses in a multi-threaded process; 0.3.47 does not. What it does not give is zone names, for which `jiff` or `chrono` with a time zone database would be needed.

### Ideas for later

- Drag the time on the globe itself: a modifier drag (Alt + left, say) where horizontal is time of day and vertical is date, which fits the camera control's habit of teaching gestures.
- Jump to the next new moon, full moon or eclipse via Astronomy Engine's search functions. The Moon already covers the Sun correctly from the camera's point of view (`sun_occlusion`, `disk_occlusion`); the Moon's shadow on the Earth is not rendered, as far as `docs/rendering.md` and the scene modules show, so a solar eclipse would look incomplete.

## Shared infrastructure

- `ui/overlays.slint` holds both components and their shared look (fill, radius, hover opacity, the `Hint` tooltips).
- One overlay visibility toggle (button and H), persisted as a window setting in `AppConfig`, not in `SceneParams`.
- Narrow previews: `min-image-width` is 240 px. Below roughly 420 px of preview width the camera control stays collapsed and the time bar shows only the pill; the loading pill in the bottom right moves above the time bar.
- Accessibility: every control gets `accessible-role` and `accessible-label`, which is also what lets `slint_ui.rs` find them with `find_by_accessible_label`.
- The sidebar groups stay. They remain the precise way in, and both overlays write the same window properties they do.

## Open questions

1. Camera control layout: stacked (A, recommended) or mode dial (B)?
2. Keyboard shortcuts as part of this work, or later?
3. Playback and the wallpaper: preview only (recommended) or one shared time?
4. Is moving the date and time out of `SceneParams` into its own engine command acceptable, given that it touches the config format and needs a migration step?
5. Local time beside UTC, and if so, which crate?
6. Which of the extras are in scope for a first version: equinox and solstice snapping, the day and night shading of the time track, the Daily playback mode, the first-run hint?
