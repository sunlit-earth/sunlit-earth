"""CLI interface for the texture pipeline."""

import signal
import time
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
from texture_pipeline.exr import read_exr_rgb
from texture_pipeline.milky_way import MilkyWayParams, flux_gain, process
from texture_pipeline.processing import downscale, encode_jxl, sharpen

# Disable Pillow's decompression bomb limit. NASA Blue Marble textures
# (e.g. 21600x10800 = 233M pixels) exceed the default ~178M pixel threshold.
# This is safe because this tool only processes known-safe local files.
Image.MAX_IMAGE_PIXELS = None

app = typer.Typer(
    name="texture-pipeline",
    help="Convert source astronomy and Earth imagery into JPEG XL textures.",
    invoke_without_command=True,
)


@app.callback()
def main() -> None:
    """Convert source astronomy and Earth imagery into JPEG XL textures."""


def run_pipeline(
    *,
    input_dir: Path,
    output_dir: Path,
    widths: list[int],
    quality: int,
    effort: int,
    do_sharpen: bool,
    ocean_shapefile: Path | None = None,
    ocean_color: tuple[int, int, int] = (10, 30, 60),
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
        # Rich installs a SIGINT handler that swallows Ctrl+C. Reset to the OS
        # default so the process terminates immediately — the main thread is often
        # blocked inside C extensions (PIL, JXL) where KeyboardInterrupt can't
        # be delivered until the call returns.
        signal.signal(signal.SIGINT, signal.SIG_DFL)
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


def _validate_nonnegative(value: float) -> float:
    if value < 0:
        raise typer.BadParameter("Value must not be negative.")
    return value


def _validate_ocean_color(value: str) -> str:
    try:
        parts = [int(x) for x in value.split(",")]
    except ValueError as err:
        raise typer.BadParameter(
            "Ocean color must be three comma-separated integers (e.g. '10,30,60')."
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


@app.command("earth")
def earth(
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
            help="Ocean fill color as R,G,B (e.g. '10,30,60').",
            callback=_validate_ocean_color,
        ),
    ] = "10,30,60",
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
    """Convert the Earth's surface maps to JPEG XL at one or more widths.

    Takes a directory of Blue Marble and Black Marble style equirectangular
    images and writes one JPEG XL per source and target width.
    """
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


def run_milky_way(
    *,
    input_path: Path,
    output_path: Path,
    target_width: int,
    quality: int,
    effort: int,
    params: MilkyWayParams,
) -> None:
    """Denoise a NASA SVS star map EXR and write it as a JPEG XL panorama.

    :param input_path: The source ``.exr`` panorama, 2:1 and linear.
    :param output_path: Path for the output ``.jxl`` file.
    :param target_width: Output width in pixels. Must divide the source width.
    :param quality: JPEG XL quality (1--100); 100 means lossless.
    :param effort: JPEG XL encoding effort (1--9).
    :param params: The denoise thresholds and lengths.
    """
    started = time.monotonic()
    source = read_exr_rgb(input_path)
    source_height, source_width = source.shape[:2]
    typer.echo(f"Source:  {source_width}x{source_height}")
    typer.echo(f"Gain:    {flux_gain(source_width):g}x onto the 4096 grid")

    result = process(
        source,
        source_width=source_width,
        target_width=target_width,
        params=params,
    )
    del source

    encode_jxl(
        Image.fromarray(result.image),
        output_path,
        quality=quality,
        effort=effort,
    )

    encoding = "lossless" if quality >= 100 else f"quality {quality}"
    size_mb = output_path.stat().st_size / 1024 / 1024
    typer.echo(f"Target:  {target_width}x{target_width // 2}")
    typer.echo(
        f"Trimmed: {result.capped * 100:.2f}% of pixels capped, "
        f"{result.strong * 100:.2f}% strong-masked"
    )
    typer.echo(
        f"Output:  {output_path} ({size_mb:.2f} MB, {encoding}, effort {effort})"
    )
    typer.echo(f"Elapsed: {time.monotonic() - started:.1f}s")


@app.command("milky-way")
def milky_way(
    input_path: Annotated[
        Path,
        typer.Option(
            "--input",
            "-i",
            help="Source EXR panorama (NASA SVS Deep Star Maps).",
            exists=True,
            file_okay=True,
            dir_okay=False,
            resolve_path=True,
        ),
    ],
    output_path: Annotated[
        Path,
        typer.Option(
            "--output",
            "-o",
            help="Output .jxl file. Parent directories are created.",
            resolve_path=True,
        ),
    ],
    width: Annotated[
        int,
        typer.Option(
            "--width",
            "-w",
            help="Target width in pixels. Must divide the source width.",
            callback=_validate_width,
        ),
    ] = 8192,
    quality: Annotated[
        int,
        typer.Option(
            "--quality",
            "-q",
            help="JPEG XL quality (1-100). 100 encodes losslessly.",
            callback=_validate_quality,
        ),
    ] = 90,
    effort: Annotated[
        int,
        typer.Option(
            "--effort",
            "-e",
            help="JPEG XL encoding effort (1-9). Higher = smaller file, slower.",
            callback=_validate_effort,
        ),
    ] = 7,
    seed: Annotated[
        int,
        typer.Option(
            "--seed",
            help="Seed of the dither drawn during 8-bit quantization.",
        ),
    ] = 7,
    k: Annotated[
        float,
        typer.Option(
            "--k",
            help="Cap luminance at this multiple of the local background.",
            callback=_validate_nonnegative,
        ),
    ] = 3.0,
    strong: Annotated[
        float,
        typer.Option(
            "--strong",
            help="Replace a star's footprint above this multiple of the background.",
            callback=_validate_nonnegative,
        ),
    ] = 9.0,
    eps: Annotated[
        float,
        typer.Option(
            "--eps",
            help="Absolute margin on both thresholds, in linear units.",
            callback=_validate_nonnegative,
        ),
    ] = 0.0005,
    bg_sigma: Annotated[
        float,
        typer.Option(
            "--bg-sigma",
            help="Background Gaussian sigma, in arcminutes.",
            callback=_validate_nonnegative,
        ),
    ] = 15.8,
    dilate: Annotated[
        float,
        typer.Option(
            "--dilate",
            help="Radius grown around a strong star, in arcminutes.",
            callback=_validate_nonnegative,
        ),
    ] = 7.9,
    fill_sigma: Annotated[
        float,
        typer.Option(
            "--fill-sigma",
            help="Sigma of the fill that replaces a strong star, in arcminutes.",
            callback=_validate_nonnegative,
        ),
    ] = 7.9,
    blur: Annotated[
        float,
        typer.Option(
            "--blur",
            help="Final Gaussian sigma, in arcminutes.",
            callback=_validate_nonnegative,
        ),
    ] = 7.9,
) -> None:
    """Denoise a NASA SVS star map EXR into a JPEG XL panorama.

    The source holds flux per pixel, so its values are first lifted onto the
    4096 grid's scale, then box-averaged to the target width, despeckled,
    blurred and written as dithered 8-bit sRGB.
    """
    run_milky_way(
        input_path=input_path,
        output_path=output_path,
        target_width=width,
        quality=quality,
        effort=effort,
        params=MilkyWayParams(
            k=k,
            strong=strong,
            eps=eps,
            bg_sigma=bg_sigma,
            dilate=dilate,
            fill_sigma=fill_sigma,
            blur=blur,
            seed=seed,
        ),
    )
