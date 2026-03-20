"""NOAA GMGSI satellite cloud imagery fetcher.

Downloads the Longwave IR (LW) band from the noaa-gmgsi-pds S3 bucket,
remaps it onto an equirectangular grid, applies levels adjustment to isolate
clouds from the surface background, upscales to 8K, and applies a mild
Gaussian blur.
"""

import tempfile
from datetime import UTC, datetime, timedelta
from pathlib import Path

import boto3
import numpy as np
from botocore import UNSIGNED
from botocore.config import Config
from PIL import Image

from cloud_fetch.processing import apply_blur, apply_levels, float_to_uint8

BUCKET = "noaa-gmgsi-pds"
BAND_PREFIX = "GMGSI_LW"

DEFAULT_FLOOR = 60
DEFAULT_CEILING = 215
DEFAULT_GAMMA = 0.7
DEFAULT_BLUR_SIGMA = 3.0
OUTPUT_WIDTH = 8192
OUTPUT_HEIGHT = 4096


def create_s3_client():
    """Create an anonymous S3 client for public bucket access.

    :returns: boto3 S3 client configured for unsigned requests.
    """
    return boto3.client("s3", config=Config(signature_version=UNSIGNED))


def find_latest_key(
    s3, date_str: str | None, hour_str: str | None
) -> tuple[str, str, str]:
    """Find the latest available S3 key for the GMGSI LW band.

    :param s3: boto3 S3 client.
    :param date_str: Optional date string (YYYYMMDD).
    :param hour_str: Optional UTC hour string (HH).
    :returns: Tuple of (s3_key, date_str, hour_str).
    :raises RuntimeError: If no data is found.
    """
    now = datetime.now(UTC)

    if date_str and hour_str:
        hours_to_try = [(date_str, hour_str)]
    elif date_str:
        hours_to_try = [(date_str, f"{h:02d}") for h in range(23, -1, -1)]
    else:
        hours_to_try = []
        for hours_back in range(25):
            t = now - timedelta(hours=hours_back)
            hours_to_try.append((t.strftime("%Y%m%d"), f"{t.hour:02d}"))

    for date, hour in hours_to_try:
        prefix = f"{BAND_PREFIX}/{date[:4]}/{date[4:6]}/{date[6:8]}/{hour}/"
        resp = s3.list_objects_v2(Bucket=BUCKET, Prefix=prefix, MaxKeys=5)
        contents = resp.get("Contents", [])
        nc_files = [obj for obj in contents if obj["Key"].endswith(".nc")]
        if nc_files:
            return nc_files[0]["Key"], date, hour

    raise RuntimeError(f"No {BAND_PREFIX} data found in the last 24 hours")


def download_netcdf(s3, key: str, dest: Path) -> Path:
    """Download an S3 object to a local file.

    :param s3: boto3 S3 client.
    :param key: S3 object key.
    :param dest: Local file path to save to.
    :returns: The *dest* path.
    """
    size_resp = s3.head_object(Bucket=BUCKET, Key=key)
    size_mb = size_resp["ContentLength"] / 1e6
    print(f"  Downloading {key} ({size_mb:.1f} MB)...")
    s3.download_file(BUCKET, key, str(dest))
    return dest


def read_gmgsi(path: Path) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    """Read a GMGSI NetCDF file and return (data, lat_2d, lon_2d).

    :param path: Path to a NetCDF file.
    :returns: Tuple of (data 2D float32, lat 2D float64, lon 2D float64).
    :raises RuntimeError: If the expected ``data`` variable is not found.
    """
    from netCDF4 import Dataset

    ds = Dataset(str(path), "r")

    if "data" not in ds.variables:
        available = list(ds.variables.keys())
        ds.close()
        raise RuntimeError(
            f"Expected variable 'data' not found. Available: {available}"
        )

    var = ds.variables["data"]
    print(f"  Using variable 'data': shape={var.shape}, dtype={var.dtype}")

    data = var[:].astype(np.float32)
    if data.ndim == 3 and data.shape[0] == 1:
        data = data[0]

    if hasattr(data, "mask"):
        n_masked = data.mask.sum()
        print(f"  Masked pixels: {n_masked:,} ({n_masked / data.size * 100:.1f}%)")
        data = np.where(data.mask, np.nan, data.data)

    lat = ds.variables["lat"][:].astype(np.float64)
    lon = ds.variables["lon"][:].astype(np.float64)
    if hasattr(lat, "mask"):
        lat = np.where(lat.mask, np.nan, lat.data)
    if hasattr(lon, "mask"):
        lon = np.where(lon.mask, np.nan, lon.data)

    print(f"  Data shape: {data.shape}")
    print(f"  Lat range: [{np.nanmin(lat):.2f}, {np.nanmax(lat):.2f}]")
    print(f"  Lon range: [{np.nanmin(lon):.2f}, {np.nanmax(lon):.2f}]")

    ds.close()
    return data, lat, lon


def remap_to_equirectangular(
    data: np.ndarray, lat_2d: np.ndarray, lon_2d: np.ndarray
) -> tuple[np.ndarray, np.ndarray]:
    """Remap irregularly-gridded data onto a regular equirectangular grid.

    Uses nearest-neighbor binning with gap-fill. The output covers the full
    globe (90N to 90S, -180 to +180) with the polar gap zero-filled.

    :param data: 2D float32 source data array.
    :param lat_2d: 2D float64 latitude array (same shape as *data*).
    :param lon_2d: 2D float64 longitude array (same shape as *data*).
    :returns: Tuple of (result 2D float32, counts 2D int32).
    """
    # Compute target resolution from median source grid spacing.
    lat_diff = np.abs(np.diff(lat_2d, axis=0))
    lon_diff = np.abs(np.diff(lon_2d, axis=1))
    median_lat_step = float(np.nanmedian(lat_diff[lat_diff > 0]))
    median_lon_step = float(np.nanmedian(lon_diff[lon_diff > 0]))
    target_res = max(median_lat_step, median_lon_step) * 1.5
    target_res = round(target_res, 3)
    print(
        f"  Source median spacing: lat={median_lat_step:.4f}, "
        f"lon={median_lon_step:.4f} deg"
    )

    nlat = round(180.0 / target_res)
    nlon = round(360.0 / target_res)
    print(f"  Target equirectangular grid: {nlon}x{nlat} ({target_res} deg/pixel)")

    # Flatten and filter invalid pixels
    flat_data = data.ravel()
    flat_lat = lat_2d.ravel()
    flat_lon = lon_2d.ravel()
    valid = np.isfinite(flat_data) & np.isfinite(flat_lat) & np.isfinite(flat_lon)
    flat_data = flat_data[valid]
    flat_lat = flat_lat[valid]
    flat_lon = flat_lon[valid]
    print(f"  Valid source pixels: {len(flat_data):,} of {data.size:,}")

    # Bin into target grid
    row = ((90.0 - flat_lat) / target_res).astype(np.int32)
    col = ((flat_lon + 180.0) / target_res).astype(np.int32)
    np.clip(row, 0, nlat - 1, out=row)
    np.clip(col, 0, nlon - 1, out=col)

    accum = np.zeros((nlat, nlon), dtype=np.float64)
    counts = np.zeros((nlat, nlon), dtype=np.int32)
    np.add.at(accum, (row, col), flat_data.astype(np.float64))
    np.add.at(counts, (row, col), 1)

    with np.errstate(divide="ignore", invalid="ignore"):
        result = np.where(counts > 0, accum / counts, 0.0).astype(np.float32)

    print(
        f"  Grid coverage after binning: "
        f"{np.count_nonzero(counts) / counts.size * 100:.1f}%"
    )

    # Fill small gaps by averaging neighbors
    gaps_before = np.count_nonzero(counts == 0)
    for _ in range(5):
        empty = counts == 0
        if not empty.any():
            break
        padded = np.pad(result, 1, mode="constant", constant_values=0)
        padded_c = np.pad(counts, 1, mode="constant", constant_values=0)
        neighbor_sum = np.zeros_like(result, dtype=np.float64)
        neighbor_cnt = np.zeros_like(counts)
        for dy in (-1, 0, 1):
            for dx in (-1, 0, 1):
                if dy == 0 and dx == 0:
                    continue
                neighbor_sum += padded[1 + dy : nlat + 1 + dy, 1 + dx : nlon + 1 + dx]
                neighbor_cnt += (
                    padded_c[1 + dy : nlat + 1 + dy, 1 + dx : nlon + 1 + dx] > 0
                ).astype(np.int32)
        fillable = empty & (neighbor_cnt >= 3)
        if not fillable.any():
            break
        with np.errstate(divide="ignore", invalid="ignore"):
            fill_vals = np.where(neighbor_cnt > 0, neighbor_sum / neighbor_cnt, 0)
        result = np.where(fillable, fill_vals.astype(np.float32), result)
        counts = np.where(fillable, 1, counts)

    gaps_after = np.count_nonzero(counts == 0)
    print(
        f"  Grid coverage after gap-fill: "
        f"{np.count_nonzero(counts > 0) / counts.size * 100:.1f}% "
        f"(filled {gaps_before - gaps_after:,} gaps)"
    )

    # Report polar gap
    first_filled = int(np.argmax(counts.any(axis=1)))
    last_filled = nlat - 1 - int(np.argmax(counts.any(axis=1)[::-1]))
    print(
        f"  Data latitude: ~{90.0 - first_filled * target_res:.1f}N "
        f"to ~{90.0 - (last_filled + 1) * target_res:.1f}S"
    )

    return result, counts


def upscale(data: np.ndarray, width: int, height: int) -> np.ndarray:
    """Upscale a 2D float array to the target dimensions using bicubic interpolation.

    :param data: 2D float32 array with values in [0, 1].
    :param width: Target width in pixels.
    :param height: Target height in pixels.
    :returns: 2D float32 array at the target resolution.
    """
    # Round-trip through uint8 before bicubic resize — matches original script.
    # Quantization loss is negligible since GMGSI source data is uint8 in the NetCDF.
    arr_u8 = float_to_uint8(data)
    img = Image.fromarray(arr_u8, mode="L")
    src_w, src_h = img.size
    print(f"  Upscaling from {src_w}x{src_h} to {width}x{height}...")
    img = img.resize((width, height), Image.BICUBIC)
    return np.asarray(img).astype(np.float32) / 255.0


def fetch_gmgsi(
    output_dir: Path,
    date: str | None = None,
    hour: str | None = None,
    floor: int = DEFAULT_FLOOR,
    ceiling: int = DEFAULT_CEILING,
    gamma: float = DEFAULT_GAMMA,
    blur: float = DEFAULT_BLUR_SIGMA,
    raw: bool = False,
) -> Path:
    """Full GMGSI cloud fetch pipeline.

    Downloads the latest GMGSI LW data, remaps to equirectangular,
    applies levels and blur, upscales to 8K, and saves as PNG.

    :param output_dir: Directory for output files (created if needed).
    :param date: Optional date string (YYYYMMDD).
    :param hour: Optional UTC hour string (HH).
    :param floor: Levels black point (0--255).
    :param ceiling: Levels white point (0--255).
    :param gamma: Levels midtone gamma.
    :param blur: Gaussian blur sigma at 8K output resolution.
    :param raw: Whether to also save the unprocessed image.
    :returns: Path to the saved processed PNG.
    """
    output_dir.mkdir(parents=True, exist_ok=True)

    s3 = create_s3_client()

    print("Searching for latest GMGSI LW data...")
    key, date_str, hour_str = find_latest_key(s3, date, hour)
    print(f"Found: {key}")

    with tempfile.TemporaryDirectory() as tmp_dir:
        nc_path = Path(tmp_dir) / "gmgsi_lw.nc"
        download_netcdf(s3, key, nc_path)

        # Read and normalize to 0-1
        print("\nReading data...")
        data, lat_2d, lon_2d = read_gmgsi(nc_path)
        data = np.nan_to_num(data, nan=0.0)
        norm = np.clip(data, 0, 255) / 255.0

        # Remap to equirectangular
        print("\nRemapping to equirectangular grid...")
        equirect, _counts = remap_to_equirectangular(norm, lat_2d, lon_2d)

        # Save raw if requested
        if raw:
            raw_path = output_dir / "clouds_lw_raw.png"
            Image.fromarray(float_to_uint8(equirect), mode="L").save(str(raw_path))
            print(f"\n  Saved raw: {raw_path}")

    # Apply levels adjustment
    print("\nApplying levels adjustment...")
    processed = apply_levels(equirect, floor, ceiling, gamma)
    print(
        f"  Levels: floor={floor}, ceiling={ceiling}, gamma={gamma} "
        f"(exponent={1.0 / gamma:.3f})"
    )
    print(f"  Output range: [{processed.min():.3f}, {processed.max():.3f}]")
    nonzero = np.count_nonzero(processed > 0.01)
    print(f"  Pixels > 1%: {nonzero / processed.size * 100:.1f}%")

    # Upscale to 8K
    print("\nUpscaling...")
    upscaled = upscale(processed, OUTPUT_WIDTH, OUTPUT_HEIGHT)

    # Blur
    if blur > 0:
        print(f"  Applying Gaussian blur (sigma={blur})...")
        upscaled = apply_blur(upscaled, sigma=blur)

    # Save
    out_img = Image.fromarray(float_to_uint8(upscaled), mode="L")
    out_path = output_dir / "clouds_lw.png"
    out_img.save(str(out_path))
    size_kb = out_path.stat().st_size / 1024
    print(f"\n  Saved {out_path} ({out_img.width}x{out_img.height}, {size_kb:.0f} KB)")
    print(f"\nDone! {date_str} {hour_str}:00 UTC -> {out_path}")

    return out_path
