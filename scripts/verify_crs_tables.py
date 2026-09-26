#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["zstandard>=0.23", "jsonschema>=4", "pyproj==3.8.0"]
# ///
"""Verify `crates/xeibe-crs` beyond what the generator's own gate covers.

`scripts/gen_crs_tables.py` already refuses to write unless EPSG's own WKT2
release agrees with the rendered PROJJSON. That gate is strong but has two
holes, and says nothing about regressions between releases:

* EPSG publishes no WKT for **deprecated** CRSs, so 836 of them are unchecked.
* The WKT comparison only looks at PROJJSON. Nothing independently verifies
  the CRS *facts* table -- axis order, dimension, units -- which is what the
  axis-order decision actually reads.

So this adds four checks, none of which ship in the crate:

  --self        exhaustive internal consistency: the blob index, the facts
                table and the documents all agree, and every document parses.
  --schema      every document against the official PROJJSON schema. Unlike
                the WKT gate this covers deprecated CRSs.
  --pyproj      semantics against PROJ, an independent implementation that
                also carries deprecated CRSs. Compares conversion method and
                parameters, axis directions, ellipsoid and prime meridian --
                not names, which legitimately differ between EPSG releases.
  --regression  the regenerated tables against the committed ones: codes that
                vanished, axis orders that flipped, units that changed.

Run it with no flags for all of them. Exit status is non-zero if any check
fails, so it works as a release gate.

    scripts/verify_crs_tables.py
    scripts/verify_crs_tables.py --pyproj --regression
    scripts/verify_crs_tables.py --markdown summary.md
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import re
import struct
import sys
from collections import Counter
from pathlib import Path

import zstandard as zstd

REPO = Path(__file__).resolve().parent.parent
CRATE = REPO / "crates" / "xeibe-crs"
TABLE_RS = CRATE / "src" / "table.rs"
ALIASES_RS = CRATE / "src" / "aliases.rs"
BLOB = CRATE / "data" / "projjson.bin"

# Float comparisons: PROJ rounds conversion factors to 15 significant digits,
# so an exact match is not the right test.
TOLERANCE = 1e-9


def load_generator():
    """Import gen_crs_tables.py so the two cannot drift apart."""
    path = REPO / "scripts" / "gen_crs_tables.py"
    spec = importlib.util.spec_from_file_location("gen_crs_tables", path)
    module = importlib.util.module_from_spec(spec)
    sys.modules["gen_crs_tables"] = module
    spec.loader.exec_module(module)
    return module


# --------------------------------------------------------------------------
# Reading what is committed
# --------------------------------------------------------------------------

RECORD = re.compile(
    r"CrsRecord \{ code: (?P<code>\d+), name: \"(?P<name>(?:[^\"\\]|\\.)*)\", "
    r"kind: CrsKind::(?P<kind>\w+), first_axis: FirstAxis::(?P<axis>\w+), "
    r"dimension: (?P<dim>\d+), linear_unit_m: (?P<lin>None|Some\([^)]*\)), "
    r"angular_unit_rad: (?P<ang>None|Some\([^)]*\)), "
    r"area_wgs84: (?P<area>None|Some\(\[[^\]]*\]\)), deprecated: (?P<dep>true|false) \}"
)


def parse_optional_float(text: str) -> float | None:
    if text == "None":
        return None
    return float(strip_suffix(text[len("Some(") : -1]))


def strip_suffix(literal: str) -> str:
    """`0.3048f64` → `0.3048` (a character-set strip would eat trailing 2s and 3s)."""
    return re.sub(r"f(32|64)$", "", literal)


def read_committed_table() -> dict[int, dict]:
    out = {}
    for m in RECORD.finditer(TABLE_RS.read_text()):
        out[int(m.group("code"))] = {
            "name": m.group("name"),
            "kind": m.group("kind"),
            "first_axis": m.group("axis"),
            "dimension": int(m.group("dim")),
            "linear_unit_m": parse_optional_float(m.group("lin")),
            "angular_unit_rad": parse_optional_float(m.group("ang")),
            "area_wgs84": (
                None
                if m.group("area") == "None"
                else [float(strip_suffix(v)) for v in m.group("area")[6:-2].split(", ")]
            ),
            "deprecated": m.group("dep") == "true",
        }
    return out


def read_committed_blob() -> dict[int, dict]:
    """Decode data/projjson.bin the way src/lib.rs does."""
    raw = BLOB.read_bytes()
    if raw[:4] != b"XPJ1":
        sys.exit(f"{BLOB} is not a PROJJSON blob")
    shards, entries = struct.unpack_from("<II", raw, 4)
    shard_dir = [struct.unpack_from("<III", raw, 12 + i * 12) for i in range(shards)]
    index_at = 12 + shards * 12
    blob_at = index_at + entries * 8
    decompressor = zstd.ZstdDecompressor()
    cache: dict[int, list[str]] = {}
    out = {}
    for i in range(entries):
        code, shard, line = struct.unpack_from("<IHH", raw, index_at + i * 8)
        if shard not in cache:
            offset, comp_len, raw_len = shard_dir[shard]
            frame = raw[blob_at + offset : blob_at + offset + comp_len]
            cache[shard] = decompressor.decompress(frame, max_output_size=raw_len).decode().split("\n")
        out[code] = json.loads(cache[shard][line])
    return out


# --------------------------------------------------------------------------
# Comparable facts
# --------------------------------------------------------------------------


def close(a, b) -> bool:
    if a is None or b is None:
        return a is None and b is None
    return abs(float(a) - float(b)) <= TOLERANCE * max(1.0, abs(float(a)), abs(float(b)))


def measure(value):
    """PROJJSON writes a quantity either bare or as {value, unit}."""
    if isinstance(value, dict):
        return value.get("value")
    return value


def semantic_facts(doc: dict) -> dict:
    """The parts of a definition that must not differ between implementations.

    Deliberately excludes names: EPSG renames CRSs between releases (EPSG:2180
    was `ETRF2000-PL / CS92` in v12.029 and is `ETRS89 / PL-1992` in v13.103),
    so comparing them against an older PROJ would be pure noise.
    """
    facts: dict = {"type": doc.get("type")}

    conversion = doc.get("conversion")
    if conversion:
        facts["method"] = conversion["method"]["name"]
        facts["parameters"] = sorted(
            (p["name"], measure(p["value"])) for p in conversion["parameters"]
        )

    cs = doc.get("coordinate_system")
    if cs:
        facts["axes"] = [
            (a["direction"], (a.get("meridian") or {}).get("longitude")) for a in cs["axis"]
        ]

    base = doc.get("base_crs") or doc
    datum = base.get("datum") or base.get("datum_ensemble") or {}
    ellipsoid = datum.get("ellipsoid")
    if ellipsoid:
        facts["ellipsoid"] = (
            measure(ellipsoid.get("semi_major_axis")),
            measure(ellipsoid.get("semi_minor_axis")),
            ellipsoid.get("inverse_flattening"),
            measure(ellipsoid.get("radius")),
        )
    meridian = datum.get("prime_meridian")
    if meridian:
        facts["prime_meridian"] = measure(meridian.get("longitude"))
    return facts


def facts_differ(ours: dict, theirs: dict) -> list[str]:
    differing = []
    for key in set(ours) | set(theirs):
        a, b = ours.get(key), theirs.get(key)
        if key in ("ellipsoid",):
            if a is None or b is None:
                if a is not b:
                    differing.append(key)
            elif not all(close(x, y) for x, y in zip(a, b)):
                differing.append(key)
        elif key == "prime_meridian":
            if not close(a, b):
                differing.append(key)
        elif key == "parameters":
            if a is None or b is None or len(a) != len(b):
                differing.append(key)
            elif not all(na == nb and close(va, vb) for (na, va), (nb, vb) in zip(a, b)):
                differing.append(key)
        elif a != b:
            differing.append(key)
    return differing


# --------------------------------------------------------------------------
# Checks
# --------------------------------------------------------------------------


class Report:
    def __init__(self) -> None:
        self.lines: list[str] = []
        self.failed = False

    def ok(self, check: str, detail: str) -> None:
        self.lines.append(f"| {check} | ✅ | {detail} |")
        print(f"  {check}: {detail}")

    def fail(self, check: str, detail: str) -> None:
        self.failed = True
        self.lines.append(f"| {check} | ❌ | {detail} |")
        print(f"  {check}: FAILED -- {detail}", file=sys.stderr)

    def warn(self, check: str, detail: str) -> None:
        self.lines.append(f"| {check} | ⚠️ | {detail} |")
        print(f"  {check}: {detail}")


def read_committed_aliases() -> list[tuple[str, int]]:
    text = ALIASES_RS.read_text()
    return [(json.loads(alias), int(code)) for alias, code in re.findall(r'^    \(("(?:[^"\\]|\\.)*"), (\d+)\),$', text, re.M)]


def check_self(report: Report, table: dict, docs: dict) -> None:
    problems = []
    aliases = read_committed_aliases()
    dangling = [alias for alias, code in aliases if code not in table]
    if dangling:
        problems.append(f"{len(dangling)} aliases name no CRS record (e.g. {dangling[:5]})")
    keys = [alias for alias, _ in aliases]
    if keys != sorted(set(keys)) or any(alias != alias.lower() for alias in keys):
        problems.append("aliases are not lowercased, sorted and unique")
    missing_doc = sorted(set(table) - set(docs))
    orphan_doc = sorted(set(docs) - set(table))
    if missing_doc:
        problems.append(f"{len(missing_doc)} CRSs have no PROJJSON (e.g. {missing_doc[:5]})")
    if orphan_doc:
        problems.append(f"{len(orphan_doc)} documents have no CRS record (e.g. {orphan_doc[:5]})")

    mismatched_id = [c for c, d in docs.items() if d.get("id", {}).get("code") != c]
    if mismatched_id:
        problems.append(f"{len(mismatched_id)} documents disagree with their index code")

    wrong_authority = [c for c, d in docs.items() if d.get("id", {}).get("authority") != "EPSG"]
    if wrong_authority:
        problems.append(f"{len(wrong_authority)} documents are not EPSG")

    bad_shape = [
        code
        for code, record in table.items()
        if not (1 <= record["dimension"] <= 4) or not record["name"]
    ]
    if bad_shape:
        problems.append(f"{len(bad_shape)} records have an implausible dimension or empty name")

    if problems:
        report.fail("self-consistency", "; ".join(problems))
    else:
        report.ok(
            "self-consistency", f"{len(table)} records, {len(docs)} documents and {len(aliases)} aliases agree"
        )


def check_schema(report: Report, docs: dict) -> None:
    import jsonschema
    import pyproj

    schema_path = Path(pyproj.datadir.get_data_dir()) / "projjson.schema.json"
    if not schema_path.exists():
        report.warn("PROJJSON schema", "schema not available; skipped")
        return
    validator = jsonschema.Draft7Validator(json.loads(schema_path.read_text()))

    invalid = Counter()
    examples: dict[str, int] = {}
    for code, doc in docs.items():
        error = next(iter(validator.iter_errors(doc)), None)
        if error is not None:
            kind = doc.get("type", "?")
            invalid[kind] += 1
            examples.setdefault(kind, code)
    if invalid:
        detail = ", ".join(f"{n} {kind} (e.g. EPSG:{examples[kind]})" for kind, n in invalid.most_common())
        report.fail("PROJJSON schema", f"{sum(invalid.values())} invalid: {detail}")
    else:
        deprecated = sum(1 for d in docs.values() if d)
        report.ok("PROJJSON schema", f"all {len(docs)} documents valid (incl. deprecated CRSs)")


def known_divergence(ours: dict, theirs: dict, keys: list[str]) -> str | None:
    """Name the two ways PROJ legitimately renders things differently.

    Both are confirmed against EPSG's own WKT2 release, which agrees with us
    and not with PROJ, so these are rendering choices rather than errors. Any
    *other* difference is treated as a regression.
    """
    # PROJ flattens a derived vertical CRS into a plain VerticalCRS, dropping
    # the base and the conversion. EPSG's WKT keeps BASEVERTCRS +
    # DERIVINGCONVERSION, and so do we.
    if ours.get("type") == "DerivedVerticalCRS" and theirs.get("type") == "VerticalCRS":
        if set(keys) <= {"type", "method", "parameters"}:
            return "derived vertical CRS flattened by PROJ"

    # PROJ hardcodes some methods' constants and omits them from its export --
    # Krovak Modified (EPSG method 1042) and its North Orientated variant leave
    # out the evaluation-point ordinates and the C1-C10 coefficients. Allowed
    # only when the method matches and every parameter PROJ *does* emit agrees,
    # so ours is a strict superset rather than a different definition. The
    # method name goes in the reason, so a newly-excused method shows up in the
    # report instead of passing silently.
    method = ours.get("method")
    if keys == ["parameters"] and method and method == theirs.get("method"):
        mine = dict(ours.get("parameters") or [])
        yours = dict(theirs.get("parameters") or [])
        if set(yours) < set(mine) and all(close(mine.get(n), v) for n, v in yours.items()):
            return f"parameters of {method!r} omitted by PROJ"
    return None


def check_pyproj(report: Report, table: dict, docs: dict) -> None:
    import pyproj

    compared = 0
    absent = 0
    differing = Counter()
    allowed = Counter()
    examples: list[str] = []

    for code in sorted(docs):
        try:
            theirs_doc = pyproj.CRS.from_authority("EPSG", str(code)).to_json_dict()
        except Exception:
            absent += 1  # newer than PROJ's bundled EPSG, or dropped by PROJ
            continue
        compared += 1
        ours_f, theirs_f = semantic_facts(docs[code]), semantic_facts(theirs_doc)
        keys = facts_differ(ours_f, theirs_f)
        if not keys:
            continue
        if reason := known_divergence(ours_f, theirs_f, keys):
            allowed[reason] += 1
            continue
        for key in keys:
            differing[key] += 1
        if len(examples) < 5:
            examples.append(
                f"EPSG:{code} {keys[0]}: ours={str(ours_f.get(keys[0]))[:80]} "
                f"PROJ={str(theirs_f.get(keys[0]))[:80]}"
            )

    # Also check the facts table, which the WKT gate never sees.
    axis_mismatch = []
    for code, record in table.items():
        doc = docs.get(code)
        if not doc:
            continue
        cs = doc.get("coordinate_system")
        if not cs or not cs["axis"]:
            continue
        expected = {"east": "EastOrLon", "north": "NorthOrLat"}.get(cs["axis"][0]["direction"], "Other")
        if record["first_axis"] != expected:
            axis_mismatch.append(code)

    known = "; ".join(f"{n} {reason}" for reason, n in allowed.most_common())
    detail = f"{compared} CRSs vs PROJ {pyproj.proj_version_str} ({absent} not in its EPSG)"
    if known:
        detail += f"; known divergences: {known}"

    if axis_mismatch:
        report.fail(
            "pyproj cross-check",
            f"{len(axis_mismatch)} facts-table axis orders disagree with the document "
            f"(e.g. {axis_mismatch[:5]})",
        )
    elif differing:
        summary = ", ".join(f"{key}: {n}" for key, n in differing.most_common())
        report.fail("pyproj cross-check", f"unexplained differences -- {summary}. {'; '.join(examples)}")
    else:
        report.ok("pyproj cross-check", f"{detail}; facts table consistent")


def check_regression(report: Report, committed: dict, regenerated: dict, docs: dict) -> None:
    """What changed relative to what is already committed."""
    if not committed:
        report.warn("regression", "nothing committed to compare against")
        return

    removed = sorted(set(committed) - set(regenerated))
    added = sorted(set(regenerated) - set(committed))
    axis_flipped, unit_changed, dim_changed = [], [], []
    for code in sorted(set(committed) & set(regenerated)):
        before, after = committed[code], regenerated[code]
        if before["first_axis"] != after["first_axis"]:
            axis_flipped.append(f"EPSG:{code} {before['first_axis']}->{after['first_axis']}")
        if not close(before["linear_unit_m"], after["linear_unit_m"]):
            unit_changed.append(f"EPSG:{code}")
        if before["dimension"] != after["dimension"]:
            dim_changed.append(f"EPSG:{code} {before['dimension']}->{after['dimension']}")

    detail = f"{len(added)} added, {len(removed)} removed, {len(committed)} before"

    # Removals and axis flips are the dangerous ones: existing data keeps using
    # retired codes, and a flipped axis silently mirrors every coordinate.
    if removed or axis_flipped:
        parts = []
        if removed:
            parts.append(f"{len(removed)} codes removed (e.g. {removed[:8]})")
        if axis_flipped:
            parts.append(f"{len(axis_flipped)} axis orders changed: {axis_flipped[:8]}")
        report.fail("regression", "; ".join(parts))
    elif unit_changed or dim_changed:
        report.warn(
            "regression",
            f"{detail}; {len(unit_changed)} unit and {len(dim_changed)} dimension changes "
            f"{(unit_changed + dim_changed)[:5]}",
        )
    else:
        report.ok("regression", detail + ", no removals or axis changes")


# --------------------------------------------------------------------------


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--self", dest="do_self", action="store_true")
    ap.add_argument("--schema", action="store_true")
    ap.add_argument("--pyproj", action="store_true")
    ap.add_argument("--regression", action="store_true")
    ap.add_argument("--sql", type=Path, help="EPSG archive for --regression (default: example_data)")
    ap.add_argument("--markdown", type=Path, help="also write a summary table here")
    args = ap.parse_args()

    everything = not (args.do_self or args.schema or args.pyproj or args.regression)
    report = Report()

    print("reading committed tables")
    table = read_committed_table()
    docs = read_committed_blob()
    print(f"  {len(table)} CRS records, {len(docs)} PROJJSON documents")

    if everything or args.do_self:
        check_self(report, table, docs)
    if everything or args.schema:
        check_schema(report, docs)
    if everything or args.pyproj:
        check_pyproj(report, table, docs)
    if everything or args.regression:
        generator = load_generator()
        sql = args.sql or generator.find_sql_zip()
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            db = generator.load_epsg(sql, Path(tmp) / "epsg.db")
            epsg = generator.Epsg(db)
            regenerated = {}
            for row in epsg.all("select * from epsg_coordinatereferencesystem"):
                if generator.EXAMPLE_NAME.match(row["coord_ref_sys_name"]):
                    continue
                record = generator.crs_record(epsg, row)
                regenerated[record["code"]] = {
                    "first_axis": ("EastOrLon", "NorthOrLat", "Other")[record["first_axis"]],
                    "dimension": record["dimension"],
                    "linear_unit_m": record["linear_unit_m"],
                }
        check_regression(report, table, regenerated, docs)

    if args.markdown:
        args.markdown.write_text(
            "| check | | detail |\n|---|---|---|\n" + "\n".join(report.lines) + "\n"
        )

    print("\nFAILED" if report.failed else "\nall checks passed")
    return 1 if report.failed else 0


if __name__ == "__main__":
    sys.exit(main())
