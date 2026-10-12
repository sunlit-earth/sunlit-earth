# Research: The Surface of Closed Sea Ice (2026-10-12)

The baked sea ice of [the sea ice look plan](2026-10-12-sea-ice-look-plan.md) got its coast between land and ice right, but its interior, the `b3b-leads` variant's crumple, still "has room for improvement" in the user's words. A full preview run takes 5 to 7 minutes, too slow to try surfaces by eye, so a standalone lab drew closed ice (100% concentration) on synthetic data only, with no Earth texture and no pipeline run, and showed each sample the way the app gets it. Three rounds of samples went to the user, who closed the third with "ok, solid ice is solved". This document records the method, the rounds, the look the user settled on with every parameter, the code that draws it, and what moving it into `sea_ice_look.py` involves. Partial coverage (floes, the ice edge) belongs to a separate lab and is not covered here.

The lab itself lived under `tmp/sea-ice-lab/solid/`, which is not tracked; everything needed to reproduce its settled output is in sections 3 and 4.

## 1. Summary and method

- The square. Each sample is drawn on 1024 x 1024 texels at the polar cap's spacing, `texel_km(4096)` = 2.873 km, so 2,942 km across: large enough to show the 300 to 1500 km activity field. The tracked helpers in `sea_ice_look` read the spacing from `texel_km(size)`; the lab pointed that name at the cap's spacing so they draw at 2.873 km on a 1024 square.
- Color. The surface is a brightness factor on the ice's color, clipped at 255, and leads blend toward the sea's fill (10, 30, 60) by their share, as `month_cap` and `composite_face` compose them at concentration 1. Colors are the pipeline's measured coastal snow medians: north winter (247, 250, 251), south winter (236, 240, 244); a third palette, the north's August (211, 217, 219) with b3d's melt tone, was on the page but never reviewed.
- As shipped. The 1024 square is area averaged (Pillow's BOX filter, as `cube.area_average`) to the face's 4.9 km per texel, 600 texels, then sent through a JPEG XL round trip at quality 85, effort 7, `lossless_jpeg=False`, as `processing.encode_jxl` encodes the faces.
- Views. The whole square at face resolution, and a 256 texel crop of the decoded face (1,254 km) magnified 3 times with bilinear filtering, standing in for the app's zoom. The crop was fixed for all samples so that it runs from flat to structured pack.
- Metrics, on the north winter color after the encode: mean brightness relative to the color; fine detail, the standard deviation of luminance (Rec. 709) less its 5 x 5 texel box mean (under about 25 km), before and after the encode and their ratio; broad variation, the standard deviation of the 5 x 5 box mean; the share of full resolution texels clipped at 255; and the lead share, the mean share of the fill the leads add, in per mille of the area.
- References. Crops of three MODIS mosaics (Wikimedia "Top of the World" 2019-08-05 and 2020-05-06, "Antarctic" 2018-01-05) at the same two scales. Their scale was estimated from geography, not read from metadata: 3.8 km per pixel for both Arctic mosaics (Greenland about 280 px wide at 70 to 72 N, where it is about 1,100 km across, and Peary Land about 320 px, 1,220 km, north of that), uncertain by about 15%; 3.0 km per pixel for the Antarctic one (the Queen Maud Land coast and the Ross Ice Shelf front about 1,150 px, 3,550 km, apart), uncertain by about 20%.
- Speed. All samples rebuild in 25 to 40 s on four processes; one sample in 3 to 5 s. Every field is seeded (seed 2026).

## 2. Rounds

| Round | Samples | The user's verdict | Learned |
|---|---|---|---|
| Before the lab (b3a) | Brightness noise with grey dots; ridged noise stretched 3 times; the folds of b3a/b3b | "a white base where some random brush dots with light grey are placed on"; the stretched ridges "read as parallel ripples, like dunes"; the folds read better "but the surface is still not there" | The target: "pretty bright, with very fine details, like a crumbled piece of paper that was very carefully flattened again. the effect is not uniform though, there are areas where a lot more snow is piling up and you can see much more structure, while some other areas can look almost perfectly flat." |
| 1 | s00 current look (b3b-leads), s01 without leads, s02 split folds (a recursive straight line tessellation with T-junctions), s03 warped Voronoi edges, s04 ridged multifractal, s05 folds at three scales, s06 straight bordered plates, s07 sharp sparse activity, s08 drift streaks on plates, s09 soft wide activity, s10 and s11 split folds at 2 and 0.5 times, s12 finer split folds, s13 split with leads, s14 all combined | s00: "too blocky, too many straight edges in the snow", its leads "look mostly nice, a bit too prominent though, should be thinner / fainter". s02: "still too blocky." s03: "not bad. a bit faint, but not bad." s04: "nice! edges could be a bit more detailed. not thicker, if anything a bit less thick, but still sharp." s05, s06, s07, s10: bad, s05 "blocky again". s08 and s09: "maybe worth iterating upon, but currently also too blocky." s11: "pretty faint, which actually makes it look better." s12: "maybe has potential." s14: "a bit much now". | No straight edges and no facets: everything built from straight lines (folds, splits, plates) read as blocky. Fainter is better. The ridged multifractal, which the b3a round and this lab had both called "wormy" and "stucco", is what the user liked. |
| 2 | Around s04: r2-01 to r2-03 three detail steps; r2-04 and r2-05 step 2 at 60% and 35%; r2-06 with Voronoi creases, r2-07 with soft activity, r2-08 with drift along a turning wind, r2-09 with bent split folds under it, r2-10 with thinner, fainter leads | r2-01 to r2-03: "all look fine, I don't see much difference. We can use all of them to vary the look in different places." r2-06 and r2-07: "also look good, a bit softer overall." r2-08: "the streaks in r2-08 are great, we should definitely have those in some places, but not everywhere." r2-09: "also not very distinct from the rest, but looks fine." r2-10: "the leads look a lot better now, I like. They still appear too much like painted with a pen. should be a bit more jagged, since the ice is shifting." | The detail level is a knob for variety, not a choice. Drift belongs in some regions only. Leads at half the width and 0.6 of the darkness are right in strength but need a broken course. |
| 3 | r3-01 to r3-06 lead styles on r2-04 (zigzag, en echelon, pinch, shear jogs, ragged, open and refrozen stretches); r3-07 to r3-10 surfaces varied by region (r3-10 with jogs plus en echelon leads) | "ok, solid ice is solved. most of the leads look good, slight preference for echelon and pinch, less for zigzag and stretches." | Settled. No single region sample was singled out (section 3.7). |

Findings that hold beyond the samples:

- The encode takes faint detail first. Of the fine detail rms, 83% survives the encode at s04's strength (r2-02), 69% at 60% (r2-04) and 57% at 35% (r2-05); s04 itself kept 85%, the old crumple 83%, the Voronoi edges 60%. A setting that looks right before the encode is fainter after it, and more so the fainter it is; judge after the encode. The settled default keeps 80%.
- The north's color clips. At (247, 250, 251) a brightening of 1.6% clips the blue channel and 3.2% the red, so in the north only the shadow side of any relief shows; the south's (236, 240, 244) keeps 4.5% to 8% of headroom and shows both sides. The old crumple clipped 11% of the texels; the settled default clips 4.5%. Relief here is asymmetric for that reason: shadow twice the light.
- Region fields must be large. Fields at 150 to 600 km made regions too small to tell apart on the 2,942 km square; 250 to 900 km reads as regions, 400 to 1400 km as a few large ones.
- The user's taste, not ours, is the guide. Twice a generator this side dismissed (the ridged noise, "wormy", "stucco") was the one the user liked, and the generators this side favored (split folds as "crumpled paper") read to the user as blocky.
- What the references show. In the Arctic crops the closed pack is a mosaic of smooth and rough plates with sharp borders, and the rough plates carry bundles of light and shadow streaks; nothing looks like a dark crease line. Some of the streaky texture there is thin cloud. The straight plate borders were tried (s06) and rejected as blocky; the streaks survived as the drift.

## 3. The settled look

Every value below is the one the lab drew with. The reference samples are r3-08 for the surface and r3-02 (en echelon) and r3-03 (pinch) for the leads; r3-11, added after the review as this document's recommended default (section 3.7), is r3-08's surface with both lead styles at once and is what section 4's code reproduces.

### 3.1 Surface: ridged multifractal at three detail levels

Musgrave's ridged multifractal, isotropic (aspect 1), lit from one direction:

- Octaves of `band_noise` with feature sizes `(f / 1.5, f * 1.5)` for f = 40, 20, 10, 5, 2.5 km (coarsest 40 km, continuing while f is at least 2.5 km), all drawn in that order from one generator.
- Each octave is read through a crinkle shift: `curl_shift` at 6 to 25 km with an rms of `warp_km`, drawn from its own generator so the octaves' noise stays the same whatever the crinkle.
- Per octave: `signal = clip(offset - |n|, 0) ** exponent * weight`, `height += persistence ** octave * signal`, `weight = clip(signal * octave_gain, 0, 1)`, with offset 1.0 and octave gain 1.5. The exponent narrows each ridge about its crest, which stays a cusp.
- Shading: the gradient of the height along the light direction, 0.6 radians from the columns toward the rows, normalized to a standard deviation of 1; brightening 0.025 per standard deviation on the lit side, darkening 0.05 on the other.

The three detail levels differ only in three values:

| Level | Exponent | Persistence | Crinkle rms | Round 2 sample |
|---|---|---|---|---|
| 1 | 3 | 0.75 | 1.5 km | r2-01 |
| 2 | 3 | 0.9 | 2.5 km | r2-02 |
| 3 | 4 | 1.0 | 3.5 km | r2-03 |

s04, the round 1 sample the user liked, is level 1 without the 2.5 km octave, with exponent 2 and no crinkle. Because the three levels share their noise and their crinkle's direction, blending them leaves no seams.

### 3.2 Region fields

Five independent fields of `band_noise` at 250 to 900 km, fixed on the plane, decide the look by place:

- Detail: weights `w3 = smoothstep(0.2, 0.8, n)`, `w1 = smoothstep(-0.2, -0.8, n)`, `w2 = 1 - w1 - w3`; the relief is `w1 * level1 + w2 * level2 + w3 * level3`.
- Soft: `soft = smoothstep(0.0, 0.8, n)`, about a third of the area (section 3.3).
- Drift: `smoothstep(0.5, 1.1, n)`, about a quarter (section 3.4).
- Underlayer: `smoothstep(0.3, 1.0, n)` when the optional underlayer is on (section 3.5).
- Strength: `gain = 0.35 + 0.5 * smoothstep(-1.0, 1.0, n)`, from 35% to 85% of s04's strength.

The brightness factor is `1 + gain * activity * relief + gain * drift`.

### 3.3 Activity and the soft areas

- The hard activity is b3b's unchanged: `band_noise` at 300 to 1500 km through `smoothstep(-0.3, 1.0, n)`, floor 0.1, so `0.1 + 0.9 * step`.
- The soft activity is r2-07's: the same noise through `smoothstep(-1.5, 1.2, n)`, floor 0.15. Structure fades in and out gradually and most of the pack carries some.
- The activity is `(1 - soft) * hard + soft * soft_activity`.
- In the soft areas the relief also takes r2-06's warped Voronoi creases at weight 1.5 times the soft weight: `facet_cells` at 20, 60 and 180 km with 35%, 50% and 80% of their points kept, read through a `curl_shift` of 5 km rms at 20 to 80 km plus one of 1.5 km at 6 to 20 km; each level's border is a crease `crease * exp(-(border / 1 km)**2)` of 0.06, 0.08 and 0.08, the strongest of the three taken; the cells' own shades are left out, since they drew grey blotches.

### 3.4 Drift streaks, in some regions only

r2-08's streaks: `steered_noise` at 8 to 30 km across, 6 times longer than wide, along a wind direction `0.7 + 0.5 * n` radians with n `band_noise` at 800 to 3000 km; the height is shaded like the ridges, 0.03 lit and 0.05 shadow per standard deviation; it shows where `smoothstep(0.3, 0.6, activity)` times the drift region field allows, and is scaled by the same regional gain.

### 3.5 Optional underlayer

r2-09's bent split folds, which the user found "not very distinct from the rest, but looks fine": the square is split recursively by straight lines, each through its cell's centroid, roughly across its long axis (scatter 0.7 rad, offset up to 0.4 of its spread), down to a depth of 6 to 16 levels set by the activity (cells of about 370 km down to about 11 km, none smaller than 6 km across); every split from level 5 is a fold, a ridge or valley of up to 0.06 lit from the light direction and fading over a quarter of the cell's spread, held to 2 to 8 km; the coordinates are bent first by a `curl_shift` of 10 km rms at 30 to 120 km plus 2 km at 6 to 20 km, so no straight line shows. Shading: 1.0 lit, 1.6 shadow, layer weight 1.2, where the underlayer field allows. r3-09 is the sample with it; the default leaves it off, and it costs about 1.5 s of the lab's 3.5 s.

### 3.6 Leads

The fracture systems are b3b's, traced by the tracked `fracture_paths` with every tunable of `LOOK` except three: width 0.7 to 2.5 km (b3b 1.5 to 5), darkness 0.06 to 0.2 (0.1 to 0.35), open systems 0.25 to 0.5 (0.45 to 0.8). That is r2-10's drawing, about half the width and 0.6 of the darkness. Seeding as b3b: twice as dense where the hard activity is full; the lab placed 150 systems per 15 million square kilometers (the north's count over its March ice), 86 on the square, and drew every system with the rank fade `clip((1 - rank) / 0.15, 0, 1)`. Along each fracture the swell and darkness variation of `draw_fractures` are kept (width times `clip(1 + 0.25 n, 0.7, 1.3)`, darkness times `clip(1 + 0.35 n, 0.3, 1.7)`, n smoothed over 60 km) and the darkness tapers to a third at the ends.

The styles, each one idea, with the user's preference:

| Style | Sample | What it does | Preference |
|---|---|---|---|
| En echelon | r3-02 | Pieces of 10 to 40 km, each turned 0.1 to 0.3 rad off the course (one sign per fracture), offset 0.5 to 1.5 km to alternate sides, extended 2 to 6 km at both ends so neighbors overlap, ends tapering to 15%; darkness times 0.75 for the overlaps | Preferred |
| Pinch | r3-03 | Width in steps every 4 to 25 km, 0.6 to 1.8 times its own, shut on 20% of the steps, steps smoothed over about 1 km | Preferred |
| Shear jogs | r3-04 | The course jumps sideways 1.5 to 4 km every 20 to 80 km over 1.5 km; branches leave from the unshifted parent, so they start offset | Acceptable |
| Ragged | r3-05 | Width times `clip(1 + 0.7 n, 0, 2.5)`, n `band_noise` at 3 to 12 km fixed on the ground | Acceptable |
| Zigzag | r3-01 | Straight pieces of 3 to 12 km, corners alternately 1 to 3 km off the course | Less preferred |
| Stretches | r3-06 | Every 10 to 50 km either open (0.25 to 0.45 of the fill, 30% of the runs) or refrozen (0.04 to 0.1) | Less preferred |

**Lead share target: about 0.93 per mille** of the area on the synthetic square (r2-10: 0.925; old b3b leads: 3.19). Every style was held there: zigzag 1.08, en echelon 0.95, pinch 0.95, jogs 0.98, ragged 0.95, stretches 0.97; the default 0.925. Combining styles changes the share (en echelon alone first drew 1.27, en echelon with pinch 0.99 at darkness 0.8), so the darkness factor is the knob that brings a combination back to the target. Stretches combined with en echelon at the target spread the darkness so evenly that the leads nearly vanished in the zoom; that combination is not recommended.

### 3.7 The default, and what was not reviewed

The user did not single out one of r3-07 to r3-10. The recommended default is r3-08's surface (regions at 250 to 900 km, the three detail levels, soft areas, drift in about a quarter, strength 35% to 85%, no underlayer), because the strength varying by region does more for variety than the detail level, which the user saw little difference in, and it reaches r2-05's faintness in places, which the user liked in s11. For the leads, en echelon and pinch together at darkness 0.75, the two styles the user preferred; they were reviewed apart, not together. That combination is r3-11, built after the review and not shown to the user; r3-08 with r3-02's leads alone is the fully reviewed fallback.

Reference numbers of r3-11 at seed 2026 on the 1024 square, north winter, after the encode: mean brightness 0.995 of the color, fine detail rms 3.54 before and 2.83 after the encode (80% kept), broad variation 1.90, 4.5% of texels clipped, lead share 0.925 per mille.

Not reviewed: the summer palette with the melt tone, the south's lead density (the lab used the north's for every color), and anything below 100% concentration.

## 4. Code

The blocks below, concatenated in order, form one module that reproduces the lab's r3-11 exactly (section 4.7 says how this was checked). They import these tracked helpers unchanged: from `texture_pipeline.sea_ice_look`, `band_noise`, `curl_shift`, `warped`, `steered_noise`, `facet_cells`, `fracture_paths`, `smoothstep`, `normalized` and `LOOK`; from `texture_pipeline.sea_ice`, `texel_km`. The lab's random generators are `default_rng([seed, stream])`; the pipeline's `_rng` adds the hemisphere, and the stream numbers below are only the lab's.

| Stream | Draws |
|---|---|
| `[seed, 5]` | Activity noise, 300 to 1500 km |
| `[seed, 31]` | Ridged octaves; the same for all three levels |
| `[seed, 32]` | Ridged crinkle shift; the same direction for all three levels |
| `[seed, 21]` | Voronoi warps and cells |
| `[seed, 15]` | Drift wind and streaks |
| `[seed + region_seed, 60 + k]` | Region fields: k = 0 detail, 1 soft, 2 drift, 3 underlayer, 4 strength (region_seed 0 by default) |
| `[seed, 11]`, `[seed, 12]`, `[seed, 16]` | Underlayer splits, coarse bend, fine bend |
| `[seed, 7]` | Fracture systems, as b3b's |
| `[seed, 70]`, `[seed, 71]` | Lead styles; ragged noise |

### 4.1 Setup

```python
"""The settled closed sea ice look of the solid ice lab (2026-10-12)."""

import dataclasses

import numpy as np
from scipy.ndimage import gaussian_filter1d

from texture_pipeline import sea_ice_look as sil
from texture_pipeline.sea_ice import texel_km

SIZE = 1024
"""Texels along each side of the lab's square."""

KM = texel_km(4096)
"""The polar cap's spacing, 2.873 km per texel."""

sil.texel_km = lambda size: KM
"""Lab only: the tracked helpers read the spacing from texel_km(size); this
points them at the cap's spacing on the 1024 square. Not needed on the cap."""

LIGHT = 0.6
"""Direction the relief is lit from, radians from the columns toward the rows."""

FILL = np.array((10, 30, 60), dtype=np.float32)
"""The sea's fill, which leads show."""


def rng(seed: int, stream: int) -> np.random.Generator:
    return np.random.default_rng([seed, stream])


def shaded(slope: np.ndarray, light: float, shadow: float) -> np.ndarray:
    return light * np.maximum(slope, 0) - shadow * np.maximum(-slope, 0)


def activity_map(noise: np.ndarray, lo: float, hi: float, floor: float) -> np.ndarray:
    return (floor + (1 - floor) * sil.smoothstep(lo, hi, noise)).astype(np.float32)


def toward_light(height: np.ndarray) -> np.ndarray:
    """The height's slope along the light, standard deviation 1."""
    gr, gc = np.gradient(height)
    return sil.normalized(np.float32(np.sin(LIGHT)) * gr + np.float32(np.cos(LIGHT)) * gc)
```

### 4.2 Settled values

```python
ACTIVITY_KM = (300.0, 1500.0)
HARD_ACTIVITY = (-0.3, 1.0, 0.1)
"""b3b's activity: the noise's step from, to, and the floor."""
SOFT_ACTIVITY = (-1.5, 1.2, 0.15)

RIDGED = {"coarse_km": 40.0, "fine_km": 2.5, "offset": 1.0, "octave_gain": 1.5,
          "warp_band_km": (6.0, 25.0), "light": 0.025, "shadow": 0.05}
DETAILS = (
    {**RIDGED, "exponent": 3.0, "persistence": 0.75, "warp_km": 1.5},
    {**RIDGED, "exponent": 3.0, "persistence": 0.9, "warp_km": 2.5},
    {**RIDGED, "exponent": 4.0, "persistence": 1.0, "warp_km": 3.5},
)

VORONOI = {
    "levels": ((20.0, 0.35, 0.06), (60.0, 0.5, 0.08), (180.0, 0.8, 0.08)),
    "warp_km": 5.0, "warp_band_km": (20.0, 80.0),
    "fine_warp_km": 1.5, "fine_warp_band_km": (6.0, 20.0),
    "crease_km": 1.0,
}
"""Per level: cell size, share of points kept, crease depth."""

DRIFT = {"km": (8.0, 30.0), "aspect": 6.0, "zone": (0.3, 0.6),
         "light": 0.03, "shadow": 0.05, "turn_km": (800.0, 3000.0), "turn": 0.5, "wind": 0.7}

REGIONS = {
    "region_km": (250.0, 900.0), "region_seed": 0,
    "detail_steps": ((-0.2, -0.8), (0.2, 0.8)),
    "soft_step": (0.0, 0.8), "drift_step": (0.5, 1.1), "under_step": (0.3, 1.0),
    "gain": (0.35, 0.85), "gain_step": (-1.0, 1.0), "voronoi_weight": 1.5,
}

UNDER = {
    "depth_flat": 6.0, "depth_full": 16.0, "min_km": 6.0, "spread": 0.7, "jitter": 0.4,
    "fold": 0.06, "fold_from": (5, 8), "fold_km": (2.0, 8.0),
    "warp_km": 10.0, "warp_band_km": (30.0, 120.0),
    "fine_warp_km": 2.0, "fine_warp_band_km": (6.0, 20.0),
    "light": 1.0, "shadow": 1.6, "weight": 1.2,
}
"""The optional underlayer (r2-09, r3-09); off in the default."""

THIN_LEADS = dataclasses.replace(
    sil.LOOK,
    fracture_width_km=(0.7, 2.5),
    fracture_darkness=(0.06, 0.2),
    fracture_open_darkness=(0.25, 0.5),
)
FRACTURE_DENSITY = 150 / 15.0e6
"""Systems per square kilometer: the north's 150 over its March ice."""

LEADS = {"path": ("echelon",), "echelon": {}, "pinch": {"share": 0.2, "scale": (0.6, 1.8)}, "dark": 0.75}
"""The default: en echelon and pinched. "path" is a tuple of path styles
applied in order ("echelon", "jogs", "zigzag"); "pinch", "ragged" and
"stretches" are keys, as in r3-01 to r3-06."""
```

### 4.3 Relief generators

```python
def relief_ridged(p: dict, seed: int) -> np.ndarray:
    """Ridged multifractal, each octave crinkled and gated by the coarser one."""
    r = rng(seed, 31)
    shift = sil.curl_shift(SIZE, p["warp_band_km"], p["warp_km"], rng(seed, 32))
    size_km = p["coarse_km"]
    height = np.zeros((SIZE, SIZE), dtype=np.float32)
    weight = np.ones((SIZE, SIZE), dtype=np.float32)
    octave = 0
    while size_km >= p["fine_km"]:
        noise = sil.warped(sil.band_noise(SIZE, (size_km / 1.5, size_km * 1.5), r), shift)
        signal = np.clip(p["offset"] - np.abs(noise), 0, None) ** p["exponent"] * weight
        height += p["persistence"] ** octave * signal
        weight = np.clip(signal * p["octave_gain"], 0, 1)
        size_km /= 2
        octave += 1
    return shaded(toward_light(height), p["light"], p["shadow"])


def relief_voronoi(p: dict, seed: int) -> np.ndarray:
    """Creases along the borders of warped cells at several sizes."""
    r = rng(seed, 21)
    shift = sil.curl_shift(SIZE, p["warp_band_km"], p["warp_km"], r)
    shift += sil.curl_shift(SIZE, p["fine_warp_band_km"], p["fine_warp_km"], r)
    crease = np.zeros((SIZE, SIZE), dtype=np.float32)
    width = p["crease_km"] / KM
    for cell_km, keep, depth in p["levels"]:
        _, border = sil.facet_cells(SIZE, cell_km, shift, r, keep)
        crease = np.maximum(crease, depth * np.exp(-((border / width) ** 2)))
    return -crease


def drift_texture(d: dict, seed: int, activity: np.ndarray, mask: np.ndarray) -> np.ndarray:
    """Streaks along a slowly turning wind, lit, where activity and mask allow."""
    r = rng(seed, 15)
    angle = sil.band_noise(SIZE, d["turn_km"], r) * np.float32(d["turn"]) + np.float32(d["wind"])
    doubled = np.stack([np.cos(2 * angle), np.sin(2 * angle)]).astype(np.float32)
    height = sil.steered_noise(SIZE, d["km"], doubled, d["aspect"], r)
    slope = toward_light(height)
    zone = sil.smoothstep(*d["zone"], activity) * mask
    return zone * shaded(slope, d["light"], d["shadow"])
```

### 4.4 Optional underlayer

The draws the lab made for features left out here (facet steps, dark creases, rubble bands) are kept, marked, so the folds come out as the lab drew them.

```python
def split_folds(p: dict, seed: int, activity: np.ndarray, shift: np.ndarray) -> np.ndarray:
    """Recursive straight splits, each a lit fold within its cell."""
    r_ = rng(seed, 11)
    rows, cols = np.mgrid[0:SIZE, 0:SIZE].astype(np.float32)
    rows += shift[0]
    cols += shift[1]
    r = rows.ravel().astype(np.float64)
    c = cols.ravel().astype(np.float64)
    del rows, cols
    act = activity.ravel().astype(np.float64)
    cell = np.zeros(r.size, dtype=np.int64)
    relief = np.zeros(r.size, dtype=np.float32)
    depth_flat, depth_full = p["depth_flat"], p["depth_full"]
    fold_from = p["fold_from"]
    light = (np.sin(LIGHT), np.cos(LIGHT))
    for level in range(int(np.ceil(depth_full + 0.5))):
        n = 1 << level
        count = np.bincount(cell, minlength=n).astype(np.float64)
        safe = np.maximum(count, 1)
        mr = np.bincount(cell, r, n) / safe
        mc = np.bincount(cell, c, n) / safe
        dr = r - mr[cell]
        dc = c - mc[cell]
        vrr = np.bincount(cell, dr * dr, n) / safe
        vcc = np.bincount(cell, dc * dc, n) / safe
        vrc = np.bincount(cell, dr * dc, n) / safe
        theta = 0.5 * np.arctan2(2 * vrc, vrr - vcc)
        phi = theta + r_.normal(0, p["spread"], n)
        lam = 0.5 * (vrr + vcc) + np.sqrt(0.25 * (vrr - vcc) ** 2 + vrc**2)
        offset = r_.uniform(-p["jitter"], p["jitter"], n) * np.sqrt(lam)
        mact = np.bincount(cell, act, n) / safe
        allowed = depth_flat + (depth_full - depth_flat) * mact + r_.uniform(-0.5, 0.5, n)
        split = (count > 0) & (level < allowed) & (count * KM * KM > p["min_km"] ** 2)
        r_.uniform(0.3, 1.0, n), r_.choice([-1.0, 1.0], n)  # the lab's facet steps
        k = np.clip((level - fold_from[0]) / max(fold_from[1] - fold_from[0], 1), 0, 1)
        lit = np.cos(phi) * light[0] + np.sin(phi) * light[1]
        ridge = p["fold"] * k * r_.uniform(0.4, 1.0, n) * r_.choice([-1.0, 1.0], n) * lit
        w = np.clip(0.25 * np.sqrt(lam) * KM, *p["fold_km"]) / KM
        r_.uniform(0.4, 1.0, n), r_.random(n)  # the lab's creases and rubble bands
        d = dr * np.cos(phi)[cell] + dc * np.sin(phi)[cell] - offset[cell]
        s = split[cell]
        fade = np.exp(-np.abs(d) / w[cell])
        relief += np.where(s, np.sign(d) * fade * ridge[cell], 0).astype(np.float32)
        cell = cell * 2 + (s & (d > 0))
    return relief.reshape(SIZE, SIZE)


def relief_under(p: dict, seed: int, activity: np.ndarray) -> np.ndarray:
    shift = sil.curl_shift(SIZE, p["warp_band_km"], p["warp_km"], rng(seed, 12))
    shift += sil.curl_shift(SIZE, p["fine_warp_band_km"], p["fine_warp_km"], rng(seed, 16))
    return shaded(split_folds(p, seed, activity, shift), p["light"], p["shadow"])
```

### 4.5 The surface

```python
def settled_surface(
    seed: int, act_noise: np.ndarray, p: dict = REGIONS, under: dict | None = None
) -> tuple[np.ndarray, np.ndarray]:
    """The brightness factor and the activity at concentration 1.

    :param act_noise: band_noise at ACTIVITY_KM, standard deviation 1.
    :param under: UNDER to draw the underlayer, None to leave it off.
    """
    band = p["region_km"]
    base_seed = seed + p["region_seed"]
    detail_n, soft_n, drift_n, under_n, gain_n = (
        sil.band_noise(SIZE, band, rng(base_seed, 60 + k)) for k in range(5)
    )
    (lo1, hi1), (lo3, hi3) = p["detail_steps"]
    w3 = sil.smoothstep(lo3, hi3, detail_n)
    w1 = sil.smoothstep(lo1, hi1, detail_n)
    w2 = 1 - w1 - w3
    hard = activity_map(act_noise, *HARD_ACTIVITY)
    soft_activity = activity_map(act_noise, *SOFT_ACTIVITY)
    soft = sil.smoothstep(*p["soft_step"], soft_n)
    activity = ((1 - soft) * hard + soft * soft_activity).astype(np.float32)
    relief = w1 * relief_ridged(DETAILS[0], seed)
    relief += w2 * relief_ridged(DETAILS[1], seed)
    relief += w3 * relief_ridged(DETAILS[2], seed)
    relief += soft * p["voronoi_weight"] * relief_voronoi(VORONOI, seed)
    if under is not None:
        share = sil.smoothstep(*p["under_step"], under_n)
        relief += share * under["weight"] * relief_under(under, seed, activity)
    g0, g1 = p["gain"]
    gain = g0 + (g1 - g0) * sil.smoothstep(*p["gain_step"], gain_n)
    out = 1 + gain * activity * relief
    mask = sil.smoothstep(*p["drift_step"], drift_n)
    out += gain * drift_texture(DRIFT, seed, activity, mask)
    return out.astype(np.float32), activity
```

### 4.6 Leads

```python
STEP = 0.8
"""Longest segment drawn, in texels: the drawing reaches about a segment's
middle only, so paths are densified to this."""


def lead_systems(seed: int, activity: np.ndarray, look=THIN_LEADS):
    """b3b's fracture systems and ranks, seeded by the hard activity."""
    r = rng(seed, 7)
    count = round(FRACTURE_DENSITY * (SIZE * KM) ** 2)
    direction = sil.band_noise(SIZE, look.fracture_turn_km, r) * np.float32(np.pi / 2)
    density = look.fracture_activity + (1 - look.fracture_activity) * activity
    flat = density.ravel() / density.sum()
    picked = r.choice(SIZE * SIZE, size=count, p=flat)
    seeds = np.stack([picked // SIZE, picked % SIZE], axis=-1) + r.random((count, 2))
    traced = sil.fracture_paths(SIZE, seeds, direction, look, r)
    return traced, r.random(count).astype(np.float32)


def densify(points: np.ndarray) -> np.ndarray:
    out = [points[:1]]
    for a, b in zip(points[:-1], points[1:], strict=True):
        pieces = max(int(np.ceil(np.linalg.norm(b - a) / STEP)), 1)
        t = (np.arange(1, pieces + 1) / pieces)[:, None]
        out.append(a + t * (b - a))
    return np.concatenate(out)


def arc_km(points: np.ndarray) -> np.ndarray:
    steps = np.linalg.norm(np.diff(points, axis=0), axis=1)
    return np.concatenate([[0.0], np.cumsum(steps)]) * KM


def normals(points: np.ndarray) -> np.ndarray:
    tangent = np.gradient(points, axis=0)
    tangent /= np.maximum(np.linalg.norm(tangent, axis=1, keepdims=True), 1e-9)
    return np.stack([-tangent[:, 1], tangent[:, 0]], axis=1)


def runs(length: float, run_km, r) -> np.ndarray:
    """Cut positions along ``length`` km at spacings uniform over ``run_km``."""
    cuts = [0.0]
    while cuts[-1] < length:
        cuts.append(cuts[-1] + r.uniform(*run_km))
    return np.array(cuts)


def smooth_noise(n: int, span: float, r) -> np.ndarray:
    span = max(int(span), 1)
    noise = np.convolve(r.standard_normal(n + 2 * span), np.hanning(2 * span + 1), "same")
    noise = noise[span : span + n]
    return noise / max(float(noise.std()), 1e-6)


def zigzag(points, p, r):
    s = arc_km(points)
    cuts = runs(s[-1], p.get("seg_km", (3.0, 12.0)), r)
    cuts[-1] = s[-1]
    base = np.stack([np.interp(cuts, s, points[:, k]) for k in range(2)], axis=1)
    n = normals(base)
    amp = r.uniform(*p.get("amp_km", (1.0, 3.0)), len(cuts)) / KM
    amp *= np.where(np.arange(len(cuts)) % 2 == 0, 1.0, -1.0)
    amp[0] = amp[-1] = 0.0
    return [densify(base + n * amp[:, None])]


def jogs(points, p, r):
    s = arc_km(points)
    cuts = runs(s[-1], p.get("every_km", (20.0, 80.0)), r)[1:-1]
    shift = np.zeros_like(s)
    ramp = p.get("ramp_km", 1.5)
    for cut in cuts:
        jog = r.choice([-1.0, 1.0]) * r.uniform(*p.get("jog_km", (1.5, 4.0)))
        shift += jog * sil.smoothstep(cut - ramp / 2, cut + ramp / 2, s)
    return [points + normals(points) * (shift / KM)[:, None]]


def echelon(points, p, r):
    s = arc_km(points)
    cuts = runs(s[-1], p.get("piece_km", (10.0, 40.0)), r)
    cuts[-1] = s[-1]
    turn = r.choice([-1.0, 1.0]) * r.uniform(*p.get("turn", (0.1, 0.3)))
    pieces = []
    for j, (a, b) in enumerate(zip(cuts[:-1], cuts[1:], strict=True)):
        over = r.uniform(*p.get("overlap_km", (2.0, 6.0)))
        span = np.linspace(a - over, b + over, max(int((b - a + 2 * over) / KM / STEP), 2))
        inside = np.clip(span, 0, s[-1])
        piece = np.stack([np.interp(inside, s, points[:, k]) for k in range(2)], axis=1)
        if len(piece) < 2:
            continue
        tangent = piece[-1] - piece[0]
        tangent /= max(float(np.linalg.norm(tangent)), 1e-9)
        piece = piece + np.outer(span - inside, tangent) / KM
        centre = piece.mean(axis=0)
        c, sn = np.cos(turn), np.sin(turn)
        piece = centre + (piece - centre) @ np.array([[c, -sn], [sn, c]])
        offset = (1 if j % 2 == 0 else -1) * r.uniform(*p.get("offset_km", (1.0, 3.0))) / 2
        normal = np.array([-tangent[1], tangent[0]])
        pieces.append(piece + normal * offset / KM)
    return pieces


PATHS = {"zigzag": zigzag, "jogs": jogs, "echelon": echelon}


def profile(points, width, darkness, p, r, piece_taper: bool):
    """Per segment width and darkness: draw_fractures' swell and darkness
    variation, the end taper, and the pinch or stretches along the fracture."""
    n = len(points) - 1
    span = 60.0 / KM / STEP
    w = width * np.clip(1 + 0.25 * smooth_noise(n, span, r), 0.7, 1.3)
    t = (np.arange(n) + 0.5) / n
    low = 0.15 if piece_taper else 1 / 3
    taper = low + (1 - low) * np.sin(np.pi * t) ** 0.5
    d = darkness * taper * np.clip(1 + 0.35 * smooth_noise(n, span, r), 0.3, 1.7)
    s = arc_km(points)
    mid = 0.5 * (s[:-1] + s[1:])
    if "pinch" in p:
        q = p["pinch"]
        cuts = runs(s[-1], q.get("run_km", (4.0, 25.0)), r)
        values = np.where(
            r.random(len(cuts)) < q.get("share", 0.3),
            0.0,
            r.uniform(*q.get("scale", (0.5, 1.6)), len(cuts)),
        )
        step = values[np.clip(np.searchsorted(cuts, mid) - 1, 0, len(cuts) - 1)]
        w = w * gaussian_filter1d(step, 0.6)
    if "stretches" in p:
        q = p["stretches"]
        cuts = runs(s[-1], q.get("run_km", (10.0, 50.0)), r)
        is_open = r.random(len(cuts)) < q.get("open_share", 0.3)
        values = np.where(
            is_open,
            r.uniform(*q.get("open", (0.25, 0.45)), len(cuts)),
            r.uniform(*q.get("frozen", (0.04, 0.1)), len(cuts)),
        )
        step = values[np.clip(np.searchsorted(cuts, mid) - 1, 0, len(cuts) - 1)]
        d = taper * gaussian_filter1d(step, 0.6) * p.get("dark", 1.0)
    return w, d


def draw(fractures, ragged: np.ndarray | None) -> np.ndarray:
    """The share of the sea's fill: each texel takes its strongest segment,
    a box filter of a texel against a line of the segment's width."""
    a = np.concatenate([f[0][:-1] for f in fractures])
    b = np.concatenate([f[0][1:] for f in fractures])
    width = np.concatenate([f[1] for f in fractures])
    dark = np.concatenate([f[2] for f in fractures])
    reach = int(np.ceil(float(width.max()) * (1.6 if ragged is not None else 1) / 2 + 1)) + 1
    middle = np.floor((a + b) / 2).astype(np.int64)
    ab = b - a
    length2 = np.maximum((ab**2).sum(axis=1), 1e-9)
    out = np.zeros(SIZE * SIZE, dtype=np.float32)
    for dr in range(-reach, reach + 1):
        for dc in range(-reach, reach + 1):
            cell = middle + np.array([dr, dc])
            inside = (cell >= 0).all(axis=1) & (cell < SIZE).all(axis=1)
            t = np.clip(((cell - a) * ab).sum(axis=1) / length2, 0, 1)
            distance = np.linalg.norm(cell - (a + t[:, None] * ab), axis=1)
            w = width
            if ragged is not None:
                rc = np.clip(cell, 0, SIZE - 1)
                w = width * ragged[rc[:, 0], rc[:, 1]]
            share = np.clip(w / 2 + 0.5 - distance, 0, np.minimum(w, 1))
            value = share * dark
            keep = inside & (value > 0.002)
            np.maximum.at(out, cell[keep, 0] * SIZE + cell[keep, 1], value[keep].astype(np.float32))
    return np.clip(out, 0, 1).reshape(SIZE, SIZE)


def jagged(p: dict, seed: int, activity: np.ndarray) -> np.ndarray:
    """The leads' share of the fill in style ``p``, every system drawn with
    the rank fade, as at concentration 1."""
    traced, ranks = lead_systems(seed, activity)
    r = rng(seed, 70)
    names = list(p.get("path", ()))
    styles = [PATHS[name] for name in names]
    fractures = []
    for number, system in enumerate(traced):
        fade = float(np.clip((1.0 - ranks[number]) / 0.15, 0, 1))
        if fade <= 0:
            continue
        for path, width, darkness in system:
            if len(path) < 2:
                continue
            paths = [densify(np.asarray(path, dtype=np.float64))]
            for name, style in zip(names, styles, strict=True):
                paths = [q for piece in paths if len(piece) >= 2 for q in style(piece, p.get(name, {}), r)]
            for piece in paths:
                if len(piece) < 2:
                    continue
                w, d = profile(piece, width, darkness * fade * p.get("dark", 1.0), p, r, echelon in styles)
                fractures.append((piece, w, d))
    ragged = None
    if "ragged" in p:
        q = p["ragged"]
        noise = sil.band_noise(SIZE, q.get("band_km", (3.0, 12.0)), rng(seed, 71))
        ragged = np.clip(1 + q.get("amp", 0.7) * noise, 0, 2.5).astype(np.float32)
    return draw(fractures, ragged)
```

### 4.7 Composition and the default

```python
def compose(brightness: np.ndarray, open_: np.ndarray | None, color) -> np.ndarray:
    """The ice's sRGB at concentration 1, as month_cap and composite_face make it."""
    ice = np.clip(brightness[..., None] * np.asarray(color, dtype=np.float32), 0, 255)
    if open_ is not None:
        ice += open_[..., None] * (FILL - ice)
    return np.clip(ice + 0.5, 0, 255).astype(np.uint8)


def settled_default(seed: int = 2026, color=(247, 250, 251)) -> np.ndarray:
    """r3-11 on the 1024 square at full resolution, before the face's
    area average and the encode."""
    act_noise = sil.band_noise(SIZE, ACTIVITY_KM, rng(seed, 5))
    brightness, _ = settled_surface(seed, act_noise)
    hard = activity_map(act_noise, *HARD_ACTIVITY)
    return compose(brightness, jagged(LEADS, seed, hard), color)
```

**Verification.** The fenced blocks above were extracted from this file, in order, into one module and run against the lab. r3-11's brightness and activity, the leads' share of the fill and the activity noise came out identical to the lab's, maximum difference 0; the shipped north winter tile (area average to 600 texels and the JPEG XL round trip) matched the lab's saved output exactly, maximum difference 0 of 255; r3-09's surface, with the underlayer, was identical too; and the leads of every style, r3-01 to r3-06 and r3-10's jogs with en echelon, matched the lab's to the last bit. The check took one script outside the tree (`tmp/sea-ice-lab/solid/work/verify_doc.py`, untracked).

## 5. Transfer notes for sea_ice_look.py

### 5.1 What replaces what

- `crumple()` goes: the straight folds (`folds()`, `fold_*`), the ridged noise at aspect 1.3 with its creases (`crumple_octaves`, `crumple_aspect`, `crumple_turn_km`, `crumple_warp_*`, `crumple_gate`, `crease_width`) and the plates (`crumple_facets`, `facet_crease_km`, `crumple_plates`, `plates_flat`). In its place: the ridged relief blended over three detail levels (`relief_ridged`), the Voronoi creases in soft areas (`relief_voronoi`, which reuses the tracked `facet_cells` for the borders and ignores its shades), the drift (`drift_texture`, on the tracked `steered_noise`) and, optionally, the underlayer.
- `activity_field()` stays as the hard activity; a soft activity from the same noise and the soft region field blend the two.
- In `month_cap`, the crumple branch's `texture = thin * (1 + active * relief + plates)` becomes `thin * (1 + gain * active * relief + gain * drift)`, with `gain` the strength region field, `relief` the blended relief and `drift` the drift streaks. The lab computed the drift's zone from the activity at concentration 1; on the cap that zone can use the month independent activity, so the drift is a fixed field.
- The fractures keep `fracture_paths` and the seeding of `fractures()`. `draw_fractures` is replaced by `draw` with the per segment `profile` and the en echelon and pinch styles, with `THIN_LEADS`' widths and darkness. `month_cap` reads `Fractures.rank` per texel to draw a share of the systems where the concentration falls; `jagged` applies the rank fade itself, as at concentration 1, and returns no rank. With `fracture_share` at (1.0, 1.0), as in b3b, the two agree everywhere; if the share is ever below 1, `draw` must also return the rank of the system each texel's value came from, as `draw_fractures` does with its sort.
- The `Fractures.systems` summary that `fracture_stats` reads comes from `fracture_paths` and is unaffected.

### 5.2 `Look` tunables to add

With the settled values (section 3 for their meaning): the ridged relief's coarsest and finest feature (40, 2.5 km), offset 1.0, octave gain 1.5, crinkle band (6, 25) km, light 0.025 and shadow 0.05, and per detail level (exponent, persistence, crinkle rms): (3, 0.75, 1.5 km), (3, 0.9, 2.5 km), (4, 1.0, 3.5 km); the light direction 0.6 rad; the region band (250, 900) km, the detail steps (-0.2, -0.8) and (0.2, 0.8), the soft step (0, 0.8), the drift step (0.5, 1.1), the underlayer step (0.3, 1.0) and the strength (0.35, 0.85) over the step (-1, 1); the soft activity range (-1.5, 1.2) and floor 0.15; the Voronoi levels ((20 km, 0.35, 0.06), (60 km, 0.5, 0.08), (180 km, 0.8, 0.08)), crease 1 km, warps 5 km at (20, 80) km and 1.5 km at (6, 20) km, weight 1.5; the drift band (8, 30) km, aspect 6, zone (0.3, 0.6), light 0.03, shadow 0.05, wind 0.7 rad turning by 0.5 rad over (800, 3000) km; an underlayer switch with the values of `UNDER`; the lead width (0.7, 2.5) km, darkness (0.06, 0.2) and open darkness (0.25, 0.5) in the existing fields; the en echelon piece (10, 40) km, turn (0.1, 0.3) rad, offset (1, 3) km, overlap (2, 6) km and end taper 0.15; the pinch run (4, 25) km, share 0.2 and scale (0.6, 1.8); and a lead darkness factor, 0.75 for en echelon with pinch.

### 5.3 Fixed on the cap plane, the same in every month

Every new field (the five region fields, the three detail reliefs or their blend, the Voronoi creases, the drift, the underlayer, the lead shares) is generated once per hemisphere in `prepare()` and kept in `Fields`, as the crumple's slope, creases and plates are now (float16 is enough for all of them); `month_cap` only composes. Nothing new depends on the month.

### 5.4 Seeds per hemisphere

The lab drew everything from one seed and used the north's lead density for every color. On the cap each stream goes through `_rng(look, hemisphere, stream)`, so north and south differ; new stream numbers are needed for the ridged octaves, the crinkle, the Voronoi cells, the drift, the five region fields and the lead styles (the lab's 5, 7, 11, 12, 15, 16, 21, 31, 32, 60 to 64, 70 and 71 collide with the tracked 0 to 9 only at 5 and 7, which are the activity and the fractures in both, and so stay). The detail levels must share the octave and crinkle streams for the blend to be seamless. The south keeps its 70 systems over a larger ice area, so its leads are sparser than the lab's; that was not reviewed.

### 5.5 Activity and concentration

The lab worked at concentration 1 only. `month_cap`'s rule stays: `active = 1 - (1 - base) * (1 - activity_edge * smoothstep(0.98, 0.75, conc))`, with `base` now the hard and soft blend; the strength field multiplies the result. The fractures' seeding uses the hard activity, as now.

### 5.6 Cost

Measured at 1024 on this machine, one process: the default surface 2.1 s (3.4 s with the underlayer) and the leads 0.8 s; the traced peak was 140 MiB for the surface, about 35 float32 fields of 4 MiB, and 28 MiB for the leads. On the cap, 16 times the texels: the FFT fields grow by a little more than 16 times; the lab draws the octave noise three times (once per detail level), which the cap should not: draw the five octaves once, then warp and fold them per level, which gives the same result. `facet_cells` on the cap already runs twice in the current crumple; the Voronoi layer needs three levels. Memory is the binding constraint: a float32 field at 4096 is 64 MiB, and the lab's `settled_surface` holds about 35 full fields at its peak, about 2.2 GiB at 4096 if done the same way, so it must work in bands of `BAND_ROWS` where it can, and the region fields, at 250 km and coarser, can be drawn on a 1024 cap and zoomed up, as `edge_orientation` does. The leads' cost is close to the current fractures': the cap keeps its 150 and 70 systems, and the drawing visits about the same texels.

### 5.7 Open, for the floes lab and later

Everything below 100% concentration: how the activity's rise toward the edge, the region fields and the drift behave in the marginal ice; the leads where the concentration falls, where `fracture_share` and the floes' water meet; whether the floes should take the soft areas' look. The summer palette with the melt tone was drawn for every sample but not reviewed. The south's lead density and its colors' extra headroom (both sides of the relief show there) were not reviewed separately.
