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
LOOK = sea_ice_look.LOOK


def stack() -> np.ndarray:
    values = np.full((12, 32, 32), 0.9, dtype=np.float32)
    values[:, :, :8] = 0.4
    return values


def land(size: int = 512) -> np.ndarray:
    """A strip of land along the cap's top."""
    out = np.zeros((size, size), dtype=bool)
    out[: size // 16] = True
    return out


@pytest.fixture(scope="module")
def fields() -> sea_ice_look.Fields:
    return sea_ice_look.prepare(stack(), "nh", 512, land=land())


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

    def test_a_block_sets_the_count_and_puts_it_back(self, monkeypatch) -> None:
        monkeypatch.setenv(parallel.ENV, "5")
        with parallel.configured(2):
            assert parallel.workers() == 2
        assert parallel.workers() == 5
        parallel.configure(3)
        try:
            with parallel.configured(None):
                assert parallel.workers() == 3
            with pytest.raises(KeyError), parallel.configured(1):
                raise KeyError
            assert parallel.workers() == 3
        finally:
            parallel.configure(None)

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
        serial = sea_ice_look.prepare(stack(), "nh", 512, workers=1, land=land())
        threaded = sea_ice_look.prepare(stack(), "nh", 512, workers=6, land=land())
        arrays_equal(serial, threaded)

    def test_the_noise_does_not_depend_on_the_process_setting(self) -> None:
        def noise() -> list[np.ndarray]:
            rng = np.random.default_rng(4)
            shift = np.zeros((2, 256, 256), dtype=np.float32)
            return [
                sea_ice_look.band_noise(256, (20.0, 200.0), rng),
                sea_ice_look.oriented_noise(256, (20.0, 200.0), 0.5, 3.0, rng),
                *sea_ice_look.facet_cells(256, 60.0, shift, rng),
            ]

        with parallel.configured(1):
            one = noise()
        with parallel.configured(6):
            many = noise()
        for a, b in zip(one, many, strict=True):
            np.testing.assert_array_equal(a, b)

    def test_the_month_does_not_depend_on_the_workers(self, fields) -> None:
        one = sea_ice_look.month_cap(fields, 3, workers=1)
        many = sea_ice_look.month_cap(fields, 3, workers=5)
        for name in ("coverage", "brightness", "activity", "floes"):
            np.testing.assert_array_equal(getattr(one, name), getattr(many, name))

    def test_the_coast_weight_does_not_depend_on_the_workers(self) -> None:
        source, water = coast_face(160)
        one = sea_ice_look.coast_weight(source, water, LOOK, 2.0, workers=1)
        many = sea_ice_look.coast_weight(source, water, LOOK, 2.0, workers=8)
        np.testing.assert_array_equal(one, many)
        assert one.max() > 0

    def test_the_digest_does_not_depend_on_the_workers(self) -> None:
        before = sp.code_digest(sea_ice_look.make_eddies, sea_ice_look.prepare)
        parallel.configure(3)
        try:
            assert (
                sp.code_digest.__wrapped__(
                    sea_ice_look.make_eddies, sea_ice_look.prepare
                )
                == before
            )
        finally:
            parallel.configure(None)


class TestParts:
    def test_a_part_comes_after_the_parts_it_needs(self) -> None:
        seen: set[str] = set()
        for part in sea_ice_look.PARTS:
            assert set(part.needs) <= seen
            seen.add(part.name)

    def test_parts_that_cannot_be_ordered_are_refused(self) -> None:
        part = sea_ice_look.Part("lonely", lambda _: {}, needs=("absent",))
        inputs = sea_ice_look.Inputs(stack(), "nh", 64, LOOK)
        with pytest.raises(ValueError, match="No part can be made"):
            sea_ice_look.run_parts([part, part], inputs, workers=2)

    def test_a_cache_is_asked_for_every_part_once(self) -> None:
        asked = []

        def cache(part, make):
            asked.append(part.name)
            return make()

        sea_ice_look.prepare(stack(), "nh", 512, cache=cache, workers=3, land=land())
        assert sorted(asked) == sorted(p.name for p in sea_ice_look.PARTS)


def keys(look: sea_ice_look.Look, record: str = "r", land_key: str = "l"):
    return sp.part_keys(look, record, 4096, land_key)


def changed(look: sea_ice_look.Look, **tunables: object) -> set[str]:
    before = keys(look)
    after = keys(dataclasses.replace(look, **tunables))
    return {name for name in before if before[name] != after[name]}


class TestPartKeys:
    def test_a_surface_tunable_changes_the_surface_key_only(self) -> None:
        assert changed(LOOK, voronoi_weight=1.2) == {"surface"}

    def test_a_lead_tunable_changes_the_fractures_key_only(self) -> None:
        assert changed(LOOK, pinch_shut=0.3) == {"fractures"}
        assert changed(LOOK, fracture_wander=0.02) == {"fractures"}

    def test_a_cell_tunable_changes_the_cascade_key_only(self) -> None:
        assert changed(LOOK, cascade_warp=0.2) == {"cascade"}

    def test_a_speckle_tunable_changes_the_floe_noise_key_only(self) -> None:
        assert changed(LOOK, speckle_pick_km=(40.0, 150.0)) == {"floe noise"}

    def test_the_rules_distance_changes_the_coast_key_only(self) -> None:
        assert changed(LOOK, coast_rule=(12.0, 0.5)) == {"coast"}

    def test_an_eddy_tunable_changes_the_eddies_key_only(self) -> None:
        assert changed(LOOK, eddy_shift_km=60.0) == {"eddies"}

    def test_a_part_that_reads_another_follows_its_changes(self) -> None:
        assert "fractures" in changed(LOOK, activity_band_km=(120.0, 900.0))
        assert "activity_band_km" not in sp.tunables_read(sea_ice_look.make_fractures)

    def test_the_seed_changes_every_key_but_the_coasts(self) -> None:
        drawn = set(keys(LOOK)) - {"coast"}
        assert changed(LOOK, seed=LOOK.seed + 1) == drawn

    def test_a_tunable_only_the_month_reads_changes_no_key(self) -> None:
        month_only = {
            "thin_shade": 0.3,
            "coast_window_km": 5.0,
            "cascade_deltas": (2.0, 1.0, 0.5, 0.0, -0.5),
            "floe_tone": (0.75, 0.9),
        }
        assert changed(LOOK, **month_only) == set()

    def test_the_record_changes_the_parts_that_read_the_concentration(self) -> None:
        other = keys(LOOK, record="other")
        differ = {name for name, key in keys(LOOK).items() if other[name] != key}
        assert differ == {"fractures"}

    def test_the_land_changes_only_the_parts_that_read_it(self) -> None:
        other = keys(LOOK, land_key="m")
        assert {n for n, k in keys(LOOK).items() if other[n] != k} == {"coast"}

    def test_the_size_changes_every_key(self) -> None:
        other = sp.part_keys(LOOK, "r", 2048, "l")
        assert all(other[name] != key for name, key in keys(LOOK).items())

    def test_a_key_is_the_same_in_another_process(self) -> None:
        code = (
            "from texture_pipeline import sea_ice_look as sl, sea_ice_preview as sp\n"
            "print(sorted(sp.part_keys(sl.LOOK, 'r', 4096, 'l').items()))\n"
            "print(sp.code_digest(sl.prepare))\n"
        )
        done = subprocess.run(
            [sys.executable, "-c", code], capture_output=True, text=True, check=True
        )
        first, second = done.stdout.splitlines()
        assert first == str(sorted(keys(LOOK).items()))
        assert second == sp.code_digest(sea_ice_look.prepare)


def bare_harness(
    tmp_path, concentration: np.ndarray, workers: int
) -> tuple[sp.Harness, list[str]]:
    """A harness that makes only the fields, and the parts it reports making."""
    computed: list[str] = []
    harness = object.__new__(sp.Harness)
    harness.cache = sp.Cache(tmp_path)
    harness.workers = workers
    harness.notes = {}
    harness.report = computed.append
    harness._fields = {}
    harness._fields_notes = {}
    harness._locks = {name: threading.Lock() for name in sea_ice.HEMISPHERES}
    harness.record = lambda hemisphere, look: (concentration, "record")
    harness.cap_land = lambda hemisphere: (land(), "land")
    return harness, computed


class TestFieldCache:
    @pytest.fixture(autouse=True)
    def small_cap(self, monkeypatch) -> None:
        monkeypatch.setattr(sea_ice, "CAP_SIZE", 512)

    def test_a_change_makes_only_the_part_that_reads_it(self, tmp_path) -> None:
        edited = dataclasses.replace(LOOK, cascade_warp=0.2)
        cold, made = bare_harness(tmp_path / "cold", stack(), 3)
        cold.fields("nh", LOOK)
        assert len(made) == len(sea_ice_look.PARTS)

        warm, made = bare_harness(tmp_path / "cold", stack(), 3)
        again = warm.fields("nh", LOOK)
        assert made == []
        arrays_equal(again, cold.fields("nh", LOOK))

        edit, made = bare_harness(tmp_path / "cold", stack(), 3)
        reused = edit.fields("nh", edited)
        assert made == ["Noise fields: nh, cascade"]

        fresh, _ = bare_harness(tmp_path / "fresh", stack(), 1)
        arrays_equal(reused, fresh.fields("nh", edited))

    def test_the_activity_reaches_the_fractures(self, tmp_path) -> None:
        edited = dataclasses.replace(LOOK, activity_band_km=(120.0, 900.0))
        first, _ = bare_harness(tmp_path, stack(), 2)
        first.fields("nh", LOOK)
        second, made = bare_harness(tmp_path, stack(), 2)
        second.fields("nh", edited)
        assert "Noise fields: nh, fractures" in made

    def test_the_notes_count_the_work_of_the_parts(self, tmp_path) -> None:
        harness, _ = bare_harness(tmp_path, stack(), 1)
        harness.fields("nh", LOOK)
        note = harness._fields_notes["nh"]
        assert set(note["seconds"]) == {p.name for p in sea_ice_look.PARTS}
        assert note["peak_gib"] is not None

    def test_the_notes_leave_the_peak_out_when_parts_run_at_once(
        self, tmp_path
    ) -> None:
        harness, _ = bare_harness(tmp_path, stack(), 3)
        harness.fields("nh", LOOK)
        assert harness._fields_notes["nh"]["peak_gib"] is None

    def test_the_fields_survive_the_cache(self, fields) -> None:
        again = sp.fields_from_arrays("nh", sp.fields_to_arrays(fields))
        assert isinstance(again.fractures, sea_ice_look.Fractures)
        assert isinstance(again.cascade, sea_ice_look.Cascade)
        arrays_equal(fields, again)


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
    def ice(self, fields) -> sea_ice_look.MonthIce:
        cap = sea_ice_look.month_cap(fields, 3)
        return sea_ice_look.MonthIce({"nh": cap})

    def test_the_rows_of_a_region_are_the_full_months_rows(self, fields) -> None:
        full = sea_ice_look.month_cap(fields, 3)
        part = sea_ice_look.month_cap(fields, 3, row_range=(100, 220))
        np.testing.assert_array_equal(part.coverage, full.coverage)
        np.testing.assert_array_equal(
            part.brightness[100:220], full.brightness[100:220]
        )
        assert not part.brightness[:100].any()
        assert not part.brightness[220:].any()

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
        flat = np.array(apply_ocean_mask(Image.fromarray(source), water, color=FILL))
        wide = flat.copy()
        full = sea_ice_look.blend_coast(
            wide, source, water, "py", ice, LOOK, fill=FILL, spacing_km=2.0
        )
        window = sea_ice_look.Window(self.size, (60, 190), (40, 200))
        rows, columns = window.rows, window.columns
        slab = (
            slice(rows[0] - 40, rows[1] + 40),
            slice(columns[0] - 40, columns[1] + 40),
        )
        inner = (slice(40, 40 + 130), slice(40, 40 + 160))
        weight = sea_ice_look.coast_weight(source[slab], water[slab], LOOK, 2.0)[inner]
        box = (slice(*rows), slice(*columns))
        part = flat[box].copy()
        applied = sea_ice_look.blend_coast(
            part,
            source[box],
            water[box],
            "py",
            ice,
            LOOK,
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
