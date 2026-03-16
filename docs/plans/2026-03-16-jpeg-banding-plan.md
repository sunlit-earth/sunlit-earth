# Plan: JPEG Banding Fix (2026-03-16)

## Summary

Fix visible banding in the Windows desktop wallpaper export by switching the output format from TIFF to PNG. Windows JPEG-transcodes TIFF wallpapers at 85% quality, destroying smooth gradients in the terminator region. Multiple independent sources confirm that Windows preserves PNG wallpapers losslessly (no JPEG transcode). The plan is structured in two phases: Phase 1 switches to PNG and includes a manual verification checkpoint to confirm lossless preservation. Phase 2 (conditional, only if Phase 1 fails) adds Interleaved Gradient Noise dithering in the fragment shader and outputs JPEG at maximum quality with 4:4:4 chroma subsampling as a fallback.

## Stakes Classification

**Level**: Low
**Rationale**: The change is confined to a single function (`save_wallpaper_image` in `wallpaper.rs`), the file path constant, dependency features in `Cargo.toml`, and corresponding documentation/test updates. No shader changes, no render pipeline changes, no new unsafe code, no architectural modifications. The `image` crate already supports PNG via its `png` feature. The old TIFF file is simply replaced by a PNG file at the same location. Rollback is trivial: revert the format change. Phase 2 (if needed) is higher stakes due to shader modifications, but it is conditional and isolated behind a verification gate.

## Context

**Research**: [`docs/plans/2026-03-16-jpeg-banding-research.md`](2026-03-16-jpeg-banding-research.md)
**Affected Areas**: `Cargo.toml`, `src/wallpaper.rs`, `src/main.rs`, `CLAUDE.md`

## Success Criteria

### Phase 1 (PNG switch)

- [ ] `Cargo.toml` enables the `png` feature on the `image` crate and removes the `tiff` crate as a direct dependency
- [ ] `save_wallpaper_image()` encodes pixels as PNG instead of TIFF
- [ ] The wallpaper file path changes from `wallpaper.tif` to `wallpaper.png`
- [ ] All existing unit tests in `wallpaper.rs` are updated and pass
- [ ] `cargo build` succeeds with no warnings
- [ ] `cargo clippy` passes
- [ ] `cargo test` passes (all existing tests unbroken)
- [ ] **Manual verification**: After setting the wallpaper, `%AppData%\Microsoft\Windows\Themes\TranscodedWallpaper` begins with PNG magic bytes (`89 50 4E 47`), confirming Windows preserved the PNG losslessly

### Phase 2 (conditional fallback -- only if Phase 1 verification fails)

- [ ] The fragment shader applies Interleaved Gradient Noise dithering before `Rgba8Unorm` quantization
- [ ] `Cargo.toml` enables the `jpeg` feature on the `image` crate
- [ ] `save_wallpaper_image()` encodes pixels as JPEG at quality 100 with 4:4:4 chroma subsampling
- [ ] Visible banding is eliminated or substantially reduced in the desktop wallpaper

## Implementation Steps

### Phase 1: Switch Wallpaper Export from TIFF to PNG

This phase replaces the TIFF encoder with PNG, updates the file extension, adjusts dependencies, and updates tests and documentation. It concludes with a manual verification checkpoint.

#### Step 1.1: Update `Cargo.toml` dependencies

- **Files**: `Cargo.toml` (line 15-16)
- **Action**: Change the `image` crate features from `["tiff"]` to `["png"]`. Remove the standalone `tiff = "0.10"` dependency since it was only used by `save_wallpaper_image()`.

  Current:

  ```toml
  image = { version = "0.25.8", default-features = false, features = ["tiff"] }
  tiff = "0.10"
  ```

  New:

  ```toml
  image = { version = "0.25.8", default-features = false, features = ["png"] }
  ```

- **Verify**: `cargo check` succeeds. `cargo tree -i tiff` shows the `tiff` crate is no longer in the dependency tree (or only present as a transitive dependency if something else pulls it in).
- **Complexity**: Small

#### Step 1.2: Rewrite `save_wallpaper_image()` to produce PNG

- **Files**: `src/wallpaper.rs` (lines 243-287)
- **Action**: Replace the entire `save_wallpaper_image()` function body and its doc comment. The new implementation:

  1. Calls `wallpaper_dir()` to get the output directory (unchanged).
  2. Constructs the path as `dir.join("wallpaper.png")` (was `.tif`).
  3. Creates an `image::RgbaImage::from_raw(width, height, pixels.to_vec())` and unwraps it (the caller guarantees the buffer is correctly sized).
  4. Uses `image::codecs::png::PngEncoder::new_with_quality()` with `CompressionType::Fast` and `FilterType::Sub` (or `Adaptive`) for fast encoding. Larger file size is acceptable — encoding speed matters because the user clicks "Set as Wallpaper" and waits. The high-level `.save()` API uses default (slow) compression, so we must use the encoder directly.
  5. Returns the path on success.

  Remove the `SRGB_ICC_PROFILE` constant and the `include_bytes!("../assets/sRGB.icc")` since the `image` crate's PNG encoder does not support embedding ICC profiles via its high-level API, and the research document notes that omitting the sRGB profile is acceptable because the pixels are already in sRGB. The `assets/sRGB.icc` file itself should be kept in the repository (it is not harmful and may be useful later), but the `include_bytes!` and the constant are removed from `wallpaper.rs`.

  Remove the `use tiff::encoder::colortype` and `use tiff::tags::Tag` imports.

  Update the module-level doc comment (line 1) from "TIFF save" to "PNG save".

- **Test cases**: Updated in Step 1.3.
- **Verify**: `cargo build` succeeds. The function compiles with the new `image` PNG feature.
- **Complexity**: Small

#### Step 1.3: Update unit tests in `wallpaper.rs`

- **Files**: `src/wallpaper.rs` (lines 289-387, the `#[cfg(test)]` block)
- **Action**: Update all tests to reflect the PNG format change:

  - `wallpaper_path_is_tif` -- rename to `wallpaper_path_is_png`. Change the path construction from `"wallpaper.tif"` to `"wallpaper.png"`. Change the assertion from `.ends_with(".tif")` to `.ends_with(".png")`. Update the assertion message.
  - `save_wallpaper_creates_valid_tiff_and_overwrites` -- rename to `save_wallpaper_creates_valid_png_and_overwrites`. Update all "TIFF" strings in assertion messages to "PNG". The `image::open()` call and pixel verification logic remain unchanged (the `image` crate reads PNG natively).
  - `set_wallpaper_rejects_missing_file` -- the path string `r"C:\nonexistent\fake_wallpaper.tif"` can remain as-is (the function rejects missing files regardless of extension) but change to `.png` for consistency.
  - `set_wallpaper_rejects_empty_file` -- change `"empty.tif"` to `"empty.png"` for consistency.

- **Test cases** (all existing, updated for PNG):
  - `wallpaper_path_is_png`: Assert `wallpaper_dir().join("wallpaper.png")` ends with `.png`.
  - `save_wallpaper_creates_valid_png_and_overwrites`: Create a 4x4 red pixel buffer, save, verify file exists and is non-empty, re-read with `image::open()`, verify dimensions. Overwrite with 2x2 blue, verify new dimensions and pixel values.
  - `set_wallpaper_rejects_missing_file`: Unchanged behavior, updated path extension.
  - `set_wallpaper_rejects_empty_file`: Unchanged behavior, updated file name.
  - `wallpaper_dir_uses_localappdata`: Unchanged (no format dependency).
  - `primary_resolution_is_nonzero`: Unchanged.
  - `primary_resolution_is_reasonable`: Unchanged.

- **Verify**: `cargo test wallpaper` passes. All 7 tests pass.
- **Complexity**: Small

#### Step 1.4: Update doc comment in `main.rs`

- **Files**: `src/main.rs` (line 155)
- **Action**: Change the doc comment on `do_set_wallpaper()` from "save as TIFF" to "save as PNG".

  Current:

  ```rust
  /// Render the current scene at the primary monitor's resolution, save as TIFF,
  /// and set it as the Windows desktop wallpaper.
  ```

  New:

  ```rust
  /// Render the current scene at the primary monitor's resolution, save as PNG,
  /// and set it as the Windows desktop wallpaper.
  ```

- **Verify**: `cargo build` succeeds. No functional change.
- **Complexity**: Small

#### Step 1.5: Update `CLAUDE.md`

- **Files**: `CLAUDE.md`
- **Action**: Update all references to the TIFF format in the wallpaper export pipeline description:

  - In the `wallpaper.rs` module description (line 59): change "TIFF save via `image` crate" to "PNG save via `image` crate".
  - In the "Wallpaper export pipeline" paragraph (line 68): change "encoded as LZW-compressed TIFF" to "encoded as PNG", change `wallpaper.tif` to `wallpaper.png`.
  - In the "Notable dependencies" section (line 72): change `image` description from "TIFF encoding for wallpaper export (via `tiff` feature)" to "PNG encoding for wallpaper export (via `png` feature)".
  - Remove any mention of the `tiff` crate as a standalone dependency if present.

- **Verify**: Read through the updated `CLAUDE.md` and confirm no stale TIFF references remain in the wallpaper-related sections.
- **Complexity**: Small

#### Step 1.6: Run full test suite and clippy

- **Files**: N/A
- **Action**: Run `cargo test` to verify all tests pass. Run `cargo clippy` to verify no warnings. Pay attention to unused import warnings from the removed `tiff` crate.
- **Verify**: `cargo test` passes with zero failures. `cargo clippy` passes with zero warnings. `cargo build` succeeds.
- **Complexity**: Small

#### Step 1.7: Manual verification checkpoint (GATE)

- **Files**: N/A (manual verification)
- **Action**: This is the critical verification step that determines whether Phase 2 is needed. Run the application, click "Set as Wallpaper", then inspect the `TranscodedWallpaper` file.

- **Manual test cases**:
  1. Run the app with `cargo run`, load a texture (Day/Night Blend mode preferred for maximum gradient visibility), and click "Set as Wallpaper". Status should show "Wallpaper set successfully".
  2. Open the wallpaper file at `%LOCALAPPDATA%\SunlitEarth\wallpaper.png` in an image viewer. Confirm it is a valid PNG at the monitor's native resolution.
  3. Inspect `%AppData%\Microsoft\Windows\Themes\TranscodedWallpaper` with a hex editor (e.g., `xxd` via Git Bash, or HxD). Check the first 4 bytes:
     - `89 50 4E 47` = PNG magic bytes -- **Phase 1 succeeds. Phase 2 is not needed.**
     - `FF D8` = JPEG magic bytes -- **Phase 1 fails. Proceed to Phase 2.**
  4. Visually inspect the desktop wallpaper in the terminator region. Confirm the smooth gradient has no visible banding steps.
  5. Optionally inspect `%AppData%\Microsoft\Windows\Themes\CachedFiles\` for any derived JPEG files that might reintroduce banding through a secondary path.

- **Verify**: The `TranscodedWallpaper` file is PNG (not JPEG). The desktop wallpaper shows smooth gradients without banding.
- **Complexity**: Small

### Phase 2: Shader Dithering + JPEG Fallback (Conditional)

**This phase is only executed if Phase 1's manual verification (Step 1.7) reveals that Windows still JPEG-transcodes the PNG wallpaper.** If Step 1.7 confirms PNG is preserved losslessly, skip this entire phase.

The strategy here is different from Phase 1: instead of avoiding the JPEG transcode, we make the image resilient to it. Interleaved Gradient Noise (IGN) dithering in the fragment shader distributes quantization error as high-frequency noise that partially survives JPEG compression at high quality. The output format switches from PNG to JPEG at quality 100 with 4:4:4 chroma subsampling (no chroma downsampling), giving us direct control over JPEG quality rather than relying on Windows' 85% default.

#### Step 2.1: Add IGN dithering to the fragment shader

- **Files**: `shaders/sphere.wgsl` (lines 44-67, `fs_main`)
- **Action**: Add an Interleaved Gradient Noise (IGN) dithering step at the end of `fs_main()`, after the final color is computed but before the `return`. The dither adds +/- 0.5/255 of noise to each RGB channel, distributing the `Rgba8Unorm` quantization error.

  Add a helper function before `fs_main`:

  ```wgsl
  /// Interleaved Gradient Noise (Jimenez, 2014).
  /// Returns a value in [0, 1) based on fragment position.
  fn ign(frag_coord: vec2<f32>) -> f32 {
      return fract(52.9829189 * fract(0.06711056 * frag_coord.x
                                     + 0.00583715 * frag_coord.y));
  }
  ```

  At the end of `fs_main`, before the final `return vec4<f32>(color, 1.0)`:

  ```wgsl
  // Dither: add +/- 0.5 LSB of noise to break up 8-bit quantization steps
  let noise = (ign(in.clip_position.xy) - 0.5) / 255.0;
  let dithered = color + vec3<f32>(noise, noise, noise);
  return vec4<f32>(dithered, 1.0);
  ```

  This applies to both single-texture mode and blend mode (the dither is added after both code paths produce a `color` value).

- **Test cases** (GPU integration tests -- behavioral, not pixel-exact):
  - `dither_does_not_change_mean_brightness`: Render a uniform-color sphere at a fixed camera angle. Compare the mean pixel brightness with and without dithering. The means should be equal within 1/255 (dithering redistributes error but does not shift the mean).
  - `dither_increases_gradient_entropy`: Render the terminator gradient. Compute per-row pixel entropy (Shannon entropy of unique values). With dithering, entropy should be higher than without (more unique values in the gradient region). This is a behavioral invariant that does not depend on exact pixel values.
  - Note: These tests require a way to toggle dithering. Since the dither is unconditional in the shader, these tests may need to compare against a known baseline or use a uniform flag. For simplicity, the dithering can be made unconditional (always on) since it is visually imperceptible in lossless output and beneficial for JPEG output. If a toggle is needed later, add a flag bit to the `flags` uniform.

- **Verify**: `cargo build` succeeds. The shader compiles (validated at pipeline creation time). Visual inspection shows no visible noise in the preview (the +/- 0.5 LSB is below perceptual threshold on 8-bit displays).
- **Complexity**: Medium

#### Step 2.2: Update `Cargo.toml` for JPEG support

- **Files**: `Cargo.toml`
- **Action**: Change the `image` crate features from `["png"]` to `["jpeg"]`. (If both PNG and JPEG are desired, use `["png", "jpeg"]`.)

  Note: The `image` crate's high-level `save()` API does not support setting JPEG quality or chroma subsampling. For quality 100 with 4:4:4, we need to use the `jpeg-encoder` crate directly (which is what `image` uses internally) or use `image::codecs::jpeg::JpegEncoder::new_with_quality()`. The `image` crate's `JpegEncoder` supports quality but does not expose chroma subsampling control. For 4:4:4, consider adding `jpeg-encoder` as a direct dependency.

  If the `image` crate's `JpegEncoder` at quality 100 produces acceptable results (quality 100 typically implies 4:4:4 in most JPEG encoders), use it directly. Otherwise, add `jpeg-encoder` as a dependency.

- **Verify**: `cargo check` succeeds.
- **Complexity**: Small

#### Step 2.3: Rewrite `save_wallpaper_image()` for JPEG output

- **Files**: `src/wallpaper.rs` (the `save_wallpaper_image()` function)
- **Action**: Replace the PNG encoder with a JPEG encoder at quality 100. The implementation:

  1. Construct the path as `dir.join("wallpaper.jpg")`.
  2. Create an `image::RgbaImage::from_raw(width, height, pixels.to_vec())`.
  3. Convert to RGB (JPEG does not support alpha): `DynamicImage::ImageRgba8(img).to_rgb8()`.
  4. Use `image::codecs::jpeg::JpegEncoder::new_with_quality(&mut writer, 100)` to create a quality-100 encoder.
  5. Encode the RGB image.
  6. Return the path.

  Update the doc comment to describe JPEG encoding.

- **Test cases**:
  - `save_wallpaper_creates_valid_jpeg_and_overwrites`: Same structure as the PNG test but verify JPEG output. Re-read with `image::open()`, verify dimensions. Pixel values may differ slightly due to JPEG lossy compression even at quality 100, so skip exact pixel assertions.

- **Verify**: `cargo test wallpaper` passes. The saved file is a valid JPEG.
- **Complexity**: Small

#### Step 2.4: Update tests, docs, and manual verification

- **Files**: `src/wallpaper.rs` (tests), `src/main.rs` (doc comment), `CLAUDE.md`
- **Action**: Update all references from PNG to JPEG (path extensions, doc comments, assertion messages). Update `CLAUDE.md` to document the JPEG format choice and the shader dithering.

- **Manual test cases**:
  1. Run the app, click "Set as Wallpaper".
  2. Open `%LOCALAPPDATA%\SunlitEarth\wallpaper.jpg` in an image viewer. Confirm it is a valid JPEG at the monitor's native resolution.
  3. Visually inspect the desktop wallpaper in the terminator region. Compare against the pre-dithering version. Banding should be substantially reduced or eliminated.
  4. If banding persists, investigate the `TranscodedWallpaper` file -- Windows may still be re-encoding the JPEG at a lower quality. In that case, consider the TranscodedWallpaper pre-placement strategy documented in the research.

- **Verify**: `cargo test` passes. `cargo clippy` passes. Visual inspection confirms reduced banding.
- **Complexity**: Small

## Test Strategy

### Automated Tests (Phase 1)

| Test Case | Type | Input | Expected Output |
| --- | --- | --- | --- |
| `wallpaper_dir_uses_localappdata` | Unit | `%LOCALAPPDATA%` env var | Path ending in `SunlitEarth`, directory exists |
| `wallpaper_path_is_png` | Unit | N/A | Path ends with `.png` |
| `primary_resolution_is_nonzero` | Unit | System display | Width > 0, height > 0 |
| `primary_resolution_is_reasonable` | Unit | System display | Width >= 640, height >= 480 |
| `set_wallpaper_rejects_missing_file` | Unit | Non-existent path | `Err` result |
| `set_wallpaper_rejects_empty_file` | Unit | Zero-byte temp file | `Err` result |
| `save_wallpaper_creates_valid_png_and_overwrites` | Unit | 4x4 RGBA pixel buffers | File exists, non-empty, re-readable, correct dimensions and pixels |
| Existing renderer/shader tests | Integration | Various | All pass unchanged |

### Automated Tests (Phase 2, conditional)

| Test Case | Type | Input | Expected Output |
| --- | --- | --- | --- |
| `dither_does_not_change_mean_brightness` | GPU integration | Uniform-color sphere | Mean brightness within 1/255 of non-dithered |
| `dither_increases_gradient_entropy` | GPU integration | Terminator gradient | Higher per-row entropy than non-dithered |
| `save_wallpaper_creates_valid_jpeg_and_overwrites` | Unit | 4x4 RGBA pixel buffers | File exists, non-empty, re-readable, correct dimensions |

### Manual Verification

- [ ] `cargo build` succeeds with no warnings
- [ ] `cargo clippy` passes with no warnings
- [ ] `cargo test` passes with no failures
- [ ] Application starts normally with no regressions
- [ ] "Set as Wallpaper" produces `wallpaper.png` at `%LOCALAPPDATA%\SunlitEarth\`
- [ ] The PNG file opens correctly in an image viewer at the monitor's native resolution
- [ ] `%AppData%\Microsoft\Windows\Themes\TranscodedWallpaper` has PNG magic bytes (`89 50 4E 47`), confirming no JPEG transcode
- [ ] The desktop wallpaper shows smooth gradients without visible banding in the terminator region
- [ ] Existing controls (camera, MSAA, texture switching, terminator, diffuse shading) work without regression
- [ ] The wallpaper persists across Windows sign-out/sign-in

## Risks and Mitigations

| Risk | Impact | Mitigation |
| --- | --- | --- |
| Windows JPEG-transcodes PNG on some builds (one user report for Windows 11 Insider) | Banding persists | Step 1.7 verification checkpoint catches this. Phase 2 provides a fallback. Alternatively, pre-place the PNG at the `TranscodedWallpaper` cache path after `SystemParametersInfoW`. |
| Removing the `tiff` crate breaks something else that depends on it | Build failure | The `tiff` crate is only used in `save_wallpaper_image()`. Search for other `tiff` imports before removing. The `image` crate's `tiff` feature is only used for TIFF encoding/decoding, and JXL decoding uses `jxl-oxide`, not `image`. |
| PNG file size is significantly larger than TIFF at 4K | Disk usage, slower wallpaper application | Using `CompressionType::Fast` for encoding speed (user waits for "Set as Wallpaper"). Larger file is acceptable. Research indicates PNG for a rendered sphere will be 3-10 MB at 4K. |
| `image` crate's PNG encoder does not embed sRGB ICC profile | Subtle color shift on wide-gamut displays | The research notes this is acceptable because the pixels are already in sRGB and Windows assumes sRGB for wallpapers. If color accuracy issues emerge, use the `png` crate directly (which supports `iCCP` chunks). |
| Phase 2 shader dithering affects the in-app preview | Visible noise in preview | IGN dithering at +/- 0.5 LSB is below the perceptual threshold on 8-bit displays. The preview renders to the same `Rgba8Unorm` target, so the dither actually improves preview gradient quality. No mitigation needed. |
| `CachedFiles` secondary JPEG pass reintroduces banding | Banding visible despite PNG preservation | Step 1.7 includes a check for `CachedFiles` directory. If this is a problem, it requires further investigation (may be unavoidable for the lock screen cache, but not the desktop wallpaper). |

## Rollback Strategy

**Phase 1 rollback**: Revert `save_wallpaper_image()` to the TIFF implementation, restore the `tiff` crate dependency, change the `image` feature back to `tiff`, revert test names and path extensions. The change is confined to `Cargo.toml`, `src/wallpaper.rs`, `src/main.rs` (doc comment), and `CLAUDE.md`. No render pipeline code is modified.

**Phase 2 rollback** (if executed): Remove the `ign()` function and dithering lines from `shaders/sphere.wgsl`, revert `save_wallpaper_image()` to PNG (or TIFF), remove the `jpeg` feature from `Cargo.toml`. The shader change is additive (two new lines at the end of `fs_main`), so reverting is straightforward.

## File Inventory

### Phase 1

```text
Cargo.toml                     (modified: image features tiff->png, remove tiff crate)
src/wallpaper.rs               (modified: save_wallpaper_image rewritten for PNG,
                                remove SRGB_ICC_PROFILE constant, update tests)
src/main.rs                    (modified: doc comment "TIFF" -> "PNG")
CLAUDE.md                      (modified: TIFF references -> PNG)
```

### Phase 2 (conditional)

```text
Cargo.toml                     (modified: add jpeg feature to image crate)
shaders/sphere.wgsl            (modified: add ign() function and dithering in fs_main)
src/wallpaper.rs               (modified: save_wallpaper_image rewritten for JPEG)
src/main.rs                    (modified: doc comment "PNG" -> "JPEG")
CLAUDE.md                      (modified: PNG references -> JPEG, document dithering)
```

## Status

- [ ] Plan approved
- [ ] Phase 1 implementation started
- [ ] Phase 1 implementation complete
- [ ] Phase 1 manual verification passed (Phase 2 not needed)
- [ ] Phase 2 implementation started (only if Phase 1 verification failed)
- [ ] Phase 2 implementation complete
