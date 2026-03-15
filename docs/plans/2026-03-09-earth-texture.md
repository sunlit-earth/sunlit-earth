# Plan 02 — Earth Texture

**Goal:** Display a real NASA Blue Marble texture on the sphere, with a UI dropdown to switch between the grid (dev) texture and the earth texture.

## Source imagery

The source is NASA Blue Marble (world.topo.bathy.200406) at 21600×10800 pixels, available as both JPG (27 MB) and GeoTIFF (259 MB) in `tmp/`. The GeoTIFF carries geo-referencing metadata we don't need, so we'll work from the JPG.

21600×10800 is far too large for a desktop wallpaper renderer. Reasonable target resolutions:

| Resolution | Pixels | ~JPG size | Notes |
|------------|--------|-----------|-------|
| 2048×1024 | 2M | ~300 KB | Matches current grid texture. Visibly blurry on 4K. |
| 4096×2048 | 8M | ~1.5 MB | Good balance. Sharp enough for most displays. |
| 8192×4096 | 33M | ~5 MB | Overkill for now, but future-proof for 4K+ wallpapers. |

**Recommendation:** Ship 4096×2048 as the default. It's sharp enough for 1440p wallpaper use and keeps file size reasonable. We can add higher resolutions later when the wallpaper export pipeline exists and we know the target display resolution.

The downscaled texture should be prepared offline (e.g. via ImageMagick) and committed to the repo in a `textures/` directory. The `tmp/` folder with the raw source files stays git-ignored.

## Image format

JPG is fine for photographic Earth imagery — the compression artifacts are invisible at this scale, and the file size is much smaller than PNG. For textures that need transparency or lossless quality (e.g. cloud overlays later), PNG would be appropriate, but not here.

## Decoding at runtime

The `image` crate (0.25.x) is already in the dependency tree via Slint, so adding it as a direct dependency has zero cost in compile time or binary size. We'll use it to decode JPG → RGBA8 pixels, then upload to a wgpu texture with CPU-generated mipmaps (same approach as the grid texture).

## File loading

Textures ship as separate files alongside the binary in a `textures/` directory. At runtime, locate them relative to the executable path (`std::env::current_exe()` → parent → `textures/`). This keeps the binary small and makes it easy to add more textures later.

For development, we can also support a `SUNLIT_EARTH_TEXTURES` environment variable or a `--textures-dir` CLI flag to override the path, which is useful when running via `cargo run` (where the executable is deep inside `target/`).

## Texture switching

Add a "Texture" ComboBox to the UI with options like "Grid" and "Earth". Switching textures requires:

1. Decode the new image (if not already cached).
2. Create a new wgpu texture + upload with mipmaps.
3. Create a new bind group pointing to the new texture/sampler.
4. Replace the bind group in `GpuResources` and invalidate dirty state to force a re-render.

The grid texture is generated procedurally (no file I/O). The earth texture is loaded from disk. Both produce the same output: RGBA8 pixel data that gets uploaded via `create_grid_texture`-style logic.

### Texture caching

Both textures should be created during `RenderingSetup` and kept alive in `GpuResources`. Switching between them only swaps which bind group is used in the render pass — no texture re-upload needed. This makes switching instant.

## Changes needed

### `textures/` directory (new)
- `earth_4k.jpg` — downscaled 4096×2048 Blue Marble image

### `ui/main.slint`
- Add a "Texture" ComboBox row above the Anti-Aliasing row
- Expose `texture-index` property and `texture-changed()` callback

### `src/renderer.rs`
- Add fields to `GpuResources`: a second bind group for the earth texture (or store both textures + swap bind groups)
- Add `texture_index` to `FrameState` for dirty-checking
- Factor out texture upload logic (currently `create_grid_texture`) into a reusable function that takes RGBA pixel data
- Load and decode `earth_4k.jpg` during `RenderingSetup`
- On texture switch: swap bind group, invalidate last state

### `src/main.rs`
- Wire up the texture ComboBox callback to `request_redraw()`
- Pass texture directory path to the renderer (resolved from exe path or env/CLI override)

### `Cargo.toml`
- Add `image = { version = "0.25", default-features = false, features = ["jpeg"] }` as a direct dependency

### `src/earth_texture.rs` (new module)
- Load and decode a JPG file to RGBA8 pixel data
- Possibly just a thin wrapper: `image::open(path)?.to_rgba8()`

## Steps

1. Downscale the source image to 4096×2048 and save to `textures/earth_4k.jpg`.
2. Add `image` crate dependency.
3. Create `earth_texture.rs` with file loading + decode.
4. Refactor texture upload in `renderer.rs` so both grid and earth textures use the same mipmap + upload path.
5. Store both bind groups in `GpuResources`; select active one based on UI state.
6. Add texture ComboBox to Slint UI.
7. Wire up callbacks in `main.rs`.
8. Add `texture_index` to `FrameState` dirty-checking.
9. Test: switching between grid and earth should be instant, no flicker.

## Texture directory resolution

The texture directory is resolved via a fallback chain:

1. `--textures-dir` CLI flag (highest priority)
2. `SUNLIT_EARTH_TEXTURES` environment variable
3. `textures/` relative to the executable path
4. `textures/` relative to the current working directory

This covers all use cases: installed deployments (exe-relative), `cargo run` during development (cwd-relative), and explicit overrides for testing.

## Future considerations

- **8192×4096** makes sense as a later addition, especially for zoomed-in views of the planet. The loading and mipmap pipeline will already support arbitrary resolutions.
- When we add day/night, clouds, and specular maps, we'll need a more structured texture management approach (texture manifest, async loading, etc.). For now, keep it simple — just two textures with hardcoded names.
