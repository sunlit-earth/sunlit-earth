# Attributions

Sunlit Earth is built on public data, freely licensed imagery and open source libraries. This file is the summary, and it is what the program's About window shows under Attributions. Two files in the repository go further: `textures/PROVENANCE.md` is the full record for the imagery, with source IDs, file hashes and download dates, and `assets/THIRD-PARTY-LICENSES.md` carries the license texts and copyright notices of every crate in the dependency tree. That last one is also the only one of the three that travels beside the binary, where it sits at the top level of the release archive.

## Clouds

Contains modified EUMETSAT data.

The live cloud composite comes from [clouds.matteason.co.uk](https://clouds.matteason.co.uk/) by Matt Eason, whose code and images are released under CC0. The cloud data behind those images is provided by EUMETSAT, and their [data licensing](https://www.eumetsat.int/eumetsat-data-licensing) asks for the notice above. Sunlit Earth downloads the composite and reprojects it onto the globe, so what it draws is modified data.

## Earth

The day side is NASA's Blue Marble Next Generation, the May 2004 topography, produced by Reto Stöckli of the NASA Earth Observatory at NASA Goddard Space Flight Center from Terra MODIS observations. Credit: [NASA Earth Observatory](https://earthobservatory.nasa.gov/).

The night side is Black Marble 2016. Credit: [NASA Earth Observatory](https://earthobservatory.nasa.gov/) images by Joshua Stevens, using Suomi NPP VIIRS data from Miguel Román, NASA GSFC, with the imagery from the NASA and NOAA Suomi NPP satellite.

## Moon

The lunar surface is the [CGI Moon Kit](https://svs.gsfc.nasa.gov/4720), SVS ID 4720, by NASA's Scientific Visualization Studio. Visualizer Ernie Wright (USRA), scientist Noah Petro (NASA/GSFC).

The map underneath it is the LROC WAC natural color Hapke normalized mosaic built at Arizona State University, covering 70N to 70S, with the latitudes beyond that filled from the LOLA laser altimeter's albedo map.

## Milky Way

The diffuse band is [Deep Star Maps 2020](https://svs.gsfc.nasa.gov/4851), SVS ID 4851, credited to NASA/GSFC/SVS. Visualizer Ernie Wright (USRA). The layer is Gaia DR2 flux for stars fainter than magnitude 11.5, credited to ESA/Gaia/DPAC, and it deliberately excludes the bright stars that are drawn individually.

## Stars

The star field is derived from the [HYG Database v4.4](https://codeberg.org/astronexus/hyg) by David Nash, licensed under [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/). The catalog was modified: it was cut to the 15,597 stars of magnitude 7 and brighter, the Sun was removed, positions were propagated from epoch 2000 to 2026 using the catalog's own proper motions, and the colors were converted from B minus V color indices to RGB.

HYG itself is a merge of three centuries of astrometry: ESA's Hipparcos catalog, the Yale Bright Star Catalogue, and the Gliese catalogue of nearby stars. The proper names it carries are those adopted by the IAU Working Group on Star Names.

The star blob inside the program is an adaptation of CC BY-SA 4.0 material shipped in a GPL-3.0-or-later program, which Creative Commons made possible in October 2015 by declaring [BY-SA 4.0 one way compatible with GPLv3](https://creativecommons.org/2015/10/08/cc-by-sa-4-0-now-one-way-compatible-with-gplv3/). Contributions to the adaptation are licensed under the GPLv3, so a redistributor looks to the GPLv3 alone to satisfy BY-SA's attribution and ShareAlike conditions.

## Astronomy

Positions of the Sun and the Moon come from [Astronomy Engine](https://github.com/cosinekitty/astronomy) by Don Cross, MIT licensed, through the [astronomy-engine-bindings](https://crates.io/crates/astronomy-engine-bindings) crate. Its copyright notice is in `THIRD-PARTY-LICENSES.md`.

## Libraries

- [Slint](https://slint.dev/) draws the window and the tray, used here under its GPLv3 option.
- [wgpu](https://wgpu.rs/) is the graphics abstraction the renderer runs on, under Apache-2.0 or MIT.
- [jxl-oxide](https://github.com/tirr-c/jxl-oxide) decodes the four JPEG XL textures, and behind it stand [libjxl](https://github.com/libjxl/libjxl) and the JPEG XL committee for the format itself.
- The [Rust](https://www.rust-lang.org/) project, for the language and the toolchain.

Every other crate in the dependency tree is listed in the program's About window under Third-party, and its license text and copyright notice are in `THIRD-PARTY-LICENSES.md`.

## Not shipped, and therefore not credited

The NASA insignia, logotype and identifiers are not in the public domain and are not used here. NASA is credited in words only, and nothing in this program implies NASA endorsement.

No font is embedded. Every supported platform resolves its own system fonts.
