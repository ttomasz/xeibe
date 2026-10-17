#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["zstandard>=0.23", "pyproj==3.8.0"]
# ///
"""Generate test vectors for `crates/xeibe-reproject` with PROJ as the oracle.

PROJ (through pyproj) is given the very PROJJSON that `xeibe-crs` embeds, not
its own EPSG database, so the vectors test the math and nothing else: the EPSG
version in pyproj's `proj.db` doesn't matter (docs/reprojection.md, level 1).

For every projected CRS chosen, points are spread over EPSG's area of use (a
3 x 3 grid plus three seeded random points, in the base CRS's own units and
prime meridian) and converted with PROJ:

* `geog` -> `proj`: PROJ's forward conversion, base CRS to projected CRS;
* `proj` -> back to geographic is checked against `geog` by the Rust test.

Points PROJ can't convert are written as `null`, and the Rust side must fail
on them too. Coordinates are in each CRS's authority axis order and units.

Two outputs:

    scripts/gen_reprojection_vectors.py              # committed subset ->
                                                     #   tests/data/reprojection/vectors.jsonl
    scripts/gen_reprojection_vectors.py --all        # every projected CRS ->
                                                     #   target/reprojection/all.jsonl

The committed subset takes a few CRSs per projection method (the lowest live
code, plus ones with non-metre units or a non-Greenwich prime meridian when
the method has them) and a fixed list of CRSs that matter to xeibe's data.
The full set is for `cargo test -p xeibe-reproject -- --ignored`, which reads
it from `target/`, before a release or after an EPSG update.

Geocentric CRSs get their own records: each against the geographic 3D CRS on
the same datum (`kind: "geocentric"`).
"""

from __future__ import annotations

import argparse
import json
import math
import random
import re
import struct
import sys
from pathlib import Path

import pyproj
import zstandard

ROOT = Path(__file__).resolve().parent.parent
BLOB = ROOT / "crates/xeibe-crs/data/projjson.bin"
TABLE = ROOT / "crates/xeibe-crs/src/table.rs"
COMMITTED = ROOT / "tests/data/reprojection/vectors.jsonl"
ALL = ROOT / "target/reprojection/all.jsonl"

# Always in the committed subset: what xeibe's own data uses.
PINNED = [
    2180, 2176, 2177, 2178, 2179,  # ETRS89 / PL-1992, PL-2000 zones
    25832, 25833, 25834, 25835,  # ETRS89 / UTM 32N-35N
    3035,  # ETRS89-extended / LAEA Europe
    3034,  # ETRS89-extended / LCC Europe
    32633, 32634,  # WGS 84 / UTM 33N, 34N
    3857,  # WGS 84 / Pseudo-Mercator
    31466, 31467, 31468,  # DHDN / 3-degree Gauss-Kruger
    27700,  # OSGB36 / British National Grid
    2154,  # RGF93 v1 / Lambert-93
    28992,  # Amersfoort / RD New
    5514,  # S-JTSK / Krovak East North
    3006,  # SWEREF99 TM
    3059,  # LKS-92 / Latvia TM
    3301,  # Estonian Coordinate System of 1997
    2056,  # CH1903+ / LV95
]
PER_METHOD = 3
GRID = 3
RANDOM_POINTS = 3


def read_projjson() -> dict[int, dict]:
    blob = BLOB.read_bytes()
    assert blob[:4] == b"XPJ1", "not a PROJJSON blob"
    shards, entries = struct.unpack_from("<II", blob, 4)
    shard_table = [struct.unpack_from("<III", blob, 12 + i * 12) for i in range(shards)]
    base = 12 + shards * 12
    index = [struct.unpack_from("<IHH", blob, base + i * 8) for i in range(entries)]
    start = base + entries * 8
    dctx = zstandard.ZstdDecompressor()
    texts = [
        dctx.decompress(blob[start + off : start + off + clen], max_output_size=rlen)
        .decode()
        .split("\n")
        for off, clen, rlen in shard_table
    ]
    return {code: json.loads(texts[shard][line]) for code, shard, line in index}


def read_table() -> dict[int, tuple[list[float] | None, bool]]:
    """Area of use ([south, west, north, east], WGS 84 degrees) and the
    deprecated flag, from the generated Rust table."""
    pattern = re.compile(
        r"code: (\d+),.*area_wgs84: (None|Some\(\[([^\]]+)\]\)), deprecated: (true|false)"
    )
    out = {}
    for line in TABLE.read_text().splitlines():
        m = pattern.search(line)
        if m:
            area = None
            if m.group(3):
                area = [float(v.removesuffix("f32")) for v in m.group(3).split(",")]
            out[int(m.group(1))] = (area, m.group(4) == "true")
    return out


def unit_degrees(unit) -> float:
    """Size of an angular unit in degrees."""
    if unit in (None, "degree"):
        return 1.0
    if isinstance(unit, dict):
        return unit["conversion_factor"] * 180.0 / math.pi
    raise ValueError(f"unexpected angular unit {unit!r}")


def pm_degrees(datum: dict) -> float:
    pm = datum.get("prime_meridian")
    if not pm:
        return 0.0
    lon = pm["longitude"]
    if isinstance(lon, dict):
        return lon["value"] * unit_degrees(lon.get("unit"))
    return float(lon)


def geographic_points(code: int, area: list[float], geog: dict) -> list[list[float]]:
    """Points over the area of use, in the geographic CRS's axis order, units
    and prime meridian."""
    south, west, north, east = area
    if east < west:  # crosses the antimeridian
        east += 360.0
    rng = random.Random(code)
    lonlat = []
    for i in range(GRID):
        for j in range(GRID):
            lonlat.append(
                (west + (east - west) * (j + 0.5) / GRID, south + (north - south) * (i + 0.5) / GRID)
            )
    for _ in range(RANDOM_POINTS):
        lonlat.append((rng.uniform(west, east), rng.uniform(south, north)))
    datum = geog.get("datum") or geog.get("datum_ensemble")
    pm = pm_degrees(datum)
    axes = geog["coordinate_system"]["axis"]
    points = []
    for lon, lat in lonlat:
        lon = (lon - pm + 180.0) % 360.0 - 180.0
        values = []
        for axis in axes:
            if axis["direction"] == "east":
                values.append(lon / unit_degrees(axis.get("unit")))
            elif axis["direction"] == "north":
                values.append(lat / unit_degrees(axis.get("unit")))
            elif axis["direction"] == "up":
                values.append(0.0)
            else:
                raise ValueError(f"EPSG:{code}: unexpected axis {axis['direction']}")
        points.append(values)
    return points


def clean(values) -> list[float] | None:
    return list(values) if all(math.isfinite(v) for v in values) else None


def with_schema(crs: dict) -> dict:
    return {"$schema": "https://proj.org/schemas/v0.7/projjson.schema.json", **crs}


def source_ref(base: dict, docs: dict[int, dict]) -> int | dict:
    """The base CRS as an EPSG code when xeibe-crs has the identical document,
    else the document itself."""
    base_id = base.get("id", {})
    if base_id.get("authority") == "EPSG":
        code = base_id["code"]
        doc = docs.get(code)
        if doc is not None:
            stripped = {k: v for k, v in doc.items() if k != "$schema"}
            if stripped == base:
                return code
    return with_schema(base)


def projected_record(code: int, doc: dict, area, docs) -> dict | None:
    base = doc["base_crs"]
    try:
        src = pyproj.CRS.from_json_dict(with_schema(base))
        dst = pyproj.CRS.from_json_dict(doc)
        transformer = pyproj.Transformer.from_crs(src, dst)
    except (pyproj.exceptions.CRSError, pyproj.exceptions.ProjError) as e:
        print(f"EPSG:{code}: PROJ can't convert it: {e}", file=sys.stderr)
        return None
    points = geographic_points(code, area, base)
    projected = []
    for p in points:
        try:
            out = transformer.transform(*p, errcheck=True)
        except pyproj.exceptions.ProjError:
            out = (math.inf, math.inf)
        projected.append(clean(out[:2]))
    return {
        "code": code,
        "kind": "projected",
        "method": doc["conversion"]["method"]["name"],
        "src": source_ref(base, docs),
        "geog": [p[:2] for p in points],
        "proj": projected,
    }


def geocentric_record(code: int, doc: dict, area, docs) -> dict | None:
    datum_key = "datum" if "datum" in doc else "datum_ensemble"
    geog = {
        "type": "GeographicCRS",
        "name": f"{doc['name']} (geographic 3D)",
        datum_key: doc[datum_key],
        "coordinate_system": {
            "subtype": "ellipsoidal",
            "axis": [
                {"name": "Geodetic latitude", "abbreviation": "Lat", "direction": "north", "unit": "degree"},
                {"name": "Geodetic longitude", "abbreviation": "Lon", "direction": "east", "unit": "degree"},
                {"name": "Ellipsoidal height", "abbreviation": "h", "direction": "up", "unit": "metre"},
            ],
        },
    }
    src = pyproj.CRS.from_json_dict(with_schema(geog))
    dst = pyproj.CRS.from_json_dict(doc)
    transformer = pyproj.Transformer.from_crs(src, dst)
    points = geographic_points(code, area, geog)
    rng = random.Random(code + 1)
    for p in points:
        p[2] = round(rng.uniform(-100.0, 3000.0), 3)
    out = [clean(transformer.transform(*p)) for p in points]
    return {
        "code": code,
        "kind": "geocentric",
        "src": with_schema(geog),
        "geog": points,
        "proj": out,
    }


def choose_committed(docs, table) -> list[int]:
    live: dict[str, list[int]] = {}
    retired: dict[str, list[int]] = {}
    for code, doc in sorted(docs.items()):
        if doc["type"] != "ProjectedCRS":
            continue
        area, deprecated = table.get(code, (None, True))
        if area is None:
            continue
        method = doc["conversion"]["method"]["name"]
        (retired if deprecated else live).setdefault(method, []).append(code)
    # Live CRSs first; a method only deprecated CRSs use (Mercator (1SP)
    # (Spherical), EPSG:3785) is still tested with those.
    by_method = {m: live.get(m) or retired[m] for m in live.keys() | retired.keys()}
    chosen = set(PINNED)
    for codes in by_method.values():
        picked = [codes[0]]

        def odd_unit(c):
            axis = docs[c]["coordinate_system"]["axis"][0]
            return axis.get("unit", "metre") != "metre"

        def odd_pm(c):
            base = docs[c]["base_crs"]
            return pm_degrees(base.get("datum") or base.get("datum_ensemble")) != 0.0

        for pred in (odd_unit, odd_pm):
            extra = next((c for c in codes if pred(c) and c not in picked), None)
            if extra is not None:
                picked.append(extra)
        for c in codes:
            if len(picked) >= PER_METHOD:
                break
            if c not in picked:
                picked.append(c)
        chosen.update(picked[:PER_METHOD])
    return sorted(chosen)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--all", action="store_true", help="every projected and geocentric CRS, into target/")
    args = parser.parse_args()

    docs = read_projjson()
    table = read_table()
    if args.all:
        codes = sorted(c for c, d in docs.items() if d["type"] in ("ProjectedCRS", "GeodeticCRS"))
        out_path = ALL
    else:
        codes = choose_committed(docs, table)
        # A few geocentric CRSs: ETRS89, WGS 84, and the first live one.
        codes += [4936, 4978]
        out_path = COMMITTED

    records = []
    for code in codes:
        doc = docs[code]
        area, _ = table.get(code, (None, True))
        if area is None:
            continue
        if doc["type"] == "ProjectedCRS":
            record = projected_record(code, doc, area, docs)
        elif doc["type"] == "GeodeticCRS" and doc["coordinate_system"]["subtype"] == "Cartesian":
            record = geocentric_record(code, doc, area, docs)
        else:
            continue
        if record is not None:
            records.append(record)

    out_path.parent.mkdir(parents=True, exist_ok=True)
    with out_path.open("w") as f:
        f.write(
            json.dumps(
                {
                    "generator": "scripts/gen_reprojection_vectors.py",
                    "pyproj": pyproj.__version__,
                    "proj": pyproj.proj_version_str,
                }
            )
            + "\n"
        )
        for record in records:
            f.write(json.dumps(record, separators=(",", ":")) + "\n")
    size = out_path.stat().st_size
    print(f"{len(records)} CRSs, {size / 1024:.0f} KiB -> {out_path.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
