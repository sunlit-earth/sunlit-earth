# Research: UI Overhaul (2026-03-23)

## Problem Statement

The Sunlit Earth control panel exposes 46 settings across 9 GroupBox sections in a single scrollable column. Every control is visible simultaneously, making the panel overwhelming for casual use. The UI overhaul reorganizes the panel so that the most common actions (setting wallpaper, choosing a camera preset) are immediately accessible, while advanced tuning controls are collapsed by default.

The overhaul also changes the save behavior: config is currently auto-saved on every slider change (debounced to 1 second). After the overhaul, "Set as Wallpaper" becomes a save-and-apply action, auto-save is removed, and explicit "Load Defaults" and "Reset" buttons provide deliberate state management.

## Requirements

1. **"Set as Wallpaper" moves to the top** of the controls panel as a prominent save-and-apply button.
2. **"Load Defaults" and "Reset" buttons** sit below it. "Load Defaults" restores factory defaults; "Reset" reloads the last-saved config from disk.
3. **3x3 camera preset grid** with 9 labeled buttons: 6 continents + 3 iconic views. Clicking a preset sets longitude, latitude, zoom, tilt, yaw, pitch, and offsets in one action.
4. **Collapsible advanced controls** section containing all current GroupBox sections, hidden by default.
5. **Reordered control groups** within the advanced section (exact order TBD in planning).
6. **No breaking changes** to the rendering pipeline, shader, or GPU resource management.

## Findings

### Collapsible Section Pattern

Slint ~1.15 has no built-in accordion or collapsible panel widget. Three viable mechanisms were evaluated:

**`if` element (recommended):** When the condition is false, the element is removed entirely from the layout -- it occupies zero space. This is already proven in the codebase: `if root.atmo-enabled` and `if root.use-custom-datetime` control the atmosphere and date/time sections today. All slider values are stored as `in-out` properties on the root `MainWindow` with `<=>` bindings, so state survives destroy/recreate cycles. No animation, but instant toggle is acceptable for a utility panel.

**Height animation with `clip`:** Animates height from 0px to a fixed value with `clip: true` to prevent overflow. The SurrealismUI third-party library uses this pattern. The critical limitation is that the expanded height must be a concrete `length` value known in advance -- Slint cannot animate to `auto` or content-driven height. This is impractical for the advanced controls section, which contains nested conditional sub-sections (atmosphere, date/time) whose total height varies.

**States and transitions:** Structurally similar to height animation but uses `states` blocks with `in`/`out` transition keywords. Same fixed-height limitation. Would be the cleanest syntax if animation is ever desired.

**Verdict:** Use the `if` element pattern. It is low-complexity, zero-cost when collapsed, and already battle-tested in this codebase. A reusable `CollapsibleSection` component with a clickable header row and `@children` slot can wrap existing GroupBox content.

### Suggested CollapsibleSection Component

```slint
component CollapsibleSection {
    in property <string> title;
    in-out property <bool> open: true;

    VerticalLayout {
        spacing: 0px;

        header := TouchArea {
            height: 28px;
            clicked => { root.open = !root.open; }

            HorizontalLayout {
                padding: 4px;
                Text { text: root.open ? "v" : ">"; }
                Text { text: root.title; font-weight: 700; }
            }
        }

        if root.open :
        VerticalLayout {
            @children
        }
    }
}
```

This can be defined in `ui/main.slint` or extracted to a separate `ui/components.slint` file. Each existing GroupBox section migrates into a `CollapsibleSection` with its contents as children.

### Camera Preset Values

Nine presets were researched with authoritative geographic and NASA image sources. Six are continent views (visually centered, not geometric center) and three are iconic/scientific views.

| # | Preset | Longitude | Latitude | Zoom | Pitch | Notes |
| --- | -------- | --------- | -------- | ---- | ----- | ----- |
| 1 | Europe | 15.0 | 52.0 | 0.42 | 0 | Central Poland/Germany; Iberia to Scandinavia |
| 2 | N. America | -100.0 | 45.0 | 0.40 | 0 | Over Kansas; Alaska to Panama visible |
| 3 | S. America | -60.0 | -15.0 | 0.42 | 0 | Over Cuiaba, Brazil |
| 4 | Africa | 17.0 | 2.0 | 0.42 | 0 | Over Republic of Congo |
| 5 | Asia | 90.0 | 35.0 | 0.35 | 0 | Over Tibetan Plateau; widest zoom |
| 6 | Oceania | 135.0 | -25.0 | 0.42 | 0 | Over Northern Territory, Australia |
| 7 | Blue Marble | 37.4 | -26.3 | 0.15 | 0 | Apollo 17 (AS17-148-22727); Africa centered |
| 8 | Earthrise | -12.0 | 4.0 | 0.10 | 45 | Apollo 8 (AS08-14-2383); max pitch for tilt |
| 9 | North Pole | 0.0 | 90.0 | 0.35 | 0 | Arctic top-down; longitude arbitrary at pole |

All tilt, yaw, offset_x, and offset_y values are 0 for every preset except Earthrise (pitch = +45 for the iconic diagonal orientation). The zoom values are tuned for visual feel rather than physical accuracy -- the Blue Marble zoom (0.15) shows the globe with surrounding darkness, matching the famous photo's composition rather than the actual 4.6-Earth-radii Apollo 17 distance.

**Earthrise limitation:** The real photo has Earth's disk rotated ~135 degrees clockwise from north-up. The pitch slider's maximum is +/-45 degrees, so +45 is the best available approximation. A full reproduction would require extending the pitch range, which is out of scope.

### Current Architecture (What Changes, What Stays)

**Files that must change:**

| File | Changes |
| ------ | ------- |
| `ui/main.slint` | Major restructure: reorder sections, add preset grid, add CollapsibleSection component, move buttons, add new callbacks |
| `src/main.rs` | New callback handlers: `apply-preset(int)`, `load-defaults()`, modified `reset-all()` semantics, remove auto-save timer (or keep it but change trigger), add save-on-wallpaper logic |
| `src/config.rs` | May need a `defaults()` method distinct from `Default::default()` if "Load Defaults" differs from the current `Default` impl; otherwise unchanged |

**Files that stay unchanged:**

| File | Reason |
| ------ | ------ |
| `src/renderer/*` | No rendering changes; presets only set camera/shading properties |
| `src/scene/camera.rs` | Preset values are within existing parameter ranges |
| `src/scene/sun.rs` | No date/time changes |
| `shaders/*.wgsl` | No shader changes |
| `src/wallpaper.rs` | Wallpaper export logic unchanged; only the trigger point moves |
| `src/cloud_fetcher.rs` | Unrelated |

### Current Callback and Data Flow Architecture

Understanding the callback wiring is critical because the overhaul changes save behavior:

**Current flow:** Every slider/toggle change fires `sliders-changed()` -> restarts a 1-second debounce `Timer` -> timer fires `save_config()`. This is the auto-save mechanism.

**New flow (after overhaul):**

- Slider changes still fire `sliders-changed()` for redraw and label updates, but the save timer is either removed or repurposed.
- "Set as Wallpaper" triggers: save config to disk, render at monitor resolution, save PNG, apply wallpaper.
- "Load Defaults" triggers: restore all properties to `AppConfig::default()` values, request redraw. Does NOT save to disk (user can revert with "Reset").
- "Reset" triggers: reload `config.toml` from disk, apply to all properties, request redraw.

**New callbacks needed in Slint:**

```slint
callback apply-preset(int);   // Index 0-8 into preset array
callback load-defaults();      // Restore factory defaults (no save)
// reset-all() already exists but semantics change to "reload from disk"
// set-wallpaper() already exists but now also saves config
```

### Preset Implementation Strategy

Presets only set camera parameters (longitude, latitude, zoom, tilt, yaw, pitch, offset_x, offset_y). They do NOT change lighting, color correction, clouds, atmosphere, or date/time settings. This keeps the mental model simple: presets control the view, not the appearance.

The preset data can be stored as:

- **Rust-side array of structs** (recommended): A `const PRESETS: [CameraParams; 9]` array in `main.rs` or `scene/camera.rs`. The `apply-preset(int)` callback indexes into this array and sets all 8 camera properties on the window. This keeps the data in one place and avoids duplicating it in Slint markup.
- **Slint-side model**: Would require defining a struct model and binding it -- more complex with no benefit since presets are static.

### Config Persistence Changes

The current `AppConfig` struct has 40 fields with `#[serde(default)]` for forward/backward compatibility. The overhaul changes when saves happen but not what is saved. Key considerations:

- **Gamma encoding asymmetry**: The UI stores gamma as a normalized 0.0-1.0 slider value; the config file stores actual gamma (0.2-3.0). Conversion functions `gamma_slider_to_value()` and `gamma_value_to_slider()` in `renderer/mod.rs` handle the mapping. This must be preserved when implementing "Load Defaults" and "Reset".
- **Atomic writes**: Config is written to `config.toml~` then renamed. This pattern should be preserved.
- **Exit-time save**: Currently saves on window close as a fallback. After the overhaul, decide whether to save on exit (preserving unsaved slider tweaks) or discard unsaved changes (treating "Set as Wallpaper" as the only save point). This is a UX decision to make during planning.

### UI Layout Structure

The current layout is a two-pane split: left `ScrollView` with controls, right `Rectangle` with the rendered Earth image, separated by a draggable `Splitter`. The minimum panel width is 160px.

The new layout within the left `ScrollView` would be (top to bottom):

1. **Set as Wallpaper** button (prominent, at top)
2. **Load Defaults** and **Reset** buttons (side by side or stacked)
3. **Wallpaper status** text (feedback from last export)
4. **3x3 Preset Grid** (9 labeled buttons in a grid layout)
5. **Advanced Controls** toggle (collapsed by default)
   - Inside: all current GroupBox sections, potentially reordered
6. **Renderer Info** text (footer)

The `Splitter` and `image-container` (right pane) are unaffected.

### Slint GridLayout for Presets

Slint's `GridLayout` supports `Row` elements for defining grid cells. A 3x3 preset grid:

```slint
GridLayout {
    spacing: 4px;
    Row { Button { text: "Europe"; } Button { text: "N. America"; } Button { text: "S. America"; } }
    Row { Button { text: "Africa"; } Button { text: "Asia"; } Button { text: "Oceania"; } }
    Row { Button { text: "Blue Marble"; } Button { text: "Earthrise"; } Button { text: "N. Pole"; } }
}
```

Each button's `clicked` handler calls `root.apply-preset(N)` with its index.

**Note on `if` in `GridLayout`:** The Slint `if` conditional element is not supported inside `GridLayout` (confirmed by GitHub discussion #8212). This is not a problem here since all 9 preset buttons are always visible.

## Technical Constraints

1. **Slint `visible: false` does NOT remove layout space.** Only the `if` element removes space. This is confirmed by official docs and maintainer statements. All collapsible sections must use `if`, not `visible`.

2. **`if` destroys and recreates elements.** Any internal-only state (not bound to a root property) is lost on toggle. This is safe for Sunlit Earth because all values are `in-out` root properties with `<=>` bindings.

3. **Height animation requires fixed heights.** Slint cannot animate from 0 to `auto`. The advanced controls section contains nested conditionals (atmosphere, date/time) whose height varies, making height animation impractical.

4. **Preset pitch range limited to +/-45 degrees.** The Earthrise photo requires ~135 degrees of roll but only 45 is available. This is an inherent limitation of the current slider range, not something the UI overhaul changes.

5. **Gamma slider encoding.** The normalized (0.0-1.0) slider representation vs. actual gamma (0.2-3.0) storage must be handled correctly when implementing "Load Defaults" (which should set the slider to 0.5, mapping to gamma 1.0).

6. **Deferred ComboBox index setting.** `aa_index` and `texture_index` are set via `invoke_from_event_loop()` to avoid a race condition. Any reset/defaults logic must use the same deferred pattern.

7. **Dead-zone snapping** on tilt, yaw, pitch, and offset sliders (done in Slint). Preset application writes values via Rust `set_*()` calls, bypassing the Slint dead-zone logic. This is correct behavior -- presets should set exact values.

## Open Questions

1. **Should the app save config on exit?** If yes, unsaved slider tweaks are preserved across sessions even without clicking "Set as Wallpaper". If no, only explicitly saved configs persist. This affects UX significantly.

2. **Should presets reset non-camera settings?** The current plan has presets only set 8 camera parameters. Should they also reset lighting/color/atmosphere to defaults for a "clean" view, or preserve the user's current appearance settings?

3. **What is the reordered group sequence?** The current order (Rendering, Camera Position, Camera Orientation, Framing, Lighting, Color Correction, Clouds, Atmosphere, Date/Time) may not be optimal. The plan should specify the new order.

4. **Should the advanced section remember its open/closed state?** If persisted in config, the section stays open across restarts. If not, it always starts collapsed.

5. **Should individual sub-sections within advanced controls be independently collapsible?** For example, Lighting and Color Correction could each have their own toggle, or the entire advanced block could be one toggle.

6. **Grid vs. list for presets?** A 3x3 grid is compact but button labels may be cramped in narrow panel widths (minimum 160px). A vertical list of 9 buttons would be wider-friendly. The grid works well at reasonable panel widths but should be tested at the 160px minimum.

## Recommendations

1. **Use the `if` element pattern** for the advanced controls toggle. Define a `CollapsibleSection` component with a `TouchArea` header and `@children` slot. Start with a single top-level toggle wrapping all advanced groups; add per-group toggles later if needed.

2. **Store preset data as a Rust-side `const` array** of `CameraParams` structs. The `apply-preset(int)` callback indexes into this array and sets all 8 camera properties. This avoids duplicating coordinates in both Slint markup and Rust code.

3. **Keep the auto-save timer for now** but make "Set as Wallpaper" also trigger an immediate save. This preserves the current UX (settings persist across restarts) while adding the explicit save semantic. Removing auto-save entirely is a bigger behavioral change that can be done separately if desired.

4. **Implement "Load Defaults" as `AppConfig::default()`** applied to all properties, and **"Reset" as `config::load_config()`** re-read from disk. Both call `apply_config_to_window()` followed by `request_redraw()`.

5. **Start with the Slint-side changes** (layout restructure, new component, preset grid) before the Rust-side changes (new callbacks, preset data). The Slint changes are the most visible and can be previewed immediately.

6. **Test preset grid at minimum panel width** (160px). If labels are too cramped, consider abbreviating continent names or switching to a 3-column `VerticalLayout` of `HorizontalLayout` rows instead of `GridLayout`.

## Sources

| Document | Focus Area |
| -------- | ---------- |
| `docs/plans/2026-03-23-ui-overhaul-codebase.md` | Current UI architecture, callback wiring, config persistence, all 46 settings with types/ranges/defaults |
| `docs/plans/2026-03-23-ui-overhaul-slint-patterns.md` | Slint collapsible/expandable patterns: `if` element, height animation, states, TabWidget, PopupWindow |
| `docs/plans/2026-03-23-ui-overhaul-presets.md` | Geographic coordinates and iconic photo geometry for 9 camera presets |
