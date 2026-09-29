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
