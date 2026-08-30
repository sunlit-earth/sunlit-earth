# Plan: multi-monitor wallpapers (2026-08-30)

## Summary

The wallpaper stops being one image at the primary monitor's resolution and becomes a plan over the monitors a session has: a list of them, a mode that says how they relate, and one image per monitor at that monitor's own size. Three modes ship: one screen, the same view on every screen, and one continuous view across all of them, the last anchored so that the chosen screen keeps exactly the framing it would have alone.

Almost none of this is platform work. The geometry is a pure function from a list of rectangles to a canvas size, a modified `SceneParams` and a set of crop rectangles, and it is testable on any machine with no display attached. What is platform work is two thin functions per OS: enumerate the monitors, and hand the desktop N files instead of one. That split is deliberate, because it is what makes a Linux test guest with two heads good evidence about Windows.

Roadmap item: "Multi-monitor support: detect monitor layout and resolution, render appropriately sized wallpapers for each display."

## Stakes

Medium. Nothing about the shaders, the engine loop or the texture pipeline moves. What moves is the `WallpaperSink` contract, which has three implementations and one test double, and the Windows setter, which gains a COM interface next to the `SystemParametersInfoW` call it has now. A single-monitor session must come out of this byte-identical to what it does today, and that is the first test to write.

## What is already there

Worth reading before touching anything, because most of the pieces exist and several of them already half-expect this feature.

- `display.rs` is the display query. `Output` already carries name, primary flag, size and position in the screen's coordinate space, with the comment that the position "is what makes a multi-monitor bounds check possible at all". `parse_outputs` handles rotation (xrandr reports post-rotation geometry), disconnected outputs, outputs with no mode, and negative offsets. The parser is separate from the process, so it is tested on every platform against real xrandr text. `outputs()` is implemented on Linux and returns `None` everywhere else.
- `wallpaper.rs` already enumerates Windows monitors with `EnumDisplayMonitors` and `GetMonitorInfoW`, but throws all of them away except the one with `MONITORINFOF_PRIMARY`, returning its size. It writes one PNG, alternating between two file names so that each publish hands the desktop a path it has to reload, sets `WallpaperStyle=10` (fill) and `TileWallpaper=0` through `winreg`, and calls `SystemParametersInfoW`.
- `desktop.rs` is the Linux table. XFCE's row already takes the connected monitors' names, because the property xfdesktop reads is named after the monitor and does not exist until something creates it. The gsettings rows already write a fill mode (`picture-options` = `zoom`) before the image.
- `wallpaper_sink.rs` is the seam: `check_supported()`, `target_size()`, `publish(pixels, width, height)`. `CountingSink` is the test double.
- `engine/mod.rs` renders a wallpaper by asking the sink for a size and calling `renderer.export_image(width, height)`, which re-runs `write_uniforms` at that size. There is no quality-tier cap on the wallpaper render, unlike the preview.
- `camera.rs` builds the projection with `Mat4::perspective_rh(fov_y, aspect, ...)` and applies `offset_x`/`offset_y` as a post-projection translation. That translation is by a matrix, so it scales with the clip `w` and comes out as an exact constant shift in NDC, which is the same thing the sky shaders do with `screen_offset`. This matters below: it means the framing offset the span mode needs is already expressible in the parameters that exist.

## The model

### Monitors

One type, in `display.rs`, replacing nothing:

```rust
pub struct Monitor {
    /// Stable per connector, and the key a setter needs: the xrandr output
    /// name on Linux, the display device path on Windows.
    pub id: String,
    /// What the UI shows. Never used to address anything.
    pub label: String,
    /// Virtual-desktop coordinates in physical pixels, post-rotation.
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}
```

`display::monitors() -> Option<Vec<Monitor>>` keeps the same three-valued answer `outputs()` has: `None` where there is no way to ask, an empty list where the query answered with nothing usable, and a list otherwise. The distinction is already load-bearing in `wallpaper_sink::target_size` and stays that way.

On Linux this is `outputs()` with the name carried through as both `id` and `label`. On Windows it is the `EnumDisplayMonitors` walk that `get_primary_monitor_resolution` already does, keeping every monitor instead of one, with `rcMonitor` as the rectangle and `szDevice` (`\\.\DISPLAY1`) as the label. The `id` on Windows should be the device path from `IDesktopWallpaper::GetMonitorDevicePathAt`, matched to an `HMONITOR` by comparing `GetMonitorRECT(id)` against `rcMonitor`, because that path is what `SetWallpaper` takes and it is stable across reboots in a way `\\.\DISPLAY1` is not. Where the COM query fails, fall back to `szDevice` and let the sink refuse per-monitor work rather than address the wrong screen.

macOS keeps `None` and everything below degrades to today's behavior, which is a refusal before rendering.

### The anchor

One monitor is the anchor. It is the screen the "one screen" mode uses and the screen the span mode centers on, and it is stored as `Option<String>`: `None` means follow whatever the system calls primary, which is the default and what almost everyone will leave it at. A stored id that is no longer in the list falls back to the system primary, logs that it did, and says so in the status line rather than silently drawing somewhere else.

`display::primary_of` already ends with `.or_else(|| outputs.first())`, which is the right answer for a session where nothing is marked primary, and the anchor resolution should reuse it.

### The three modes

```rust
pub enum DisplayMode { OneScreen, EveryScreen, AcrossScreens }
```

- `OneScreen`: render the anchor at its own resolution. Other monitors are left as they are where the desktop can address them individually, and get the same image where it cannot. This is the closest thing to today and the escape hatch for anyone whose setup the other two modes get wrong.
- `EveryScreen`: the same view on every screen, rendered per monitor at that monitor's own size and aspect ratio, so nothing is scaled or letterboxed by the desktop. Default. On a single monitor it is identical to `OneScreen`, which is why no migration is needed.
- `AcrossScreens`: one continuous view over the whole virtual desktop, cut into one image per monitor. The anchor keeps the framing it would have had alone and the other screens continue the same view outward, which is what "extending the primary's field of view" means geometrically.

### Non-uniform layouts

The rules, all of them decided in pure code and tested with fabricated monitor lists:

- **Different resolutions.** Each monitor gets its own image at its own pixel size. Nothing is ever handed to a desktop at a size other than the screen it is for.
- **Different aspect ratios, `EveryScreen`.** The vertical field of view is the setting, so a wider screen sees more sky to the sides and the globe keeps its apparent height. That breaks down on a portrait screen, where a fixed vertical field of view runs the globe off the sides, so the rule is to contain: for `width >= height` the vertical fov is the setting; for `width < height` it is scaled so the horizontal extent equals what the vertical extent would have been, `tan(fov/2) * height / width`. The same rule applies to `sky_fov` in its quarter-angle form, because the sky lens is anchored to the vertical axis too.
- **Different heights, `AcrossScreens`.** The canvas is the bounding box, so a shorter monitor shows a band out of the middle of the view rather than a squashed copy of it. That is the geometrically honest answer and it is what a window onto a scene does.
- **Gaps and non-contiguous layouts.** Pixels in the bounding box that no monitor covers are rendered and discarded. Wasteful and correct; the alternative is per-tile rendering, which is below under what is not in this.
- **Rotation.** Both queries report post-rotation rectangles, so a portrait monitor is simply a tall rectangle here and needs no special case.
- **Mirrored monitors** (two entries with the same rectangle) get the same image twice and cost one render, because of the deduplication below.
- **Scaling and DPI.** Everything here is in physical pixels. On Linux xrandr reports physical pixels. On Windows `rcMonitor` is in virtual-screen coordinates, which are physical pixels only when the process is per-monitor DPI aware, and that is the single largest unknown in this plan. It is called out again under testing.
- **A layout that changed.** The monitor list is re-queried on every publish, not cached. There is no display-change subscription: `WM_DISPLAYCHANGE` and RandR events are both real and both a separate piece of work, and the auto-refresh timer means the layout is never stale for long.

### The geometry of `AcrossScreens`

This is the part worth writing down exactly, because it is where the mode either lands on the anchor or does not.

Let the anchor be `A` and the canvas `C` be the bounding box of every monitor rectangle, with size `(W, H)` and origin `(X, Y)`. The requirement is that the canvas render, cropped to the anchor's rectangle, is the render the anchor would have got alone. A perspective projection has a constant scale in tan-space at the image plane, so this reduces to keeping pixels per unit of tan-space equal between the two renders and moving the principal point:

- `k = H / A.height`
- `camera_fov_canvas = 2 * atan(tan(camera_fov / 2) * k)`
- `sky_fov_canvas = 4 * atan(tan(sky_fov / 4) * k)`, because the sky lens is stereographic and `sky_lens_edge_radius()` is `tan(sky_fov / 4)` with NDC y of ±1 mapped to it. The shader clamps `sky_fov` to `[60, 180]`, so this saturates at `k` around 1.43 for the default 140 degrees, and past that the sky no longer continues exactly. Log it when it clamps.
- The anchor's center in canvas pixels is `(cx, cy) = (A.x - X + A.width / 2, A.y - Y + A.height / 2)`, and in canvas NDC `(nx, ny) = (2 * cx / W - 1, 1 - 2 * cy / H)`.
- The framing offset becomes `offset_x_canvas = -nx + offset_x * A.width / W` and `offset_y_canvas = -ny + offset_y * A.height / H`, the second term carrying the user's own offset, which is in the anchor's NDC, into the canvas's. Signs against the existing convention (`mvp` translates by `-offset`) are for the implementer to confirm against the invariant test, which is the point of having one.
- Each monitor's crop is its rectangle minus the canvas origin.

Two consequences to write into the code as comments rather than discover later:

- For the common case of monitors side by side at equal height, `k` is 1: both fields of view are untouched, the canvas is simply wider, and the anchor's crop is exactly the standalone render. The mode costs nothing but pixels there.
- Where `k` is not 1, star sprites do not match: `sphere.wgsl` scales them by `clamp(viewport_size.y / 1080, 1, 2)` and sizes them in pixels against `viewport_size`, so a canvas taller than the anchor draws slightly larger stars than the anchor alone would. The globe, the atmosphere, the Sun and the Milky Way all follow the tan-space scaling and do match. This is a departure, not a bug to fix here.

## The render path

Nothing in the renderer or the shaders changes. `export_image(width, height)` already re-runs `write_uniforms` at an arbitrary size, and every mode above is expressible as a size plus a modified `SceneParams`.

- `EveryScreen` and `OneScreen`: one `export_image` per distinct render, where "distinct" means a distinct `(width, height)` after the contain rule, since the scene is otherwise identical. Two identical monitors cost one render and two files. `render_groups(&[Monitor], mode) -> Vec<(Size, Vec<MonitorId>)>` is a pure function and gets its own tests.
- `AcrossScreens`: one `export_image` at the canvas size, then crops. A crop is a row-wise copy out of the readback buffer, so it belongs next to the layout math as a pure function over `&[u8]`, not in the sink.

Guards, both checked before rendering and reported through the same status line every other refusal uses:

- `max_texture_dimension_2d`. The app requests `Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits())`, and `using_resolution` copies exactly the three resolution limits from the adapter, so the cap is the adapter's, typically 16384. Four 4K monitors side by side are 15360 wide and fit; five do not.
- `max_buffer_size`, which `using_resolution` does not raise and which the downlevel defaults set to 256 MiB. At four bytes per pixel plus row padding that is around 60 megapixels of canvas, which no real desktop reaches, but the check is one line and the failure without it is a wgpu panic rather than a message.
- A plain byte budget on the readback, logged. A 5120x1440 canvas is 29.5 MB, a 15360x2160 one is 133 MB, and `memory.rs` should see that as a transient. The soak test measures growth rather than peak, so a large transient is visible in the report and does not fail it.

## The publish path

### The contract

`WallpaperSink` stops trading in one image:

```rust
pub struct Frame { pub pixels: Vec<u8>, pub width: u32, pub height: u32 }

pub enum WallpaperJob {
    /// One image per monitor, each already at that monitor's own size.
    PerMonitor { images: Vec<(Monitor, Frame)> },
    /// One canvas over `bounds`; a monitor's image is the crop at its rect.
    Spanned { canvas: Frame, bounds: Rect, monitors: Vec<Monitor> },
}

pub trait WallpaperSink: Send + Sync {
    fn check_supported(&self) -> Result<(), String>;
    fn monitors(&self) -> Result<Vec<Monitor>, String>;
    fn publish(&self, job: &WallpaperJob) -> Result<(), String>;
}
```

`Spanned` carries the canvas rather than pre-cut crops because some desktops want the whole thing and cutting it for them would be wasted work: Windows has a span position and GNOME has a span picture-option. A sink that wants per-monitor images calls the shared `crop` helper for the monitors it addresses. The engine decides the mode; the sink decides how to realize it and is allowed to degrade, as long as it says what it did.

Files: one PNG per image, alternating as today so the path always changes, named `wallpaper-<slot>-<index>.png` with the slot alternating per publish and the index being the monitor's position in the layout, plus `wallpaper-<slot>-canvas.png` for a span. `published_wallpaper_file()` becomes `published_wallpaper_files() -> Vec<PathBuf>`, which the e2e case reads. Files left over from a layout with more monitors are deleted on publish, or they accumulate at full resolution.

### Windows

`SystemParametersInfoW` cannot address a monitor, so per-monitor work needs `IDesktopWallpaper`, which is Windows 8 and later and therefore always present on what this ships to.

Checked while writing this plan: `windows-sys` 0.59 has `CoCreateInstance` but generates no COM interfaces at all, so `IDesktopWallpaper` is not in it. The `windows` crate 0.62.2 has it (`CLSID` `c2cf3110-460e-4fc1-b9d0-8a1c0c9cc4bd`, `IID` `b92b56a9-8b55-4e14-9a89-0199bbb6f93b`) with `SetWallpaper`, `GetWallpaper`, `GetMonitorDevicePathCount`, `GetMonitorDevicePathAt`, `GetMonitorRECT`, `SetPosition` and `GetPosition`, all `unsafe fn` returning `windows_core::Result`. That crate is already in `Cargo.lock` at exactly that version as a transitive dependency, so taking a direct dependency on it for `Win32_UI_Shell` and `Win32_System_Com` adds a feature to a crate that is already compiled rather than a new tree. Every call site takes a scoped `#[allow(unsafe_code)]` and a `// SAFETY:` comment, which is the pattern the memory and monitor code already follows.

Then:

- `PerMonitor`: `SetPosition(DWPOS_FILL)` once, then `SetWallpaper(device_path, file)` per monitor. Images are already at native size, so the position only decides what happens if Windows disagrees about the size, and fill is the same choice `WallpaperStyle=10` makes today.
- `Spanned`: write the canvas once and either `SetPosition(DWPOS_SPAN)` with `SetWallpaper(null, canvas)`, which lets Windows do the cutting, or cut it here and take the `PerMonitor` path. Prefer letting Windows do it, because it is one file instead of N and because it is the path Windows itself keeps consistent when a monitor is unplugged. Verify against the mixed-DPI question below before committing to it; if Windows scales a span image per monitor under mixed scaling, cutting it here is the fallback and needs no new code.
- A single monitor keeps the `SystemParametersInfoW` path exactly as it is, so the one configuration that is regression-tested on every desktop and in the Hyper-V guest does not move.

`GetWallpaper(monitor_id)` is the read-back, and it is worth more than it looks: it is the first time the Windows setter can be asserted the way the Linux one already is, by asking the shell what it holds rather than trusting an exit code.

### Linux

Capability differs per desktop and the table should say so rather than the sink guessing:

```rust
enum Reach { PerMonitor, Spanned, OneImage }
```

- **XFCE** is `PerMonitor` and already almost there: the backdrop properties are named after the monitor, the row already lists them and already creates the ones a session lacks. Per-monitor images are the same walk with a different path per monitor instead of the same path for all.
- **GNOME, Cinnamon, MATE** are `Spanned`. There is no per-monitor wallpaper in any of them. What they do have is `picture-options`, where the row already writes `zoom`, and the schema's `spanned` value stretches one image across the whole virtual desktop. So `AcrossScreens` writes the canvas and sets `spanned`; the other two modes write the anchor's image and keep `zoom`.
- **KDE Plasma** is `OneImage` through `plasma-apply-wallpaperimage`, which sets every containment. Per-screen needs a plasmashell script over D-Bus (`evaluateScript` walking `desktops()` and writing `Image` per containment, with `d.screen` as the screen index), which means a new program on `PATH` (`dbus-send` or `qdbus`) and an ordering assumption between containment index and monitor. Neither is a thing to write blind. Plan: ship KDE as `OneImage` in the first pass, using the anchor's image for `OneScreen` and `EveryScreen` and the canvas for `AcrossScreens` (Plasma has no span mode, so the canvas will be zoomed per screen and look wrong, which is a reason to prefer sending it the anchor image and saying so). Then verify the script in the two-head guest and promote KDE to `PerMonitor` in a follow-up with the read-back to prove it.
- **LXQt** stays `OneImage`.
- A mode a desktop cannot reach is not a failure. The sink does the nearest thing and the status line says which, once, in the same voice as the existing refusals: something like "GNOME has one wallpaper for all monitors, so every screen got the same image".

### macOS

Unchanged: `check_supported` refuses before anything renders, and `monitors()` answers `None`. The refusal message already avoids naming a platform.

## The UI

Everything here uses vocabulary `main.slint` already has: `SettingCombo`, `SettingCheck`, `SettingHeading`, a `GroupBox`, and a Rust-provided model, of which `year-options` is the precedent.

A "Displays" group, placed under the wallpaper actions near the auto-refresh block rather than in the advanced panel, containing:

1. **Mode**, a `SettingCombo` with the three modes spelled as sentences: "One screen", "Same view on every screen", "One view across all screens".
2. **Screen**, a `SettingCombo` whose model comes from Rust: "Primary (automatic)" first, then one row per monitor, labelled like `DP-1  3440x1440  primary` on Linux and `Display 1  2560x1440` on Windows. It is the anchor for every mode, and its hint says so: which screen gets the picture in one-screen mode, and which screen the view is centered on in the other two.
3. **A layout diagram**, a small box drawing the monitor rectangles to scale with the anchor highlighted and each rectangle labelled with its index. The model is `[{ label, x, y, w, h, anchor }]` normalized to 0 to 1 in Rust, so the `.slint` side only multiplies by the box size and the arithmetic stays where it can be tested. This is the part that makes a wrong anchor or an unexpected layout obvious without a second monitor to look at.
4. The whole group hides itself when there is one monitor, because three modes that all mean the same thing is noise. The setting still persists, so unplugging and replugging a second screen does not lose it.

The preview window keeps rendering at its own aspect ratio and is not a wallpaper preview. Trying to make it one is a bigger change than this feature and the diagram covers the question the preview would have answered.

## Configuration, CLI and IPC

- `AppConfig` gains `display_mode: DisplayMode` (default `EveryScreen`) and `anchor_monitor: Option<String>` (default `None`). Neither is a shader parameter, so neither belongs in `SceneParams` or `ParamsDigest`; they follow `texture_resolution`'s precedent and travel on their own `EngineCommand::SetDisplayPlan { mode, anchor }`. `params.rs`'s table-driven digest test stays untouched, and a companion test asserts these two do not change the digest.
- A `displays` subcommand next to `render`: print the monitors as the app sees them, the resolved anchor, and the plan the current config would produce, image by image with its size, and with `--out <dir>` write those images without touching the desktop. It exists so that checking this on a borrowed two-screen Windows machine is thirty seconds rather than an afternoon, and so that a bug report can carry the layout.
- An IPC `displays` command answering one `SIGNAL:displays ...` line, in the shape `query-memory` already established, for the e2e cases to parse. Same reasoning as the note in CLAUDE.md that the single-line format is a contract.

## Testing

The question this feature has to answer is how much of Windows can be believed on the strength of a Linux guest. The answer depends on where the platform seam is put, which is why the seam is two functions.

### The layers that need no display at all

These run on all three operating systems, in CI, with no VM and no monitors. They are the majority of the feature.

- **Layout math**, in `display::layout`, against fabricated monitor lists: two side by side at equal height; two of different heights; a vertical stack; a portrait secondary; negative origins (a monitor left of or above the primary, which Windows gives freely); a gap between two; two identical rectangles (mirrored); one monitor; an empty list; a monitor with a zero dimension. Asserted: canvas bounds, per-monitor crop rectangles, the derived `camera_fov` and `sky_fov`, the derived offsets, the render grouping, and anchor resolution including a stored id that no longer exists.
- **The contain rule**, as its own set: a landscape monitor keeps the fov, a portrait one gets the horizontal extent the vertical one would have had, a square one is unchanged, and the round trip is monotonic in the base fov.
- **The span identity**, the invariant that gives the mode its meaning: with equal-height monitors, the anchor's derived fov, sky fov and offsets equal the standalone ones exactly. Pure, no GPU.
- **The span identity on the GPU**, in `tests/render_pipeline.rs`: render a two-monitor canvas, crop the anchor, render the anchor alone, and compare with the golden suite's tolerance. Equal heights so the star-sprite departure does not apply; a second case with unequal heights asserts the weaker invariant that the globe's silhouette centroid and radius match, because the sprites will not.
- **The crop helper**, against a synthetic buffer: crops tile the canvas, a crop at the far edge is not off by a row, a crop of the whole canvas is the canvas.
- **The publish plan**, in `tests/engine.rs` with a fake sink that reports a fabricated monitor list and records what it was handed. This is the one that proves the engine asks for the right images in each mode without any display anywhere, and it runs on Windows and macOS as well as Linux.
- **The desktop table**, in `desktop.rs`, for a two-monitor session per backend: XFCE writes a property per monitor, GNOME writes `spanned` for the span mode and `zoom` otherwise, KDE writes one command, and every row is asserted against a fabricated environment exactly as the table is tested today.
- **The UI logic**, in `tests/slint_ui.rs`: the monitor model reaches the combo, the anchor selection round-trips through the config, the diagram's normalized rectangles are right for a fabricated layout, and the group hides itself for one monitor.

### What the two-head Linux guest adds

The guest work is `--screens <n>` on `cargo xtask vm up` and `cargo xtask e2e`, up to four, and it is built. `docs/vm-setup.md` has the guide and `docs/vm-internals.md` the design; what the plan needs from it is three facts and one limit, each measured in the Debian guest on 2026-08-30 rather than reasoned about.

Three things on QEMU's command line make a screen, and two of them fail quietly. `max_outputs` sets the scanout count, which is how many connectors the guest's DRM driver creates. An `outputs` list giving each scanout its own `xres`/`yres` is what makes the guest see anything plugged into them: with `max_outputs` alone the second connector reads `disconnected` and the session has one screen, because virtio-gpu enables only output 0 at realize and nothing enables the rest until a UI says how big it is, which a VNC client connecting is not. And each `-vnc` server needs `display=<device id>` as well as `head=<n>`: `head=` alone parses, starts, and serves the first screen, which is two windows showing one screen in front of an extended desktop. A device id that does not exist is refused at startup, which is what makes naming it safe and omitting it not. With all three, both connectors come up connected and the Plasma session extends them side by side on its own; the boot places them explicitly anyway and reports what the session ended up with.

The limit is the pointer, and it bounds what interactive work in the guest can be. A guest with two screens has one absolute pointer for a desktop twice the width of either window, because QEMU scales a head's coordinates onto the whole absolute range using that head's width and adds no offset for where the head sits. A tablet per head, bound with `display=`/`head=`, is the obvious answer and is not available: unlike `-vnc`, an input device accepts a `display=` naming nothing at all, and with one tablet bound to each head, events from both VNC servers were measured arriving at the same device. So the boot maps the single tablet to the primary output with `xinput --map-to-output`, which makes the first screen's window click exactly where it points and leaves the others to look at. Their windows drive the first screen too.

That costs the image two packages, which is the one piece of this plan that needs a rebuild: `xinput`, without which the mapping cannot happen at all, and `arandr`, because the default Plasma session has no display settings UI in this minimal install while GNOME, XFCE and Cinnamon all have theirs. Until the rebuild the boot says in one line that the pointer covers the whole desktop, and everything else works.

With that, the guest can assert things no unit test can:

- `display::monitors()` in a real session returns two monitors with the rectangles the guest was configured with. This is the Linux half of the platform seam, and it is the half that is otherwise pure faith.
- Each desktop actually holds what the mode says it should: XFCE's per-monitor backdrop properties hold two different paths, GNOME's `picture-options` reads back `spanned` with the canvas path, and the existing rule that a setter's exit code is not evidence continues to apply.
- Two publishes in a row change every path, per monitor, which is the alternation rule the current case already enforces for one.
- A screendump of each head as a run artifact, so a person can look at the seam between the two screens in span mode, which is the one thing about this feature that no assertion captures. `screendump` takes a device id and a head, and the multi-screen display device carries the id `gpu` for exactly that; the first screen is also the default console, which needs no id.
- Cost: a span render on a software adapter in a VM is several times the single-screen cost, and the existing case already allows two minutes per publish. Measure it before assuming the budget holds.

### What only Windows can answer

Four questions, and they are the whole Windows-specific risk:

1. **Whether `rcMonitor` is physical pixels.** Virtual-screen coordinates are physical only when the process is per-monitor DPI aware, and what winit and Slint declare on our behalf has to be read, not assumed. Under mixed scaling with the wrong answer, every rectangle is wrong and every image is the wrong size. No Linux test can see this.
2. **The `HMONITOR` to device-path mapping.** `GetMonitorRECT` against `rcMonitor` is the documented way and it is exactly the kind of thing that works on one monitor and is ambiguous on two mirrored ones.
3. **Whether Windows persists per-monitor images**, and what it does with them when a monitor is unplugged and replugged.
4. **What `DWPOS_SPAN` does under mixed DPI**, which decides whether the canvas goes to Windows whole or is cut here.

None of these are inferable from Linux, so the plan is to make them cheap rather than to pretend otherwise. The `displays` subcommand answers 1 and 2 by printing what the app sees, in seconds, on any two-screen Windows machine. `IDesktopWallpaper::GetWallpaper` per monitor answers 3 as an assertion rather than an eyeball. Question 4 needs two screens with different scaling, which is the one case that needs real hardware or an indirect display driver in the Hyper-V guest; the Hyper-V synthetic adapter has no multi-monitor mode (`Set-VMVideo` has no monitor-count parameter, checked on this host) and enhanced-session multi-monitor needs the host to have the monitors, so a driver in the guest is the only route to an automated Windows multi-monitor case. That is its own piece of work and this feature should not wait for it.

The Hyper-V guest still earns its run: with one monitor it exercises the whole new path, including the COM plumbing, the id mapping and the read-back, for N = 1. What it cannot exercise is Windows' own N > 1 behavior.

### How certain this makes us

High confidence, from tests that need no display, that the model is right: the canvas, the crops, the derived fields of view, the framing, which image goes to which monitor in which mode, and that the anchor's crop is the anchor's own render. High confidence, from the Linux guest, that the mechanism works end to end against real desktops with real monitors. Low confidence, until someone runs `sunlit-earth displays` on a two-screen Windows box, about DPI and about the monitor-id mapping, because those are Windows facts and nothing else can stand in for them. Structuring the code so that exactly those two functions are the unknown is the best available answer, and the `displays` subcommand is what turns the remaining unknown into a thirty-second check.

## Implementation steps

### Step 1: the monitor list

- Files: `crates/sunlit-core/src/display.rs`, `crates/sunlit-core/src/wallpaper.rs`.
- `Monitor` and `monitors()`, Linux from `parse_outputs`, Windows from the existing `EnumDisplayMonitors` walk with every monitor kept. `get_primary_monitor_resolution` becomes a thin wrapper over the new list so there is one enumeration, not two.
- Tests: the parser cases already there, extended with two connected outputs; a Windows case asserting at least one monitor with a non-empty rectangle and exactly one primary; `monitors()` off Linux and Windows is `None`.
- Verify: `cargo unit`, `cargo clippy --all-targets`.

### Step 2: the layout math

- Files: new `crates/sunlit-core/src/display/layout.rs` (or a `layout` module inside `display.rs` if it stays small).
- `DisplayMode`, anchor resolution, `canvas_of`, `crops_of`, the contain rule, the span derivation, `render_groups`, and the `crop` helper over a pixel buffer.
- Tests: the whole list under "layers that need no display", every one of them pure.
- Verify: `cargo unit`.

### Step 3: the sink contract

- Files: `crates/sunlit-core/src/engine/wallpaper_sink.rs`, `crates/sunlit-core/src/wallpaper.rs`, `crates/sunlit-core/src/engine/mod.rs`.
- `Frame`, `WallpaperJob`, the new trait methods, the multi-file naming and the stale-file sweep, `published_wallpaper_files()`, `CountingSink` updated, and a new `RecordingSink` that reports a fabricated monitor list and records the job.
- The engine builds the job from the mode, the anchor and the monitor list, renders it, and publishes. `render_wallpaper_pixels` becomes `build_wallpaper_job`.
- Tests: the publish-plan cases in `tests/engine.rs` against `RecordingSink`, one per mode plus the single-monitor identity case.
- Verify: `cargo test -p sunlit-core --test engine`.

### Step 4: Windows

- Files: `crates/sunlit-core/Cargo.toml`, `crates/sunlit-core/src/wallpaper.rs`.
- The `windows` dependency with `Win32_UI_Shell` and `Win32_System_Com`, `IDesktopWallpaper` behind a small safe wrapper (`monitor_paths()`, `set(monitor, path)`, `set_span(path)`, `get(monitor)`), each unsafe block scoped and commented. `CoInitializeEx` on the engine thread once, and never on a thread that might already be in a different apartment.
- The single-monitor path keeps `SystemParametersInfoW` and the registry style write.
- Verify: `cargo test` on Windows, plus `cargo run -- displays` by hand.

### Step 5: Linux

- Files: `crates/sunlit-core/src/desktop.rs`, `crates/sunlit-core/src/engine/wallpaper_sink.rs`.
- `Reach` per row, `commands` taking the job instead of one path, XFCE per monitor, the gsettings rows choosing `zoom` or `spanned`, KDE and LXQt taking the one image the mode implies, and the degradation message.
- Tests: the table against fabricated two-monitor sessions, per row.
- Verify: `cargo unit`.

### Step 6: config, engine command, UI

- Files: `crates/sunlit-core/src/config.rs`, `crates/sunlit-core/src/engine/mod.rs`, `crates/sunlit-app/ui/main.slint`, `crates/sunlit-app/src/ui_callbacks.rs`.
- The two config fields with defaults and validation, `SetDisplayPlan`, the Displays group, the monitor model and the diagram model.
- Tests: config round-trip including an unknown mode string and a stored anchor that no longer exists; the `slint_ui.rs` cases; the digest test asserting neither field is a shader parameter.
- Verify: `cargo test`, then `cargo run` and look at it.

### Step 7: CLI and IPC

- Files: `crates/sunlit-app/src/main.rs`, `crates/sunlit-app/src/ipc.rs`.
- The `displays` subcommand with `--out`, and the IPC command with its single `SIGNAL:displays` line.
- Verify: `cargo run -- displays`, and the e2e case below.

### Step 0: the two-screen guest

Done, on `feat/multi-monitor`, before the app work: `cargo xtask vm up linux --screens <n>` and the same flag on `cargo xtask e2e`, one VNC window per screen, the outputs placed and reported. What is left of it is the image, which is the next step and the one thing here that costs a rebuild.

### Step 0b: two packages in the Linux image

- Files: `vm/linux/scripts/desktop.sh`, then `cargo xtask vm build-image linux` (the better part of an hour) and a `vm up linux --screens 2` to confirm.
- `xinput`, which `vm::map_pointer_command` needs and without which a two-screen guest's pointer covers the whole desktop and clicks at twice the x it was aimed at. The boot already says so in one line, so this changes a warning into a working pointer.
- `arandr`, a display settings UI that works in every session. The default Plasma session has none in this minimal install; GNOME, XFCE and Cinnamon already carry `gnome-control-center`, `xfce4-display-settings` and `cinnamon-settings`. Plasma's own module (`kscreen` plus `systemsettings`) is the alternative and costs tens of megabytes; `arandr` covers every session for about one.
- Both were installed by hand into a running guest to check they do the job before the image carries them. A guest is pristine on every boot, so that proof does not survive one.
- While the image is open: the roadmap's own guest items are cheapest to fix in the same rebuild, and none of them is this feature's business. Decide them separately rather than smuggling them in here.

### Step 8: the e2e cases and the guest

- Files: `crates/sunlit-app/tests/e2e.rs`, and `crates/xtask/src/**` for whatever the cases need beyond `--screens`.
- Cases: `test_displays_reports_the_session_layout` (every platform, one monitor or more); `test_set_wallpaper` extended to assert one file per monitor and that all of them change between two publishes; a span case that asserts the canvas file's size equals the bounding box; and the per-desktop read-back for two monitors on Linux.
- The suite drives the app over IPC and SSH, so the guest's one-pointer limit costs it nothing. What that limit does cost is a person clicking around a two-screen guest by hand, which is worth remembering when a case is written to be watched rather than asserted.
- Verify: `cargo e2e` on the desktop, then `cargo xtask e2e --target linux --screens 2` and `--target windows`.

### Step 9: docs

- `docs/rendering.md` gets the span geometry and the star-sprite departure. `docs/architecture.md` gets the sink contract and the new `EngineCommand`. `docs/platforms.md` gets the per-desktop reach table and the Windows COM path. `docs/testing.md` gets the two-head guest and what it does and does not prove. `docs/roadmap.md` gets the item closed with a link to this plan and the open Windows questions listed as their own item. `README.md` gets the `displays` subcommand and the new settings.

## Not in this

- **Per-tile rendering.** Everything above renders the whole bounding box in span mode. Rendering each monitor separately would skip the gaps and lift the texture-size cap, but it needs the sky shaders to distinguish the canvas they reconstruct directions from and the tile they are rasterizing into, which is a new pair of uniforms and a change at every site that converts pixels to NDC. Worth doing when someone has a layout the cap refuses, not before.
- **A different scene per monitor**, one continent per screen. It is a real feature and it needs per-monitor `SceneParams`, which is a config and UI change several times this one.
- **Bezel correction**, the gap in the image that makes a continuous view line up across the physical frames. Cheap to add later as a per-monitor inset once the layout math exists, and easy to get wrong without the monitors in front of you.
- **Display-change events.** Re-querying on publish is enough while the auto-refresh runs.
- **A wallpaper preview of the whole layout** in the settings window. The diagram is the cheap 80 percent.
- **macOS**, which has no setter at all yet.

## Open questions

1. Is the process per-monitor DPI aware, and is `rcMonitor` therefore physical pixels? Read what winit and Slint declare before writing the Windows monitor query, because the answer decides whether a scaling factor belongs in the layout math.
2. Does `DWPOS_SPAN` do the right thing under mixed DPI, or should the canvas be cut here? Decide with a real two-screen machine; the fallback needs no new code.
3. Does the Plasma containment index line up with the monitor order, and is `dbus-send` an acceptable new dependency for the KDE row? Answer in the two-head guest before promoting KDE past `OneImage`.
4. ~~Does the guest's X session bring the second virtio head up connected, and does the desktop extend rather than mirror it?~~ Answered on 2026-08-30: with a size on each scanout both connectors come up connected, and the Plasma session extends them without being asked. The other three sessions are unobserved, which is why the boot places the outputs itself and reports the result. Note that the fixture in `display.rs` describes a guest with a disconnected `Virtual-2`, which is what the old single-screen guest reported and is worth keeping as the case it is.
5. Is `EveryScreen` the right default, or should a session that has never been configured stay on `OneScreen` and let the user opt in? `EveryScreen` is a strictly better version of what a multi-monitor Windows desktop already does with a fill-style wallpaper, which is the argument for it being the default, but it also quietly starts painting screens the app has never touched before.
