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
    --sea-ice /path/to/osi450 --fetch \
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

`--sea-ice`: Directory of the 24 OSI-450-a1 monthly files of 2004, both hemispheres, as the OSI SAF THREDDS server names them (`ice_conc_nh_ease2-250_cdr-v3p1_200403.nc`). With it, every month's sea ice is generated and composited into the month's open water. Without it the faces have no sea ice.

`--fetch`: Download the files missing from the `--sea-ice` directory first, one at a time over HTTPS, and write their checksums to `SHA256SUMS` beside them. About 22 MB.

`--sea-ice-cap-size`: Texels along each side of the polar caps the ice is generated on (default: 4096, 2.87 km a texel). 8192 is no sharper at 2048 px faces and needs over 6 GiB.

`--imagery-water/--no-imagery-water`: Whether the day maps' own water joins the mask poleward of 50 degrees (default: it does). A source pixel short of the shapefile's open ocean becomes ocean when the maps draw it as water in all twelve months, its brightest luminance over the year at most 14 and its blue at least 7 above its red in every month, and it is connected to the ocean through such pixels, so lakes stay land. The thresholds were measured over the Blue Marble's open ocean (`ocean_masking.IMAGERY_WATER`). Natural Earth's coastline and the Blue Marble's disagree along the Arctic coasts, North Greenland's most, and without the join the maps' dark water on the shapefile's land showed as strokes beside the ice.

`--ocean-ice-south-only/--ocean-ice-both-poles`: Where the ice detection looks (default: south only). South of the latitude gate it keeps the Antarctic ice shelves as land; north of it the generated sea ice takes the place of the Blue Marble's own coastal ice and snow. `--ocean-ice-both-poles` is the detection of earlier bakes.

The other `--ocean-*` options are those of `earth`. The one difference is that ice the detector finds in any month is kept as land in every month, because a single mask serves the whole year.

### Output

Six faces per set, named `px`, `nx`, `py`, `ny`, `pz` and `nz` for +X, -X, +Y, -Y, +Z and -Z, which is also the layer order of a cube texture. The frame is the app's world frame: +Y is north, +Z is longitude 0 and +X is longitude 90 E, and the faces are laid out by the OpenGL and Direct3D cube map table, so Africa sits upright on `pz` and the Arctic Ocean is centered on `py`. The texel grid is equi-angular: a face texel at row `r` and column `c` of `n` lies at the warped coordinates `t = (r + 0.5) / n * 2 - 1` and `s = (c + 0.5) / n * 2 - 1`, and `tan(s * pi / 4)` and `tan(t * pi / 4)` are the coordinates on the plain cube face that the table combines into a direction. `src/texture_pipeline/cube.py` holds the table.

The day faces are 8-bit RGB with no alpha, the night faces 8-bit RGB, and the mask faces single-channel 8-bit and lossless.

### The water mask

A mask texel is the fraction of its footprint that is open water: 255 is open ocean, 0 is land, and values in between are coastline. Poleward of 50 degrees the open ocean includes the day maps' own water connected to it (`--imagery-water`). Ice that the detector finds on the ocean side of the shapefile's coastline, which on Blue Marble south of the gate is the Antarctic ice shelves, counts as land and is 0. With `--sea-ice` a texel the mask calls open water is the fill or sea ice, and nothing else. The day faces are flattened toward the ocean fill color (default 10, 30, 60) in proportion to the same mask, so wherever a mask texel is 255 the day texel is the fill to within one level before the lossy encode moves it; a consumer that wants a flat ocean should write the fill itself where the mask is 255 rather than trust the decoded color.

### What the command does

1. Rasterizes the shapefile once at the day maps' size, with the `earth` command's supersampling and coast offset.
2. Unless `--no-imagery-water` is given, reads the twelve months for each pixel's brightest luminance and least blue over the year, and joins the maps' own water poleward of 50 degrees to the ocean.
3. Runs the ice detection on every month, south of the gate unless `--ocean-ice-both-poles` is given, and subtracts the union of what it finds from the mask.
4. Resamples the mask to six faces at the working size, a quarter of the source's width (5400 for the 21600 maps), which is the source's own density at the equator. These working faces flatten every month, and their area average down to the face size is what `mask/` holds.
5. With `--sea-ice`, reads the twelve monthly means of each hemisphere, raises the concentration along coasts where the ice 50 to 100 km out is closed (the fast ice band), extends the means three cells into land from the nearest ocean cell and resamples them onto a polar cap of 1024 texels (the polar Lambert azimuthal equal-area plane out to 35 degrees of latitude on its axes) through a cubic B-spline. It reads the land on a cap of `--sea-ice-cap-size` from the working mask faces and generates the hemisphere's noise fields there (`src/texture_pipeline/sea_ice_look.py`), seeded and the same in every month: the closed pack's ridged relief with its region fields, creases and drift, the fracture systems drawn as thin jagged leads, the cascade's cells and the floes' noise, the coast, and a divergence-free swirl the concentration is read through.
6. For each month, makes both caps' coverage and brightness: the floes read the concentration through a tone curve that closes the pack from 85%, their water is a cascade of whole cells around a regional target with a calm speckle from 40% to 70%, and the sea within 10 km of the land is ice where the concentration is half or more. It resamples the picture to six faces at the working size and flattens the ocean through the working mask; on `py` and `ny` it blends the coast's flat snowy land toward the ice within 6 to 18 km of the water, and the maps' dark water left on the mask's land toward what the sea beside it shows; it composites the month's ice into the water share of every texel (a texel `(1 - w) * land + w * fill` becomes `(1 - w) * land + w * mix(fill, ice, coverage)`), averages each face down to the face size over every texel's footprint (Pillow's box filter, which weighs partial pixels exactly at a non-integer ratio), and encodes it. With `--sea-ice` it prints each hemisphere's ice area over the mask's water against the source's concentration, and against the source's concentration read through the tone curve, which is what the floes are asked to draw.
7. Resamples the night map the same way at a quarter of its own width, without flattening.

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

## `preview`

A harness for the look of the sea ice. A variant is a set of options of the bake and tunables of the generator in `sea_ice_look.py` that differ from the production bake's; the harness rebakes the faces four views read, renders the views through the app, measures them against the baseline and writes a page to compare them. Its inputs are named in `tmp/sea-ice-look/sources.toml` (the twelve Blue Marble months, the shapefile, the 24 OSI-450-a1 files, the shipped textures and a release build of the app), and its caches and outputs are under that directory, which is not tracked.

```bash
uv run texture-pipeline preview run baseline      # bake, render and measure a variant
uv run texture-pipeline preview draft baseline --place svalbard   # a region in seconds
uv run texture-pipeline preview page              # rebuild the page
```

`preview run` writes `tmp/sea-ice-look/<variant>/` and rebuilds `index.html`. `--north-winter`, `--north-summer`, `--south-winter` and `--south-summer` choose the months of the four views: March and August in the north, September and February in the south.

`baseline` is the production bake: its faces and mask faces come out byte for byte as `cube --sea-ice` writes them, and its metrics report that against the shipped textures. A new variant is a `Variant` in `sea_ice_preview.py`, derived from `BASELINE` or from another variant with the tunables it changes, and added to `VARIANTS`; its notes compare it with the baseline and with its parent. The metrics, per view: (a) the dark texels of the mask's land, split into the coast's and the lakes'; (b) the coastal snow against the ice; (c) the ice area on the faces against the record and against the record read through the tone curve; (d) the ice interior's fine detail after the encode; (e) how the coastline shows under closed ice at flat and at mountain coasts; and the open water along closed coasts, the floes' water in the pack and the leads' share of it.

### Caches

The noise fields are kept per hemisphere and per part: the surface, the cascade's cells, the floes' noise, the coast, the activity, the fractures and the eddies (`sea_ice_look.PARTS`). A part's key is made of the cap size, the values of the tunables the part's code reads, a digest of that code, and the keys of the parts, the record and the land on the cap it reads. Changing a tunable remakes the parts that read it and the ones that read those, nothing else: a lead tunable remakes the fractures (about 5 s for both hemispheres), a cell tunable the cascade (about 16 s), a speckle tunable the floes' noise (about 8 s), the coast rule's distance the coast (about 3 s), a surface tunable the surface (about 30 s), and a tunable only the month reads, the tone curve, the cascade's schedule or the colors, none. Parts made side by side take longer each than alone, and less together. Editing the code of a part, or of a function it calls, remakes that part alone; `tests/test_sea_ice_perf.py` pins which tunables reach which part.

The other cached pieces are keyed the same way: the concentration read from the record with the fast ice band, the mask with the joined water, the land on the caps, the weight the coast blend gives the land (by a hash of the face's pixels, the mask and the tunables), the lossless mask faces, and the snow colors of a variant that measures them again. `rm -r tmp/sea-ice-look/cache/fields-*` clears the fields and nothing else.

### Threads

Bands of rows, the independent parts of the fields, the two hemispheres, the four views, the four renders and the measurements run on threads. None of it changes a byte of the output: every part draws from a random stream of its own, and each band computes what it would compute alone. `--workers N`, the `TEXTURE_PIPELINE_WORKERS` environment variable, or `texture_pipeline.parallel.configure(N)` sets the thread count (default: the logical cores up to 16), and `1` runs everything in turn. Peak memory is about 6.5 GiB of working set and 7.7 GiB of commit with everything at once, measured on `baseline`; a smaller machine should take a lower count. The renders run at once, each on a tile cache of its own; `--warm-check` renders in turn on one tile cache and once more warm, which is how `pack_build_s` and `warm_render_s` are measured.

### The draft

`preview draft` draws the square around a named place of one view's face at the app's working size (5400 texels per face edge), with the variant's coast blend and composite and nothing else: no encode, no app render, no metrics, no page. `--half` is half the square's side in texels (default 512, so 1024 by 1024, about 390 texels of the 2048 face); `--view` names the face and month (default: the winter view of the place's hemisphere); `--out` sets the PNG's path, which defaults to `tmp/sea-ice-look/<variant>/draft-<view>-<place>.png`. The places are those of the detail crops: `north-greenland`, `ellesmere-north`, `svalbard`, `franz-josef-land`, `canadian-arctic-west`, `east-greenland`, `beaufort-sea`, `antarctic-peninsula`, `ross-sea-coast`, `east-antarctica`, `ronne-ice-shelf-front`, `ross-ice-shelf-front`, `amery-ice-shelf`, `victoria-land` and `weddell-sea`.

With the fields cached the draft takes 4.7 to 6.0 s of work, about 4 s of it the month's coverage, which the floes make over the whole cap, and the rest the brightness for the cap rows the square reads, the coast weight of the square with a margin, the blend and the composite; 6 to 8 s with Python's start. After an edit to a tunable it makes the parts that changed first. The square is the same texels the full bake writes there, which `tests/test_sea_ice_perf.py` checks on a generated face and a run on the real inputs did for seven places of both hemispheres. It shows the bake's face before the area average to 2048 and the JPEG XL encode, so it is sharper than the face in the textures.

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

The full-resolution NASA Blue Marble (21600 x 10800) requires roughly 1.75 GB of RAM when loaded into Pillow. The 16k star map is 16384 x 8192 half floats, which is 1.6 GB once it is float32 RGB, and the pipeline holds a second copy of it while it clips and scales. Both are expected for an offline tool. The `cube` command reads the twelve 21600 maps one at a time and resamples in bands of 256 rows. With `--sea-ice` its peak is a month's caps and their intermediates beside the working faces: a working set of 6.0 GiB and a commit of 6.9 GiB for the full bake of the app's textures, which took about thirteen minutes on 16 logical cores (`textures/PROVENANCE.md` has the stages).
