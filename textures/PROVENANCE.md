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

## sea_ice_2010_2020.jxl

The sea ice concentration of each calendar month, the mean of 2010 to 2020, as 24 polar caps in one 6144 x 1024 lossless single-channel JPEG XL: twelve columns of 512 px caps, January to December, the north's in the top row and the south's in the bottom one. A byte is the concentration, 255 for 100%. Each cap is the polar aspect of the spherical Lambert azimuthal equal-area projection with longitude 0 straight down in the north and straight up in the south, as the source's grid has it, reaching from the pole to plus or minus 0.9235 sphere radii on both axes, 35 degrees of latitude on the axes; its outermost ring of texels is 0. `docs/rendering.md` describes how the shader reads it.

- Source: EUMETSAT OSI SAF, Global sea ice concentration climate data record 1978-2020 (v3.1, 2025), OSI-450-a1, doi:10.15770/EUM_SAF_OSI_0023, [product page](https://osi-saf.eumetsat.int/products/osi-450-a1). The monthly files, 25 km EASE2 grid (EPSG:6931 in the north, EPSG:6932 in the south), `ice_conc` as published.
- Files taken: the 264 monthly files of 2010 to 2020, both hemispheres, 243,946,783 bytes together, downloaded on 2026-10-10 one at a time from `https://thredds.met.no/thredds/fileServer/osisaf/met.no/reprocessed/ice/conc_450a1_files/monthly/YYYY/ice_conc_{nh,sh}_ease2-250_cdr-v3p1_YYYYMM.nc`. Their checksums are at the end of this section.
- License: Creative Commons Attribution 4.0 International. The atlas is a modification of the data (an average, a reprojection and a quantization), and the About window carries "Contains modified EUMETSAT OSI SAF data" with the product and its DOI.

Preparation, with the texture pipeline in `tools/texture-pipeline`, run on 2026-10-10 at 20:53 UTC on Windows 11:

```
uv run texture-pipeline sea-ice --input <the 264 files> --output textures/sea_ice_2010_2020.jxl
```

(with `--fetch`, the command downloads whatever of the files is missing and writes their checksums into `SHA256SUMS` beside them). Every option at its default: caps of 512 px, effort 7. netCDF4 1.7.4 reads the files, rasterio 1.5.0 with its bundled PROJ does the ellipsoidal projection, scipy 1.17.1 the land extension and the resampling, and pillow-jxl-plugin 1.3.7 the encode, as the pipeline's `uv.lock` pins them.

What that does, in order: reads `ice_conc` of every file in percent and checks that every file of a hemisphere has the same land cells (88,847 in the north, 33,305 in the south); averages each calendar month over the eleven years; gives every land cell within three cells (75 km) of the ocean the value of its nearest ocean cell and every other land cell 0, so the smooth read in the shader does not pull the land's zero into the ice along a coast (the app's coastline is the water mask's, which is finer); maps every texel center of a cap back to a latitude and longitude through the inverse of the spherical projection, takes that latitude as geodetic to EASE2 kilometers with PROJ, and samples the extended field bilinearly there, 0 outside the grid; rounds to bytes and zeroes each cap's outermost ring; and writes the atlas losslessly. The run took 2.4 s, and the atlas is 340,437 bytes.

The extent survives the bake. Counting texels at or above 15% concentration over the source's ocean cells, at the equal area each texel covers, against the source cells at or above 15% in the eleven-year mean, the atlas is between 0.07% and 0.85% smaller in every month of both hemispheres (the pipeline's suite checks 3% when the files are there); the source's March maximum in the north is 15.59 million km2 and its September minimum 5.64, the south's September maximum 20.18 and its February minimum 3.93. The extension into land counts for nothing there, since the app's mask removes it and the count leaves it out.

What the data is and is not, from the product's documentation (`docs/plans/2026-10-10-sea-ice-research.md`, section 2): the monthly files set means below 10% to 0, which the 15% threshold of the shader draws nothing of anyway; the effective resolution is coarser than the grid; melt ponds and thin ice are underestimated in summer; land spill-over leaves false ice along some coasts; the Arctic's pole hole is filled by interpolation. The Caspian Sea and the Great Lakes have values; the app's water mask counts the Caspian as sea and the Great Lakes as land, so the northern Caspian gets its winter ice and the Great Lakes get none.

The 264 files and their SHA-256, as `--fetch` wrote them:

```
06ce5940aa0cfeafe9d0cfa2ead2295a41488ea9d9856589939b10b2dd3c9c32  ice_conc_nh_ease2-250_cdr-v3p1_201001.nc
72b6b849ff61a670c48f86b65a2d23b5decdab01f0b7b94f38bf0117a658a167  ice_conc_nh_ease2-250_cdr-v3p1_201002.nc
a793a5111eec6b708e652fcf2515c34b85fba19d9cc6c76ef4230e4369a4ccda  ice_conc_nh_ease2-250_cdr-v3p1_201003.nc
a34fa0308388e4113fe85c654d4e8ae2fe3bb841d4feed02bd3a3d8006f9c609  ice_conc_nh_ease2-250_cdr-v3p1_201004.nc
e563ceb46c9d628b1739d8679bc0c212e15f18ef4ae5851bf7701547d1bd721d  ice_conc_nh_ease2-250_cdr-v3p1_201005.nc
388e64f20d77b244d3cc593e68d71e945ac23625b216556724c4c3e925a242da  ice_conc_nh_ease2-250_cdr-v3p1_201006.nc
e34347f87e258c0afeccbaf0f744065641454f9cea46b0e726dbe841127a1b38  ice_conc_nh_ease2-250_cdr-v3p1_201007.nc
da6611fc77329ad07e4c86fbc97e174bb4c90e826b46c98c32f03df9a80ca8db  ice_conc_nh_ease2-250_cdr-v3p1_201008.nc
496b5c24e19e8448a71cb4499313764fe06572a264fad1fb23a98dd43b2000fc  ice_conc_nh_ease2-250_cdr-v3p1_201009.nc
518adc202b859b360c5f8c32fa87569e4d0c359a54fa88a0220c6d9c667b4624  ice_conc_nh_ease2-250_cdr-v3p1_201010.nc
a9eb4ccbaab215a9f93467de48672e9ceb79730795b86fbf145f644e3fc43256  ice_conc_nh_ease2-250_cdr-v3p1_201011.nc
6fe8c5892b1b388bd97dcc6c2a1e84345a3ddc32d6022d38aa7c2bee3a786daa  ice_conc_nh_ease2-250_cdr-v3p1_201012.nc
ed4d70b224580569f4e0bda46bfe67a33a370d6b0b7825bccee72c05e7f68b95  ice_conc_nh_ease2-250_cdr-v3p1_201101.nc
2db8a6a64d53c2a2681276ca9e20992757d152f7565476aa666b4bc947d3009c  ice_conc_nh_ease2-250_cdr-v3p1_201102.nc
15278303b644a8899c49b7d9e3ea070f3ab5546f0f2708ebf7dd73d56e161923  ice_conc_nh_ease2-250_cdr-v3p1_201103.nc
f314a649ea7620d9160bcc5ec8ed19db87092a2407a00379c8135fc4d7434a05  ice_conc_nh_ease2-250_cdr-v3p1_201104.nc
34e27aec4ef515d16669222078ca9f6f7b57827903324561dc7e49d38de3b692  ice_conc_nh_ease2-250_cdr-v3p1_201105.nc
0919fad35d6e5d3109767b819b3dfbef053ebc70d28f263db1c20e5a543a8e6d  ice_conc_nh_ease2-250_cdr-v3p1_201106.nc
52bf48608208acb98a74e139895c4f175927ea5787e41ef0cb5a7b0abb1703da  ice_conc_nh_ease2-250_cdr-v3p1_201107.nc
97799f09dc6f287e36552d3584b714626de14508c96eabf2adbbef9f63e37bfb  ice_conc_nh_ease2-250_cdr-v3p1_201108.nc
3080c032d7fd3fb1b0c9532eec109519ab071dad63a62583ac80c9a11253506f  ice_conc_nh_ease2-250_cdr-v3p1_201109.nc
aa78ac441003656aadb4a238535a9fe86fa40b1b3a82995d792c2dec2c61e07e  ice_conc_nh_ease2-250_cdr-v3p1_201110.nc
97a680e0e73913d9cb3d6152dea6b74a80a24b152a49fd9ee004e52312bef884  ice_conc_nh_ease2-250_cdr-v3p1_201111.nc
2112985fb2b6995be59b4efae9e9e384dde2564691d3c7a6d402f6a5435f6bd3  ice_conc_nh_ease2-250_cdr-v3p1_201112.nc
37d6b0ee436d062af28343d3c8799d4ab446356fa524f78c72f362fa5bd5dbde  ice_conc_nh_ease2-250_cdr-v3p1_201201.nc
8914dbe50281f5388b70665afb752795042fb40398850f1bd531f5d3f03e254c  ice_conc_nh_ease2-250_cdr-v3p1_201202.nc
5c3e284e91f11878a1988a8bebaf5ef852da3c8800cae00b7a0e37bc85da831c  ice_conc_nh_ease2-250_cdr-v3p1_201203.nc
8e4da1bd88a13be5e58800a1870925f3aa1f9b2a3903558229b16c6bc5d39c86  ice_conc_nh_ease2-250_cdr-v3p1_201204.nc
d8f0d7f5b60e032579df709a75dde01184835ba8735859b3db3d1bfbd329b670  ice_conc_nh_ease2-250_cdr-v3p1_201205.nc
a8b7e1920bf3ace9253a2ffcbec988b7a4bb73541df7cc8d4fbe0b548aed3b75  ice_conc_nh_ease2-250_cdr-v3p1_201206.nc
9b7cf025267b96c695462067e3a1637d8ec98745ad74a4428591d823a7c26f06  ice_conc_nh_ease2-250_cdr-v3p1_201207.nc
2520f93f42853b64506bbc674bd1bfc6c863aaba3b788c17611a284b40301524  ice_conc_nh_ease2-250_cdr-v3p1_201208.nc
82a0236844864765477647fa4cb0546a49567441474a480ff9eebdd47ce9c340  ice_conc_nh_ease2-250_cdr-v3p1_201209.nc
129285b14af7d649e72e7371caa3b04523427508a883a5afb50afd32c2833472  ice_conc_nh_ease2-250_cdr-v3p1_201210.nc
19efb3c193a9a0dfa9b0e33931f63a7a3b2a1e1121d63fbe25d9bcd57a4028d4  ice_conc_nh_ease2-250_cdr-v3p1_201211.nc
c30557596a8929360ed919493c86a971fda21b7fdb3b8fef44cfc72b37d94e1d  ice_conc_nh_ease2-250_cdr-v3p1_201212.nc
295bb4f1772d13e0c3bab6314498e70cc8fafcf6dd490fd36de13ee63298391c  ice_conc_nh_ease2-250_cdr-v3p1_201301.nc
21219809fc4e9641ac5314c5fb74cea5783d4e4c435ecaa9ec764c41c2613329  ice_conc_nh_ease2-250_cdr-v3p1_201302.nc
bef93fb73053d558fa6870f848963214c9d5a7dcde414a16dbc0f0bdfc57d4c4  ice_conc_nh_ease2-250_cdr-v3p1_201303.nc
2516e08d546b44ed4c304f8895e1ecb25a9c46961fcd11e090750a5386b2161e  ice_conc_nh_ease2-250_cdr-v3p1_201304.nc
ec8c412c804752d4f6c7a2e59f7f17893e8ad658f328f474b2e5e566536bc164  ice_conc_nh_ease2-250_cdr-v3p1_201305.nc
a43a888a2dc3dc9126b3014131ee2aa547b6f6036fdcd8174005410b2abfca8b  ice_conc_nh_ease2-250_cdr-v3p1_201306.nc
0bec63bd4a67af11d116918e63e81a87fe92a574e562fb719368c6e8e345e17c  ice_conc_nh_ease2-250_cdr-v3p1_201307.nc
3e7a76a0002cb52a63dd64c637158e073888d36a4fe71a70fd6c43502f92b8de  ice_conc_nh_ease2-250_cdr-v3p1_201308.nc
0eb1ee484404f881d503bf7b022ebb4195f60283cc2b566b6b4a0a5ad5a478a8  ice_conc_nh_ease2-250_cdr-v3p1_201309.nc
dd2c6986d2f01e68a49f978b1f485fca7fb3b3a7571f6c47bcad510a5219e7e5  ice_conc_nh_ease2-250_cdr-v3p1_201310.nc
33dd9fff737b27e87e7d8b040d5b43943be2ca8acce3617d9ef695c5ea5acce2  ice_conc_nh_ease2-250_cdr-v3p1_201311.nc
ea12abab32fb038a81554eb557c294ef0947be52a1a3ca4a4816df1d81c43a4a  ice_conc_nh_ease2-250_cdr-v3p1_201312.nc
c6bb9399fcb5570bd09802d6d14d4c61cf10c780d79ad78750989c7d7a54e8d8  ice_conc_nh_ease2-250_cdr-v3p1_201401.nc
5130b399e02fcdd74a821ac221dd268e5a406b2cd9969a6ac7ed837172647300  ice_conc_nh_ease2-250_cdr-v3p1_201402.nc
fe5c6c53eea2ee7d93057c587b60afbc65eeeb3642dbf8fe523a58ca2d626cc9  ice_conc_nh_ease2-250_cdr-v3p1_201403.nc
54a999988ade326fc77508a87e702d2c8e1e68582c29d42209ece47434e0a294  ice_conc_nh_ease2-250_cdr-v3p1_201404.nc
a442b98fb05193503f362d50efe1440061a2829f052715515724909e7e09cf9e  ice_conc_nh_ease2-250_cdr-v3p1_201405.nc
112fdf15444325d630ddea257d9697e87b27423e74533c3cf4465c3b2433f419  ice_conc_nh_ease2-250_cdr-v3p1_201406.nc
56c52c6ef9974d3f47c5b1838805913ef33a000827ea38de7b3f37bb05856617  ice_conc_nh_ease2-250_cdr-v3p1_201407.nc
0e32f2cb235f6ea11d221b427275792a63b1dfd77944bbeae6ca1c82bffd7fb2  ice_conc_nh_ease2-250_cdr-v3p1_201408.nc
7309a36f8d88b169396023070bd8d0a63ad4f5721563c4ce742e15b569849a1d  ice_conc_nh_ease2-250_cdr-v3p1_201409.nc
0956d314109d1df168f818066da42b328083bb40c0b4145924696d310beebfb4  ice_conc_nh_ease2-250_cdr-v3p1_201410.nc
e4518090f0b722f2c44d21329089def84c5fcf891e0788d8517832d4570df16c  ice_conc_nh_ease2-250_cdr-v3p1_201411.nc
95bf88240461707769adc27e2251cc812c6b241cdf994ef772f41a84edf96baf  ice_conc_nh_ease2-250_cdr-v3p1_201412.nc
14a213138587acbd4a9a101d6003eaa7ffc8d30627caac7bc264eaa1b923034e  ice_conc_nh_ease2-250_cdr-v3p1_201501.nc
18405c5d299f2fc199f4c32cbfe1f60f40decdb8d9b8b341a486770219d9932c  ice_conc_nh_ease2-250_cdr-v3p1_201502.nc
6c4292139ebcf37f1581a7425d94b95366e4c83179d45e506e3e4c300dafe373  ice_conc_nh_ease2-250_cdr-v3p1_201503.nc
ceca3d66fd40dbf7661f68fbb450e9e5f7933cc53b3131d30d8478f19961e2e5  ice_conc_nh_ease2-250_cdr-v3p1_201504.nc
015df8c4b2d1338f1c10ac4bfa014ac75b6ff6d546f3a964557ba1014a8de940  ice_conc_nh_ease2-250_cdr-v3p1_201505.nc
bae4e3ee6322a76548e038f51b6159b1f6d73cc7fe8f540aae28d50135e0eea9  ice_conc_nh_ease2-250_cdr-v3p1_201506.nc
e4d63e50b9aa6b98aef356d06218cfaf010a9015657484f6aec9fdbaa01d25ea  ice_conc_nh_ease2-250_cdr-v3p1_201507.nc
308df4117211ef819b990feb7b76f8ef68ba7288f9940ab162f1efd88c493717  ice_conc_nh_ease2-250_cdr-v3p1_201508.nc
adeb68b7fec0b249e361194b041f9c66b8c2ccd64480f1c961ffceac7d82d559  ice_conc_nh_ease2-250_cdr-v3p1_201509.nc
b0d46ef11f51feb6a59de6edac94025e7c10a391df0a1bdb75df1027ba3a785b  ice_conc_nh_ease2-250_cdr-v3p1_201510.nc
2bca1e64cc4f811f3706cb4a04f19b4768989fd5975311acd3f4c5b4fe84790f  ice_conc_nh_ease2-250_cdr-v3p1_201511.nc
6ba58092a42f3b44b7e1289dd37cc32250374b97c409a5a4bae50bfd57a1d226  ice_conc_nh_ease2-250_cdr-v3p1_201512.nc
684a75549e05e338772e95fa8f53eb747671d3c252a7810b51b74fd53b7f821e  ice_conc_nh_ease2-250_cdr-v3p1_201601.nc
05cbcb32e0ac80c37973f69d71edebd318cbf0c4d7422dd4a4103ed9d2c649d6  ice_conc_nh_ease2-250_cdr-v3p1_201602.nc
f9e482ec8d4dafce4bf19a967358d1c4dec39aaaca4ae6c5bdbd5de94041a6ac  ice_conc_nh_ease2-250_cdr-v3p1_201603.nc
b15043ab6890c8b170ad18c89c863fdede96cbfda492f09b72159fe9494ec8b4  ice_conc_nh_ease2-250_cdr-v3p1_201604.nc
133c1f1b896cfbcfde90d47cb329727ed254bff582fec75fde08fb8866701a4d  ice_conc_nh_ease2-250_cdr-v3p1_201605.nc
eeba103486071d67073bcc1e89ce7bd0cca0930e55c39480659b77779df0a2f5  ice_conc_nh_ease2-250_cdr-v3p1_201606.nc
4c178677fa3c976fa252128163320b530b74c91a7098f96ad67184c562950907  ice_conc_nh_ease2-250_cdr-v3p1_201607.nc
3192f73080001fc5d70e465bd19232896d3231943f4047e1b0d6ffe638f6fd67  ice_conc_nh_ease2-250_cdr-v3p1_201608.nc
0b59e1e1e62ad402691e32b9c6c37e4a30723bbc050b91a8bf2f5244caf5498d  ice_conc_nh_ease2-250_cdr-v3p1_201609.nc
de96556bda811a3f53504c50ad1190c56bf9faa0c626c78f82b269136a1daf7e  ice_conc_nh_ease2-250_cdr-v3p1_201610.nc
996709a0d8bd248404e5a67be90110b0ed68851cf7050ccda530f60c02d5aeae  ice_conc_nh_ease2-250_cdr-v3p1_201611.nc
0d5c1b497435e06a7cd9972081daf792dae42988636badb4a1cf7be4d9297cd8  ice_conc_nh_ease2-250_cdr-v3p1_201612.nc
4cda980471485bd9812165892f6d8ee235d1c2e31268bff159d95baf21258f8c  ice_conc_nh_ease2-250_cdr-v3p1_201701.nc
5a484005cc1b48e701856b21f7be6e8b15635e7ca4f38dabfd1fd22c4a0b4363  ice_conc_nh_ease2-250_cdr-v3p1_201702.nc
9ebd7b3ef305e3842b35366f3a3775ec42a28256ac9f92706e6f8e485418a12f  ice_conc_nh_ease2-250_cdr-v3p1_201703.nc
87541e62a30a4cc04d1501bb430606a14e872a5d1973e42500f21c230752e3db  ice_conc_nh_ease2-250_cdr-v3p1_201704.nc
8cd6659d6eff5681fa6842b7e0d3ca9ffdddb844982edccaabfb81df033f5e29  ice_conc_nh_ease2-250_cdr-v3p1_201705.nc
69ccb29a9ae3cb35312085db8ca4ffbfee8045b1ce52bc3e5e6d3c7f78f4be82  ice_conc_nh_ease2-250_cdr-v3p1_201706.nc
d950043419a37637f3fa30a7365e5b9e859b491cab43e62c079d7871c85be6ad  ice_conc_nh_ease2-250_cdr-v3p1_201707.nc
9c28818cd2d3e40852954ef4834644f59c4b36c270207ae8baa1341cf001813a  ice_conc_nh_ease2-250_cdr-v3p1_201708.nc
d717d26c7f66d2fe278cc79e577a94bcd5e6f8a18130251704d5d1dd6a7cbca9  ice_conc_nh_ease2-250_cdr-v3p1_201709.nc
8f015e2bb2d1b93bdfda232aa1337d00361a89eee9bc126a46ca6c6931d49e65  ice_conc_nh_ease2-250_cdr-v3p1_201710.nc
9c31ddbcc90cfad5efbf14eeab37d688888165f195140319e6d9db9d313f015d  ice_conc_nh_ease2-250_cdr-v3p1_201711.nc
f4839d553d10d533c363c4ab83443187017ac895a554d59d390be927d20d608c  ice_conc_nh_ease2-250_cdr-v3p1_201712.nc
fb31354989560e02385a8ee5b7a35d177fd2f986edc9c25c0e265ca6553a04e5  ice_conc_nh_ease2-250_cdr-v3p1_201801.nc
f6e6cbf8725c3ae1dd3c1f10b3a6b45a0fb21bbc7f18d42b6cfa8a6e7e6242dc  ice_conc_nh_ease2-250_cdr-v3p1_201802.nc
9f199192a2a7f0198381b73c60ae8806eafd744eb8c144306d810755a2132076  ice_conc_nh_ease2-250_cdr-v3p1_201803.nc
5fdb79a97a5b10cdc864d9a7c6f79ae27bcf7d60cec0120ff68bb36f3bc47334  ice_conc_nh_ease2-250_cdr-v3p1_201804.nc
fe4499d142041d2b46fe8969c7e857b5a50523fdc96935e338b3cb9b031b34f5  ice_conc_nh_ease2-250_cdr-v3p1_201805.nc
2b59540004c4b63180dd3e8ae5b1caf6b7efca2630f2186b919acc3f1968b01b  ice_conc_nh_ease2-250_cdr-v3p1_201806.nc
a09e9f76a9d0e45349539c616472f52a7291cd9509a8166e39efc59db9020ffb  ice_conc_nh_ease2-250_cdr-v3p1_201807.nc
e40cbd950e5ab9571926c4666cc268d6d019fb9be87cf3643f78dfe475eb710e  ice_conc_nh_ease2-250_cdr-v3p1_201808.nc
7a302008b1be213773eede2775732148b21ab042d7c8e0fa9c2ff8273432b996  ice_conc_nh_ease2-250_cdr-v3p1_201809.nc
f606ba834992e678bd04ac2aea0a7ef6baa0e1749f462ca891a01d34b1b04200  ice_conc_nh_ease2-250_cdr-v3p1_201810.nc
46f03aee11a462008d58cd81c1f32e9867a46d8d64da1536f7343cf76f724952  ice_conc_nh_ease2-250_cdr-v3p1_201811.nc
de65092b08ec73a4469baa9ee26a839474326649a577ae45416474b82afd18a9  ice_conc_nh_ease2-250_cdr-v3p1_201812.nc
796789006246686b9ec5ec935822c3006804cfd2767629d3a7932c6e8281cdf0  ice_conc_nh_ease2-250_cdr-v3p1_201901.nc
4f07e05844935dd8759c9badacdf5679f1e791cd48050e67e0d8138d6ba77fbe  ice_conc_nh_ease2-250_cdr-v3p1_201902.nc
166365f19e3c328478416e91057f731c37e6f1949e77e1e94147985aae302bfc  ice_conc_nh_ease2-250_cdr-v3p1_201903.nc
031d7789100f0ea0707be54106ceeaf63334f8e339c560821445ffed2fc071d9  ice_conc_nh_ease2-250_cdr-v3p1_201904.nc
7022dbd9b4f5b6e7079483e065d19f21ec53eb5b751bbe5d5cf8d636b2ab23c0  ice_conc_nh_ease2-250_cdr-v3p1_201905.nc
bfaae3f9d98de291946aebfac3079409b2a4b0b5dbad75c1f68b4971b8e28859  ice_conc_nh_ease2-250_cdr-v3p1_201906.nc
cd39c381c25faaea1e4905fd1036b0522025c21acbd0b1024b7a643ada2e8f61  ice_conc_nh_ease2-250_cdr-v3p1_201907.nc
979b26921968befe655635522ff0ae9d2e9ad74a396aba8a256e0817117e8ba4  ice_conc_nh_ease2-250_cdr-v3p1_201908.nc
57c6a57c425b52db3e00630d64fc8ff66391371c649eb202abf3e3d5555c765b  ice_conc_nh_ease2-250_cdr-v3p1_201909.nc
b56d5cd117831ecfd378dea4a1d15134ce83a4fe83a91c9ee484bbf03b1771cc  ice_conc_nh_ease2-250_cdr-v3p1_201910.nc
3282e38d248f14ea39752a246c7180c8dc8c6c24c97ff55deabe129d7ad8c2ea  ice_conc_nh_ease2-250_cdr-v3p1_201911.nc
1228bc9d6a8f80545c2e17ca3e738150dbf1076deca08397e172641327fe7aa1  ice_conc_nh_ease2-250_cdr-v3p1_201912.nc
84fd6c224e612d95d2af9de76e19f9ecc9dd77927d95111e349137681877fbfb  ice_conc_nh_ease2-250_cdr-v3p1_202001.nc
9cd5e0ba6a9a6f3f17dff0484f1ede50648bf0285db3d35fc4245e611e73bbfe  ice_conc_nh_ease2-250_cdr-v3p1_202002.nc
f4cc44e20b843228cfe6bbf99afa9d40e9499d62a2763ec7ffe17133ed1cb5e3  ice_conc_nh_ease2-250_cdr-v3p1_202003.nc
e46b01adfe104cab0580b2f1eaa1fde5510420b85d686ef103b9cd4597ff5fd6  ice_conc_nh_ease2-250_cdr-v3p1_202004.nc
c077d79d50f2d0f630e4eb975a86ca49bb1a1df145b099a8e1a8feb906213bce  ice_conc_nh_ease2-250_cdr-v3p1_202005.nc
4219218844d34f003e4cd464767bb21497aae9b8c9e640800e76af9635518cba  ice_conc_nh_ease2-250_cdr-v3p1_202006.nc
4db3e0cc4775bec0290f0332cb012de72119b4cab9dfe07f73d219119d94c7a6  ice_conc_nh_ease2-250_cdr-v3p1_202007.nc
6023d16972806b791817a9e8b49d2b9827977fceb95b24dc673416d30fbd4e0b  ice_conc_nh_ease2-250_cdr-v3p1_202008.nc
81d84bd9f50829fb292bb8809f7342fa82029cbf694a8a7370c21e8b74d6a642  ice_conc_nh_ease2-250_cdr-v3p1_202009.nc
99aa5537b46e231c1a2b2959d1e7c66868aefabff60f25c09c37bd8211d01c8c  ice_conc_nh_ease2-250_cdr-v3p1_202010.nc
d7f9b53f214facc92d6d7d37204b2d65eef184a864b4a82af6ca5766e8956f1b  ice_conc_nh_ease2-250_cdr-v3p1_202011.nc
5ea3a08158a261641010a6cc7eb56562a6f01c549eed474061be73ce9cc33701  ice_conc_nh_ease2-250_cdr-v3p1_202012.nc
ee7b2dca311b6e605eaa70199a2ab9e9e48e60f7dfb91c4914187bb5edf246e5  ice_conc_sh_ease2-250_cdr-v3p1_201001.nc
040882eb40d1d6e446c8c8a9e7a7602bd72fe03e40b9c582c7a05e4dcbac653c  ice_conc_sh_ease2-250_cdr-v3p1_201002.nc
865f73a1fcc9f04fda992a347b4d926ef2c8ef1ac53f9e161e053e4ccf25b4c3  ice_conc_sh_ease2-250_cdr-v3p1_201003.nc
bc316f7954105c1eeedee0b95c4b5a77943c2823e84a4d1130e2556f2927c201  ice_conc_sh_ease2-250_cdr-v3p1_201004.nc
2abf44e06668dadaa9a61357fd0861bee52207d958b879d9f0eab57e4d6d094a  ice_conc_sh_ease2-250_cdr-v3p1_201005.nc
5aa8d4baff3be4c959de8d0602d82caea69f4d88810bdc910d12570acfbd6040  ice_conc_sh_ease2-250_cdr-v3p1_201006.nc
ed63828c6b960c10a5dd39cfb90f00fea91393a7d8048c45d043c60897778913  ice_conc_sh_ease2-250_cdr-v3p1_201007.nc
49a42d7bd7c954b4ddad499d7b79de94b49f1a4a7aff4ba87bc8c9d8fe601be3  ice_conc_sh_ease2-250_cdr-v3p1_201008.nc
a1bead82c4f0927b297a77c6d7aba74b72db6bcf28e91b8ed2c8a7db67231be3  ice_conc_sh_ease2-250_cdr-v3p1_201009.nc
157624f869c880aa264adcd2bf2315f0bd9c50a8200970f362070a426bd23539  ice_conc_sh_ease2-250_cdr-v3p1_201010.nc
294fa3dcf82cdecdd42f270050699630fa6003de4f3b197e12bc9a92b1879421  ice_conc_sh_ease2-250_cdr-v3p1_201011.nc
b67a0757d0dbe1ab73937a0b69d655322fa7349bbe1a19025c1a6c10bbdb12af  ice_conc_sh_ease2-250_cdr-v3p1_201012.nc
8a04fae331872421372435fe7a2aa7b0a932b7c50c87bee86502cce93e076aa3  ice_conc_sh_ease2-250_cdr-v3p1_201101.nc
7b4db4072b7ad74d7c8a1206a92de5be8fa9b4ef7f583d471b087bc35266ad36  ice_conc_sh_ease2-250_cdr-v3p1_201102.nc
c73e5f1b89b011ceaca95d953b3397f81d1ad4fbfb384f42e2628ce27662c44c  ice_conc_sh_ease2-250_cdr-v3p1_201103.nc
a394e89fa9f43f58fb243c2950717cca7ff5951cff71c531b9c3aefd88bba167  ice_conc_sh_ease2-250_cdr-v3p1_201104.nc
9ee399e99823800a32c3908e02a89bf048784fc5a1db037aade0460769b81c63  ice_conc_sh_ease2-250_cdr-v3p1_201105.nc
befdcaa49e3e7f05f3e66cbd4312c8d1ee28ae56a2885d0da9c1059ba063f9dd  ice_conc_sh_ease2-250_cdr-v3p1_201106.nc
7427ddb01c41a71539b7ac29a873b1b32e9da877a011b103a476174f0efb1b73  ice_conc_sh_ease2-250_cdr-v3p1_201107.nc
eeb430016b5ee2694b4794c2f944350035299871a3675d3ddd220238ef9686e2  ice_conc_sh_ease2-250_cdr-v3p1_201108.nc
2ec382658461ecd656c170e9cfa4a0afe03535e10d75a928d64dc7852afd760a  ice_conc_sh_ease2-250_cdr-v3p1_201109.nc
f650b997882dacfb2c9f96d9cfd1ab1cc6b797b3efafbf6f4afbf0dbfb3a5916  ice_conc_sh_ease2-250_cdr-v3p1_201110.nc
eab46e3cb223c5c031724cd743397fc7ac02bb18cd3210c96b857a11b02736be  ice_conc_sh_ease2-250_cdr-v3p1_201111.nc
a0f4ea736e9b17fe94ceb1677a349161217dd52c84e1fad971d80b4f229958af  ice_conc_sh_ease2-250_cdr-v3p1_201112.nc
a845ac256c4e7f93d255ade380ef4891457c12cc266d4fe3c4dec0f5c3876009  ice_conc_sh_ease2-250_cdr-v3p1_201201.nc
3986fdd8e35ea89f798e04df5a7570477590938217a3f940514684b5a22b03ed  ice_conc_sh_ease2-250_cdr-v3p1_201202.nc
d2de155db49eeb1a3a61520d1482ad9fc299f0a291ac5780cec2367b79a89c4a  ice_conc_sh_ease2-250_cdr-v3p1_201203.nc
a7ce42ddd2fe12d741cf74a3ebe79cd52f289bf1b9f2fc18b4b2193e76c572cc  ice_conc_sh_ease2-250_cdr-v3p1_201204.nc
9bcac9dd387b42247f73db40297598ae710698ece5bb56306d7a39f92c2511bb  ice_conc_sh_ease2-250_cdr-v3p1_201205.nc
48734b8cdc6e8622af491462584521504824c25b407de743c4b583bcc6d0e36b  ice_conc_sh_ease2-250_cdr-v3p1_201206.nc
be50812436b73a32674297acbbc475e51daa726dcd9d392b86d8ae8af48c277a  ice_conc_sh_ease2-250_cdr-v3p1_201207.nc
612a2bb29071fb042bc956ee15aa1edee0b68f02b1374c1e3ee22958e241f029  ice_conc_sh_ease2-250_cdr-v3p1_201208.nc
83f2633f46d319b1158e857e669a3bae1473237d875024d45e13d1a9d6dae057  ice_conc_sh_ease2-250_cdr-v3p1_201209.nc
654a765612f258bc7a600daf74a9a138f61afd4c7373dcb7d100e8b014b71999  ice_conc_sh_ease2-250_cdr-v3p1_201210.nc
a65cbd2d1f2db43008c0e9812bf93a62cfabd0767d05e294752f9816f31dc9a7  ice_conc_sh_ease2-250_cdr-v3p1_201211.nc
fd45cae09cab41d8a3ff75eaa80fb57450794c6765985cdf8278a269a8d9a4e2  ice_conc_sh_ease2-250_cdr-v3p1_201212.nc
8eee4897c31f88763fb4aea5e84d7fa58fdc7d005f1e12c4c51f1bb03663afa6  ice_conc_sh_ease2-250_cdr-v3p1_201301.nc
3c186750e95511d3a67d8507fd7bab20aa3425b6d0df9c16763b8ff1b0739cef  ice_conc_sh_ease2-250_cdr-v3p1_201302.nc
6c09c747881620ea1c61ba70e6a3c16c1e5066984700c134115fdee3aebdbb25  ice_conc_sh_ease2-250_cdr-v3p1_201303.nc
f6d052438e1b807bd2a16c1b3256c6f610d71dbf3677fb64f3e7524398334661  ice_conc_sh_ease2-250_cdr-v3p1_201304.nc
3778ec339655b492df2d8dec94525b27b362211eebf15d71719da117b4999ebe  ice_conc_sh_ease2-250_cdr-v3p1_201305.nc
3f2887f0411adb507cda20b698dab1880a339e19bca7e7b2026de9b9386039a6  ice_conc_sh_ease2-250_cdr-v3p1_201306.nc
4798fa56ad02a55fb94fc2dc70d74b744a6e35556e8692b45c82a6d556287ef8  ice_conc_sh_ease2-250_cdr-v3p1_201307.nc
c9db2e6c20ce813024d587b7906226b46f54d34e7a07681bca3d7901cab948a4  ice_conc_sh_ease2-250_cdr-v3p1_201308.nc
9d3c64b939c2f5818e076952d967be2ebb034a18958848c216f61c6b56e0384b  ice_conc_sh_ease2-250_cdr-v3p1_201309.nc
5a11c088223c491570b7cd88a2e3c9cbc66703d00c4f7ced55cb8d57e4e7c4ca  ice_conc_sh_ease2-250_cdr-v3p1_201310.nc
cc75042f6dd97ca9209a7dd07c99f1881b91c3f2c5ddf2ddb27f8249eaaad24e  ice_conc_sh_ease2-250_cdr-v3p1_201311.nc
8bd2eebd96e4f4bbad6243ba80cbd6eb4d4f37fc102bf0ca327e71c2b9849b19  ice_conc_sh_ease2-250_cdr-v3p1_201312.nc
610899571fe6ce4a8fd23083972ac5ae987eb6ddd3999750af74e574303e9de4  ice_conc_sh_ease2-250_cdr-v3p1_201401.nc
d98d4b5328b600896916815aed716df05e821071c1a9ce4ad8a775c70b4585a9  ice_conc_sh_ease2-250_cdr-v3p1_201402.nc
37eca7433919e9d43218d4cfe8cfc0825343d00ac4aca9afa18387bbee4c3886  ice_conc_sh_ease2-250_cdr-v3p1_201403.nc
8bd8f98db5d27a6a8dfcb6945a0d1863e8745f680e54ab6a23f0776364214a48  ice_conc_sh_ease2-250_cdr-v3p1_201404.nc
90aafe3bbcae87b03d09880f7e6ab045c4897c99516fa3f46584994a4df17322  ice_conc_sh_ease2-250_cdr-v3p1_201405.nc
67e87b588d08c1aea83a63e0f7ed1ba25cb3618c99fd4fb6ad04259614a15dc8  ice_conc_sh_ease2-250_cdr-v3p1_201406.nc
040b53be2501344c7a572f6b7951ab666cd0925294feb94260905dbd075f7278  ice_conc_sh_ease2-250_cdr-v3p1_201407.nc
3bdd11a2258dfd3964cc91f7fe7c3ce880fb31138d9e775fb2036c1e5fa61b09  ice_conc_sh_ease2-250_cdr-v3p1_201408.nc
81374d66a7ffd289bd7d15ab2befb87ea6019422a02501b9a48c361e4801a8ac  ice_conc_sh_ease2-250_cdr-v3p1_201409.nc
4a2a537b95255528665bf64e92aeb8ac05ff3e2c8fa413503f7ac867e8a14db2  ice_conc_sh_ease2-250_cdr-v3p1_201410.nc
8502641f15374dded96a574c090603fb95341812dca76028a53de77f29e9f138  ice_conc_sh_ease2-250_cdr-v3p1_201411.nc
e0ec1b0fdf6b1eb560f58aaf00589b4877ee75a979aa8390e055c7c17b0e864a  ice_conc_sh_ease2-250_cdr-v3p1_201412.nc
311843a241667e10559177129a0a01f195f4fe06989994284c961d8b307bc831  ice_conc_sh_ease2-250_cdr-v3p1_201501.nc
b9d9b693bfbc597d0a96e32dbc50a6eb7000762fe034ffcd615b691f34f9a675  ice_conc_sh_ease2-250_cdr-v3p1_201502.nc
18cb08e02ce6a9fb581db5c1f3a40c7c05848c2985dfdce5cf25c61a8861a9f0  ice_conc_sh_ease2-250_cdr-v3p1_201503.nc
4fec9f05df84b069cd02bfe3242c0f7cc9f233fec99869b1038d94bd24519590  ice_conc_sh_ease2-250_cdr-v3p1_201504.nc
18617fdd08d8bcab9250fa39baaff67b55e42a5544f47b41da708c35b419e981  ice_conc_sh_ease2-250_cdr-v3p1_201505.nc
6b7261f76f5eb46aa53ee3d2591ba46eaf3199a441336d80f86c9bc3dc37681c  ice_conc_sh_ease2-250_cdr-v3p1_201506.nc
fc71225a253f59679df58429374b5ef29704cddfb555cc0c5156ed92fcf680a3  ice_conc_sh_ease2-250_cdr-v3p1_201507.nc
5fa0ec200d2914dc3328a82eebc9e90938b4a92df540e616965caa5f6b6c660a  ice_conc_sh_ease2-250_cdr-v3p1_201508.nc
e7c4fb20ac0a962daaab6834be42ab9ae5fcbee8d6c0e90711503348b3d6b735  ice_conc_sh_ease2-250_cdr-v3p1_201509.nc
c77d38fd9ce35cc9110893d98ea029b1f2f11e7944e95629d542684f8bad2de1  ice_conc_sh_ease2-250_cdr-v3p1_201510.nc
307b70e2b8aeb6154c8fb0fd8f3b9652c415105c18f5d525ddf45beaa58218d7  ice_conc_sh_ease2-250_cdr-v3p1_201511.nc
17614a918cef9c93467b8b91513ed22ec5b5d9a97c76832e66e5fcad70e1dbdc  ice_conc_sh_ease2-250_cdr-v3p1_201512.nc
8392ad9b8b9a7e15a78596556092f29aac356cbb48761b727550c2d014a37cc6  ice_conc_sh_ease2-250_cdr-v3p1_201601.nc
4acf6e9b97e1387becd2648721a9b2a2d2ad7a1924f6cae5513be9c6073beeeb  ice_conc_sh_ease2-250_cdr-v3p1_201602.nc
ee8d44a2769e73b594739c142c9e94ea6c1676598b626152623b69a7289bd780  ice_conc_sh_ease2-250_cdr-v3p1_201603.nc
bb12e43028bf28810e7a089e1697db65b089441a994e19d3b2018e2baa003f67  ice_conc_sh_ease2-250_cdr-v3p1_201604.nc
f6f42ade38aabc3d04d8f229b09c74458471ce34fd34d917abeb777a75430896  ice_conc_sh_ease2-250_cdr-v3p1_201605.nc
1403594b55965cbf871118245021818645fd8ce305c4f25e83300a27fa4b4e37  ice_conc_sh_ease2-250_cdr-v3p1_201606.nc
bafc9574f7fd125e60153b3c729fd51f678c629adfd7f39d34d9b646a828a266  ice_conc_sh_ease2-250_cdr-v3p1_201607.nc
65cfe3f0d643e532d638d3fe82ede3e4717f40cc221cbec32eaaa9446be6db1f  ice_conc_sh_ease2-250_cdr-v3p1_201608.nc
f8186c1a8ac07ceaa91051ea16df27d0da181bf5deef4896f865537f416c894a  ice_conc_sh_ease2-250_cdr-v3p1_201609.nc
e1a81303abeec5e0ad09cb2416785b85ede0bc748c86b7e09887e99a0038be10  ice_conc_sh_ease2-250_cdr-v3p1_201610.nc
2a916264d0f464b0f931a5c52b30f0b6415cb27fb7dc3909a528e856d440758a  ice_conc_sh_ease2-250_cdr-v3p1_201611.nc
98f251fb8e6d72b0c5e5cf1e4bcc2989a59038b1f33881f6b82c4a1a8fa7d926  ice_conc_sh_ease2-250_cdr-v3p1_201612.nc
6bed780c19dcd620c4894a73d616de432a6e26901e72eeb52f256d4730574029  ice_conc_sh_ease2-250_cdr-v3p1_201701.nc
2bab0e8fedaf39e409a369157bb20a6170cdf5da57321730ff9ad538bdb9e981  ice_conc_sh_ease2-250_cdr-v3p1_201702.nc
02a572fcdda66d9f9978f2cb30e9cff9e25fdb77996e31dd416e354ffec9dc1c  ice_conc_sh_ease2-250_cdr-v3p1_201703.nc
11710714c97ba5e6663002b1cb28cf3461671953f1455a0e03d5097ad0f61a85  ice_conc_sh_ease2-250_cdr-v3p1_201704.nc
bdbfa139fcc1834f930a19aeb009fca12c1b2b577156a69f86fc3c6d88cda391  ice_conc_sh_ease2-250_cdr-v3p1_201705.nc
dd5f9859d7e376890364cac754c3fdd8674537969737ac20a918b496b8d8e3b0  ice_conc_sh_ease2-250_cdr-v3p1_201706.nc
67c90a6288e43ddf792f1f68a3046d35a68e0db5a2fc4009376af9eddb3280c8  ice_conc_sh_ease2-250_cdr-v3p1_201707.nc
296b56f9a71a0c4726448ff586f90434527922c4885b2123f5957190db0dfc2b  ice_conc_sh_ease2-250_cdr-v3p1_201708.nc
c036b06747aed0e40a552e3c8e933d886bb879ef28487982d2bce97138922bf6  ice_conc_sh_ease2-250_cdr-v3p1_201709.nc
8d5857924950d8668d8a7c773efec1efd4311b30e4edbed340e36fae85ef8ba5  ice_conc_sh_ease2-250_cdr-v3p1_201710.nc
3eda221912f3e02e460b5a93833dc681fbb9efebe3fe3a3b298ba315db68a452  ice_conc_sh_ease2-250_cdr-v3p1_201711.nc
a9a04d5e77cfc1fdeec0107685297a8d6539faf799960306e73d7c412e64514d  ice_conc_sh_ease2-250_cdr-v3p1_201712.nc
7a8551093142c849bf9b530fbf5544976f2c6c1ab0ecb6d2b2ce013697a508ef  ice_conc_sh_ease2-250_cdr-v3p1_201801.nc
a5c25d8e5a47ee346701bf8fdc6c2bcdf32948b6d253c9e682eb0ba69470667c  ice_conc_sh_ease2-250_cdr-v3p1_201802.nc
800900ecbbf00671a64399009b0f15cb032f80d6fb2960c625c31b2b999cdc49  ice_conc_sh_ease2-250_cdr-v3p1_201803.nc
f5ad94c29ef5a79e44c9aae1d3ef7d45a06836cd56d68f453ca813af7a09e503  ice_conc_sh_ease2-250_cdr-v3p1_201804.nc
3582d41c586e88534bddca674dfd1574caeae4c347cadcfe899e421cab0b219f  ice_conc_sh_ease2-250_cdr-v3p1_201805.nc
bcb04706335471a15c4ee3776af7987afbd97064d0933ac0a043a1cf29fe7185  ice_conc_sh_ease2-250_cdr-v3p1_201806.nc
00c8e134a680583a4bcd5fc43b435738110aa5d83e10ac0d38b61ea0b0b36041  ice_conc_sh_ease2-250_cdr-v3p1_201807.nc
502e25f33964a2712deb3302fa7748359d88781c11e43ab4be72a566dd165944  ice_conc_sh_ease2-250_cdr-v3p1_201808.nc
620d242d86b0db26fe81a2b7c41f136d91ec0a357be32fa2d89b9ccf666e7eeb  ice_conc_sh_ease2-250_cdr-v3p1_201809.nc
246eab22e7b206eb546bfc6b40dd83af487e87bdbbe9dce80b53d99559289298  ice_conc_sh_ease2-250_cdr-v3p1_201810.nc
2dfd09d671c737eec8ebe9b5855eb6127f3ce1c46aecc94ee673320456b8cef3  ice_conc_sh_ease2-250_cdr-v3p1_201811.nc
b6f66229949e69c2d973ffb6024b49f3fd772a093a61be32a87d2fd0cd956e82  ice_conc_sh_ease2-250_cdr-v3p1_201812.nc
3d5c17e22327e0314b42beac22acb281124e094523d191c626655966c9444aa7  ice_conc_sh_ease2-250_cdr-v3p1_201901.nc
21859306a32ae2acae2cd584f7357883d09164f120ae55520dde755fc3fa7abd  ice_conc_sh_ease2-250_cdr-v3p1_201902.nc
c014466024b0dfb75f2ebb84644427a5aeb0734712a8b886c6ab0cae6bbdfe92  ice_conc_sh_ease2-250_cdr-v3p1_201903.nc
25b8501977cb33f7a9d6d421e58b5a949efc04907654281097799d33258062c5  ice_conc_sh_ease2-250_cdr-v3p1_201904.nc
9400d6e3ff5cefc7c20be1482fe1f74dc9a8c25743dbbbbfc2b317f4099683d2  ice_conc_sh_ease2-250_cdr-v3p1_201905.nc
a996fac444d3d8ab4376f48c077b8d169b78744c8ee47056cf7f74718eb40f00  ice_conc_sh_ease2-250_cdr-v3p1_201906.nc
4a12415685233e8fff2f4cdb1da8afac56826d43786f582ef1f92f06f28827cb  ice_conc_sh_ease2-250_cdr-v3p1_201907.nc
7294019cbde9cdd98c48912d7a1e03a7a2d99d206feca501ae23d12ec98b1c66  ice_conc_sh_ease2-250_cdr-v3p1_201908.nc
9020601e0a22cda0949a3b5cf0be5c28739a783d5dee7e2e7e8aed3911002587  ice_conc_sh_ease2-250_cdr-v3p1_201909.nc
bb5cd3550588015a287226190ced181e6839f2073cc931b9c0c0c765c43c6ce7  ice_conc_sh_ease2-250_cdr-v3p1_201910.nc
5574e85e55b3a6e34264278b6454b2a45c7c3755e1520258232208b56fd63902  ice_conc_sh_ease2-250_cdr-v3p1_201911.nc
eed381b1a79ea8f8a5a9a29f67592f1b0d5dd9d68e2d7f9d346a402f6891f0da  ice_conc_sh_ease2-250_cdr-v3p1_201912.nc
e732dd23449710f26118e47e85b103f9e0830368110b8536248cded071e23aff  ice_conc_sh_ease2-250_cdr-v3p1_202001.nc
af25d051efe4978cb9040ac8f5d259dd29fda199da0c6dff0fc552e912f24cee  ice_conc_sh_ease2-250_cdr-v3p1_202002.nc
c92e578a8c17f127beb5f9d8109b36eebbc891a2de5d9c77ff3ec609e0ae3b2a  ice_conc_sh_ease2-250_cdr-v3p1_202003.nc
35b07430eac1b8c3fb568a0656303c8d1129f9738cae4f95c956bc67a86149b5  ice_conc_sh_ease2-250_cdr-v3p1_202004.nc
f4b1532bd231a59be8465a4a911fa08b3ebfb271cce86e85b498ce001352e56b  ice_conc_sh_ease2-250_cdr-v3p1_202005.nc
398e87e5f83da6c3ade9833883f250c328351a37dc53071b47a18f81d9697a17  ice_conc_sh_ease2-250_cdr-v3p1_202006.nc
c20364bb3b0c66f75d35492da1f4fd676b9e03e2135bb89841ab96bedb3916fa  ice_conc_sh_ease2-250_cdr-v3p1_202007.nc
80a72cfcc03826c8ede9d951723d9b1a1edc4a1e5c6bb64532ff724fe3f54d7c  ice_conc_sh_ease2-250_cdr-v3p1_202008.nc
a6ba8916667877511931ffcdbe25a4d606823318c8ae78da0a65338fff7db371  ice_conc_sh_ease2-250_cdr-v3p1_202009.nc
77c4dd8fbd649b0fa69803592240e6c608db427ac1a2d055d88b34c448fd2719  ice_conc_sh_ease2-250_cdr-v3p1_202010.nc
a54a8d171b1f41e0ae371ab69726fdb9b838107c1218b214344fd72f4f821501  ice_conc_sh_ease2-250_cdr-v3p1_202011.nc
7844570ad9b516c193e978bc6bfa6a20095f94ca7f30dd31113433f60871138e  ice_conc_sh_ease2-250_cdr-v3p1_202012.nc
```
