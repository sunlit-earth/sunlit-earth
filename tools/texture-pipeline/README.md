# texture-pipeline

A CLI tool that turns source imagery into the JPEG XL textures the app ships. It has three commands:

- `earth` converts NASA Blue Marble and Black Marble style surface maps to JPEG XL at one or more widths.
- `cube` bakes the twelve monthly Blue Marble maps, the Black Marble night map and an ocean shapefile into the equi-angular cube faces the globe samples.
- `milky-way` turns a NASA SVS Deep Star Maps 2020 EXR into the denoised sky panorama.

All three take equirectangular sources with an exact 2:1 aspect ratio and refuse to upscale.

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

## `cube`

```bash
uv run texture-pipeline cube \
    --input /path/to/bmng-topography \
    --night /path/to/BlackMarble_2016_3km.jpg \
    --ocean-mask /path/to/ne_10m_ocean.shp \
    --output ../../textures
```

### Options

`--input`, `-i`: Directory of monthly day maps, one per month, each named with a `.YYYYMM.` stamp the way NASA names them (`world.topo.200405.3x21600x10800.jpg`). Not searched recursively. All of them must be the same size, since one water mask is rasterized for all of them.

`--night`: The equirectangular night map.

`--ocean-mask`: The ocean shapefile (`.shp`), as for `earth`. Required, because the water mask is part of the output.

`--output`, `-o`: Directory that receives `day/YYYYMM/<face>.jxl`, `night/<face>.jxl` and `mask/<face>.jxl`. For the app's own assets that is the repository's `textures/`.

`--face-size`: Edge length of every face in pixels (default: 2048). Each source must be at least four times as wide.

`--quality`, `-q`: JPEG XL quality of the day and night faces, 1 to 100 (default: 85). The mask faces are always lossless.

`--effort`, `-e`: JPEG XL encoding effort, 1 to 9 (default: 7).

`--workers`: Threads that resample the bands of a face (default: the CPU count, at most 4).

The `--ocean-*` options are those of `earth`. The one difference is that ice the detector finds in any month is kept as land in every month, because a single mask serves the whole year.

### Output

Six faces per set, named `px`, `nx`, `py`, `ny`, `pz` and `nz` for +X, -X, +Y, -Y, +Z and -Z, which is also the layer order of a cube texture. The frame is the app's world frame: +Y is north, +Z is longitude 0 and +X is longitude 90 E, and the faces are laid out by the OpenGL and Direct3D cube map table, so Africa sits upright on `pz` and the Arctic Ocean is centered on `py`. The texel grid is equi-angular: a face texel at row `r` and column `c` of `n` lies at the warped coordinates `t = (r + 0.5) / n * 2 - 1` and `s = (c + 0.5) / n * 2 - 1`, and `tan(s * pi / 4)` and `tan(t * pi / 4)` are the coordinates on the plain cube face that the table combines into a direction. `src/texture_pipeline/cube.py` holds the table.

The day faces are 8-bit RGB with no alpha, the night faces 8-bit RGB, and the mask faces single-channel 8-bit and lossless.

### The water mask

A mask texel is the fraction of its footprint that is open water: 255 is open ocean, 0 is land, and values in between are coastline. Ice that the detector finds on the ocean side of the shapefile's coastline, which on Blue Marble is the Antarctic ice shelves and snow along Arctic coasts, counts as land and is 0. The day faces are flattened toward the ocean fill color (default 10, 30, 60) in proportion to the same mask, so wherever a mask texel is 255 the day texel is the fill to within one level before the lossy encode moves it; a consumer that wants a flat ocean should write the fill itself where the mask is 255 rather than trust the decoded color.

### What the command does

1. Rasterizes the shapefile once at the day maps' size, with the `earth` command's supersampling and coast offset.
2. Runs the ice detection on every month and subtracts the union of what it finds from the mask.
3. Resamples the mask to six faces at the working size, a quarter of the source's width (5400 for the 21600 maps), which is the source's own density at the equator. These working faces flatten every month, and their area average down to the face size is what `mask/` holds.
4. For each month, resamples the picture to six faces at the working size, flattens the ocean through the working mask, averages each face down to the face size over every texel's footprint (Pillow's box filter, which weighs partial pixels exactly at a non-integer ratio), and encodes it.
5. Resamples the night map the same way at a quarter of its own width, without flattening.

The resampling maps every texel center back into the source and samples it bilinearly, wrapping in longitude and clamping at the poles, one band of 256 rows at a time, so the memory a face needs does not grow with its size.

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

`tests/test_cube.py` bakes a tiny set of generated sources through the `cube` command, losslessly so the comparison can be exact, and compares every decoded face with the committed bake in `tests/fixtures/cube/`. A change to the bake that is meant fails it once; regenerate the fixture with `TEXTURE_PIPELINE_UPDATE_FIXTURES=1 uv run pytest tests/test_cube.py` and commit the result. CI runs this suite in the `pipeline` job of `.github/workflows/ci.yml`, on Ubuntu, whenever that workflow is dispatched for all runners or for `ubuntu-latest`.

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

The full-resolution NASA Blue Marble (21600 x 10800) requires roughly 1.75 GB of RAM when loaded into Pillow. The 16k star map is 16384 x 8192 half floats, which is 1.6 GB once it is float32 RGB, and the pipeline holds a second copy of it while it clips and scales. Both are expected for an offline tool. The `cube` command reads the twelve 21600 maps one at a time and resamples in bands of 256 rows, so its peak is the ocean raster at twice the source size: 3.0 GiB working set for the full bake of the app's textures, which took about ten minutes on 16 logical cores (`textures/PROVENANCE.md` has the stages).
