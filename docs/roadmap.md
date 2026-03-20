# Roadmap

Features and improvements planned for Sunlit Earth, roughly ordered by priority within each category.

## Wallpaper app

- [x] Wallpaper export and setting: save rendered frame to an image file and set it as the desktop wallpaper via OS APIs. Windows (`SystemParametersInfoW` via `windows-sys`) implemented; Linux and macOS later.
- [ ] Periodic re-rendering: timer-driven scheduler that re-renders every N minutes so the terminator tracks the sun. Update interval should be user-configurable.
- [ ] System tray and background operation: minimize to tray with a status menu ("Render Now", "Open Settings", "Quit"). Support headless/daemon mode without the GUI window.
- [x] Configuration persistence: save and load settings (camera position, update interval, rendering options) between launches.
- [ ] Wallpaper setting on Linux and macOS: extend the wallpaper setter to support GNOME/KDE (`gsettings`/DBus), X11/Wayland, and macOS (`osascript`/`NSWorkspace`).
- [ ] Multi-monitor support: detect monitor layout and resolution, render appropriately sized wallpapers for each display.

## Rendering

- [x] Blue Marble water brightness: ocean areas are too dark and sometimes contain satellite imagery. Pre-process textures to get a uniform ocean color, preserving different extents of sea ice per season
- [x] Color correction: Rendered textures still look darker than their sources. Investigate if there is a color space issue to resolve and provide in-app texture processing to adjust e.g. gamma and saturation (current image may be oversaturated)
- [x] Water on the day side looks matte and more like a solid surface that happens to be blue. Investigate what kind of effect can be applied to make it look more like actual water. First iteration: specular sun glint (Blinn-Phong). Second iteration: Schlick Fresnel for specular modulation and diffuse color shift toward sky at grazing angles, with UI sliders for mix strength and extent.
- [ ] Seasonal texture switching: auto-select from the 12 monthly NASA Blue Marble variants based on the current month.
- [ ] Atmosphere glow / Airglow: subtle blue/orange glow at the Earth's limb.
- [ ] Cloud overlay: semi-transparent cloud layer from near-real-time satellite data. Requires researching data sources (GOES/Himawari composites, etc.) and building a download + caching pipeline.
- [ ] Memory budget management: MSAA 8x at 4K uses 600+ MB in render textures (see [notes.md](notes.md)). Auto-reduce MSAA or cap resolution based on available memory.
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
- [ ] Preset camera views: quick-select buttons for common viewpoints (Europe, Americas, Asia, etc.), as well as recreations of iconic photographs (blue marble, earthrise) 
- [x] Change date and time of day: checkbox to enable custom date/time, sliders for hour (UTC) and day of year, year dropdown. Implemented with proper leap year handling and live sun position updates.

## CI and distribution

- [x] CI pipeline: GitHub Actions workflows for building, testing, and linting on Windows. GPU integration tests run on the software adapter. Release workflow builds and publishes Windows binaries on version tags.
- [ ] CI format check: the `fmt` job in `ci.yml` is commented out. The codebase needs a one-time reformat with current rustfmt (1.8.0+). Steps: optionally add a `rustfmt.toml` with `style_edition`, run `cargo fmt`, commit, uncomment the `fmt` job. Similar issue with Clippy, `RUSTFLAGS: "-D warnings"` was commented out in `ci.yml`.
- [ ] Cross-platform release builds: produce binaries for Windows, Linux, and macOS from CI. Publish as GitHub release artifacts.

## Bugs and polish

- [ ] Non-blocking texture loading: the main window is unresponsive while textures load (can't move or resize). Texture decoding runs on a background thread, but something still blocks the UI thread.
- [x] Diffuse shading banding on JPEG wallpapers: fixed by switching the wallpaper export format from TIFF to PNG. Windows preserves PNG wallpapers losslessly (no JPEG transcode), eliminating the banding artifact.
- [ ] Automated slint UI testing
