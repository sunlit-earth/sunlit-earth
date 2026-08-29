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
Earth's.

## milkyway_2020_4k.jxl

The diffuse Milky Way, 4096x2048, as a full-sky equirectangular panorama in
equatorial J2000 coordinates.

- Source: NASA Scientific Visualization Studio, [Deep Star Maps 2020](https://svs.gsfc.nasa.gov/4851)
  (SVS ID 4851, released 2020-09-09). Visualizer Ernie Wright (USRA).
- File taken: `milkyway_2020_4k.exr`, the celestial-coordinate variant at
  4096x2048, sha256
  `2eb802d6e68d170b410f766c7fec07f7518619f6b6708fdc81e9302d93e74fdb`,
  downloaded 2026-08-27 from
  `https://svs.gsfc.nasa.gov/vis/a000000/a004800/a004851/milkyway_2020_4k.exr`.
- Underlying data: Gaia DR2 star fluxes, with the Hipparcos and Tycho bright
  stars deliberately excluded, so this layer is the unresolved starlight and
  nothing that phase A already draws as a sprite. The companion `hiptyc_2020`
  layer holds those bright stars and is not used. Linear half-float, values in
  0 to 1 with the maximum reached by about sixteen pixels.
- The celestial variant rather than the galactic one: it needs no galactic
  rotation at runtime, since the renderer already has the J2000 equatorial to
  world rotation that everything else in the sky goes through.
- License: US Government work, public domain. The SVS asks for credit to
  "NASA/GSFC/SVS", and Gaia DR2 for "ESA/Gaia/DPAC"; the About window carries
  both.

Preparation, on a Windows host with ImageMagick 7 and libjxl 0.11.1:

```
magick milkyway_2020_4k.exr -set colorspace RGB -colorspace sRGB -depth 8 milkyway_srgb.png
magick milkyway_srgb.png -quality 100 milkyway_2020_4k.jxl
```

`-set colorspace RGB` declares the EXR's data linear without converting it, and
`-colorspace sRGB` then applies the sRGB transfer curve. That is the whole tone
map: no exposure gain and no artistic curve, because the renderer works in
display values throughout (the surface textures, the clear color and the
additive sky draws are all in that space) and `milky_way_intensity` covers taste
at runtime. Nothing clips, since the source's own maximum is 1.0. The sRGB
curve's linear toe rather than a plain gamma is what keeps the faint sky off
zero: the darkest sky in the source lands a few code values above black instead
of on it.

Quality 100 is lossless, and measured through the app's own `jxl-oxide` path
against `milkyway_srgb.png` it decodes to a mean channel difference of 0.0000
with no pixel off at all, at 9.9 MB. Quality 99 is 0.92 of 255 with a worst
pixel off by 27, at 4.6 MB; 97 is 1.43 and 36 at 3.3 MB; 95 is 1.69 and 36 at
2.7 MB; 90 is 2.91 and 42 at 1.6 MB. Lossless was taken because this asset is
Gaia photon noise over a smooth gradient, which is the content a lossy codec
handles worst, and because it is the one texture that covers the whole frame:
an artifact in it is a property of every pixel of the sky rather than of a
thirty-pixel disc.

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

## world.topo.200405.jxl, world.topo.200405.original.jxl, BlackMarble_2016.jxl

The Earth's day and night surfaces, 8192 wide, from NASA's Blue Marble Next
Generation (May 2004 topography) and Black Marble (2016) imagery. These predate
this file and their exact download URLs and preparation steps were not recorded
at the time; the About window's credit lines are what ships with them. The
`.original` file is the day map before whatever adjustment produced the one the
app loads, and nothing reads it.
