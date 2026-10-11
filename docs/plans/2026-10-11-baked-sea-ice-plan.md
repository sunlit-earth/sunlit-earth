# Plan: Sea Ice in the Day Faces

## Summary

The globe gains sea ice that grows and melts with the date, in both hemispheres, as part of the monthly day faces rather than as a layer drawn at run time. The texture pipeline's `cube` command gains a step: for each of the twelve months it generates the ice from the monthly mean concentration of 2004, the Blue Marble's own year, from EUMETSAT OSI SAF's OSI-450-a1, on the polar cap plane at a resolution finer than the face's, and composites it over the open water of the month's faces before the downsample. The ice then gets what the land already has: the same resolution, the same supersampled downsample, the same codec, the same BC7 transcode, the same tiles and mip chains, and the same color corrections. Nothing is fetched at run time, no texture slot, binding, uniform or setting is added, and the renderer changes in two places only: the pack builder stores day tiles that hold ice instead of flattening them to the ocean color, and the fragment shader takes the glint and the Fresnel term off a texel that is bright where the mask says water, which after this bake can only be ice.

This replaces the first plan, [2026-10-10-sea-ice-plan.md](2026-10-10-sea-ice-plan.md) on `feature/sea-ice`, whose shader generated floes in screen space so that the pack changed shape with zoom and camera angle. Its data work, the fetch, the land extension and the cap projection, is reused in the pipeline; its renderer, shader, setting and atlas are not. The research behind both is [2026-10-10-sea-ice-research.md](2026-10-10-sea-ice-research.md); section 7 holds this round's findings.

## Stakes Classification

Medium. The data is additive and the renderer changes are small, but the bake rewrites all 72 day faces and the mask under Git LFS, every user's tile cache rebuilds once, and every golden that shows polar ocean changes. The risk is visual: the ice has to look right beside the Blue Marble's snow at every zoom, and the look is judged by eye in Step 0 before anything reaches the faces.

## Key Design Decisions

### 1. The ice is part of the day faces

The Blue Marble's own snow is in the day faces and changes month to month, and the ice follows it: one picture per month, cut at mid-month with the floor (`architecture.md`, Months). The faces are 2048 px, about 4.9 km a texel, which is finer than any public concentration product and the resolution the land itself has at the poles, so the ice can never be softer or sharper than the coast beside it. Level of detail is the tile and mip machinery's, which is what the first prototype lacked.

What this gives up: a "Sea ice" setting, since the ice is in the picture like the snow (settled with the user), and the first plan's continuous movement of the edge between months, which the land does not have either. The night drawn alone shows no ice, as it shows no snow.

### 2. Source and year: OSI-450-a1 monthly files of 2004

The source stays OSI-450-a1, the first plan's decision 1: the one candidate with an explicit license (CC BY 4.0), both poles, every month, one record from 1978 to 2020 (research section 2). The year is 2004, the year of the Blue Marble the day faces are baked from: 24 monthly files, two hemispheres by twelve months, about 22 MB, which `--fetch` downloads with a manifest. No averaging: a monthly file is already the mean of that month's daily fields, so the ice edge of March 2004 stands where it stood in the month the land's snow line was photographed, and a coast the Blue Marble shows under snow has frozen sea beside it in the same month. A single year's monthly mean is also sharper than a decadal one: its gradient zone is where the edge moved within the month, not where it moved over eleven years, which suits decision 4. What is given up is the smaller Arctic of the 2010s; `PROVENANCE.md` names the year.

The 25 km grid is not the limit here, since the structure finer than the mean is generated (decision 4). Where the grid does hurt is stationary geography its land mask swallows, the Canadian Archipelago's channels, Svalbard's fjords, the Gulf of Finland, and in summer the channels the land extension fills from a still frozen ocean cell beside them. If Step 0 shows it, a finer 2004 exists only from AMSR-E: NSIDC's AU_SI12 at 12.5 km behind an Earthdata login, or Bremen's ASI at 6.25 km with no stated license (research sections 1 and 7.3). Neither is the first choice.

### 3. The bake: generated on the cap plane, composited into the faces

The `cube` command takes `--sea-ice <dir>`, the directory of the 24 files, with `--fetch` as the first plan's `sea-ice` command had it, and composites ice into every month; without the option it bakes as today. In order, with the first plan's `sea_ice.py` reused for the first three:

1. Read `ice_conc` of each month's file and check that the land cells are the same in every file of a hemisphere.
2. Extend the field three cells into land from the nearest ocean cell, so the smooth read does not pull land's zero into the ice along a coast; the app's mask gives the coastline.
3. Reproject each month's mean to the cap plane, the polar Lambert azimuthal equal-area projection to `RHO_MAX`, through the ellipsoidal inverse the first plan built, at `--sea-ice-cap-size` texels a side, default 4096 (2.87 km), with a cubic reconstruction of the 25 km field so no cell diamonds survive. Step 0 decides whether 8192 earns its memory.
4. Generate the ice's coverage and color on the cap (decision 4), one float layer each, the same noise fields for all twelve months.
5. For each face of each month, after the face is reprojected at 5400 and flattened through the 5400 mask, map every texel center to latitude and longitude as `reproject_face` already does, then to the cap, and sample the coverage and color bilinearly; below the cap's reach nothing is sampled. Composite `texel = mix(texel, ice_color, coverage * water)` with `water` the 5400 mask's share, so a coast texel is covered in proportion to its water, over the already flattened fill, and land is not touched. Then average down to 2048 as today.
6. Write `PROVENANCE.md`'s day section with the ice's source, the window, the file checksums and the credit.

Generating on the cap keeps every noise scale in kilometers and isotropic to within the projection's 3.5% at 60 degrees and 13% at the cap's edge; generating on the face grid would have to be done on five faces with their own warps. Compositing at 5400 and averaging down gives the ice the same antialiasing the coast has.

### 4. The look

What a monthly satellite view shows at 5 km a texel is not floes but three things, and the generator makes those and nothing finer. Its tunables live in one table in the module and are chosen by eye in Step 0 against MODIS true color from GIBS of the same months, recorded as a departure.

- **The edge.** The marginal zone is filaments 10 to 30 km wide and 100 to 300 km long and eddies of 50 to 150 km drawn along the edge by wind and current, thinning into dark water, hundreds of kilometers wide in the Antarctic winter and streaming south through Fram Strait. Coverage is a soft step of `c` against a threshold field `t = 0.5 + A n`, with `n` fractal noise in minus one to one at 20 to 150 km, domain-warped along the tangent of the concentration's gradient so it stretches along the edge, and `A` about 0.35. Where `c` is around a half this gives half the area as ice in streaks, so the drawn area follows the data on average and the monthly mean's gradient zone, where the edge moved during the month, becomes structure rather than a gray ramp. Where the threshold's center sits, at the 15% the products count ice from or higher, is a tunable Step 0 chooses.
- **The interior.** Bright, slightly blue white, a little below the ice sheets, nearly uniform. A sparse network of long, roughly oriented darker lines from the ridges of a cellular noise at 15 to 40 km, darkening 10 to 25%, denser as `c` falls from 1 toward 0.85, which is where the data itself says there is open water in the pack. A second noise at 100 to 300 km moves the brightness two to four percent so the pack does not read as a fill. Fast ice fills straits and fjords to the coast through the land extension and the mask, so the Canadian Archipelago reads as one white mass in winter.
- **Tone by season.** The first plan's color tables, one per hemisphere per month, chosen against MODIS and the Blue Marble's own snow at about 240: the Arctic from (224, 229, 236) in winter to (192, 197, 204) in August for melt ponds and bare ice, with mottling at 5 to 20 km from June to August; the Antarctic from (226, 231, 235) to (210, 215, 220) in January and February, snow covered all year. The color layer is the month's table color times the brightness noise, in the day faces' own sRGB.

The noise fields are seeded and fixed on the cap plane, the same in every month, so a change of month moves the edge and leaves the interior where it was.

### 5. The mask's ice pass keeps the Antarctic shelves only

The cube bake's ice pass counts what the detector finds on the ocean side of the coastline as land in every month: the Antarctic ice shelves, which are the same in every month and should stay land, and snow along Arctic coasts, which the roadmap records as a gray rim around the islands in summer (research 7.2). With generated ice on the water side, the Arctic strips would show the Blue Marble's own pixels inside the ice. The pass gets a hemisphere gate and runs south of 60 S only, so the Arctic strips become water, are flattened like the rest, and take the generated ice in the months the data gives it. This closes the roadmap item. The Caspian stays water and gets its winter ice; the Great Lakes stay land.

### 6. The pack builder stores day tiles that hold ice

A day tile is flagged constant ocean only when its whole footprint on the finest mask is open water and every texel of it lies within a tolerance of the pack's ocean color, the rule the night pack already applies for lights at sea, with a tolerance measured in Step 0 on the lossy faces (the night's is 4 levels). The pack's ocean color becomes the median per channel of the texels the mask calls open water instead of their mean, since ice is now among those texels and a winter month would pull a mean several levels toward white. On ice-free faces both rules flag the same tiles and give the same color as today.

### 7. The water term reads the ice off the day texel

After this bake a texel the mask calls open water is the flattened fill, (10, 30, 60) plus codec noise, or ice. The fragment shader scales `water` by one minus a `smoothstep` over the day texel's luminance, with its ramp above the fill's noise and below any ice, measured in Step 0; constant-ocean tiles return the exact fill and keep their glint. Nothing is added to the bind groups or the uniforms. The term reads the same texel the picture is drawn from, filaments, half-covered mip texels and all, so the glint and the Fresnel term fade with the ice exactly, at every zoom. The single-surface modes read no mask and draw no glint at all (`globe_surface`), so nothing is needed there.

### 8. Fixtures and goldens

The Earth fixture under `crates/sunlit-core/tests/fixtures/earth/` is regenerated from the new faces at 256 px and gains a March set, and `write_earth_fixture` lays March for March and July for every other month, so the existing cases keep their faces and the goldens gain the Arctic in March, the Arctic in September and the Antarctic in September. The pipeline's 16 px cube fixture is rebaked with synthetic monthly means from `conftest`, so its comparison against a fresh bake covers the ice step. The first plan's `sea_ice_points.csv`, the real file's latitude and longitude at pinned texels, moves to the pipeline's tests with the projection.

### 9. Not chosen

- **A higher-resolution ice atlas drawn in the shader**, 1024 or 2048 px caps with mips over two month layers. It keeps the toggle and the continuous edge but costs 24 to 96 MiB of texture, cross-fades two pictures between months, and at 1024 is visibly softer than the coast beside it. The toggle is not wanted.
- **A monthly water mask** to drive the glint instead of decision 7: twelve mask packs and a monthly mask in `SurfaceSet` against three lines of shader.
- **Keeping a small concentration atlas for the glint alone**: a slot, a binding, a loader path and a month blend to do what the day texel already says.
- **Photographic ice from MODIS or VIIRS composites**: polar night, cloud and ice alike in true color, and drift smearing a month of frames (research 6.1).
- **Real texture from the 89 GHz brightness temperature** as NASA's visualizations do (research 7.3): a second iteration if the generated interior looks too clean, not the first.
- **A multi-year climatology**, the first plan's 2010 to 2020: more recent, but 264 files against 24, an edge smeared over eleven years' positions, and ice that does not belong to the land's year.

## Success Criteria

1. The area follows the data: for every month and hemisphere, the coverage-weighted area of the composited ice over the mask's water is within 10% of the source's ice area, the sum of concentration times cell area over the month's mean on the EASE2 grid. The pipeline's suite checks it on the real bake when the files are present.
2. Land is untouched: before the encode, every texel the 5400 mask calls land (0) is identical with and without `--sea-ice`, and after it every such texel of the 2048 faces is within 2 levels, the codec's own spread.
3. Open water is the fill: before the encode, every texel the mask calls open water with no coverage is the fill, as today.
4. The look does not follow the view: the mean color of a 200 km patch of pack and of marginal ice rendered at the closest and the farthest zoom at which it fills more than 64 pixels agree within 2/255 per channel, in `tests/render_pipeline.rs` over the Earth fixture.
5. Ice has no glint: with the sun's reflection centered on a fully covered patch, the patch's brightness is that of the diffusely lit ice alone, and open water beside it glints as today.
6. Tiles: on the real faces, a day tile with any ice in its texels is stored, a tile of flattened open water is flagged constant ocean, and the pack's ocean color is within 2 levels of the fill in every month.
7. The bake stays affordable: the cube bake with `--sea-ice` runs in under 20 minutes on the machine `PROVENANCE.md` measured and peaks under 5 GiB.
8. Attribution: `assets/ATTRIBUTION.md` and the About window credit "Contains modified EUMETSAT OSI SAF data" with the product and its DOI, and `PROVENANCE.md` documents the ice in the day section.
9. The look is right: March and September of both poles at three zooms, judged by the user in the running app against Worldview true color of the same months.
10. `cargo test`, `cargo clippy --all-targets`, `cargo fmt --check` and the pipeline's suite (`uv run pytest`, ruff, ty) pass, and the goldens are regenerated for every adapter in `GENERATED_ADAPTERS`.

## Implementation Steps

### Step 0: Spikes

Each is short and each can change a decision; the results go into the departures.

(a) The look. A script in the pipeline that writes the generated cap of a month as a PNG beside a GIBS true-color image of the same month and pole: March, July and September in the north, February and September in the south. Iterate decision 4's tunables there, then composite two months into faces and render the Arctic and the Antarctic at three zooms through `sunlit-earth render`. Decides the tunables, 4096 against 8192 for the cap, and whether the 25 km geography of decision 2 needs a finer AMSR-E source.

(b) The brightness invariant. Decode the shipped faces and measure the luminance of every texel the mask calls open water, and of the same texels after the BC7 transcode, to set decision 7's ramp and decision 6's tolerance with margin; then the same on faces with ice, where the ice's darkest texel sets the other end.

(c) The ice pass. Bake the mask with the pass gated to the south and diff it against the shipped one: the Antarctic shelves must be all that remains, and the Blue Marble's pixels over the former Arctic strips in July and August are looked at before they are uncovered.

(d) Cost. Time and peak memory of a one-month bake with and without the ice step.

### Step 1: The pipeline

Port `sea_ice.py`, its tests, `conftest`'s synthetic NetCDF and the fixture from `feature/sea-ice` (fetch, manifest, land extension, cap geometry, extent check), dropping the eleven-year average and the atlas assembly. A new module for decision 4's generator with unit tests for the noise fields' determinism, the coverage's mean against `c` over a uniform patch, and the land and open-water invariants of criteria 2 and 3 on synthetic faces. The composite in `run_cube` with `--sea-ice`, `--fetch`, `--sea-ice-cap-size` and the hemisphere gate of decision 5. Rebake the 16 px cube fixture. Run the real bake, commit the 72 day faces and the 6 mask faces under Git LFS, and write `PROVENANCE.md`. The pipeline's README gets the option.

### Step 2: The renderer

`build.rs`: the day tiles' flat rule and the median ocean color (decision 6), with unit tests beside the night's. `sphere.wgsl`: decision 7's term, with cases in `tests/render_pipeline.rs` for criteria 4 and 5 over the Earth fixture. Regenerate the Earth fixture with March (decision 8), add the three polar goldens, and regenerate every golden per adapter.

### Step 3: Attribution, validation and documentation

`assets/ATTRIBUTION.md` and the About window's credit. Validation in the app for criterion 9, and a look at the winter Arctic under the cloud haze: if frozen sea keeps the veil the haze plan expected, the haze's land weight takes the same brightness rule off the day floor at its coarse level, recorded as a departure. Documentation: `docs/rendering.md` (The globe's surface: the ice is in the faces, the water term, the tile rule), `docs/architecture.md` (Months: the ice switches with the floor), `docs/testing.md`, `docs/assets.md`, `textures/PROVENANCE.md`, `docs/roadmap.md` (the feature, and the coastal strips item closed). README's texture list changes only with the user's permission.

## Out of Scope

- Live or daily ice, and any data after 2020.
- Lake ice beyond the Caspian, river ice and icebergs.
- Ice lit by the Moon on the night side.
- Real texture from brightness temperatures (decision 9).

## Risks and Mitigations

- The ice looks pasted on. Step 0 (a) judges it against true color before any face is written, and the color tables already come from that comparison.
- Codec ringing around bright ice over the dark fill makes open water near the edge brighter than the fill and the glint term misfires there. Step 0 (b) measures the spread and the ramp sits above it; the ramp's foot is far from the fill's own noise.
- The 25 km geography shows in straits. Step 0 (a) looks at the Archipelago and the Baltic; an AMSR-E product is the fallback (decision 2).
- The faces grow under LFS and the first start after the update rebuilds every pack. The ice has texture the flat fill did not; Step 1 records the new sizes against the table in `PROVENANCE.md`.
- The uncovered Arctic strips turn out to hold real land the shapefile puts in the sea, deltas or low shores, which would become flat sea in summer. Step 0 (c) diffs the two masks and looks at those places in July and August; if any are land, the pass keeps them through a narrower rule, recorded as a departure.
- License. CC BY 4.0 allows modification and redistribution with attribution; the faces are a modification of the data and carry the credit (criterion 8).

## Rollback Strategy

Baking without `--sea-ice` and the pass gate reproduces today's faces, and on ice-free faces decision 6's rule flags the same tiles as before and decision 7's term is inert, since nothing bright lies over mask water. The pipeline step and the two renderer changes are independent commits.

## Departures

None yet.

## Validation Record

None yet.

## Settled with the user

- The ice is baked into the day faces; the "Sea ice" setting of the first plan is dropped.
- Aesthetics come first and realism is not given up: the extent and the edge follow the data, the texture is generated.
- At least one picture per month.
- The ice is 2004's, the Blue Marble's year, one monthly mean per month and no averaging (decision 2).
- The app knows ice only by brightness over mask water, in the glint term and the tile builder (decisions 6 and 7).
- The mask's ice pass keeps the Antarctic shelves only (decision 5); Step 0 (c) still looks at what the Arctic strips uncover, and anything that turns out to be real land is recorded as a departure.
