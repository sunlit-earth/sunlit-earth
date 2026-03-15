# Roadmap

Features and improvements planned for Sunlit Earth, roughly ordered by priority within each category.

## Wallpaper app

- [ ] Wallpaper export and setting: save rendered frame to an image file and set it as the desktop wallpaper via OS APIs. Windows 11 (`SystemParametersInfo`) first; Linux and macOS later.
- [ ] Periodic re-rendering: timer-driven scheduler that re-renders every N minutes so the terminator tracks the sun. Update interval should be user-configurable.
- [ ] System tray and background operation: minimize to tray with a status menu ("Render Now", "Open Settings", "Quit"). Support headless/daemon mode without the GUI window.
- [ ] Configuration persistence: save and load settings (camera position, update interval, rendering options) between launches.
- [ ] Wallpaper setting on Linux and macOS: extend the wallpaper setter to support GNOME/KDE (`gsettings`/DBus), X11/Wayland, and macOS (`osascript`/`NSWorkspace`).
- [ ] Multi-monitor support: detect monitor layout and resolution, render appropriately sized wallpapers for each display.

## Rendering

- [ ] Blue Marble water brightness: ocean areas are too dark. Pre-process textures (gamma correction, levels adjustment, or a color ramp on water pixels).
- [ ] Seasonal texture switching: auto-select from the 12 monthly NASA Blue Marble variants based on the current month.
- [ ] Atmosphere glow: subtle blue/orange glow at the Earth's limb.
- [ ] Cloud overlay: semi-transparent cloud layer from near-real-time satellite data. Requires researching data sources (GOES/Himawari composites, etc.) and building a download + caching pipeline.
- [ ] Memory budget management: MSAA 8x at 4K uses 600+ MB in render textures (see [notes.md](notes.md)). Auto-reduce MSAA or cap resolution based on available memory.
- [ ] Star field: astronomically correct background stars.
- [ ] Moon: rendered at correct position and phase.
- [ ] Visible planets: at correct positions.
- [ ] Eclipse visualization.
- [ ] Specular highlights on oceans.

## Camera and controls

- [ ] Offset / pan: shift the camera north/south or east/west so the Earth doesn't have to be centered in the frame.
- [ ] Tilt: rotate the camera around its view axis for angled compositions.
- [ ] Zoom curve: zoom should be slower when close to the Earth and faster when far away. Extend the zoom range limits.
- [ ] Improved Controls: Rotate Earth with mouse drag, Zoom with scroll wheel in addition to existing UI controls.
- [ ] Preset camera views: quick-select buttons for common viewpoints (Europe, Americas, Asia, etc.).

## Bugs and polish

- [ ] Non-blocking texture loading: the main window is unresponsive while textures load (can't move or resize). Texture decoding runs on a background thread, but something still blocks the UI thread.
