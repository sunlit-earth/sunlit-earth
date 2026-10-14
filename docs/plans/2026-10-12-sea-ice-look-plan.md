# Sea ice look, second iteration: plan

Follows `docs/plans/2026-10-11-baked-sea-ice-plan.md` (PR #76), whose bake the user accepted as a first iteration. This plan improves the look of the baked sea ice in three ways the user found by comparing the app with MODIS true color imagery. The experiments came first: through a preview harness of the real bake (phases A and B), and then, because each harness run took minutes, in two standalone labs on synthetic data. The labs' settled results are recorded, with code that reproduces them, in `docs/plans/2026-10-12-sea-ice-solid-research.md` (closed pack and leads) and `docs/plans/2026-10-12-sea-ice-floes-research.md` (partial coverage and the coast). Phase C moves that settled look into the pipeline and ships it, with tests, goldens, docs and validation.

Worktree `.worktrees/sea-ice-look`, branch `feature/sea-ice-look`, from `feature/baked-sea-ice` at 75055b3. It is rebased onto `main` once #76 is merged. Commits so far: 5c368ed (the harness and the variants B1 to B3d), 843b035 (the two research documents), 2260387 (the harness and generator speedups), 670faad (this plan for Phase C), 4f399db and ab83636 (C1, the `settled` variant), 17e9eb4 and c83e683 (C1b, the variants `tone-80-90` and `tone-70-85`).

## The three imperfections

1. **The interior is too uniform, and its texture reads as dots and scratches.** The pack is a white base with soft gray blobs, light gray dots and short straight dark lines. Real sea ice, the Antarctic's above all, is covered in snow and looks much like the snow covered land: bright, with very fine detail, like a crumpled sheet of paper flattened again. The detail is not uniform: some areas show much more structure, others look almost flat.
2. **The coastline stays visible under the ice.** From high above the boundary between snow covered land and snow covered pack disappears; only open water between floes gives the ice away. In the app the edge between land and ice is always visible.
3. **The mask does not fit the Blue Marble's coastline.** Along North Greenland's coast, where satellite imagery shows no boundary in winter, the app shows strokes of dark blue water all year.

References the user chose, from Wikimedia Commons' categories of satellite pictures of the Arctic and of Antarctica:

- `File:Top_of_the_World_(MODIS_2019-08-05).jpg`, the Arctic in summer
- `File:Three_views_of_The_Top_of_the_World_(MODIS_2020-05-06).jpg`, the Arctic in late spring
- `File:Antarctic_(MODIS_2018-01-05).jpg`, Antarctica in summer

## Evidence

Measured on 2026-10-12 on the shipped faces at 75055b3.

**The mask.** The mask is Natural Earth's 1:10m ocean, rasterized at the source's 21600 x 10800; the Blue Marble draws its own coastline. Where the Blue Marble draws water on the land side of Natural Earth's line, the bake treats it as land: it is neither flattened nor given ice, so it stays the Blue Marble's dark water in every month. On the mask's land side, texels dark (luminance below 60) and bluish (blue above red by more than 8) in all twelve months number 1,202 on `py` poleward of 60 degrees and 668 on `ny`. They cluster where Natural Earth is weakest: North Greenland (Peary Land), Ellesmere's north coast, Svalbard and Franz Josef Land. The same mismatch runs along coasts elsewhere, Norway, the British Isles and the Gulf of Alaska, where there is no ice and it shows only as the Blue Marble's water beside the fill. In a coast texel that is partly water, the same dark water sits in the land share, which draws the dark outline along coasts.

**The color.** The ice is darker than the snow beside it. Medians of land brighter than 180 within 8 texels of the coast against ice brighter than 180 at least 8 texels out:

| Faces | Coastal snow | Ice |
|---|---|---|
| `py` (poleward of 70), January and April | (247, 250, 252), (246, 248, 249) | (240, 243, 247) |
| `py`, July | (224, 227, 227) | (206, 212, 217) |
| `ny` (poleward of 60), April and September | (234, 239, 243), (235, 239, 243) | (224, 229, 233), (225, 230, 234) |
| `ny`, January | (235, 240, 243) | (209, 213, 217) |

**The generator.** `sea_ice_look.py` draws one flat color per hemisphere and month, varied by isotropic noise alone: a 60 to 300 km brightness noise at 2% everywhere, and a 5 to 20 km noise at up to 8% where the concentration is below 95% and in the Arctic summer. Those are the blobs and the dots. The leads are borders of an elongated cell pattern, 80 by 240 km cells, of which 8% to 40% are drawn, which leaves short isolated segments: the scratches. Apart from the concentration, nothing varies how much structure there is.

## Settled with the user

- Experiments first. The user inspects every variant before anything is committed to.
- The experiments run through a preview harness: four renders, the north and the south pole in winter and in summer, and an HTML page that shows them.
- An implementer subagent writes the code. The orchestrator guides it and checks each variant's results for plausibility before handing them to the user.
- No validation rounds until the user has committed to a solution.
- No git commits or pushes without the user's approval.
- After inspecting A, B1 and B2 (2026-10-12): B1 and B2 are kept as the base of what follows; North Greenland's coast looks a lot better. A clear edge between land and sea ice remains, and the user wants the ice's texture to reach a little inland so that the edge disappears where the terrain is flat, as along much of Antarctica's coast, while mountain ranges may keep the coastline visible (B2b). B3 follows in full, and the user inspects the renders when both are done rather than after each sub-step.
- B1 stays poleward of 50 degrees, and Franz Josef Land's islands shrink to where the Blue Marble and Natural Earth agree. These were the orchestrator's recommendations; the user did not take them up, so they carry into production as defaults.
- After inspecting B2b and B3 (2026-10-12): the edge between land and sea ice looks good; the ice surface still has room for improvement; and waiting minutes for each harness run is too slow. The look work moved to standalone labs on synthetic data, one for closed ice and one for partial coverage, which the user reviewed round by round (departure 8), and the harness was sped up (departure 9).
- The closed pack is settled (`2026-10-12-sea-ice-solid-research.md`, section 3): ridged relief at three detail levels, varied by large region fields in detail, strength and softness, with drift streaks in some regions only; leads thinner and fainter than B3b's, "a bit more jagged, since the ice is shifting", with en echelon and pinch preferred and zigzag and stretches least. The default is that document's r3-11, r3-08's surface with en echelon and pinch together, a combination the user has not yet seen; r3-08 with r3-02's en echelon leads alone is the reviewed fallback.
- Partial coverage is settled (`2026-10-12-sea-ice-floes-research.md`, section 3): r3-03's cascade of whole cells with a calm zone from 40 to 70% and big floes on dark water in loose ice, r4-03's gap color (one color per water patch, the largest patches dark first, gray to dark from 95% to 45%), no edge model (departure 10), and r5-06's add-only coast rule in a 10 km band (departure 11).
- Kept from Phase B: B1, B2, B2b and B4. B3's sub-steps are replaced by the research documents' look, and B3's fallback, a detail layer cut from MODIS scenes, is not needed.
- Performance is not paramount, but it must not fall back to where it was before 2260387 (the performance budget below).
- After inspecting `settled` (2026-10-14): "edge between land and sea ice now looks good. sea ice itself also looks good." Five changes follow, made in C1b:
  - The south's September color is "a bit off". The south's winter color goes halfway toward the north's first, the midpoint of the two.
  - The north's summer gray is fine but "could be a little brighter": the summer darkening is cut by a third.
  - "the random blue-ish brush strokes mixed into the ice sheet look bad; they also have no reference in satellite imagery, let's just remove those": the melt tone goes.
  - "the grey pools of water on ice sheets with low water percentage don't really work, I think we should remove them completely for now": the gap color goes, and every gap is the dark fill.
  - Most of the pack showed gaps, while "it's rare to see gaps far away from the edge of the ice sheet". Measured on the record: beyond 400 km from open water the median concentration is 98% in the north's March and 95% in the south's September, and the cascade drew every missing percent as cells of water 16 to 64 km across. The shortfall is mostly what passive microwave reads as part water (thin new ice in refrozen leads, melt ponds, the leads themselves), with a percent or two of retrieval noise: above 90%, a quarter of the north's March cells read over 100% before the product clipped them, and more than twice as many read 95% to 99%. The user agreed to draw no gaps above 90% and chose a tone curve on the concentration the floes read, in two variants to compare: a fade from 80% to 90% and one from 70% to 85%.
- After inspecting `tone-80-90` and `tone-70-85` (2026-10-14): "amazing. the 70-85 variant is it." The settled look is `tone-70-85`, its +10.3% of ice area in the north's August included, which the orchestrator had flagged as 0.3 points over metric (c)'s 10%. Phase C then runs in full with validation rounds.

## Phase A: the preview harness

Built in 5c368ed, sped up in 2260387 (departure 9). What it does:

- **Inputs.** The twelve Blue Marble JPEGs of 2004 at 21600 x 10800, `ne_10m_ocean.shp`, the 24 OSI-450-a1 files, and the shipped textures as the base every variant starts from.
- **Months.** North: March 2004 for winter, at the maximum extent, and August 2004 for summer, with the melt. South: September 2004 for winter and February 2004 for summer. The four are arguments with these defaults.
- **The bake.** Only what the renders read: `py` for the north's two months and `ny` for the south's, with the production settings (faces of 2048 from 5400, caps of 4096, JPEG XL quality 85 and effort 7), so what the codec does to fine detail is part of what the user sees. Anything that reads all twelve months, such as the mask, is computed from all twelve.
- **Caching.** Whatever a look change does not touch (the shapefile raster, the mask, the concentration caps, noise fields keyed by their tunables) is cached, so a look change turns around in a few minutes. The harness reports each variant's time.
- **The renders.** The app's headless `render` at 2048 x 2048, centered on each pole and framed to about 55 degrees of latitude, as the references are. It draws the day surface without lighting, because at the winter dates the poles are in polar night, with the atmosphere, the stars and the clouds off, on a date at which the month's faces are drawn alone (the polar goldens use the first of the month). It reads a variant's texture directory, the shipped set with the rebaked faces and mask in place, through a cache directory of its own, so the tile packs come from the new faces. Following `tmp/sea-ice/spike/render.sh` of the first iteration, every `SUNLIT_EARTH_*` directory points into the variant's output, and the app runs in a console of its own so it writes nothing into the user's app data.
- **The baseline.** The shipped textures through the same harness, as variant `baseline`.
- **Outputs.** `tmp/sea-ice-look/<variant>/`: the four renders, the faces and mask it baked, a short `notes.md` saying what the variant changes, and `metrics.json`. `tmp/sea-ice-look/index.html` shows them.
- **The page.** One self-contained HTML file, no network: a 2 x 2 grid of the renders labeled with pole, season and month; a variant selector; a comparison of the selected variant against the baseline (a toggle or a swipe); a click to view a render at full size and pan it; each variant's notes and metrics; and the three reference images, kept under `tmp/sea-ice-look/refs/`.
- **Metrics,** per variant: (a) the dark texels of the mask's land, as in the evidence, on `py` and `ny` poleward of 60 degrees (baseline 1,202 and 668); (b) the coastal snow against the ice, as in the evidence, for the rendered months; (c) each rendered month's ice area on the faces against the source's, which the first plan holds within 10%; (d) a measure of the ice interior's fine detail after the encode, for the texture work.
- **The draft.** `preview draft <variant> --place <name>` composites one region of a view's face around a named place at the working size in seconds, without the encode, the renders or the metrics (2260387).

## Phase B: the experiments

Done. B1, B2, B2b and B4 are kept; B3's sub-steps were superseded by the labs (departure 8). Each step is one or more variants. For each, the implementer writes `notes.md` and the metrics; the orchestrator checks the result (the metrics, the renders against the baseline, nothing changed outside the ice and the coasts it means to change, no seams at the caps' or faces' edges); then the user inspects it and decides what follows.

**B1. The water from the Blue Marble itself.** A source pixel on the shapefile's land side becomes water when it is the Blue Marble's water in all twelve months and is connected to the shapefile's ocean. Polar land is bright with snow in some month, so the twelve month test separates the two; connection to the ocean leaves lakes alone. The thresholds come from measuring the Blue Marble's water over the shapefile's open ocean, not from guesses. It runs poleward of 50 degrees in both hemispheres first; the counts it would change elsewhere are reported, so the user can decide whether it runs everywhere. The Antarctic ice pass still keeps the ice shelves as land afterwards. The variant shows the mask's change as an image. Expected: metric (a) near zero, and North Greenland's strokes gone and under ice in winter.

**B2. The ice's color from the coastal snow.** Each hemisphere's winter color is taken from the median of its coastal snow in the Blue Marble, and the summer months keep the darkening the current table gives them relative to winter. Expected: metric (b) within a few levels in winter.

**B2b. The coast's land side.** After B2 the coast stays visible as a change of texture and a thin edge. The ice's texture stops at the mask's line, and the edge is probably the Blue Marble's own coastline pixels, part snow and part its dark water, in the land share of the coast texels; the implementer measures what it is before changing anything. The composite then blends a band of land along the coast toward the ice, with a weight that is:

- the ice's coverage there (the caps extend the concentration a few source cells into land already);
- times how much the land is snow, bright and unsaturated;
- times how flat it is, low local relief in the Blue Marble;
- falling off with the distance inland over about 10 to 20 km.

Snow and flatness are read from the land's surroundings rather than from the pixel alone, so the coastline's mixed pixels are blended where the land behind them is flat snow. The land takes the ice's color and fine texture but not its leads, floes or open water, which belong to the sea. Mountain coasts keep their relief, and land without snow keeps its color. This changes land texels, which the first plan's composite left untouched before the encode; the change stays within the band, and becomes a departure if kept. Metric (e): at flat snow coasts in the winter months, the luminance step between land and ice a few kilometers either side of the coast and the depth of the darkest pixel across it, reported against the same at mountain coasts, which keep theirs.

**B3. The interior.** In sub-steps, each a variant chained on the one before, all presented to the user together:

- an activity field of noise at hundreds to a thousand kilometers, deciding where the pack is almost flat and where it is structured, with more structure where the concentration is below 1 and near the edge;
- a crumple texture in place of the blobs and dots: ridged noise from a few texels up to about 30 km, drawing thin creases rather than round spots, low in contrast, slightly stretched along a slowly turning direction, scaled by the activity;
- leads as a few long, branching, curving fracture systems hundreds of kilometers long, of varied width and darkness, fresh leads dark and refrozen ones gray, more in the Arctic than in the Antarctic;
- the marginal ice below about 85% concentration as floes, a cell pattern with dark water between bright floes, in place of the broken ice noise;
- the summer melt as a smooth regional tone instead of dots.

The crumple has to survive the encode at quality 85, which removes faint fine detail first; metric (d) shows whether it does.

**B4. A fast ice band, only if needed.** After B1, measure whether gaps of open water remain along coasts where the ice offshore is closed. Only if they do: a band of 10 to 20 km along the coast is filled where the offshore concentration is high. Some flaw polynyas along coasts are real, so the band follows the data.

**Fallback for B3.** If the procedural texture does not convince, the fine detail can be taken from cloud free MODIS scenes of pack ice: a detail layer cut from real imagery and placed on the caps. That brings clouds and shadows in the scenes, visible repetition and a new source to document, so it is a fallback only.

## The settled look

What Phase C ships, part by part, with where each is specified:

| Part | What | Specified in |
|---|---|---|
| Mask | The Blue Marble's own water joins the mask poleward of 50 degrees | B1, `b1-water` |
| Color | Each hemisphere's winter color from its coastal snow, the south's then moved halfway toward the north's; the north's summer darkening cut by a third | B2, `b2-color`; C1b |
| Coast's land side | Flat snowy land within 6 to 18 km of the coast blends toward the ice; the Blue Marble's sea on the mask's land takes the sea's look | B2b and departure 4, `b2b-coast`; its coverage input changes (C1) |
| Fast ice band | The record's concentration raised along coasts where the ice 50 to 100 km out is closed | B4 and departure 3 |
| Closed pack surface | Ridged relief at three detail levels, region fields for detail, strength (35% to 85%) and softness, Voronoi creases in soft areas, drift in some regions; no underlayer | Solid document, sections 3.1 to 3.5 and 3.7 |
| Leads | B3b's fracture systems, drawn thin and faint, en echelon and pinched | Solid document, section 3.6 |
| Partial coverage | Water share calibrated against the leads, a regional target, the cascade of whole cells cut at the coastline, the calm speckle from 40 to 70%, open water at 0% | Floes document, sections 3.1 to 3.7 |
| Tone curve | The floes read the concentration through a curve that closes the pack, a fade from 70% to 85%: no floes' water at 85% and above | C1b, `tone-70-85` |
| Gap color | None: every gap is the dark fill. The floes document's per patch gray (section 3.8) was removed at the review of `settled` | C1b |
| Coast rule | Add-only: in the sea within 10 km of land, ice wherever the concentration is 50% or more | Floes document, section 3.9 |
| Unchanged from the first iteration | The eddy shift the concentration is read through, the thin ice shading, the per month colors, every field fixed on the cap plane, per hemisphere seeds | Floes document, section 5.1 |
| Season | The per month colors alone; B3d's melt tone was removed at the review of `settled` | C1b |

## Phase C: production

Starts when the user approves this plan. In order:

**C1. The settled look in the generator, as a harness variant.** In `sea_ice_look.py`, behind `Look` switches that are off by default, so the production bake and the `baseline` variant stay byte-identical until C2:

- the closed pack's surface and leads, following the solid document's section 5: the ridged relief, region fields, soft activity and Voronoi creases, and drift replace the crumple's folds, plates and stretched ridges; `draw_fractures` gives way to the en echelon and pinch drawing; the octaves are drawn once, not once per detail level; the region fields are drawn on a coarser cap and zoomed; and the new streams take the numbers that document lists;
- the floes, following the floes document's section 5: the edge model goes (its threshold and `streak`, with the eddy shift and the thin ice shading kept), the B3c floes give way to the cascade, the calm speckle, the regional target and the per patch gap color, with their fixed tables (the z table, the speckle threshold table, the cascade's per level sorts) computed once per hemisphere rather than once per month, and streams 80 to 90;
- a land mask on the cap plane, made once per hemisphere from the faces' water masks, with the concentration set to 0 on land, the cascade tagged by it, the patches excluding it, and the coast band computed from it;
- the coast rule applied on the cap after the leads and the gap color;
- the coast blend's coverage rewired: each land texel takes the final coverage, after the coast rule, of its nearest sea texel, so snowy land next to the rim blends toward ice and land next to open water stays land;
- the fast ice band kept as it is; whether the new floes need to stay shut where it raised the concentration (as `floe_fast` did) is decided by measurement, B4's metric of open water along closed coasts, before and after;
- each new part in the per part field cache of 2260387, so a change to one part recomputes that part alone.

The variant `settled` chains on `b2b-coast` (B1, B2, B2b and B4) and adds the settled look. The orchestrator checks it: metrics (a) to (e), the ice area within 10% in all four renders, the coast metrics against `b2b-coast`, the share of pack streaks (near 0 with no edge model), no seams, and the performance budget. Then the user inspects the four renders and `preview draft` crops of the places the experiments watched (North Greenland, Ellesmere, Svalbard, Franz Josef Land, the Beaufort Sea, East Antarctica, the Ronne and Ross shelf fronts, the Antarctic Peninsula). This is the first time the surface, the leads and the floes are seen together, on the real coastline, in summer and in the south; the open questions below are answered here. Changes the user asks for are made in the same variant, with the labs available for anything that needs fast iteration.

**C1b. The first review's changes, as two variants.** C1 is done (4f399db, ab83636), and the user's review of `settled` asked for five changes (see "Settled with the user"). They go behind `Look` switches or variant options that are off by default, as in C1, so `baseline` stays byte-identical. `tone-80-90` chains on `settled`; `tone-70-85` chains on `tone-80-90` and differs in the curve alone, so its comparison against its parent shows the curve and nothing else.

- **The south's colors.** The south's winter color is the midpoint of the two hemispheres' winter colors as B2 measures them, today (247, 250, 251) and (236, 240, 244), so about (242, 245, 248). Every southern month keeps its ratio to winter per channel, as in B2, so February becomes about (229, 231, 233).
- **The north's summer.** Each northern month's ratio to winter, per channel, becomes `1 - (1 - r) * 2 / 3`, so the winter months are unchanged and August goes from (211, 217, 219) to about (223, 228, 230).
- **No melt tone.**
- **No gap color.** Every texel of the floes' water is the fill. The water share no longer divides by the gray's mean darkness, so it is `1 - kept`, and `patch_dark` is not run.
- **The tone curve.** The floes read `c' = c + (1 - c) * smoothstep(lo, hi, c)` in place of the concentration wherever they read it: the water share, the regional target, the cascade's schedule and its cells' means, and the calm zone. Open water at 0% stays, since `c'` is 0 where `c` is. Everything else keeps reading the record: the leads, the coast rule's 50%, the fast ice band, the thin ice shading and the activity. At and above `hi` the floes draw no water, guaranteed rather than likely: the cascade's cells are cut at the `hi` line the way they are cut at the coastline, and the closed side is ice. `tone-80-90` has (0.8, 0.9) and `tone-70-85` has (0.7, 0.85).

  | Concentration | 50% | 70% | 75% | 80% | 85% | 88% | 90% |
  |---|---|---|---|---|---|---|---|
  | Floes' water before the leads, `settled` | 50% | 30% | 25% | 20% | 15% | 12% | 10% |
  | `tone-80-90` | 50% | 30% | 25% | 20% | 7.5% | 1.2% | 0 |
  | `tone-70-85` | 50% | 30% | 18.5% | 5.2% | 0 | 0 | 0 |

  `settled` divided these by the gray's darkness as well, so it drew more water than this row at high concentration. Estimated on the record's own grid, the curves add 2.5% to 5.0% (`tone-80-90`) and 3.7% to 7.8% (`tone-70-85`) to the ice area of the four rendered months, the summer months most. Added to `settled`'s +0.5%, -0.2%, -2.9% and -7.2%, both stay within metric (c)'s 10%; the harness reports what the faces actually draw. The pack streak measure (departure 15) then counts the floes' dark water at 85% and more, expected small for `tone-80-90` and 0 for `tone-70-85`.
- **The cache.** Every new tunable in its part's key, as in C1. Removing the gap color drops its labeling and distance transform from every month.

The orchestrator checks both variants as it did `settled`: metrics (a) to (e), no seams, the performance budget, and `baseline` still byte-identical. Then the user inspects both on the page, with the drafts, and chooses a curve. No validation round.

Done in 17e9eb4 and c83e683 (departure 16). The user chose `tone-70-85`. Its metric (c): +4.6% in the north's March, +10.3% in its August, +5.9% in the south's September and -1.4% in its February; the dark gaps alone add 1.0% to 2.6%, which the estimate above left out.

**C2. The settled look becomes the default.** `LOOK`'s defaults, and the color options the harness's variants carry, switch to `tone-70-85`'s: the settled look with C1b's changes and the curve (0.7, 0.85). `main.py`'s `cube` command gains what the harness does and production does not yet: B1's water, B2's colors with C1b's moves (measured from the coastal snow as the harness does, or the measured tables made `LOOK`'s default, whichever keeps the two identical), the fast ice band and the coast blend, which `main.py` calls none of today. The harness and production then make the same faces from the same code, so `baseline` and a production bake agree. The code only the superseded experiments used goes: the edge model's threshold, the folds and plates, the B3c floes, the old cell leads, the per patch gap color, the melt tone and mottling, and the switches that only selected between the old and the new. The experiment variants go with them, `b1-water` to `tone-70-85`; the variant machinery and `baseline` stay for future experiments. The research documents and the history keep what was removed. `baseline` then means the new look, and its byte identity against `textures/` returns after C3.

Done in 46a5fbe (departure 17). The new `baseline` made every face and mask face byte-identical to `tone-70-85`'s; `uv run ty check` is back to the 6 diagnostics in `tests/test_milky_way.py`.

**C3. The production bake.** All twelve months and all faces with `cube --sea-ice`, the mask faces B1 changes included. `textures/PROVENANCE.md` gets the new command line, the measurements and the sizes. `TestRealIce` holds the drawn ice area within 10% in every month and hemisphere of the area the floes are asked to draw: the source's concentration read through the tone curve, on the source's grid. Against the record itself the tone curve adds ice by design (C1b, up to about 10% in the summer months, which the user accepted), so that comparison is reported, per month and hemisphere, in `PROVENANCE.md` and the PR, but not held to 10%. The coast rule adds ice in its band by design too (floes document, section 3.9) and is part of the total either way. The harness's metric (c) reports both.

Done with the bake of 2026-10-14 (departure 18). Two runs wrote all 84 files byte for byte alike; against the first iteration's 29 changed: the 27 day faces with sea ice and the mask's `py` and `ny`. `baseline` through the harness afterwards is byte-identical to `textures/` in both mask faces and all four day faces. `TestRealIce` passes: against the toned source every month and hemisphere is within -5.7% (the south's February) and +2.9% (the north's July); against the record within -1.4% (the south's February) and +10.3% (the north's August). `PROVENANCE.md` has the table per month and hemisphere.

| Measure | Budget | Measured |
|---|---|---|
| Preview run, fields cached | 60 s | 55.8 s, peak working set 6.46 GiB, commit 7.60 GiB (69.6 s with other builds on the machine) |
| Preview run, fields cold | 180 s | 112.8 s, fields 52.6 s of it, peak working set 6.52 GiB, commit 7.69 GiB |
| `preview draft` | 10 s | 6.2 to 7.8 s with Python's start, 4.7 to 6.0 s of work |
| Production bake, all twelve months | 900 s | 804.8 s; a second run, identical, 1,100.2 s with other builds on the machine and every stage slower |
| Peak memory of the production bake | 8 GiB | working set 5.99 GiB, commit 6.94 GiB at the default worker count |

**C4. The app's side.** The real face checks in the Rust tests (the tile packs store every tile with ice and flag open water), the polar goldens regenerated on WARP here and on lavapipe and Metal from CI's artifacts, as #76 did; the Earth fixture under `crates/sunlit-core/tests/fixtures/earth/` regenerated from the new faces if #76's procedure calls for it. The branch is not on GitHub yet: when CI is needed, the implementer stops at that point and the orchestrator has the branch pushed and dispatches the workflow. The shader's side, checked in lit renders: the brighter ice raises the night side's ice, which scales with the day texel's luminance; the ice gate's luminance of 52 to 76 is not affected; new water at the coasts takes the glint in summer.

Done locally on 2026-10-14; the lavapipe and Metal goldens and `ci.yml` wait for the branch on GitHub. The Earth fixture's March and July `py` and `ny` and its mask's `py` and `ny` were regenerated under `TEXTURE_PIPELINE_UPDATE_FIXTURES=1`, as #76 did; the pipeline's cube fixture changed its four polar day faces in C2, which moved the cutter digest, re-pinned there with `FORMAT_VERSION` at 2 as in 0e16148, since the cutter is unchanged. The WARP set regenerated over the new fixture moves `arctic_in_march`, `arctic_in_september` and `antarctic_in_september` in 48,729, 48,100 and 33,522 pixels by up to 150 levels, `africa_on_the_z_face` in 482 pixels by at most 7 at the faces' polar rims, and `clouds_across_the_terminator` in 68 pixels by 1 at the frame's top edge; each was checked against its old reference side by side, and the changes are the pack's closing, brightening and finer texture, nothing outside the ice and its rims. `the_real_day_packs_store_the_sea_ice_and_flag_the_open_water` passes on the new packs: the ocean color is (10, 30, 60) in every month, 187 to 223 tiles a month hold ice and all are stored, 423 to 471 are open water and all are flagged. The lit renders, the textures before C3 against after, in blend mode with the atmosphere, the stars and the clouds off:

- The night side's ice. Over the mask's open water poleward of 70 S on 15 July, in polar night, the ice's mean color went from (40.9, 47.4, 73.0) to (42.6, 49.6, 76.3), 4.5% brighter, as its day luminance went from 229 to 243, past the 240 where `NIGHT_ICE` stops scaling. Poleward of 72 N on 15 January the day luminance was above 240 before and after (242 and 247), and the night ice stayed within 0.3 of (41.7, 48.7, 75.4).
- The ice gate. Of the mask's open water poleward of 35 degrees, the share that decodes between 52 and 76, the gate's ramp, fell on all four views' faces: `py` in March 0.23% to 0.17% and in August 0.29% to 0.19%, `ny` in February 0.13% to 0.10% and in September 0.25% to 0.18%. Every gap is the fill now and the pack brighter, so fewer texels sit between the two.
- The glint at the coast's new water. In August renders of Svalbard and North Greenland at 2.2 radii, at the pixels where the mask gained water: over new open water, 652 of 678 and 625 of 654 pixels change when `spec_intensity` and `fresnel_mix` go to 0, by 2.2 levels on average, against 565 and 546 by 1.1 on the old faces, where only the texture filtering gave them a share of water; over new water under ice, 0 of 2,138 and 1 of 2,443 change, against 824 and 941. The new water takes the glint where it is open and the gate takes it off where it is ice.

**C5. Docs.** `docs/rendering.md`, `docs/assets.md`, `docs/testing.md` for the new tests, `docs/roadmap.md` (the first iteration's look items resolved, the open questions that remain), and the pipeline's README for the look, the harness and `preview draft`.

Done on 2026-10-14: `docs/rendering.md` (the look in a paragraph, the gate's ramp, the night ice's change, the day tile counts, the mask's joined water and its glint), `docs/assets.md`, `docs/testing.md` (the pipeline's tests, `TestRealIce`'s rule, the regenerated fixture and WARP set), `docs/roadmap.md` (the second look done; the first look's refinements resolved; what remains, departures 13 and 18 among it) and the pipeline's README (`--imagery-water`, the command's steps, the harness, its caches, the draft and the memory). The lavapipe and Metal golden runs, once CI has made them, go into `docs/testing.md` with their run numbers.

**C6. Validation.** Rounds of adversarial review until one reports no major finding, then `ci.yml` dispatched for all three operating systems. Whether this lands in #76 or in a PR of its own after #76 merges is the user's decision.

## Performance budget

Performance is not paramount, but the iteration loop and the bake must not fall back to where they were before 2260387. Measured on this machine:

| Measure | Before 2260387 | At 2260387 | Budget for Phase C |
|---|---|---|---|
| Preview run, fields cached | 150 to 160 s (`b3c-floes`) | 45 s (`b3c-floes`), 33 s (`baseline`) | 60 s for `settled` |
| Preview run, fields cold | about 420 s (`b3c-floes`) | 169 s (`b3c-floes`), 62 s (`baseline`) | 180 s for `settled` |
| A change to one part of the fields | every field rebuilt | that part and what reads it | the same; at most 30 s of field work for a change to the floes or the leads |
| `preview draft` | none | about 4 s with the start | 10 s |
| Production bake, all twelve months | 712 s (#76) | 589 s | 900 s |
| Peak memory | 4.32 GiB commit in the production bake (#76); about 7 GiB in the harness with everything concurrent (measured by the investigation, not since) | not measured again | 8 GiB at the default worker count, with `--workers 1` kept for a serial run |

The floes document extrapolates its per month cost at the cap's size to about 20 s per hemisphere if done as the lab does, which would put the production bake over budget; hoisting its fixed tables out of the month (C1) is what keeps it within. The folds, which made most of the cold field time in 2260387, go with C1. Every budget is measured, not estimated, and recorded with the variant's notes and in the PR.

## Gates

During the experiments:

- `uv run pytest -q` in `tools/texture-pipeline`, and `uv run ruff check` and `uv run ruff format --check` on the files touched. An experiment that changes behavior a test pins records which tests and why in its notes rather than editing the tests to pass.
- If Rust code changes: `cargo fmt --check`, `cargo clippy --all-targets` and `cargo test`.
- The harness runs end to end for the variant and reports its metrics and time.

In Phase C, in addition:

- `uv run ruff check .` and `uv run ruff format --check .` over the whole pipeline, and `TestRealIce` with `TEXTURE_PIPELINE_OSI_DIR` set, every month and hemisphere within 10% of the toned source (C3).
- `uv run ty check` with no diagnostics beyond the 6 in `tests/test_milky_way.py` that `feature/baked-sea-ice` already has. At c83e683 this branch has 85, so the 79 it added are fixed in C2.
- Until C2, `baseline` stays byte-identical to `textures/`, faces and masks.
- The performance budget, measured.
- The goldens on WARP, lavapipe and Metal, and `ci.yml` green on all three operating systems.

## Open questions, answered at C1

- The combined en echelon and pinch leads (r3-11), and the solid look as a whole on the real map.
- The summer: the melt tone and the summer palette with the new surface, leads and floes; no lab reviewed them.
- The south: its lead density (the solid lab used the north's), its colors' extra headroom, the floes in the south's own seeds.
- Coasts other than green vegetation and snow: bare rock, dark forest in winter, the ice shelves; islands and lakes, which the synthetic coast had none of.
- The fast ice band with the new floes, and the rewired coast blend.
- The two labs estimated the references' scale differently (2.8 and 3.8 km per pixel for the Arctic mosaics). Nothing in production depends on it.

Answered at the review of `settled` (2026-10-14): the solid look on the real map, its combined leads included, and the coasts look good; the melt tone goes and the summer gets a little brighter; the south's colors move halfway toward the north's; the gray gaps go, and the pack closes above a tone curve's top (C1b). The user did not comment on the south's lead density, which stays as it is. The curve is `tone-70-85`'s.

## Departures

1. **Metric (a) is split into the coast's mismatch and lakes.** The evidence's measure counts every texel of the mask's land that is dark and bluish in all twelve months, and on `py` 732 of the baseline's 1,202 such texels poleward of 60 degrees are not connected to the mask's water at all: they are lakes, Great Bear Lake, Great Slave Lake and the Finnish lakes among them, which the Blue Marble draws as water in every month and which B1 leaves alone by design. Metric (a) therefore cannot fall near zero, and the harness reports it as defined and also split by whether the dark texel is connected to the mask's water through dark texels, four-connected: the connected part is the coast's mismatch that B1 is about (baseline 470 on `py` and 640 on `ny`), the rest are lakes (732 and 28).

2. **B1's thresholds are measured over the open ocean of the baseline's mask, where the sea ice record has no ice all year.** Measured over the shapefile's ocean as written, the south's water came out with a 95th percentile of luminance at 239, because the shapefile's ocean holds the Antarctic ice shelves, which the bake's ice pass keeps as land and on which the sea ice record, land in its own grid, has no ice. The measurement therefore reads the baseline's mask, the shapefile's ocean less what the ice pass keeps, at least 10 source pixels from anything else, poleward of 50 degrees, and only where OSI-450-a1's concentration stays below 15% in every month of 2004, so that seasonal ice in the Blue Marble does not count as water's brightness either. The thresholds are the 99th percentile of the brightest luminance over the year and the 1st percentile of the least blue less red, pooled over both hemispheres.

3. **B4 is built, folded into B3c, before the user decided on it.** After B2b the south's flat coasts still showed open water in September, and OSI-450's concentration within about 15 km of the flat Antarctic fronts reads 0.48 to 0.72 at the median against 0.91 to 1.0 at 50 to 100 km. The floes of B3c open water below 88%, so without a band they would line exactly those coasts with water and bring back the edge the user wants gone. At the orchestrator's direction the fast ice band goes in with the floes: within 25 km of the record's land, tapering out to 50 km, an ocean cell's concentration is raised toward the mean of the cells 50 to 100 km out, where that mean is above 80% to 90%, and never lowered, so open water offshore leaves the coast as the record has it. It works on the record's 25 km grid before the cap is made, and its tunables and code are part of the noise fields' cache key. The Ross Sea polynya at the Ross Ice Shelf front stays: the record's ocean cells there read 0.73 to 0.76 and the water further out is not closed either.

4. **The coast blend also gives the Blue Marble's sea on the mask's land the sea's look.** The plan has the coast's land take the ice's color where it is flat snow. In the top tenth of the south's flat coast dips after the first version, the darkest texel was mostly land on the mask (80%) holding the Blue Marble's dark sea (luminance 6 to 36), which B1's threshold of 14 had not joined, and the blend applied there was below 0.2 in 90% of them because that sea read as land without snow. Within the blend's band, pixels darker than 60 with blue above red by more than 8 are now left out of the reading of snow and relief and take what the sea beside them shows, the fill under the ice's coverage, at the band's falloff alone; the south's flat coast dip p90 fell from 66 to 51.

5. **The crumple has plates.** The plan's crumple is ridged noise drawing creases. Against the May 2020 reference, which shows crisp plates of slightly different tone, the ridged shading alone read as soft wrinkles, so the snow also breaks into cells of 30 and 120 km, each a little lighter or darker (1.5% at full activity, a quarter of it on flat pack), with creases along the smaller cells' borders. Superseded by departure 8.

6. **The crumple's creases are straight folds.** The plan's ridged noise, stretched three times along its direction field, drew parallel wavy ripples like dunes, and at an aspect of 1.3 it drew wormy wrinkles like stucco; per cell shades of Voronoi facets alone drew a granular stone. Neither reads as the user's crumpled sheet of paper flattened again. The shading and the creases now come mostly from straight folds (`Look.crumple_folds`): one every 30 km on average, 25 to 120 km long, at every angle, each a step in shade that fades over 10 km either side with a faint crease along it, tapering at its ends, so the creases meet at angles and enclose polygonal facets. The ridged noise keeps a quarter of the weight, the plates stay. The fields take about 35 s longer per hemisphere. Superseded by departure 8: the user found the folds too blocky and chose isotropic ridged relief.

7. **The edge model's streaks leave the pack, and the floes carry its open water.** The first iteration's edge model draws the open water a concentration leaves as a threshold noise streaked along the edge, with thresholds up to 0.88, so pack at 85% to 95% held gray elongated smudges 50 to 150 km long: 4.7% to 5.0% of the pack above 0.85 in the four renders (763,000 km2 in the north's March), and they dominated the north's August. These are most likely the brush dots the user disliked. With `Look.edge_pack` at 0.7 the threshold is squeezed into 0.12 to 0.7, so the streaks stay where the concentration falls toward the ice edge, whose shape is unchanged in form, and the pack's share fell to 0.1% to 0.6%. The open water the edge model then leaves undrawn, `1 - c / drawn`, goes to the floes from 95% concentration down, all of it below 80%, rather than the floes raising the edge's coverage as B3c first did; the floes do not open where the fast ice band raised the concentration by more than 0.03, so B4's coasts stay closed. The floes themselves are filled floes: each opens at a level of its own, clustered by a noise at 60 to 250 km so loose regions and compact ones alternate, to a gap of 0.08 to 0.32 cells, on cells of varied size (a 20 km grid thinned to 35% of its points); the water between them is gray new ice at 45% of the fill above 90% concentration and open water at 95% below 60%. Superseded by departures 8 and 10.

8. **The look was settled in standalone labs on synthetic data, not in the harness.** After B2b and B3 the user found the coast good, the surface not yet right, and each harness run of 5 to 7 minutes too slow to iterate on. Two labs replaced the harness for the look: synthetic squares at the cap's 2.873 km per texel, shown the way the app gets them (area averaged to about 4.9 km and through the JPEG XL round trip), regenerating every sample in seconds. The solid lab ran three rounds on closed ice, the floes lab five rounds on synthetic concentration and, in its last two, synthetic coasts; the user reviewed every round. The labs live in `tmp/sea-ice-lab/` (untracked); what they settled, their verdicts and code that reproduces their samples exactly are in the two research documents. B3's sub-steps and departures 5 to 7 are superseded by them, and the variants `b3a-crumple` to `b3d-melt` stay only until C2.

9. **The harness and the generator were sped up, which the plan did not have.** A profile found the noise fields built one after the other on one core and rebuilt whole on any tunable change. In 2260387 the parts of `prepare` and both hemispheres run at once, `streak`, `month_cap` and `coast_weight` work in threaded bands, each part of the fields has its own cache key, the harness bakes, renders and measures its four views at once and caches the coast weight, the mask faces and the snow colors, and `preview draft` renders one region in seconds. The output is unchanged: both `baseline` and `b3c-floes` came out identical to the unmodified code in every face, JPEG XL file, render and metric, and a full production bake wrote all 84 files byte-identical to `textures/`. The faster folds of the investigation were left out, since C1 removes the folds.

10. **The edge model goes; the floes draw all of the open water.** The plan kept the first iteration's edge model and drew floes beside it (departure 7). In the floes lab its threshold, streaked 250 km along the edge's direction, drew the "vertical brush strokes of water" the user rejected wherever the concentration sat in its range, and with the floes drawing all of the concentration's water it adds nothing: the ice edge is where the cascade's cells reach the 0% line. Its threshold and `streak` go in C1; the eddy shift the concentration is read through and the thin ice shading, which read two of its tunables, stay (floes document, section 5.1).

11. **The coast gets a rule of its own: add-only ice in a 10 km band.** The plan has the coast's land side blend toward the ice (B2b) and the fast ice band close coasts the record leaves open (B4), but nothing about floes and water against the land. On a synthetic worst case coast of green vegetation with a hard edge, floes and gaps touching the land drew the coastline; the user chose r5-06: in the sea within 10 km of land, coverage is 1 wherever the concentration is 50% or more, and nothing changes below. It needs land at the cap's spacing, which the cap does not have today, so C1 adds a land mask from the faces' water masks. Its cost, by construction: the band carries about 10 points more ice than its concentration, about 3 points within 40 km, and 0.2 points over a sample's whole sea, within metric (c)'s 10%.

12. **Every land texel of the cap takes the coverage of its nearest sea texel, and the thin ice shading reads the concentration extended into land.** C1 has the coast blend's coverage rewired that way. But `composite_face` reads the cap bilinearly, so at the face texels along every coast a land texel's coverage shows in the sea beside it, and the floes' own coverage on land, where the concentration is set to 0, is open water; read as it is, it would draw a dark rim along every coast, the edge the user wants gone. So the cap's coverage itself takes the nearest sea texel's on land, and `Cap.land_coverage` is not needed. For the same reason only the floes and the coast rule read the concentration set to 0 on land; the thin ice shading and the activity keep reading the record extended into land, as before, so the coast's land does not blend toward an ice shaded as thin as open water's.

13. **The land on the cap is coarser than the coastline.** The land mask is the six mask faces sampled at four points inside each cap texel, land where less than half is water. At the cap's 2.873 km it disagrees with the 5400 faces' masks on 0.44% of `py` and 0.08% of `ny` poleward of 60 degrees, all within 3 km of the faces' coastline, so the coast rule's band, three and a half cap texels, sits where the plan wants it. Islands smaller than two cap texels are mostly sea on the cap: 647 of the 1,294 islands on `py` poleward of 60 degrees, so 837 of their face texels. There the cascade's cells are not cut at the coastline and the coast rule does not reach; the floes treat those islands as sea, as the first iteration did everywhere.

14. **The cascade's per level sorts are bincounts, and its levels are made in turn.** The plan has each level's `unique` keys computed once per hemisphere. A key is the cell's index times two plus the land tag, so `bincount` over the keys gives the counts and the means directly, with no sort, in the same order as the documents' code, which it reproduces exactly; the per cell decisions run on threads, and the z table is computed once per process. The five levels of cells are made one after the other rather than at once, which costs nothing measurable and kept the cold run's peak commit under 8 GiB (8.73 GiB with the levels at once).

15. **The pack streak measure counts the floes' gray gaps for the settled variant.** The edge model is not drawn, so the streaks it measured are 0 by construction. With no edge model the measure reads the floes' own coverage instead, and reports 6.9% to 17.9% of the pack at 85% and more below 0.8: these are the gray gaps the floes draw by design, `(1 - c) / 0.45` of the area at a concentration `c`, not streaks.

16. **The pack streak measure also reads the concentration the floes read.** The measure of departure 15 reads the record's concentration zoomed to the cap without the eddy shift, which moves it by about 30 km, so at the pack's 85% it counts water the floes draw where the concentration they read is below 85%, and C1b's guarantee cannot show in it: `tone-70-85` reads 0.1% to 0.8% there. With the settled floes the harness now also reports the share where the concentration read through the eddy shift, as `month_cap` reads it, is 85% and more: 0 in all four views for `tone-70-85`, and 0.3% to 1.0% for `tone-80-90`, whose curve still draws water up to 90%. The read is `sea_ice_look.read_concentration`, which `month_cap` now calls, so the two cannot drift apart; `baseline` stayed byte-identical.

17. **B1's thresholds and B2's colors are constants, measured once.** C2 leaves the choice for the colors open; for B1 the plan has the production bake gain the harness's water without saying how it gets the thresholds, which the harness measured over the baseline's mask. Measuring in production would need a second ice pass over all twelve months only to find two numbers that do not change while the sources do not. So `ocean_masking.IMAGERY_WATER` holds the measured luminance 14 and blue above red 7, `ocean_masking.join_imagery_water` is the one join both the harness and `cube` call, and `Look.colors` holds `tone-70-85`'s tables; the harness's threshold measure is removed, and its `snow_colors` option stays, measuring again from the current table, for a future experiment. The new `baseline` made every face and mask face byte-identical to `tone-70-85`'s, which shows the defaults carry the chosen look.

18. **The coast blend runs on the polar faces alone.** C2 has `cube` gain the coast blend without saying on which faces; the harness bakes only `py` and `ny`. The side faces hold every coast equatorward of 35 to 45 degrees, and the blend there would do nothing for the ice: with the production fields, of the 785,048 land texels of `px` at the working size within the blend's 18 km of the water, and as many on `nx`, `pz` and `nz`, none has a coverage above 1% in any of the twelve months (the Bohai Sea's ice on `px` in January covers 15 texels of water, none of them at the coast). What the blend would do there is its other half, the Blue Marble's dark water on the mask's land taking the fill, along every coast of the world, a change the user has not seen and the ice does not need, at about 10 s per face and month, 480 s for the four. So `cube` blends `py` and `ny` (`main.POLAR_FACES`), as the harness does, and the side faces keep the first iteration's composite alone.
