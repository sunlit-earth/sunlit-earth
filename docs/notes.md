# Notes

## Memory usage at high resolutions

MSAA 8× on a 4K display uses 600+ MB in render textures alone. On low-RAM machines with large displays this could crash the system.

Needs proper research before picking a solution. Possible directions:

- Tiled rendering: render the scene in smaller tiles and stitch together
- Progressive rendering: render at lower resolution first, refine when idle
- Resolution cap with smart upscaling
- Reduce MSAA automatically based on available memory
- wgpu memory budget APIs (if any exist)
- How other wallpaper engines handle this (e.g., Wallpaper Engine, Lively)

## CI format check

The `fmt` job in `.github/workflows/ci.yml` is commented out. The codebase was formatted with an older rustfmt and the current stable rustfmt (1.8.0) wants to reformat ~19 files. To enable it:

1. Add a `rustfmt.toml` with the desired `style_edition` (or accept the 2024 default)
2. Run `cargo fmt` to reformat everything
3. Commit the reformatting
4. Uncomment the `fmt` job in `ci.yml`

