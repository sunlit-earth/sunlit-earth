# Research: Seasonal Textures and a Tiled Texture System

Code reconnaissance, seven delegated research passes, and measurements on the shipped day map and on this machine, all on 2026-09-29. This is the research behind the roadmap item "Seasonal texture switching" and the wider question of how the globe's surface should be stored, loaded and sampled. The plan that uses it is not written yet.

Every claim below carries how far it was checked. [V] means verified against a source, a document or the wgpu 28 sources in the local cargo registry, with the source named. [M] means measured, on the shipped `world.topo.200405.jxl`, on this machine (Ryzen 7 5800X, AMD RX 6800 XT, Windows 11), or on a probe compiled against wgpu 28.0.0. [R] means reasoned from arithmetic that is shown. [A] means assumed. Measurement scripts were written in the session's scratch directory and are named where a number comes from one; they are not in the tree.

## 1. What the measurements say about the three premises

The overhaul was proposed on three premises: equirectangular maps waste pixels near the poles, most of the surface is ocean and needs no texture, and at most half the sphere is visible. All three are true. Each buys less than it seems, and a fourth lever that was not in the question buys more.

| Lever | What it saves at today's 8192 density | Basis |
|---|---|---|
| Cube map instead of equirectangular | 25% of the texels (25.2 M against 33.5 M) | [R] 6 x 2048^2 against 8192 x 4096 |
| Skipping tiles that are pure ocean | 25% to 31% of 256 px tiles, 7% to 10% of 512 px tiles | [M] section 4 |
| Keeping only what the camera can see | about 50% at the far end of the zoom, 83% at the near end, before frustum cropping | [R] the visible cap is (1 - 1/d)/2 |
| BC7 instead of RGBA8 | 75% of every byte that remains | [V] 8 bits per texel, section 6 |

The ocean saving is small because coastlines and islands touch most tiles: at 256 px only about a quarter of the cube's tiles carry no land at all, and at 1024 px in the equirectangular layout none do. The visible-set saving is real but bounded, because the horizon only ever removes half the sphere.

Two consequences shape the recommendation.

First, at today's density the whole land pyramid fits comfortably. A 2048-per-face cube with all six faces and all mips is 128 MiB in RGBA8 and 32 MiB in BC7, against 171 MiB for one 8K equirectangular map today [R]. Only the tiles with land, with their mips, come to about 129 MiB RGBA8 and 24 to 32 MiB BC7 [M]. So at 8192 nothing needs to be streamed for memory's sake; a resident cube in BC7 already cuts the day map by more than five times.

Second, streaming pays where the density goes beyond 8192. The 8K level is exactly what a 4K frame needs when the globe fills it: the default field of view is 20 degrees (`scene/camera.rs`, `DEFAULT_CAMERA_FOV`), a 2160-line frame is filled at a camera distance of 5.76 radii, and there an 8K texel at the sub-camera point projects to 4.7 / (d - 1) = 1.0 pixel [R]. Closer than that, the frame shows a patch of the sphere and finer levels would be visible; farther, 4096 and then 2048 suffice. A 1080p wallpaper never needs more than 4096. The NASA sources go to 21600 px wide, 2.6 times finer than what ships, and a tiled, visible-only cache is what makes that detail affordable: the worst-case working set of a tiled scheme is about 1.2 to 1.4 times the frame's pixel count divided by the tile area, whatever the finest level is [M], about 150 tiles of 256 px for a 4K frame.

So the shape of the answer is two stages. Stage one changes the projection, the format and the month set, and keeps whole surfaces resident. Stage two adds tiles, a page table and a streaming loader, for levels finer than 8192 and for the memory ceiling the user asked for. Sections 5 to 11 have the detail, section 14 the recommendation and what a plan should prototype first.

## 2. Today's texture path

Read from the tree on 2026-09-29.

- Four file-backed textures in slot order after the procedural grid: day (`world.topo.200405.jxl`, 8192 x 4096), night (`BlackMarble_2016.jxl`, 8192 x 4096), Moon (1024 x 512), Milky Way (4096 x 2048); the cloud overlay is last and comes from the fetcher (`renderer/slots.rs`, `startup.rs`). `SlotLayout` derives the cloud slot from the number of paths so that a file-backed texture can be added without moving it.
- The day map's alpha is a water mask: land 255, ocean 128, coastline in between; `fs_main` reads it as `saturate((1 - a) * 2)` for the Fresnel and specular terms. The RGB under the ocean is a flat fill produced by the texture pipeline's ocean masking stage (`tools/texture-pipeline`, `--ocean-mask`).
- Each texture decodes on its own background thread through `texture_cache::load_at_resolution`, which halves the 8K source once for the 4096 and 2048 settings and caches the result as PNG under the cache directory. The decode posts into a latest-value mailbox with one slot per texture and a generation stamp; the engine drains it and builds the GPU texture with a CPU box-filter mip chain on the engine thread (`renderer/textures.rs`). An 8K decode is 0.3 to 1.3 s in release builds (`2026-03-15-jxl-decode-performance-research.md`), and single-threaded on this machine 2.39 s [M].
- The globe is a UV sphere of 64 stacks and 64 sectors with equirectangular UVs, sampled with `textureSample`, trilinear, 16x anisotropic. The four shells (clouds, Rayleigh, two nightglow layers) reuse the same mesh and UVs.
- The device is requested with `Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits())` and no feature beyond `TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES` (`wgpu_init.rs`); adapters are enumerated over `Backends::all()`.
- A wallpaper publish is held back while `Renderer::textures_pending` says a texture the current mode needs is on its way, so the desktop never shows the grid; `textures_ready` is what `run_render` and the tests wait for.
- Measured on the real assets: 1259.7 MiB of private bytes at 8192 against 616.5 MiB at 2048 (`docs/architecture.md`). `memory::resident_texture_bytes` budgets three resident 8K textures at four thirds of their base level.
- `SceneParams.datetime` carries the custom day of year; the derived sun direction is compared outside the digest, which is the pattern a derived month weight would follow.

## 3. The data: Blue Marble Next Generation, twelve months of 2004

All three variants exist for every month: base, with topography (what ships now, `world.topo`), and with topography and bathymetry. NASA re-hosted the files in December 2025 under one URL pattern; all 72 monthly JPEGs at the two sizes answered 200 when fetched [V]:

```
https://assets.science.nasa.gov/content/dam/science/esd/eo/images/bmng/<bmng-base|bmng-topography|bmng-topography-bathymetry>/<monthname>/<stem>.3x<W>x<H>.jpg
```

with the stem `world.2004MM`, `world.topo.2004MM` or `world.topo.bathy.2004MM`, `_geo.tif` in place of `.jpg` for GeoTIFF, and the 500 m data as eight tiles `<stem>.3x21600x21600.<A-D><1-2>.jpg`. The server sends `Accept-Ranges: bytes`. The old `visibleearth.nasa.gov` and `neo.gsfc.nasa.gov` addresses redirect to `science.nasa.gov` [V].

| Set | Pixels | Ground size per pixel [R] | JPEG per month [V] | Twelve months [R] |
|---|---|---|---|---|
| "8 km" | 5400 x 2700 | 7.4 km at the equator | 1.6 to 1.9 MB | 21 MB |
| "2 km" | 21600 x 10800 | 1.85 km | 21.8 to 24.5 MB (topography) | 279 MB |
| "500 m" | 8 tiles of 21600 x 21600 | 463 m | 371 MB (May, topography) | 4.5 GB |

The current 8192 map is above the 5400 set's native width, so any bake at 8192 or finer starts from the 21600 files: 279 MB of downloads for the twelve topography months, kept out of the repository like the sources today.

License and credit [V]: NASA imagery is not copyrighted in the US, and the BMNG page asks that anyone republishing credit "NASA Earth Observatory"; the data were produced by Reto Stöckli (NASA Earth Observatory, GSFC). Antarctica is the Landsat Image Mosaic of Antarctica (USGS/NASA), and the bathymetry variant uses GEBCO data with its own credit line. The About window carries the NASA credit already.

Quirks measured on the 5400 base JPEGs of all twelve months and read in the BMNG paper [M][V]:

- The ocean is a uniform dark blue in every month and there is no sea ice anywhere: the Arctic Ocean is the same flat fill in January and July. The paper says ocean pixels are averaged over the year. The set therefore cannot deliver seasonal sea ice, which the ocean masking plan once hoped to preserve; what changes month to month is snow and vegetation on land, and only that.
- Antarctica is static: south of 60 S the pixels are identical in every month (a single Landsat fill).
- Snow is a hard replacement, not a blend: the paper says snow-flagged reflectances replace the snow-free ones where at least half of a month's 8-day composites are snow-flagged. Snow edges therefore jump between months, and NASA notes that short-lived snow is not fully distinguished from cloud. The largest month-to-month change is May to June (mean absolute difference 6.1 on 8-bit RGB) and September to October (5.0); other pairs are 1.2 to 3.6.
- Brightness is consistent across months: the Sahara and the Pacific are within about 2 levels of their yearly mean, so a cross-fade will not pulse.
- North of about 78 N the winter months have no data and show a dark fill; the antimeridian seam is clean between 30 N and 60 S and shows a hard discontinuity in the top rows above about 75 N.
- No month carries a nominal date. NASA describes each image as the whole month; Web WorldWind anchors each at the first and switches to the nearest, which lands the switch mid-month.

Seasonal night lights exist as Black Marble monthly radiance products (VNP46A3, 15 arc-second HDF5, Earthdata login) but are data rather than pictures, and city lights barely change month to month; the annual composite that ships is the right night map [V][A]. No freely usable monthly composite newer or finer than BMNG 2004 was found; EOX Sentinel-2 cloudless is yearly and CC BY-NC-SA, which rules it out for a redistributed app [V].

## 4. Measurements on the shipped day map

Script `landmask-measure/measure.py`, 31 s over the decoded 8192 x 4096 RGBA file; equirectangular rows weighted by cos(latitude) for area. Conventions: the app's world frame from `scene/camera.rs`, +Y north, +Z longitude 0, +X 90 E; cube faces by the OpenGL and Direct3D table; "has land" is alpha above 128.

- Alpha takes 129 distinct values: 128 on 64.3% of the pixels, 255 on 34.0%, coastline values in between on 1.7%. Weighted by area, pure water is 69.4% of the sphere and any water 71.1%, which matches the textbook figure [M].
- The ocean fill is (9, 29, 59), not the (10, 40, 80) the pipeline's default suggests; 4,039 distinct RGB values occur under the mask, 0.6% of the water pixels are more than 2 levels from the fill, and 31 pixels are more than 16 away. That is encoding noise from the lossy JXL [M][A]. A bake that wants a "constant ocean" tile must classify by the mask and write the fill exactly, not trust the decoded color.
- Ice shelves and the Antarctic interior are alpha 255 at RGB (241, 241, 241): ice on land counts as land, and no bright pixel anywhere is water-masked [M].
- Land fraction per band: 100% south of 80 S, 73% at 70 to 80 S, 1% to 4% between 40 and 60 S, 24% to 26% in the tropics, 53% to 75% between 40 and 70 N, 36% at 70 to 80 N and 13% at the top band.

Tile occupancy [M]:

| Layout | Tile | Total tiles | Pure water | Land tiles | Stored pixels against the full 8K map |
|---|---|---|---|---|---|
| Equirectangular | 128 | 2048 | 41.4% | 1201 | 58.6% |
| Equirectangular | 256 | 512 | 24.6% | 386 | 75.4% |
| Equirectangular | 512 | 128 | 7.0% | 119 | 93.0% |
| Equirectangular | 1024 | 32 | 0 | 32 | 100% |
| Cube, 2048 faces | 128 | 1536 | 45.9% | 831 | 40.6% |
| Cube, 2048 faces | 256 | 384 | 31.0% | 265 | 51.8% |
| Cube, 2048 faces | 512 | 96 | 10.4% | 86 | 67.2% |

Two other passes counted the same 256 px cube tiles and got 254 and 290 land tiles, from a different face warp (section 5), a gutter included in the classification, and a different land threshold. The range 254 to 290 of 384 is the honest figure: a 256 px cube tile set stores 30% fewer pixels than equirectangular land tiles of the same size, and about half of the full equirectangular map.

The visible set, cube of 2048 faces and 256 px tiles, finest level only, twelve sub-camera points at the vertices of an icosahedron, a tile counted when its center is within the horizon (the conservative variant counts a tile whose bounding cone reaches the horizon) [M]:

| Camera distance (radii) | Cap fraction | Tiles within the horizon | Land tiles min / mean / max | RGBA8 at the max (MiB) | BC7 at the max (MiB) | Conservative: land max, RGBA8, BC7 |
|---|---|---|---|---|---|---|
| 1.5 | 16.7% | 66 | 29 / 44 / 64 | 16.0 | 4.0 | 81, 20.3, 5.1 |
| 2 | 25.0% | 98 | 51 / 68 / 88 | 22.0 | 5.5 | 108, 27.0, 6.8 |
| 3 | 33.3% | 128 | 69 / 89 / 112 | 28.0 | 7.0 | 130, 32.5, 8.1 |
| 5 | 40.0% | 148 | 80 / 102 / 128 | 32.0 | 8.0 | 151, 37.8, 9.4 |
| 8 | 43.8% | 172 | 95 / 118 / 143 | 35.8 | 8.9 | 162, 40.5, 10.1 |
| 20 | 47.5% | 178 | 99 / 123 / 149 | 37.3 | 9.3 | 168, 42.0, 10.5 |
| 80 | 49.4% | 182 | 103 / 126 / 151 | 37.8 | 9.4 | 170, 42.5, 10.6 |

The coarser levels add about a third. The horizon alone leaves half the tiles at the far end, the ocean removes about 30% of what is left, and a land-heavy hemisphere seen from far away is the worst case: 151 to 170 tiles, 38 to 43 MiB in RGBA8 or 9 to 11 MiB in BC7 at the finest level. That is the number "only what is visible" buys at 8192: 3.4 times under the 128 MiB base level of today's map in the same format. Frustum culling at close zoom cuts much more; a second simulation with the level-of-detail rule applied per pixel (section 8) found 6 tiles at 1.5 radii.

## 5. Projection

Scripts `density.py`, `tiles.py`, `resid.py`; numerical Jacobians over each mapping; the waste column is total texels divided by an ideal equal-area map at 8K equatorial density, 8192^2 / pi = 21.4 M texels [M][R]. The scale of each candidate is chosen so that no texel is larger in area than one 8K equatorial texel ("area") or longer on any side ("linear").

| Projection | Texel area ratio, max over min | Max anisotropy | Waste, area criterion | Waste, linear criterion |
|---|---|---|---|---|
| Equirectangular | unbounded (2608 in the last row) | unbounded | 1.571 (33.6 M) | 1.571 |
| Equirectangular, width halved per band where cos(lat) <= 2^-k | 2 | 2 | 1.225 (1.325 with 256-row bands) | same |
| Cube, gnomonic (plain) | 5.18 | 1.73 | 1.910 (40.8 M) | 1.910 |
| Cube, tangent warp theta = pi/4 (equi-angular) | 1.41 | 1.73 | 1.178 (25.2 M = 6 x 2048^2) | 1.569 |
| Cube, tangent warp theta = 0.8687 | 1.30 | 1.73 | 1.140 | 1.971 |
| Cube, COBE six-parameter | 1.02 | 1.73 | 1.012 | 1.749 |
| Octahedral | 5.18 | 2.62 | 1.654 | 2.865 |
| Octahedral, equal-area | 1.00 | 3.00 | 1.000 | 2.995 |
| HEALPix | 1.00 | 2.43 | 1.000 | 2.431 |

The warps and the optimal parameters are from Zucker and Higashi, "Cube-to-sphere Projections for Procedural Texturing and Beyond", JCGT 7(2) 2018, which also records Google VR's use of the pi/4 tangent warp [V]. Every cube variant reaches an anisotropy of the square root of 3 at the eight corners, where three faces meet at 120 degrees [R].

The equi-angular cube (EAC, the pi/4 warp) is the recommendation, for these reasons:

- Along a face's axes it is exactly uniform, 2048 texels per 90 degrees, which is the 8K equator density; its largest texel area is exactly one equatorial texel's. Its longest texel side exceeds that by more than 5% on 10.5% of the sphere and reaches 1.154 at the corners [M].
- The inverse is one line, branchless, and keeps the major axis: `w = atan(n / max(|n.x|, |n.y|, |n.z|)) * 4 / pi`, because atan(±1) * 4 / pi = ±1, so the hardware picks the same face and receives equal-angle coordinates [R]. COBE needs an iterative inverse whose closed-form start errs by 1.9 texels at 2048; HEALPix needs a zone test and twelve rotated base pixels; octahedral needs a mirrored fold.
- Stored as a real `texture_cube`, the hardware filters across face edges: seamless cube filtering is the Vulkan default (the non-seamless extension exists to turn it off) [V]. Mips are per face and the warped direction is continuous across edges, so a 2 x 2 quad straddling an edge gets sane derivatives [R]. Desktop GL does not filter seamlessly under wgpu (gfx-rs/wgpu issue 10486) [V]; since the app enumerates every backend, a machine that lands on GL would show seams at face edges. That is a risk to accept or to gate.
- A 2048 face fits even the unraised `downlevel_webgl2` 2D limit of 2048 [V].

Banded equirectangular is the one scheme that beats it under the linear criterion, and it would keep the existing UV path and the cloud overlay's layout untouched, at the price of band seams that need resampled gutters and special handling at the poles, where the anisotropy of the plain layout passes 16 above 86.4 degrees [R]. It is the fallback if the cube's reprojection bake or the GL seam risk is judged too expensive.

A cube costs one reprojection resample at bake time (about 50 lines of inverse mapping with bilinear or bicubic sampling; numpy did an 8K day and night pair in 23 s single-threaded [M]), an orientation convention that a golden test must guard, and an extra three `atan` per fragment. The cube can also be rotated about the polar axis at bake time to put its eight corners, where the resolution is lowest, over ocean; in the default orientation two of the northern corners fall on Iran and Japan [R][A].

The cloud overlay, the Moon and the Milky Way keep their equirectangular UVs; only the globe's own surface samples through the cube.

## 6. What wgpu 28 permits

Verified in the wgpu 28.0.0 sources (wgpu-core 28.0.1, wgpu-hal 28.0.1, wgpu-types 28.0.0, naga 28.0.0) and on a probe compiled against wgpu 28.0.0 and run on this machine [V][M]. Note that 28.0.0 is the only 28 release; 30.0.1 is current.

Compressed formats:

| Adapter | BC (1 to 7) | ETC2 | ASTC | Evidence |
|---|---|---|---|---|
| Vulkan, AMD RX 6800 XT | yes | no | no | probe |
| DX12, AMD RX 6800 XT | yes | no | no | probe |
| WARP (DX12 software) | yes | no | no | probe; the DX12 backend advertises BC unconditionally |
| Mesa lavapipe | yes | no | no | Mesa `lvp_device.c` and `vulkaninfo` in WSL |
| Metal, Intel Mac | yes | no | no | `metal/adapter.rs`: `format_bc = os_type == Macos` |
| Metal, Apple Silicon | yes | yes | yes | same; that Apple Silicon answers the deprecated feature-set query is inferred |
| Linux aarch64 with Mali or Broadcom GPUs | often no | often yes | often yes | inferred, not verified |

BC7 is the one compressed format every desktop adapter this app runs on accepts, the software adapters included. Whether GitHub's macOS "Apple Paravirtual device" decodes it correctly is unverified; the golden tests would show it. The feature must be requested at device creation and only when `adapter.features()` contains it, so an RGBA8 path stays as the fallback. BC copies must be 4 px aligned, and BC textures cannot be render targets, so their mips are produced offline [V].

Limits: `downlevel_webgl2_defaults()` sets `max_texture_array_layers` to 256, `max_sampled_textures_per_shader_stage` to 16, `max_uniform_buffer_binding_size` to 16 KiB, every storage and compute limit to 0 and `max_binding_array_elements_per_shader_stage` to 0; `using_resolution` copies only the three texture dimension limits [V]. Consequences: any compute pipeline fails validation under the current request, binding arrays are unavailable, and a tile cache is capped at 256 layers unless `max_texture_array_layers` is taken from the adapter, which offers 8192 on Vulkan (AMD), 2048 on DX12, lavapipe and Metal [M][V].

Texture arrays and writes: a 2D texture with layers, viewed as `D2Array` and sampled as `texture_2d_array`, is the portable tile cache. `Queue::write_texture` writes a sub-rectangle of one layer and one mip and touches nothing else. Each call synchronously allocates a transient staging buffer, copies the data in, and records the copy into a pending-writes encoder that `submit` places ahead of the submitted command buffers, so a write issued after a frame's submit lands after that frame on the GPU, and a frame encoded before the write but submitted after it sees the new data. Staging memory lives until the consuming submission completes, so hundreds of tiles between two submits hold all their bytes at once; budget uploads by bytes per tick and submit after each batch. If the first write to a layer and mip does not cover it, wgpu clears the whole layer first, so a pooled layer should be written in full once at creation [V].

Binding arrays (`binding_array<texture_2d<f32>>`) need `TEXTURE_BINDING_ARRAY` and a non-zero element limit; they work on Vulkan with descriptor indexing, DX12 tier 3 (WARP reports them), lavapipe, and Metal only with argument buffers tier 2, which Intel Mac GPUs lack [V][A]. A plain texture array is therefore the portable route.

Sparse or partially resident textures: none in wgpu 28 or 30, no open PR, and the WebGPU issue is still an investigation [V].

Mipmaps on the GPU: no built-in; the wgpu repository's mipmap example draws a fullscreen triangle per level with a linear sampler, which needs no compute and works under the current limits; Bevy's path is a compute single-pass downsampler, which does not [V]. Offline mips are the answer for BC7 anyway.

Sampling: `textureSampleGrad` and `textureSampleLevel` are both allowed in non-uniform control flow; only implicit-derivative sampling demands uniformity (naga's analyzer) [V]. With explicit gradients the Vulkan spec derives the anisotropic footprint from the given gradients, so anisotropy applies to `textureSampleGrad` and not to `textureSampleLevel` [V]. A tiled sampler computes the UV and the derivatives of the global coordinate once at the top of the fragment and passes rescaled gradients everywhere.

## 7. Formats on disk and on the GPU

Measured with jxl-oxide 0.12.6 in a release build on this machine, scripts `tile.py`, `bc7.py` and the `bench/` crate [M]:

| Per tile | Size on disk | Decode, one thread | Tiles per 50 ms tick, one thread | Tiles per tick, four workers |
|---|---|---|---|---|
| JPEG XL q85, 256^2 RGB | 5.6 KB | 10.8 ms | 4.6 | 13.6 |
| JPEG XL q85, 512^2 RGB | 15.5 KB | 18.9 ms | 2.6 | 9.4 |
| PNG, 256^2 | 54 KB | 0.33 ms | about 150 | not CPU bound |
| PNG, 512^2 | 178 KB | 1.35 ms | about 37 | not CPU bound |
| BC7 + zstd level 19, 256^2 | 29 KB | about 0.13 ms (proxy) | about 380 | not CPU bound |
| RGBA8 + zstd level 19, 256^2 | 67 KB | 0.37 ms | about 135 | not CPU bound |

Rendering one JPEG XL image costs a fixed 8.5 ms whatever its size (8.7 ms at 64^2, 10.3 ms at 256^2, 14.6 ms at 512^2, the same at effort 3, 7 and 9), so per texel a 512 px tile is as efficient as the full 8K decode and a 256 px tile costs 2.3 times as much [M]. Rayon inside one tile does not help; parallelism across tiles scales to 3.7 ms per 256 px tile on four workers and 1.9 ms on eight [M].

jxl-oxide can decode a region of a larger image (`JxlImage::set_image_region`, since 0.4) and the 0.12.6 source skips the per-group inverse DCT for groups outside the region, so the cost scales with the region area plus the fixed global and LF work, but the whole codestream must be in memory and the saving was not benchmarked [V]. A 1/8 low-frequency decode exists only as an unmerged PR. The `image` integration hook arrived in 0.12.5. jxl-rs (the pure-Rust decoder used by Chrome and Firefox, 0.7.4 as of 2026-09-17) claims libjxl parity but has no region decode yet [V].

JPEG XL is the right shipping format by size: the 8K-equivalent day land tiles of one month are 1.43 MB at q85 against 7.43 MB as BC7 plus zstd [M]. It is the wrong runtime format: 10 ms per tile against 0.13 ms, and RGBA8 on the GPU. BC7 is the right runtime format: four times smaller in VRAM, a near-free load, and accepted everywhere. The obstacle is the encoder. No pure-Rust BC7 encoder exists on crates.io [V]. The options:

| Encoder | Nature | Speed | Notes |
|---|---|---|---|
| `intel_tex_2` (Traverse Research) | FFI to prebuilt ISPC kernels, no ISPC at build, aarch64 macOS and Linux libraries shipped | BC7 "fast" 8.7 MP/s, "very fast" 16 MP/s, "ultra fast" 170 MP/s, BC1 640 MP/s, one thread on an M5 Max, from the PR merged 2026-09-28 on git main; crates.io still at 0.5.0 | a 25 M texel month is about 3 s single-threaded at "fast" [R] |
| `ctt` and `ctt-bc7enc-rdo` (a wgpu maintainer) | wraps bc7e with rate-distortion optimization, Intel ISPC, etcpak, astcenc; writes KTX2 or DDS with zstd | depends on backend | published 2026-09-27; RDO is what makes BC7 compress well under zstd |
| `block_compression` 0.8 | pure Rust plus WGSL compute shaders on wgpu 28 | not published | needs compute limits, which the current device request sets to 0; tested on DX12, Metal and Vulkan |
| `texpresso` | pure Rust | not published | BC1 to BC5 only, no BC7 |

zstd on BC7 game textures reaches 1.76:1 without RDO and 3.65:1 with it (cbloom, 2020); satellite imagery is noisier and will do worse [V][A]. For decoding zstd at runtime, `ruzstd` is pure Rust and 1.4 to 3.5 times slower than the C library, which is immaterial at these sizes; `zstd` is FFI. The `unsafe_code = "deny"` lint applies to the workspace's own code, not to dependencies, so FFI crates are a preference question rather than a lint one [V].

Three routes follow, and the plan has to pick one:

1. Bake BC7 offline (xtask or the Python pipeline through the `ctt` CLI), ship BC7 plus zstd packs. Runtime is pure and trivial. The archive grows by roughly 10 MB per month without RDO, about 130 MB for twelve months and night [R]; RDO could roughly halve that [A].
2. Ship JPEG XL tiles (about 2.0 MB per month with all levels, 26 MB for twelve months and night against 4.0 MB today; section 17 measures the archive at 32.6 MB against today's 17.8 MB [M][R]) and transcode to BC7 on first use into the cache directory, the way id Tech 5 transcoded pages. That needs an encoder in the app: `intel_tex_2` (FFI) or `block_compression` on the GPU (raise the compute limits; unmeasured on the software adapters).
3. Ship JPEG XL tiles and use RGBA8 on the GPU. No encoder, no BC7, the smallest change, and four times the VRAM of the other two. It is where a plan can start and it degrades to nothing on adapters without BC.

Route 3 with the cube already cuts the day map from 171 to 128 MiB; route 1 or 2 takes it to 32 MiB. Route 2 is the one that keeps the archive small and the VRAM low, at the price of the encoder dependency and a one-time transcode of a few seconds per month.

The mip chain: the current box filter, applied at full precision before each level is encoded separately, does not compound BC7 error across levels [R]. Each tile carries its own mips (two levels for a 256 px tile plus gutter in stage two, the full chain in the cube faces of stage one).

## 8. Residency: what stays on the GPU

The level a pixel needs. With texel arc a at level L, focal length f = H / (2 tan(fov / 2)) in pixels, and the nearest distance to the tile, the projected texel size is a f / dist; refine while it exceeds one pixel. This is Cesium's screen-space error rule, whose globe default is 2 px [V]. At the default 20 degree field of view and 2160 lines, the 8K level projects to one pixel at 5.7 radii, the 4K level at 10.4, 2K at 19.8 and 1K at 38.6 [R]. So a whole-disk wallpaper on a 4K screen needs the 8K level and no more, a 1080p one needs 4096, and finer levels than 8192 only ever matter inside the near half of the zoom slider, where the frame holds a small patch: at 1.5 radii the 16:9 frame spans about 18.5 by 10.2 degrees of arc, about 20 tiles of 256 px at a 16K-equivalent level [R].

A per-pixel simulation of that rule (3840 x 2160, rays at stride 2, the 16x anisotropic LOD rule, 256 px tiles with an 8 px gutter and two mips per layer, a resident 512-per-face base cube for everything coarser, ocean tiles skipped, the view over Europe and Africa) gives the working set of a two-level tiled scheme with 8192 as its finest level [M]:

| Camera distance | Finest-level tiles in view | Next-level tiles | Resident layers | RGBA8 MiB | BC7 MiB |
|---|---|---|---|---|---|
| 1.5 | 6 | 0 | 6 | 10.1 | 2.5 |
| 2 | 16 | 0 | 16 | 13.6 | 3.4 |
| 3 | 65 | 5 | 60 | 29.2 | 7.3 |
| 4 | 102 | 17 | 103 | 44.3 | 11.1 |
| 5.76 (the disk fills the frame) | 132 | 31 | 142 | 58.1 | 14.5 |
| 11.5 | 0 | 44 | 44 | 23.5 | 5.9 |
| 20 | 0 | 2 | 2 | 8.7 | 2.2 |
| 40 and 80 | 0 | 0 | 0 | 8.0 | 2.0 |
| worst over fields of view 10 to 30 degrees | 144 | 30 | 153 | 62.0 | 15.5 |

The totals include the base cube (8 MiB RGBA8, 2 MiB BC7). Pacific and polar views are within 10% of these. An equirectangular pyramid under the same rule needs up to 268 tiles at the pole against 184 for the cube [M]. The worst case is about 1.2 to 1.4 times frame pixels over tile area, so adding a 16K or 32K level costs almost nothing more in residency; it costs disk and bake time.

Tile size, worst case [M]:

| Tile | Worst resident layers | RGBA8 MiB | BC7 MiB | Pure-ocean tiles at the finest level | Page-table entries for two levels | Gutter overhead |
|---|---|---|---|---|---|---|
| 128 | 435 | 51.0 | 12.8 | 40.8% | 1920 | 1.27 |
| 256 | 153 | 62.0 | 15.5 | 24.5% | 480 | 1.13 |
| 512 | 57 | 83.8 | 20.9 | 5.2% | 120 | 1.06 |

256 is the balance: it fits a 256-layer array for one set (two sets need the limit raised, which every adapter allows), its page table fits a uniform buffer, and its JPEG XL decode is tolerable. 512 halves the per-texel decode cost but keeps far more ocean.

Three GPU-side structures were compared:

- A texture array as the tile cache with a page table: each layer is one tile with its own two-level mip chain, so nothing bleeds between tiles; the page table is a uniform `array<vec4<u32>>` resolved on the CPU so that every finest-level cell already names the layer and level of the best resident ancestor, with a flag for constant ocean; one lookup in the shader, no loop. This is the recommendation for stage two.
- A single atlas: no layer limit, but hardware mips mix neighbors once the gutter is smaller than the footprint, so only mip 0 is usable and trilinear across two page levels doubles the lookups. Rejected.
- Clipmap rings: on a cube the window spans up to three faces and the worst view wants 36% of all finest-level tiles, which no ring covers. Rejected.

The "middle" design that was asked about, a resident 2048 map plus 8K detail tiles blended by level of detail, is not materially simpler than the two-level scheme (it drops the level choice, about ten lines, and keeps the gutter bake, the visible set, the loader and the residency bookkeeping) and saves nothing over it: at 11.5 radii it needs 123 finest-level tiles where the two-level scheme needs 44 coarser ones [M]. The simple design that does pay is the one without tiles at all: whole EAC cubes at 2048 faces, BC7, hardware seamless filtering, 32 MiB per month. That is stage one.

Gutters and sampling in stage two: a layer holds a 256 px tile with an 8 px gutter on each side (272^2) and two mips; the anisotropic footprint at mip 0 reaches below maxAniso + 1 texels and half that at mip 1, so a gutter of maxAniso + 2 is enough. With the tile sampler's anisotropy clamped to 8 and the gutter at 8, the rare one-texel overshoot reads the clamped gutter edge, which holds true neighbor data; the base cube keeps 16x with hardware seams [R]. Face-coordinate gradients come from the warped direction's screen derivatives projected onto the face basis, exact across edges [R]. On the software adapters the extra cost per fragment is one dynamic uniform load, three `atan` and about 40 ALU operations against three or four anisotropic samples; the samples are expected to dominate, unmeasured [A].

Constant ocean tiles are a bit in the page-table entry from a static, month-invariant bitset; a 1 x 1 layer would waste a layer and falling back to the coarser level would blur the coastal neighbors. The flag is lossless only if the bake writes the fill exactly and classifies with the gutter included [R]. The water mask stays in the day tiles' alpha: BC7 is 8 bits per texel with or without alpha, one fetch reads both, and the mask is the same for every month so a blend leaves it intact [V][A]. It would move to a separate BC4 cube (4 MiB at 1024 faces) only if the day went to BC1, which has no alpha and bands on smooth imagery [A].

The night map is not seasonal and fits the same shader with one more cube binding. Its ocean is not black (maximum channel 14 to 16 over the ocean), so the bake must flatten it; then 108 of its 384 finest-level tiles are constant, nearly the same set as the day's, and in stage two it can share the day's page-table indexing through a parallel array [M][R].

## 9. Loading, prefetch, eviction and the artifact guarantees

Stage one loads whole faces; stage two loads tiles. The loader below is written for stage two and degrades to stage one with faces as the unit.

The wanted set is computed on the CPU from the camera, not from a GPU feedback pass. For each face, descend the quadtree from the root; each tile carries a precomputed center direction n and angular radius r (the corners are the extreme points, since cube tile edges are great-circle arcs). A tile survives the horizon when angle(n, camera) < acos(1/d) + r, the same test Cesium's horizon culling makes against the tangent cone [V]; it survives the frustum when its cap meets the four side planes widened by the prefetch margin; it refines while the projected texel size exceeds the threshold. The descent visits a few thousand nodes for a few hundred leaves, under a millisecond [R]. The union over every output of the display plan is the set. A feedback buffer would be a frame late (id Tech 5 accepts a frame of latency from an 80 x 60 readback), cannot see off-screen tiles for prefetch, would need `map_async` and a poll in an engine that renders on demand, and would add a pass on the software adapters; a sphere whose only occluder is its own horizon has an analytic answer [V][R].

Prefetch is one ring of tiles around the visible set at each level plus a lead along the drag direction of velocity times latency. With four workers a pending tile costs 3.7 ms, so a queue of 40 is 150 ms; a fast drag of 5 degrees per 50 ms tick then wants a 15 degree lead, three 16K-equivalent tiles or one to two 8K ones [R]. Supply keeps up: a 5 degree step at 1.5 radii brings in one column of two or three leaves per tick and four workers decode 13.6 [M]. Priority is the pinned floor first, then visible tiles by level deficit (largest first, as id Tech 5 does) and center outward, then the margin and the predicted tiles; ancestors above the floor are in the set so coarse arrives before fine, which is Cesium's `preloadAncestors` [V]. Whole-level residency beats any margin whenever a level's land tiles fit the budget, which is always the case at 8192 and stops at 16K-equivalent, where 793 land tiles would be 208 MiB in RGBA8 and only 20 to 60 are ever visible at the zoom that needs them [M][R].

Workers: min(4, available parallelism minus 2), each decoding one tile single-threaded (`JxlThreadPool::none()` exists) [V][M]. There is no request queue. The engine publishes the wanted set, ordered, with a generation, into a latest-value slot; workers claim the next unclaimed key under the slot's mutex and park on a condvar when nothing is left. Cancellation is implicit: a key missing from the new set is never claimed, and a decode already running finishes in 10 to 20 ms. Results go back through a bounded channel of twice the worker count; a full channel blocks the worker, which is the backpressure; a decoded buffer lives only in that channel until the next tick uploads or drops it. Each result carries its key (pack, level, face, x, y) and the device epoch, and the engine drops a result whose key left the set or whose epoch is old. That satisfies the project's queue rules: one latest-value slot, one bounded channel, no parked pixels, a producer that always runs and names its consumer. Uploads are budgeted in bytes per tick (16 MiB is 64 tiles of 256 px RGBA8), then the page table is rewritten whole on the CPU (about 8 KB for two levels) and uploaded with one `write_buffer`, then the frame is rendered, all in one submit, so the writes execute before the render and a reused layer can never be sampled with a stale mapping [V][R]. Other systems bound the same things: Cesium Native's `maximumSimultaneousTileLoads` is 20, Unreal's `r.VT.MaxUploadsPerFrame` is 8 in game, id Tech 5 transcodes 8 to 16 pages per frame, Celestia drains its ready queue under a byte budget [V].

Eviction: only a tile outside the wanted set may go, the one wanted longest ago first; the floor is pinned and never counted against the streamed budget (id Tech 5 pins the coarsest pages too, Celestia keeps a coarser tile while a descendant is resident) [V]. Budget: the floor plus twice the peak visible-plus-margin leaf count, around 368 + 256 layers of 256 px for a 16K-equivalent finest level, about 160 MiB in RGBA8 or 40 MiB in BC7 [R]. Hysteresis: load at 1.0 px and one ring, keep until 0.7 px or two rings, evict only under pressure and only after two seconds out of the set [A].

The guarantees:

- No hole, ever: the floor is pinned, so every cell has a resident ancestor and the page table names it. At 8192 the floor is today's quality, so a coarse tile can only show while zoomed in past the full disk, and even then no worse than today.
- No half-uploaded tile: writes, page table and render share one submit, and the page table is replaced whole, so it is atomic without double buffering.
- No pop on the desktop: a wallpaper is rendered only when the set is complete. The predicate is that for every output of the display plan, every tile the level rule wants at threshold 1 px is resident, in a pack within one blend quantum of the target, plus the night set. While an export is pending its set joins the wanted set at top priority. After a timeout (5 s is generous: 100 missing tiles take 0.4 s on four workers) the export proceeds with fallbacks, logs it, and a re-export is scheduled on completion rather than at the next interval [R][A]. This generalizes today's hold-back in `publish_wallpaper`, and `textures_ready` and `textures_pending` become "the wanted set is complete" and "something wanted is on its way".
- A pop in the preview only when zoomed in past the full disk, from 8K detail to finer. A 200 ms cross-fade would need an arrival time per entry, a second sample, and four to six extra preview renders in an engine that renders on demand; skip it unless the pop turns out to be visible in practice [R].
- Convergence after a drag: while dragging, use a 2 px threshold to halve demand; when it stops, the threshold returns to 1 px, at most about 100 leaves are missing, and they drain in about 0.4 s, re-rendering the preview once per tick in which a visible tile landed [R].

Failure modes: a missing or corrupt tile (crc mismatch, decode error, or a worker panic caught with `catch_unwind`) is marked failed for its pack, never requested again, counted as satisfied by the predicate, drawn from its ancestor, and logged once per pack; a pack whose floor fails is invalid and the previous month is used, then the grid. A cache directory that is not writable skips downloads and keeps the shipped levels. A decode that never returns is marked failed after 2 s and a replacement worker started, up to a limit, since the stuck thread cannot be killed. Device loss goes through `Device::set_device_lost_callback`: recreate, clear residency, bump the epoch so in-flight results drop, reload the floor first, block exports until it is complete [V][A].

## 10. Seasons: the transition between months

What others do [V]: OpenSpace's temporal tile provider hard-switches by default and can blend the two adjacent slices in a shader with `interpolation = true`; its bundled seasonal Blue Marble asset does not enable it. CesiumJS's time-dynamic imagery switches at the interval boundary and preloads the next interval. Web WorldWind's BMNG layer switches to the nearest of twelve monthly sub-layers anchored at the first of each month, so the switch lands mid-month. xplanet needs a wrapper script to swap the map by month. Celestia, Stellarium and Space Engine have nothing built in. NASA's own SVS animation of the set fades from month to month. No paper or manual describes blending by day of month.

The BMNG paper undercuts a linear blend for snow: snow is a hard replacement per month, so a mid-month blend shows the old and the new snow line at half opacity each, a translucent second edge; vegetation is a smooth Fourier fit and blends naturally; the ocean and Antarctica do not change [V]. Which of a ghosted edge for weeks and a jump once a month looks better on a wallpaper is a question for the eye, not for this document, and the answer may differ by region. Blending in linear light, dithering the transition per texel so an edge moves rather than fades, and treating bright low-saturation pixels (snow) as a threshold switch rather than a mix are the remedies to try [A].

The weight itself: place each image at the center of its month, t = month + (day - 1 + fraction) / days_in_month - 0.5, blend floor(t) and ceil(t) by frac(t), wrapping December to January, with each month's own length so the weight does not depend on the year [R]. The weight moves about 1/30 per day, 1/8640 per five-minute wallpaper [R]. It is derived from `SceneParams.datetime` like the sun direction and should be compared outside the digest the same way, quantized. The custom-date slider lets the user scrub the whole year, so a month pair can change at any rate; with the loader above that is just a new pack key in the wanted set.

Where the blend runs depends on the format:

| Option | VRAM at 8192 (with mips) | Decode and upload | Shader | What the user sees |
|---|---|---|---|---|
| Two months resident, blend in the shader | BC7: 2 x 32 = 64 MiB for whole cubes, or 2 x 13.5 + base for stage two; RGBA8: 2 x 128 MiB | one month set per month | two samples and a mix, or two page tables over one cache | continuous, and any blend function (mix, dither, threshold) is a shader edit |
| Blend on the CPU in the worker before upload, weight quantized to 1/32 of a month, re-upload on each step | one set: 129 MiB RGBA8 | two decodes per tile per step, 736 tiles or 2.7 s on four workers about every 0.94 days [R] | unchanged | steps of 1/32 of the month difference, about one 8-bit level, invisible [A] |
| Hard switch mid-month, one-time cross-fade in the preview | one set, two during the fade | one month set per month | fade in the preview only | one wallpaper a month jumps a whole month, visible in the snow months |

The CPU blend is cheap only in RGBA8; on BC7 it would mean decode, blend and re-encode of 25 M texels per step, seconds of encoding, and is rejected there [A]. So route 3 of section 7 pairs with the CPU blend, routes 1 and 2 with the shader blend. The shader blend is the more flexible of the two, since the ghosting question is answered by changing one line, and its cost at stage one is 32 MiB of BC7 and one extra sample. When a month's weight reaches zero it is swapped for the next, so there is never a third month resident [R].

## 11. Distribution and the bake

On disk, one packed file per month plus one for night: a header (magic, version, tile size, levels, flags), an index sorted by level, face, row and column with offset, length, crc32 and the land-or-ocean flag per entry, then the blobs coarse levels first so the floor is one contiguous read. Reads go through `std::os::windows::fs::FileExt::seek_read` and `std::os::unix::fs::FileExt::read_at`, safe positional reads on one shared handle (the Windows one moves the cursor and has no exact variant, so a 15-line wrapper does the loop); `memmap2` needs `unsafe` because a file truncated under a mapping is undefined behavior [V]. A directory of about 5,000 small files for thirteen packs means more opens and more per-file antivirus work on Windows for no benefit [A].

Sizes at the 8K-equivalent finest level, JPEG XL q85 [M][R]: day land tiles 1.43 MB (256 px) or 1.30 MB (512 px), night 1.15 MB, about 2.0 MB per month with the coarser levels; twelve months and night about 26 MB against 4.0 MB today. The estimate in this paragraph predates section 17, which measured whole faces at 1.43 MB per month and the archive at 32.6 MB against today's 17.8 MB. A 16K-equivalent level would be roughly 6.5 MB per month, 78 MB for twelve, an upper bound since denser imagery usually compresses better. The alpha water mask adds 5 to 10% unless it is derived from the ocean flag and a separate low-resolution mask [A]. The BC7 route is about 10 MB per month before RDO.

Recommendation on what ships: all twelve months at 8192 and below in the archive, because offline-first is what a wallpaper app owes its user and the cost is about 15 MB (section 17); the levels finer than 8192 as per-month downloads from the GitHub release assets through the existing HTTP cache, verified by the index crcs, so an offline user has today's quality in every season and an online one gets the detail [R][A].

Bake: the pipeline already has Pillow with the JXL plugin, numpy, scipy, rasterio and geopandas, so the reprojection, the ocean flattening by the existing shapefile mask, the pyramid and the tile encode are additions to `texture-pipeline earth`, not a new tool. Decoding a 21600 x 10800 JPEG, area-resampling to the faces, building the pyramid and encoding about 23 M texels took about 40 s per month in a rough accounting (libjxl through ImageMagick ran at 2.3 MP/s including one process per tile), about 8 minutes for twelve months sequentially, four times that for a 16K level [M][R]. `textures/PROVENANCE.md` gets a section per month and the pack format a version.

## 12. Fit with the current architecture

What the tree's rules and tests imply for a plan, from the reconnaissance in section 2:

- `SlotLayout` and `TextureMode` survive; a "slot" becomes a set (two month cubes, or a tile cache plus page tables) but the modes Grid, Day, Night and Blend keep their meaning, and the cloud, Moon and Milky Way slots are untouched.
- The month weight and the pack selection are derived from `datetime`, which is outside the digest by design; the sun direction's separate comparison is the template.
- The resolution setting keeps its meaning as a cap on the finest level loaded; the halving cache becomes unnecessary once levels are baked, which simplifies `texture_cache`.
- `textures_ready` and `textures_pending` are redefined over the wanted set (section 9); `publish_wallpaper`'s hold-back and `TexturesReady` keep working through them.
- The mailbox rule (one slot per texture, latest value) becomes the wanted-set slot plus a bounded result channel; the reasoning belongs at the declaration as the rule demands.
- Every golden image of the globe changes with the reprojection; regenerate per adapter, and add a golden that pins the cube's orientation (Africa on +Z, north up), since a face permutation is the classic bake bug.
- The memory budget in `memory.rs` and the engine test that measures 1259.7 MiB against 616.5 MiB get new expectations; with BC7 the resident term shrinks by about four times.
- CI has no LFS textures and renders the grid; the tile packs are LFS like the maps, so CI still never decodes a real tile. The generated fixtures the engine tests use today must gain a tiny pack writer, which is small.
- The device request has to raise `max_texture_array_layers` from the adapter (stage two), request `TEXTURE_COMPRESSION_BC` when offered, and, if the GPU encoder route is taken, move from the webgl2 limits to `downlevel_defaults` for compute.

## 13. Prior art in one table

| System | Surface layout | Residency | Months | Source |
|---|---|---|---|---|
| Web WorldWind BMNG | geographic tiles, 45 degree level 0, 5 levels, 256 px, 10,912 tiles per month | on-demand tiles | nearest of twelve, switch mid-month | `BMNGRestLayer.js` [V] |
| Java WorldWind BMNG | 36 degree level 0, 512 px, 5 levels, about 490 m per pixel | on-demand tiles | one month per layer | `BMNGWMSLayer.xml` [V] |
| CesiumJS | geographic or web-mercator quadtree, screen-space error 2 px, horizon culling against the tangent cone, `preloadAncestors` | bounded loads, 20 at once | time-dynamic imagery hard-switches and preloads the next interval | `TimeDynamicImagery.js`, Cesium blog on horizon culling [V] |
| OpenSpace | globe browsing tiles | streamed | optional shader blend between adjacent slices | `temporaltileprovider.cpp` [V] |
| id Tech 5 | sparse virtual texture, 128 px pages, feedback buffer 80 x 60, coarsest pages pinned, LRU on leaves | 8 to 16 pages transcoded per frame | not applicable | van Waveren, "Software Virtual Textures" [V] |
| Celestia | virtual textures, tile quadtree on equirectangular | async loads drained under a byte budget, grace frames before eviction | none built in | `virtualtex.cpp` [V] |
| bevy_terrain | chunked clipmap quadtree, duplicated tile borders, GPU atlas | request-driven, panics when full, no priority | not applicable | `docs/implementation.md` [V] |
| Google VR, Zucker and Higashi | equi-angular cube (pi/4 tangent warp) | not applicable | not applicable | JCGT 7(2) 2018 [V] |

No reusable Rust crate for tile streaming over wgpu exists; `wgpu-virtual-texturing` is a 2023 TODO list and `rend3` is archived [V].

## 14. Recommendation and what to settle by prototype

Section 15 records the decisions taken after this was first read and the encoder measurements that settle prototype 3 below; where they differ from this section, section 15 wins.

Recommended shape:

1. Stage one. Equi-angular cube at 2048 per face as a `texture_cube` with offline box mips, for the day and the night; twelve monthly day cubes baked from the 21600 sources with the ocean flattened exactly by the mask; two months resident and blended in the shader; BC7 where the adapter offers it (that is every target adapter) with RGBA8 as the fallback. VRAM for day, night and the second month: 96 MiB in BC7 against 342 MiB for day and night today; 384 MiB in the RGBA8 fallback, which is why BC7 is part of stage one and not an optimization [R].
2. Stage two. 256 px tiles with an 8 px gutter and two mips in a texture array, a CPU-resolved uniform page table per month set, a pinned 512-per-face base cube, the wanted set computed from the camera, four decode workers behind a latest-value set and a bounded result channel, a byte-budgeted upload per tick, and per-month downloads of the levels finer than 8192. Worst case about 15.5 MiB of BC7 per set for a 4K frame, whatever the finest level [M].

Before a plan commits, six things should be tried, each an afternoon:

1. The look of the month transition. Crop Scandinavia in November and December and Canada in March and April from the 5400 base set and compare a linear blend at 50%, a dithered dissolve, a snow-threshold switch and a hard switch, side by side. This decides the shader's blend line and whether the CPU-blend route is even in contention.
2. BC7 on the software adapters and on GitHub's macOS runner: encode one face, sample it in a shader test, compare with the RGBA8 original under the golden tolerance. This decides whether BC7 can be the format the golden references are generated with.
3. The encoder route: time `intel_tex_2` from git on one 2048 face at "fast", and `block_compression` 0.8 on the GPU on this machine and on lavapipe; measure the zstd ratio with and without RDO on real tiles. This decides between routes 1 and 2 of section 7 and sizes the archive.
4. The EAC shader on `warp` and lavapipe: the three `atan` and the gradient math against the current `textureSample`, at 1080p and 4K, with the Milky Way's measurement (0.2 ms on the GPU, 133 ms on `warp`) as the yardstick.
5. jxl-oxide region decode: whether decoding four 256 px tiles from one 512 px image amortizes the 8.5 ms fixed cost, which decides between 256 and 512 px tiles on disk if JPEG XL is the runtime format anywhere.
6. Cube orientation: search rotations about the polar axis for the one that puts the most corner area over ocean, since the corners are where the resolution is lowest.

Open questions the research did not close:

- Whether the GL backend can ever be selected on a real user's machine, and therefore whether the non-seamless cube filtering there matters. A gate that refuses GL when a cube is in use, or a gutter-based array for the base as well, are the fallbacks.
- Whether the wgpu 28 `Queue::write_texture` staging pressure at 64 tiles per tick is acceptable on the software adapters, or whether a persistent staging buffer with `copy_buffer_to_texture` is needed from the start.
- The 16K-equivalent level's real size after encoding, which the estimate in section 11 bounds from above.
- The memory report's expected table and the soak test's growth limit under a cache that legitimately changes size with the camera.

## 15. Follow-up: decisions and the BC7 encoder measurements

Recorded later on 2026-09-29 after the first read of this document. Three decisions were made, and the encoder question in section 7 was measured rather than surveyed. Where this section and section 14 disagree, this section wins.

Decisions:

1. Tiles, the visible-only cache and the ocean skipping are in from the start, not deferred to a second stage, and the tiles are smaller than the 256 px section 8 settled on. The JPEG XL fixed cost per image (8.5 ms, section 7) does not constrain that, because the shipped image and the cached tile are decoupled: the archive carries JPEG XL as whole cube faces or large chunks, decoded once, and the local cache holds BC7 tiles at whatever size the residency design wants. 128 px tiles make 41% to 46% of the finest-level tiles pure ocean (section 4) at the cost of four times the page-table entries and a 1.27 gutter overhead (section 8).
2. Months are a hard cut, with no blend. The next month's tiles for the current view are loaded ahead of the boundary, the switch is one page-table swap, and the export predicate of section 9 makes the first wallpaper after the boundary the first to show the new month. Two months are resident only for the minutes around the boundary, and the month-transition table in section 10 is moot.
3. Textures ship as JPEG XL, in whatever image size decodes best, and BC7 is produced on the user's machine once into a disk cache. That is route 2 of section 7, which is why the encoder had to be measured.

Rotation without lag follows from decision 3. Loading a tile from the BC7 cache is a positional read of a few kilobytes and one upload, about 0.1 ms per tile (section 7), against 10 ms from JPEG XL. A drag of 5 degrees per tick wants two or three new tiles per tick at the near end of the zoom and the loader supplies hundreds. The pinned low-resolution floor (a 512-per-face cube, 2 MiB in BC7) guarantees that a late tile shows as a briefly softer patch in the preview and never as a hole, and the desktop is written only when the wanted set is complete. The one-time transcode of a month is the only slow path, and it runs in the background behind the floor.

The encoders, measured on two 2048 x 2048 crops of the shipped maps (Europe and Africa, day with the water mask in alpha, night opaque), release builds, one thread unless stated, PSNR against the source through the pure-Rust `bcdec_rs` decoder; scripts under the session's `bc7bench` and `purebc7` directories [M]:

| Encoder | Nature | Setting | MP/s day / night | PSNR RGB day / night | Alpha day |
|---|---|---|---|---|---|
| `dds` 0.2.0 (image-rs) | pure Rust, `forbid(unsafe_code)`, MIT or Apache, rayon optional | Fast | 2.8 / 2.3 | 46.3 / 47.5 | 49.8, flat mask values exact |
| `dds` 0.2.0 | | Normal | 0.9 / 0.9 | 47.6 / 48.4 | 49.6 |
| `dds` 0.2.0 | | High | 0.35 / 0.35 | 47.9 / 49.2 | 50.0 |
| `block_compression` 0.8.0 CPU path | pure safe Rust, a scalar port of Intel's ISPC kernel | ultra_fast | 2.9 / 15.8 | 46.5 / 46.7 | 47.5 |
| `block_compression` 0.8.0 CPU path | | basic | 0.17 / 0.25 | 47.9 / 48.9 | 47.8 |
| `block_compression` 0.8.0 GPU path | WGSL compute | basic, Vulkan on the RX 6800 XT | 94 / 201 | 48.0 / 48.9 | 47.8 |
| `block_compression` 0.8.0 GPU path | | basic, WARP with DXC | 0.65 / 1.5 | 47.9 / 48.9 | 47.8 |
| `intel_tex_2` 0.5.0 (crates.io) | ISPC static libraries through FFI, MIT or Apache | very_fast | 3.0 / 4.9 | 47.6 / 47.8 | 47.7 |
| `intel_tex_2` `arm64-perf` branch (unreleased) | | basic | 2.8 / 5.4 | 48.0 / 49.2 | 47.8 |
| `intel_tex_2` `arm64-perf` branch | | ultra_fast | 36.6 / 122 | 46.5 / 46.7 | 47.5 |
| own mode-6 prototype, 400 lines | pure Rust, no dependencies | 1 refinement round, native CPU | 6.6 | 45.3 / 46.9 | 41.6 (45.8 with a 40-line mode 5) |
| `texpresso` 2.0.2, BC3 | pure Rust, MIT | ClusterFit | 1.4 / 2.6 | 39.9 / 37.5 | 52.7, mask exact |
| `texpresso` 2.0.2, BC1 | | RangeFit | 35 / 37 | 37.8 / 34.7 | none |

For scale, the JPEG XL encode at quality 85 the app already accepts costs 41.2 dB on the day crop and 42.0 on the night crop [M]. BC7 on top of that costs a further 0.6 dB; BC1 or BC3 on top costs 3.4 to 5.5 dB, and on a synthetic ocean-like gradient BC3 leaves 10% to 16% of the samples off by more than 2 levels where BC7 mode 6 leaves none [M]. So the alternatives to BC7 that have a pure-Rust encoder, BC1 and BC3, are measurably worse on exactly the content this app draws, and the question of alternatives dissolves because BC7 itself has one: `dds`, which the survey in section 7 missed.

What the measurements say about each route:

- `dds` is the recommendation: safe Rust, maintained under image-rs, all eight BC7 modes, BC4 and BC5 too, and the best quality of the pure-Rust options at every preset, with exact handling of the flat mask values. At Fast it is 11 to 13 s per month of 8K-equivalent land tiles on one core, 3 to 4 s on four with its rayon feature, so a whole year is about a minute in the background; Normal is three times slower for another dB. Its documentation says a preset's output may change between versions, so the cache key must include the crate version and the preset beside the source stamp [V][R].
- The GPU compute route is not worth its problems for a one-time transcode: creating the BC7 pipeline took 100 s cold on the AMD Vulkan driver and 32 s on DX12, wgpu 28's default DX12 shader compiler (FXC) fails on the shader outright after six minutes with an unrollable-loop error, so WARP runs it only with a DXC DLL the app would have to ship, and WARP is no faster than the CPU encoders anyway [M].
- `intel_tex_2` works everywhere the app ships: prebuilt libraries for x86_64 and aarch64 on Windows, macOS and Linux, a clean link with `+crt-static`, and a safe public API. But the fast kernels live on an unreleased branch, the 0.5.0 release leaves a missing length check behind a safe function, and it is FFI; nothing it offers is needed once `dds` exists [M][V].
- The mode-6 prototype shows what a dependency-free encoder costs: about 400 lines for 1 to 2 dB less than `dds` Fast and two to three times its speed. It is the fallback if `dds` proves unmaintained [M].

The water mask: every BC7 encoder in its alpha presets shares endpoints and partitions between color and alpha, and on the coastline that costs it. Across the ISPC-derived encoders 0.2% of the flat mask pixels land more than 2 levels off, the maximum alpha error is 44, and about 2,000 pixels of the 4.2 M flip their land-or-water call at a threshold of 192; `dds` keeps the flat values exact; BC3's separate alpha block keeps them exact with a maximum error of 15 [M]. The clean answer is a separate mask texture: the mask is the same for all twelve months, so it is encoded once, as BC4 (one channel, 4 bits per texel, the same block type as BC3's alpha, which `dds` encodes) at the day tiles' resolution or lower, and the day tiles become opaque BC7, which the opaque presets encode about twice as fast. This revises section 8's "keep the mask in alpha".

What the plan takes from this: JPEG XL faces in the archive at today's quality 85; a background transcoder over `dds` Fast into a per-month pack of BC7 tiles plus one BC4 mask pack, keyed by the source stamp, the crate version and the preset; the current month first, the rest of the year afterwards while idle; RGBA8 tiles decoded straight from JPEG XL as the path for an adapter without BC, which the probe suggests does not exist among the targets.

Sample tiles for visual inspection were generated from every CPU encoder path (four 256 px tiles: day land, day coast, night city, night coast; each at 1x and 4x with amplified difference images) and looked at. The differences between the BC7 encoders are hard to see at all, and the JPEG XL step the app already takes causes most of the measured loss: the JPEG XL plus `dds` results sit 0.3 to 1.5 dB below JPEG XL alone on every tile [M]. On that basis the decision is `dds` at the Fast preset, on the grounds of simplicity as much as speed: one safe pure-Rust dependency, one call per tile, threading through its rayon feature, and exact constant alpha. Nothing measured is both simpler and faster: the ISPC encoder is 5 to 30 times faster but FFI with 10 to 20 MB of static libraries per target and its fast kernels unreleased; the `block_compression` CPU encoder is no faster on day tiles and ties the app's wgpu version to its own; BC1 and BC3 are ten times faster and the one option whose loss is visible; the mode-6 prototype is twice as fast and 400 lines to own. Per month of day tiles at today's density with mips, `dds` Fast is about 9 s on one core and 2.5 s on four, the JPEG XL decode of the month's faces about 2 s on one core, and the whole year with the night set about two minutes on one core or 35 s on four [R from the measured rates].

## 16. Tile layout, counts and the poles

The layout the plan should take, with the decisions of section 15 applied: an equi-angular cube (section 5), 128 px tiles (decision 1), a plain `texture_cube` as the pinned floor, and BC7 tiles above it.

Levels and counts. The face width at each level, its equirectangular equivalent (four times the face width, which is what the resolution setting's labels can keep saying), and what the 128 px tiling of it holds [R from the section 4 occupancy figures; the land counts at the two finer levels are interpolated from the 256 px measurements at the next coarser density, since a 128 px tile at one level covers the same ground as a 256 px tile at the next]:

| Face width | Equivalent width | Tiles per face | Tiles in total | Land tiles, about | BC7 with gutter and mips, all land tiles |
|---|---|---|---|---|---|
| 512, the floor | 2048 | not tiled: one `texture_cube` | 6 faces | all | 2 MiB |
| 1024 | 4096 | 64 | 384 | 265 | 6.7 MiB |
| 2048 | 8192 | 256 | 1536 | 830 | 21 MiB |
| 4096 | 16384 | 1024 | 6144 | 2560 | 65 MiB |

A layer is a 128 px tile with an 8 px gutter on each side and one mip below it, 144^2 + 72^2 bytes in BC7, about 26 KB; the gutter is 27% overhead at this tile size, the price of small tiles. The 4096 level is what the 21600 px NASA sources support without upscaling (their equivalent face width is 5400), and it is the level the on-demand download of section 11 carries; the three levels up to 2048 are what ships. Everything coarser than the floor comes from the floor's own mip chain.

What is resident is a small subset of that: the worst case for a 4K frame at any zoom is about 435 layers (section 8's tile-size table), 11 MiB in BC7, plus the floor's 2 MiB, and it does not grow when the 4096 level is added, because the finer the level the fewer tiles a frame covers. A cache of about twice that, 900 layers, needs `max_texture_array_layers` raised from the adapter, which every target offers at 2048 or more (section 6).

The page table changes shape at this tile count. With the 4096 level the finest grid is 6 faces of 32 x 32 cells, 6144 entries at 4 bytes, which no longer fits the 16 KiB uniform buffer the current limits allow. The portable answer is the classic one from sparse virtual texturing: an indirection texture, `R32Uint`, a 2D array of 6 layers at 32 x 32, read with `textureLoad`, 24 KB, rewritten whole and uploaded with one `write_texture` per tick that changed anything. Each cell holds the layer index and level of the best resident ancestor and the constant-ocean flag, resolved on the CPU exactly as section 8 describes; the shader does one `textureLoad`, then either the floor cube, the ocean color, or one `textureSampleGrad` into the tile array. Nothing about the fragment shader depends on how many levels exist.

Why 128 px and not 64 or 256. The tile size trades the gutter overhead, the ocean and visibility savings, and the bookkeeping against each other; the 64 px row is extrapolated from the trend of the measured ones [M][R]:

| Tile | Gutter overhead | Pure-ocean tiles at 2048 faces | Worst case resident for a 4K frame | Page-table entries, two levels |
|---|---|---|---|---|
| 64 | 56% | about 55% | about 1,500 layers, about 9 MiB BC7 | 7,680 |
| 128 | 27% | 41% to 46% | 435 layers, 11 MiB | 1,920 |
| 256 | 13% | 25% to 31% | 153 layers, 15.5 MiB | 480 |
| 512 | 6% | 7% to 10% | 57 layers, 21 MiB | 120 |

The gutter is a fixed 8 px, so its share grows as the tile shrinks and dominates below 128. Smaller tiles follow coastlines and the edge of the visible set more closely, which is what buys 128 its 30% memory advantage over 256. The tile count grows four times per halving, in the visible-set descent, the upload calls and the page table, and at 64 px the resident count passes the 2048 array layers that DX12, Metal and lavapipe allow, so it would need several arrays; at 128 the count stays inside every target's limit with room for hysteresis. The on-disk cache barely moves with tile size (about 22, 24 and 28 MiB per month at 128, 256 and 512), and quality and encoding do not move at all, since BC7 works on 4 x 4 blocks and the download is whole faces. So 128 is the knee: the smallest size at which the gutter is not the dominant cost and the layer count fits every adapter. If the 27% gutter overhead ever matters, a 4 px gutter with the tile sampler's anisotropy clamped to 2 halves it, at some cost to sharpness near the limb.

The poles. This is where the cube earns its keep against the layout in use today. In an equirectangular map the top row's 8192 pixels all describe one point, the texel area ratio between the equator and the last row is 2608 to 1, the anisotropy is unbounded, and the sampler's 16x anisotropy runs out above 86.4 degrees of latitude, which is why the pole shows a pinched star of blurred columns when the camera looks down on it (section 5). In the cube each pole sits at the center of the +Y or -Y face, which is the one place on a face where the equi-angular texel is exactly the equator's size and square. A pole is an ordinary point inside an ordinary tile: no convergence of tiles, no seam column, no special case in the visible-set computation, and a drag across the pole moves the sub-camera point across the face like anywhere else. The worst places on the cube are not the poles but its eight corners, at 35.3 degrees north and south and, in the default orientation, at longitudes 45, 135, 225 and 315 degrees, where a texel is 15% longer on one side and the anisotropy reaches the square root of 3, which the 8x anisotropic tile sampler covers with room to spare. The bake can rotate the cube about the polar axis so more of those corners fall on ocean; in the default orientation two of the northern ones land on Iran and Japan.

Two things follow for the polar tiles themselves. The north polar face's center is the Arctic Ocean, which is a flat fill in every month of the source (section 3), so the tiles around the north pole are constant-ocean tiles and cost nothing. The south polar face's center is Antarctica, land in the mask and identical in every month, so those tiles are baked twelve times to the same bytes; a pack format that deduplicates identical tiles across months, or a shared "static" pack for tiles the mask says never change, would remove that, and the bake can measure how much it is worth. The one polar artifact the cube does not remove is in the data: the source's antimeridian discontinuity above 75 degrees north (section 3) lies in the middle of the +Y face rather than on a face edge, so if it is visible after the bake it shows as a line across the Arctic Ocean, and the ocean flattening by the mask is what hides it.

The floor cube keeps 16x anisotropy and hardware seamless filtering across its face edges; the tile array keeps 8x and its gutters; both are sampled through the same warped direction and the same screen derivatives, so the level switch at the floor boundary is continuous (section 8).

## 17. Shipping size: whole faces or tiles, the 4096 level, deduplication, multi-frame files

Measured on 2026-09-29 on the real NASA files: May 2004 at 21600 x 10800 reprojected to equi-angular faces of 4096 and 2048 with the ocean flattened through the shipped mask, and all twelve months at 5400 x 2700 for the cross-month questions. JPEG XL at quality 85, effort 7, through the pipeline's Pillow plugin (libjxl 0.11.1). Scripts under the session's `shipsize` directory [M].

First, the baseline in section 11 was wrong: the v0.2.1 Windows archive is 17.8 MB, of which the four textures are 4.7 MB stored uncompressed and the day and night maps 4.0 MB; the 30 MB figure was the unpacked executable, which deflates to 13 MB [M].

Whole faces against tiles, May, day only:

| Level | Variant | Files | Bytes | Ocean tiles dropped | Decode, estimated |
|---|---|---|---|---|---|
| 4096 | six whole faces | 6 | 4.93 MB | none | 7.2 s |
| 4096 | 1024 px tiles | 90 | 4.96 MB | 6% | 7.5 s |
| 4096 | 512 px tiles | 283 | 5.07 MB | 26% | 7.7 s |
| 4096 | 256 px tiles | 869 | 5.54 MB | 43% | 11.4 s |
| 4096 | 128 px tiles | 2780 | 6.24 MB | 55% | 26.9 s |
| 2048 | six whole faces | 6 | 1.43 MB | none | 1.8 s |
| 2048 | 512 px tiles | 90 | 1.49 MB | 6% | 2.4 s |
| 2048 | 256 px tiles | 283 | 1.63 MB | 26% | 3.7 s |
| 2048 | 128 px tiles | 869 | 1.83 MB | 43% | 8.4 s |

Decode times are one core, from the measured jxl-oxide costs of 8.5 ms per image plus 71 ns per pixel [R]. Whole faces are the smallest download at both levels, even though the tiled variants drop a quarter to a half of the tiles, because a flattened ocean costs almost nothing in JPEG XL to begin with. Cutting a face into tiles loses coding context across the cuts: measured on 1024 px tiles that are land throughout, splitting into 512 px costs 1.6% to 2.5%, into 256 px 9% to 12%, into 128 px 24% to 28%, on top of about 80 bytes of header per file [M]. So the shipped form is six whole faces per level per month, decoded once on the user's machine and cut into the small BC7 tiles there; the tile size of the cache and the image size of the download are independent, as decision 1 in section 15 assumed.

The 4096 level (16384-equivalent): 4.93 MB per month for the day, about 3.6 MB for the night (estimated by the day's ratio, since the night source in the tree is only 8192 wide), and about 1 MB for a mask at that level. Twelve months of it add about 64 MB to the archive [M][R]. That is too much to ship for detail that only shows when zoomed past the full disk (section 8); it stays a per-month download of about 5 MB, through the same HTTP cache the clouds use, and a user who never zooms in never fetches it.

Deduplication across months, measured on the twelve 5400 months at 512 per face with 32 px tiles, which cover the same ground as 128 px tiles at 2048 [M]:

| Tiles of land | Byte-identical in all twelve months | Identical to the previous month | Stored after deduplication |
|---|---|---|---|
| all 869 | 12.3% (17.7% within 2 levels) | 15.9% | 84% of the tiles, 94% of the JPEG XL bytes |
| Antarctica, 60 | 98% | | 10% |
| 60 S to 25 N, 479 | 9% (18% within 2 levels) | | 84% |
| 25 N to 90 N, 330 | 1% | | 98% |

Almost every tile that never changes is Antarctica, and outside it the never-changing tiles are coastal slivers with no land to speak of; no mostly-land tile stays byte-identical across the year, the Sahara included, because the source's vegetation model moves every land pixel a little every month. Deduplicating the download therefore saves 5% to 7% and is not worth a tiled format, which costs more than that (above). In the BC7 cache the identical tiles are 16% to 20% of the tile count, because Antarctica's tiles cost the same 26 KB as any other there; but the better answer for the cache is not to hold twelve months at all. A year of 128 px BC7 tiles at the 2048 level is 265 MiB on disk, with the 4096 level about 800 MiB; a cache that holds the current month and the next, rebuilt as the months come at 2.5 s each (section 15), is about 45 MiB and needs no deduplication. The BC7 encoder is a pure function, so a content hash in the pack index would deduplicate for free if a full-year cache is ever wanted [R].

Multi-frame JPEG XL: twelve months as twelve files came to 12,212,882 bytes and as one twelve-frame file to 12,212,723 bytes, 159 bytes less, so libjxl 0.11 concatenates frames and predicts nothing between them; its own format overview says patches and reference frames are used only in a limited way by the encoder, and an application wanting inter-frame gains would have to compute cropped or blended frames itself [M][V]. An explicit month-to-month difference image, encoded on its own, is 43% to 55% of a month's size at the same setting but reconstructs 1 to 3 dB worse, and about 20% smaller at equal quality with the errors accumulating along the chain [M]. Not worth doing: one file per month per level.

The archive, with the two 8192 maps replaced by twelve months of 2048 faces, one night cube and one lossless mask cube, against today's 17.8 MB [M][R]:

| What ships | Textures | Archive |
|---|---|---|
| today: two 8192 maps, the Moon, the Milky Way | 4.7 MB | 17.8 MB |
| twelve months of whole 2048 faces, night cube, mask cube | 18.7 MB | 32.6 MB |
| the same as 512 px tiles | 19.6 MB | 33.4 MB |
| the same plus the 4096 level for all twelve months | 82.9 MB | 96.8 MB |

The moon and the Milky Way are unchanged in every row, and the twelve-month totals scale May by the measured ratio of the twelve 5400 months to May, 12.16, because winter months carry more snow and compress slightly worse.

Decisions taken on these measurements:

4. The archive carries one JPEG XL file per face per month at the 2048 level: 72 day files, 6 night faces and 6 lossless mask faces, 84 files beside the Moon and the Milky Way. The 1024 level and the 512 floor are halved from the 2048 faces on the client with the same box filter the mip chain uses, so they ship as nothing. A 4096 level, if it is ever offered, is another 72 files as a per-month download.
5. The client decodes the faces, cuts the 128 px tiles, encodes them with `dds` Fast and keeps the whole year in the BC7 cache, not just the current month, so that the custom-date slider can move through the year with every month's tiles a positional read away. That is about 265 MiB on disk at the 2048 level (section 17) and about 35 s of background work on four cores on first run (section 15), the current month first.
6. The cache deduplicates tiles by content hash of the encoded BC7 bytes, since the measured share of identical tiles, 16% to 20% of a year's tiles, is above the 5% below which it would not be worth the index entry. The saving is 40 to 55 MiB of the 265 MiB, mostly Antarctica, and somewhat less once the gutter is part of the tile, since a tile only matches when its neighbors match too; the plan should measure the real figure on the first full bake and drop the hash if it comes out under 5%. The hash needs to be collision-safe for a cache that outlives the process, a 256-bit one from a pure-Rust crate such as `blake3` or `sha2`.

## 18. Sources

Data: https://science.nasa.gov/earth/earth-observatory/blue-marble-next-generation/, https://science.nasa.gov/earth/earth-observatory/blue-marble-next-generation/base-map/, https://science.nasa.gov/earth/earth-observatory/blue-marble-next-generation/topography-bathymetry-maps/, https://earthobservatory.nasa.gov/ContentFeature/BlueMarble/bmng.pdf (Stöckli et al., the BMNG paper), https://www.nasa.gov/nasa-brand-center/images-and-media/, https://cloudless.eox.at/documentation/license, https://ladsweb.modaps.eosdis.nasa.gov/missions-and-measurements/products/VNP46A3/, https://svs.gsfc.nasa.gov/3272/.

Projection and sampling: https://jcgt.org/published/0007/02/01/ (Zucker and Higashi), https://docs.vulkan.org/features/latest/features/proposals/VK_EXT_non_seamless_cube_map.html, https://docs.vulkan.org/spec/latest/chapters/textures.html, https://www.w3.org/TR/WGSL/, https://github.com/gfx-rs/wgpu/issues/10486, https://mrelusive.com/publications/papers/Software-Virtual-Textures.pdf (van Waveren).

wgpu: https://docs.rs/wgpu/28.0.0/wgpu/struct.Limits.html, https://docs.rs/wgpu/28.0.0/wgpu/struct.Queue.html, https://docs.rs/wgpu/28.0.0/wgpu/struct.TexelCopyBufferLayout.html, https://docs.rs/wgpu/28.0.0/wgpu/struct.Device.html, https://github.com/gfx-rs/wgpu/tree/v28.0.0/examples/features/src/mipmap, https://github.com/gfx-rs/wgpu/issues/3637, https://github.com/gpuweb/gpuweb/issues/455, https://github.com/bevyengine/bevy/pull/22286, plus wgpu-types 28.0.0 `limits.rs` and `features.rs`, wgpu-core 28.0.1 `queue.rs`, `transfer.rs`, `binding_model.rs`, wgpu-hal 28.0.1 `dx12/adapter.rs`, `vulkan/adapter.rs`, `metal/adapter.rs`, naga 28.0.0 `valid/analyzer.rs`.

Encoders (section 15): https://github.com/image-rs/image-dds, https://crates.io/crates/dds, https://crates.io/crates/block_compression, https://github.com/Traverse-Research/intel-tex-rs-2 (branch `arm64-perf`, PR #48), https://github.com/richgel999/bc7enc (published mode-6 against full-BC7 figures), https://crates.io/crates/texpresso, https://crates.io/crates/bcdec_rs.

Crates: https://docs.rs/jxl-oxide/latest/jxl_oxide/, https://github.com/tirr-c/jxl-oxide/blob/main/CHANGELOG.md, https://github.com/tirr-c/jxl-oxide/pull/505, https://github.com/tirr-c/jxl-oxide/issues/411, https://github.com/libjxl/jxl-rs, https://github.com/libjxl/jxl-rs/issues/933, https://github.com/Traverse-Research/intel-tex-rs-2/pull/50, https://crates.io/crates/ctt, https://crates.io/crates/block_compression, https://crates.io/crates/texpresso, https://crates.io/crates/ktx2, https://crates.io/crates/bcdec_rs, https://crates.io/crates/ruzstd, https://github.com/Cykooz/fast_image_resize, http://cbloomrants.blogspot.com/2020/07/performance-of-various-compressors-on.html, https://aras-p.info/blog/2020/12/08/Texture-Compression-in-2020/, https://doc.rust-lang.org/std/os/unix/fs/trait.FileExt.html, https://doc.rust-lang.org/std/os/windows/fs/trait.FileExt.html, https://docs.rs/memmap2/latest/memmap2/struct.Mmap.html, https://db.cs.cmu.edu/mmap-cidr2022/, https://github.com/kurtkuehnert/bevy_terrain, https://github.com/carloskiki/wgpu-virtual-texturing.

Streaming and prior art: https://cesium.com/blog/2013/04/25/horizon-culling/, https://cesium.com/learn/cesium-native/ref-doc/structCesium3DTilesSelection_1_1TilesetOptions.html, https://github.com/CesiumGS/cesium/blob/main/packages/engine/Source/Scene/TimeDynamicImagery.js, https://github.com/OpenSpace/OpenSpace/blob/master/modules/globebrowsing/src/tileprovider/temporaltileprovider.cpp, https://github.com/NASAWorldWind/WebWorldWind/blob/develop/src/layer/BMNGRestLayer.js, https://github.com/CelestiaProject/Celestia/blob/master/src/celengine/virtualtex.cpp, https://dev.epicgames.com/documentation/en-us/unreal-engine/runtime-virtual-texturing-in-unreal-engine, https://docs.unity3d.com/ScriptReference/QualitySettings-streamingMipmapsMaxFileIORequests.html, https://xplanet.sourceforge.net/README.config, https://github.com/Stellarium/stellarium/discussions/4153.

## 19. Step 0 spikes

### 19.1 Spike (a): BC7 on the software adapters and on the Metal device

Measured on 2026-09-29 by a new shader test target, `crates/sunlit-core/tests/bc7.rs`, which stays in the tree as the permanent test of BC7 sampling that Step 3 needs [M]. The fixture is generated rather than committed: 256 px of five-octave value noise as terrain over a smooth ocean ramp, a per-texel grain of 36 levels peak to peak on the land, hard coastlines, and 0.3% saturated points. Its mip chain is the renderer's own box filter (`texture_loader::downsample_2x`, now public for the test) down to 4 x 4, the smallest level that holds a whole block; each level is encoded with `dds` 0.2.0 at `CompressionQuality::Fast` and uploaded as `Bc7RgbaUnorm`, the unorm variant because the surfaces are `Rgba8Unorm` today. One render pipeline samples three textures: the BC7 one, the RGBA8 original, and `dds`'s own CPU decode of the same blocks uploaded as RGBA8. Two framings: flat, one texel per pixel at texel centers through an isotropic linear sampler, and receding, a strip of two triangles whose top vertices are 8x wider in clip space than the bottom ones over the same u span, through the globe's sampler (trilinear, 16x anisotropy, repeat in u, clamp in v). It is not a receding plane: the two triangles get different projective maps, so the texture creases along the diagonal. In the lower-left triangle u runs at 4 texels per pixel along the bottom and 32 near the top-left corner, in the upper-right triangle at 0.5 along the bottom and 4 at the top, and v goes from 8x magnified to 8 texels per pixel. The comparison stays valid because the BC7 and the RGBA8 textures go through the same geometry. The device is the golden suite's (the software adapter where there is one, the default adapter on macOS) with `TEXTURE_COMPRESSION_BC` requested where it is offered and the app's own limits, and the comparison is the golden suite's, moved into `tests/support` so the two targets share it.

| Adapter | BC reported | BC7 against the original, flat: mean, share over 24, worst | Receding | BC7 against `dds`'s decode, worst channel, flat and receding | One mip level lost, flat |
|---|---|---|---|---|---|
| warp (Microsoft Basic Render Driver, DX12, this machine) | yes | 0.727, 0.002%, 32 | 0.431, 0%, 11 | 0 and 1 | 6.11 |
| lavapipe (llvmpipe, Mesa 23.2.1, LLVM 15.0.7, Ubuntu 22.04 in WSL, this machine) | yes | 0.727, 0.002%, 32 | 0.475, 0%, 8 | 0 and 0 | 6.16 |
| lavapipe (llvmpipe, LLVM 20.1.2, `ubuntu-latest`, CI runs 36636984850 and 36639048927) | yes | 0.727, 0.002%, 32 | 0.592, 0%, 10 | 0 and 0 | 6.16 |
| metal (Apple Paravirtual device, `macos-latest`, CI run 36630793491) | yes | 0.727, 0.002%, 32 | 0.415, 0%, 11 | 0 and 0 | 6.12 |

What the table says:

- Every adapter reports `TEXTURE_COMPRESSION_BC`, as section 6 expected, the paravirtual Metal device included.
- Every adapter decodes BC7 as `dds` does: at texel centers the adapter's sample and the CPU decode agree to the bit on all three, and through the receding framing to one step on warp, which is filtering precision. The paravirtual device's BC7 decode, which section 6 left unverified, is correct.
- The flat framing reproduces the texels exactly on all three (the case asserts it), so its 0.727 is the encoder's own error, the same figure the CPU gives for the level 0 blocks. That is about a third of the golden mean budget, with 0.002% of the pixels over 24 against the 1% allowed; through the globe's sampler it drops to 0.42 to 0.59, the top of that range being CI's lavapipe. One lost mip level of the same fixture reads 6.1 on every adapter, so the tolerance does see a loss the size a wrong decode would cause.
- The fixture is harder on the encoder than the real maps. `dds` Fast on 72 crops of 256 px spread over each shipped map, made opaque as the day tiles will be, gives a mean channel error of 0.22 on the day map (worst crop 0.87, worst channel 23, no pixel over 24) and 0.15 on the night map (worst crop 1.60, worst channel 34, at most 0.006% of a crop over 24); on the crops with any texture in them (a green standard deviation over 8) that is 49.5 dB and 51.9 dB PSNR.

One finding on the side, and the reason the flat framing has its own sampler: lavapipe's anisotropic filter is not an identity on a one-texel footprint. Through the globe's 16x sampler, lavapipe's flat render of the original sat a mean of 4.69 channel steps from its own texels (worst 161, at a saturated point), and one lost mip level then read 2.05 against the 2.0 tolerance; with anisotropy 1 the frame is exact and the figures are warp's. Warp and the Metal device are exact at 16x. That was measured on WSL's Mesa 23.2.1 only: the CI runs above sample the flat framing at anisotropy 1 too, so nothing has measured the one-texel blur at 16x on the Mesa that `golden.yml` generates the lavapipe references on, where the receding framing reads 0.592 rather than 0.475. Whether the blur is in the lavapipe golden references already is therefore unconfirmed.

Outcome: BC7 sampling is inside the golden tolerance of its original on all three adapters and correct on all three, so as far as correctness goes the references could be generated from BC7 everywhere. Section 19.2 is why the two software adapters should sample RGBA8 decoded from the BC7 on the CPU rather than BC7 itself.

### 19.2 Spike (b): what the equi-angular path costs on the software adapters

Measured on 2026-09-29 and 2026-09-30 on this machine (AMD Ryzen 7 5800X, 8 cores and 16 threads, Windows 11; warp through DX12, lavapipe through Vulkan in WSL 2 on the same CPU) with a throwaway harness in the run directory (`spike-eac/`, wgpu 28.0.0, pinned by a copy of the tree's lockfile) [M]. It draws the globe the app draws: the app's 64 x 64 UV sphere with back faces culled, a depth buffer, 4x MSAA resolved into `Rgba8Unorm`, and `fs_main`'s shading tail after the samples, with `blend.wgsl` copied from the tree and the default config's shading values in a uniform buffer so that nothing folds away. The lens is the default 20 degrees at a distance of 2.5, so the globe covers every pixel of the frame: the worst case, and the Milky Way's 133 ms is a full-frame cost too. The textures are the shipped maps. Today's path samples the 8192 x 4096 day map (the mask in alpha) and the night map, oriented by the loader's `orient` and mipped by the same box filter; the cube paths sample six 2048 faces reprojected from those oriented maps at the direction the mesh's UVs give each texel, so every variant draws the same picture, which was checked by eye on the saved frames. Two views: mid-latitude, where the frame crosses cube face edges and a corner, and the north pole, where the frame lies inside the +Y face. A frame is timed by the wall clock from encoding to `poll(wait)`; each figure is the mean of 8 to 12 frames after one or two warm-up frames, with the variants interleaved round by round, and of 2 to 6 frames on lavapipe where a frame takes seconds. On warp the standard deviations are 1% to 7% of the mean. On lavapipe the frames of a few tens of milliseconds vary by 10% to 30%, which is a few milliseconds, and the frames of seconds by 5% to 15% except where the table says otherwise.

The variants: today's path (`textureSample` of the two equirectangular maps at the mesh UVs); the equi-angular cube like for like (`w = atan(n / max(|n|)) * 4 / pi`, `dpdx(w)` and `dpdy(w)`, and `textureSampleGrad` of a day cube carrying the mask in alpha and of a night cube), in RGBA8 and in BC7; the plan's floor (opaque BC7 day and night cubes and the BC4 mask cube at 1024, three samples); and a prototype of the plan's tile path (the face and its face coordinates from `w`, one `textureLoad` of an `Rg32Uint` page table of 6 x 16 x 16 cells, the 144 px layers with their 8 px gutters in two `texture_2d_array`s of 1,536 layers and two mips, sampled with `textureSampleGrad` and the gradients carried into layer space, and the BC4 mask cube). Each figure below is the difference from today's path in the same rounds, in milliseconds.

Warp, with the plan's samplers (16x on the globe's sampler, 8x on a second one for the tiles):

| Variant | 1080p, mid-latitude | 1080p, pole | 4K, mid-latitude | 4K, pole |
|---|---|---|---|---|
| today's path, the whole frame | 200 ms | 220 ms | 785 ms | 819 ms |
| cube, RGBA8 | +1.4 | -2.5 | +2.5 | +10.9 |
| cube, RGBA8, implicit derivatives | +1.1 | -4.7 | +0.8 | +1.8 |
| cube, BC7 | +41 | +65 | +110 | +177 |
| floor: BC7 cubes and the BC4 mask | +60 | +87 | +201 | +243 |
| tiles in BC7, and the BC4 mask | +206 | +225 | +792 | +839 |
| tiles in RGBA8, and the BC4 mask | +193 | +198 | +748 | +801 |

The same run with anisotropy 1 on both samplers moves nothing beyond the noise (cube RGBA8 +2.5, +2.1, +12.2 and +4.8; floor +72, +98, +195 and +206; tiles in BC7 +210, +228, +788 and +819), and neither does rendering without MSAA (at 1080p, cube RGBA8 -1.0 and -2.1, floor +63 and +79, tiles in BC7 +208 and +207).

Lavapipe:

| Variant | 1080p, mid-latitude | 1080p, pole | 4K, mid-latitude | 4K, pole |
|---|---|---|---|---|
| today's path, the whole frame, plan's samplers | 42 ms | 55 ms | 163 ms | not measured |
| cube, RGBA8, plan's samplers | +7,274 | -3.2 | +8,703 (sd 14%) | not measured |
| cube, RGBA8, implicit derivatives, plan's samplers | +2,326 | -0.4 | +5,039 (sd 39%) | not measured |
| cube, BC7, plan's samplers | about +116,000 (one frame) | not measured | not measured | not measured |
| tiles in BC7, plan's samplers | +2,000 | +2,482 | +4,468 (sd 22%) | not measured |
| tiles in RGBA8, plan's samplers | +652 | +74 | +1,162 | not measured |
| today's path, the whole frame, anisotropy 1 | 33 ms | 31 ms | 107 ms | 113 ms |
| cube, RGBA8, anisotropy 1 | +3.6 | -0.8 | +2.6 | +2.5 |
| cube, BC7, anisotropy 1 | +263 | +240 | +774 | +843 |
| floor, anisotropy 1 | +268 | +247 | +784 | +895 |
| tiles in BC7, anisotropy 1 | +250 | +255 | +843 | +898 |
| tiles in RGBA8, anisotropy 1 | +4.9 | +5.8 | +7.2 | +22.4 |

Anisotropy 2 and 4 on lavapipe cost what 16 does (cube RGBA8 +6,083 and +6,188 ms at 1080p mid-latitude, the BC7 cube 120 s a frame at 4), so lavapipe's slow path is switched on by any anisotropy at all on a cube map, and it is taken where the frame crosses a face edge: the pole view, inside one face, costs nothing at 16x.

On this machine's GPU (RX 6800 XT, Vulkan) every variant is within 0.2 ms of today's path at 4K, the tile prototype included, so the question is only the software adapters.

What it comes to, against the yardstick of 133 ms at 1080p and 555 ms at 4K on warp (section 14 item 4):

1. The equi-angular mapping costs nothing measurable on either software adapter. Its warp, its derivatives and a `textureSampleGrad` of a cube stay within 7 ms of today's path at 1080p and 12 ms at 4K, 5% and 2% of the yardstick, so decision 1 stands and the banded equirectangular fallback is not reopened. Explicit gradients cost nothing either: the cube with implicit derivatives reads the same as with explicit ones, and so does today's path with explicit ones (+2.9 ms at 16x and -0.9 ms at 1x, warp, 1080p mid-latitude).
2. BC7 sampling is what costs on a software adapter. On warp the floor in BC7 and BC4 is +60 to +98 ms at 1080p and +195 to +243 ms at 4K: under the yardstick, but 45% to 74% of it at 1080p, for a saving a CPU adapter does not need, since its textures are system memory anyway; the cache can stay BC7 and be decoded to RGBA8 at upload, which `dds` does at about 600 megapixels a second on one thread, 0.03 ms for a 144 px layer. On lavapipe BC7 costs +240 to +268 ms at 1080p and +774 to +898 ms at 4K even at anisotropy 1, over the yardstick at both sizes, and with any anisotropy it is 116 s to 120 s a frame. The same paths in RGBA8 at anisotropy 1 cost at most +22 ms. This is the plan's departure 4.
3. Any anisotropy on a cube map is pathological on lavapipe where the frame crosses a face edge: +6.1 s to +8.7 s a frame in RGBA8, against +4 ms at anisotropy 1. Warp does not care. Departure 4 covers this too.
4. On warp the tile prototype costs +190 to +230 ms at 1080p and +750 to +850 ms at 4K, over the yardstick, and none of that is the tile path. At 1080p it stays between +190 and +225 ms with the page-table load removed, with the layer made uniform, with the face selection and the cell arithmetic removed, with a 2D atlas in place of the array, and at anisotropy 1. With the tiles sampled through the globe's own sampler instead of a second one, the stripped variant is -1.4 ms (pole view; -4.7 without the mask), a second sampler whose state is identical to the first costs the same +198 ms as the tiles' own, and the whole prototype in BC7 costs what the BC7 floor costs in the same rounds: +56 and +59 ms at 1080p and +137 and +159 ms at 4K, against the floor's +52 and +62 and +134 and +149. Warp charges about 200 ms per 1080p frame for a second sampler binding in the fragment shader, whatever its state. This is the plan's departure 5. Lavapipe charges nothing for it (tiles in RGBA8 at anisotropy 1, two samplers, +5 ms).

Not measured: CI's `ubuntu-latest` carries a newer Mesa than the 23.2.1 of Ubuntu 22.04 in WSL, and its anisotropic and BC7 paths may differ; the harness would have to be in the tree to run there. The Metal device's cost was not asked for and not measured.

### 19.3 Spike (c): the full bake of one month

Measured on 2026-09-29 on Windows 11, 16 logical cores, 64 GB, through a throwaway script run with `uv run` in the pipeline's own environment (CPython 3.14.5, numpy, Pillow with pillow-jxl-plugin, the pipeline's `get_or_create_mask`, `detect_ice_regions`, `reduce_mask_for_ice`, `apply_ocean_mask` and `encode_jxl` imported unchanged) [M]. Input: May 2004 topography at 21600 x 10800 (22.7 MB JPEG), the Natural Earth 10m ocean shapefile, and the in-tree Black Marble map, which is 8192 x 4096.

Method, in order:

1. Decode the JPEG and rasterize the shapefile at the source size with the `earth` command's settings (supersample 2, Lanczos down), then the ice detection and reduction at their defaults (60 degrees, luminance 200).
2. Reproject the picture and the mask to six equi-angular faces at 5400 px, the density of the 21600 source at the equator: inverse mapping from each face texel center (tangent warp of pi/4, the cube map table of decision 1, longitude `atan2(x, z)`), bilinear with wrap in longitude and clamp at the poles, uint8 out.
3. Flatten the ocean at 5400 with `apply_ocean_mask` and the pipeline's default fill (10, 30, 60), using the face mask.
4. Area-average 5400 to 2048 with Pillow's `BOX` resize (exact area weights at the non-integer ratio 2.64), for the picture and the mask alike.
5. Encode the six day faces with `encode_jxl` at quality 85, effort 7; the six mask faces lossless (quality 100 through the same function) as 8-bit single channel, 255 for ocean and 0 for land, ice already subtracted; the night faces are bilinear at 2048 straight from the 8192 map (no working size, since a 2048 face never magnifies an 8192 source) and quality 85.

Timings, six worker threads over the faces:

| Stage | Seconds |
|---|---|
| Decode the 21600 JPEG | 1.4 |
| Rasterize the ocean shapefile at 21600 x 10800 | 40.3 |
| Ice detection and reduction | 12.2 |
| Reproject the day map to 6 x 5400 | 18.0 |
| Reproject the mask to 6 x 5400 | 10.6 |
| Flatten the ocean at 5400 | 1.9 |
| Area-average to 2048 (picture and mask) | 0.4 |
| Encode six day faces | 4.8 |
| Night: decode, reproject, encode | 6.2 |
| Encode six lossless mask faces | 6.8 |
| Total, wall clock | 102.5 |

The same run with two worker threads: 115 s in total (reproject 26.5 s and 13.9 s), so the reprojection is memory bound rather than core bound and the parallelism buys about 12%.

Peak working set: 19.8 GiB with six threads, 8.4 GiB with two. The floor is the decoded source (about 0.7 GB) plus the ocean raster at supersample 2 (0.9 GB) and the ice band buffers, 3.6 GiB at the end of the ice stage; the excess is the per-face float and int64 intermediates of the reprojection (5400^2 texels, coordinates in float64 and int64, four gathered neighbours), which are at their worst with six faces in flight [M].

Output sizes (bytes), one month:

| Face | Day, q85 | Night, q85 | Mask, lossless |
|---|---|---|---|
| +X | 270,721 | 257,790 | 86,024 |
| -X | 196,733 | 218,821 | 48,039 |
| +Y | 519,679 | 294,521 | 147,241 |
| -Y | 103,580 | 30,142 | 40,946 |
| +Z | 251,780 | 197,737 | 46,171 |
| -Z | 57,721 | 34,584 | 33,024 |
| Total | 1,400,214 | 1,033,595 | 401,445 |

The day total agrees with the 1.43 MB measured in section 17 for the same month at the same setting (1.40 MB here, the small difference being the area average against the 2x2 box). The +Y face is the largest of the six because the Arctic snow and the northern land are on it. Twelve months at this size extrapolate to about 17 MB of day faces with the section 17 winter factor, plus 1.0 MB of night and 0.4 MB of mask, in line with the 18.7 MB of that section's table [R].

Orientation, checked two ways. Forward: an independent function takes a longitude and latitude, forms the direction, selects the face by the major axis and computes the texel with the OpenGL and Direct3D cube map selection formulas (`sc`, `tc`, `ma`, then the pi/4 warp), the inverse direction of what the bake does. It was run against 13 places: the Gulf of Guinea at longitude 0 (face +Z, texel 1024, 1024, ocean), Cairo, the Sahara, the Congo and Cape Town on +Z (land), the Amazon on -X, Australia on +X, the Pacific on -Z, Tokyo on -Z, Greenland and the pole itself on +Y (the pole is ocean, texel 1026, 1024), Antarctica on -Y, and a mid-Atlantic point on +Z; each read the expected mask value (land below 60, ocean above 200) and a plausible color, all 13 passed. Visual: a cube cross contact sheet (`out/check/cross.png`) shows Africa upright on +Z with the Sahara above the Congo, the Americas on -X to its left, Asia and Australia on +X to its right, the Arctic Ocean centered on +Y with Europe's edge toward +Z, and Antarctica centered on -Y [M].

What this changes for Step 1:

- The bake is about 100 s a month on this machine and 12 months is about 20 minutes, but 52 s of each month (the shapefile raster and the ice pass) do not depend on the month except through the ice detection, which reads the month's picture. The raster of the ocean at 21600 can be computed once and shared across months, saving 40 s a month, or 8 of the 20 minutes; the ice pass has to stay per month unless the source's sea ice is judged absent (section 3 says the set carries none, so the ice pass may be a no-op on this data and worth measuring on all twelve months before deciding to drop it).
- Memory needs a cap, not a thread count: run the reprojection one or two faces at a time (peak 8.4 GiB at two workers, and less at one), or reproject in row bands, so the bake also runs on a 16 GB CI or contributor machine.
- The mask is reprojected from a source-resolution raster; T1a should produce it once per bake, not once per month, and the twelve day files of a run share the same six mask faces byte for byte.
- The night faces come from the 8192 map, so the night level is source limited at 2048 exactly as the research said; the sources are unchanged from the tree.
- The scripts and outputs are in the run directory under `spike-bake/` (`bake_one_month.py`, `check_orientation.py`, `out/`); they are throwaway and not in the tree.

## 20. Step 1: the full bake

Measured on 2026-09-29 by the pipeline's `cube` command on the twelve 21600 x 10800 months, the 13500 x 6750 Black Marble and the Natural Earth ocean layer, on the machine of section 19.3 [M]. The bake took 572.8 s with four workers at a peak working set of 3.04 GiB, set by the shapefile raster; the resampling runs in bands of 256 rows and stays below it. The 84 files come to 18,464,893 bytes, against the 18.7 MB section 17 estimated. `textures/PROVENANCE.md` has the stages, the sizes per set and the sources.

Deduplication, measured on the decoded faces rather than on the source [M]. The 84 files were decoded by libjxl through Pillow, a 1024 level made from each 2048 face by a 2 x 2 box, and both levels cut into 128 px tiles, once bare and once with an 8 px gutter: inside a face the gutter is the neighboring texels, at a face edge the edge texels repeated, so the edge tiles are approximate. A tile whose window of the mask at its level is all 255 counts as constant ocean and is not stored; the rest are hashed with SHA-256 over their pixels. Identical pixels make identical BC7 blocks, since the encoder is a pure function, so this is the share a content hash in the pack index can remove.

| A year of day tiles, both levels | Bare 128 px | With the 8 px gutter |
|---|---|---|
| All tiles | 23,040 | 23,040 |
| Constant ocean, not stored | 9,300 | 8,724 |
| Stored before deduplication | 13,740 | 14,316 |
| Distinct | 13,307 | 14,000 |
| Saving | 3.2% | 2.2% |

That is far under the 16% to 20% of section 17, and the reason is the lossy encode. Section 17 counted tiles identical in the source, which south of 60 S is the same in every month, and the bake is deterministic, so the months' faces are identical there before they are encoded [R]. Each month is encoded on its own, though, and decoded the identity is gone: in the central 800 x 800 texels of `ny`, which lie wholly south of 65 S, January and July differ in 5.7% of the texels, by a mean of 0.03 of 255 and at most 6, and one differing texel is enough to make a 128 px tile unique. On `ny` 1,452 tiles are stored and 1,356 of them are distinct. The saving is under the 5% below which the plan drops the dedup pass (decision 3 and the last of its risks); the real gutters cross face edges, which touches only the edge tiles. Getting the identity back would mean encoding the unchanging region once for all months, which one file per face and month does not allow.

## 21. Step 2: the tile packs

Measured on 2026-09-30 on the machine of section 19.3 (AMD Ryzen 7 5800X, 8 cores and 16 threads, Windows 11) with `assets::tiles` as of the commit that added this section, through a throwaway harness in the run directory (`tiles-bench/`, a release build pinned by a copy of the tree's lockfile) that builds all fourteen packs from the 84 shipped faces with `ensure_pack`, checks them again, and reads them back [M]. `RAYON_NUM_THREADS` sets the thread count, and it bounds the JPEG XL decode and the encode alike, since both run on rayon's global pool. The geometry is the plan's: 128 px tiles with an 8 px gutter and one mip at the 1024 and 2048 levels, a 512 px floor cube with its full chain in every surface pack, and the mask as a BC4 cube of 1024 px with its chain.

What the packs hold:

| Pack | Tiles | Constant ocean | Stored | Bytes |
|---|---|---|---|---|
| One month of day, each of the twelve | 1,920 | 725: 89 of 384 at 1024, 636 of 1,536 at 2048 | 1,195 | 33,180,104 |
| Night | 1,920 | 716: 87 at 1024, 629 at 2048 | 1,204 | 33,413,376 |
| Mask | none, six BC4 faces | | | 4,195,072 |
| All fourteen | | | | 435,769,696 (415.6 MiB) |

A stored tile is 25,920 bytes, 144 px and 72 px of BC7; the six floor faces are 2,097,312 bytes together; an index entry is 56 bytes and the key about 480, so the header and index of a surface pack come to 108 KB. The day's constant-ocean count is the same in every month, since one mask serves them all, and with the gutter the 2048 level's constant-ocean share is 41.4% against the 46% section 16 counted without one.

Building one month (May), in seconds:

| Threads | Decode the twelve faces | Cut, encode, hash and write | Total |
|---|---|---|---|
| 1 | 2.10 | 7.84 | 9.94 |
| 4 | 1.06 | 2.59 | 3.65 |
| 16 | 0.98 | 1.32 | 2.30 |

The whole year, all fourteen packs in a row: 58.5 s on four threads (4.4 to 4.5 s a month once the machine is warm, the night 4.8 s, the mask 0.6 s) and 30.6 s on sixteen, at a peak working set of 205 MiB. The second pass, which opens each pack and compares its key and builds nothing, takes 54 ms for the fourteen. Reading every stored tile of May back takes 0.014 ms a tile with the file in the page cache, and decoding a tile's two levels from BC7 to RGBA8 with `dds`, which is what a CPU adapter's upload adds (plan departure 4), 0.042 ms on one thread.

The first version encoded one tile at a time and let `dds` split each layer across the pool, in the four-row fragments its BC7 encoder asks for: 5.1 s a month and 66 s the year on four threads, an encode speedup over one thread of 2.0. Encoding 64 tiles side by side, each on one thread, and splitting only the whole faces, makes that 3.0 and gives the figures above, with byte-identical packs. Against section 15's estimate of 2.5 s a month and 35 s the year on four cores: the encode alone, 2.6 s, is what that section estimated, and the rest is the decode of the month's twelve faces, 2.1 s on one thread, which halves on four and gains little beyond, and the night pack and the floors, which the year's figure carries and the estimate did not.

The night over open water. Of the 453 tiles at 2048 whose layer lies inside one face and whose mask there is all open water, the worst texel sits within 1 level of the tile's mean in 40.6%, within 2 in 98.0% and within 4 in 98.5%; the other 1.5% carry lights at sea, up to 250 above the mean, 92 texels over 40 in all. The night's open water averages (5.0, 5.0, 15.2). May's flattened ocean, for comparison, is within 1 of its mean in 98.7% of the same tiles and within 3 in all of them, at a mean of exactly the pipeline's fill (10, 30, 60). So a night tile is flagged constant ocean where the mask says open water across its footprint, as a day tile is, and every texel of its layer also lies within 4 of the night pack's ocean color, the rounded mean of its open water, (5, 5, 15); 716 of the 725 water tiles qualify and the rest keep their lights. That takes the night pack from 51,971,928 bytes with every tile stored to 33,413,376. The day packs record (10, 30, 60) as their ocean color in every month.

Not measured: the build under Linux, on a machine with fewer cores, or with the engine rendering beside it, which is the transcoder worker's (T2b) to measure.

## 22. Step 2: the transcoder worker

Measured on 2026-09-30 on the machine of section 21 with `assets::tiles::Transcoder` as of the commit that added this section, through a throwaway harness in the run directory (`transcoder-bench/`, a release build) that starts the transcoder on the 84 shipped faces with May in force and records every status it publishes [M]. The pool size is the harness's argument; the default on this machine is four, the cores less two capped at four. Every thread of the worker runs below normal priority.

A first run, with the time each pack took and when the first frame's three packs (the mask, May and the night) were all ready:

| Pool threads | Mask | May | Night | A later month | First frame ready | Year |
|---|---|---|---|---|---|---|
| 4 (default here) | 0.44 s | 4.07 s | 4.97 s | 4.6 to 4.9 s | 9.48 s | 60.7 s |
| 2 | 0.66 s | 7.13 s | 8.32 s | 7.6 to 8.8 s | 16.1 s | 102.5 s |
| 1 | 1.31 s | 14.6 s | 13.2 s | 9.9 to 12.2 s | 29.1 s | 142.2 s |

The month in force is ready 4.5 s into a first run on four threads, which is criterion 4's 10 s with room, and the year in 60.7 s against 58.5 s for the same builds in a row without the worker (section 21). The pool size stands in for a machine with fewer cores only roughly, since the rest of this machine stayed free. On one thread most months take 10 s, as section 21 found, and three took 12.2 to 14.6 s, which this run does not explain; a below-normal thread gives way to whatever else the machine runs, and nothing else of this project ran beside it. A second start over the whole cache, which checks the fourteen keys and builds nothing, is Done in 5 to 6 ms, and 59 ms the first time after the year was built.

How fast the worker gives way. Starting June, the first pack past the first frame, and then either making October the month in force or closing the pause gate at a set time into the build, the status leaves June after 17 to 204 ms over sixteen trials at 0.1 to 4 s into it, 78 ms at the median, with October building or the worker paused and June not ready. That is the granularity of the cancel flag, which `ensure_pack` looks at between face decodes and between batches of 64 tiles.

What the lowered priority buys. A foreground load of sixteen threads of integer work, 2.8 to 2.9 s alone, takes 2.8 to 2.9 s beside the transcoder building the year on a pool of sixteen, and 3.1 to 3.5 s beside the same year built through `ensure_pack` on a rayon pool of sixteen at normal priority, which itself went from 30.8 s alone to 34.9 s. The lowered transcoder pays for it instead: its year takes 37.1 s with the load beside it against 30.9 s alone, where the load ran for about 6 s of it. So on Windows the worker's threads take only the time the foreground leaves; how that holds against the engine's own render is for the engine wiring to measure.

Not measured: the timings under Linux, where the unit test of `thread_priority` shows the nice value lowered by 10 on the worker and on the threads it makes and nothing more, and against the engine rendering beside it, which needs the engine to start the transcoder.

## 23. Step 4: the wanted set

Measured on 2026-09-30 on the machine of section 19.3 with `renderer::residency` as of the commit that added this section and revised after its first review, through a throwaway harness in the run directory (`residency-bench/`, a release build with the tree's LTO settings) [M]. The stored predicate is the real May and night packs' constant-ocean flags, 725 and 716 of their 1,920 tiles, from a cache the transcoder built; the output is 3840 x 2160; the zoom slider is swept in 201 even steps from 0 to 1; the rule is the one docs/rendering.md describes (The wanted set): the texel counted as the sampler's level of detail counts it, at anisotropy 8 on a GPU and 1 on a CPU adapter, with every bound erring toward the finer level, and the horizon the mesh reaches.

The worst in view over the zoom, day only, per place, lens and adapter, with the distance in radii:

| Place (longitude, latitude) | fov 10, GPU | fov 20, GPU | fov 30, GPU | fov 20, CPU |
|---|---|---|---|---|
| Europe and Africa (15, 20) | 482 at 12.6 | 448 at 8.6 | 425 at 6.2 | 298 at 5.0 |
| Atlantic (-30, 0) | 458 at 16.3 | 433 at 8.6 | 403 at 6.4 | 247 at 4.9 |
| Pacific (-150, 0) | 399 at 16.3 | 369 at 9.2 | 330 at 6.4 | 174 at 4.9 |
| Americas (-80, 10) | 379 at 15.7 | 357 at 8.6 | 336 at 6.0 | 228 at 5.1 |
| Asia (100, 30) | 539 at 12.3 | 507 at 8.3 | 489 at 6.5 | 328 at 4.9 |
| North pole (0, 89.9) | 531 at 10.7 | 491 at 6.0 | 482 at 6.3 | 321 at 4.8 |
| South pole (0, -89.9) | 341 at 10.7 | 301 at 6.0 | 287 at 6.3 | 155 at 4.8 |
| Cube corner (45, 35.26) | 512 at 11.2 | 502 at 9.3 | 481 at 6.3 | 359 at 5.1 |

The margin is empty at every one of these: the whole disk is in the frame, and the margin rings the frame, not the horizon (plan departure 19). The peak sits where most of the cap in view wants the 2048 level, below the zoom at which a 1024 texel at the limb projects to a pixel, which is 9.4 radii through the default lens. Counting the texel as an anisotropy of 8 does takes the last degrees before the limb down a level, which moves the peaks nearer, from 19 to 12 radii through the narrow lens and from 9.5 to 8.3 through the default one, and puts them 2% to 4% below the first version's (550 over Asia through the narrow lens, 528 through the default one). Section 8's sweep sampled 5.76 and 11.5 radii, either side of the peak, and one view; at 5.76 the descent wants 431 here, section 16's 435. So the worst day is about 540 tiles, a quarter over 435 (plan departure 18). A CPU adapter's sampler has no anisotropy, so its level follows the texel's shorter side and it wants about a third fewer.

Blend mode, the worst in view over the zoom and the eight places, on a GPU, for the default shading and the extremes the settings window offers (terminator 0.01 to 0.3, floor 0 to 1, ramp 0.1 to 1), with the Sun over three longitudes:

| Settings | Sun over | fov 10 | fov 20 | fov 30 |
|---|---|---|---|---|
| Defaults: terminator 0.1, shading on, floor 0.7, ramp 0.2 | 105 E | 665 | 634 | 610 |
| | 60 E | 667 | 626 | 609 |
| | 15 E | 672 | 628 | 615 |
| Shading off, terminator 0.1 | 105 E | 632 | 601 | 577 |
| | 15 E | 643 | 602 | 588 |
| Terminator 0.3, floor 0.7, ramp 0.2 | 15 E | 769 | 717 | 706 |
| Terminator 0.1, floor 0, ramp 1 | 105 E | 1,033 | 983 | 953 |
| | 15 E | 954 | 915 | 887 |
| Terminator 0.3, floor 0, ramp 1 | 105 E | 1,071 | 1,014 | 979 |
| | 15 E | 982 | 970 | 936 |

With diffuse shading `blend_fragment` holds the shaded day at or above `min(night, day)` wherever the shading darkens it, below the ramp, so the night is wanted up the ramp on the day side as well as below the terminator: the default ramp of 0.2 adds 26 to 33 tiles to the terminator's band, and a ramp of 1 with a floor below 1 wants the night over nearly all of the day side, about as many tiles again as the day, over the 900 layers of `TILE_LAYER_BUDGET` by up to 19%. The worst views are Asia, the north pole and the cube corner, the ones with the most land in the cap.

Day through the default lens over Europe and Africa along the zoom: in view and margin on a GPU with the real flags, in view with every tile stored, in view during a drag (the 2 px threshold, no rate), and the tiles a ray cast of every second pixel finds needed by the pixel's own footprint at 1 px, then the same two on a CPU adapter:

| Radii | In view | Margin | All stored | Drag | Ray cast | In view, CPU | Ray cast, CPU |
|---|---|---|---|---|---|---|---|
| 1.5 | 17 | 54 | 17 | 17 | 15 | 17 | 15 |
| 2 | 53 | 73 | 53 | 53 | 45 | 53 | 45 |
| 3 | 196 | 79 | 278 | 195 | 169 | 182 | 163 |
| 4 | 313 | 62 | 460 | 304 | 295 | 255 | 232 |
| 5.76 | 431 | 0 | 667 | 203 | 434 | 285 | 269 |
| 8 | 447 | 0 | 686 | 140 | 442 | 231 | 173 |
| 9.4 | 448 | 0 | 688 | 140 | 409 | 175 | 115 |
| 11.5 | 154 | 0 | 195 | 5 | 144 | 81 | 68 |
| 15 | 150 | 0 | 188 | 0 | 142 | 60 | 34 |
| 20 | 57 | 0 | 76 | 0 | 16 | 10 | 0 |
| 40 and 80 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |

The ray cast counts the tile of the level each pixel asks for, so where the descent wants a coarse tile for part of its area and its children for the rest the two count differently, and at 5.76 radii the ray cast finds three more; elsewhere the descent wants 1% to 18% more, most where tiles along the frame's edges, whose balls reach into the frame while they do not, are a larger share of a small set, and at 20 radii 57 tiles where 16 have a pixel that needs one, since a tile's bound passes 1 px for tiles well past the pixels that do. A drag halves the set or more where the two levels mix and changes nothing where the view wants the finest level at 2 px as well.

How much a tile is read past its coarser level, the other side of the approximation: of the pixels drawn from a tile (the cap's level, where that tile is stored), every fourth pixel of the same view, the share whose level of detail at the sampler's anisotropy is past the tile's second level, the share past it by more than a level, and the worst:

| Radii | GPU: past | more than a level | worst | CPU: past | more than a level | worst |
|---|---|---|---|---|---|---|
| 1.5 | 0% | 0% | 0 | 0% | 0% | 0 |
| 3 | 0.0% | 0.0% | 1.42 | 0.6% | 0.0% | 3.42 |
| 5.76 | 0.4% | 0.1% | 2.17 | 18.1% | 0.9% | 3.14 |
| 8 | 2.7% | 0.1% | 1.94 | 42.7% | 0.5% | 1.73 |
| 9.4 | 18.9% | 0.1% | 2.31 | 43.7% | 0.2% | 1.40 |
| 11.5 | 1.5% | 0.0% | 1.35 | 21.3% | 0.0% | 1.19 |

A tile is judged at its worst point, the nearest and least oblique, so its farther and more oblique parts ask for less than its coarser level; nearly all of it is by less than a level, a filter up to twice as sharp as the sampler would take, and on a CPU adapter the obliqueness across a tile is the larger share. Before the rule counted the anisotropy, the review measured 30% of a CPU adapter's tiled pixels past at 5.76 radii, 69% at 8 and 92% at 9.4, 16% to 22% by more than a level and 5.2 levels at worst (plan departure 20).

Cost of one computation on a GPU, 500 runs each, median, 95th percentile and worst:

| Case | Median | p95 | Worst | Tiles |
|---|---|---|---|---|
| Near end, 1.5 radii | 0.024 ms | 0.027 ms | 0.035 ms | 71 |
| Full disk, 5.76 | 0.108 ms | 0.121 ms | 0.242 ms | 431 |
| Mixed levels, 8 | 0.113 ms | 0.116 ms | 0.159 ms | 447 |
| Far end, 80 | 0.021 ms | 0.021 ms | 0.024 ms | 0 |
| Drag at 5.76, 100 and 20 degrees a second | 0.289 ms | 0.332 ms | 0.472 ms | 234 |
| Blend at the defaults, 9.4 | 0.123 ms | 0.155 ms | 0.194 ms | 560 |

`Residency::new` for the shipped geometry takes 1.2 ms and is done once. The loader's convergence after a drag, and what the margin and the lead prefetch, are in section 25.

## 24. Step 4: the tile loader

Measured on 2026-09-30 on the machine of section 19.3 with `engine::tile_loader` as of the commit that added this section, through a throwaway harness in the run directory (`loader-bench/`, a release build) [M]. The packs are the shipped faces' (section 21), May's day pack and the night pack; the wanted sets are computed as in section 23, over the same eight places, the three lenses and 201 steps of the zoom.

What one tile costs at each of the loader's three steps, on one thread, with the pack warm in the OS cache: a positional read with its CRC check, 0.012 to 0.033 ms for the 25,920 bytes of a BC7 layer and its mip; the decode to RGBA8 a CPU adapter's tile array takes (`renderer::tiles::decode_tile`, through `dds`), 0.072 ms for the 103,680 bytes it makes; and the upload through `SurfaceTiles::upload`, staged and then through a submit and a wait for the device, median of five batches:

| Tiles | RX 6800 XT (Vulkan), BC7: MiB, staged, through the device | WARP, RGBA8 decoded | WARP, BC7 |
|---|---|---|---|
| 1 | 0.02, 0.03 ms, 0.15 ms | 0.10, 0.05 ms, 2.7 ms | 0.02, 0.05 ms, 0.43 ms |
| 8 | 0.20, 0.06 ms, 0.20 ms | 0.79, 0.28 ms, 19.6 ms | 0.20, 0.20 ms, 1.7 ms |
| 40 | 0.99, 0.25 ms, 0.55 ms | 3.96, 0.78 ms, 96 ms | 0.99, 0.62 ms, 7.2 ms |
| 160 | 3.96, 0.93 ms, 1.7 ms | 15.8, 4.3 ms, 385 ms | 3.96, 1.9 ms, 27 ms |
| 400 | 9.89, 2.8 ms, 4.7 ms | 39.6, 10.4 ms, 963 ms | 9.89, 4.7 ms, 68 ms |

On the GPU a tile costs about 11 us through the device, 0.43 ms a MiB. On WARP the cost is in the submit, WARP's copy into its own texture layout, and it is 2.4 ms a tile in RGBA8, 24 ms a MiB, where the same layers in BC7 take 0.17 ms, 6.9 ms a MiB: the copy costs WARP more a byte in RGBA8 as well as four times the bytes. Reads and decodes are never what limits the loader: four workers read well over 100,000 tiles a second or decode about 55,000, against a peak set of 539.

So `UPLOAD_BUDGET` is 4 MiB a tick, which is one constant for both adapters: on the GPU 160 tiles in about 1.7 ms, so the largest day set arrives in four ticks, and on WARP 40 RGBA8 tiles in about 96 ms through the device, half of what a 1080p frame costs WARP on its own (section 19.2), so the frames go on between the uploads while a set streams in.

The drain reaches that budget only because it waits for the workers. The result channel holds two tiles a worker, eight on four workers, and the first drain took only what was queued, 10 to 14 tiles, so the budget never bound and the largest day set took about 40 ticks (found in the loader's review). The drain as built takes what is queued and then, while a worker still holds a claimed tile or has one left to claim, waits for the next result, `DRAIN_WAIT` of 4 ms at the most in all, until the budget is spent; every result is uploaded before the drain returns, so no tile's texels are parked. Measured on 2026-09-30 end to end through an engine over the real May pack (`t4d/drain-bench` in the run directory, a release build with debug assertions so the loader's log is there, the preview drawn but not read back, timed from the switch of the resolution setting from 2048 to 8192 until the report holds every wanted tile resident) [M]:

| Adapter, view | Tiles | Drains, tiles each | Complete after | The drain that took what was queued |
|---|---|---|---|---|
| RX 6800 XT, 4K, Asia (100, 30) at 8.3 radii, fov 20 | 506 BC7 | 4: 162, 162, 162, 20 | 15 ms | 50 drains, 37 ms |
| WARP, 4K, cube corner at 5.1 radii, fov 20 | 357 RGBA8 | 9: eight of 41, 29 | 95 ms (427 ms on a cold file cache) | 47 drains, 400 to 440 ms |
| WARP, 1920 x 1088, Asia at 8.3 radii | 50 RGBA8 | 2: 41, 9 | 54 ms | |

On WARP these count the uploads handed to the queue; WARP's copy into its layout runs when the next readback waits for the device, which the preview's does every tick in the app. The byte budget binds on both, 162 BC7 tiles or 41 RGBA8 ones a drain, and the wait never came near its 4 ms: the workers read and decode faster than the engine takes the results.

The worst wanted set over the zoom, the lenses and the places, in view and with the margin, for one 4K output and for the same camera also drawn into a 1920 x 1088 preview, which is the app's case when a wallpaper is exported with the settings window open (T4d adds the export's own output):

| Surfaces | Outputs | GPU, in view | GPU, with margin | CPU adapter, in view | CPU adapter, with margin |
|---|---|---|---|---|---|
| Day | 4K | 539 | 539 | 389 | 408 |
| Day | 4K and preview | 679 | 679 | 458 | 466 |
| Blend, default shading | 4K | 665 | 665 | 496 | 517 |
| Blend, default shading | 4K and preview | 842 | 842 | 589 | 597 |
| Blend, terminator 0.3, floor 0, ramp 1 | 4K | 1,071 | 1,071 | 753 | 785 |
| Blend, terminator 0.3, floor 0, ramp 1 | 4K and preview | 1,352 | 1,352 | 889 | 901 |

A CPU adapter's sampler has no anisotropy, so its rule wants 67% to 75% of the tiles a GPU's does. On a GPU the largest sets with the margin are the sets in view, since the whole disk is in the frame at the zoom where they peak; on a CPU adapter they peak at 8 to 10 radii through the narrowest lens, and the largest set with the margin is up to 32 tiles more than the largest in view. The two budgets keep the same ratio to those peaks: `TILE_LAYER_BUDGET`, 900 BC7 layers on a GPU, 22.2 MiB, holds the day at 1.7 times its peak, blend at the default shading at 1.35 times, and blend with a preview at 1.07 times; `CPU_TILE_LAYER_BUDGET`, 640 RGBA8 layers on a CPU adapter, 63.3 MiB of system memory, holds the same at 1.57, 1.24 and 1.07 times. Only the shading sliders at their extremes pass either, by 19% on a GPU and 23% on a CPU adapter for one output and by 50% and 41% with a preview beside it; there the array takes the set in its order until its layers are spoken for, and the rest, the margin and then the coarser level farthest from the middle of the frame, draws from what is resident above it or from the floor (plan departure 22).

## 25. Step 4: readiness, the exports and the drag

Measured on 2026-09-30 on the machine of section 19.3 through real engines over the real May pack (`t4d/drain-bench` in the run directory, its `drag`, `publish` and `lead` programs, release builds with debug assertions) [M]. The preview is drawn and read back on every frame, as the app does with its window shown.

A drag of 5 degrees of longitude every 50 ms, twelve moves from (-10, 20) at the day surface, timed from the last move until the loader's report shows the drag over and every tile in view of the 1 px set resident or failed (plan success criterion 5):

| Adapter, preview, zoom | Seen as a drag | Missing in view during it, mean and worst | Drag seen to end | Missing then | Converged |
|---|---|---|---|---|---|
| RX 6800 XT, 1920 x 1088, 0 (1.5 radii) | 11 of 12 moves | 0, 0 | 103 ms | 0 of 21 | 103 ms |
| RX 6800 XT, 3840 x 2160, 0 | 11 of 12 | 0, 0 | 102 ms | 0 of 21 | 102 ms |
| RX 6800 XT, 1920 x 1088, 0.1 (2.2 radii) | 11 of 12 | 0, 0 | 104 ms | 0 of 75 | 104 ms |
| RX 6800 XT, 3840 x 2160, 0.3 (4.0 radii) | 11 of 12 | 0, 0 | 111 to 112 ms | 0 of 423 | 111 to 113 ms |
| WARP, 1920 x 1088, 0 | 9 of 12 | 0, 0 | 445 ms | 0 of 21 | 445 ms |
| WARP, 1920 x 1088, 0.3 | 9 of 12 | 4.8, 22 | 530 ms | 62 of 173 | 928 ms |

The first move of each drag is not one, since it follows a rest, and on the GPU the drag ends `DRAG_PAUSE` after the tick that drew its last move and converges in that same tick. On WARP a 1080p frame and its readback take about 200 ms: the last move is drawn up to a frame after it arrives, the pause counts from the end of that tick, and the tiles the drag's 2 px threshold left out then land at 41 RGBA8 tiles a tick, one tick a frame, which is the 400 ms between the drag's end and the convergence at zoom 0.3. At zoom 0 nothing was missing when the drag was seen to end, so WARP's 445 ms there is the end of the drag being seen and nothing else: its frames and readbacks, and `DRAG_PAUSE`, the one constant in that figure. Criterion 5 measures the near end of the zoom, and on WARP it was met there, at 445 ms against 500 ms in the one run made, a margin of 55 ms that the phase of the last move, drawn up to a frame after it arrives, can move by more than that, so one run shows it met once and not that it holds; the 530 and 928 ms are at zoom 0.3, which the criterion does not cover. Before the pause counted from the end of the tick, WARP saw none of the twelve moves as a drag, since each came more than 100 ms after the one before it was drawn (plan departure 25). The engine case `a_drag_converges_to_the_one_pixel_set_within_half_a_second_of_stopping` measures 100 ms on the mock clock over the Earth fixture on warp.

What the margin and the lead prefetch, simulated with `Residency::wanted` over the same pack at the same drag, 60 degrees in steps of 5 degrees at 50 ms or 1.6 degrees at 16 ms: a tile in view that the set of the tick before did not ask for is one the frame draws from its ancestor if loads take a tick, and at the stop the 1 px set is held against the last drag set.

| Constants (`MARGIN_TILES`, `DRAG_LEAD_SECONDS`, 3 steps) | 1080p, zoom 0 | 1080p, zoom 0.1 | 4K, zoom 0.3, 5 degrees | 4K, zoom 0.3, 1.6 degrees | 4K, zoom 0.3, set with margin | 4K, zoom 0.3, missing at the stop |
|---|---|---|---|---|---|---|
| 0, 0 s | 3.3 a tick, worst 5 | 6.0, 9 | 18.8, 22 | 6.1, 14 | 393 | 15 |
| 1, 0 s | 0 | 0 | 18.1, 21 | 5.9, 14 | 413 | 15 |
| 0, 0.15 s | 0.2, 3 | 0.5, 6 | 1.1, 13 | 0.2, 8 | 452 | 4 |
| 1, 0.15 s (as built) | 0 | 0 | 1.1, 13 | 0.2, 8 | 470 | 4 |
| 1, 0.3 s | 0 | 0 | 1.2, 13 | 0.2, 8 | 529 | 4 |

Near the globe the margin is what keeps up: the frame's edge moves a tile or less a tick. Farther out, where the drag turns tiles over the limb, the lead is: without it 18 tiles a tick in view were not asked for, with it one. A lead of 0.3 s buys nothing over 0.15 s and asks for 59 more tiles, and every row wants nothing new in view at the stop at the near end, so the constants stay as T4a set them. At zoom 0.3 on a 1080p preview the 2 px set is the coarser level nearly everywhere, 397 to 402 of the 415 tiles of the 1 px set are not in it at the stop, and the worst tick, 109 tiles, is the first of the drag, where the set drops to the coarser level; those are the tiles the drag's threshold exists to leave out, and they load after it, in 3 drains on the Radeon. `UPLOAD_BUDGET` stays 4 MiB: on the Radeon the convergence is the pause and one tick, and on WARP a larger budget would take fewer ticks but lengthen each by 2.4 ms a tile through WARP's copy (section 24), which the frames between the uploads pay for.

A 4K wallpaper asked for while a 1920 x 1088 preview's set is resident, which joins the 4K set at the front of the wanted set, timed from `RenderWallpaperNow` until `WallpaperSet`, and a second publish of the same with every tile resident:

| Adapter, view | Preview in view | Set with the 4K render, in view | Missing when the set was computed | Published after | With every tile resident |
|---|---|---|---|---|---|
| RX 6800 XT, Asia (100, 30) at 8.3 radii | 153 | 642 | 165 | 39 ms | 10 ms |
| RX 6800 XT, cube corner at 9.3 radii | 146 | 625 | 479 | 31 ms | 8 ms |
| WARP, Asia at 8.3 radii | 50 | 270 | 179 | 1,212 ms | 245 ms |
| WARP, cube corner at 5.1 radii | 163 | 409 | 205 | 1,869 ms | 432 ms |

So a publish waits 20 to 30 ms for its tiles on the GPU and 1.0 to 1.4 s on WARP, well inside `TILE_WAIT`'s 5 s; on WARP each tick of the wait is a 1080p preview frame and its readback beside the 41 tiles uploaded. `Residency::cap` for one 4K output, which each export render and each wanted set computes once more (plan departure 23), takes a median of 0.041 to 0.060 ms and at worst 0.099 ms.

## 26. Step 5: the floors of every month

Measured on 2026-10-01 on the machine of section 19.3 through a real engine over the real packs (`t5/floors-bench` in the run directory, a release build with debug assertions), a 1920 x 1088 preview at the 8192 setting in blend mode, from a cache holding the whole year, so every pack lands in the first tick and the other months' floors follow one a tick once the engine has been idle for the two seconds of its busy span (plan departure 32). The memory report's computed table and wgpu's texture counter before the other months' floors and after all twelve, and the time each floor took to make resident, timed around `Renderer::install_surface` in a throwaway build [M]:

| Adapter | A day floor | Twelve day floors | Counter, first frame to twelve | Private bytes | One install | All twelve after the first frame |
|---|---|---|---|---|---|---|
| RX 6800 XT (Vulkan), BC7 | 2.00 MiB | 24.0 MiB, 26.0 with the night | 57.3 to 82.1 MiB (+24.8) | 392.4 MiB, unchanged | 1.9 to 2.3 ms; the first two 9.5 and 6.4 ms, the mask 4.2 | 2.6 s |
| WARP, RGBA8 | 8.00 MiB | 96.0 MiB, 104.0 with the night | 107.9 to 196.6 MiB (+88.7) | 626.6 to 883.1 MiB (+256.5) | 200 to 214 ms; the mask 292 | 4.2 s |

The time for all twelve on the Radeon is the two seconds of the busy span and then one floor a tick of 50 ms, eleven of them: four runs polling the memory report every 100 ms, which leaves the engine its idle tick of 20 a second, measured 2.609 to 2.610 s after the first report. The first measurement polled every 10 ms and gave 2.1 s, because every command ticks the engine and it ran at 100 ticks a second. WARP's 4.2 s is bound by the 0.2 s an install takes and was not measured again.

On a GPU the twelve floors and the night's are decision 4's 26 MiB, the counter is a little above the computed total as it is everywhere (section 24), and the floors are not process memory. On WARP the counter follows the computed total to within 0.7 MiB, but the private bytes rose by 256.5 MiB in one step, at the third floor of the queue, and by nothing at the other ten installs. The memory report's allocator section says why: wgpu's default `MemoryHints::Performance` has the D3D12 allocator take device memory in blocks of 128 to 256 MiB, and on a CPU adapter those are committed process memory; at the first frame 109.4 MiB was allocated in 256 MiB reserved, three blocks, and with the twelve floors 198.1 MiB in 512 MiB, four. The buffers stayed at 1.5 MiB, so no staging outlived its install (`install_surface` submits its upload and waits for it), and ten seconds and three frames later the private bytes were 882.3 MiB, so nothing was waiting for a later submit either. So a CPU adapter keeps the month in force's floor and the month ahead's (plan departure 35): with that the private bytes stayed at 626.4 MiB through a change of month, the allocations at 109.4 MiB in the first 256 MiB, and an export whose date named another month took 265 ms against 63 ms for one in the same month, the difference being that month's floor made resident in the frame that named it. A smaller block size (`MemoryHints::MemoryUsage`, 8 to 64 MiB) would shrink such steps for every texture on a CPU adapter; it was not measured.

The month ahead (plan departure 34) is read within a day of a hand-over. The live clock moves at a second a second, and the engine draws at least every `SKY_INTERVAL`, two minutes, while the date is live, so the month ahead joins the set no later than two minutes into the day before its hand-over, and its tiles in view take a frame on a GPU and about a second on WARP to be read (sections 24 and 25); the custom date moves only when a slider does, a day a step of the day slider and less along the hour slider. Near a hand-over the set holds up to twice the day's tiles in view, 1,078 at the worst 4K frame on a GPU (section 23), and past the 900 layers what is left is the month ahead's, which comes last.

## 27. Step 6: memory on the cube surface

Measured on 2026-10-01 on the machine of section 19.3 and in the two e2e guests [M].

**The memory hint on a CPU adapter** (plan departure 38). A real engine over the shipped packs (`t6/hints-bench` in the run directory, a release build with debug assertions), 1920 x 1088 preview at the 8192 setting in blend mode, the hint switched by a throwaway edit of `wgpu_init.rs`, two runs each, each figure the two runs' range:

| WARP, private bytes | `Performance` (wgpu's default) | `MemoryUsage` |
|---|---|---|
| First frame | 634 to 637 MiB (256 MiB reserved, 3 blocks) | 606 to 608 MiB (200 MiB reserved, 6 blocks) |
| Idle, the CPU adapter's two floors | 634 to 636 MiB | 578 to 580 MiB |
| After a change of month | 650 to 652 MiB | 603 to 604 MiB |
| Idle, every month's floor | 890 to 892 MiB (512 MiB reserved, 4 blocks) | 642 MiB (264 MiB reserved, 7 blocks) |
| Preview frame, median of 15 | 59.2 to 60.6 ms | 59.1 to 59.9 ms |
| 1920 x 1088 export, median of 7 | 59.7 to 60.0 ms | 59.8 to 61.6 ms |

The allocations themselves are the same either way (109.4 MiB at the first frame, 198.1 MiB with the twelve floors); the hint moves only what the allocator holds around them, which on a CPU adapter is committed process memory. So a CPU adapter's device asks for `MemoryUsage`, and a GPU, whose device memory is not the process's, keeps the default.

**The release app** (`t6/measure.ps1`): the release binary at dd94d90, `--mode window --texture-resolution 8192`, its config, cache and metrics in the run directory, the private bytes sampled every 100 ms from outside and `PeakPagefileUsage` read before it quits. The sandbox has no network, so the cloud image came from a copy of an 8192 x 4096 cloud cache entry placed in the empty cache directory, which the app posts at startup as it would after a download; "cold" is a cache directory with no packs, so the app builds all fourteen, "warm" the same directory again:

| Run | Peak private | When | Year built | Settled private |
|---|---|---|---|---|
| RX 6800 XT (Vulkan), cold, no cloud image | 772 MiB | 1.6 s | 66.9 s | 501 MiB |
| RX 6800 XT, cold | 1097 MiB | 1.4 s | 64.7 s | 727 MiB |
| RX 6800 XT, warm | 1032 MiB | 1.2 s | | 724 MiB |
| WARP, cold | 1096 MiB | 1.6 s | 64.3 s | 630 MiB |
| WARP, warm | 1140 MiB | 1.8 s | | 674 MiB |

The peak comes in the first two seconds whether or not anything is built: it is the startup's decodes, the cloud image's 8192 x 4096 among them, about 325 MiB of the GPU's peak. The first frame's packs build while the startup's decodes settle, and with the cloud image reached 1021 to 1062 MiB around 6 to 7 s, under the peak; the rest of the year then runs at 530 to 689 MiB on the GPU without the cloud image, 740 to 921 with it, and 661 to 823 on WARP, its decoded faces freed with each pack, and the footprint drops by about 130 MiB when the last pack is written. Against the flat path's 2488 MiB cold start, measured the same way, the cube surface's is 1140 MiB at the most, and `memory.rs` takes that as `MEASURED_COLD_START_PEAK`. `COLD_START_BYTES`, what a launch costs before any surface texture or cloud image is resident, is 1 GiB: 772 MiB on the GPU without the cloud image, and on WARP 1140 MiB less the 311 MiB of textures the budget counts for 8192, 829 MiB, rounded up. With the headroom of 512 MiB the budget is 1591 MiB at 2048, 1719 at 4096 and 1847 at 8192, clearing the peak by 451 MiB at the narrow end and staying 433 MiB under twice it at the wide one (testing.md).

**The guests**, through a throwaway e2e case that ran the debug app the suite runs, with no cloud image, at the 8192 setting and the window's own 576 x 576 preview, first on an empty cache until the year was built and 30 s more, then twice each warm with every month's floor and with the CPU adapter's two (`EngineConfig::every_floor` set from an environment variable in a throwaway build):

| Guest | Cold: peak private, year built, settled | Warm, two floors | Warm, every floor |
|---|---|---|---|
| Windows 11, WARP | 428 MiB, 257 s, 245 MiB | 257, 257 MiB | 384, 387 MiB |
| Debian 13, lavapipe | 533 MiB, 236 s, 374 MiB | 314, 315 MiB | 427, 428 MiB |

With the hint the twelve floors cost 128 MiB in the Windows guest and 113 MiB in the Linux one, their allocators' reserves going from 140 to 268 MiB and from 111 to 239 MiB, against the 62 MiB measured here, where the reserve at the first frame had room; the CPU adapter keeps its two floors (plan departure 35). The e2e suite's render case, an 800 x 800 render from an empty cache that builds the first frame's packs, peaked at 504 MB of RSS in the Linux guest and ended at 492 MB, against 2088 and 444 on the flat path (testing.md).

## 28. Memory on the desktop GPU: three quick wins

Measured on 2026-10-04 on this desktop: Radeon RX 6800 XT on Vulkan (AMD driver 32.0.21043.10005, "26.5.2"), one 3440 x 1440 screen, release builds of `feature/memory-quick-wins` at 363c014 and at the commits after it [M]. The starting point was a release app idling at about 1.07 GB of private bytes at the 8192 setting while its working set stayed near 400 MiB.

**Method.** Two harnesses, each run twice per variant, everything in the run directory (`run-memory/`):

- The app: the release binary, `--mode window --texture-resolution 8192`, its config a copy of the user's (8x MSAA in the config, the window 1932 x 1068, which gives a 1536 x 1024 preview), `SUNLIT_EARTH_CONFIG`, `SUNLIT_EARTH_CACHE_DIR` and `SUNLIT_EARTH_METRICS_DIR` in the run directory, and the cache a fresh copy for each run of a seed built once against the worktree's textures, so every run is warm (no pack written during it, checked). `SUNLIT_EARTH_CLOUD_URL` points at a closed local port, and the seed holds the user's 8192 x 4096 cloud image as `clouds_cache_override.jpg`, so every run posts the same clouds at startup and never downloads. Private bytes and the working set are sampled once a second from outside (`measure-app.ps1`); memory reports come over IPC 10 s after the start, after an `export-test` 2 s later, and after 60 s idle, then `quit`. The IPC export is 64 x 64, which goes through the same `Renderer::export_image_with` as a wallpaper; no wallpaper was set.
- A headless engine (`bench/`, linked against the worktree's `sunlit-core`): the GPU adapter, the user's scene at a fixed date, a 1536 x 1024 preview at 8x MSAA from the start, the twelve day floors, the clouds and the Milky Way resident; then the median of 21 preview frames (an `UpdateParams` to the `PreviewFrame` it produces) and ten 3440 x 1440 exports, the wallpaper this screen gets, through `EngineHandle::export_pixels`, with memory reports around them. `MQW_PREVIEW=2880x1344` repeats it at a preview of a window filling most of this screen.

Hints other than the build's own were measured through a throwaway `MQW_MEMORY_HINTS` override in `wgpu_init.rs`, never committed.

**Device memory is private bytes on this driver.** `bench/src/bin/devmem.rs` creates a Vulkan device, allocates 4096 x 4096 RGBA8 render targets, clears them and frees them:

| Step | `Performance` | `MemoryUsage` |
|---|---|---|
| Before the device | 15.8 MiB | 15.8 MiB |
| Device created (192 / 12 MiB reserved) | 214.7 | 34.3 |
| Seven 64 MiB targets created, nothing written | 731.1 (704 reserved) | 490.1 (462 reserved) |
| The same, cleared | 731.1 | 490.2 |
| Six freed (reserve back to 192 / 76) | 346.4 | 296.3 |
| A 128 MiB mappable buffer, then freed | 474.6, then 474.6 | 359.9, then 359.9 |

A block counts in private bytes the moment the allocator takes it, with nothing written and the working set flat (103 to 108 MiB throughout); D3D12 on the same card behaves the same (322 MiB once its device exists, 837 with the targets). Freed blocks come back only in part and, in the app, a few seconds later: the driver keeps some of what it was handed back. So on this machine the GPU bytes do overlap private bytes, which `architecture.md` had said only of WARP and lavapipe, and the allocator's slack is paid for in full.

**The baseline.** The app as built at 363c014 draws its preview at 2x MSAA although the config asks for 8x (the second win below says why), and at the first change of any setting it moves to 8x. Both states, and the headless engine, which is at 8x from the start:

| 363c014 | Private, 10 s / after export / idle | Working set | wgpu textures | Allocator in use / reserved | Cloud texture | MSAA color, depth |
|---|---|---|---|---|---|---|
| App, as started (2x) | 792 to 793 / 797 to 798 / 778 to 779 | 381 to 402 | 319.0 | 320.3 / 512 in 4 | 170.8 | 13.6, 12.2 |
| App, 8x (win 2 alone) | 1046 to 1076 / 1051 to 1076 / 1032 to 1058 | 381 to 455 | 395.5 | 396.8 / 768 in 5 | 170.8 | 54.1, 48.2 |
| Engine, 8x | 896 to 906 after the frames, 903 to 913 after the exports | 271 to 431 | 395.5 | 396.8 / 768 in 5 | 170.8 | 54.1, 48.2 |

The second row is the user's report: 1023 MiB private, 768 MiB reserved in five blocks, the same MSAA rows. The engine's preview frame is 2.86 to 2.88 ms, its 3440 x 1440 export 45.9 to 46.2 ms (the first one 52 to 54), and the app's peak private bytes 1081 to 1083 MiB.

**Win 1: the clouds as one channel** (`Keep the cloud overlay as one channel`). `fs_cloud` reads `.r` and nothing else reads the cloud slot, so the fetcher keeps the red channel straight out of the decoder (a gray JPEG is one channel already; upstream's are three equal channels, so the copy is the R of the RGB decode and never an RGBA buffer), `orient` and the box filter work on any channel count, and `create_mipmapped_texture` uploads `R8Unorm`. `R8Unorm` is a core, filterable format on every backend, and the sampled red is the same unorm byte either way; the WARP goldens pass unchanged, which is all this machine can run, and nothing in the change is specific to an adapter, so lavapipe and Metal need no new references. Alone:

| Win 1 | Private | wgpu textures | In use / reserved | Frame | Export |
|---|---|---|---|---|---|
| App, 2x | 792 to 816 / 797 to 822 / 779 to 803 | 191.0 | 192.2 / 512 in 4 | | |
| Engine, 8x | 641 to 649 after the frames, 648 to 656 after the exports | 267.5 | 268.8 / 512 in 4 | 2.80 to 2.87 ms | 77.3 to 77.5 ms |

128 MiB less in use everywhere, and the decoded frame goes from 128 to 32 MiB. Private bytes follow only where a block goes with it: the engine at 8x drops a 256 MiB block and 255 MiB of private bytes, the app at 2x frees the bytes inside blocks it keeps and does not move. The app's peak private bytes went from 1081 to 1083 to 1041 to 1065 MiB and its peak working set from 831 to 836 to 736 to 793. The export got slower, 46 to 77 ms, because it no longer finds room for its targets in the slack the cloud texture left and allocates fresh blocks every time; the hint below takes most of that back.

The soak test's cloud fixture had a decoded frame of 8 MiB at 2048 x 1024 RGBA, which its limits were written in; at one channel that frame would be 2 MiB and a frame parked once a simulated day would no longer fail it. The fixture became 4096 x 2048 gray, 8 MiB again, which made the soak 69 to 77 s on WARP, alone and inside the suite, where 2048 x 1024 of one channel took 21.6 s. The soak has since been rebuilt on exact checks, below.

**Win 2: the export's MSAA targets** (`Set the combo rows before the window's scene is pushed`). The export's targets are not what stays. `export_image_with` makes its four targets as locals; the readback ends in `poll(Wait)`, which retires the submissions that used them, and the locals drop when it returns, which frees them there and then: in the engine the wgpu counters and the allocator read 395.5 MiB and 396.8 / 768 MiB before the first export, after it and after ten (`benchruns/*`). Private bytes do jump right after an export, by 135 MiB (906 to 1041), and are back 5 s later (913): that is the driver releasing what it was handed back late, as in the probe above, not anything wgpu holds. The MSAA rows of the user's report are the preview's own targets, 1536 x 1024 at 8 samples, x1 each in the allocation list and in the expected table, 48 MiB each plus the 6 MiB AMD keeps beside a color target of that many samples.

They had grown because the sample count had: the first report was taken at 2x, the second at 8x. At startup `app.rs` pushes the window's scene to the engine in the same turn that `defer_combobox_indices` asks for the combo rows to be set on the next turn, so the push read the AA row the `.slint` file starts on, 1, which is 2x on this adapter; nothing pushed again until a setting changed, and the first change (in the report, the switch to 8192) sent the configured 8x. Load-defaults had the same order. `set_combobox_indices` now sets the rows at once and again on the next turn, and `tests/slint_ui.rs` holds load-defaults to pushing the config's sample count and texture mode (it fails with the first write removed). Alone this costs the as-started app what 8x costs: idle 1032 to 1058 MiB rather than 778 to 779, the second row of the baseline table, which is where every session ended up after its first change anyway.

**Win 3: the allocator's block size.** Each hint on the baseline (the app as started, at 2x, idle; the engine at 8x after its exports), then with win 1 at both preview sizes:

| Hint (device blocks) | App idle | App reserved | Engine | Engine reserved | Frame | Export |
|---|---|---|---|---|---|---|
| `Performance` (128 to 256 MiB) | 777 to 802 | 512 in 4 | 903 to 911 | 768 in 5 | 2.88 to 2.98 ms | 46.1 to 46.3 ms |
| `MemoryUsage` (8 to 64) | 638 to 641 | 375 in 8 | 607 to 612 | 473 in 9 | 3.41 to 3.47 | 54.4 to 54.9 |
| `Manual` 16 to 64 | 663 to 664 | 403 in 7 | 569 to 570 | 435 in 8 | 2.87 to 3.01 | 64.2 to 64.7 |
| `Manual` 32 to 128 | 687 to 689 | 427 in 6 | 690 to 692 | 555 in 7 | 2.92 to 2.99 | 56.0 to 56.7 |

| With win 1, engine | 1536 x 1024: private, reserved | frame | export | 2880 x 1344: private, reserved | frame | export |
|---|---|---|---|---|---|---|
| `Performance` | 641 to 649, 512 in 4 | 2.80 to 2.87 | 77.3 to 77.5 | 897 to 906, 768 in 5 | 5.49 to 6.12 | 78.5 to 80.5 |
| `MemoryUsage` | not measured | | | 662 to 664, 517 in 9 | 6.34 to 6.42 | 63.9 to 64.0 |
| `Manual` 16 to 64 | 456, 328 in 8 | 2.89 to 2.91 | 64.0 to 64.5 | 652 to 653, 509 in 8 | 6.22 to 6.25 | 64.1 to 64.3 |
| `Manual` 32 to 128 | 511 to 513, 384 in 6 | 2.86 to 2.87 | 55.8 to 55.9 | 648, 518 in 7 | 5.52 to 5.53 | 73.9 to 76.2 |
| `Manual` 64 to 128 | 544 to 545, 416 in 5 | 2.83 to 2.91 | 73.8 to 74.7 | not measured | | |

Every smaller block saves 100 to 300 MiB of private bytes, more the more is resident. The one cost besides the export is the preview frame under `MemoryUsage` and 16 to 64 MiB: the preview is read back through a staging buffer made for each frame (6 MiB at 1536 x 1024, 15 MiB at 2880 x 1344), in host memory whose first block those hints make 4 or 8 MiB, so a buffer that does not fit takes a block of its own, allocated and freed every frame: 0.5 to 0.9 ms of a 3 to 6 ms frame. The default's 64 MiB host blocks hide that, and so do 32 to 128 MiB's 16 MiB ones up to a 16 MiB readback, but no block size covers every window. On WARP with win 1, at the same settings, `MemoryUsage` holds 586 to 593 MiB and 32 to 128 MiB 663 to 669, with frames (248 to 260 ms) and 1920 x 1088 exports (310 to 327 ms) the same within their spread.

**Decision (win 3): `MemoryUsage` on every adapter, and the preview read back through a buffer the renderer keeps** (`Ask every device for small allocator blocks`). The hint saves about 300 MiB over the default on this GPU with the engine at 8x (607 to 612 MiB against 903 to 911) and held the least of the four in the app at 2x (638 to 641), on WARP it held the least of the hints measured there, and it is already what a CPU adapter asks for (plan departure 38), so one setting now serves every adapter and `wgpu_init::memory_hints` and its per-type test are gone. It is not the least on this GPU at 8x: `Manual` 16 to 64 MiB held 37 to 43 MiB less in the engine (569 to 570) and 9 to 12 MiB less at 2880 x 1344 (652 to 653 against 662 to 664). Its frame cost was not the hint's but the per-frame staging buffer's, so that buffer is now made once per preview size (`Renderer::read_preview_pixels`, dropped on a resize, a failed read or the preview being switched off, and made again at the next frame; `the_preview_readback_goes_while_the_preview_is_off` holds the last), which removes a per-frame allocation under any hint. The 16 to 64 MiB and 32 to 128 MiB ranges were measured to see whether a range could keep the default's frame time without that change; 32 to 128 did up to a 16 MiB readback but held 75 MiB more than `MemoryUsage` on WARP, and on this GPU 46 to 51 MiB more in the app idle (687 to 689 against 638 to 641) and 78 to 85 MiB more in the engine (690 to 692 against 607 to 612), though 14 to 16 MiB less at 2880 x 1344 with win 1 (648 against 662 to 664). With the buffer kept, neither range saves more than about 40 MiB over `MemoryUsage` on this GPU, and neither is what a CPU adapter asks for. Win 3 alone, over 363c014:

| Win 3 | Private | In use / reserved | Frame | Export |
|---|---|---|---|---|
| App, 2x, at 10 s / after export / idle | 661 to 692 / 666 to 697 / 647 to 678 | 326.3 / 383 in 9 | | |
| Engine, 8x, 1536 x 1024 | 639 to 679 after the frames, 614 to 622 after the exports | 402.5 / 479 in 10 | 2.86 to 2.87 ms | 54.1 to 54.5 ms |
| Engine, 8x, 2880 x 1344 | 767 to 783 | 581.7 / 638 in 10 | 4.63 to 4.67 ms | 33.7 to 33.8 ms |

The frame is the default's at 1536 x 1024 and faster than it at 2880 x 1344, where the default too had been making a 15 MiB buffer every frame (5.49 to 6.12 ms with win 1). In use is 6 MiB higher than with the default: the kept readback buffer. The export is 54 ms where the default took 46 with the baseline's slack around.

**What the MSAA finding means for the 1.07 GB.** The MSAA rows were never the export's: they were a preview that ran at 2 samples from startup until a UI change pushed the configured 8x. The report the user took after switching to 8192 caught that push: the 8x targets (+76 MiB in use) arrived together with the 8192 cloud image (+128 MiB), and the two together took the allocator's fifth 256 MiB block, which this driver charges to private bytes in full. So the 1.07 GB is the app in the state it was always meant to be in, the configured 8x, with the default hint's slack and an RGBA cloud texture, and not a leak or a retained export; and a session that nobody touched had been drawing at 2x and reading about 250 MiB lower only because of the bug. With win 2 every session starts in that state, which is why win 2 alone reads 1032 to 1058 MiB.

**All three** (579bf63, d6c354d and the hint commit), against that state and against the baseline as it started:

| All three | Private, 10 s / after export / idle | Working set | wgpu textures | In use / reserved | Cloud | MSAA color, depth | Peak private, peak working set |
|---|---|---|---|---|---|---|---|
| App, 8x | 683 to 716 / 687 to 721 / 668 to 702 | 267 to 346 | 267.5 | 274.7 / 404 in 11 | 42.8 | 54.1, 48.1 | 906 to 938, 623 to 685 |
| Engine, 8x, 1536 x 1024 | 439 to 446 after the frames, 445 to 453 after the exports | 150 to 182 | 267.5 | 274.5 / 308 in 9 | 42.8 | 54.1, 48.1 | 887 to 944 |
| Engine, 8x, 2880 x 1344 | 660, then 675 to 676 | 158 to 174 | 437.9 | 453.7 / 531 in 10 | 42.8 | 134.2, 118.5 | 1174 to 1175 |

Against win 2 alone, the same 8x state at 363c014, the idle app holds 668 to 702 MiB instead of 1032 to 1058, 340 to 365 MiB less, with 404 MiB reserved instead of 768; against the baseline as it started, at 2x, it holds 80 to 110 MiB less while drawing at 8x. The headless engine goes from 903 to 913 MiB to 445 to 453, its preview frame from 2.86 to 2.88 ms to 2.36 to 2.88, and its 3440 x 1440 export from 45.9 to 46.2 ms to 34.0 to 64.1, the two runs falling on either side (the export now always allocates its 360 MiB of targets, and how long the driver takes to hand them over varies). The app's startup peak goes from 1081 to 1083 MiB to 906 to 938 and its peak working set from 831 to 836 to 623 to 685. The working set barely moves anywhere, as it should: what went is device memory the process was charged for.

A sunlit-earth process this run did not start was running on the desktop from 00:52 on 2026-10-05, beside the gates; the measurements above are per process and were all taken the day before.

**The soak test, rebuilt on exact checks** (2026-10-05; `Count the decoded pixel buffers alive in the process`, `Check the soak exactly with the decoded frames and wgpu's counters`). The fixture growth above cost a minute per `cargo test` on WARP, and the soak inferred a parked frame from private bytes, which is why its fixture's size mattered at all. The user chose to count the frames instead.

Where the time went, from timing printed around each step and inside the decode and the upload (a throwaway instrumentation, `run-memory/f1/profile-instrumentation.patch`, never committed), on WARP in the debug build the tests run. At the 4096 x 2048 gray fixture a step without a new cloud texture is 12 to 14 ms, 10 to 11 of them the 160 x 96 export's render. A cloud update costs 8 to 13 ms of JPEG decode, 11 to 21 ms of `orient`, 98 to 130 ms of box filter for the mips, and then the first export that samples the new texture takes 1050 to 1230 ms instead of 12; 61.6 of the run's 66.4 s were exports, almost all of them those first draws. At a 512 x 256 color fixture the JPEG decode is 0.4 to 0.5 ms, the copy of the red channel out of the RGB decode 4 to 6 ms, `orient` 0.2 ms, the mips 1.7 to 2.1 ms, and the first draw after an upload 25 to 37 ms; the same seven days took 3.8 s. On lavapipe the large texture's first draw costs nothing extra (seven days in 9.6 s, the mips 77 ms) and the small fixture's seven days take 5.9 s, 16 to 18 ms an export and 2.6 to 8 ms a memory report.

**Decision: the counter.** `DecodedImage::pixels` is a `texture_loader::Pixels`, which adds a frame and its bytes (the buffer's capacity) to a process-wide count when it is made and takes them off when it drops. A wrapper on the buffer and not `Drop` on the image, because `create_mipmapped_texture` destructures the image; it keeps the `Pixels` and swaps each mip level into it (`Pixels::replace`), so the frame is one frame until its last level is uploaded and it drops at the end of that function. Beside the live frames and bytes the count keeps the number of frames ever made, which the task did not ask for and the soak needs: a download is counted by the fixture before it is decoded, so a live count of zero right after one would pass in the moment before the decode, and "made has passed the downloads and nothing is alive" is the exact form. The count is a section of `memory-report` (`decoded pixels:`) and two fields of the `query-memory` line, `decoded_frames` and `decoded_bytes`.

**Which backends keep wgpu's counters.** Read from wgpu-hal 28.0.1 and checked on WARP, lavapipe and Metal (`macos-latest`). D3D12 keeps the texture and buffer bytes (`suballocation.rs` adds and subtracts the allocation size) and the texture and buffer object counts. Vulkan keeps the bytes and the buffer count, but `create_texture` never adds to the texture count while `destroy_texture` subtracts, so it only goes down: -45 on lavapipe after the warm-up, two lower for every export. Metal keeps the object counts and no bytes. No backend keeps the allocation count: D3D12 never touches it, and Vulkan sets it from a counter of its own that nothing increments, so the report's `allocations` is zero everywhere. Because Metal would otherwise have nothing exact on the GPU side, the memory report and the soak read the texture and buffer object counts too, which the task did not ask for. A counter that reads zero is taken as not kept and one below zero as half kept; the soak prints which and asserts on neither.

**Decision: the soak's shape.** After every simulated hour the test waits until the frames made have passed the downloads so far and none is alive, frames and bytes both, and fails after 30 s naming the hour. wgpu's counters are compared at every hour after the warm-up, from a settled report (the Metal run below), and each has to equal the first of them exactly. Private bytes are one backstop, 64 MiB of growth from the end of the warm-up to the end, for host memory outside the counted buffers and GPU memory on Metal, and at this fixture a coarse one, whose reach the paragraph on the downloads below gives. The floors over seven-publication windows, the rise rule, the 32 MiB total, the 128 MiB warm-up limit and the four unit tests that held traces to them are gone: they inferred from private bytes what is now counted, and the lavapipe steps and macOS excursions they were built around are noise a 64 MiB backstop does not see. The fixture is a 512 x 256 JPEG of three components, so the decode goes through the color arm, as a downloaded map does.

**Decision: the length.** Two simulated days, 48 steps of an hour: 48 exports and 16 publications three hours apart for the schedule assertions, the change of month at hour 12 and the month ahead's floor inside an 18-hour warm-up that ends two publications after it, and 30 compared hours with ten publications and a midnight among them. The leak checks need no length, since each is exact at every hour; a week bought the old statistical check its windows.

**Measured** (the soak target alone; "loop" is the 48 hours). WARP, six runs: 3.1 to 3.5 s, loop 1.3 to 1.8 s, against 69 to 77 s before and 21.6 s with the 2048 x 1024 fixture. Lavapipe in WSL, three runs: 2.9 to 3.3 s, loop 1.3 to 1.4 s, against 9.4 s. On WARP texture bytes 15.0 MiB, buffer bytes 1.7 MiB, 13 textures and 9 buffers at all 20 compared hours; on lavapipe texture bytes 15.8 MiB, buffer bytes 0.9 MiB and 9 buffers, with the texture count at -45 and skipped; private bytes +0.2 MiB on WARP and +0.1 on lavapipe. Mutants, each applied, run on WARP and reverted, never committed: a decoded frame parked in a static once per update fails at hour 1 ("1 decoded frames (0.12 MiB) were still alive 30s after the hour"), once per simulated day (every eighth post) at hour 21; a cloud texture kept alive in a static once per update fails the texture bytes from hour 22 on, once per eighth upload from hour 25 on. These figures are from before the settled report; the numbers after it are below.

**The soak on Metal, and the settled report** (2026-10-05; CI run 37282379220 on `macos-latest`, Apple Paravirtual device). The soak failed there on the buffer count alone: "wgpu buffers read 9 after the warm-up and then 14 at hour 23 on metal", with 14 textures at all 20 compared hours and both byte counters at zero, as the source said. Not a leak: the count was 9 again at every compared hour after 23, and a buffer made per update or per day stays. Not Metal's own lifetime either. In wgpu-hal 28.0.1 Metal's `create_buffer` and `destroy_buffer` add and subtract one at the call, `wait` spins until the command buffer of the value asked for reports `Completed`, and `get_fence_value` reads the same status, so after a wait every buffer of a finished submission is destroyed there as on the other backends. What all of them share is wgpu-core's timing: each `write_buffer` and `write_texture` makes a staging buffer, the writes go out with the next submit, and the staging buffers are destroyed at the first `maintain` (a submit or a poll) that finds that submission finished. The export's readback waits, which retires everything before it, but writes go on after it: `export_image_capped` puts the preview's cap back on the page table after the export's submit, a staged rewrite of the table, and a tile read for the preview's finer view that lands after the export is uploaded, two levels and a table rewrite, and makes the next tick draw. With the preview disabled no poll follows until the next hour's export, so an unsettled report counts those staging buffers. The table's was there at every reading on every backend: with a settled report WARP reads 8 buffers and 1703936 buffer bytes where it read 9 and 1769472, one 64 KiB block less, and lavapipe 8 and 957528 where it read 9 and 963672, 6144 bytes less, the table's 48 rows of 32 bytes each padded to a copy pitch of 128. The run printed nothing that names Metal's five more at hour 23; a tile that landed after that export and the draw it caused account for that many. Whether a tile lands before its export or after is timing, and on WARP and lavapipe none did after in ten runs.

**Decision: a settled report for the soak.** `EngineCommand::ReportMemory` carries `settled`, and `EngineHandle::settled_memory_report` sets it: the engine thread submits what is staged and waits for the GPU (`Renderer::flush_and_wait`, the same two calls a floor's install makes) and reads the counters in the same command, so no tick can stage a write between the two. Its counters then count only what something keeps, on every backend, which made the check exact on Metal without loosening it anywhere, and stricter on all of them: the publication hours, left out before because the upload's staging buffers were still alive, are compared too, 30 hours where there were 20. `EngineHandle::memory_report`, the `memory-report` IPC command and the dump stay unsettled, since they report what is alive, and `a_floor_made_resident_while_idle_is_uploaded_at_once` needs the unsettled one: the staging buffers its upload leaves without a submit are what it looks for, and a settled report would pass that mutation. Measured: WARP, three runs, 3.1 to 3.3 s, 13 textures, 8 buffers, texture bytes 15728640 and buffer bytes 1703936 at all 30 hours; lavapipe, three runs, 3.0 to 3.3 s, 8 buffers, texture bytes 16518784 and buffer bytes 957528 at all 30 hours, the texture count at -45 and skipped; Metal on `macos-latest` (run 37306688347), 3.1 s, 14 textures and 8 buffers at all 30 hours, both byte counters zero and not checked, private bytes +0.0 MiB. The kept cloud texture mutant fails the texture bytes from hour 21 once per update and from hour 24 once per eighth upload, a publication hour earlier than before.

**Decision: the e2e hidden-window case counts frames too** (`test_hidden_window_cloud_updates_do_not_grow_memory`). Its private-bytes limit was written for an RGBA frame of 8 MiB; at one channel the fixture's frame is 2 MiB, so a frame parked per update would grow 30 MiB over the fifteen updates and pass the 40 MiB limit. The case now asks `query-memory` after each batch of updates until `decoded_frames` reads zero, for at most 18 s after the last download, and fails on a frame still alive, which holds at any fixture size. The fixture stays 2048 x 1024 and the limit 40 MiB, as a backstop for memory outside a decoded frame: a 4096 x 2048 fixture, the validator's suggestion, would restore the limit's margin against a parked frame but add its own decode's allocator growth to a limit the i3 guest already runs close to (37.7 to 40.4 MiB, roadmap), and with the frames counted the size buys the case nothing. Measured on 2026-10-05: in the KDE X11 guest no decoded frame was alive after the four updates or after the fifteen, private bytes grew 15.4 MiB and fifteen cloud textures were made while hidden, the suite 17 passed; in the Windows guest none either, private bytes grew 0.1 MiB and fifteen textures were made while hidden, the suite 18 passed.

**Decision: count the downloads too** (2026-10-05; `Count the downloaded cloud images alive in the process`). The second review found what the private-bytes backstop reaches at this fixture. It compares hour 18 with hour 48, ten publications and thirty exports apart, so it fails a leak of more than about 6.4 MiB a publication or 2.1 MiB an export. What a cloud update allocates besides the counted frame is the download, 9758 bytes for the soak's fixture (the mutant below printed it), and the 384 KiB three-channel buffer the color decode goes through, so a change that kept either once per update would park under 4 MiB over the compared hours and pass. The soak this replaced would likely have failed a kept download, since a 4096 x 2048 gradient JPEG is about 0.45 MB (the review's estimate, not run). The user chose to count the downloads the way the frames are counted. `CloudImage::bytes` is a `cloud_source::Download`, which counts one buffer and its bytes (the capacity) while it lives, with the number made beside them; on the buffer and not as `Drop` on `CloudImage`, because `poll_once` moves the ETag and the date out of the image. Three choices beyond the task: the cache read in `post_cached` wraps the file it reads in a `Download` too, since it holds the same bytes for the same moment, so the count is every cloud JPEG body in memory whichever way it came; the count is a section of its own in `memory-report`, `cloud downloads:`, rather than a second line under `decoded pixels:`, which makes six sections and one more entry in the e2e `REPORT_SECTIONS`; and the `query-memory` line carries it as `downloads` and `download_bytes`. The cost is a few atomic operations per download, a few times an hour. The soak waits for the downloads after the frames and on their own, because a download outlives its frame: the poll that fetched it posts the frame and drops the download when it returns, so for a moment after the frame is gone the download is still alive. The 384 KiB decode buffer stays uncounted: it is a local of `decode_cloud_jpeg`, and the e2e hidden-window case's private-bytes limit catches it kept per update at that case's 2048 x 1024 fixture (6 MiB an update, 90 MiB over fifteen against 40). The mutant, applied, run on WARP and reverted, never committed: the download kept in a static once per publication fails the soak at hour 1, "simulated hour 1: 1 downloads (9758 bytes) were still alive 30s after the hour, with 1 of the 1 downloads so far made". Measured: WARP, three runs, 3.1 to 3.4 s, loop 1.4 s, the counters as before at all 30 hours, private bytes +0.0 to +0.1 MiB; lavapipe in WSL, three runs, 3.0 to 3.1 s, the counters as before, private bytes +0.1 MiB. The e2e suite, with the hidden-window case asserting on both counts: in the KDE X11 guest 17 passed, no frame and no download alive after the four updates or the fifteen, private bytes +16.5 MiB, fifteen cloud textures made while hidden; in the Windows guest 18 passed, none alive either, +0.9 MiB, fifteen textures.
