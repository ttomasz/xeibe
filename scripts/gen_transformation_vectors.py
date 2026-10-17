#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["zstandard>=0.23", "pyproj==3.8.0"]
# ///
"""Generate datum-transformation test vectors for `crates/xeibe-reproject`
(`docs/reprojection.md`, level 2), with PROJ as the oracle.

PROJ (pyproj 3.8.0 = PROJ 9.8.1) runs on a `proj.db` rebuilt from the same
EPSG release as `xeibe-crs` (`scripts/build_proj_oracle.py`), so the vectors
test our code, not the gap between EPSG releases. It runs without grids and
without network, with ballpark transformations disallowed and grid-based
operations discarded (`ONLY_BEST=NO`): exactly level 2's world.

Two kinds of vectors:

* `operations`: one EPSG transformation forced, forward, for points in its
  area of use. Tests each method's math, independent of the selection.
* `selection`: the automatic choice between two CRSs. For each pair, PROJ's
  sorted list of operations, and for points in the area of interest the
  output and the name of the operation PROJ used for each.

    scripts/build_proj_oracle.py                    # once per EPSG release
    scripts/gen_transformation_vectors.py           # committed subsets -> tests/data/reprojection/
    scripts/gen_transformation_vectors.py --all     # everything -> target/reprojection/
"""

from __future__ import annotations

import argparse
import json
import math
import random
import struct
import sys
import warnings
from pathlib import Path

import pyproj
import zstandard
from pyproj.transformer import TransformerGroup

ROOT = Path(__file__).resolve().parent.parent
ORACLE = ROOT / "target" / "proj-oracle"
OPERATIONS_BIN = ROOT / "crates/xeibe-crs/data/operations.bin"
COMMITTED = ROOT / "tests/data/reprojection"
ALL = ROOT / "target/reprojection"

# Methods PROJ can run without grids (its helmert and other transformations).
GRID_FREE_METHODS = {
    1031, 1032, 1033, 1034, 1035, 1037, 1038, 1039, 1053, 1054, 1055, 1056, 1057, 1058,
    1061, 1062, 1063, 1065, 1066, 1132, 1133, 1140, 9603, 9606, 9607, 9636,
    9601, 9618, 9619, 9621, 9624, 9656, 9660,
}  # fmt: skip
PER_METHOD = 4

# Pairs always in the committed selection vectors: what xeibe's data meets.
PINNED_PAIRS = [
    (2180, 4326), (2180, 2176), (2180, 2177), (2180, 2178), (2180, 2179), (4258, 4326),
    (2176, 4326), (25832, 4326), (25833, 4326), (31466, 25832), (31467, 25832), (31468, 25833),
    (27700, 4326), (27700, 4258), (2154, 4326), (28992, 4326), (5514, 4326), (2056, 4326),
    (3006, 4326), (3301, 4326), (3059, 4326), (4230, 4326), (4230, 4258), (4314, 4258),
    (4267, 4269), (4269, 4326), (32633, 4258), (3035, 4326), (4936, 4978), (4937, 4979),
    (31700, 4326), (2100, 4326), (3763, 4326), (2193, 4326), (4283, 7844), (4807, 4326),
]  # fmt: skip


def load_registry() -> dict:
    blob = OPERATIONS_BIN.read_bytes()
    assert blob[:4] == b"XOP1"
    raw_len = struct.unpack("<I", blob[4:8])[0]
    return json.loads(zstandard.ZstdDecompressor().decompress(blob[8:], max_output_size=raw_len))


def clean(values) -> list[float] | None:
    values = list(values)
    return values if all(math.isfinite(v) for v in values) else None


def sample_area(seed: int, west: float, south: float, east: float, north: float) -> list[tuple[float, float]]:
    """A 3 x 3 grid and three random points, as (lon, lat) degrees."""
    if east < west:
        east += 360.0
    rng = random.Random(seed)
    pts = [
        (west + (east - west) * (j + 0.5) / 3, south + (north - south) * (i + 0.5) / 3)
        for i in range(3)
        for j in range(3)
    ]
    pts += [(rng.uniform(west, east), rng.uniform(south, north)) for _ in range(3)]
    return [((lon + 180.0) % 360.0 - 180.0, lat) for lon, lat in pts]


def points_in(crs: pyproj.CRS, lonlat: list[tuple[float, float]], seed: int) -> list[list[float]]:
    """Points in `crs` (authority order and units) near the given lon/lat."""
    to_crs = pyproj.Transformer.from_crs("EPSG:4326", crs, allow_ballpark=True)
    dim = len(crs.axis_info)
    rng = random.Random(seed)
    out = []
    for lon, lat in lonlat:
        if dim == 3:
            h = round(rng.uniform(0.0, 500.0), 3)
            p = to_crs.transform(lat, lon, h, errcheck=False)
        else:
            p = to_crs.transform(lat, lon, errcheck=False)
        p = clean(p)
        if p is not None:
            if dim == 3 and len(p) == 2:
                p.append(0.0)
            out.append(p)
    return out


def operation_vectors(registry: dict, everything: bool) -> list[dict]:
    by_method: dict[int, list[dict]] = {}
    for op in registry["operations"]:
        if op["deprecated"] or op["table"] not in ("helmert_transformation", "other_transformation"):
            continue
        if op["method"] not in GRID_FREE_METHODS or not op["src"] or not op["dst"]:
            continue
        by_method.setdefault(op["method"], []).append(op)
    chosen = []
    for ops in by_method.values():
        chosen += ops if everything else ops[:PER_METHOD]
    records = []
    for op in chosen:
        code = op["code"]
        try:
            co = pyproj.crs.CoordinateOperation.from_epsg(code)
            proj_string = co.to_proj4()
            if proj_string is None:
                print(f"operation {code}: PROJ has no pipeline for it", file=sys.stderr)
                continue
            pipeline = pyproj.Transformer.from_pipeline(proj_string)
            src = pyproj.CRS.from_epsg(op["src"])
            area = co.area_of_use
            if area is None:
                continue
            pin = points_in(src, sample_area(code, area.west, area.south, area.east, area.north), code)
        except (pyproj.exceptions.CRSError, pyproj.exceptions.ProjError) as e:
            print(f"operation {code}: skipped ({e})", file=sys.stderr)
            continue
        out = [clean(pipeline.transform(*p, errcheck=False)) for p in pin]
        records.append(
            {"op": code, "method": op["method"], "src": op["src"], "dst": op["dst"], "in": pin, "out": out}
        )
    return records


def crs_area(crs: pyproj.CRS):
    a = crs.area_of_use
    return None if a is None else (a.west, a.south, a.east, a.north)


def pseudo_area(a) -> float:
    w, s, e, n = a
    if w > e:
        e += 360.0
    return (e - w) * (math.sin(math.radians(n)) - math.sin(math.radians(s)))


def selection_record(src_code: int, dst_code: int) -> dict | None:
    try:
        src = pyproj.CRS.from_epsg(src_code)
        dst = pyproj.CRS.from_epsg(dst_code)
    except pyproj.exceptions.CRSError:
        return None
    areas = [a for a in (crs_area(src), crs_area(dst)) if a]
    if not areas:
        return None
    interest = min(areas, key=pseudo_area)
    record: dict = {"src": src_code, "dst": dst_code}
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        try:
            # Informational only: pyproj's TransformerGroup fails on a few pairs.
            group = TransformerGroup(src, dst, authority="EPSG", allow_ballpark=False)
            record["operations"] = [t.description for t in group.transformers]
        except (pyproj.exceptions.ProjError, pyproj.exceptions.CRSError, IndexError):
            record["operations"] = None
        try:
            transformer = pyproj.Transformer.from_crs(
                src, dst, authority="EPSG", allow_ballpark=False, only_best=False
            )
        except (pyproj.exceptions.ProjError, pyproj.exceptions.CRSError):
            record["points"] = []
            return record
        pts = []
        seed = src_code * 100003 + dst_code
        try:
            inputs = points_in(src, sample_area(seed, *interest), seed)
        except pyproj.exceptions.ProjError:
            return None
        for p in inputs:
            out = clean(transformer.transform(*p, errcheck=False))
            op = None
            if out is not None:
                try:
                    op = transformer.get_last_used_operation().description
                except pyproj.exceptions.ProjError:
                    # A transformer with a single operation reports none.
                    op = transformer.description
            pts.append({"in": p, "out": out, "op": op})
    record["points"] = pts
    return record


def selection_pairs(registry: dict, everything: bool) -> list[tuple[int, int]]:
    pairs = list(PINNED_PAIRS)
    if everything:
        seen = set(pairs)
        # Level 2 is about geodetic CRSs: pairs of them.
        geodetic = {g[0] for g in registry["geodetic_crs"]}
        for op in registry["operations"]:
            if op["deprecated"] or op["table"] == "conversion" or not op["src"] or not op["dst"]:
                continue
            if op["src"] not in geodetic or op["dst"] not in geodetic or op["src"] == op["dst"]:
                continue
            pair = (op["src"], op["dst"])
            if pair not in seen:
                seen.add(pair)
                pairs.append(pair)
        # Pairs no single operation joins, so the search has to pivot: every
        # live geographic 2D CRS to WGS 84 and ETRS89, and every tenth live
        # projected CRS to WGS 84. Then every CRS with several areas of use
        # (xeibe takes the area covering them, PROJ the first) to both.
        extra = []
        for g in registry["geodetic_crs"]:
            code, _, kind, _, deprecated = g
            if kind == "geographic 2D" and not deprecated:
                extra += [(code, 4326), (code, 4258)]
        projected = sorted(
            r for r in (int(c) for c in pyproj.get_codes("EPSG", "PROJECTED_CRS", allow_deprecated=False))
        )
        extra += [(code, 4326) for code in projected[::10]]
        for code, areas in registry["crs_extent"]:
            if len(areas) > 1 and code in set(projected) | {g[0] for g in registry["geodetic_crs"]}:
                extra += [(code, 4326), (code, 4258)]
        for pair in extra:
            if pair[0] != pair[1] and pair not in seen:
                seen.add(pair)
                pairs.append(pair)
    return pairs


def write(path: Path, records: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w") as f:
        header = {
            "generator": "scripts/gen_transformation_vectors.py",
            "pyproj": pyproj.__version__,
            "proj": pyproj.proj_version_str,
            "epsg": pyproj.database.get_database_metadata("EPSG.VERSION"),
        }
        f.write(json.dumps(header) + "\n")
        for r in records:
            f.write(json.dumps(r, separators=(",", ":")) + "\n")
    print(f"{len(records)} records, {path.stat().st_size / 1024:.0f} KiB -> {path.relative_to(ROOT)}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--all", action="store_true", help="every operation and pair, into target/")
    parser.add_argument("--only", choices=("operations", "selection"), help="one kind of vectors")
    args = parser.parse_args()
    if not (ORACLE / "proj.db").exists():
        sys.exit("target/proj-oracle/proj.db is missing; run scripts/build_proj_oracle.py")
    pyproj.datadir.set_data_dir(str(ORACLE))
    pyproj.network.set_network_enabled(False)
    registry = load_registry()
    want = registry["epsg_version"]
    have = pyproj.database.get_database_metadata("EPSG.VERSION")
    if have != f"v{want}":
        sys.exit(f"the oracle has EPSG {have}, xeibe-crs {want}: rerun scripts/build_proj_oracle.py")
    out = ALL if args.all else COMMITTED
    if args.only in (None, "operations"):
        write(out / "operations.jsonl", operation_vectors(registry, args.all))
    if args.only in (None, "selection"):
        records = [r for pair in selection_pairs(registry, args.all) if (r := selection_record(*pair))]
        write(out / "selection.jsonl", records)
    return 0


if __name__ == "__main__":
    sys.exit(main())
