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

## world.topo.200405.jxl, world.topo.200405.original.jxl, BlackMarble_2016.jxl

The Earth's day and night surfaces, 8192 wide, from NASA's Blue Marble Next
Generation (May 2004 topography) and Black Marble (2016) imagery. These predate
this file and their exact download URLs and preparation steps were not recorded
at the time; the About window's credit lines are what ships with them. The
`.original` file is the day map before whatever adjustment produced the one the
app loads, and nothing reads it.
