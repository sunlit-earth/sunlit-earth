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


