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

Leads and polynyas are open or thinly frozen water inside the pack and look near black to dark gray-blue against it [U].

## 5. Techniques for a coarse field

None of these comes from a source; they are the reasoning the plan builds on [U].

- A field at 25 km drawn by bilinear filtering shows its cells as diamonds wherever it is thresholded. A smooth reconstruction (a cubic B-spline, which costs four bilinear taps) hides them.
- Thresholding the smoothed concentration against a fixed, uniformly distributed noise field turns it into a pattern of floes and leads whose area fraction equals the concentration on average, so the picture keeps the data's extent while gaining detail the data does not have. The noise is fixed in space, so the pattern stays put and only the ice edge moves as the concentration changes.
- Interpolating between two dates must happen on the concentration before the threshold; blending two thresholded pictures cross-fades instead of moving the edge.
- Ice should hide the ocean's glint and Fresnel term, which the water mask drives today.
- Passive microwave data is unreliable at coasts, both as false ice and as cells marked land, so the fine coastline should come from the app's own water mask and the coarse field should be extended a little past its own land mask before it is sampled.

## 6. Second round: real imagery instead of procedural floes (2026-10-11)

The floes the shader draws from the 25 km field did not convince: their detail follows the pixel footprint, so the pack changes shape with zoom and camera angle, and nothing procedural approaches what satellite pictures of sea ice show. This round asked whether real data could supply one texture per month per pole instead, daily changes not being needed. The answer is no ready-made source; the findings follow.

### 6.1 True-color imagery

- NASA GIBS serves the daily MODIS Corrected Reflectance true color of Terra (from 2000-02-24) and Aqua (from 2002-07-03) at 250 m in the polar projections EPSG:3413 and EPSG:3031 [V, capabilities documents and test images]. It shows pack ice, leads and the marginal ice zone, with nothing masked over the ocean [V, Arctic 2020-03-15]. VIIRS true color is the same kind of data from 2015-11 (Suomi NPP) and 2018 (NOAA-20) [V].
- Three things stand in the way [V, test images]. Polar night is black fill, roughly November to February in the Arctic and May to August in the Antarctic, and a disk of about 2 degrees around the pole has no data in any month. Any single day is mostly cloud. In true color, cloud and ice are both white, so the darkest-pixel or median compositing that clears clouds over the open ocean does not clear them over ice.
- GIBS also serves the MODIS band 7-2-1 false color (`MODIS_Terra_CorrectedReflectance_Bands721`), in which ice is cyan, water dark and cloud white, which is the usual way to tell them apart [V, layer present in the EPSG:3413 capabilities; the colors are the documented convention, not measured]. A composite would classify every pixel of every day with it and keep the median true color of the clear ones. Whether a month of several years leaves enough clear days, particularly in the Arctic summer and over the Southern Ocean, and whether thin cloud and fog are caught, is unknown [U]. Floes drift during a composite window and would smear [U].
- The polar night gaps matter less than they seem: where the satellites see no sun, the renderer's Sun does not shine either, so the day texture there shows only in twilight, in the Day surface mode and under a date override [U, reasoned from the shading].
- Blue Marble NG has no ice: NASA's page says "Deep oceans are not included in the source data" [V], and the earlier research measured a uniform ocean, a static Antarctica (one Landsat mosaic in every month) and a dark fill north of about 78 N in the winter months. That dark fill is NASA's handling of the polar night: none. No variant of Blue Marble NG with sea ice was found [V, by absence].
- EOX Sentinel-2 cloudless fills the oceans with a bathymetry render and is CC BY-NC-SA from 2018 on [V]; the MODIS Mosaic of Antarctica (MOA2009) is one austral summer of the ice sheet [S]. Neither carries seasonal sea ice.

Links: GIBS API https://nasa-gibs.github.io/gibs-api-docs/, layers https://gibs.earthdata.nasa.gov/wmts/epsg3413/best/1.0.0/WMTSCapabilities.xml and https://gibs.earthdata.nasa.gov/wmts/epsg3031/best/1.0.0/WMTSCapabilities.xml, viewer https://worldview.earthdata.nasa.gov/, Blue Marble NG https://science.nasa.gov/earth/earth-observatory/blue-marble-next-generation, EOX https://cloudless.eox.at/, MOA2009 https://nsidc.org/data/nsidc-0593/.

### 6.2 Data that works through the polar night

All of it is a mask, a concentration or a temperature field, not a picture of ice; only radar shows real floe texture.

| Source | Content | Grid and period | Polar night | Terms |
|---|---|---|---|---|
| NOAA/NSIDC IMS, G02156 | Daily ice and snow mask drawn by analysts [V] | 1 km and 4 km, Northern Hemisphere only, 1997 to the present [V] | Yes [U, analyst-drawn from several sensors] | Citation required, redistribution not stated [V] |
| MODIS MOD29/MYD29, MOD29P1D/P1N | Ice extent (daylight only) and ice surface temperature, which shows leads as warm streaks [V] | 1 km, both poles, 2000 to the present, daily [V] | Temperature only [V] | NASA Earthdata, login to download [V] |
| VIIRS VNP29, VNP30 | Ice cover (daylight only) at 375 m, surface temperature at 750 m [V] | Both poles, start year unverified | Temperature only [V] | NASA Earthdata [V] |
| Bremen AMSR-E/AMSR2 ASI | Concentration, smooth [V] | Both poles, 2002 to 2011 and 2012 on, daily; the 3.125 km grid is from section 1 | Yes | Citation required, no license stated [V] |
| Bremen merged MODIS and AMSR2 | Concentration at 1 km with leads visible [V] | Arctic only, October to May, from 2019 [V] | Yes | Citation and acknowledgement, no license stated [V] |
| Sentinel-1 SAR | Radar backscatter with real floes and leads; Copernicus Marine derives a 1 km Arctic concentration and type product from December 2020 [V] | No pole-wide monthly mosaic of backscatter exists; one would be built from EW scenes [V, by absence] | Yes | Free and open with the credit "Copernicus Sentinel data [year]" [V] |
| US National Ice Center charts, G10013 | Weekly charts of concentration, stage and floe form, polygon edges [V] | Both poles, 2003 to the present, with 5 and 10 year climatologies [V] | Yes | Citation required [V] |

Links: IMS https://nsidc.org/data/g02156/versions/1, MOD29 https://nsidc.org/data/mod29/versions/61, VNP29 https://www.earthdata.nasa.gov/data/catalog/nsidc-cprd-vnp29-2-0, Bremen AMSR2 https://seaice.uni-bremen.de/sea-ice-concentration/amsre-amsr2/, Bremen MODIS-AMSR2 https://seaice.uni-bremen.de/sea-ice-concentration/modis-amsr2/, Copernicus Marine https://data.marine.copernicus.eu/product/SEAICE_ARC_PHY_AUTO_L4_NRT_011_015/, Sentinel legal notice https://sentinel.esa.int/documents/247904/690755/Sentinel_Data_Legal_Notice, NIC charts https://nsidc.org/data/G10013.

### 6.3 How others got ice into a globe texture

- NASA's Scientific Visualization Studio takes where the ice is from AMSR2 concentration and its look from the 89 GHz brightness temperature at about 6 to 10 km, which carries surface detail; older pages did the same with AMSR-E (https://svs.gsfc.nasa.gov/5064, https://svs.gsfc.nasa.gov/3372) [S]. This extends section 3.
- Natural Earth III offers a static Arctic and Antarctic sea ice mask, credited to "NASA Blue Marble, obtained at The Celestia Motherlode", and one texture variant without Arctic ice (https://shadedrelief.com/natural3/pages/extra.html) [S]. It is not monthly and its origin is unclear.
- The Celestia community made monthly ice add-ons by hand from NSIDC data, of unclear license (https://celestiaproject.space/forum/viewtopic.php?p=75191) [S].
- Space Engine, Flight Simulator, Google Earth, Cesium, Outerra and Universe Sandbox were not researched this round.

### 6.4 Where it leaves the feature

Real imagery means a cloud-cleared MODIS composite built here, with the polar night filled from neighboring months, and an unknown amount of residual cloud. The data-driven way is NASA's: concentration places the ice, and its look comes from the 89 GHz brightness or a fixed ice color, at one level of detail that does not follow the pixel footprint. Neither gives photographic ice for every month at both poles.

## 7. Third round: the ice as part of the day faces (2026-10-11)

The first prototype, the shader of [the first plan](2026-10-10-sea-ice-plan.md) on `feature/sea-ice`, drew floes whose detail followed the pixel footprint, so the pack changed shape with zoom and camera angle, and the Voronoi cells did not look like ice. This round asked what it takes to bake the ice into the monthly day faces instead, so that the tiles and mip chains the land already has carry the level of detail. [The second plan](2026-10-11-baked-sea-ice-plan.md) makes the decisions. Everything here was read in the tree unless marked otherwise.

### 7.1 What the cube gives the ice

- The shipped day faces are 2048 px a side, equi-angular, so a texel is about 4.9 km at the face's center (a quarter of the circumference, 10,007 km, over 2048) and close to that everywhere on the face. The `cube` command works at 5400 a face, about 1.85 km a texel, and averages down (`textures/PROVENANCE.md`, Preparation). A day pack holds the month's 128 px tiles at the 1024 and 2048 levels in BC7 and a 512 px floor with its full mip chain (`assets/tiles/mod.rs`).
- The cap of the first plan, the polar Lambert azimuthal equal-area plane out to `RHO_MAX = 2 sin(27.5 degrees)`, reaches latitude 35 degrees on its axes, below the Bohai Sea's ice at 38 to 40 N, and its geometry, the reprojection from the EASE2 grid through PROJ and the land extension are in `tools/texture-pipeline/src/texture_pipeline/sea_ice.py` on `feature/sea-ice`, with tests. A cap of 4096 px is 2.87 km a texel over its 11,770 km, one of 8192 px 1.44 km.
- The month in force is cut at mid-month and the floor and tiles switch with it (`architecture.md`, Months), so ice in the faces moves once a month, with the snow line.
- The flatten invariant: the day faces were flattened toward the fill (10, 30, 60) in proportion to the mask, so a texel the mask calls open water (255) is the fill to within one level before the lossy encode and carries only codec noise after it (`PROVENANCE.md`, What a mask texel means). Ice composited only where the mask says water would make a mask-255 texel either the fill or ice, and nothing else, which a shader can tell apart by brightness.

### 7.2 What stands in the way

- The pack builder flags a day tile constant ocean, storing nothing, when its whole footprint on the finest mask, gutter included, is open water, by the mask alone; the shader then paints the pack's ocean color for the tile without reading texels (`assets/tiles/build.rs`, `renderer/tiles.rs`, `PAGE_OCEAN` in `sphere.wgsl`). A night tile is flagged only where its texels also lie within `NIGHT_OCEAN_TOLERANCE`, 4 levels, of the night's ocean color, so lights at sea keep their tiles. Ice baked over open water would be flattened away by the day rule as it stands.
- The pack's ocean color is the mean of the texels the mask calls open water (`ocean_color` in `build.rs`). With ice in those texels the mean would shift toward white by several levels in the winter months, and the constant tiles would be painted a lighter blue than the floor around them.
- The mask's ice pass keeps what the detector finds on the ocean side of the coastline as land in every month: the Antarctic ice shelves and 0.35 M to 1.27 M source pixels of snow along Arctic coasts (`PROVENANCE.md`, Preparation; roadmap item on the coastal strips). With generated ice on the water side, the Arctic strips would show the Blue Marble's own pixels inside the ice.
- The glint and the Fresnel term are driven by the water mask alone (`fs_main`, `if water > 0.0`), so without something more the sun would glint on frozen sea. The night-alone bind group holds no day cube.
- The golden suite draws the Earth fixture, `crates/sunlit-core/tests/fixtures/earth/`, 256 px faces of July 2004 laid out for every month by `test_support::write_earth_fixture`, so a golden of the Arctic in March needs a March set in the fixture. The pipeline's own cube fixture is 16 px faces under `tools/texture-pipeline/tests/fixtures/cube/`, compared against a fresh bake.
- The cloud haze's land weight reads the water mask at a coarse level and gives frozen sea the ocean's haze removal; the cloud haze plan left whether a winter Arctic keeps a veil as an open item (`plans/2026-10-10-cloud-haze-plan.md`, Relation to the sea ice plan).

### 7.3 Finer and textured sources, for later

Found through search result snippets only [S], not read on the product pages:

- NSIDC AU_SI12, DOI 10.5067/RA1MIJOYPK3P: AMSR-E/AMSR2 unified daily 12.5 km polar stereographic grids of brightness temperatures at 18.7, 23.8, 36.5 and 89.0 GHz, sea ice concentration and snow depth, both poles, HDF-EOS5, free Earthdata login. https://nsidc.org/data/AU_SI12
- NSIDC AU_SI6, DOI 10.5067/NX1R09ORNOZN: the 89 GHz brightness temperatures alone at 6.25 km, both polarizations, EPSG:3411 and 3412, AMSR-E 2002-06-01 to 2011-10-04 and AMSR2 from 2012-07-02, HDF-EOS5, Earthdata login. The intermediate product the 12.5 and 25 km sea ice products are made from. https://nsidc.org/data/AU_SI6/versions/1
- EUMETSAT OSI SAF OSI-408-a: daily AMSR2 sea ice concentration on a 10 km polar stereographic grid, both poles, CC BY 4.0, from 2017 (OSI-408 to April 2023, OSI-408-a since); the 19 GHz footprint it uses is about 18 km. A monthly mean would have to be computed from the daily files. https://osi-saf.eumetsat.int/products/osi-408-a

Averaging either brightness temperature or a finer concentration over years smears the texture the way it smears the edge, so real texture would have to come from one representative month of one year per calendar month, laid under the climatological envelope. Not pursued in the second plan.
