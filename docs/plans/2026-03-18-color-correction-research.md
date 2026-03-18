# Research: Color Correction (2026-03-18)

## Problem Statement

Sunlit Earth needs interactive per-texture gamma and saturation controls so users can adjust the visual appearance of day and night textures independently. The feature requires new UI sliders, new uniform parameters threaded through the full Slint-to-WGSL pipeline, and two new WGSL helper functions applied at the correct point in the fragment shader. This research consolidates findings from codebase analysis and external technical research into a single reference for implementation.

## Requirements

- Four new user-facing parameters: day gamma, day saturation, night gamma, night saturation.
- Identity defaults (gamma = 1.0, saturation = 1.0) so existing users see no visual change.
- Independent correction per texture, applied before blending and shading.
- No changes to sRGB handling or texture format -- the feature works within the existing pipeline.

---

## Findings

### 1. Current Rendering Pipeline: sRGB-in-sRGB-out

The entire pipeline operates in sRGB space with no linearization step. This is the foundation for understanding where color correction fits.

**Texture decoding**: JXL textures are decoded via the `image` crate's jxl-oxide integration. The `.into_rgba8()` call produces sRGB-encoded u8 pixels -- values of 128/255 represent approximately 22% linear light, not 50%. This is the standard `image` crate convention for u8 output, matching libjxl's documentation: "Pixels are assumed to be nonlinear sRGB for integer data types."

**GPU texture format**: All textures (day, night, grid, dummy) use `wgpu::TextureFormat::Rgba8Unorm`, which maps bytes linearly to `[0, 1]` with no gamma decode. The shader receives sRGB-encoded values directly.

**Render target format**: Also `Rgba8Unorm`. The shader's output is stored as-is, with no sRGB encode step.

**Display path**: The render target is converted to a Slint `Image` and displayed. The monitor applies its ~2.2 gamma curve. Since the pixel data is already sRGB-encoded, this produces the correct visual result.

The pipeline is internally consistent: sRGB data flows through unchanged, and all shader math (diffuse shading, Fresnel, blending, specular) operates on sRGB values. This is not physically correct (lighting should happen in linear space), but it produces acceptable results and is a deliberate design choice. The only known artifact is that CPU-computed mipmaps in `downsample_2x` average sRGB values directly, producing slightly-too-dark lower mip levels -- a pre-existing issue unrelated to color correction.

**Conclusion**: No sRGB conversion changes are needed for the color correction feature. The gamma and saturation controls will operate on sRGB-encoded values, which is standard for artistic color controls (Photoshop, Lightroom, Premiere all work this way).

### 2. Shader Architecture and Insertion Point

The shader is split across two WGSL files concatenated at load time by `gpu_setup.rs`:

- `shaders/blend.wgsl` -- contains `blend_fragment()`, a pure function for day/night blending with diffuse shading
- `shaders/sphere.wgsl` -- contains vertex transform, texture sampling, uniforms struct, and `fs_main` which calls `blend_fragment()`

The current color flow in `fs_main` (`sphere.wgsl:61-114`):

1. Sample day texture (`textureSample` on line 62)
2. Single-texture early return: if `terminator_width < 0`, return `day.rgb` directly (lines 66-68)
3. Sample night texture (line 70)
4. Compute lighting dot product (line 72)
5. Call `blend_fragment` for day/night blending with diffuse shading (lines 74-80)
6. Water effects: Fresnel reflectance, specular glint, diffuse color shift (lines 86-111)
7. Final output (line 113)

**Correct insertion point**: After texture sampling, before `blend_fragment`. This ensures:

- Day and night textures each receive independent correction before blending.
- Diffuse shading in `blend_fragment` operates on the corrected colors.
- Water Fresnel and specular effects operate on the corrected colors.
- The single-texture early return path (lines 66-68) also applies correction -- it should use day correction since the day texture slot is what's bound as `sphere_texture` in all single-texture modes.

Applying correction after blending would be wrong: the darkened night-side would get gamma-lifted, and a single set of parameters could not independently adjust day vs. night appearance.

### 3. The Algorithms

#### Gamma correction: `pow(max(color, 0.0), 1.0 / gamma)`

The classic gamma formula raises each channel to the power `1/gamma`. With `gamma = 1.0` this is the identity. Values above 1.0 brighten midtones (lift the curve); values below 1.0 darken midtones.

```wgsl
fn apply_gamma(color: vec3<f32>, gamma: f32) -> vec3<f32> {
    return pow(max(color, vec3<f32>(0.0)), vec3<f32>(1.0 / gamma));
}
```

The `max(color, 0.0)` guard is essential: `pow` of a negative value is undefined in WGSL and produces NaN. Negative channel values can occur after diffuse shading or blending arithmetic. Without this guard, NaN propagates through the rest of the shader and corrupts the output.

Operating on sRGB-encoded values (rather than linearizing first) matches the user expectation for a "gamma" slider. This is what Photoshop's Levels gamma midpoint does.

#### Saturation adjustment: luminance-weighted mix

```wgsl
fn adjust_saturation(color: vec3<f32>, saturation: f32) -> vec3<f32> {
    let luminance = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
    return mix(vec3<f32>(luminance), color, saturation);
}
```

At `saturation = 1.0` this is the identity. At 0.0 the output is greyscale. Above 1.0 the color difference from grey is amplified (oversaturation).

The weights `(0.2126, 0.7152, 0.0722)` are Rec. 709 relative luminance coefficients. Strictly, these are defined for linear RGB, not sRGB-encoded values. Applying them directly to sRGB values introduces a small perceptual error. However, this is standard practice in all major image editors and real-time renderers -- the error is not perceptually significant for the +-50% adjustment range of a user-facing saturation slider. Alternative sRGB-aware weights exist (Poynton: `0.309, 0.609, 0.082`; Asahi Lina: `0.299, 0.518, 0.183`) but are not necessary for this application.

#### Application order

Gamma first, then saturation. This matches the Lightroom/Photoshop adjustment order: correct the tonal curve, then adjust color intensity. The two operations are order-independent in the sense that either order produces acceptable results, but gamma-first-then-saturation is the conventional sequence.

#### Performance

Gamma costs approximately 1-3 ALU cycles (one `pow` per channel). Saturation costs approximately 2 ALU operations (one `dot`, one `mix`). Total: ~5-8 ALU operations per texture per fragment, against a background of tens to hundreds of existing operations in the shader. For a wallpaper renderer that draws one frame per 2-minute timer tick, this is negligible.

### 4. Full Data Flow for New Parameters

Traced end-to-end using `fresnel_mix` as the established pattern, each new parameter (`day_gamma`, `day_saturation`, `night_gamma`, `night_saturation`) must be threaded through these files:

| Step                  | File                          | What to add                                                                                 |
|-----------------------|-------------------------------|---------------------------------------------------------------------------------------------|
| UI property           | `ui/main.slint`               | `in-out property <float>` with default value at identity (1.0)                              |
| UI slider             | `ui/main.slint`               | Slider with `value <=> root.property`, `changed => { root.sliders-changed(); }`, display    |
| Config field          | `src/config.rs`               | Field in `AppConfig`, default in `AppConfig::default()`                                     |
| Config apply          | `src/main.rs`                 | `apply_config_to_window`: `window.set_*()`                                                  |
| Config read           | `src/main.rs`                 | `read_config_from_window`: `window.get_*()`                                                 |
| Reset All             | `src/main.rs`                 | `on_reset_all`: `win.set_*(lighting.*)`                                                     |
| Read from UI          | `src/renderer/mod.rs`         | `win.get_*()` in `BeforeRendering` callback                                                 |
| Dirty check           | `src/renderer/frame.rs`       | Field in `FrameState`, param in `build_frame_state`, quantize to integer thousandths        |
| Shading params        | `src/renderer/render_pass.rs` | Field in `ShadingParams`                                                                    |
| Uniform struct (Rust) | `src/renderer/uniforms.rs`    | Field in `Uniforms`, update size assertion                                                  |
| Uniform write         | `src/renderer/render_pass.rs` | Populate field in `write_uniforms`                                                          |
| Uniform struct (WGSL) | `shaders/sphere.wgsl`         | Field in WGSL `Uniforms` struct                                                             |
| Shader use            | `shaders/sphere.wgsl`         | Call `apply_gamma` / `adjust_saturation` in `fs_main`                                       |
| Helper functions      | `shaders/blend.wgsl`          | New `apply_gamma` and `adjust_saturation` functions                                         |

**Slint defaults and Rust `AppConfig` defaults must match.** The Slint defaults are used on first render before `apply_config_to_window` runs; the Rust defaults are used when config is missing. Both must be 1.0 for all four parameters.

**Wallpaper export** requires no special handling. The export path uses `last_shading` from the previous frame, which automatically includes the new parameters once `ShadingParams` is extended.

### 5. Uniform Buffer Layout

The current `Uniforms` struct is 128 bytes (`src/renderer/uniforms.rs`). Adding four `f32` fields (16 bytes) brings the total to 144 bytes, which is already a multiple of 16 (the std140 alignment requirement). No extra padding fields are needed.

New fields appended after `fresnel_exp` at offset 124:

```text
day_gamma: f32,           // offset 128
day_saturation: f32,      // offset 132
night_gamma: f32,         // offset 136
night_saturation: f32,    // offset 140
```

The compile-time size assertion must be updated from `== 128` to `== 144`. The uniform buffer size is computed from `std::mem::size_of::<Uniforms>()`, so it adjusts automatically.

The WGSL `Uniforms` struct must add matching fields at the same offsets. Since all four are scalar `f32` with no `vec3` alignment concerns, no padding is needed in WGSL either.

### 6. UI Organization

All lighting sliders follow the same pattern: `in-out property` declaration with a default, a `Slider` with bidirectional `<=>` binding, a `changed` callback that fires `root.sliders-changed()` (which triggers `window.request_redraw()` in Rust), and a display `Text` showing the rounded value.

The four new sliders should be added as a new "Color Correction" `GroupBox` section, or added to the existing Lighting group. Proposed ranges and defaults:

| Parameter        | Default | Min | Max |
|------------------|---------|-----|-----|
| Day Gamma        | 1.0     | 0.2 | 3.0 |
| Day Saturation   | 1.0     | 0.0 | 2.0 |
| Night Gamma      | 1.0     | 0.2 | 3.0 |
| Night Saturation | 1.0     | 0.0 | 2.0 |

All defaults at identity means the feature is opt-in with zero visual change for existing users.

---

## External Research

### sRGB Handling in wgpu

`Rgba8Unorm` and `Rgba8UnormSrgb` store data identically as four 8-bit integers. The difference is automatic GPU conversion: `Rgba8UnormSrgb` applies the sRGB EOTF on read (sRGB to linear) and the sRGB OETF on write (linear to sRGB). `Rgba8Unorm` performs no conversion. The wgpu wiki describes these as "monitor referred" vs. "scene referred" values.

The recommended pattern for a correct sRGB pipeline is: load color textures as `Rgba8UnormSrgb`, perform lighting in linear space, output via `Rgba8UnormSrgb` render target. Sunlit Earth deliberately uses `Rgba8Unorm` throughout, which is internally consistent but means shading math happens in sRGB space.

The `view_formats` feature (wgpu 0.15+) allows mixing sRGB and linear views of one texture, which could enable a future linear-light upgrade without changing texture storage. This is not needed for the color correction feature.

**Sources**: [wgpu TextureFormat docs](https://wgpu.rs/doc/wgpu/enum.TextureFormat.html), [wgpu wiki on texture color formats](https://github.com/gfx-rs/wgpu/wiki/Texture-Color-Formats-and-Srgb-conversions), [Learn Wgpu HDR tutorial](https://sotrh.github.io/learn-wgpu/intermediate/tutorial13-hdr/), [wgpu GitHub issue #3030](https://github.com/gfx-rs/wgpu/issues/3030)

### Gamma Correction in Fragment Shaders

The `pow(color, 1.0/gamma)` formula is the standard approach, confirmed across all sources consulted. The `max(color, 0.0)` guard against negative inputs is essential because WGSL `pow` of a negative value is undefined.

For a user-facing artistic control (as opposed to a physically accurate tone mapper), operating directly on sRGB values is the convention. This is what Photoshop's Levels gamma midpoint does. For a physically meaningful exposure adjustment, one would need to linearize first, which is not appropriate for this use case.

The exact sRGB transfer function is piecewise (with a linear segment near black), but the power-law approximation `pow(x, 2.2)` / `pow(x, 1/2.2)` is standard and sufficient for artistic controls.

**Sources**: [LearnOpenGL Gamma Correction](https://learnopengl.com/Advanced-Lighting/Gamma-Correction), [GPU Gems 3, Chapter 24](https://developer.nvidia.com/gpugems/gpugems3/part-iv-image-effects/chapter-24-importance-being-linear), [Reedbeta sRGB GLSL gist](https://gist.github.com/Reedbeta/e8d3817e3f64bba7104b8fafd62906df)

### Saturation Adjustment in Fragment Shaders

The luminance-based mix approach (`mix(vec3(luminance), color, saturation)`) is the standard real-time technique. Rec. 709 weights `(0.2126, 0.7152, 0.0722)` are the conventional coefficients, though they are technically defined for linear RGB. Applying them to sRGB values is standard practice in all major image editors and produces perceptually acceptable results for artistic controls.

Alternative sRGB-aware weights (Poynton: `0.309, 0.609, 0.082`) exist but are not necessary for this application's accuracy requirements.

**Sources**: [Tim Severien, Colour Correction with WebGL](https://tsev.dev/posts/2020-06-19-colour-correction-with-webgl/), [30fps.net, Better sRGB to Greyscale Conversion](https://30fps.net/pages/better-srgb-to-greyscale/), [KinematicSoup gamma/linear article](https://kinematicsoup.com/news/2016/6/15/gamma-and-linear-space-what-they-are-how-they-differ)

### jxl-oxide Color Space Behavior

The `image` crate integration always produces sRGB-encoded u8 pixels via `.into_rgba8()`. The lower-level `JxlImage::request_color_encoding` API (added in jxl-oxide 0.10.0) could request linear output, but this API is not accessible through the `image` crate integration path. No changes to the texture loader are needed.

**Sources**: [jxl-oxide CHANGELOG](https://github.com/tirr-c/jxl-oxide/blob/main/CHANGELOG.md), libjxl encoder API documentation

---

## Technical Constraints

- **Uniform alignment**: The expanded `Uniforms` struct (144 bytes) is a multiple of 16, so no extra padding is needed. The WGSL and Rust struct field order must match exactly.
- **`build_frame_state` argument count**: Already has `#[allow(clippy::too_many_arguments)]`. Adding four more parameters makes it even longer. Consider grouping color correction params into a struct to reduce argument count.
- **Slint/Rust default synchronization**: The Slint `in-out property` defaults and `AppConfig::default()` values must both be 1.0 for all four parameters. A mismatch causes a brief visual flicker on startup.
- **`pow` of negative values**: The `max(color, 0.0)` guard in `apply_gamma` is non-negotiable. WGSL `pow` of a negative base is undefined and produces NaN that propagates through the entire fragment output.
- **Single-texture early return path**: Lines 66-68 of `sphere.wgsl` return early for single-texture modes (Grid, Day, Night alone). Color correction must be applied before this return, using the day correction parameters (since the day texture slot is always what's bound as `sphere_texture`).
- **Dirty-checking**: All four new parameters must be added to `FrameState` with thousandths quantization, or slider adjustments will not trigger redraws.

---

## Open Questions

1. **UI grouping**: Should the four sliders be a new "Color Correction" `GroupBox` section, or added to the existing Lighting group? A dedicated section is cleaner but adds UI height.
2. **Shader file placement**: Should the helper functions go in `blend.wgsl` (which is concatenated first) or in a new `color.wgsl` file? A new file is cleaner but adds a third concatenation to `gpu_setup.rs`.
3. **Identity optimization**: Should the shader branch to skip `apply_gamma`/`adjust_saturation` when parameters are at identity (1.0)? GPU branch divergence is zero on a sphere render (all fragments take the same branch), so this is valid but unnecessary -- `pow(x, 1.0) = x` and `mix(grey, color, 1.0) = color` are already identity operations.

---

## Recommendations

1. **Add four `f32` uniforms** (`day_gamma`, `day_saturation`, `night_gamma`, `night_saturation`) to the end of the Uniforms struct. Update the size assertion to 144 bytes. No padding needed.

2. **Add `apply_gamma` and `adjust_saturation` helper functions** to `blend.wgsl`. Apply gamma first, then saturation, to both day and night texture samples in `fs_main`, after `textureSample` and before `blend_fragment`. Also apply to the single-texture early return path.

3. **Thread parameters through the full pipeline** following the established `fresnel_mix` pattern: Slint property, slider, `AppConfig`, `read_config_from_window`, `apply_config_to_window`, `on_reset_all`, `BeforeRendering` read, `FrameState`, `ShadingParams`, `write_uniforms`.

4. **Do not change sRGB handling.** The current `Rgba8Unorm`-throughout pipeline is internally consistent. The gamma and saturation controls operate as expected on sRGB-encoded values, matching standard image editor behavior.

5. **Always apply the operations** (no identity-value branching). The math is identity-neutral at the default values and the performance cost is negligible. This keeps the shader simpler.

6. **Guard `pow` against negative inputs** with `max(color, vec3(0.0))`. This is the single most important correctness detail in the implementation.

---

## Sources

| Document                                  | Focus Area                                                                                             |
|-------------------------------------------|--------------------------------------------------------------------------------------------------------|
| `2026-03-18-color-correction-codebase.md` | Shader pipeline, uniform layout, UI patterns, data flow trace, sRGB analysis, dirty-checking           |
| `2026-03-18-color-correction-external.md` | wgpu sRGB formats, jxl-oxide color space, gamma/saturation algorithms, best practices, common mistakes |
