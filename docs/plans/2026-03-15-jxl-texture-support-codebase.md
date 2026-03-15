# Codebase Research: JXL Texture Support and Day/Night Textures

Date: 2026-03-15

This document captures a thorough analysis of the current texture loading, UI texture switching, and rendering pipeline in the Sunlit Earth Rust codebase, for the goal of adding JPEG-XL decoding support and replacing existing JPG textures with new day/night JXL textures.

---

## 1. Current Texture Loading

### 1.1 Earth Texture (`src/earth_texture.rs`)

The module provides two public items:

**`DecodedImage` struct** holds raw RGBA8 pixel data with width/height metadata. This is the universal interchange type between the loader and the renderer:

```rust
pub struct DecodedImage {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
}
```

**`load(path: &Path) -> Result<DecodedImage, String>`** decodes a single image file using the `image` crate's `image::open()` function, then applies two coordinate transforms:

1. **Horizontal flip** (`fliph()`): Standard equirectangular maps have east-to-the-right, but the sphere UV winding goes in the opposite direction.
2. **Horizontal shift** (`shift_horizontal`): Rotates all rows right by 3/4 width (equivalent to shifting left by 1/4 width), aligning the prime meridian with u=0 in the sphere's UV mapping.

The `image::open()` call uses the `image` crate's format auto-detection. Currently only JPEG decoding is enabled via the `features = ["jpeg"]` flag in Cargo.toml.

**`resolve_textures_dir(cli_override: Option<&Path>) -> Option<PathBuf>`** resolves the textures directory via a four-step fallback chain:
1. `--textures-dir` CLI flag
2. `SUNLIT_EARTH_TEXTURES` environment variable
3. `textures/` relative to the executable
4. `textures/` relative to the current working directory

### 1.2 Grid Texture (`src/grid_texture.rs`)

A procedurally generated 2048x1024 RGBA8 equirectangular grid texture. It has no file I/O and is computed at startup via `generate(width, height) -> Vec<u8>`. It draws latitude/longitude lines with ocean/land color blending.

This module is not affected by JXL support but demonstrates the expected texture data format: raw `Vec<u8>` of RGBA8 pixels.

### 1.3 Image Crate Configuration (`Cargo.toml`, line 15)

```toml
image = { version = "0.25", default-features = false, features = ["jpeg"] }
```

The `image` crate is used with `default-features = false` and only the `jpeg` feature enabled. This is a deliberate choice to minimize binary size and compile time. The `image::open()` function detects format from file contents, so adding new format support requires enabling additional features or registering external decoders.

**Key finding**: The `image` crate v0.25 does NOT have built-in JPEG-XL support. JXL is not in its `default-formats` feature set. An external decoder must be used.

---

## 2. UI Texture Switching

### 2.1 Slint UI (`ui/main.slint`)

The texture ComboBox is defined in the controls panel:

```slint
in property <[string]> texture-options: ["Grid", "Earth"];
in-out property <int> texture-index: 1;

callback texture-changed();

texture-combo := ComboBox {
    model: texture-options;
    current-index <=> root.texture-index;
    selected => { root.texture-changed(); }
}
```

Key observations:
- `texture-options` is an `in` property (set from Rust, read-only from Slint).
- `texture-index` is `in-out` (bidirectional between Rust and Slint).
- The `texture-changed` callback fires when the user selects a different option.
- The default model is `["Grid", "Earth"]` but this is overwritten from Rust.

### 2.2 Rust-side UI Wiring (`src/main.rs`)

Texture options are set up in `main()`:

```rust
// Set up texture options -- only show "Earth" if the texture loaded
let has_earth = earth_pixels.is_some();
if has_earth {
    let labels: Vec<slint::SharedString> = vec!["Grid".into(), "Earth".into()];
    window.set_texture_options(slint::ModelRc::new(slint::VecModel::from(labels)));
}
```

If no earth texture is available, the default `["Grid", "Earth"]` model from Slint stays, but no earth bind group is created, so selecting "Earth" falls back to the grid bind group (see renderer section below).

The default texture index is set to 1 (Earth) when the earth texture is available, deferred via `slint::invoke_from_event_loop`:

```rust
if has_earth {
    win.set_texture_index(1);
}
```

The `texture-changed` callback triggers a redraw:

```rust
window.on_texture_changed(move || {
    if let Some(win) = window_weak.upgrade() {
        win.window().request_redraw();
    }
});
```

### 2.3 Current Texture Loading Flow

The earth texture is loaded eagerly at startup, before the window is created:

1. `resolve_textures_dir()` finds the textures directory.
2. Hardcoded filename `earth_4k.jpg` is joined to the directory path.
3. `earth_texture::load()` decodes the JPG to `DecodedImage`.
4. The `DecodedImage` is passed to `renderer::setup_rendering_notifier()`.
5. During `RenderingSetup`, the pixels are consumed to create a GPU texture.

**Important**: The earth texture filename is hardcoded in `main.rs` line 51:
```rust
let path = dir.join("earth_4k.jpg");
```

---

## 3. GPU Texture Creation and Upload

### 3.1 Texture Upload Pipeline (`src/renderer.rs`)

All textures flow through `create_mipmapped_texture()` (lines 603-643):

```rust
fn create_mipmapped_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    width: u32,
    height: u32,
    rgba_pixels: &[u8],
) -> wgpu::Texture
```

This function:
1. Computes mip level count: `width.max(height).ilog2() + 1`
2. Creates a wgpu texture with all mip levels at format `Rgba8Unorm`
3. Uploads mip level 0 from the raw pixel data
4. Generates mip levels 1..N via CPU box-filter downsampling (`downsample_2x()`)
5. Uploads each mip level via `queue.write_texture()`

The texture usage flags are `TEXTURE_BINDING | COPY_DST`.

### 3.2 Bind Group Creation

Each texture gets its own bind group via `create_bind_group()`, containing:
- Binding 0: Uniform buffer (MVP matrix, shared across all textures)
- Binding 1: Texture view
- Binding 2: Sampler (shared, trilinear + 16x anisotropic)

Both bind groups are created during `RenderingSetup` and stored in `GpuResources`:

```rust
struct GpuResources {
    grid_bind_group: wgpu::BindGroup,
    earth_bind_group: Option<wgpu::BindGroup>,
    // ...
}
```

The earth bind group is `Option` because it may not be available if the texture failed to load.

### 3.3 Texture Selection at Render Time

During `BeforeRendering`, the bind group is selected based on `texture_index`:

```rust
let bind_group = if texture_index == 1 {
    res.earth_bind_group.as_ref().unwrap_or(&res.grid_bind_group)
} else {
    &res.grid_bind_group
};
pass.set_bind_group(0, bind_group, &[]);
```

This is a simple index-based switch: index 0 = grid, index 1 = earth. If earth is not available, it falls back to grid. There is no provision for more than two textures currently.

### 3.4 Sampler Configuration

A single shared sampler is used for all textures:

```rust
let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
    address_mode_u: wgpu::AddressMode::Repeat,
    address_mode_v: wgpu::AddressMode::ClampToEdge,
    mag_filter: wgpu::FilterMode::Linear,
    min_filter: wgpu::FilterMode::Linear,
    mipmap_filter: wgpu::MipmapFilterMode::Linear,
    anisotropy_clamp: 16,
    ..Default::default()
});
```

### 3.5 Shader (`shaders/sphere.wgsl`)

The fragment shader simply samples the texture at the interpolated UV coordinate:

```wgsl
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let color = textureSample(sphere_texture, sphere_sampler, in.uv).rgb;
    return vec4<f32>(color, 1.0);
}
```

There is currently no day/night blending logic in the shader; it samples a single texture.

---

## 4. Dirty-Checking for Texture Changes

The `FrameState` struct captures all inputs that affect the rendered image:

```rust
#[derive(Clone, PartialEq)]
struct FrameState {
    longitude: f32,
    latitude: f32,
    zoom: f32,
    sample_count: u32,
    texture_index: i32,
    width: u32,
    height: u32,
}
```

Before each render, the current state is compared to the last rendered state:

```rust
if res.last_state.as_ref() == Some(&current_state) {
    return;
}
```

The `texture_index` field means switching the texture ComboBox invalidates the dirty state and triggers a re-render. No new GPU resources are created; only the bind group selection changes.

---

## 5. Existing Texture Files

```
textures/
  earth_4k.jpg          -- 1,490,356 bytes (1.42 MB), 4096x2048, committed
  BlackMarble_2016.jxl   -- 1,382,310 bytes (1.32 MB), untracked, JXL night lights
  world.topo.200405.jxl  -- 2,086,065 bytes (1.99 MB), untracked, JXL day texture
```

The two JXL files are new, produced by the texture pre-processing pipeline (`tools/texture-pipeline/`). They are currently untracked in git. These are the day and night textures that will replace `earth_4k.jpg` once JXL decoding is added to the Rust side.

---

## 6. JPEG-XL Decoding Options for Rust

### 6.1 Option A: `jxl-oxide` (Pure Rust, recommended)

[jxl-oxide](https://crates.io/crates/jxl-oxide) (v0.12.5, released 2025-09-30) is a pure Rust JPEG XL decoder. Key characteristics:

- **Pure Rust**: No C dependencies, no build complexity, no `unsafe` code concerns.
- **`image` crate integration**: The `image` feature flag exposes `jxl_oxide::integration::JxlDecoder` which implements `image::ImageDecoder`. This plugs into the `image` crate's hooks/registration system so that `image::open("file.jxl")` can decode JXL files transparently.
- **Optional dependency**: `jxl-oxide = { version = "0.12", features = ["image"] }` in Cargo.toml, with `image` crate's `hooks` module for registration.
- **Color management**: Basic built-in CMS for sRGB conversions; optional `lcms2` or `moxcms` features for complex profiles. For this project, sRGB output is all that's needed.
- **License**: MIT OR Apache-2.0.

The `image` crate v0.25.8 added a hooks module allowing external decoders to be registered:
```rust
// Conceptual usage (exact API to be verified):
// Register JxlDecoder with image crate, then image::open() handles .jxl files
```

Alternatively, jxl-oxide can be used directly without the `image` crate integration:
```rust
let image = JxlImage::builder().open("input.jxl")?;
let render = image.render_frame(0)?;
// Extract pixel data from render
```

### 6.2 Option B: `jpegxl-rs` (libjxl bindings)

[jpegxl-rs](https://crates.io/crates/jpegxl-rs) (v0.13.1) wraps the official libjxl C library. Key characteristics:

- **C dependency**: Requires libjxl. Has a `vendored` feature for static linking which builds libjxl from source (needs CMake + C/C++ compiler).
- **Performance**: Reference implementation, likely faster than pure Rust for large images.
- **Build complexity**: Vendored build is slow and adds C toolchain requirement. Non-vendored requires libjxl installed on system.
- **License**: GPL-3.0-or-later (matches this project's license).

### 6.3 Recommendation

**Use `jxl-oxide` with the `image` feature.** Reasons:
1. Pure Rust aligns with the project's `unsafe_code = "deny"` policy.
2. No build toolchain requirements beyond `rustc`.
3. The `image` crate integration means `earth_texture::load()` can decode JXL files with minimal code changes -- `image::open()` will handle format detection if the decoder is registered.
4. Performance is adequate for loading a single 4K-8K texture at startup.

---

## 7. Impact Analysis for JXL Support + Day/Night Textures

### 7.1 Changes Needed

**`Cargo.toml`**:
- Add `jxl-oxide` dependency with `image` feature.
- May need to add `image` features for the hooks module if not already included.

**`src/earth_texture.rs`**:
- Register the JXL decoder with the `image` crate's hooks system (one-time initialization).
- The existing `load()` function uses `image::open()` which auto-detects format, so it should work for JXL files once the decoder is registered.
- The coordinate transforms (fliph + shift) are format-agnostic and work on any RGBA8 data.

**`src/main.rs`**:
- Change the hardcoded filename `earth_4k.jpg` to load JXL files instead (e.g. `world.topo.200405.jxl` for day, `BlackMarble_2016.jxl` for night).
- Load two earth textures (day and night) instead of one.
- Update the texture ComboBox labels to reflect available textures (e.g. "Grid", "Day", "Night").
- Pass both `DecodedImage` values to the renderer.

**`src/renderer.rs`**:
- Add a third bind group for the night texture (or generalize to a `Vec<wgpu::BindGroup>`).
- Update the texture selection logic to handle index 0=Grid, 1=Day, 2=Night.
- Update `FrameState` if needed (texture_index already supports arbitrary `i32` values).

**`ui/main.slint`**:
- No structural changes needed. The `texture-options` property is already a string array set from Rust, and `texture-index` is an integer. Adding a third option ("Night") requires no Slint-side changes.

**Shader** (`shaders/sphere.wgsl`):
- No changes needed for basic texture switching. Day/night blending based on sun position would be a future shader change, not part of this initial JXL support task.

### 7.2 Data Flow (Current vs. Proposed)

**Current**:
```
textures/earth_4k.jpg
  -> image::open() [JPEG decoder]
  -> fliph + shift
  -> DecodedImage { pixels, width, height }
  -> create_mipmapped_texture()
  -> earth_bind_group
  -> texture_index == 1 selects it
```

**Proposed**:
```
textures/world.topo.200405.jxl
  -> image::open() [JXL decoder via jxl-oxide]
  -> fliph + shift
  -> DecodedImage { pixels, width, height }
  -> create_mipmapped_texture()
  -> day_bind_group

textures/BlackMarble_2016.jxl
  -> image::open() [JXL decoder via jxl-oxide]
  -> fliph + shift
  -> DecodedImage { pixels, width, height }
  -> create_mipmapped_texture()
  -> night_bind_group

ComboBox: ["Grid", "Day", "Night"]
  index 0 -> grid_bind_group
  index 1 -> day_bind_group
  index 2 -> night_bind_group
```

### 7.3 Risk Assessment

| Risk | Impact | Mitigation |
|---|---|---|
| `jxl-oxide` image integration API is underdocumented | May need trial-and-error to register decoder | Can fall back to direct `JxlImage::builder().open()` API, bypassing `image::open()` |
| JXL decode is slower than JPEG at startup | Noticeable delay on first load | JXL files are the same or smaller than JPEG equivalents; decode time should be comparable for 4K textures |
| Two textures double GPU memory for earth textures | ~128 MB for two 4K RGBA8 textures with mipmaps | Acceptable; could lazy-load in future |
| `jxl-oxide` v0.12.5 failed to build on docs.rs | Documentation gap | v0.12.4 docs are available; crate itself builds fine locally |
| Coordinate transforms may behave differently for JXL sources | Visual misalignment | The transforms are on decoded RGBA8 data, not format-specific; no impact expected |

### 7.4 Texture Generalization Opportunity

The current code has `grid_bind_group` and `earth_bind_group: Option<wgpu::BindGroup>` as separate fields. With three textures (grid, day, night), this pattern becomes unwieldy. A generalization to `Vec<wgpu::BindGroup>` with the grid always at index 0 would be cleaner:

```rust
struct GpuResources {
    bind_groups: Vec<wgpu::BindGroup>,  // index 0 = grid, 1+ = loaded textures
    // ...
}
```

The texture selection in the render pass becomes:
```rust
let bind_group = &res.bind_groups[texture_index.clamp(0, res.bind_groups.len() - 1)];
```

---

## 8. Summary of Key Findings

1. **Texture loading is format-agnostic after decoding.** The `DecodedImage` struct and `create_mipmapped_texture()` function work with raw RGBA8 data. Adding JXL support only requires making `image::open()` understand JXL files.

2. **The `image` crate v0.25 has no built-in JXL support.** The `jxl-oxide` crate (pure Rust, v0.12.x) provides an `image` feature that registers a JXL decoder. This is the recommended approach.

3. **UI texture switching is data-driven.** The ComboBox labels and count are set from Rust via `set_texture_options()`. Adding a third option requires no Slint changes.

4. **Texture selection is index-based.** The `texture_index` i32 from the ComboBox directly selects a bind group. The dirty-checking already includes `texture_index`.

5. **Coordinate transforms are format-independent.** The fliph + shift operations work on decoded RGBA8 data and apply identically regardless of whether the source was JPEG or JXL.

6. **Two JXL textures already exist.** `BlackMarble_2016.jxl` (night lights, 1.32 MB) and `world.topo.200405.jxl` (day surface, 1.99 MB) are in the textures directory but untracked.

7. **The hardcoded filename is the main coupling point.** `main.rs` line 51 hardcodes `earth_4k.jpg`. This needs to change to load JXL files and support multiple earth textures.

8. **The bind group pattern needs generalization.** Currently two named fields (`grid_bind_group`, `earth_bind_group`). With three textures, a Vec-based approach is cleaner.

---

## Sources

- [jxl-oxide on crates.io](https://crates.io/crates/jxl-oxide) -- pure Rust JXL decoder, v0.12.5
- [jxl-oxide on GitHub](https://github.com/tirr-c/jxl-oxide) -- source and README
- [jxl-oxide API docs (v0.12.4)](https://docs.rs/jxl-oxide/0.12.4/jxl_oxide/) -- most recent successful docs.rs build
- [jpegxl-rs on crates.io](https://crates.io/crates/jpegxl-rs) -- libjxl bindings, v0.13.1
- [image crate on GitHub](https://github.com/image-rs/image) -- Cargo.toml confirms no JXL in default features
- [image crate hooks module](https://docs.rs/image/0.25/image/index.html) -- decoder registration API
