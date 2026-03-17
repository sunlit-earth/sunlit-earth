# Plan: Config Persistence (2026-03-17)

## Summary

Persist all 14 user-configurable settings (camera position,
orientation, framing, rendering options, lighting) to a TOML file
at `%LOCALAPPDATA%\SunlitEarth\config.toml`. Settings are loaded
at startup, saved automatically via a debounced Slint timer after
any UI change, and saved on exit. The implementation adds three new
dependencies (`serde`, `toml`, `dirs`), a new `config` module,
Slint property changes for lighting bidirectionality, and
integration into the existing `main.rs` startup/callback/exit flow.

## Stakes Classification

**Level**: Medium
**Rationale**: The change touches multiple files (Cargo.toml,
lib.rs, main.rs, main.slint, new config module) and introduces new
dependencies, but it is well-isolated from the GPU rendering
pipeline. All changes are additive --- no existing rendering
behavior changes. A failed config load falls back to existing
defaults, making the failure mode benign. Rollback is
straightforward: revert the commits.

## Context

**Research**:
`docs/plans/2026-03-17-config-persistence-research.md`
**Supporting research**:
`docs/plans/2026-03-17-config-persistence-codebase.md`,
`docs/plans/2026-03-17-config-persistence-external.md`

**Affected Areas**:

- `Cargo.toml` --- new dependencies
- `src/lib.rs` --- new module declaration
- `src/config.rs` --- new module (config struct, load, save,
  path resolution)
- `ui/main.slint` --- four lighting properties change from
  `out` to `in-out`
- `src/main.rs` --- config load at startup, debounce timer,
  save-on-exit, config-aware callbacks

## Success Criteria

- [ ] App starts with no config file and uses defaults
      (identical to current behavior)
- [ ] Changing any of the 14 settings triggers a debounced
      save (~1s) to `config.toml`
- [ ] Closing and reopening the app restores all 14 settings
      from config
- [ ] Corrupt or partially invalid config falls back to
      defaults without crashing
- [ ] Config file with extra unknown fields loads without
      error (forward compatibility)
- [ ] Config file with missing fields fills them from defaults
      (backward compatibility)
- [ ] MSAA sample count in config resolves to nearest
      available on a different GPU
- [ ] Atomic write prevents corruption: `config.toml~` is
      written first, then renamed
- [ ] `cargo clippy` passes with no new warnings
- [ ] All existing tests continue to pass
- [ ] New unit tests cover config serialization,
      deserialization, defaults, and edge cases

## Implementation Steps

### Phase 1: Dependencies and Slint Property Changes

#### Step 1.1: Add serde, toml, and dirs dependencies

- **Files**: `Cargo.toml`
- **Action**: Add `serde`, `toml`, and `dirs` to
  `[dependencies]`:
  - `serde = { version = "1", features = ["derive"] }`
  - `toml = "0.9"`
  - `dirs = "6"`
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

#### Step 1.2: Change lighting properties from `out` to `in-out`

- **Files**: `ui/main.slint:43-46`
- **Action**: Change the four lighting properties from `out` to
  `in-out` with `<=>` bidirectional bindings to their respective
  widget values. Specifically:
  - `terminator-width` becomes `in-out` with default `0.1`;
    slider gets `value <=> root.terminator-width;`
  - `diffuse-shading` becomes `in-out` with default `true`;
    checkbox gets `checked <=> root.diffuse-shading;`
  - `diffuse-floor` becomes `in-out` with default `0.50`;
    slider gets `value <=> root.diffuse-floor;`
  - `diffuse-ramp` becomes `in-out` with default `0.25`;
    slider gets `value <=> root.diffuse-ramp;`
- **Test cases** (manual):
  - App launches with same default lighting values as before
    (terminator 0.1, diffuse on, floor 0.50, ramp 0.25)
  - Moving lighting sliders still triggers `sliders-changed`
    and causes redraws
  - Lighting slider display text still updates correctly
- **Verify**: `cargo build` succeeds, `cargo clippy` clean,
  manual verification of lighting defaults and slider behavior
- **Complexity**: Small

### Phase 2: Config Module --- Struct and Serialization

#### Step 2.1: Write tests for AppConfig defaults and serde (RED)

- **Files**: `src/config.rs` (new file, tests section)
- **Action**: Write failing tests for the `AppConfig` struct
  before implementing it. Tests should cover:
  - **`default_values_match_camera_params`**:
    `AppConfig::default()` camera fields match
    `CameraParams::default()` values (longitude 0, latitude 30,
    zoom ~0.421, offsets 0, tilt/yaw/pitch 0)
  - **`default_values_lighting`**:
    `AppConfig::default()` lighting fields match current Slint
    defaults (terminator_width 0.1, diffuse_shading true,
    diffuse_floor 0.50, diffuse_ramp 0.25)
  - **`default_values_rendering`**:
    `AppConfig::default()` rendering fields match current
    defaults (texture_index 3, sample_count 8)
  - **`serde_round_trip`**: serialize an `AppConfig` to TOML
    and deserialize back, assert all 14 fields are preserved
  - **`serde_round_trip_non_default`**: same but with
    non-default values for every field
  - **`deserialize_empty_string`**: empty TOML string `""`
    deserializes to `AppConfig::default()`
    (via `#[serde(default)]`)
  - **`deserialize_missing_fields`**: TOML with only
    `longitude = 42.0` fills all other fields from default
  - **`deserialize_unknown_fields_ignored`**: TOML with
    `future_field = true` deserializes without error
  - **`deserialize_invalid_toml`**: malformed TOML returns
    an error (not panic)
- **Verify**: Tests exist but fail to compile (no `AppConfig`
  struct yet)
- **Complexity**: Small

#### Step 2.2: Implement AppConfig struct (GREEN)

- **Files**: `src/config.rs` (new file), `src/lib.rs`
- **Action**: Create the `AppConfig` struct with:
  - `#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]`
  - `#[serde(default)]` at struct level
  - 14 fields: `longitude`, `latitude`, `zoom`, `offset_x`,
    `offset_y`, `tilt`, `yaw`, `pitch` (all `f32`),
    `texture_index` (`i32`), `sample_count` (`u32`),
    `terminator_width`, `diffuse_floor`, `diffuse_ramp`
    (all `f32`), `diffuse_shading` (`bool`)
  - `impl Default` with values matching current
    `CameraParams::default()` + Slint lighting defaults +
    rendering defaults (texture_index: 3, sample_count: 8)
  - Add `pub mod config;` to `src/lib.rs`
- **Verify**: All tests from Step 2.1 pass
- **Complexity**: Small

#### Step 2.3: Write tests for config file I/O (RED)

- **Files**: `src/config.rs` (tests section)
- **Action**: Write failing tests for `load_config` and
  `save_config` functions:
  - **`load_from_nonexistent_returns_default`**: loading
    from a path that does not exist returns
    `AppConfig::default()`
  - **`save_and_load_round_trip`**: save a non-default config
    to a temp file, load it back, assert equality
  - **`save_creates_parent_directory`**: save to a path whose
    parent directory does not exist, verify it creates the
    directory and succeeds
  - **`save_atomic_write_uses_tilde`**: after saving, the
    final file exists at the target path (not the tilde path);
    this validates the rename completed
  - **`load_corrupt_file_returns_default`**: write invalid
    TOML to a file, load returns default (with eprintln
    warning, not a crash)
  - **`load_partial_file_fills_defaults`**: write TOML with
    only `longitude = 99.0`, load returns config with
    longitude 99.0 and all other fields at default
- **Verify**: Tests exist but fail to compile (no
  `load_config`/`save_config` yet)
- **Complexity**: Small

#### Step 2.4: Implement load_config and save_config (GREEN)

- **Files**: `src/config.rs`
- **Action**: Implement:
  - `pub fn config_path() -> Option<PathBuf>` --- returns
    `dirs::data_local_dir()?.join("SunlitEarth")
    .join("config.toml")` (platform-independent path
    resolution; on Windows this resolves to
    `%LOCALAPPDATA%\SunlitEarth\config.toml`)
  - `pub fn load_config() -> AppConfig` --- reads from
    `config_path()`, parses TOML, returns default on any
    error (missing file, parse error, permission error) with
    an `eprintln!` warning for parse errors. Missing file is
    silent (expected on first run).
  - `pub fn save_config(config: &AppConfig)` --- serializes
    to TOML, writes to `config_path()~` (tilde suffix),
    renames to `config_path()`. Creates parent directory if
    needed via `fs::create_dir_all`. Errors are logged via
    `eprintln!` but never propagated (config save failure
    must not crash the app).
- **Verify**: All tests from Step 2.3 pass
- **Complexity**: Small

#### Step 2.5: Write test for sample count resolution (RED)

- **Files**: `src/config.rs` (tests section)
- **Action**: Write failing test for
  `find_sample_count_index`:
  - **`find_sample_count_exact_match`**: given counts
    `[1, 2, 4, 8]` and desired `4`, returns index `2`
  - **`find_sample_count_missing_falls_back`**: given
    counts `[1, 2, 4]` and desired `8`, returns index `2`
    (highest available)
  - **`find_sample_count_one_returns_zero`**: given counts
    `[1]` and desired `8`, returns index `0`
  - **`find_sample_count_exact_match_8x`**: given counts
    `[1, 2, 4, 8]` and desired `8`, returns index `3`
- **Verify**: Tests exist but fail to compile
- **Complexity**: Small

#### Step 2.6: Implement find_sample_count_index (GREEN)

- **Files**: `src/config.rs`
- **Action**: Implement
  `pub fn find_sample_count_index(aa_counts: &[u32],
  desired: u32) -> i32` --- finds the index of `desired` in
  `aa_counts`, or falls back to the last index (highest
  available count). Returns the index as `i32` for direct use
  with `win.set_aa_index()`.
- **Verify**: All tests from Step 2.5 pass
- **Complexity**: Small

### Phase 3: Main Integration

#### Step 3.1: Load config at startup and apply to UI

- **Files**: `src/main.rs:40-84`
- **Action**: After window creation (line 40) and before the
  deferred `invoke_from_event_loop` (line 77):
  1. Call `let config = sunlit_earth::config::load_config();`
  2. Apply all 14 saved values to the window via `win.set_*()`
     calls:
     - Camera: `set_camera_longitude`,
       `set_camera_latitude`, `set_camera_zoom`,
       `set_camera_offset_x`, `set_camera_offset_y`,
       `set_camera_tilt`, `set_camera_yaw`,
       `set_camera_pitch`
     - Lighting: `set_terminator_width`,
       `set_diffuse_shading`, `set_diffuse_floor`,
       `set_diffuse_ramp`
     - Texture: `set_texture_index` (directly, before the
       deferred call)
  3. Modify the deferred `invoke_from_event_loop` closure to
     use `config.texture_index` instead of hardcoded
     `blend_mode_index` and
     `find_sample_count_index(&aa_counts, config.sample_count)`
     instead of `aa_default`
- **Test cases** (manual):
  - With no config file: app starts with same defaults as
    before
  - With a config file containing `longitude = 90.0`: app
    starts with globe rotated to 90 degrees
  - With a config file containing `sample_count = 4` on a
    GPU supporting `[1, 2, 4, 8]`: AA combobox shows
    "MSAA 4x" selected
  - With a config file containing `sample_count = 16` on a
    GPU supporting `[1, 2, 4, 8]`: AA falls back to 8x
    (highest available)
- **Verify**: `cargo build` succeeds, `cargo clippy` clean,
  manual verification
- **Complexity**: Medium

#### Step 3.2: Create function to read config from UI state

- **Files**: `src/main.rs` or `src/config.rs`
- **Action**: Add a helper function
  `pub fn read_from_window(win: &MainWindow) -> AppConfig` on
  `AppConfig` (or as a standalone function) that reads all 14
  properties from the Slint window and returns an `AppConfig`.
  This is used by both the debounce save and save-on-exit
  paths. Fields read:
  - `get_camera_longitude()`, `get_camera_latitude()`,
    `get_camera_zoom()`
  - `get_camera_offset_x()`, `get_camera_offset_y()`
  - `get_camera_tilt()`, `get_camera_yaw()`,
    `get_camera_pitch()`
  - `get_texture_index()`
  - `get_aa_index()` --- looked up via `aa_counts[idx]` to
    get the sample count
  - `get_terminator_width()`, `get_diffuse_shading()`,
    `get_diffuse_floor()`, `get_diffuse_ramp()`
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

#### Step 3.3: Add debounced save timer and save-on-exit

- **Files**: `src/main.rs`
- **Action**:
  1. Create a `slint::Timer` for config save debouncing
     (similar to the existing `sun_timer`). Store it
     alongside `sun_timer` so it stays alive.
  2. In each of the five change callbacks
     (`on_sliders_changed`, `on_msaa_changed`,
     `on_texture_changed`, `on_mouse_drag`,
     `on_mouse_scroll`), after the existing
     `request_redraw()` call, restart the debounce timer
     with a 1-second `SingleShot` duration. The timer
     callback reads current state from the window and calls
     `save_config()`.
  3. Also restart the debounce timer in `on_reset_camera`
     (resetting to defaults should be saved).
  4. After `window.run()` returns (line 211) and before
     `drop(sun_timer)` (line 214), read the current config
     from the window and call `save_config()` as a final
     save-on-exit.
  5. The debounce timer's callback needs a `window_weak`
     clone and a clone of `aa_counts`. Each callback that
     restarts the timer simply calls
     `config_timer.restart()`.
- **Test cases** (manual):
  - Move a slider, wait 2 seconds, check that
    `config.toml` has been created/updated
  - Move a slider, immediately close the app, reopen ---
    setting is preserved (save-on-exit backstop)
  - Rapidly drag a slider for 3 seconds, release, wait
    2 seconds --- only one config write occurs (not dozens)
  - Change AA dropdown --- saved as sample count, not index
  - Click "Reset Camera" --- defaults are saved to config
- **Verify**: `cargo build` succeeds, `cargo clippy` clean,
  manual verification of save behavior
- **Complexity**: Medium

### Phase 4: Verification

#### Step 4.1: Run full test suite and clippy

- **Files**: N/A
- **Action**: Run `cargo test` and `cargo clippy` to ensure
  no regressions
- **Verify**: All tests pass, no clippy warnings
- **Complexity**: Small

#### Step 4.2: End-to-end manual verification

- **Files**: N/A
- **Action**: Full manual test sequence:
  1. Delete `%LOCALAPPDATA%\SunlitEarth\config.toml` if it
     exists
  2. Launch app --- verify all defaults are correct
  3. Change every setting (all 14): longitude, latitude,
     zoom, tilt, yaw, pitch, offset X, offset Y, texture,
     AA, terminator, diffuse toggle, floor, ramp
  4. Wait 2 seconds --- verify `config.toml` exists and
     contains all 14 settings
  5. Close and reopen the app --- verify all 14 settings
     are restored
  6. Edit `config.toml` by hand: add
     `unknown_future_field = 42` --- verify app starts
     without error
  7. Edit `config.toml` to contain only
     `longitude = 120.0` --- verify app starts with
     longitude 120, all other settings at defaults
  8. Replace `config.toml` contents with `{{{invalid` ---
     verify app starts with defaults and prints a warning
     to stderr
  9. Delete `config.toml` --- verify app starts with
     defaults (no error)
- **Verify**: All manual checks pass
- **Complexity**: Small

## Test Strategy

### Automated Tests

<!-- markdownlint-disable MD013 -->

| Test Case | Type | Input | Expected Output |
| --- | --- | --- | --- |
| Default values match CameraParams | Unit | `AppConfig::default()` | Camera fields match `CameraParams::default()` |
| Default lighting values | Unit | `AppConfig::default()` | terminator 0.1, diffuse true, floor 0.50, ramp 0.25 |
| Default rendering values | Unit | `AppConfig::default()` | texture_index 3, sample_count 8 |
| Serde round-trip (default) | Unit | Serialize + deserialize default | Identity |
| Serde round-trip (non-default) | Unit | Serialize + deserialize modified | Identity |
| Deserialize empty string | Unit | `""` | `AppConfig::default()` |
| Deserialize missing fields | Unit | `"longitude = 42.0"` | longitude 42.0, rest default |
| Deserialize unknown fields | Unit | `"future_field = true"` | No error, default config |
| Deserialize invalid TOML | Unit | `"{{{invalid"` | Error (not panic) |
| Load nonexistent file | Unit | Missing path | `AppConfig::default()` |
| Save and load round-trip | Unit | Non-default config | Identity after save + load |
| Save creates parent dir | Unit | Path with missing parent | Dir created, save succeeds |
| Atomic write completes | Unit | Save to path | Final file exists (not tilde) |
| Load corrupt file | Unit | Invalid TOML in file | `AppConfig::default()` |
| Load partial file | Unit | `"longitude = 99.0"` in file | longitude 99.0, rest default |
| find_sample_count exact | Unit | counts=[1,2,4,8], desired=4 | index 2 |
| find_sample_count fallback | Unit | counts=[1,2,4], desired=8 | index 2 (highest) |
| find_sample_count minimal | Unit | counts=[1], desired=8 | index 0 |

<!-- markdownlint-enable MD013 -->

### Manual Verification

- [ ] App launches with correct defaults when no config file
      exists
- [ ] All 14 settings persist across app restart
- [ ] Lighting sliders work identically after the `out` to
      `in-out` change
- [ ] MSAA sample count resolves correctly on current GPU
- [ ] Corrupt config file produces stderr warning and default
      startup
- [ ] Config file with unknown fields loads without error
- [ ] Rapid slider dragging produces a single debounced save
      (not disk thrashing)
- [ ] Save-on-exit works when closing during the debounce
      window

## Risks and Mitigations

<!-- markdownlint-disable MD013 -->

| Risk | Impact | Mitigation |
| --- | --- | --- |
| Lighting `out`-to-`in-out` change breaks bindings | Sliders stop working or show wrong defaults | Test lighting immediately after Step 1.2; `<=>` pattern is proven for camera properties |
| Config save failure (permissions, disk full) | App crash | All save errors caught and logged via `eprintln!`, never propagated |
| Corrupt config prevents startup | App fails to start | `load_config` catches all errors, returns `AppConfig::default()` |
| Rename atomicity on network drives | Partial config file | Acceptable risk for desktop wallpaper app; network drives are edge case |
| `dirs` crate returns `None` for data dir | Config silently never saved | Log warning, proceed without persistence; app works normally |
| Debounce timer not firing (blocked event loop) | Config not saved after change | Save-on-exit backstop catches missed saves |

<!-- markdownlint-enable MD013 -->

## Rollback Strategy

All changes are additive. The config module is self-contained
and only called from `main.rs`. To roll back: revert commits.
The app will start with hardcoded defaults exactly as before.
Any existing `config.toml` files on disk are harmless (ignored
without the config module).

## Status

- [ ] Plan approved
- [ ] Implementation started
- [ ] Implementation complete
