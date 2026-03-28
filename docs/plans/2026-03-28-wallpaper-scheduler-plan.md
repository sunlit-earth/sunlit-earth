# Plan: Wallpaper Scheduler (2026-03-28)

## Summary

Add automatic wallpaper update scheduling to Sunlit
Earth so the desktop wallpaper tracks the sun's
position throughout the day. This includes UI controls
(checkbox + interval slider), a `slint::Timer`-based
scheduler, tray menu integration ("Refresh Now" /
"Auto-refresh"), config persistence, and correct
behavior in tray mode with hidden startup. The
implementation is gated on a Phase 1 e2e test that
validates the critical assumption that GPU resources
persist after `window.hide()`.

## Stakes Classification

**Level**: Medium

**Rationale**: Changes span 8 files across UI, config,
timer, tray, and IPC modules, but each change follows
well-established patterns in the codebase. The
architectural assumption (GPU persistence after hide)
is validated first via an e2e gate test. All changes
are additive --- no existing behavior is modified.
Rollback is straightforward: revert the branch.

## Context

**Research**:
[2026-03-28-wallpaper-scheduler-research.md](2026-03-28-wallpaper-scheduler-research.md)

**Corrected finding**: The research document claims GPU
resources are destroyed when the window is hidden
(`RenderingTeardown` fires on `hide()`). This is
**wrong**. Investigation confirmed that
`RenderingTeardown` is NOT triggered by `hide()` or
`HideWindow` --- only on actual window destruction.
GPU resources persist in the thread-local
`GPU_RESOURCES` after hiding,
`export_wallpaper_image()` works after hide, Slint
timers continue firing via
`run_event_loop_until_quit()`, and `BeforeRendering`
callbacks fire on hidden windows when
`request_redraw()` is called. This simplifies the
hidden startup case considerably --- no
show-render-hide cycles are needed for scheduled
updates.

**Affected files**:

- `src/ipc.rs` --- add `export-test` IPC command
- `tests/e2e.rs` --- add GPU persistence e2e test
- `src/config.rs` --- add `auto_refresh_enabled`,
  `auto_refresh_interval_minutes`
- `ui/main.slint` --- add scheduler UI controls
- `src/ui_callbacks.rs` --- add scheduler callbacks,
  extend config bridge
- `src/main.rs` --- create scheduler timer in
  `run_event_loop()`
- `src/tray.rs` --- add "Refresh Now" and
  "Auto-refresh" menu items

## Success Criteria

- [ ] E2e test confirms `export_wallpaper_image()`
  succeeds after `window.hide()`
- [ ] UI checkbox enables/disables auto-refresh; slider
  sets interval (1--30 min, default 5)
- [ ] When enabled, wallpaper updates automatically on
  the configured interval
- [ ] On startup with auto-refresh enabled, wallpaper
  updates after first frame renders
- [ ] Tray menu has "Refresh Now" (immediate export)
  and "Auto-refresh" (checkable toggle)
- [ ] Auto-update settings persist across restarts via
  `AppConfig`
- [ ] Scheduler works correctly in hidden tray mode
  (no window flash needed)
- [ ] `cargo test` passes (including new unit tests
  for config fields)
- [ ] `cargo clippy` passes with no new warnings

## Implementation Steps

### Phase 1: Gate Test --- GPU Persistence After Hide

This phase validates the critical assumption that GPU
resources survive `window.hide()`. If this test fails,
the entire approach must be reconsidered. **Do not
proceed to Phase 2 until this test passes.**

#### Step 1.1: Add `export-test` IPC command

- **Files**: `src/ipc.rs`
- **Action**: Add an `export-test` command to
  `dispatch_command()` that calls
  `renderer::export_wallpaper_image()` at a small
  resolution (e.g., 64x64) and signals success or
  failure via stdout. This provides a way for e2e
  tests to trigger a GPU export without needing the
  full wallpaper pipeline. The command should:
  1. Call `renderer::export_wallpaper_image(64, 64)`
     inside `invoke_from_event_loop`
  2. On success, emit `SIGNAL:export_test_ok` to
     stdout
  3. On failure, emit `SIGNAL:export_test_failed` to
     stdout
- **Verify**: `cargo build` succeeds;
  `cargo clippy` passes
- **Complexity**: Small

#### Step 1.2: Write and run the GPU persistence e2e test

- **Files**: `tests/e2e.rs`
- **Action**: Add `test_gpu_persistence_after_hide`
  that validates `export_wallpaper_image()` works
  after the window has been hidden. Test sequence:
  1. Spawn binary in tray mode with
     `--tray-start visible` and IPC enabled
  2. Wait for `ipc_listener_ready` and
     `first_frame_rendered` signals
  3. Send `hide-window` IPC command, wait for
     `window_hidden` signal
  4. Send `export-test` IPC command, wait for
     `export_test_ok` signal (this is the critical
     assertion --- if GPU resources were destroyed
     by hide, this would fail with
     "GPU not initialized")
  5. Send `quit`, verify clean exit
- **Test cases**:
  - GPU export after hide succeeds (receives
    `export_test_ok` signal within 30s)
  - No ERROR lines in stderr
  - Process exits with code 0
- **Verify**: Run
  `cargo test --test e2e test_gpu_persistence_after_hide -- --ignored`
  and confirm it passes on the local machine
- **Complexity**: Medium

### Phase 2: Config and UI Foundation

These steps add the data model and UI elements. They
build on each other sequentially.

#### Step 2.1: Add config fields with tests (RED then GREEN)

- **Files**: `src/config.rs`
- **Action**: Add two new fields to `AppConfig`:
  - `auto_refresh_enabled: bool` (default: `false`)
  - `auto_refresh_interval_minutes: u32`
    (default: `5`)

  Both use `#[serde(default)]` at the struct level
  (already present), so missing fields in existing
  config files will get defaults automatically.
- **Test cases**:
  - Deserialize empty string:
    `auto_refresh_enabled` is `false`,
    `auto_refresh_interval_minutes` is `5`
  - Deserialize with explicit values:
    `auto_refresh_enabled = true` and
    `auto_refresh_interval_minutes = 15`
    round-trip correctly
  - Deserialize with only one field present: the
    other gets its default
  - Existing `serde_round_trip` and
    `serde_round_trip_non_default` tests must be
    updated to include the new fields and continue
    passing
- **Verify**: `cargo test config` passes
- **Complexity**: Small

#### Step 2.2: Add Slint UI properties and controls

- **Files**: `ui/main.slint`
- **Action**: Add two new `in-out` properties to
  `MainWindow`:
  - `auto-refresh-enabled: bool` (default `false`)
  - `auto-refresh-interval: float`
    (default `5.0`, range 1--30)

  Add a callback: `auto-refresh-changed()` (fires
  when checkbox or slider changes).

  Add UI controls in the always-visible top section,
  between the "Set as Wallpaper" button and the
  "Load Defaults" / "Reset" row. Layout: a
  `HorizontalLayout` with a `CheckBox`
  ("Auto-refresh") and, when checked, a `Slider`
  (1--30) plus a `Text` element showing the
  interval (e.g., "every 5 min"). The slider and
  label use the `if root.auto-refresh-enabled :`
  pattern from Date/Time and Atmosphere sections.
- **Verify**: `cargo build` succeeds; visual
  inspection confirms controls appear correctly
- **Complexity**: Medium

#### Step 2.3: Wire config bridge functions

- **Files**: `src/ui_callbacks.rs`
- **Action**: Update `apply_config_to_window()` to
  set the new Slint properties from `AppConfig`:
  - `window.set_auto_refresh_enabled(...)`
  - `window.set_auto_update_interval(...)`

  Update `read_config_from_window()` to read them
  back:
  - `auto_refresh_enabled: window.get_...()`
  - `auto_refresh_interval_minutes: window.get_...()
    as u32`
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

#### Step 2.4: Register auto-refresh change callback

- **Files**: `src/ui_callbacks.rs`
- **Action**: In `register_change_callbacks()`,
  register a handler for the `auto-refresh-changed`
  callback. When the auto-refresh checkbox or
  interval slider changes:
  1. Read the current config from the window
  2. Save config to disk (these settings must
     persist across restarts since they affect
     background behavior)

  The actual timer restart logic will be handled in
  Phase 3 when the timer is created.
- **Verify**: `cargo build` succeeds
- **Complexity**: Small

### Phase 3: Scheduler Timer

#### Step 3.1: Create the scheduler timer in `run_event_loop()`

- **Files**: `src/main.rs`
- **Action**: After the sun timer setup and before
  the event loop starts, create the scheduler timer
  using `Rc<slint::Timer>`:
  1. Create
     `let scheduler_timer = Rc::new(slint::Timer::default())`
  2. If `config.auto_refresh_enabled` is true, start
     the timer with `Repeated` mode at the
     configured interval, calling
     `do_set_wallpaper()` in the callback
  3. Register an `on_auto_refresh_changed` callback
     on the window that restarts or stops the timer
     based on the current checkbox/slider state.
     The callback captures a clone of the
     `Rc<slint::Timer>` and the window weak
     reference. When the checkbox is enabled, it
     calls `timer.start(Repeated, interval, cb)`.
     When disabled, it calls `timer.stop()`
  4. The timer stays alive via the `Rc` (captured
     in the callback closure) --- no `mem::forget`
     needed
  5. Pass `aa_counts` into `run_event_loop()` so
     the config can be read from the window and
     saved when settings change
- **Test cases** (manual verification):
  - Enable auto-refresh with 1-minute interval,
    verify wallpaper updates after 1 minute
  - Change interval while running, verify the new
    interval takes effect
  - Disable auto-refresh, verify wallpaper stops
    updating
- **Verify**: `cargo build` succeeds; manual test
  with 1-minute interval confirms timer fires
- **Complexity**: Medium

#### Step 3.2: Trigger wallpaper update on startup

- **Files**: `src/main.rs`
- **Action**: When auto-refresh is enabled at startup,
  the wallpaper should update as soon as the first
  frame has rendered. The existing `textures_ready`
  polling pattern (used by the render subcommand)
  provides the mechanism. Add a one-shot startup
  timer that:
  1. Only activates when
     `config.auto_refresh_enabled` is true and
     the mode is not `render`
  2. Polls `textures_ready` at 200ms intervals
     (same as render timer)
  3. On first ready, calls `do_set_wallpaper()`,
     then stops itself

  This ensures the wallpaper is up-to-date
  immediately on launch without waiting for the
  first scheduler interval to elapse.
- **Test cases** (manual verification):
  - Start app with auto-refresh enabled in config,
    verify wallpaper is set shortly after launch
  - Start app with auto-refresh disabled, verify no
    automatic wallpaper set on launch
- **Verify**: `cargo build` succeeds; manual test
  confirms wallpaper updates shortly after startup
- **Complexity**: Medium

### Phase 4: Tray Menu Integration

#### Step 4.1: Add tray menu items

- **Files**: `src/tray.rs`
- **Action**: In `run_tray_event_loop()`, add two
  new menu items between "Open" and "Exit":
  1. `MenuItem::new("Refresh Now", true, None)` ---
     triggers `do_set_wallpaper()` via
     `invoke_from_event_loop`
  2. `CheckMenuItem::new("Auto-refresh", ...)` ---
     toggleable, syncs with the window's
     `auto-refresh-enabled` property. Use
     `tray_icon::menu::CheckMenuItem` (available in
     the `muda` re-export). When toggled:
     - Read the new checked state from the
       `CheckMenuItem`
     - Dispatch to the Slint event loop to update
       `window.set_auto_refresh_enabled()`
     - Invoke the `auto-refresh-changed` callback so
       the timer restarts/stops and config saves

  The initial checked state should reflect the
  window's `auto-refresh-enabled` property. Since
  the tray thread starts before the window is fully
  configured, read the initial state from the Slint
  event loop via `invoke_from_event_loop`.
- **Test cases** (manual verification):
  - Click "Refresh Now" in tray menu, verify
    wallpaper updates immediately
  - Toggle "Auto-refresh" in tray menu, verify
    checkbox in UI updates and timer starts/stops
  - Toggle auto-refresh via UI checkbox, verify
    tray menu checkmark updates
- **Verify**: `cargo build` succeeds; manual
  verification of both tray menu items
- **Complexity**: Medium

#### Step 4.2: Sync tray menu state with UI

- **Files**: `src/tray.rs`, `src/main.rs`
- **Action**: The tray "Auto-refresh" `CheckMenuItem`
  must stay in sync with the UI checkbox. Two sync
  directions:
  1. **Tray -> UI** (handled in Step 4.1): tray
     toggle dispatches to Slint event loop to
     update window property
  2. **UI -> Tray**: when the UI checkbox changes,
     the tray checkmark should update. Since
     `CheckMenuItem` is `!Send`, cross-thread sync
     is non-trivial. `muda` does not have a
     "menu about to show" event for lazy sync.

  Given the complexity, the pragmatic approach is:
  - The tray `CheckMenuItem` starts with the
    correct initial state
  - Tray toggle updates the UI (Step 4.1)
  - UI toggle does NOT update the tray checkmark
    (minor cosmetic limitation)
  - This is acceptable because users typically use
    either the tray OR the UI, not both
    simultaneously
- **Verify**: Manual verification that tray -> UI
  sync works correctly
- **Complexity**: Small (one-way sync limitation
  accepted)

### Phase 5: Final Verification

#### Step 5.1: Run full test suite and clippy

- **Files**: N/A
- **Action**: Run `cargo test` and `cargo clippy` to
  verify no regressions
- **Verify**: All tests pass, no clippy warnings
- **Complexity**: Small

#### Step 5.2: Update roadmap

- **Files**: `docs/roadmap.md`
- **Action**: Check off the "Periodic re-rendering"
  and "System tray extended features" items,
  updating descriptions as needed to reflect what
  was implemented
- **Verify**: Roadmap reflects current state
- **Complexity**: Small

#### Step 5.3: Manual end-to-end verification

- **Files**: N/A
- **Action**: Full manual test of all scenarios:
- **Manual test cases**:
  - [ ] Start in windowed mode, enable auto-refresh
    with 1-min interval, wait for 2 cycles,
    verify wallpaper updates twice
  - [ ] Change interval to 2 minutes while running,
    verify next update fires after 2 minutes
  - [ ] Disable auto-refresh, verify no more updates
  - [ ] Start in tray mode with
    `--tray-start hidden`, auto-refresh enabled in
    config, verify wallpaper updates shortly after
    launch without the window becoming visible
  - [ ] Open tray menu, click "Refresh Now", verify
    immediate wallpaper update
  - [ ] Toggle "Auto-refresh" via tray menu, verify
    timer starts
  - [ ] Close and reopen the app, verify auto-refresh
    settings are preserved
  - [ ] Run
    `cargo test --test e2e -- --ignored`
    to verify all e2e tests pass
- **Verify**: All manual checks pass
- **Complexity**: Small

## Test Strategy

### Automated Tests

<!-- markdownlint-disable MD013 -->

| Test Case | Type | Input | Expected |
| --- | --- | --- | --- |
| GPU export after hide | E2e | show, render, hide, export-test | `export_test_ok`, exit 0 |
| Config default new fields | Unit | `toml::from_str("")` | `false`, `5` |
| Config round-trip | Unit | Serialize + deserialize | Values preserved |
| Config forward compat | Unit | Old config, no new fields | Defaults filled |
| Existing tests pass | Unit | All existing cases | No regressions |

<!-- markdownlint-enable MD013 -->

### Manual Verification

- [ ] Auto-update timer fires at configured interval
  and updates wallpaper
- [ ] Interval changes take effect immediately
  (timer restarts)
- [ ] Disabling stops the timer
- [ ] Startup with auto-refresh enabled triggers
  immediate wallpaper update after first render
- [ ] Tray "Refresh Now" triggers immediate export
- [ ] Tray "Auto-refresh" toggle starts/stops the
  scheduler
- [ ] Hidden tray mode works without window flash

## Risks and Mitigations

<!-- markdownlint-disable MD013 -->

| Risk | Impact | Mitigation |
| --- | --- | --- |
| GPU resources destroyed on hide (contradicting corrected finding) | Blocks entire feature | Phase 1 gate test catches this before feature work |
| `do_set_wallpaper()` blocks UI thread | Brief UI hang | Acceptable; same as manual "Set as Wallpaper" button |
| Tray `CheckMenuItem` desync with UI | Cosmetic confusion | One-way sync accepted; documented in code |
| Config save on slider change | Disk I/O | Config is small TOML, atomic write is fast |

<!-- markdownlint-enable MD013 -->

## Rollback Strategy

All changes are on the `wallpaper-scheduler` branch
in a git worktree. If any phase fails:

- **Phase 1 failure** (GPU persistence test fails):
  Stop. Do not proceed. The hidden-window approach
  does not work, and the feature needs a
  fundamentally different design (possibly headless
  rendering or a show-render-hide cycle).
- **Phase 2--4 failure**: Revert to the last working
  commit on the branch. Each phase produces a
  compilable, testable state.
- **Full rollback**: Delete the branch. No changes
  to `main`.

## Status

- [ ] Plan approved
- [ ] Phase 1 complete (gate test passes)
- [ ] Phase 2 complete (config + UI)
- [ ] Phase 3 complete (scheduler timer)
- [ ] Phase 4 complete (tray integration)
- [ ] Phase 5 complete (verification)
- [ ] Implementation complete
