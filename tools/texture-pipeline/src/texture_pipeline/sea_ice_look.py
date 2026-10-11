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

from collections.abc import Callable, Iterator
from concurrent.futures import Executor
from dataclasses import dataclass, field

import numpy as np
from scipy import fft
from scipy.ndimage import gaussian_filter, map_coordinates, zoom
from scipy.special import ndtr

from texture_pipeline.cube import face_directions
from texture_pipeline.sea_ice import (
    AUTHALIC_RADIUS_KM,
    HEMISPHERES,
    RHO_MAX,
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

    brightness: np.ndarray
    """float16 broad brightness noise, standard deviation 1."""

    mottle: np.ndarray
    """float16 melt mottling noise, standard deviation 1."""

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
) -> Fields:
    """Generate a hemisphere's noise fields.

    :param concentration: float array ``(12, n, n)`` from 0 to 1.
    :param hemisphere: ``nh`` or ``sh``.
    :param size: Texels along each side of the cap the ice is generated on.
    :param look: The tunables.
    :param report: Told what is being generated.
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

    def ice_rgb(self, brightness: np.ndarray) -> np.ndarray:
        """Return the ice's color for sampled brightness values.

        :param brightness: float array of any shape.
        :returns: float32 array of that shape plus 3, from 0 to 255.
        """
        rgb = np.asarray(self.color, dtype=np.float32)
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
    mottle = (
        look.mottle_amplitude * look.mottle_months[month - 1]
        if fields.hemisphere == "nh"
        else 0.0
    )
    low = look.edge_center - look.edge_spread
    high = look.edge_center + look.edge_spread
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
        coverage[start:stop] = smoothstep(
            t - look.edge_softness, t + look.edge_softness, conc
        )
        thin = look.thin_shade + (1 - look.thin_shade) * smoothstep(low, high, conc)
        share = look.lead_share_closed + (
            look.lead_share_open - look.lead_share_closed
        ) * smoothstep(1.0, 0.85, conc)
        drawn = fields.leads.rank[start:stop] < share
        lead = look.lead_darkening * fields.leads.profile[start:stop] * drawn
        broad = 1 + look.brightness_amplitude * fields.brightness[start:stop]
        rough = look.rough_amplitude * smoothstep(
            look.rough_range[1], look.rough_range[0], conc
        )
        melt = 1 + (mottle + rough) * fields.mottle[start:stop]
        brightness[start:stop] = thin * broad * melt * (1 - lead)
    return Cap(coverage, brightness, look.colors[fields.hemisphere][month - 1])


@dataclass(frozen=True)
class MonthIce:
    """Both hemispheres' ice in one month, ready to composite into faces."""

    caps: dict[str, Cap]
    """The north's and the south's cap."""

    def sample(
        self, x: np.ndarray, y: np.ndarray, z: np.ndarray
    ) -> tuple[np.ndarray, np.ndarray]:
        """Sample the coverage and the ice's color along directions.

        Directions further than ``RHO_MAX`` from their pole on the plane, below
        about 35 degrees of latitude, have no ice.

        :param x: Direction components toward longitude 90 E.
        :param y: Direction components toward the north pole.
        :param z: Direction components toward longitude 0.
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
            coverage[here] = map_coordinates(
                cap.coverage, [row, col], order=1, mode="nearest"
            )
            rgb[here] = cap.ice_rgb(
                map_coordinates(cap.brightness, [row, col], order=1, mode="nearest")
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
