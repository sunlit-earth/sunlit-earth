# texture-pipeline

A CLI tool that turns source imagery into the JPEG XL textures the app ships. It has two commands, because the two kinds of texture have nothing in common but their output format:

- `earth` converts NASA Blue Marble and Black Marble style surface maps to JPEG XL at one or more widths.
- `milky-way` turns a NASA SVS Deep Star Maps 2020 EXR into the denoised sky panorama.

Both take equirectangular sources with an exact 2:1 aspect ratio and refuse to upscale.

## Requirements

- Python 3.14+
- [uv](https://docs.astral.sh/uv/) for dependency management

## Setup

```bash
cd tools/texture-pipeline
uv sync
```

This creates a `.venv/` with all runtime and dev dependencies.

## `earth`

```bash
uv run texture-pipeline earth \
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

`--quality`, `-q` — JPEG XL encoding quality, 1 to 100 (default: 85). Higher values produce better quality at larger file sizes. 100 encodes losslessly.

`--effort`, `-e` — JPEG XL encoding effort, 1 to 9 (default: 7). Higher values produce smaller files but encode more slowly.

`--sharpen` — Apply UnsharpMask sharpening after downscaling. Off by default.

The ocean masking options (`--ocean-mask` and the `--ocean-*` settings beside it) fill the ocean with a flat color from a shapefile, preserving polar ice. Run `uv run texture-pipeline earth --help` for the full list.

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

## `milky-way`

```bash
uv run texture-pipeline milky-way \
    --input milkyway_2020_16k.exr \
    --output milkyway_2020_8k.jxl \
    --width 8192
```

### Options

`--input`, `-i` — The source `.exr` panorama. Any of the published resolutions, 2:1, linear.

`--output`, `-o` — The output `.jxl` file. Parent directories are created.

`--width`, `-w` — Target width in pixels (default: 8192). Must be even and must divide the source width, since the downscale is a box average over whole pixels.

`--quality`, `-q` — JPEG XL quality, 1 to 100 (default: 90). 100 encodes losslessly.

`--effort`, `-e` — JPEG XL encoding effort, 1 to 9 (default: 7).

`--seed` — Seed of the dither drawn during 8-bit quantization (default: 7).

`--k` — Cap a pixel's luminance at this multiple of its local background (default: 3.0).

`--strong` — Replace a star's whole footprint above this multiple of the background (default: 9.0).

`--eps` — Absolute margin on both thresholds, in linear units (default: 0.0005), so a near-black sky is not read as all stars.

`--bg-sigma` — Sigma of the Gaussian that estimates the background, in arcminutes (default: 15.8).

`--dilate` — Radius grown around a strong star to take its rendered wings, in arcminutes (default: 7.9).

`--fill-sigma` — Sigma of the normalized convolution that fills a strong star's hole, in arcminutes (default: 7.9).

`--blur` — Sigma of the final blur, in arcminutes (default: 7.9).

The four lengths are in arcminutes rather than pixels so that one set of values means the same thing on the sky at any output width. At 8192 they come to 6, 3, 3 and 3 pixels, which are the values the defaults were judged at.

### What the command does and why

The SVS layer holds every Gaia DR2 star fainter than magnitude 11.5, each rendered with a point spread function about 7 pixels across at 8k and summed into pixels. What looks like noise is the sky itself at a scale no eye resolves: a grain of magnitude 12 to 16 stars everywhere, and isolated stars just past the Tycho limit peaking at 5 to 20 times their surroundings. A plain blur cannot fix that, because the radius that removes the grain leaves every isolated star as a soft blob. So the command caps what stands above a locally estimated background, replaces the footprints of the strong stars by a normalized convolution from the sky around them, and only then blurs.

Its pixels hold flux rather than radiance, so every published resolution has its own scale: a pixel of the 16k file covers a sixteenth of the sky a 4k pixel does and holds a sixteenth of the light. The renderer's brightness was tuned against the 4k asset, so the command first multiplies by `(source width / 4096)^2`, which is 16 for the 16k file, 4 for the 8k and 1 for the 4k. Every output therefore lands on the same scale whatever the source and target widths are.

The output is dithered before it is quantized to 8 bits. Most of the panorama is a near-black sky that the blur has made very smooth, and rounding that to code values 0 to 3 turns it into visible blotches. A triangular dither of one code value, drawn from `--seed` so a run is reproducible, replaces the blotches with a grain far below what the sky's own texture was.

Lossy at quality 90 by default. The worry that a lossy codec would flatten the dither and bring the banding back was measured on the 8k candidate and does not hold: at quality 90 the mean absolute channel difference against the source is 0.50 of 255 and the maximum is 5, no per-region mean moves by more than 0.03 of a code value, and the north galactic pole tile keeps its count of distinct levels. That is 0.9 MB against the 21.5 MB a lossless encode costs, the dither being most of the entropy.

Only the 8192 output from the 16k source has been judged on renders. Other widths work and are not validated, because the thresholds are relative to a local mean whose grain depends on how many stars a pixel holds.

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

The OpenEXR wheel is a bare extension module carrying no type information, so `stubs/OpenEXR.pyi` declares the handful of names this tool uses and `pyproject.toml` points `ty` at it.

### Memory note

The full-resolution NASA Blue Marble (21600 x 10800) requires roughly 1.75 GB of RAM when loaded into Pillow. The 16k star map is 16384 x 8192 half floats, which is 1.6 GB once it is float32 RGB, and the pipeline holds a second copy of it while it clips and scales. Both are expected for an offline tool.
