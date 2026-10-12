# Research: Sea Ice at Partial Coverage, the Floes (2026-10-12)

The baked sea ice of [the sea ice look plan](2026-10-12-sea-ice-look-plan.md) draws partial coverage with the `b3c-floes` variant: the edge model's streaked threshold, filled floes on a 20 km grid that open at levels of their own, and gray or dark water between them. A full preview run takes minutes, too slow to try floes by eye, so a second standalone lab, after the one in [the solid ice research](2026-10-12-sea-ice-solid-research.md), drew sea ice at partial concentration on synthetic data only and showed each sample the way the app gets it. Five rounds went to the user, who closed the fifth with "ok, let's use r5-06." This document records the method, the rounds, the look the user settled on with every parameter, the code that draws it, and what moving it into `sea_ice_look.py` involves. The closed ice's surface and leads are the solid document's; here they are inputs.

The lab lived under `tmp/sea-ice-lab/floes/`, which is not tracked; everything needed to reproduce its settled coverage is in sections 3 and 4.

## 1. Summary and method

- The canvas. Every row is drawn on one 2048 x 2048 canvas at the polar cap's spacing, `texel_km(CAP_SIZE)` = 2.873 km, 5,884 km across, which holds all samples side by side: twelve uniform concentrations (100, 95, 90, 85, 80, 70, 60, 50, 40, 30, 20 and 15%) of 512 x 512 texels (1,471 km); a ramp from 100% to 0% over 400 km, 512 x 1024; and a noisy, record-like field, 512 x 1024, a 25 km grid of smoothed noise (0.78 plus 0.16 standard deviations, clipped to 0.4 to 1) read bilinearly. Rounds 4 and 5 put coast samples on the canvas instead (section 3.9). Noise fields are stationary textures over the whole canvas; anything that reads the concentration reads it inside its own sample. As in the solid lab, `sea_ice_look.texel_km` was pointed at the cap's spacing so the tracked helpers draw at 2.873 km on the smaller square.
- What every row shares. The concentration is read through the pipeline's eddy shift (`curl_shift` at 50 to 150 km with 30 km rms, the `make_eddies` part, linear inside each sample), as `month_cap` reads the record; the coast samples are read without it. The interior is `b3c-floes`' crumpled snow with its thin ice shading (`thin_shade` 0.82 rising to 1 over concentrations 0.12 to 0.88), and its fracture systems (214 on the canvas, one per 100,000 square kilometers of ice, as the north's cap has), so only the floes and the edge differ between rows. The solid document replaces the crumple and the leads.
- Color. Composited as the pipeline does: `fill + coverage * (clip(brightness * color, 0, 255) - fill)`, fill (10, 30, 60), the north winter color (247, 250, 251) for every sample and the south winter color (236, 240, 244) repeated for the ramp and the noisy field.
- As shipped. Each sample is area averaged (Pillow's BOX filter, as `cube.area_average`) to the face's 4.89 km per texel, 512 texels to 301, then sent through a JPEG XL round trip at quality 85, effort 7, as `processing.encode_jxl` encodes the faces.
- Views. Each sample at face resolution, and a 256 texel crop of the decoded face magnified 3 times with bilinear filtering. Round 5 instead showed six crops of 20 face texels (98 km) centered on the coastline, magnified 32 times with bilinear filtering as the GPU magnifies the face at a close zoom, and one crop of 102 texels (499 km) magnified 4 times.
- Coverage is counted as the pipeline's metric (c) counts it: the mean of the coverage, so a gray gap that shows 45% of the fill counts as 55% ice. Every row was held within about 2 points of its sample's concentration, and a sample further off was flagged. Under the ramp the page drew coverage against concentration; under the noisy field, coverage per 10 point bin.
- References. Crops of the three MODIS mosaics the solid lab used, area averaged to 4.89 km per texel. The floes lab estimated their scale at 2.8 km per pixel, good to about 15%: Greenland, about 1,050 km across at 77 N, spans 360 to 400 pixels in the two Arctic mosaics, and the Antarctic coast at 66 to 70 S lies about 900 pixels from the pole, where the mosaic's seams meet, against about 2,600 km. The solid document estimated 3.8 km per pixel for the Arctic mosaics (Greenland at 70 to 72 N) and 3.0 for the Antarctic one; the two Arctic estimates disagree by about a third and were not reconciled. At 4.9 km per texel the references' loose pack reads as speckle: white specks and clumps of one to three texels on dark water, or dark specks in white, gathered into loose and compact regions with sharp, irregular borders, and swirls near the edge. No cell outlines show; single floes are visible only where they are tens of kilometers across.
- Speed. A row generates in 3 to 6 s once its noise fields are cached, and round 4's 32 rows rebuilt in about 42 s on four processes. Every field is seeded from 2004, the pipeline's `Look.seed`.

## 2. Rounds

| Round | Rows | The user's verdict | Learned |
|---|---|---|---|
| 1 | b3c-floes (the current look), base (the lab's calibrated base), speckle (water where 2.9 to 25 km noise falls below a level, clustered by a 60 to 250 km region noise), mottle, cascade (cells of 64 to 4 km resolving coarsest first), speckle-only (speckle with the edge model off), eddies (the edge advected by swirls), gap-dark, floe-shade, combo | Speckle: "not bad. There's a lot of vertical brush strokes of water all over the place at 70% and below, those look bad, but otherwise it's fine." Mottle: "similar to speckle, but more uniform, which makes it less interesting, and again with the vertical brush strokes of water." Cascade: "interessting, with bigger gaps, I like it. vertical strokes again." Speckle-only: "yeah, those look a lot better. now, at lower ice coverage, speckle is too noisy." Eddies: "looks too artificial, that's out", which took combo with it. Gap-dark: "has the streaks again"; floe-shade "as well". "where's cascade without the brush strokes?" | The strokes were the edge model (finding 1). Speckle at 4.9 km reads like the references; the cascade's larger gaps were liked. |
| 2 | Edge model off in every row: r2-01 cascade only, r2-02 speckle or cascade per 16 km cell with a coarse schedule, r2-03 speckle with fading octaves, r2-04 and r2-05 speckle with dark gaps or shaded floes, r2-06 cascade with dark gaps and shaded floes, r2-07 cascade with the coarse schedule | r2-01: "I like it." r2-02: "Interesting idea. I like the bigger floes at low density, but there's too many tiny ones at higher ice coverage." r2-03: "Yeah, that's going in the right direction." r2-04, r2-05: "Too noisy." r2-06: "Also too noisy below 80%." r2-07: "Pretty good." Then asked for r2-02's big floes from 0 to 40%, r2-01 from 70 to 100% and a calm zone from 40 to 70% | The open ocean counted as 5% ice (finding 2). |
| 3 | r3-01 one cascade whose level scale follows r2-01 above 70% and r2-02 below 40%, coarsening between with no 8 or 4 km cells; r3-02 that zone at 50 to 80%; r3-03 the zone drawn by r2-03's speckle with its fine octaves faded, handed over region by region; r3-04 the zone calmed by clumping; r3-05 smaller floes at low coverage; r3-06 dark gaps in the pack | "I liked samples 01-04, slight preference for 03." Then: "let's do a final round to pin down the coast and the gray to dark transition." | Deciding cells whole (finding 3). |
| 4 | r4-01 to r4-04 one gap color per water patch on r3-03 (r4-01 random order, r4-02 largest first with the handover at 85% to 55%, r4-03 largest first at 95% to 45%, r4-04 r4-02 on r3-01); r4-05 to r4-09 a coast next to snowy land: no rule, the hard 50% rule in a 10 or 5 km band, a ragged 45 to 55% step in a 10 or 5 km band | "r4-03 looks good." The coast: "the coast sample was useless, since the entire coast is ice and looks just the same as the ice we generate. let's do the worst case instead: coast has a hard edge and green vegetation." | One color per patch, largest first (finding 4). |
| 5 | The worst case coast, green land with a hard edge: r5-01 no rule, r5-02 and r5-03 the hard rule in 10 and 5 km bands, r5-04 and r5-05 the ragged step in 10 and 5 km bands, r5-06 add-only in a 10 km band, r5-07 add-only with its extra ice taken back 10 to 40 km offshore | "ok, let's use r5-06." | Add-only (finding 5). |

Findings that hold beyond the samples:

1. **The edge model's streaks were the user's brush strokes, and the edge model is redundant once the floes draw all the water.** The edge model covers a texel by `smoothstep(t - 0.08, t + 0.08, c)` against a threshold `t` drawn from 10 to 120 km noise streaked 250 km along the edge's direction (`make_edge`: `edge_orientation`, `streak`). In the lab's uniform samples that direction was fixed down the columns, so wherever the concentration sat inside the threshold's range (0.12 to 0.88, squeezed to 0.12 to 0.7 in `b3c-floes`) the threshold drew long vertical strokes of water, in every round 1 row that kept it. On the cap they follow the edge but are the same streaks. Turned off, with the floes drawing all of the concentration's open water, nothing is lost: the ice edge on the ramp is drawn by the cascade's cells reaching the 0% line, ragged at the cells' size, and the loose ice at the edge by the big floes.
2. **The open ocean counted as 5% ice in round 2.** `b3c-floes` darkens its water to at most 95% of the fill (`floe_darkness` (0.45, 0.95)). With the edge model off, the open ocean beyond the ice is water of share 1 at that darkness, so it showed and counted as 5% ice, and round 2's ramps measured 1.5 to 2.4 points over their target. Dark water is now the fill itself. A fade from 0.95 to 1.0 over 20% to 2% instead drew a lighter band of water along the edge and was dropped.
3. **Cascade cells are decided whole.** Rounds 1 and 2 decided each texel of a cell from its own concentration and ice share, so where the concentration changes inside a cell the cell was cut along a contour of the concentration: on the ramp that drew floes and gaps with straight vertical edges, in the noisy field edges that follow the 25 km grid. From round 3 each cell resolves, and turns ice or water, from its mean concentration and mean ice share, so every floe edge is a cell border. The price is a bias toward ice, because the ice share is averaged over cells of different sizes at each level: round 3's samples ran from 0.9 points under to 1.8 over, most 0.5 to 1 point over.
4. **One gap color per water patch, the largest patches dark first.** Round 3 colored water per texel by the concentration, gray (45% of the fill) in the pack to dark (the fill) in the loose ice, so a water patch spanning a gradient shaded from gray to dark inside itself, which showed as soft gray smears. Round 4 gives each connected patch of water one color. In random order the small dark share at 80 to 85% can only be met by small patches, so r4-01 sprinkled dark specks into the gray pack; largest first instead skips any patch that would overshoot the share, so the pack stays gray down to about 75 to 80%, the first dark water is a large lead, and small cracks stay gray into the loose ice, as wide leads stay open while narrow cracks refreeze. A first version that spread the ranks by area instead of picking greedily left r4-02 at 80% without any dark patch and 2.7 points over. The user chose the wider handover, 95% to 45% (r4-03).
5. **The coast's worst case is green land, and add-only won there.** Round 4's coast sat next to snowy land, where any white rim merges with the land, so it showed nothing. Round 5 put green vegetation with a hard edge against the sea, which the pipeline's coast blend leaves alone. At 50% and above, the hard rule and add-only both draw a white rim about two face texels wide between the green land and any gray or dark patch that reached the coast; it reads as fast ice closing the pack against the land, though its inner edge runs parallel to the coast. Below 50% the hard rule and the ragged step open the band: floes that touched the land are cut back along the band's outer line, which leaves a dark channel along the coast and floe edges flattened parallel to it. The ragged step's partial coverage shows as blue gray texels along the coast rather than floes or gaps. Bands of 5 km are about one face texel and mostly merge with the coast's mixed texel. Add-only keeps the rim and leaves the floes below 50% as the generator drew them, with no channel. It cannot hold the band's coverage, since it adds ice and takes none away; r5-07 took the excess back 10 to 40 km offshore, which drew a looser, darker strip there at 50 to 70%, like a flaw lead, and was not chosen.

## 3. The settled look

Every value below is the one the lab drew with. The reference rows are r4-03 for the open sea (r3-03's water with r4-03's gap colors) and r5-06 for the coast (the same with the add-only rule); section 4's code reproduces both.

### 3.1 From concentration to coverage

1. Read the concentration `c` through the eddy shift, as `month_cap` does now; no edge model, so the ice is not thinned by a threshold.
2. Count the leads: the floes' water is calibrated so that the leads' open water and the floes' together meet the concentration (section 3.2).
3. Draw a regional ice target around the concentration (section 3.3).
4. Draw the floes with a cascade of cells, coarsest first, deciding each cell whole (sections 3.4 and 3.5).
5. Between 40 and 70% hand the floes over, region by region, to a faded speckle (section 3.6).
6. Force open water where the concentration is 0 (section 3.7).
7. Color the water, one color per patch (section 3.8).
8. Coverage `(1 - leads) * (1 - dark * water)`, then the coast rule (section 3.9).

### 3.2 The water share

- The leads' open share `gaps` (today `fractures.open` times the rank fade, after the solid document its `jagged` leads' share of the fill) is smoothed with a Gaussian of 40 km to `lost`.
- The ice the floes must keep is `kept = clip(c / max(1 - lost, 1e-4), 0, 1)`.
- The water's mean darkness at each concentration is `d = 0.45 + 0.55 * smoothstep(0.95, 0.45, c)`, the share of the water that turns dark in section 3.8.
- The floes' water share is `share = clip((1 - kept) / d, 0, 1)`, so that gray water counts as part ice.

### 3.3 The regional target

The ice share `ice = Phi(mu + 0.8 n)`, with n a band noise at 60 to 250 km and `mu = Phi^-1(1 - share) * sqrt(1 + 0.8^2)`, so its expectation is `1 - share`; it is 0 or 1 where `1 - share` is. Loose and compact regions alternate at 60 to 250 km. The speckle (section 3.6) reads the same noise for its water target, `Phi(mu' + 0.8 n)` with `mu'` from `share`, so where the cascade has more ice the speckle has more water.

### 3.4 The cascade

Five levels of cells, 64, 32, 16, 8 and 4 km: a jittered grid at each level (one point per grid cell, at a uniform offset inside it), each texel belonging to its nearest point after its coordinates are bent by a `curl_shift` with feature sizes `(cell / 2, cell * 2)` and an rms of 15% of the cell, so cell borders are curved. Each cell carries two uniforms, `u` (whether it resolves) and `v` (whether it is ice).

Coarsest first, a share of the cells still open resolves at each level, each to ice with probability `Phi(z + delta_k)`, deltas (2.0, 1.2, 0.5, -0.2, -0.6): coarse cells lean to ice, fine ones to water, which is what keeps the loose ice as big floes on dark water. The finest level resolves everything left. `z` comes from a table over 51 concentrations by 201 ice shares, solved so the expected ice is the target.

The share resolving at level k is `sigmoid(2 (k + 0.5 - scale))`, with the level scale piecewise linear in the concentration:

| Concentration | Scale | Source |
|---|---|---|
| 20% and below | 0.6 | r2-02's and r2-07's coarse schedule, the big floes |
| 40% | 0.867 | the same |
| 70% | 1.667 | r2-01's sized schedule, the cascade of the pack |
| 90% | 0.6 | the same |
| 100% | 0.0 | the same |

A scale of 0 resolves most of the area in 64 km cells, 1 in 32 km cells, 2 in 16 km cells. Between 40 and 70% the scale rises linearly, so the floes coarsen as the ice thins. In the calm zone, 40 to 70% with soft ends 5 points wide (`zone(c, 0.4, 0.7)`), every cell still open at the 16 km level resolves there, so the 8 and 4 km cells take nothing.

### 3.5 Whole cells, cut at the coastline

Each cell is decided from its mean concentration and mean ice share: whether it resolves, `u < share_k(mean c)`, and whether it is ice, `v < Phi(z(mean c, mean ice) + delta_k)`. Means are taken per cell and per tag: the lab tagged each sample, and in the coast samples land and sea, so no cell straddles two samples and a cell on the coastline is two cells. Without the cut, the land's 0% thins the floes along every coast.

### 3.6 The calm zone: faded speckle, handed over by region

Inside 40 to 70% the floes come from r2-03's speckle with its finest octave gone and the next one weak: a field mixing band noises at 8 to 25 km (weight 0.15), 25 to 60 km (0.7) and 60 to 120 km (0.5), normalized, is water where it falls below the level that leaves the regional water target wet, with a soft step of 0.12 either side (the level from a table of the field's own distribution). The handover is regional: with `pick = Phi(n)`, n a band noise at 30 to 120 km, a texel takes the speckle where `pick < zone(c, 0.4, 0.7)` and the cascade elsewhere, so whole 30 to 120 km regions switch, and the share of speckle regions rises across 35 to 45% and falls across 65 to 75%. The speckle's water is soft at its rims (between 0 and 1), unlike the cascade's.

### 3.7 Open water at 0%, and no edge model

Where `c <= 0` the water share is forced to 1. The concentration falls to 0 smoothly, so the cells there already have almost no ice; the clamp makes it a guarantee, which matters next to land. There is no edge model, so the ice edge is where the cascade's cells and the clamp put it.

### 3.8 Gap color

r4-03's rule. The patches are the 8-connected components of texels more than half water (land excluded), and every texel takes the color of its nearest patch, so the speckle's soft rims match their patch. A patch is gray, 45% of the fill (darkness 0.45), or dark, the fill itself (1.0), from its mean concentration. Within each 5 point bin of that mean, the patches are taken largest first, their size jittered by about a factor of two (key `-log(area) + 0.7 n`), and each is turned dark when that brings the dark water area closer to `sum(wet * smoothstep(0.95, 0.45, c_patch))`. The area that turns dark therefore matches the mean darkness the water share was calibrated with (section 3.2), whatever the patches' sizes. All gaps are gray above 95%, all are dark below 45%, and the large connected open ocean beyond the edge is one dark patch.

### 3.9 The coast rule: add-only in a 10 km band

r5-06's rule: in the sea within 10 km of the coastline, coverage is 1 wherever the concentration is 50% or more; below 50% nothing changes. The distance is the Euclidean distance transform of the sea to the nearest land texel, in km, less half a texel, so it is measured from the coastline rather than from the land texel's center; at 2.873 km per texel the band is about three and a half texels. The rule replaces the coverage after the leads and the gap color, so the rim is solid ice with the surface's brightness and no leads.

Its cost, on round 5's sample (sea only; the concentration falls along the shore from 100% to 0% and by 35 points per 1,000 km offshore): the 10 km band is covered 60.0% for a concentration of 49.5%, 10.5 points over; within 40 km of land 51.8% for 48.5%, 3.2 over; over the whole sea 37.8% for 38.5% (the reference 37.6%), so 0.2 points over the reference. The hard rule, which also opens the band below 50%, covered it 51.3%. Add-only cannot hold the band, by construction. The alternative r5-07, which lowered the concentration the generator draws from over the 30 km beyond the band by the rim's excess, held 49.6% within 40 km but drew a looser, darker strip offshore at 50 to 70%, and was rejected.

### 3.10 Reference numbers, and what was not reviewed

r4-03's coverage against its concentration after the floes, the leads and the gap colors, on the lab's canvas: 100%: 99.8, 95%: 95.5, 90%: 90.3, 85%: 84.6, 80%: 80.0, 70%: 70.1, 60%: 60.1, 50%: 50.8, 40%: 39.7, 30%: 30.6, 20%: 21.0, 15%: 15.6; the ramp 49.8 for 50.0; the noisy field 77.3 for 78.0. Within the noisy field's 10 point bins the coverage strays further, from 6.1 points over in the 40 to 50% bin (51.0 for 44.9) to 3.0 under in the 80 to 90% bin (81.7 for 84.7); the field as a whole holds, and the bins were not investigated further. Round 5's coast sample with r5-06: the numbers in section 3.9; the reference r5-01 covered the band 50.2%.

Not reviewed: land darker or lighter than green vegetation other than snow (bare rock, dark forest in winter, ice shelves), islands and lakes (round 5's coastline had neither, by construction), a coastline at the cap's real resolution and shape, the summer palette, the south winter color in the coast rows (round 5 drew the north's only), the fast ice band and the coast blend together with the new floes, and the record's own concentration near coasts (the coast samples were synthetic and read without the eddy shift).

## 4. Code

The blocks below, concatenated in order, form one module that reproduces the lab's r4-03 and r5-06 coverage from the lab's inputs (section 4.9 says how this was checked). They import these tracked helpers unchanged: from `texture_pipeline.sea_ice_look`, `band_noise`, `curl_shift`, `normalized`, `rows` and `smoothstep`; from `texture_pipeline.sea_ice`, `CAP_SIZE` and `texel_km`. The lab's generators are `default_rng([2004, family, stream])`; the pipeline's `_rng` adds the hemisphere, and the stream numbers below are only the lab's.

| Generator | Draws |
|---|---|
| `[2004, 7, 104]` | Regional target noise, 60 to 250 km |
| `[2004, 7, 142]`, `[2004, 7, 143]`, `[2004, 7, 144]` | Calm speckle octaves, 8 to 25, 25 to 60 and 60 to 120 km |
| `[2004, 7, 151]` | Calm zone handover regions, 30 to 120 km |
| `[2004, 31, k]` | Cascade level k: the curl shift, the jittered points, then `u` and `v` |
| `[2004, 401, 0]` | The size jitter of the patch order |

The functions work on any square; the lab's canvas was 2048. Three things in them exist for the lab's canvas only and are optional: `regions` in `lost_share` and `patch_dark` (the lab smoothed and colored per sample) and any tag beyond land and sea in `cascade_whole`.

### 4.1 Setup

```python
"""The settled floes look of the floes lab (2026-10-12): the coverage of sea
ice at partial concentration, its gap color and the coast rule."""

import numpy as np
from scipy import ndimage
from scipy.spatial import cKDTree
from scipy.special import ndtr, ndtri

from texture_pipeline import sea_ice_look as sil
from texture_pipeline.sea_ice import CAP_SIZE, texel_km

KM = texel_km(CAP_SIZE)
"""The polar cap's spacing, 2.873 km per texel."""

sil.texel_km = lambda size: KM
"""Lab only: the tracked helpers read the spacing from texel_km(size); this
points them at the cap's spacing on a smaller square. Not needed on the cap."""

SEED = 2004
FILL = np.array((10, 30, 60), dtype=np.float32)


def smoothstep(e0, e1, x):
    return sil.smoothstep(e0, e1, x)


def band(size: int, band_km: tuple[float, float], stream: int, seed: int = SEED) -> np.ndarray:
    """A band noise with standard deviation 1, from its own generator."""
    rng = np.random.default_rng([seed, 7, stream])
    return sil.band_noise(size, band_km, rng).astype(np.float32)


def mix(*parts: tuple[float, np.ndarray]) -> np.ndarray:
    out = sum(np.float32(w) * np.asarray(n, np.float32) for w, n in parts)
    return sil.normalized(out.astype(np.float32))


def zone(c: np.ndarray, lo: float, hi: float, ramp: float = 0.05) -> np.ndarray:
    """1 inside ``lo`` to ``hi``, 0 outside, with soft steps ``ramp`` either side."""
    return (smoothstep(lo - ramp, lo + ramp, c) * smoothstep(hi + ramp, hi - ramp, c)).astype(
        np.float32
    )


def gauss_target(mean: np.ndarray, noise: np.ndarray, sigma: float) -> np.ndarray:
    """Shares around a mean share: Phi(mu + sigma n) has mean ``mean`` for a
    standard normal n when mu = Phi^-1(mean) sqrt(1 + sigma^2)."""
    m = np.clip(mean, 1e-5, 1 - 1e-5)
    mu = ndtri(m) * np.float32(np.sqrt(1 + sigma * sigma))
    out = ndtr(mu + np.float32(sigma) * noise).astype(np.float32)
    out[mean <= 0] = 0
    out[mean >= 1] = 1
    return out
```

### 4.2 Settled values

```python
LOST_KM = 40.0
"""The leads' open water is smoothed over this before it counts against the floes."""

GRAY = 0.45
"""Gray water's darkness: the share of the fill it shows."""
DARK_BAND = (0.95, 0.45)
"""No water is dark above the first concentration, all of it below the second."""
DARK_BIN = 0.05
DARK_JITTER = 0.7

CLUSTER_KM = (60.0, 250.0)
CLUSTER_STREAM = 104
CLUSTER_SIGMA = 0.8

CASCADE_KM = (64.0, 32.0, 16.0, 8.0, 4.0)
CASCADE_WARP = 0.15
"""The cells' curl shift, as a share of the cell."""
DELTAS = (2.0, 1.2, 0.5, -0.2, -0.6)
LEVEL_SCALE = ((0.2, 0.6), (0.4, 0.867), (0.7, 1.667), (0.9, 0.6), (1.0, 0.0))
"""(concentration, level scale): r2-02's big floes up to 40%, r2-01's cascade
from 70% up, linear between."""
CALM = (0.4, 0.7)
CALM_LEVEL = 2
"""Inside the calm zone every cell still open at this level (16 km) resolves."""

SPECKLE = (((8.0, 25.0), 142, 0.15), ((25.0, 60.0), 143, 0.7), ((60.0, 120.0), 144, 0.5))
"""(band km, stream, weight) of the calm zone's speckle field."""
SPECKLE_SOFT = 0.12
PICK_KM = (30.0, 120.0)
PICK_STREAM = 151

COAST_BAND_KM = 10.0
COAST_RIM = 0.5
"""Within the band, ice wherever the concentration is at least this."""
```

### 4.3 The water share

```python
def dark_share(c: np.ndarray) -> np.ndarray:
    """The share of the water that is dark at a concentration."""
    return smoothstep(DARK_BAND[0], DARK_BAND[1], c).astype(np.float32)


def mean_dark(c: np.ndarray) -> np.ndarray:
    """The water's mean darkness at a concentration."""
    return (GRAY + (1 - GRAY) * dark_share(c)).astype(np.float32)


def lost_share(gaps: np.ndarray, regions=None) -> np.ndarray:
    """The leads' share of the fill smoothed over LOST_KM; per region in the lab."""
    sigma = LOST_KM / KM
    if regions is None:
        return ndimage.gaussian_filter(np.asarray(gaps, np.float32), sigma, mode="nearest")
    out = np.empty_like(gaps, dtype=np.float32)
    for region in regions:
        out[region] = ndimage.gaussian_filter(
            np.asarray(gaps[region], np.float32), sigma, mode="nearest"
        )
    return out


def water_share(c: np.ndarray, lost: np.ndarray) -> np.ndarray:
    """The share of each texel the floes' water takes, before its darkness, so
    that the leads and the floes together meet the concentration."""
    kept = np.clip(c / np.maximum(1 - lost, 1e-4), 0, 1)
    return np.clip((1 - kept) / mean_dark(c), 0, 1).astype(np.float32)
```

### 4.4 The cascade

```python
def cascade_cells(size: int, seed: int = SEED) -> list[dict[str, np.ndarray]]:
    """Each level's cells: a jittered grid, each texel's nearest point after a
    curl shift that scales with the cell, and two uniforms per cell."""
    levels = []
    for k, cell_km in enumerate(CASCADE_KM):
        rng = np.random.default_rng([seed, 31, k])
        cell = cell_km / KM
        shift = sil.curl_shift(size, (cell_km / 2, cell_km * 2), CASCADE_WARP * cell_km, rng)
        count = int(np.ceil(size / cell)) + 2
        grid = np.stack(np.mgrid[-1 : count - 1, -1 : count - 1], axis=-1).reshape(-1, 2)
        points = (grid + rng.random(grid.shape)) * cell
        tree = cKDTree(points)
        ids = np.empty((size, size), np.int32)
        for start, stop in sil.rows(size):
            r = np.arange(start, stop, dtype=np.float32)[:, None] + shift[0, start:stop]
            c = np.arange(size, dtype=np.float32)[None, :] + shift[1, start:stop]
            _, index = tree.query(np.stack([r.ravel(), c.ravel()], -1), k=1, workers=-1)
            ids[start:stop] = index.reshape(r.shape)
        u = rng.random(len(points)).astype(np.float32)
        v = rng.random(len(points)).astype(np.float32)
        levels.append({"id": ids, "u": u, "v": v})
    return levels


def resolving(c: np.ndarray) -> list[np.ndarray]:
    """The share of the still open cells that resolves at each level."""
    xs = [p[0] for p in LEVEL_SCALE]
    ys = [p[1] for p in LEVEL_SCALE]
    scale = np.interp(c, xs, ys).astype(np.float32)
    shares = [
        (1 / (1 + np.exp(-2.0 * (k + 0.5 - scale)))).astype(np.float32)
        for k in range(len(CASCADE_KM))
    ]
    w = zone(c, *CALM)
    shares[CALM_LEVEL] = shares[CALM_LEVEL] + (1 - shares[CALM_LEVEL]) * w
    return shares


def z_table() -> np.ndarray:
    """The probit offset z that makes the expected ice f at concentration c, on
    a grid of 51 concentrations by 201 ice shares."""
    levels = len(CASCADE_KM)
    cs = np.linspace(0, 1, 51, dtype=np.float32)
    zs = np.linspace(-7, 7, 561, dtype=np.float32)
    fs = np.linspace(0, 1, 201, dtype=np.float32)
    table = np.empty((len(cs), len(fs)), np.float32)
    for i, c in enumerate(cs):
        s = resolving(np.array([c], np.float32))
        weight, alive = [], 1.0
        for k in range(levels):
            share = 1.0 if k == levels - 1 else float(s[k][0])
            weight.append(alive * share)
            alive *= 1 - share
        expected = sum(w * ndtr(zs + d) for w, d in zip(weight, DELTAS, strict=True))
        table[i] = np.interp(fs, np.maximum.accumulate(expected), zs)
    return table


def z_lookup(table: np.ndarray, c: np.ndarray, f: np.ndarray) -> np.ndarray:
    ci = np.clip(c, 0, 1) * (table.shape[0] - 1)
    fi = np.clip(f, 0, 1) * (table.shape[1] - 1)
    c0 = np.minimum(ci.astype(np.int32), table.shape[0] - 2)
    f0 = np.minimum(fi.astype(np.int32), table.shape[1] - 2)
    tc, tf = ci - c0, fi - f0
    return (
        table[c0, f0] * (1 - tc) * (1 - tf)
        + table[c0 + 1, f0] * tc * (1 - tf)
        + table[c0, f0 + 1] * (1 - tc) * tf
        + table[c0 + 1, f0 + 1] * tc * tf
    )


def cascade_whole(
    cells: list[dict[str, np.ndarray]], ice: np.ndarray, c: np.ndarray, tag: np.ndarray
) -> np.ndarray:
    """Ice (1) or water (0) per texel. Coarsest first, a share of the cells
    still open resolves, each to ice with probability Phi(z + delta); the
    finest level resolves all. A cell is decided whole from its mean
    concentration and mean ice share, and cut where ``tag`` changes (land and
    sea on the cap)."""
    levels = len(CASCADE_KM)
    table = z_table()
    ntag = int(tag.max()) + 1
    out = np.zeros(c.shape, np.float32)
    open_ = np.ones(c.shape, bool)
    r = c.ravel().astype(np.float64)
    f = ice.ravel().astype(np.float64)
    for k, level in enumerate(cells):
        key = (level["id"].astype(np.int64) * ntag + tag).ravel()
        uniq, inverse = np.unique(key, return_inverse=True)
        count = np.bincount(inverse)
        rm = (np.bincount(inverse, r) / count).astype(np.float32)
        fm = (np.bincount(inverse, f) / count).astype(np.float32)
        cell = uniq // ntag
        share = resolving(rm)[k] if k < levels - 1 else np.ones_like(rm)
        resolved = level["u"][cell] < share
        solid = level["v"][cell] < ndtr(z_lookup(table, rm, fm) + DELTAS[k])
        here = open_ & resolved[inverse].reshape(c.shape)
        out[here] = solid[inverse].reshape(c.shape)[here]
        open_ &= ~here
    return out
```

### 4.5 The calm speckle

```python
def threshold_water(field: np.ndarray, share: np.ndarray, half: float) -> np.ndarray:
    """Water where a field falls below the level that leaves ``share`` of the
    area wet, with a soft step ``half`` either side of that level."""
    sample = np.asarray(field[::4, ::4], np.float32)
    thetas = np.linspace(-6, 6, 241, dtype=np.float32)
    means = np.array([float(smoothstep(t + half, t - half, sample).mean()) for t in thetas])
    means = np.maximum.accumulate(means)
    theta = np.interp(share, means, thetas).astype(np.float32)
    return smoothstep(theta + half, theta - half, field)


def floe_noises(size: int, seed: int = SEED) -> dict[str, np.ndarray]:
    """The noise fields the floes read, fixed on the plane."""
    return {
        "cluster": band(size, CLUSTER_KM, CLUSTER_STREAM, seed),
        "speckle": mix(*((w, band(size, km, stream, seed)) for km, stream, w in SPECKLE)),
        "pick": ndtr(band(size, PICK_KM, PICK_STREAM, seed)),
    }
```

### 4.6 The floes' water

```python
def floes_water(
    c: np.ndarray,
    lost: np.ndarray,
    tag: np.ndarray,
    cells: list[dict[str, np.ndarray]],
    noises: dict[str, np.ndarray],
) -> np.ndarray:
    """r3-03's water: the cascade, the faded speckle in the calm zone handed
    over by region, and open water where the concentration is 0."""
    share = water_share(c, lost)
    ice = gauss_target(1 - share, noises["cluster"], CLUSTER_SIGMA)
    water = 1 - cascade_whole(cells, ice, c, tag)
    wet = gauss_target(share, noises["cluster"], CLUSTER_SIGMA)
    specks = threshold_water(noises["speckle"], wet, SPECKLE_SOFT)
    water = np.where(noises["pick"] < zone(c, *CALM), specks, water).astype(np.float32)
    water[c <= 0] = 1
    return water
```

### 4.7 Gap color

```python
def patch_dark(
    water: np.ndarray,
    c: np.ndarray,
    regions=None,
    land: np.ndarray | None = None,
    seed: int = SEED,
) -> np.ndarray:
    """r4-03's darkness per texel: one per water patch, gray or dark. Patches
    are the 8-connected pieces of water (more than half water, land excluded);
    every texel takes the color of its nearest patch. Per 5 point bin of the
    patches' mean concentration, the largest patches turn dark first while that
    brings the dark water area closer to the share ``mean_dark`` assumes."""
    rng = np.random.default_rng([seed, 401, 0])
    out = mean_dark(c)
    if land is not None:
        water = np.where(land, 0, water).astype(np.float32)
    structure = np.ones((3, 3), bool)
    for region in regions or [np.s_[:, :]]:
        w = np.asarray(water[region], np.float32)
        cr = np.asarray(c[region], np.float32)
        labels, n = ndimage.label(w > 0.5, structure=structure)
        if n == 0:
            continue
        flat = labels.ravel()
        area = np.bincount(flat, minlength=n + 1)[1:].astype(np.float64)
        wet = np.bincount(flat, w.ravel(), n + 1)[1:]
        cm = np.bincount(flat, cr.ravel(), n + 1)[1:] / area
        key = -np.log(area) + DARK_JITTER * rng.standard_normal(n)
        want = wet * dark_share(cm.astype(np.float32))
        dark = np.zeros(n, bool)
        bins = np.floor(cm / DARK_BIN + 1e-6).astype(np.int64)
        for b in np.unique(bins):
            idx = np.nonzero(bins == b)[0]
            idx = idx[np.argsort(key[idx], kind="stable")]
            target, got = float(want[idx].sum()), 0.0
            for i in idx:
                if abs(got + wet[i] - target) < abs(got - target):
                    dark[i] = True
                    got += wet[i]
        shade = np.where(dark, 1.0, GRAY).astype(np.float32)
        _, (iy, ix) = ndimage.distance_transform_edt(labels == 0, return_indices=True)
        out[region] = shade[labels[iy, ix] - 1]
    return out
```

### 4.8 The coast rule

```python
def coast_band(land: np.ndarray, km: float = COAST_BAND_KM) -> np.ndarray:
    """The sea within ``km`` of the coastline: the distance to the nearest land
    texel less half a texel."""
    dist = ndimage.distance_transform_edt(~land).astype(np.float32) * KM
    dist = np.where(land, 0, dist - KM / 2).astype(np.float32)
    return (~land) & (dist <= km)


def coast_rule(coverage: np.ndarray, c: np.ndarray, band_: np.ndarray) -> np.ndarray:
    """r5-06's add-only rule: ice in the band wherever the concentration is at
    least COAST_RIM; nothing is opened."""
    out = coverage.copy()
    out[band_ & (c >= COAST_RIM)] = 1
    return out
```

### 4.9 Coverage, composition and the settled look

```python
def coverage(water: np.ndarray, dark: np.ndarray, gaps: np.ndarray) -> np.ndarray:
    """What the leads and the floes' water leave of the ice; gray water counts
    as part ice."""
    return (1 - gaps) * (1 - dark * water)


def compose(cov: np.ndarray, brightness: np.ndarray, color) -> np.ndarray:
    """sRGB as month_cap and composite_face make it, over the sea's fill."""
    rgb = np.clip(brightness[..., None] * np.asarray(color, np.float32), 0, 255)
    out = FILL + cov[..., None] * (rgb - FILL)
    return np.clip(out + 0.5, 0, 255).astype(np.uint8)


def settled_coverage(
    c: np.ndarray,
    gaps: np.ndarray,
    land: np.ndarray,
    cells: list[dict[str, np.ndarray]],
    noises: dict[str, np.ndarray],
) -> np.ndarray:
    """The settled look on a whole square: r3-03's floes, r4-03's gap colors
    and r5-06's coast rule. ``c`` is the concentration (0 on land), ``gaps`` the
    leads' share of the fill, ``land`` the land mask at the same spacing."""
    tag = land.astype(np.int32)
    water = floes_water(c, lost_share(gaps), tag, cells, noises)
    dark = patch_dark(water, c, land=land)
    return coast_rule(coverage(water, dark, gaps), c, coast_band(land))
```

**Verification.** The fenced blocks above were extracted from this file, in order, into one module and run against the lab on its 2048 canvas, with the lab's own inputs: the concentration read through the eddy shift, the fracture systems' open share and rank, the crumpled interior's brightness, and for r5-06 the coast sample's land and concentration. The module's cascade cells and noise fields, drawn fresh, were identical to the lab's cached ones. r4-03 was regenerated with the lab's per sample regions (`lost_share` and `patch_dark` per sample, the cascade tagged by sample) and r5-06 with the lab's coast tags (land, coast sample, the rest), the leads smoothed per sample, the patches over the coast sample and the band measured inside it. Both coverages came out identical to the lab's, maximum difference 0, and so did r5-06's band; the shipped pictures (the area average, the JPEG XL round trip and the crops) matched the lab's saved ones exactly, maximum difference 0 of 255, for all 32 of r4-03's samples and views and all 8 of r5-06's. The check is one script outside the tree (`tmp/sea-ice-lab/floes/work/verify_doc.py`, untracked).

## 5. Transfer notes for sea_ice_look.py

### 5.1 What replaces what

- The edge model goes: the `edge` part (`make_edge`: the 10 to 120 km threshold noise on stream 0, `edge_orientation` and `streak`), `Fields.threshold` (whose shape `Fields.size` reads today, so the size must come from another field), and in `month_cap` the line `covered = smoothstep(t - edge_softness, t + edge_softness, conc)` with `edge_pack`'s squeeze. The ice is no longer thinned by a threshold: `covered` is 1, and all of the concentration's open water is the floes'. `edge_center` and `edge_spread` are still read by the thin ice shading, `thin = thin_shade + (1 - thin_shade) * smoothstep(0.12, 0.88, conc)`, which every reviewed row had; keep that shading, with its range as its own tunable or with those two values.
- The eddy shift stays: the `eddies` part (`make_eddies`, stream 1) and `Fields.shift`, through which `month_cap` reads the concentration. Every reviewed open sea sample read the concentration through it (linearly inside each sample, where `month_cap` reads the record with a cubic B-spline). It is not round 1's rejected "eddies" row, which advected the edge model's threshold by a separate, stronger swirl. Its name says it swirls the edge; it now swirls the concentration the floes are drawn from.
- The `b3c-floes` floes go: the `floes` part (`make_floes`, `floe_cells` on stream 8), `Floes`, `floe_gap`, `floe_water`, `Fields.floes` and the `floe_*` tunables (`floe_cells_km`, `floe_keep`, `floe_ramp`, `floe_width`, `floe_cluster`, `floe_cluster_km`, `floe_open`, `floe_small`, `floe_small_share`, `floe_darkness`, `floe_dark_range`, `floe_gap`, `floe_fast`, `floe_warp_km`, `floe_warp_band_km`). In their place: the cascade cells and the floe noises as a fixed part (section 5.6), and per month the water share, `floes_water`, `patch_dark`, the coverage and the coast rule.
- The fractures, or the solid document's leads, stay as they are; their share of the fill is `gaps`. With `fracture_share` at (1.0, 1.0), every system is drawn at every concentration, also across the floes' water, where it changes nothing that is already the fill.
- `month_cap` works in bands of 128 rows, but three steps of the floes are not local: the cascade's per cell means, the patch labels and their nearest patch transform, and the distance to land. The banded pass computes the shifted concentration and the leads' share as now and keeps both for the whole cap (two float32 fields); the floes then run over the whole cap; the coverage and brightness follow. Everything per texel in the floes (the water share, the targets, the speckle threshold, the coverage) can stay banded.

### 5.2 `Look` tunables to add

With the settled values (section 3 for their meaning): the leads' smoothing, 40 km; gray 0.45 of the fill, the dark band (0.95, 0.45), the patch bin 0.05 and the size jitter 0.7; the regional target band (60, 250) km and its sigma 0.8; the cascade levels (64, 32, 16, 8, 4) km, the cell warp 0.15 of the cell, the deltas (2.0, 1.2, 0.5, -0.2, -0.6) and the level scale ((0.2, 0.6), (0.4, 0.867), (0.7, 1.667), (0.9, 0.6), (1.0, 0.0)); the calm zone (0.4, 0.7) with ramps 0.05 and its level, 2; the calm speckle's octaves ((8, 25) km at 0.15, (25, 60) at 0.7, (60, 120) at 0.5) and soft step 0.12; the handover band (30, 120) km; and the coast band, 10 km, and rim threshold, 0.5. A switch for the new floes replaces `floes`; `edge_pack` and the `floe_*` tunables go.

### 5.3 The land mask and where the coast rule applies

The settled look needs land at the cap's spacing in three places: the cascade's tag (cells cut at the coastline, section 3.5), the patches (land excluded, section 3.8) and the coast band (section 3.9). The cap has none today. `Fields.concentration` is the record's monthly mean on a coarser cap, the record's land is its 25 km cells, and `cap_concentration` extends the concentration into land before the B-spline, so land texels carry their neighbors' concentration rather than 0. The true coastline exists only in the faces' water masks, which `composite_face` and `blend_coast` read at the face's spacing.

The recommendation is one fixed land mask per hemisphere on the cap plane, made once from the faces' water masks (land where a mask is less than half water) resampled to the cap, and with it: set the concentration to 0 on land before the floes, as the lab's land was; tag the cascade with it (`land.astype(int32)`); pass it to `patch_dark`; and compute the band once with `coast_band`, the Euclidean distance transform of the cap's sea to the nearest land texel, times 2.873 km, less half a texel, at most 10 km. Applying the rule on the cap matches the lab, where the band was 3.5 cap texels and then area averaged with everything else; applying it in `composite_face` on the face's mask would make the band two face texels and needs the concentration on the face, which the face does not have. Coverage over land does not matter for the picture, since `composite_face` mixes the ice only into the mask's water share; it matters for the coast blend (section 5.4).

### 5.4 The fast ice band and the coast blend

- `fast_ice` raises the record within 25 km of land, tapering to 50 km, toward the mean 50 to 100 km out where that mean is above 80 to 90%. It acts on the record before the cap and can stay; the floes then draw that higher concentration, as pack with gray cracks. The old floes also stayed shut where the band raised the concentration by more than 0.03 (`floe_fast`); the new ones have no such switch. The lab had no fast ice band, so whether the raised coast needs one (water forced to 0 where `Fields.fast` is above 0.03) was not tested.
- `blend_coast` mixes snowy, flat land toward the ice by `coast_weight` times `Cap.land_coverage`, which `month_cap` fills with the edge model's `covered` at the land texels, before leads and floes, and with `land_brightness`, the texture. With the edge model gone `covered` is 1 everywhere, so every snowy coast would blend toward ice, also next to open water. It needs a replacement, and the lab did not test one (round 4's snowy land was painted flat, round 5's green land is outside the blend). A candidate that follows the settled coast: each land texel takes the final coverage, after the coast rule, of its nearest sea texel (the indices of the same distance transform), so land next to the rim blends to white and land next to open water stays land.

### 5.5 With the solid document's surface and leads

The two documents compose as `month_cap` composes now. The brightness is the thin shading times the solid document's surface (`1 + gain * activity * relief + gain * drift`), the same on floes and pack. The coverage is `coverage(water, dark, gaps)` with `gaps` the solid document's leads' share of the fill (its `jagged`), then the coast rule; the solid document's `compose` blends leads toward the fill by their share, which is the factor `1 - gaps` here. The floes' water is calibrated against those leads through `lost_share`, so their open water counts toward the concentration and the two together hold metric (c). The solid lab drew at concentration 1 only, and this lab drew `b3c-floes`' crumple and fractures, not the solid document's surface and leads, so the combination itself was not reviewed. The coast rim is solid ice with the surface's brightness and no leads.

### 5.6 Fixed on the cap plane, the same in every month

Fixed per hemisphere, made once like the crumple's fields: the cascade cells (five int32 id fields and their `u` and `v`), the three floe noises (the regional target, the calm speckle field, the handover pick), the land mask, its tag and the coast band. Also fixed, though the lab recomputed them per row: the z table (it depends on the schedule only), the speckle's threshold table (the `means` in `threshold_water` depend on the field only, not on the share), and in `cascade_whole` each level's `unique` keys, `inverse` and `count`, which depend on the cells and the tag only. Per month: the shifted concentration, the leads' share, the water share, the regional targets, the per cell means (two `bincount`s per level), the speckle threshold, the patch colors, the coverage and the rule. Because the cells and noises do not move, a floe stays where it is from month to month wherever the concentration changes little.

### 5.7 Seeds per hemisphere

The lab drew from `default_rng([2004, family, stream])` (section 4's table). On the cap each generator goes through `_rng(look, hemisphere, stream)`, so north and south differ. The tracked streams are 0 to 9, and the solid document's new ones are 11, 12, 15, 16, 21, 31, 32, 60 to 64, 70 and 71, so the floes need numbers clear of both, for example 80 for the regional target, 81 to 83 for the speckle octaves, 84 for the handover, 85 to 89 for the five cascade levels (each level its own generator, drawing the curl shift, the points, `u` and `v` in that order) and 90 for the patch order's jitter. The south was never drawn in its own seeds; its look at these values was reviewed only in the south winter color on the ramp and the noisy field.

### 5.8 Cost

Measured on this machine, one process, with the module of section 4; only `cKDTree.query` in `cascade_cells` uses more than one core:

| Step | 1024 | 2048 |
|---|---|---|
| Fixed fields: cascade cells and floe noises | 1.4 s | 5.5 s (cells 4.3, noises 1.2) |
| Per month: `settled_coverage` in all | 1.1 to 1.2 s | about 5 s, the sum of its steps |
| of which `lost_share` | | 0.4 to 0.5 s |
| of which `floes_water` (the cascade 2.1 to 2.4 s) | | 3.3 to 3.8 s |
| of which `patch_dark`, one region | | 0.6 s |
| of which `coast_band` | | 0.3 s |

The cap at 4096 has 16.8 million texels, four times the lab's canvas; at about four times the 2048 figures, the fixed fields would take about 22 s per hemisphere and each month about 20 s, four minutes for twelve months, if done as the lab does. These are extrapolations, not measurements. Hoisting the fixed parts of section 5.6 (the speckle table, the z table and the cascade's `unique` sorts, five sorts of 16.8 million keys) out of the month should take a large part of the per month cost away. Memory at 4096: the cells' ids are 5 x 64 MiB as int32, the noises 3 x 64 MiB; `cascade_whole` holds two float64 copies of the concentration and the target (128 MiB each) and, per level, an int64 key and its inverse (128 MiB each); `patch_dark` over one region holds the int32 labels and the nearest patch indices of the distance transform (2 x 64 MiB). The peak is about 1.5 GiB if done the same way; keeping the inverses as int32 and accumulating the `bincount`s over bands of rows would lower it. `patch_dark`'s Python loop runs once per patch; on the lab's canvas that was well under a second, and the cap's count of patches scales with its area.

### 5.9 Open

Darker land other than green vegetation, islands and lakes, the cap's real coastline, the summer palette, the south winter color at the coast, the fast ice band's switch, the coast blend's coverage (section 5.4), and the combination with the solid document's surface and leads (section 5.5). The preview harness composites at the 5400 working size and area averages to 2048 as `main.py` does, so its faces show the speckle as the shipped faces will.
