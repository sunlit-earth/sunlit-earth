# Research: Wallpaper Export (2026-03-16)

## Problem Statement

Sunlit Earth needs a "Set as Wallpaper" feature that renders the current 3D Earth scene at the user's desktop resolution, saves it to a PNG file, and sets it as the Windows desktop wallpaper. This requires extending the existing wgpu render pipeline to support arbitrary-resolution off-screen rendering, reading back pixel data from the GPU, encoding the image to a file, and invoking the Windows wallpaper API.

## Requirements

- Render the current scene (camera position, sun direction, textures, shading parameters) at the desktop's native resolution, independent of the Slint window size.
- Save the rendered frame as a lossless image file (TIFF with LZW compression preferred — fast encoding, future 16-bit HDR path).
- Set the saved image as the Windows desktop wallpaper, persisting across reboots.
- Detect the primary monitor's physical resolution for the render target size.
- Add a UI button in the controls panel to trigger the export.
- Must work within the existing `thread_local! { RefCell<Option<GpuResources>> }` ownership model.

## Findings

### GPU Rendering at Arbitrary Resolution

The current render resolution is tightly coupled to the Slint window via `quantized_viewport_size()`, which reads window properties, scales by `scale_factor()`, and quantizes to 64px boundaries. All size-dependent resources (render texture, depth texture, MSAA textures) live in `GpuResources`.

For wallpaper export, a separate set of render textures must be created at the target resolution. The recommended approach is **Approach B from the codebase research**: create temporary textures at the target resolution, reuse the existing pipeline and bind groups, render, read back, and discard.

**Resources that can be reused** (resolution-independent): pipeline (if same `sample_count`), vertex/index buffers, uniform buffer, all bind groups, sampler, shader, pipeline layout, device, queue.

**Resources that must be created fresh**: render texture (`Rgba8Unorm`, with `RENDER_ATTACHMENT | COPY_SRC`), depth texture (`Depth32Float`, with `RENDER_ATTACHMENT`). MSAA textures can be skipped by using `sample_count=1` for the export pass, simplifying the implementation. If MSAA is desired at export resolution, a temporary non-MSAA pipeline would need to be created unless the current pipeline already uses `sample_count=1`.

The aspect ratio passed to `OrbitalCamera::mvp_matrix(aspect)` must use the target resolution's aspect ratio, not the window's.

### Render Texture Usage Flags

The current render texture is created with `RENDER_ATTACHMENT | TEXTURE_BINDING` but **lacks `COPY_SRC`**, which is required for GPU-to-CPU pixel readback. The integration tests (`tests/render_pipeline.rs`) already create textures with `RENDER_ATTACHMENT | COPY_SRC`, so the pattern is established in the codebase.

For export, the temporary render texture must include `COPY_SRC`. The production render texture does not need to change (its readback goes through Slint's `Image::try_from(Texture)` path, not a buffer copy).

### Pixel Readback from GPU

The full readback pattern is demonstrated in `tests/common/mod.rs::read_texture_rgba8()`:

1. Create a staging buffer with `MAP_READ | COPY_DST`.
2. `encoder.copy_texture_to_buffer()` with 256-byte row alignment (`wgpu::COPY_BYTES_PER_ROW_ALIGNMENT`).
3. `queue.submit()`, `buffer_slice.map_async()`, `device.poll(Wait)`, `get_mapped_range()`.
4. Strip row padding if `bytes_per_row != width * 4`.

This function currently lives in the test crate (`tests/common/mod.rs`). It will need to be moved or duplicated into the main crate for production use.

### Image Encoding and File Format

The `image` crate (v0.25.8) is already a dependency but has `default-features = false`, which **includes no encoders**. To save PNG files, the `png` feature must be enabled on the `image` dependency. The `image` crate delegates PNG encoding to the `png` crate, which uses `fdeflate` for fast encoding -- a 2560x1440 frame should encode in well under 1 second.

The pixel data from GPU readback is RGBA8 (`Vec<u8>`, 4 bytes/pixel), directly compatible with `image::RgbaImage::from_raw(width, height, pixels)`, which can then be saved with `.save("path.png")`.

**Format comparison for wallpaper use:**

| Format | Documented for wallpaper | Works in practice | File size (2560x1440) | Lossless | 16-bit HDR path |
|--------|--------------------------|-------------------|-----------------------|----------|-----------------|
| BMP    | Yes                      | Yes               | ~14 MB                | Yes      | No              |
| JPEG   | Yes (Vista+)             | Yes               | Small                 | No       | No              |
| PNG    | No (undocumented)        | Yes (Win 10/11)   | ~3-8 MB               | Yes      | Yes             |
| TIFF   | Yes (official .theme spec)| Yes (Win 10/11)  | ~5-10 MB (LZW)       | Yes      | Yes             |

**Recommendation:** Write TIFF with LZW compression. TIFF is officially documented in the Windows Theme File Format specification (`.bmp, .gif, .jpg, .png, or .tif`). LZW encoding is fast (no deflate overhead). TIFF natively supports 16-bit-per-channel data, providing a clean upgrade path for HDR support. WIC has a native TIFF codec since Windows Vista.

### Windows Wallpaper API

**Primary API: `SystemParametersInfoW`**

The canonical Win32 function for setting the desktop wallpaper, from `user32.dll`:

- Action: `SPI_SETDESKWALLPAPER` (0x0014)
- `pvParam`: full absolute path to image file as null-terminated UTF-16 string
- `fWinIni`: `SPIF_UPDATEINIFILE | SPIF_SENDCHANGE` (persists across reboots and broadcasts update)
- Path must be absolute and within `MAX_PATH` length
- File must exist on disk before the call

**Known quirk (High confidence):** As of Windows 10/11 22H2, `SystemParametersInfoW` returns `TRUE` even when the file path does not exist or is not a valid image, setting the background to solid black. Mitigation: verify the file exists and is non-empty before calling.

**A vs W anomaly (Low confidence, single source):** One blog post from ~2018 reported that the wide (W) variant produced a black wallpaper while the ASCII (A) variant worked. This is likely outdated or environment-specific. Safe mitigation: ensure the file path contains only ASCII characters.

**Alternative: `IDesktopWallpaper` COM interface (Windows 8+)**

A richer COM interface supporting per-monitor wallpaper and style control. Relevant for future multi-monitor support but adds COM complexity. Requires the `windows` crate (not `windows-sys`) with `Win32_UI_Shell` feature. Not needed for the initial implementation.

### Rust Crate for Windows API

Three options were evaluated:

| Crate | Status | COM support | Build speed | Recommendation |
| --- | --- | --- | --- | --- |
| `windows-sys` 0.61 | Active, Microsoft-maintained | No | Fast | **Recommended for initial implementation** |
| `windows` 0.62 | Active, Microsoft-maintained | Yes | Slower | Needed only if using `IDesktopWallpaper` |
| `winapi` 0.3 | Effectively unmaintained | No | -- | Do not use |

Required `windows-sys` features:

- `Win32_UI_WindowsAndMessaging` -- `SystemParametersInfoW`, `GetSystemMetrics`, `SPI_SETDESKWALLPAPER`
- `Win32_Graphics_Gdi` -- `EnumDisplayMonitors`, `GetMonitorInfoW`, `MONITORINFOEXW`
- `Win32_Foundation` -- `BOOL`, `HWND`, `RECT`, etc.

All calls are `unsafe`. Path encoding uses `std::os::windows::ffi::OsStrExt::encode_wide()`.

### Screen Resolution Detection

**Primary monitor resolution** (recommended target for rendering):

Query via `EnumDisplayMonitors` + `GetMonitorInfoW`. The monitor with the `MONITORINFOF_PRIMARY` flag set provides `rcMonitor: RECT { left, top, right, bottom }` in the virtual screen coordinate space. Width = `right - left`, height = `bottom - top`.

**DPI caveat:** `GetSystemMetrics` returns logical pixels unless the process declares DPI awareness. For accurate physical pixel values, the application should declare `DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2` (Windows 10 1703+). Sunlit Earth does not currently declare a DPI manifest -- this may cause resolution query to return scaled-down values on high-DPI displays.

**Alternative:** `EnumDisplayDevices` always returns physical resolution regardless of DPI awareness settings.

**Virtual screen dimensions** (all monitors combined): `GetSystemMetrics(SM_CXVIRTUALSCREEN)` / `SM_CYVIRTUALSCREEN`. Useful for "span" mode but not needed initially.

### Multi-Monitor Strategy

When `SystemParametersInfoW` sets a single image, Windows stretches, tiles, or fills based on the active wallpaper style (a separate registry setting). For Sunlit Earth's single-sphere scene:

- **Initial recommendation:** Render at the primary monitor's physical resolution. Windows "Fill" mode will handle display on each monitor. This avoids doubling/tripling render cost for multi-monitor virtual screen sizes.
- **Future enhancement:** Use `IDesktopWallpaper` COM interface for per-monitor wallpaper at each monitor's native resolution.

(Confidence: Medium -- multi-monitor wallpaper behavior depends on many user-controlled variables.)

### File Save Location

**Recommended: `%LOCALAPPDATA%\SunlitEarth\wallpaper.png`**

- Unambiguously owned by the application
- Stable (not cleaned by temp directory sweeps)
- Non-roaming (not synced across machines on domain profiles)
- Easy to locate for cleanup on uninstall
- Create directory with `std::fs::create_dir_all` if needed
- Obtain via `std::env::var("LOCALAPPDATA")`

Alternatives considered: `%APPDATA%\Microsoft\Windows\Themes\` (conventional but pollutes Windows folder), `%TEMP%` (risk of deletion by disk cleaners).

### UI Integration

The Slint UI (`ui/main.slint`) has a controls panel with a `VerticalLayout` (alignment: start). A "Set as Wallpaper" button fits naturally after the existing controls, before the renderer info text (which is absolutely positioned at the panel bottom).

The callback pattern is established: define `callback set-wallpaper()` in Slint, add a `Button` that triggers it, connect in Rust with `window.on_set_wallpaper(move || { ... })`.

### GpuResources Access Model

`GpuResources` lives in a `thread_local! { RefCell<Option<GpuResources>> }` and can only be accessed from the UI thread. The simplest approach is to run the export synchronously on the UI thread within `GPU_RESOURCES.with(|r| { ... })`:

1. Borrow `GpuResources` from the thread-local.
2. Create temporary render textures at the target resolution.
3. Write uniforms with the correct aspect ratio.
4. Execute a render pass using the existing pipeline and bind groups.
5. Read back pixels via staging buffer.
6. Release the thread-local borrow.
7. Encode to PNG and save to disk (outside the borrow).
8. Call `SystemParametersInfoW` to set the wallpaper.

The render itself is fast (single draw call for a sphere). File I/O and PNG encoding happen after releasing the GPU borrow.

## Technical Constraints

- `unsafe_code = "deny"` in Cargo.toml: the Windows API calls (`SystemParametersInfoW`, `GetSystemMetrics`, `EnumDisplayMonitors`, `GetMonitorInfoW`) are all `unsafe`. They will need scoped `#[allow(unsafe_code)]` annotations, following the precedent set by `sun.rs` for FFI calls.
- The `image` crate currently has `default-features = false` with no encoders. The `png` feature must be added for PNG encoding.
- Render texture readback requires `COPY_SRC` usage flag, which the production render texture does not have. Export textures must be created with this flag.
- Windows path encoding: `SystemParametersInfoW` requires null-terminated UTF-16. Use `OsStr::encode_wide()` chain with `.chain(Some(0))`.
- `MAX_PATH` limit applies to the wallpaper file path.
- DPI awareness may affect resolution detection accuracy on high-DPI displays. Needs testing.
- The `windows-sys` crate should be gated with `cfg(windows)` to maintain cross-platform compilability of the rest of the codebase.

## Open Questions

1. **DPI awareness:** Sunlit Earth does not declare a DPI manifest. Does `GetMonitorInfoW` return physical or logical pixels without one? Does using `EnumDisplayDevices` avoid this issue entirely? Needs direct testing on a high-DPI display.
2. **A vs W variant behavior:** One source reported `SystemParametersInfoW` producing a black wallpaper while `SystemParametersInfoA` worked. Is this reproducible on modern Windows 11? Likely outdated but worth a quick test.
3. **PNG wallpaper support longevity:** PNG is undocumented for `SPI_SETDESKWALLPAPER` but works in practice. Should a BMP fallback be implemented from the start, or deferred until/unless PNG fails?
4. **Export during texture loading:** Resolved — always render a fresh image when the user requests it. If textures are still loading, wait for them to complete before rendering.
5. **MSAA for export:** Resolved — use MSAA at the same sample count as the preview. Never lose quality compared to the preview image.
6. **Wallpaper style control:** Resolved — Sunlit Earth must ensure "Fill" mode is set when applying the wallpaper.

## Recommendations

| Decision | Recommendation | Rationale |
| --- | --- | --- |
| Wallpaper API | `SystemParametersInfoW` via `windows-sys` | Simplest, no COM overhead, upgradeable to `IDesktopWallpaper` later |
| Windows crate | `windows-sys` 0.61 with `cfg(windows)` gate | Fast build, raw FFI sufficient for the needed calls |
| Resolution target | Primary monitor physical resolution | Covers the common case without multi-monitor complexity |
| Image format | TIFF with LZW compression | Officially documented, fast encoding, 16-bit HDR path |
| Image encoding | Enable `tiff` feature on existing `image` crate | No new dependency needed |
| File location | `%LOCALAPPDATA%\SunlitEarth\wallpaper.png` | Application-owned, stable, non-roaming |
| Pixel readback | Move/adapt `read_texture_rgba8` from test crate to main crate | Pattern already proven in integration tests |
| Export execution | Synchronous on UI thread within `GPU_RESOURCES.with()` borrow | Single draw call is fast; avoids cross-thread GPU resource sharing |
| MSAA for export | Same `sample_count` as preview | Never lose quality compared to the preview image |
| UI button | Controls panel, after existing controls | Follows established callback pattern |

## Sources

| Document | Focus Area |
| --- | --- |
| `docs/plans/2026-03-16-wallpaper-export-codebase.md` | Current rendering pipeline, GPU resource model, resolution coupling, pixel readback, UI structure, dependency analysis |
| `docs/plans/2026-03-16-wallpaper-export-external.md` | Windows wallpaper API, Rust crates for Win32, screen resolution detection, multi-monitor strategy, image format support, file save location |
