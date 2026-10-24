"""Differential tests against pyproj on the oracle ``proj.db``.

``scripts/build_proj_oracle.py`` rebuilds PROJ's database from the EPSG
release in ``xeibe-crs``, so pyproj and xeibe_proj read the same EPSG data
and any difference is ours to explain (docs/pyproj-compat.md, Testing).
Skipped when ``target/proj-oracle/proj.db`` is missing or holds another EPSG
release. The network is off on both sides, so this also checks that the
choice of operation matches pyproj's default install.

Tolerances are those of docs/reprojection.md: 1 mm for projected and
geocentric coordinates, 1e-9° for geographic ones.
"""

from __future__ import annotations

import math
import random
from pathlib import Path
from collections.abc import Iterator
from typing import Any

import pytest

import xeibe_proj
from xeibe_proj import CRS, Transformer, _epsg
from xeibe_proj.exceptions import ProjError

from .conftest import ROOT

pyproj = pytest.importorskip("pyproj")
pytestmark = pytest.mark.oracle

ORACLE = ROOT / "target/proj-oracle"
# Pairs whose results differ from PROJ's for reasons in xeibe-reproject's
# choice of operation: `<source>-><target>  <reason>`. A pair leaves the list
# when it matches; a pair not on it that differs fails the test.
KNOWN = Path(__file__).with_name("oracle-known.txt")


@pytest.fixture(scope="module", autouse=True)
def oracle() -> Iterator[None]:
    if not (ORACLE / "proj.db").exists():
        pytest.skip("target/proj-oracle/proj.db is missing; run scripts/build_proj_oracle.py")
    before = pyproj.datadir.get_data_dir()
    pyproj.datadir.set_data_dir(str(ORACLE))
    version = pyproj.database.get_database_metadata("EPSG.VERSION")
    if version != f"v{xeibe_proj.EPSG_VERSION}":
        pyproj.datadir.set_data_dir(before)
        pytest.skip(f"the oracle holds EPSG {version}, xeibe-crs {xeibe_proj.EPSG_VERSION}")
    pyproj.network.set_network_enabled(False)
    yield
    pyproj.datadir.set_data_dir(before)


def _facts(crs: Any) -> dict[str, object]:
    """What both packages report about a CRS, for comparison."""
    area = crs.area_of_use
    return {
        "name": crs.name,
        "type_name": crs.type_name,
        "is_geographic": crs.is_geographic,
        "is_projected": crs.is_projected,
        "is_vertical": crs.is_vertical,
        "is_geocentric": crs.is_geocentric,
        "is_compound": crs.is_compound,
        "is_engineering": crs.is_engineering,
        "is_deprecated": crs.is_deprecated,
        "to_epsg": crs.to_epsg(),
        "utm_zone": crs.utm_zone,
        "axes": [(a.name, a.abbrev, a.direction, a.unit_name) for a in crs.axis_info],
        "area": None if area is None else tuple(round(v, 6) for v in area.bounds),
        "area_name": None if area is None else area.name,
        "scope": crs.scope,
        "remarks": crs.remarks,
        "datum": crs.datum and crs.datum.name,
        "ellipsoid": crs.ellipsoid and crs.ellipsoid.name,
        "sub_crs": [c.name for c in crs.sub_crs_list],
        "method": crs.coordinate_operation and crs.coordinate_operation.method_name,
    }


def _known(key: str, code: int, ours: object, theirs: object) -> bool:
    """Differences documented in docs/pyproj-compat.md (Implementation status)."""
    record = _epsg.record(code)
    kind = None if record is None else record.kind
    if key == "datum":
        # A vertical CRS on a datum ensemble: pyproj gives no datum.
        return kind == "vertical" and theirs is None
    if key in ("area", "area_name"):
        # EPSG gives a description but no box: PROJ says -1000 on every
        # side, we say None (the description stays in the PROJJSON).
        return ours is None and record is not None and record.area is None
    if key == "remarks":
        # PROJ's database leaves most CRS remarks out; where it has one,
        # it is EPSG's, as ours.
        return theirs is None and ours is not None
    if key == "to_epsg":
        # PROJ's identification skips engineering CRSs, whose id we read.
        return kind == "engineering" and ours == code and theirs is None
    return False


def test_every_crs_matches() -> None:
    """Every EPSG CRS reports what PROJ reports, property by property."""
    mismatches: dict[str, list[str]] = {}
    for code in _epsg.records():
        try:
            theirs = _facts(pyproj.CRS.from_epsg(code))
        except pyproj.exceptions.CRSError:
            continue  # PROJ can't instantiate it either
        ours = _facts(CRS.from_epsg(code))
        for key, value in ours.items():
            if value != theirs[key] and not _known(key, code, value, theirs[key]):
                mismatches.setdefault(key, []).append(f"EPSG:{code}: {value!r} != {theirs[key]!r}")
    report = {k: (len(v), v[:3]) for k, v in sorted(mismatches.items())}
    assert not mismatches, report


def _areas() -> list[tuple[int, tuple[float, float, float, float]]]:
    return [
        (r.code, r.area)
        for r in _epsg.records().values()
        if r.kind in ("geographic_2d", "projected") and not r.deprecated and r.area is not None
        and r.area[0] <= r.area[2]
    ]


def _pairs(n: int, seed: int) -> list[tuple[int, int, list[tuple[float, float]]]]:
    """Random pairs of CRSs whose areas overlap, with points (longitude,
    latitude) inside both."""
    rng = random.Random(seed)
    areas = _areas()
    out: list[tuple[int, int, list[tuple[float, float]]]] = []
    while len(out) < n:
        (a, aa), (b, ba) = rng.sample(areas, 2)
        w, s, e, nn = max(aa[0], ba[0]), max(aa[1], ba[1]), min(aa[2], ba[2]), min(aa[3], ba[3])
        if w >= e or s >= nn:
            continue
        points = [(rng.uniform(w, e), rng.uniform(s, nn)) for _ in range(4)]
        out.append((a, b, points))
    return out


def _close(a: list[float], b: list[float], geographic: bool) -> bool:
    tolerance = 1e-9 if geographic else 1e-3
    return all(math.isclose(x, y, rel_tol=0, abs_tol=tolerance) for x, y in zip(a, b, strict=True))


def _known_pairs() -> dict[str, str]:
    out: dict[str, str] = {}
    for line in KNOWN.read_text().splitlines():
        if line.strip() and not line.startswith("#"):
            pair, _, reason = line.partition(" ")
            out[pair] = reason.strip()
    return out


@pytest.mark.parametrize("seed", range(8))
def test_random_pairs_match(seed: int) -> None:
    """Coordinates converted between random EPSG pairs match PROJ's, with
    the network off on both sides."""
    known = _known_pairs()
    seen: set[str] = set()
    problems: list[str] = []
    compared = 0
    for a, b, lonlat in _pairs(100, seed):
        pair = f"{a}->{b}"
        seen.add(pair)
        try:
            to_a = pyproj.Transformer.from_crs(4326, a, always_xy=True)
            xs, ys = zip(*(to_a.transform(lon, lat, errcheck=True) for lon, lat in lonlat), strict=True)
        except pyproj.exceptions.ProjError:
            continue
        try:
            theirs_t = pyproj.Transformer.from_crs(a, b, always_xy=True)
        except pyproj.exceptions.ProjError:
            continue  # PROJ can't set it up either
        ballpark = "ballpark" in theirs_t.description.lower()
        try:
            ours_t = Transformer.from_crs(a, b, always_xy=True)
        except ProjError as e:
            if not ballpark:
                try:
                    theirs_t.transform(list(xs), list(ys), errcheck=True)
                except pyproj.exceptions.ProjError:
                    continue  # PROJ's operation can't run either
                problems.append(f"{pair}: we raise ({e}), PROJ uses {theirs_t.description}")
            continue
        if ballpark:
            problems.append(f"{pair}: PROJ uses a ballpark, we don't raise")
            continue
        # Without ballparks, as we convert: by default PROJ switches to one
        # for a point outside every operation's area, where we use the best
        # operation anyway (docs/pyproj-compat.md, divergence 1).
        theirs_t = pyproj.Transformer.from_crs(a, b, always_xy=True, allow_ballpark=False)
        try:
            tx, ty = theirs_t.transform(list(xs), list(ys), errcheck=True)
        except pyproj.exceptions.ProjError:
            continue
        try:
            ox, oy = ours_t.transform(list(xs), list(ys))
        except ProjError as e:
            problems.append(f"{pair}: we raise ({e}), PROJ converts")
            continue
        compared += 1
        geographic = pyproj.CRS(b).is_geographic
        if not (_close(ox, tx, geographic) and _close(oy, ty, geographic)):
            worst = max(max(abs(p - q) for p, q in zip(ox, tx)), max(abs(p - q) for p, q in zip(oy, ty)))
            problems.append(f"{pair}: off by {worst:.3g} ({ours_t.description} | {theirs_t.description})")
    assert compared > 40, f"only {compared} pairs could be compared"
    differing = {p.split(":", 1)[0] for p in problems}
    new = [p for p in problems if p.split(":", 1)[0] not in known]
    fixed = sorted(p for p in known if p in seen and p not in differing)
    assert not new, f"{len(new)} of {compared} pairs differ from PROJ:\n" + "\n".join(new)
    assert not fixed, f"these pairs match PROJ now; remove them from {KNOWN.name}: {fixed}"


# CRSs whose Proj differs from pyproj's beyond the tolerances, with why
# (docs/pyproj-phase2.md, Proj): pyproj's PROJ string has +R_A (the
# authalic sphere) for EPSG's spherical LAEA, which PROJ's own conversion
# of the CRS doesn't use; ours matches that conversion.
PROJ_KNOWN = {3973, 3974}


def test_proj_matches_pyproj() -> None:
    """``Proj(code)`` forwards from the middle of each EPSG CRS's area, and
    back from pyproj's result, as pyproj's: within 1 mm (in the CRS's
    unit) and 1e-8°. pyproj's PROJ strings round parameters to 15 digits,
    use PROJ's own prime meridians and ellipsoids where EPSG's are close,
    and read input as WGS 84 when they say +datum=NAD83: up to 0.35 mm."""
    compared, failed, ours_only = 0, [], 0
    for code, record in _epsg.records().items():
        if record.deprecated or record.area is None or record.kind not in (
            "geographic_2d", "geographic_3d", "projected", "geocentric", "compound"
        ):
            continue
        w, s, e, n = record.area
        lon = (w + e) / 2 if w <= e else ((w + e + 360) / 2 + 180) % 360 - 180
        lat = (s + n) / 2
        try:
            theirs = pyproj.Proj(code)
            fwd = theirs(lon, lat, errcheck=True)
            back = theirs(*fwd, inverse=True, errcheck=True)
        except (pyproj.exceptions.ProjError, pyproj.exceptions.CRSError):
            try:
                xeibe_proj.Proj(code)(lon, lat)
                ours_only += 1
            except ProjError:
                pass
            continue
        ours = xeibe_proj.Proj(code)
        compared += 1
        geographic = ours.crs.is_geographic
        ok = _proj_close(ours(lon, lat), fwd, geographic) and _proj_close(ours(*fwd, inverse=True), back, True)
        if not ok and code not in PROJ_KNOWN:
            failed.append(f"EPSG:{code} {ours(lon, lat)} != {fwd}")
    assert compared > 7500 and not failed, (compared, len(failed), failed[:10])
    # Where PROJ can't write a PROJ string, there is no pyproj Proj to
    # compare with; ours still projects.
    assert ours_only > 0


def _proj_close(a: tuple[float, float], b: tuple[float, float], geographic: bool) -> bool:
    tolerance = 1e-8 if geographic else 1e-3
    return all(math.isclose(x, y, rel_tol=0, abs_tol=tolerance) for x, y in zip(a, b, strict=True))


def _dimension_facts(crs: Any) -> dict[str, object]:
    """What to_2d and to_3d give, as both packages report it. Remarks are
    left out: PROJ's database has few, and ours are EPSG's (see _known)."""
    area = crs.area_of_use
    geodetic = crs.geodetic_crs
    return {
        "type_name": crs.type_name,
        "name": crs.name,
        "id": (crs.to_json_dict().get("id") or {}).get("code"),
        "axes": [(a.name, a.abbrev, a.direction, a.unit_name) for a in crs.axis_info],
        "datum": crs.datum and crs.datum.name,
        "scope": crs.scope,
        "area": None if area is None else (area.name, tuple(round(v, 6) for v in area.bounds)),
        "geodetic": geodetic and (geodetic.name, (geodetic.to_json_dict().get("id") or {}).get("code")),
    }


def _known_dimension(code: int, key: str, ours: object, theirs: object) -> bool:
    """pyproj rebuilds the result of to_2d and to_3d from its WKT, which
    names the components of a compound CRS without their ids, and PROJ's
    Pseudo-Mercator keeps its 2D base when promoted
    (docs/pyproj-phase2.md, to_2d)."""
    record = _epsg.record(code)
    if key == "geodetic" and record is not None and record.kind == "compound":
        return isinstance(ours, tuple) and isinstance(theirs, tuple) and ours[0] == theirs[0] and theirs[1] is None
    if code == 3857 and key in ("geodetic", "datum"):
        return True
    return _known(key, code, ours, theirs)


def test_dimensions_match_pyproj() -> None:
    """to_2d and to_3d of every current EPSG CRS, and get_non_deprecated of
    every deprecated one, as pyproj's."""
    mismatches: list[str] = []
    for code, record in _epsg.records().items():
        try:
            theirs = pyproj.CRS.from_epsg(code)
        except pyproj.exceptions.CRSError:
            continue
        ours = CRS.from_epsg(code)
        if record.deprecated:
            # pyproj may give a deprecated CRS's replacement's definition
            # under its own id; only the replacements are compared.
            mine = [c.to_epsg() for c in ours.get_non_deprecated()]
            pyprojs = [c.to_epsg() for c in theirs.get_non_deprecated()]
            if mine != pyprojs:
                mismatches.append(f"EPSG:{code} get_non_deprecated: {mine} != {pyprojs}")
            continue
        assert ours.get_non_deprecated() == []
        for method in ("to_2d", "to_3d"):
            a = _dimension_facts(getattr(ours, method)())
            b = _dimension_facts(getattr(theirs, method)())
            for key, value in a.items():
                if value != b[key] and not _known_dimension(code, key, value, b[key]):
                    mismatches.append(f"EPSG:{code} {method} {key}: {value!r} != {b[key]!r}")
    assert not mismatches, (len(mismatches), mismatches[:10])


def _without_derived(i: object) -> object:
    """An id without PROJ's DERIVED_FROM(…), which PROJ gives an operation
    run between other CRSs than its own only where its search took a pivot
    datum (docs/pyproj-phase2.md, Transformer.to_json)."""
    if not isinstance(i, dict):
        return i
    authority = str(i.get("authority"))
    for wrapper in ("DERIVED_FROM(", "INVERSE(DERIVED_FROM("):
        if authority.startswith(wrapper):
            authority = authority[len(wrapper) :].removesuffix("))" if wrapper.startswith("INVERSE") else ")")
            if wrapper.startswith("INVERSE"):
                authority = f"INVERSE({authority})"
    return {**i, "authority": authority}


def _si(p: dict[str, Any]) -> object:
    value, unit = p.get("value"), p.get("unit")
    if not isinstance(value, (int, float)):
        return value
    factor = {"metre": 1.0, "degree": math.pi / 180, "unity": 1.0}.get(unit, 1.0) if isinstance(unit, str) else 1.0
    if isinstance(unit, dict):
        factor = unit.get("conversion_factor", 1.0)
    return float(f"{value * factor:.9g}")


def _step_facts(s: dict[str, Any], theirs: dict[str, Any] | None = None) -> tuple[object, ...]:
    """A step as both packages write it, in SI units. A projection's
    parameters are the CRS's, compared by the CRS tests; a CRS PROJ uses
    without an id (a 2D one it promoted to 3D on the way) counts as ours."""
    method = s.get("method") or {}
    params = None if s["type"] == "Conversion" else [(p["id"]["code"], _si(p)) for p in s.get("parameters", [])]

    def crs(key: str) -> object:
        code = (s.get(key) or {}).get("id", {}).get("code")
        if theirs is not None and key in s and not (theirs.get(key) or {}).get("id"):
            return None
        return code

    return (
        s["type"], s["name"], (method.get("id") or {}).get("authority"), (method.get("id") or {}).get("code"),
        params, crs("source_crs"), crs("target_crs"), _without_derived(s.get("id")), s.get("accuracy"),
        sorted(s), (s.get("remarks") or "")[:60], s.get("area"),
    )  # fmt: skip


def _operation_facts(d: dict[str, Any] | None, theirs: dict[str, Any] | None = None) -> object:
    if d is None:
        return None
    steps = d.get("steps") or [d]
    their_steps = (theirs.get("steps") or [theirs]) if theirs is not None else [None] * len(steps)
    return (
        d["type"], d["name"], d.get("accuracy"), _without_derived(d.get("id")), d.get("area"), d.get("bbox"),
        sorted(d), [_step_facts(s, t) for s, t in zip(steps, their_steps)], len(steps),
    )  # fmt: skip


def _operation_pairs(n: int, seed: int) -> list[tuple[int, int]]:
    rng = random.Random(seed)
    kinds = ("geographic_2d", "geographic_3d", "geocentric", "projected", "compound")
    areas = [
        (r.code, r.area)
        for r in _epsg.records().values()
        if r.kind in kinds and not r.deprecated and r.area is not None and r.area[0] <= r.area[2]
    ]
    out: list[tuple[int, int]] = []
    while len(out) < n:
        (a, aa), (b, ba) = rng.sample(areas, 2)
        if max(aa[0], ba[0]) < min(aa[2], ba[2]) and max(aa[1], ba[1]) < min(aa[3], ba[3]):
            out.append((a, b))
    return out


@pytest.mark.parametrize("seed", range(3))
def test_operation_json_matches_pyproj(seed: int) -> None:
    """Transformer.to_json_dict of every operation between random EPSG
    pairs, as pyproj's for the same operation, and to_wkt where pyproj
    writes one. PROJ writes a horizontal and a vertical operation run
    together as a PROJ pipeline, which isn't compared."""
    mismatches: list[str] = []
    compared = 0
    for a, b in _operation_pairs(100, seed):
        try:
            theirs = pyproj.transformer.TransformerGroup(a, b, allow_ballpark=False).transformers
        except (pyproj.exceptions.ProjError, IndexError):
            continue  # pyproj 3.8.0 indexes past PROJ's list for some pairs
        try:
            ours = xeibe_proj.transformer.TransformerGroup(a, b).transformers
        except ProjError:
            continue
        by_name = {t.description: t for t in theirs}
        for t in ours:
            u = by_name.get(t.description)
            if u is None:
                continue
            p = u.to_json_dict()
            if p is not None and str(p.get("method", {}).get("name", "")).startswith("PROJ-based operation"):
                continue
            compared += 1
            o = t.to_json_dict()
            if _operation_facts(o, p) != _operation_facts(p):
                mismatches.append(f"{a}->{b} {t.description}: {_operation_facts(o, p)} != {_operation_facts(p)}")
            if (t.to_wkt() is None) != (u.to_wkt() is None):
                mismatches.append(f"{a}->{b} {t.description}: to_wkt {t.to_wkt() is None} != {u.to_wkt() is None}")
    assert compared > 50, f"only {compared} operations could be compared"
    assert not mismatches, (len(mismatches), mismatches[:5])
