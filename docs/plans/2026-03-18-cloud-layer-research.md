# Cloud Layer Research Summary

**Date:** 2026-03-18
**Status:** Consolidated summary of all cloud layer research conducted 2026-03-17

This document synthesizes findings from seven research documents covering data sources, cloud mask quality, model-based alternatives, GFS extraction procedures, and GPU rendering techniques.

---

## Data Sources

### The core problem: surface contamination

Most satellite-derived cloud products contain visible surface features (continents, ocean, terrain) because the sensors observe top-of-atmosphere radiance — when the sky is clear, they see the ground. This is acceptable for weather forecasting but problematic for a globe renderer where the Earth surface is already rendered separately. Any surface signal in the cloud texture creates a ghostly double-image of geography.

Three categories of sources were evaluated, from least to most processing effort:

### Category 1: Pre-composited satellite images (ready to use)

**[matteason/live-cloud-maps](https://github.com/matteason/live-cloud-maps)** is the most practical immediate source. It provides a global equirectangular greyscale JPEG at up to 8192x4096 via direct HTTPS (`https://clouds.matteason.co.uk/images/4096x2048/clouds.jpg`), updated every 3 hours, CC0 licensed, no auth required. The ~1.4 MB file at 4096x2048 is derived from EUMETSAT geostationary IR and visible channels, processed with a multi-channel filter that suppresses most surface signal. Continental outlines remain faintly visible over arid regions — an inherent limitation of the IR-based approach where cold desert surfaces resemble cloud tops. The main risk is reliability: single-developer hobby project with no SLA.

**[matteason/daily-cloud-maps](https://github.com/matteason/daily-cloud-maps)** takes a different approach, multiplying NASA GIBS VIIRS true-color imagery by the VIIRS Clear Sky Confidence field. This retains surface detail at partial-confidence pixels, making it a cloud-weighted visible composite rather than a true cloud mask.

**[apollo-ng/cloudmap](https://github.com/apollo-ng/cloudmap)** is a similar IR-based composite but appears unmaintained (satellite sources from ~2013–2018) and uses GitHub raw content as CDN, which is unreliable for automated downloads.

Other pre-composited options evaluated and found unsuitable:

| Source | Why not |
|---|---|
| [Xeric Design](https://www.xericdesign.com/xplanet.php) | Paid subscription ($25–115/year), credential management |
| [OpenWeatherMap](https://openweathermap.org/api/weathermaps) | API key required, Mercator projection, tile assembly |
| [SSEC Mollweide composites](https://www.ssec.wisc.edu/data/composites/) | Mollweide projection requires GDAL reprojection; GIF format |
| [EUMETSAT direct](https://view.eumetsat.int) | Regional only, geostationary projection, account required |
| [JMA Himawari](https://himawari8.nict.go.jp) | Regional only, HSD format, commercial use prohibited on NICT |

### Category 2: Satellite cloud mask products (true cloud detection, require compositing)

True binary cloud masks from operational weather satellites contain no surface signal — they classify each pixel as clear or cloudy using multi-spectral tests, not surface-observing imagery. However, each satellite covers only a portion of the globe:

- **[GOES ABI Clear Sky Mask](https://registry.opendata.aws/noaa-goes/)** (`ABI-L2-ACMF`): Americas + Atlantic, 10-minute updates, 2 km resolution, publicly available on AWS S3 as NetCDF in ABI Fixed Grid projection.
- **[EUMETSAT MSG Cloud Mask](https://data.eumetsat.int/product/EO:EUM:DAT:MSG:CLM)**: Europe/Africa/Atlantic, 15-minute updates, ~3.5 km, requires EUMETSAT Data Store registration, GRIB/NetCDF in geostationary disk projection.
- **[NASA GIBS VIIRS Clear Sky Confidence](https://gibs.earthdata.nasa.gov)**: Global, daily, accessible via WMS. This is a per-pixel confidence value (0=cloudy, 1=clear) from the VIIRS Cloud Mask algorithm. The closest thing to a global cloud-only product from a single source, but the geographic cloud climatology pattern (deserts are genuinely less cloudy) is still present — this is physically correct, not an artifact.

Compositing GOES + Meteosat + Himawari masks into a global equirectangular image would produce the highest-quality cloud-only texture, but requires fetching from multiple sources, reprojecting each from its native geostationary projection, and handling seams. This is the approach used by operational weather analysis systems but requires significant processing infrastructure.

### Category 3: Weather model data (definitively cloud-only, requires GRIB2 processing)

**[NOAA GFS Total Cloud Cover](https://registry.opendata.aws/noaa-gfs-bdp-pds/) (TCDC)** is the strongest finding of this research. GFS is NOAA's operational global forecast model running 4x/day at 0.25° resolution (~28 km). TCDC is a model-computed field representing total column cloud fraction (0–100%) with absolutely no surface signal — it is generated purely by atmospheric dynamical equations, not satellite observation. There are no swath boundaries, no stitching seams, and no surface photography.

Access is straightforward: the `noaa-gfs-bdp-pds` S3 bucket is public (no credentials), and the `.idx` companion files enable HTTP byte-range downloads of individual GRIB2 records (~1–4 MB for the TCDC field alone, versus ~508 MB for the full file). The `TCDC:entire atmosphere` record from `gfs.tHHz.pgrb2.0p25.f000` on the 0.25° grid (1440x721 points) is the correct field. Latency is ~3.5 hours after analysis time; update cadence is every 6 hours.

The trade-off is format conversion: GRIB2 → equirectangular PNG requires either a sidecar script (Python with cfgrib/xarray, or GDAL) or Rust GRIB2 parsing via the [`grib` crate](https://github.com/noritada/grib-rs) (pure Rust with JPEG2000 support via openjpeg). The longitude convention (0–360°) requires a 180° roll to match the standard -180/+180 equirectangular convention.

A complete Python extraction script was developed and tested (see `2026-03-17-gfs-tcdc-extraction.md`), covering S3 path discovery, byte-range download, cfgrib decoding, longitude rolling, and PNG output. Five conversion tool options were evaluated: cfgrib+xarray (recommended), GDAL gdal_translate, wgrib2+Python, eccodes grib_get_data, and Herbie.

**[NOAA Science on a Sphere](https://sos.noaa.gov/)** serves what appears to be a pre-rendered GFS cloud texture via FTP (`ftp://public.sos.noaa.gov/rt/grids/gfsclouds/vis/`), but format, resolution, and reliability are underdocumented.

### Other sources investigated

- **[NASA NEO MODIS Cloud Fraction](https://neo.gsfc.nasa.gov/view.php?datasetId=MODAL2_M_CLD_FR)**: Monthly/8-day composites permanently show continental outlines due to real geographic cloud climatology (ocean ~72% vs land ~55% average coverage). Not suitable as an instantaneous cloud mask.
- **[NOAA GMGSI](https://registry.opendata.aws/noaa-gmgsi/)**: Best-quality dedicated geostationary mosaic (hourly, ~3 km, four spectral bands), but distributed as NetCDF with no viable Rust decoder. The visible band is the best raw material for cloud visualization; the LW IR band could support threshold-based cloud extraction. Also accessible as WMS via [nowCOAST](https://nowcoast.noaa.gov/geoserver/satellite/wms) (public, no auth).
- **[EUMETSAT IR108 WMS](https://view.eumetsat.int/geoserver/mumi/wms)**: The raw upstream data behind matteason's product, publicly accessible without authentication. Useful as a fallback data source but has the same surface contamination issue as matteason.
- **Open-Meteo**: Point data only, [no image output](https://github.com/open-meteo/open-meteo/issues/635).
- **Windy / Ventusky**: Use GFS internally but do not expose raw cloud images.
- **GridSat-B1**: Historical CDR (through ~2024), not real-time.
- **CMSAF / ISCCP**: Climate research products, not real-time public services.
- **[NASA SatCORPS](https://www.earthdata.nasa.gov/data/projects/nsite/solutions/global-cloud-composites-satcorps)**: Potentially excellent (global, 3 km, hourly from multiple satellites), but not yet publicly available; expected mid-2026.

### Data source recommendation

| Priority | Source | Why |
|---|---|---|
| **Immediate** | matteason/live-cloud-maps | Zero friction: direct HTTP, equirectangular JPEG, CC0, no auth. Surface contamination is mild and acceptable at globe scale. Cache locally with If-Modified-Since for resilience. |
| **Best long-term** | GFS TCDC via sidecar script | Provably cloud-only, no surface artifacts, authoritative NOAA source. Requires a preprocessing step (Python/GDAL) to convert GRIB2 to PNG, but the script is straightforward and runs on a 6-hour cron schedule. |
| **Fallback** | EUMETSAT IR108 WMS or nowCOAST WMS | Public, no auth, hourly. Same raw data as matteason but without the multi-channel filtering. Useful if matteason goes offline before GFS pipeline is built. |

---

## Rendering Approach

### Two-sphere model

The cloud layer renders as a second sphere at radius 1.0015 (corresponding to ~9.6 km altitude above a 6,371 km Earth). This produces correct parallax at oblique viewing angles automatically — clouds appear shifted relative to the surface at the limb, matching real satellite observations. At close zoom, parallax is visible; at full-globe zoom it is sub-pixel. A fixed radius works across all zoom levels.

The cloud draw call is issued in the same render pass as the Earth, after the Earth draw. Both draws share the same MSAA color/depth attachments with a single resolve at the end. The Earth pipeline is unchanged (opaque, depth writes enabled). The cloud pipeline uses alpha blending (`BlendState::ALPHA_BLENDING`, straight alpha) with depth writes disabled and depth test still enabled (so clouds behind the globe are discarded but don't block future layers).

The vertex shader reuses the Earth's vertex/index buffer, multiplying positions by `cloud_sphere_radius` from a uniform. No duplicate geometry needed.

### Cloud shader

The cloud fragment shader samples the red channel of the cloud texture as cloud density, computes sun-angle brightness via a single smoothstep through the terminator (bright white on the day side, faint grey at 0.05 on the night side), and outputs straight alpha: `vec4(brightness, brightness, brightness, density * opacity)`.

No Fresnel or specular effects — clouds are diffuse scatterers. No secondary diffuse ramp — the single smoothstep produces uniform dayside brightness. An optional limb fade (`smoothstep(0.0, 0.15, n_dot_v)`) was considered to simulate atmospheric depth at the globe's edge but was deferred because the cloud sphere at 1.0015 extends only ~0.75 pixels beyond the Earth sphere at typical resolutions.

### Cloud shadows (deferred)

Cloud shadows on the Earth surface require a ray-sphere intersection in the Earth fragment shader: for each surface fragment, cast a ray toward the sun, intersect with the cloud sphere, sample the cloud texture at the hit point's UV, and darken the surface. Use `textureSampleLevel` at mip level 2.0 for soft penumbra without a blur pass. This requires adding the cloud texture as a binding to the Earth shader's bind group — a separate feature.

### Cloud texture format

The plan called for R8Unorm (single channel, half memory) but deferred it in favor of RGBA8 through the existing texture loading pipeline. The `image` crate expands the greyscale PNG to RGBA (r=g=b=gray, a=255) and the shader samples the `r` channel. The R8Unorm optimization saves ~25 MB for a 4096x2048 texture but requires changes to `create_mipmapped_texture` and `downsample_2x`.

---

## Caching and Update Strategy

For live cloud data (when network fetching is implemented):

1. Store the last downloaded cloud image locally with a modification timestamp.
2. Use `If-Modified-Since` / ETag headers for conditional HTTP GET (304 if unchanged).
3. Fetch on a background thread, never blocking the render loop. Swap the texture only after the new file is fully downloaded and decoded.
4. Poll every 3 hours (matching matteason) or 6 hours (matching GFS).
5. On failure, continue rendering with the cached texture. Log the error; never show a blank cloud layer.
6. Provide a user-configurable option to disable automatic cloud updates.

---

## Sources

### Data sources
- https://clouds.matteason.co.uk/ — matteason/live-cloud-maps service
- https://github.com/matteason/live-cloud-maps — source code and processing pipeline
- https://github.com/matteason/daily-cloud-maps — VIIRS-based daily cloud maps
- https://registry.opendata.aws/noaa-gfs-bdp-pds/ — NOAA GFS on AWS Open Data
- https://nomads.ncep.noaa.gov/cgi-bin/filter_gfs_0p25.pl — NOMADS GFS 0.25° filter
- https://registry.opendata.aws/noaa-gmgsi/ — NOAA GMGSI on AWS Open Data
- https://www.ospo.noaa.gov/products/imagery/gmgsi/ — NOAA OSPO GMGSI product page
- https://vlab.noaa.gov/web/towr-s/gmgsi — NOAA VLAB GMGSI specs
- https://nowcoast.noaa.gov/geoserver/satellite/wms — NOAA nowCOAST WMS
- https://view.eumetsat.int/geoserver/mumi/wms — EUMETSAT EUMETView public WMS
- https://nasa-gibs.github.io/gibs-api-docs/ — NASA GIBS API
- https://wvs.earthdata.nasa.gov/ — Worldview Snapshots
- https://neo.gsfc.nasa.gov/view.php?datasetId=MODAL2_M_CLD_FR — NASA NEO cloud fraction
- https://registry.opendata.aws/noaa-goes/ — GOES data on AWS
- https://data.eumetsat.int/product/EO:EUM:DAT:MSG:CLM — EUMETSAT MSG Cloud Mask
- https://www.xericdesign.com/xplanet.php — Xeric Design cloud maps
- https://openweathermap.org/api/weathermaps — OpenWeatherMap
- https://www.ssec.wisc.edu/data/composites/ — SSEC composites
- https://github.com/apollo-ng/cloudmap — apollo-ng cloudmap
- https://www.earthdata.nasa.gov/data/projects/nsite/solutions/global-cloud-composites-satcorps — NASA SatCORPS
- https://sos.noaa.gov/catalog/datasets/gfs-forecast-model-clouds-visible-real-time/ — NOAA SOS GFS clouds
- https://github.com/noritada/grib-rs — Rust GRIB2 parser
- https://herbie.readthedocs.io/ — Herbie Python library for GFS
- https://news.ycombinator.com/item?id=38289376 — matteason methodology discussion

### Rendering techniques
- https://webgpufundamentals.org/webgpu/lessons/webgpu-transparency.html — WebGPU transparency
- https://wgpu.rs/doc/wgpu/struct.BlendState.html — wgpu BlendState
- https://blog.mastermaps.com/2013/09/creating-webgl-earth-with-threejs.html — two-sphere cloud technique
- https://discourse.threejs.org/t/how-to-cast-shadows-from-an-outer-sphere-to-an-inner-sphere/53732 — cloud shadow technique
- https://gamedev.net/forums/topic/671726-struggling-with-casting-cloud-shadow-on-earth-sphere-in-opengl/ — cloud shadow ray-sphere intersection
- https://www.realtimerendering.com/blog/gpus-prefer-premultiplication/ — premultiplied vs straight alpha
- https://www.mdpi.com/2072-4292/12/3/365 — parallax shift correction based on cloud height
