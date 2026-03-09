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

## HDR rendering and tone mapping

The current lighting pipeline operates in LDR (low dynamic range): all color values are clamped to [0, 1] in the Rgba8UnormSrgb render target. This means light intensity above 1.0 simply clips, and there's no way to represent the full brightness range of a sunlit scene. The result looks dim because realistic sunlight intensity should be far above 1.0.

The proper solution is HDR rendering:

1. Render to a float16 intermediate framebuffer (e.g. Rgba16Float) where values can exceed 1.0
2. Apply lighting with physically realistic intensities (sun = 10–100+)
3. Run a tone mapping pass (Reinhard, ACES, etc.) that compresses the HDR values back to displayable [0, 1] range while preserving detail in both highlights and shadows

This is how virtually all modern 3D engines handle lighting. It requires a second render pass (fullscreen quad with the tone mapping shader), which adds some complexity but is well-understood.

Currently we use half-Lambert diffuse + a light multiplier above 1.0 as a pragmatic workaround. This works well enough for a single-texture globe but will become insufficient once we add day/night rendering, city lights, and atmosphere effects — all of which involve mixing very bright and very dark elements in the same frame.
