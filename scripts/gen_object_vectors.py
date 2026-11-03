#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["pyproj==3.8.0"]
# ///
"""Generate test vectors for EPSG's objects on their own (`xeibe-crs`'s
`objects.bin`, `docs/pyproj-phase3.md`, "The objects table"), with PROJ as
the oracle. PROJ (pyproj 3.8.0 = PROJ 9.8.1) runs on the `proj.db` rebuilt
from the same EPSG release as `xeibe-crs` (`scripts/build_proj_oracle.py`,
`target/proj-oracle`). Three files:

* `objects.jsonl`: every ellipsoid, prime meridian, datum (and ensemble)
  and conversion of EPSG's in PROJ's database, deprecated ones included:
  pyproj's `from_epsg`, with PROJ's PROJJSON and its WKT2 2019 and WKT1_GDAL, `null` where PROJ
  writes none.
* `names.jsonl`: each of those objects' names and the aliases PROJ's name
  search uses (not EPSG's old names), with where the name comes from
  (`name`, or the alias's source: `EPSG`, `PROJ`, `ESRI`, …) and pyproj's
  `from_name` for the
  object's class: the authority and code found, or the name and PROJJSON of
  what pyproj made without one (an ellipsoid from PROJ's `+ellps` table),
  or `null`. Also each name with another case, and deprecated objects'
  names with " (deprecated)".
* `query.jsonl`: `query_geodetic_crs_from_datum` for every geodetic datum
  and ensemble, with each `pj_type` and none, and with the CRS authority
  `EPSG` and none: the CRSs' authorities and codes in PROJ's order.

The objects are IOGP's EPSG data as PROJ renders it, under the notice in
`crates/xeibe-crs/EPSG-NOTICE.md`.

    scripts/build_proj_oracle.py                 # once per EPSG release
    scripts/gen_object_vectors.py                # committed subset -> tests/data/crs-objects/
    scripts/gen_object_vectors.py --all          # everything -> target/crs-objects/
"""

from __future__ import annotations

import argparse
import json
import re
import sqlite3
import sys
import warnings
from pathlib import Path

import pyproj
from pyproj.crs import CoordinateOperation, Datum, Ellipsoid, PrimeMeridian
from pyproj.database import query_geodetic_crs_from_datum
from pyproj.enums import PJType
from pyproj.exceptions import CRSError

ROOT = Path(__file__).resolve().parent.parent
ORACLE = ROOT / "target" / "proj-oracle"
CRS_CARGO = ROOT / "crates/xeibe-crs/Cargo.toml"
COMMITTED = ROOT / "tests/data/crs-objects"
ALL = ROOT / "target/crs-objects"
# PROJ's tables of each kind, and how pyproj makes one by its EPSG code.
KINDS = {
    "ellipsoid": (["ellipsoid"], Ellipsoid),
    "prime_meridian": (["prime_meridian"], PrimeMeridian),
    "datum": (["geodetic_datum", "vertical_datum", "engineering_datum"], Datum),
    "conversion": (["conversion"], CoordinateOperation),
}
# In the committed subset: every ellipsoid and prime meridian, and every
# this many datums and conversions.
EVERY = {"datum": 8, "conversion": 40}
CRS_TYPES = [None, PJType.GEOGRAPHIC_2D_CRS, PJType.GEOGRAPHIC_3D_CRS, PJType.GEOCENTRIC_CRS]


def epsg_versions() -> tuple[str, str]:
    """The EPSG release of `xeibe-crs` and of the oracle, both as `13.106`."""
    m = re.search(r'^version = "[^"+]*\+epsg-([0-9.]+)"', CRS_CARGO.read_text(), re.M)
    if m is None:
        sys.exit(f"no EPSG version in {CRS_CARGO.relative_to(ROOT)}")
    db = sqlite3.connect(f"file:{ORACLE / 'proj.db'}?mode=ro", uri=True)
    row = db.execute("SELECT value FROM metadata WHERE key = 'EPSG.VERSION'").fetchone()
    db.close()
    return m.group(1), (row[0].removeprefix("v") if row else "none")


def oracle() -> sqlite3.Connection:
    return sqlite3.connect(f"file:{ORACLE / 'proj.db'}?mode=ro", uri=True)


def codes(db: sqlite3.Connection, kind: str) -> list[int]:
    tables, _ = KINDS[kind]
    out: set[int] = set()
    for table in tables:
        out |= {int(c) for (c,) in db.execute(f"SELECT code FROM {table} WHERE auth_name = 'EPSG'")}
    return sorted(out)


def make(kind: str, code: int):
    _, cls = KINDS[kind]
    return cls.from_epsg(code)


def wkt(obj, version: str) -> str | None:
    try:
        return obj.to_wkt(version)
    except Exception:  # noqa: BLE001 - PROJ writes nothing for it
        return None


def object_vector(kind: str, code: int) -> dict:
    try:
        obj = make(kind, code)
    except CRSError as e:
        return {"kind": kind, "code": code, "projjson": None, "error": str(e)}
    return {
        "kind": kind,
        "code": code,
        "projjson": obj.to_json_dict(),
        "wkt": {v: wkt(obj, v) for v in ("WKT2_2019", "WKT1_GDAL")},
    }


def names(db: sqlite3.Connection, kind: str, chosen: set[int]) -> list[tuple[str, int, bool, str]]:
    """`(name, code, deprecated, source)` of each name and alias of the
    chosen objects: the source is `name` for the object's own, else the
    alias's (`EPSG`, `PROJ`, `ESRI`, …)."""
    tables, _ = KINDS[kind]
    out = []
    for table in tables:
        for name, code, deprecated in db.execute(f"SELECT name, code, deprecated FROM {table} WHERE auth_name = 'EPSG'"):
            if int(code) in chosen:
                out.append((name, int(code), bool(deprecated), "name"))
        for name, code, deprecated, source in db.execute(
            f"""SELECT a.alt_name, o.code, o.deprecated, a.source FROM alias_name a
                  JOIN {table} o ON o.auth_name = a.auth_name AND o.code = a.code
                 WHERE a.table_name = ? AND a.auth_name = 'EPSG' AND a.source != 'EPSG_OLD'""",
            (table,),
        ):
            if int(code) in chosen:
                out.append((name, int(code), bool(deprecated), source))
    return sorted(set(out))


def from_name(kind: str, name: str):
    _, cls = KINDS[kind]
    try:
        obj = cls.from_name(name)
    except CRSError:
        return None
    doc = obj.to_json_dict()
    ident = doc.get("id")
    if ident:
        code = ident["code"]
        return [ident["authority"], int(code) if str(code).isdigit() else code]
    return {"name": doc.get("name"), "projjson": doc}


def name_vectors(db: sqlite3.Connection, kind: str, chosen: set[int]) -> list[dict]:
    out = []
    for name, code, deprecated, source in names(db, kind, chosen):
        asked = [name, name.swapcase()]
        if deprecated:
            asked.append(f"{name} (deprecated)")
        for text in asked:
            out.append({"kind": kind, "name": text, "of": code, "source": source, "found": from_name(kind, text)})
    return out


def query_vectors(datum_codes: list[int]) -> list[dict]:
    out = []
    for code in datum_codes:
        for auth in ("EPSG", None):
            for pj_type in CRS_TYPES:
                crss = query_geodetic_crs_from_datum(auth, "EPSG", str(code), pj_type)
                found = [list(c.to_authority(min_confidence=100) or ("", "")) for c in crss]
                out.append(
                    {
                        "datum": code,
                        "crs_auth_name": auth,
                        "pj_type": None if pj_type is None else pj_type.name,
                        "crs": [[a, int(c) if c.isdigit() else c] for a, c in found],
                    }
                )
    return out


def write(path: Path, records: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w") as f:
        header = {
            "generator": "scripts/gen_object_vectors.py",
            "pyproj": pyproj.__version__,
            "proj": pyproj.proj_version_str,
            "epsg": pyproj.database.get_database_metadata("EPSG.VERSION"),
            "notice": "EPSG Dataset (IOGP) rendered by PROJ; see crates/xeibe-crs/EPSG-NOTICE.md",
        }
        f.write(json.dumps(header) + "\n")
        for r in records:
            f.write(json.dumps(r, separators=(",", ":"), ensure_ascii=False) + "\n")
    print(f"{len(records)} records, {path.stat().st_size / 1024:.0f} KiB -> {path.relative_to(ROOT)}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--all", action="store_true", help="every object, into target/")
    args = parser.parse_args()
    if not (ORACLE / "proj.db").exists():
        sys.exit("target/proj-oracle/proj.db is missing; run scripts/build_proj_oracle.py")
    want, have = epsg_versions()
    if want != have:
        sys.exit(f"the oracle has EPSG {have}, xeibe-crs {want}: rerun scripts/build_proj_oracle.py")
    pyproj.datadir.set_data_dir(str(ORACLE))
    pyproj.network.set_network_enabled(False)
    warnings.simplefilter("ignore")
    db = oracle()
    objects, name_records, datums = [], [], []
    for kind in KINDS:
        every = 1 if args.all else EVERY.get(kind, 1)
        chosen = codes(db, kind)[::every]
        objects += [object_vector(kind, c) for c in chosen]
        name_records += name_vectors(db, kind, set(chosen))
        if kind == "datum":
            geodetic = {int(c) for (c,) in db.execute("SELECT code FROM geodetic_datum WHERE auth_name = 'EPSG'")}
            datums = [c for c in chosen if c in geodetic]
    out = ALL if args.all else COMMITTED
    write(out / "objects.jsonl", objects)
    write(out / "names.jsonl", name_records)
    write(out / "query.jsonl", query_vectors(datums))
    return 0


if __name__ == "__main__":
    sys.exit(main())
