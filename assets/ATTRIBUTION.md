# Attributions

Sunlit Earth stands on public data, freely licensed imagery and open source libraries. Fuller records live in `textures/PROVENANCE.md` for the imagery and `assets/THIRD-PARTY-LICENSES.md` for every crate's license text and copyright notice.

## Imagery

- Day side: NASA Blue Marble Next Generation, May 2004 topography. Credit [NASA Earth Observatory](https://earthobservatory.nasa.gov/), produced by Reto Stöckli, NASA Goddard Space Flight Center, from Terra MODIS.
- Night side: NASA Black Marble 2016. Credit [NASA Earth Observatory](https://earthobservatory.nasa.gov/) images by Joshua Stevens, using Suomi NPP VIIRS data from Miguel Román, NASA GSFC.
- Moon: [CGI Moon Kit](https://svs.gsfc.nasa.gov/4720) by NASA's Scientific Visualization Studio. Visualizer Ernie Wright (USRA), scientist Noah Petro (NASA/GSFC), over the LROC WAC mosaic built at Arizona State University with LOLA altimetry beyond 70 degrees.
- Milky Way: [Deep Star Maps 2020](https://svs.gsfc.nasa.gov/4851), credit NASA/GSFC/SVS, visualizer Ernie Wright (USRA), from Gaia DR2, credit ESA/Gaia/DPAC.
- Clouds: [clouds.matteason.co.uk](https://clouds.matteason.co.uk/) by Matt Eason. Contains modified EUMETSAT data, per EUMETSAT's [data licensing](https://www.eumetsat.int/eumetsat-data-licensing).

## Stars

- Derived from the [HYG Database v4.4](https://codeberg.org/astronexus/hyg) by David Nash, licensed [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/). HYG merges the Hipparcos, Yale Bright Star and Gliese catalogues, and carries the proper names adopted by the IAU Working Group on Star Names.
- The catalog was modified: cut to the 15,597 stars of magnitude 7 and brighter, the Sun removed, positions propagated from epoch 2000 to 2026 with the catalog's own proper motions, and colors converted from B minus V indices to RGB.
- The result is a CC BY-SA 4.0 adaptation shipped inside a GPL-3.0-or-later program, which Creative Commons permits by declaring [BY-SA 4.0 one way compatible with GPLv3](https://creativecommons.org/2015/10/08/cc-by-sa-4-0-now-one-way-compatible-with-gplv3/). A redistributor looks to the GPLv3 alone to satisfy BY-SA's attribution and ShareAlike conditions.

## Software

- [Astronomy Engine](https://github.com/cosinekitty/astronomy) by Don Cross, for the positions of the Sun and the Moon.
- [Slint](https://slint.dev/), for the window and the tray icon.
- [wgpu](https://wgpu.rs/), for the renderer.
- [jxl-oxide](https://github.com/tirr-c/jxl-oxide), with [libjxl](https://github.com/libjxl/libjxl) and the JPEG XL committee behind it, for the textures.
- The [Rust](https://www.rust-lang.org/) project, for the language and the toolchain.

Every crate in the dependency tree is listed under Third-party, and each one's license text and copyright notice is in `assets/THIRD-PARTY-LICENSES.md`.

NASA is credited in words only. Its insignia and logotype are not used here, and nothing in this program implies NASA endorsement.
