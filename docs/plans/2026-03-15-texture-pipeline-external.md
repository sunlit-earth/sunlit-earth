# Texture Pre-Processing Pipeline: Research Notes

Date: 2026-03-15

This document captures research for building an external Python-based texture
pre-processing pipeline for the Sunlit Earth project. The pipeline's purpose
is to convert large source textures (e.g. NASA Blue Marble at 21600×10800) into
optimised JPEG XL files at multiple resolutions, managed with `uv` inside this
Rust repository.

---

## 1. JPEG XL Python Libraries

### 1.1 Summary of Options

Four candidate libraries provide JXL encoding from Python. They fall into two
tiers: thin Pillow plugins (good for familiar `Image.save()` API) and
lower-level codec libraries (better for numpy pipelines or non-Pillow stacks).

| Library | PyPI name | Latest (as of 2026-03) | Bindings | Maintained? |
|---|---|---|---|---|
| pillow-jxl-plugin | `pillow-jxl-plugin` | 1.3.7 (2025-12-21) | Rust (safe wrapper) | Yes — active |
| jxlpy | `jxlpy` | 0.9.5 | Cython / libjxl | Slow; alpha |
| imagecodecs | `imagecodecs` | 2026.3.6 | Cython / many codecs | Yes — active |
| Pillow native | n/a (not yet merged) | — | C / libjxl | Planned; not released |

#### pillow-jxl-plugin (recommended)

PyPI: `pillow-jxl-plugin`. GitHub: `Isotr0py/pillow-jpegxl-plugin`.

This is the most production-ready option as of early 2026. Key facts:

- Written in Rust with safe Python bindings; no unsafe Cython.
- Ships pre-built wheels for Windows (x86, x64, ARM64), macOS (x64, ARM64),
  manylinux, and musllinux across CPython 3.10–3.14 and PyPy 3.11. No Rust
  toolchain required for end-users.
- Latest release 1.3.7 on 2025-12-21; 178 commits; regular cadence since
  1.0.0 in October 2023.
- Listed as an official third-party plugin in Pillow's own documentation.
- Usage: `import pillow_jxl` to register the plugin, then use standard
  `Image.save()`.

Documented save parameters:

```python
from PIL import Image
import pillow_jxl  # registers .jxl format

img.save("out.jxl", lossless=True)          # lossless
img.save("out.jxl", quality=85)             # lossy, JPEG-style 1–100
img.save("out.jxl", quality=85, effort=7)   # effort 1–9 (higher = smaller + slower)
```

The `quality` parameter maps to libjxl's distance metric internally (higher
quality = lower distance). The `effort` parameter controls the
compression speed/size tradeoff: effort 1 is fastest, effort 9 is smallest.
Default effort in libjxl is 7. For an offline pre-processing pipeline, effort
7–9 is appropriate.

EXIF metadata preservation is supported.

Confidence: **High**. PyPI page and GitHub repository verified directly.

#### imagecodecs

PyPI: `imagecodecs`. GitHub: `cgohlke/imagecodecs`.

This is a comprehensive multi-codec library designed for numpy array pipelines.
It supports JXL encode/decode including JPEG-to-JXL transcoding. Requires
numpy. Heavier install (~30 MB wheel) because it bundles many codecs. Latest
version 2026.3.6 includes Zarr 3 compatibility fixes. Best suited for
scientific/numerical pipelines that already use numpy heavily; for a simple
image-to-file tool, `pillow-jxl-plugin` is simpler.

Confidence: **High** for existence and maintenance. **Medium** for exact JXL
parameter API (not verified in detail).

#### jxlpy

Alpha stage, Cython-based, no numpy dependency. Last meaningful release was
0.9.5. Much less active than pillow-jxl-plugin. Not recommended for new
projects.

Confidence: **High** (negative recommendation).

#### Pillow native JXL support

A long-running issue (Pillow issue #5898) tracks native JXL support. As of
early 2026, this has not been merged into Pillow's main branch. The
`pillow-jxl-plugin` package is the intended stop-gap and will continue to work
even after Pillow gains native support (the plugin is opt-in via import).

Confidence: **Medium** (based on public GitHub issue; may have progressed since
research date).

### 1.2 Compression Options

The libjxl encoder (used by all Python wrappers) exposes two primary axes:

**Lossy vs. lossless:**

- Lossless (`lossless=True`) produces pixel-perfect output, typically 20–35%
  smaller than equivalent PNG for photographic content. Good for archival or
  when the source is already a palette/indexed image.
- Lossy (`quality=N`) uses psychovisual optimisation. For photographic textures
  the quality range 85–95 is the sweet spot for near-invisible loss with
  significant size reduction.

**Quality (lossy):**

libjxl's quality is internally a "distance" value (Butteraugli distance) where
0 = lossless and 15 = very low quality. The `quality` parameter in
pillow-jxl-plugin follows the inverted JPEG convention (1–100) for familiarity.
Rough guide:
- `quality=90` → visually transparent for photographic textures
- `quality=85` → very good; default for many workflows
- `quality=75` → acceptable for previews

**Effort:**

Controls encoder search depth. Range 1–9, default 7.
- `effort=3` → fast encode, suitable for development/testing
- `effort=7` → good default for production
- `effort=9` → maximum compression, 3–5× slower than effort 7

### 1.3 File Size Savings

Benchmark data from multiple 2024–2025 sources:

- JXL vs. JPEG: **20–55% smaller** at equivalent perceptual quality.
  Average ~20% when re-encoding existing JPEGs at quality=90; up to 34%
  observed in a high-resolution photographic test (11 MB JPEG → 7.3 MB JXL).
- JXL vs. PNG (lossless): JXL is approximately **46% smaller** than lossless PNG.
- JXL vs. AVIF: JXL is approximately **25% smaller** than AVIF at equivalent
  quality for static photographic content.

For the Blue Marble 21600×10800 source (typically ~300 MB TIFF or ~80–120 MB
JPEG), a 4096×2048 JXL derivative at quality=85 should land in the 3–6 MB range
based on these ratios, compared to ~6–10 MB for JPEG and ~15–25 MB for PNG.

Sources: jpegxl.info, alexandrehtrb.github.io/posts/2024/01/modern-image-formats-jxl-and-avif/,
openbenchmarking.org JXL benchmarks, Cloudinary 2024 format comparison.

Confidence: **Medium-High**. Benchmarks are from multiple independent sources
but vary by image content; photographic textures are the best-case scenario
for JXL.

---

## 2. High-Quality Image Downscaling

### 2.1 Pillow Resampling Filters

Pillow exposes these filters via `Image.Resampling.*`:

| Filter | Window | Quality | Speed | Notes |
|---|---|---|---|---|
| `NEAREST` | 1px | Low | Fastest | Pixel art only |
| `BILINEAR` | 2px | Moderate | Fast | Blurry for large downscales |
| `HAMMING` | 2px | Moderate | Fast | Like bilinear but less aliasing |
| `BOX` | variable | Good | Moderate | Uniform averaging; good for integer downscales |
| `BICUBIC` | 4px | High | Moderate | Good general purpose |
| `LANCZOS` | 8px | Highest | Slowest | Best for photographic downscaling |

For downscaling photographic textures, **`LANCZOS` is the standard
recommendation** and is what most image editors use for "high quality resize".
It has the largest kernel window and best preserves fine detail while
suppressing aliasing.

Since Pillow 2.7.0 (2014), the filters correctly implement anti-aliasing for
downscaling automatically. There is no longer a need for multi-step downscaling
tricks.

The `reducing_gap` parameter (default 2.0) controls how Pillow boxes the image
before applying the main filter, providing a speed–quality tradeoff without
noticeably reducing output quality.

### 2.2 Post-Downscale Sharpening

Downscaling with any filter introduces some softness. Pillow's
`ImageFilter.UnsharpMask` is the standard correction:

```python
from PIL import ImageFilter

img_down = img.resize(target_size, Image.Resampling.LANCZOS)
img_sharp = img_down.filter(ImageFilter.UnsharpMask(radius=1.0, percent=80, threshold=3))
```

Typical parameters for photographic textures after 2–4× downscale:
- `radius=0.5–1.5` (blur radius of the mask)
- `percent=50–100` (strength; 80 is a conservative default)
- `threshold=2–4` (minimum difference to sharpen; prevents sharpening noise)

Do not over-sharpen: textures viewed on a sphere at varying distances benefit
from slight softness at lower mip levels.

### 2.3 pyvips as an Alternative

pyvips (Python bindings to libvips) is the best alternative to Pillow for
large image downscaling. Key advantages:

- **Memory**: Demand-driven architecture loads images in tiles rather than
  fully into RAM. Processing a 21600×10800 TIFF with Pillow requires ~1.75 GB
  of RAM; pyvips requires ~400 MB for the same task.
- **Speed**: Typically 5× faster than Pillow-SIMD for large images; up to 10×
  faster in real-world benchmarks (watermarking/resize pipelines).
- **Quality**: pyvips defaults to `lanczos3` for its final reduce step. It
  also supports `nohalo` and `lbb` (locally bounded bicubic) kernels, and as
  of November 2024, a new "Magic Kernel Sharp" implementation.

pyvips downscale approach:

```python
import pyvips

img = pyvips.Image.new_from_file("source.tiff", access="sequential")
# thumbnail_image handles the multi-step shrink correctly
thumb = img.thumbnail_image(4096, height=2048, size=pyvips.Size.FORCE)
thumb.write_to_file("out.jxl", Q=85)  # pyvips can write JXL natively
```

The `sequential` access mode enables the low-memory streaming path.

**When to prefer pyvips over Pillow:**
- Source image is larger than 8192×4096 (RAM becomes a concern)
- Processing many images in a batch where memory overhead accumulates
- Need faster throughput

**When to prefer Pillow:**
- Simpler code / fewer dependencies (pyvips requires libvips native library)
- Windows users may find pyvips installation more complex (requires libvips DLL)
- For 4096×2048 output from a moderate source, Pillow is perfectly adequate

**Recommendation for this project:** Start with Pillow for simplicity. If the
source is the full 21600×10800 Blue Marble TIFF, consider switching to pyvips
to avoid 1.75 GB RAM spikes.

Confidence: **High** for performance claims (multiple independent sources).
**Medium** for pyvips JXL write support (pyvips can write JXL via libvips ≥8.14
which bundles libjxl; verify on target platform).

### 2.4 scikit-image

scikit-image's `transform.resize()` uses area-averaging by default and is
accurate but slow. It is primarily a scientific image processing library, not
optimised for batch throughput. For this use case, it offers no advantage over
Pillow and is not recommended.

### 2.5 Equirectangular Texture Considerations

Equirectangular (lat/lon) textures have a key geometric property: pixel density
increases toward the poles. A pixel at latitude 80° represents a much smaller
area on the sphere than a pixel at the equator.

Implications for downscaling:

1. **Standard filters are acceptable for preview/rendering use.** A uniform
   resampling filter like LANCZOS does not know about the projection but it also
   does not introduce visible artefacts in rendered output. The distortion is
   in the source projection, not introduced by downscaling.

2. **Pole regions may appear over-sharpened** after a Lanczos downscale because
   they contain more redundant data (many pixels map to the same sphere point).
   If this is visually distracting, a slightly heavier unsharp mask suppression
   at the poles can help, but this requires custom processing beyond standard
   libraries.

3. **Aspect ratio must be preserved.** The canonical equirectangular aspect
   ratio is 2:1 (longitude 360° : latitude 180°). Always resize to exact 2:1
   sizes (e.g. 8192×4096, 4096×2048, 2048×1024) to avoid introducing
   non-uniform stretching.

4. **NASA Blue Marble coordinate convention.** The Blue Marble textures have
   latitude running top-to-bottom with north at top, and longitude starting at
   the prime meridian. The existing `earth_texture.rs` already handles the flip
   and shift required to match wgpu's UV convention. The pre-processing pipeline
   should output textures in standard geographic convention (north-up,
   prime-meridian-left) and let the shader handle the rest.

For the purposes of this pipeline (offline pre-processing to produce
power-of-two textures for GPU use), standard LANCZOS downscaling to 2:1 target
sizes is sufficient. Latitude-aware resampling is a research-grade technique
used in 360° video super-resolution and is not necessary here.

Confidence: **High** for geometric facts. **Medium** for the "standard
LANCZOS is sufficient" claim (based on practical experience in geospatial
rendering, not a formal study).

---

## 3. UV Project Management

### 3.1 Overview

`uv` (astral-sh/uv) is a Rust-based Python package and project manager that
replaces pip, virtualenv, and pip-tools with a single fast tool. It is the
recommended way to manage a Python tool within a non-Python repository as of
2025–2026.

Key commands:

| Command | Purpose |
|---|---|
| `uv init --app tools/texture-pipeline` | Create a new app project |
| `uv add pillow pillow-jxl-plugin` | Add dependencies to pyproject.toml |
| `uv sync` | Create/update .venv from lock file |
| `uv run python script.py` | Run script in project env |
| `uv run texture-pipeline --help` | Run CLI entry point |
| `uv lock` | Regenerate uv.lock |

### 3.2 Recommended Layout for a Tool in a Rust Repo

Place the Python tool in a subdirectory, completely isolated from the Rust
project root. uv will manage its own `.venv` and `uv.lock` inside that
subdirectory. Cargo and uv do not interfere with each other.

```
sunlit-earth/                  ← Rust project root (Cargo.toml)
├── Cargo.toml
├── src/
├── tools/
│   └── texture-pipeline/      ← Python tool root
│       ├── pyproject.toml
│       ├── uv.lock
│       ├── .venv/             ← gitignored; created by uv sync
│       ├── .python-version    ← e.g. "3.12"
│       └── src/
│           └── texture_pipeline/
│               ├── __init__.py
│               └── main.py
```

Add `.venv/` to `.gitignore` (uv does this automatically) and commit `uv.lock`
for reproducible builds.

### 3.3 pyproject.toml Structure

Minimal `pyproject.toml` for a CLI app:

```toml
[project]
name = "texture-pipeline"
version = "0.1.0"
description = "Texture pre-processing pipeline for Sunlit Earth"
requires-python = ">=3.12"
dependencies = [
    "pillow>=11.0",
    "pillow-jxl-plugin>=1.3.7",
]

[project.scripts]
texture-pipeline = "texture_pipeline.main:main"

[build-system]
requires = ["hatchling"]
build-backend = "hatchling.build"

[tool.hatch.build.targets.wheel]
packages = ["src/texture_pipeline"]
```

The `[project.scripts]` entry creates a console script entry point. After
`uv sync`, the command `texture-pipeline` is available inside the `.venv`.

For development installs with optional dependencies (e.g. pyvips):

```toml
[project.optional-dependencies]
vips = ["pyvips>=2.2"]
```

Install with: `uv sync --extra vips`

### 3.4 Running the Tool

From the `tools/texture-pipeline/` directory:

```bash
# First-time setup
uv sync

# Run the CLI
uv run texture-pipeline --input source.tif --output out/

# Run with a specific Python version
uv run --python 3.12 texture-pipeline --input source.tif

# Run without installing (one-shot, slower)
uv run --with pillow --with pillow-jxl-plugin python -c "..."
```

From the Rust project root (using `--directory`):

```bash
uv --directory tools/texture-pipeline run texture-pipeline --input source.tif
```

Or add a `Makefile` or shell script wrapper at the repo root:

```bash
#!/usr/bin/env bash
# tools/run-texture-pipeline.sh
cd "$(dirname "$0")/tools/texture-pipeline"
exec uv run texture-pipeline "$@"
```

### 3.5 Inline Script Alternative (PEP 723)

For very simple single-file scripts, uv supports PEP 723 inline dependency
metadata, which avoids needing a full project structure:

```python
#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = [
#   "pillow>=11.0",
#   "pillow-jxl-plugin>=1.3.7",
# ]
# ///

from PIL import Image
import pillow_jxl
# ...
```

Run with: `uv run convert_texture.py --input source.tif`

uv downloads and caches dependencies on first run. When inline script metadata
is present, uv ignores any surrounding project's `pyproject.toml`, so this
works cleanly from anywhere in the Rust repo tree.

For a pipeline with multiple files and subcommands, a full project layout
(section 3.2) is preferable. For a single-purpose script under ~200 lines, the
inline approach is simpler.

Confidence: **High**. Documentation verified directly from docs.astral.sh.

---

## 4. CLI Design

### 4.1 Framework Comparison

| Framework | Source | Boilerplate | Type safety | Best for |
|---|---|---|---|---|
| `argparse` | stdlib | High | Manual | Simple scripts; no extra deps |
| `click` | PyPI | Medium | Decorator-based | Feature-rich CLIs; ~38% market share |
| `typer` | PyPI | Low | Python type hints | Type-annotated codebases |

For this pipeline:

- **Typer is recommended.** It integrates naturally with Python 3.12+ type
  hints, produces clean `--help` output automatically, and has minimal
  boilerplate. The pipeline is a relatively simple CLI (one or two subcommands),
  which is Typer's sweet spot.
- `click` is a fine alternative if Typer's magic annotation style feels
  opaque. Click and Typer are internally related (Typer is built on Click).
- `argparse` is acceptable if avoiding a third-party dependency is a hard
  requirement, but it requires significantly more code for the same features.

Typer example for an image processing CLI:

```python
import typer
from pathlib import Path

app = typer.Typer()

@app.command()
def convert(
    input: Path = typer.Argument(..., help="Input image or directory"),
    output: Path = typer.Argument(..., help="Output directory"),
    quality: int = typer.Option(85, "--quality", "-q", help="JXL quality (1-100)"),
    effort: int = typer.Option(7, "--effort", "-e", help="JXL effort (1-9)"),
    sizes: list[int] = typer.Option([4096, 2048, 1024], "--size", "-s",
                                     help="Output widths to generate"),
    lossless: bool = typer.Option(False, help="Use lossless JXL encoding"),
):
    ...

if __name__ == "__main__":
    app()
```

Confidence: **High**.

### 4.2 Progress Reporting

Two strong options:

**tqdm** — The standard choice for loops with a known count:

```python
from tqdm import tqdm

for img_path in tqdm(input_paths, desc="Converting", unit="image"):
    process(img_path)
```

Lightweight, battle-tested, integrates with Jupyter, logs-friendly.

**rich** — Better visual output (coloured progress bars, multiple tasks):

```python
from rich.progress import Progress, SpinnerColumn, BarColumn, TextColumn

with Progress(SpinnerColumn(), TextColumn("[bold]{task.description}"),
              BarColumn(), TextColumn("{task.completed}/{task.total}")) as progress:
    task = progress.add_task("Converting textures", total=len(input_paths))
    for img_path in input_paths:
        process(img_path)
        progress.advance(task)
```

Rich also provides `rich.progress.track()` as a simpler drop-in for `tqdm`:

```python
from rich.progress import track

for img_path in track(input_paths, description="Converting..."):
    process(img_path)
```

**Recommendation:** Use `rich` if you also want coloured logging and structured
output. Use `tqdm` for pure progress bars with minimal footprint. Both are
lightweight (tqdm ~600 KB, rich ~2 MB) and worth adding to the pipeline's
dependencies.

For updates during a single long-running image encode (where the inner loop
is C code with no Python callbacks), emit a log message before and after each
file rather than trying to show sub-file progress.

Confidence: **High**.

---

## 5. Recommended Stack Summary

| Concern | Recommendation | Confidence |
|---|---|---|
| JXL encoding | `pillow-jxl-plugin` 1.3.7+ | High |
| Image I/O | `Pillow` 11.x | High |
| Large image downscale | Pillow LANCZOS for ≤8K source; pyvips for larger | High |
| Post-downscale sharpening | `ImageFilter.UnsharpMask(radius=1.0, percent=80, threshold=3)` | Medium |
| Package management | `uv` in `tools/texture-pipeline/` subdirectory | High |
| CLI framework | `typer` | High |
| Progress reporting | `rich` | High |

### Minimal dependency list for pyproject.toml:

```toml
dependencies = [
    "pillow>=11.0",
    "pillow-jxl-plugin>=1.3.7",
    "typer>=0.12",
    "rich>=13.0",
]
```

Optional:

```toml
[project.optional-dependencies]
vips = ["pyvips>=2.2"]
```

---

## 6. Open Questions

- Does `pillow-jxl-plugin` expose the `effort` parameter via `Image.save()`?
  The PyPI page only documents `quality` and `lossless`. The underlying libjxl
  API has effort 1–9; it may be accessible as an undocumented kwarg or require
  a version >1.3.7. **Action:** test `img.save("out.jxl", quality=85, effort=7)`
  and confirm no TypeError.

- pyvips on Windows: libvips ships as a DLL bundle via the `pyvips` wheel on
  Windows since pyvips 2.2+, but verify that JXL write (`write_to_file(...jxl)`)
  works without a separately installed libjxl. The vips DLL bundle may not
  include libjxl.

- What is the correct input colour space for the pipeline? NASA Blue Marble
  TIFF files are in sRGB. JXL preserves colour profiles. Confirm that
  `pillow-jxl-plugin` passes through the ICC profile on save, or strip it to
  avoid colour shift at load time in the shader.

- Should the pipeline output a single JXL per resolution, or use JXL's
  progressive encoding / embedded thumbnails? For GPU texture use, separate
  files per resolution are simpler and map more directly to the existing
  `earth_texture.rs` loader.

---

## Sources

- [pillow-jxl-plugin on PyPI](https://pypi.org/project/pillow-jxl-plugin/)
- [Isotr0py/pillow-jpegxl-plugin on GitHub](https://github.com/Isotr0py/pillow-jpegxl-plugin)
- [jxlpy on PyPI](https://pypi.org/project/jxlpy/)
- [imagecodecs on PyPI](https://pypi.org/project/imagecodecs/)
- [JPEG XL issue in Pillow #5898](https://github.com/python-pillow/Pillow/issues/5898)
- [jpegxl.info — file size comparisons and encoder settings](https://jpegxl.info/index.html)
- [Modern image formats: JXL and AVIF (alexandrehtrb, 2024)](https://alexandrehtrb.github.io/posts/2024/01/modern-image-formats-jxl-and-avif/)
- [Pillow Performance benchmarks](https://python-pillow.github.io/pillow-perf/)
- [Performance comparison: Pillow vs PyVIPS (gist)](https://gist.github.com/amw/2febf24ebcb3baf409c50decbea71e6e)
- [Optimising satellite image processing with pyvips (DEV Community)](https://dev.to/aykhara/optimizing-satellite-image-processing-with-pyvips-4n3f)
- [uv running scripts documentation](https://docs.astral.sh/uv/guides/scripts/)
- [uv workspaces documentation](https://docs.astral.sh/uv/concepts/projects/workspaces/)
- [uv project layout documentation](https://docs.astral.sh/uv/concepts/projects/layout/)
- [uv creating projects documentation](https://docs.astral.sh/uv/concepts/projects/init/)
- [Python CLI tools comparison (dasroot.net, 2025)](https://dasroot.net/posts/2025/12/building-cli-tools-python-click-typer-argparse/)
- [Rich progress display documentation](https://rich.readthedocs.io/en/latest/progress.html)
- [libvips HOWTO image shrinking](https://github.com/libvips/libvips/wiki/HOWTO----Image-shrinking)
- [Equirectangular projection — PanoTools wiki](https://wiki.panotools.org/Equirectangular_Projection)
