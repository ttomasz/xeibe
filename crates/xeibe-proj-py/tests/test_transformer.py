"""Transformer: results against pyproj, container rules, rejected arguments."""

from __future__ import annotations

import math
import pickle
from array import array

import pytest

from xeibe_proj import Transformer
from xeibe_proj.enums import TransformDirection
from xeibe_proj.exceptions import ProjError

pyproj = pytest.importorskip("pyproj")
np = pytest.importorskip("numpy")

# Pairs whose operations didn't change between pyproj's EPSG and ours, with
# points inside both CRSs' areas, in authority axis order.
PAIRS = [
    (4326, 3857, [(52.0, 21.0), (-33.9, 151.2), (0.0, 0.0)]),
    (4326, 32633, [(52.0, 15.0), (45.0, 13.5)]),
    (4258, 25832, [(52.0, 9.0), (48.1, 11.6)]),
    (4326, 4978, [(52.0, 21.0), (-10.0, 120.0)]),
    (27700, 4277, [(400000.0, 300000.0), (530000.0, 180000.0)]),
    (3857, 3035, [(2337709.3, 6800125.5), (1000000.0, 6000000.0)]),
    (4326, 2180, [(52.0, 21.0), (50.1, 19.9)]),
]


@pytest.mark.parametrize(("src", "dst", "points"), PAIRS)
@pytest.mark.parametrize("always_xy", [False, True])
def test_matches_pyproj(src: int, dst: int, points: list[tuple[float, float]], always_xy: bool) -> None:
    ours = Transformer.from_crs(src, dst, always_xy=always_xy)
    theirs = pyproj.Transformer.from_crs(src, dst, always_xy=always_xy)
    xs = [p[1] if always_xy and pyproj.CRS(src).axis_info[0].direction == "north" else p[0] for p in points]
    ys = [p[0] if always_xy and pyproj.CRS(src).axis_info[0].direction == "north" else p[1] for p in points]
    ox, oy = ours.transform(xs, ys)
    tx, ty = theirs.transform(xs, ys)
    # xeibe-reproject agrees with PROJ to well under a millimetre.
    tolerance = 1e-9 if pyproj.CRS(dst).is_geographic else 1e-3
    assert ox == pytest.approx(tx, abs=tolerance)
    assert oy == pytest.approx(ty, abs=tolerance)
    if pyproj.CRS(dst).is_geocentric:
        return  # the way back needs Z
    bx, by = ours.transform(ox, oy, direction="INVERSE")
    assert bx == pytest.approx(xs, abs=1e-7)
    assert by == pytest.approx(ys, abs=1e-7)


def test_containers() -> None:
    t = Transformer.from_crs(4326, 3857)
    x, y = t.transform(52, 21)
    assert isinstance(x, float) and isinstance(y, float)
    lx, ly = t.transform([52.0], [21.0])
    assert isinstance(lx, list) and isinstance(ly, list)
    tx, ty = t.transform((52.0,), (21.0,))
    assert isinstance(tx, tuple) and isinstance(ty, tuple)
    ax, ay = t.transform(array("d", [52.0]), array("d", [21.0]))
    assert isinstance(ax, array) and ax.typecode == "d" and isinstance(ay, array)
    nx, ny = t.transform(np.array([52, 53]), np.array([21.0, 22.0], dtype=np.float32))
    assert isinstance(nx, np.ndarray) and nx.dtype == np.float64
    assert nx[0] == pytest.approx(x) and ny[0] == pytest.approx(y)
    grid = np.full((2, 3), 52.0), np.full((2, 3), 21.0)
    gx, gy = t.transform(*grid)
    assert gx.shape == (2, 3) and np.allclose(gx, x) and np.allclose(gy, y)
    fx, _ = t.transform(np.asfortranarray(grid[0]), np.asfortranarray(grid[1]))
    assert np.allclose(fx, x)


def test_inputs_are_not_modified() -> None:
    t = Transformer.from_crs(4326, 3857)
    xs, ys = np.array([52.0]), np.array([21.0])
    t.transform(xs, ys)
    assert xs[0] == 52.0 and ys[0] == 21.0


def test_z_and_t() -> None:
    t = Transformer.from_crs(4326, 4978)
    theirs = pyproj.Transformer.from_crs(4326, 4978)
    ours = t.transform(52.0, 21.0, 100.0, 2020.5)
    assert ours[3] == 2020.5
    assert ours[:3] == pytest.approx(theirs.transform(52.0, 21.0, 100.0), abs=1e-6)


def test_radians() -> None:
    t = Transformer.from_crs(4326, 3857)
    x, y = t.transform(math.radians(52.0), math.radians(21.0), radians=True)
    assert (x, y) == pytest.approx(t.transform(52.0, 21.0))
    back = t.transform(x, y, radians=True, direction=TransformDirection.INVERSE)
    assert back == pytest.approx((math.radians(52.0), math.radians(21.0)))


def test_ident() -> None:
    t = Transformer.from_crs(4326, 3857)
    assert t.transform(52.0, 21.0, direction="ident") == (52.0, 21.0)


def test_failed_coordinate_raises() -> None:
    t = Transformer.from_crs(4326, 32633)
    with pytest.raises(ProjError, match="coordinate 1"):
        t.transform([52.0, 95.0], [15.0, 15.0])


def test_nan_passes_through() -> None:
    t = Transformer.from_crs(4326, 3857)
    x, y = t.transform(math.nan, math.nan)
    assert math.isnan(x) and math.isnan(y)


@pytest.mark.parametrize(
    "kwargs",
    [{"errcheck": True}, {"errcheck": False}, {"inplace": True}, {"inplace": False}],
)
def test_errcheck_and_inplace_are_rejected(kwargs: dict[str, bool]) -> None:
    t = Transformer.from_crs(4326, 3857)
    with pytest.raises(NotImplementedError, match="always raises"):
        t.transform(52.0, 21.0, **kwargs)
    if "errcheck" in kwargs:
        with pytest.raises(NotImplementedError):
            list(t.itransform([(52.0, 21.0)], **kwargs))
        with pytest.raises(NotImplementedError):
            t.transform_bounds(50, 20, 52, 22, **kwargs)


def test_positional_errcheck_is_rejected() -> None:
    t = Transformer.from_crs(4326, 3857)
    with pytest.raises(NotImplementedError):
        t.transform(52.0, 21.0, None, None, False, True)  # type: ignore[call-overload]


def test_unknown_keyword() -> None:
    with pytest.raises(TypeError):
        Transformer.from_crs(4326, 3857).transform(52.0, 21.0, foo=1)  # type: ignore[call-overload]


def test_itransform_matches_pyproj() -> None:
    points = [(22.95, 40.63), (22.81, 40.53), (23.51, 40.86)]
    for kwargs in ({}, {"switch": True}):
        ours = list(Transformer.from_crs(4326, 2100).itransform(points, **kwargs))
        theirs = list(pyproj.Transformer.from_crs(4326, 2100).itransform(points, **kwargs))
        assert [v for p in ours for v in p] == pytest.approx([v for p in theirs for v in p], abs=1e-3)
    ours3 = list(Transformer.from_crs(4326, 3857).itransform([(52.0, 21.0, 2020.0)], time_3rd=True))
    assert ours3[0][2] == 2020.0
    with pytest.raises(ValueError):
        list(Transformer.from_crs(4326, 3857).itransform([]))


def test_transform_bounds_matches_pyproj() -> None:
    for src, dst, box, always_xy in [
        (4326, 3857, (40, -120, 64, -80), False),
        (4326, 32633, (10.0, 45.0, 20.0, 55.0), True),
        (3857, 4326, (-2e6, 5e6, 2e6, 7e6), True),
    ]:
        ours = Transformer.from_crs(src, dst, always_xy=always_xy).transform_bounds(*box)
        theirs = pyproj.Transformer.from_crs(src, dst, always_xy=always_xy).transform_bounds(*box)
        assert ours == pytest.approx(theirs, rel=1e-9, abs=1e-6)


def test_no_transformation_raises() -> None:
    with pytest.raises(ProjError):
        Transformer.from_crs(4326, 5773)


def test_options() -> None:
    with pytest.raises(NotImplementedError, match="not supported"):
        Transformer.from_crs(4326, 3857, authority="ESRI")
    Transformer.from_crs(4326, 3857, authority="EPSG", allow_ballpark=False)
    with pytest.raises(NotImplementedError, match="not supported"):
        Transformer.from_pipeline("+proj=noop")


def test_identity_and_pickle() -> None:
    t = Transformer.from_crs(4326, 2180, always_xy=True)
    assert t == Transformer.from_crs("EPSG:4326", "EPSG:2180", always_xy=True)
    assert t != Transformer.from_crs(4326, 2180)
    copy = pickle.loads(pickle.dumps(t))
    assert copy == t and copy.transform(21.0, 52.0) == t.transform(21.0, 52.0)
    assert t.source_crs.to_epsg() == 4326 and t.target_crs.to_epsg() == 2180
    assert "Poland CS92" in t.description


def test_ogc_crs84_uses_epsg_operations() -> None:
    ours = Transformer.from_crs("OGC:CRS84", 2180).transform(21.0, 52.0)
    theirs = pyproj.Transformer.from_crs("OGC:CRS84", 2180).transform(21.0, 52.0)
    assert ours == pytest.approx(theirs, abs=1e-3)


def test_metadata_of_a_conversion() -> None:
    t = Transformer.from_crs(4326, 3857)
    assert t.accuracy == 0.0
    assert t.operations == ()
    assert t.area_of_use is not None and t.area_of_use.west == -180.0
    assert t.remarks is None and t.scope is None


def test_metadata_when_chosen_per_coordinate() -> None:
    t = Transformer.from_crs(4277, 4326)
    # Several candidates: PROJ reports -1 and no area, as here.
    assert t.accuracy == -1.0
    assert t.area_of_use is None
    with pytest.raises(ProjError, match="Last used operation not found"):
        t.get_last_used_operation()
    t.transform(52.0, -1.5)
    used = t.get_last_used_operation()
    theirs = pyproj.Transformer.from_crs(4277, 4326)
    theirs.transform(52.0, -1.5)
    assert used.description == theirs.get_last_used_operation().description == "OSGB36 to WGS 84 (6)"
    assert used.accuracy == 2.0
    assert used.area_of_use is not None and used.area_of_use.name is not None
    [operation] = used.operations or ()
    assert operation.name == "OSGB36 to WGS 84 (6)" and operation.accuracy == 2.0
    assert operation.to_json_dict()["id"] == {"authority": "EPSG", "code": 1314}
    copy = pickle.loads(pickle.dumps(used))
    assert copy.description == used.description
    assert copy.transform(52.0, -1.5) == used.transform(52.0, -1.5)


def test_transformer_group() -> None:
    import warnings

    from xeibe_proj import network
    from xeibe_proj.transformer import TransformerGroup

    with pytest.warns(UserWarning, match="uk_os_OSTN15_NTv2_OSGBtoETRS.tif"):
        group = TransformerGroup(4277, 4326)
    assert not group.best_available
    assert [t.description for t in group.transformers][:2] == ["OSGB36 to WGS 84 (6)", "OSGB36 to WGS 84 (1)"]
    [missing] = group.unavailable_operations
    assert missing.name == "OSGB36 to WGS 84 (9)"
    assert [g.short_name for g in missing.grids] == ["uk_os_OSTN15_NTv2_OSGBtoETRS.tif"]
    assert not missing.grids[0].available
    # Each transformer runs its own operation everywhere.
    second = group.transformers[1]
    assert second.accuracy == 21.0 and second.operations
    assert second.transform(52.0, -1.5) != group.transformers[0].transform(52.0, -1.5)
    # With the network on, the grid can be fetched: nothing is fetched yet.
    network.set_network_enabled(True)
    with warnings.catch_warnings():
        warnings.simplefilter("error")
        online = TransformerGroup(4277, 4326)
    assert online.best_available and not online.unavailable_operations
    assert online.transformers[0].description == "OSGB36 to WGS 84 (9)"
    # One datum: the conversion itself.
    assert len(TransformerGroup(4326, 3857).transformers) == 1


def _option_records() -> list[dict[str, object]]:
    import json

    from .conftest import ROOT

    lines = (ROOT / "tests/data/reprojection/selection-options.jsonl").read_text().splitlines()
    return [json.loads(line) for line in lines[1:]]


def test_area_of_interest_and_accuracy_match_proj() -> None:
    """from_crs with pyproj's area_of_interest and accuracy against PROJ's
    results (scripts/gen_transformation_vectors.py --options): the Python
    side passes the options as xeibe-reproject's tests check them."""
    from xeibe_proj.aoi import AreaOfInterest

    compared = 0
    for r in _option_records():
        aoi = r.get("area_of_interest")
        area = AreaOfInterest(*aoi) if isinstance(aoi, list) else None
        accuracy = r.get("accuracy")
        points = r["points"]
        assert isinstance(points, list)
        try:
            t = Transformer.from_crs(r["src"], r["dst"], area_of_interest=area, accuracy=accuracy)  # type: ignore[arg-type]
        except ProjError:
            assert all(p["out"] is None for p in points), r
            continue
        for p in points:
            if p["out"] is None:
                continue
            out = t.transform(*p["in"])
            assert all(math.isclose(a, b, abs_tol=1e-6) for a, b in zip(out, p["out"], strict=False)), (r["src"], r["dst"], p)
            compared += 1
    assert compared > 500


def test_only_best_without_grids() -> None:
    """With the network off, a coordinate whose best operation needs a grid
    that isn't to hand is an error with only_best, a fallback without."""
    lenient = Transformer.from_crs(4267, 4269, always_xy=True, only_best=False)
    assert math.isfinite(lenient.transform(-100.0, 40.0)[0])
    strict = Transformer.from_crs(4267, 4269, always_xy=True, only_best=True)
    with pytest.raises(ProjError, match="grid"):
        strict.transform(-100.0, 40.0)
    # Pickling keeps the options.
    copy = pickle.loads(pickle.dumps(strict))
    with pytest.raises(ProjError, match="grid"):
        copy.transform(-100.0, 40.0)


def test_group_options() -> None:
    from xeibe_proj.aoi import AreaOfInterest
    from xeibe_proj.transformer import TransformerGroup

    everything = TransformerGroup(4230, 4326)
    precise = TransformerGroup(4230, 4326, accuracy=2.0)
    assert 0 < len(precise.transformers) < len(everything.transformers)
    assert all(0 <= t.accuracy <= 2.0 for t in precise.transformers)
    spain = AreaOfInterest(-4.0, 40.0, -3.0, 41.0)
    local = TransformerGroup(4230, 4326, area_of_interest=spain)
    for t in local.transformers:
        area = t.area_of_use
        assert area is not None and area.west <= -3.0 and area.east >= -4.0 and area.south <= 41.0
    # Nothing that accurate: an empty group, as pyproj's.
    none = TransformerGroup(4230, 4326, accuracy=0.001)
    assert none.transformers == [] and none.unavailable_operations == [] and none.best_available
    with pytest.raises(ProjError):
        Transformer.from_crs(4230, 4326, accuracy=0.001)


def test_operation_as_json_and_wkt() -> None:
    import json

    # A projection and its inverse: the conversion, with its EPSG id.
    mercator = Transformer.from_crs(4326, 3857).to_json_dict()
    assert mercator is not None and mercator["type"] == "Conversion"
    assert mercator["id"] == {"authority": "EPSG", "code": 3856}
    back = Transformer.from_crs(3857, 4326).to_json_dict()
    assert back is not None and back["name"] == "Inverse of Popular Visualisation Pseudo-Mercator"
    assert back["method"]["id"]["authority"] == back["id"]["authority"] == "INVERSE(EPSG)"
    # On one datum: PROJ's own operations.
    geocentric = Transformer.from_crs(4326, 4978).to_json_dict()
    assert geocentric is not None and geocentric["method"]["id"]["code"] == 9602
    height = Transformer.from_crs(4326, 4979).to_json_dict()
    assert height is not None and height["type"] == "Transformation" and height["accuracy"] == "0"
    # A transformation and a projection; geocentric translations inverted
    # by their parameters, as PROJ inverts them.
    t = Transformer.from_crs(4326, 2180)
    poland = t.to_json_dict()
    assert poland is not None and poland["type"] == "ConcatenatedOperation"
    assert poland["name"] == t.description and poland["accuracy"] == "1"
    first, second = poland["steps"]
    assert first["name"] == "Inverse of ETRS89 to WGS 84 (1)"
    assert first["id"] == {"authority": "INVERSE(EPSG)", "code": 1149}
    assert first["method"]["id"] == {"authority": "EPSG", "code": 9603}
    assert first["source_crs"]["id"]["code"] == 4326 and "area" not in first
    assert second["id"] == {"authority": "EPSG", "code": 18300}
    assert json.loads(t.to_json() or "") == poland
    assert (t.to_json(pretty=True, indentation=4) or "").startswith('{\n    "$schema"')
    wkt = t.to_wkt() or ""
    assert wkt.startswith('CONCATENATEDOPERATION["Inverse of ETRS89 to WGS 84 (1) + Poland CS92",SOURCECRS[GEOGCRS[')
    assert 'STEP[COORDINATEOPERATION["Inverse of ETRS89 to WGS 84 (1)"' in wkt and wkt.endswith("]")
    # Chosen per coordinate among several: nothing yet, as in pyproj.
    several = Transformer.from_crs(4277, 4326)
    assert several.to_json_dict() is None and several.to_json() is None and several.to_wkt() is None


def test_operation_json_with_ensemble_and_geocentric_steps() -> None:
    from xeibe_proj.transformer import TransformerGroup

    # PROJ's null operation from ETRS89 to a national realization of it.
    group = TransformerGroup(3046, 3131)
    [finland] = [t for t in group.transformers if t.description.startswith("Inverse of UTM zone 34N + ETRS89 to ")]
    d = finland.to_json_dict()
    assert d is not None
    step = d["steps"][1]
    assert step["id"] == {"authority": "PROJ", "code": "ETRS89_TO_ETRS89-FIN[EUREF-FIN]"}
    assert step["remarks"] == "Accuracy 0.1 m, from datum ensemble definition"
    # A transformation between geocentric CRSs, with the conversions to
    # and from them.
    t = Transformer.from_crs(6318, 9000)
    steps = (t.to_json_dict() or {})["steps"]
    assert steps[0]["name"].startswith("Conversion from NAD83(2011) (geog2D) to NAD83(2011) (geocentric)")
    assert steps[-1]["method"]["id"]["code"] == 9602
