# Plan: Celestial A, Sky Transform, Stars, Planets, About Window

Phase A of [2026-08-23-celestial-bodies-research.md](2026-08-23-celestial-bodies-research.md), sections 3, 4, 7, and the attribution surface from section 9.

## Summary

The sky frame transform (`SkyState` and the J2000-to-world rotation), the star catalog baked from HYG v4.4 into an embedded blob, stars and the five naked-eye planets rendered as instanced sprites behind the Earth, and an About window reachable from the tray menu and the settings window that carries the version and the attributions, starting with the catalog's CC BY-SA 4.0 credit. No image assets, no Git LFS, no memory budget change. Pure code plus one 250 KiB embedded data file.

## Stakes Classification

Medium-low. The work rewires where the production sun direction comes from and adds a draw to the render pass, so mistakes show as a wrong sky orientation, a subtly shifted terminator, or a stale frame, all visible and all recoverable. The one obligation that outlives the code is the catalog license, which this phase discharges by shipping the attribution with the blob.

## Research

Recorded from code reconnaissance on 2026-08-23, references to the tree at that date.

The sun path is the template and the thing being generalized. `scene/sun.rs` computes RA/Dec of date plus GAST and converts to the world frame (`sun.rs:23-86`); its observer is topocentric at 0N 0E, not geocentric as its comment claims (`sun.rs:32`), an error of at most 8.8 arcseconds for the sun; `make_time` (`sun.rs:109-125`) and `compute_sun_direction_at` (`sun.rs:163-179`) are the datetime plumbing to reuse. The engine asks for the sun once per render (`engine/mod.rs:837-838`, called at `846-847` and `960-962`) and refreshes on a 120-second schedule (`SUN_INTERVAL`, `engine/mod.rs:46`); the renderer receives it through `FrameInputs` (`render_pass.rs:16-19`) and the scheduler overwrites it for hidden-window exports through `Renderer::set_sun_direction` (`renderer/mod.rs:441-445`). Dirty checking compares the digest plus size plus the milliradian-quantized sun direction (`frame.rs:9-30`, `params.rs:231-233`). The render pass clears to (0.02, 0.02, 0.05) and draws Earth, clouds, Rayleigh, and two additive nightglow shells (`render_pass.rs:126-216`); the nightglow pipelines are the additive-blend, no-depth-write template (`gpu_setup.rs:387-434`); the shader module is two concatenated WGSL files (`gpu_setup.rs:219-222`); `Uniforms` is 208 bytes with a compile-time size assertion (`uniforms.rs:42`). Vertex and index buffers are created once in `create_renderer` (`gpu_setup.rs:51-63`); nothing in the tree uses instancing yet. The params bridge is `read_params_from_window` (`ui_callbacks.rs:336`), `apply_params_to_window` (`ui_callbacks.rs:290`), and the change callbacks (`ui_callbacks.rs:121`); the digest table test that must gain rows is `every_shader_parameter_triggers_dirty` (`params.rs:420`). The tray menu is a Slint `Menu` with Open, Refresh Now, Auto-refresh, Exit (`ui/main.slint:1227-1249`), wired in `tray::create_tray` (`tray.rs:76`); `main.slint` exports `MainWindow` and `TrayIcon`, so a third exported component is the established route to another window. The xtask CLI is a clap `Command` enum (`xtask/src/main.rs:43`) and does not depend on `sunlit-core`, which the bake tool must preserve. Golden cases pin a custom datetime through `base_params` (`tests/golden.rs:109-125`). HYG v4.4 facts (119,614 rows, J2000 RA in decimal hours, Dec in degrees, `mag`, `ci` as B-V, `pmra`/`pmdec` in mas/yr, CC BY-SA 4.0) are in the research document, measured from the current CSV.

## Key Design Decisions

1. **`SkyState` becomes the one producer of per-frame astronomy, and the sun direction moves onto it.** A new `scene::sky` module computes, as a pure function of `DateTimeInput` plus a UTC now: the 3x3 `R_world_from_eqj` (from `Astronomy_Rotation_EQJ_EQD` plus the GAST rotation plus the axis permutation), the sun direction as `R` applied to the sun's geocentric EQJ unit vector from `Astronomy_GeoVector`, and the five planet entries (direction through `R`, magnitude from `Astronomy_Illumination`). The engine's `sun_direction()` call sites become one `sky_state()` call; `FrameInputs` carries the state; `set_sun_direction` generalizes accordingly. The existing subsolar-point path in `sun.rs` stays as the reference implementation for the equivalence test, which also retires its topocentric wrinkle from production: the two paths must agree within 0.05 degrees, and Polaris (RA 2h31m, Dec +89.26) must land within 1 degree of world +Y. The production sun direction moves by at most 0.0024 degrees, below the milliradian dirty-check quantum.

2. **The star blob is the instance buffer, byte for byte.** The bake tool writes a small header (magic, version, count) followed by 16-byte records: EQJ direction as 3x f32 (proper motion propagated to epoch 2026.0), then Unorm8x4 packing the baked sRGB tint in RGB and the quantized magnitude in A (linear map from the range -2 to 8). The reader in `sunlit-core::assets` validates the header and hands the payload straight to `create_buffer_init` as vertex data, so there is no runtime parse. Color is baked from B-V through the Ballesteros temperature approximation and a blackbody-to-sRGB fit, desaturated toward white; rows without B-V get white. Embedded with `include_bytes!`; a fixture blob of a handful of hand-checked stars pins the reader, and a test over the shipped blob asserts count, unit-length directions, and the magnitude range.

3. **The bake tool is `cargo xtask bake-stars`, and xtask stays independent of `sunlit-core`.** A new `Command::BakeStars { input, output }` reads the HYG CSV and writes the blob plus a sidecar `ATTRIBUTION.md` naming HYG v4.4, the Codeberg source, and CC BY-SA 4.0. The format contract between writer and reader is held by the checked-in fixture and the shipped blob's invariant test, not by a shared crate, because pulling `sunlit-core` (and with it wgpu) into xtask is a build-cost regression this does not justify.

4. **Stars and planets render as one instanced draw, first in the pass.** A new pipeline (additive blend and no depth write like the nightglow shells, depth test irrelevant because it draws into a cleared buffer first) draws a 4-vertex triangle strip per instance. The vertex shader rotates the instance direction by `R_world_from_eqj` (a new uniform), applies the view rotation with w = 0, projects, applies the same post-projection screen offset the globe gets, pins depth to the far plane, and expands the corners by a pixel-sized offset computed from a new viewport-size uniform. The fragment shader draws a compact filled core with a 0.55 pixel antialiased boundary rather than widening a Gaussian, so increasing size keeps the point crisp. Core radius grows from 0.5 pixels for faint stars to 1.0 pixel for the brightest objects. A separate Gaussian halo has independent strength and extent, and fades out by magnitude 4. The entire footprint scales from 1.0 at 1080p to 2.0 at 4K so its physical size does not collapse as output density rises. The contrast control maps the compressed magnitude exponent from `0.10` to `0.20`, while brightness is a separate gain. Instances above the magnitude limit are removed from the submitted catalog prefix. Catalog stars live in the static buffer from decision 2; the planets are a second, tiny instance buffer of at most six records rewritten with `queue.write_buffer` on every sky refresh, drawn by the same pipeline with the same layout, tinted per planet (Venus white, Mars salmon, Jupiter cream, Saturn pale yellow, Mercury gray).

5. **Six scene parameters receive full digest treatment.** `star_intensity`, `star_size`, `star_glow_strength`, `star_glow_radius`, `star_contrast`, and `star_mag_limit` are `SceneParams` fields, config fields with widgets in a new "Celestial" group in the Advanced section, digest entries, digest-table rows, and uniforms. Intensity is presented as a brightness gain from 0 to 5, with 0 skipping the draw entirely. Size changes only the crisp core. Glow strength reaches 300 percent and glow radius reaches 30 pixels, owning all softness independently. Magnitude contrast runs from minus 100 percent through plus 100 percent. Zero preserves the original compressed response, negative values make magnitudes more uniform, and positive values increase separation. The reviewed defaults are 2.0 brightness, 1.0 size, 50 percent glow strength, 8 pixel glow radius, 0 percent contrast, and a faintest magnitude of 6.5 (the baked 7.0 leaves headroom to slide deeper). Planets follow the same controls rather than getting their own switches.

6. **The dirty check gains the sky rotation.** `FrameState` adds two quantized basis vectors of `R_world_from_eqj` (the third is their cross product, so two suffice), quantized with the existing `quantize_direction`. The sun direction already trips the check every 120-second tick, so render cadence does not change; this addition exists so a params-push render during the same tick cannot skip a sky that moved.

7. **The About window is a third exported Slint component, fed from Rust.** `AboutWindow` shows the version (`env!("CARGO_PKG_VERSION")`) and an attribution list passed as a model from Rust, so the text lives in one Rust constant next to the things it credits: HYG v4.4 (CC BY-SA 4.0), Astronomy Engine (MIT), NASA Blue Marble and Black Marble, the live cloud composite, with NASA SVS lines arriving in phases C and D. The tray menu gains an About item before Exit; the settings window gets a small About control (bottom of the controls panel); both share one lazily created window handle. No `ComponentHandle` complications arise because `AboutWindow` is an ordinary window component, unlike the tray icon.

## Success Criteria

1. At a pinned datetime the rendered sky is verifiably right: the equivalence and Polaris unit tests pass, and a screenshot at a night-side framing shows a recognizable constellation (checked by a human against a planetarium view once, then pinned as a golden).
2. New golden case (night side with stars) generated for the local adapters and stable across three consecutive runs; existing goldens still pass, which also proves the sun-source swap stayed below the visual threshold.
3. All six star controls round-trip through config save/load and the Slint bridge, have digest rows, and intensity 0 demonstrably skips the draw (no star pixels, engine test).
4. Planets appear at correct positions: a unit test pins one planet's direction at a known date against a JPL Horizons value within tolerance, and Venus's magnitude at a known date is brighter than -3.
5. The About window opens from the tray menu and from the settings window and lists the version and the HYG attribution; the blob ships with its `ATTRIBUTION.md` sidecar.
6. Engine render time on the software adapter is unchanged within noise (compare the traced render span before and after), and `cargo test`, `cargo clippy --all-targets`, `cargo fmt --check` are green on Windows and in WSL.
7. CLAUDE.md (SkyState, the star pipeline, the bake tool, the About window) and `docs/roadmap.md` reflect the work.

## Implementation Steps

### Step 1: `scene::sky` and the engine rewiring

`SkyState`, `R_world_from_eqj`, the sun through `GeoVector`, planet entries; the equivalence, Polaris, orthonormality, and planet-position unit tests; engine call sites, `FrameInputs`, `set_sun_direction` generalized, `FrameState` extension with its dirty-check test. No rendering yet; the frame looks identical.

### Step 2: The bake tool and the blob

`cargo xtask bake-stars`, the format, proper-motion propagation, B-V color baking, the sidecar; the reader in `sunlit-core::assets` with the fixture test; the shipped blob and its invariant test.

### Step 3: The star pipeline

Instance layout, the two buffers, the new pipeline and WGSL entry points, the viewport uniform, PSF and flux mapping, the planet buffer rewrite on sky refresh, draw-order placement, and an engine test that a frame renders with stars enabled plus one that intensity 0 draws none.

### Step 4: Parameters, config, UI

The Celestial group, both fields through the whole bridge, digest rows, defaults, `slint_ui.rs` coverage.

### Step 5: About window

The Slint component, the Rust attribution constant, the tray menu item, the settings-window control, wiring and a UI test that the callback shows the window.

### Step 6: Goldens and documentation

The night-side-with-stars golden on the local adapters, CLAUDE.md, roadmap.

## Risks and Mitigations

- The sun-source swap could shift goldens. The delta is bounded at 0.0024 degrees, two orders below the dirty-check quantum; if a golden still moves past tolerance on some adapter, regenerate it once and record the cause, since the new source is the more correct one.
- Sign or handedness errors in `R_world_from_eqj` produce a mirrored or counter-rotating sky. The Polaris and equivalence tests exist for exactly this; both fail loudly on any single sign mistake.
- The e2e render case samples pixels by color; stars add faint pixels to the background. The existing samples target the globe, but verify the fixture expectations before merging and adjust sample points if any sit on background.
- Cross adapter golden stability of small sprites. Mitigated by the analytic antialiased core and reviewed references for each adapter; if an adapter still disagrees, adjust the core boundary rather than the tolerance.
- CC BY-SA compliance is a process risk, not a code risk: the sidecar and About text must land in the same phase as the blob, which is why they share this plan.

## Rollback Strategy

Feature branch, plain revert. New config fields are ignored by older binaries (serde), the blob is embedded and carries no persisted state, and the About window is additive UI. Reverting Step 1 alone restores the old sun path, which was kept as the test reference and therefore never deleted.
