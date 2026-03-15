# Research: Texture Pre-Processing Pipeline (2026-03-15)

## Problem Statement

Sunlit Earth renders a 3D Earth globe using equirectangular textures loaded as RGBA8 pixel data into wgpu. The project currently ships a single committed 4096x2048 JPEG file (1.42 MB) and loads it at runtime via the `image` crate.

NASA Blue Marble source textures are available at up to 21600x10800 resolution. The project needs a pre-processing pipeline to convert these large source images into optimised, smaller files at multiple standard resolutions for distribution. JPEG XL is the target format for its superior compression of photographic content (20-55% smaller than JPEG at equivalent quality).

The pipeline will be a standalone Python CLI tool managed with `uv`, living inside the Rust repository but independent of the Rust build system.

## Requirements

1. **Input**: A folder of source textures in standard image formats (TIFF, JPEG, PNG) at arbitrary resolutions, following the equirectangular projection convention (2:1 aspect ratio, north-up, prime-meridian-left).
2. **Output**: JPEG-XL files at one or more target resolutions (e.g. 8192x4096, 4096x2048, 2048x1024), written to a new folder preserving the source directory structure.
3. **CLI parameters**: Target image width(s), JXL compression quality, JXL encoding effort, input/output paths.
4. **High-quality downscaling**: Lanczos resampling with optional post-downscale sharpening.
5. **Minimal external dependencies**: Installable on Windows, macOS, and Linux with `uv sync`.
6. **Separation of concerns**: The pipeline produces files; the Rust app will later gain JXL decoding support as a separate task.

## Findings

### Current Texture System in Rust

The existing texture pipeline is simple and linear:

- `earth_texture.rs` loads a single JPEG file using the `image` crate (v0.25, JPEG feature only), decodes to RGBA8, applies a horizontal flip (sphere UV winding) and a 1/4-width horizontal shift (prime meridian alignment).
- `renderer.rs` receives raw RGBA8 pixel data and creates a wgpu texture with CPU-computed mipmaps via a box-filter `downsample_2x()` function. The texture format is `Rgba8Unorm`.
- The sampler uses trilinear filtering with 16x anisotropic, Repeat on U axis / ClampToEdge on V axis.
- Texture resolution is documented in `plan-02-earth-texture.md` as 4096x2048 recommended default, with 2048x1024 (blurry on 4K) and 8192x4096 (future) as alternatives.

The `image` crate v0.25 has no JPEG-XL support. Adding JXL decoding to the Rust side is a separate future task, outside the scope of this pipeline.

**Key constraint**: The pipeline should output textures in standard geographic convention (north-up, prime-meridian-left). The existing coordinate transforms (flip + shift) remain in the Rust loader and should not be baked into the pipeline output.

### JPEG-XL Encoding

Four Python libraries can encode JXL. The clear winner is **pillow-jxl-plugin** (PyPI: `pillow-jxl-plugin`, v1.3.7):

- Written in Rust with safe Python bindings. Ships pre-built wheels for Windows, macOS, and Linux across CPython 3.10-3.14 and PyPy 3.11.
- Listed as an official third-party plugin in Pillow's own documentation.
- Usage: `import pillow_jxl` registers the plugin, then standard `Image.save("out.jxl", quality=85)` works.
- Documented parameters: `quality` (1-100, JPEG-style), `lossless` (boolean), and likely `effort` (1-9) though the effort parameter needs verification (see Open Questions).
- Actively maintained with regular release cadence since October 2023.

Alternatives considered and rejected:

| Library | Status | Reason to skip |
|---|---|---|
| imagecodecs | Active, heavy (~30 MB) | Overkill; bundles many unneeded codecs |
| jxlpy | Alpha, Cython-based | Unmaintained, not production-ready |
| Pillow native JXL | Not yet merged | Pillow issue #5898 still open as of early 2026 |

**Compression characteristics** (from multiple independent benchmarks):

- JXL vs. JPEG: 20-55% smaller at equivalent perceptual quality.
- JXL vs. PNG (lossless): ~46% smaller.
- For the target use case (4096x2048 photographic texture), quality 85 should produce files in the 3-6 MB range vs. 6-10 MB for equivalent JPEG.
- The quality range 85-95 is the sweet spot for photographic textures (near-invisible loss with significant compression).
- Effort 7-9 is appropriate for an offline pipeline (higher effort = smaller files, slower encode).

### Image Downscaling

**Pillow LANCZOS** is the standard recommendation for photographic downscaling:

- 8-pixel kernel window, best detail preservation and aliasing suppression.
- Since Pillow 2.7.0, anti-aliasing for downscaling is handled automatically.
- The `reducing_gap` parameter (default 2.0) provides a speed-quality tradeoff without visible quality loss.

**Post-downscale sharpening** with `ImageFilter.UnsharpMask` corrects the slight softness introduced by any downscaling filter. Recommended defaults for photographic textures:

- `radius=1.0`, `percent=80`, `threshold=3` (conservative).
- Over-sharpening should be avoided: textures viewed on a sphere at varying distances benefit from slight softness at lower mip levels.

**pyvips** is a stronger alternative for very large sources:

- Demand-driven architecture uses ~400 MB RAM for a 21600x10800 image vs. ~1.75 GB for Pillow.
- Typically 5x faster than Pillow for large images.
- Defaults to lanczos3 for downscaling; also supports nohalo and Magic Kernel Sharp (since November 2024).
- Can write JXL natively via libvips >= 8.14.
- Downside: requires the libvips native library (DLL on Windows), making installation more complex.

**Recommendation**: Start with Pillow for simplicity. Add pyvips as an optional dependency (`[project.optional-dependencies] vips = ["pyvips>=2.2"]`) for users processing full 21600x10800 Blue Marble TIFFs.

### Equirectangular Projection Considerations

Equirectangular textures have non-uniform pixel density (higher near poles). Standard LANCZOS downscaling does not account for this distortion but also does not introduce visible artifacts in rendered output. The distortion exists in the source projection, not in the downscaling.

Pole regions may appear slightly over-sharpened after downscaling (many source pixels map to the same sphere point), but latitude-aware resampling is a research-grade technique not needed here.

The critical constraint: **aspect ratio must be exactly 2:1** (360 degrees longitude : 180 degrees latitude). All target sizes must respect this (8192x4096, 4096x2048, 2048x1024).

### Project Structure and Package Management

`uv` is the recommended tool for managing a Python project within this Rust repository. The pipeline should live in a dedicated subdirectory, fully isolated from Cargo:

```text
sunlit-earth/
├── Cargo.toml
├── src/
├── tools/
│   └── texture-pipeline/
│       ├── pyproject.toml
│       ├── uv.lock            (committed for reproducibility)
│       ├── .venv/             (gitignored, created by uv sync)
│       ├── .python-version    (e.g. "3.12")
│       └── src/
│           └── texture_pipeline/
│               ├── __init__.py
│               └── main.py
```

There is no existing Python or uv setup in the repository. This is a greenfield addition.

The `pyproject.toml` should use hatchling as the build backend and define a `[project.scripts]` entry point so `uv run texture-pipeline` works as a command.

An alternative for very simple scripts is PEP 723 inline dependency metadata (single-file scripts with `# /// script` headers), but a full project layout is preferable once the tool has multiple files or subcommands.

### CLI Design

**Typer** is the recommended CLI framework. It integrates with Python 3.12+ type hints, produces clean `--help` output automatically, and has minimal boilerplate. Built on Click internally.

Proposed CLI interface:

```bash
texture-pipeline convert \
    --input ./tmp/texture-source/ \
    --output ./tmp/texture-output/ \
    --quality 85 \
    --effort 7 \
    --size 4096 --size 2048 --size 1024
```

**Progress reporting**: `rich` is recommended over `tqdm` for its coloured progress bars and structured logging. Both are lightweight. Since JXL encoding is a C-level operation with no Python callbacks, progress should be reported per-file (before/after each encode) rather than as sub-file progress.

### Recommended Dependency Stack

| Concern | Package | Version | Confidence |
|---|---|---|---|
| Image I/O and downscaling | `pillow` | >= 11.0 | High |
| JXL encoding | `pillow-jxl-plugin` | >= 1.3.7 | High |
| CLI framework | `typer` | >= 0.12 | High |
| Progress and logging | `rich` | >= 13.0 | High |
| Large image support (opt.) | `pyvips` | >= 2.2 | High |

Minimal `pyproject.toml` dependencies:

```toml
dependencies = [
    "pillow>=11.0",
    "pillow-jxl-plugin>=1.3.7",
    "typer>=0.12",
    "rich>=13.0",
]

[project.optional-dependencies]
vips = ["pyvips>=2.2"]
```

## External Research

All external findings come from the external research document with source citations.

**JXL library landscape** (High confidence): Verified directly from PyPI pages and GitHub repositories. pillow-jxl-plugin has 178 commits, regular releases, and pre-built wheels for all major platforms.

**Compression benchmarks** (Medium-High confidence): Drawn from jpegxl.info, alexandrehtrb.github.io, openbenchmarking.org, and Cloudinary comparisons. Results vary by image content but photographic textures (this project's use case) are the best-case scenario for JXL.

**pyvips performance** (High confidence): Multiple independent sources confirm 5-10x speed improvement and dramatically lower memory usage for large images. JXL write support via libvips >= 8.14 needs platform verification (Medium confidence for Windows).

**uv project management** (High confidence): Documentation verified directly from docs.astral.sh.

**Pillow resampling** (High confidence): LANCZOS filter behaviour and anti-aliasing support documented in Pillow's own performance benchmarks and documentation.

Sources are listed in full in the external research document.

## Technical Constraints

1. **GPU texture format**: The Rust renderer expects RGBA8 pixel data in `Rgba8Unorm` format. The pipeline's output format (JXL) is a distribution/storage concern; the Rust side will need a JXL decoder added later.
2. **2:1 aspect ratio**: All output resolutions must be exactly 2:1 for correct equirectangular projection.
3. **Coordinate convention**: Output textures should be in standard geographic convention (north-up, prime-meridian-left). The Rust loader handles the flip and shift to match wgpu's UV winding.
4. **No JXL support in `image` crate v0.25**: The Rust side cannot currently load JXL files. Adding JXL decoding is a separate task that must be completed before the pipeline's output can be used by the app.
5. **Windows compatibility**: pillow-jxl-plugin ships Windows wheels. pyvips on Windows requires the libvips DLL bundle, which may not include libjxl for JXL write support.
6. **Memory**: Pillow loads full images into RAM (~1.75 GB for 21600x10800). For very large sources, pyvips's streaming architecture is the mitigation.

## Resolved Questions (Empirically Tested 2026-03-15)

All tests run with Python 3.14.3, Pillow 12.1.1, pillow-jxl-plugin 1.3.7 on Windows 11.

### 1. `effort` parameter — WORKS

pillow-jxl-plugin accepts `effort=1` through `effort=9` without errors. Higher effort yields smaller files at the cost of encode time. At 4096x2048 earth texture, quality=85:

| Effort | Size | Encode time |
|---|---|---|
| 1 | 948 KB | 0.1s |
| 3 | 876 KB | 0.1s |
| 5 | 881 KB | 0.3s |
| 7 | 889 KB | 0.6s |
| 9 | 836 KB | 11.0s |

Effort 7 is a good default (good compression, sub-second encode).

### 2. pyvips JXL on Windows — DOES NOT WORK

pyvips fails to import on Windows without a manual libvips DLL installation (`OSError: cannot load library 'libvips-42.dll'`). Not viable as a dependency for this project. **Decision: Pillow-only, no pyvips dependency.**

### 3. ICC profile handling — WORKS, NORMALIZES PROFILES

JXL normalizes ICC profiles (sRGB 588 bytes → 536 bytes on roundtrip, semantically equivalent). Images without explicit ICC profiles get a default sRGB profile embedded by JXL. This is correct behaviour for our use case — the shader assumes sRGB. No special handling needed.

### 4. JPEG reconstruction gotcha — CRITICAL

When a Pillow Image was loaded from a JPEG file, `pillow-jxl-plugin` defaults to **lossless JPEG transcoding** (a JXL feature that preserves exact JPEG bitstream). This causes `quality` and `effort` parameters to be silently ignored. The pipeline **must** pass `lossless_jpeg=False` when saving to JXL to get actual lossy re-encoding. This is not a concern when the image has been resized (fresh pixel data, no JPEG source), but matters if the source is a JPEG that is saved at the same size.

### 5. Compression results (4096x2048 earth texture, `lossless_jpeg=False`)

| Quality | Size | vs. JPEG (1455 KB) |
|---|---|---|
| 75 | 587 KB | -60% |
| 85 | 889 KB | -39% |
| 95 | 1453 KB | ~same |

Quality 85 delivers substantial savings with near-invisible loss.

### 6. Decided: separate files per resolution, no mipmap pre-computation, coordinate transforms stay in Rust loader.

## User Requirements

- **Default output width**: 8192 (8K), producing 8192x4096 textures
- **Default quality**: 85
- **Python version**: 3.14 (confirmed: Pillow 12.1.1 and pillow-jxl-plugin 1.3.7 have 3.14 wheels)

## Recommendations

1. **Pillow + pillow-jxl-plugin only.** No pyvips. This covers all requirements with the simplest setup and works on all platforms without native library installation.

2. **Use the full `uv` project layout** at `tools/texture-pipeline/`. The tool is likely to grow beyond a single file (multiple subcommands, configuration presets, batch processing), so the project layout is more maintainable than a PEP 723 inline script.

3. **CLI with Typer + Rich.** Typer provides a clean type-annotated interface with automatic help generation. Rich provides coloured progress reporting. Both are lightweight and well-maintained.

4. **Default settings**: quality 85, effort 7, default width 8192, LANCZOS resampling with optional UnsharpMask (radius=1.0, percent=80, threshold=3). These are conservative defaults that produce near-invisible loss with good file size reduction.

5. **Multiple output sizes in a single run.** Accept a list of target widths (e.g. `--width 8192 --width 4096`). Heights are computed automatically to maintain 2:1 aspect ratio.

6. **Always pass `lossless_jpeg=False`** when saving to JXL to avoid the silent JPEG reconstruction behaviour.

7. **Keep the Rust-side JXL decoder as a separate task.** The pipeline can be built and tested independently; its output can be verified by decoding JXL files with any image viewer.

## Sources

| Document | Focus Area |
|---|---|
| `docs/plans/2026-03-15-texture-pipeline-codebase.md` | Current Rust texture system, format constraints, resolution strategy, project structure |
| `docs/plans/2026-03-15-texture-pipeline-external.md` | JXL libraries, downscaling techniques, pyvips, uv project management, CLI frameworks, progress reporting |
