# Research: The Haze over Land in the Cloud Layer

Written 2026-10-10, from the observation that clouds over the ocean look good but over land many places sit under a constant haze that makes the ground texture look bleak. The existing controls, `cloud_floor` and `cloud_gamma`, trade one against the other: raising the floor clears the land and loses the thin cloud over the ocean, and over land where the source is quiet it loses detail too. The goal is a better picture, not a more faithful one, so a fix may be a heuristic. The plan that follows from this is [2026-10-10-cloud-haze-plan.md](2026-10-10-cloud-haze-plan.md).

Every measurement here is on one map, the 8192 x 4096 variant the app had cached at 18:39 UTC on 2026-10-10 (`clouds_cache_8192x4096.jpg`, ETag `d202101700baf82c19a4127681a21f9b`), reduced to 2048 x 1024 by 4 x 4 box averaging. The upstream image does not record its own time. The land mask used for the analysis and the prototype is the `global-land-mask` Python package (GLOBE 1 km), not the app's water mask cube, which only the GPU holds.

## 1. What the source encodes

matteason's pipeline (`index.js` on the `main` branch of `matteason/live-cloud-maps`, read on 2026-10-10) builds each pixel from three EUMETSAT EUMETView WMS layers:

```js
const ir = interpolate(72, 178, 0, 255, irMap.bitmap.data[idx]);   // mumi:worldcloudmap_ir108
const irGammaCorrected = gamma(1.46, ir);
const outputValue = gamma(2.0, screen(dust, irGammaCorrected * 0.77)) // dust from the Dust RGB layer
// where the natural color layer looks grey (channel spread under 25):
cloudMap = max(gamma(1.5, max(visibleR, visibleG, visibleB)), outputValue)
```

The base is the 10.8 µm infrared window channel, where colder is brighter, stretched linearly from display value 72 to 178 and then brightened. It measures temperature rather than cloud. Clear ground that is cold reads as cloud: high latitudes, autumn and winter, high terrain, and land at night. The visible term only adds brightness. It cannot take any away, so a grey surface (snow, salt flats, bright desert) also passes. The data sources review of 2026-03-19 already named this ("IR channel reads cold arid surfaces ... as cloud-like") and answered it with the global levels adjustment that became `cloud_floor` and `cloud_gamma`.

## 2. The distribution, land against ocean

Within 60 degrees of the equator:

| | median | 5th / 25th / 75th / 95th percentile | share under 0.05 |
|---|---|---|---|
| land | 0.62 | 0.00 / 0.20 / 0.86 / 0.95 | 18.1% |
| ocean | 0.63 | 0.02 / 0.30 / 0.86 / 0.95 | 8.5% |

The two are close. Land has more pixels at zero (hot deserts and tropical land in daylight) and otherwise the same shape. So the data alone does not say "land is hazier". What differs is the background a veil is drawn over: a translucent white over dark, nearly uniform ocean reads as texture, and over colored land it desaturates the ground.

A local measure says the same. Calling a pixel haze where the standard deviation over a window of about 2 degrees is under 0.04 and the mean is between 0.25 and 0.8:

| latitude | land: haze, median local s.d., median value | ocean: haze, median local s.d., median value |
|---|---|---|
| 0 to 20 | 2.6%, 0.142, 0.38 | 4.7%, 0.128, 0.33 |
| 20 to 40 | 2.3%, 0.112, 0.44 | 4.0%, 0.099, 0.53 |
| 40 to 60 | 5.2%, 0.053, 0.83 | 6.9%, 0.037, 0.87 |
| 60 to 75 | 2.4%, 0.014, 0.94 | 0.4%, 0.013, 0.94 |

The level of the map rises with latitude and its local variation falls, on land and ocean alike.

## 3. Checked against what the sky really showed

GIBS serves GOES-East GeoColor (`GOES-East_ABI_GeoColor`, equirectangular through the EPSG:4326 WMS) for 18:00 UTC the same day. Within 40 degrees of longitude 80 W and 55 degrees of the equator, with "clear" meaning colored ground (saturation over 0.25 and brightest channel under 0.5), the cloud map reads:

| latitude | clear land: 10th / 25th / 50th / 75th / 90th percentile | n |
|---|---|---|
| 10 to 30 | 0.08 / 0.26 / 0.46 / 0.78 / 0.91 | 7882 |
| 30 to 45 | 0.30 / 0.37 / 0.50 / 0.66 / 0.79 | 8206 |
| 45 to 55 | 0.58 / 0.70 / 0.79 / 0.83 / 0.86 | 3525 |

Pixels GeoColor shows as bright white cloud over land read a median of 0.87. So at 45 to 55 degrees clear ground and real cloud sit 0.08 apart. Through the defaults (floor 0.25, gamma 0.65, day opacity 0.85), clear ground at 0.79 is drawn at an alpha of 0.51. Side by side, the Great Plains and eastern Canada are clear in GeoColor and a uniform mid to light grey in the map. Off California the grey in the map is marine stratocumulus that GeoColor shows too.

The classifier is crude. Thin cirrus over land can pass as clear, and over ocean GeoColor's blue makes thin cloud look clear, which is why the ocean figures are left out: at 30 to 45 degrees they put "clear" ocean at a median of 0.61, which is not credible. The land figures are enough for the conclusion: over land in the mid latitudes, most of the haze is the ground's own temperature.

## 4. Why the global controls cannot fix it

The clear-sky level of the map varies from about 0 (tropical ocean, hot deserts by day) to about 0.8 (mid-latitude land in October) and real cloud sits above it by as little as 0.08. A floor is one number for the whole globe. At 0.8 it clears Canada and erases nearly all of the ocean's low cloud. At 0.25 it leaves Canada under a veil. Gamma reshapes the same single curve and has the same problem. Any fix has to let the floor vary across the globe.

## 5. Options considered

- A floor that follows a local background estimate, computed from the map itself (section 6). Works on one snapshot, no history, no new data source.
- Separate floor and gamma for land and ocean, through the water mask. Addresses the perception difference of section 2, not the latitude dependence of section 3.
- A clear-sky composite over time: the per-pixel minimum over several days, per UTC hour, at low resolution. The standard satellite technique, and the only one here that would not thin a large real deck, since a deck moves on within days. It needs persistent state, a warm-up of days, and an app that has been running to collect it. A candidate for the own pre-processor that is to replace matteason in the long run.
- Local contrast stretching (CLAHE and similar). Amplifies the faint texture inside a uniform haze into structure that is not there.
- Gating alpha on local variation. A real overcast deck is as smooth as a haze (section 2), so it would go too.
- A different source. GMGSI and the raw EUMETView IR ring share the contamination (2026-03-19 review). Out of scope until the own pre-processor.

## 6. The background estimate

The assumption: within a window of a few hundred kilometers, at least a small share of the pixels are clear sky, and those pixels show how the ground itself reads to the IR channel. That reading varies smoothly, with surface temperature, while clouds have edges and gaps. So a low percentile over a large window, smoothed, estimates the clear-sky level everywhere.

The prototype (a Python script, outside the tree):

1. Reduce the map to 512 x 256 by box averaging, about 78 km a pixel at the equator.
2. The 15th percentile over a 9 x 9 window (about 700 km at the equator), wrapping across the antimeridian.
3. A Gaussian blur of sigma 3 pixels (about 230 km), wrapping the same way.
4. Cap at 0.85.
5. Multiply by a strength from a soft land weight: 1.0 over land, 0.15 over open water, the mask blurred over about 80 km at every latitude.
6. Upsample bilinearly to the map's size, and replace each pixel with `(raw - floor) / (1 - floor)`. The shader's own floor and gamma then apply as before.

On this map the resulting floor averages 0.56 over land and 0.10 over water. Where the window holds no haze (the tropics, hot deserts, warm ocean) the background is near zero and nothing changes.

The window is a fixed number of pixels, so it narrows in kilometers toward the poles. That was left as it is: the estimate held up at the latitudes that matter, and at the poles the source is a mirrored fill (rows within 22.5 degrees of each pole are copied from the rows next to them) rather than data.

## 7. What the prototype looked like

Rendered with `sunlit-earth render` (debug build, with the temporary wait of section 8) at 960 x 720, the processed map handed to it through `SUNLIT_EARTH_CLOUD_URL`, over North America at 18:00 UTC and South America at 17:00 UTC on 2026-10-10:

| variant | North America | South America and the Southern Ocean |
|---|---|---|
| today's defaults | Plains and eastern Canada under a uniform veil | Argentina and Patagonia hazy, Southern Ocean swirls rich |
| land floor +0.25, no estimate | veil reduced, still visible over Canada | Argentina somewhat cleaner, ocean unchanged |
| background, full strength everywhere | land clean and matching GeoColor | Southern Ocean swirls gutted |
| background at 60% everywhere | modest improvement | ocean slightly reduced |
| background, land 1.0 and ocean 0.15 | land clean, cloud bands kept | Argentina clean, Southern Ocean kept |
| the same, shader floor 0.10 | as above, remaining cloud denser | as above |

The land-weighted estimate was the best of these, and lowering the shader's floor to about 0.10 suits it, because the two floors compose: subtracting a background `b` and then a floor `f` is one subtraction of `1 - (1 - b)(1 - f)`.

Its failure is a large real deck over land. Inside a deck wider than the window the low percentile is cloud, so the deck is taken for ground and thinned: the storm over the southeastern United States, bright white in GeoColor, came out grey. The cap of 0.85 limits that and does not prevent it. The second visible effect was at Antarctica and Greenland, full-strength land with a background at the cap, which lost nearly all their cloud. The polar rows are a mirrored fill, so little real data is lost there, and a land weight blurred evenly in kilometers (rather than in pixels of the equirectangular map) removed the hard edge the first version drew along the Antarctic coast.

## 8. A side finding

`sunlit-earth render` does not wait for the cloud overlay, which is excluded from texture readiness, so with a warm tile cache the export usually lands before the cached cloud map is decoded and posted. In the debug build, where that decode took 1.17 s for the 5.4 MB map, one render of nine had clouds. The comparisons above needed a temporary five second wait before the export. `displays --out` sends its publish without waiting either. The plan fixes both (decision 10).
