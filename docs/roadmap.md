# Roadmap

Features and improvements planned for Sunlit Earth, roughly ordered by priority within each category.

## Wallpaper app

- [x] Wallpaper export and setting: save rendered frame to an image file and set it as the desktop wallpaper via OS APIs. Windows (`SystemParametersInfoW` via `windows-sys`) implemented. Off Windows the sink returns a plain "not supported on this platform yet" that the status line shows; the headless `render` subcommand is the cross-platform mode today.
- [ ] Periodic re-rendering: timer-driven scheduler that re-renders every N minutes so the terminator tracks the sun. Update interval should be user-configurable.
- [x] System tray icon: minimize to tray on window close with "Open" / "Exit" context menu, single-instance enforcement, `--windowed` flag for original close-exits behavior (Windows only).
- [x] System tray extended features: "Refresh Now" and a checkable "Auto-refresh" entry are in the tray menu, and the engine runs headlessly with no window at all (the `render` subcommand creates no window and no Slint backend). A long-running daemon mode with no settings window is still open.
- [x] Configuration persistence: save and load settings (camera position, update interval, rendering options) between launches.
- [ ] Wallpaper setting on Linux and macOS: extend the wallpaper setter to support GNOME/KDE (`gsettings`/DBus), X11/Wayland, and macOS (`osascript`/`NSWorkspace`). Each setter brings the native display query with it, which also replaces two placeholders Phase 2 left behind: the 2560x1440 default in `wallpaper_sink` and the coordinate-range window-position check in `config`.
- [ ] Multi-monitor support: detect monitor layout and resolution, render appropriately sized wallpapers for each display.

## Rendering

- [x] Blue Marble water brightness: ocean areas are too dark and sometimes contain satellite imagery. Pre-process textures to get a uniform ocean color, preserving different extents of sea ice per season
- [x] Color correction: Rendered textures still look darker than their sources. Investigate if there is a color space issue to resolve and provide in-app texture processing to adjust e.g. gamma and saturation (current image may be oversaturated)
- [x] Water on the day side looks matte and more like a solid surface that happens to be blue. Investigate what kind of effect can be applied to make it look more like actual water. First iteration: specular sun glint (Blinn-Phong). Second iteration: Schlick Fresnel for specular modulation and diffuse color shift toward sky at grazing angles, with UI sliders for mix strength and extent.
- [ ] Seasonal texture switching: auto-select from the 12 monthly NASA Blue Marble variants based on the current month.
- [x] Atmosphere glow / Airglow: three physically-motivated layers (Rayleigh scattering, orange nightglow, green nightglow) with separate controls.
- [x] Cloud overlay: semi-transparent cloud layer from near-real-time satellite data. Requires researching data sources (GOES/Himawari composites, etc.) and building a download + caching pipeline.
- [ ] Make clouds less intense over land, since our cloud data sources have a bias towards creating a haze of clouds over land.
- [ ] Memory budget management: MSAA 8x at 4K uses 600+ MB in render textures (see [notes.md](notes.md)). Auto-reduce MSAA or cap resolution based on available memory. Partly addressed by the Phase 1 quality tiers, which cap the sample count and the preview width per tier (low, medium, high) and default to low in debug builds; what remains is choosing the tier from the memory actually available rather than from the build profile.
- [ ] Star field: astronomically correct background stars.
- [ ] Moon: rendered at correct position and phase.
- [ ] Visible planets: at correct positions.
- [ ] Eclipse visualization.
- [x] Specular highlights on oceans.

## Camera and controls

- [x] Offset / pan: shift the camera north/south or east/west so the Earth doesn't have to be centered in the frame.
- [x] Tilt: rotate the camera around its view axis for angled compositions.
- [x] Zoom curve: zoom should be slower when close to the Earth and faster when far away. Extend the zoom range limits.
- [x] Improved Controls: Rotate Earth with mouse drag, Zoom with scroll wheel in addition to existing UI controls.
- [x] Preset camera views: quick-select buttons for common viewpoints (Europe, Americas, Asia, etc.), as well as recreations of iconic photographs (blue marble, earthrise). Implemented as a 3x3 grid of preset buttons with `PRESETS` constant array.
- [x] Change date and time of day: checkbox to enable custom date/time, sliders for hour (UTC) and day of year, year dropdown. Implemented with proper leap year handling and live sun position updates.

## CI and distribution

- [x] CI pipeline: GitHub Actions workflows for building, testing, and linting on Windows. GPU integration tests run on the software adapter. Release workflow builds and publishes Windows binaries on version tags.
- [x] CI lint gates: the codebase-wide reformat and the warning cleanup landed in Phase 2, so `ci.yml` runs a `fmt` job again and sets `RUSTFLAGS: "-D warnings"`. Clippy stays a local command: its artifacts do not share the test cache and running it in CI would force a full recompile.
- [x] Cross-platform build and test: the workspace builds, tests, and renders headlessly on Linux (lavapipe) and macOS (Metal), and CI is a three-OS matrix. Phase 2, see [plans/2026-08-15-phase2-cross-platform-plan.md](plans/2026-08-15-phase2-cross-platform-plan.md).
- [ ] Cross-platform release builds: `release.yml` still builds Windows only. Produce binaries for Linux and macOS too and publish them as GitHub release artifacts.
- [ ] Golden references on macOS: the `metal` reference set is generated through `golden.yml`, which needs the workflow on the default branch before it can be dispatched. Until it exists the golden test skips on macOS.

## Bugs and polish

- [ ] Memory leak in tray mode: decoded 8K cloud textures (~134 MB each) accumulate in the unbounded texture channel because the only consumer (`process_decoded_textures`) runs in `BeforeRendering`, which stops firing while the window is hidden. Observed: 7.3 GB RSS after 10 days. Analysis in [retrospective-2026-08.md](retrospective-2026-08.md) section 4.1. **Fix landed** (`TextureMailbox` plus a 5-second drain timer, see [plans/2026-08-15-phase0-memory-leak-plan.md](plans/2026-08-15-phase0-memory-leak-plan.md)): the e2e regression test `test_hidden_window_cloud_updates_do_not_grow_memory` went from 120.4 MiB to 1.8 MiB of private-bytes growth across 15 hidden cloud updates. Left unchecked pending the multi-day validation in retrospective section 8.2.
- [x] Non-blocking texture loading: the main window used to be unresponsive while textures loaded, because mipmap generation and GPU upload ran on the UI thread inside `BeforeRendering` and the cached cloud JPEG was decoded synchronously at startup. Resolved by the Phase 1 restructure ([plans/2026-08-15-phase1-restructure-plan.md](plans/2026-08-15-phase1-restructure-plan.md)): decode and mip generation happen on engine-owned threads, and the UI thread only copies finished pixel buffers into a `slint::Image`.
- [x] Diffuse shading banding on JPEG wallpapers: fixed by switching the wallpaper export format from TIFF to PNG. Windows preserves PNG wallpapers losslessly (no JPEG transcode), eliminating the banding artifact.
- [x] Refactor `main()`: extract mouse math into `mouse_math.rs` (with unit tests and proptests), UI callback registration into `ui_callbacks.rs`, and initialization into sub-functions. Removed `clippy::too_many_lines` suppression.
- [ ] Automated slint UI testing
- [ ] `Renderer::textures_ready` never becomes true when a texture is missing or fails to decode, so `TexturesReady` is an event that cannot arrive and every client waiting on it waits forever. A failed decode clears `source_path` so it is not retried, which leaves the slot in exactly the state an unconfigured slot is in: no bind group, no path, not loading. Both are terminal and both should read as ready. Blend mode additionally requires `composite_bind_group`, which can never be built if either texture is absent, so that condition needs the same treatment. Surfaced in Phase 2 by CI, where `textures/**` is Git LFS and `actions/checkout` leaves pointer files that fail to decode as JXL; `run_render` has a local guard for the missing-file half, and the smoke step points at an empty textures directory to keep CI deterministic. The general fix belongs with the asset-pipeline work.
