"""Tests for the Rust golden suite's cube of the real Earth.

The fixture is the shipped cube at an eighth of its size: July's day faces,
the night faces and the water mask faces, each area-averaged from 2048 to 256
texels, so the orientation golden shows the real continents without needing
the Git LFS objects. Rerun with ``TEXTURE_PIPELINE_UPDATE_FIXTURES=1`` in a
checkout that has them to regenerate it after the shipped faces change.
"""

import os
from pathlib import Path

import numpy as np
import pillow_jxl  # noqa: F401 - registers the JPEG XL plugin with Pillow
import pytest
from PIL import Image

from texture_pipeline.cube import FACES, area_average
from texture_pipeline.processing import encode_jxl

REPOSITORY = Path(__file__).resolve().parents[3]
SHIPPED = REPOSITORY / "textures"
EARTH_FIXTURE = REPOSITORY / "crates" / "sunlit-core" / "tests" / "fixtures" / "earth"
EARTH_FACE = 256
EARTH_MONTH = "200407"
EARTH_QUALITY = 90
EARTH_SETS = (f"day/{EARTH_MONTH}", "night", "mask")
LFS_POINTER = b"version https://git-lfs"

# What a quality 90 encode moves a face from the texels it was given, with a
# margin: a mean of up to 1.9 and a 99.9th percentile of up to 24 when the
# fixture was made, where a mirrored or stale face is tens of steps off.
LOSSY_MEAN = 2.5
LOSSY_P999 = 32


def decode(path: Path) -> np.ndarray:
    """Decode a JPEG XL file into an array."""
    with Image.open(path) as image:
        return np.asarray(image)


def faces_of(root: Path) -> list[Path]:
    """Return every face of the fixture's sets under ``root``."""
    return [root / name / f"{face}.jxl" for name in EARTH_SETS for face in FACES]


def is_lfs_pointer(path: Path) -> bool:
    """Return whether ``path`` is a Git LFS pointer rather than its object."""
    with path.open("rb") as file:
        return file.read(len(LFS_POINTER)) == LFS_POINTER


def shrink_shipped_cube() -> dict[Path, np.ndarray]:
    """Area-average the shipped faces the fixture is made from.

    :returns: Each face's path relative to a textures directory, and its
        texels at the fixture's size.
    """
    return {
        path.relative_to(SHIPPED): area_average(decode(path), EARTH_FACE)
        for path in faces_of(SHIPPED)
    }


class TestEarthFixture:
    def test_every_face_is_there_at_the_fixture_size(self) -> None:
        for name in EARTH_SETS:
            names = sorted(p.name for p in (EARTH_FIXTURE / name).iterdir())
            assert names == sorted(f"{face}.jxl" for face in FACES)
        for path in faces_of(EARTH_FIXTURE):
            channels = () if path.parent.name == "mask" else (3,)
            assert decode(path).shape == (EARTH_FACE, EARTH_FACE, *channels), path

    def test_the_fixture_is_the_shipped_cube_shrunk(self) -> None:
        if any(is_lfs_pointer(path) for path in faces_of(SHIPPED)):
            pytest.skip("the shipped faces are Git LFS pointers in this checkout")
        shrunk = shrink_shipped_cube()
        if os.environ.get("TEXTURE_PIPELINE_UPDATE_FIXTURES"):
            for relative, texels in shrunk.items():
                lossless = relative.parts[0] == "mask"
                encode_jxl(
                    Image.fromarray(texels),
                    EARTH_FIXTURE / relative,
                    quality=100 if lossless else EARTH_QUALITY,
                )
        for relative, texels in shrunk.items():
            committed = decode(EARTH_FIXTURE / relative)
            if relative.parts[0] == "mask":
                np.testing.assert_array_equal(
                    committed,
                    texels,
                    err_msg=f"{relative} is not the shipped mask shrunk; rerun "
                    "with TEXTURE_PIPELINE_UPDATE_FIXTURES=1",
                )
                continue
            error = np.abs(committed.astype(np.int16) - texels.astype(np.int16))
            assert error.mean() <= LOSSY_MEAN, (
                f"{relative} is {error.mean():.2f} away from the shipped face shrunk "
                "on average; rerun with TEXTURE_PIPELINE_UPDATE_FIXTURES=1"
            )
            tail = np.percentile(error, 99.9)
            assert tail <= LOSSY_P999, (
                f"{relative} is {tail:.0f} away from the shipped face shrunk in its "
                "worst thousandth; rerun with TEXTURE_PIPELINE_UPDATE_FIXTURES=1"
            )
