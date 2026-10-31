#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["pyproj==3.8.0"]
# ///
"""Generate PROJ string test vectors for `xeibe-reproject`'s PROJ string
reader and writer (`docs/pyproj-phase3.md`, "PROJ strings"), with PROJ as
the oracle.

PROJ strings have no specification: what a string means as a CRS is what
PROJ 9.8.1's `PROJStringParser` makes of it, and what a CRS's string is is
what its `exportToPROJString` writes. PROJ (pyproj 3.8.0) runs on the full
`proj.db` rebuilt from the same EPSG release as `xeibe-crs`
(`scripts/build_proj_oracle.py`, `target/proj-oracle`). Two files:

* `epsg.jsonl`: for every EPSG CRS PROJ has, PROJ's `to_proj4()` (PROJ_5,
  the default, and PROJ_4) or the error, and PROJ's reading of that string
  back: its PROJJSON, `to_epsg()` and `to_proj4()`. The writer's test set.
* `corpus.jsonl`: hand-written strings, each as pyproj prepares it (`+proj`
  first, `+type=crs` added; the `srs` field) with PROJ's PROJJSON or its
  error, `to_epsg()`, `to_proj4()`, and a few points converted from the
  CRS's geodetic CRS (`always_xy`, `null` where PROJ fails). The strings
  come from PROJ's `test/unit/test_io.cpp` (the tarball
  `build_proj_oracle.py` downloads), pyproj's tests (`target/pyproj-src`,
  cloned at the tag `tests/pyproj_suite/run.py` pins when missing), and the
  list in `EXTRA` (GeoPandas' tests and cases the design names). The
  reader's test set.

The committed subset (`tests/data/proj-string/`) has the whole corpus and,
of the EPSG CRSs, `PINNED` plus a few per conversion method. `--all` writes
every EPSG CRS to `target/proj-string/` for the `--ignored` tests.

    scripts/gen_proj_string_vectors.py
    scripts/gen_proj_string_vectors.py --all
"""

from __future__ import annotations

import argparse
import json
import math
import os
import random
import re
import sqlite3
import subprocess
import sys
import tarfile
import time
import warnings
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

import pyproj
from pyproj.enums import ProjVersion

ROOT = Path(__file__).resolve().parent.parent
ORACLE = ROOT / "target" / "proj-oracle"
TARBALL = ORACLE / "build" / "proj-9.8.1.tar.gz"
PYPROJ_SRC = ROOT / "target" / "pyproj-src"
PYPROJ_TAG = "3.8.0"
CRS_CARGO = ROOT / "crates/xeibe-crs/Cargo.toml"
COMMITTED = ROOT / "tests/data/proj-string"
ALL = ROOT / "target/proj-string"

PINNED = [
    4326, 4258, 4269, 4267, 4277, 4314, 4312, 4121, 4223, 4299, 4272,  # PROJ's +datum names
    4978, 4979,          # geocentric, geographic 3D
    2180, 2176, 3035, 3857, 27700, 2056, 32633, 32733, 25832, 28992, 2154,
    5514, 2065,          # Krovak East North, Krovak
    3031, 3413, 5041,    # polar stereographic
    2263, 2232,          # US survey feet
    27572, 4807,         # NTF (Paris): grads, Paris meridian
    3395, 3034, 3577, 6933, 3410,  # Mercator, LCC, Albers, EASE
    9651, 5555,          # compound
    5714,                # vertical
]  # fmt: skip
PER_METHOD = 3
SEED = 20261031

# Strings not in PROJ's or pyproj's tests: GeoPandas' tests, and what the
# design's rules are about.
EXTRA = [
    # GeoPandas 1.2.0 tests
    "+proj=longlat +ellps=WGS84 +datum=WGS84 +no_defs",
    "+proj=utm +zone=18 +ellps=GRS80 +datum=NAD83 +units=m +no_defs +nadgrids=null",
    "+proj=utm +zone=30",
    "+proj=cea +lon_0=0 +lat_ts=45 +x_0=0 +y_0=0 +ellps=WGS84 +units=m",
    "+init=epsg:4326",
    "+init=epsg:26918 +no_defs",
    # +init with other parameters: PROJ re-reads the CRS's string with them
    "+init=epsg:4326 +towgs84=1,2,3",
    "+init=epsg:26918 +units=ft",
    "+init=epsg:2180 +ellps=bessel",
    # Unknown datums
    "+proj=longlat +ellps=GRS80",
    "+proj=utm +zone=33 +ellps=GRS80",
    "+proj=longlat +ellps=bessel",
    "+proj=longlat +a=6378137 +rf=298.257223563",
    "+proj=longlat +R=6371000",
    # +datum names
    "+proj=longlat +datum=NAD27",
    "+proj=longlat +datum=potsdam",
    "+proj=longlat +datum=hermannskogel",
    "+proj=longlat +datum=OSGB36",
    "+proj=longlat +datum=GGRS87",
    "+proj=longlat +datum=carthage",
    "+proj=longlat +datum=ire65",
    "+proj=longlat +datum=nzgd49",
    # Bound CRSs
    "+proj=tmerc +lat_0=0 +lon_0=19 +k=0.9993 +x_0=500000 +y_0=-5300000 +ellps=GRS80 "
    "+towgs84=0,0,0,0,0,0,0 +units=m +no_defs",
    "+proj=somerc +lat_0=46.9524055555556 +lon_0=7.43958333333333 +k_0=1 +x_0=2600000 "
    "+y_0=1200000 +ellps=bessel +towgs84=674.374,15.056,405.346,0,0,0,0 +units=m +no_defs",
    "+proj=longlat +ellps=bessel +towgs84=598.1,73.7,418.2,0.202,0.045,-2.455,6.7",
    "+proj=sinu +R=6371007.181 +nadgrids=@null +wktext",
    "+proj=merc +a=6378137 +b=6378137 +lat_ts=0 +lon_0=0 +x_0=0 +y_0=0 +k=1 +units=m "
    "+nadgrids=@null +wktext +no_defs",
    "+proj=longlat +ellps=clrk66 +nadgrids=@conus,@alaska,@ntv2_0.gsb,@ntv1_can.dat",
    # Units and axes
    "+proj=tmerc +lat_0=0 +lon_0=-81 +k=0.9996 +x_0=500000.00000001 +y_0=0 +datum=NAD83 "
    "+units=us-ft +no_defs",
    "+proj=lcc +lat_1=33 +lat_2=45 +lat_0=39 +lon_0=-96 +x_0=0 +y_0=0 +datum=NAD83 "
    "+to_meter=0.3048006096012192",
    "+proj=longlat +datum=WGS84 +axis=neu",
    "+proj=tmerc +lat_0=0 +lon_0=21 +k=0.9996 +x_0=500000 +datum=WGS84 +axis=wsu",
    "+proj=longlat +datum=WGS84 +pm=paris",
    "+proj=longlat +ellps=clrk80ign +pm=paris",
    "+proj=longlat +datum=WGS84 +pm=2.337229167",
    # Options PROJ keeps only as a PROJ-based operation
    "+proj=longlat +datum=WGS84 +over",
    "+proj=longlat +datum=WGS84 +lon_wrap=180",
    "+proj=merc +datum=WGS84 +over",
    "+proj=tmerc +lon_0=21 +datum=WGS84 +foo=bar",
    # PROJ-only projections (decision 6)
    "+proj=robin",
    "+proj=moll +lon_0=0 +datum=WGS84",
    "+proj=wintri",
    "+proj=natearth",
    "+proj=eck4",
    # DMS angles
    "+proj=tmerc +lat_0=49d30'N +lon_0=2d20'14.025\"E +k=0.9996 +datum=WGS84",
]


def error_text(e: Exception) -> str:
    return str(e).splitlines()[0] if str(e) else type(e).__name__


def to_proj4(crs: pyproj.CRS, version: ProjVersion = ProjVersion.PROJ_5) -> dict:
    try:
        return {"proj4": crs.to_proj4(version)}
    except pyproj.exceptions.CRSError as e:
        return {"proj4": None, "error": error_text(e)}


def reading(srs: str) -> dict:
    """PROJ's CRS from a prepared string: its PROJJSON, to_epsg and string."""
    try:
        crs = pyproj.CRS(srs)
    except pyproj.exceptions.CRSError as e:
        return {"projjson": None, "error": error_text(e)}
    out = {"projjson": crs.to_json_dict()}
    try:
        out["to_epsg"] = crs.to_epsg()
    except pyproj.exceptions.CRSError:
        out["to_epsg"] = None
    out["to_proj4"] = to_proj4(crs).get("proj4")
    return out


def epsg_vector(code: int) -> dict | None:
    try:
        crs = pyproj.CRS.from_epsg(code)
    except pyproj.exceptions.CRSError:
        return None
    record: dict = {"code": code, "type": crs.type_name}
    written = to_proj4(crs)
    record.update(written)
    record["proj4_v4"] = to_proj4(crs, ProjVersion.PROJ_4).get("proj4")
    if written["proj4"] is not None:
        record["reading"] = reading(written["proj4"])
    return record


def prepare(s: str) -> str:
    """The string as pyproj's CRS hands it to PROJ (`_prepare_from_string`)."""
    return pyproj.crs.crs._prepare_from_string(s)


def angle(s: str, key: str) -> float:
    m = re.search(rf"\+{key}=(-?[0-9.]+)(?:\s|$)", s)
    return float(m.group(1)) if m else 0.0


def points(srs: str) -> list[list[float]]:
    lon0 = angle(srs, "lon_0") or angle(srs, "lonc")
    lat0 = angle(srs, "lat_0")
    out = []
    for dx, dy in [(0, 0), (1, 1), (-1, 0.5), (2, -1), (-2, -2)]:
        lat = max(-85.0, min(85.0, lat0 + dy))
        out.append([lon0 + dx, lat])
    return out


def converted(crs: pyproj.CRS, pts: list[list[float]]) -> list[list[float] | None] | None:
    base = crs.source_crs if crs.is_bound else crs
    if base is None or base.geodetic_crs is None or not (base.is_projected or base.is_geographic):
        return None
    try:
        t = pyproj.Transformer.from_crs(base.geodetic_crs, base, always_xy=True)
    except pyproj.exceptions.ProjError:
        return None
    out: list[list[float] | None] = []
    for lon, lat in pts:
        try:
            x, y = t.transform(lon, lat, errcheck=True)
        except pyproj.exceptions.ProjError:
            out.append(None)
            continue
        out.append([x, y] if math.isfinite(x) and math.isfinite(y) else None)
    return out


def corpus_vector(item: tuple[str, str]) -> dict:
    source, string = item
    record: dict = {"source": source, "string": string}
    try:
        with warnings.catch_warnings(record=True) as caught:
            warnings.simplefilter("always")
            srs = prepare(string)
    except pyproj.exceptions.CRSError as e:
        record.update(srs=None, projjson=None, error=error_text(e))
        return record
    record["srs"] = srs
    record["warnings"] = [f"{w.category.__name__}: {w.message}" for w in caught]
    record.update(reading(srs))
    if record["projjson"] is not None:
        pts = points(srs)
        out = converted(pyproj.CRS(srs), pts)
        if out is not None:
            record["points"] = pts
            record["converted"] = out
    return record


# Adjacent C++ string literals, which the compiler concatenates.
LITERAL = r'"((?:[^"\\\n]|\\.)*)"'


def test_io_strings() -> list[str]:
    if not TARBALL.exists():
        sys.exit(f"{TARBALL.relative_to(ROOT)} is missing; run scripts/build_proj_oracle.py")
    with tarfile.open(TARBALL) as tar:
        member = tar.extractfile("proj-9.8.1/test/unit/test_io.cpp")
        assert member is not None
        src = member.read().decode()
    out = []
    for m in re.finditer(rf"(?:{LITERAL}\s*)+", src):
        s = "".join(re.findall(LITERAL, m.group(0))).replace('\\"', '"').replace("\\t", "\t").strip()
        # PROJ strings, not WKT or PROJJSON that mention one.
        if re.search(r"(^|\s)\+?(proj|init)=", s) and not re.search(r"[\[{]", s):
            out.append(s)
    return list(dict.fromkeys(out))


def pyproj_strings() -> list[str]:
    marker = PYPROJ_SRC / ".xeibe-pyproj-tag"
    if not (marker.is_file() and marker.read_text().strip() == PYPROJ_TAG):
        subprocess.run(
            ["git", "clone", "--quiet", "--depth", "1", "--branch", PYPROJ_TAG,
             "https://github.com/pyproj4/pyproj", str(PYPROJ_SRC)],
            check=True,
        )  # fmt: skip
        marker.write_text(PYPROJ_TAG + "\n")
    out = []
    for path in sorted((PYPROJ_SRC / "test").rglob("*.py")):
        for m in re.finditer(r"""(["'])((?:(?!\1).)*\+proj=(?:(?!\1).)*)\1""", path.read_text()):
            s = m.group(2).strip()
            if not re.search(r"[\[{%]", s):
                out.append(s)
    return list(dict.fromkeys(out))


def init_worker() -> None:
    pyproj.datadir.set_data_dir(str(ORACLE))
    pyproj.network.set_network_enabled(False)
    warnings.simplefilter("ignore")


def epsg_versions() -> tuple[str, str]:
    m = re.search(r'^version = "[^"+]*\+epsg-([0-9.]+)"', CRS_CARGO.read_text(), re.M)
    if m is None:
        sys.exit(f"no EPSG version in {CRS_CARGO.relative_to(ROOT)}")
    db = sqlite3.connect(f"file:{ORACLE / 'proj.db'}?mode=ro", uri=True)
    row = db.execute("SELECT value FROM metadata WHERE key = 'EPSG.VERSION'").fetchone()
    db.close()
    return m.group(1), (row[0].removeprefix("v") if row else "none")


def select(records: list[dict]) -> list[dict]:
    """`PINNED`, plus `PER_METHOD` CRSs per conversion method and per kind of
    PROJ error, so every special case of the writer is in."""
    chosen = {r["code"] for r in records if r["code"] in PINNED}
    groups: dict[str, list[int]] = {}
    for r in records:
        if r["proj4"] is None:
            key = "error:" + r["error"]
        else:
            m = re.match(r"\+proj=(\S+)", r["proj4"])
            key = (m.group(1) if m else "?") + ("+towgs84" if "+towgs84" in r["proj4"] else "")
        groups.setdefault(key, []).append(r["code"])
    rng = random.Random(SEED)
    for _, codes in sorted(groups.items()):
        chosen.update(rng.sample(codes, min(PER_METHOD, len(codes))))
    return [r for r in records if r["code"] in chosen]


def write(path: Path, records: list[dict], notice: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w") as f:
        header = {
            "generator": "scripts/gen_proj_string_vectors.py",
            "pyproj": pyproj.__version__,
            "proj": pyproj.proj_version_str,
            "epsg": pyproj.database.get_database_metadata("EPSG.VERSION"),
            "notice": notice,
        }
        f.write(json.dumps(header) + "\n")
        for r in records:
            f.write(json.dumps(r, separators=(",", ":"), ensure_ascii=False) + "\n")
    print(f"{len(records)} records, {path.stat().st_size / 1024:.0f} KiB -> {path.relative_to(ROOT)}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--all", action="store_true", help="every EPSG CRS, into target/")
    parser.add_argument("--jobs", type=int, default=os.cpu_count(), help="worker processes (default: all CPUs)")
    args = parser.parse_args()
    if not (ORACLE / "proj.db").exists():
        sys.exit("target/proj-oracle/proj.db is missing; run scripts/build_proj_oracle.py")
    want, have = epsg_versions()
    if want != have:
        sys.exit(f"the oracle has EPSG {have}, xeibe-crs {want}: rerun scripts/build_proj_oracle.py")
    init_worker()
    started = time.monotonic()
    codes = sorted({int(i.code) for i in pyproj.database.query_crs_info(auth_name="EPSG", allow_deprecated=True)})
    with ProcessPoolExecutor(max_workers=args.jobs, initializer=init_worker) as pool:
        epsg = [r for r in pool.map(epsg_vector, codes, chunksize=16) if r is not None]
    out = ALL if args.all else COMMITTED
    write(
        out / "epsg.jsonl",
        epsg if args.all else select(epsg),
        "EPSG Dataset (IOGP) rendered by PROJ; see crates/xeibe-crs/EPSG-NOTICE.md",
    )
    items = [("test_io.cpp", s) for s in test_io_strings()]
    items += [("pyproj", s) for s in pyproj_strings()]
    items += [("extra", s) for s in EXTRA]
    seen: set[str] = set()
    items = [i for i in items if not (i[1] in seen or seen.add(i[1]))]
    with ProcessPoolExecutor(max_workers=args.jobs, initializer=init_worker) as pool:
        corpus = list(pool.map(corpus_vector, items, chunksize=4))
    write(
        out / "corpus.jsonl",
        corpus,
        "PROJ strings from PROJ's and pyproj's tests (MIT) and GeoPandas' (BSD-3-Clause), read by PROJ",
    )
    written = sum(r["proj4"] is not None for r in epsg)
    read = sum(r.get("reading", {}).get("projjson") is not None for r in epsg)
    print(f"EPSG   : {len(epsg)} CRSs, {written} written by PROJ, {read} of those read back")
    ok = sum(r["projjson"] is not None for r in corpus)
    print(f"corpus : {len(corpus)} strings, {ok} read by PROJ as a CRS")
    print(f"{time.monotonic() - started:.0f} s with {args.jobs} processes")
    return 0


if __name__ == "__main__":
    sys.exit(main())
