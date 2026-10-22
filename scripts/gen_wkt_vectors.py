#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["pyproj==3.8.0"]
# ///
"""Generate WKT test vectors for `xeibe-reproject`'s WKT reader, writer and
identification (`docs/crs-wkt.md`, "Test data"), with PROJ as the oracle.

PROJ (pyproj 3.8.0 = PROJ 9.8.1) runs on the full `proj.db` rebuilt from the
same EPSG release as `xeibe-crs` (`scripts/build_proj_oracle.py`,
`target/proj-oracle`), so both sides see the same EPSG data. For every EPSG
CRS PROJ has, deprecated ones included, two files:

* `wkt.jsonl`: PROJ's `to_wkt()` in `WKT2_2019`, `WKT2_2015`, `WKT1_GDAL` and
  `WKT1_ESRI` (single line), `null` with the error in `errors` where PROJ
  can't write a version; and PROJ's reading of each string back: whether it
  `equals` the original, also with `ignore_axis_order`, and its axis
  directions (what the WKT1 axis rules are measured on).
* `epsg.jsonl`: EPSG's own WKT2 2019 for each CRS, from the "WKT File"
  archive in `example_data/` that `gen_crs_tables.py` also reads (current
  CRSs only; skipped when the archive is missing). The reader's first test
  set.
* `identify.jsonl`: one record per CRS and version, the string with its ids
  removed (`ID[...]` in WKT2, `AUTHORITY[...]` in WKT1) and, for it and for
  the string as written (ids kept), PROJ's `list_authority` (EPSG only,
  `[code, confidence]`, best first, at most 5) and `to_epsg()`.

The WKT is IOGP's EPSG data as PROJ renders it, under the notice in
`crates/xeibe-crs/EPSG-NOTICE.md`.

    scripts/build_proj_oracle.py                 # once per EPSG release
    scripts/gen_wkt_vectors.py                   # committed subset -> tests/data/crs-wkt/
    scripts/gen_wkt_vectors.py --all             # every CRS -> target/crs-wkt/
"""

from __future__ import annotations

import argparse
import json
import zipfile
import os
import random
import re
import sqlite3
import sys
import time
import warnings
from collections import Counter
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

import pyproj

ROOT = Path(__file__).resolve().parent.parent
ORACLE = ROOT / "target" / "proj-oracle"
CRS_CARGO = ROOT / "crates/xeibe-crs/Cargo.toml"
COMMITTED = ROOT / "tests/data/crs-wkt"
ALL = ROOT / "target/crs-wkt"

VERSIONS = ("WKT2_2019", "WKT2_2015", "WKT1_GDAL", "WKT1_ESRI")
ID_KEYWORDS = {"WKT2_2019": {"ID"}, "WKT2_2015": {"ID"}, "WKT1_GDAL": {"AUTHORITY"}, "WKT1_ESRI": {"AUTHORITY"}}
MAX_MATCHES = 5

# Codes always in the committed subset: what the design's rules are about.
# 32596 is a derived projected CRS, which PROJ's database doesn't have.
PINNED = [
    4326, 4258, 2180, 3035, 3857, 27700, 2056, 32633, 25832, 2176, 28992, 2154,
    4978,                # geocentric
    4979, 7912,          # geographic 3D (ITRF2014: dynamic)
    5514,                # Krovak East North
    9651, 7415, 5555,    # compound
    5714, 5715, 10364,   # vertical: height, depths
    11378,               # engineering
    32596,               # derived projected
    4807, 27572,         # NTF (Paris): Paris meridian, grads
    3031, 5041,          # polar: axes along meridians
    2263,                # US survey feet
    3785,                # deprecated (Popular Visualisation CRS / Mercator)
]  # fmt: skip
# The rest of the subset: per CRS type, this many live and deprecated ones.
PER_TYPE_LIVE = 8
PER_TYPE_DEPRECATED = 2
SEED = 20261022
# EPSG's archive in the committed subset: the chosen codes, plus the first
# few CRSs that use each construct the reader must handle.
EPSG_FEATURES = [
    r"DEFININGTRANSFORMATION", r"ANCHOREPOCH", r"GEOIDMODEL", r"DYNAMIC\[",
    r"BASEVERTCRS", r"MERIDIAN\[", r"^ENGCRS", r"^DERIVEDPROJCRS", r"\(E\(X\)\)",
    r'"EPSG ",', r'PRIMEM\["Paris"', r"ANGLEUNIT\[\"grad\"", r"^COMPOUNDCRS",
]  # fmt: skip
PER_FEATURE = 2


def find_close(s: str, i: int) -> int:
    """The index just past the bracket closing the one at `s[i]` (`[` or
    `(`; WKT1 allows either), skipping quoted strings."""
    depth = 0
    n = len(s)
    while i < n:
        c = s[i]
        if c == '"':
            i = skip_string(s, i)
            continue
        if c in "[(":
            depth += 1
        elif c in "])":
            depth -= 1
            if depth == 0:
                return i + 1
        i += 1
    raise ValueError("unbalanced brackets")


def skip_string(s: str, i: int) -> int:
    """The index just past the quoted string starting at `s[i]`; WKT escapes
    a quote by doubling it."""
    i += 1
    while True:
        j = s.index('"', i)
        if j + 1 < len(s) and s[j + 1] == '"':
            i = j + 2
            continue
        return j + 1


def strip_ids(wkt: str, keywords: set[str]) -> str:
    """`wkt` without the nodes named by `keywords`, anywhere in the tree, each
    with the comma that separated it from its neighbour."""
    spans = []
    i, n = 0, len(wkt)
    while i < n:
        c = wkt[i]
        if c == '"':
            i = skip_string(wkt, i)
        elif c.isalpha() or c == "_":
            j = i
            while j < n and (wkt[j].isalnum() or wkt[j] == "_"):
                j += 1
            k = j
            while k < n and wkt[k].isspace():
                k += 1
            if k < n and wkt[k] in "[(" and wkt[i:j].upper() in keywords:
                end = find_close(wkt, k)
                spans.append(widen(wkt, i, end))
                i = end
            else:
                i = j
        else:
            i += 1
    out, last = [], 0
    for start, end in spans:
        start = max(start, last)
        out.append(wkt[last:start])
        last = max(last, end)
    out.append(wkt[last:])
    return "".join(out)


def widen(wkt: str, start: int, end: int) -> tuple[int, int]:
    """A node's span plus its separating comma: the one before it, or for a
    first child, the one after it."""
    b = start
    while b > 0 and wkt[b - 1].isspace():
        b -= 1
    if b > 0 and wkt[b - 1] == ",":
        return b - 1, end
    e = end
    while e < len(wkt) and wkt[e].isspace():
        e += 1
    if e < len(wkt) and wkt[e] == ",":
        e += 1
        while e < len(wkt) and wkt[e].isspace():
            e += 1
        return start, e
    return start, end


def error_text(e: Exception) -> str:
    return " ".join(str(e).split())


def identify(crs: pyproj.CRS) -> dict:
    """PROJ's EPSG matches for a CRS, best first, and its `to_epsg()`."""
    out: dict = {}
    try:
        matches = crs.list_authority(auth_name="EPSG", min_confidence=0)
        out["authority"] = [[int(m.code), m.confidence] for m in matches[:MAX_MATCHES]]
    except pyproj.exceptions.CRSError as e:
        out["authority_error"] = error_text(e)
    try:
        out["to_epsg"] = crs.to_epsg()
    except pyproj.exceptions.CRSError as e:
        out["to_epsg_error"] = error_text(e)
    return out


def directions(crs: pyproj.CRS) -> list[str]:
    return [a.direction for a in crs.axis_info]


def vectors(code: int, deprecated: bool) -> tuple[dict, list[dict]] | None:
    """The `wkt.jsonl` record of a CRS and its `identify.jsonl` records."""
    try:
        crs = pyproj.CRS.from_epsg(code)
    except pyproj.exceptions.CRSError:
        return None
    record: dict = {
        "code": code,
        "name": crs.name,
        "kind": crs.type_name,
        "deprecated": deprecated,
        "axes": directions(crs),
        "wkt": {},
    }
    errors, read, ids = {}, {}, []
    for version in VERSIONS:
        try:
            wkt = crs.to_wkt(version)
            if wkt is None:
                errors[version] = "PROJ wrote nothing"
        except pyproj.exceptions.CRSError as e:
            wkt = None
            errors[version] = error_text(e)
        record["wkt"][version] = wkt
        if wkt is None:
            continue
        try:
            back = pyproj.CRS.from_wkt(wkt)
        except pyproj.exceptions.CRSError as e:
            read[version] = {"error": error_text(e)}
            back = None
        if back is not None:
            read[version] = {
                "equals": back.equals(crs),
                "equals_ignore_axis_order": back.equals(crs, ignore_axis_order=True),
                "axes": directions(back),
            }
        stripped = strip_ids(wkt, ID_KEYWORDS[version])
        entry: dict = {"code": code, "version": version, "wkt": stripped}
        try:
            entry["stripped"] = identify(pyproj.CRS.from_wkt(stripped))
        except pyproj.exceptions.CRSError as e:
            entry["stripped"] = {"error": error_text(e)}
        if stripped == wkt:
            # No ids to remove (ESRI's): the same input.
            entry["kept"] = entry["stripped"]
        elif back is None:
            entry["kept"] = read[version]
        else:
            entry["kept"] = identify(back)
        ids.append(entry)
    if errors:
        record["errors"] = errors
    record["read"] = read
    return record, ids


def init_worker() -> None:
    pyproj.datadir.set_data_dir(str(ORACLE))
    pyproj.network.set_network_enabled(False)
    warnings.simplefilter("ignore")


def run(item: tuple[int, bool]):
    return vectors(*item)


def epsg_versions() -> tuple[str, str]:
    """The EPSG release of `xeibe-crs` (from its version's build metadata)
    and of the oracle database, both as `13.106`."""
    m = re.search(r'^version = "[^"+]*\+epsg-([0-9.]+)"', CRS_CARGO.read_text(), re.M)
    if m is None:
        sys.exit(f"no EPSG version in {CRS_CARGO.relative_to(ROOT)}")
    db = sqlite3.connect(f"file:{ORACLE / 'proj.db'}?mode=ro", uri=True)
    row = db.execute("SELECT value FROM metadata WHERE key = 'EPSG.VERSION'").fetchone()
    db.close()
    return m.group(1), (row[0].removeprefix("v") if row else "none")


def select(crs_info: dict[int, pyproj.database.CRSInfo], everything: bool) -> list[int]:
    if everything:
        return sorted(crs_info)
    chosen = []
    for code in PINNED:
        if code in crs_info:
            chosen.append(code)
        else:
            print(f"EPSG:{code}: not in PROJ's database, skipped", file=sys.stderr)
    rng = random.Random(SEED)
    groups: dict[tuple[str, bool], list[int]] = {}
    for code in sorted(crs_info):
        groups.setdefault((crs_info[code].type.name, crs_info[code].deprecated), []).append(code)
    for (_, deprecated), codes in sorted(groups.items()):
        k = PER_TYPE_DEPRECATED if deprecated else PER_TYPE_LIVE
        chosen += [c for c in rng.sample(codes, min(k, len(codes))) if c not in chosen]
    return sorted(set(chosen))


def epsg_wkt(version: str, chosen: list[int] | None) -> list[dict] | None:
    """EPSG's own WKT for its CRSs, from the archive of the same release:
    all of them, or `chosen` plus a few per construct in `EPSG_FEATURES`."""
    archive = ROOT / "example_data" / f"EPSG-v{version.replace('.', '_')}-WKT.Zip"
    if not archive.exists():
        print(f"{archive.relative_to(ROOT)} is missing: no epsg.jsonl", file=sys.stderr)
        return None
    texts = {}
    with zipfile.ZipFile(archive) as z:
        for name in z.namelist():
            m = re.fullmatch(r"EPSG-CRS-(\d+)\.wkt", Path(name).name)
            text = z.read(name).decode("utf-8").strip() if m else ""
            # Four files are prose: "WKT is not supported for vertical CRS
            # derived in multiple steps."
            if m and re.match(r"[A-Z]+\[", text):
                texts[int(m.group(1))] = text
    codes = set(texts) if chosen is None else {c for c in chosen if c in texts}
    if chosen is not None:
        for pattern in EPSG_FEATURES:
            codes |= set([c for c in sorted(texts) if re.search(pattern, texts[c])][:PER_FEATURE])
    return [{"code": c, "wkt": texts[c]} for c in sorted(codes)]


def write(path: Path, records: list[dict], notice: str = "rendered by PROJ") -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w") as f:
        header = {
            "generator": "scripts/gen_wkt_vectors.py",
            "pyproj": pyproj.__version__,
            "proj": pyproj.proj_version_str,
            "epsg": pyproj.database.get_database_metadata("EPSG.VERSION"),
            "notice": f"EPSG Dataset (IOGP) {notice}; see crates/xeibe-crs/EPSG-NOTICE.md",
        }
        f.write(json.dumps(header) + "\n")
        for r in records:
            f.write(json.dumps(r, separators=(",", ":"), ensure_ascii=False) + "\n")
    print(f"{len(records)} records, {path.stat().st_size / 1024:.0f} KiB -> {path.relative_to(ROOT)}")


def summary(records: list[dict], ids: list[dict]) -> None:
    print(f"{len(records)} CRSs ({sum(r['deprecated'] for r in records)} deprecated)")
    for v in VERSIONS:
        made = [r for r in records if r["wkt"][v] is not None]
        read = [r["read"][v] for r in made if "error" not in r["read"][v]]
        lost = sum(1 for r in made if "axes" in r["read"][v] and r["read"][v]["axes"] != r["axes"])
        print(
            f"{v:9}: {len(made)} written, {len(records) - len(made)} not; read back {len(read)}: "
            f"{sum(x['equals'] for x in read)} equal, "
            f"{sum(x['equals_ignore_axis_order'] for x in read)} ignoring axis order, "
            f"{lost} with other axes"
        )
        errors = Counter(r["errors"][v] for r in records if v in r.get("errors", {}))
        for text, n in errors.most_common(3):
            print(f"    {n} x {text[:150]}")
    for v in VERSIONS:
        mine = [e for e in ids if e["version"] == v]
        parts = []
        for side in ("stripped", "kept"):
            best = Counter()
            for e in mine:
                m = e[side].get("authority") or []
                if m and m[0][0] == e["code"]:
                    best[m[0][1]] += 1
                elif m:
                    best["other"] += 1
                else:
                    best["none"] += 1
            to_epsg = sum(e[side].get("to_epsg") == e["code"] for e in mine)
            levels = ", ".join(f"{k}: {best[k]}" for k in (100, 90, 70, 25, "other", "none") if best[k])
            parts.append(f"{side}: own code first at {{{levels}}}, to_epsg right {to_epsg}")
        print(f"{v:9}: {len(mine)} strings; " + "; ".join(parts))


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
    # PROJ lists a CRS once per area of use: one entry per code.
    crs_info = {int(i.code): i for i in pyproj.database.query_crs_info(auth_name="EPSG", allow_deprecated=True)}
    codes = select(crs_info, args.all)
    started = time.monotonic()
    with ProcessPoolExecutor(max_workers=args.jobs, initializer=init_worker) as pool:
        results = list(pool.map(run, [(c, crs_info[c].deprecated) for c in codes], chunksize=8))
    records, ids = [], []
    for code, result in zip(codes, results):
        if result is None:
            print(f"EPSG:{code}: PROJ can't build it, skipped", file=sys.stderr)
            continue
        records.append(result[0])
        ids += result[1]
    out = ALL if args.all else COMMITTED
    write(out / "wkt.jsonl", records)
    write(out / "identify.jsonl", ids)
    epsg = epsg_wkt(want, None if args.all else codes)
    if epsg is not None:
        write(out / "epsg.jsonl", epsg, "as EPSG's own WKT export")
    summary(records, ids)
    print(f"{time.monotonic() - started:.0f} s with {args.jobs} processes")
    return 0


if __name__ == "__main__":
    sys.exit(main())
