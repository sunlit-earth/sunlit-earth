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
import functools
import itertools
from collections.abc import Callable, Iterator, Mapping
from concurrent.futures import FIRST_COMPLETED, Executor, ThreadPoolExecutor, wait
from dataclasses import dataclass, field

import numpy as np
from scipy import fft
from scipy.ndimage import (
    distance_transform_edt,
    gaussian_filter,
    gaussian_filter1d,
    label,
    map_coordinates,
    zoom,
)
from scipy.spatial import cKDTree
from scipy.special import ndtr, ndtri

from texture_pipeline import parallel
from texture_pipeline.cube import face_directions
from texture_pipeline.sea_ice import (
    AUTHALIC_RADIUS_KM,
    HEMISPHERES,
    RHO_MAX,
    SOURCE_CELL_KM,
    Hemisphere,
    cap_concentration,
    cap_texel_centers,
    cap_to_latlon,
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

    ridged: bool = False
    """Whether the crumple is ridged relief at three detail levels, varied by
    region fields in detail, strength and softness, with creases along warped
    cells in the soft regions and drift streaks in some, in place of the
    stretched ridges, the folds and the plates."""

    ridged_km: tuple[float, float] = (40.0, 2.5)
    """The feature size of the relief's coarsest octave and the least one an
    octave may have; each octave halves it."""

    ridged_offset: float = 1.0
    """What an octave's noise is folded from: ``offset - |n|``."""

    ridged_octave_gain: float = 1.5
    """How strongly an octave's ridges confine the next finer one to them."""

    ridged_warp_band_km: tuple[float, float] = (6.0, 25.0)
    """Feature sizes of the shift each octave is crinkled by."""

    ridged_levels: tuple[tuple[float, float, float], ...] = (
        (3.0, 0.75, 1.5),
        (3.0, 0.9, 2.5),
        (4.0, 1.0, 3.5),
    )
    """Per detail level, least detailed first: the exponent that narrows each
    ridge about its crest, the persistence of the finer octaves and the root
    mean square of the crinkle in kilometers."""

    ridged_shading: tuple[float, float] = (0.025, 0.05)
    """Brightening per standard deviation of the relief's slope toward the
    light, and darkening per standard deviation away from it."""

    light_angle: float = 0.6
    """The light's direction, radians from the columns toward the rows."""

    region_band_km: tuple[float, float] = (250.0, 900.0)
    """Feature sizes of the five region fields: detail, softness, drift, the
    underlayer and strength."""

    region_cap: int = 1024
    """Texels along each side of the coarser cap the region fields and the
    drift's wind are drawn on before they are zoomed to the cap's size."""

    detail_steps: tuple[tuple[float, float], tuple[float, float]] = (
        (-0.2, -0.8),
        (0.2, 0.8),
    )
    """The detail field's steps to the least and to the most detailed level;
    the middle level takes the rest."""

    soft_step: tuple[float, float] = (0.0, 0.8)
    """The softness field's step to the soft regions."""

    soft_activity: tuple[float, float, float] = (-1.5, 1.2, 0.15)
    """The soft regions' activity: the activity noise's step from, to, and
    the floor, so structure fades in and out gradually."""

    strength: tuple[float, float] = (0.35, 0.85)
    """The relief's and the drift's least and greatest strength."""

    strength_step: tuple[float, float] = (-1.0, 1.0)
    """The strength field's values over which it goes from the least to the
    greatest."""

    voronoi_levels: tuple[tuple[float, float, float], ...] = (
        (20.0, 0.35, 0.06),
        (60.0, 0.5, 0.08),
        (180.0, 0.8, 0.08),
    )
    """Per level of the soft regions' creases: the cells' size, the share of
    their points kept and the crease's depth."""

    voronoi_crease_km: float = 1.0
    """Gaussian half width of a crease along a cell's border."""

    voronoi_warp: tuple[tuple[float, tuple[float, float]], ...] = (
        (5.0, (20.0, 80.0)),
        (1.5, (6.0, 20.0)),
    )
    """The shifts the cells are read through: the root mean square in
    kilometers and the feature sizes of each."""

    voronoi_weight: float = 1.5
    """The creases' weight against the soft regions' share."""

    drift_step: tuple[float, float] = (0.5, 1.1)
    """The drift field's step to the regions with drift streaks."""

    drift_km: tuple[float, float] = (8.0, 30.0)
    """Feature sizes of the drift streaks across."""

    drift_aspect: float = 6.0
    """How much longer the streaks are than wide."""

    drift_zone: tuple[float, float] = (0.3, 0.6)
    """Activity over which the streaks go from none to full."""

    drift_shading: tuple[float, float] = (0.03, 0.05)
    """Brightening and darkening of the streaks per standard deviation of
    their slope."""

    drift_wind: tuple[float, float, tuple[float, float]] = (0.7, 0.5, (800.0, 3000.0))
    """The wind the streaks lie along: its mean direction and how far it
    turns, in radians, and the feature sizes of the noise it turns with."""

    jagged: bool = False
    """Whether the fracture systems are drawn as thin leads broken into pieces
    en echelon and pinched shut in places, in place of smooth strokes."""

    echelon: tuple[tuple[float, float], ...] = (
        (10.0, 40.0),
        (0.1, 0.3),
        (1.0, 3.0),
        (2.0, 6.0),
    )
    """A lead's pieces: their length in kilometers, their turn off the course
    in radians, one sign per fracture, their offset to alternate sides and
    their overlap with their neighbors at both ends, in kilometers."""

    echelon_taper: float = 0.15
    """The darkness a piece tapers to at its ends."""

    pinch_run_km: tuple[float, float] = (4.0, 25.0)
    """Lengths of the runs a lead's width steps between."""

    pinch_shut: float = 0.2
    """Share of the runs where the lead is pinched shut."""

    pinch_scale: tuple[float, float] = (0.6, 1.8)
    """Range of the width of the other runs, times the lead's own."""

    jagged_darkness: float = 0.75
    """Factor on every lead's darkness, which brings a combination of styles
    back to the lead share it was tuned at."""

    cascade: bool = False
    """Whether the open water the concentration leaves is drawn by a cascade
    of whole cells, a calm speckle between 40% and 70%, and one gray or dark
    color per patch of water, in place of the edge model's threshold and the
    floes: the ice edge is where the cells reach no concentration."""

    floe_lost_km: float = 40.0
    """The leads' open water is smoothed over this before the floes take it
    off the water they draw."""

    gap_gray: float = 0.45
    """The share of the sea's fill a gray patch of water shows."""

    gap_dark: tuple[float, float] = (0.95, 0.45)
    """Concentrations above which no water is dark and below which all is."""

    gap_bin: float = 0.05
    """Width of the bins of the patches' mean concentration within which the
    largest patches turn dark first."""

    gap_jitter: float = 0.7
    """Standard deviation of the noise on the logarithm of a patch's area in
    that order."""

    dark_gaps: bool = False
    """Whether every gap between the settled floes is the sea's fill, in place
    of one gray or dark color per patch: the floes' water is then what the
    leads leave of the concentration's shortfall, all of it dark."""

    floe_tone: tuple[float, float] | None = None
    """When set, ``(lo, hi)``: the settled floes read the concentration ``c``
    as ``c + (1 - c) smoothstep(lo, hi, c)``, and at ``hi`` and above they
    draw no water, their cells cut at that line as at the coastline. The
    leads, the coast rule, the thin ice shading and the activity read ``c``
    itself."""

    floe_target: tuple[float, tuple[float, float]] = (0.8, (60.0, 250.0))
    """The regional ice target: the weight of its noise and the noise's
    feature sizes, so loose and compact regions alternate."""

    cascade_km: tuple[float, ...] = (64.0, 32.0, 16.0, 8.0, 4.0)
    """The cascade's cell sizes, coarsest first."""

    cascade_warp: float = 0.15
    """Root mean square of the shift that bends a level's cells, as a share of
    its cell."""

    cascade_deltas: tuple[float, ...] = (2.0, 1.2, 0.5, -0.2, -0.6)
    """Per level, how far its cells lean to ice: coarse cells to ice, fine
    ones to water."""

    cascade_scale: tuple[tuple[float, float], ...] = (
        (0.2, 0.6),
        (0.4, 0.867),
        (0.7, 1.667),
        (0.9, 0.6),
        (1.0, 0.0),
    )
    """(concentration, level) pairs, interpolated: the level at which most of
    the cells still open resolve."""

    calm_zone: tuple[float, float, float] = (0.4, 0.7, 0.05)
    """The concentrations of the calm zone, and the width of its soft steps."""

    calm_level: int = 2
    """The level at which every cell still open in the calm zone resolves."""

    speckle_octaves: tuple[tuple[tuple[float, float], float], ...] = (
        ((8.0, 25.0), 0.15),
        ((25.0, 60.0), 0.7),
        ((60.0, 120.0), 0.5),
    )
    """The calm speckle's noise: per octave its feature sizes and weight."""

    speckle_soft: float = 0.12
    """Half the step of the speckle's noise over which a texel turns water."""

    speckle_pick_km: tuple[float, float] = (30.0, 120.0)
    """Feature sizes of the noise that hands regions to the speckle."""

    coast_rule: tuple[float, float] | None = None
    """When set, the sea within the first, in kilometers, of the land is ice
    wherever the concentration is at least the second; nothing is opened."""


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
    noise: np.ndarray,
    doubled: np.ndarray,
    length_km: float,
    finest_km: float,
    workers: int | None = None,
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
    :param workers: Threads for the bands of rows; the process's setting when
        ``None``. The result does not depend on it.
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

    def band(span: tuple[int, int]) -> None:
        start, stop = span
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

    parallel.pmap(band, rows(size), workers)
    return normalized(out)


def curl_shift(
    size: int, band_km: tuple[float, float], rms_km: float, rng: np.random.Generator
) -> np.ndarray:
    """Return a divergence-free shift field in texels.

    :param size: Texels along each side of the cap.
    :param band_km: Feature sizes of the stream function.
    :param rms_km: Root mean square of the shift's length.
    :param rng: The generator of the stream function's noise.
    :returns: float32 array ``(2, size, size)``, row shift first; zero where
        the band is finer than the cap's texel.
    """
    shift, rms = _curl(size, band_km, rng)
    if rms > 0:
        shift *= rms_km / texel_km(size) / rms
    return shift


def _curl(
    size: int, band_km: tuple[float, float], rng: np.random.Generator
) -> tuple[np.ndarray, float]:
    """Return :func:`curl_shift`'s field before its scaling, and its root
    mean square in texels."""
    stream = band_noise(size, band_km, rng)
    gr, gc = np.gradient(stream)
    del stream
    shift = np.stack([gc, -gr]).astype(np.float32)
    return shift, float(np.sqrt(np.mean(gr * gr + gc * gc)))


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


def light_slope(height: np.ndarray, light: float) -> np.ndarray:
    """Return a height's slope along a light's direction, standard deviation 1.

    :param height: float32 array.
    :param light: The light's direction, radians from the columns toward the
        rows.
    :returns: float32 array of the height's shape.
    """
    gr, gc = np.gradient(height)
    return normalized(np.float32(np.sin(light)) * gr + np.float32(np.cos(light)) * gc)


def lit(slope: np.ndarray, shading: tuple[float, float]) -> np.ndarray:
    """Return the shading of a slope: brightened toward the light by the
    first of ``shading`` per unit, darkened away from it by the second."""
    light, shadow = shading
    return light * np.maximum(slope, 0) - shadow * np.maximum(-slope, 0)


def stepped_activity(
    noise: np.ndarray, step: tuple[float, float], floor: float
) -> np.ndarray:
    """Return an activity from the activity noise: ``floor`` below the step,
    1 above it.

    :param noise: The activity noise, standard deviation 1.
    :param step: The noise's values the activity rises over.
    :param floor: The least activity.
    :returns: float32 array of the noise's shape.
    """
    return (floor + (1 - floor) * smoothstep(*step, noise)).astype(np.float32)


def ridged_reliefs(
    size: int,
    look: Look,
    octaves: np.random.Generator,
    crinkle: np.random.Generator,
    workers: int | None = None,
) -> Iterator[np.ndarray]:
    """Return the ridged relief at each of :attr:`Look.ridged_levels`, shaded.

    Musgrave's ridged multifractal: each octave of band noise, crinkled by a
    divergence-free shift, is folded at zero, ``clip(offset - |n|, 0)``,
    raised to the level's exponent and gated by the coarser octave's signal,
    and the height so built is shaded by its slope toward the light. The
    octaves and the crinkle's direction are drawn once and shared by the
    levels, which differ only in the exponent, the persistence and the
    crinkle's strength, so blending them leaves no seams.

    :param size: Texels along each side of the cap.
    :param look: The tunables.
    :param octaves: The generator of the octaves' noise.
    :param crinkle: The generator of the crinkle.
    :param workers: Threads for the bands of rows; the process's setting when
        ``None``. The result does not depend on it.
    :returns: Per level, in turn, a float32 array ``(size, size)``: the
        factor's change of the brightness.
    """
    noises = []
    feature, finest = look.ridged_km
    while feature >= finest:
        noises.append(band_noise(size, (feature / 1.5, feature * 1.5), octaves))
        feature /= 2
    unit, rms = _curl(size, look.ridged_warp_band_km, crinkle)

    def level(exponent: float, persistence: float, warp_km: float) -> np.ndarray:
        shift = unit * (warp_km / texel_km(size) / rms) if rms > 0 else unit
        height = np.empty((size, size), dtype=np.float32)

        def band(span: tuple[int, int]) -> None:
            start, stop = span
            r = np.arange(start, stop, dtype=np.float32)[:, None] + shift[0, start:stop]
            c = np.arange(size, dtype=np.float32)[None, :] + shift[1, start:stop]
            weight = np.ones((stop - start, size), dtype=np.float32)
            built = np.zeros((stop - start, size), dtype=np.float32)
            for octave, noise in enumerate(noises):
                n = map_coordinates(noise, [r, c], order=1, mode="grid-wrap")
                signal = (
                    np.clip(look.ridged_offset - np.abs(n), 0, None) ** exponent
                    * weight
                )
                built += persistence**octave * signal
                weight = np.clip(signal * look.ridged_octave_gain, 0, 1)
            height[start:stop] = built

        parallel.pmap(band, rows(size), workers)
        return lit(light_slope(height, look.light_angle), look.ridged_shading)

    for values in look.ridged_levels:
        yield level(*values)


def voronoi_creases(size: int, look: Look, rng: np.random.Generator) -> np.ndarray:
    """Return creases along the borders of warped cells at several sizes.

    Each level's border is a Gaussian crease of its depth, and the deepest of
    the levels is taken; the cells' own shades are left out.

    :param size: Texels along each side of the cap.
    :param look: The tunables.
    :param rng: The generator of the shifts and the cells.
    :returns: float32 array ``(size, size)``, 0 or below.
    """
    (rms, band), *finer = look.voronoi_warp
    shift = curl_shift(size, band, rms, rng)
    for rms, band in finer:
        shift += curl_shift(size, band, rms, rng)
    crease = np.zeros((size, size), dtype=np.float32)
    width = look.voronoi_crease_km / texel_km(size)
    for cell_km, keep, depth in look.voronoi_levels:
        _, border = facet_cells(size, cell_km, shift, rng, keep)
        crease = np.maximum(crease, depth * np.exp(-((border / width) ** 2)))
        del border
    return -crease


def drift_streaks(
    size: int,
    look: Look,
    activity: np.ndarray,
    share: np.ndarray,
    rng: np.random.Generator,
    coarse: int | None = None,
) -> np.ndarray:
    """Return streaks along a slowly turning wind, lit, where the activity and
    the drift field allow.

    :param size: Texels along each side of the cap.
    :param look: The tunables.
    :param activity: float32 array ``(size, size)``, the month independent
        activity.
    :param share: float32 array ``(size, size)``: the drift field's step.
    :param rng: The generator of the wind and the streaks.
    :param coarse: Texels along each side of the cap the wind is drawn on and
        zoomed from; ``size`` when ``None``.
    :returns: float32 array ``(size, size)``: the factor's change of the
        brightness, before the strength.
    """
    wind, turn, turn_km = look.drift_wind
    angle = _zoomed(band_noise(coarse or size, turn_km, rng), size)
    angle = angle * np.float32(turn) + np.float32(wind)
    doubled = np.stack([np.cos(2 * angle), np.sin(2 * angle)]).astype(np.float32)
    del angle
    height = steered_noise(size, look.drift_km, doubled, look.drift_aspect, rng)
    del doubled
    slope = light_slope(height, look.light_angle)
    del height
    zone = smoothstep(*look.drift_zone, activity) * share
    return zone * lit(slope, look.drift_shading)


def _zoomed(field_: np.ndarray, size: int) -> np.ndarray:
    """Return a field of a coarser cap at a cap's size, linearly."""
    if field_.shape[-1] == size:
        return field_
    factor = size / field_.shape[-1]
    return zoom(field_, factor, order=1, mode="nearest", grid_mode=True)


@dataclass(frozen=True)
class Surface:
    """The closed pack's surface, the same in every month."""

    relief: np.ndarray
    """float16 relief blended over the detail levels, with the soft regions'
    creases: the factor's change of the brightness at full activity."""

    gain: np.ndarray
    """float16 strength of the relief and the drift."""

    drift: np.ndarray
    """float16 drift streaks: the factor's change of the brightness, before
    the strength."""

    base: np.ndarray
    """float16 activity before the concentration raises it: the hard
    activity, and the soft one in the soft regions."""


def ridged_surface(
    size: int,
    look: Look,
    noise: np.ndarray,
    regions: list[np.ndarray],
    rngs: Mapping[str, np.random.Generator],
    workers: int | None = None,
    coarse: int | None = None,
) -> dict[str, np.ndarray]:
    """Compose the closed pack's surface from its parts.

    :param size: Texels along each side of the cap.
    :param look: The tunables.
    :param noise: The activity noise, standard deviation 1.
    :param regions: float32 arrays, the detail, softness, drift, underlayer
        and strength fields, standard deviation 1, at the cap's size or a
        coarser cap's, which is zoomed to it.
    :param rngs: The generators of the ``octaves``, the ``crinkle``, the
        ``voronoi`` cells and the ``drift``.
    :param workers: Threads; the process's setting when ``None``.
    :param coarse: The size the drift's wind is drawn at; ``size`` when
        ``None``.
    :returns: float32 arrays ``(size, size)``: the ``relief``, its ``gain``,
        the ``drift`` and the ``base`` activity, as :class:`Surface` keeps
        them.
    """
    detail = _zoomed(regions[0], size)
    (lo1, hi1), (lo3, hi3) = look.detail_steps
    w3 = smoothstep(lo3, hi3, detail)
    w1 = smoothstep(lo1, hi1, detail)
    del detail
    hard = stepped_activity(noise, look.activity_range, look.activity_floor)
    lo, hi, floor = look.soft_activity
    gentle = stepped_activity(noise, (lo, hi), floor)
    soft = smoothstep(*look.soft_step, _zoomed(regions[1], size))
    base = ((1 - soft) * hard + soft * gentle).astype(np.float32)
    del hard, gentle
    levels = ridged_reliefs(size, look, rngs["octaves"], rngs["crinkle"], workers)
    relief = w1 * next(levels)
    w2 = 1 - w1 - w3
    del w1
    relief += w2 * next(levels)
    del w2
    relief += w3 * next(levels)
    del w3, levels
    relief += soft * look.voronoi_weight * voronoi_creases(size, look, rngs["voronoi"])
    del soft
    g0, g1 = look.strength
    gain = g0 + (g1 - g0) * smoothstep(*look.strength_step, _zoomed(regions[4], size))
    share = smoothstep(*look.drift_step, _zoomed(regions[2], size))
    drift = drift_streaks(size, look, base, share, rngs["drift"], coarse)
    return {"relief": relief, "gain": gain, "drift": drift, "base": base}


def surface(size: int, look: Look, hemisphere: str, workers: int | None) -> Surface:
    """Draw a hemisphere's closed pack surface.

    The region fields and the drift's wind, at 250 km and coarser, are drawn
    on a cap of :attr:`Look.region_cap` texels and zoomed.

    :param size: Texels along each side of the cap.
    :param look: The tunables.
    :param hemisphere: ``nh`` or ``sh``.
    :param workers: Threads; the process's setting when ``None``.
    :returns: The surface.
    """
    coarse = min(look.region_cap, size)
    regions = [
        band_noise(coarse, look.region_band_km, _rng(look, hemisphere, 60 + k))
        for k in range(5)
    ]
    noise = band_noise(size, look.activity_band_km, _rng(look, hemisphere, 5))
    streams = (("octaves", 31), ("crinkle", 32), ("voronoi", 21), ("drift", 15))
    rngs = {name: _rng(look, hemisphere, stream) for name, stream in streams}
    made = ridged_surface(size, look, noise, regions, rngs, workers, coarse)
    return Surface(**{name: value.astype(np.float16) for name, value in made.items()})


@dataclass(frozen=True)
class Fractures:
    """The fracture systems drawn on the cap."""

    open: np.ndarray
    """float16 share of the sea's fill each texel's fracture shows, its
    darkness times how much of the texel it covers."""

    rank: np.ndarray | None
    """float16 rank of that fracture's system, uniform from 0 to 1: a system
    is drawn where its rank is below the share drawn there; ``None`` where
    the drawing faded the systems by their rank itself, as jagged leads do."""

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


LEAD_STEP = 0.8
"""Longest segment of a jagged lead, in texels: a segment is drawn only to
about its middle's neighbors, so its path is densified to this."""


def _densify(points: np.ndarray) -> np.ndarray:
    out = [points[:1]]
    for a, b in itertools.pairwise(points):
        pieces = max(int(np.ceil(np.linalg.norm(b - a) / LEAD_STEP)), 1)
        t = (np.arange(1, pieces + 1) / pieces)[:, None]
        out.append(a + t * (b - a))
    return np.concatenate(out)


def _arc_km(points: np.ndarray, spacing: float) -> np.ndarray:
    steps = np.linalg.norm(np.diff(points, axis=0), axis=1)
    return np.concatenate([[0.0], np.cumsum(steps)]) * spacing


def _runs(length: float, run_km: tuple[float, float], rng: np.random.Generator):
    """Cut positions along ``length`` km at spacings uniform over ``run_km``."""
    cuts = [0.0]
    while cuts[-1] < length:
        cuts.append(cuts[-1] + rng.uniform(*run_km))
    return np.array(cuts)


def _smooth_noise(n: int, span: float, rng: np.random.Generator) -> np.ndarray:
    span = max(int(span), 1)
    noise = np.convolve(
        rng.standard_normal(n + 2 * span), np.hanning(2 * span + 1), "same"
    )[span : span + n]
    return noise / max(float(noise.std()), 1e-6)


def _echelon(
    points: np.ndarray, look: Look, spacing: float, rng: np.random.Generator
) -> list[np.ndarray]:
    """Break a lead into straight pieces, each turned a little off the course
    and offset to alternate sides, overlapping their neighbors."""
    piece_km, turn_range, offset_km, overlap_km = look.echelon
    s = _arc_km(points, spacing)
    cuts = _runs(s[-1], piece_km, rng)
    cuts[-1] = s[-1]
    turn = rng.choice([-1.0, 1.0]) * rng.uniform(*turn_range)
    cos, sin = np.cos(turn), np.sin(turn)
    turned = np.array([[cos, -sin], [sin, cos]])
    pieces = []
    for number, (a, b) in enumerate(itertools.pairwise(cuts)):
        over = rng.uniform(*overlap_km)
        count = max(int((b - a + 2 * over) / spacing / LEAD_STEP), 2)
        span = np.linspace(a - over, b + over, count)
        inside = np.clip(span, 0, s[-1])
        piece = np.stack([np.interp(inside, s, points[:, k]) for k in range(2)], axis=1)
        tangent = piece[-1] - piece[0]
        tangent /= max(float(np.linalg.norm(tangent)), 1e-9)
        piece = piece + np.outer(span - inside, tangent) / spacing
        centre = piece.mean(axis=0)
        piece = centre + (piece - centre) @ turned
        side = 1 if number % 2 == 0 else -1
        offset = side * rng.uniform(*offset_km) / 2
        normal = np.array([-tangent[1], tangent[0]])
        pieces.append(piece + normal * offset / spacing)
    return pieces


def _lead_profile(
    points: np.ndarray,
    width: float,
    darkness: float,
    look: Look,
    spacing: float,
    rng: np.random.Generator,
) -> tuple[np.ndarray, np.ndarray]:
    """Return a piece's width and darkness per segment: the swell and the
    darkness's variation along it, the taper at its ends, and the pinches."""
    n = len(points) - 1
    span = 60.0 / spacing / LEAD_STEP
    w = width * np.clip(1 + 0.25 * _smooth_noise(n, span, rng), 0.7, 1.3)
    t = (np.arange(n) + 0.5) / n
    low = look.echelon_taper
    taper = low + (1 - low) * np.sin(np.pi * t) ** 0.5
    vary = _smooth_noise(n, span, rng)
    d = darkness * taper * np.clip(1 + look.fracture_dark_vary * vary, 0.3, 1.7)
    s = _arc_km(points, spacing)
    middle = 0.5 * (s[:-1] + s[1:])
    cuts = _runs(s[-1], look.pinch_run_km, rng)
    values = np.where(
        rng.random(len(cuts)) < look.pinch_shut,
        0.0,
        rng.uniform(*look.pinch_scale, len(cuts)),
    )
    step = values[np.clip(np.searchsorted(cuts, middle) - 1, 0, len(cuts) - 1)]
    return w * gaussian_filter1d(step, 0.6), d


def draw_jagged(
    size: int,
    systems: list[list[tuple[np.ndarray, float, float]]],
    ranks: np.ndarray,
    look: Look,
    rng: np.random.Generator,
) -> np.ndarray:
    """Draw fracture systems as thin leads in pieces en echelon, pinched.

    Every system is drawn, faded by its rank as :func:`month_cap` fades the
    systems where the concentration is 1, so the fade is part of the drawing.
    Each texel takes its strongest segment: a box filter of a texel against a
    line of the segment's width.

    :param size: Texels along each side of the cap.
    :param systems: As :func:`fracture_paths` returns them.
    :param ranks: Each system's rank.
    :param look: The tunables.
    :param rng: The generator of the pieces, the pinches and the variation.
    :returns: float32 array ``(size, size)``: the share of the sea's fill.
    """
    spacing = texel_km(size)
    starts, ends, widths, darks = [], [], [], []
    for number, fractures_ in enumerate(systems):
        fade = float(np.clip((1.0 - ranks[number]) / 0.15, 0, 1))
        if fade <= 0:
            continue
        for path, width, darkness in fractures_:
            if len(path) < 2:
                continue
            dense = _densify(np.asarray(path, dtype=np.float64))
            for piece in _echelon(dense, look, spacing, rng):
                if len(piece) < 2:
                    continue
                dark = darkness * fade * look.jagged_darkness
                w, d = _lead_profile(piece, width, dark, look, spacing, rng)
                starts.append(piece[:-1])
                ends.append(piece[1:])
                widths.append(w)
                darks.append(d)
    out = np.zeros(size * size, dtype=np.float32)
    if not starts:
        return out.reshape(size, size)
    a, b = np.concatenate(starts), np.concatenate(ends)
    width, dark = np.concatenate(widths), np.concatenate(darks)
    reach = int(np.ceil(float(width.max()) / 2 + 1)) + 1
    middle = np.floor((a + b) / 2).astype(np.int64)
    ab = b - a
    length2 = np.maximum((ab**2).sum(axis=1), 1e-9)
    for dr in range(-reach, reach + 1):
        for dc in range(-reach, reach + 1):
            cell = middle + np.array([dr, dc])
            inside = (cell >= 0).all(axis=1) & (cell < size).all(axis=1)
            t = np.clip(((cell - a) * ab).sum(axis=1) / length2, 0, 1)
            distance = np.linalg.norm(cell - (a + t[:, None] * ab), axis=1)
            share = np.clip(width / 2 + 0.5 - distance, 0, np.minimum(width, 1))
            value = share * dark
            keep = inside & (value > 0.002)
            np.maximum.at(
                out,
                cell[keep, 0] * size + cell[keep, 1],
                value[keep].astype(np.float32),
            )
    return np.clip(out, 0, 1).reshape(size, size)


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
    if look.jagged:
        styles = _rng(look, hemisphere, 70)
        shade = draw_jagged(size, systems, ranks, look, styles).astype(np.float16)
        rank = None
    else:
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
class Cascade:
    """The cascade's cells, the coarsest level first."""

    ids: np.ndarray
    """int32 array ``(levels, size, size)``: each texel's cell at each level,
    counted from the level's first."""

    u: np.ndarray
    """float32 uniform per cell, every level's after the one before: whether
    the cell resolves at its level."""

    v: np.ndarray
    """float32 uniform per cell: whether it resolves to ice."""

    starts: np.ndarray
    """int64 array ``(levels + 1,)``: where each level's cells start in
    :attr:`u` and :attr:`v`."""


def cascade_cells(
    size: int,
    look: Look,
    rngs: list[np.random.Generator],
    workers: int | None = None,
) -> Cascade:
    """Return the cascade's cells: per level a jittered grid, each texel in
    the cell of its nearest point after a curl shift that scales with the
    cell, so the borders are curved, and two uniforms per cell.

    :param size: Texels along each side of the cap.
    :param look: The tunables.
    :param rngs: One generator per level, which draws its shift, its points
        and its uniforms in that order.
    :param workers: Threads for the queries of the cells' points; the
        process's setting when ``None``. The levels are made in turn, which
        bounds the memory.
    :returns: The cells.
    """

    threads = parallel.workers(workers)

    def level(k: int) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
        rng, cell_km = rngs[k], look.cascade_km[k]
        cell = cell_km / texel_km(size)
        shift = curl_shift(
            size, (cell_km / 2, cell_km * 2), look.cascade_warp * cell_km, rng
        )
        count = int(np.ceil(size / cell)) + 2
        grid = np.stack(np.mgrid[-1 : count - 1, -1 : count - 1], axis=-1)
        grid = grid.reshape(-1, 2)
        points = (grid + rng.random(grid.shape)) * cell
        tree = cKDTree(points)
        ids = np.empty((size, size), np.int32)
        for start, stop in rows(size):
            r = np.arange(start, stop, dtype=np.float32)[:, None] + shift[0, start:stop]
            c = np.arange(size, dtype=np.float32)[None, :] + shift[1, start:stop]
            query = np.stack([r.ravel(), c.ravel()], -1)
            _, index = tree.query(query, k=1, workers=threads)
            ids[start:stop] = index.reshape(r.shape)
        u = rng.random(len(points)).astype(np.float32)
        v = rng.random(len(points)).astype(np.float32)
        return ids, u, v

    made = [level(k) for k in range(len(look.cascade_km))]
    lengths = [len(u) for _, u, _ in made]
    return Cascade(
        np.stack([ids for ids, _, _ in made]),
        np.concatenate([u for _, u, _ in made]),
        np.concatenate([v for _, _, v in made]),
        np.concatenate([[0], np.cumsum(lengths)]).astype(np.int64),
    )


@dataclass(frozen=True)
class FloeNoise:
    """The noise fields the floes read, fixed on the plane."""

    target: np.ndarray
    """float32 noise the regional ice target leans with, standard deviation 1."""

    speckle: np.ndarray
    """float32 noise of the calm speckle, standard deviation 1."""

    pick: np.ndarray
    """float32 uniform from 0 to 1, smooth: where the speckle takes over."""

    levels: np.ndarray
    """float32 array ``(241,)``: levels of the speckle's noise."""

    wet: np.ndarray
    """float64 array ``(241,)``: the share of the area that is water when the
    speckle's threshold is at each level, never falling."""


def speckle_table(field_: np.ndarray, half: float) -> tuple[np.ndarray, np.ndarray]:
    """Tabulate the share of a field's area below a soft threshold.

    :param field_: The noise, standard deviation 1.
    :param half: Half the threshold's soft step.
    :returns: The levels and the shares, as :class:`FloeNoise` keeps them.
    """
    sample = np.asarray(field_[::4, ::4], np.float32)
    levels = np.linspace(-6, 6, 241, dtype=np.float32)
    wet = np.array(
        [float(smoothstep(t + half, t - half, sample).mean()) for t in levels]
    )
    return levels, np.maximum.accumulate(wet)


def floe_noise(size: int, look: Look, rngs: list[np.random.Generator]) -> FloeNoise:
    """Draw the floes' noise fields.

    :param size: Texels along each side of the cap.
    :param look: The tunables.
    :param rngs: The generators of the target's noise, of each speckle octave
        and of the handover, in that order.
    :returns: The fields.
    """
    _, target_km = look.floe_target
    target = band_noise(size, target_km, rngs[0]).astype(np.float32)
    octaves = [
        np.float32(weight) * band_noise(size, km, rng).astype(np.float32)
        for (km, weight), rng in zip(look.speckle_octaves, rngs[1:-1], strict=True)
    ]
    speckle = normalized(sum(octaves).astype(np.float32))
    del octaves
    pick = ndtr(band_noise(size, look.speckle_pick_km, rngs[-1]).astype(np.float32))
    levels, wet = speckle_table(speckle, look.speckle_soft)
    return FloeNoise(target, speckle, pick, levels, wet)


@dataclass(frozen=True)
class Coast:
    """The land on the cap and what the coast rule reads of it."""

    land: np.ndarray
    """bool array ``(size, size)``: less than half of the texel is water."""

    band: np.ndarray
    """bool array ``(size, size)``: the sea within the coast rule's distance
    of the coastline."""

    nearest: np.ndarray
    """int32 array ``(size, size)``: the flat index of each texel's nearest
    texel of sea, its own for the sea."""


_FACE_PLANE: dict[str, Callable[[np.ndarray, np.ndarray, np.ndarray], tuple]] = {
    "px": lambda x, y, z: (-z, -y),
    "nx": lambda x, y, z: (z, -y),
    "py": lambda x, y, z: (x, z),
    "ny": lambda x, y, z: (x, -z),
    "pz": lambda x, y, z: (x, -y),
    "nz": lambda x, y, z: (-x, -y),
}
"""Each face's ``s`` and ``t`` of a direction scaled to 1 along its axis, as
:func:`texture_pipeline.cube.face_directions` combines them."""


def sample_faces(
    faces: Mapping[str, np.ndarray], x: np.ndarray, y: np.ndarray, z: np.ndarray
) -> np.ndarray:
    """Sample cube faces bilinearly along directions.

    :param faces: Every face's array, by name, all of one size.
    :param x: Direction components toward longitude 90 E.
    :param y: Direction components toward the north pole.
    :param z: Direction components toward longitude 0.
    :returns: float32 array of the directions' shape.
    """
    components = np.stack([x, y, z])
    major = np.argmax(np.abs(components), axis=0)
    out = np.zeros(x.shape, dtype=np.float32)
    for face, to_plane in _FACE_PLANE.items():
        axis = "xyz".index(face[1])
        along = components[axis]
        on = (major == axis) & ((along > 0) if face[0] == "p" else (along < 0))
        if not on.any():
            continue
        scale = np.abs(along[on])
        s, t = to_plane(x[on] / scale, y[on] / scale, z[on] / scale)
        size = faces[face].shape[0]
        col = (np.arctan(s) / (np.pi / 4) + 1) / 2 * size - 0.5
        row = (np.arctan(t) / (np.pi / 4) + 1) / 2 * size - 0.5
        out[on] = map_coordinates(
            faces[face], [row, col], order=1, mode="nearest", output=np.float32
        )
    return out


def cap_land(
    water: Mapping[str, np.ndarray],
    hemisphere: str,
    size: int,
    workers: int | None = None,
) -> np.ndarray:
    """Return the land on a hemisphere's cap, from the faces' water masks.

    A texel is land where less than half of it is water, its share of water
    the mean of the masks at four points inside it.

    :param water: uint8 arrays of every face, 255 for open water, by name.
    :param hemisphere: ``nh`` or ``sh``.
    :param size: Texels along each side of the cap.
    :param workers: Threads for the bands of rows; the process's setting when
        ``None``. The result does not depend on it.
    :returns: bool array ``(size, size)``.
    """
    quarter = RHO_MAX / size / 2
    land = np.empty((size, size), dtype=bool)

    def band(span: tuple[int, int]) -> None:
        start, stop = span
        x, y = cap_texel_centers(size, start, stop)
        share = np.zeros(x.shape, dtype=np.float32)
        for dx, dy in ((-1, -1), (-1, 1), (1, -1), (1, 1)):
            latitude, longitude = cap_to_latlon(
                x + dx * quarter, y + dy * quarter, hemisphere
            )
            phi, lam = np.radians(latitude), np.radians(longitude)
            share += sample_faces(
                water,
                np.cos(phi) * np.sin(lam),
                np.sin(phi),
                np.cos(phi) * np.cos(lam),
            )
        land[start:stop] = share < 2 * 255

    parallel.pmap(band, rows(size), workers)
    return land


def coast_fields(land: np.ndarray, look: Look) -> Coast:
    """Return what the floes and the coast rule read of the land.

    The coast rule's band is the sea whose distance to the nearest land
    texel, less half a texel, is within :attr:`Look.coast_rule`'s, so it is
    measured from the coastline rather than from the land texel's center.

    :param land: bool array ``(size, size)``.
    :param look: The tunables.
    :returns: The coast.
    """
    size = land.shape[0]
    spacing = texel_km(size)
    km = look.coast_rule[0] if look.coast_rule is not None else 0.0
    distance = distance_transform_edt(~land).astype(np.float32) * spacing
    distance = np.where(land, 0, distance - spacing / 2).astype(np.float32)
    band = ~land & (distance <= km)
    del distance
    _, (row, col) = distance_transform_edt(land, return_indices=True)
    nearest = (row.astype(np.int64) * size + col).astype(np.int32)
    return Coast(land, band, nearest)


def zone(c: np.ndarray, lo: float, hi: float, ramp: float) -> np.ndarray:
    """1 inside ``lo`` to ``hi``, 0 outside, with soft steps ``ramp`` either
    side."""
    return (
        smoothstep(lo - ramp, lo + ramp, c) * smoothstep(hi + ramp, hi - ramp, c)
    ).astype(np.float32)


def gauss_target(mean: np.ndarray, noise: np.ndarray, sigma: float) -> np.ndarray:
    """Return shares around a mean share: ``Phi(mu + sigma n)`` has the mean
    ``mean`` for a standard normal ``n`` when ``mu = Phi^-1(mean)
    sqrt(1 + sigma**2)``; 0 or 1 where the mean is."""
    m = np.clip(mean, 1e-5, 1 - 1e-5)
    mu = ndtri(m) * np.float32(np.sqrt(1 + sigma * sigma))
    out = ndtr(mu + np.float32(sigma) * noise).astype(np.float32)
    out[mean <= 0] = 0
    out[mean >= 1] = 1
    return out


def dark_share(c: np.ndarray, look: Look) -> np.ndarray:
    """The share of the water that is dark at a concentration."""
    return smoothstep(*look.gap_dark, c).astype(np.float32)


def mean_dark(c: np.ndarray, look: Look) -> np.ndarray:
    """The water's mean darkness at a concentration: gray and dark mixed."""
    return (look.gap_gray + (1 - look.gap_gray) * dark_share(c, look)).astype(
        np.float32
    )


def water_share(c: np.ndarray, lost: np.ndarray, look: Look) -> np.ndarray:
    """Return the share of each texel the floes' water takes, before its
    darkness, so that the leads' open water and the floes' together meet the
    concentration, gray water counting as part ice.

    With :attr:`Look.dark_gaps` all of the water is dark, so its share is
    the shortfall itself.

    :param c: The concentration.
    :param lost: The leads' share of the fill, smoothed.
    :param look: The tunables.
    :returns: float32 array of ``c``'s shape.
    """
    kept = np.clip(c / np.maximum(1 - lost, 1e-4), 0, 1)
    if look.dark_gaps:
        return (1 - kept).astype(np.float32)
    return np.clip((1 - kept) / mean_dark(c, look), 0, 1).astype(np.float32)


def toned(c: np.ndarray, tone: tuple[float, float]) -> np.ndarray:
    """Return the concentration the settled floes read through
    :attr:`Look.floe_tone`: ``c + (1 - c) smoothstep(lo, hi, c)``, which
    rises with ``c``, is 0 where ``c`` is and 1 from ``hi`` up.

    :param c: The concentration, from 0 to 1.
    :param tone: ``(lo, hi)``.
    :returns: float32 array of ``c``'s shape.
    """
    lo, hi = tone
    c = np.asarray(c, np.float32)
    return (c + (1 - c) * smoothstep(lo, hi, c)).astype(np.float32)


def _resolving(
    c: np.ndarray,
    k: int,
    scale_: tuple[tuple[float, float], ...],
    calm: tuple[float, float, float],
    calm_level: int,
) -> np.ndarray:
    scale = np.interp(c, [p[0] for p in scale_], [p[1] for p in scale_])
    share = (1 / (1 + np.exp(-2.0 * (k + 0.5 - scale.astype(np.float32))))).astype(
        np.float32
    )
    if k == calm_level:
        share = share + (1 - share) * zone(c, *calm)
    return share


def resolving(c: np.ndarray, k: int, look: Look) -> np.ndarray:
    """Return the share of the cells still open at level ``k`` that resolve
    there, at concentrations ``c``."""
    return _resolving(c, k, look.cascade_scale, look.calm_zone, look.calm_level)


@functools.cache
def z_table(
    scale_: tuple[tuple[float, float], ...],
    deltas: tuple[float, ...],
    calm: tuple[float, float, float],
    calm_level: int,
) -> np.ndarray:
    """Return the probit offset that makes the expected ice of the cascade
    an ice share, on a grid of 51 concentrations by 201 ice shares.

    It depends on the schedule alone, so it is made once.
    """
    levels = len(deltas)
    cs = np.linspace(0, 1, 51, dtype=np.float32)
    zs = np.linspace(-7, 7, 561, dtype=np.float32)
    fs = np.linspace(0, 1, 201, dtype=np.float32)
    table = np.empty((len(cs), len(fs)), np.float32)
    for i, c in enumerate(cs):
        at = np.array([c], np.float32)
        weight, alive = [], 1.0
        for k in range(levels):
            share = (
                1.0
                if k == levels - 1
                else float(_resolving(at, k, scale_, calm, calm_level)[0])
            )
            weight.append(alive * share)
            alive *= 1 - share
        expected = sum(w * ndtr(zs + d) for w, d in zip(weight, deltas, strict=True))
        table[i] = np.interp(fs, np.maximum.accumulate(expected), zs)
    table.flags.writeable = False
    return table


def z_lookup(table: np.ndarray, c: np.ndarray, f: np.ndarray) -> np.ndarray:
    """Read :func:`z_table` bilinearly."""
    ci = np.clip(c, 0, 1) * (table.shape[0] - 1)
    fi = np.clip(f, 0, 1) * (table.shape[1] - 1)
    c0 = np.minimum(ci.astype(np.int32), table.shape[0] - 2)
    f0 = np.minimum(fi.astype(np.int32), table.shape[1] - 2)
    tc, tf = ci - c0, fi - f0
    return (
        table[c0, f0] * (1 - tc) * (1 - tf)
        + table[c0 + 1, f0] * tc * (1 - tf)
        + table[c0, f0 + 1] * (1 - tc) * tf
        + table[c0 + 1, f0 + 1] * tc * tf
    )


CELL_CHUNK = 1 << 20
"""Cells whose decisions one thread works out at once."""


def cascade_whole(
    cells: Cascade,
    ice: np.ndarray,
    c: np.ndarray,
    tag: np.ndarray,
    look: Look,
    workers: int | None = None,
) -> np.ndarray:
    """Return ice (1) or water (0) per texel.

    Coarsest first, a share of the cells still open resolves, each to ice
    with probability ``Phi(z + delta)``; the finest level resolves all. A
    cell is decided whole from its mean concentration and mean ice share,
    and cut where ``tag`` changes, so a cell on the coastline is two cells.

    :param cells: The cells.
    :param ice: The regional ice target.
    :param c: The concentration.
    :param tag: int or bool array of ``c``'s shape, 1 on land, and on the
        closed pack with :attr:`Look.floe_tone`, and 0 on the sea.
    :param look: The tunables.
    :param workers: Threads for the cells' decisions; the process's setting
        when ``None``. The result does not depend on it.
    :returns: float32 array of ``c``'s shape.
    """
    levels = len(look.cascade_km)
    ntag = int(tag.max()) + 1
    out = np.zeros(c.size, np.float32)
    undecided = np.ones(c.size, bool)
    flat_tag = tag.ravel()
    for k in range(levels):
        key = (cells.ids[k].ravel() * np.int32(ntag) + flat_tag).astype(np.intp)
        resolved, solid = _decide(
            cells, k, key, ntag, c.ravel(), ice.ravel(), look, workers
        )
        here = undecided & resolved[key]
        out[here] = solid[key][here]
        undecided &= ~here
    return out.reshape(c.shape)


def _decide(
    cells: Cascade,
    k: int,
    key: np.ndarray,
    ntag: int,
    conc: np.ndarray,
    share_ice: np.ndarray,
    look: Look,
    workers: int | None,
) -> tuple[np.ndarray, np.ndarray]:
    """Return whether each of a level's cells, by key, resolves and is ice."""
    levels = len(look.cascade_km)
    table = z_table(
        look.cascade_scale, look.cascade_deltas, look.calm_zone, look.calm_level
    )
    first, last = int(cells.starts[k]), int(cells.starts[k + 1])
    span = (last - first) * ntag
    count = np.bincount(key, minlength=span)
    present = np.flatnonzero(count)
    many = count[present]
    rm = (np.bincount(key, conc, span)[present] / many).astype(np.float32)
    fm = (np.bincount(key, share_ice, span)[present] / many).astype(np.float32)
    cell = first + present // ntag
    resolved = np.zeros(span, bool)
    solid = np.zeros(span, bool)

    def chunk(start: int) -> None:
        here = slice(start, start + CELL_CHUNK)
        mean, at = rm[here], present[here]
        share = resolving(mean, k, look) if k < levels - 1 else 1.0
        resolved[at] = cells.u[cell[here]] < share
        z = z_lookup(table, mean, fm[here]) + look.cascade_deltas[k]
        solid[at] = cells.v[cell[here]] < ndtr(z)

    parallel.pmap(chunk, range(0, len(present), CELL_CHUNK), workers)
    return resolved, solid


def floes_water(
    c: np.ndarray,
    lost: np.ndarray,
    tag: np.ndarray,
    cells: Cascade,
    noise: FloeNoise,
    look: Look,
    workers: int | None = None,
    closed: np.ndarray | None = None,
) -> np.ndarray:
    """Return the floes' water: the cascade, the calm speckle in the calm
    zone handed over region by region, open water where the concentration
    is 0, and none where the pack is closed.

    :param c: The concentration, 0 on land.
    :param lost: The leads' share of the fill, smoothed.
    :param tag: int or bool array, 1 where the cells are cut off from the
        open sea, on land and on the closed pack, and 0 on the open sea.
    :param cells: The cascade's cells.
    :param noise: The floes' noise fields.
    :param look: The tunables.
    :param workers: Threads for the bands of rows and the cells; the
        process's setting when ``None``. The result does not depend on it.
    :param closed: bool array of ``c``'s shape: the closed pack, which is ice
        whatever the cells decide; none when ``None``.
    :returns: float32 array of ``c``'s shape from 0 to 1.
    """
    sigma, _ = look.floe_target
    half = look.speckle_soft
    ice = np.empty(c.shape, np.float32)
    specks = np.empty(c.shape, np.float32)

    def targets(span: tuple[int, int]) -> None:
        here = slice(*span)
        share = water_share(c[here], lost[here], look)
        ice[here] = gauss_target(1 - share, noise.target[here], sigma)
        wet = gauss_target(share, noise.target[here], sigma)
        theta = np.interp(wet, noise.wet, noise.levels).astype(np.float32)
        specks[here] = smoothstep(theta + half, theta - half, noise.speckle[here])

    parallel.pmap(targets, _bands(c.shape[0]), workers)
    solid = cascade_whole(cells, ice, c, tag, look, workers)
    water = np.empty(c.shape, np.float32)

    def combine(span: tuple[int, int]) -> None:
        here = slice(*span)
        calm = noise.pick[here] < zone(c[here], *look.calm_zone)
        drawn = np.where(calm, specks[here], 1 - solid[here]).astype(np.float32)
        drawn[c[here] <= 0] = 1
        if closed is not None:
            drawn[closed[here]] = 0
        water[here] = drawn

    parallel.pmap(combine, _bands(c.shape[0]), workers)
    return water


def _bands(size: int) -> list[tuple[int, int]]:
    return [
        (a, min(a + MONTH_BAND_ROWS, size)) for a in range(0, size, MONTH_BAND_ROWS)
    ]


def patch_dark(
    water: np.ndarray,
    c: np.ndarray,
    land: np.ndarray,
    look: Look,
    rng: np.random.Generator,
) -> np.ndarray:
    """Return the water's darkness per texel: one per patch, gray or dark.

    Patches are the 8-connected pieces of water, more than half water, land
    excluded; every texel takes the color of its nearest patch. Per bin of
    the patches' mean concentration the largest patches turn dark first,
    their sizes jittered, while that brings the dark water's area closer to
    the share :func:`mean_dark` assumes, so small cracks stay gray into the
    loose ice and the first dark water is a large lead.

    :param water: The floes' water.
    :param c: The concentration, 0 on land.
    :param land: bool array of the land.
    :param look: The tunables.
    :param rng: The generator of the jitter.
    :returns: float32 array of ``c``'s shape: the share of the fill.
    """
    labels, n = label((water > 0.5) & ~land, structure=np.ones((3, 3), bool))
    if n == 0:
        return mean_dark(c, look)
    flat = labels.ravel()
    area = np.bincount(flat, minlength=n + 1)[1:].astype(np.float64)
    wet = np.bincount(flat, water.ravel(), n + 1)[1:]
    mean_c = np.bincount(flat, c.ravel(), n + 1)[1:] / area
    order = -np.log(area) + look.gap_jitter * rng.standard_normal(n)
    want = wet * dark_share(mean_c.astype(np.float32), look)
    dark = np.zeros(n, bool)
    bins = np.floor(mean_c / look.gap_bin + 1e-6).astype(np.int64)
    for b in np.unique(bins):
        idx = np.nonzero(bins == b)[0]
        idx = idx[np.argsort(order[idx], kind="stable")]
        target, got = float(want[idx].sum()), 0.0
        for i in idx:
            if abs(got + wet[i] - target) < abs(got - target):
                dark[i] = True
                got += wet[i]
    shade = np.where(dark, 1.0, look.gap_gray).astype(np.float32)
    iy, ix = distance_transform_edt(
        labels == 0, return_distances=False, return_indices=True
    )
    return shade[labels[iy, ix] - 1]


def blurred(field_: np.ndarray, sigma: float, workers: int | None = None) -> np.ndarray:
    """Return :func:`scipy.ndimage.gaussian_filter` of a float32 field with
    ``mode="nearest"``, its two passes on threads by bands; the result is
    the same."""
    out = np.empty_like(field_)
    size = field_.shape[0]

    def columns(span: tuple[int, int]) -> None:
        start, stop = span
        gaussian_filter1d(
            field_[:, start:stop], sigma, 0, output=out[:, start:stop], mode="nearest"
        )

    def across(span: tuple[int, int]) -> None:
        start, stop = span
        out[start:stop] = gaussian_filter1d(out[start:stop], sigma, 1, mode="nearest")

    parallel.pmap(columns, rows(size), workers)
    parallel.pmap(across, rows(size), workers)
    return out


def settled_coverage(
    conc: np.ndarray,
    gaps: np.ndarray,
    fields: Fields,
    look: Look,
    workers: int | None = None,
) -> tuple[np.ndarray, np.ndarray]:
    """Return a month's coverage of the settled floes over a whole cap.

    The floes' water is calibrated against the leads, drawn by the cascade
    and the calm speckle, and colored one patch at a time, or all dark with
    :attr:`Look.dark_gaps`; the coverage is what the leads and that water
    leave of the ice, and the coast rule adds ice along the coast. With
    :attr:`Look.floe_tone` the floes read the concentration through its
    curve, and the pack at its top and above is cut off from the cells and
    closed. A texel of land takes the coverage of its nearest texel of sea,
    which is what the coast's land blends with and what a face texel of sea
    beside the coast reads of it.

    :param conc: The concentration through the eddy shift, the record's
        extended into land.
    :param gaps: The leads' share of the fill.
    :param fields: The hemisphere's noise fields.
    :param look: The tunables.
    :param workers: Threads; the process's setting when ``None``.
    :returns: The coverage, and the floes' alone, before the leads and the
        coast rule; float32 arrays of ``conc``'s shape.
    """
    assert fields.coast is not None
    assert fields.cascade is not None
    assert fields.floe_noise is not None
    land = fields.coast.land
    sea = np.where(land, 0, conc).astype(np.float32)
    lost = blurred(gaps, look.floe_lost_km / texel_km(conc.shape[0]), workers)
    read, tag, closed = sea, land, None
    if look.floe_tone is not None:
        read = toned(sea, look.floe_tone)
        closed = sea >= look.floe_tone[1]
        tag = land | closed
    water = floes_water(
        read, lost, tag, fields.cascade, fields.floe_noise, look, workers, closed
    )
    del lost, tag, closed
    if look.dark_gaps:
        floes = np.subtract(1, water, out=water)
    else:
        floes = patch_dark(water, read, land, look, _rng(look, fields.hemisphere, 90))
        floes *= water
        np.subtract(1, floes, out=floes)
    del water, read
    coverage = 1 - gaps
    coverage *= floes
    if look.coast_rule is not None:
        coverage[fields.coast.band & (sea >= look.coast_rule[1])] = 1
    del sea
    coverage = coverage.ravel()[fields.coast.nearest.ravel()].reshape(conc.shape)
    return coverage, floes


@dataclass(frozen=True)
class Fields:
    """The noise fields of one hemisphere, the same in every month."""

    hemisphere: str
    """``nh`` or ``sh``."""

    concentration: np.ndarray
    """float32 array ``(12, n, n)``: each month's mean on the coarser cap."""

    threshold: np.ndarray | None
    """float32 array ``(size, size)``: the concentration at which a texel
    turns to ice; ``None`` with the cascade, which has no edge model."""

    shift: np.ndarray
    """float32 array ``(2, size, size)``: where each texel reads the
    concentration from, in texels, row first."""

    leads: Leads | None
    """The lead network; ``None`` when the fractures replace it."""

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

    surface: Surface | None = None
    """The closed pack's ridged surface; with :attr:`Look.ridged`."""

    cascade: Cascade | None = None
    """The cascade's cells; with :attr:`Look.cascade`."""

    floe_noise: FloeNoise | None = None
    """The settled floes' noise fields; with :attr:`Look.cascade`."""

    coast: Coast | None = None
    """The land on the cap; with :attr:`Look.cascade`."""

    @property
    def size(self) -> int:
        """Texels along each side of the cap."""
        return self.shift.shape[-1]


@dataclass(frozen=True)
class Inputs:
    """What a part of the noise fields is made from."""

    concentration: np.ndarray
    """float array ``(12, n, n)`` from 0 to 1."""

    hemisphere: str
    """``nh`` or ``sh``."""

    size: int
    """Texels along each side of the cap."""

    look: Look
    """The tunables."""

    done: Mapping[str, Mapping[str, object]] = field(default_factory=dict)
    """The parts finished so far, by name: the ones the part needs."""

    workers: int | None = None
    """Threads for the steps inside a part; the process's setting when ``None``."""

    land: np.ndarray | None = None
    """bool array ``(size, size)``: the land on the cap, as :func:`cap_land`
    makes it from the faces' water masks; for the parts that read it."""


def make_edge(inputs: Inputs) -> dict[str, object]:
    """Draw the threshold the concentration is read against."""
    look, size = inputs.look, inputs.size
    doubled = edge_orientation(inputs.concentration, size, look)
    fine = band_noise(size, look.edge_band_km, _rng(look, inputs.hemisphere, 0))
    stretched = streak(
        fine, doubled, look.edge_streak_km, look.edge_band_km[0], inputs.workers
    )
    del doubled
    stretched *= np.float32(np.sqrt(1 - look.edge_isotropic**2))
    stretched += np.float32(look.edge_isotropic) * fine
    del fine
    stretched = normalized(stretched)
    threshold = (
        look.edge_center + look.edge_spread * (2 * ndtr(stretched) - 1)
    ).astype(np.float32)
    return {"threshold": threshold}


def make_eddies(inputs: Inputs) -> dict[str, object]:
    """Draw the shift that swirls the edge."""
    look = inputs.look
    shift = curl_shift(
        inputs.size,
        look.eddy_band_km,
        look.eddy_shift_km,
        _rng(look, inputs.hemisphere, 1),
    )
    return {"shift": shift}


def make_leads(inputs: Inputs) -> dict[str, object]:
    """Draw the lead network."""
    look = inputs.look
    return {"leads": leads(inputs.size, look, _rng(look, inputs.hemisphere, 2))}


def make_classic(inputs: Inputs) -> dict[str, object]:
    """Draw the broad brightness and the melt mottling of the first look."""
    look, size = inputs.look, inputs.size
    brightness = band_noise(
        size, look.brightness_band_km, _rng(look, inputs.hemisphere, 3)
    ).astype(np.float16)
    mottle = band_noise(
        size, look.mottle_band_km, _rng(look, inputs.hemisphere, 4)
    ).astype(np.float16)
    return {"brightness": brightness, "mottle": mottle}


def make_activity(inputs: Inputs) -> dict[str, object]:
    """Draw where the pack is flat and where it is structured."""
    look = inputs.look
    activity = activity_field(inputs.size, look, _rng(look, inputs.hemisphere, 5))
    return {"activity": activity}


def make_crumple(inputs: Inputs) -> dict[str, object]:
    """Draw the crumpled snow."""
    look = inputs.look
    slope, creases, plates = crumple(
        inputs.size, look, _rng(look, inputs.hemisphere, 6)
    )
    return {"slope": slope, "creases": creases, "plates": plates}


def make_fractures(inputs: Inputs) -> dict[str, object]:
    """Trace and draw the fracture systems, where the activity says."""
    look = inputs.look
    cracks = fractures(
        inputs.size,
        look,
        inputs.hemisphere,
        inputs.done["activity"]["activity"],
        inputs.concentration,
        _rng(look, inputs.hemisphere, 7),
    )
    return {"fractures": cracks}


def make_melt(inputs: Inputs) -> dict[str, object]:
    """Draw the melt's tone."""
    look = inputs.look
    melt = band_noise(inputs.size, look.melt_band_km, _rng(look, inputs.hemisphere, 9))
    return {"melt": melt.astype(np.float16)}


def make_floes(inputs: Inputs) -> dict[str, object]:
    """Draw the floes of the marginal ice."""
    look = inputs.look
    return {"floes": floe_cells(inputs.size, look, _rng(look, inputs.hemisphere, 8))}


def make_surface(inputs: Inputs) -> dict[str, object]:
    """Draw the closed pack's ridged surface."""
    look = inputs.look
    made = surface(inputs.size, look, inputs.hemisphere, inputs.workers)
    return {"surface": made}


def make_cascade(inputs: Inputs) -> dict[str, object]:
    """Draw the cascade's cells."""
    look, hemisphere = inputs.look, inputs.hemisphere
    rngs = [_rng(look, hemisphere, 85 + k) for k in range(len(look.cascade_km))]
    return {"cascade": cascade_cells(inputs.size, look, rngs, inputs.workers)}


def make_floe_noise(inputs: Inputs) -> dict[str, object]:
    """Draw the settled floes' noise fields."""
    look, hemisphere = inputs.look, inputs.hemisphere
    streams = [80, *range(81, 81 + len(look.speckle_octaves)), 84]
    rngs = [_rng(look, hemisphere, stream) for stream in streams]
    return {"floe_noise": floe_noise(inputs.size, look, rngs)}


def make_coast(inputs: Inputs) -> dict[str, object]:
    """Read the land on the cap for the floes and the coast rule."""
    if inputs.land is None:
        msg = "The settled floes need the land on the cap."
        raise ValueError(msg)
    return {"coast": coast_fields(inputs.land, inputs.look)}


@dataclass(frozen=True)
class Part:
    """One independent piece of a hemisphere's noise fields.

    Each part draws from a random stream of its own, so the parts can be made
    in any order or at once, and one that nothing changed can be kept.
    """

    name: str
    """What the part is called, in reports and in the cache."""

    make: Callable[[Inputs], dict[str, object]]
    """Draws it: the :class:`Fields` attributes it sets, by name."""

    needs: tuple[str, ...] = ()
    """The parts it reads, which are made first."""

    reads_record: bool = False
    """Whether it reads the concentration."""

    reads_land: bool = False
    """Whether it reads the land on the cap."""


type PartCache = Callable[[Part, Callable[[], dict[str, object]]], dict[str, object]]
"""Takes a part and what makes it, and returns what it is: from a cache, or
by calling the maker."""


def parts_for(look: Look, hemisphere: str) -> list[Part]:
    """Return the parts of a hemisphere's noise fields that a look needs.

    :param look: The tunables.
    :param hemisphere: ``nh`` or ``sh``.
    :returns: The parts, longest first, each after the ones it needs.
    """
    crumpled = look.crumple
    cracked = crumpled and look.fractures
    parts = []
    if crumpled and look.ridged:
        parts.append(Part("surface", make_surface))
    elif crumpled:
        parts.append(Part("crumple", make_crumple))
    if crumpled and look.cascade:
        parts.append(Part("cascade", make_cascade))
        parts.append(Part("floe noise", make_floe_noise))
        parts.append(Part("coast", make_coast, reads_land=True))
    else:
        parts.append(Part("edge", make_edge, reads_record=True))
    if crumpled and look.floes and not look.cascade:
        parts.append(Part("floes", make_floes))
    if crumpled:
        parts.append(Part("activity", make_activity))
    if cracked:
        parts.append(
            Part("fractures", make_fractures, needs=("activity",), reads_record=True)
        )
    parts.append(Part("eddies", make_eddies))
    if not cracked:
        parts.append(Part("leads", make_leads))
    if crumpled and look.melt_tone and hemisphere == "nh":
        parts.append(Part("melt", make_melt))
    if not crumpled:
        parts.append(Part("classic", make_classic))
    return parts


def run_parts(
    parts: list[Part],
    inputs: Inputs,
    report: Callable[[str], None] = lambda _: None,
    workers: int | None = None,
    cache: PartCache | None = None,
) -> dict[str, dict[str, object]]:
    """Make parts of the noise fields, the independent ones at once.

    :param parts: The parts, each after the ones it needs.
    :param inputs: What they are made from.
    :param report: Told each part's name as its making starts.
    :param workers: Parts made at once; the process's setting when ``None``.
    :param cache: Takes each part and its maker, and returns the part from
        wherever it was kept or by calling the maker.
    :returns: Each part's attributes, by the part's name.
    """
    done: dict[str, dict[str, object]] = {}
    seen = dataclasses.replace(inputs, done=done)

    def run(part: Part) -> dict[str, object]:
        report(part.name)

        def make() -> dict[str, object]:
            return part.make(seen)

        return make() if cache is None else cache(part, make)

    threads = min(parallel.workers(workers), len(parts))
    if threads <= 1:
        for part in parts:
            done[part.name] = run(part)
        return done
    waiting = list(parts)
    running = {}
    with ThreadPoolExecutor(threads) as pool:
        while waiting or running:
            for part in [p for p in waiting if all(n in done for n in p.needs)]:
                waiting.remove(part)
                running[pool.submit(run, part)] = part
            if not running:
                msg = f"No part can be made: {[p.name for p in waiting]} wait."
                raise ValueError(msg)
            finished, _ = wait(running, return_when=FIRST_COMPLETED)
            for future in finished:
                done[running.pop(future).name] = future.result()
    return {part.name: done[part.name] for part in parts}


def prepare(
    concentration: np.ndarray,
    hemisphere: str,
    size: int,
    look: Look = LOOK,
    report: Callable[[str], None] = lambda _: None,
    fast: np.ndarray | None = None,
    workers: int | None = None,
    cache: PartCache | None = None,
    land: np.ndarray | None = None,
) -> Fields:
    """Generate a hemisphere's noise fields.

    The parts of the fields that nothing joins, the edge, the eddies, the
    crumple and the rest, are made at once on threads; each draws from a random
    stream of its own, so they are what a serial run makes.

    :param concentration: float array ``(12, n, n)`` from 0 to 1.
    :param hemisphere: ``nh`` or ``sh``.
    :param size: Texels along each side of the cap the ice is generated on.
    :param look: The tunables.
    :param report: Told the name of each part as its making starts.
    :param fast: How much the fast ice band raised the concentration, as
        :func:`fast_ice_raise` returns it; kept with the floes.
    :param workers: Threads; the process's setting when ``None``, and ``1``
        makes the fields serially.
    :param cache: Takes each part and its maker, and returns the part from
        wherever it was kept or by calling the maker.
    :param land: The land on the cap, as :func:`cap_land` makes it; the
        settled floes need it.
    :returns: The fields.
    """
    inputs = Inputs(concentration, hemisphere, size, look, workers=workers, land=land)
    made = run_parts(parts_for(look, hemisphere), inputs, report, workers, cache)
    kept = {key: value for part in made.values() for key, value in part.items()}
    broken = kept.get("floes")
    return Fields(
        hemisphere,
        concentration.astype(np.float32),
        kept.get("threshold"),
        kept["shift"],
        kept.get("leads"),
        brightness=kept.get("brightness"),
        mottle=kept.get("mottle"),
        activity=kept.get("activity"),
        slope=kept.get("slope"),
        creases=kept.get("creases"),
        plates=kept.get("plates"),
        fractures=kept.get("fractures"),
        floes=broken,
        melt=kept.get("melt"),
        fast=(
            fast.astype(np.float16) if fast is not None and broken is not None else None
        ),
        surface=kept.get("surface"),
        cascade=kept.get("cascade"),
        floe_noise=kept.get("floe_noise"),
        coast=kept.get("coast"),
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

    floes: np.ndarray | None = None
    """float32 array ``(size, size)``: what the settled floes' water leaves of
    the ice, before the leads and the coast rule; with the cascade."""

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


MONTH_BAND_ROWS = 128
"""Cap rows :func:`month_cap` hands to a thread at once."""


def read_concentration(
    fields: Fields, month: int, start: int, stop: int
) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    """Read a month's concentration through the eddy shift in rows of a cap,
    as the ice is drawn from it.

    :param fields: The hemisphere's noise fields.
    :param month: The month, 1 to 12.
    :param start: The first row.
    :param stop: The row after the last.
    :returns: The rows and the columns read from, in texels of the cap, and
        the concentration there; float arrays ``(stop - start, size)``.
    """
    size = fields.size
    field_ = fields.concentration[month - 1]
    scale = field_.shape[-1] / size
    r = np.arange(start, stop, dtype=np.float32)[:, None] + fields.shift[0, start:stop]
    c = np.arange(size, dtype=np.float32)[None, :] + fields.shift[1, start:stop]
    conc = map_coordinates(
        field_,
        [(r + 0.5) * scale - 0.5, (c + 0.5) * scale - 0.5],
        order=3,
        mode="nearest",
        prefilter=False,
    )
    return r, c, conc


def month_cap(
    fields: Fields,
    month: int,
    look: Look = LOOK,
    row_range: tuple[int, int] | None = None,
    workers: int | None = None,
) -> Cap:
    """Generate the coverage and brightness of one month.

    :param fields: The hemisphere's noise fields.
    :param month: The month, 1 to 12.
    :param look: The tunables.
    :param row_range: The first cap row and the one after the last to make,
        for a region; the other rows of the arrays are zero.
    :param workers: Threads for the bands of rows; the process's setting when
        ``None``. The result does not depend on it.
    :returns: The month's cap.
    """
    size = fields.size
    field_ = fields.concentration[month - 1]
    scale = field_.shape[-1] / size
    alloc = np.empty if row_range is None else np.zeros
    settled = fields.cascade is not None
    coverage = None if settled else alloc((size, size), dtype=np.float32)
    brightness = alloc((size, size), dtype=np.float32)
    land = alloc((size, size), dtype=np.float32) if look.coast_blend else None
    activity = alloc((size, size), dtype=np.float32) if look.crumple else None
    whole = None
    opened = fields.fractures is not None or fields.floes is not None
    if opened and look.coast_blend and not settled:
        whole = alloc((size, size), dtype=np.float32)
    concs = np.empty((size, size), dtype=np.float32) if settled else None
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

    def shifted(start: int, stop: int) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
        return read_concentration(fields, month, start, stop)

    def read(span: tuple[int, int]) -> None:
        assert concs is not None
        concs[span[0] : span[1]] = shifted(*span)[2]

    def work(span: tuple[int, int]) -> None:
        start, stop = span
        if concs is None:
            r, c, conc = shifted(start, stop)
        else:
            conc = concs[start:stop]
        covered = None
        if fields.threshold is not None:
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
            if fields.fractures.rank is None:
                gaps = fields.fractures.open[start:stop]
            else:
                closed, wide = look.fracture_share
                share = closed + (wide - closed) * smoothstep(1.0, 0.85, conc)
                rank = fields.fractures.rank[start:stop]
                drawn = np.clip((share - rank) / 0.15, 0, 1)
                gaps = fields.fractures.open[start:stop] * drawn
            if covered is not None:
                coverage[start:stop] = covered * (1 - gaps)
            lead = 0.0
        else:
            drawn = fields.leads.rank[start:stop] < share
            lead = look.lead_darkening * fields.leads.profile[start:stop] * drawn
        if activity is not None and fields.surface is not None:
            ridged = fields.surface
            base = ridged.base[start:stop].astype(np.float32)
            active = 1 - (1 - base) * (
                1
                - look.activity_edge
                * smoothstep(
                    look.activity_edge_range[0], look.activity_edge_range[1], conc
                )
            )
            activity[start:stop] = active
            gain = ridged.gain[start:stop].astype(np.float32)
            relief = ridged.relief[start:stop].astype(np.float32)
            drift = ridged.drift[start:stop].astype(np.float32)
            texture = thin * (1 + gain * active * relief + gain * drift)
        elif activity is not None:
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

    first, last = (0, size) if row_range is None else row_range
    spans = [
        (a, min(a + MONTH_BAND_ROWS, last)) for a in range(first, last, MONTH_BAND_ROWS)
    ]
    floes = None
    if concs is not None:
        whole_cap = [
            (a, min(a + MONTH_BAND_ROWS, size)) for a in range(0, size, MONTH_BAND_ROWS)
        ]
        parallel.pmap(read, whole_cap, workers)
        parallel.pmap(work, spans, workers)
        gaps = (
            np.zeros((size, size), dtype=np.float32)
            if fields.fractures is None
            else fields.fractures.open.astype(np.float32)
        )
        coverage, floes = settled_coverage(concs, gaps, fields, look, workers)
        del gaps
    else:
        parallel.pmap(work, spans, workers)
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
        floes=floes,
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


@dataclass(frozen=True)
class Window:
    """A rectangle of a face, for working on a region instead of the whole."""

    size: int
    """The face's edge in texels."""

    rows: tuple[int, int]
    """The first row and the one after the last."""

    columns: tuple[int, int]
    """The first column and the one after the last."""

    @property
    def shape(self) -> tuple[int, int]:
        """The rectangle's height and width."""
        return self.rows[1] - self.rows[0], self.columns[1] - self.columns[0]


def composite_face(
    flat: np.ndarray,
    water: np.ndarray,
    face: str,
    ice: MonthIce,
    fill: Rgb,
    pool: Executor | None = None,
    band_rows: int = 256,
    window: Window | None = None,
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
    :param window: The rectangle of the face that ``flat`` and ``water`` hold,
        when they hold a region; the whole face when ``None``.
    :returns: Per hemisphere, the ice's area on this face in square
        kilometers: coverage times the mask's share of water times the
        texel's area.
    """
    size, first, columns, height = _extent(flat, window)
    fill_rgb = np.asarray(fill, dtype=np.float32)

    def band(start: int) -> tuple[float, float]:
        stop = min(start + band_rows, height)
        share = water[start:stop]
        wet = share > 0
        if not wet.any():
            return 0.0, 0.0
        x, y, z = (
            axis[wet]
            for axis in face_directions(
                face, size, first + start, first + stop, columns
            )
        )
        coverage, rgb = ice.sample(x, y, z)
        weight = share[wet].astype(np.float32) / 255 * coverage
        texels = flat[start:stop][wet].astype(np.float32)
        texels += weight[:, None] * (rgb - fill_rgb)
        flat[start:stop][wet] = np.clip(texels + 0.5, 0, 255).astype(np.uint8)
        area = weight * texel_solid_angles(x, y, z, size) * AUTHALIC_RADIUS_KM**2
        north = float(area[y > 0].sum())
        return north, float(area.sum()) - north

    starts = range(0, height, band_rows)
    bands = map(band, starts) if pool is None else pool.map(band, starts)
    north, south = (sum(values) for values in zip(*bands, strict=True))
    return {"nh": north, "sh": south}


def _extent(
    flat: np.ndarray, window: Window | None
) -> tuple[int, int, tuple[int, int] | None, int]:
    if window is None:
        return flat.shape[0], 0, None, flat.shape[0]
    return window.size, window.rows[0], window.columns, window.shape[0]


def winter_months(table: tuple[Rgb, ...]) -> list[int]:
    """Return the months whose color is a table's brightest, its winter.

    :param table: Twelve colors, January first.
    :returns: The months, 1 to 12.
    """
    brightest = max(table, key=sum)
    return [month + 1 for month, color in enumerate(table) if color == brightest]


def rescaled_colors(
    table: tuple[Rgb, ...], winter: Rgb, darkening: float = 1.0
) -> tuple[Rgb, ...]:
    """Return a table with a new winter color and every month's darkening kept.

    Each month's color is the new winter color times the month's ratio to the
    table's winter color, per channel; with a ``darkening`` other than 1 the
    ratio ``r`` becomes ``1 - (1 - r) darkening``, so the winter months keep
    the new winter color and the others darken by that share of what they
    did.

    :param table: Twelve colors, January first.
    :param winter: The new color of the table's brightest months.
    :param darkening: The share of each month's darkening kept.
    :returns: Twelve colors.
    """
    before = np.asarray(max(table, key=sum), dtype=np.float64)
    after = np.asarray(winter, dtype=np.float64)
    months = np.asarray(table, dtype=np.float64)
    if darkening == 1:
        scaled = after * months / before
    else:
        scaled = after * (1 - (1 - months / before) * darkening)
    scaled = np.clip(np.rint(scaled), 0, 255)
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


def _blurred(lum: np.ndarray, snowy: np.ndarray, inner: np.ndarray) -> list[np.ndarray]:
    """Return the four fields that are smoothed to read the land around.

    :param lum: Luminance.
    :param snowy: Whether a texel is snow.
    :param inner: The land at least a distance from the water, 0 or 1.
    :returns: How much land there is, the snow, and the snow's luminance and
        its square.
    """
    snow = inner * snowy
    return [inner, snow, lum * snow, lum * lum * snow]


def _surroundings(
    blurred: list[np.ndarray],
) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    """Read the land around each texel from :func:`_blurred`'s fields after the
    smoothing: how much of it there is, how much of it is snow, and the
    standard deviation of the snow's luminance."""
    read, covered, mean, square = blurred
    share = covered / np.maximum(read, 1e-6)
    mean /= np.maximum(covered, 1e-6)
    square /= np.maximum(covered, 1e-6)
    relief = np.sqrt(np.maximum(square - mean * mean, 0.0))
    relief[covered < 0.02] = np.inf
    return read, share, relief


def coast_weight(
    source: np.ndarray,
    water: np.ndarray,
    look: Look = LOOK,
    spacing_km: float | None = None,
    workers: int | None = None,
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
    :param workers: Threads for the distance transform, the colors and the
        eight Gaussian filters; the process's setting when ``None``. The
        result does not depend on it.
    :returns: float32 array ``(N, N)`` from 0 to 1, 0 on open water.
    """
    size = water.shape[0]
    spacing = (
        np.pi / 2 * AUTHALIC_RADIUS_KM / size if spacing_km is None else spacing_km
    )

    def distance() -> np.ndarray:
        inland = distance_transform_edt(water < 128).astype(np.float32)
        inland *= np.float32(spacing)
        return inland

    def colors() -> tuple[np.ndarray, np.ndarray, np.ndarray]:
        rgb = source.astype(np.float32)
        lum = rgb @ np.array([0.2126, 0.7152, 0.0722], dtype=np.float32)
        top = rgb.max(axis=-1)
        snowy = (lum > look.coast_snow[0]) & (
            top - rgb.min(axis=-1) < look.coast_snow[1] * np.maximum(top, 1)
        )
        return lum, snowy, imagery_sea(source, look)

    inland, (lum, snowy, sea) = parallel.pmap(
        lambda make: make(), [distance, colors], workers
    )
    reach = smoothstep(look.coast_reach_km[1], look.coast_reach_km[0], inland)
    reach[water == 255] = 0
    if not reach.any():
        return reach
    sigma = look.coast_window_km / spacing
    near_km, far_km = look.coast_inner_km
    inside = [((inland >= km) & ~sea).astype(np.float32) for km in (near_km, far_km)]
    smooth = parallel.pmap(
        lambda field_: gaussian_filter(field_, sigma, mode="nearest"),
        [f for inner in inside for f in _blurred(lum, snowy, inner)],
        workers,
    )
    del lum, snowy, inside
    _, share, relief = _surroundings(smooth[:4])
    read, far_share, far_relief = _surroundings(smooth[4:])
    del smooth
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
    weight: np.ndarray | None = None,
    window: Window | None = None,
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
    :param weight: The :func:`coast_weight` of ``source`` and ``water`` when
        it is at hand, changed in place as the result is; made when ``None``.
    :param window: The rectangle of the face that the arrays hold, when they
        hold a region; the whole face when ``None``.
    :returns: float32 array ``(N, N)``: ``k`` times the land share, the share
        of each texel the ice took.
    """
    size, first, columns, height = _extent(flat, window)
    if weight is None:
        weight = coast_weight(source, water, look, spacing_km)
    fill_rgb = np.asarray(fill if fill is not None else (0, 0, 0), dtype=np.float32)

    def band(start: int) -> None:
        stop = min(start + band_rows, height)
        here = weight[start:stop] > 0
        if not here.any():
            return
        x, y, z = (
            axis[here]
            for axis in face_directions(
                face, size, first + start, first + stop, columns
            )
        )
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

    starts = range(0, height, band_rows)
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
