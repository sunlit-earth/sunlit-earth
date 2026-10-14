"""Tests for the sea ice's look: the ridged surface, the jagged leads, the
cascade of floes and its tone curve, the land on the cap, the coast rule, and
the colors."""

import dataclasses

import numpy as np
import pytest

from texture_pipeline import sea_ice, sea_ice_look
from texture_pipeline import sea_ice_preview as sp
from texture_pipeline.cube import FACES, face_directions

SIZE = 384

NO_RULE = (10.0, 1.1)
"""A coast rule whose concentration is never reached."""


def look(**changes: object) -> sea_ice_look.Look:
    return dataclasses.replace(sea_ice_look.LOOK, **changes)


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
    return sea_ice_look.prepare(values, "nh", SIZE, land=coast_land())


def floes_at(
    cells: sea_ice_look.Cascade,
    noise: sea_ice_look.FloeNoise,
    c: np.ndarray,
    land: np.ndarray,
) -> np.ndarray:
    lost = np.zeros(c.shape, np.float32)
    water = sea_ice_look.floes_water(
        c, lost, land.astype(np.int32), cells, noise, look()
    )
    return 1 - water


@pytest.fixture(scope="module")
def floe_parts(cap_spacing):
    rngs = [np.random.default_rng([1, k]) for k in range(5)]
    cells = sea_ice_look.cascade_cells(512, look(), rngs, workers=2)
    noise = sea_ice_look.floe_noise(
        512, look(), [np.random.default_rng([2, k]) for k in range(5)]
    )
    return cells, noise


def floes_of(fields, c: np.ndarray, toned: sea_ice_look.Look) -> np.ndarray:
    gaps = np.zeros_like(c)
    return sea_ice_look.month_coverage(c, gaps, fields, toned)[1]


class TestFloes:
    @pytest.mark.parametrize("conc", [0.95, 0.85, 0.7, 0.55, 0.4, 0.25])
    def test_the_coverage_follows_the_concentration(self, floe_parts, conc) -> None:
        cells, noise = floe_parts
        c = np.full((512, 512), conc, np.float32)
        covered = floes_at(cells, noise, c, np.zeros(c.shape, bool))
        assert abs(float(covered.mean()) - conc) < 0.03

    def test_closed_pack_has_no_water(self, floe_parts) -> None:
        cells, noise = floe_parts
        c = np.ones((512, 512), np.float32)
        assert floes_at(cells, noise, c, np.zeros(c.shape, bool)).min() == 1

    def test_no_concentration_is_open_water(self, floe_parts) -> None:
        cells, noise = floe_parts
        c = np.zeros((512, 512), np.float32)
        assert floes_at(cells, noise, c, np.zeros(c.shape, bool)).max() == 0

    def test_cells_are_cut_at_the_coastline(self, floe_parts) -> None:
        cells, _ = floe_parts
        c = np.full((512, 512), 0.6, np.float32)
        land = np.zeros(c.shape, bool)
        land[:, :200] = True
        c[land] = 0
        ice = np.where(land, 0, 0.6).astype(np.float32)
        tag = land.astype(np.int32)
        cut = sea_ice_look.cascade_whole(cells, ice, c, tag, look())
        whole = sea_ice_look.cascade_whole(cells, ice, c, np.zeros_like(tag), look())
        near = np.zeros(c.shape, bool)
        near[:, 200:212] = True
        offshore = np.zeros(c.shape, bool)
        offshore[:, 300:] = True
        assert abs(cut[near].mean() - cut[offshore].mean()) < 0.08
        assert cut[near].mean() > whole[near].mean() + 0.05

    def test_the_water_share_is_the_shortfall(self) -> None:
        c = np.linspace(0, 1, 101, dtype=np.float32)
        for lost in (0.0, 0.2):
            share = sea_ice_look.water_share(c, np.full_like(c, lost))
            expected = 1 - np.minimum(c / (1 - lost), 1)
            np.testing.assert_allclose(share, expected, atol=1e-6)

    def test_every_gap_is_the_fill(self, fields) -> None:
        c = np.full((SIZE, SIZE), 0.78, np.float32)
        sea = ~fields.coast.land
        floes = floes_of(fields, c, look(coast_rule=NO_RULE))[sea]
        assert set(np.unique(floes)) == {0.0, 1.0}


class TestCoastRule:
    def coverage(self, fields, conc: float, rule) -> tuple[np.ndarray, np.ndarray]:
        ruled = look(coast_rule=rule)
        c = np.full((SIZE, SIZE), conc, np.float32)
        gaps = np.zeros_like(c)
        coast = sea_ice_look.coast_fields(fields.coast.land, ruled)
        moved = dataclasses.replace(fields, coast=coast)
        return sea_ice_look.month_coverage(c, gaps, moved, ruled)[0], coast.band

    def test_the_band_is_ice_at_half_and_more(self, fields) -> None:
        covered, band = self.coverage(fields, 0.6, (10.0, 0.5))
        assert band.any()
        assert covered[band].min() == 1

    def test_the_rule_only_adds(self, fields) -> None:
        for conc in (0.3, 0.6):
            ruled, band = self.coverage(fields, conc, (10.0, 0.5))
            plain, _ = self.coverage(fields, conc, NO_RULE)
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
        cap = sea_ice_look.month_cap(fields, 3, workers=2)
        sea = ~fields.coast.land
        assert 0.5 < cap.coverage[sea].mean() < 1

    def test_without_land_the_floes_cannot_be_made(self) -> None:
        inputs = sea_ice_look.Inputs(stack(0.5), "nh", 64, look())
        with pytest.raises(ValueError, match="land on the cap"):
            sea_ice_look.make_coast(inputs)


class TestSurface:
    def test_the_detail_levels_share_their_crests(self, cap_spacing) -> None:
        first, second, third = sea_ice_look.ridged_reliefs(
            256, look(), np.random.default_rng(1), np.random.default_rng(2), workers=2
        )
        assert np.corrcoef(first.ravel(), third.ravel())[0, 1] > 0.5
        assert np.corrcoef(first.ravel(), second.ravel())[0, 1] > 0.7

    def test_shadow_is_twice_the_light(self, fields) -> None:
        relief = fields.surface.relief.astype(np.float32)
        assert -relief.min() > 1.5 * relief.max()

    def test_the_strength_spans_its_range(self, fields) -> None:
        gain = fields.surface.gain.astype(np.float32)
        low, high = look().strength
        assert gain.min() >= low - 1e-3
        assert gain.max() <= high + 1e-3

    def test_the_pack_keeps_its_color_on_the_mean(self, cap_spacing) -> None:
        land = np.zeros((SIZE, SIZE), bool)
        land[:4] = True
        closed = sea_ice_look.prepare(stack(1.0), "nh", SIZE, land=land)
        cap = sea_ice_look.month_cap(closed, 3, workers=2)
        assert abs(float(cap.brightness[~land].mean()) - 1) < 0.02


class TestJaggedLeads:
    def test_a_lead_breaks_into_pieces(self, cap_spacing) -> None:
        path = np.stack([np.full(200, 100.0), np.linspace(20, 360, 200)], axis=1)
        drawn = sea_ice_look.draw_jagged(
            384,
            [[(path, 1.0, 0.2)]],
            np.zeros(1, np.float32),
            look(),
            np.random.default_rng(4),
        )
        rows = np.nonzero(drawn.max(axis=1) > 0.01)[0]
        assert rows.max() - rows.min() >= 1
        assert drawn.max() <= 0.2 * look().jagged_darkness * 1.7 + 1e-6

    def test_pinches_shut_parts_of_a_lead(self, cap_spacing) -> None:
        path = np.stack([np.full(400, 100.0), np.linspace(10, 370, 400)], axis=1)
        drawn = sea_ice_look.draw_jagged(
            384,
            [[(path, 1.0, 0.2)]],
            np.zeros(1, np.float32),
            look(pinch_shut=0.5),
            np.random.default_rng(5),
        )
        along = drawn[90:110, 20:360].max(axis=0)
        assert (along < 0.005).mean() > 0.1

    def test_a_system_of_the_top_rank_fades_out(self, cap_spacing) -> None:
        path = np.stack([np.full(200, 100.0), np.linspace(20, 360, 200)], axis=1)
        drawn = sea_ice_look.draw_jagged(
            384,
            [[(path, 1.0, 0.2)]],
            np.ones(1, np.float32),
            look(),
            np.random.default_rng(4),
        )
        assert not drawn.any()

    def test_the_leads_show_a_share_of_the_fill(self, fields) -> None:
        assert 0 < float(fields.fractures.open.astype(np.float32).max()) <= 1

    def test_a_cap_without_ice_has_no_leads(self) -> None:
        land = np.zeros((128, 128), bool)
        bare = sea_ice_look.prepare(stack(0.0), "sh", 128, land=land)
        assert len(bare.fractures.systems) == 0
        assert not bare.fractures.open.any()


class TestWorkers:
    def test_the_fields_do_not_depend_on_the_workers(self, cap_spacing) -> None:
        values = stack(0.8)
        serial = sea_ice_look.prepare(values, "sh", SIZE, workers=1, land=coast_land())
        threaded = sea_ice_look.prepare(
            values, "sh", SIZE, workers=3, land=coast_land()
        )
        left, right = sp.fields_to_arrays(serial), sp.fields_to_arrays(threaded)
        assert left.keys() == right.keys()
        for name in left:
            np.testing.assert_array_equal(left[name], right[name], err_msg=name)

    def test_the_month_does_not_depend_on_the_workers(self, fields) -> None:
        one = sea_ice_look.month_cap(fields, 8, workers=1)
        three = sea_ice_look.month_cap(fields, 8, workers=3)
        for name in ("coverage", "brightness", "floes", "activity"):
            np.testing.assert_array_equal(getattr(one, name), getattr(three, name))

    def test_a_region_is_the_full_months_rows(self, fields) -> None:
        full = sea_ice_look.month_cap(fields, 8, workers=2)
        part = sea_ice_look.month_cap(fields, 8, row_range=(100, 230), workers=2)
        np.testing.assert_array_equal(part.coverage, full.coverage)
        np.testing.assert_array_equal(
            part.brightness[100:230], full.brightness[100:230]
        )
        assert not part.brightness[:100].any()
        assert not part.brightness[230:].any()

    def test_the_fields_survive_the_cache(self, fields) -> None:
        again = sp.fields_from_arrays("nh", sp.fields_to_arrays(fields))
        left, right = sp.fields_to_arrays(fields), sp.fields_to_arrays(again)
        assert left.keys() == right.keys()
        for name in left:
            np.testing.assert_array_equal(left[name], right[name], err_msg=name)


TONES = [(0.8, 0.9), (0.7, 0.85)]

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
        floes = floes_of(fields, c, look(floe_tone=tone))
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
        toned = look(floe_tone=tone, coast_rule=NO_RULE)
        floes = floes_of(fields, c, toned)
        alone = floes_of(fields, np.full_like(c, 0.75), toned)
        row = np.arange(SIZE)[:, None]
        sea = ~fields.coast.land
        near = sea & (row >= STEP_ROW - 12) & (row < STEP_ROW)
        assert (floes[near] < 1).any()
        assert (floes[near] == alone[near]).mean() > 0.99

    def test_the_floes_follow_the_curve(self, fields) -> None:
        c = np.full((SIZE, SIZE), 0.78, np.float32)
        read = float(sea_ice_look.toned(c[:1, :1], look().floe_tone)[0, 0])
        floes = floes_of(fields, c, look(coast_rule=NO_RULE))[~fields.coast.land]
        assert abs(float(floes.mean()) - read) < 0.04
        assert float(floes.mean()) > 0.78 + 0.06

    def test_the_toned_area_is_the_curve_over_the_cells(self) -> None:
        means = np.full((12, 4, 4), np.nan, np.float32)
        means[:, :, :2] = 0.8
        land = np.isnan(means[0])
        record = sea_ice.Hemisphere("nh", means, land)
        area = sea_ice_look.toned_area_km2(record)
        read = float(sea_ice_look.toned(np.array(0.8, np.float32), look().floe_tone))
        np.testing.assert_allclose(area, 8 * read * sea_ice.SOURCE_CELL_KM**2)
        assert (area > sea_ice.source_area_km2(record)).all()


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

    def test_summer_is_darker_than_winter_in_both_hemispheres(self) -> None:
        for hemisphere, summer in (("nh", 8), ("sh", 2)):
            table = self.table[hemisphere]
            winter = max(table, key=sum)
            assert all(s < w for s, w in zip(table[summer - 1], winter, strict=True))
