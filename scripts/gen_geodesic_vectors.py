#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["pyproj==3.8.0"]
# ///
"""Generate geodesic test vectors for `xeibe_reproject::geodesic`
(`docs/pyproj-phase2.md`, "Geodesics"), with pyproj's `Geod` as the oracle.

pyproj 3.8.0 calls PROJ 9.8.1's `geodesic.c`, the code the Rust module is
ported from, so the results are expected to agree to the last bit. One file,
`geodesic.jsonl`, one record per problem, on several ellipsoids (WGS 84,
GRS 80, a sphere, oblate and prolate ones with |f| = 1/150 and 1/50, and
f = 0.1, where the direct problem takes its Newton step):

* `inv`: `[lat1, lon1, lat2, lon2]` → `[s12, azi1, azi2]` (`Geod.inv` with
  `return_back_azimuth=False`);
* `fwd`: `[lat1, lon1, azi1, s12]` → `[lat2, lon2, azi2]` (`Geod.fwd`);
* `inv_line`: `[lat1, lon1, lat2, lon2, npts]` → the points from
  `inv_intermediate` with both ends (`initial_idx=0`, `terminus_idx=0`),
  the azimuths, `del_s` and the distance (`geod_inverseline`,
  `geod_position`);
* `fwd_line`: `[lat1, lon1, azi1, del_s, npts]` → the same from
  `fwd_intermediate` (`geod_lineinit`);
* `polygon`: the vertices → `[area, perimeter]`
  (`polygon_area_perimeter`, `geod_polygonarea`).

Points are random, plus the hard cases: nearly and exactly antipodal,
meridional, equatorial, polar, coincident and very short lines, and
polygons around the poles.

    scripts/gen_geodesic_vectors.py              # committed subset -> tests/data/geodesic/
    scripts/gen_geodesic_vectors.py --all        # more of each -> target/geodesic/
"""

from __future__ import annotations

import argparse
import json
import math
import random
import sys
from pathlib import Path

import pyproj
from pyproj import Geod
from pyproj.enums import GeodIntermediateFlag

ROOT = Path(__file__).resolve().parent.parent
COMMITTED = ROOT / "tests/data/geodesic"
ALL = ROOT / "target/geodesic"

ELLIPSOIDS = [
    ("WGS 84", Geod(ellps="WGS84")),
    ("GRS 80", Geod(ellps="GRS80")),
    ("sphere", Geod(a=6370997.0, f=0.0)),
    ("oblate 1/150", Geod(a=6.4e6, f=1 / 150)),
    ("prolate 1/150", Geod(a=6.4e6, f=-1 / 150)),
    ("oblate 1/50", Geod(a=6.4e6, f=1 / 50)),
    ("prolate 1/50", Geod(a=6.4e6, f=-1 / 50)),
    ("oblate 0.1", Geod(a=6.4e6, f=0.1)),
]


def random_point(rng: random.Random) -> tuple[float, float]:
    """Uniform on the sphere."""
    return math.degrees(math.asin(rng.uniform(-1, 1))), rng.uniform(-180, 180)


def hard_pairs(rng: random.Random) -> list[tuple[float, float, float, float]]:
    lat, lon = random_point(rng)
    tiny = 10 ** rng.uniform(-12, -3)
    return [
        (lat, lon, -lat, lon + 180),  # antipodal
        (lat, lon, -lat + tiny, lon + 180 - tiny),  # nearly antipodal
        (lat, lon, -lat - tiny, lon + 179.5),
        (lat, lon, rng.uniform(-90, 90), lon),  # meridian
        (lat, lon, rng.uniform(-90, 90), lon + 180),
        (0.0, lon, 0.0, lon + rng.uniform(-180, 180)),  # equator
        (0.0, lon, 0.0, lon + 180 - tiny),
        (90.0, lon, *random_point(rng)),  # pole
        (-90.0, lon, *random_point(rng)),
        (lat, lon, lat, lon),  # coincident
        (lat, lon, lat + tiny, lon + tiny),  # very short
        (lat, lon, lat, lon + 720 + rng.uniform(-180, 180)),  # unnormalised
    ]


def inv_record(name: str, g: Geod, lat1: float, lon1: float, lat2: float, lon2: float) -> dict:
    azi1, azi2, s12 = g.inv(lon1, lat1, lon2, lat2, return_back_azimuth=False)
    return {"kind": "inv", "ellipsoid": name, "a": g.a, "f": g.f, "in": [lat1, lon1, lat2, lon2], "out": [s12, azi1, azi2]}


def fwd_record(name: str, g: Geod, lat1: float, lon1: float, azi1: float, s12: float) -> dict:
    lon2, lat2, azi2 = g.fwd(lon1, lat1, azi1, s12, return_back_azimuth=False)
    return {"kind": "fwd", "ellipsoid": name, "a": g.a, "f": g.f, "in": [lat1, lon1, azi1, s12], "out": [lat2, lon2, azi2]}


def line_out(r) -> dict:
    return {
        "lats": list(r.lats),
        "lons": list(r.lons),
        "azis": list(r.azis),
        "del_s": r.del_s,
        "dist": r.dist,
    }


def inv_line_record(name: str, g: Geod, lat1: float, lon1: float, lat2: float, lon2: float, npts: int) -> dict:
    r = g.inv_intermediate(
        lon1, lat1, lon2, lat2, npts=npts, initial_idx=0, terminus_idx=0, return_back_azimuth=False, flags=GeodIntermediateFlag.AZIS_KEEP
    )
    return {"kind": "inv_line", "ellipsoid": name, "a": g.a, "f": g.f, "in": [lat1, lon1, lat2, lon2, npts], "out": line_out(r)}


def fwd_line_record(name: str, g: Geod, lat1: float, lon1: float, azi1: float, del_s: float, npts: int) -> dict:
    r = g.fwd_intermediate(
        lon1, lat1, azi1, npts=npts, del_s=del_s, initial_idx=0, terminus_idx=0, return_back_azimuth=False, flags=GeodIntermediateFlag.AZIS_KEEP
    )
    return {"kind": "fwd_line", "ellipsoid": name, "a": g.a, "f": g.f, "in": [lat1, lon1, azi1, del_s, npts], "out": line_out(r)}


def polygon_record(name: str, g: Geod, lats: list[float], lons: list[float]) -> dict:
    area, perimeter = g.polygon_area_perimeter(lons, lats)
    return {"kind": "polygon", "ellipsoid": name, "a": g.a, "f": g.f, "lats": lats, "lons": lons, "out": [area, perimeter]}


def random_polygon(rng: random.Random) -> tuple[list[float], list[float]]:
    kind = rng.random()
    n = rng.randint(3, 16)
    if kind < 0.2:
        # Around a pole, possibly twice.
        lat = rng.choice([1, -1]) * rng.uniform(60, 89.9)
        turns = rng.choice([1, 1, 2])
        lons = [i * 360.0 * turns / n + rng.uniform(-5, 5) for i in range(n)]
        return [lat + rng.uniform(-0.05, 0.05) for _ in range(n)], lons
    # A star-shaped polygon around a random centre, small to hemispheric.
    clat, clon = random_point(rng)
    radius = 10 ** rng.uniform(-4, 1.6)
    angles = sorted(rng.uniform(0, 2 * math.pi) for _ in range(n))
    if rng.random() < 0.5:
        angles.reverse()
    lats = [max(-90.0, min(90.0, clat + radius * math.sin(t))) for t in angles]
    lons = [clon + radius * math.cos(t) / max(0.05, math.cos(math.radians(clat))) for t in angles]
    return lats, lons


def records(n: int, seed: int) -> list[dict]:
    rng = random.Random(seed)
    out: list[dict] = []
    for name, g in ELLIPSOIDS:
        for _ in range(n):
            out.append(inv_record(name, g, *random_point(rng), *random_point(rng)))
            azi = rng.uniform(-180, 180)
            s = rng.choice([rng.uniform(0, 2e7), rng.uniform(-2e7, 0), 10 ** rng.uniform(-6, 7.6)])
            out.append(fwd_record(name, g, *random_point(rng), azi, s))
        for _ in range(max(1, n // 20)):
            for pair in hard_pairs(rng):
                out.append(inv_record(name, g, *pair))
            lat, lon = random_point(rng)
            for azi in (0.0, -0.0, 90.0, 180.0, -180.0, rng.uniform(-1e-15, 1e-15)):
                out.append(fwd_record(name, g, lat, lon, azi, rng.uniform(1, 4e7)))
            out.append(fwd_record(name, g, 90.0, lon, rng.uniform(-180, 180), rng.uniform(1, 2e7)))
            out.append(fwd_record(name, g, 0.0, lon, 90.0, rng.uniform(1, 4e7)))
        for _ in range(max(1, n // 25)):
            out.append(inv_line_record(name, g, *random_point(rng), *random_point(rng), rng.randint(2, 12)))
            del_s = rng.uniform(1, 5e6)
            out.append(fwd_line_record(name, g, *random_point(rng), rng.uniform(-180, 180), del_s, rng.randint(2, 12)))
        for _ in range(max(2, n // 10)):
            out.append(polygon_record(name, g, *random_polygon(rng)))
    return out


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--all", action="store_true", help="many more of each, into target/")
    args = parser.parse_args()
    out_dir, n = (ALL, 5000) if args.all else (COMMITTED, 40)
    out_dir.mkdir(parents=True, exist_ok=True)
    recs = records(n, seed=20261023)
    path = out_dir / "geodesic.jsonl"
    header = {"generator": "scripts/gen_geodesic_vectors.py", "pyproj": pyproj.__version__,
              "proj": pyproj.proj_version_str, "geodesic": pyproj.geod.geodesic_version_str}
    with path.open("w") as f:
        f.write(json.dumps(header) + "\n")
        for r in recs:
            f.write(json.dumps(r, allow_nan=False) + "\n")
    print(f"{path}: {len(recs)} records", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
