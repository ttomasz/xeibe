#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["zstandard>=0.23", "pyproj==3.8.0"]
# ///
"""Generate datum-transformation test vectors for `crates/xeibe-reproject`
(`docs/reprojection.md`, levels 2 and 3), with PROJ as the oracle.

PROJ (pyproj 3.8.0 = PROJ 9.8.1) runs on a `proj.db` rebuilt from the same
EPSG release as `xeibe-crs` (`scripts/build_proj_oracle.py`), so the vectors
test our code, not the gap between EPSG releases. Its operations are EPSG's
plus PROJ's null operations between a datum ensemble and its members, which
xeibe-reproject has too (`target/proj-oracle-epsg`). It runs without
network, with ballpark transformations disallowed (`ONLY_BEST=NO`), and
either:

* without grids, discarding grid-based operations: level 2's world; or
* with `--grids DIR`, a directory of PROJ-data's grids (all of them, as on
  `cdn.proj.org`): level 3's world, the vectors then named `*-grids.jsonl`.

Two kinds of vectors:

* `operations`: one EPSG transformation forced, forward, for points in its
  area of use. Tests each method's math, independent of the selection.
* `selection`: the automatic choice between two CRSs. For each pair, PROJ's
  sorted list of operations, and for points in the area of interest the
  output and the name of the operation PROJ used for each. With
  `--vertical`, between CRSs with heights (`selection-vertical*.jsonl`):
  compound CRSs to geographic 3D ones and to each other.

    scripts/build_proj_oracle.py                    # once per EPSG release
    scripts/gen_transformation_vectors.py           # committed subsets -> tests/data/reprojection/
    scripts/gen_transformation_vectors.py --grids tests/data/reprojection/grids
    scripts/gen_transformation_vectors.py --vertical [--grids tests/data/reprojection/grids]
    scripts/gen_transformation_vectors.py --all     # everything -> target/reprojection/
    scripts/gen_transformation_vectors.py --all --grids target/proj-grids [--vertical]
"""

from __future__ import annotations

import argparse
import json
import math
import random
import re
import struct
import sys
import warnings
from pathlib import Path

import pyproj
import zstandard
from pyproj.transformer import TransformerGroup

ROOT = Path(__file__).resolve().parent.parent
ORACLE = ROOT / "target" / "proj-oracle-epsg"
OPERATIONS_BIN = ROOT / "crates/xeibe-crs/data/operations.bin"
COMMITTED = ROOT / "tests/data/reprojection"
ALL = ROOT / "target/reprojection"

# Methods PROJ can run without grids (its helmert and other transformations).
GRID_FREE_METHODS = {
    1031, 1032, 1033, 1034, 1035, 1037, 1038, 1039, 1053, 1054, 1055, 1056, 1057, 1058,
    1061, 1062, 1063, 1065, 1066, 1132, 1133, 1140, 9603, 9606, 9607, 9636,
    9601, 9618, 9619, 9621, 9624, 9656, 9660,
}  # fmt: skip
# Grid-based methods xeibe implements: NADCON, NTv1, NTv2, NADCON5 (2D,
# 3D), IGN's geocentric translations by grid, velocity grids over a fixed
# time span (NTv2_Vel geographic 3D offsets, Position Vector + NKG
# velocities), TIN interpolation (Cartesian, Geographic2D).
GRID_METHODS = {9613, 9614, 9615, 1074, 1075, 1087, 1114, 1142, 1143, 1138, 1145}
# Vertical methods: geoid models (PROJ's `isGeographic3DToGravityRelatedHeight`
# list, run from a grid), vertical offset grids, VERTCON, the vertical TIN and
# velocity grid; without grids, constant offsets and geoid heights.
GEOID_GRID_METHODS = {
    1025, 1030, 1045, 1047, 1048, 1050, 1059, 1060, 1072, 1073, 1081, 1082, 1099, 1106, 1117,
    1123, 1134, 9661, 9662, 9663, 9664, 9665, 1083, 1088, 1089, 1090, 1091, 1092, 1093, 1094,
    1095, 1096, 1097, 1098, 1100, 1103, 1105, 1109, 1110, 1115, 1118, 1122, 1124, 1127, 1128,
    1135, 9635,
}  # fmt: skip
VERTICAL_GRID_METHODS = GEOID_GRID_METHODS | {1112, 1126, 1071, 1080, 1084, 1085, 1129, 1101, 9658, 1137, 1113}
VERTICAL_FREE_METHODS = {9616, 1046, 1136, 1131}
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


# With --vertical and no --all: compound CRSs always in the committed vectors.
PINNED_COMPOUNDS = {9705, 7409, 5555, 9286, 7415, 8360, 9389, 6871, 5972}

# With --vertical --grids and no --all: pairs through the committed vertical
# fixtures: a geoid model (La Désirade) and two offset grids (Denmark,
# interpolated in ETRS89; Wellington, in NZGD2000).
VERTICAL_GRID_PINNED_PAIRS = [
    (9536, 5488), (5488, 9536), (9536, 4979), ([5489, 5618], 5488), (5488, [5489, 5618]),
    (7416, 9422), (9422, 7416), (4098, 9422),
    (9528, [4167, 5770]), ([4167, 5770], 9528), ([2193, 5770], 9528),
]  # fmt: skip

# With --grids and no --all: pairs whose candidate grids are all in the
# committed fixtures (scripts/gen_grid_fixtures.py), both ways.
GRID_PINNED_PAIRS = [
    (4138, 4269), (4269, 4138), (4139, 4269), (4269, 4139), (4139, 4326), (4326, 4139),
    (4470, 10671), (10671, 4470),
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


# Engineering CRSs: the other end of an EPSG operation each one has, filled
# in by `engineering_pairs`; their points come from there.
ENGINEERING_PARTNERS: dict[int, int] = {}


def engineering_points(code: int, crs: pyproj.CRS, seed: int) -> list[list[float]]:
    """Points in an engineering CRS: PROJ's conversion of points around the
    CRS an EPSG operation joins it to, or, with none, a grid of local
    coordinates (any are valid on a datum no other CRS reaches)."""
    partner = ENGINEERING_PARTNERS.get(code)
    if partner is not None:
        other = pyproj.CRS.from_epsg(partner)
        to_eng = pyproj.Transformer.from_crs(other, crs, authority="EPSG", allow_ballpark=False)
        lonlat = sample_area(seed, *(crs_area(crs) or crs_area(other)))
        return [p for q in points_in(other, lonlat, seed) if (p := clean(to_eng.transform(*q, errcheck=False)))]
    rng = random.Random(seed)
    pts = [[1000.0 + 1500.0 * i + 0.37, 2000.0 + 1300.0 * j + 0.61] for i in range(3) for j in range(3)]
    return pts + [[round(rng.uniform(0.0, 9000.0), 3), round(rng.uniform(0.0, 9000.0), 3)] for _ in range(3)]


def operation_vectors(registry: dict, everything: bool, grids: Path | None) -> list[dict]:
    by_method: dict[int, list[dict]] = {}
    first = "grid_transformation" if grids else "helmert_transformation"
    tables = (first, "other_transformation")
    methods = (GRID_METHODS | VERTICAL_GRID_METHODS) if grids else (GRID_FREE_METHODS | VERTICAL_FREE_METHODS)
    geodetic = {g[0]: g for g in registry["geodetic_crs"]}
    vertical = {v[0] for v in registry["vertical_crs"]}
    for op in registry["operations"]:
        if op["deprecated"] or op["table"] not in tables:
            continue
        if op["method"] not in methods or not op["src"] or not op["dst"]:
            continue
        by_method.setdefault(op["method"], []).append(op)
    chosen = []
    for ops in by_method.values():
        # With grids, every operation whose grids the directory has.
        chosen += ops if everything or grids else ops[:PER_METHOD]
    records = []
    for op in chosen:
        code = op["code"]
        ends = vertical_ends(op, geodetic, vertical)
        if ends is None:
            print(f"operation {code}: skipped (no horizontal CRS for its heights)", file=sys.stderr)
            continue
        src_x, dst_x, swap = ends
        try:
            co = pyproj.crs.CoordinateOperation.from_epsg(code)
            proj_string = co.to_proj4()
            if proj_string is None:
                print(f"operation {code}: PROJ has no pipeline for it", file=sys.stderr)
                continue
            pipeline = pyproj.Transformer.from_pipeline(proj_string)
            src = crs_of(src_x)
            area = co.area_of_use
            if area is None:
                continue
            pin = points_in(src, sample_area(code, area.west, area.south, area.east, area.north), code)
        except (pyproj.exceptions.CRSError, pyproj.exceptions.ProjError) as e:
            print(f"operation {code}: skipped ({e})", file=sys.stderr)
            continue
        # pyproj reports some grids by EPSG's name as unavailable although
        # the pipeline reads PROJ-data's file: look for the pipeline's files.
        if grids and not all(
            (grids / g).exists() for g in re.findall(r"\+(?:grids|file)=(\S+)", proj_string)
        ):
            print(f"operation {code}: skipped (grid missing)", file=sys.stderr)
            continue
        out = [run_pipeline(pipeline, p, swap) for p in pin]
        if op["method"] in GEOID_GRID_METHODS:
            # PROJ's pipeline for a geoid model leaves the height in metres;
            # its CRS-to-CRS pipelines then convert to the target's unit.
            factor = crs_of(dst_x).axis_info[-1].unit_conversion_factor
            out = [None if o is None else [*o[:2], o[2] / factor] for o in out]
        records.append({"op": code, "method": op["method"], "src": src_x, "dst": dst_x, "in": pin, "out": out})
    return records


def crs_of(x) -> pyproj.CRS:
    """An EPSG code, or `[horizontal, vertical]` for their compound CRS."""
    if isinstance(x, list):
        return pyproj.CRS.from_user_input(f"EPSG:{x[0]}+{x[1]}")
    return pyproj.CRS.from_epsg(x)


def seed_code(x) -> int:
    """A CRS's number for seeding its sample points."""
    return x[0] * 100003 + x[1] if isinstance(x, list) else x


def geographic_2d(code: int, geodetic: dict) -> int | None:
    """The geographic 2D CRS of a geographic 3D one's name and datum."""
    g = geodetic.get(code)
    if g is None or g[2] == "geographic 2D":
        return code
    twins = [c for c in geodetic.values() if c[2] == "geographic 2D" and c[1] == g[1] and c[3] == g[3]]
    twins.sort(key=lambda c: (c[4], c[0]))
    return twins[0][0] if twins else None


def vertical_ends(op: dict, geodetic: dict, vertical: set) -> tuple | None:
    """The CRSs xeibe runs a forced operation between, and whether PROJ's
    pipeline takes the first two values the other way round: an operation
    to or from a vertical CRS runs between compound CRSs, its vertical CRS
    with the operation's interpolation CRS (or the geographic CRS of its
    other end, for a geoid model)."""
    src, dst = op["src"], op["dst"]
    if src not in vertical and dst not in vertical:
        return src, dst, False
    params = {p[0]: p[1] for p in op.get("params", [])}
    if src in vertical and dst in vertical:
        interp = int(params.get(1048) or params.get(1037) or 4326)
        h = geographic_2d(interp, geodetic)
        if h is None:
            return None
        # PROJ's vertical steps read the position east first.
        swap = pyproj.CRS.from_epsg(h).axis_info[0].direction in ("north", "south")
        return [h, src], [h, dst], swap
    if dst in vertical:
        h = geographic_2d(src, geodetic)
        return (src, [h, dst], False) if h else None
    h = geographic_2d(dst, geodetic)
    return ([h, src], dst, False) if h else None


def run_pipeline(pipeline, p: list[float], swap: bool) -> list[float] | None:
    if swap:
        out = clean(pipeline.transform(p[1], p[0], *p[2:], errcheck=False))
        return None if out is None else [out[1], out[0], *out[2:]]
    return clean(pipeline.transform(*p, errcheck=False))


def crs_area(crs: pyproj.CRS):
    a = crs.area_of_use
    return None if a is None else (a.west, a.south, a.east, a.north)


def pseudo_area(a) -> float:
    w, s, e, n = a
    if w > e:
        e += 360.0
    return (e - w) * (math.sin(math.radians(n)) - math.sin(math.radians(s)))


def selection_record(src_code, dst_code) -> dict | None:
    """The record for two CRSs: EPSG codes, or `[horizontal, vertical]`."""
    try:
        src = crs_of(src_code)
        dst = crs_of(dst_code)
    except pyproj.exceptions.CRSError:
        return None
    # A compound CRS made up here has no area of its own: its parts'.
    parts = []
    for x, crs in ((src_code, src), (dst_code, dst)):
        parts += crs.sub_crs_list if isinstance(x, list) else [crs]
    areas = [a for a in map(crs_area, parts) if a]
    if not areas:
        return None
    interest = min(areas, key=pseudo_area)
    record: dict = {"src": src_code, "dst": dst_code}
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        try:
            # Informational only: pyproj's TransformerGroup fails on a few pairs.
            group = TransformerGroup(src, dst, allow_ballpark=False)
            record["operations"] = [t.description for t in group.transformers]
        except (pyproj.exceptions.ProjError, pyproj.exceptions.CRSError, IndexError):
            record["operations"] = None
        try:
            transformer = pyproj.Transformer.from_crs(src, dst, allow_ballpark=False, only_best=False)
        except (pyproj.exceptions.ProjError, pyproj.exceptions.CRSError):
            record["points"] = []
            return record
        pts = []
        seed = seed_code(src_code) * 100003 + seed_code(dst_code)
        try:
            if src.is_engineering:
                inputs = engineering_points(src_code, src, seed)
            else:
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
            point = {"in": p, "out": out, "op": op}
            if out is not None:
                try:
                    used = re.findall(r"\+grids=(\S+)", transformer.get_last_used_operation().definition)
                except pyproj.exceptions.ProjError:
                    used = []
                if used:
                    point["grids"] = used
            pts.append(point)
    record["points"] = pts
    return record


def engineering_pairs(registry: dict) -> list[tuple[int, int]]:
    """Pairs with engineering CRSs: across each EPSG operation, both ways;
    each to the geographic CRS of the operation's other end and to WGS 84;
    and between the CRSs on one engineering datum."""
    codes = sorted(int(c) for c in pyproj.get_codes("EPSG", "ENGINEERING_CRS", allow_deprecated=False))
    engineering = set(codes)
    pairs = []
    for op in registry["operations"]:
        a, b = op["src"], op["dst"]
        if op["deprecated"] or not (a in engineering or b in engineering):
            continue
        eng, other = (a, b) if a in engineering else (b, a)
        ENGINEERING_PARTNERS.setdefault(eng, other)
        pairs += [(a, b), (b, a)]
        base = pyproj.CRS.from_epsg(other).geodetic_crs
        if base is not None and base.to_epsg():
            pairs += [(eng, base.to_epsg()), (base.to_epsg(), eng)]
    by_datum: dict[str, list[int]] = {}
    for code in codes:
        pairs.append((code, 4326))
        by_datum.setdefault(pyproj.CRS.from_epsg(code).datum.name, []).append(code)
    for members in by_datum.values():
        pairs += [(x, y) for x in members for y in members if x != y]
    return pairs


def selection_pairs(registry: dict, everything: bool, grids: bool) -> list[tuple[int, int]]:
    if grids and not everything:
        return list(GRID_PINNED_PAIRS)
    pairs = list(PINNED_PAIRS)
    if not grids:
        # Engineering CRSs: few, all in, without grids (none of theirs uses one).
        pairs += [p for p in engineering_pairs(registry) if p not in pairs]
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


def vertical_pairs(registry: dict, everything: bool) -> list[tuple[int, int]]:
    """Pairs with heights: compound CRSs to geographic 3D CRSs (through a
    geoid model, or nothing without one), back, to other compound CRSs
    across EPSG's vertical operations, and to geographic 2D CRSs (the
    height dropped)."""
    geodetic = {g[0]: g for g in registry["geodetic_crs"]}
    vertical = {v[0] for v in registry["vertical_crs"]}
    compounds = []
    for code in sorted(int(c) for c in pyproj.get_codes("EPSG", "COMPOUND_CRS", allow_deprecated=False)):
        crs = pyproj.CRS.from_epsg(code)
        parts = [c.to_epsg() for c in crs.sub_crs_list]
        if len(parts) == 2 and None not in parts:
            compounds.append((code, parts[0], parts[1]))
    if not everything:
        compounds = [c for c in compounds if c[0] in PINNED_COMPOUNDS]

    def base_geographic(h: int) -> int | None:
        if h in geodetic:
            return h
        try:
            base = pyproj.CRS.from_epsg(h).geodetic_crs
        except pyproj.exceptions.CRSError:
            return None
        return base.to_epsg() if base else None

    def twin_3d(g: int) -> int | None:
        row = geodetic.get(g)
        if row is None:
            return None
        twins = [c for c in geodetic.values() if c[2] == "geographic 3D" and c[1] == row[1] and c[3] == row[3]]
        twins.sort(key=lambda c: (c[4], c[0]))
        return twins[0][0] if twins else None

    pairs = []
    for i, (code, h, v) in enumerate(compounds):
        g = base_geographic(h)
        g3 = twin_3d(g) if g else None
        if g3:
            pairs += [(code, g3), (g3, code)]
        pairs.append((code, 4979))
        if g and i % 5 == 0:
            pairs.append((code, g))
    by_vertical: dict[int, list[int]] = {}
    for code, h, v in compounds:
        by_vertical.setdefault(v, []).append(code)
    for op in registry["operations"]:
        a, b = op["src"], op["dst"]
        if op["deprecated"] or a not in vertical or b not in vertical:
            continue
        for x, y in ((a, b), (b, a)):
            if by_vertical.get(x) and by_vertical.get(y):
                pairs.append((by_vertical[x][0], by_vertical[y][0]))
    seen = set()
    return [p for p in pairs if p[0] != p[1] and not (p in seen or seen.add(p))]


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
    parser.add_argument("--grids", type=Path, help="directory of PROJ-data grids: level 3 vectors")
    parser.add_argument("--vertical", action="store_true", help="selection between CRSs with heights")
    args = parser.parse_args()
    if not (ORACLE / "proj.db").exists():
        sys.exit("target/proj-oracle-epsg/proj.db is missing; run scripts/build_proj_oracle.py")
    pyproj.datadir.set_data_dir(str(ORACLE))
    if args.grids:
        pyproj.datadir.append_data_dir(str(args.grids.resolve()))
    pyproj.network.set_network_enabled(False)
    registry = load_registry()
    want = registry["epsg_version"]
    have = pyproj.database.get_database_metadata("EPSG.VERSION")
    if have != f"v{want}":
        sys.exit(f"the oracle has EPSG {have}, xeibe-crs {want}: rerun scripts/build_proj_oracle.py")
    out = ALL if args.all else COMMITTED
    suffix = "-grids" if args.grids else ""
    if args.vertical:
        pairs = VERTICAL_GRID_PINNED_PAIRS if args.grids and not args.all else vertical_pairs(registry, args.all)
        records = [r for pair in pairs if (r := selection_record(*pair))]
        if not args.all:
            # Committed: only pairs with points to check.
            records = [r for r in records if r["points"]]
        write(out / f"selection-vertical{suffix}.jsonl", records)
        return 0
    if args.only in (None, "operations"):
        write(out / f"operations{suffix}.jsonl", operation_vectors(registry, args.all, args.grids))
    if args.only in (None, "selection"):
        pairs = selection_pairs(registry, args.all, bool(args.grids))
        records = [r for pair in pairs if (r := selection_record(*pair))]
        write(out / f"selection{suffix}.jsonl", records)
    return 0


if __name__ == "__main__":
    sys.exit(main())
