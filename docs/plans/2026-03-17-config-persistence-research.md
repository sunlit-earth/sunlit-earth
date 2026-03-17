# Research: Config Persistence (2026-03-17)

## Problem Statement

Sunlit Earth has 14 user-configurable settings (camera position,
orientation, framing, rendering options, lighting) that reset to
hardcoded defaults every time the app launches. Users who adjust the
globe view lose their configuration on restart. The goal is to persist
these settings to disk and restore them automatically on launch.

## Requirements

1. **Persist all 14 user-configurable settings** across application
   restarts
2. **Automatic save** -- no explicit "Save" button; settings persist
   transparently
3. **Graceful degradation** -- missing, corrupt, or out-of-range
   config values fall back to defaults without crashing
4. **Forward and backward compatibility** -- old binaries tolerate
   new fields; new binaries tolerate missing fields
5. **Human-readable config file** -- users who find the file can
   read and edit it
6. **Consistent file location** -- config lives alongside the
   existing wallpaper export in `%LOCALAPPDATA%\SunlitEarth\`
7. **Atomic writes** -- partial writes must not corrupt the config
   file

## Findings

### Current Settings Architecture

Settings exist in three runtime layers with no centralized struct:

1. **Slint UI properties** (`ui/main.slint`) -- the authoritative
   runtime source. Every slider, combobox, and checkbox is bound to
   a named property on `MainWindow`.
2. **`CameraParams` struct** (`src/scene/camera.rs`) -- a transient
   struct rebuilt from UI state each frame; never stored
   persistently. Groups the 8 camera values.
3. **`FrameState` struct** (`src/renderer/frame.rs`) -- a quantized
   snapshot of all rendering inputs used for dirty-checking.

The data flow is unidirectional from Slint to Rust during rendering.
The only reverse flow is mouse drag/scroll callbacks and the Reset
Camera button, where Rust writes back to Slint via `win.set_*()`
calls.

Defaults are duplicated in two places that must stay in sync
manually: `CameraParams::default()` in Rust and Slint property
initializers in `ui/main.slint`. A config persistence system should
establish a single authoritative source for defaults.

### Complete Settings Inventory

14 user-configurable settings, organized by UI group:

**Camera Position (3):** longitude (`f32`, -180..180, default 0),
latitude (`f32`, -89..89, default 30), zoom (`f32`, 0.0..1.0,
default ~0.421 via `distance_to_zoom(8.0)`).

**Camera Orientation (3):** tilt (`f32`, -180..180, default 0),
yaw (`f32`, -90..90, default 0), pitch (`f32`, -90..90, default 0).

**Framing (2):** offset_x (`f32`, -1.0..1.0, default 0),
offset_y (`f32`, -1.0..1.0, default 0).

**Rendering (2):** texture_index (`int`, 0-3, default 3 =
Day/Night Blend), MSAA sample count (`int`, GPU-dependent,
prefers 8x).

**Lighting (4):** terminator_width (`f32`, 0.01..0.3, default
0.1), diffuse_shading (`bool`, default true), diffuse_floor
(`f32`, 0.0..1.0, default 0.50), diffuse_ramp (`f32`, 0.1..1.0,
default 0.25).

**Not persisted** (derived or runtime-only): window size, splitter
position, renderer info, wallpaper status, loading text, sun
direction, zoom display distance.

### The `out` Property Problem

The four lighting settings (`terminator-width`, `diffuse-shading`,
`diffuse-floor`, `diffuse-ramp`) are declared as `out` properties on
`MainWindow` -- meaning Rust can read but not write them. They are
bound to slider/checkbox values via direct expressions. To restore
saved lighting settings at startup, these must be changed to `in-out`
with `<=>` bidirectional bindings, matching the existing pattern used
by camera properties. This is a prerequisite change.

### AA Index vs Sample Count

The AA combobox index is machine-dependent (it maps into a
dynamically built list of GPU-supported sample counts). The config
file must store the desired **sample count** (e.g., 8), not the
index. At load time, the app finds the matching index or falls back
to the best available. The texture index (0-3) is stable and safe to
persist directly.

### Config Format

TOML is the recommended format. It is the de facto standard for Rust
application configuration, supports comments, has excellent parse
error messages with line/column info, and maps cleanly to the flat
struct of camera and render parameters. The `toml` crate (v0.9+)
provides first-class serde integration.

Alternatives evaluated and rejected:

- **JSON** -- no comment support, trailing comma restrictions, not
  idiomatic for Rust user-facing config
- **RON** -- low adoption outside Rust ecosystem, unfamiliar to
  non-developer end users, no advantage for flat scalar configs
- **YAML** -- whitespace-sensitive parsing, implicit type coercion
  gotchas, overkill for flat settings

### Config File Location

`%LOCALAPPDATA%\SunlitEarth\config.toml` -- reuses the existing app
data directory established by `wallpaper.rs` for wallpaper export
(`%LOCALAPPDATA%\SunlitEarth\wallpaper.png`). The config and
wallpaper files become siblings.

`%LOCALAPPDATA%` (not `%APPDATA%`) is correct because the config is
inherently tied to the local machine's display setup. Roaming the
config via Active Directory would apply settings designed for a
different display.

For cross-platform resolution, `dirs::data_local_dir()` returns the
appropriate base path on each platform:

| Platform | Base path                       | Config subpath             |
| -------- | ------------------------------- | -------------------------- |
| Windows  | `%LOCALAPPDATA%`                | `SunlitEarth\config.toml`  |
| Linux    | `$HOME/.local/share`            | `SunlitEarth/config.toml`  |
| macOS    | `~/Library/Application Support` | `SunlitEarth/config.toml`  |

Note: on Linux, `dirs::config_local_dir()` (`~/.config`) would be
more semantically correct. This can be revisited when cross-platform
support is added.

### Save Strategy

**Debounced save (~1-2 seconds after last change) combined with
save-on-exit.**

- Save on every change is impractical -- continuous slider drags
  produce dozens of changes per second
- Save-on-exit alone is risky for an app that may run indefinitely
  as a background process (system reboots would lose all changes)
- Debouncing at ~1s means the save fires shortly after the user
  stops dragging a slider
- Save-on-exit as a backstop covers the case where the debounce
  timer has not yet fired
- The app already has a 2-minute Slint timer for sun position
  redraws; a config-save timer fits naturally alongside it
- No UI feedback needed ("Saved" indicators are appropriate for
  document editors, not settings panels)

### Atomic Write Pattern

Config writes must be atomic to prevent corruption from partial
writes (e.g., process killed mid-write):

1. Write to `config.toml.tmp` in the same directory
2. Rename over `config.toml`

The rename operation is atomic on both Windows NTFS and Linux
ext4/XFS. `std::fs::write` + `std::fs::rename` is sufficient for a
file this small; the `tempfile` crate is unnecessary.

### Application Lifecycle Integration

**Load point:** Between window creation (step 4 in startup) and
deferred index setup (step 10). After loading, call
`win.set_camera_longitude()` etc. for each saved value. The deferred
index setup must use saved values instead of hardcoded defaults.

**Save-on-exit point:** Between `window.run()` return and the
existing `std::process::exit(0)` call on line 217 of `main.rs`. The
hard exit prevents any cleanup code after it, so the save must happen
in that narrow window.

**Change detection hooks:** The existing `sliders_changed`,
`msaa_changed`, `texture_changed`, mouse drag, and mouse scroll
callbacks are natural places to trigger a debounce timer reset.

### Serde Resilience Pattern

The config struct should use `#[serde(default)]` at the struct level
so that missing fields are filled from `Default::default()`. This
handles both forward compatibility (old binary reads new config with
extra fields, which are silently ignored) and backward compatibility
(new binary reads old config missing new fields, which get defaults).

`#[serde(deny_unknown_fields)]` must NOT be used -- it breaks forward
compatibility and has documented bugs when combined with
`#[serde(flatten)]`.

`#[serde(alias = "old_name")]` provides zero-cost backward
compatibility when fields are renamed.

Schema versioning via a `schema_version: u32` field is available for
future structural migrations but is not needed for the initial
implementation.

### Crate Evaluation

**`toml` + `dirs` (recommended):** Minimal dependencies, full
control over path/format/error handling/atomic write, consistent with
existing filesystem patterns. ~15 lines of glue code.

**`confy` (evaluated, not recommended):** Uses `%APPDATA%` (Roaming)
on Windows by default, which is semantically wrong for display-tied
settings. The `load_path`/`store_path` workaround eliminates most of
confy's value. No atomic write. No debounce.

**`config-rs` (rejected):** No write/save support. GitHub issue #176
open since 2019 with no resolution.

**`figment` (rejected):** No write support. Designed for server
applications with multiple config sources.

## External Research

All external findings are from web research conducted on 2026-03-17.
Confidence is high for format comparison and crate evaluation
(well-documented ecosystem); medium for save strategy (based on UI
pattern best practices rather than Rust-specific sources).

Key sources:

- Serde documentation (serde.rs) for `#[serde(default)]`,
  `deny_unknown_fields`, and `flatten` behavior
- confy GitHub repository and docs.rs for API evaluation and Windows
  path behavior
- config-rs GitHub issue #176 for write support status
- Advanced Installer documentation for `%APPDATA%` vs
  `%LOCALAPPDATA%` semantics
- Rust Forum threads on config format choice and atomic file writes

## Technical Constraints

- **No existing serialization dependencies** -- adding config
  persistence requires `serde`, `toml`, and `dirs` as new
  dependencies
- **`unsafe_code = "deny"`** -- the config module should not need
  unsafe code
- **`std::process::exit(0)` on shutdown** -- save-on-exit must
  happen before this call; no cleanup runs after it
- **Lighting `out` properties** -- four Slint properties must be
  changed from `out` to `in-out` before they can be restored at
  startup
- **AA index is GPU-dependent** -- must store sample count, not
  index, and resolve at load time
- **Slint deferred index setup** -- `aa_index` and `texture_index`
  are set via `invoke_from_event_loop` after model changes; config
  restore must integrate with this timing

## Open Questions

1. **Window geometry persistence** -- window size and splitter
   position are not currently exposed as `in-out` Slint properties.
   Should they be persisted? This would require additional Slint
   property changes.
2. **Update interval** -- the external research includes
   `update_interval_secs` as a potential setting. The codebase
   currently hardcodes a 2-minute sun timer. Should this be
   user-configurable?
3. **Config migration testing** -- how should forward/backward
   compatibility be tested? Property-based tests with randomly
   omitted/added fields would be thorough but may be overkill for
   an initial implementation.
4. **Linux `config_local_dir` vs `data_local_dir`** -- when
   cross-platform support is added, should the config use
   `~/.config` (XDG config) or `~/.local/share` (XDG data)?
   Semantically, `~/.config` is more correct for a config file.
5. **Debounce timer implementation** -- should the debounce use a
   Slint timer (staying in the UI thread) or a separate thread? A
   Slint timer is simpler and avoids synchronization but depends on
   the event loop being responsive.

## Recommendations

1. **Use `toml` + `serde` + `dirs`** as the dependency set. Skip
   `confy` -- its path defaults are wrong for this project and the
   workarounds negate its simplicity.

2. **Create an `AppConfig` struct** with
   `#[derive(Serialize, Deserialize, Default)]` and
   `#[serde(default)]`. This struct becomes the single source of
   truth for default values, replacing the current duplication
   between `CameraParams::default()` and Slint property initializers.

3. **Store at `%LOCALAPPDATA%\SunlitEarth\config.toml`** using
   `dirs::data_local_dir()` for path resolution, reusing the
   existing app data directory.

4. **Change the four lighting properties** from `out` to `in-out` in
   `ui/main.slint` as a prerequisite, with `<=>` bidirectional
   bindings matching the camera property pattern.

5. **Persist MSAA as sample count, not index.** At load time, find
   the matching index in the GPU's supported counts or fall back to
   the best available.

6. **Implement debounced save** (~1s after last change) using a
   Slint timer, plus a save-on-exit call between `window.run()`
   return and `std::process::exit(0)`.

7. **Use atomic writes** (write `.toml.tmp`, rename to `.toml`) for
   every save operation.

8. **Load config early in startup** (after window creation, before
   callback registration) and apply values via `win.set_*()` calls,
   with the deferred index setup using saved values instead of
   hardcoded defaults.

## Sources

- `2026-03-17-config-persistence-codebase.md` -- runtime settings
  flow, UI binding, application lifecycle, settings inventory
- `2026-03-17-config-persistence-external.md` -- format comparison,
  file location, save strategy, serde patterns, crate evaluation
