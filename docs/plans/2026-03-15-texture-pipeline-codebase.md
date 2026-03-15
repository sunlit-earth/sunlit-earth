# Texture Pipeline — Codebase Research

## Current Texture System

### Earth Texture Loading (`src/earth_texture.rs`)

- Loads JPG files using the `image` crate (v0.25, JPEG feature only)
- Decodes to RGBA8
- Applies horizontal flip (sphere UV winding convention)
- Applies horizontal shift by 1/4 width (aligns prime meridian to u=0)
- Texture directory resolution via fallback chain: CLI flag → env var → exe-relative → cwd-relative

### Grid Texture (`src/grid_texture.rs`)

- Procedurally generated 2048×1024 RGBA8 texture
- No file I/O; computed at startup
- Not relevant to the pipeline, but shows the expected texture format

### Renderer Integration (`src/renderer.rs`)

- `create_mipmapped_texture()` (lines 602-643): Takes raw RGBA8 pixel data
- Creates wgpu texture with mip levels: `width.max(height).ilog2() + 1`
- CPU-side mipmap generation via box-filter `downsample_2x()` (lines 705-730)
- **Texture format**: `wgpu::TextureFormat::Rgba8Unorm`
- **Sampler** (lines 323-332): Trilinear + 16× anisotropic, Repeat U / ClampToEdge V

### Current Texture File

- `textures/earth_4k.jpg` — 4096×2048, ~1.42 MB, committed to repo
- Filename hardcoded in `main.rs` line 51

## Format Constraints

- GPU expects RGBA8 pixel data
- Textures must be equirectangular with 2:1 aspect ratio
- Coordinate transforms (flip + shift) currently applied at load time in Rust
- Image crate `0.25` has no JPEG-XL support

## Resolution Strategy (from docs/plans/2026-03-09-earth-texture.md)

- 2048×1024: ~300 KB (blurry on 4K)
- 4096×2048: ~1.5 MB (recommended default)
- 8192×4096: ~5 MB (future-proofing)
- Plans for multiple resolutions with UI selection

## No Existing Python/uv Setup

- No `pyproject.toml`, `uv.lock`, or Python-related files
- No build scripts using Python
- Greenfield opportunity for the tools directory

## Key Implications for Pipeline Design

1. **Output format**: Pipeline produces files that Rust code will decode; Rust side needs JPEG-XL decoding support added later (separate concern)
2. **Coordinate transforms**: Could be applied in the pipeline (pre-baked) or kept in Rust loader; pipeline should at least support it as an option
3. **Mipmap pre-computation**: Currently CPU-generated at runtime; pipeline could optionally pre-compute for faster startup
4. **Multiple resolutions**: Pipeline should support generating multiple output sizes from one source
5. **Aspect ratio**: Must maintain exact 2:1 for equirectangular projection
