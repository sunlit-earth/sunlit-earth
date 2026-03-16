# Plan: Wallpaper Export (2026-03-16)

## Summary

Add a "Set as Wallpaper" feature to Sunlit Earth that renders the current scene at the primary monitor's physical resolution, saves the result as a TIFF file with LZW compression, and sets it as the Windows desktop wallpaper using the Win32 `SystemParametersInfoW` API. The implementation adds a new `wallpaper` module (gated with `cfg(windows)`), a public `export_wallpaper` function callable from the renderer's thread-local context, a new `read_texture_rgba8` utility in the main crate (adapted from the test helper), and a "Set as Wallpaper" button in the Slint UI. The wallpaper file is written to `%LOCALAPPDATA%\SunlitEarth\wallpaper.tif`.

## Stakes Classification

**Level**: High
**Rationale**: This plan introduces `unsafe` Win32 FFI calls (with `unsafe_code = "deny"` globally), creates temporary GPU resources at a different resolution from the preview, adds a new production dependency (`windows-sys`), adds a new image encoder dependency (`tiff` feature on the `image` crate), modifies the Slint UI, and extends the renderer's public API. The GPU readback path (staging buffer, row alignment, `device.poll(Wait)`) is proven in tests but has never run in production code. A mistake in the render texture flags, row alignment math, or Win32 path encoding could produce a black wallpaper or silently fail. The changes span 8+ files across 4 layers (dependencies, GPU, filesystem, Win32 API).

## Context

**Research**: [`docs/plans/2026-03-16-wallpaper-export-research.md`](2026-03-16-wallpaper-export-research.md)
**Affected Areas**: `Cargo.toml`, `src/lib.rs`, `src/main.rs`, `src/wallpaper.rs` (new), `src/renderer/mod.rs`, `src/renderer/render_pass.rs`, `src/renderer/gpu_setup.rs`, `ui/main.slint`, `CLAUDE.md`

## Success Criteria

- [ ] `windows-sys` is added as a `cfg(windows)` dependency with features `Win32_UI_WindowsAndMessaging`, `Win32_Graphics_Gdi`, and `Win32_Foundation`
- [ ] The `tiff` feature is enabled on the existing `image` crate dependency
- [ ] A new `src/wallpaper.rs` module exists, gated with `#[cfg(windows)]`, containing all Win32 FFI calls behind scoped `#[allow(unsafe_code)]` annotations
- [ ] `wallpaper::get_primary_monitor_resolution()` returns the primary monitor's physical pixel dimensions
- [ ] `wallpaper::set_wallpaper(path)` calls `SystemParametersInfoW` with `SPI_SETDESKWALLPAPER` and `SPIF_UPDATEINIFILE | SPIF_SENDCHANGE`, persisting the wallpaper across reboots
- [ ] `wallpaper::set_wallpaper` ensures "Fill" wallpaper style before setting the image
- [ ] The wallpaper file is saved to `%LOCALAPPDATA%\SunlitEarth\wallpaper.tif`, creating the directory if needed
- [ ] A `read_texture_rgba8` function exists in the main crate (adapted from `tests/common/mod.rs`) for GPU-to-CPU pixel readback
- [ ] The export render pass creates temporary textures at the target resolution with `COPY_SRC` usage, reuses the existing pipeline and bind groups, uses the same `sample_count` as the preview, and renders with the correct aspect ratio
- [ ] If textures are still loading when the user clicks "Set as Wallpaper", the export waits for them to finish
- [ ] A "Set as Wallpaper" button appears in the Slint UI controls panel
- [ ] Clicking the button renders, saves, and sets the wallpaper (the full pipeline works end-to-end)
- [ ] `cargo build` succeeds with no warnings on Windows
- [ ] `cargo clippy` passes
- [ ] `cargo test` passes (existing tests unbroken, new unit tests pass)
- [ ] Cross-compilation check: `cargo check` succeeds on a non-Windows target (the `cfg(windows)` gate compiles away cleanly)

## Implementation Steps

### Phase 1: Dependencies

This phase adds the new crate dependencies and the image encoder feature needed for the rest of the implementation.

#### Step 1.1: Add `windows-sys` dependency and enable `tiff` image feature

- **Files**: `Cargo.toml`
- **Action**: Add `windows-sys` as a platform-specific dependency with the required features. Enable the `tiff` feature on the existing `image` crate so TIFF encoding is available.

  Add under `[dependencies]`:

  ```toml
  image = { version = "0.25.8", default-features = false, features = ["tiff"] }
  ```

  Add a new target-specific section:

  ```toml
  [target.'cfg(windows)'.dependencies]
  windows-sys = { version = "0.59", features = [
      "Win32_UI_WindowsAndMessaging",
      "Win32_Graphics_Gdi",
      "Win32_Foundation",
  ] }
  ```

- **Verify**: `cargo check` succeeds. `cargo tree -i windows-sys` shows the crate with the expected features.
- **Complexity**: Small

### Phase 2: Pixel Readback Utility

This phase moves the GPU-to-CPU readback function from the test crate into the main crate so it can be used by production code.

#### Step 2.1: Create `read_texture_rgba8` in the renderer module

- **Files**: `src/renderer/render_pass.rs`
- **Action**: Add a `pub(crate)` function `read_texture_rgba8` adapted from `tests/common/mod.rs::read_texture_rgba8()`. The function takes `device`, `queue`, `texture`, `width`, and `height` parameters and returns `Vec<u8>` of RGBA8 pixel data. The implementation is identical to the test version: create a staging buffer with `MAP_READ | COPY_DST`, copy texture to buffer with 256-byte row alignment, submit, poll with `Wait`, map, strip row padding, and return the pixel data.

  Place it in `render_pass.rs` because this module already handles render pass encoding and texture operations. Mark it `pub(crate)` so it is accessible from the wallpaper export code but not part of the public API.

- **Test cases**:
  - This function is already exhaustively tested indirectly by the GPU integration tests in `tests/render_pipeline.rs` and `tests/shading.rs` (which use the identical logic from `tests/common/mod.rs`). The code is a direct copy with no behavioral changes. New unit tests are not practical here because the function requires a real GPU device.
- **Verify**: `cargo build` succeeds. The function signature matches the test version.
- **Complexity**: Small

### Phase 3: Windows Platform Module

This phase creates the `wallpaper` module containing all Windows-specific code: monitor resolution detection, wallpaper style enforcement, and the `SystemParametersInfoW` call.

#### Step 3.1: Create `src/wallpaper.rs` with module structure and `wallpaper_dir` helper

- **Files**: `src/wallpaper.rs` (new), `src/lib.rs`
- **Action**: Create the new module file with `#[cfg(windows)]` gating. Add `#[cfg(windows)] pub mod wallpaper;` to `src/lib.rs`.

  Implement a `wallpaper_dir()` function that returns the path `%LOCALAPPDATA%\SunlitEarth\`, creating the directory with `std::fs::create_dir_all` if it does not exist. Use `std::env::var("LOCALAPPDATA")` to get the base path. Return `Result<PathBuf, String>` for error handling.

- **Test cases** (unit, in `wallpaper.rs` `#[cfg(test)]` block):
  - `wallpaper_dir_uses_localappdata`: Set the `LOCALAPPDATA` env var to a temp directory, call `wallpaper_dir()`, assert the returned path ends with `SunlitEarth` and the directory exists on disk. (Use `std::env::set_var` in a serial test, or construct the expected path from the env var without calling the function if env manipulation is risky in parallel tests.)
  - `wallpaper_path_is_tiff`: Assert that the full wallpaper path returned by the helper ends with `.tiff`.
- **Verify**: `cargo build` succeeds. `cargo test wallpaper` passes on Windows.
- **Complexity**: Small

#### Step 3.2: Implement `get_primary_monitor_resolution`

- **Files**: `src/wallpaper.rs`
- **Action**: Implement `pub fn get_primary_monitor_resolution() -> Result<(u32, u32), String>` using `EnumDisplayMonitors` + `GetMonitorInfoW`. The function enumerates all monitors, finds the one with `MONITORINFOF_PRIMARY` flag, and returns `(width, height)` from `rcMonitor`. Each FFI call site gets a scoped `#[allow(unsafe_code)]` annotation with a `// SAFETY:` comment, following the pattern established in `src/scene/sun.rs`.

  Implementation outline:
  1. Define a callback for `EnumDisplayMonitors` that receives each monitor handle and a mutable pointer to a `Vec<HMONITOR>` (passed through `lparam`). The callback pushes each handle into the vector.
  2. Call `EnumDisplayMonitors(null_mut(), null(), callback, &mut monitors as *mut _ as LPARAM)`.
  3. For each monitor handle, call `GetMonitorInfoW` with a `MONITORINFOEXW` struct (initialize `cbSize` to `size_of::<MONITORINFOEXW>()`).
  4. Find the monitor with `dwFlags & MONITORINFOF_PRIMARY != 0`.
  5. Compute `width = rcMonitor.right - rcMonitor.left`, `height = rcMonitor.bottom - rcMonitor.top`.
  6. Return `(width as u32, height as u32)`.

  All `unsafe` blocks must have `#[allow(unsafe_code)]` and `// SAFETY:` comments.

- **Test cases** (unit):
  - `primary_resolution_is_nonzero`: Call `get_primary_monitor_resolution()`, assert both width and height are > 0.
  - `primary_resolution_is_reasonable`: Assert width >= 640 and height >= 480 (any modern display).
  - These tests run only on Windows (`#[cfg(test)]` within the `#[cfg(windows)]` module).
- **Verify**: `cargo test wallpaper` passes. The returned resolution matches the system's display settings.
- **Complexity**: Medium

#### Step 3.3: Implement `ensure_fill_style`

- **Files**: `src/wallpaper.rs`
- **Action**: Implement a helper function `fn ensure_fill_style() -> Result<(), String>` that sets the Windows wallpaper display style to "Fill" (style 10, tile 0) by writing to the registry keys `HKCU\Control Panel\Desktop\WallpaperStyle` and `HKCU\Control Panel\Desktop\TileWallpaper`.

  Use `windows_sys::Win32::System::Registry` functions (`RegOpenKeyExW`, `RegSetValueExW`, `RegCloseKey`) or, more simply, use `std::process::Command` to invoke `reg.exe` for setting the two values. The registry approach is more robust; the `reg.exe` approach is simpler but adds a process spawn.

  **Recommended approach**: Use the `Win32_System_Registry` feature of `windows-sys` with `RegOpenKeyExW` / `RegSetValueExW`. Add the `Win32_System_Registry` feature to the `windows-sys` dependency in `Cargo.toml`.

  Alternatively, since `SystemParametersInfoW` with `SPI_SETDESKWALLPAPER` already triggers a wallpaper refresh, and the style is controlled by registry keys that persist, write the registry values before calling `SystemParametersInfoW`.

  The two registry values to set:
  - `HKEY_CURRENT_USER\Control Panel\Desktop\WallpaperStyle` = `"10"` (Fill)
  - `HKEY_CURRENT_USER\Control Panel\Desktop\TileWallpaper` = `"0"` (not tiled)

- **Test cases** (manual only -- modifying user registry in automated tests is invasive):
  - **Manual**: After calling `ensure_fill_style`, open `regedit` and verify the two registry values are set correctly.
- **Verify**: `cargo build` succeeds. Manual verification of registry values.
- **Complexity**: Medium

#### Step 3.4: Implement `set_wallpaper`

- **Files**: `src/wallpaper.rs`
- **Action**: Implement `pub fn set_wallpaper(path: &Path) -> Result<(), String>` that:
  1. Verifies the file exists and is non-empty (mitigating the known Windows quirk where `SystemParametersInfoW` returns `TRUE` even for missing files).
  2. Converts the path to an absolute path via `std::fs::canonicalize`.
  3. Calls `ensure_fill_style()` to set "Fill" mode.
  4. Encodes the path as null-terminated UTF-16 using `std::os::windows::ffi::OsStrExt::encode_wide().chain(Some(0))`.
  5. Calls `SystemParametersInfoW(SPI_SETDESKWALLPAPER, 0, wide_path.as_mut_ptr().cast(), SPIF_UPDATEINIFILE | SPIF_SENDCHANGE)`.
  6. Checks the return value and calls `GetLastError()` on failure.

  The `unsafe` block for `SystemParametersInfoW` gets `#[allow(unsafe_code)]` with a `// SAFETY:` comment explaining that the path is a valid null-terminated UTF-16 string pointing to an existing file.

- **Test cases** (unit):
  - `set_wallpaper_rejects_missing_file`: Call `set_wallpaper` with a non-existent path, assert it returns `Err`.
  - `set_wallpaper_rejects_empty_file`: Create a temp file with 0 bytes, call `set_wallpaper`, assert it returns `Err`.
  - Full end-to-end testing is done manually in Phase 6 (setting the wallpaper modifies user-visible system state).
- **Verify**: `cargo test wallpaper` passes. `cargo build` succeeds.
- **Complexity**: Medium

### Phase 4: Export Rendering

This phase implements the core export pipeline: rendering the scene at an arbitrary resolution, reading back pixels, and encoding to TIFF.

#### Step 4.1: Implement `export_wallpaper_image` in the renderer

- **Files**: `src/renderer/mod.rs`, `src/renderer/render_pass.rs`
- **Action**: Add a new public function `pub fn export_wallpaper_image(target_width: u32, target_height: u32) -> Result<Vec<u8>, String>` in `src/renderer/mod.rs`. This function runs synchronously on the UI thread within `GPU_RESOURCES.with(|r| { ... })`.

  Implementation outline:

  1. Borrow `GpuResources` from the thread-local. Return `Err` if `None` (GPU not initialized).
  2. Create a temporary render texture at `(target_width, target_height)` with `Rgba8Unorm` format and `RENDER_ATTACHMENT | COPY_SRC` usage flags. This is the key difference from the preview texture, which has `RENDER_ATTACHMENT | TEXTURE_BINDING` but no `COPY_SRC`.
  3. Create a temporary depth texture at `(target_width, target_height)` with `Depth32Float` format and `RENDER_ATTACHMENT` usage.
  4. If `res.sample_count > 1`, create temporary MSAA color and depth textures at the target resolution with the current `sample_count`. Also create a temporary pipeline with the same `sample_count` only if the target resolution requires different multisampling behavior -- actually, the pipeline is resolution-independent and sample-count-dependent, so **reuse the existing pipeline** as long as `sample_count` matches.
  5. Compute the camera MVP matrix using the target resolution's aspect ratio (`target_width as f32 / target_height as f32`), reading camera parameters from the last rendered `FrameState` (or current UI state).
  6. Write uniforms to the existing uniform buffer (this is safe because we are on the same thread and no other render is in flight).
  7. Encode and submit a render pass using the temporary textures, existing pipeline, and existing bind groups. The render pass structure mirrors `execute_render_pass` but targets the temporary textures.
  8. Call `read_texture_rgba8(device, queue, &temp_render_texture, target_width, target_height)` to read back the pixels.
  9. Return the pixel data as `Vec<u8>`.

  The temporary textures are dropped when the function returns, reclaiming GPU memory.

  For reading the current scene state (camera position, sun direction, texture selection, shading parameters), the function reads from the Slint window properties via `MainWindow`. Pass a `&MainWindow` reference, or read the necessary values from `GpuResources::last_state`. Using `last_state` is simpler and avoids needing a window reference, but it reflects the last rendered frame rather than the current UI state. Since the export should match what the user sees, using `last_state` is correct.

  **Texture loading wait**: If the user's selected texture mode requires textures that are still loading, the export must wait. Check `res.texture_slots[slot].loading` for the relevant slots. If any are loading, return an error (the UI layer will retry or show a message). A blocking wait is not practical because texture loading uses the `mpsc` channel that is polled in `BeforeRendering` -- busy-waiting on the UI thread would block the channel consumer.

  **Strategy for handling loading textures**: Return `Err("Textures are still loading")` if the required textures are not ready. The UI callback can display this message and the user can retry. This avoids blocking the UI thread.

- **Test cases**:
  - This function requires the full Slint rendering context and thread-local `GpuResources`, which cannot be constructed in unit tests. Testing is done via manual verification in Phase 6 and indirectly through the existing GPU integration tests that validate the render pass and readback mechanics.
- **Verify**: `cargo build` succeeds. The function compiles and is callable from `main.rs`.
- **Complexity**: Large

#### Step 4.2: Implement TIFF encoding and file save

- **Files**: `src/wallpaper.rs`
- **Action**: Add a `pub fn save_wallpaper_image(pixels: &[u8], width: u32, height: u32) -> Result<std::path::PathBuf, String>` function that:
  1. Calls `wallpaper_dir()` to get the output directory.
  2. Constructs the full path: `dir.join("wallpaper.tif")`.
  3. Creates an `image::RgbaImage::from_raw(width, height, pixels.to_vec())`.
  4. Saves via `.save(&path)` -- the `image` crate auto-detects TIFF format from the `.tiff` extension and uses LZW compression by default.
  5. Returns the saved path on success.

- **Test cases** (unit):
  - `save_wallpaper_creates_valid_tiff`: Create a 4x4 solid-color pixel buffer, call `save_wallpaper_image`, assert the file exists, is non-empty, and can be re-read by `image::open()`.
  - `save_wallpaper_overwrites_existing`: Call `save_wallpaper_image` twice with different pixel data, assert the file contains the second image's data (verify by checking file modification time or re-reading pixels).
- **Verify**: `cargo test wallpaper` passes. The saved TIFF file opens correctly in an image viewer.
- **Complexity**: Small

### Phase 5: UI Integration

This phase adds the "Set as Wallpaper" button to the Slint UI and wires it to the export pipeline.

#### Step 5.1: Add the "Set as Wallpaper" button to the Slint UI

- **Files**: `ui/main.slint`
- **Action**: Import `Button` from `std-widgets.slint` (add to the existing import line). Add a callback `callback set-wallpaper()` to the `MainWindow` component. Add an in-out property `in-out property <string> wallpaper-status;` for displaying status messages. Add a `Button` in the controls panel `VerticalLayout`, after the Diffuse Ramp slider row and before the renderer info text.

  ```slint
  Button {
      text: "Set as Wallpaper";
      clicked => { root.set-wallpaper(); }
  }

  Text {
      text: wallpaper-status;
      color: #888888;
      font-size: 11px;
      wrap: word-wrap;
  }
  ```

  The status text shows feedback like "Wallpaper set successfully" or "Error: Textures are still loading".

- **Test cases** (manual):
  - The button appears in the controls panel below the shading controls.
  - The button is clickable and visually responds to hover/press states.
  - The status text area appears below the button.
- **Verify**: `cargo build` succeeds. The button is visible in the UI.
- **Complexity**: Small

#### Step 5.2: Wire the button callback in `main.rs`

- **Files**: `src/main.rs`
- **Action**: Add a callback handler for `on_set_wallpaper`. The callback:

  1. Calls `wallpaper::get_primary_monitor_resolution()` to get the target size.
  2. Calls `renderer::export_wallpaper_image(width, height)` to render and read back pixels.
  3. Calls `wallpaper::save_wallpaper_image(&pixels, width, height)` to encode and save the TIFF.
  4. Calls `wallpaper::set_wallpaper(&path)` to apply the wallpaper.
  5. Updates `win.set_wallpaper_status(...)` with a success or error message.

  The entire sequence is synchronous on the UI thread. The render itself is a single draw call (fast), the TIFF encoding is fast (LZW, no deflate), and the file write is small (~5-10 MB). The `SystemParametersInfoW` call is near-instantaneous. Total expected latency: under 1 second.

  Gate the entire handler with `#[cfg(windows)]` since the `wallpaper` module only exists on Windows. On non-Windows platforms, the callback can show "Not supported on this platform" or the button can be hidden.

  ```rust
  #[cfg(windows)]
  {
      let window_weak = window.as_weak();
      window.on_set_wallpaper(move || {
          let Some(win) = window_weak.upgrade() else { return };
          match do_set_wallpaper() {
              Ok(()) => win.set_wallpaper_status("Wallpaper set successfully".into()),
              Err(e) => win.set_wallpaper_status(format!("Error: {e}").into()),
          }
      });
  }
  ```

  Extract the actual work into a helper function `fn do_set_wallpaper() -> Result<(), String>` for clean error handling with `?`.

- **Test cases** (manual):
  - Click "Set as Wallpaper" with textures loaded: wallpaper changes, status shows success.
  - Click "Set as Wallpaper" while textures are still loading: status shows "Textures are still loading".
  - Verify the wallpaper persists after a Windows sign-out/sign-in cycle.
- **Verify**: `cargo build` succeeds. End-to-end manual test passes.
- **Complexity**: Medium

### Phase 6: Integration Testing and Polish

#### Step 6.1: Run the full test suite and clippy

- **Files**: N/A
- **Action**: Run `cargo test` to verify all existing tests pass and new tests pass. Run `cargo clippy` to verify no new warnings. Pay special attention to:
  - Scoped `#[allow(unsafe_code)]` annotations in `wallpaper.rs` (ensure they are as narrow as possible -- on individual `unsafe` blocks or functions, not the whole module).
  - `cast_possible_truncation` warnings from monitor resolution math.
  - Unused imports when compiling on non-Windows targets.
- **Verify**: `cargo test` passes with zero failures. `cargo clippy` passes with zero warnings.
- **Complexity**: Small

#### Step 6.2: Manual end-to-end verification

- **Files**: N/A (manual verification)
- **Action**: Run the application and perform comprehensive manual testing.
- **Manual test cases**:
  - Application starts normally with no regressions.
  - "Set as Wallpaper" button is visible in the controls panel.
  - Click the button in Day/Night Blend mode with both textures loaded: wallpaper changes to a TIFF of the current scene at monitor resolution. Status shows success.
  - Open `%LOCALAPPDATA%\SunlitEarth\wallpaper.tif` in an image viewer -- the image matches the current scene, at the monitor's native resolution, with correct aspect ratio.
  - The wallpaper display style is "Fill" (check in Windows Settings > Personalization > Background).
  - Click the button in Grid mode: wallpaper changes to the grid texture.
  - Click the button in single-texture Day mode: wallpaper shows the day texture with no blending.
  - Click the button while textures are loading: status shows an appropriate error message.
  - Move the camera, change settings, click again: wallpaper updates to reflect the new scene.
  - Sign out of Windows and sign back in: wallpaper persists.
  - Existing controls (camera sliders, MSAA, texture switching, terminator, diffuse shading) still work correctly.
  - Window resize still works correctly.
  - `cargo run -- --software-rendering` still works.
- **Verify**: All manual test cases pass.
- **Complexity**: Medium

#### Step 6.3: Update `CLAUDE.md`

- **Files**: `CLAUDE.md`
- **Action**: Update the architecture documentation to reflect the new feature:
  - Add `wallpaper.rs` to the "Key modules" list: `wallpaper.rs` -- Windows-only wallpaper export (monitor resolution detection, TIFF save, `SystemParametersInfoW` via `windows-sys`), gated with `cfg(windows)`
  - Add `windows-sys` to the notable dependencies section
  - Mention the `tiff` feature on the `image` crate
  - Update the UI description to mention the "Set as Wallpaper" button
  - Add `read_texture_rgba8` to the renderer module description
  - Note the `COPY_SRC` usage flag difference between preview and export textures
- **Verify**: `CLAUDE.md` accurately reflects the updated architecture.
- **Complexity**: Small

## Test Strategy

### Automated Tests

| Test Case | Type | Input | Expected Output |
| --- | --- | --- | --- |
| `wallpaper_dir_uses_localappdata` | Unit | `%LOCALAPPDATA%` env var | Path ending in `SunlitEarth`, directory exists |
| `wallpaper_path_is_tiff` | Unit | N/A | Path ends with `.tiff` |
| `primary_resolution_is_nonzero` | Unit | System display | Width > 0, height > 0 |
| `primary_resolution_is_reasonable` | Unit | System display | Width >= 640, height >= 480 |
| `set_wallpaper_rejects_missing_file` | Unit | Non-existent path | `Err` result |
| `set_wallpaper_rejects_empty_file` | Unit | Zero-byte temp file | `Err` result |
| `save_wallpaper_creates_valid_tiff` | Unit | 4x4 RGBA pixel buffer | File exists, non-empty, re-readable |
| `save_wallpaper_overwrites_existing` | Unit | Two calls with different data | Second call's data is in the file |
| Existing renderer tests | Integration | Various | All pass unchanged |
| Existing shader tests | Integration | Various | All pass unchanged |

### Manual Verification

- [ ] "Set as Wallpaper" button appears in the controls panel
- [ ] Clicking the button in blend mode sets the wallpaper and shows "Wallpaper set successfully"
- [ ] The TIFF file at `%LOCALAPPDATA%\SunlitEarth\wallpaper.tif` is a valid image at monitor resolution
- [ ] The wallpaper display style is "Fill" (verified in Windows Settings)
- [ ] Clicking the button in each texture mode (Grid, Day, Night, Blend) produces the correct wallpaper
- [ ] Clicking while textures are loading shows an error message
- [ ] The wallpaper persists across sign-out/sign-in
- [ ] Camera and shading changes are reflected in subsequent wallpaper exports
- [ ] All existing controls and rendering modes work without regression
- [ ] `cargo build --release` succeeds
- [ ] `cargo clippy` passes
- [ ] `cargo test` passes

## Risks and Mitigations

| Risk | Impact | Mitigation |
| --- | --- | --- |
| DPI awareness: `GetMonitorInfoW` returns logical pixels without a DPI manifest, causing undersized renders on high-DPI displays | Wallpaper resolution is lower than the physical display | Test on a high-DPI display during manual verification. If logical pixels are returned, switch to `EnumDisplayDevices` which always returns physical resolution. The research document flags this as an open question requiring direct testing. |
| `SystemParametersInfoW` silently fails (returns TRUE but sets black wallpaper) for certain path encodings | User sees a black desktop | Verify the file exists and is non-empty before calling. Use only ASCII characters in the path (`%LOCALAPPDATA%\SunlitEarth\wallpaper.tif` is all-ASCII). Log the full path on error for debugging. |
| `windows-sys` version conflict or missing features | Build failure | Pin to a specific version (0.59). The crate is Microsoft-maintained and stable. |
| TIFF encoding produces a file that Windows cannot display as wallpaper | Black or missing wallpaper | TIFF is officially documented in the Windows Theme File Format spec. The `image` crate's TIFF encoder produces standard LZW-compressed TIFF. If TIFF fails, fallback to BMP (add `bmp` feature to `image` crate). |
| Export render at monitor resolution causes GPU out-of-memory on low-VRAM devices | Render failure or crash | The temporary textures are RGBA8 at monitor resolution (e.g., 2560x1440 = ~14 MB color + ~14 MB depth + staging buffer). With MSAA, multiply by `sample_count`. Total is well under 1 GB even at 4K with 8x MSAA. If OOM occurs, the error propagates through `wgpu`'s error handling. |
| Blocking the UI thread during export causes a noticeable freeze | Poor user experience | The export is fast: single draw call (~1 ms), TIFF encoding (~100 ms for LZW at 2560x1440), file write (~50 ms), API call (~1 ms). Total under 500 ms, which is acceptable for a button click. If this becomes a problem, the export can be moved to a background thread in a future iteration. |
| `unsafe` code in `wallpaper.rs` introduces UB | Memory safety violation | Each `unsafe` block has a scoped `#[allow(unsafe_code)]` and `// SAFETY:` comment. The FFI surface is small (4 functions: `EnumDisplayMonitors`, `GetMonitorInfoW`, `SystemParametersInfoW`, registry functions). All are well-documented Win32 APIs with straightforward calling conventions. |
| Compile failure on non-Windows targets due to missing `cfg` gates | CI/cross-platform breakage | The entire `wallpaper` module and the `windows-sys` dependency are gated with `cfg(windows)`. The button callback in `main.rs` is also gated. Run `cargo check` targeting a non-Windows triple if CI supports it. |

## Rollback Strategy

Revert the commits from this plan. The changes are self-contained:

- Remove `windows-sys` from `Cargo.toml` (the target-specific dependency section)
- Remove `features = ["tiff"]` from the `image` dependency
- Delete `src/wallpaper.rs`
- Remove `pub mod wallpaper;` from `src/lib.rs`
- Remove the `on_set_wallpaper` callback and `do_set_wallpaper` function from `src/main.rs`
- Remove `read_texture_rgba8` and `export_wallpaper_image` from the renderer
- Revert `ui/main.slint` (remove Button, callback, status property)
- Revert `CLAUDE.md`

No existing rendering behavior is modified. The preview pipeline is completely unchanged. The export creates only temporary GPU resources that are dropped after use.

## File Inventory

```text
Cargo.toml                     (modified: add windows-sys, add tiff feature to image)
src/lib.rs                     (modified: add cfg(windows) mod wallpaper)
src/main.rs                    (modified: add on_set_wallpaper callback)
src/wallpaper.rs               (new: Windows platform module — monitor resolution,
                                wallpaper style, set_wallpaper, save_wallpaper_image,
                                wallpaper_dir)
src/renderer/mod.rs            (modified: add pub export_wallpaper_image function)
src/renderer/render_pass.rs    (modified: add pub(crate) read_texture_rgba8 function)
ui/main.slint                  (modified: add Button import, set-wallpaper callback,
                                wallpaper-status property, Button element, status Text)
CLAUDE.md                      (modified: updated architecture docs)
```

## Status

- [ ] Plan approved
- [ ] Implementation started
- [ ] Implementation complete
