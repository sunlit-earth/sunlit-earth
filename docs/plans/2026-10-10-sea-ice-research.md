# Research: Seasonal Sea Ice (2026-10-10)

Blue Marble Next Generation, which the day surface is baked from, has no sea ice: its ocean pixels are averaged over the year, and the Arctic Ocean is the same flat fill in January and July ([texture overhaul research](2026-09-29-texture-overhaul-research.md), section on the source). What the cube bake keeps as land on the ocean side of the coastline is what the ice pass found (`textures/PROVENANCE.md`): the Antarctic ice shelves, the same in every month, and some coastal snow along Arctic shores. Seasonal sea ice therefore needs a source of its own and a layer of its own. This document collects what was found about both; [the plan](2026-10-10-sea-ice-plan.md) makes the decisions.

Every claim carries how far it was checked. [V] means read on the source page or measured from a downloaded file, with the source named. [S] means it came from a search result snippet only. [U] means unverified or reasoned. The web pages were read through a summarizing fetch tool, so a [V] from a page is that tool's reading of it; the measurements on the OSI SAF files are direct.

## 1. Candidate data sources

| Source | Content | Grid | Access | Terms |
|---|---|---|---|---|
| EUMETSAT OSI SAF OSI-450-a1 | Concentration climate data record, daily and monthly, both hemispheres, 1978-10-25 to 2020-12-31 [V] | EASE2 25 km, Lambert azimuthal equal area [V] | HTTPS through THREDDS, anonymous FTP [V] | CC BY 4.0 [V] |
| NSIDC Sea Ice Index G02135 v4 | Concentration and extent, daily and monthly, both hemispheres, 1979 to the present; monthly 1981-2010 median ice edges as shapefiles [V] | 25 km NSIDC polar stereographic, 304 x 448 north and 316 x 332 south [V] | Plain HTTPS, no login [V] | No license named, citation required [V]; NSIDC's citation policy page answered 404 |
| NOAA/NSIDC CDR G02202 v6 | Concentration, daily and monthly, 1978 to the present [V] | 25 km polar stereographic | HTTPS | Citation required, no license named [V] |
| University of Bremen ASI | Daily concentration at 3.125 km, GeoTIFF [V] | Polar stereographic | HTTPS, no login [V] | No license or rate limit stated [V] |
| NASA GIBS | Colorized map tiles only; the AMSR2 concentration layers end on 2025-09-01 [V] | 4326, 3413, 3031 | No auth [V] | Open [V] |
| NASA NEO | Decommissioned in September 2026 per NASA's redirect page [V] | | | |

URLs: OSI-450-a1 https://osi-saf.eumetsat.int/products/osi-450-a1, G02135 https://nsidc.org/data/g02135/versions/4, G02202 https://nsidc.org/data/g02202/versions/6, Bremen https://seaice.uni-bremen.de/data-archive/.

No public monthly true-color composite of sea ice was found [V via search results]. The original 2002 Blue Marble included sea ice in a single composite (https://science.nasa.gov/resource/blue-marble/) [V], which is no help for seasons. The look of the ice has to come from the renderer, driven by concentration.

OSI-450-a1 was chosen for its explicit license and because its period covers the 2010-2020 window the plan averages over.

## 2. OSI-450-a1 in detail

Identity [V]: product version 3.1, DOI 10.15770/EUM_SAF_OSI_0023, the 2025 corrected release; OSI-450-a, which it supersedes, had a land spill-over bug on 343 days. Inputs for 2010-2020 are SSMIS on F16, F17 and F18, with ERA5 for the weather correction (global attribute `source`).

Continuation [V]: OSI-430-a continues the record from 2021-01 with the same algorithm on the operational SSMIS stream. It is suspended: its last date is 2025-10-16, its last monthly file on the FTP server is 2025-08, and its page points to OSI-438 (AMSR2), a different sensor. The validation report calls OSI-430-a "equally well or only slightly worse" than OSI-450-a over 2019-2020. No statement was found that permits treating the two as one record [U], so a window past 2020 would mix products.

Access [V]:

- Monthly over HTTPS: `https://thredds.met.no/thredds/fileServer/osisaf/met.no/reprocessed/ice/conc_450a1_files/monthly/YYYY/ice_conc_{nh,sh}_ease2-250_cdr-v3p1_YYYYMM.nc`
- Monthly over FTP: `ftp://osisaf.met.no/reprocessed/ice/conc/v3p1/monthly/YYYY/` with the same file names.
- All 264 monthly files of 2010-2020, both hemispheres, exist, 0.89 to 0.94 MB each, about 240 MB in all. THREDDS asks users not to run parallel sessions.

Contents, measured on the north and south files of 2015-03 and 2015-09 [V]:

- Dimensions `time` 1, `xc` 432, `yc` 432. Projection `+proj=laea +lon_0=0 +datum=WGS84 +ellps=WGS84 +lat_0=90` (EPSG:6931) for the north and `lat_0=-90` for the south. `xc` and `yc` are in kilometers, cell centers from -5387.5 to 5387.5, so the edges are at plus or minus 5400 km; `yc` descends, so row 0 is the top. The north grid reaches from 16.62 N at its corners to 89.84 N.
- `ice_conc` is int32 with `scale_factor` 0.01 and `_FillValue` -32767, valid 0 to 10000, in percent. Land is the fill value: 88,847 cells in the north and 33,305 in the south, the same in all four files.
- `status_flag` is int16 with bits 1 land, 2 lake, 4 open water filtered, 8 land spill-over, 16 high t2m, 32 spatial interpolation, 64 temporal interpolation, 128 outside the monthly maximum extent climatology. In the monthly files only the values 0, 1, 4 and 128 occur.
- Ocean outside the monthly maximum extent climatology is 0, not masked (flag 128), with a 150 km buffer in the north and 250 km in the south.
- Monthly means below 10% are set to 0 in `ice_conc` and kept in `raw_ice_conc_values` (Product User Manual section 5.1.5).
- The Arctic pole hole is filled by interpolation; the monthly files carry the filled value without a flag. The south pole cell is land.
- Lakes: only the Caspian Sea and the Great Lakes get values (Product User Manual section 3.7); Ladoga, Aral, Baikal and Vänern are land. The Baltic is sea.
- Global attributes include `license = http://creativecommons.org/licenses/by/4.0/`, `doi` and `references`; there is no citation attribute.

Attribution [V]: the Product User Manual v3.3 gives the form "[Contains modified] EUMETSAT OSI SAF Global sea ice concentration CDR/ICDR", and a derived climatology is a modification. Recommended citation: "EUMETSAT OSI SAF, Global sea ice concentration climate data record 1978-2020 (v3.1, 2025), OSI-450-a1, doi:10.15770/EUM_SAF_OSI_0023".

Known limitations [V, from the Product User Manual and the validation report]: the open-water filter removes true ice up to about 10% concentration, more in the marginal ice zone; melt ponds and thin ice are underestimated in summer; the effective resolution is coarser than the grid (the 19 GHz footprint is 43 x 69 km); land spill-over leaves false ice along some coasts; a 2 m temperature bias in ERA5 over the Great Lakes before 2014 affects 2010-2013.

Documents: Product User Manual v3.3 https://osisaf-hl.met.no/sites/osisaf-hl/files/user_manuals/osisaf_pum_sea-ice-conc-climate-data-record_3.3.pdf, ATBD v3.2 https://osisaf-hl.met.no/sites/osisaf-hl/files/baseline_document/osisaf_atbd_sea-ice-conc-climate-data-record_3.2.pdf, validation report v3.3 https://osisaf-hl.met.no/sites/osisaf-hl/files/validation_reports/osisaf_svr_sea-ice-conc-climate-data-record_3.3.pdf.

## 3. How others draw it

- NASA's Scientific Visualization Studio composites daily AMSR2 concentration over Blue Marble NG: ice opacity follows concentration and only above 15%, the color is a bluish white derived from the 89 GHz brightness temperature, both fields get a three day running minimum over water, and the land is the monthly Blue Marble cross-faded while the ice changes daily (https://svs.gsfc.nasa.gov/4455, https://svs.gsfc.nasa.gov/3404/) [V]. The opacity curve is not published.
- NSIDC counts ice from 15% concentration and describes the false ice that land spill-over leaves at coasts and river mouths (https://nsidc.org/learn/ask-scientist/do-satellites-sometimes-see-ice-where-there-isnt-any) [V].
- Worldview and GIBS show concentration as a colormapped data layer, not as a picture of ice [V].
- No technique documentation was found for OpenSpace, Cesium, xplanet, SpaceEngine, Outerra, Google Earth or Microsoft Flight Simulator [V, by absence]. A Celestia forum thread notes that the Blue Marble monthlies lack polar ice [S].

## 4. What sea ice looks like

Albedo figures, all [S] and to be checked before they are quoted: open water about 0.07; nilas about 0.15, young grey ice about 0.26, grey-white ice about 0.33; first-year ice without snow 0.41 to 0.54 and with snow roughly 0.7 to 0.9 (Brandt et al. 2005). On the Arctic drift of SHEBA the albedo fell from 0.8 to 0.9 in April to about 0.4 on average by late July, with bare ice at 0.6 to 0.7 and melt ponds, which covered up to 50 to 60% of the surface, down to about 0.1. Antarctic sea ice stays snow covered through the year, has few surface melt ponds and melts mostly from below and at its edges, so it does not darken in summer the way the Arctic does.

Leads and polynyas are open or thinly frozen water inside the pack and look near black to dark grey-blue against it [U].

## 5. Techniques for a coarse field

None of these comes from a source; they are the reasoning the plan builds on [U].

- A field at 25 km drawn by bilinear filtering shows its cells as diamonds wherever it is thresholded. A smooth reconstruction (a cubic B-spline, which costs four bilinear taps) hides them.
- Thresholding the smoothed concentration against a fixed, uniformly distributed noise field turns it into a pattern of floes and leads whose area fraction equals the concentration on average, so the picture keeps the data's extent while gaining detail the data does not have. The noise is fixed in space, so the pattern stays put and only the ice edge moves as the concentration changes.
- Interpolating between two dates must happen on the concentration before the threshold; blending two thresholded pictures cross-fades instead of moving the edge.
- Ice should hide the ocean's glint and Fresnel term, which the water mask drives today.
- Passive microwave data is unreliable at coasts, both as false ice and as cells marked land, so the fine coastline should come from the app's own water mask and the coarse field should be extended a little past its own land mask before it is sampled.
