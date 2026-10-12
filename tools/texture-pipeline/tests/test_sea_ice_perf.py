"""Tests for the parallel generator, the per-part field caches and the draft path."""

import dataclasses
import subprocess
import sys
import threading

import numpy as np
import pytest
from PIL import Image

from texture_pipeline import parallel, sea_ice, sea_ice_look
from texture_pipeline import sea_ice_preview as sp
from texture_pipeline.cube import face_directions
from texture_pipeline.ocean_masking import apply_ocean_mask

FILL = (10, 30, 60)


def b3_look() -> sea_ice_look.Look:
    return dataclasses.replace(sea_ice_look.LOOK, **sp.VARIANTS["b3d-melt"].look)


def stack() -> np.ndarray:
    values = np.full((12, 32, 32), 0.9, dtype=np.float32)
    values[:, :, :8] = 0.4
    return values


@pytest.fixture(scope="module")
def b3_fields() -> sea_ice_look.Fields:
    return sea_ice_look.prepare(stack(), "nh", 512, b3_look())


def arrays_equal(a: sea_ice_look.Fields, b: sea_ice_look.Fields) -> None:
    left, right = sp.fields_to_arrays(a), sp.fields_to_arrays(b)
    assert left.keys() == right.keys()
    for name in left:
        np.testing.assert_array_equal(left[name], right[name], err_msg=name)


class TestWorkers:
    def test_a_request_wins_over_every_setting(self, monkeypatch) -> None:
        monkeypatch.setenv(parallel.ENV, "5")
        parallel.configure(3)
        try:
            assert parallel.workers(2) == 2
            assert parallel.workers() == 3
        finally:
            parallel.configure(None)
        assert parallel.workers() == 5

    def test_a_blank_variable_is_unset(self, monkeypatch) -> None:
        monkeypatch.setenv(parallel.ENV, "  ")
        assert 1 <= parallel.workers() <= parallel.LIMIT

    def test_a_variable_that_is_not_a_number_is_refused(self, monkeypatch) -> None:
        monkeypatch.setenv(parallel.ENV, "many")
        with pytest.raises(ValueError, match="whole number"):
            parallel.workers()

    def test_the_count_is_at_least_one(self) -> None:
        assert parallel.workers(0) == 1
        with pytest.raises(ValueError, match="at least 1"):
            parallel.configure(0)

    def test_results_come_back_in_the_items_order(self) -> None:
        assert parallel.pmap(lambda n: n * n, range(20), 4) == [
            n * n for n in range(20)
        ]

    def test_one_worker_runs_on_the_calling_thread(self) -> None:
        seen = parallel.pmap(lambda _: threading.get_ident(), range(3), 1)
        assert set(seen) == {threading.get_ident()}

    def test_an_exception_is_raised(self) -> None:
        def fail(n: int) -> int:
            if n == 2:
                raise KeyError(n)
            return n

        with pytest.raises(KeyError):
            parallel.pmap(fail, range(4), 3)


class TestSerialEqualsParallel:
    def test_the_fields_do_not_depend_on_the_workers(self) -> None:
        look = b3_look()
        serial = sea_ice_look.prepare(stack(), "nh", 512, look, workers=1)
        threaded = sea_ice_look.prepare(stack(), "nh", 512, look, workers=6)
        arrays_equal(serial, threaded)

    def test_the_month_does_not_depend_on_the_workers(self) -> None:
        look = b3_look()
        fields = sea_ice_look.prepare(stack(), "nh", 512, look)
        one = sea_ice_look.month_cap(fields, 3, look, workers=1)
        many = sea_ice_look.month_cap(fields, 3, look, workers=5)
        for name in ("coverage", "brightness", "land_coverage", "activity"):
            np.testing.assert_array_equal(getattr(one, name), getattr(many, name))

    def test_the_streaks_do_not_depend_on_the_workers(self) -> None:
        rng = np.random.default_rng(1)
        noise = sea_ice_look.normalized(rng.standard_normal((256, 256), np.float32))
        angle = rng.random((256, 256), dtype=np.float32) * 3
        doubled = np.stack([np.cos(angle), np.sin(angle)]).astype(np.float32)
        one = sea_ice_look.streak(noise, doubled, 300.0, 30.0, workers=1)
        many = sea_ice_look.streak(noise, doubled, 300.0, 30.0, workers=7)
        np.testing.assert_array_equal(one, many)

    def test_the_coast_weight_does_not_depend_on_the_workers(self) -> None:
        source, water = coast_face(160)
        look = b3_look()
        one = sea_ice_look.coast_weight(source, water, look, 2.0, workers=1)
        many = sea_ice_look.coast_weight(source, water, look, 2.0, workers=8)
        np.testing.assert_array_equal(one, many)
        assert one.max() > 0

    def test_the_digest_does_not_depend_on_the_workers(self) -> None:
        before = sp.code_digest(sea_ice_look.make_edge, sea_ice_look.prepare)
        parallel.configure(3)
        try:
            assert (
                sp.code_digest.__wrapped__(sea_ice_look.make_edge, sea_ice_look.prepare)
                == before
            )
        finally:
            parallel.configure(None)


class TestParts:
    def test_fractures_replace_the_lead_network(self, b3_fields) -> None:
        names = [p.name for p in sea_ice_look.parts_for(b3_look(), "nh")]
        assert "leads" not in names
        assert "fractures" in names
        assert b3_fields.leads is None

    def test_without_fractures_the_network_is_made(self) -> None:
        look = dataclasses.replace(b3_look(), fractures=False)
        fields = sea_ice_look.prepare(stack(), "nh", 512, look)
        assert fields.leads is not None
        assert fields.fractures is None

    def test_the_first_look_makes_the_network_and_the_brightness(self) -> None:
        names = [p.name for p in sea_ice_look.parts_for(sea_ice_look.LOOK, "nh")]
        assert sorted(names) == ["classic", "eddies", "edge", "leads"]

    def test_the_melt_is_the_norths(self) -> None:
        look = b3_look()
        north = [p.name for p in sea_ice_look.parts_for(look, "nh")]
        south = [p.name for p in sea_ice_look.parts_for(look, "sh")]
        assert "melt" in north
        assert "melt" not in south

    def test_a_part_comes_after_the_parts_it_needs(self) -> None:
        parts = sea_ice_look.parts_for(b3_look(), "nh")
        seen: set[str] = set()
        for part in parts:
            assert set(part.needs) <= seen
            seen.add(part.name)

    def test_parts_that_cannot_be_ordered_are_refused(self) -> None:
        part = sea_ice_look.Part("lonely", lambda _: {}, needs=("absent",))
        inputs = sea_ice_look.Inputs(stack(), "nh", 64, sea_ice_look.LOOK)
        with pytest.raises(ValueError, match="No part can be made"):
            sea_ice_look.run_parts([part, part], inputs, workers=2)

    def test_a_cache_is_asked_for_every_part_once(self) -> None:
        asked = []

        def cache(part, make):
            asked.append(part.name)
            return make()

        look = b3_look()
        sea_ice_look.prepare(stack(), "nh", 512, look, cache=cache, workers=3)
        wanted = [p.name for p in sea_ice_look.parts_for(look, "nh")]
        assert sorted(asked) == sorted(wanted)


def keys(look: sea_ice_look.Look, record: str = "r") -> dict[str, str]:
    return sp.part_keys(look, "nh", record, 4096)


def changed(look: sea_ice_look.Look, **tunables: object) -> set[str]:
    before = keys(look)
    after = keys(dataclasses.replace(look, **tunables))
    return {name for name in before if before[name] != after[name]}


class TestPartKeys:
    def test_a_floes_tunable_changes_the_floes_key_only(self) -> None:
        assert changed(b3_look(), floe_cells_km=(30.0, 12.0)) == {"floes"}

    def test_a_fracture_tunable_changes_the_fractures_key_only(self) -> None:
        assert changed(b3_look(), fracture_wander=0.02) == {"fractures"}

    def test_a_crumple_tunable_changes_the_crumple_key_only(self) -> None:
        assert changed(b3_look(), crumple_aspect=1.9) == {"crumple"}

    def test_an_edge_tunable_changes_the_edge_key_only(self) -> None:
        assert changed(b3_look(), edge_streak_km=300.0) == {"edge"}

    def test_an_eddy_tunable_changes_the_eddies_key_only(self) -> None:
        assert changed(b3_look(), eddy_shift_km=60.0) == {"eddies"}

    def test_a_melt_tunable_changes_the_melt_key_only(self) -> None:
        assert changed(b3_look(), melt_band_km=(50.0, 400.0)) == {"melt"}

    def test_a_part_that_reads_another_follows_its_changes(self) -> None:
        look = b3_look()
        assert changed(look, activity_band_km=(120.0, 900.0)) == {
            "activity",
            "fractures",
        }

    def test_the_seed_changes_every_key(self) -> None:
        look = b3_look()
        assert changed(look, seed=look.seed + 1) == set(keys(look))

    def test_a_tunable_only_the_month_reads_changes_no_key(self) -> None:
        look = b3_look()
        assert changed(look, thin_shade=0.3, floe_gap=0.2, coast_window_km=5.0) == set()

    def test_the_record_changes_the_parts_that_read_the_concentration(self) -> None:
        look = b3_look()
        other = sp.part_keys(look, "nh", "other", 4096)
        differ = {name for name, key in keys(look).items() if other[name] != key}
        assert differ == {"edge", "fractures"}

    def test_the_size_changes_every_key(self) -> None:
        look = b3_look()
        other = sp.part_keys(look, "nh", "r", 2048)
        assert all(other[name] != key for name, key in keys(look).items())

    def test_a_key_is_the_same_in_another_process(self) -> None:
        code = (
            "import dataclasses\n"
            "from texture_pipeline import sea_ice_look as sl, sea_ice_preview as sp\n"
            "look = dataclasses.replace(sl.LOOK, **sp.VARIANTS['b3d-melt'].look)\n"
            "print(sorted(sp.part_keys(look, 'nh', 'r', 4096).items()))\n"
            "print(sp.code_digest(sp.coastal_snow, sp.face_latitude))\n"
        )
        done = subprocess.run(
            [sys.executable, "-c", code], capture_output=True, text=True, check=True
        )
        first, second = done.stdout.splitlines()
        assert first == str(sorted(keys(b3_look()).items()))
        assert second == sp.code_digest(sp.coastal_snow, sp.face_latitude)


def settled_look() -> sea_ice_look.Look:
    return dataclasses.replace(sea_ice_look.LOOK, **sp.VARIANTS["settled"].look)


def settled_keys(look: sea_ice_look.Look, land: str = "l") -> dict[str, str]:
    return sp.part_keys(look, "nh", "r", 4096, land)


def settled_changed(look: sea_ice_look.Look, **tunables: object) -> set[str]:
    before = settled_keys(look)
    after = settled_keys(dataclasses.replace(look, **tunables))
    return {name for name in before if before[name] != after[name]}


class TestSettledKeys:
    def test_the_settled_parts(self) -> None:
        names = {p.name for p in sea_ice_look.parts_for(settled_look(), "nh")}
        assert {"surface", "cascade", "floe noise", "coast", "fractures"} <= names
        assert not names & {"edge", "crumple", "floes", "leads"}

    def test_a_surface_tunable_changes_the_surface_key_only(self) -> None:
        assert settled_changed(settled_look(), voronoi_weight=1.2) == {"surface"}

    def test_a_lead_tunable_changes_the_fractures_key_only(self) -> None:
        assert settled_changed(settled_look(), pinch_shut=0.3) == {"fractures"}

    def test_a_cell_tunable_changes_the_cascade_key_only(self) -> None:
        assert settled_changed(settled_look(), cascade_warp=0.2) == {"cascade"}

    def test_a_speckle_tunable_changes_the_floe_noise_key_only(self) -> None:
        assert settled_changed(settled_look(), speckle_pick_km=(40.0, 150.0)) == {
            "floe noise"
        }

    def test_the_rules_distance_changes_the_coast_key_only(self) -> None:
        assert settled_changed(settled_look(), coast_rule=(12.0, 0.5)) == {"coast"}

    def test_a_tunable_only_the_month_reads_changes_no_key(self) -> None:
        look = settled_look()
        assert (
            changed(look, gap_gray=0.5, cascade_deltas=(2.0, 1.0, 0.5, 0.0, -0.5))
            == set()
        )

    def test_the_land_changes_only_the_parts_that_read_it(self) -> None:
        look = settled_look()
        other = settled_keys(look, "m")
        assert {n for n, k in settled_keys(look).items() if other[n] != k} == {"coast"}


def bare_harness(tmp_path, concentration: np.ndarray, workers: int) -> sp.Harness:
    harness = object.__new__(sp.Harness)
    harness.cache = sp.Cache(tmp_path)
    harness.workers = workers
    harness.notes = {}
    harness.computed = []
    harness.report = harness.computed.append
    harness._fields = {}
    harness._fields_notes = {}
    harness._locks = {name: threading.Lock() for name in sea_ice.HEMISPHERES}
    harness.record = lambda hemisphere, look: (concentration, None, "record")
    return harness


class TestFieldCache:
    @pytest.fixture(autouse=True)
    def small_cap(self, monkeypatch) -> None:
        monkeypatch.setattr(sea_ice, "CAP_SIZE", 512)

    def test_a_change_makes_only_the_part_that_reads_it(self, tmp_path) -> None:
        look = b3_look()
        edited = dataclasses.replace(look, floe_cells_km=(30.0, 12.0))
        cold = bare_harness(tmp_path / "cold", stack(), 3)
        cold.fields("nh", look)
        assert len(cold.computed) == len(sea_ice_look.parts_for(look, "nh"))

        warm = bare_harness(tmp_path / "cold", stack(), 3)
        again = warm.fields("nh", look)
        assert warm.computed == []
        arrays_equal(again, cold.fields("nh", look))

        edit = bare_harness(tmp_path / "cold", stack(), 3)
        reused = edit.fields("nh", edited)
        assert edit.computed == ["Noise fields: nh, floes"]

        fresh = bare_harness(tmp_path / "fresh", stack(), 1)
        arrays_equal(reused, fresh.fields("nh", edited))

    def test_the_activity_reaches_the_fractures(self, tmp_path) -> None:
        look = b3_look()
        edited = dataclasses.replace(look, activity_band_km=(120.0, 900.0))
        first = bare_harness(tmp_path, stack(), 2)
        first.fields("nh", look)
        second = bare_harness(tmp_path, stack(), 2)
        second.fields("nh", edited)
        assert sorted(second.computed) == [
            "Noise fields: nh, activity",
            "Noise fields: nh, fractures",
        ]

    def test_the_notes_count_the_work_of_the_parts(self, tmp_path) -> None:
        harness = bare_harness(tmp_path, stack(), 1)
        harness.fields("nh", b3_look())
        note = harness._fields_notes["nh"]
        assert set(note["seconds"]) == {
            p.name for p in sea_ice_look.parts_for(b3_look(), "nh")
        }
        assert note["peak_gib"] is not None
        assert "leads.rank" not in note["mib"]

    def test_the_notes_leave_the_peak_out_when_parts_run_at_once(
        self, tmp_path
    ) -> None:
        harness = bare_harness(tmp_path, stack(), 3)
        harness.fields("nh", b3_look())
        assert harness._fields_notes["nh"]["peak_gib"] is None

    def test_fields_without_leads_survive_the_cache(self, b3_fields) -> None:
        again = sp.fields_from_arrays("nh", sp.fields_to_arrays(b3_fields))
        assert again.leads is None
        arrays_equal(b3_fields, again)


def coast_face(size: int) -> tuple[np.ndarray, np.ndarray]:
    """A snowy land with a sea and a dark inlet, and the mask for it."""
    rng = np.random.default_rng(5)
    yy, xx = np.mgrid[0:size, 0:size]
    water = np.where(
        (xx - size * 0.55) ** 2 + (yy - size * 0.5) ** 2 < (size * 0.3) ** 2, 255, 0
    )
    water[: size // 8] = 0
    water = water.astype(np.uint8)
    source = np.full((size, size, 3), (205, 210, 214), dtype=np.uint8)
    source += (rng.random((size, size, 1)) * 30).astype(np.uint8)
    source[size // 2 :, : size // 5] = (30, 50, 70)
    source[water == 255] = (20, 40, 70)
    return source, water


class TestRegion:
    size = 256

    @pytest.fixture(scope="class")
    def ice(self, b3_fields) -> sea_ice_look.MonthIce:
        cap = sea_ice_look.month_cap(b3_fields, 3, b3_look())
        return sea_ice_look.MonthIce({"nh": cap})

    def test_the_rows_of_a_region_are_the_full_months_rows(self, b3_fields) -> None:
        look = b3_look()
        full = sea_ice_look.month_cap(b3_fields, 3, look)
        part = sea_ice_look.month_cap(b3_fields, 3, look, row_range=(100, 220))
        np.testing.assert_array_equal(part.coverage[100:220], full.coverage[100:220])
        np.testing.assert_array_equal(
            part.land_coverage[100:220], full.land_coverage[100:220]
        )
        assert not part.coverage[:100].any()
        assert not part.coverage[220:].any()

    def test_a_column_range_of_directions_is_a_slice_of_the_rows(self) -> None:
        full = face_directions("py", 64, 10, 30)
        part = face_directions("py", 64, 10, 30, (5, 40))
        for a, b in zip(full, part, strict=True):
            np.testing.assert_array_equal(a[:, 5:40], b)

    def test_the_window_has_a_shape(self) -> None:
        window = sea_ice_look.Window(64, (10, 30), (5, 40))
        assert window.shape == (20, 35)

    def test_the_composite_of_a_region_is_the_composite_of_the_face(self, ice) -> None:
        source, water = coast_face(self.size)
        flat = np.array(apply_ocean_mask(Image.fromarray(source), water, color=FILL))
        wide = flat.copy()
        sea_ice_look.composite_face(wide, water, "py", ice, FILL)
        window = sea_ice_look.Window(self.size, (60, 190), (40, 200))
        rows, columns = window.rows, window.columns
        part = flat[rows[0] : rows[1], columns[0] : columns[1]].copy()
        sea_ice_look.composite_face(
            part,
            water[rows[0] : rows[1], columns[0] : columns[1]],
            "py",
            ice,
            FILL,
            window=window,
        )
        np.testing.assert_array_equal(
            part, wide[rows[0] : rows[1], columns[0] : columns[1]]
        )
        assert (wide != flat).any()

    def test_the_coast_blend_of_a_region_is_the_blend_of_the_face(self, ice) -> None:
        source, water = coast_face(self.size)
        look = b3_look()
        flat = np.array(apply_ocean_mask(Image.fromarray(source), water, color=FILL))
        wide = flat.copy()
        full = sea_ice_look.blend_coast(
            wide, source, water, "py", ice, look, fill=FILL, spacing_km=2.0
        )
        window = sea_ice_look.Window(self.size, (60, 190), (40, 200))
        rows, columns = window.rows, window.columns
        slab = (
            slice(rows[0] - 40, rows[1] + 40),
            slice(columns[0] - 40, columns[1] + 40),
        )
        inner = (slice(40, 40 + 130), slice(40, 40 + 160))
        weight = sea_ice_look.coast_weight(source[slab], water[slab], look, 2.0)[inner]
        box = (slice(*rows), slice(*columns))
        part = flat[box].copy()
        applied = sea_ice_look.blend_coast(
            part,
            source[box],
            water[box],
            "py",
            ice,
            look,
            fill=FILL,
            weight=np.ascontiguousarray(weight),
            window=window,
        )
        np.testing.assert_array_equal(part, wide[box])
        np.testing.assert_array_equal(applied, full[box])
        assert (wide != flat).any()

    def test_a_window_is_kept_on_the_face(self) -> None:
        window = sp.region_window("py", 82.3, -38.0, 5400, 512)
        assert window.size == 5400
        assert 0 <= window.rows[0] < window.rows[1] <= 5400
        assert 0 <= window.columns[0] < window.columns[1] <= 5400
        assert window.shape == (1024, 1024)

    def test_a_window_at_the_edge_is_cut_short(self) -> None:
        window = sp.region_window("py", 55.0, 90.0, 5400, 2000)
        assert (
            window.columns[1] == 5400 or window.rows[1] == 5400 or window.rows[0] == 0
        )

    def test_a_point_off_the_face_is_refused(self) -> None:
        with pytest.raises(ValueError, match="not on the py face"):
            sp.region_window("py", -70.0, 0.0, 5400, 512)

    def test_the_cap_rows_cover_the_region(self) -> None:
        window = sp.region_window("py", 82.3, -38.0, 5400, 256)
        low, high = sp.cap_rows("py", window, 4096)
        assert 0 <= low < high <= 4096
        assert high - low < 4096

    def test_a_place_is_found_by_name(self) -> None:
        assert sp.find_place("svalbard")[0] == "svalbard"
        with pytest.raises(ValueError, match="No place"):
            sp.find_place("atlantis")
