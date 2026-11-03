"""CF's grid mapping attributes (``docs/pyproj-phase3.md``, "CF"): ``to_cf``
against pyproj's on a sample of EPSG's CRSs, ``from_cf`` and ``to_cf``
round trips, and where ours differs from pyproj's on purpose."""

from __future__ import annotations

import math
import warnings
from typing import Any

import pytest

pyproj = pytest.importorskip("pyproj")

from xeibe_proj import CRS, Transformer, _native  # noqa: E402

# EPSG's current CRSs pyproj knows, every fifteenth, and some that
# exercise every grid mapping.
_CURRENT = sorted(r[0] for r in _native.crs_records() if not r[4])
SAMPLE = sorted(
    set(_CURRENT[::15])
    | {4326, 4979, 4978, 32633, 2180, 3035, 3857, 5041, 3031, 3413, 2062, 27572, 4807, 5498, 9705, 2056, 3995}
)
# Names our newer EPSG release can spell otherwise.
NAMES = {"geographic_crs_name", "horizontal_datum_name", "projected_crs_name", "geopotential_datum_name", "reference_ellipsoid_name", "prime_meridian_name"}


def close(a: Any, b: Any) -> bool:
    if isinstance(a, (int, float)) and isinstance(b, (int, float)):
        return math.isclose(a, b, rel_tol=1e-9, abs_tol=1e-9)
    if isinstance(a, (list, tuple)) and isinstance(b, (list, tuple)):
        return len(a) == len(b) and all(close(x, y) for x, y in zip(a, b))
    return bool(a == b)


def secant(cf: dict[str, Any]) -> bool:
    """Ours: a Lambert Conic Conformal (1SP) with a scale under 1, as the
    cone through two parallels it is."""
    sp = cf.get("standard_parallel")
    return cf.get("grid_mapping_name") == "lambert_conformal_conic" and isinstance(sp, tuple) and "latitude_of_projection_origin" in cf


def scaled_up(crs: Any) -> bool:
    """A Lambert Conic Conformal (1SP) with a scale over 1 (the US's
    low-distortion ground systems, RMTCRS, WISCRS, …)."""
    op = crs.coordinate_operation
    if op is None or op.method_name != "Lambert Conic Conformal (1SP)":
        return False
    return any(p.name == "Scale factor at natural origin" and p.value > 1 for p in op.params)


def in_grads(crs: Any) -> bool:
    """A CRS with angles in grads (NTF's), which CF has in degrees and
    pyproj writes as they are."""
    return "grad" in crs.to_wkt()


@pytest.mark.parametrize("code", SAMPLE)
def test_to_cf_matches_pyproj(code: int) -> None:
    try:
        theirs = pyproj.CRS.from_epsg(code)
    except pyproj.exceptions.CRSError:
        pytest.skip("not in pyproj's EPSG")
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        t, o = theirs.to_cf(), CRS.from_epsg(code).to_cf()
    assert o.pop("crs_wkt") and t.pop("crs_wkt")
    if secant(o) or in_grads(theirs):
        pytest.skip("test_secant_cones, test_angles_in_degrees")
    if scaled_up(theirs):
        # A cone with a scale over 1 touches no parallel: CF has no
        # attributes for it, and ours writes its WKT only.
        assert "grid_mapping_name" not in o
        return
    keys = (o.keys() | t.keys()) - NAMES
    # The datum of a vertical CRS on an ensemble, which pyproj has none of.
    if "geopotential_datum_name" in o and "geopotential_datum_name" not in t:
        assert "ensemble" in o["geopotential_datum_name"]
    assert {k: o.get(k) for k in keys if not close(o.get(k), t.get(k))} == {}, (o, t)
    assert (o.keys() & NAMES) >= (t.keys() & NAMES)


@pytest.mark.parametrize("code", SAMPLE)
def test_cf_round_trips(code: int) -> None:
    """The CRS CF's attributes make has the same attributes: a height's
    datum too, which pyproj's ``from_cf`` leaves off a geographic CRS."""
    cf = CRS.from_epsg(code).to_cf()
    cf.pop("crs_wkt")
    if "grid_mapping_name" not in cf:
        pytest.skip("no grid mapping")
    again = CRS.from_cf(cf).to_cf()
    again.pop("crs_wkt")
    assert {k: (cf.get(k), again.get(k)) for k in cf.keys() | again.keys() if not close(cf.get(k), again.get(k))} == {}


@pytest.mark.parametrize("code", [2062, 27572, 27561, 27573, 27574])
def test_secant_cones(code: int) -> None:
    """A Lambert Conic Conformal (1SP) with a scale under 1 is CF's cone
    through two parallels, which converts as the CRS does; pyproj writes
    its parallel of origin as the one standard parallel, another
    projection."""
    crs = CRS.from_epsg(code)
    cf = crs.to_cf()
    cf.pop("crs_wkt")
    assert len(cf["standard_parallel"]) == 2
    back = CRS.from_cf(cf)
    assert back == crs or back.coordinate_operation is not None
    aou = crs.area_of_use
    assert aou is not None
    lon, lat = (aou.west + aou.east) / 2, (aou.south + aou.north) / 2
    geodetic = crs.geodetic_crs
    assert geodetic is not None
    x0, y0 = Transformer.from_crs(geodetic, crs, always_xy=True).transform(lon, lat)
    x1, y1 = Transformer.from_crs(geodetic, back, always_xy=True).transform(lon, lat)
    assert math.hypot(x1 - x0, y1 - y0) < 1e-6
    theirs = pyproj.CRS.from_epsg(code).to_cf()
    assert not isinstance(theirs["standard_parallel"], tuple)


def test_angles_in_degrees() -> None:
    """CF has angles in degrees; NTF's EPSG CRSs have theirs in grads."""
    zone2 = CRS.from_epsg(27572).to_cf()
    assert math.isclose(zone2["latitude_of_projection_origin"], 46.8)
    assert math.isclose(zone2["longitude_of_prime_meridian"], 2.33722917)
    assert math.isclose(pyproj.CRS.from_epsg(27572).to_cf()["longitude_of_prime_meridian"], 2.5969213)


def test_one_parallel_away_from_the_origin() -> None:
    """CF's cone with one standard parallel and its origin elsewhere is the
    cone through that parallel twice (pyproj puts the origin on the
    parallel)."""
    cf = {
        "grid_mapping_name": "lambert_conformal_conic",
        "standard_parallel": 25.0,
        "latitude_of_projection_origin": 20.0,
        "longitude_of_central_meridian": 265.0,
    }
    ours = CRS.from_cf(cf)
    assert ours.coordinate_operation is not None
    params = {p.name: p.value for p in ours.coordinate_operation.params}
    assert ours.coordinate_operation.method_name == "Lambert Conic Conformal (2SP)"
    assert params["Latitude of 1st standard parallel"] == params["Latitude of 2nd standard parallel"] == 25.0
    assert params["Latitude of false origin"] == 20.0
    # The same cone as pyproj's, with the origin where CF puts it.
    theirs = pyproj.CRS.from_cf(cf)
    x0, y0 = Transformer.from_crs(4326, ours, always_xy=True).transform(-95.0, 25.0)
    x1, y1 = pyproj.Transformer.from_crs(4326, theirs, always_xy=True).transform(-95.0, 25.0)
    assert math.isclose(x0, x1, abs_tol=1e-6) and y0 > y1


def test_missing_attributes_say_which() -> None:
    from xeibe_proj.exceptions import CRSError

    with pytest.raises(CRSError, match="needs perspective_point_height"):
        CRS.from_cf({"grid_mapping_name": "vertical_perspective"})
    with pytest.raises(CRSError, match="Unsupported grid mapping name: nonsense"):
        CRS.from_cf({"grid_mapping_name": "nonsense"})


def test_vertical_axes_have_cf_names() -> None:
    """A vertical CRS's height is CF's height above its geopotential datum
    and a depth CF's depth; pyproj names both the ellipsoidal height."""
    height = CRS.from_epsg(5703).cs_to_cf()
    assert [a["standard_name"] for a in height] == ["height_above_geopotential_datum"]
    assert height[0]["positive"] == "up"
    depth = CRS.from_epsg(5715).cs_to_cf()
    assert [(a["standard_name"], a["positive"]) for a in depth] == [("depth", "down")]
    assert pyproj.CRS.from_epsg(5703).cs_to_cf()[0]["standard_name"] == "height_above_reference_ellipsoid"
    # An ellipsoidal height stays one.
    assert CRS.from_epsg(4979).cs_to_cf()[2]["standard_name"] == "height_above_reference_ellipsoid"
