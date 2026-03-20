# Cloud Layer Data Sources

**Date:** 2026-03-19
**Status:** Consolidated evaluation with hands-on test results

This document is the definitive reference for all cloud data sources evaluated for Sunlit Earth. It incorporates findings from the original research (2026-03-17), the GMGSI deep-dive, the additional sources survey, and hands-on testing of matteason, GMGSI LW, GDPS, and MODIS COT.

---

## Evaluation Criteria

| Criterion | Weight | Notes |
|---|---|---|
| Visual realism | High | Continuous opacity, fine detail, filamentary cloud edges |
| Resolution | High | 4096x2048 minimum; 8192x4096 preferred |
| Freshness | Medium | Updated at least daily; sub-6-hour preferred |
| No surface contamination | High | Continents must not ghost through the cloud layer |
| Seam-free | High | No stitching artifacts, swath gaps, or polar holes |
| Processing effort | Medium | Less is better, but a sidecar pipeline is acceptable |
| Reliability | Medium | Should not depend on a single hobby project |

---

## Tested Sources

These sources were downloaded, processed, and visually evaluated.

### matteason/live-cloud-maps

- **URL:** `https://clouds.matteason.co.uk/images/{W}x{H}/clouds.jpg`
- **Resolution:** Up to 8192x4096
- **Update frequency:** Every 3 hours
- **Format:** JPEG greyscale, direct HTTP, no auth
- **License:** CC0 (EUMETSAT attribution appreciated)

Blends three EUMETSAT EUMETView WMS layers (IR 10.8 um, Dust RGB, visible light) with a per-pixel heuristic to isolate clouds. Polar regions (top/bottom 12.5%) are fabricated by mirroring adjacent rows.

**Test result:** Good cloud detail with continuous opacity gradients. Surface contamination is moderate -- IR channel reads cold arid surfaces (Sahara, Arabia, Tibet at night) as cloud-like, and snow/ice/white salt flats pass through the visible filter. Post-processing with levels adjustment (floor=50, ceiling=255, gamma=0.3) significantly reduces the haze. The mirrored polar fill is visible but not terrible at globe scale.

**Reliability risk:** Single-developer hobby project. Actively maintained (last commit March 2026), GitHub Actions runs every 30 minutes. A third-party archiver has been backing up the 8K images since March 2026.

**Verdict:** Best ready-to-use option. Highest resolution and best visual detail of any source tested. Surface contamination is manageable with post-processing.

### NOAA GMGSI Longwave IR

- **S3 bucket:** `noaa-gmgsi-pds` (public, no auth)
- **Path:** `GMGSI_LW/{YYYY}/{MM}/{DD}/{HH}/GLOBCOMPLIR_v3r0_blend_...nc`
- **Resolution:** 3000x5000 native grid (~3-8 km), 2D lat/lon coordinates
- **Update frequency:** Hourly, ~35-45 min latency
- **Format:** NetCDF4, irregular 2D grid requiring binning remap
- **Coverage:** ~73 N/S (polar gap, no fill)
- **License:** NOAA open data, no restrictions

Mosaics five geostationary satellites (GOES-18/19, Meteosat-9/10, Himawari-9) with active blending at seam zones. Four spectral bands available; LW IR is the best for 24/7 cloud rendering.

**Test result:** After remapping to equirectangular (3333x1667 native) and applying levels (floor=60, ceiling=215, gamma=0.7), cloud patterns are clear and recognizable. Surface contamination is comparable to matteason -- warm surfaces appear dark (good) but the overall dynamic range is lower. The image upscales cleanly to 8K with bicubic interpolation. Polar gap is zero-filled (black).

**Key finding:** The VIS band is unusable -- no distinction between clouds, continents, and water.

**Processing complexity:** Requires boto3, netcdf4, numpy for the download/remap pipeline. The irregular 2D grid (each pixel has its own lat/lon) needs binning rather than simple row insertion.

**Verdict:** Viable alternative to matteason with institutional reliability. Hourly updates and authoritative NOAA source. Main downsides: polar gap, lower native resolution after remap, and NetCDF processing complexity.

### ECCC GDPS TCDC (Environment Canada GEM)

- **URL:** `https://dd.weather.gc.ca/today/model_gem_global/15km/grib2/lat_lon/{HH}/{FFF}/`
- **Resolution:** 0.15 degrees, 2400x1201 grid
- **Update frequency:** Every 12 hours, ~4-6h latency
- **Format:** GRIB2, regular lat-lon grid, ~800 KB per file
- **License:** Open Government Licence - Canada

Highest-resolution freely available NWP cloud field on a regular grid. No regridding needed.

**Test result:** Fully global, no seams, no gaps, definitively cloud-only. But only ~10 discrete opacity levels (0%, 24%, 32%, 44%, 76%, 88%, 100% etc.) with sharp boundaries between cloudy and clear regions. Looks more like a weather map than cloud photography.

**Verdict:** Clean and surface-free but visually flat. Best suited as a supplementary mask or polar fill, not a standalone cloud texture.

### GFS TCDC (NOAA)

- **S3 bucket:** `noaa-gfs-bdp-pds` (public, no auth)
- **Resolution:** 0.25 degrees, 1440x721 grid
- **Update frequency:** Every 6 hours, ~3.5h latency
- **Format:** GRIB2, byte-range downloadable via `.idx` files (~1-4 MB per field)

Same visual characteristics as GDPS (discrete opacity levels, sharp edges) but at coarser resolution. Well-documented with existing fetch script.

**Verdict:** Lower resolution than GDPS, same visual limitations. Simpler to access. Good baseline NWP option.

### MODIS COT via GIBS WMS

**Tested and rejected.** GIBS returns colormapped visualizations (252-entry rainbow encoding cloud phase + COT), not raw data. Built a colormap reversal pipeline -- technically recovered COT values but output was very noisy. Aqua+Terra combined coverage: only 52.2% (47.8% swath gaps). The combination of colormap quantization artifacts and massive gaps makes this fundamentally unsuitable.

---

## Evaluated but Not Tested

### Pre-composited Images

| Source | Resolution | Update | Notes | Verdict |
|---|---|---|---|---|
| matteason/daily-cloud-maps | 8192x4096 | Daily | VIIRS true-color * clear sky confidence. Retains surface at partial-confidence pixels. | Worse than live-cloud-maps for cloud-only use |
| NOAA SOS FTP (all 3 products) | 4096x2048 | ~6h | Full-scene Earth visualizations with land/ocean/clouds composited. Not cloud-only. | **Eliminated** |
| Xeric Design | Hourly | $25-115/year | Paid subscription, credential management | Not pursued |
| apollo-ng/cloudmap | Unknown | Unknown | Appears unmaintained (satellite sources from ~2013-2018) | Not pursued |

### Weather Model Data

| Source | Resolution | Update | Grid Format | Notes |
|---|---|---|---|---|
| ECMWF IFS TCC | 0.25 deg | 6h | Regular lat-lon | Same resolution as GFS. 9 km tier planned for 2026 but not yet available. CC-BY-4.0. |
| ECMWF AIFS TCC | 0.25 deg | 6h | Regular lat-lon | ML model, faster latency than IFS. Same resolution. CC-BY-4.0. |
| DWD ICON CLCT | ~13 km | 6h | Icosahedral (needs regridding) | Better than GFS resolution but icosahedral grid adds processing complexity. CC-BY-4.0. |
| Meteo-France ARPEGE | 0.5 deg | 6h | Regular lat-lon | Too coarse. |
| Environment Canada GEM GDPS | 0.15 deg | 12h | Regular lat-lon | **Tested.** Highest-res free NWP on regular grid. See above. |

All NWP sources share the same fundamental visual limitation: ~10 discrete opacity levels with sharp cloud boundaries. Clean and surface-free, but unrealistic without heavy post-processing.

### Satellite Cloud Mask Products

| Source | Resolution | Coverage | Update | Access |
|---|---|---|---|---|
| NASA GIBS VIIRS Clear Sky Confidence | 2 km | Global (swath) | ~3h NRT | Public WMS, no auth. Scientific false-color palette needs colormap inversion. Day/Night are separate layers. |
| GOES ABI Clear Sky Mask (ACMF) | 2 km | Americas + Atlantic | 10 min | AWS S3, NetCDF, ABI Fixed Grid projection |
| EUMETSAT MSG Cloud Mask (CLM) | ~3 km | Europe/Africa/Atlantic | 15 min | EUMETView WMS public; Data Store requires free registration |

All are regional. Global coverage requires compositing multiple sources.

### Cloud Optical Depth Products

Cloud optical depth (COD/COT) is physically superior to binary masks -- it maps directly to visual opacity.

| Source | Resolution | Coverage | Update | Access |
|---|---|---|---|---|
| GOES ABI COD (CODF) | 4 km | Americas | 10 min | AWS S3, NetCDF. Daytime: COD 0.5-50. Nighttime: degraded (1-8 only). |
| MTG FCI OCA (Meteosat-12) | ~2 km | Europe/Africa/IO | 10 min | EUMETSAT Data Store, free registration. Two-layer retrieval. |
| Himawari AHI COT | 5 km | Asia-Pacific | 10 min | JAXA P-Tree, free registration. Daytime only. |
| VIIRS CLDPROP_L2 | 750 m | Global (swath) | ~3h NRT | LAADS DAAC, free. Highest-res but requires compositing many swaths. |

Compositing GOES + MTG + Himawari COD would produce excellent dayside results but nighttime quality degrades significantly and building the compositor is substantial work.

### Global Compositing Approaches

**NASA SatCORPS GCC** -- The ideal source if it becomes publicly available. Merges GOES-16/18/19, Meteosat-9/10/12, Himawari-9, MODIS, and VIIRS onto a unified 3 km global grid with hourly updates and COD (not binary). GEO+LEO variant fills polar gaps. A one-year archive (Jul 2023 - Jun 2024) is available at `satcorps.larc.nasa.gov`. Full public availability expected mid-2026.

**DIY geostationary compositing** via Satpy (GOES + MSG + Himawari cloud masks) or Sanchez CLI produces equivalent quality but requires building the full pipeline. This is what operational weather analysis systems do.

**EUMETSAT EUMETView IR Ring** (`mumi:worldcloudmap_ir108`) -- The raw upstream data behind matteason. Available as WMS at up to 8192x4096, no auth. Same surface contamination as matteason but without the multi-channel filtering.

**NOAA nowCOAST WMS** -- Serves GMGSI VIS (layer 17) and LW IR (layer 21) as PNG via ArcGIS WMS, no auth. Same data as the S3 NetCDF but as pre-rendered tiles. Coverage limited to 60 N/S.

---

## Comparison Matrix

| Source | Resolution | Freshness | Surface Signal | Polar Coverage | Processing | Reliability |
|---|---|---|---|---|---|---|
| **matteason** | 8192x4096 | 3h | Mild (arid regions) | Mirrored fill | Levels only | Hobby project |
| **GMGSI LW** | ~3333x1667 | 1h | Moderate (thermal) | None (73 N/S) | NetCDF remap + levels | NOAA |
| **GFS TCDC** | 1440x721 | 6h | None | Full | GRIB2 → PNG | NOAA |
| **GDPS TCDC** | 2400x1201 | 12h | None | Full | GRIB2 → PNG | ECCC |
| **ECMWF IFS TCC** | 1440x721 | 6h | None | Full | GRIB2 → PNG | ECMWF |
| **SatCORPS GCC** | ~12000x6000 | 1h | None (COD) | Full (GEO+LEO) | NetCDF → PNG | NASA (not yet public) |

---

## Recommendations

### Current Best: matteason/live-cloud-maps

Best visual quality available today. 8K resolution, continuous opacity, real satellite detail. Surface contamination manageable with levels adjustment (floor=50, gamma=0.3). Cache locally for resilience. Fall back to EUMETSAT WMS if it goes offline.

### Authoritative Alternative: NOAA GMGSI LW

Institutional reliability, hourly updates, no single-point-of-failure risk. After levels adjustment (floor=60, ceiling=215, gamma=0.7) and 8K upscale, quality is comparable to matteason. Polar gap is the main disadvantage. Could be filled with GFS/GDPS model data in a hybrid approach.

### Polar Fill: GFS or GDPS TCDC

NWP data provides clean, surface-free cloud cover at the poles where geostationary satellites can't see. Best used as a fill layer blended with satellite data at the polar gap boundary, not as a standalone texture. GDPS has higher resolution (0.15 vs 0.25 deg) but only 2 runs/day.

### Future: NASA SatCORPS GCC

The ultimate source: 3 km, hourly, global, cloud optical depth, polar coverage. Monitor `satcorps.larc.nasa.gov` for public availability (expected mid-2026).

### Also Worth Monitoring

- **ECMWF 9 km open data** -- If/when the higher-resolution IFS TCC becomes freely available, it would produce a texture 3x finer than the current 0.25 degree tier.
- **MTG-I1 FCI products** -- Meteosat Third Generation is operational since Dec 2024 with 2 km IR. Cloud products may become freely accessible.

---

## Implementation Status

| Script | Source | Status |
|---|---|---|
| `scripts/fetch_matteason_clouds.py` | matteason 8K | Working. Levels: floor=50, ceil=255, gamma=0.3. |
| `scripts/fetch_gmgsi_clouds.py` | NOAA GMGSI LW | Working. NetCDF remap + levels (floor=60, ceil=215, gamma=0.7) + 8K upscale. |
| `scripts/fetch_gfs_clouds.py` | GFS TCDC | Working. GRIB2 byte-range download + rasterio decode. |
| `scripts/fetch_gdps_clouds.py` | ECCC GDPS TCDC | Working. Single-file GRIB2 download + rasterio decode. |

All scripts use `uv run` with PEP 723 inline dependencies.

---

## Sources

### Tested Sources
- <https://clouds.matteason.co.uk/> -- matteason/live-cloud-maps
- <https://github.com/matteason/live-cloud-maps> -- source code and pipeline details
- <https://registry.opendata.aws/noaa-gmgsi/> -- NOAA GMGSI on AWS
- <https://www.ospo.noaa.gov/products/imagery/gmgsi/> -- GMGSI product page
- <https://vlab.noaa.gov/web/towr-s/gmgsi> -- GMGSI technical specs
- <https://registry.opendata.aws/noaa-gfs-bdp-pds/> -- GFS on AWS
- <https://dd.weather.gc.ca/today/model_gem_global/15km/grib2/lat_lon/> -- GDPS data
- <https://eccc-msc.github.io/open-data/msc-data/nwp_gdps/readme_gdps-datamart_en/> -- GDPS docs

### Weather Model Data
- <https://www.ecmwf.int/en/forecasts/datasets/open-data> -- ECMWF open data
- <https://data.ecmwf.int/forecasts/> -- ECMWF direct HTTPS access
- <https://opendata.dwd.de/weather/nwp/icon/grib/> -- DWD ICON
- <https://registry.opendata.aws/meteo-france-models/> -- Meteo-France ARPEGE

### Satellite Products
- <https://registry.opendata.aws/noaa-goes/> -- GOES ABI data on AWS
- <https://gibs.earthdata.nasa.gov/wms/epsg4326/best/wms.cgi> -- NASA GIBS WMS
- <https://data.eumetsat.int/product/EO:EUM:DAT:MSG:CLM> -- MSG Cloud Mask
- <https://view.eumetsat.int/geoserver/wms> -- EUMETView WMS
- <https://www.goes-r.gov/products/baseline-cloud-opt-depth.html> -- GOES COD
- <https://user.eumetsat.int/resources/user-guides/mtg-fci-l2-oca-data-guide> -- MTG OCA
- <https://www.eorc.jaxa.jp/ptree/userguide.html> -- Himawari AHI COT

### Global Composites
- <https://www.earthdata.nasa.gov/data/projects/nsite/solutions/global-cloud-composites-satcorps> -- SatCORPS GCC
- <https://satcorps.larc.nasa.gov/> -- SatCORPS product page
- <https://nowcoast.noaa.gov/arcgis/rest/services/nowcoast/sat_meteo_imagery_time/MapServer> -- nowCOAST WMS
- <https://github.com/pytroll/satpy> -- Satpy compositing library
- <https://github.com/nullpainter/sanchez> -- Sanchez geostationary compositing CLI

### Other Pre-composited Services
- <https://github.com/matteason/daily-cloud-maps> -- VIIRS-based daily alternative
- <https://sos.noaa.gov/catalog/datasets/gfs-forecast-model-clouds-visible-real-time/> -- SOS GFS (eliminated)
- <https://www.xericdesign.com/xplanet.php> -- Xeric Design (paid)
- <https://cdn.star.nesdis.noaa.gov/GOES19/ABI/FD/GEOCOLOR/> -- GOES GeoColor CDN
