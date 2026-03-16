# Research: JPEG Banding in Wallpaper Export (2026-03-16)

## Problem Statement

When Sunlit Earth sets a rendered image as the Windows desktop wallpaper, the smooth diffuse shading gradient across the day/night terminator shows visible banding -- discrete color steps instead of a continuous transition. The source image (currently a lossless TIFF) looks correct; the banding appears after Windows internally re-encodes the image as JPEG at 85% quality for its `TranscodedWallpaper` cache. The `JPEGImportQuality` registry key set to 100 did not resolve the issue.

The goal of this research is to identify the root cause and recommend the most effective fix.

## Requirements

1. Understand the full data flow from shader computation to pixels on the desktop.
2. Identify where precision is lost and where banding is introduced.
3. Evaluate two candidate solutions: (a) adding dithering in the fragment shader to break up gradient steps, and (b) switching the export format to PNG so Windows preserves the image losslessly.
4. Recommend a single clear course of action.

## Findings

### The Banding Mechanism

The banding arises from the interaction of three factors in the current pipeline:

**1. Smooth, slowly-varying shader gradient.** The `blend_fragment()` function in `shaders/blend.wgsl` uses two nested `smoothstep` operations -- one for the terminator blend and one for diffuse shading -- to produce a continuously varying brightness across the sphere. With default parameters (`terminator_width=0.1`, `diffuse_ramp=0.25`), hundreds of adjacent pixels in the terminator region differ by fractions of a percent in brightness. The `smoothstep` Hermite polynomial has zero derivative at its endpoints, meaning the gradient is especially slowly varying near `n_dot_l = -w`, `n_dot_l = 0`, and `n_dot_l = +w`.

**2. 8-bit quantization in the render target.** The GPU computes the gradient in full f32 precision, but the render target is `Rgba8Unorm` (8 bits per channel, 256 discrete levels). In the slowly-varying gradient region, many adjacent pixels collapse to the same 8-bit value, creating faint steps even before the image leaves the GPU.

**3. JPEG compression amplifies the steps.** Windows transcodes the wallpaper to JPEG at 85% quality (default). JPEG's 8x8 DCT block processing quantizes the low-frequency coefficients that encode the smooth gradient, collapsing small per-pixel differences into uniform blocks. This makes the pre-existing 8-bit steps visually prominent by adding block-boundary artifacts aligned to an 8-pixel grid.

The first quantization (f32 to 8-bit) is inherent to the render target format. The second quantization (JPEG at 85%) is the dominant source of visible banding. At quality 85, the luma quantization table compresses values aggressively enough that a gradient spanning, say, 64 distinct 8-bit levels across 800 pixels is reduced to far fewer visually distinct levels.

### The Current Export Pipeline

The full path from button click to pixels on the desktop:

1. `export_wallpaper_image()` creates temporary `Rgba8Unorm` GPU textures at the monitor's native resolution with `COPY_SRC` usage.
2. The fragment shader computes per-pixel color in f32, writing to the `Rgba8Unorm` render target.
3. `read_texture_rgba8()` copies the texture to a staging buffer and reads back tightly-packed RGBA8 pixels (lossless).
4. `save_wallpaper_image()` encodes the pixels as LZW-compressed TIFF with an embedded sRGB ICC profile (lossless).
5. `set_wallpaper()` passes the TIFF path to `SystemParametersInfoW`.
6. Windows re-encodes the TIFF as JPEG to `%AppData%\Microsoft\Windows\Themes\TranscodedWallpaper` -- **banding introduced here**.

Steps 1-4 are lossless (modulo the inherent f32-to-8-bit render target quantization). The problem is entirely in step 6.

### Candidate Fix A: Shader Dithering

The codebase research identified dithering in the fragment shader as a potential fix. The idea is to add +/- 0.5/255 of noise (e.g., Interleaved Gradient Noise) to the final RGB color before the `Rgba8Unorm` quantization. This breaks up the smooth gradient into high-frequency noise that distributes quantization error more evenly.

**Strengths:**
- Acts before the first quantization point (f32 to 8-bit), which is the most principled location.
- Well-established technique in real-time rendering (Valve, Activision/Jorge Jimenez).
- Computationally trivial -- a single hash function on `@builtin(position)`.

**Fatal weakness for this use case: dithering does not survive JPEG compression.** The external research found strong evidence that JPEG's block-based DCT compression destroys dither patterns:

- JPEG assumes smooth color transitions and uses quantization tables that attenuate high-frequency detail. Dither noise is high-frequency by design, so it is the first thing JPEG discards.
- What survives JPEG re-encoding is a degraded, partially-destroyed version of the noise that may look worse than the original banding.
- File size increases dramatically (one analysis showed 5,316% increase) because the dither noise raises image entropy, which JPEG cannot efficiently encode.
- The academic and practitioner consensus is that dithering is intended for lossless or palette-quantized output (PNG, GIF), not for images that will be JPEG-compressed.

Adding very low-amplitude noise (~half an LSB) before JPEG encoding can marginally reduce visible contouring by randomizing which quantization bucket each pixel falls into. But this effect is small and does not resolve banding at quality 85.

**Verdict:** Shader dithering is an effective technique for lossless output paths but is counterproductive when the output will be JPEG-transcoded by Windows. It would improve the in-app preview display (which reads from the same `Rgba8Unorm` render target), but that is not where the banding problem is visible.

### Candidate Fix B: Switch Export Format to PNG

The external research found that **Windows preserves PNG wallpapers losslessly** -- it does not JPEG-transcode them.

**Evidence:**

- Multiple independent user tests and tutorial sources (ElevenForum, TenForums, HowToGeek, TechJunkie) consistently report that PNG source images produce a `TranscodedWallpaper` file of virtually identical byte size to the original, stored as PNG rather than transcoded to JPEG.
- This behavior is documented for Windows 10 and Windows 11 (current retail builds).
- The `JPEGImportQuality` registry key only governs JPEG source files and is irrelevant to PNG sources.
- DisplayFusion (Binary Fortress Software) encountered this exact problem and resolved it in version 5.1 Beta 8, with the developer confirming they investigated lossless PNG output as the solution.
- IrfanView takes a similar approach, converting to lossless BMP to bypass the JPEG transcode.

**Caveat:** One Windows 11 user report noted instances where PNG was being transcoded to JPEG, suggesting possible version-dependent behavior in Insider or preview builds. The general consensus for current retail Windows 11 is that PNG is safe from lossy transcoding.

**Practical considerations:**
- PNG file sizes for monitor-resolution images are manageable: 3-10 MB at 4K for natural images, likely less for a rendered sphere on a black background.
- The `image` crate (already a dependency via the `tiff` feature) supports PNG writing natively -- no new dependencies are needed.
- PNG supports full 8-bit-per-channel RGBA, matching the render target's `Rgba8Unorm` format exactly.
- The change is minimal: replace the TIFF encoder call with a PNG encoder call and change the file extension.

**Verdict:** Switching to PNG eliminates the JPEG transcode entirely, which is the root cause of the banding. This is a simpler, more robust solution than trying to mitigate the effects of lossy compression through dithering.

### Comparison of TIFF, PNG, and BMP

| Format | Lossless | Windows preserves losslessly? | File size (4K RGBA) | Dependency |
|--------|----------|-------------------------------|---------------------|------------|
| TIFF (LZW) | Yes | **Uncertain** -- not clearly documented | ~5-15 MB | `tiff` crate (current) |
| PNG | Yes | **Yes** -- well-documented on Win 10/11 | ~3-10 MB | `image` crate (already a dep) or `png` crate |
| BMP | Yes | **Yes** -- well-documented | ~24 MB (uncompressed) | `image` crate |

TIFF is the current format and its transcoding behavior is the least well-documented of the three. The banding problem confirms that Windows is JPEG-transcoding the TIFF. PNG is the clear winner: lossless, compact, well-documented Windows behavior, and no new dependencies.

### Relevant Code Locations

| File | Function | Relevance |
|------|----------|-----------|
| `shaders/blend.wgsl` (lines 1-29) | `blend_fragment()` | The smoothstep gradient math that produces the banding-susceptible output |
| `shaders/sphere.wgsl` (lines 45-67) | `fs_main()` | Fragment shader entry; where dithering would be added if pursued |
| `src/renderer/gpu_setup.rs` (line 318) | `create_render_textures()` | `Rgba8Unorm` format -- the first quantization point |
| `src/wallpaper.rs` (lines 257-287) | `save_wallpaper_image()` | TIFF encoding -- **the function to modify** for the format switch |
| `src/wallpaper.rs` (lines 185-241) | `set_wallpaper()` | `SystemParametersInfoW` call -- file path must change from `.tif` to `.png` |
| `src/renderer/mod.rs` (lines 80-159) | `export_wallpaper_image()` | Export entry point; passes pixel data to `save_wallpaper_image()` |

## Technical Constraints

1. **`unsafe_code = "deny"`**: The wallpaper module already has scoped `#[allow(unsafe_code)]` for Win32 FFI calls. The format change does not introduce new unsafe code.

2. **Dependency management**: The `image` crate is already in `Cargo.toml` with the `tiff` feature. Adding the `png` feature (or switching to it) is a minor `Cargo.toml` change. Alternatively, the `png` crate can be used directly for finer control, similar to how the current code uses the `tiff` crate directly.

3. **sRGB ICC profile**: The current TIFF encoder embeds an sRGB ICC profile from `assets/sRGB.icc`. PNG supports ICC profiles via the `iCCP` chunk. Whether to embed it depends on whether Windows' color management reads it from the wallpaper -- this is a minor consideration and omitting it is likely fine since the pixels are already in sRGB.

4. **File path**: The wallpaper is saved to `%LOCALAPPDATA%\SunlitEarth\wallpaper.tif`. This must change to `wallpaper.png`. The old `.tif` file should ideally be cleaned up on first run, but this is not critical.

## Open Questions

1. **Empirical verification needed.** The research strongly suggests PNG avoids JPEG transcoding on Windows 10/11, but this has not been tested in the Sunlit Earth pipeline specifically. The fix should be verified by: writing a PNG, setting it as wallpaper, then examining `%AppData%\Microsoft\Windows\Themes\TranscodedWallpaper` with a hex editor to confirm the magic bytes are `89 50 4E 47` (PNG) rather than `FF D8` (JPEG).

2. **Windows version edge cases.** At least one user report noted PNG-to-JPEG transcoding on a specific Windows 11 build. If this affects Sunlit Earth users, a fallback strategy would be needed -- either the TranscodedWallpaper pre-placement hack (copy the desired file to the cache path after `SystemParametersInfoW`) or adding shader dithering as a secondary defense for users stuck on problematic Windows builds.

3. **CachedFiles secondary pass.** Windows generates a derived `CachedFiles\CachedImage_*.jpg` file from the TranscodedWallpaper at 90% quality. It is unknown whether this secondary JPEG pass also applies to PNG-sourced wallpapers, or only to JPEG-sourced ones. If it applies, it could reintroduce banding through a different path.

4. **Shader dithering for preview quality.** Even if PNG solves the wallpaper export problem, the in-app preview also displays the `Rgba8Unorm` render target, which has the same 8-bit quantization. On most monitors with 8-bit panels, this is unlikely to produce visible banding in the preview (the preview is smaller and not JPEG-compressed), but if it does become noticeable, shader dithering would be the appropriate fix for that separate concern.

## Recommendation

**Switch the wallpaper export format from TIFF to PNG.** This is the primary and likely sufficient fix.

The reasoning:

1. **Root cause elimination.** The banding is caused by Windows JPEG-transcoding the TIFF wallpaper at 85% quality. PNG wallpapers are preserved losslessly by Windows on current retail Windows 10 and 11. Switching to PNG eliminates the root cause rather than mitigating its symptoms.

2. **Simplicity.** The change is confined to `save_wallpaper_image()` in `wallpaper.rs` (swap the encoder) and the file path constant (`.tif` to `.png`). No shader changes, no new dependencies, no changes to the render pipeline.

3. **Dithering is not needed.** If the output is lossless PNG and Windows preserves it losslessly, there is no quantization step beyond the inherent `Rgba8Unorm` render target. Adding dithering to fight a JPEG transcode that no longer occurs would be pointless. Dithering should be set aside unless a separate need for it emerges (e.g., visible banding in the preview display on 8-bit monitors).

4. **No registry manipulation required.** The `JPEGImportQuality` registry key is irrelevant for PNG sources. The fix works without requiring users to modify their system configuration.

5. **File size is acceptable.** A PNG at 2560x1440 or 3840x2160 for a rendered sphere (large black background, smooth gradients) will compress efficiently, likely 3-8 MB. This is comparable to or smaller than the current TIFF.

**After implementing the switch, verify empirically** that the `TranscodedWallpaper` file contains PNG data (magic bytes `89 50 4E 47`) rather than JPEG data (`FF D8`). If any Windows configuration is found to JPEG-transcode PNG, the fallback options in priority order are:

1. Pre-place the PNG at the `TranscodedWallpaper` cache path after calling `SystemParametersInfoW`.
2. Add shader dithering as a partial mitigation if lossless delivery is impossible on that system.

## Sources

| Document | Focus Area |
|----------|------------|
| `docs/plans/2026-03-16-jpeg-banding-codebase.md` | End-to-end export pipeline, shader gradient math, texture formats, absence of existing dithering, intervention points |
| `docs/plans/2026-03-16-jpeg-banding-external.md` | Windows transcoding behavior by format, PNG lossless preservation, JPEG banding mechanics, dithering vs. JPEG interaction, third-party approaches (DisplayFusion, IrfanView) |
