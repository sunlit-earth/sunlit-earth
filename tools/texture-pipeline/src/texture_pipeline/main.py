"""CLI interface for the texture pipeline."""

from pathlib import Path
from typing import Annotated

import typer
from PIL import Image
from rich.progress import (
    BarColumn,
    Progress,
    SpinnerColumn,
    TaskProgressColumn,
    TextColumn,
)

from texture_pipeline.discovery import compute_output_path, discover_images
from texture_pipeline.processing import downscale, encode_jxl, sharpen

# Disable Pillow's decompression bomb limit. NASA Blue Marble textures
# (e.g. 21600x10800 = 233M pixels) exceed the default ~178M pixel threshold.
# This is safe because this tool only processes known-safe local files.
Image.MAX_IMAGE_PIXELS = None

app = typer.Typer(
    name="texture-pipeline",
    help="Convert NASA Blue Marble textures to JPEG XL at multiple resolutions.",
    invoke_without_command=True,
)


@app.callback()
def main() -> None:
    """Convert NASA Blue Marble textures to JPEG XL at multiple resolutions."""


def run_pipeline(
    *,
    input_dir: Path,
    output_dir: Path,
    widths: list[int],
    quality: int,
    effort: int,
    do_sharpen: bool,
    ocean_shapefile: Path | None = None,
    ocean_color: tuple[int, int, int] = (10, 40, 80),
    ocean_supersample: int = 2,
    ocean_coast_offset: int = 0,
    ocean_preserve_ice: bool = True,
    ocean_ice_luminance: int = 200,
    ocean_ice_latitude: float = 60.0,
) -> None:
    """Execute the texture conversion pipeline.

    :param input_dir: Directory containing source textures.
    :param output_dir: Root directory for converted output.
    :param widths: List of target widths in pixels.
    :param quality: JPEG XL encoding quality (1--100).
    :param effort: JPEG XL encoding effort (1--9).
    :param do_sharpen: Whether to apply UnsharpMask after downscaling.
    :param ocean_shapefile: Path to ocean shapefile for masking, or None.
    :param ocean_color: RGB fill color for ocean regions.
    :param ocean_supersample: Supersampling factor for mask anti-aliasing.
    :param ocean_coast_offset: Signed coastline shift in pixels.
    :param ocean_preserve_ice: Whether to detect and preserve polar ice.
    :param ocean_ice_luminance: Seed luminance threshold for ice detection.
    :param ocean_ice_latitude: Minimum absolute latitude for polar gate.
    """
    images = discover_images(input_dir)

    if not images:
        typer.echo("No supported image files found in input directory.")
        return

    total_tasks = len(images) * len(widths)
    total_input_size = 0
    total_output_size = 0
    files_processed = 0

    with Progress(
        SpinnerColumn(),
        TextColumn("[progress.description]{task.description}"),
        BarColumn(),
        TaskProgressColumn(),
        TextColumn("{task.fields[current_file]}"),
    ) as progress:
        task = progress.add_task(
            "Processing textures...", total=total_tasks, current_file=""
        )

        for source_path in images:
            total_input_size += source_path.stat().st_size
            img = Image.open(source_path)

            # Ensure RGB mode for consistent processing
            if img.mode != "RGB":
                img = img.convert("RGB")

            if ocean_shapefile is not None:
                from texture_pipeline.ocean_masking import (
                    apply_ocean_mask,
                    detect_ice_regions,
                    get_or_create_mask,
                    reduce_mask_for_ice,
                )

                mask = get_or_create_mask(
                    ocean_shapefile,
                    img.size[0],
                    img.size[1],
                    supersample=ocean_supersample,
                    coast_offset=ocean_coast_offset,
                )

                if ocean_preserve_ice:
                    ice = detect_ice_regions(
                        img,
                        mask,
                        latitude_threshold=ocean_ice_latitude,
                        seed_luminance=ocean_ice_luminance,
                    )
                    mask = reduce_mask_for_ice(mask.copy(), ice)

                img = apply_ocean_mask(img, mask, color=ocean_color)
                # Embed ocean mask in the upper half of the alpha range:
                # land=255, ocean=128, coastlines smoothly between. Keeping all
                # alpha >= 128 prevents lossy JXL from degrading RGB on either side.
                # The shader remaps: water = saturate((1 - alpha) * 2).
                img.putalpha(Image.fromarray(255 - mask // 2, mode="L"))

            for width in widths:
                progress.update(task, current_file=f"{source_path.name} → {width}px")

                output_path = compute_output_path(
                    source_path, input_dir, output_dir, width
                )

                scaled = downscale(img, target_width=width)

                if do_sharpen:
                    scaled = sharpen(scaled)

                encode_jxl(scaled, output_path, quality=quality, effort=effort)

                total_output_size += output_path.stat().st_size
                files_processed += 1
                progress.advance(task)

    if ocean_shapefile is not None:
        from texture_pipeline.ocean_masking import clear_mask_cache

        clear_mask_cache()

    typer.echo(f"\nProcessed {files_processed} file(s).")
    typer.echo(f"Total input size:  {total_input_size / 1024 / 1024:.2f} MB")
    typer.echo(f"Total output size: {total_output_size / 1024 / 1024:.2f} MB")
    if total_input_size > 0:
        ratio = total_output_size / total_input_size
        typer.echo(f"Compression ratio: {ratio:.2f}x")


def _validate_quality(value: int) -> int:
    if value < 1 or value > 100:
        raise typer.BadParameter("Quality must be between 1 and 100.")
    return value


def _validate_effort(value: int) -> int:
    if value < 1 or value > 9:
        raise typer.BadParameter("Effort must be between 1 and 9.")
    return value


def _validate_width(value: int) -> int:
    if value <= 0:
        raise typer.BadParameter("Width must be a positive integer.")
    if value % 2 != 0:
        raise typer.BadParameter("Width must be even (to maintain 2:1 aspect ratio).")
    return value


def _validate_ocean_color(value: str) -> str:
    try:
        parts = [int(x) for x in value.split(",")]
    except ValueError as err:
        raise typer.BadParameter(
            "Ocean color must be three comma-separated integers (e.g. '10,40,80')."
        ) from err
    if len(parts) != 3:
        raise typer.BadParameter("Ocean color must have exactly 3 components (R,G,B).")
    if not all(0 <= c <= 255 for c in parts):
        raise typer.BadParameter("Each color component must be between 0 and 255.")
    return value


def _validate_ocean_supersample(value: int) -> int:
    if value < 1:
        raise typer.BadParameter("Ocean supersample must be at least 1.")
    return value


def _validate_ocean_coast_offset(value: int) -> int:
    if abs(value) > 50:
        raise typer.BadParameter("Coast offset magnitude must be at most 50.")
    return value


def _validate_ocean_ice_luminance(value: int) -> int:
    if value < 0 or value > 255:
        raise typer.BadParameter("Ice luminance must be between 0 and 255.")
    return value


def _validate_ocean_ice_latitude(value: float) -> float:
    if value < 0 or value > 90:
        raise typer.BadParameter("Ice latitude must be between 0 and 90.")
    return value


@app.command()
def convert(
    input_dir: Annotated[
        Path,
        typer.Option(
            "--input",
            "-i",
            help="Input directory containing source textures.",
            exists=True,
            file_okay=False,
            dir_okay=True,
            resolve_path=True,
        ),
    ],
    output_dir: Annotated[
        Path,
        typer.Option(
            "--output",
            "-o",
            help="Output directory for converted textures.",
            resolve_path=True,
        ),
    ],
    width: Annotated[
        list[int] | None,
        typer.Option(
            "--width",
            "-w",
            help="Target width(s) in pixels. Can be specified multiple times.",
        ),
    ] = None,
    quality: Annotated[
        int,
        typer.Option(
            "--quality",
            "-q",
            help="JPEG XL encoding quality (1-100).",
            callback=_validate_quality,
        ),
    ] = 85,
    effort: Annotated[
        int,
        typer.Option(
            "--effort",
            "-e",
            help="JPEG XL encoding effort (1-9). Higher = smaller file, slower.",
            callback=_validate_effort,
        ),
    ] = 7,
    do_sharpen: Annotated[
        bool,
        typer.Option(
            "--sharpen",
            help="Apply UnsharpMask sharpening after downscale.",
        ),
    ] = False,
    ocean_mask: Annotated[
        Path | None,
        typer.Option(
            "--ocean-mask",
            help="Path to ocean shapefile (.shp) for masking ocean pixels.",
            exists=True,
            dir_okay=False,
        ),
    ] = None,
    ocean_color: Annotated[
        str,
        typer.Option(
            "--ocean-color",
            help="Ocean fill color as R,G,B (e.g. '10,40,80').",
            callback=_validate_ocean_color,
        ),
    ] = "10,40,80",
    ocean_supersample: Annotated[
        int,
        typer.Option(
            "--ocean-supersample",
            help="Supersampling factor for ocean mask anti-aliasing (minimum 1).",
            callback=_validate_ocean_supersample,
        ),
    ] = 2,
    ocean_coast_offset: Annotated[
        int,
        typer.Option(
            "--ocean-coast-offset",
            help="Shift coastline by N px (source res). + = expand, - = erode.",
            callback=_validate_ocean_coast_offset,
        ),
    ] = 0,
    ocean_preserve_ice: Annotated[
        bool,
        typer.Option(
            "--ocean-preserve-ice/--no-ocean-preserve-ice",
            help="Detect and preserve ice regions in polar ocean areas.",
        ),
    ] = True,
    ocean_ice_luminance: Annotated[
        int,
        typer.Option(
            "--ocean-ice-luminance",
            help="Seed luminance threshold for ice detection (0-255).",
            callback=_validate_ocean_ice_luminance,
        ),
    ] = 200,
    ocean_ice_latitude: Annotated[
        float,
        typer.Option(
            "--ocean-ice-latitude",
            help="Minimum absolute latitude for polar ice gate (0-90).",
            callback=_validate_ocean_ice_latitude,
        ),
    ] = 60.0,
) -> None:
    """Convert source textures to JPEG XL at one or more target resolutions."""
    widths = width if width else [8192]

    # Validate each width
    for w in widths:
        _validate_width(w)

    output_dir.mkdir(parents=True, exist_ok=True)

    # Parse ocean color string into tuple
    color_tuple = tuple(int(x) for x in ocean_color.split(","))
    assert len(color_tuple) == 3  # guaranteed by validator

    run_pipeline(
        input_dir=input_dir,
        output_dir=output_dir,
        widths=widths,
        quality=quality,
        effort=effort,
        do_sharpen=do_sharpen,
        ocean_shapefile=ocean_mask,
        ocean_color=color_tuple,  # type: ignore[arg-type]
        ocean_supersample=ocean_supersample,
        ocean_coast_offset=ocean_coast_offset,
        ocean_preserve_ice=ocean_preserve_ice,
        ocean_ice_luminance=ocean_ice_luminance,
        ocean_ice_latitude=ocean_ice_latitude,
    )
