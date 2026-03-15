# Research: JXL Texture Support (2026-03-15)

## Problem Statement

Sunlit Earth renders a 3D Earth globe using equirectangular textures loaded as RGBA8 pixel data into wgpu. The project currently loads a single committed JPEG file (`textures/earth_4k.jpg`, 1.42 MB, 4096x2048) at startup via the `image` crate (v0.25, JPEG feature only).

The texture pre-processing pipeline (completed separately) has produced two JPEG-XL files: a day surface texture (`world.topo.200405.jxl`, 1.99 MB) and a night lights texture (`BlackMarble_2016.jxl`, 1.32 MB). These are already present in the `textures/` directory but untracked in git and unusable because the Rust application has no JXL decoding capability.

This task adds JPEG-XL decoding to the Rust application, replaces the single JPEG earth texture with the two JXL textures (day and night), and extends the UI to allow switching between Grid, Day, and Night textures.

## Requirements

1. **JXL decoding**: Load `.jxl` texture files at startup, producing the same RGBA8 pixel data the renderer already consumes.
2. **Two earth textures**: Load both day and night textures (when available), each getting its own GPU bind group.
3. **UI extension**: The texture ComboBox should offer "Grid", "Day", and "Night" options, driven from Rust.
4. **Minimal code changes**: Leverage the existing `image::open()` call path and `DecodedImage` interchange type. The coordinate transforms (fliph + shift) are format-agnostic and must remain unchanged.
5. **No C dependencies**: Maintain the project's `unsafe_code = "deny"` policy and avoid introducing C/C++ build toolchain requirements.
6. **Windows compatibility**: Must build and run on Windows 11 without extra system libraries.

## Findings

### JXL Decoder Selection

The Rust ecosystem has two credible JXL decoding crates and two that are unsuitable.

**jxl-oxide** (v0.12.5, released 2025-09-30) is a pure Rust, spec-conforming JPEG-XL decoder. MIT OR Apache-2.0 license. Actively maintained by tirr-c (Wonwoo Choi). Key characteristics:

- Pure Rust with zero C/C++ dependencies. No build-time toolchain requirements beyond `rustc`.
- Multithreaded decoding via Rayon (default feature, uses all available cores).
- `image` feature flag (added in v0.12.5, PR #480) registers a decoding hook with the `image` crate's external format registration API (image v0.25.8+). After calling `jxl_oxide::integration::register_decoding_hook()`, the standard `image::ImageReader::open("file.jxl")?.decode()?` path transparently handles JXL files.
- Output converts to `DynamicImage`, which supports `.into_rgba8()` for GPU-ready pixel data.
- Color management: built-in sRGB handling works without any external CMS. Optional `lcms2` (C) or `moxcms` (pure Rust) features exist for complex ICC profiles but are not needed for NASA textures.
- Performance: approximately 2x slower than libjxl C++ reference implementation. For a one-time startup load of 4K textures, this translates to a few hundred milliseconds on a modern CPU — acceptable.
- Windows CI-verified. No special platform requirements.
- Known issue: docs.rs build failed for v0.12.5 (v0.12.4 docs are available and current).
- Rust edition 2024, requires Rust 1.85+.

**jpegxl-rs** (v0.13.1) wraps the official libjxl C++ library. GPL-3.0-or-later license (compatible with this project). However, it requires MSVC, Clang, Ninja, and CMake to build on Windows with the `vendored` feature, or a pre-installed libjxl library. This significantly complicates CI and developer onboarding.

**libjxl/jxl-rs** (v0.3.0) is the official libjxl team's Rust reimplementation. Work-in-progress, not production-ready as of early 2026.

**zune-jpegxl** (v0.5.2) is an encoder only, not a decoder.

**Recommendation: jxl-oxide.** It is the only production-ready pure-Rust decoder. It integrates directly with the `image` crate's hook system, requires no build toolchain changes, and respects the project's `unsafe_code = "deny"` policy.

### Current Texture System in Rust

The existing texture pipeline is linear and format-agnostic after the decode step:

- `earth_texture.rs` exports `DecodedImage { pixels: Vec<u8>, width: u32, height: u32 }` as the universal interchange type between the loader and the renderer.
- `earth_texture::load(path)` calls `image::open()` (format auto-detected), applies `fliph()` then a 3/4-width `rotate_right` shift, and returns `DecodedImage`. These coordinate transforms work on raw RGBA8 data and are independent of the source format.
- `renderer.rs` receives raw RGBA8 pixels and creates a wgpu texture via `create_mipmapped_texture()`, which computes CPU-side mip levels via box-filter downsampling and uploads all levels with `queue.write_texture()`.
- A single shared sampler (trilinear + 16x anisotropic, Repeat on U / ClampToEdge on V) is used for all textures.
- The fragment shader simply samples one texture at the interpolated UV; there is no day/night blending logic.

The `image` crate is configured as `image = { version = "0.25", default-features = false, features = ["jpeg"] }`. It has **no built-in JXL support** as of v0.25.10. JXL is provided exclusively via the external hook mechanism introduced in v0.25.8.

### UI Texture Switching

The texture ComboBox is data-driven:

- `texture-options` is an `in` property (string array set from Rust).
- `texture-index` is an `in-out` integer property (bidirectional).
- `texture-changed` callback triggers a redraw.

No Slint-side changes are needed to add a third option. The Rust side calls `set_texture_options()` with a `Vec<SharedString>` and the Slint ComboBox adapts automatically.

### GPU Bind Group Architecture

Currently, `GpuResources` stores two named bind groups:

```rust
struct GpuResources {
    grid_bind_group: wgpu::BindGroup,
    earth_bind_group: Option<wgpu::BindGroup>,
    // ...
}
```

Texture selection at render time is a simple index check: index 0 = grid, index 1 = earth (falling back to grid if earth is `None`). The dirty-checking `FrameState` already includes `texture_index: i32`, so switching textures invalidates the cache and triggers a re-render.

With three textures, this named-field pattern becomes unwieldy. A `Vec<wgpu::BindGroup>` (grid always at index 0, loaded textures at 1+) would be cleaner:

```rust
struct GpuResources {
    bind_groups: Vec<wgpu::BindGroup>,  // 0 = grid, 1+ = loaded textures
    // ...
}
```

Selection becomes `bind_groups[texture_index.clamp(0, len - 1)]`.

### Integration Path

**Step 1 — Raise the image version floor to 0.25.8** to access the external decoder hook API.

```toml
image = { version = "0.25.8", default-features = false, features = ["jpeg"] }
```

**Step 2 — Add jxl-oxide with `image` and `rayon` features.**

```toml
jxl-oxide = { version = "0.12.5", default-features = false, features = ["image", "rayon"] }
```

`default-features = false` avoids pulling in CMS dependencies. The `rayon` feature enables multithreaded decode for better startup performance.

**Step 3 — Register the decoding hook once at startup**, before any image load.

```rust
jxl_oxide::integration::register_decoding_hook();
```

This is idempotent and should live in `main()` before `resolve_textures_dir()`.

**Step 4 — Update texture loading in `main.rs`.** Replace the single hardcoded `earth_4k.jpg` path with loading of two JXL files (`world.topo.200405.jxl` for day, `BlackMarble_2016.jxl` for night). The existing `earth_texture::load()` function works unchanged because `image::open()` auto-detects format after the hook is registered.

**Step 5 — Update texture options and bind groups.** Pass both `DecodedImage` values to the renderer. Create bind groups for each loaded texture. Update the ComboBox labels to `["Grid", "Day", "Night"]` (omitting labels for textures that failed to load).

**Step 6 — Generalize bind group storage** from named fields to a `Vec<wgpu::BindGroup>`, with index-based selection clamped to bounds.

### Data Flow (Current vs. Proposed)

**Current:**
```
textures/earth_4k.jpg
  -> image::open() [JPEG decoder]
  -> fliph + shift
  -> DecodedImage
  -> create_mipmapped_texture()
  -> earth_bind_group
  -> texture_index == 1 selects it
```

**Proposed:**
```
jxl_oxide::integration::register_decoding_hook()

textures/world.topo.200405.jxl
  -> image::open() [JXL decoder via hook]
  -> fliph + shift -> DecodedImage
  -> create_mipmapped_texture() -> day_bind_group

textures/BlackMarble_2016.jxl
  -> image::open() [JXL decoder via hook]
  -> fliph + shift -> DecodedImage
  -> create_mipmapped_texture() -> night_bind_group

ComboBox: ["Grid", "Day", "Night"]
  index 0 -> grid_bind_group
  index 1 -> day_bind_group
  index 2 -> night_bind_group
```

### Memory Considerations

Each 4096x2048 RGBA8 texture with full mipmaps consumes approximately 64 MB of GPU memory (32 MB base + mipmaps). Two earth textures double this to ~128 MB. This is acceptable on any discrete GPU and most integrated GPUs. Higher resolutions (8192x4096 = ~256 MB each) may need consideration on low-VRAM systems but are outside the scope of this initial task.

The CPU-side `DecodedImage` buffers (~32 MB each for 4K) are transient — they can be dropped after GPU upload. JXL decode itself may use additional working memory during the Rayon-parallelized decode, but this is also transient.

## External Research

All external findings come from the external research document with source citations.

**jxl-oxide ecosystem position** (High confidence): No serious competition in the pure-Rust JXL decoder space. The `image` crate hook integration (v0.12.5, PR #480) is the canonical way to add JXL support to projects already using the `image` crate. Verified from GitHub repository, crates.io metadata, and release notes.

**jpegxl-rs Windows build requirements** (High confidence): MSVC + Clang + CMake required. Documented in the crate's own README. Verified from lib.rs and GitHub.

**image crate JXL status** (High confidence): No built-in JXL support as of v0.25.10. Confirmed from the official image-rs/image GitHub repository. The format list (AVIF, BMP, DDS, EXR, FF, GIF, HDR, ICO, JPEG, PNG, PNM, QOI, TGA, TIFF, WebP) does not include JXL.

**jxl-oxide performance** (Medium confidence): ~2x slower than libjxl C++ reference. No benchmarks found for specific texture sizes relevant to this project. Worth timing during implementation.

**RGBA8 output compatibility** (High confidence): `DynamicImage::into_rgba8()` is standard image crate API, producing `ImageBuffer<Rgba<u8>, Vec<u8>>`. The raw bytes are identical in layout to what JPEG decoding produces — no changes to the wgpu upload path.

Sources are listed in full in the external research document.

## Technical Constraints

1. **GPU texture format**: The renderer expects RGBA8 pixel data in `Rgba8Unorm` format. The JXL decoder must produce this via `.into_rgba8()`.
2. **Coordinate transforms**: The fliph + shift in `earth_texture::load()` are format-independent and must not change.
3. **`unsafe_code = "deny"`**: Rules out crates with C FFI (jpegxl-rs) or unsafe Rust internals. jxl-oxide is pure safe Rust.
4. **Slint rendering lifecycle**: GPU resources are created during `RenderingSetup` and stored in a `thread_local! { RefCell<Option<GpuResources>> }`. The additional bind groups must follow this same pattern.
5. **Dirty-checking**: `FrameState.texture_index` is already `i32` and supports arbitrary values. No changes needed to the dirty-checking mechanism.
6. **image crate version**: Must be bumped from `"0.25"` to `"0.25.8"` minimum to access the external decoder hook API.
7. **Backward compatibility**: If JXL files are not found in the textures directory, the application should degrade gracefully (same as the current behavior when `earth_4k.jpg` is missing).

## Open Questions

1. **Startup latency**: Exact decode time for two 4K JXL files on a mid-range CPU is unknown. Should be measured during implementation. If unacceptable, options include: lazy-loading the non-default texture, or decoding on a background thread before GPU upload.

2. **image hook pixel format**: Whether the hook always produces sRGB output, or passes through HDR data for HDR JXL files, was not fully confirmed. For NASA textures (sRGB), this is not a concern. Would matter only if HDR JXL textures are introduced in the future.

3. **`register_decoding_hook` API surface**: Whether jxl-oxide's `image` feature also exports a `JxlDecoder` struct for manual use (vs. only the hook registration function) was not fully confirmed from public docs. The hook is sufficient for this project's needs. If finer control is needed (e.g., passing `force_wide_buffers()` for very large images), the direct `JxlImage::builder().open()` API can be used as a fallback, bypassing `image::open()`.

4. **Removal of `earth_4k.jpg`**: Whether to keep the JPEG file for backward compatibility or remove it in favor of the JXL files is a project decision. The JXL files are strictly better (smaller, higher quality). The JPEG could be removed once JXL support is confirmed working.

## Recommendations

1. **Use jxl-oxide with `image` and `rayon` features, `default-features = false`.** This is the path of least resistance: a single `register_decoding_hook()` call makes the existing `image::open()` code path handle JXL files transparently. No changes to `earth_texture::load()` beyond the hook registration.

2. **Generalize bind group storage to `Vec<wgpu::BindGroup>`.** The current two-field pattern (grid + optional earth) does not scale to three textures. A Vec with the grid always at index 0 is cleaner and supports future additions (e.g., cloud overlay, specular map) without structural changes.

3. **Load both JXL textures eagerly at startup.** The combined decode time for two 4K JXL files should be under one second. Add timing log statements to verify. If startup latency proves problematic, lazy-loading or background decoding can be added later.

4. **Graceful degradation.** Each texture should load independently. If day loads but night fails, show "Grid" and "Day" only. If both fail, show "Grid" only. This matches the current pattern where a missing `earth_4k.jpg` simply hides the "Earth" option.

5. **Time the JXL decode.** Add `eprintln!` timing around each texture load to measure real-world startup impact. This resolves the main open question cheaply.

6. **Keep JPEG feature in image crate.** Even after switching to JXL files, retaining `features = ["jpeg"]` costs nothing and maintains backward compatibility if users have custom JPEG textures.

## Sources

| Document | Focus Area |
|---|---|
| `docs/plans/2026-03-15-jxl-texture-support-codebase.md` | Current Rust texture loading, UI wiring, GPU pipeline, bind group architecture, dirty-checking, file inventory, impact analysis |
| `docs/plans/2026-03-15-jxl-texture-support-external.md` | JXL crate survey (jxl-oxide, jpegxl-rs, jxl-rs, zune-jpegxl), image crate hook API, integration steps, memory considerations, performance estimates |
