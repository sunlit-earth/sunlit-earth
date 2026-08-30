"""Reading OpenEXR images as float32 RGB arrays."""

from pathlib import Path

import numpy as np
import OpenEXR

RGB_CHANNELS = ("R", "G", "B")


def read_exr_rgb(path: Path) -> np.ndarray:
    """Read the R, G and B channels of an OpenEXR file as float32.

    The channels are read one at a time, so a file that stores them at
    different types reads correctly and any other channel the file carries,
    alpha included, is ignored. Values keep the file's own linear units.

    :param path: Path to the ``.exr`` file.
    :returns: An ``(H, W, 3)`` float32 array.
    :raises ValueError: If the file has no R, G or B channel.
    :raises ValueError: If the three channels differ in size.
    """
    with OpenEXR.File(str(path), separate_channels=True) as source:
        channels = source.channels()
        missing = [name for name in RGB_CHANNELS if name not in channels]
        if missing:
            found = ", ".join(sorted(channels)) or "none"
            msg = (
                f"{path} has no {', '.join(missing)} channel (channels found: {found})"
            )
            raise ValueError(msg)

        planes = [channels[name].pixels for name in RGB_CHANNELS]
        shapes = {plane.shape for plane in planes}
        if len(shapes) != 1:
            sizes = ", ".join(
                f"{name}={plane.shape}"
                for name, plane in zip(RGB_CHANNELS, planes, strict=True)
            )
            msg = f"{path} has channels of different sizes: {sizes}"
            raise ValueError(msg)

        height, width = planes[0].shape
        rgb = np.empty((height, width, 3), dtype=np.float32)
        for index, plane in enumerate(planes):
            rgb[..., index] = plane

    return rgb
