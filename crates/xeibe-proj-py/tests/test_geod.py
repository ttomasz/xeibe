"""``Geod`` against pyproj's, method by method (docs/pyproj-phase2.md).

Both run PROJ's ``geodesic.c`` algorithms, so the numbers agree to a few
ulps on x86-64 (exactly on Linux but for rare cube-root roundings). On
arm64 the C compiler fuses multiplies and adds into FMAs, which Rust never
does, and pyproj's results drift by tens of ulps.
"""

from __future__ import annotations

import math
import pickle
import platform
import random
import warnings
from array import array
from typing import Any

import pytest

import xeibe_proj
from xeibe_proj import CRS, Geod
from xeibe_proj.enums import GeodIntermediateFlag
from xeibe_proj.exceptions import GeodError

pyproj = pytest.importorskip("pyproj")
np = pytest.importorskip("numpy")

ULPS = 4
FMA = platform.machine().lower() in ("arm64", "aarch64")


def close(ours: Any, theirs: Any) -> bool:
    """Equal within a few ulps (within 1e-12 on arm64), element by
    element, for floats and sequences of them."""
    if isinstance(ours, (list, tuple)) or hasattr(ours, "__len__"):
        return len(ours) == len(theirs) and all(close(o, t) for o, t in zip(ours, theirs, strict=True))
    if math.isnan(ours) or math.isnan(theirs):
        return math.isnan(ours) and math.isnan(theirs)
    if FMA:
        return math.isclose(ours, theirs, rel_tol=1e-12, abs_tol=1e-12)
    return abs(ours - theirs) <= ULPS * math.ulp(max(abs(ours), abs(theirs)))


ELLIPSOIDS = [
    {"ellps": "WGS84"},
    {"ellps": "clrk66"},
    {"ellps": "sphere"},
    {"a": 6378137, "f": 1 / 150},
    {"a": 6378137, "f": -1 / 150},
    {"a": 6378137, "rf": 298.257222101},
    {"a": 6378137, "b": 6356752.314140347},
    {"a": 6378137, "es": 0.0066943799901413165},
    {"a": 6378137, "e": 0.08181919104281579},
    {"a": 6371000},
]


def random_points(n: int, seed: int) -> tuple[list[float], list[float]]:
    rng = random.Random(seed)
    lats = [math.degrees(math.asin(rng.uniform(-1, 1))) for _ in range(n)]
    lons = [rng.uniform(-180, 180) for _ in range(n)]
    return lons, lats


@pytest.mark.parametrize("kwargs", ELLIPSOIDS, ids=repr)
def test_construction_matches_pyproj(kwargs: dict[str, Any]) -> None:
    ours, theirs = Geod(**kwargs), pyproj.Geod(**kwargs)
    for attr in ("a", "b", "f", "es", "sphere", "initstring"):
        assert getattr(ours, attr) == getattr(theirs, attr), attr
    assert repr(ours) == repr(theirs)
    copy = pickle.loads(pickle.dumps(ours))
    assert copy == ours and repr(copy) == repr(ours)


def test_initstrings_and_equality() -> None:
    for text in ("+ellps=clrk66", "+a=6378137 +rf=298.257223563", "+proj=geod +ellps=WGS84 +no_defs"):
        assert repr(Geod(text)) == repr(pyproj.Geod(text)), text
    assert Geod(ellps="WGS84") == Geod(a=6378137, rf=298.257223563)
    assert Geod(ellps="WGS84") != Geod(ellps="GRS80")
    assert Geod(ellps="WGS84") != "WGS84"


def test_every_named_ellipsoid() -> None:
    assert xeibe_proj.get_ellps_map() == pyproj.get_ellps_map()
    assert xeibe_proj.pj_ellps == pyproj.pj_ellps
    for name in pyproj.pj_ellps:
        ours, theirs = Geod(ellps=name), pyproj.Geod(ellps=name)
        assert (ours.a, ours.b, ours.f, ours.es, ours.sphere) == (theirs.a, theirs.b, theirs.f, theirs.es, theirs.sphere)
        assert repr(ours) == repr(theirs)


def test_prime_meridians_and_version() -> None:
    assert xeibe_proj.get_prime_meridians_map() == pyproj.get_prime_meridians_map()
    assert xeibe_proj.geodesic_version_str == pyproj.geodesic_version_str
    with pytest.raises(NotImplementedError):
        xeibe_proj.get_proj_operations_map()


@pytest.mark.parametrize("kwargs", ELLIPSOIDS[:5], ids=repr)
@pytest.mark.parametrize("radians", [False, True])
@pytest.mark.parametrize("back", [True, False])
def test_fwd_and_inv_match_pyproj(kwargs: dict[str, Any], radians: bool, back: bool) -> None:
    ours, theirs = Geod(**kwargs), pyproj.Geod(**kwargs)
    lons1, lats1 = random_points(200, 1)
    lons2, lats2 = random_points(200, 2)
    if radians:
        lons1, lats1, lons2, lats2 = ([math.radians(v) for v in vs] for vs in (lons1, lats1, lons2, lats2))
    inv = ours.inv(lons1, lats1, lons2, lats2, radians, return_back_azimuth=back)
    their_inv = theirs.inv(lons1, lats1, lons2, lats2, radians, return_back_azimuth=back)
    assert all(close(o, t) for o, t in zip(inv, their_inv, strict=True))
    azis = inv[0]
    dists = inv[2]
    fwd = ours.fwd(lons1, lats1, azis, dists, radians, return_back_azimuth=back)
    their_fwd = theirs.fwd(lons1, lats1, azis, dists, radians, return_back_azimuth=back)
    assert all(close(o, t) for o, t in zip(fwd, their_fwd, strict=True))


def test_containers_keep_their_types() -> None:
    g = Geod(ellps="WGS84")
    scalar = g.inv(0.0, 0.0, 10.0, 10.0)
    assert all(isinstance(v, float) for v in scalar)
    assert scalar == pyproj.Geod(ellps="WGS84").inv(0.0, 0.0, 10.0, 10.0)
    for container in (list, tuple):
        out = g.fwd(container([0.0, 1.0]), container([0.0, 1.0]), container([45.0, 90.0]), container([1e5, 1e6]))
        assert all(type(v) is container for v in out)
    out = g.inv(array("d", [0.0]), array("d", [0.0]), array("d", [1.0]), array("d", [1.0]))
    assert all(isinstance(v, array) for v in out)
    arrays = g.inv(np.array([0.0, 1.0]), np.array([0.0, 2.0]), np.array([3.0, 4.0]), np.array([5.0, 6.0]))
    assert all(isinstance(v, np.ndarray) for v in arrays)


def test_errors() -> None:
    g = Geod(ellps="WGS84")
    with pytest.raises(GeodError, match="Array lengths are not the same"):
        g.inv([0.0, 1.0], [0.0], [1.0], [1.0])
    with pytest.raises(GeodError, match="mutually exclusive"):
        g.inv_intermediate(0, 0, 10, 10, npts=5, del_s=1000, return_back_azimuth=True)
    with pytest.raises(NotImplementedError, match="inplace"):
        g.fwd(0.0, 0.0, 90.0, 1000.0, False, True)  # type: ignore[call-arg]
    with pytest.raises(NotImplementedError, match="inplace"):
        g.inv(0.0, 0.0, 1.0, 1.0, inplace=False)  # type: ignore[call-arg]
    with pytest.raises(KeyError):
        Geod(ellps="nonsense")
    with pytest.raises(GeodError, match="Invalid geometry"):
        g.geometry_length(object())


@pytest.mark.parametrize("radians", [False, True])
def test_npts_matches_pyproj(radians: bool) -> None:
    ours, theirs = Geod(ellps="WGS84"), pyproj.Geod(ellps="WGS84")
    args = (-73.78, 40.64, 103.99, 1.36)
    if radians:
        args = tuple(math.radians(v) for v in args)
    for initial, terminus in ((1, 1), (0, 0), (0, 1)):
        a = ours.npts(*args, 7, radians=radians, initial_idx=initial, terminus_idx=terminus)
        b = theirs.npts(*args, 7, radians=radians, initial_idx=initial, terminus_idx=terminus)
        assert len(a) == len(b) and all(close(p, q) for p, q in zip(a, b, strict=True))


FLAGS = [
    GeodIntermediateFlag.DEFAULT,
    GeodIntermediateFlag.NPTS_CEIL,
    GeodIntermediateFlag.NPTS_TRUNC | GeodIntermediateFlag.AZIS_KEEP,
    GeodIntermediateFlag.DEL_S_NO_RECALC | GeodIntermediateFlag.AZIS_KEEP,
    GeodIntermediateFlag.NPTS_ROUND | GeodIntermediateFlag.DEL_S_RECALC | GeodIntermediateFlag.AZIS_KEEP,
]


@pytest.mark.parametrize("flags", FLAGS, ids=str)
@pytest.mark.parametrize("back", [True, False])
def test_intermediate_matches_pyproj(flags: GeodIntermediateFlag, back: bool) -> None:
    ours, theirs = Geod(ellps="WGS84"), pyproj.Geod(ellps="WGS84")
    for kwargs in ({"npts": 12}, {"del_s": 350_000.0}, {"del_s": 1_000_000.0, "initial_idx": 0, "terminus_idx": 0}):
        a = ours.inv_intermediate(-73.78, 40.64, 103.99, 1.36, flags=flags, return_back_azimuth=back, **kwargs)
        b = theirs.inv_intermediate(-73.78, 40.64, 103.99, 1.36, flags=flags, return_back_azimuth=back, **kwargs)
        assert (a.npts, close(a.del_s, b.del_s), close(a.dist, b.dist)) == (b.npts, True, True)
        assert close(a.lons, b.lons) and close(a.lats, b.lats)
        assert (a.azis is None) == (b.azis is None)
        if a.azis is not None:
            assert close(a.azis, b.azis)
    a = ours.fwd_intermediate(10, 50, 33, 9, 120_000.0, flags=flags, return_back_azimuth=back)
    b = theirs.fwd_intermediate(10, 50, 33, 9, 120_000.0, flags=flags, return_back_azimuth=back)
    assert (a.npts, a.del_s, close(a.dist, b.dist)) == (b.npts, b.del_s, True)
    assert close(a.lons, b.lons) and close(a.lats, b.lats)
    if b.azis is not None:
        assert close(a.azis, b.azis)


def test_intermediate_into_given_arrays_and_radians() -> None:
    ours, theirs = Geod(ellps="WGS84"), pyproj.Geod(ellps="WGS84")
    lons, lats, azis = np.zeros(10), np.zeros(10), np.zeros(10)
    r = ours.inv_intermediate(
        0.1, 0.2, 1.5, -0.3, npts=10, radians=True, out_lons=lons, out_lats=lats, out_azis=azis, return_back_azimuth=False
    )
    s = theirs.inv_intermediate(0.1, 0.2, 1.5, -0.3, npts=10, radians=True, return_back_azimuth=False, flags=GeodIntermediateFlag.AZIS_KEEP)
    assert r.lons is lons and close(lons, s.lons) and close(lats, s.lats) and close(azis, s.azis)
    with pytest.raises(GeodError, match="not long enough"):
        ours.inv_intermediate(0, 0, 10, 10, npts=20, out_lons=np.zeros(5), out_lats=np.zeros(20), return_back_azimuth=True)
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        ours.inv_intermediate(0, 0, 10, 10, npts=3)
    assert any("Back azimuth" in str(w.message) for w in caught)


def test_lengths_and_areas_match_pyproj() -> None:
    ours, theirs = Geod(ellps="WGS84"), pyproj.Geod(ellps="WGS84")
    lons, lats = random_points(40, 3)
    assert close(ours.line_length(lons, lats), theirs.line_length(lons, lats))
    assert close(ours.line_lengths(lons, lats), theirs.line_lengths(lons, lats))
    assert ours.line_lengths(1.0, 2.0) == theirs.line_lengths(1.0, 2.0)
    assert ours.line_length([1.0], [2.0]) == theirs.line_length([1.0], [2.0]) == 0.0
    square = ([0.0, 1.0, 1.0, 0.0], [0.0, 0.0, 1.0, 1.0])
    for lo, la in (square, (square[0][::-1], square[1][::-1]), ([0, 90, 180, 270], [89, 89, 89, 89])):
        assert close(ours.polygon_area_perimeter(lo, la), theirs.polygon_area_perimeter(lo, la))
        rad = ([math.radians(v) for v in lo], [math.radians(v) for v in la])
        assert close(ours.polygon_area_perimeter(*rad, radians=True), theirs.polygon_area_perimeter(*rad, radians=True))


class _Ring:
    def __init__(self, lons: list[float], lats: list[float]) -> None:
        self.xy = (array("d", lons), array("d", lats))


class _Polygon:
    def __init__(self, exterior: _Ring, interiors: list[_Ring]) -> None:
        self.exterior = exterior
        self.interiors = interiors


class _Multi:
    def __init__(self, *geoms: Any) -> None:
        self.geoms = geoms


def test_geometries_duck_typed() -> None:
    ours, theirs = Geod(ellps="WGS84"), pyproj.Geod(ellps="WGS84")
    outer = _Ring([0, 2, 2, 0, 0], [0, 0, 2, 2, 0])
    hole = _Ring([0.5, 0.5, 1, 1, 0.5], [0.5, 1, 1, 0.5, 0.5])
    shape = _Multi(_Polygon(outer, [hole]), outer)
    assert close(ours.geometry_area_perimeter(shape), theirs.geometry_area_perimeter(shape))
    assert close(ours.geometry_length(shape), theirs.geometry_length(shape))


def test_reverse_azimuth() -> None:
    from xeibe_proj.geod import reverse_azimuth

    values = [-180.0, -90.0, -0.0, 0.0, 45.0, 180.0]
    assert reverse_azimuth(values) == pyproj.geod.reverse_azimuth(values)
    assert reverse_azimuth(1.0, radians=True) == pyproj.geod.reverse_azimuth(1.0, radians=True)


@pytest.mark.parametrize("code", [4326, 2180, 27700, 4978, 5773, 9707, 4807])
def test_crs_get_geod(code: int) -> None:
    ours, theirs = CRS(code).get_geod(), pyproj.CRS(code).get_geod()
    assert (ours is None) == (theirs is None)
    if ours is not None and theirs is not None:
        assert (ours.a, ours.b, ours.f, ours.es, ours.sphere) == (theirs.a, theirs.b, theirs.f, theirs.es, theirs.sphere)
        assert repr(ours) == repr(theirs)
