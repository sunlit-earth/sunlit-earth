# Textures, asset tools, and notices

## Fetching textures

The runtime texture files in `textures/` are stored with Git LFS. A clone without LFS objects contains small pointer files instead of images. From the checkout, fetch the assets with:

```bash
git lfs install
git lfs pull
```

The app uses these files:

- `world.topo.200405.jxl` supplies the Earth daytime surface at 8K.
- `BlackMarble_2016.jxl` supplies the Earth night lights at 8K.
- `lroc_color_poles_1k.jxl` supplies the Moon's surface.
- `milkyway_2020_4k.jxl` supplies the Milky Way panorama.

[PROVENANCE.md](../textures/PROVENANCE.md) records their sources, hashes, and preparation details. Missing Moon and Milky Way files omit those overlays without blocking the Earth render.

The app selects the first existing texture directory in this order:

1. The `--textures-dir` argument.
2. The `SUNLIT_EARTH_TEXTURES` environment variable.
3. `textures/` relative to the current working directory.
4. `textures/` beside the executable, then in its ancestor directories.

Without Earth textures, the renderer can show a procedural grid for basic pipeline checks. Current startup code filters out files smaller than 64 KiB, including LFS pointers. Older versions attempted to decode those pointers and could delay a headless export until its timeout. Fetch the real LFS objects if an older build reports JPEG XL decode errors. For an intentional run without assets, select an empty directory with `--textures-dir`.

A debug build logs `resolved texture paths` at startup. Release builds compile info level logging out, so use a debug build when investigating asset discovery.

## Resolution and caching

Advanced → Rendering selects a maximum texture width of 8192, 4096, or 2048. The default is 4096, including configurations saved before this setting existed. Selecting another width in the UI persists it. A source narrower than the selected width is not enlarged.

Downscaled Earth textures are cached under `texture_cache/` in the cache directory. Halving each dimension reduces texture pixel storage to about a quarter of the original size and makes subsequent loads faster. The cache is disposable and can be regenerated. See [architecture.md](architecture.md#texture-resolution) for validation, measurements, and loading behavior.

Cloud imagery downloads at runtime from [Matteason](https://clouds.matteason.co.uk) and is cached. The same resolution setting selects the cloud image size, reducing download size and texture memory at lower resolutions. Existing clouds remain visible while the new size downloads or when connectivity is unavailable. Setting `SUNLIT_EARTH_NO_CLOUDS` disables cloud fetching entirely. `SUNLIT_EARTH_CLOUD_URL` overrides the selected provider URL. All location and polling overrides are listed in [architecture.md](architecture.md#environment-knobs).

## Preparing assets

The committed icon, star catalog, and dependency notices can be regenerated from the repository root:

```bash
cargo xtask bake icon
cargo xtask bake icon --review DIR
cargo xtask bake stars --input hyg_v44.csv --output crates/sunlit-core/src/assets/stars/hyg_v4_4_mag7.bin
cargo xtask bake licenses
```

The icon bake writes `assets/icon/baked/`; the review option produces a contact sheet for checking small icon sizes. See [app-icon.md](app-icon.md) for the artwork and [rendering.md](rendering.md) for the star catalog pipeline.

Two standalone Python tools under `tools/` use uv and are separate from the Rust build:

- [texture-pipeline](../tools/texture-pipeline/README.md) prepares imagery, including NASA Blue Marble sources, as JPEG XL assets at different resolutions.
- [cloud-fetch](../tools/cloud-fetch/README.md) fetches and processes cloud imagery from the Matteason composite or NOAA GMGSI.

## Licenses and attribution

The application uses [GPL-3.0-or-later](../LICENSE), as declared in the workspace manifest. Slint uses its GPLv3 licensing option, and Astronomy Engine is MIT licensed.

- [assets/ATTRIBUTION.md](../assets/ATTRIBUTION.md) contains the credits displayed in the About window's attributions tab.
- [assets/third-party.md](../assets/third-party.md) contains the generated dependency list displayed in the About window.
- [assets/THIRD-PARTY-LICENSES.md](../assets/THIRD-PARTY-LICENSES.md) contains dependency licenses and their full texts from the committed SPDX texts in `assets/licenses/`. This file ships beside the executable in a complete release bundle.
- [textures/PROVENANCE.md](../textures/PROVENANCE.md) records image sources, file hashes, and download dates.
- The [star catalog notice](../crates/sunlit-core/src/assets/stars/ATTRIBUTION.md) lives beside the committed catalog blob.

`cargo xtask bake licenses` regenerates both dependency notice files. See [the release bundle documentation](vm-setup.md#the-release-bundle) for archive contents.
