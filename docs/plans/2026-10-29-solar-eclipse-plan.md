# Plan: Solar Eclipses

## Summary

Two features that work together. First, the Moon's shadow is drawn on the Earth where and when it falls: a penumbra a little more than an Earth diameter across that darkens the globe, the clouds and the blue limb, with the umbra or the antumbra a dark spot inside it. It is drawn in the blend surface mode, from the true geometry on the WGS84 ellipsoid while the globe stays a sphere, and costs nothing outside the few hours of each eclipse. Second, eclipse mode: a button in the time bar that sets the time to the next solar eclipse and keeps the camera over the shadow as the time of day moves, with a panel above the bar for the previous and the next eclipse, the phases and their times, and what the instant on screen shows. The date track marks every eclipse of the year, and while the mode is on the day track shows when the shadow is on the Earth.

The research, the measurements and the decisions behind this are in `2026-10-29-solar-eclipse-research.md`. The rendering of the Moon in front of the Sun as the camera sees it stays as it is (research section 4).

## Stakes Classification

Medium to high. The shadow adds a branch to `fs_main`, `blend_fragment`, `fs_cloud` and `fs_rayleigh`, which every frame runs, and its promise is that frames outside an eclipse are bit for bit what they are today; the golden suite checks that. Eclipse mode adds a third case to `SceneParams::camera_at`, whose callers already funnel every camera of the preview, the wallpaper and the tile residency through it, and a set of exits spread over the time bar, the camera controls and the settings window. The likely mistakes are a sign or a frame error in the shadow's place, which an engine case pins against Astronomy Engine; a camera action that should end the mode and does not, which the window tests walk one by one; and a panel that covers a control or the camera panel in a small preview.

## What exists today

Checked against `main` at `ca634ac`:

- `scene::sky::SkyState` holds `sun_direction`, the aberrated geocentric Sun normalized, and `moon_position`, `GeoMoon` in Earth radii, both in the Earth-fixed world frame through `world_from_eqj`. The Sun's distance is normalized away. Astronomy Engine's own eclipse search builds the shadow from the same two vectors.
- The cube faces put the Blue Marble's geodetic latitude at the direction whose elevation is that latitude (`asin(direction.y)` in the texture pipeline), so each direction of the drawn sphere is the true surface normal of the place drawn there, and the camera's latitude means the same. Only positions on the sphere differ from the Earth's (research section 2.3).
- `SceneParams::camera_at(sky)` (`params.rs:326`) resolves the camera for the sky of a frame: the settings' camera in the Earth-fixed mode, the longitude from the time of day in the Sun-fixed mode. `write_uniforms` (`render_pass.rs:149`), the frame key (`frame.rs:38`) and the tile outputs (`engine/mod.rs:1089`, `1120`, `1286`) take it. `held_camera` is the camera as the settings hold it, which the digest and the drag watch take.
- The uniform block is 608 bytes, asserted in `uniforms.rs`; `flags` uses the values 1, 4, 8 and 16 (`render_pass.rs:62` to `68`). The shells' radii are `CLOUD_SPHERE_RADIUS` 1.0015 and `RAYLEIGH_RADIUS` 1.015 (`params.rs`).
- `the_shader_and_the_cpu_agree_on_the_three_shared_rules` (`tests/render_pipeline.rs:1353`) probes the rules that live in both Rust and WGSL.
- The time bar (`crates/sunlit-ui/ui/overlays/time-bar.slint`, `crates/sunlit-app/src/time_bar.rs`): the transport is centered over the tracks, 400 px at full pitch; the tracks' column is the bar's width less `beside-tracks`, 260 px; the open bar is 124 px tall, at most 800 px and at least `narrowest`, 564 px. The date track carries `SeasonMark`s from `refresh_year`; the day track is shaded by the Sun's altitude over the camera (`refresh_shading`), skipped in the Sun-fixed mode. `set_time` writes the time setting and ends an excursion; Now, the tracks, the date picker and exact time entry go through it or beside it. Hover hints sit 8 px above the bar (`main.slint`, `hint-over-bar`).
- The preview's keys are handled in `main.slint`'s `track-key` and `KeyBinding`s. Page Up and Page Down are bound nowhere.
- The camera panel sits at the preview's top right, 92 px wide, 12 px from the edges.
- The time setting is held to the years 1900 to 2100.

## Key Design Decisions

### Part A: the shadow

#### A1. One light-left rule, in Rust and in WGSL

A new module `scene::eclipse` in `sunlit-core` holds the geometry. Its center is the light-left function of research section 3.1: for a point `P` in world space in Earth radii, with the Moon's center `M`, the unit axis `a` from the Sun's center through the Moon's, and the Moon to Sun distance `D`,

```
q = P - M,  z = dot(q, a),  r = length(q - z * a)
separation = atan(r / z) - atan(r / (z + D))
moon_radius = asin(R_moon / length(q)),  sun_radius = asin(R_sun / length(q + D * a))
light_left = 1 - overlap(separation, moon_radius, sun_radius) / (pi * sun_radius^2)
```

with `overlap` the circle-circle lens area that `disk_occlusion::overlap_area` already computes, and 1 for `z <= 0`, behind the Moon. The disk is uniform: limb darkening (research section 3.2) changes brightness only inside the penumbra, and the look decides later whether to add it. `R_moon` is 1737.4 km and `R_sun` 695,700 km, Astronomy Engine's radii, as constants in Earth radii of `EARTH_RADII_PER_AU`.

`P` is the true point of the place drawn, not the point of the sphere. For a fragment at the sphere's direction `n` and a height `h` above the surface (0 for the globe, `CLOUD_SPHERE_RADIUS - 1` and `RAYLEIGH_RADIUS - 1` for the shells),

```
surface_point(n, h) = N * (n.x, (1 - e^2) * n.y, n.z) + h * n,   N = a / sqrt(1 - e^2 * n.y^2)
```

with the WGS84 equatorial radius `a` in the app's Earth radii and `e^2 = f * (2 - f)`, `f = 1 / 298.257223563`. That is one inverse square root and a few multiplies, and it takes the shadow from the sphere's place, up to 78 km off near the poles, to Astronomy Engine's own (research section 2.3). The globe, the shells and everything else drawn stay as they are.

The WGSL twin, `eclipse_light(n, h)`, lives in `sphere.wgsl` with its own `surface_point`, and the rule becomes the fourth shared one: the test is renamed `the_shader_and_the_cpu_agree_on_the_four_shared_rules`.

#### A2. The shadow's inputs travel in the sky state

`SkyState` gains `shadow: ShadowAxis { moon: Vec3, axis: Vec3, moon_to_sun: f32 }`, computed in `compute_sky_state` in f64 from the Sun vector before it is normalized and the Moon vector, and rotated into the world frame with the rest. `moon` equals `moon_position`. Every frame then has the shadow of its own instant, through the excursion, the export replays and the wallpaper, by the path the Sun already takes.

#### A3. A gate keeps every other frame as it is

On the CPU, per frame, the penumbral cone's distance from the Earth's center is compared with `RAYLEIGH_RADIUS`, the outermost shell, by the sphere's contact test of research section 5.5 with that radius. The sphere encloses the ellipsoid and its shells, so the gate never closes on a fragment the shadow reaches; near a pole it opens early by the time the penumbra takes to cover the 21 km the sphere stands above the ellipsoid there, which costs those frames the shadow's arithmetic and nothing else. Only when the cone reaches it, and only in the blend surface mode, `write_uniforms` sets a new flag, `FLAG_ECLIPSE` = 32, and fills the new uniforms. Each shader tests the flag once, in uniform control flow, and outside it runs the code it runs today. A frame outside an eclipse, or in the grid, Day or Night mode, is then the frame of today.

#### A4. From light left to pixels

The shade is `mix(1.0, pow(light_left, 1.0 / 2.2), moon_shadow)`, the light left re-encoded as the stored values are, scaled by the strength setting. It multiplies every term that is sunlight (research section 3.3): the day surface in `blend_fragment` after its `min(night, day)` clamp, the specular glint and the Fresnel sky color in `fs_main`, the day brightness and the share of the deck drawn at the day opacity in `fs_cloud`, and the day blue and the sunrise lobe in `fs_rayleigh`. The nightglow shells do not take it. Nothing else changes: the umbra darkens and shows no night texture (decision of 2026-10-29), so the wanted set of tiles is untouched.

#### A5. Uniforms

Appended to the block: `eclipse_moon: vec3` with `eclipse_moon_to_sun: f32`, `eclipse_axis: vec3` with `eclipse_strength: f32`, then `eclipse_moon_radius: f32`, `eclipse_sun_radius: f32`, `eclipse_earth_radius: f32` (WGS84's `a`) and `eclipse_earth_e2: f32`, 32 bytes, so the block grows to 640 and the size assertion with it. The four are constants but go through the block so that the WGSL holds no copy of them to drift.

#### A6. One setting

`moon_shadow`, 0 to 1, default 1, in the Moon's row group of the Celestial settings as "Shadow on Earth", hint "How dark the Moon's shadow is drawn on the Earth during a solar eclipse. Drawn in the Blend surface mode." It is the fourteen edits of a shader parameter (CLAUDE.md), in the `quantized` group of `scene_digest!`. Neither `moon_size` nor the pixel floor reaches the shadow (research section 3.4).

#### A7. The cost is measured before it is accepted

Inside the gate a fragment of the globe, the cloud shell and the Rayleigh shell runs `eclipse_light` once: two `atan`, two `asin`, the two `acos` of the lens area and a few square roots. Step 2 measures a preview frame inside an eclipse against one outside it, on WARP and on the desktop GPU, at the preview sizes of the Sun-fixed plan's table, and the step does not close until the user has seen the numbers.

### Part B: the eclipses themselves

#### B1. A search over Astronomy Engine

`scene::eclipse` wraps `Astronomy_SearchGlobalSolarEclipse`, `Astronomy_NextGlobalSolarEclipse` and `Astronomy_SearchLocalSolarEclipse`, each call site with a scoped `#[allow(unsafe_code)]` and a `// SAFETY:` comment, in the pattern of `sky::seasons`. Its public surface:

```rust
pub struct Eclipse {
    pub kind: EclipseKind,                // Partial, Annular, Total
    pub peak: OffsetDateTime,
    pub contacts: Contacts,               // p1, p4, and u1, u4 where the umbra touches
    pub central_duration: Option<Duration>,
    pub greatest: Greatest,               // latitude, longitude, share of the Sun covered
}
pub fn next_after(at: OffsetDateTime) -> Option<Eclipse>;
pub fn previous_before(at: OffsetDateTime) -> Option<Eclipse>;
pub fn nearest(at: OffsetDateTime) -> Option<Eclipse>;
pub fn on_earth_at(at: OffsetDateTime) -> Option<Eclipse>;   // P1 <= at <= P4
pub fn in_year(year: i32, offset_at: impl Fn(OffsetDateTime) -> UtcOffset) -> Vec<Eclipse>;
```

`previous_before` starts 200 days back and steps forward (research section 5.1). Every function answers `None` or leaves out what falls outside 1900 to 2100, so previous and next stop at the range's ends.

#### B2. Contact times from the cone geometry, on the ellipsoid

`Contacts` comes from the two cone conditions of research section 5.5 with the ellipsoid in place of the sphere, in f64 in the world frame, whose +Y is the Earth's axis of date. The method is the Besselian one: project the ellipsoid along the shadow axis onto the plane through the Earth's center perpendicular to it, which gives an ellipse with semi-axes `a` and `a * sqrt(sin^2 d + (1 - f)^2 * cos^2 d)`, `d` the axis's angle to the equator; a cone touches the Earth when the axis's distance from that ellipse equals the cone's radius in the plane, the cone's opening carried as in the sphere's test. The distance from a point to an ellipse is a one dimensional Newton iteration. A scan at two minute steps from six hours before the peak to six hours after brackets each root, and bisection takes it to a tenth of a second.

The research computed the same contacts by minimizing the cone's distance over the ellipsoid itself, within 6 seconds of NASA's maps for P1 and P4 and 9 seconds for U1 and U4 over six eclipses (research section 2.3). The projection leaves out the cone's opening across the depth of the ellipsoid, about 21 km times `tan(0.27 degrees)`, a tenth of a km, so it should agree with that to well under a second; test 3 checks it. The panel shows minutes.

#### B3. Facts at greatest eclipse

For a central eclipse, `Greatest` is the library's latitude, longitude and share covered, and `central_duration` the local search at that point, within six seconds of NASA's durations. For a partial eclipse, `Greatest` is the shadow's center at the peak (B4), on the limb, and the local search's share covered there, and `central_duration` is `None`.

#### B4. The shadow's center and the point the camera follows

```rust
pub fn shadow_center(shadow: &ShadowAxis) -> (f32, f32)    // latitude, longitude in degrees
pub fn followed_point(shadow: &ShadowAxis) -> (f32, f32)
```

Both are computed in the frame where the world's +Y is scaled by `1 / (1 - f)` and the ellipsoid becomes a sphere of radius `a`, which maps the axis to a line. There, with `c` the point of the axis nearest the center and `a_s` the scaled axis's direction, `shadow_center` is `c - sqrt(max(0, a^2 - |c|^2)) * a_s`: the axis's intersection with the ellipsoid once scaled back, exactly, or the point of the outline nearest the axis when it misses, to within the flattening's effect on distances. `followed_point` is the same with the square root softened to `sqrt(max(0, a^2 - |c|^2) + EPS^2)`, `EPS` 0.2, so the camera turns at most about 2.4 degrees per minute of eclipse and hangs at most about 10 degrees of arc off the shadow's center, 18 to 463 km at the peaks of 2026 to 2030 (research section 5.4, measured on the sphere). Scaled back, each point's latitude is read as geodetic, `atan2(y, (1 - f)^2 * hypot(x, z))`, which is what the camera's latitude and the map mean. The camera takes `followed_point`; everything that says where the shadow is takes `shadow_center`.

### Part C: the camera

#### C1. A third case of `camera_at`

`SceneParams` gains `eclipse_follow: bool`, in the `switched` group of `scene_digest!` beside `sun_fixed`. With it on, `camera_at(sky)` returns the settings' camera with the latitude and the longitude of `followed_point(&sky.shadow)`, ahead of the Sun-fixed case. Distance, tilt, yaw, pitch, lens and framing stay the settings'. `held_camera` ignores the flag, so the drag watch never takes the shadow's motion for a drag and the digest does not move with time; the frame key already compares the resolved camera (Sun-fixed plan, decision 9), so every frame of an excursion draws where the shadow is.

#### C2. The setting

`AppConfig` gains `eclipse_follow = false` in the camera position block, and `CameraPositionSettings` in `main.slint` gains `eclipse-follow`. No migration: the key is new and its default draws what every config draws today. The eclipse the mode is on is not saved: it is `eclipse::nearest` to the time setting (research section 5.3).

#### C3. No schedule of its own

The mode always sits on a fixed time, since entering it writes the peak and every return to live time leaves it, so the sky schedule is not involved. A config edited by hand to live time with the flag on draws a camera over wherever the axis points, on the two minute schedule, which is harmless.

### Part D: the time bar

#### D1. Entering the mode

`time_bar::enter_eclipse(window, link, eclipse)` writes the peak through `set_time`, which ends an excursion, sets `eclipse-follow`, pushes the params and refreshes the bar. The mode button enters at `on_earth_at(on screen)` or else `next_after(on screen)`; previous and next, in the panel and on Page Up and Page Down, at `previous_before` and `next_after` of the current eclipse's peak, or of the instant on screen when the mode is off; a click on a date track mark at that mark's eclipse.

#### D2. Leaving the mode

`time_bar::leave_eclipse(window)` resolves the camera at the instant on screen with `followed_point` under `sky::compute_sky_state` of that instant, writes the latitude and, through the same branch `set_held_longitude` uses, the longitude or the time of day, and clears the flag. Callers, from research section 5.3:

| Action | Where |
|---|---|
| The mode button while on | the new `time-eclipse-toggled` callback |
| Now, the 0 key | `on_time_now` |
| The override switched off | `on_datetime_override_toggled` |
| A setting write that changes the displayed day | `set_time`, when the new instant's date in the displayed zone differs from the old one's; `on_datetime_edited` likewise |
| Date playback: play, a rate, a shuttle, reverse or a skip with the Date kind | `act` and `on_time_skip`, when the kind is `Date` and the action moves the instant |
| The globe drag | the first move after `on_mouse_drag_globe_begin` |
| The orbit stick, the orbit arrows, the orbit keys | `apply_camera_stick` for `Orbit`, `on_camera_orbit_step` |
| A preset | `on_apply_preset` |
| The Sun-fixed switch | `on_camera_frame_toggled`, before its conversion |
| Load Defaults, Reset | the flag comes from the config like every key, without a conversion |

The phase rows and previous and next write the time without leaving, since they are the mode's own. Zoom, look, frame, roll and north up do not touch it.

#### D3. The mode button

A 28 px button at the right end of the transport row, its right edge on the tracks' right edge, in the transport's own button style. Its glyph is the total eclipse mark (D5), drawn in `OverlayStyle.dim` while off and on `OverlayStyle.accent` while on, the way play lights while running. `narrowest` grows by the button and a 6 px gap, 34 px, to 598 px, and `pitch` counts them: the transport stays centered over the tracks while its right end clears the gap, and otherwise centers in the room left of it. Hint id `eclipse-mode`: "Eclipse mode" / "Jumps to the next solar eclipse and keeps the Moon's shadow in view as the time of day moves. Click again to stop following." with the keys Page Up and Page Down for the neighbors.

#### D4. The panel

A component of its own, `overlays/eclipse-panel.slint`, about 240 px wide, its right edge on the bar's right edge and its bottom 8 px above the bar, shown exactly while the bar is open and the mode is on. A pointer over it counts as over the bar, so moving up into it does not let the bar collapse. Top to bottom:

```
‹   ● Total solar eclipse                  ›
    Wed 12 Aug 2026 · UTC+2
◌  Partial begins                    17:34
●  Total begins                      18:57
◆  Peak                              19:45
●  Total ends                        20:34
◌  Partial ends                      21:57
Totality 2 min 21 s at greatest eclipse
Now: Sun 100% covered at 65° N
```

(the times there are the 2026-08-12 eclipse's in UTC+2 as the research's ellipsoid contacts and Astronomy Engine's peak give them). A partial eclipse has three phase rows and "93% of the Sun covered at most, at 61° N"; an annular one says "Annularity". The row of the current phase is highlighted, and a click on a row sets the time to it. Previous and next are 20 px chevrons, disabled at the range's ends, with hints naming their target, "Previous solar eclipse: Partial, Sat 29 Mar 2025". The last line follows the instant on screen: before P1 "Shadow reaches Earth in 1 h 12 min", between P1 and P4 the share of the Sun covered at the shadow's center (one minus `light_left` at `shadow_center`) and its latitude, after P4 "Shadow left Earth 20 min ago".

The room above the bar decides how much is shown. Where the panel would reach the camera panel's bottom plus 8 px, in a preview narrower than about 1000 px and too short for both, it drops the phase rows and the fact line and keeps the header and the last line. The bar's hover hints go above the panel while it is shown.

#### D5. Marks on the date track

`EclipseMark { fraction, kind, label }`, set by `refresh_year` with the season marks from `eclipse::in_year` in the displayed zone, keyed like them by year and zone. Each is a 9 px `Glyph` in `OverlayStyle.sun` on the track's centerline: total, a dark disk with a thin bright rim; annular, a thick bright ring around a smaller dark center; partial, a bright disk with a dark bite. Hovering one shows its label in place of the track's date label, "Total solar eclipse · Wed 12 Aug 19:45"; a click calls `eclipse-picked(index)`, which enters the mode at it. A mark has its own `TouchArea` above the track's, so a press on it never starts a drag. Where an eclipse mark and a season mark are closer than a glyph, the eclipse is drawn over the diamond and the label names both, "Total solar eclipse · Mon 20 Mar 10:17 · March equinox" in UTC.

#### D6. Bands on the day track

While the mode is on, `refresh_shading` gives way to `refresh_eclipse_bands`, which sets the bands of the current eclipse in fractions of the displayed day: P1 to P4 in the sun color at low opacity, U1 to U4 stronger, a 1 px line at the peak, each clipped to the day and, where clipped, ending in a small arrow at the track's edge. Leaving the mode clears the shading key, so the altitude shading comes back on the next refresh.

#### D7. The collapsed bar and the settings window

While the mode is on, the collapsed pill shows the eclipse's glyph before its text. In the settings window, the Camera Position group replaces its position rows (Longitude or Time of day, and Latitude) with one line, "Following the solar eclipse of Wed 12 Aug 2026", the way the Sun-fixed mode swaps its rows; Zoom and the Sun-fixed switch stay, and the switch leaves the mode (D2).

#### D8. Keys

Page Up enters or moves the mode to the previous eclipse and Page Down to the next, in `track-key` with the other time keys, and both rows go into the shortcut sheet. No key toggles the mode (open item).

## Edge cases at a glance

| Situation | Behavior |
|---|---|
| A frame outside every eclipse, or not in the blend mode | The frame of today, bit for bit (A3) |
| The mode button with the shadow on the Earth now | Stays on that eclipse and writes its peak (D1) |
| Previous at the first eclipse after 1900, next at the last before 2101 | The button is disabled, the key does nothing |
| Play the clock in the mode | The camera follows the shadow in the preview; the setting and the wallpaper stay at the time set |
| Play past P4 or back before P1 | The camera keeps following the axis's nearest point, which moves on smoothly; the mode stays on |
| Date playback, a date track pick, the date picker | The mode ends, the camera stays where it was (D2) |
| Day track drag across the peak | The mode stays and the camera follows |
| Globe drag in the mode | The mode ends at the first move, and the drag carries on from the camera shown |
| Zoom, look, frame, roll in the mode | Applied; the mode stays |
| Sun-fixed mode on when entering | Eclipse mode takes precedence; on leaving, the time of day is set from the camera shown |
| An eclipse across midnight in the displayed zone | The bands are cut and arrowed; the phase rows give every time |
| Restart in the mode | The flag and the fixed time come back; the eclipse is the one nearest the time setting |
| `render`, `displays --out` | Drawn from the config, with the camera following if the flag is set |
| A partial eclipse | Three phase rows; the camera follows the point nearest the axis, on the limb at the peak |
| An eclipse mark on a season's day | Drawn over the diamond, the label names both |
| A narrow and short preview | The panel keeps its header and last line (D4) |
| An older build reads the config | Drops `eclipse_follow` and shows the position saved at the last exit |

## Success Criteria

- Every golden reference passes unchanged and no existing test changes, other than the rename of the shared rule test.
- At the peaks of 2024-04-08 and 2026-08-12 the darkest pixel of the globe reprojects to Astronomy Engine's peak point within the larger of 25 km and two pixels.
- The contact times of the six eclipses of research section 2.3 come out within 15 seconds of NASA's, and the durations within six seconds.
- The mode button, previous, next and a date track mark each put the camera over the shadow at the peak; every exit of D2 ends the mode without the frame jumping.
- In the mode, playing the clock keeps the umbra inside the globe's disk from U1 to U4 at the default zoom.
- A frame inside an eclipse costs WARP no more than the budget fixed in step 2.

## Implementation Steps

### Step 1: The geometry and the search

`scene::eclipse`: the radii and the WGS84 constants, `surface_point`, `light_left`, `ShadowAxis`, the sphere's cone test for the gate and the ellipsoid's for the contacts, `shadow_center`, `followed_point`, `Eclipse` and the five search functions with their FFI. `SkyState::shadow` in `compute_sky_state`. Unit tests 1 to 8.

### Step 2: The shadow

The uniforms of A5 and their WGSL; `FLAG_ECLIPSE` and the gate in `write_uniforms`; `eclipse_light` in `sphere.wgsl` and its use in `blend_fragment`, `fs_main`, `fs_cloud` and `fs_rayleigh`; the `moon_shadow` setting's fourteen edits. Render tests 9 to 13. Then the measurement of A7: a preview draw inside and outside the 2024 eclipse, on WARP (`--software-rendering`) and on the desktop GPU, at 704 by 576, 1088 by 896 and 1920 by 1088, median of 25 frames after 5, recorded here as a departure with a budget the user agrees to.

### Step 3: The camera

`eclipse_follow` in `SceneParams`, `from_config`, `write_to_config`, `scene_digest!` and `AppConfig`; the third case of `camera_at`. Unit tests 14 to 16 and engine test 17.

### Step 4: The time bar

`EclipseMark` and its glyphs; the mode button and the layout change of D3; `eclipse-panel.slint` and its view struct, filled in `time_bar::refresh` from a cached `Eclipse` keyed by its peak; the bands of D6; the hint ids and texts; the pill glyph; Page Up and Page Down and the shortcut sheet.

### Step 5: Entering and leaving

`enter_eclipse` and `leave_eclipse`, and every caller of the table in D2; the Camera Position group's line of D7.

### Step 6: Window tests

Tests 18 to 27.

### Step 7: Documentation

`docs/roadmap.md`: the eclipse item under Rendering becomes the solar half done, the lunar eclipse and the corona left. `docs/rendering.md`: a section on the Moon's shadow, its rule, its gate, its mapping and the cost measured. `docs/architecture.md`: `camera_at`'s third case next to the Sun-fixed one. `docs/usage.md`: eclipse mode, its panel, the marks, the keys and what leaves the mode. `docs/testing.md`: the fourth shared rule. README only with the user's permission.

### Step 8: Validation with the user

Run the app at the 2024-04-08 and the 2026-08-12 eclipses: the shadow's look at the default strength, the camera's follow while playing the clock at 1 min/s and 10 min/s, `EPS` 0.2 against a stiffer or a softer value, the panel and the bands at the full width and at the narrowest bar, and each exit of D2. Publish a wallpaper in the mode on two screens in a Linux guest with `--screens 2`.

## Tests

Core, unit (`scene::eclipse`):

1. `light_left` is 1 outside the penumbra and behind the Moon, 0 on the axis at a total geometry, positive at the antumbra's center at an annular geometry, and falls monotonically from the penumbra's edge to the axis.
2. `light_left` computed in f32 in the axis frame agrees with an f64 reference to a thousandth along a line through the umbra.
3. The contacts of the six eclipses of research section 2.3 are within 15 seconds of NASA's maps, the times taken onto Astronomy Engine's ΔT as there, and for every eclipse of 2020 to 2030 P1 precedes the peak, which precedes P4, and U1 and U4, where present, lie between them. `surface_point` puts a pole at `a * (1 - f)` from the center and the equator at `a`, and its direction is never more than 0.2 degrees from `n`.
4. `next_after` and `previous_before` are inverse over 2020 to 2030: the previous of each eclipse's next is the eclipse itself; and `previous_before` of the first eclipse of 1900 and `next_after` of the last of 2100 are `None`.
5. `on_earth_at` is the eclipse between P1 and P4 and `None` an hour outside them; `nearest` of a peak plus or minus a day is that eclipse.
6. The central durations of 2024-04-08 and 2027-08-02 are within ten seconds of NASA's 4m28.1s and 6m22.6s.
7. `shadow_center` at the peak of each central eclipse of 2026 to 2030 is within 2 km of Astronomy Engine's peak point, the two near the poles included, and at the peak of 2025-03-29 within a degree of NASA's 61 N, 77 W.
8. Over P1 to P4 of 2024-04-08 sampled each 10 seconds, `followed_point` never moves more than 3 degrees a minute and never strays more than 11 degrees from `shadow_center` while the axis is on the Earth.

Render, `tests/render_pipeline.rs` and the `golden` target:

9. The shared rule probe compares `eclipse_light` with `light_left`, `surface_point` included, along a line of directions through the umbra, the penumbra and outside it, on the globe and at both shells' heights, at a total and an annular geometry.
10. At the peaks of 2024-04-08 and of 2026-08-12, whose shadow meets the Earth near the pole, in blend mode with the Earth fixture, the darkest pixel of the globe reprojects, as a geodetic latitude, to within the larger of 25 km and two pixels of Astronomy Engine's peak point; at the peak of 2023-10-14 the antumbra's center is lit above zero and darker than the penumbra around it.
11. A frame at an instant with no eclipse, and a frame inside one in the grid mode, are bit for bit the frame with `moon_shadow` at 0.
12. With `moon_shadow` at 0 inside an eclipse the frame is the frame without the feature; at 0.5 it lies between that and 1.
13. One golden over the Earth fixture with the cloud fixture at a time when the penumbra and the umbra are both in view.

Core, `params.rs`:

14. With `eclipse_follow` on, `camera_at` returns `followed_point`'s latitude and longitude under any sky and the settings' zoom, tilt, yaw, pitch and framing; with it off, today's cameras exactly.
15. `eclipse_follow` takes precedence over `sun_fixed`, and `held_camera` is unchanged by it.
16. The digest's mutation table covers `eclipse_follow`; a config without the key loads with the mode off, and the key round trips.

Engine, `tests/engine.rs` on the mock clock:

17. In the mode inside an eclipse, a clock excursion's frames resolve cameras that follow the axis, the outputs the tile loader is asked for carry them, and no drag is reported.

Window, `tests/slint_ui.rs`, at pinned instants:

18. The mode button with the time on 2026-06-01 writes the peak of 2026-08-12 and turns the flag on; with the time inside the penumbra of 2026-08-12 it writes that peak.
19. Previous and next in the panel, and Page Up and Page Down, walk 2026-02-17, 2026-08-12, 2027-02-06 and back, keeping the flag on; with the mode off, Page Down enters it.
20. A click on a date track mark enters the mode at that mark's eclipse; a press on a mark starts no drag.
21. Each exit of D2 clears the flag, and the resolved camera before and after agrees to a hundredth of a degree; in the Sun-fixed mode the time of day is what puts that longitude under the camera.
22. A day track drag, exact time entry, the Hour slider, zoom, look, frame and roll keep the flag.
23. A clock playback keeps the flag; a Date playback, a Date skip and a date track pick clear it.
24. The panel shows exactly while the bar is open and the flag is on, its phase rows match `Contacts` in the displayed zone, a row click writes that instant and keeps the flag, and a partial eclipse has three rows.
25. With the flag on, the day track shows the bands and no altitude shading; after leaving, the shading comes back.
26. The open bar is never narrower than the new `narrowest`, and at it the transport and the mode button do not overlap.
27. Load Defaults turns the mode off.

## Out of Scope

- The Moon in front of the Sun as the camera sees it: a corona, the glare's centroid, Baily's beads (research section 4).
- Limb darkening and a color cast in the penumbra, which the look may ask for later (research section 3.2).
- The night texture in the umbra (decision 2).
- The shadow in the grid, Day and Night modes (decision 1).
- Lunar eclipses.
- Place names, the Saros series, the path width.
- An ellipsoidal globe. The drawn globe stays a sphere; only the eclipse's geometry is computed on the ellipsoid (research section 2.3).

## Risks and Mitigations

- The shadow's branch costs more than a software adapter can afford during the hours it runs. Step 2 measures before the step is closed; the gate already keeps every other frame free, and a cheaper rule (a precomputed table of the lens area by separation) is the fallback.
- A shader change outside the gate would move every golden reference. Test 11 compares frames bit for bit and the golden suite stays as it is.
- An exit missed in D2 leaves a camera following the shadow after the user took it over. The table lists every writer of the camera's position in `ui_callbacks.rs` as of `ca634ac`, and tests 21 to 23 walk them; a new writer of the position later has to call `leave_eclipse`, which `docs/architecture.md` says next to `camera_at`.
- The panel covers something in a small preview. D4's compact form and step 8's check at the narrowest bar.

## Open items

- A key for the mode itself, if wanted: X is free and does not depend on the layout.
- `EPS`, 0.2 proposed, confirmed in step 8.
- The cost budget of step 2.
- The texts of the hints, the panel's lines and the Camera Position line.
