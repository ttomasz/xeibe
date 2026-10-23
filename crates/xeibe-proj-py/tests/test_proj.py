"""``Proj`` against pyproj's (docs/pyproj-phase2.md, Proj). Every EPSG CRS
is compared on the oracle database in test_oracle.py; these are the calling
conventions."""

from __future__ import annotations

import math
import pickle

import pytest

import xeibe_proj
from xeibe_proj import Proj, Transformer
from xeibe_proj.exceptions import CRSError, ProjError

pyproj = pytest.importorskip("pyproj")

# (code, lon, lat): N/E axes, westing/southing (TMSO), polar N/E, Krovak
# East North, a geographic CRS, a geocentric and a compound one.
CASES = [
    (2180, 19.5, 52.0),
    (3035, 10.0, 52.0),
    (2053, 29.0, -26.0),
    (32661, 10.0, 80.0),
    (5514, 15.0, 49.5),
    (27700, -2.0, 54.0),
    (4326, 190.0, 10.0),
    (4978, 10.0, 50.0),
    (9707, 10.0, 50.0),
]


def close(a: tuple[float, ...], b: tuple[float, ...], tolerance: float) -> bool:
    return all(math.isclose(x, y, rel_tol=0, abs_tol=tolerance) for x, y in zip(a, b, strict=True))


@pytest.mark.parametrize(("code", "lon", "lat"), CASES)
def test_matches_pyproj(code: int, lon: float, lat: float) -> None:
    ours, theirs = Proj(code), pyproj.Proj(code)
    fwd = theirs(lon, lat)
    assert close(ours(lon, lat), fwd, 1e-6)
    assert close(ours(*fwd, inverse=True), theirs(*fwd, inverse=True), 1e-9)
    rad = (math.radians(lon), math.radians(lat))
    assert close(ours(*rad, radians=True), theirs(*rad, radians=True), 1e-6)
    back = theirs(*fwd, inverse=True, radians=True)
    assert close(ours(*fwd, inverse=True, radians=True), back, 1e-11)


def test_containers_and_transformer_api() -> None:
    p = Proj(2180)
    xs, ys = p([19.0, 20.0], [52.0, 53.0])
    assert isinstance(xs, list) and isinstance(ys, list)
    assert p.transform(19.0, 52.0) == p(19.0, 52.0)
    assert p.transform(*p(19.0, 52.0), direction="INVERSE") == pytest.approx((19.0, 52.0))
    assert list(p.itransform([(19.0, 52.0)])) == [p(19.0, 52.0)]
    assert close(p.transform_bounds(14, 49, 24, 55), pyproj.Proj(2180).transform_bounds(14, 49, 24, 55), 1e-6)
    assert isinstance(p, Transformer)
    assert p.source_crs is None and p.target_crs is None
    assert p.crs == xeibe_proj.CRS(2180) and p.srs == "EPSG:2180"
    assert p.to_latlong()(19.0, 52.0) == pytest.approx((19.0, 52.0))


def test_units_and_meridians() -> None:
    # EPSG's parameters at full precision: within 1 mm of pyproj's rounded
    # PROJ string in feet.
    assert close(Proj(2249)(-71.0, 42.3), pyproj.Proj(2249)(-71.0, 42.3), 1e-3)
    metres = Proj(2249, preserve_units=False)(-71.0, 42.3)
    assert close(metres, pyproj.Proj(2249, preserve_units=False)(-71.0, 42.3), 1e-3)
    assert close(metres, (Proj(2249)(-71.0, 42.3)[0] * 0.3048006096012192, metres[1]), 1e-6)
    # Longitude from Greenwich in, from Paris out (EPSG's Paris, 3e-9° from
    # PROJ's +pm=paris).
    assert close(Proj(4807)(2.5, 46.5), pyproj.Proj(4807)(2.5, 46.5), 1e-8)


def test_equality_and_pickling() -> None:
    p = Proj(2180)
    assert p == Proj("EPSG:2180") and p != Proj(32633)
    assert Proj(2249) != Proj(2249, preserve_units=False)
    assert hash(p) == hash(Proj("EPSG:2180"))
    copy = pickle.loads(pickle.dumps(p))
    assert copy == p and copy(19.0, 52.0) == p(19.0, 52.0)
    feet = Proj(2249, preserve_units=False)
    assert pickle.loads(pickle.dumps(feet))(-71.0, 42.3) == feet(-71.0, 42.3)
    assert "Transverse Mercator" in repr(p)


def test_errors() -> None:
    with pytest.raises(CRSError):
        Proj(5773)
    with pytest.raises(ProjError):
        Proj(2180)(19.0, 95.0)
    with pytest.raises(NotImplementedError, match="errcheck"):
        Proj(2180)(19.0, 52.0, False, True)  # type: ignore[call-arg]
    with pytest.raises(NotImplementedError, match="phase 3"):
        Proj(proj="utm", zone=33)
    with pytest.raises(NotImplementedError, match="phase 3"):
        Proj(2180).definition_string()
