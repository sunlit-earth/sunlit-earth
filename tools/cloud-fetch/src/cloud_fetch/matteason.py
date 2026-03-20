"""Matteason cloud image fetcher.

Downloads the pre-composited equirectangular greyscale cloud JPEG from
clouds.matteason.co.uk and applies post-processing (levels, blur).
"""

import io
import urllib.request
from pathlib import Path

import numpy as np
from PIL import Image

from cloud_fetch.processing import apply_blur, apply_levels, float_to_uint8

SOURCE_URL = "https://clouds.matteason.co.uk/images/8192x4096/clouds.jpg"

DEFAULT_FLOOR = 50
DEFAULT_CEILING = 255
DEFAULT_GAMMA = 0.3
DEFAULT_BLUR_SIGMA = 0.0


def download_cloud_image(url: str = SOURCE_URL) -> bytes:
    """Download the cloud JPEG from the given URL.

    :param url: URL of the cloud image.
    :returns: Raw JPEG bytes.
    """
    req = urllib.request.Request(url, headers={"User-Agent": "SunlitEarth/1.0"})
    with urllib.request.urlopen(req, timeout=30) as resp:
        return resp.read()


def decode_grayscale(jpeg_data: bytes) -> np.ndarray:
    """Decode JPEG bytes into a 2D float32 array in [0, 1].

    :param jpeg_data: Raw JPEG bytes.
    :returns: 2D float32 array with values in [0, 1].
    """
    img = Image.open(io.BytesIO(jpeg_data)).convert("L")
    return np.asarray(img).astype(np.float32) / 255.0


def fetch_matteason(
    output_dir: Path,
    floor: int = DEFAULT_FLOOR,
    ceiling: int = DEFAULT_CEILING,
    gamma: float = DEFAULT_GAMMA,
    blur: float = DEFAULT_BLUR_SIGMA,
    raw: bool = False,
) -> Path:
    """Full matteason cloud fetch pipeline.

    Downloads the image, optionally saves the raw version, applies levels
    and blur, and saves the processed result as PNG.

    :param output_dir: Directory for output files (created if needed).
    :param floor: Levels black point (0--255).
    :param ceiling: Levels white point (0--255).
    :param gamma: Levels midtone gamma.
    :param blur: Gaussian blur sigma in pixels.
    :param raw: Whether to also save the unprocessed image.
    :returns: Path to the saved processed PNG.
    """
    output_dir.mkdir(parents=True, exist_ok=True)

    print(f"Downloading {SOURCE_URL}...")
    jpeg_data = download_cloud_image()
    print(f"  Downloaded {len(jpeg_data) / 1024:.0f} KB")

    arr = decode_grayscale(jpeg_data)
    print(f"  Image: {arr.shape[1]}x{arr.shape[0]}")

    if raw:
        raw_path = output_dir / "clouds_matteason_raw.png"
        Image.fromarray(float_to_uint8(arr), mode="L").save(str(raw_path))
        print(f"  Saved raw: {raw_path}")

    print("\nApplying levels adjustment...")
    arr = apply_levels(arr, floor, ceiling, gamma)
    print(
        f"  Levels: floor={floor}, ceiling={ceiling}, gamma={gamma} "
        f"(exponent={1.0 / gamma:.3f})"
    )
    nonzero = np.count_nonzero(arr > 0.01)
    print(f"  Pixels > 1%: {nonzero / arr.size * 100:.1f}%")

    if blur > 0:
        print(f"  Applying Gaussian blur (sigma={blur})...")
        arr = apply_blur(arr, sigma=blur)

    out_img = Image.fromarray(float_to_uint8(arr), mode="L")
    out_path = output_dir / "clouds_matteason.png"
    out_img.save(str(out_path))
    size_kb = out_path.stat().st_size / 1024
    print(f"\n  Saved {out_path} ({out_img.width}x{out_img.height}, {size_kb:.0f} KB)")

    return out_path
