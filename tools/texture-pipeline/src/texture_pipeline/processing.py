"""Core image processing functions: downscale, sharpen, encode to JXL."""

from pathlib import Path

import pillow_jxl  # noqa: F401 - registers the JPEG XL plugin with Pillow
from PIL import Image, ImageFilter


def downscale(img: Image.Image, target_width: int) -> Image.Image:
    """Downscale an equirectangular image to a target width.

    Preserves the 2:1 aspect ratio.

    :param img: Source PIL image. Must have a 2:1 (width:height) aspect ratio.
    :param target_width: Desired output width in pixels.
    :returns: A new PIL image resized to (target_width, target_width // 2).
    :raises ValueError: If the source image does not have a 2:1 aspect ratio.
    :raises ValueError: If target_width is larger than the source width (upscaling).
    """
    src_width, src_height = img.size

    if src_width != src_height * 2:
        msg = f"Source image must have a 2:1 aspect ratio, got {src_width}x{src_height}"
        raise ValueError(msg)

    if target_width > src_width:
        msg = (
            f"Upscaling is not supported: target width {target_width} "
            f"exceeds source width {src_width}"
        )
        raise ValueError(msg)

    target_height = target_width // 2
    return img.resize((target_width, target_height), Image.Resampling.LANCZOS)


def sharpen(
    img: Image.Image,
    radius: float = 1.0,
    percent: int = 80,
    threshold: int = 3,
) -> Image.Image:
    """Apply UnsharpMask sharpening to an image.

    :param img: Source PIL image.
    :param radius: Size of the blur kernel.
    :param percent: Strength of the sharpening effect (0 = no effect).
    :param threshold: Minimum brightness difference to sharpen.
    :returns: A new PIL image with sharpening applied.
    """
    return img.filter(ImageFilter.UnsharpMask(radius, percent, threshold))


def encode_jxl(
    img: Image.Image,
    output_path: Path,
    quality: int = 85,
    effort: int = 7,
) -> None:
    """Encode a PIL image as JPEG XL.

    Never reconstructs a JPEG (``lossless_jpeg=False``), so that quality and
    effort take effect even when the source is one. Quality 100 asks the
    plugin for a lossless encode rather than for its highest lossy setting.

    :param img: Source PIL image.
    :param output_path: Path for the output ``.jxl`` file.
    :param quality: Encoding quality (1--100). Higher = better quality, larger file.
    :param effort: Encoding effort (1--9). Higher = smaller file, slower encode.
    """
    output_path.parent.mkdir(parents=True, exist_ok=True)
    img.save(
        output_path,
        quality=quality,
        effort=effort,
        lossless_jpeg=False,
        lossless=quality >= 100,
    )
