"""The look of the sea ice: its coverage and color, generated on a polar cap.

A monthly mean at 25 km says how much of the sea is ice, not what the ice
looks like. What a satellite shows at the day faces' 5 km is three things,
and the generator makes those and nothing finer:

- The edge, where the concentration falls to open water: filaments and eddies
  drawn along it. Coverage is a soft step of the concentration against a
  threshold field, fractal noise stretched along the edge and swirled by a
  divergence-free shift, so that half the area of a cell at 50% is ice, in
  streaks.
- The interior: bright, slightly blue white, crossed by a sparse network of
  darker lines, leads, denser where the concentration falls below 85%, with a
  broad brightness noise so the pack does not read as a fill.
- The season: one color per hemisphere and month, with melt mottling in the
  Arctic summer.

Every noise field is seeded and fixed on the cap's plane, the same in every
month, so a change of month moves the edge and leaves the interior where it
was. Scales are in kilometers on the plane, which the equal-area projection
keeps isotropic to within 3.5% at 60 degrees and 13% at the cap's edge; a
feature's size is half the wavelength of the noise that draws it.
"""

import dataclasses
from collections.abc import Callable, Iterator
from concurrent.futures import Executor
from dataclasses import dataclass, field

import numpy as np
from scipy import fft
from scipy.ndimage import (
    distance_transform_edt,
    gaussian_filter,
    map_coordinates,
    zoom,
)
from scipy.spatial import cKDTree
from scipy.special import ndtr

from texture_pipeline.cube import face_directions
from texture_pipeline.sea_ice import (
    AUTHALIC_RADIUS_KM,
    HEMISPHERES,
    RHO_MAX,
    SOURCE_CELL_KM,
    Hemisphere,
    cap_concentration,
    directions_to_cap,
    plane_to_texel,
    texel_km,
)

type Rgb = tuple[int, int, int]

_ARCTIC: tuple[Rgb, ...] = (
    (240, 243, 247),
    (240, 243, 247),
    (240, 243, 247),
    (240, 243, 247),
    (234, 238, 243),
    (220, 225, 231),
    (207, 213, 218),
    (205, 211, 216),
    (222, 229, 233),
    (233, 238, 242),
    (238, 242, 246),
    (240, 243, 247),
)

_ANTARCTIC: tuple[Rgb, ...] = (
    (212, 216, 220),
    (214, 218, 221),
    (218, 222, 226),
    (226, 231, 235),
    (226, 231, 235),
    (226, 231, 235),
    (226, 231, 235),
    (226, 231, 235),
    (226, 231, 235),
    (226, 231, 235),
    (222, 227, 231),
    (216, 220, 224),
)


@dataclass(frozen=True)
class Look:
    """The generator's tunables, chosen by eye against MODIS true color."""

    seed: int = 2004
    """Seeds every noise field."""

    edge_band_km: tuple[float, float] = (10.0, 120.0)
    """Feature sizes of the threshold noise, smallest first."""

    edge_streak_km: float = 250.0
    """Length of the average along the edge that stretches the noise."""

    edge_isotropic: float = 0.2
    """Share of unstretched noise of the same band in the threshold."""

    orientation_km: float = 100.0
    """Smoothing of the edge's direction field."""

    edge_center: float = 0.5
    """Concentration at which half the area is drawn as ice."""

    edge_spread: float = 0.38
    """Half the range of the threshold around its center."""

    edge_softness: float = 0.08
    """Half the concentration step over which coverage goes from 0 to 1."""

    edge_pack: float | None = None
    """When set, the concentration above which the edge model draws all ice:
    its threshold is squeezed from its range into the range's low end to this,
    so its streaks of open water stay at the ice edge and leave the pack. The
    open water the edge then draws too little of is the floes'."""

    eddy_band_km: tuple[float, float] = (50.0, 150.0)
    """Feature sizes of the swirl that shifts the concentration."""

    eddy_shift_km: float = 30.0
    """Root mean square of that shift."""

    thin_shade: float = 0.82
    """Brightness of the ice where the concentration is at the threshold's
    low end, rising to 1 at its high end."""

    lead_cell_km: float = 80.0
    """Across-side of the cells whose borders are the leads."""

    lead_aspect: float = 3.0
    """How much longer the cells are along their long axis."""

    lead_bend_km: float = 20.0
    """Root mean square of the shift that bends the leads."""

    lead_bend_band_km: tuple[float, float] = (300.0, 800.0)
    """Feature sizes of that shift."""

    lead_width_km: float = 3.0
    """Gaussian half width of a lead's darkening."""

    lead_darkening: float = 0.12
    """Darkening at the middle of a lead."""

    lead_share_closed: float = 0.08
    """Share of cell borders drawn as leads where the concentration is 1."""

    lead_share_open: float = 0.4
    """Share drawn where it is 0.85 or less."""

    brightness_band_km: tuple[float, float] = (60.0, 300.0)
    """Feature sizes of the broad brightness noise."""

    brightness_amplitude: float = 0.02
    """Standard deviation of the broad brightness noise."""

    mottle_band_km: tuple[float, float] = (5.0, 20.0)
    """Feature sizes of the melt mottling."""

    mottle_amplitude: float = 0.05
    """Standard deviation of the mottling at full melt."""

    mottle_months: tuple[float, ...] = (0, 0, 0, 0, 0.3, 1, 1, 1, 0.3, 0, 0, 0)
    """How much of the mottling each month of the Arctic gets."""

    rough_amplitude: float = 0.08
    """Standard deviation of the texture of broken ice, on the mottling's
    noise, where the concentration is at or below ``rough_range[0]``."""

    rough_range: tuple[float, float] = (0.7, 0.95)
    """Concentrations over which the broken ice's texture fades out."""

    colors: dict[str, tuple[Rgb, ...]] = field(
        default_factory=lambda: {"nh": _ARCTIC, "sh": _ANTARCTIC}
    )
    """The ice's color per hemisphere and month, in the day faces' sRGB."""

    crumple: bool = False
    """Whether the interior is drawn as crumpled snow, its structure varied by
    an activity field, in place of the broad brightness noise, the texture of
    broken ice and the melt mottling."""

    activity_band_km: tuple[float, float] = (300.0, 1500.0)
    """Feature sizes of the noise that decides where the pack is structured."""

    activity_range: tuple[float, float] = (-0.3, 1.0)
    """Values of that noise, standard deviation 1, over which the activity
    rises from flat to fully structured."""

    activity_floor: float = 0.1
    """The least activity, so that flat pack is almost, not quite, flat."""

    activity_edge: float = 0.6
    """How far the activity rises toward full where the concentration falls."""

    activity_edge_range: tuple[float, float] = (0.98, 0.75)
    """Concentrations over which that rise goes from none to full."""

    crumple_octaves: tuple[tuple[float, float, float, float], ...] = (
        (5.0, 10.0, 0.3, 0.3),
        (10.0, 20.0, 0.6, 1.0),
        (20.0, 40.0, 1.0, 0.6),
        (40.0, 80.0, 0.8, 0.0),
    )
    """Per octave of the crumple's ridged noise: its smallest and largest
    feature size, the weight of its ridges in the height that is shaded, and
    the weight of its creases."""

    crumple_aspect: float = 3.0
    """How much longer the crumple's features are along the direction field."""

    crumple_turn_km: tuple[float, float] = (800.0, 3000.0)
    """Feature sizes of the noise the direction field turns with."""

    crumple_warp_km: float = 6.0
    """Root mean square of the shift that warps the crumple."""

    crumple_warp_band_km: tuple[float, float] = (30.0, 120.0)
    """Feature sizes of that shift."""

    crumple_gate: float = 0.6
    """How much a finer octave is confined to the coarser one's ridges."""

    crease_width: float = 0.4
    """Values of an octave's noise, standard deviation 1, either side of zero
    that its crease spans."""

    crumple_facets: tuple[tuple[float, float, float], ...] = (
        (30.0, 0.6, 0.6),
        (120.0, 1.0, 0.2),
    )
    """Per level of the plates the snow breaks into: the cells' size, the
    weight of their shades and the weight of the creases along their
    borders."""

    facet_crease_km: float = 2.0
    """Half width of the crease along a plate's border."""

    crumple_shadow: float = 0.06
    """Darkening of the crumple's facets turned away from the light, per
    standard deviation of their slope, at full activity."""

    crumple_light: float = 0.02
    """Brightening of the facets turned toward it."""

    crumple_crease: float = 0.07
    """Darkening at the middle of a crease, at full activity."""

    crumple_plates: float = 0.015
    """Standard deviation of the plates' shades, at full activity."""

    plates_flat: float = 0.25
    """Share of the plates' shades that flat pack keeps."""

    crumple_folds: bool = False
    """Whether the crumple's shading and creases are straight folds in every
    direction, as in a sheet of paper crumpled and flattened again, in place
    of the ridged noise's."""

    fold_spacing_km: float = 30.0
    """Mean distance between the folds' centers."""

    fold_length_km: tuple[float, float] = (25.0, 120.0)
    """Range of a fold's length; lengths are spread log-uniformly over it."""

    fold_shade_km: float = 10.0
    """Distance from a fold over which its step in shade fades."""

    fold_line_km: float = 1.2
    """Gaussian half width of the crease along a fold."""

    fold_ridges: float = 0.25
    """Share of the ridged noise's shading and creases kept beside the
    folds'."""

    fractures: bool = False
    """Whether the leads are long fracture systems in place of the borders of
    a cell pattern; open water in them shows as the sea's fill."""

    fracture_systems: tuple[int, int] = (150, 70)
    """Fracture systems seeded on the north's and the south's cap."""

    fracture_length_km: tuple[float, float] = (150.0, 800.0)
    """Range of a system's main fracture's length; lengths are spread
    log-uniformly over it."""

    fracture_turn_km: tuple[float, float] = (1500.0, 6000.0)
    """Feature sizes of the noise the fractures' direction field turns with."""

    fracture_conjugate: tuple[float, float] = (0.35, 0.6)
    """Range of the angle, either side, between a system's fractures and the
    direction field: fractures form along two conjugate sets that cross, and
    a branch runs along the other set from its parent's."""

    fracture_share: tuple[float, float] = (1.0, 1.0)
    """Share of the systems drawn where the concentration is 1 and where it
    is 0.85 or less."""

    fracture_follow: float = 0.02
    """How far a fracture turns toward the field's direction per step."""

    fracture_wander: float = 0.012
    """Standard deviation of a fracture's own turn per step, in radians."""

    fracture_step_km: float = 2.5
    """Length of a fracture's segments."""

    fracture_branching_km: float = 400.0
    """Mean distance along a fracture between branches."""

    fracture_branch_angle: tuple[float, float] = (0.6, 1.1)
    """Range of a branch's angle off its parent, in radians."""

    fracture_width_km: tuple[float, float] = (1.5, 5.0)
    """Range of a fracture's width; a branch is narrower than its parent."""

    fracture_darkness: tuple[float, float] = (0.1, 0.35)
    """Range of the share of the sea's fill in a fracture's middle: grey
    refrozen ice at the low end, open water at the high end."""

    fracture_activity: float = 0.5
    """Seeding density where the activity is 0, against 1 where it is 1."""

    fracture_open_share: float = 0.12
    """Share of the systems that are fresh open water, darker than
    :attr:`fracture_darkness`'s refrozen leads."""

    fracture_open_darkness: tuple[float, float] = (0.45, 0.8)
    """Range of the share of the sea's fill in an open system's middle."""

    fracture_bend: float = 0.22
    """Standard deviation, in radians, of a fracture's heading about its
    course, a smooth noise along it, so it wanders over tens of kilometers."""

    fracture_bend_km: float = 16.0
    """Gaussian sigma, along the fracture, of the noise that bends it."""

    fracture_dark_vary: float = 0.35
    """Standard deviation of a fracture's darkness along it, relative to its
    own, from a smooth noise over the width's swell."""

    floes: bool = False
    """Whether the marginal ice breaks into floes with water between them."""

    floe_cells_km: tuple[float, float] = (20.0, 10.0)
    """Sizes of the large floes and of the small ones nearer the edge."""

    floe_keep: float = 0.35
    """Share of the large floes' grid points kept, at random, so their sizes
    vary, around a mean of the cell over the share's square root."""

    floe_ramp: float = 0.15
    """Range of the opening level over which a floe goes from closed to its
    full gap. Each floe opens at a level of its own, uniform from 0 to 1, so
    at a low share of water a few floes stand apart, filled, in their water,
    while the rest hold together as larger floes of irregular shape."""

    floe_width: tuple[float, float] = (0.08, 0.32)
    """Range of a floe's full gap, in cells from its border; a floe whose gap
    reaches its middle is gone."""

    floe_cluster: float = 0.75
    """Weight of a smooth noise in the level at which a floe opens, against
    the floe's own: floes open together in loose regions and hold together in
    compact ones."""

    floe_cluster_km: tuple[float, float] = (60.0, 250.0)
    """Feature sizes of that noise."""

    floe_open: tuple[float, float] = (0.95, 0.8)
    """Concentrations over which the floes go from taking none to all of the
    open water the edge model leaves: what the concentration leaves open less
    what the edge's streaks draw."""

    floe_small: tuple[float, float] = (0.7, 0.35)
    """Concentrations over which the small floes' share of that water rises
    from none to :attr:`floe_small_share`."""

    floe_small_share: float = 0.6
    """The small floes' largest share of the water between floes."""

    floe_darkness: tuple[float, float] = (0.45, 0.95)
    """Share of the sea's fill in the water between the floes: grey new ice
    where the concentration is high, open water toward the edge."""

    floe_dark_range: tuple[float, float] = (0.9, 0.6)
    """Concentrations over which that share goes from the first to the
    second."""

    floe_gap: float = 0.45
    """The largest share of a texel's area between floes. The water is the
    share of the area times its darkness, so grey gaps are wider."""

    floe_fast: float = 0.03
    """Rise of the concentration by the fast ice band above which no floes
    are drawn: the band closes the coast."""

    floe_warp_km: float = 3.0
    """Root mean square of the shift that bends the floes' borders."""

    floe_warp_band_km: tuple[float, float] = (8.0, 40.0)
    """Feature sizes of that shift."""

    fast_ice: bool = False
    """Whether the record's concentration next to its land is replaced by the
    one further out where that is high: a band of fast ice."""

    fast_ice_km: tuple[float, float] = (25.0, 50.0)
    """Distances from the record's land over which its concentration goes from
    unreliable, replaced in full, to trusted."""

    fast_ice_offshore_km: tuple[float, float] = (50.0, 100.0)
    """Distances from the record's land the concentration is carried in from."""

    fast_ice_closed: tuple[float, float] = (0.8, 0.9)
    """Concentrations offshore over which the band goes from none to full."""

    melt_tone: bool = False
    """Whether the Arctic summer's melt is a smooth regional tone, with the
    crumple, in place of the mottling."""

    melt_band_km: tuple[float, float] = (50.0, 300.0)
    """Feature sizes of the melt's tone."""

    melt_color: Rgb = (150, 175, 200)
    """The bluish grey a region of much melt leans toward."""

    melt_amplitude: float = 0.5
    """How far a region of the most melt leans from the month's color toward
    :attr:`melt_color`, at full melt. The tone is the noise through a soft
    step from its mean to two standard deviations above it, less its mean, so
    most of the pack stays near the month's color, a few regions lean well
    toward the melt color, and the month's color stays the mean."""

    coast_blend: bool = False
    """Whether the ice's color and texture reach onto flat snow along the
    coast, so that the coastline does not show under closed ice."""

    coast_reach_km: tuple[float, float] = (6.0, 18.0)
    """Distances inland over which the blend falls from full to none."""

    coast_inner_km: tuple[float, float] = (2.0, 6.0)
    """Land nearer the water than these is not read for snow and relief, since
    it holds the imagery's own coastline and the fronts of ice shelves: the
    nearer where the window holds no land beyond the farther, on small
    islands and narrow peninsulas, the farther elsewhere."""

    coast_window_km: float = 8.0
    """Gaussian sigma of the surroundings snow and relief are read over."""

    coast_snow: tuple[float, float] = (190.0, 0.2)
    """Least luminance and largest saturation of a pixel of snow."""

    coast_snow_share: tuple[float, float] = (0.5, 0.9)
    """Shares of snow in the surroundings over which the blend rises from
    none to full."""

    coast_sea: tuple[float, float] = (60.0, 8.0)
    """Largest luminance and least blue above red of the imagery's sea: on the
    mask's land along the coast it is not read as land, and it takes what the
    sea beside it shows, the fill under the ice's coverage."""

    coast_relief: tuple[float, float] = (6.0, 10.0)
    """Standard deviations of the snow's luminance in the surroundings over
    which the land goes from flat, full blend, to mountainous, none."""


LOOK = Look()
"""The tunables the bake uses."""

BAND_ROWS = 512
"""Cap rows worked at once, which bounds the float64 intermediates."""


def smoothstep(
    edge0: float | np.ndarray, edge1: float | np.ndarray, x: np.ndarray
) -> np.ndarray:
    """Hermite step from 0 at ``edge0`` to 1 at ``edge1``, as in GLSL.

    :param edge0: Where the step starts.
    :param edge1: Where it ends; may be below ``edge0`` for a falling step.
    :param x: The values stepped.
    :returns: float32 array of the shape of ``x``.
    """
    t = np.clip((x - edge0) / (edge1 - edge0), 0.0, 1.0)
    return (t * t * (3 - 2 * t)).astype(np.float32)


def _rng(look: Look, hemisphere: str, stream: int) -> np.random.Generator:
    return np.random.default_rng([look.seed, HEMISPHERES.index(hemisphere), stream])


def normalized(field: np.ndarray) -> np.ndarray:
    """Scale a zero-mean field to a standard deviation of 1, in place.

    A field without variance stays zero.

    :param field: float32 array.
    :returns: ``field``.
    """
    spread = float(field.std())
    if spread > 0:
        field /= spread
    return field


def band_noise(
    size: int,
    band_km: tuple[float, float],
    rng: np.random.Generator,
) -> np.ndarray:
    """Return periodic Gaussian noise with equal energy in every octave of a band.

    White noise is shaped in the frequency domain to an amplitude of ``1/k``
    between the band's frequencies, with Gaussian falloffs in log frequency on
    either side, and scaled to a standard deviation of 1. A band whose largest
    feature is finer than a texel is zero everywhere, since the cap cannot draw
    it and its falloff alone would be scaled up to texel noise. The white noise
    is drawn either way, so the generator is left where a drawn band leaves it.

    :param size: Texels along each side of the cap.
    :param band_km: The smallest and largest feature size, half a wavelength.
    :param rng: The generator of the white noise.
    :returns: float32 array of shape ``(size, size)``.
    """
    spacing = texel_km(size)
    white = rng.standard_normal((size, size), dtype=np.float32)
    if band_km[1] < spacing:
        return np.zeros((size, size), dtype=np.float32)
    spectrum = fft.rfft2(white, workers=-1)
    del white
    fy = fft.fftfreq(size, d=spacing).astype(np.float32)
    fx = fft.rfftfreq(size, d=spacing).astype(np.float32)
    k = np.hypot(fy[:, None], fx[None, :])
    k[0, 0] = 1.0
    low, high = 1 / (2 * band_km[1]), 1 / (2 * band_km[0])
    log_k = np.log(k)
    outside = np.maximum(np.log(low) - log_k, 0) + np.maximum(log_k - np.log(high), 0)
    gain = np.exp(-0.5 * (outside / 0.35) ** 2) / k
    gain[0, 0] = 0.0
    spectrum *= gain
    del gain, log_k, outside, k
    noise = fft.irfft2(spectrum, s=(size, size), workers=-1).astype(np.float32)
    return normalized(noise)


def rows(size: int) -> Iterator[tuple[int, int]]:
    """Yield the bands of :data:`BAND_ROWS` rows of a cap."""
    for start in range(0, size, BAND_ROWS):
        yield start, min(start + BAND_ROWS, size)


def edge_orientation(concentration: np.ndarray, size: int, look: Look) -> np.ndarray:
    """Return the doubled angle of the edge's direction at every texel of a cap.

    The direction is the minor axis of the structure tensor of every month's
    concentration together, smoothed over :attr:`Look.orientation_km`, so a
    place takes its direction from the months whose edge passed it and from
    its surroundings where none did.

    :param concentration: float array ``(12, n, n)`` on a coarser cap.
    :param size: Texels along each side of the cap the result is for.
    :param look: The tunables.
    :returns: float32 array ``(2, size, size)``: ``cos`` and ``sin`` of twice
        the angle of the gradient's axis, row first.
    """
    coarse = concentration.shape[-1]
    sigma = look.orientation_km / texel_km(coarse)
    jrr = np.zeros((coarse, coarse), dtype=np.float64)
    jrc = np.zeros_like(jrr)
    jcc = np.zeros_like(jrr)
    for month in concentration:
        gr, gc = np.gradient(gaussian_filter(month.astype(np.float64), sigma / 2))
        jrr += gr * gr
        jrc += gr * gc
        jcc += gc * gc
    a = gaussian_filter(jrr - jcc, sigma)
    b = gaussian_filter(2 * jrc, sigma)
    norm = np.hypot(a, b) + 1e-12
    doubled = np.stack([a / norm, b / norm])
    factor = size / coarse
    return np.stack(
        [zoom(d, factor, order=1, mode="nearest", grid_mode=True) for d in doubled]
    ).astype(np.float32)


def streak(
    noise: np.ndarray, doubled: np.ndarray, length_km: float, finest_km: float
) -> np.ndarray:
    """Average a noise field along the edge's direction at every texel.

    The samples along the line are spaced at most half the noise's smallest
    feature apart, so the average does not alias its finest streaks into
    ladders.

    :param noise: float32 array ``(size, size)``, periodic.
    :param doubled: The doubled angle of the gradient's axis, as
        :func:`edge_orientation` returns it.
    :param length_km: About the length of a streak: four Gaussian sigmas.
    :param finest_km: The noise's smallest feature size.
    :returns: float32 array of the noise's shape, standard deviation 1.
    """
    size = noise.shape[0]
    spacing = texel_km(size)
    sigma = length_km / 4 / spacing
    reach = int(np.ceil(2 * sigma))
    step = max(0.5, min(sigma / 4, finest_km / spacing / 2))
    offsets = np.arange(-reach, reach + step / 2, step)
    weights = np.exp(-0.5 * (offsets / sigma) ** 2)
    out = np.empty_like(noise)
    for start, stop in rows(size):
        angle = 0.5 * np.arctan2(doubled[1, start:stop], doubled[0, start:stop])
        tr, tc = -np.sin(angle), np.cos(angle)
        r = np.arange(start, stop, dtype=np.float32)[:, None]
        c = np.arange(size, dtype=np.float32)[None, :]
        acc = np.zeros((stop - start, size), dtype=np.float32)
        for offset, weight in zip(offsets, weights, strict=True):
            acc += weight * map_coordinates(
                noise,
                [r + offset * tr, c + offset * tc],
                order=1,
                mode="grid-wrap",
            )
        out[start:stop] = acc
    return normalized(out)


def curl_shift(
    size: int, band_km: tuple[float, float], rms_km: float, rng: np.random.Generator
) -> np.ndarray:
    """Return a divergence-free shift field in texels.

    :param size: Texels along each side of the cap.
    :param band_km: Feature sizes of the stream function.
    :param rms_km: Root mean square of the shift's length.
    :param rng: The generator of the stream function's noise.
    :returns: float32 array ``(2, size, size)``, row shift first.
    """
    stream = band_noise(size, band_km, rng)
    gr, gc = np.gradient(stream)
    del stream
    shift = np.stack([gc, -gr]).astype(np.float32)
    rms = float(np.sqrt(np.mean(gr * gr + gc * gc)))
    shift *= rms_km / texel_km(size) / rms
    return shift


def _pair_rank(a: np.ndarray, b: np.ndarray) -> np.ndarray:
    low = np.minimum(a, b).astype(np.uint64)
    high = np.maximum(a, b).astype(np.uint64)
    h = low * np.uint64(0x9E3779B97F4A7C15) ^ high * np.uint64(0xC2B2AE3D27D4EB4F)
    h ^= h >> np.uint64(31)
    h *= np.uint64(0xBF58476D1CE4E5B9)
    h ^= h >> np.uint64(27)
    return ((h >> np.uint64(40)).astype(np.float64) / float(1 << 24)).astype(np.float32)


@dataclass(frozen=True)
class Leads:
    """The cell borders a lead may follow."""

    profile: np.ndarray
    """float16 darkening profile from 0 to 1, 1 on a border."""

    rank: np.ndarray
    """float16 rank of each texel's nearest border, uniform from 0 to 1: a
    border is drawn where its rank is below the share drawn there."""


def leads(size: int, look: Look, rng: np.random.Generator) -> Leads:
    """Return the borders of elongated, bent cellular noise.

    Feature points are jittered on a grid of cells :attr:`Look.lead_aspect`
    times longer along the columns than along the rows, and the plane is bent
    by a smooth shift before the nearest two are found, so the borders form a
    network of long, roughly oriented, slightly curved lines.

    :param size: Texels along each side of the cap.
    :param look: The tunables.
    :param rng: The generator of the points and the bend.
    :returns: Each texel's distance profile and its border's rank.
    """
    spacing = texel_km(size)
    cell_r = look.lead_cell_km / spacing
    cell_c = cell_r * look.lead_aspect
    count_r = int(np.ceil(size / cell_r))
    count_c = int(np.ceil(size / cell_c))
    points = rng.random((2, count_r, count_c), dtype=np.float32)
    bend = curl_shift(size, look.lead_bend_band_km, look.lead_bend_km, rng)
    profile = np.empty((size, size), dtype=np.float16)
    rank = np.empty((size, size), dtype=np.float16)
    width = look.lead_width_km / spacing
    for start, stop in rows(size):
        r = np.arange(start, stop, dtype=np.float32)[:, None] + bend[0, start:stop]
        c = np.arange(size, dtype=np.float32)[None, :] + bend[1, start:stop]
        qr = r / cell_r
        qc = c / cell_c
        base_r = np.floor(qr).astype(np.int64)
        base_c = np.floor(qc).astype(np.int64)
        nearest = NearestPoints(qr.shape)
        for dr in (-1, 0, 1):
            for dc in (-1, 0, 1):
                jr = base_r + dr
                jc = base_c + dc
                wr = jr % count_r
                wc = jc % count_c
                pr = jr + points[0, wr, wc]
                pc = jc + points[1, wr, wc]
                nearest.offer(
                    (qr - pr) ** 2 + (qc - pc) ** 2, wr * count_c + wc, pr, pc
                )
        nr = nearest.row[1] - nearest.row[0]
        nc = nearest.col[1] - nearest.col[0]
        apart = np.hypot(nr, nc)
        border = (nearest.distance[1] - nearest.distance[0]) / (2 * apart)
        across = np.hypot(nr / apart / cell_r, nc / apart / cell_c)
        distance = border / across
        profile[start:stop] = np.exp(-((distance / width) ** 2))
        rank[start:stop] = _pair_rank(nearest.cell[0], nearest.cell[1])
    return Leads(profile, rank)


class NearestPoints:
    """The nearest two feature points of every texel seen so far."""

    def __init__(self, shape: tuple[int, ...]) -> None:
        self.distance = [np.full(shape, np.inf, dtype=np.float32) for _ in range(2)]
        self.cell = [np.zeros(shape, dtype=np.int64) for _ in range(2)]
        self.row = [np.zeros(shape, dtype=np.float32) for _ in range(2)]
        self.col = [np.zeros(shape, dtype=np.float32) for _ in range(2)]

    def offer(
        self, distance: np.ndarray, cell: np.ndarray, row: np.ndarray, col: np.ndarray
    ) -> None:
        """Take a candidate point where it is nearer than the nearest two.

        :param distance: Squared distance of the candidate from each texel.
        :param cell: The candidate's cell index.
        :param row: The candidate's row in cell units.
        :param col: The candidate's column in cell units.
        """
        first = distance < self.distance[0]
        second = ~first & (distance < self.distance[1])
        for values, candidate in (
            (self.distance, distance),
            (self.cell, cell),
            (self.row, row),
            (self.col, col),
        ):
            values[1] = np.where(
                first, values[0], np.where(second, candidate, values[1])
            )
            values[0] = np.where(first, candidate, values[0])


def oriented_noise(
    size: int,
    band_km: tuple[float, float],
    angle: float,
    aspect: float,
    rng: np.random.Generator,
) -> np.ndarray:
    """Return periodic band noise whose features are longer along a direction.

    As :func:`band_noise`, with the frequency along the direction counted
    ``aspect`` times, so a feature is ``aspect`` times longer along it than
    across it.

    :param size: Texels along each side of the cap.
    :param band_km: The smallest and largest feature size across.
    :param angle: The direction, radians from the columns toward the rows.
    :param aspect: How much longer the features are along it.
    :param rng: The generator of the white noise.
    :returns: float32 array ``(size, size)``, standard deviation 1.
    """
    spacing = texel_km(size)
    white = rng.standard_normal((size, size), dtype=np.float32)
    spectrum = fft.rfft2(white, workers=-1)
    del white
    fy = fft.fftfreq(size, d=spacing).astype(np.float32)[:, None]
    fx = fft.rfftfreq(size, d=spacing).astype(np.float32)[None, :]
    along = (fx * np.float32(np.cos(angle)) + fy * np.float32(np.sin(angle))) * (
        np.float32(aspect)
    )
    across = fy * np.float32(np.cos(angle)) - fx * np.float32(np.sin(angle))
    k = np.hypot(along, across)
    del along, across
    k[0, 0] = 1.0
    low, high = 1 / (2 * band_km[1]), 1 / (2 * band_km[0])
    log_k = np.log(k)
    outside = np.maximum(np.log(low) - log_k, 0) + np.maximum(log_k - np.log(high), 0)
    gain = np.exp(-0.5 * (outside / 0.35) ** 2) / k
    gain[0, 0] = 0.0
    del log_k, outside, k
    spectrum *= gain
    del gain
    return normalized(fft.irfft2(spectrum, s=(size, size), workers=-1))


def steered_noise(
    size: int,
    band_km: tuple[float, float],
    doubled: np.ndarray,
    aspect: float,
    rng: np.random.Generator,
) -> np.ndarray:
    """Return band noise stretched along a slowly turning direction field.

    Noise stretched along four directions, 45 degrees apart, is blended by
    how close each is to the field's direction, the two nearest at most.

    :param size: Texels along each side of the cap.
    :param band_km: The smallest and largest feature size across.
    :param doubled: float32 array ``(2, size, size)``: cosine and sine of
        twice the field's angle, as :func:`oriented_noise` measures it.
    :param aspect: How much longer the features are along the field.
    :param rng: The generator of the white noise.
    :returns: float32 array ``(size, size)``, about standard deviation 1.
    """
    out = np.zeros((size, size), dtype=np.float32)
    weights = np.zeros((size, size), dtype=np.float32)
    for step in range(4):
        angle = step * np.pi / 4
        weight = np.maximum(
            doubled[0] * np.float32(np.cos(2 * angle))
            + doubled[1] * np.float32(np.sin(2 * angle)),
            0,
        )
        out += weight * oriented_noise(size, band_km, angle, aspect, rng)
        weights += weight * weight
    out /= np.sqrt(np.maximum(weights, 1e-6))
    return out


def warped(field: np.ndarray, shift: np.ndarray) -> np.ndarray:
    """Read a periodic field through a shift.

    :param field: float32 array ``(size, size)``.
    :param shift: float32 array ``(2, size, size)`` in texels, row first.
    :returns: float32 array of the field's shape.
    """
    size = field.shape[0]
    out = np.empty_like(field)
    for start, stop in rows(size):
        r = np.arange(start, stop, dtype=np.float32)[:, None] + shift[0, start:stop]
        c = np.arange(size, dtype=np.float32)[None, :] + shift[1, start:stop]
        out[start:stop] = map_coordinates(field, [r, c], order=1, mode="grid-wrap")
    return out


def activity_field(size: int, look: Look, rng: np.random.Generator) -> np.ndarray:
    """Return where the pack is flat and where it is structured.

    :param size: Texels along each side of the cap.
    :param look: The tunables.
    :param rng: The generator of the noise.
    :returns: float16 array ``(size, size)`` from 0, flat, to 1.
    """
    noise = band_noise(size, look.activity_band_km, rng)
    return smoothstep(*look.activity_range, noise).astype(np.float16)


def facet_cells(
    size: int,
    cell_km: float,
    shift: np.ndarray,
    rng: np.random.Generator,
    keep: float = 1.0,
) -> tuple[np.ndarray, np.ndarray]:
    """Return plates: cells around points jittered on a grid, read through a
    shift so their borders bend.

    :param size: Texels along each side of the cap.
    :param cell_km: The grid's spacing.
    :param shift: float32 array ``(2, size, size)`` in texels, row first.
    :param rng: The generator of the points and the shades.
    :param keep: The share of the points kept, at random, so the cells' sizes
        vary more than a jittered grid's.
    :returns: float32 arrays ``(size, size)``: each texel's plate's shade,
        standard normal, and its distance to the plate's border in texels.
    """
    cell = cell_km / texel_km(size)
    count = int(np.ceil(size / cell)) + 2
    grid = np.stack(np.mgrid[-1 : count - 1, -1 : count - 1], axis=-1).reshape(-1, 2)
    points = (grid + rng.random(grid.shape)) * cell
    if keep < 1:
        points = points[rng.random(len(points)) < keep]
    shades = rng.standard_normal(len(points)).astype(np.float32)
    tree = cKDTree(points)
    shade = np.empty((size, size), dtype=np.float32)
    border = np.empty((size, size), dtype=np.float32)
    for start, stop in rows(size):
        r = np.arange(start, stop, dtype=np.float32)[:, None] + shift[0, start:stop]
        c = np.arange(size, dtype=np.float32)[None, :] + shift[1, start:stop]
        query = np.stack([r.ravel(), c.ravel()], axis=-1)
        distance, index = tree.query(query, k=2, workers=-1)
        apart = np.linalg.norm(points[index[:, 1]] - points[index[:, 0]], axis=-1)
        to_border = (distance[:, 1] ** 2 - distance[:, 0] ** 2) / (2 * apart)
        shade[start:stop] = shades[index[:, 0]].reshape(r.shape)
        border[start:stop] = to_border.reshape(r.shape)
    return shade, border


FOLD_CHUNK = 65536
"""Fold pieces drawn at once, which bounds the intermediates."""


def folds(
    size: int, look: Look, rng: np.random.Generator
) -> tuple[np.ndarray, np.ndarray]:
    """Draw straight folds at random places, directions and lengths.

    Each fold is cut into pieces about a texel long, and every texel within
    reach of a piece, beside it rather than past its ends, takes the fold's
    step in shade, its own sign either side and fading over
    :attr:`Look.fold_shade_km`, and its crease. Both taper toward the fold's
    ends. Folds wrap around the cap's edges, as the noise does.

    :param size: Texels along each side of the cap.
    :param look: The tunables.
    :param rng: The generator of the folds.
    :returns: float32 arrays ``(size, size)``: the shade, standard deviation
        1, and the creases, from 0 to about 1.
    """
    spacing = texel_km(size)
    count = round((size * spacing / look.fold_spacing_km) ** 2)
    centers = rng.random((count, 2)) * size
    angles = rng.uniform(0, np.pi, count)
    low, high = np.log(look.fold_length_km[0]), np.log(look.fold_length_km[1])
    lengths = np.exp(rng.uniform(low, high, count)) / spacing
    amplitudes = rng.standard_normal(count)
    pieces = np.maximum(np.ceil(lengths), 1).astype(np.int64)
    owner = np.repeat(np.arange(count), pieces)
    index = np.arange(owner.size) - np.repeat(np.cumsum(pieces) - pieces, pieces)
    t = (index + 0.5) / pieces[owner]
    piece = lengths[owner] / pieces[owner]
    along = np.stack([np.sin(angles), np.cos(angles)], axis=-1)[owner]
    middle = centers[owner] + ((t - 0.5) * lengths[owner])[:, None] * along
    taper = np.sin(np.pi * t) ** 0.5
    amplitude = amplitudes[owner] * taper
    shade_width = look.fold_shade_km / spacing
    line_width = look.fold_line_km / spacing
    reach = int(np.ceil(2.5 * shade_width))
    shade = np.zeros(size * size, dtype=np.float64)
    line = np.zeros(size * size, dtype=np.float64)
    offsets = np.arange(-reach, reach + 1)
    for start in range(0, owner.size, FOLD_CHUNK):
        stop = min(start + FOLD_CHUNK, owner.size)
        base = np.floor(middle[start:stop]).astype(np.int64)
        half = piece[start:stop] / 2
        texels, shades, lines = [], [], []
        for dr in offsets:
            for dc in offsets:
                cell = base + np.array([dr, dc])
                rel = cell + 0.5 - middle[start:stop]
                a = (rel * along[start:stop]).sum(axis=1)
                d = rel[:, 0] * along[start:stop, 1] - rel[:, 1] * along[start:stop, 0]
                beside = (np.abs(d) < 2.5 * shade_width) & (-half <= a) & (a < half)
                d = d[beside]
                fade = np.exp(-np.abs(d) / shade_width) * smoothstep(
                    2.5 * shade_width, 1.5 * shade_width, np.abs(d)
                )
                texels.append((cell[beside, 0] % size) * size + cell[beside, 1] % size)
                shades.append(np.sign(d) * fade * amplitude[start:stop][beside])
                lines.append(
                    np.exp(-((d / line_width) ** 2)) * taper[start:stop][beside]
                )
        flat = np.concatenate(texels)
        shade += np.bincount(flat, np.concatenate(shades), minlength=size * size)
        line += np.bincount(flat, np.concatenate(lines), minlength=size * size)
    return (
        normalized(shade.reshape(size, size).astype(np.float32)),
        np.clip(line.reshape(size, size), 0, 1).astype(np.float32),
    )


def crumple(
    size: int, look: Look, rng: np.random.Generator
) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    """Return the crumpled snow: the slope of its height, its creases and its
    plates.

    Each octave is band noise stretched along a slowly turning direction field
    and warped by a smooth shift, folded at zero: ``(1 - |n|)**2`` is a ridge
    along every zero crossing, and its middle a crease. Octaves are added
    coarsest first, each finer one confined toward the coarser one's ridges by
    :attr:`Look.crumple_gate`, so the creases branch off one another and
    enclose facets. The height is shaded by its slope toward a fixed light.

    :param size: Texels along each side of the cap.
    :param look: The tunables.
    :param rng: The generator of the noise.
    The snow also breaks into plates, cells of :attr:`Look.crumple_facets`
    each with a shade of its own and creases along their borders.

    :returns: float16 arrays ``(size, size)``: the slope toward the light,
        standard deviation 1, the creases, from 0 to about 1, and the plates'
        shade, standard deviation 1.
    """
    angle = band_noise(size, look.crumple_turn_km, rng) * np.float32(np.pi / 2)
    doubled = np.stack([np.cos(2 * angle), np.sin(2 * angle)]).astype(np.float32)
    del angle
    shift = curl_shift(size, look.crumple_warp_band_km, look.crumple_warp_km, rng)
    height = np.zeros((size, size), dtype=np.float32)
    creases = np.zeros((size, size), dtype=np.float32)
    coarser = np.ones((size, size), dtype=np.float32)
    for low, high, ridge_weight, crease_weight in sorted(
        look.crumple_octaves, reverse=True
    ):
        noise = warped(
            steered_noise(size, (low, high), doubled, look.crumple_aspect, rng), shift
        )
        folded = np.abs(noise)
        del noise
        gate = np.float32(1 - look.crumple_gate) + np.float32(look.crumple_gate) * (
            coarser
        )
        ridge = np.clip(1 - folded, 0, 1) ** 2
        height += np.float32(ridge_weight) * ridge * gate
        crease = np.clip(1 - folded / np.float32(look.crease_width), 0, 1) ** 2
        creases += np.float32(crease_weight) * crease * gate
        coarser = ridge
        del folded, gate, crease
    del doubled, coarser
    gr, gc = np.gradient(height)
    del height
    slope = normalized(np.float32(np.sqrt(0.5)) * (gr + gc))
    del gr, gc
    plates = np.zeros((size, size), dtype=np.float32)
    width = look.facet_crease_km / texel_km(size)
    for cell_km, shade_weight, crease_weight in look.crumple_facets:
        shade, border = facet_cells(size, cell_km, shift, rng)
        plates += np.float32(shade_weight) * shade
        creases += np.float32(crease_weight) * np.clip(1 - border / width, 0, 1) ** 2
        del shade, border
    del shift
    weights = [o[3] for o in look.crumple_octaves] + [f[2] for f in look.crumple_facets]
    creases /= np.float32(max(sum(weights), 1e-6))
    if look.crumple_folds:
        shade, line = folds(size, look, rng)
        keep = np.float32(look.fold_ridges)
        slope = normalized(keep * slope + (1 - keep) * shade)
        creases = keep * creases + (1 - keep) * line
        del shade, line
    return (
        slope.astype(np.float16),
        creases.astype(np.float16),
        normalized(plates).astype(np.float16),
    )


@dataclass(frozen=True)
class Fractures:
    """The fracture systems drawn on the cap."""

    open: np.ndarray
    """float16 share of the sea's fill each texel's fracture shows, its
    darkness times how much of the texel it covers."""

    rank: np.ndarray
    """float16 rank of that fracture's system, uniform from 0 to 1: a system
    is drawn where its rank is below the share drawn there."""

    systems: np.ndarray
    """float32 array ``(n, 3)``: each system's length in kilometers, branches
    included, its number of branches and its darkness."""


def _wrap(angle: float) -> float:
    return (angle + np.pi) % (2 * np.pi) - np.pi


def fracture_paths(
    size: int,
    seeds: np.ndarray,
    direction: np.ndarray,
    look: Look,
    rng: np.random.Generator,
) -> list[list[tuple[np.ndarray, float, float]]]:
    """Trace fracture systems from seeds along a direction field.

    A main fracture runs both ways from its seed, half its length each way;
    each step it turns part of the way toward the field's direction, either
    way along it, and by a little of its own. Branches leave it at random
    distances, at an angle to one side, and run a share of what is left of
    their parent.

    :param size: Texels along each side of the cap.
    :param seeds: float array ``(n, 2)`` of row and column.
    :param direction: float32 array ``(size, size)``: the field's angle,
        radians from the columns toward the rows.
    :param look: The tunables.
    :param rng: The generator of lengths, turns and branches.
    :returns: Per system, its fractures: points ``(k, 2)`` in texels, the
        width in texels and the darkness.
    """
    spacing = texel_km(size)
    step = look.fracture_step_km / spacing
    low, high = np.log(look.fracture_length_km[0]), np.log(look.fracture_length_km[1])
    branch_chance = look.fracture_step_km / look.fracture_branching_km

    bend_steps = look.fracture_bend_km / look.fracture_step_km

    def bends(steps: int) -> np.ndarray:
        if look.fracture_bend <= 0:
            return np.zeros(steps)
        span = int(np.ceil(3 * bend_steps))
        kernel = np.exp(-0.5 * (np.arange(-span, span + 1) / bend_steps) ** 2)
        noise = np.convolve(rng.standard_normal(steps + 2 * span), kernel, "valid")
        return look.fracture_bend * noise / max(float(np.sqrt((kernel**2).sum())), 1e-9)

    def trace(
        start: np.ndarray, heading: float, steps: int, offset: float
    ) -> np.ndarray:
        points = [start.copy()]
        point = start.astype(np.float64)
        bend = bends(steps)
        for number in range(steps):
            r = int(np.clip(point[0], 0, size - 1))
            c = int(np.clip(point[1], 0, size - 1))
            field = float(direction[r, c]) + offset
            turn = _wrap(field - heading)
            if abs(turn) > np.pi / 2:
                turn = _wrap(field + np.pi - heading)
            heading += look.fracture_follow * turn + rng.normal(0, look.fracture_wander)
            course = heading + bend[number]
            point = point + step * np.array([np.sin(course), np.cos(course)])
            if not (0 <= point[0] < size and 0 <= point[1] < size):
                break
            points.append(point.copy())
        return np.array(points)

    systems = []
    for seed in seeds:
        length = np.exp(rng.uniform(low, high)) / look.fracture_step_km
        width = rng.uniform(*look.fracture_width_km) / spacing
        darkness = rng.uniform(*look.fracture_darkness)
        if look.fracture_open_share > 0 and rng.random() < look.fracture_open_share:
            darkness = rng.uniform(*look.fracture_open_darkness)
        offset = rng.choice([-1.0, 1.0]) * rng.uniform(*look.fracture_conjugate)
        heading = float(direction[int(seed[0]), int(seed[1])]) + offset
        halves = [
            trace(seed, heading, int(length / 2), offset),
            trace(seed, heading + np.pi, int(length / 2), offset),
        ]
        main = np.concatenate([halves[1][::-1], halves[0][1:]])
        fractures = [(main, width, darkness)]
        pending = [(main, width, darkness, 0, offset)]
        while pending:
            path, w, dark, depth, parent_offset = pending.pop()
            if depth >= 1 or len(path) < 8:
                continue
            for index in np.nonzero(rng.random(len(path)) < branch_chance)[0]:
                if index + 2 >= len(path):
                    continue
                along = path[index + 1] - path[index]
                parent = float(np.arctan2(along[0], along[1]))
                side = -np.sign(parent_offset)
                angle = parent + side * rng.uniform(*look.fracture_branch_angle)
                left = len(path) - index
                steps = int(left * rng.uniform(0.2, 0.5))
                branch = trace(path[index], angle, steps, -parent_offset)
                if len(branch) < 4:
                    continue
                fractures.append((branch, w * 0.7, dark * 0.85))
                pending.append(
                    (branch, w * 0.7, dark * 0.85, depth + 1, -parent_offset)
                )
        systems.append(fractures)
    return systems


def draw_fractures(
    size: int,
    systems: list[list[tuple[np.ndarray, float, float]]],
    ranks: np.ndarray,
    rng: np.random.Generator,
    swell_km: float = 60.0,
    dark_vary: float = 0.0,
) -> tuple[np.ndarray, np.ndarray]:
    """Draw fracture systems antialiased: each texel takes the strongest.

    A fracture's width swells and narrows along it, from 0.7 to 1.3 times its
    own, with smoothed noise over about ``swell_km``.

    A fracture's segments are lines of its width whose darkness tapers to a
    third at both of its ends. A texel's share of a line is a box filter of a
    texel against it: the line's width less the texel's distance from it,
    capped at the width where that is below a texel.

    :param size: Texels along each side of the cap.
    :param systems: As :func:`fracture_paths` returns them.
    :param ranks: Each system's rank.
    :param dark_vary: Standard deviation of the darkness along a fracture,
        relative to its own, from smoothed noise over ``swell_km``, held
        within 0.3 to 1.7 times.
    :returns: float16 arrays ``(size, size)``: the share of fill, and the
        rank of the system that gave it.
    """
    starts, ends, widths, darks, which = [], [], [], [], []
    for number, fractures in enumerate(systems):
        for path, width, darkness in fractures:
            if len(path) < 2:
                continue
            n = len(path) - 1
            t = (np.arange(n) + 0.5) / n
            taper = 1 / 3 + 2 / 3 * np.sin(np.pi * t) ** 0.5
            starts.append(path[:-1])
            ends.append(path[1:])
            span = max(int(swell_km / texel_km(size)), 1)
            noise = np.convolve(
                rng.standard_normal(n + 2 * span), np.hanning(2 * span + 1), "same"
            )[span : span + n]
            noise /= max(float(noise.std()), 1e-6)
            widths.append(width * np.clip(1 + 0.25 * noise, 0.7, 1.3))
            shade = darkness * taper
            if dark_vary > 0:
                vary = np.convolve(
                    rng.standard_normal(n + 2 * span), np.hanning(2 * span + 1), "same"
                )[span : span + n]
                vary /= max(float(vary.std()), 1e-6)
                shade = shade * np.clip(1 + dark_vary * vary, 0.3, 1.7)
            darks.append(shade)
            which.append(np.full(n, number))
    a = np.concatenate(starts)
    b = np.concatenate(ends)
    width = np.concatenate(widths)
    dark = np.concatenate(darks)
    system = np.concatenate(which)
    reach = int(np.ceil(max(float(width.max()) / 2 + 1, 1))) + 1
    middle = np.floor((a + b) / 2).astype(np.int64)
    ab = b - a
    length2 = np.maximum((ab**2).sum(axis=1), 1e-9)
    texels, values, owners = [], [], []
    for dr in range(-reach, reach + 1):
        for dc in range(-reach, reach + 1):
            cell = middle + np.array([dr, dc])
            inside = (cell >= 0).all(axis=1) & (cell < size).all(axis=1)
            centre = cell + 0.0
            t = np.clip(((centre - a) * ab).sum(axis=1) / length2, 0, 1)
            distance = np.linalg.norm(centre - (a + t[:, None] * ab), axis=1)
            share = np.clip(width / 2 + 0.5 - distance, 0, np.minimum(width, 1))
            keep = inside & (share > 0.01)
            texels.append(cell[keep, 0] * size + cell[keep, 1])
            values.append(share[keep] * dark[keep])
            owners.append(system[keep])
    texel = np.concatenate(texels)
    value = np.concatenate(values)
    owner = np.concatenate(owners)
    order = np.lexsort((value, texel))
    texel, value, owner = texel[order], value[order], owner[order]
    last = np.r_[texel[1:] != texel[:-1], True]
    shade = np.zeros(size * size, dtype=np.float16)
    rank = np.ones(size * size, dtype=np.float16)
    shade[texel[last]] = np.clip(value[last], 0, 1)
    rank[texel[last]] = ranks[owner[last]]
    return shade.reshape(size, size), rank.reshape(size, size)


def fractures(
    size: int,
    look: Look,
    hemisphere: str,
    activity: np.ndarray,
    concentration: np.ndarray,
    rng: np.random.Generator,
) -> Fractures:
    """Seed, trace and draw a hemisphere's fracture systems.

    Seeds fall where there is ice in some month, denser where the activity
    is high, and the systems follow a direction field of noise that turns
    over :attr:`Look.fracture_turn_km`.

    :param size: Texels along each side of the cap.
    :param look: The tunables.
    :param hemisphere: ``nh`` or ``sh``.
    :param activity: float16 array ``(size, size)``.
    :param concentration: float array ``(12, n, n)`` on a coarser cap.
    :param rng: The generator of everything.
    :returns: The fractures.
    """
    count = look.fracture_systems[HEMISPHERES.index(hemisphere)]
    direction = band_noise(size, look.fracture_turn_km, rng) * np.float32(np.pi / 2)
    iced = zoom(
        concentration.max(axis=0),
        size / concentration.shape[-1],
        order=1,
        grid_mode=True,
        mode="nearest",
    )
    density = (iced > 0.5) * (
        look.fracture_activity
        + (1 - look.fracture_activity) * activity.astype(np.float32)
    )
    del iced
    flat = density.ravel() / density.sum()
    picked = rng.choice(size * size, size=count, p=flat)
    seeds = np.stack([picked // size, picked % size], axis=-1) + rng.random((count, 2))
    systems = fracture_paths(size, seeds, direction, look, rng)
    ranks = rng.random(count).astype(np.float32)
    shade, rank = draw_fractures(
        size, systems, ranks, rng, dark_vary=look.fracture_dark_vary
    )
    summary = np.array(
        [
            [
                sum((len(path) - 1) * look.fracture_step_km for path, _, _ in system),
                len(system) - 1,
                system[0][2],
            ]
            for system in systems
        ],
        dtype=np.float32,
    )
    return Fractures(shade, rank, summary)


@dataclass(frozen=True)
class Floes:
    """Cells the marginal ice breaks into, in two sizes."""

    border: np.ndarray
    """float16 array ``(2, size, size)``: each texel's distance to its cell's
    border, in cells."""

    opening: np.ndarray
    """float16 array ``(2, size, size)``: the level at which each texel's
    floe starts to open, uniform from 0 to 1."""

    width: np.ndarray
    """float16 array ``(2, size, size)``: each texel's floe's full gap, in
    cells."""

    levels: np.ndarray
    """float32 array ``(65,)``: the opening levels the shares are tabulated
    at."""

    shares: np.ndarray
    """float32 array ``(2, 65)``: the share of the area that is gap at each
    level, so a share of gap is read back as a level."""


def floe_gap(floes: Floes, layer: int, level: np.ndarray, ramp: float, rows: slice):
    """Return each texel's floe's gap, in cells, at an opening level.

    :param floes: The floes.
    :param layer: The floes' size, 0 for the large ones.
    :param level: float array of the rows' shape, or a scalar.
    :param ramp: :attr:`Look.floe_ramp`.
    :param rows: The rows.
    :returns: float32 array of the rows' shape.
    """
    opening = floes.opening[layer, rows].astype(np.float32)
    width = floes.width[layer, rows].astype(np.float32)
    return width * np.clip((level - opening) / np.float32(ramp), 0, 1)


def floe_cells(size: int, look: Look, rng: np.random.Generator) -> Floes:
    """Return the floes' cells, their borders bent by a smooth shift.

    Each floe takes a level at which it opens and a full gap, and the share
    of the area that is gap is tabulated against the level.

    :param size: Texels along each side of the cap.
    :param look: The tunables.
    :param rng: The generator of the shift and the cells.
    :returns: The floes.
    """
    shift = curl_shift(size, look.floe_warp_band_km, look.floe_warp_km, rng)
    border = np.empty((2, size, size), dtype=np.float16)
    opening = np.empty((2, size, size), dtype=np.float16)
    width = np.empty((2, size, size), dtype=np.float16)
    levels = np.linspace(0.0, 1.0 + look.floe_ramp, 65, dtype=np.float32)
    shares = np.empty((2, len(levels)), dtype=np.float32)
    low, high = look.floe_width
    together = np.float32(look.floe_cluster)
    alone = np.float32(np.sqrt(max(1 - look.floe_cluster**2, 0)))
    for layer, cell_km in enumerate(look.floe_cells_km):
        keep = look.floe_keep if layer == 0 else 1.0
        shade, distance = facet_cells(size, cell_km, shift, rng, keep)
        distance /= np.float32(cell_km / np.sqrt(keep) / texel_km(size))
        border[layer] = distance
        width[layer] = low + (high - low) * ((ndtr(shade) * 7919.0) % 1.0)
        cluster = band_noise(size, look.floe_cluster_km, rng)
        opening[layer] = ndtr(together * cluster + alone * shade)
        del shade, cluster
        sample = np.s_[layer, ::4, ::4]
        d = border[sample].astype(np.float32)
        o = opening[sample].astype(np.float32)
        w = width[sample].astype(np.float32)
        shares[layer] = [
            float((d < w * np.clip((level - o) / look.floe_ramp, 0, 1)).mean())
            for level in levels
        ]
    return Floes(border, opening, width, levels, shares)


@dataclass(frozen=True)
class Fields:
    """The noise fields of one hemisphere, the same in every month."""

    hemisphere: str
    """``nh`` or ``sh``."""

    concentration: np.ndarray
    """float32 array ``(12, n, n)``: each month's mean on the coarser cap."""

    threshold: np.ndarray
    """float32 array ``(size, size)``: the concentration at which a texel
    turns to ice."""

    shift: np.ndarray
    """float32 array ``(2, size, size)``: where each texel reads the
    concentration from, in texels, row first."""

    leads: Leads
    """The lead network."""

    brightness: np.ndarray | None = None
    """float16 broad brightness noise, standard deviation 1; ``None`` with the
    crumple."""

    mottle: np.ndarray | None = None
    """float16 melt mottling noise, standard deviation 1; ``None`` with the
    crumple."""

    activity: np.ndarray | None = None
    """float16 from 0, flat, to 1, fully structured; with the crumple."""

    slope: np.ndarray | None = None
    """float16 slope of the crumple's height toward the light, standard
    deviation 1; with the crumple."""

    creases: np.ndarray | None = None
    """float16 crease strength from 0 to about 1; with the crumple."""

    plates: np.ndarray | None = None
    """float16 shade of the plates, standard deviation 1; with the crumple."""

    fractures: Fractures | None = None
    """The fracture systems, in place of the lead network's use."""

    floes: Floes | None = None
    """The floes of the marginal ice."""

    melt: np.ndarray | None = None
    """float16 tone of the melt, standard deviation 1; with the melt tone."""

    fast: np.ndarray | None = None
    """float16 array ``(12, n, n)``: how much the fast ice band raised each
    month's concentration, on the coarser cap; with the band and the
    floes."""

    @property
    def size(self) -> int:
        """Texels along each side of the cap."""
        return self.threshold.shape[0]


def prepare(
    concentration: np.ndarray,
    hemisphere: str,
    size: int,
    look: Look = LOOK,
    report: Callable[[str], None] = lambda _: None,
    fast: np.ndarray | None = None,
) -> Fields:
    """Generate a hemisphere's noise fields.

    :param concentration: float array ``(12, n, n)`` from 0 to 1.
    :param hemisphere: ``nh`` or ``sh``.
    :param size: Texels along each side of the cap the ice is generated on.
    :param look: The tunables.
    :param report: Told what is being generated.
    :param fast: How much the fast ice band raised the concentration, as
        :func:`fast_ice_raise` returns it; kept with the floes.
    :returns: The fields.
    """
    report("orientation")
    doubled = edge_orientation(concentration, size, look)
    report("edge noise")
    fine = band_noise(size, look.edge_band_km, _rng(look, hemisphere, 0))
    stretched = streak(fine, doubled, look.edge_streak_km, look.edge_band_km[0])
    del doubled
    stretched *= np.float32(np.sqrt(1 - look.edge_isotropic**2))
    stretched += np.float32(look.edge_isotropic) * fine
    del fine
    stretched = normalized(stretched)
    threshold = (
        look.edge_center + look.edge_spread * (2 * ndtr(stretched) - 1)
    ).astype(np.float32)
    del stretched
    report("eddies")
    shift = curl_shift(
        size, look.eddy_band_km, look.eddy_shift_km, _rng(look, hemisphere, 1)
    )
    report("leads")
    network = leads(size, look, _rng(look, hemisphere, 2))
    if look.crumple:
        report("activity")
        activity = activity_field(size, look, _rng(look, hemisphere, 5))
        report("crumple")
        slope, creases, plates = crumple(size, look, _rng(look, hemisphere, 6))
        cracks = None
        if look.fractures:
            report("fractures")
            cracks = fractures(
                size,
                look,
                hemisphere,
                activity,
                concentration,
                _rng(look, hemisphere, 7),
            )
        melt = None
        if look.melt_tone and hemisphere == "nh":
            report("melt")
            melt = band_noise(size, look.melt_band_km, _rng(look, hemisphere, 9))
            melt = melt.astype(np.float16)
        broken = None
        if look.floes:
            report("floes")
            broken = floe_cells(size, look, _rng(look, hemisphere, 8))
        return Fields(
            hemisphere,
            concentration.astype(np.float32),
            threshold,
            shift,
            network,
            floes=broken,
            activity=activity,
            slope=slope,
            creases=creases,
            plates=plates,
            melt=melt,
            fractures=cracks,
            fast=(
                fast.astype(np.float16)
                if fast is not None and broken is not None
                else None
            ),
        )
    report("brightness")
    brightness = band_noise(
        size, look.brightness_band_km, _rng(look, hemisphere, 3)
    ).astype(np.float16)
    mottle = band_noise(size, look.mottle_band_km, _rng(look, hemisphere, 4)).astype(
        np.float16
    )
    return Fields(
        hemisphere,
        concentration.astype(np.float32),
        threshold,
        shift,
        network,
        brightness,
        mottle,
    )


@dataclass(frozen=True)
class Cap:
    """One hemisphere's ice in one month."""

    coverage: np.ndarray
    """float32 array ``(size, size)`` from 0 to 1: the share of a texel that
    is ice."""

    brightness: np.ndarray
    """float32 array ``(size, size)``: the factor on the month's color."""

    color: Rgb
    """The month's color."""

    land_coverage: np.ndarray | None = None
    """The coverage the coast's land blends with, without the open water of
    leads and floes, which belongs to the sea; ``coverage`` when ``None``."""

    land_brightness: np.ndarray | None = None
    """The brightness the coast's land takes, without leads; ``brightness``
    when ``None``."""

    activity: np.ndarray | None = None
    """float32 array ``(size, size)``: the month's activity, with the
    crumple."""

    tint: np.ndarray | None = None
    """float32 array ``(size, size)``: how far each texel's color leans along
    :attr:`tint_rgb`, with the melt tone."""

    tint_rgb: tuple[float, float, float] = (0.0, 0.0, 0.0)
    """The shift of the color per unit of the tint."""

    def ice_rgb(
        self, brightness: np.ndarray, tint: np.ndarray | None = None
    ) -> np.ndarray:
        """Return the ice's color for sampled brightness values.

        :param brightness: float array of any shape.
        :param tint: Sampled tint of that shape, if any.
        :returns: float32 array of that shape plus 3, from 0 to 255.
        """
        rgb = np.asarray(self.color, dtype=np.float32)
        if tint is not None:
            rgb = rgb + tint[..., None] * np.asarray(self.tint_rgb, dtype=np.float32)
        return np.clip(brightness[..., None] * rgb, 0, 255)


def month_cap(fields: Fields, month: int, look: Look = LOOK) -> Cap:
    """Generate the coverage and brightness of one month.

    :param fields: The hemisphere's noise fields.
    :param month: The month, 1 to 12.
    :param look: The tunables.
    :returns: The month's cap.
    """
    size = fields.size
    field_ = fields.concentration[month - 1]
    scale = field_.shape[-1] / size
    coverage = np.empty((size, size), dtype=np.float32)
    brightness = np.empty((size, size), dtype=np.float32)
    land = np.empty((size, size), dtype=np.float32) if look.coast_blend else None
    activity = np.empty((size, size), dtype=np.float32) if look.crumple else None
    whole = None
    opened = fields.fractures is not None or fields.floes is not None
    if opened and look.coast_blend:
        whole = np.empty((size, size), dtype=np.float32)
    mottle = (
        look.mottle_amplitude * look.mottle_months[month - 1]
        if fields.hemisphere == "nh"
        else 0.0
    )
    low = look.edge_center - look.edge_spread
    high = look.edge_center + look.edge_spread
    ceiling = high if look.edge_pack is None else look.edge_pack
    squeeze = np.float32((ceiling - low) / (high - low))
    fast = None if fields.fast is None else fields.fast[month - 1].astype(np.float32)
    for start, stop in rows(size):
        r = (
            np.arange(start, stop, dtype=np.float32)[:, None]
            + fields.shift[0, start:stop]
        )
        c = np.arange(size, dtype=np.float32)[None, :] + fields.shift[1, start:stop]
        conc = map_coordinates(
            field_,
            [(r + 0.5) * scale - 0.5, (c + 0.5) * scale - 0.5],
            order=3,
            mode="nearest",
            prefilter=False,
        )
        t = fields.threshold[start:stop]
        if look.edge_pack is not None:
            t = low + (t - low) * squeeze
        covered = smoothstep(t - look.edge_softness, t + look.edge_softness, conc)
        coverage[start:stop] = covered
        thin = look.thin_shade + (1 - look.thin_shade) * smoothstep(low, high, conc)
        share = look.lead_share_closed + (
            look.lead_share_open - look.lead_share_closed
        ) * smoothstep(1.0, 0.85, conc)
        if fields.fractures is not None:
            closed, opened = look.fracture_share
            share = closed + (opened - closed) * smoothstep(1.0, 0.85, conc)
            drawn = np.clip((share - fields.fractures.rank[start:stop]) / 0.15, 0, 1)
            gaps = fields.fractures.open[start:stop] * drawn
            coverage[start:stop] = covered * (1 - gaps)
            lead = 0.0
        else:
            drawn = fields.leads.rank[start:stop] < share
            lead = look.lead_darkening * fields.leads.profile[start:stop] * drawn
        if activity is not None:
            floor = look.activity_floor
            base = floor + (1 - floor) * fields.activity[start:stop].astype(np.float32)
            active = 1 - (1 - base) * (
                1
                - look.activity_edge
                * smoothstep(
                    look.activity_edge_range[0], look.activity_edge_range[1], conc
                )
            )
            activity[start:stop] = active
            slope = fields.slope[start:stop].astype(np.float32)
            relief = (
                look.crumple_light * np.maximum(slope, 0)
                - look.crumple_shadow * np.maximum(-slope, 0)
                - look.crumple_crease * fields.creases[start:stop]
            )
            plates = (look.plates_flat + (1 - look.plates_flat) * active) * (
                look.crumple_plates * fields.plates[start:stop]
            )
            texture = thin * (1 + active * relief + plates)
        else:
            broad = 1 + look.brightness_amplitude * fields.brightness[start:stop]
            rough = look.rough_amplitude * smoothstep(
                look.rough_range[1], look.rough_range[0], conc
            )
            melt = 1 + (mottle + rough) * fields.mottle[start:stop]
            texture = thin * broad * melt
        if fields.floes is not None:
            band = None
            if fast is not None:
                band = map_coordinates(
                    fast,
                    [(r + 0.5) * scale - 0.5, (c + 0.5) * scale - 0.5],
                    order=3,
                    mode="nearest",
                    prefilter=False,
                )
            coverage[start:stop] *= 1 - floe_water(
                fields.floes, conc, band, look, start, stop, (low, ceiling), size
            )
        if whole is not None:
            whole[start:stop] = covered
        brightness[start:stop] = texture * (1 - lead)
        if land is not None:
            land[start:stop] = texture
    color = look.colors[fields.hemisphere][month - 1]
    tint, tint_rgb = None, (0.0, 0.0, 0.0)
    melting = look.mottle_months[month - 1] if fields.hemisphere == "nh" else 0.0
    if fields.melt is not None and melting > 0:
        tone = smoothstep(0.0, 2.0, fields.melt.astype(np.float32))
        tint = (look.melt_amplitude * melting) * (tone - np.float32(tone.mean()))
        tint_rgb = tuple(
            float(m - c) for m, c in zip(look.melt_color, color, strict=True)
        )
    return Cap(
        coverage,
        brightness,
        color,
        land_coverage=whole,
        land_brightness=land,
        activity=activity,
        tint=tint,
        tint_rgb=tint_rgb,
    )


def floe_water(
    floes: Floes,
    conc: np.ndarray,
    band: np.ndarray | None,
    look: Look,
    start: int,
    stop: int,
    edge: tuple[float, float],
    size: int,
) -> np.ndarray:
    """Return the share of the sea's fill between the floes in rows of a cap.

    The edge model draws all of a texel as ice at its ceiling and above, and
    the share ``(c - low) / (ceiling - low)`` of it below; what that leaves
    of the concentration's open water, ``1 - c / drawn``, is the floes'
    from :attr:`Look.floe_open`'s first concentration down to all of it at
    the second, except where the fast ice band raised the concentration. The
    water is grey new ice at high concentrations and open water toward the
    edge, and the floes open to an area of that water over its darkness,
    split between the large and the small floes.

    :param floes: The floes' cells.
    :param conc: The concentration in the rows.
    :param band: How much the fast ice band raised it there, if it did.
    :param look: The tunables.
    :param start: The first row.
    :param stop: The row after the last.
    :param edge: The edge model's threshold's low end and its ceiling.
    :param size: Texels along each side of the cap.
    :returns: float32 array of the rows' shape: the share of each texel the
        fill takes from the ice.
    """
    low, ceiling = edge
    drawn = np.clip((conc - low) / (ceiling - low), 0, 1)
    open_ = np.where(drawn > conc, 1 - conc / np.maximum(drawn, 1e-6), 0)
    take = smoothstep(*look.floe_open, conc)
    if band is not None:
        take *= smoothstep(look.floe_fast, 0.0, band)
    dark = look.floe_darkness[0] + (
        look.floe_darkness[1] - look.floe_darkness[0]
    ) * smoothstep(*look.floe_dark_range, conc)
    area = np.minimum(open_ * take / dark, look.floe_gap)
    small = look.floe_small_share * smoothstep(*look.floe_small, conc)
    kept = np.ones(conc.shape, dtype=np.float32)
    for layer, part in enumerate((area * (1 - small), area * small)):
        level = np.interp(part, floes.shares[layer], floes.levels)
        gap = floe_gap(floes, layer, level, look.floe_ramp, slice(start, stop))
        cell_km = look.floe_cells_km[layer] / np.sqrt(
            look.floe_keep if layer == 0 else 1
        )
        soft = np.float32(0.5 * texel_km(size) / cell_km)
        distance = floes.border[layer, start:stop].astype(np.float32)
        water = smoothstep(gap + soft, gap - soft, distance)
        kept *= 1 - water * np.minimum(gap / soft, 1)
    return (dark * (1 - kept)).astype(np.float32)


@dataclass(frozen=True)
class MonthIce:
    """Both hemispheres' ice in one month, ready to composite into faces."""

    caps: dict[str, Cap]
    """The north's and the south's cap."""

    def sample(
        self, x: np.ndarray, y: np.ndarray, z: np.ndarray, land: bool = False
    ) -> tuple[np.ndarray, np.ndarray]:
        """Sample the coverage and the ice's color along directions.

        Directions further than ``RHO_MAX`` from their pole on the plane, below
        about 35 degrees of latitude, have no ice.

        :param x: Direction components toward longitude 90 E.
        :param y: Direction components toward the north pole.
        :param z: Direction components toward longitude 0.
        :param land: Sample what the coast's land blends with instead.
        :returns: The coverage, float32 of the directions' shape, and the
            ice's color, float32 of that shape plus 3.
        """
        south, px, py = directions_to_cap(x, y, z)
        coverage = np.zeros(x.shape, dtype=np.float32)
        rgb = np.zeros((*x.shape, 3), dtype=np.float32)
        reach = np.hypot(px, py) <= RHO_MAX
        for hemisphere, here in (("nh", ~south & reach), ("sh", south & reach)):
            if not here.any():
                continue
            cap = self.caps[hemisphere]
            row, col = plane_to_texel(px[here], py[here], cap.coverage.shape[0])
            cover, bright = cap.coverage, cap.brightness
            if land and cap.land_coverage is not None:
                cover = cap.land_coverage
            if land and cap.land_brightness is not None:
                bright = cap.land_brightness
            coverage[here] = map_coordinates(cover, [row, col], order=1, mode="nearest")
            tint = None
            if cap.tint is not None:
                tint = map_coordinates(cap.tint, [row, col], order=1, mode="nearest")
            rgb[here] = cap.ice_rgb(
                map_coordinates(bright, [row, col], order=1, mode="nearest"), tint
            )
        return coverage, rgb


def texel_solid_angles(x: np.ndarray, y: np.ndarray, z: np.ndarray, size: int):
    """Return the solid angle of face texels from their unnormalized directions.

    A face's directions are ``(u, v, 1)`` up to order and sign, with ``u`` and
    ``v`` the tangents of the warped coordinates, so a texel spans
    ``(pi / 2n)**2 (1 + u**2)(1 + v**2) / (1 + u**2 + v**2)**1.5``.

    :param x: Direction components, one of them 1 in magnitude.
    :param y: Direction components of the same shape.
    :param z: Direction components of the same shape.
    :param size: Edge length of the face in texels.
    :returns: float64 array of steradians.
    """
    squared = x * x + y * y + z * z
    product = (1 + x * x) * (1 + y * y) * (1 + z * z) / 2
    return (np.pi / (2 * size)) ** 2 * product / squared**1.5


def composite_face(
    flat: np.ndarray,
    water: np.ndarray,
    face: str,
    ice: MonthIce,
    fill: Rgb,
    pool: Executor | None = None,
    band_rows: int = 256,
) -> dict[str, float]:
    """Put the month's ice into the water share of a flattened face.

    The flatten made every texel ``(1 - w) * land + w * fill`` with ``w`` the
    mask's share of water; here the fill's share becomes
    ``w * mix(fill, ice, coverage)``, so a texel of land is untouched, open
    water without ice stays the fill, and a coast texel's land share keeps its
    color.

    :param flat: uint8 array ``(N, N, 3)``, the flattened face, changed in
        place.
    :param water: uint8 array ``(N, N)``, the mask, 255 for open water.
    :param face: The face's name.
    :param ice: The month's ice.
    :param fill: The ocean fill color the face was flattened with.
    :param pool: Runs the bands concurrently when given.
    :param band_rows: Face rows worked at once.
    :returns: Per hemisphere, the ice's area on this face in square
        kilometers: coverage times the mask's share of water times the
        texel's area.
    """
    size = flat.shape[0]
    fill_rgb = np.asarray(fill, dtype=np.float32)

    def band(start: int) -> tuple[float, float]:
        stop = min(start + band_rows, size)
        share = water[start:stop]
        wet = share > 0
        if not wet.any():
            return 0.0, 0.0
        x, y, z = (axis[wet] for axis in face_directions(face, size, start, stop))
        coverage, rgb = ice.sample(x, y, z)
        weight = share[wet].astype(np.float32) / 255 * coverage
        texels = flat[start:stop][wet].astype(np.float32)
        texels += weight[:, None] * (rgb - fill_rgb)
        flat[start:stop][wet] = np.clip(texels + 0.5, 0, 255).astype(np.uint8)
        area = weight * texel_solid_angles(x, y, z, size) * AUTHALIC_RADIUS_KM**2
        north = float(area[y > 0].sum())
        return north, float(area.sum()) - north

    starts = range(0, size, band_rows)
    bands = map(band, starts) if pool is None else pool.map(band, starts)
    north, south = (sum(values) for values in zip(*bands, strict=True))
    return {"nh": north, "sh": south}


def winter_months(table: tuple[Rgb, ...]) -> list[int]:
    """Return the months whose color is a table's brightest, its winter.

    :param table: Twelve colors, January first.
    :returns: The months, 1 to 12.
    """
    brightest = max(table, key=sum)
    return [month + 1 for month, color in enumerate(table) if color == brightest]


def rescaled_colors(table: tuple[Rgb, ...], winter: Rgb) -> tuple[Rgb, ...]:
    """Return a table with a new winter color and every month's darkening kept.

    Each month's color is the new winter color times the month's ratio to the
    table's winter color, per channel.

    :param table: Twelve colors, January first.
    :param winter: The new color of the table's brightest months.
    :returns: Twelve colors.
    """
    before = np.asarray(max(table, key=sum), dtype=np.float64)
    after = np.asarray(winter, dtype=np.float64)
    scaled = np.clip(
        np.rint(after * np.asarray(table, dtype=np.float64) / before), 0, 255
    )
    return tuple((int(r), int(g), int(b)) for r, g, b in scaled)


def imagery_sea(rgb: np.ndarray, look: Look = LOOK) -> np.ndarray:
    """Return where the imagery draws its sea: dark and bluish.

    :param rgb: Array of any shape ending in 3.
    :param look: The tunables.
    :returns: bool array without the last axis.
    """
    values = rgb.astype(np.float32)
    lum = values @ np.array([0.2126, 0.7152, 0.0722], dtype=np.float32)
    return (lum < look.coast_sea[0]) & (
        values[..., 2] - values[..., 0] > look.coast_sea[1]
    )


def _surroundings(
    lum: np.ndarray, snowy: np.ndarray, inner: np.ndarray, sigma: float
) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    """Read the land around each texel: how much of it there is, how much of
    it is snow, and the standard deviation of the snow's luminance."""
    read = gaussian_filter(inner, sigma, mode="nearest")
    snow = inner * snowy
    covered = gaussian_filter(snow, sigma, mode="nearest")
    share = covered / np.maximum(read, 1e-6)
    mean = gaussian_filter(lum * snow, sigma, mode="nearest")
    mean /= np.maximum(covered, 1e-6)
    square = gaussian_filter(lum * lum * snow, sigma, mode="nearest")
    square /= np.maximum(covered, 1e-6)
    relief = np.sqrt(np.maximum(square - mean * mean, 0.0))
    relief[covered < 0.02] = np.inf
    return read, share, relief


def coast_weight(
    source: np.ndarray,
    water: np.ndarray,
    look: Look = LOOK,
    spacing_km: float | None = None,
) -> np.ndarray:
    """Return how far each texel's land takes the ice's color, before coverage.

    The weight is the product of how much of the land around is snow, how
    flat that snow is, and a falloff with the distance inland. Snow and
    relief are read from the land at least :attr:`Look.coast_inner_km` from
    the water, over a Gaussian of :attr:`Look.coast_window_km`, so the
    imagery's own coastline pixels, part snow and part its dark water, take
    their weight from the land behind them. The relief is the standard
    deviation of the snow's luminance there; land without enough snow around
    to measure it gets none.

    :param source: uint8 array ``(N, N, 3)``, the face before the flatten.
    :param water: uint8 array ``(N, N)``, the mask, 255 for open water.
    :param look: The tunables.
    :param spacing_km: The texels' side; a face's at its center when ``None``.
    :returns: float32 array ``(N, N)`` from 0 to 1, 0 on open water.
    """
    size = water.shape[0]
    spacing = (
        np.pi / 2 * AUTHALIC_RADIUS_KM / size if spacing_km is None else spacing_km
    )
    inland = distance_transform_edt(water < 128).astype(np.float32)
    inland *= np.float32(spacing)
    reach = smoothstep(look.coast_reach_km[1], look.coast_reach_km[0], inland)
    reach[water == 255] = 0
    if not reach.any():
        return reach
    rgb = source.astype(np.float32)
    lum = rgb @ np.array([0.2126, 0.7152, 0.0722], dtype=np.float32)
    top = rgb.max(axis=-1)
    snowy = (lum > look.coast_snow[0]) & (
        top - rgb.min(axis=-1) < look.coast_snow[1] * np.maximum(top, 1)
    )
    sea = imagery_sea(source, look)
    del rgb, top
    sigma = look.coast_window_km / spacing
    near_km, far_km = look.coast_inner_km
    _, share, relief = _surroundings(
        lum, snowy, ((inland >= near_km) & ~sea).astype(np.float32), sigma
    )
    read, far_share, far_relief = _surroundings(
        lum, snowy, ((inland >= far_km) & ~sea).astype(np.float32), sigma
    )
    del lum, snowy
    far = smoothstep(0.02, 0.1, read)
    share += far * (far_share - share)
    relief = np.where(far > 0.5, far_relief, relief)
    del far_share, far_relief, read, far
    flat = smoothstep(look.coast_relief[1], look.coast_relief[0], relief)
    weight = reach * flat * smoothstep(*look.coast_snow_share, share)
    np.maximum(weight, reach * sea, out=weight)
    return weight.astype(np.float32)


def blend_coast(
    flat: np.ndarray,
    source: np.ndarray,
    water: np.ndarray,
    face: str,
    ice: MonthIce,
    look: Look = LOOK,
    pool: Executor | None = None,
    band_rows: int = 256,
    spacing_km: float | None = None,
    fill: Rgb | None = None,
) -> np.ndarray:
    """Blend the land share of a flattened face toward the ice along the coast.

    The land share ``(1 - w) * land`` of a texel becomes
    ``(1 - w) * mix(land, ice, k)``, with ``k`` the :func:`coast_weight`
    times the ice's coverage there. The ice is what the coast's land blends
    with: its color and texture without leads and open water. Where the
    land share holds the imagery's sea it takes the sea's color instead,
    ``mix(fill, ice, coverage)``, at the :func:`coast_weight` alone.

    :param flat: uint8 array ``(N, N, 3)``, the flattened face, changed in
        place.
    :param source: uint8 array ``(N, N, 3)``, the face before the flatten.
    :param water: uint8 array ``(N, N)``, the mask, 255 for open water.
    :param face: The face's name.
    :param ice: The month's ice.
    :param look: The tunables.
    :param pool: Runs the bands concurrently when given.
    :param band_rows: Face rows worked at once.
    :param spacing_km: The texels' side; a face's at its center when ``None``.
    :param fill: The ocean fill color the face was flattened with; the
        imagery's sea is left out of the blend when ``None``.
    :returns: float32 array ``(N, N)``: ``k`` times the land share, the share
        of each texel the ice took.
    """
    size = flat.shape[0]
    weight = coast_weight(source, water, look, spacing_km)
    fill_rgb = np.asarray(fill if fill is not None else (0, 0, 0), dtype=np.float32)

    def band(start: int) -> None:
        stop = min(start + band_rows, size)
        here = weight[start:stop] > 0
        if not here.any():
            return
        x, y, z = (axis[here] for axis in face_directions(face, size, start, stop))
        coverage, rgb = ice.sample(x, y, z, land=True)
        land = 1 - water[start:stop][here].astype(np.float32) / 255
        original = source[start:stop][here]
        k = weight[start:stop][here] * coverage * land
        if fill is not None:
            sea = imagery_sea(original, look)
            if sea.any():
                rgb[sea] = fill_rgb + coverage[sea, None] * (rgb[sea] - fill_rgb)
                k[sea] = weight[start:stop][here][sea] * land[sea]
        weight[start:stop][here] = k
        texels = flat[start:stop][here].astype(np.float32)
        texels += k[:, None] * (rgb - original.astype(np.float32))
        flat[start:stop][here] = np.clip(texels + 0.5, 0, 255).astype(np.uint8)

    starts = range(0, size, band_rows)
    for _ in map(band, starts) if pool is None else pool.map(band, starts):
        pass
    return weight


def fast_ice(hemisphere: Hemisphere, look: Look = LOOK) -> Hemisphere:
    """Carry the concentration offshore in to the coast where it is high.

    Within :attr:`Look.fast_ice_km` of the record's land its 25 km cells mix
    land and sea and read low along coasts the ice offshore closes. Each
    ocean cell there is raised toward the mean of the cells
    :attr:`Look.fast_ice_offshore_km` out, over a Gaussian of two cells,
    fully where that mean is above :attr:`Look.fast_ice_closed`'s upper end
    and the cell is nearer than the first distance. A cell is never lowered,
    so open water offshore leaves the coast as the record has it.

    :param hemisphere: The record.
    :param look: The tunables; the record comes back unchanged without
        :attr:`Look.fast_ice`.
    :returns: The record with the band.
    """
    if not look.fast_ice:
        return hemisphere
    ocean = ~hemisphere.land
    distance = distance_transform_edt(ocean).astype(np.float32) * SOURCE_CELL_KM
    ring = (
        ocean
        & (distance >= look.fast_ice_offshore_km[0])
        & (distance <= look.fast_ice_offshore_km[1])
    ).astype(np.float32)
    read = gaussian_filter(ring, 2.0, mode="constant")
    near = smoothstep(look.fast_ice_km[1], look.fast_ice_km[0], distance) * ocean
    means = hemisphere.means.copy()
    for month in range(len(means)):
        values = np.nan_to_num(means[month])
        offshore = gaussian_filter(values * ring, 2.0, mode="constant") / np.maximum(
            read, 1e-6
        )
        weight = near * smoothstep(*look.fast_ice_closed, offshore) * (read > 0.05)
        raised = values + weight * np.maximum(offshore - values, 0)
        means[month] = np.where(ocean, raised, means[month])
    return dataclasses.replace(hemisphere, means=means)


def fast_ice_raise(record: Hemisphere, banded: Hemisphere) -> np.ndarray | None:
    """Return how much the fast ice band raised the concentration, on the cap.

    :param record: The record.
    :param banded: The record with the band, as :func:`fast_ice` returns it.
    :returns: float32 array ``(12, n, n)`` as
        :func:`~texture_pipeline.sea_ice.cap_concentration` resamples it, or
        ``None`` where there is no band.
    """
    if banded is record:
        return None
    return cap_concentration(
        dataclasses.replace(record, means=banded.means - record.means)
    )
