# Sunlit Earth

**Website:** `sunlit.earth`

A free software desktop application that renders a realistic 3D view of Earth as seen from space and sets it as the desktop wallpaper. The image updates every few minutes, reflecting the current time of day, season, cloud cover, and other real-world conditions.

## Goals

1. **Realistic Earth rendering** - A 3D globe with day/night cycle, seasonal vegetation, city lights on the night side, atmospheric effects, and real-time cloud overlay from satellite data.
2. **Arbitrary camera position** - Users can choose any viewpoint (e.g. centered on Europe/Germany), unlike satellite imagery apps which are locked to fixed geostationary positions above the equator.
3. **Cross-platform** - Windows 11 is the primary target, with Linux and macOS as secondary platforms.
4. **Lightweight background operation** - Runs as a system tray app with minimal CPU/RAM usage when idle. Renders a new frame every few minutes, saves it to disk, and sets it as the wallpaper via OS APIs.
5. **User-friendly settings UI** - A configuration window for camera position, update interval, and rendering options.
6. **Graceful degradation** - Works on systems without a GPU by falling back to software rendering.

## Motivation

The original [DesktopEarth](https://web.archive.org/web/20221006113849/http://www.anka.me/desktopearth.aspx) had all of these features but was abandoned and broke on Windows 11. No existing free/open-source alternative combines a rendered 3D globe, arbitrary camera angles, and cross-platform support. Satellite imagery apps (SpaceEye, Satpaper) can't provide good views of regions like Europe due to geostationary orbit limitations. Commercial options (EarthView, EarthDesk) exist but are proprietary and paid.

## Technology Stack (Option A)

| Component | Technology | Notes |
|---|---|---|
| Language | **Rust** | Single static binary, no runtime, lowest memory footprint (~5-15MB idle) |
| 3D rendering | **wgpu** | WebGPU-based abstraction over Vulkan/Metal/DX12/OpenGL. Built-in software fallback via `force_fallback_adapter` (WARP on Windows, lavapipe on Linux) |
| GUI | **Slint** | Declarative UI via `.slint` DSL. Official wgpu integration (v1.12+). Built-in software renderer. Good accessibility and IME support. Licensed GPLv3 (free for open source) |
| Astronomy | **Astronomy Engine** (C, via FFI) | Sun/moon/planet positions, eclipses, coordinate transforms. MIT licensed, actively maintained. Used via the `astronomy-engine-bindings` Rust crate |
| Earth textures | **NASA Blue Marble Next Generation** | Monthly equirectangular images for seasonal vegetation and snow cover |
| Cloud data | TBD | Real-time cloud cover overlay from satellite sources (to be researched) |
| Wallpaper API | OS-specific | Windows: `SystemParametersInfo`. Linux: `gsettings`/DBus. macOS: `osascript`/`NSWorkspace` |

## Rendering Approach

The application renders a textured sphere (Earth) using wgpu shaders with multiple composited layers:

1. **Base texture** - NASA Blue Marble image for the current month (seasonal vegetation, snow, ice)
2. **Day/night terminator** - Computed from the sun's position at the current date/time
3. **City lights** - Shown on the night side, blended based on terminator position
4. **Cloud overlay** - Semi-transparent layer from near-real-time satellite data
5. **Atmosphere** - Subtle glow at the limb of the Earth

The rendered frame is saved to an image file and set as the desktop wallpaper via OS APIs.

### Future rendering goals

- Star field background with astronomically correct positions
- Moon rendered at correct position and phase
- Visible planets at correct positions
- Eclipse visualization
- Dynamic snow cover based on live snow coverage data (instead of static seasonal textures)

## Architecture

- **System Tray App**
  - **Scheduler** - triggers render every N minutes
  - **Astronomy Engine** - computes sun/moon/planet positions
  - **wgpu Renderer** - renders Earth to an offscreen texture
    - Textures: Blue Marble, city lights, clouds
    - Shaders: sphere, day/night, atmosphere
  - **Image Export** - saves rendered frame to PNG/JPG
  - **Wallpaper Setter** - OS-specific API to set desktop background
  - **Slint Settings UI** - camera position, interval, options
    - 3D preview (wgpu texture imported as Slint image)

## Distribution

- Single static binary, ~5-15MB
- No runtime dependencies
- Bundled textures (Blue Marble images) shipped alongside the binary or downloaded on first run
- Installable via GitHub releases; potential future packaging for winget, Flatpak, etc.
