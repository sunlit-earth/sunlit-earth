# Plan: Seasonal Sea Ice

## Summary

The globe gains sea ice that grows and melts with the date, in both hemispheres. The data is a climatology: for each calendar month, the mean sea ice concentration of 2010 to 2020 from EUMETSAT OSI SAF's climate data record OSI-450-a1, computed once by a new `sea-ice` command in the texture pipeline and shipped as one small lossless JPEG XL atlas beside the Moon and the Milky Way. Nothing is fetched at runtime.

The atlas holds 24 layers, a north and a south polar cap for each month, each 512 px square in a spherical polar Lambert azimuthal equal-area projection whose forward mapping from the surface normal is four multiplications and a square root in the shader. The engine keeps all 24 layers resident as one `R8Unorm` texture array, 6 MiB. Each frame interpolates the concentration between the two monthly means around the date, each anchored at the middle of its month, smooths it with a cubic B-spline, and thresholds it against a fixed noise field so the coarse 25 km field reads as floes and leads whose area fraction matches the data. Ice is drawn only where the water mask says water, is lit like the land around it, and takes the open-water share away from the glint and Fresnel terms.

The research behind it, sources and the measured facts about the files, is in [2026-10-10-sea-ice-research.md](2026-10-10-sea-ice-research.md).

## Stakes Classification

Medium. Everything is additive: a new asset, a new pipeline command, one new binding, one new setting and a new term in `fs_main`. Without the asset the globe draws as it does today. The risks are visual (how the ice looks next to the Blue Marble snow and coastlines) and cost on the software adapters, both of which Step 0 measures before the code moves. Every globe golden that shows polar ocean changes.

## Key Design Decisions

### 1. Source and window: OSI-450-a1 monthly files, 2010 to 2020

The 264 monthly files (11 years, 12 months, two hemispheres) are the input, about 240 MB. OSI-450-a1 is CC BY 4.0, the one candidate with an explicit license, and it ends on 2020-12-31. Its continuation OSI-430-a uses the same algorithm but is a separate, now suspended product whose consistency with the record is described only for 2019 and 2020 (research section 2), so the window stops at 2020 rather than mixing the two. The monthly files are used and not the daily ones: a monthly mean of daily fields is what a monthly file already is, and the dailies would be about 9 GB.

`ice_conc` is used as published. Its monthly 10% cut and its open-water filter only touch cells whose concentration in a given month and year is around or below 10%, at the outer edge of the marginal ice zone, where decision 5's 15% threshold draws little or nothing; reconstructing them from `raw_ice_conc_values` would move the eleven-year mean only there.

The mean of the eleven years is taken, not the median. The mean blurs the ice edge into a gradient a few cells wide, which is what decision 5 turns into a marginal ice zone of scattered floes; a median edge would be crisper and draw a hard line where the data does not have one.

### 2. The bake: a `sea-ice` command in the texture pipeline

`uv run texture-pipeline sea-ice --input <dir> --output textures/sea_ice_2010_2020.jxl`, with `--fetch` downloading whatever of the 264 files is missing from `<dir>` over HTTPS from THREDDS, one at a time as THREDDS asks, and writing their sha256 into a manifest beside them. In order:

1. For each hemisphere, read `ice_conc` (times 0.01, so percent) of every month and check that the land cells (the fill value) are the same in every file; the command fails if they are not, since the fill below assumes one land mask.
2. Average each calendar month over the eleven years, per cell.
3. Extend the field into land: every land cell within three cells (75 km) of ocean takes the value of its nearest ocean cell (`scipy.ndimage.distance_transform_edt` with `return_indices`), every other land cell becomes 0. The app's coastline comes from the water mask, which is finer than these cells, and without the extension a smooth sampler would pull the land cells' zero into the ice along every winter coast. The three-cell limit keeps the far interior from inheriting values, and with it the Caspian and the Great Lakes, which OSI-450-a1 does give values to, are left to the water mask, which counts them as land anyway (decision 7).
4. Reproject into the target grid of decision 3. For each target texel center, invert the spherical projection to a latitude and longitude, treat that latitude as geodetic, since the app's sphere places the Blue Marble's geodetic latitudes directly, take it to EASE2 kilometers with the ellipsoidal projection through PROJ (`rasterio.warp.transform` from EPSG:4326 to EPSG:6931 or EPSG:6932), and sample the extended field bilinearly. Outside the source grid the value is 0.
5. Quantize to 8 bits, `round(c / 100 * 255)`, and set the outermost ring of texels in each layer to 0, so a clamped sample past the edge of a cap reads no ice.
6. Write the atlas as a lossless single-channel JPEG XL, 12 columns by 2 rows of 512 px caps (6144 x 1024): the top row the north from January to December, the bottom row the south. That layout is the one a person can open and read.

`netCDF4` joins the pipeline's dependencies, unless Step 0 finds no wheel for the pipeline's Python 3.14, in which case the files are read through rasterio's GDAL netCDF driver, which is already in the dependency tree.

The command is run by hand like `cube`, and `textures/PROVENANCE.md` gets a section with the product, the DOI, the 264 file names with their checksums (from the manifest), the command line, the run's time, and the license.

### 3. The target grid: spherical polar Lambert azimuthal equal area, 512 px

The shader takes the unit normal `n` in the world frame (+Y north, +Z longitude 0, +X longitude 90 E, `cube.py`'s `equirect_coordinates`) to plane coordinates without trigonometry. With `k = sqrt(2 / (1 + n.y))` the north cap's point is `(k * n.x, -k * n.z)`, and with `k = sqrt(2 / (1 - n.y))` the south cap's is `(k * n.x, k * n.z)`, in units of the sphere's radius; these are the north and south polar aspects of the spherical projection with longitude 0 along the cap's vertical axis, as EASE2 has it. A cap spans `-RHO_MAX` to `RHO_MAX` on both axes with `RHO_MAX = 2 sin(27.5 degrees) = 0.9235`, which reaches 35 degrees of latitude on the axes and further toward the corners: far enough for the Bohai Sea and the Sea of Okhotsk at their winter maximum. At 512 px a texel is about 23 km across at the pole, close to the source's 25 km, so the reprojection neither throws away nor invents resolution. The projection is equal-area, so every texel of a cap covers the same area, which is what makes the extent check of criterion 1 a sum.

Baking into this grid instead of shipping the EASE2 grid as it is puts the ellipsoid-to-sphere step in the bake, where PROJ does it exactly, and leaves the shader the one formula above. Baking onto the cube instead was considered: it would read through `w` like every other surface, but ice reaches latitudes below the polar faces' edges (45 degrees at an edge's middle), so it would need all six faces of every month, and it would need a cube array or rebinding at every month change. The caps need neither.

The same formula lives in three places, the WGSL, the pipeline's inverse in step 4 of decision 2, and a Rust function the tests use, and each is pinned by a test against known points: the pipeline's against the `lat` and `lon` arrays of a real OSI-450-a1 file (committed as a handful of sampled points, not the file), the Rust function against the pipeline's.

### 4. Resident as one texture array, loaded like the other file-backed textures

`startup.rs` picks `sea_ice_2010_2020.jxl` from the textures directory beside the Moon and the Milky Way, and `xtask`'s `TEXTURE_FILES` gains it, so `bundle --verify` and `dist` check that it ships. The file is decoded on a loader thread through `texture_loader`'s decode with `Channels::Red`, and the engine thread cuts the decoded atlas into 24 layers of one `R8Unorm` `texture_2d_array` at 512 x 512 with a single mip, 6 MiB; the decoded buffer is dropped once uploaded, as the rules on decoded pixel buffers require. It needs no mips, because the cap's texels are never minified by more than the preview's scale and the B-spline of decision 5 reads at level 0 in any case.

It takes a slot after the Milky Way, which moves the cloud overlay from 3 to 4 through `SlotLayout`, whose job that is. The slot's texture is an array where the others are plain 2D textures, so the slot carries its view dimension; the implementer may instead make it a separate unit beside the slots, as the cube surface is, if that turns out simpler, and records which in the departures. Until it is resident, or when the file is missing or an LFS pointer, binding 9 holds a 1 x 1 stand-in of 24 layers of zero and the ice flag of decision 8 is cleared, so the globe draws as today. A missing file is logged once; readiness and the publish hold treat the atlas like the Moon.

All 24 layers stay resident on every adapter. Selecting the month is a uniform, so a change of month rebuilds no bind group and stages no upload.

### 5. The shader: interpolate, smooth, threshold, mix

A function `sea_ice(n, n_dx, n_dy) -> f32` returns the share of a fragment covered by ice:

1. Pick the cap by the sign of `n.y` and compute the plane point by decision 3. The two layers are `2 * month_a + cap` and `2 * month_b + cap`.
2. Read each layer with a cubic B-spline built from four bilinear `textureSampleLevel` taps at level 0, so eight taps for the two months, and mix the two by `ice_weight`. `textureSampleLevel` is allowed in non-uniform control flow, which the cap choice is.
3. Remap: `c = saturate((c - 0.15) / 0.85)`, the 15% convention of the products.
4. Threshold against noise: `ice = smoothstep(t - f, t + f, c)` with `t` a fixed noise value of the point on the sphere in (0, 1), and `f` the filter width from the noise's frequency and the pixel's footprint. The noise is two octaves of cellular noise in three dimensions on the unit normal, with feature sizes of tens of kilometers for the larger floes and a few kilometers for the smaller ones (Step 0 tunes both), mapped through its own measured distribution so that `t` is close to uniform; with uniform `t`, the expected coverage equals `c`. An octave whose features are smaller than about two pixels fades out, so the pattern does not shimmer at a distance.

What an octave fades to decides how the edge of the pack looks from afar, and it fades to a constant threshold, `EDGE_CONTOUR`, not to the coverage it would average to. With every octave faded, `t` is that constant and `f` is the larger of the noise's filter width and the pixel footprint of `c` itself (`fwidth(c)`, from the derivatives taken in uniform control flow), so `ice` is a step at the contour `c = EDGE_CONTOUR`, antialiased over about a pixel: a hard line between ice and water wherever the floes are too small to resolve. Averaging the coverage instead would keep the area exact at every zoom but draw the marginal ice zone of the eleven-year mean, a band tens to a few hundred kilometers wide, as a gray gradient on the far globe, which is a fuzzy edge in a place the eye expects a line. Closer in, the octaves resolve and the same band becomes floes that thin out toward open water, each with a hard edge of its own. `EDGE_CONTOUR` is tuned in Step 0 (a); a low value, near the 15% the extent is defined at, keeps the far globe's ice area close to the data's.

None of this blurs in time: the two months are mixed before the threshold, so as the date moves the contour moves and stays a line, and nothing cross-fades.

The derivatives that `f` needs are taken at the top of `fs_main` from the normal, in uniform control flow, where `globe_surface` already takes those of `w`.

In `fs_main` the ice is applied to the day color before the gamma and saturation corrections, so it gets the same color treatment as the land around it, and to the water share:

```wgsl
let cover = ice * surface.water;
day = mix(day, ice_color(n), cover);
water = surface.water * (1.0 - ice);
```

`surface.water` limits the ice to what the water mask calls water, so land, lakes and coastline texels on the land side never change, and `water` then drives the glint and the Fresnel term as today. A partly covered fragment shows the flat ocean color of the day surface between the floes, which is what leads look like at this scale.

The day-alone bind group gains the water mask at binding 6, since the ice needs it there too, and `globe_surface` samples it in that mode while the ice flag is set. The night-alone mode draws no ice: nothing on the night side is lit, and the night floor's ocean is the same color under ice and water.

### 6. The month blend: each mean anchored at the middle of its month

`scene::sea_ice::ice_blend(&DateTimeInput, now_utc) -> IceBlend` returns `month_a`, `month_b` and `weight`: a monthly mean stands for the middle of its month, the instant halfway between its first and its last moment, and a date between two such anchors interpolates linearly between them, December's and January's across the turn of the year. It reads the date through the same `CivilTime` as `scene::month`, and leap years move February's anchor.

This differs from the land's month in force, which is cut at mid-month with each month anchored at its first day (plan departure 11 of the seasonal textures). The ice follows the data, whose monthly mean is centered on the month, and the land's hard switch moves snow by up to half a month against it. The ice extent changes by a large fraction within a month in spring and autumn, so a continuous blend matters more for it than agreement with the land's switch.

`IceBlend` is derived beside the month in force in `Engine::sync_month` and handed to `write_uniforms`. `FrameState` compares it quantized (the weight to 1/256) beside the sun direction, outside the digest, so a date that moves the ice marks the frame dirty on its own.

### 7. What the ice does not reach

The water mask comes from Natural Earth's ocean layer, and decision 5 multiplies by it. Natural Earth keeps lakes in a separate layer, so the Great Lakes, and the Caspian if the ocean layer leaves it out as well (Step 0 (c) checks), get no ice although OSI-450-a1 has values for them. The Baltic is sea in both and gets its ice. The Antarctic ice shelves stay as the mask has them, land drawn from the Blue Marble, and the sea ice grows from their fronts outward.

The mask's ice pass also counts some Arctic coastal strips as land in every month, because it found snow there in some month of 2004 (`textures/PROVENANCE.md`). With sea ice drawn on the water side, those strips show the Blue Marble's own pixels in every month, which in summer may differ from the flattened ocean beside them. Step 0 looks at this; if it shows, narrowing the ice pass to the Antarctic shelves is a change to the cube bake and every pack, and a plan of its own.

### 8. The color and a setting

The ice color is two RGBA8 uniforms, one per hemisphere, which the engine computes from the date with the same blend as the concentration. The base is a slightly blue white matched by eye in Step 0 to the Blue Marble's own snow and ice shelves (RGB 241, 241, 241 in the source) and to MODIS true-color images of the Arctic in March, so that sea ice is a little darker and bluer than the ice sheets beside it. The color varies by month in both hemispheres, through a table of twelve values per hemisphere in `scene::sea_ice`, blended between months like the concentration. The Arctic's darkens toward a blue grey from June to August for melt ponds and bare ice; the Antarctic's changes far less, since its ice stays snow covered through the summer (research section 4), and its table may well come out nearly constant. The table's values are chosen by eye against true-color references of each month and are a look, not a measurement.

A "Sea ice" checkbox, on by default, joins the surface settings. It is a shader parameter and takes the fourteen edits of CLAUDE.md: a `sea_ice: bool` in `AppConfig` and `SceneParams`, one `scene_digest!` entry, and the next free bit of `flags`. A file without the key takes the default, so `config/migrate.rs` gets no step. The uniforms gain `ice_months` (both month indices packed in one `u32`), `ice_weight` and the two colors through `uniform_block!`, which takes the block's two trailing pads and grows it from 560 to 576 bytes.

### 9. Not chosen

- Painting the ice into the monthly day faces at bake time. The one water mask serves every month, so the ice would glint unless the mask became twelve masks; the ice would jump at every hand-over; and every month's packs would grow where today's ocean tiles are flagged constant and not stored.
- Live daily ice. None of the near-real-time sources states an acceptable-use policy for many end users, and a mirror of our own would be infrastructure to run; the research covers the options should it come back.
- A window past 2020 through OSI-430-a (decision 1).

## Success Criteria

1. The baked extent matches the source: for every month and hemisphere, the area of the atlas's texels at or above 15% concentration (texel count times the equal area of one texel) is within 3% of the area of the source cells at or above 15% in the eleven-year mean on the EASE2 grid. The pipeline's suite checks this on the real bake when the files are present.
2. Land does not change: with the ice on and off, a render in March of the Arctic and of the Antarctic is the same wherever the water mask is 0, byte for byte, and the same everywhere with the setting off as it is today.
3. The ice moves continuously: in the shading target, the ice coverage of a fixed patch sampled at every day of the year never jumps between consecutive days by more than the largest change between two neighboring anchors divided by the days between them, plus a tolerance.
4. The coverage matches the concentration where the floes are resolved: at the closest zoom, over a patch of uniform concentration `c`, the mean drawn coverage is within 0.05 of the remapped `c`, for `c` from 0 to 1 in steps of 0.1, and no ice is drawn at or below 15% at any zoom.
5. The edge is hard where they are not: at the farthest zoom, across a concentration ramp, coverage goes from below 0.1 to above 0.9 within two pixels.
6. Ice has no glint: with the sun's reflection centered on a fully covered patch, the patch's brightness is that of the diffusely lit ice alone.
7. Cost: a 1080p frame on WARP costs at most 10% more with the ice on than off, and a 4K frame on this machine's Radeon shows no difference above the measurement's noise. Step 0 measures the baseline.
8. The About window credits "Contains modified EUMETSAT OSI SAF data" with the product and DOI, and `textures/PROVENANCE.md` documents the bake.
9. `cargo test`, `cargo clippy --all-targets`, `cargo fmt --check` and the texture pipeline's suite (`uv run pytest`, ruff, ty) pass, and the goldens are regenerated for every adapter in `GENERATED_ADAPTERS`.

## Implementation Steps

### Step 0: Spikes

Each is short and each can change a decision, so they go first and their results go into the departures.

(a) The look. A throwaway bake of two months (March and September) and a throwaway shader branch: bilinear against B-spline sampling, the noise's two feature sizes, `EDGE_CONTOUR`, the base color and the monthly colors of both hemispheres, rendered over the Arctic and the Antarctic at three zooms and compared with MODIS true-color images of the same months from NASA Worldview. Decides decisions 5 and 8's tunables and whether the B-spline earns its cost; the colors of the ten other months are set the same way once the real bake exists, in Step 3.

(b) The cost. The same branch on WARP and lavapipe at 1080p and 4K, against the frame without it, and on the Radeon. If WARP exceeds criterion 7 with the B-spline, the bilinear read is the fallback, and if the noise is what costs, one octave.

(c) The coastal strips of decision 7, in July and August next to the new ice, and whether the shipped mask counts the Caspian as land.

(d) Reading the files: `netCDF4` under `uv` on Windows with Python 3.14, else rasterio's netCDF driver (decision 2).

### Step 1: The bake

The `sea-ice` command and `--fetch` in `tools/texture-pipeline`, a `sea_ice.py` module beside `cube.py` for steps 1 to 6 of decision 2, and tests: the projection and its inverse round-trip, the pinned points against the file's `lat` and `lon`, the land extension's three-cell limit, the quantization and the zero ring, the atlas layout, and a committed tiny fixture bake from synthetic NetCDF inputs that conftest writes, compared against a fresh bake the way the cube fixture is. Run the real bake, commit the atlas under Git LFS, and write its `PROVENANCE.md` section. Add a 64 px version of the real atlas to `crates/sunlit-core/tests/fixtures/earth/` beside the day, night and mask faces, kept equal to a fresh downsample by the pipeline's suite as those are.

### Step 2: The blend and the colors

`scene::sea_ice` with `ice_blend`, the color table and the Rust mirror of the projection, with unit tests: the weight at each anchor, continuity at every month boundary and at the turn of the year, February in a leap year, and the projection against the pipeline's points.

### Step 3: The renderer and the shader

The array texture and its stand-in, binding 9 in every bind group and the mask in the day-alone group, the slot or unit of decision 4, the uniforms, `write_uniforms`, the flag, `FrameState`'s comparison, and `sea_ice` with its use in `fs_main`. The shading target gets cases for criteria 2 to 6, driven by a synthetic array whose layers hold known concentration ramps. The golden suite gains an Arctic in March and an Arctic in September from the earth fixture, and every globe golden is regenerated per adapter.

### Step 4: Loading, the setting and the app

The loader path for the atlas, `startup.rs`, the checkbox, `apply_params_to_window` and `read_params_from_window`, the `AppConfig` field, the memory report's expected table and `memory::resident_texture_bytes` (6 MiB plus the stand-in), and `TEXTURE_FILES` in the xtask. Engine cases: a missing atlas renders without ice and logs once; an LFS pointer is skipped with a printed reason in the cases that need the real file; the ice appears once the atlas is resident and the frame is redrawn.

### Step 5: Documentation

`docs/rendering.md` (a section on the ice: the grid, the blend, the threshold and how it meets the mask), `docs/architecture.md` (the slot list and Months), `docs/assets.md`, `textures/PROVENANCE.md`, `assets/ATTRIBUTION.md` and `docs/roadmap.md`. README lists the textures and the settings; whatever it needs is proposed to the user and edited only with permission.

## Out of Scope

- Live or daily ice, and any data after 2020.
- Lake ice, river ice and icebergs.
- Changing the mask's ice pass or the cube bake (decision 7).
- Ice lit by the Moon on the night side; the night side has no moonlight on any surface.
- Ice in the cloud layer's shadow or under the atmosphere beyond what the land already gets.

## Risks and Mitigations

- The ice looks pasted on next to the Blue Marble's snow. Step 0 (a) matches the color to the textures' own snow and to true-color references before the shader is written for real, and the setting lets a user switch it off.
- The noise shimmers while the camera moves. Octaves fade by pixel footprint (decision 5), and the pattern is fixed on the sphere, so a still wallpaper never changes except where the ice edge moves.
- The software adapters pay too much. Step 0 (b) measures, with the bilinear read and a single octave as fallbacks; the setting turns it off entirely.
- False ice at coasts. The ice is limited to the water mask, and the land extension is three cells at most; if Step 0 shows spill-over along particular coasts, the bake can erode the field there, recorded as a departure.
- The climatology looks too generous for today. 2010 to 2020 is the most recent window one record covers; the extent is what the data says for that decade, and PROVENANCE.md names the window.
- License. CC BY 4.0 requires attribution and allows modification and redistribution; the atlas ships as data under that license with the credit in the About window and PROVENANCE.md, as the HYG stars ship under CC BY-SA.

## Rollback Strategy

Turning the setting's default off hides the feature without a release of the asset; removing the atlas from `textures/` and `TEXTURE_FILES` brings back today's picture exactly, since the stand-in path is what the globe draws without it. The pipeline command is independent of everything else in the pipeline.

## Settled with the user

- The checkbox of decision 8 is wanted, on by default.
- The ice color varies by month (decision 8).
- The mid-month anchoring of decision 6 is accepted, with its offset of up to half a month against the land's switch.
- The edge of the pack is a hard line where the floes are too small to resolve, not a gradient (decision 5).
