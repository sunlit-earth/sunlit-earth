# texture-pipeline

A CLI tool that converts NASA Blue Marble source textures (JPEG, PNG, TIFF) into JPEG XL files at one or more target resolutions. Designed for offline batch processing of equirectangular map projections.

## Requirements

- Python 3.14+
- [uv](https://docs.astral.sh/uv/) for dependency management

## Setup

```bash
cd tools/texture-pipeline
uv sync
```

This creates a `.venv/` with all runtime and dev dependencies.

## Usage

```bash
uv run texture-pipeline convert \
    --input /path/to/source/textures \
    --output /path/to/output \
    --width 8192 --width 4096 --width 2048 \
    --quality 85 \
    --effort 7 \
    --sharpen
```

### Options

`--input`, `-i` — Input directory containing source textures. Searched recursively for JPEG, PNG, and TIFF files.

`--output`, `-o` — Output directory. Created if it does not exist. Each target width gets its own subdirectory, mirroring the input directory structure.

`--width`, `-w` — Target width in pixels (default: 8192). Can be specified multiple times for multi-resolution output. Must be even. Source images must be at least as wide as the target.

`--quality`, `-q` — JPEG XL encoding quality, 1–100 (default: 85). Higher values produce better quality at larger file sizes.

`--effort`, `-e` — JPEG XL encoding effort, 1–9 (default: 7). Higher values produce smaller files but encode more slowly.

`--sharpen` — Apply UnsharpMask sharpening after downscaling. Off by default.

### Input constraints

Source images must have an exact 2:1 width-to-height aspect ratio (equirectangular projection). The tool rejects non-conforming images and refuses to upscale.

### Output structure

Given an input tree:

```
source/
  land/earth.jpg
  ocean/water.png
```

and `--width 4096 --width 2048`, the output is:

```
output/
  4096/
    land/earth.jxl
    ocean/water.jxl
  2048/
    land/earth.jxl
    ocean/water.jxl
```

## Development

Run tests:

```bash
uv run pytest
```

Lint and format:

```bash
uv run ruff check src/ tests/
uv run ruff format --check src/ tests/
```

Type check:

```bash
uv run ty check src/
```

### Memory note

The full-resolution NASA Blue Marble (21600 x 10800) requires roughly 1.75 GB of RAM when loaded into Pillow. This is expected for an offline tool.
