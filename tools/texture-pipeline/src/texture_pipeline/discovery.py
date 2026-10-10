"""File discovery and output path construction for the texture pipeline."""

import re
from pathlib import Path

SUPPORTED_EXTENSIONS: set[str] = {".jpg", ".jpeg", ".png", ".tif", ".tiff"}


def discover_images(input_dir: Path) -> list[Path]:
    """Recursively find all supported image files under *input_dir*.

    Matching is case-insensitive on the file extension.

    :param input_dir: Root directory to search.
    :returns: Sorted list of paths to image files.
    """
    results: list[Path] = []
    for path in input_dir.rglob("*"):
        if path.is_file() and path.suffix.lower() in SUPPORTED_EXTENSIONS:
            results.append(path)
    return sorted(results)


def compute_output_path(
    source_path: Path,
    input_root: Path,
    output_root: Path,
    width: int,
) -> Path:
    """Compute the output path for a processed image.

    The output mirrors the input directory structure under a width-specific
    subdirectory, with the extension changed to ``.jxl``.

    :param source_path: Path to the source image file.
    :param input_root: Root of the input directory tree.
    :param output_root: Root of the output directory tree.
    :param width: Target width (used as subdirectory name).
    :returns: Output path like ``output_root / str(width) / relative / name.jxl``.
    """
    relative = source_path.relative_to(input_root)
    return output_root / str(width) / relative.with_suffix(".jxl")


def discover_months(input_dir: Path) -> dict[str, Path]:
    """Find the monthly maps in *input_dir*, keyed by their ``YYYYMM`` stamp.

    The stamp is the six-digit component between dots that NASA's names carry,
    as in ``world.topo.200405.3x21600x10800.jpg``. The directory is not
    searched recursively.

    :param input_dir: Directory holding one image per month.
    :returns: Paths keyed by stamp, in stamp order.
    :raises ValueError: If an image carries no stamp, a stamp names no month,
        or two images carry the same stamp.
    """
    months: dict[str, Path] = {}
    for path in sorted(input_dir.iterdir()):
        if not path.is_file() or path.suffix.lower() not in SUPPORTED_EXTENSIONS:
            continue
        match = re.search(r"\.(\d{4})(\d{2})\.", path.name)
        if match is None:
            msg = f"{path.name} carries no month stamp such as .200405."
            raise ValueError(msg)
        if not 1 <= int(match[2]) <= 12:
            msg = f"{path.name}: {match[2]} is not a month"
            raise ValueError(msg)
        stamp = match[1] + match[2]
        if stamp in months:
            msg = f"{months[stamp].name} and {path.name} both carry {stamp}"
            raise ValueError(msg)
        months[stamp] = path
    return dict(sorted(months.items()))
