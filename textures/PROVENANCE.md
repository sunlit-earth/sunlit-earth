# Where these textures came from

Everything in this directory is Git LFS (`textures/**` in `.gitattributes`), so
a checkout without the objects holds pointer files rather than images. This file
is the exception, and says so in the same place.

## lroc_color_poles_1k.jxl

The Moon's surface, 1024x512.

- Source: NASA Scientific Visualization Studio, [CGI Moon Kit](https://svs.gsfc.nasa.gov/4720)
  (SVS ID 4720, released 2019-09-06). Visualizer Ernie Wright (USRA), scientist
  Noah Petro (NASA/GSFC).
- File taken: `lroc_color_poles_2k.tif`, the 2019 color map at 2048x1024,
  sha256 `13b797422e8c4b8607ff2b2623ac3a046a6da0132d567c2d272d92fad7052c4a`,
  downloaded 2026-08-26 from
  `https://svs.gsfc.nasa.gov/vis/a000000/a004700/a004720/lroc_color_poles_2k.tif`.
- Underlying data: the LROC WAC natural-color Hapke-normalized mosaic (Arizona
  State University) from 70N to 70S, with the latitudes outside that filled from
  the LOLA laser altimeter's albedo map. Equirectangular, centered on 0 degrees
  longitude, which is the near side: the Mean Earth frame the IAU rotational
  elements in `scene::sky::moon_rotation` orient the sphere to.
- License: US Government work, public domain. The SVS asks that credit be given
  to "NASA's Scientific Visualization Studio", which the About window carries.

Preparation, on a Windows host with ImageMagick 7 and libjxl 0.11.1:

```
magick lroc_color_poles_2k.tif -filter Box -resize 1024x512! moon_1024.png
magick moon_1024.png -quality 99 lroc_color_poles_1k.jxl
```

Box at exactly half the width is the average of each 2x2 block, which is the
filter `assets::texture_loader::downsample_2x` uses for the mip chain, so the
asset is the 2k map's first mip level. The published `lroc_color_poles_1k.jpg`
would have saved the step and brought JPEG artifacts with it.

The two `magick` calls rather than `cjxl`: this machine has libjxl through
ImageMagick and no cjxl binary. Measured through the app's own `jxl-oxide` path
against `moon_1024.png`, quality 99 decodes to a mean channel difference of 0.41
of 255 with a worst pixel off by 5, at 285 KB; quality 95 is 0.95 and 9 at 169
KB, and quality 90 is 1.63 and 19 at 108 KB.

No orientation work is done offline. The loader's own `orient` applies the
horizontal flip and the quarter-width shift that line an equirectangular map up
with the sphere's UVs, and it applies them to this map exactly as it does to the
cloud map.

## milkyway_2020_4k.jxl

The diffuse Milky Way, 4096x2048, as a full-sky equirectangular panorama in
equatorial J2000 coordinates, denoised.

- Source: NASA Scientific Visualization Studio, [Deep Star Maps 2020](https://svs.gsfc.nasa.gov/4851)
  (SVS ID 4851, released 2020-09-09). Visualizer Ernie Wright (USRA).
- File taken: `milkyway_2020_8k.exr`, the celestial-coordinate variant at
  8192x4096, 137,307,727 bytes, sha256
  `361d1961647af073b3b3e4aea4fca85f15b3c9d915e0c8c4befb02520dadd10a`,
  downloaded 2026-08-30 from
  `https://svs.gsfc.nasa.gov/vis/a000000/a004800/a004851/milkyway_2020_8k.exr`.
- Underlying data: Gaia DR2 star fluxes for every star fainter than magnitude
  11.5, with the Hipparcos and Tycho-2 stars deliberately excluded, so this
  layer is the unresolved starlight and nothing that is drawn as a sprite. The
  companion `hiptyc_2020` layer holds those bright stars and is not used.
  Linear half-float. The pixels hold flux per pixel, so each published
  resolution has its own scale: the 4k file's values are four times the 8k
  file's and sixteen times the 16k file's.
- The celestial variant rather than the galactic one: it needs no galactic
  rotation at runtime, since the renderer already has the J2000 equatorial to
  world rotation that everything else in the sky goes through.
- License: US Government work, public domain. The SVS asks for credit to
  "NASA/GSFC/SVS", and Gaia DR2 for "ESA/Gaia/DPAC"; the About window carries
  both.

Preparation, with the texture pipeline in `tools/texture-pipeline`:

```
uv run texture-pipeline milky-way -i milkyway_2020_8k.exr -o milkyway_2020_4k.jxl -w 4096
```

What that does, in linear light: a gain of 4 lifts the 8k file onto the 4k
grid's flux scale, which is the scale `milky_way_intensity` was tuned against;
a box average takes it to 4096 wide; a despeckle caps each pixel's luminance at
three times a clipped local background and replaces the footprints of the stars
above nine times it, which on this run trimmed 2.04% of the pixels and replaced
0.03%; a Gaussian of 7.9 arcminutes blurs what is left; and the sRGB transfer
curve with one code value of triangular dither takes it to 8 bits, the dither
because the smoothed dark sky posterizes without it. The output is a lossy JPEG
XL at the command's default quality 90 and effort 7, 501,115 bytes. The
reasoning behind every parameter and the measurements that set them are in
`docs/plans/2026-08-30-milky-way-denoise-pipeline-plan.md`.

The published map's grain is the sky itself at a scale no eye resolves, a
different handful of magnitude 12 to 16 stars in every pixel and isolated stars
just past the Tycho limit peaking at five to twenty times their surroundings,
which is why the earlier lossless encode of the plain 4k file was noisy and why a
plain blur could not fix it: the radius that removes the grain leaves every
isolated star as a soft blob. Lossless is no longer needed either, since the
noise that a lossy codec handles worst is gone.

No orientation work is done offline, as with the Moon. What the source's own
layout is, measured against known sky positions rather than assumed, is that a
source column `c` of `W` sits at right ascension `(0.5 - c / W) * 360` degrees
and a row `r` of `H` at declination `90 - (r / H) * 180`: the standard
astronomical all-sky layout, centered on right ascension zero with right
ascension increasing to the left, north at the top. The loader's own `orient`
then mirrors it and shifts it a quarter width, and `milky_way_uv` in
`sphere.wgsl` maps a direction to the result. The measurement: crops at the
galactic center, the two Magellanic Clouds, Carina, Crux, Cygnus and Cassiopeia
are all bright, and crops at the two galactic poles are the darkest of the set,
which the mirrored reading of the same layout gets backwards.

## day/2004MM/, night/, mask/: the cube faces

The Earth's surface as an equi-angular cube: twelve monthly day sets under `day/200401/` to `day/200412/`, one night set under `night/` and one water mask under `mask/`, six faces each, 84 files. Every face is 2048 x 2048 and named `px`, `nx`, `py`, `ny`, `pz` or `nz` for the cube's +X, -X, +Y, -Y, +Z and -Z, the layer order of a cube texture. The frame is the app's world frame, +Y north, +Z longitude 0 and +X longitude 90 E, with the faces laid out by the OpenGL and Direct3D cube map table, so Africa is upright on `pz` and the Arctic Ocean is centered on `py`. The texel grid is the tangent warp of pi/4. `tools/texture-pipeline/README.md` gives the geometry in full, and `src/texture_pipeline/cube.py` there holds the table.

- Day faces: 8-bit RGB without alpha, lossy JPEG XL at quality 85, effort 7.
- Night faces: 8-bit RGB, lossy JPEG XL at quality 85, effort 7.
- Mask faces: 8-bit single channel, lossless JPEG XL.

### What a mask texel means

A mask texel is the share of its footprint that is open water: 255 is open ocean, 0 is land, and the values in between are coastline. Ice the detector finds on the ocean side of the shapefile's coastline counts as land and is 0. One mask serves all twelve months. The day faces were flattened toward the ocean fill color (10, 30, 60) in proportion to the same mask, so where a mask texel is 255 the day texel was that fill to within one level before the lossy encode; decoded, it carries the codec's noise, and anything that wants a flat ocean writes (10, 30, 60) itself where the mask is 255. Nothing in the cube set has an alpha channel.

Natural Earth's ocean layer holds Null Island, a hole of about a kilometer at 0 N 0 E, which comes out as four texels of 245 at the center of `pz`.

### Day: Blue Marble Next Generation, 2004

NASA Blue Marble Next Generation, the topography variant, the twelve months of 2004 at 21600 x 10800 (about 1.85 km per pixel at the equator), downloaded on 2026-09-29 from

```
https://assets.science.nasa.gov/content/dam/science/esd/eo/images/bmng/bmng-topography/<month>/world.topo.2004MM.3x21600x10800.jpg
```

with `<month>` the lowercase English month name, `january` for `200401` through `december` for `200412`. All twelve answered 200, and the server reports each as last modified on 2025-12-16, when NASA re-hosted the collection.

| Month | Bytes | sha256 |
|---|---|---|
| 200401 | 24,322,238 | `da0b85bfeaeb2287a322b0a69375e96387e5e1da8ededd7a51352a5507c4821c` |
| 200402 | 24,061,482 | `ffec46c42c0969119c9653bbeef3e5f25f4e11e67a4ed63ac35cb4bf3e7e3633` |
| 200403 | 23,655,982 | `dd8972ee49889847f1a6c6df7f380dcd8038e8f86a1086dd48df3cae65532943` |
| 200404 | 23,116,755 | `95ef60e0a30ddfcadedfce557f16517eaefa453101582f0c122e967ae79e6bb2` |
| 200405 | 22,744,147 | `5b9e86bb64d5c6d837460e75c03c184ab882d0a2e1ebe0d09716446f3039cc79` |
| 200406 | 22,548,491 | `6aebde5c1e11864198a0d104e75250bf4feb68f170026aecef4cd36ec0768aff` |
| 200407 | 21,796,912 | `40a42c4511ccfbcdc6928d026a50da190e718772c7e6141ebd4ee9dbe946eda8` |
| 200408 | 21,792,417 | `b60fc9d1d87337a3ee1ef48bad5c85b076ed4f7ed0045a7f33505784d3af6fd9` |
| 200409 | 22,910,406 | `d38f6099976e9ce91d45cfc3a48a24a78ef2a8deb4c0242b75a0b884b76d45fa` |
| 200410 | 23,422,296 | `9ed49a5138455859f9c8ebd74f65c4d1a0815db350e2a2e8a45197397c7064da` |
| 200411 | 24,327,901 | `29061630b63429a84c954c5a6f4b76de9a467f5ac969a2c2c2dc424f26ed8b5c` |
| 200412 | 24,485,015 | `a9afcc27cf3cae515947edf8eff53a87ec26197a640404915f25cb2fd0130bb4` |

- Credit: NASA Earth Observatory, produced by Reto Stöckli, NASA Goddard Space Flight Center, from Terra MODIS. Antarctica is the Landsat Image Mosaic of Antarctica (USGS/NASA), and south of 60 S the source is the same in every month.
- License: US Government work, not copyrighted in the US. NASA asks that republished imagery credit "NASA Earth Observatory".

### Night: Black Marble 2016

NASA Earth Observatory's Black Marble 2016, `BlackMarble_2016_3km.jpg`, 13500 x 6750, 8,106,233 bytes, sha256 `230aac448ae68c358be433dd518888cccb3a85ccf66f7b44326441c324ad6725`, downloaded on 2026-09-29 from `https://assets.science.nasa.gov/content/dam/science/esd/eo/images/imagerecords/144000/144898/BlackMarble_2016_3km.jpg`. The older address `https://eoimages.gsfc.nasa.gov/images/imagerecords/144000/144898/BlackMarble_2016_3km.jpg` answers with a file of the same length.

- Credit: NASA Earth Observatory images by Joshua Stevens, using Suomi NPP VIIRS data from Miguel Román, NASA GSFC.
- License: US Government work, as above.

### Mask: Natural Earth ocean

Natural Earth's 1:10m physical vectors, the Ocean layer (`ne_10m_ocean`), version 5.1.1, `https://naciscdn.org/naturalearth/10m/physical/ne_10m_ocean.zip`, 3,189,765 bytes, sha256 `db626fcd5d50b096b156c78a2cc95011b39f32a61b4e47d147e3f7a77b8b2719`, downloaded on 2026-09-29. Natural Earth is in the public domain.

### Preparation

With the texture pipeline in `tools/texture-pipeline`, the twelve JPEGs in one directory:

```
uv run texture-pipeline cube --input <the twelve JPEGs> --night BlackMarble_2016_3km.jpg --ocean-mask ne_10m_ocean.shp --output textures
```

Every other option at its default: face size 2048, quality 85, effort 7, fill 10,30,60, supersample 2, coast offset 0, ice kept at seed luminance 200 above 60 degrees of latitude, four workers. The encoder is libjxl through pillow-jxl-plugin 1.3.7, the version the pipeline's `uv.lock` pins.

What that does, in order: rasterizes the shapefile once at 21600 x 10800; runs the ice detection on all twelve months and subtracts the union of what it finds; resamples that mask to six faces at 5400, the source's density at the equator; resamples every month to six faces at 5400 by inverse mapping each texel center into the source and sampling it bilinearly, flattens the ocean through the 5400 mask, and averages each face down to 2048 over every texel's footprint; resamples the night at 3375, a quarter of its width, and averages it down the same way without flattening; and writes the mask faces as the same average of the 5400 mask faces.

The ice pass changes the mask on this data. Per month it keeps 3.1 M to 4.0 M source pixels as land, 0.53% to 0.62% of the ocean's area: 2.71 M of them are Antarctic ice shelves and the same in every month, the rest is snow along Arctic coasts, from 0.35 M in August to 1.27 M in February. The union over the year is 4.02 M pixels, 0.62% of the ocean.

### Measurements

Run on 2026-09-29 on Windows 11, an AMD Ryzen 7 5800X (16 logical cores) with 64 GB, while another agent's builds shared the machine: 572.8 s in all, a peak working set of 3.04 GiB and a peak commit of 4.16 GiB. The shapefile raster took 38.6 s, the ice pass over twelve months 134.1 s, the mask faces 13.7 s, the months 27.0 s to 42.5 s each and 373.6 s together, and the night 12.9 s. The peak is the shapefile raster at twice the source's size; the resampling, done in bands of 256 rows, stays below it.

| Set | Files | Bytes |
|---|---|---|
| day/200401 | 6 | 1,553,014 |
| day/200402 | 6 | 1,542,041 |
| day/200403 | 6 | 1,504,676 |
| day/200404 | 6 | 1,448,285 |
| day/200405 | 6 | 1,400,336 |
| day/200406 | 6 | 1,347,752 |
| day/200407 | 6 | 1,288,658 |
| day/200408 | 6 | 1,282,466 |
| day/200409 | 6 | 1,349,437 |
| day/200410 | 6 | 1,411,854 |
| day/200411 | 6 | 1,508,024 |
| day/200412 | 6 | 1,545,721 |
| day, twelve months | 72 | 17,182,264 |
| night | 6 | 883,886 |
| mask | 6 | 398,743 |
| all | 84 | 18,464,893 |

The northern winter months are the largest, as section 17 of `docs/plans/2026-09-29-texture-overhaul-research.md` expected from their snow. The night baked from the 13500 source is 883,886 bytes; the one-month spike in section 19.3 there resampled the 8192 map the flat texture used to be (no longer in this directory) to the same faces and got 1,033,595.
