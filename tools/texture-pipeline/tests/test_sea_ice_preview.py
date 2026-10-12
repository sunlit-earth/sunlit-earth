"""Tests for the sea ice preview harness and the options its variants use."""

import dataclasses

import numpy as np
import pytest

from texture_pipeline import sea_ice_look
from texture_pipeline.cube import face_directions
from texture_pipeline.ocean_masking import (
    YearlyWater,
    latitude_rows,
    water_from_imagery,
)
from texture_pipeline.sea_ice import texel_km
from texture_pipeline.sea_ice_preview import (
    COAST_OFFSETS,
    FIELD_ROOTS,
    VARIANTS,
    Variant,
    activity_shares,
    coast_profiles,
    code_closure,
    code_digest,
    default_views,
    fields_from_arrays,
    fields_key,
    fields_to_arrays,
    in_box,
    polar_face_texels,
    render_config,
    seaward,
    tunables_read,
    view_directions,
)
from texture_pipeline.sea_ice_preview_page import markdown


class TestYearlyWater:
    def test_it_keeps_the_brightest_luminance_and_the_least_blue(self) -> None:
        stats = YearlyWater.empty(2, 3)
        dark_blue = np.zeros((2, 3, 3), dtype=np.uint8)
        dark_blue[..., 2] = 40
        bright = np.full((2, 3, 3), 200, dtype=np.uint8)
        bright[0, 0] = (30, 30, 35)
        stats.add(dark_blue, band_rows=1)
        stats.add(bright, band_rows=1)
        assert stats.max_luma[0, 0] == round(30 * 0.2126 + 30 * 0.7152 + 35 * 0.0722)
        assert stats.max_luma[1, 2] == 200
        assert stats.min_blue[0, 0] == 5
        assert stats.min_blue[1, 2] == 0


class TestLatitudeRows:
    def test_the_bands_hold_the_rows_poleward_of_the_latitude(self) -> None:
        north, south = latitude_rows(180, 50.0)
        assert (north, south) == (40, 140)

    def test_the_equator_splits_the_map(self) -> None:
        assert latitude_rows(180, 0.0) == (90, 90)


class TestWaterFromImagery:
    @staticmethod
    def scene() -> tuple[np.ndarray, np.ndarray]:
        mask = np.zeros((8, 8), dtype=np.uint8)
        mask[:, :2] = 255
        water = np.zeros((8, 8), dtype=bool)
        water[1, 2:5] = True
        water[5:7, 5:7] = True
        water[3, 7] = True
        water[4, 6] = True
        return mask, water

    def test_water_connected_to_the_ocean_joins_it(self) -> None:
        mask, water = self.scene()
        mask, joined = water_from_imagery(mask, water)
        assert np.all(mask[1, 2:5] == 255)
        assert joined == 3

    def test_a_lake_stays_land(self) -> None:
        mask, water = self.scene()
        mask, _ = water_from_imagery(mask, water)
        assert np.all(mask[5:7, 5:7] == 0)

    def test_a_diagonal_touch_does_not_connect(self) -> None:
        mask, water = self.scene()
        mask, _ = water_from_imagery(mask, water)
        assert mask[3, 7] == 0
        assert mask[4, 6] == 0

    def test_a_coast_pixel_short_of_open_ocean_can_join(self) -> None:
        mask, water = self.scene()
        mask[1, 2] = 128
        mask, _ = water_from_imagery(mask, water)
        assert mask[1, 2] == 255


class TestRescaledColors:
    def test_the_winter_months_are_the_brightest(self) -> None:
        assert sea_ice_look.winter_months(sea_ice_look.LOOK.colors["nh"]) == [
            1,
            2,
            3,
            4,
            12,
        ]
        assert sea_ice_look.winter_months(sea_ice_look.LOOK.colors["sh"]) == list(
            range(4, 11)
        )

    @pytest.mark.parametrize("hemisphere", ["nh", "sh"])
    def test_winter_takes_the_new_color_and_the_rest_keep_their_ratio(
        self, hemisphere: str
    ) -> None:
        table = sea_ice_look.LOOK.colors[hemisphere]
        winter = (250, 252, 254)
        scaled = sea_ice_look.rescaled_colors(table, winter)
        old_winter = np.asarray(max(table, key=sum), dtype=float)
        for month in sea_ice_look.winter_months(table):
            assert scaled[month - 1] == winter
        for before, after in zip(table, scaled, strict=True):
            expected = np.asarray(winter) * np.asarray(before) / old_winter
            assert np.all(np.abs(np.asarray(after) - expected) <= 0.5)

    def test_its_own_winter_leaves_the_table_alone(self) -> None:
        table = sea_ice_look.LOOK.colors["nh"]
        assert sea_ice_look.rescaled_colors(table, max(table, key=sum)) == table


class TestViews:
    @pytest.mark.parametrize("view", default_views(), ids=lambda v: v.key)
    def test_the_frame_reaches_the_framing_latitude_at_its_edges(self, view) -> None:
        size = 512
        x, y, z, hit = view_directions(view, size)
        assert hit.all()
        lat = np.degrees(np.arctan2(y, np.hypot(x, z)))
        edges = [lat[0, size // 2], lat[-1, size // 2], lat[size // 2, 0]]
        assert np.allclose(np.abs(edges), 55.0, atol=0.2)
        center = lat[size // 2 - 1 : size // 2 + 1, size // 2 - 1 : size // 2 + 1]
        assert np.all(np.abs(center) > 89.0)

    @pytest.mark.parametrize("face", ["py", "ny"])
    def test_a_texel_center_lands_on_its_own_texel(self, face: str) -> None:
        size = 16
        x, y, z = face_directions(face, size, 0, size)
        row, col, on = polar_face_texels(face, x, y, z, size)
        assert on.all()
        rows, cols = np.mgrid[0:size, 0:size]
        assert np.allclose(row, rows)
        assert np.allclose(col, cols)

    def test_the_config_draws_the_day_surface_on_the_first_of_the_month(self) -> None:
        north_winter = default_views()[0]
        config = render_config(north_winter)
        assert "texture_index = 1" in config
        assert "atmo_enabled = false" in config
        assert "custom_day_of_year = 61" in config
        assert "custom_year = 2004" in config

    def test_every_variant_has_a_summary(self) -> None:
        assert "baseline" in VARIANTS
        assert all(v.summary for v in VARIANTS.values())


class TestMarkdown:
    def test_lists_nest_and_code_is_marked(self) -> None:
        html = markdown("# Title\n\nSome `code` here.\n\n- one\n  - two\n- three\n")
        assert "<h3>Title</h3>" in html
        assert "<code>code</code>" in html
        assert html.count("<ul>") == 2
        assert "three" in html


class TestFieldsKey:
    def test_the_closure_reaches_what_prepare_calls(self) -> None:
        reached, constants = code_closure(sea_ice_look.prepare)
        names = {f"{v.__module__}.{v.__qualname__}" for v in reached}
        assert "texture_pipeline.sea_ice_look.band_noise" in names
        assert "texture_pipeline.sea_ice_look.leads" in names
        assert "texture_pipeline.sea_ice.texel_km" in names
        assert "texture_pipeline.sea_ice_look.month_cap" not in names
        assert "texture_pipeline.sea_ice_look.BAND_ROWS=512" in constants

    def test_a_tunable_the_fields_read_changes_the_key(self) -> None:
        look = sea_ice_look.LOOK
        changed = dataclasses.replace(look, edge_spread=look.edge_spread / 2)
        assert fields_key(changed) != fields_key(look)

    def test_a_tunable_only_the_month_reads_keeps_the_key(self) -> None:
        look = sea_ice_look.LOOK
        changed = dataclasses.replace(
            look, thin_shade=look.thin_shade / 2, coast_blend=not look.coast_blend
        )
        assert fields_key(changed) == fields_key(look)

    def test_the_tunables_read_are_the_fields_ones(self) -> None:
        read = tunables_read(*FIELD_ROOTS)
        assert "edge_band_km" in read
        assert "colors" not in read
        assert "coast_relief" not in read

    def test_different_code_digests_differently(self) -> None:
        assert code_digest(sea_ice_look.band_noise) != code_digest(sea_ice_look.streak)


class TestDerive:
    def test_a_child_takes_its_parents_options(self) -> None:
        parent = Variant("p", "parent", imagery_water=True, look={"seed": 1})
        child = parent.derive("c", "child", look={"thin_shade": 0.5}, tests=("t",))
        assert child.imagery_water
        assert child.look == {"seed": 1, "thin_shade": 0.5}
        assert child.parent == "p"
        assert child.tests == ("t",)
        assert child.remarks == ()

    def test_a_child_may_override_an_option(self) -> None:
        parent = Variant("p", "parent", imagery_water=True)
        assert not parent.derive("c", "child", imagery_water=False).imagery_water

    def test_every_variant_chain_ends_at_the_baseline(self) -> None:
        for variant in VARIANTS.values():
            seen = variant
            while seen.parent is not None:
                seen = VARIANTS[seen.parent]
            assert seen.name == "baseline"


SPACING_KM = 1.85


def coast_scene(land_rgb, size: int = 96) -> tuple[np.ndarray, np.ndarray]:
    """Water left of a third of the face, land right of it."""
    water = np.zeros((size, size), dtype=np.uint8)
    water[:, : size // 3] = 255
    source = np.empty((size, size, 3), dtype=np.uint8)
    source[...] = (10, 30, 60)
    source[:, size // 3 :] = land_rgb
    return source, water


class TestCoastWeight:
    def test_flat_snow_takes_full_weight_at_the_coast_and_none_inland(self) -> None:
        source, water = coast_scene((240, 243, 246))
        look = sea_ice_look.LOOK
        weight = sea_ice_look.coast_weight(source, water, look, SPACING_KM)
        coast = 96 // 3
        assert weight[48, coast] > 0.95
        far = coast + int(np.ceil(look.coast_reach_km[1] / SPACING_KM)) + 1
        assert weight[48, far] == 0
        assert not weight[water == 255].any()

    def test_mountains_keep_their_coast(self) -> None:
        source, water = coast_scene((240, 243, 246))
        source[::2, 32:] = (205, 208, 211)
        weight = sea_ice_look.coast_weight(source, water, spacing_km=SPACING_KM)
        assert not weight.any()

    def test_land_without_snow_keeps_its_coast(self) -> None:
        source, water = coast_scene((90, 110, 60))
        weight = sea_ice_look.coast_weight(source, water, spacing_km=SPACING_KM)
        assert not weight.any()

    def test_a_dark_rim_takes_the_weight_of_the_snow_behind_it(self) -> None:
        source, water = coast_scene((240, 243, 246))
        source[:, 32] = (60, 70, 90)
        weight = sea_ice_look.coast_weight(source, water, spacing_km=SPACING_KM)
        assert weight[48, 32] > 0.95

    def test_a_small_island_is_read_from_its_own_land(self) -> None:
        water = np.full((64, 64), 255, dtype=np.uint8)
        water[30:35, 30:35] = 0
        source = np.full((64, 64, 3), 240, dtype=np.uint8)
        weight = sea_ice_look.coast_weight(source, water, spacing_km=SPACING_KM)
        assert weight[32, 32] > 0.95


def uniform_ice(coverage: float, color=(230, 235, 240)) -> sea_ice_look.MonthIce:
    caps = {
        name: sea_ice_look.Cap(
            np.full((8, 8), coverage, np.float32),
            np.full((8, 8), 1.0, np.float32),
            color,
        )
        for name in ("nh", "sh")
    }
    return sea_ice_look.MonthIce(caps)


class TestBlendCoast:
    def test_the_coast_takes_the_ice_and_the_rest_is_untouched(self) -> None:
        source, water = coast_scene((250, 250, 250))
        source[:, 32] = (60, 70, 90)
        flat = source.copy()
        flat[water == 255] = (10, 30, 60)
        before = flat.copy()
        sea_ice_look.blend_coast(
            flat, source, water, "py", uniform_ice(1.0), spacing_km=SPACING_KM
        )
        np.testing.assert_allclose(flat[48, 32], (230, 235, 240), atol=3)
        np.testing.assert_array_equal(flat[water == 255], before[water == 255])
        np.testing.assert_array_equal(flat[:, 60:], before[:, 60:])

    def test_without_ice_nothing_changes(self) -> None:
        source, water = coast_scene((250, 250, 250))
        flat = source.copy()
        before = flat.copy()
        sea_ice_look.blend_coast(
            flat, source, water, "py", uniform_ice(0.0), spacing_km=SPACING_KM
        )
        np.testing.assert_array_equal(flat, before)

    def test_the_land_takes_the_ice_without_its_leads(self) -> None:
        source, water = coast_scene((250, 250, 250))
        flat = source.copy()
        ice = uniform_ice(1.0)
        cap = ice.caps["nh"]
        ice.caps["nh"] = dataclasses.replace(
            cap,
            brightness=np.full((8, 8), 0.5, np.float32),
            land_brightness=np.full((8, 8), 1.0, np.float32),
        )
        sea_ice_look.blend_coast(flat, source, water, "py", ice, spacing_km=SPACING_KM)
        np.testing.assert_allclose(flat[48, 33], (230, 235, 240), atol=3)


class TestCoastSteps:
    def test_a_box_across_the_date_line_wraps(self) -> None:
        lat = np.array([-78.0, -78.0, -78.0])
        lon = np.array([175.0, -170.0, 0.0])
        inside = in_box(lat, lon, (-78.8, -77.3), (172.0, -160.0))
        assert inside.tolist() == [True, True, False]

    def test_the_normals_point_to_the_sea(self) -> None:
        mask = np.zeros((32, 32), dtype=np.uint8)
        mask[:, 16:] = 255
        rows, cols = np.array([16]), np.array([15])
        lum = np.tile(np.arange(32, dtype=np.float32), (32, 1))
        (profile,) = coast_profiles((lum,), seaward(mask), rows, cols)
        np.testing.assert_allclose(profile[0], 15 + COAST_OFFSETS, atol=1e-4)


def b3_look() -> sea_ice_look.Look:
    return dataclasses.replace(sea_ice_look.LOOK, **VARIANTS["b3d-melt"].look)


@pytest.fixture(scope="module")
def b3_fields() -> sea_ice_look.Fields:
    stack = np.full((12, 32, 32), 0.9, dtype=np.float32)
    stack[:, :, :8] = 0.4
    return sea_ice_look.prepare(stack, "nh", 512, b3_look())


class TestInterior:
    def test_the_fields_survive_the_cache(self, b3_fields) -> None:
        again = fields_from_arrays("nh", fields_to_arrays(b3_fields))
        assert isinstance(again.fractures, sea_ice_look.Fractures)
        assert isinstance(again.floes, sea_ice_look.Floes)
        np.testing.assert_array_equal(again.creases, b3_fields.creases)
        np.testing.assert_array_equal(again.floes.opening, b3_fields.floes.opening)
        assert again.brightness is None

    def test_the_activity_spans_flat_to_structured(self, b3_fields) -> None:
        cap = sea_ice_look.month_cap(b3_fields, 3, b3_look())
        shares = activity_shares(cap)
        assert sum(shares.values()) == pytest.approx(1.0, abs=0.01)
        assert cap.activity is not None
        assert cap.activity.min() >= b3_look().activity_floor - 1e-6

    def test_open_water_takes_coverage_and_the_land_keeps_it(self, b3_fields) -> None:
        cap = sea_ice_look.month_cap(b3_fields, 3, b3_look())
        assert cap.land_coverage is not None
        assert np.all(cap.coverage <= cap.land_coverage + 1e-6)
        assert (cap.coverage < cap.land_coverage - 0.01).any()

    def test_the_edge_streaks_stay_out_of_the_pack(self, b3_fields) -> None:
        cap = sea_ice_look.month_cap(b3_fields, 3, b3_look())
        assert cap.land_coverage is not None
        pack = cap.land_coverage[:, 256:]
        assert float(pack.min()) > 0.99

    def test_the_pack_opens_no_more_than_the_record_leaves(self, b3_fields) -> None:
        cap = sea_ice_look.month_cap(b3_fields, 3, b3_look())
        drawn = float(cap.coverage[:, 256:].mean())
        assert 0.9 - 0.01 <= drawn < 1.0

    def test_the_melt_tone_keeps_the_months_color_as_its_mean(self, b3_fields) -> None:
        august = sea_ice_look.month_cap(b3_fields, 8, b3_look())
        march = sea_ice_look.month_cap(b3_fields, 3, b3_look())
        assert august.tint is not None
        assert abs(float(august.tint.mean())) < 1e-3
        assert march.tint is None


class TestFractures:
    def test_a_line_shows_its_width_times_its_darkness_across(self) -> None:
        path = np.stack([np.full(41, 20.3), np.linspace(5, 45, 41)], axis=-1)
        shade, rank = sea_ice_look.draw_fractures(
            64, [[(path, 0.6, 0.8)]], np.array([0.25]), np.random.default_rng(0)
        )
        across = shade[:, 25].astype(np.float32).sum()
        assert 0.6 * 0.8 * 0.7 <= across <= 0.6 * 0.8 * 1.3
        assert rank[20, 25] == 0.25
        assert rank[50, 25] == 1


def record(conc: float, offshore: float):
    from texture_pipeline.sea_ice import Hemisphere

    land = np.zeros((9, 20), dtype=bool)
    land[:, :2] = True
    means = np.full((12, 9, 20), offshore, dtype=np.float32)
    means[:, :, 2] = conc
    means[:, land] = np.nan
    return Hemisphere("sh", means, land)


class TestFastIce:
    def test_a_low_coast_under_closed_ice_is_raised(self) -> None:
        look = dataclasses.replace(sea_ice_look.LOOK, fast_ice=True)
        raised = sea_ice_look.fast_ice(record(0.6, 0.97), look)
        assert raised.means[8, 4, 2] > 0.9
        assert np.isnan(raised.means[8, 4, 0])

    def test_open_water_offshore_leaves_the_coast_alone(self) -> None:
        look = dataclasses.replace(sea_ice_look.LOOK, fast_ice=True)
        before = record(0.6, 0.5)
        after = sea_ice_look.fast_ice(before, look)
        np.testing.assert_array_equal(after.means, before.means)

    def test_without_the_band_the_record_is_unchanged(self) -> None:
        before = record(0.6, 0.97)
        assert sea_ice_look.fast_ice(before) is before


class TestFolds:
    def test_the_creases_run_every_way(self) -> None:
        look = dataclasses.replace(sea_ice_look.LOOK, fold_spacing_km=200.0)
        shade, line = sea_ice_look.folds(256, look, np.random.default_rng(1))
        assert float(shade.std()) == pytest.approx(1.0, abs=1e-3)
        assert float(line.min()) >= 0
        assert float(line.max()) <= 1
        gr, gc = np.gradient(line)
        weight = gr * gr + gc * gc
        doubled = (gc * gc - gr * gr + 2j * gr * gc) / np.maximum(weight, 1e-12)
        assert abs(complex((weight * doubled).sum() / weight.sum())) < 0.2

    def test_a_fold_is_straight(self) -> None:
        look = dataclasses.replace(
            sea_ice_look.LOOK,
            fold_spacing_km=512 * texel_km(512),
            fold_length_km=(4000.0, 4000.0),
            fold_line_km=30.0,
        )
        _, line = sea_ice_look.folds(512, look, np.random.default_rng(2))
        rows, cols = np.nonzero(line > 0.5)
        points = np.stack([rows, cols], axis=-1).astype(float)
        points -= points.mean(axis=0)
        across = np.linalg.svd(points, compute_uv=False)[-1] / np.sqrt(len(points))
        assert len(points) > 20
        assert across < 1.0


def straight_field(size: int) -> np.ndarray:
    return np.zeros((size, size), dtype=np.float32)


class TestFractureLook:
    def test_most_leads_are_grey_and_few_open(self, b3_fields) -> None:
        assert b3_fields.fractures is not None
        darkness = b3_fields.fractures.systems[:, 2]
        high = b3_look().fracture_darkness[1]
        assert np.mean(darkness <= high) > 0.75
        assert np.mean(darkness > high) > 0

    @pytest.mark.parametrize("bend", [0.0, 0.22])
    def test_a_fracture_wanders_only_with_a_bend(self, bend: float) -> None:
        look = dataclasses.replace(
            sea_ice_look.LOOK,
            fracture_bend=bend,
            fracture_follow=0.0,
            fracture_wander=0.0,
            fracture_branching_km=1e9,
            fracture_length_km=(600.0, 600.0),
            fracture_conjugate=(0.0, 0.0),
        )
        (system,) = sea_ice_look.fracture_paths(
            1024,
            np.array([[512.0, 512.0]]),
            straight_field(1024),
            look,
            np.random.default_rng(4),
        )
        path = system[0][0]
        across = np.abs(path[:, 0] - 512.0)
        if bend == 0:
            assert float(across.max()) < 0.01
        else:
            assert float(across.max()) > 1.0


def floe_look(**changes: object) -> sea_ice_look.Look:
    return dataclasses.replace(
        sea_ice_look.LOOK,
        floes=True,
        edge_pack=0.7,
        floe_cells_km=(300.0, 100.0),
        **changes,
    )


@pytest.fixture(scope="module")
def floes() -> sea_ice_look.Floes:
    return sea_ice_look.floe_cells(512, floe_look(), np.random.default_rng(5))


class TestFloes:
    def water(self, floes, conc: float, band: float | None = None) -> np.ndarray:
        look = floe_look()
        low = look.edge_center - look.edge_spread
        values = np.full((512, 512), conc, dtype=np.float32)
        raised = None if band is None else np.full_like(values, band)
        return sea_ice_look.floe_water(
            floes, values, raised, look, 0, 512, (low, 0.7), 512
        )

    def test_closed_pack_has_no_floes(self, floes) -> None:
        assert float(self.water(floes, 0.99).max()) == 0

    def test_the_gaps_are_grey_in_the_pack_and_dark_at_the_edge(self, floes) -> None:
        look = floe_look()
        grey = self.water(floes, 0.9)
        dark = self.water(floes, 0.5)
        assert 0 < float(grey.max()) <= look.floe_darkness[0] + 1e-3
        assert float(dark.max()) > 0.9

    def test_the_floes_take_what_the_edge_leaves_open(self, floes) -> None:
        look = floe_look()
        low = look.edge_center - look.edge_spread
        drawn = (0.75 - low) / (0.7 - low)
        left = 1 - 0.75 / min(drawn, 1)
        assert float(self.water(floes, 0.75).mean()) == pytest.approx(left, abs=0.03)

    def test_most_floes_hold_together_where_few_open(self, floes) -> None:
        water = self.water(floes, 0.9)
        assert float((water > 0).mean()) < 0.5

    def test_the_fast_ice_band_closes_the_floes(self, floes) -> None:
        assert float(self.water(floes, 0.75).max()) > 0
        assert float(self.water(floes, 0.75, band=0.1).max()) == 0
