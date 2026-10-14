"""Tests for the settled sea ice look: the ridged surface, the jagged leads, the
cascade of floes, the land on the cap, the coast rule, and the review's
changes: the tone curve, the dark gaps and the colors."""

import dataclasses

import numpy as np
import pytest

from texture_pipeline import sea_ice, sea_ice_look
from texture_pipeline import sea_ice_preview as sp
from texture_pipeline.cube import FACES, face_directions

SIZE = 384


def settled_look(**changes: object) -> sea_ice_look.Look:
    look = dataclasses.replace(sea_ice_look.LOOK, **sp.VARIANTS["settled"].look)
    return dataclasses.replace(look, **changes)


@pytest.fixture(scope="module")
def cap_spacing():
    """Draw small squares at the polar cap's spacing, as the labs did, so the
    finest features are wider than a texel."""
    with pytest.MonkeyPatch.context() as patch:
        spacing = sea_ice.texel_km(sea_ice.CAP_SIZE)
        patch.setattr(sea_ice_look, "texel_km", lambda size: spacing)
        yield


def coast_land(size: int = SIZE) -> np.ndarray:
    """Land over the left third, and an island."""
    yy, xx = np.mgrid[0:size, 0:size]
    return (xx < size // 3) | ((xx - 0.7 * size) ** 2 + (yy - 0.3 * size) ** 2 < 15**2)


def stack(value: float) -> np.ndarray:
    return np.full((12, 32, 32), value, dtype=np.float32)


@pytest.fixture(scope="module")
def fields(cap_spacing) -> sea_ice_look.Fields:
    values = stack(0.9)
    values[:, :, 20:] = 0.45
    return sea_ice_look.prepare(values, "nh", SIZE, settled_look(), land=coast_land())


def floes_at(
    cells: sea_ice_look.Cascade,
    noise: sea_ice_look.FloeNoise,
    c: np.ndarray,
    land: np.ndarray,
    look: sea_ice_look.Look,
) -> np.ndarray:
    lost = np.zeros(c.shape, np.float32)
    water = sea_ice_look.floes_water(c, lost, land.astype(np.int32), cells, noise, look)
    rng = np.random.default_rng(0)
    dark = sea_ice_look.patch_dark(water, c, land, look, rng)
    return 1 - dark * water


@pytest.fixture(scope="module")
def floe_parts(cap_spacing):
    look = settled_look()
    rngs = [np.random.default_rng([1, k]) for k in range(5)]
    cells = sea_ice_look.cascade_cells(512, look, rngs, workers=2)
    noise = sea_ice_look.floe_noise(
        512, look, [np.random.default_rng([2, k]) for k in range(5)]
    )
    return cells, noise


class TestFloes:
    @pytest.mark.parametrize("conc", [0.95, 0.85, 0.7, 0.55, 0.4, 0.25])
    def test_the_coverage_follows_the_concentration(self, floe_parts, conc) -> None:
        cells, noise = floe_parts
        c = np.full((512, 512), conc, np.float32)
        land = np.zeros(c.shape, bool)
        covered = floes_at(cells, noise, c, land, settled_look())
        assert abs(float(covered.mean()) - conc) < 0.03

    def test_closed_pack_has_no_water(self, floe_parts) -> None:
        cells, noise = floe_parts
        c = np.ones((512, 512), np.float32)
        covered = floes_at(cells, noise, c, np.zeros(c.shape, bool), settled_look())
        assert covered.min() == 1

    def test_no_concentration_is_open_water(self, floe_parts) -> None:
        cells, noise = floe_parts
        c = np.zeros((512, 512), np.float32)
        covered = floes_at(cells, noise, c, np.zeros(c.shape, bool), settled_look())
        assert covered.max() == 0

    def test_a_patch_of_water_has_one_color(self, floe_parts) -> None:
        _, xx = np.mgrid[0:512, 0:512]
        c = np.clip(1.0 - xx / 512, 0, 1).astype(np.float32)
        look = settled_look()
        land = np.zeros(c.shape, bool)
        water = sea_ice_look.floes_water(
            c, np.zeros(c.shape, np.float32), land.astype(np.int32), *floe_parts, look
        )
        dark = sea_ice_look.patch_dark(water, c, land, look, np.random.default_rng(0))
        labels, n = sea_ice_look.label(water > 0.5, structure=np.ones((3, 3), bool))
        for patch in range(1, n + 1):
            assert np.unique(dark[labels == patch]).size == 1
        assert set(np.unique(dark[water > 0.5])) <= {np.float32(look.gap_gray), 1.0}

    def test_the_largest_gaps_turn_dark_first(self, floe_parts) -> None:
        cells, noise = floe_parts
        c = np.full((512, 512), 0.75, np.float32)
        look = settled_look()
        land = np.zeros(c.shape, bool)
        water = sea_ice_look.floes_water(
            c, np.zeros(c.shape, np.float32), land.astype(np.int32), cells, noise, look
        )
        dark = sea_ice_look.patch_dark(water, c, land, look, np.random.default_rng(0))
        labels, n = sea_ice_look.label(water > 0.5, structure=np.ones((3, 3), bool))
        area = np.bincount(labels.ravel())[1:]
        shade = np.array([dark[labels == k + 1][0] for k in range(n)])
        assert area[shade == 1].mean() > 2 * area[shade < 1].mean()

    def test_cells_are_cut_at_the_coastline(self, floe_parts) -> None:
        cells, _ = floe_parts
        c = np.full((512, 512), 0.6, np.float32)
        land = np.zeros(c.shape, bool)
        land[:, :200] = True
        c[land] = 0
        ice = np.where(land, 0, 0.6).astype(np.float32)
        tag = land.astype(np.int32)
        look = settled_look()
        cut = sea_ice_look.cascade_whole(cells, ice, c, tag, look)
        whole = sea_ice_look.cascade_whole(cells, ice, c, np.zeros_like(tag), look)
        near = np.zeros(c.shape, bool)
        near[:, 200:212] = True
        offshore = np.zeros(c.shape, bool)
        offshore[:, 300:] = True
        assert abs(cut[near].mean() - cut[offshore].mean()) < 0.08
        assert cut[near].mean() > whole[near].mean() + 0.05


class TestCoastRule:
    def coverage(self, fields, conc: float, rule) -> tuple[np.ndarray, np.ndarray]:
        look = settled_look(coast_rule=rule)
        c = np.full((SIZE, SIZE), conc, np.float32)
        gaps = np.zeros_like(c)
        coast = sea_ice_look.coast_fields(fields.coast.land, look)
        moved = dataclasses.replace(fields, coast=coast)
        return sea_ice_look.settled_coverage(c, gaps, moved, look)[0], coast.band

    def test_the_band_is_ice_at_half_and_more(self, fields) -> None:
        covered, band = self.coverage(fields, 0.6, (10.0, 0.5))
        assert band.any()
        assert covered[band].min() == 1

    def test_the_rule_only_adds(self, fields) -> None:
        for conc in (0.3, 0.6):
            ruled, band = self.coverage(fields, conc, (10.0, 0.5))
            plain, _ = self.coverage(fields, conc, None)
            sea = ~fields.coast.land
            assert (ruled[sea] >= plain[sea]).all()
            np.testing.assert_array_equal(ruled[sea & ~band], plain[sea & ~band])
            if conc < 0.5:
                np.testing.assert_array_equal(ruled[band], plain[band])

    def test_the_band_reaches_ten_kilometers(self, fields) -> None:
        land = fields.coast.land
        _, band = self.coverage(fields, 0.6, (10.0, 0.5))
        reach = sea_ice_look.distance_transform_edt(~land)[band].max()
        spacing = sea_ice.texel_km(sea_ice.CAP_SIZE)
        assert (reach - 0.5) * spacing <= 10.0
        assert (reach + 0.5) * spacing > 10.0

    def test_land_takes_the_coverage_of_the_nearest_sea(self, fields) -> None:
        covered, _ = self.coverage(fields, 0.3, (10.0, 0.5))
        land = fields.coast.land
        nearest = fields.coast.nearest.ravel()
        assert not land.ravel()[nearest].any()
        np.testing.assert_array_equal(covered.ravel(), covered.ravel()[nearest])


class TestLand:
    def test_a_face_texel_center_reads_its_own_texel(self) -> None:
        rng = np.random.default_rng(3)
        faces = {f: rng.integers(0, 256, (16, 16), dtype=np.uint8) for f in FACES}
        for face in FACES:
            x, y, z = face_directions(face, 16, 0, 16)
            got = sea_ice_look.sample_faces(faces, x, y, z)
            np.testing.assert_allclose(got, faces[face], atol=1e-3)

    @pytest.mark.parametrize("hemisphere", sea_ice.HEMISPHERES)
    def test_the_land_is_where_the_masks_hold_less_than_half_water(
        self, hemisphere
    ) -> None:
        size = 256
        faces = {}
        for face in FACES:
            x, y, z = face_directions(face, 512, 0, 512)
            latitude = np.degrees(np.arctan2(y, np.hypot(x, z)))
            faces[face] = np.where(np.abs(latitude) > 75, 0, 255).astype(np.uint8)
        land = sea_ice_look.cap_land(faces, hemisphere, size, workers=2)
        x, y = sea_ice.cap_texel_centers(size)
        latitude, _ = sea_ice.cap_to_latlon(x, y, hemisphere)
        clear = np.abs(np.abs(latitude) - 75) > 0.5
        np.testing.assert_array_equal(land[clear], (np.abs(latitude) > 75)[clear])

    def test_the_concentration_on_land_draws_no_ice_offshore(self, fields) -> None:
        cap = sea_ice_look.month_cap(fields, 3, settled_look(), workers=2)
        land = fields.coast.land
        assert cap.floes is not None
        sea = ~land
        assert 0.5 < cap.coverage[sea].mean() < 1


class TestSurface:
    def test_the_detail_levels_share_their_crests(self, cap_spacing) -> None:
        look = settled_look()
        first, second, third = sea_ice_look.ridged_reliefs(
            256, look, np.random.default_rng(1), np.random.default_rng(2), workers=2
        )
        assert np.corrcoef(first.ravel(), third.ravel())[0, 1] > 0.5
        assert np.corrcoef(first.ravel(), second.ravel())[0, 1] > 0.7

    def test_shadow_is_twice_the_light(self, fields) -> None:
        relief = fields.surface.relief.astype(np.float32)
        assert -relief.min() > 1.5 * relief.max()

    def test_the_strength_spans_its_range(self, fields) -> None:
        gain = fields.surface.gain.astype(np.float32)
        low, high = settled_look().strength
        assert gain.min() >= low - 1e-3
        assert gain.max() <= high + 1e-3

    def test_the_pack_keeps_its_color_on_the_mean(self, cap_spacing) -> None:
        land = np.zeros((SIZE, SIZE), bool)
        land[:4] = True
        closed = sea_ice_look.prepare(stack(1.0), "nh", SIZE, settled_look(), land=land)
        cap = sea_ice_look.month_cap(closed, 3, settled_look(), workers=2)
        assert abs(float(cap.brightness[~land].mean()) - 1) < 0.02


class TestJaggedLeads:
    def test_a_lead_breaks_into_pieces(self, cap_spacing) -> None:
        path = np.stack([np.full(200, 100.0), np.linspace(20, 360, 200)], axis=1)
        look = settled_look()
        drawn = sea_ice_look.draw_jagged(
            384,
            [[(path, 1.0, 0.2)]],
            np.zeros(1, np.float32),
            look,
            np.random.default_rng(4),
        )
        rows = np.nonzero(drawn.max(axis=1) > 0.01)[0]
        assert rows.max() - rows.min() >= 1
        assert drawn.max() <= 0.2 * look.jagged_darkness * 1.7 + 1e-6

    def test_pinches_shut_parts_of_a_lead(self, cap_spacing) -> None:
        path = np.stack([np.full(400, 100.0), np.linspace(10, 370, 400)], axis=1)
        look = settled_look(pinch_shut=0.5)
        drawn = sea_ice_look.draw_jagged(
            384,
            [[(path, 1.0, 0.2)]],
            np.zeros(1, np.float32),
            look,
            np.random.default_rng(5),
        )
        along = drawn[90:110, 20:360].max(axis=0)
        assert (along < 0.005).mean() > 0.1

    def test_the_leads_are_faded_by_rank_in_the_drawing(self, fields) -> None:
        assert fields.fractures.rank is None
        assert 0 < float(fields.fractures.open.astype(np.float32).max()) <= 1


class TestWorkers:
    def test_the_fields_do_not_depend_on_the_workers(self, cap_spacing) -> None:
        values = stack(0.8)
        serial = sea_ice_look.prepare(
            values, "sh", SIZE, settled_look(), workers=1, land=coast_land()
        )
        threaded = sea_ice_look.prepare(
            values, "sh", SIZE, settled_look(), workers=3, land=coast_land()
        )
        left, right = sp.fields_to_arrays(serial), sp.fields_to_arrays(threaded)
        assert left.keys() == right.keys()
        for name in left:
            np.testing.assert_array_equal(left[name], right[name], err_msg=name)

    def test_the_month_does_not_depend_on_the_workers(self, fields) -> None:
        look = settled_look()
        one = sea_ice_look.month_cap(fields, 8, look, workers=1)
        three = sea_ice_look.month_cap(fields, 8, look, workers=3)
        for name in ("coverage", "brightness", "floes", "land_brightness"):
            np.testing.assert_array_equal(getattr(one, name), getattr(three, name))

    def test_a_region_is_the_full_months_rows(self, fields) -> None:
        look = settled_look()
        full = sea_ice_look.month_cap(fields, 8, look, workers=2)
        part = sea_ice_look.month_cap(fields, 8, look, row_range=(100, 230), workers=2)
        for name in ("coverage", "brightness"):
            np.testing.assert_array_equal(
                getattr(part, name)[100:230], getattr(full, name)[100:230]
            )

    def test_the_fields_survive_the_cache(self, fields) -> None:
        again = sp.fields_from_arrays("nh", sp.fields_to_arrays(fields))
        assert again.threshold is None
        assert again.fractures.rank is None
        left, right = sp.fields_to_arrays(fields), sp.fields_to_arrays(again)
        assert left.keys() == right.keys()
        for name in left:
            np.testing.assert_array_equal(left[name], right[name], err_msg=name)


TONES = [sp.VARIANTS[name].look["floe_tone"] for name in ("tone-80-90", "tone-70-85")]


def toned_look(**changes: object) -> sea_ice_look.Look:
    look = dataclasses.replace(sea_ice_look.LOOK, **sp.VARIANTS["tone-80-90"].look)
    return dataclasses.replace(look, **changes)


def floes_of(fields, c: np.ndarray, look: sea_ice_look.Look) -> np.ndarray:
    gaps = np.zeros_like(c)
    return sea_ice_look.settled_coverage(c, gaps, fields, look)[1]


STEP_ROW = SIZE // 2 + 7


def step_rows(low: float, high: float) -> np.ndarray:
    return np.where(np.arange(SIZE) < STEP_ROW, low, high).astype(np.float32)


class TestToneCurve:
    @pytest.mark.parametrize("tone", TONES)
    def test_the_curve_rises_from_0_to_1(self, tone) -> None:
        lo, hi = tone
        c = np.linspace(0, 1, 2001, dtype=np.float32)
        read = sea_ice_look.toned(c, tone)
        assert read[0] == 0
        assert read[-1] == 1
        assert (np.diff(read) >= 0).all()
        assert (read >= c).all()
        assert (read[c >= hi] == 1).all()
        np.testing.assert_array_equal(read[c <= lo], c[c <= lo])

    @pytest.mark.parametrize("tone", TONES)
    @pytest.mark.parametrize("profile", ["ramp", "step"])
    def test_no_floes_water_at_and_above_the_top(self, fields, tone, profile) -> None:
        _, hi = tone
        if profile == "ramp":
            rows = np.linspace(0.6, 1.0, SIZE, dtype=np.float32)
        else:
            rows = step_rows(0.3, 0.95)
        c = np.repeat(rows[:, None], SIZE, axis=1)
        floes = floes_of(fields, c, toned_look(floe_tone=tone))
        sea = ~fields.coast.land
        above, below = sea & (c >= hi), sea & (c < hi)
        ids = fields.cascade.ids[0]
        straddling = np.isin(ids, np.intersect1d(ids[above], ids[below]))
        assert (above & straddling).sum() > 1000
        assert floes[above].min() == 1
        assert floes[below].min() < 1

    @pytest.mark.parametrize("tone", TONES)
    def test_the_open_side_is_cut_at_the_top(self, fields, tone) -> None:
        c = np.repeat(step_rows(0.75, 0.95)[:, None], SIZE, axis=1)
        look = toned_look(floe_tone=tone, coast_rule=None)
        floes = floes_of(fields, c, look)
        alone = floes_of(fields, np.full_like(c, 0.75), look)
        row = np.arange(SIZE)[:, None]
        sea = ~fields.coast.land
        near = sea & (row >= STEP_ROW - 12) & (row < STEP_ROW)
        assert (floes[near] < 1).any()
        assert (floes[near] == alone[near]).mean() > 0.99

    def test_the_floes_follow_the_curve(self, fields) -> None:
        tone = (0.7, 0.85)
        c = np.full((SIZE, SIZE), 0.78, np.float32)
        read = float(sea_ice_look.toned(c[:1, :1], tone)[0, 0])
        look = toned_look(floe_tone=tone, coast_rule=None)
        floes = floes_of(fields, c, look)[~fields.coast.land]
        assert abs(float(floes.mean()) - read) < 0.04
        assert float(floes.mean()) > 0.78 + 0.06

    def test_the_curves_differ_in_the_curve_alone(self) -> None:
        first, second = sp.VARIANTS["tone-80-90"], sp.VARIANTS["tone-70-85"]
        assert second.parent == first.name
        assert first.parent == "settled"
        same = {"name", "summary", "parent", "remarks", "look"}
        for f in dataclasses.fields(sp.Variant):
            if f.name not in same:
                assert getattr(first, f.name) == getattr(second, f.name), f.name
        changed = {
            k
            for k in first.look.keys() | second.look.keys()
            if first.look.get(k) != second.look.get(k)
        }
        assert changed == {"floe_tone"}


class TestDarkGaps:
    def test_the_water_share_is_the_shortfall(self) -> None:
        c = np.linspace(0, 1, 101, dtype=np.float32)
        look = toned_look()
        for lost in (0.0, 0.2):
            share = sea_ice_look.water_share(c, np.full_like(c, lost), look)
            expected = 1 - np.minimum(c / (1 - lost), 1)
            np.testing.assert_allclose(share, expected, atol=1e-6)

    def test_every_gap_is_the_fill(self, fields) -> None:
        c = np.full((SIZE, SIZE), 0.85, np.float32)
        sea = ~fields.coast.land
        dark = floes_of(fields, c, toned_look(floe_tone=None))[sea]
        gray = floes_of(fields, c, toned_look(floe_tone=None, dark_gaps=False))[sea]
        assert set(np.unique(dark)) == {0.0, 1.0}
        assert ((gray > 0) & (gray < 1)).any()
        assert abs(float(dark.mean()) - 0.85) < 0.04


MEASURED = {"nh": (247, 250, 251), "sh": (236, 240, 244)}


class TestColors:
    table = sea_ice_look.LOOK.colors

    def ratios(self, table: tuple) -> np.ndarray:
        rgb = np.asarray(table, np.float64)
        return rgb / np.asarray(max(table, key=sum), np.float64)

    def test_the_south_takes_the_midpoint_and_keeps_its_ratios(self) -> None:
        colors, winters = sp.snow_tables(self.table, MEASURED, south_toward_north=0.5)
        middle = (np.asarray(MEASURED["nh"]) + np.asarray(MEASURED["sh"])) / 2
        assert np.abs(np.asarray(winters["sh"]) - middle).max() <= 0.5
        expected = np.asarray(winters["sh"]) * self.ratios(self.table["sh"])
        assert np.abs(np.asarray(colors["sh"]) - expected).max() <= 0.5
        plain, _ = sp.snow_tables(self.table, MEASURED)
        assert colors["nh"] == plain["nh"]

    def test_the_summer_lift_leaves_winter_unchanged(self) -> None:
        lifted, _ = sp.snow_tables(self.table, MEASURED, north_darkening=2 / 3)
        plain, _ = sp.snow_tables(self.table, MEASURED)
        assert lifted["sh"] == plain["sh"]
        for month in sea_ice_look.winter_months(self.table["nh"]):
            assert lifted["nh"][month - 1] == MEASURED["nh"]
        r = self.ratios(self.table["nh"])
        expected = np.asarray(MEASURED["nh"]) * (1 - (1 - r) * 2 / 3)
        assert np.abs(np.asarray(lifted["nh"]) - expected).max() <= 0.5
        darkest = int(np.argmin(r.sum(axis=1)))
        assert (np.asarray(lifted["nh"][darkest]) > plain["nh"][darkest]).all()
