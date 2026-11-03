"""The builders (``docs/pyproj-phase3.md``, "Builders") against pyproj's:
the same signatures, and the same PROJJSON and WKT for every class with
several sets of arguments. pyproj's tests check a few cases of each; these
check every argument."""

from __future__ import annotations

import inspect
import json
import math
import re
import subprocess
import sys
import warnings
from collections.abc import Callable
from types import ModuleType, SimpleNamespace
from typing import Any

import pytest

pyproj = pytest.importorskip("pyproj")

import pyproj.crs.coordinate_operation  # noqa: E402
import pyproj.crs.coordinate_system  # noqa: E402
import pyproj.crs.datum  # noqa: E402
import pyproj.crs.enums  # noqa: E402

import xeibe_proj.crs  # noqa: E402
import xeibe_proj.crs.coordinate_operation  # noqa: E402
import xeibe_proj.crs.coordinate_system  # noqa: E402
import xeibe_proj.crs.datum  # noqa: E402
import xeibe_proj.crs.enums  # noqa: E402


def _lib(root: Any) -> SimpleNamespace:
    return SimpleNamespace(
        crs=root.crs,
        op=root.crs.coordinate_operation,
        cs=root.crs.coordinate_system,
        datum=root.crs.datum,
        enums=root.crs.enums,
    )


OURS, THEIRS = _lib(xeibe_proj), _lib(pyproj)
MODULES = ("crs", "op", "cs", "datum", "enums")
TOKEN = re.compile(r'"(?:[^"]|"")*"|[-+]?\d+(?:\.\d*)?(?:[eE][-+]?\d+)?|[^\s\d"]+')


def same(a: Any, b: Any, path: str = "") -> list[str]:
    """Where two PROJJSON documents differ, numbers to 1e-12 relative."""
    if isinstance(a, dict) and isinstance(b, dict):
        out = [f"{path}.{k}: only one has it" for k in a.keys() ^ b.keys()]
        for k in a.keys() & b.keys():
            out += same(a[k], b[k], f"{path}.{k}")
        return out
    if isinstance(a, list) and isinstance(b, list):
        if len(a) != len(b):
            return [f"{path}: {len(a)} items, {len(b)}"]
        return [d for i, (x, y) in enumerate(zip(a, b)) for d in same(x, y, f"{path}[{i}]")]
    if isinstance(a, (int, float)) and isinstance(b, (int, float)) and not isinstance(a, bool):
        return [] if math.isclose(a, b, rel_tol=1e-12, abs_tol=1e-12) else [f"{path}: {a} != {b}"]
    return [] if a == b else [f"{path}: {a!r} != {b!r}"]


def comparable(value: Any, inside_id: bool = False) -> Any:
    """PROJJSON as PROJ writes its ids, which ours keeps nested as our
    CRSs do: none inside an object with an id, but an ensemble's and its
    members', and a method's and its parameters'. Without EPSG's remarks
    on a CRS, which ours keeps apart (``CRS.remarks``)."""
    if isinstance(value, dict):
        has_id = "id" in value or "ids" in value
        keep = "members" in value
        return {
            k: v if k in ("members", "method", "parameters") else comparable(v, inside_id or has_id)
            for k, v in value.items()
            if k != "remarks" and (keep or not (inside_id and k in ("id", "ids")))
        }
    if isinstance(value, list):
        return [comparable(v, inside_id) for v in value]
    return value


# PROJ's WKT has the usage and remarks of a compound or Bound CRS's
# components, which its PROJJSON of them leaves out, and so ours.
USAGE = re.compile(r',(?:USAGE\[SCOPE\["(?:[^"]|"")*"\],AREA\["(?:[^"]|"")*"\],BBOX\[[^\]]*\]\]|REMARK\["(?:[^"]|"")*"\])')


def same_wkt(a: str, b: str) -> bool:
    a, b = USAGE.sub("", a), USAGE.sub("", b)
    ta, tb = TOKEN.findall(a), TOKEN.findall(b)
    if len(ta) != len(tb):
        return False
    for x, y in zip(ta, tb):
        if x != y:
            try:
                if not math.isclose(float(x), float(y), rel_tol=1e-11):
                    return False
            except ValueError:
                return False
    return True


def classes(module: ModuleType) -> dict[str, type]:
    """The builder classes pyproj's module defines."""
    return {
        n: c
        for n, c in vars(module).items()
        if isinstance(c, type) and c.__module__ == module.__name__ and not issubclass(c, pyproj.enums.BaseEnum)
    }


@pytest.mark.parametrize("module", MODULES)
def test_the_same_names_and_signatures(module: str) -> None:
    theirs, ours = getattr(THEIRS, module), getattr(OURS, module)
    for name, cls in classes(theirs).items():
        mine = getattr(ours, name, None)
        assert mine is not None, f"{module}.{name} is missing"
        if issubclass(cls, pyproj.enums.BaseEnum):
            continue
        constructor = "__new__" if "__new__" in vars(cls) else "__init__"
        want = inspect.signature(getattr(cls, constructor))
        got = inspect.signature(getattr(mine, constructor))
        # An enum's default is each library's own member: by its value.
        assert [(p.name, p.kind, getattr(p.default, "value", p.default)) for p in got.parameters.values()] == [
            (p.name, p.kind, getattr(p.default, "value", p.default)) for p in want.parameters.values()
        ], (module, name)


@pytest.mark.parametrize("name", sorted(classes(pyproj.crs.enums)))
def test_the_same_enums(name: str) -> None:
    theirs, ours = getattr(THEIRS.enums, name), getattr(OURS.enums, name)
    assert [(m.name, m.value) for m in theirs] == [(m.name, m.value) for m in ours]


# Arguments that aren't numbers, and numbers that must be in a range.
SPECIAL: dict[str, Any] = {
    "sweep_angle_axis": "x",
    "zone": "33",
    "hemisphere": "S",
    "scale_factor_natural_origin": 0.9996,
    "scale_factor_projection_centre": 0.99,
}
# Deprecated aliases, which pyproj's own tests cover.
DEPRECATED = {"azimuth_initial_line", "scale_factor_on_initial_line"}


def arguments(cls: type, only_required: bool) -> dict[str, Any]:
    """A distinct value for each of a conversion's arguments."""
    out: dict[str, Any] = {}
    for i, p in enumerate(inspect.signature(cls.__new__).parameters.values()):
        if p.name == "cls" or p.name in DEPRECATED or p.kind is p.VAR_KEYWORD:
            continue
        if only_required and p.default is not p.empty and p.name != "azimuth_projection_centre":
            continue
        out[p.name] = SPECIAL.get(p.name, 1.25 + 3.5 * i)
    if cls.__name__ == "MercatorAConversion":
        out["latitude_natural_origin"] = 0.0
    if cls.__name__ == "ToWGS84Transformation":
        out["source_crs"] = 4277
    return out


CONVERSIONS = sorted(classes(pyproj.crs.coordinate_operation))


def check(build: Callable[[SimpleNamespace], Any], wkt: bool = True) -> None:
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        theirs, ours = build(THEIRS), build(OURS)
    # pyproj's part builders make an instance of the part's class
    # (`TransverseMercatorConversion()` is a `CoordinateOperation`); ours of
    # the builder, a subclass of it.
    assert type(theirs).__name__ in [c.__name__ for c in type(ours).__mro__]
    diff = same(comparable(ours.to_json_dict()), comparable(theirs.to_json_dict()))
    assert not diff, diff
    if wkt:
        assert same_wkt(ours.to_wkt(), theirs.to_wkt()), (ours.to_wkt(), theirs.to_wkt())


@pytest.mark.parametrize("only_required", [False, True])
@pytest.mark.parametrize("name", CONVERSIONS)
def test_conversions(name: str, only_required: bool) -> None:
    if name == "AlbersEqualAreaConversion":
        pytest.skip("test_albers_in_metres")
    kwargs = arguments(getattr(THEIRS.op, name), only_required)
    check(lambda lib: getattr(lib.op, name)(**kwargs), wkt=False)


def test_mercator_a_is_on_the_equator() -> None:
    for lib in (OURS, THEIRS):
        with pytest.raises(lib.crs.CRSError, match="only latitude_natural_origin = 0"):
            lib.op.MercatorAConversion(latitude_natural_origin=1)


def test_albers_in_metres() -> None:
    # pyproj gives Albers' false easting and northing a unit of its own
    # named "Metre"; ours is PROJJSON's "metre", as in its other classes.
    for lib in (OURS, THEIRS):
        units = [p.unit_name for p in lib.op.AlbersEqualAreaConversion(1, 2).params]
        assert units[-2:] == (["metre", "metre"] if lib is OURS else ["Metre", "Metre"])


CASES: dict[str, Callable[[SimpleNamespace], Any]] = {
    # Coordinate systems, every axis of every kind.
    **{
        f"{cs}-{axis.name}": (lambda cs, axis: lambda lib: getattr(lib.cs, cs)(axis.name))(cs, axis)
        for cs, enum in (
            ("Ellipsoidal2DCS", pyproj.crs.enums.Ellipsoidal2DCSAxis),
            ("Ellipsoidal3DCS", pyproj.crs.enums.Ellipsoidal3DCSAxis),
            ("Cartesian2DCS", pyproj.crs.enums.Cartesian2DCSAxis),
            ("VerticalCS", pyproj.crs.enums.VerticalCSAxis),
        )
        for axis in enum
    },
    # Datums, ellipsoids and prime meridians.
    "datum-default": lambda lib: lib.datum.CustomDatum(),
    "datum-parts": lambda lib: lib.datum.CustomDatum(
        "mine", lib.datum.Ellipsoid.from_epsg(7001), lib.datum.PrimeMeridian.from_name("Lisbon")
    ),
    "datum-custom": lambda lib: lib.datum.CustomDatum(
        ellipsoid=lib.datum.CustomEllipsoid(semi_major_axis=6378000, semi_minor_axis=6357000),
        prime_meridian=lib.datum.CustomPrimeMeridian(longitude=3.5),
    ),
    "ellipsoid-rf": lambda lib: lib.datum.CustomEllipsoid(semi_major_axis=6378137, inverse_flattening=298.257222101),
    "ellipsoid-b": lambda lib: lib.datum.CustomEllipsoid(name="b", semi_major_axis=6378137, semi_minor_axis=6356752.314),
    "ellipsoid-radius": lambda lib: lib.datum.CustomEllipsoid(radius=6371000),
    "meridian": lambda lib: lib.datum.CustomPrimeMeridian(longitude=2, name="frank"),
    # CRSs.
    "geographic-default": lambda lib: lib.crs.GeographicCRS(),
    "geographic-custom": lambda lib: lib.crs.GeographicCRS(datum=lib.datum.CustomDatum(ellipsoid="GRS 1980")),
    "geographic-frame": lambda lib: lib.crs.GeographicCRS(datum=1178),
    "derived-geographic": lambda lib: lib.crs.DerivedGeographicCRS(
        4326, lib.op.RotatedLatitudeLongitudeConversion(o_lat_p=37.5, o_lon_p=177.5)
    ),
    "geocentric-custom": lambda lib: lib.crs.GeocentricCRS(datum=lib.datum.CustomDatum(ellipsoid="Airy 1830")),
    "projected-default": lambda lib: lib.crs.ProjectedCRS(lib.op.UTMConversion(33)),
    "projected": lambda lib: lib.crs.ProjectedCRS(
        lib.op.TransverseMercatorConversion(0, 19, 500000, -5300000, 0.9993),
        "PL-1992-like",
        lib.cs.Cartesian2DCS("NORTHING_EASTING"),
        lib.crs.GeographicCRS(datum=lib.datum.CustomDatum("ETRS89-like", "GRS 1980")),
    ),
    "projected-feet": lambda lib: lib.crs.ProjectedCRS(
        lib.op.LambertConformalConic2SPConversion(33, 45, 23, -96),
        cartesian_cs=lib.cs.Cartesian2DCS("EASTING_NORTHING_US_FT"),
        geodetic_crs=4269,
    ),
    "projected-scale": lambda lib: lib.crs.ProjectedCRS(lib.op.LambertCylindricalEqualAreaScaleConversion(10, scale_factor_natural_origin=0.9)),
    "vertical": lambda lib: lib.crs.VerticalCRS("NAVD88 height", "urn:ogc:def:datum:EPSG::5103", geoid_model="GEOID12B"),
    "vertical-depth": lambda lib: lib.crs.VerticalCRS("depth", 5103, lib.cs.VerticalCS("DEPTH_FT")),
    "compound": lambda lib: lib.crs.CompoundCRS(
        "NAD83 + NAVD88 height", [4269, lib.crs.VerticalCRS("NAVD88 height", "urn:ogc:def:datum:EPSG::5103")]
    ),
    "bound": lambda lib: lib.crs.BoundCRS(
        4277, 4326, lib.op.ToWGS84Transformation(4277, 446.448, -125.157, 542.06, 0.15, 0.247, 0.842, -20.489)
    ),
}


# Cases with an ensemble named by a datum URN, which is the ensemble as a
# datum (`urn:ogc:def:datum:EPSG::6326`). pyproj builds them in a process
# of its own: PROJ caches the datum, and pyproj's `CRS(4326).datum` is
# then that datum, not the ensemble, for the rest of the process.
SEPARATE = {
    "geocentric-default": "lib.crs.GeocentricCRS()",
    "geocentric-wgs84": 'lib.crs.GeocentricCRS("geocentric", "urn:ogc:def:datum:EPSG::6326")',
    # ETRS89 as a datum: our newer EPSG's ensemble has more members.
    "geographic-etrs89": 'lib.crs.GeographicCRS("mine", "urn:ogc:def:datum:EPSG::6258", lib.cs.Ellipsoidal3DCS())',
}
PYPROJ_SIDE = """
import json, sys
from types import SimpleNamespace
import pyproj.crs, pyproj.crs.coordinate_operation, pyproj.crs.coordinate_system, pyproj.crs.datum
lib = SimpleNamespace(crs=pyproj.crs, op=pyproj.crs.coordinate_operation, cs=pyproj.crs.coordinate_system, datum=pyproj.crs.datum)
built = {case: eval(source) for case, source in json.loads(sys.argv[1]).items()}
print(json.dumps({case: [crs.to_json_dict(), crs.to_wkt()] for case, crs in built.items()}))
"""


@pytest.fixture(scope="module")
def separate() -> dict[str, list[Any]]:
    run = subprocess.run(
        [sys.executable, "-c", PYPROJ_SIDE, json.dumps(SEPARATE)], capture_output=True, text=True, check=True
    )
    out: dict[str, list[Any]] = json.loads(run.stdout)
    return out


@pytest.mark.parametrize("case", sorted(SEPARATE))
def test_builders_in_a_process_of_their_own(case: str, separate: dict[str, list[Any]]) -> None:
    theirs_json, theirs_wkt = separate[case]
    ours = eval(SEPARATE[case], {"lib": OURS})
    diff = same(comparable(ours.to_json_dict()), comparable(theirs_json))
    assert not diff, diff
    assert same_wkt(ours.to_wkt(), theirs_wkt), (ours.to_wkt(), theirs_wkt)


@pytest.mark.parametrize("case", sorted(CASES))
def test_builders(case: str) -> None:
    # PROJ writes these three axes without their abbreviations, `(h)`,
    # alone of coordinate systems; ours as every other.
    check(CASES[case], wkt=case != "Ellipsoidal3DCS-LATITUDE_LONGITUDE_HEIGHT")


@pytest.mark.parametrize(
    "cls, value",
    [
        ("GeographicCRS", 4326),
        ("ProjectedCRS", 2180),
        ("GeocentricCRS", 4978),
        ("VerticalCRS", 5703),
        ("CompoundCRS", "EPSG:2180+5703"),
        ("DerivedGeographicCRS", "+proj=ob_tran +o_proj=longlat +o_lat_p=37.5 +o_lon_p=177.5"),
    ],
)
def test_builder_classes_hold_crss_of_their_type(cls: str, value: Any) -> None:
    for lib in (OURS, THEIRS):
        crs = getattr(lib.crs, cls).from_user_input(value)
        assert type(crs).__name__ == cls
        assert getattr(lib.crs, cls).from_json_dict(crs.to_json_dict()) == crs
        other = "GeographicCRS" if cls != "GeographicCRS" else "ProjectedCRS"
        with pytest.raises(lib.crs.CRSError, match="Invalid type"):
            getattr(lib.crs, other).from_user_input(value)
