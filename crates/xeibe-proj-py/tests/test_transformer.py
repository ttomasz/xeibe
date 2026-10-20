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
    with pytest.raises(NotImplementedError, match="phase 2"):
        Transformer.from_crs(4326, 3857, accuracy=1.0)
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
