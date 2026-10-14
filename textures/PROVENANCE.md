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

A mask texel is the share of its footprint that is open water: 255 is open ocean, 0 is land, and the values in between are coastline. Poleward of 50 degrees the ocean includes the Blue Marble's own water where it lies on Natural Earth's land and is connected to the ocean (Preparation, below). Ice the detector finds on the ocean side of the shapefile's coastline south of 60 S, the Antarctic ice shelves, counts as land and is 0. One mask serves all twelve months. The day faces were flattened toward the ocean fill color (10, 30, 60) in proportion to the same mask, and the month's sea ice was then composited into the water share of every texel, so where a mask texel is 255 the day texel was that fill to within one level before the lossy encode, or sea ice; decoded, open water carries the codec's noise, at most 43 of 255 in luminance on the faces measured, below any ice, and anything that wants a flat ocean writes (10, 30, 60) itself where the mask is 255 and the texel is dark. Nothing in the cube set has an alpha channel.

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

### Sea ice: EUMETSAT OSI SAF OSI-450-a1, 2004

The day faces carry each month's sea ice, generated from the monthly mean sea ice concentration of 2004, the Blue Marble's own year, and composited into the open water of that month's faces (`docs/plans/2026-10-11-baked-sea-ice-plan.md` for the bake, `docs/plans/2026-10-12-sea-ice-look-plan.md` for the look).

- Source: EUMETSAT OSI SAF, Global sea ice concentration climate data record 1978-2020 (v3.1, 2025), OSI-450-a1, doi:10.15770/EUM_SAF_OSI_0023, [product page](https://osi-saf.eumetsat.int/products/osi-450-a1). The monthly files on the 25 km EASE2 grid (EPSG:6931 in the north, EPSG:6932 in the south), `ice_conc` as published. Inputs for 2004 are SSM/I, with ERA5 for the weather correction (global attribute `source`).
- Files taken: the 24 monthly files of 2004, both hemispheres, 22,188,301 bytes together, downloaded on 2026-10-11 one at a time by `cube --fetch` from `https://thredds.met.no/thredds/fileServer/osisaf/met.no/reprocessed/ice/conc_450a1_files/monthly/2004/ice_conc_{nh,sh}_ease2-250_cdr-v3p1_2004MM.nc`. Their checksums are below.
- License: Creative Commons Attribution 4.0 International. The faces hold a modification of the data (a reprojection, a generated texture and a composite), and carry the credit "Contains modified EUMETSAT OSI SAF data" with the product and its DOI.
- What is drawn: each month's mean, raised along coasts where the ice 50 to 100 km out is closed (a fast ice band, since the 25 km cells at a coast mix land and sea), extended three cells into land from the nearest ocean cell, resampled onto a polar cap through a cubic B-spline, and turned into coverage and brightness by `tools/texture-pipeline/src/texture_pipeline/sea_ice_look.py` at 4096 texels a cap side, 2.87 km. The closed pack is ridged relief at three detail levels lit from one side, varied by region fields in detail, strength and softness; the leads are long fracture systems drawn thin and jagged; the open water the concentration leaves, less the leads', is a cascade of whole cells around a regional target with a calm speckle from 40% to 70%, all of it the ocean fill, read through a tone curve `c + (1 - c) smoothstep(0.7, 0.85, c)` that closes the pack from 85%; the sea within 10 km of the land is ice where the concentration is half or more. The same noise fields serve every month; the color is one per hemisphere and month, the winter's the median of the Blue Marble's coastal snow (the south's moved halfway toward the north's). On `py` and `ny` the coast's flat snowy land within 6 to 18 km of the water is blended toward the ice. The structure is generated; the extent follows the data.
- Area: the coverage-weighted area over the mask's water against the area the floes are asked to draw, the source's concentration read through the tone curve times its cells' area, came within -5.7% (Antarctic, February) and +2.9% (Arctic, July) in every month of both hemispheres. Against the source's concentration itself it came within -1.4% and +10.3%: the curve draws refrozen leads and melt ponds, which passive microwave reads as part water, as ice, most in the Arctic summer. In million km2, from the bake's log:

| Month | Arctic drawn | toned | record | Antarctic drawn | toned | record |
|---|---|---|---|---|---|---|
| 01 | 13.50 | +1.5% | +4.7% | 4.04 | -1.1% | +2.9% |
| 02 | 14.18 | +0.6% | +4.0% | 2.45 | -5.7% | -1.4% |
| 03 | 14.25 | +0.9% | +4.6% | 3.60 | -1.7% | +4.5% |
| 04 | 13.24 | +1.2% | +4.7% | 6.42 | -0.3% | +5.9% |
| 05 | 11.90 | +1.4% | +4.7% | 9.47 | -1.1% | +4.0% |
| 06 | 10.43 | +1.9% | +8.0% | 12.79 | -0.7% | +4.7% |
| 07 | 8.05 | +2.9% | +10.0% | 15.22 | -0.8% | +4.7% |
| 08 | 5.82 | +2.3% | +10.3% | 16.56 | -1.0% | +5.0% |
| 09 | 5.50 | +1.4% | +9.7% | 17.22 | -1.2% | +5.9% |
| 10 | 7.63 | +1.8% | +6.9% | 16.65 | -1.2% | +4.8% |
| 11 | 10.02 | +1.5% | +4.9% | 13.90 | -1.0% | +6.1% |
| 12 | 12.02 | +1.1% | +4.6% | 7.63 | -1.1% | +3.7% |

```
acdef73bd5db4a714acb800f63852aa92682d523f72ede605376451445ea38fd  ice_conc_nh_ease2-250_cdr-v3p1_200401.nc
cf7e51b1e4b8b4a186f5e9079a6731045591d615f7f20b5d1a89aaa165ce3ac2  ice_conc_nh_ease2-250_cdr-v3p1_200402.nc
093e9e3413d9097c6d0aed901052716ebac2156d432e85a39cbf9fd7731877ed  ice_conc_nh_ease2-250_cdr-v3p1_200403.nc
9da93fe392fc60e7d596bea0e0cce055b6b2e39c104dc856040acd8cba9c74bb  ice_conc_nh_ease2-250_cdr-v3p1_200404.nc
f701d7773d81c28d82e533430d7e93543a18c1d557d30f8ad4c47dae72e9db14  ice_conc_nh_ease2-250_cdr-v3p1_200405.nc
f134aecc9768b4fd12e80b0c3cd30b1871f639b8f64335a7b44a03987f493e79  ice_conc_nh_ease2-250_cdr-v3p1_200406.nc
0b165ce516eb09f4e9cb3d4a67ea62d9f1dd8b54e6f5f3bf9b13124ff9d12a50  ice_conc_nh_ease2-250_cdr-v3p1_200407.nc
90bd4def9d163b035536b1ee021dedae597c678f7de928f878bd5a4401835ccc  ice_conc_nh_ease2-250_cdr-v3p1_200408.nc
cfc93a642d2a8e26ca36baec2c172b9b7e51d1a3b2d3e30f76691fb4a7b3b572  ice_conc_nh_ease2-250_cdr-v3p1_200409.nc
6d841bdb87d94ddb6cb56393996efc88f2ff1470aaedcf91d5430d2c7ffd15c0  ice_conc_nh_ease2-250_cdr-v3p1_200410.nc
248498b698d98c1c6e13cdc84665b3bf710e8faa1def1f7b34a445834c8d241e  ice_conc_nh_ease2-250_cdr-v3p1_200411.nc
a53a3ef51c58b4a06bf9c6a084a4480e6c4b5438a1c1aff8076015fde9d04c9b  ice_conc_nh_ease2-250_cdr-v3p1_200412.nc
0d7ade62c23647d50e60b8291a16aed185751e77018424eeae034332f9abe303  ice_conc_sh_ease2-250_cdr-v3p1_200401.nc
8d82c94bc7bff4d9153fa36c41352debbcb632dedba1558f507c1071ca5d0c21  ice_conc_sh_ease2-250_cdr-v3p1_200402.nc
afe07e80138bf6157b4ee328f89b6ab05639658a90f3b226305287213a0149cd  ice_conc_sh_ease2-250_cdr-v3p1_200403.nc
0f40bc0cea6f1c08cdade9fd81fcb020a52d8ce2c3f3e4243685e3340f1d2da4  ice_conc_sh_ease2-250_cdr-v3p1_200404.nc
c6cacbca47a3c41e2e539aa72c9fff279af90a7b4360daccc6e0c487735efc73  ice_conc_sh_ease2-250_cdr-v3p1_200405.nc
13ea47863bb250d40d57bcbff0606cd2eebf1d7be763b92b34d0928a02c3673c  ice_conc_sh_ease2-250_cdr-v3p1_200406.nc
d6afe15996b83d7206cc1bc15abff65f470f39d23af7fcc0eeb5bf6fc3ea8ba1  ice_conc_sh_ease2-250_cdr-v3p1_200407.nc
10cfe60325718c60cdb9227b76f9f269380f440cc96b5dd279d729e71eb3bdbd  ice_conc_sh_ease2-250_cdr-v3p1_200408.nc
3e5d7b288303f7ebb1ead291105031089bf0e02b9e3a14a89c5482bd5ad599f3  ice_conc_sh_ease2-250_cdr-v3p1_200409.nc
8f3ce18089b2eb5e80c0e41afe9b01933c856d1c405f3528230a755edbdfd87e  ice_conc_sh_ease2-250_cdr-v3p1_200410.nc
36f72f00f6f3b890da2124100941406b5e7e1d117175c099edc9efcf26006921  ice_conc_sh_ease2-250_cdr-v3p1_200411.nc
460f99d6deda45fcfd118183184e27b07f8686825d628a4f6659ea9c7a57bbd0  ice_conc_sh_ease2-250_cdr-v3p1_200412.nc
```

### Night: Black Marble 2016

NASA Earth Observatory's Black Marble 2016, `BlackMarble_2016_3km.jpg`, 13500 x 6750, 8,106,233 bytes, sha256 `230aac448ae68c358be433dd518888cccb3a85ccf66f7b44326441c324ad6725`, downloaded on 2026-09-29 from `https://assets.science.nasa.gov/content/dam/science/esd/eo/images/imagerecords/144000/144898/BlackMarble_2016_3km.jpg`. The older address `https://eoimages.gsfc.nasa.gov/images/imagerecords/144000/144898/BlackMarble_2016_3km.jpg` answers with a file of the same length.

- Credit: NASA Earth Observatory images by Joshua Stevens, using Suomi NPP VIIRS data from Miguel Román, NASA GSFC.
- License: US Government work, as above.

### Mask: Natural Earth ocean

Natural Earth's 1:10m physical vectors, the Ocean layer (`ne_10m_ocean`), version 5.1.1, `https://naciscdn.org/naturalearth/10m/physical/ne_10m_ocean.zip`, 3,189,765 bytes, sha256 `db626fcd5d50b096b156c78a2cc95011b39f32a61b4e47d147e3f7a77b8b2719`, downloaded on 2026-09-29. Natural Earth is in the public domain. Poleward of 50 degrees the Blue Marble's own water joins it where the two coastlines disagree.

### Preparation

With the texture pipeline in `tools/texture-pipeline`, the twelve JPEGs in one directory:

```
uv run texture-pipeline cube --input <the twelve JPEGs> --night BlackMarble_2016_3km.jpg --ocean-mask ne_10m_ocean.shp --sea-ice <the 24 OSI-450-a1 files> --fetch --output textures
```

Every other option at its default: face size 2048, quality 85, effort 7, fill 10,30,60, supersample 2, coast offset 0, the Blue Marble's water joined poleward of 50 degrees, ice kept at seed luminance 200 south of 60 degrees south alone, sea ice caps of 4096, four workers for the resampling and the logical cores, 16, for the sea ice. The encoder is libjxl through pillow-jxl-plugin 1.3.7, the version the pipeline's `uv.lock` pins.

What that does, in order: reads the 24 sea ice files and checks that none is missing; rasterizes the shapefile once at 21600 x 10800; reads all twelve months for each pixel's brightest luminance and least blue above red over the year, and poleward of 50 degrees turns the shapefile's land into ocean where the Blue Marble draws water in every month (luminance at most 14, blue at least 7 above red) connected to the ocean, so lakes stay land; runs the ice detection south of 60 S on all twelve months and subtracts the union of what it finds; resamples that mask to six faces at 5400, the source's density at the equator, and writes the mask faces as the average of those down to 2048; raises each month's concentration in the fast ice band, resamples it onto the polar caps, reads the land on the caps from the 5400 mask faces and generates both hemispheres' noise fields; for each month makes both caps' coverage and brightness, resamples every month to six faces at 5400 by inverse mapping each texel center into the source and sampling it bilinearly, flattens the ocean through the 5400 mask, on `py` and `ny` blends the coast's flat snowy land toward the ice, composites the month's sea ice into the water share of every texel, and averages each face down to 2048 over every texel's footprint; and resamples the night at 3375, a quarter of its width, and averages it down the same way without flattening.

The join makes 143,152 source pixels ocean in the north and 34,728 in the south, along North Greenland, Ellesmere, Svalbard and Franz Josef Land above all, where Natural Earth's coastline lies seaward of the Blue Marble's; the mask's `py` face gained water in 7,186 texels, 45,774 km2, and `ny` in 3,798, 25,801 km2, and the side faces are unchanged. The ice pass then keeps 2,712,255 source pixels as land, the Antarctic ice shelves, the same in every month. Bakes before the first sea ice ran it at both poles, where it also kept 0.35 M to 1.27 M source pixels of snow and fast ice along Arctic coasts as land in every month; that water takes the generated ice, and the first sea ice bake's `py` face gained 73,155 texels of water, 421,530 km2.

### Measurements

Run on 2026-10-14 on Windows 11, an AMD Ryzen 7 5800X (16 logical cores) with 64 GB: 804.8 s in all. The shapefile raster took 32.8 s, the join of the Blue Marble's water 33.5 s, the ice pass over twelve months 52.0 s, the mask faces 11.7 s, the sea ice's noise fields 31.8 s and 31.5 s for the two hemispheres, the months 46.3 s to 56.2 s each with their sea ice and the coast blend, and the night 9.4 s. A second run wrote all 84 files byte for byte as the first and peaked at a working set of 5.99 GiB and a commit of 6.94 GiB; it took 1,100.2 s, every stage 25% to 50% slower, the shapefile raster this change does not touch included, with other builds running on the machine. Against the first sea ice bake, the night faces and the side faces of the mask came out byte for byte as before, and 29 files changed: the 27 day faces that hold sea ice, `py` and `ny` in every month and `px` in January, February and December for the Bohai Sea, and the mask's `py` and `ny`.

The first sea ice bake, on 2026-10-11 with nothing else running, took 712.3 s, a peak working set of 3.37 GiB and a peak commit of 4.32 GiB, its noise fields 68.8 s for each hemisphere and its months 34.9 s to 43.1 s each.

The bake before the sea ice, on 2026-09-29 with another agent's builds sharing the machine, took 572.8 s, a peak working set of 3.04 GiB and a peak commit of 4.16 GiB, its ice pass over twelve months at both poles 134.1 s.

| Set | Files | Bytes |
|---|---|---|
| day/200401 | 6 | 1,629,264 |
| day/200402 | 6 | 1,589,107 |
| day/200403 | 6 | 1,557,452 |
| day/200404 | 6 | 1,526,202 |
| day/200405 | 6 | 1,488,708 |
| day/200406 | 6 | 1,485,554 |
| day/200407 | 6 | 1,454,674 |
| day/200408 | 6 | 1,438,944 |
| day/200409 | 6 | 1,496,628 |
| day/200410 | 6 | 1,554,345 |
| day/200411 | 6 | 1,653,360 |
| day/200412 | 6 | 1,697,768 |
| day, twelve months | 72 | 18,572,006 |
| night | 6 | 883,886 |
| mask | 6 | 408,062 |
| all | 84 | 19,863,954 |

The northern winter months are the largest, as section 17 of `docs/plans/2026-09-29-texture-overhaul-research.md` expected from their snow. The night baked from the 13500 source is 883,886 bytes; the one-month spike in section 19.3 there resampled the 8192 map the flat texture used to be (no longer in this directory) to the same faces and got 1,033,595.
