# Sea ice look, second iteration: plan

Follows `docs/plans/2026-10-11-baked-sea-ice-plan.md` (PR #76), whose bake the user accepted as a first iteration. This plan improves the look of the baked sea ice in three ways the user found by comparing the app with MODIS true color imagery. It runs in two phases: experiments the user inspects and steers, then, once the user has chosen a solution, the production bake with tests, goldens, docs and validation. Nothing of the second phase starts before the user's decision.

Worktree `.worktrees/sea-ice-look`, branch `feature/sea-ice-look`, from `feature/baked-sea-ice` at 75055b3. It is rebased onto `main` once #76 is merged.

## The three imperfections

1. **The interior is too uniform, and its texture reads as dots and scratches.** The pack is a white base with soft grey blobs, light grey dots and short straight dark lines. Real sea ice, the Antarctic's above all, is covered in snow and looks much like the snow covered land: bright, with very fine detail, like a crumpled sheet of paper flattened again. The detail is not uniform: some areas show much more structure, others look almost flat.
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
- No git commits or pushes without the user's approval; the experiment work stays uncommitted until then.
- After inspecting A, B1 and B2 (2026-10-12): B1 and B2 are kept as the base of what follows; North Greenland's coast looks a lot better. A clear edge between land and sea ice remains, and the user wants the ice's texture to reach a little inland so that the edge disappears where the terrain is flat, as along much of Antarctica's coast, while mountain ranges may keep the coastline visible (B2b). B3 follows in full, and the user inspects the renders when both are done rather than after each sub-step.
- B1 stays poleward of 50 degrees, and Franz Josef Land's islands shrink to where the Blue Marble and Natural Earth agree. These were the orchestrator's recommendations; the user did not take them up, so they stand as defaults until the decision.

## Phase A: the preview harness

What it does:

- **Inputs.** The twelve Blue Marble JPEGs of 2004 at 21600 x 10800, `ne_10m_ocean.shp`, the 24 OSI-450-a1 files, and the shipped textures as the base every variant starts from.
- **Months.** North: March 2004 for winter, at the maximum extent, and August 2004 for summer, with the melt. South: September 2004 for winter and February 2004 for summer. The four are arguments with these defaults.
- **The bake.** Only what the renders read: `py` for the north's two months and `ny` for the south's, with the production settings (faces of 2048 from 5400, caps of 4096, JPEG XL quality 85 and effort 7), so what the codec does to fine detail is part of what the user sees. Anything that reads all twelve months, such as the mask, is computed from all twelve.
- **Caching.** Whatever a look change does not touch (the shapefile raster, the mask, the concentration caps, noise fields keyed by their tunables) is cached, so a look change turns around in a few minutes. The harness reports each variant's time.
- **The renders.** The app's headless `render` at 2048 x 2048, centered on each pole and framed to about 55 degrees of latitude, as the references are. It draws the day surface without lighting, because at the winter dates the poles are in polar night, with the atmosphere, the stars and the clouds off, on a date at which the month's faces are drawn alone (the polar goldens use the first of the month). It reads a variant's texture directory, the shipped set with the rebaked faces and mask in place, through a cache directory of its own, so the tile packs come from the new faces. Following `tmp/sea-ice/spike/render.sh` of the first iteration, every `SUNLIT_EARTH_*` directory points into the variant's output, and the app runs in a console of its own so it writes nothing into the user's app data.
- **The baseline.** The shipped textures through the same harness, as variant `baseline`.
- **Outputs.** `tmp/sea-ice-look/<variant>/`: the four renders, the faces and mask it baked, a short `notes.md` saying what the variant changes, and `metrics.json`. `tmp/sea-ice-look/index.html` shows them.
- **The page.** One self-contained HTML file, no network: a 2 x 2 grid of the renders labeled with pole, season and month; a variant selector; a comparison of the selected variant against the baseline (a toggle or a swipe); a click to view a render at full size and pan it; each variant's notes and metrics; and the three reference images, kept under `tmp/sea-ice-look/refs/`.
- **Metrics,** per variant: (a) the dark texels of the mask's land, as in the evidence, on `py` and `ny` poleward of 60 degrees (baseline 1,202 and 668); (b) the coastal snow against the ice, as in the evidence, for the rendered months; (c) each rendered month's ice area on the faces against the source's, which the first plan holds within 10%; (d) a measure of the ice interior's fine detail after the encode, for the texture work.

## Phase B: the experiments

Each step is one or more variants. For each, the implementer writes `notes.md` and the metrics; the orchestrator checks the result (the metrics, the renders against the baseline, nothing changed outside the ice and the coasts it means to change, no seams at the caps' or faces' edges); then the user inspects it and decides what follows.

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
- leads as a few long, branching, curving fracture systems hundreds of kilometers long, of varied width and darkness, fresh leads dark and refrozen ones grey, more in the Arctic than in the Antarctic;
- the marginal ice below about 85% concentration as floes, a cell pattern with dark water between bright floes, in place of the broken ice noise;
- the summer melt as a smooth regional tone instead of dots.

The crumple has to survive the encode at quality 85, which removes faint fine detail first; metric (d) shows whether it does.

**B4. A fast ice band, only if needed.** After B1, measure whether gaps of open water remain along coasts where the ice offshore is closed. Only if they do: a band of 10 to 20 km along the coast is filled where the offshore concentration is high. Some flaw polynyas along coasts are real, so the band follows the data.

**Fallback for B3.** If the procedural texture does not convince, the fine detail can be taken from cloud free MODIS scenes of pack ice: a detail layer cut from real imagery and placed on the caps. That brings clouds and shadows in the scenes, visible repetition and a new source to document, so it is a fallback only.

## Phase C: production, after the user's decision

Not started before the user has chosen what to keep. Then:

- the bake of all twelve months and all faces, and `textures/PROVENANCE.md` with the new command line, measurements and sizes;
- the pipeline's tests for what is kept, and the Rust tests whose fixtures or expectations move;
- the polar goldens on WARP, lavapipe and Metal;
- `docs/rendering.md`, `docs/assets.md`, `docs/roadmap.md`, and the pipeline's README;
- the shader's side: a brighter ice raises the night side's ice, which scales with the day texel's luminance up to 240; the ice gate's luminance of 52 to 76 is not affected; new water at the coasts takes the glint in summer; all three are checked in lit renders;
- validation rounds until one reports no major finding.

## Gates during the experiments

- `uv run pytest -q` in `tools/texture-pipeline`, and `uv run ruff check` and `uv run ruff format --check` on the files touched. An experiment that changes behavior a test pins records which tests and why in its notes rather than editing the tests to pass.
- If Rust code changes: `cargo fmt --check`, `cargo clippy --all-targets` and `cargo test`.
- The harness runs end to end for the variant and reports its metrics and time.

## Departures

1. **Metric (a) is split into the coast's mismatch and lakes.** The evidence's measure counts every texel of the mask's land that is dark and bluish in all twelve months, and on `py` 732 of the baseline's 1,202 such texels poleward of 60 degrees are not connected to the mask's water at all: they are lakes, Great Bear Lake, Great Slave Lake and the Finnish lakes among them, which the Blue Marble draws as water in every month and which B1 leaves alone by design. Metric (a) therefore cannot fall near zero, and the harness reports it as defined and also split by whether the dark texel is connected to the mask's water through dark texels, four-connected: the connected part is the coast's mismatch that B1 is about (baseline 470 on `py` and 640 on `ny`), the rest are lakes (732 and 28).

2. **B1's thresholds are measured over the open ocean of the baseline's mask, where the sea ice record has no ice all year.** Measured over the shapefile's ocean as written, the south's water came out with a 95th percentile of luminance at 239, because the shapefile's ocean holds the Antarctic ice shelves, which the bake's ice pass keeps as land and on which the sea ice record, land in its own grid, has no ice. The measurement therefore reads the baseline's mask, the shapefile's ocean less what the ice pass keeps, at least 10 source pixels from anything else, poleward of 50 degrees, and only where OSI-450-a1's concentration stays below 15% in every month of 2004, so that seasonal ice in the Blue Marble does not count as water's brightness either. The thresholds are the 99th percentile of the brightest luminance over the year and the 1st percentile of the least blue less red, pooled over both hemispheres.

3. **B4 is built, folded into B3c, before the user decided on it.** After B2b the south's flat coasts still showed open water in September, and OSI-450's concentration within about 15 km of the flat Antarctic fronts reads 0.48 to 0.72 at the median against 0.91 to 1.0 at 50 to 100 km. The floes of B3c open water below 88%, so without a band they would line exactly those coasts with water and bring back the edge the user wants gone. At the orchestrator's direction the fast ice band goes in with the floes: within 25 km of the record's land, tapering out to 50 km, an ocean cell's concentration is raised toward the mean of the cells 50 to 100 km out, where that mean is above 80% to 90%, and never lowered, so open water offshore leaves the coast as the record has it. It works on the record's 25 km grid before the cap is made, and its tunables and code are part of the noise fields' cache key. The Ross Sea polynya at the Ross Ice Shelf front stays: the record's ocean cells there read 0.73 to 0.76 and the water further out is not closed either.

4. **The coast blend also gives the Blue Marble's sea on the mask's land the sea's look.** The plan has the coast's land take the ice's color where it is flat snow. In the top tenth of the south's flat coast dips after the first version, the darkest texel was mostly land on the mask (80%) holding the Blue Marble's dark sea (luminance 6 to 36), which B1's threshold of 14 had not joined, and the blend applied there was below 0.2 in 90% of them because that sea read as land without snow. Within the blend's band, pixels darker than 60 with blue above red by more than 8 are now left out of the reading of snow and relief and take what the sea beside them shows, the fill under the ice's coverage, at the band's falloff alone; the south's flat coast dip p90 fell from 66 to 51.

5. **The crumple has plates.** The plan's crumple is ridged noise drawing creases. Against the May 2020 reference, which shows crisp plates of slightly different tone, the ridged shading alone read as soft wrinkles, so the snow also breaks into cells of 30 and 120 km, each a little lighter or darker (1.5% at full activity, a quarter of it on flat pack), with creases along the smaller cells' borders.

6. **The crumple's creases are straight folds.** The plan's ridged noise, stretched three times along its direction field, drew parallel wavy ripples like dunes, and at an aspect of 1.3 it drew wormy wrinkles like stucco; per cell shades of Voronoi facets alone drew a granular stone. Neither reads as the user's crumpled sheet of paper flattened again. The shading and the creases now come mostly from straight folds (`Look.crumple_folds`): one every 30 km on average, 25 to 120 km long, at every angle, each a step in shade that fades over 10 km either side with a faint crease along it, tapering at its ends, so the creases meet at angles and enclose polygonal facets. The ridged noise keeps a quarter of the weight, the plates stay. The fields take about 35 s longer per hemisphere.

7. **The edge model's streaks leave the pack, and the floes carry its open water.** The first iteration's edge model draws the open water a concentration leaves as a threshold noise streaked along the edge, with thresholds up to 0.88, so pack at 85% to 95% held grey elongated smudges 50 to 150 km long: 4.7% to 5.0% of the pack above 0.85 in the four renders (763,000 km2 in the north's March), and they dominated the north's August. These are most likely the brush dots the user disliked. With `Look.edge_pack` at 0.7 the threshold is squeezed into 0.12 to 0.7, so the streaks stay where the concentration falls toward the ice edge, whose shape is unchanged in form, and the pack's share fell to 0.1% to 0.6%. The open water the edge model then leaves undrawn, `1 - c / drawn`, goes to the floes from 95% concentration down, all of it below 80%, rather than the floes raising the edge's coverage as B3c first did; the floes do not open where the fast ice band raised the concentration by more than 0.03, so B4's coasts stay closed. The floes themselves are filled floes: each opens at a level of its own, clustered by a noise at 60 to 250 km so loose regions and compact ones alternate, to a gap of 0.08 to 0.32 cells, on cells of varied size (a 20 km grid thinned to 35% of its points); the water between them is grey new ice at 45% of the fill above 90% concentration and open water at 95% below 60%.
