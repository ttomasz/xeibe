#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["pyproj==3.8.0"]
# ///
"""Test vectors from IOGP's GIGS test dataset, for `xeibe-reproject`.

GIGS (Geospatial Integrity of Geoscience Software, https://gigs.iogp.org)
publishes conversions (series 5100) and transformations (series 5200)
with their expected results, tolerances and round trips. This script
downloads the dataset at a pinned commit of
https://github.com/IOGP-GIGS/GIGSTestDataset into
`example_data/gigs/<commit>/` (series 2200, 3200, 5100 and 5200),
builds each CRS a test uses (its EPSG
equivalent, or PROJJSON from GIGS's definitions where it has none), pairs
each input with its expected output, records PROJ's result for each, and
writes `target/gigs/gigs.jsonl`, which `tests/it/gigs.rs` reads (run with
`--ignored`). The GIGS data stays out of git: its licence (IOGP's GIGS
Terms of Use) forbids distribution for profit, so it isn't open.

    scripts/gen_gigs_vectors.py                 # downloads if missing
    scripts/gen_gigs_vectors.py --gigs DIR      # a local copy (GIGSTestDatasetFiles)

Each record is one point: `src` and `dst` (`{"epsg": code}`,
`{"projjson": {...}}` or `{"compound": [h, v]}`), `op` (an EPSG
transformation to force, or null for a conversion or the automatic
choice), `in` and `out` in the CRSs' own axis order and units, the
tolerance per output ordinate, `round_trip` with its tolerances, and
`proj`, PROJ 9.8.1's result on the oracle database (`target/proj-oracle`)
and PROJ-data's grids (`target/proj-grids`), or null where PROJ has none,
and for a round-trip point `proj_round_trip`, where PROJ's result is after
going back and forth 1000 times.
"""

from __future__ import annotations

import argparse
import io
import json
import re
import sys
import tarfile
import urllib.request
import warnings
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
COMMIT = "c076a6fd7586dd400ba13ba6d61ca8a842de4873"  # v2.1, 2022-10-12
TARBALL = f"https://codeload.github.com/IOGP-GIGS/GIGSTestDataset/tar.gz/{COMMIT}"
DEST = REPO / "example_data" / "gigs" / COMMIT
OUT = REPO / "target" / "gigs"
ORACLE = REPO / "target" / "proj-oracle"
GRIDS = REPO / "target" / "proj-grids"

CONVERSIONS = "GIGS 5100 Conversion test data/ASCII"
TRANSFORMATIONS = "GIGS 5200 Coordinate transformation test data/ASCII"
USER = "GIGS 3200 User-defined Geodetic Data Objects test data/ASCII"

# The 5200 tests, by output file: the EPSG transformation that is GIGS's
# (its 3208 or 2208 definition names the equivalent), or None for a
# conversion. Files left out can't be run as an EPSG operation; SKIPPED
# says why.
TRANSFORMATION_TESTS = {
    "GIGS_tfm_5201_GeogGeocen_output.txt": None,
    "GIGS_tfm_5203_PosVec_output_part1.txt": 1314,  # GIGS 61314, OSGB36 to WGS 84 (6)
    "GIGS_tfm_5203_PosVec_output_part2.txt": 1314,  # the same in 3D (method 1037)
    "GIGS_tfm_5204_CoordFrame_output_part1.txt": 15929,  # BD72 to WGS 84 (3)
    "GIGS_tfm_5204_CoordFrame_output_part2.txt": 15929,  # in 3D (method 1038)
    "GIGS_tfm_5207_NTv2_output_part1.txt": 1803,  # AGD66 to GDA94 (11), A66 National (13.09)
    "GIGS_tfm_5207_NTv2_output_part2.txt": 1313,  # NAD27 to NAD83 (4), Canada's ntv2_0
    "GIGS_tfm_5208_LonRot_output.txt": 1763,  # NTF (Paris) to NTF (1), reversed
    "GIGS_tfm_5211_3trnslt_Geocen_output.txt": 1196,  # OSGB36 to WGS 84 (2), geocentric
    "GIGS_tfm_5212_3trnslt_Geog3D_output_EPSGconcat.txt": 1196,  # in 3D
    "GIGS_tfm_5213_3trnslt_Geog2D_output_EPSGconcat.txt": 1196,
}
SKIPPED = {
    "GIGS_tfm_5205_MolBad_output_part1.txt": "GIGS 61003, Molodensky-Badekas with parameters of its own (no EPSG equivalent)",
    "GIGS_tfm_5205_MolBad_output_part2.txt": "GIGS 61003 in 3D (no EPSG equivalent)",
    "GIGS_tfm_5206_Nadcon_output.txt": "GIGS 61004, NADCON on GIGS's own grid files (no EPSG equivalent)",
    "GIGS_tfm_5209_BinGrid_output.txt": "P6 seismic bin grid, an engineering CRS xeibe doesn't have",
    "GIGS_tfm_5212_3trnslt_Geog3D_output_AbrMol.txt": "Abridged Molodensky, for reference only",
    "GIGS_tfm_5213_3trnslt_Geog2D_output_AbrMol.txt": "Abridged Molodensky, for reference only",
    "GIGS_conv_5101_TM_output_part*_USGS.txt": "Transverse Mercator by the USGS formulas, for software using them; "
    "xeibe and PROJ use the exact series, which GIGS's JHS outputs give",
}
# 5210: heights and depths on Pulkovo 1942, each column from each other.
VERTICAL_TEST = "GIGS_tfm_5210_VertOff_output.txt"
VERTICAL_HORIZONTAL = 4284


def fetch(dest: Path) -> None:
    """GIGSTestDatasetFiles at COMMIT, into `dest`, without anything outside it."""
    print(f"downloading {TARBALL}")
    with urllib.request.urlopen(TARBALL) as response:
        data = response.read()
    prefix = f"GIGSTestDataset-{COMMIT}/GIGSTestDatasetFiles/"
    # Only the series the tests use: not the seismic and well data (~90 MB).
    wanted = ("GIGS 2200 ", "GIGS 3200 ", "GIGS 5100 ", "GIGS 5200 ")
    dest.mkdir(parents=True, exist_ok=True)
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as tar:
        members = []
        for m in tar.getmembers():
            if not m.name.startswith(prefix) or not (m.isfile() or m.isdir()):
                continue
            m.name = m.name[len(prefix) :]
            if m.name.startswith(wanted):
                members.append(m)
        tar.extractall(dest, members=members, filter="data")


# --------------------------------------------------------------------------
# GIGS's text files
# --------------------------------------------------------------------------


def read_table(path: Path) -> tuple[dict[str, str], list[str], list[list[str]]]:
    """The `# Key: value` header lines, the field descriptions and the rows."""
    header, fields, rows = {}, [], []
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        line = line.rstrip("\t\r ")
        if line.startswith("#"):
            m = re.match(r"#\s*\[(\d+)\]:\s*(.*)", line)
            if m:
                fields.append(m.group(2).strip())
            elif ":" in line:
                key, _, value = line[1:].partition(":")
                header[key.strip()] = value.strip()
        elif line.strip():
            rows.append(line.split("\t"))
    return header, fields, rows


def number(text: str) -> float | None:
    text = text.strip()
    return None if text in ("", "NULL") else float(text)


def tolerance(header: dict[str, str], key: str) -> float | None:
    value = header.get(key)
    return float(value.split()[0]) if value else None


FIELD = re.compile(
    r"^(?P<quantity>[^(]+?)\s*\((?:GIGS CRS Code (?P<gigs>\d+); )?(?P<name>[^;]+); (?P<datum>[^;]+); "
    r"(?P<unit>[^;]+); (?:EPSG CRS code (?P<epsg>\d+)|No direct EPSG equivalent)\)"
)


def describe(field: str) -> dict | None:
    m = FIELD.match(field)
    return m.groupdict() if m else None


# --------------------------------------------------------------------------
# CRSs
# --------------------------------------------------------------------------


class Definitions:
    """GIGS's user-defined objects (series 3200), for the CRSs that have
    no EPSG equivalent."""

    def __init__(self, root: Path):
        user = root / USER
        _, _, self.conversions = read_table(user / "GIGS_user_3206_Conversion.txt")
        _, _, self.projected = read_table(user / "GIGS_user_3207_ProjectedCRS.txt")
        _, _, self.geodetic = read_table(user / "GIGS_user_3205_GeodeticCRS.txt")
        _, _, self.datums = read_table(user / "GIGS_user_3204_GeodeticDatum.txt")

    def row(self, rows: list[list[str]], code: str) -> list[str]:
        for r in rows:
            if r and r[0] == code:
                return r
        raise KeyError(code)

    def conversion(self, code: str) -> dict:
        r = self.row(self.conversions, code)
        # Name, value and unit, then the value in decimal degrees: given for
        # every parameter in most rows (NULL where not angular), only for
        # the sexagesimal ones in others.
        params, i = [], 3
        while i + 2 < len(r) and r[i] not in ("", "NULL") and not re.fullmatch(r"-?[\d.]+", r[i]):
            name, value, unit = r[i : i + 3]
            decimal = r[i + 3] if i + 3 < len(r) else ""
            has_decimal = decimal == "NULL" or re.fullmatch(r"-?[\d.]+", decimal or "x") is not None
            if unit == "sexagesimal DMS":
                value, unit = decimal, "degree"
            unit = {"Unity": "unity", "metre": "metre", "degree": "degree", "grad": "grad"}.get(unit, unit)
            params.append({"name": name, "value": float(value), "unit": unit})
            i += 4 if has_decimal else 3
        return {"name": r[1], "method": {"name": r[2]}, "parameters": params}

    def projected_crs(self, code: str, base: dict) -> dict:
        """A projected CRS GIGS defines: its conversion and axes on `base`,
        the PROJJSON of the geographic CRS the test file gives with it."""
        r = self.row(self.projected, code)
        axes = []
        for name, abbrev, direction, unit in (r[8:12], r[12:16]):
            axes.append(
                {
                    "name": name,
                    "abbreviation": "" if abbrev == "none" else abbrev,
                    "direction": direction,
                    "unit": {"US survey foot": "US survey foot", "foot": "foot"}.get(unit, "metre"),
                }
            )
        return {
            "$schema": "https://proj.org/schemas/v0.7/projjson.schema.json",
            "type": "ProjectedCRS",
            "name": r[2],
            # With its EPSG id, which is what says which datum it is.
            "base_crs": {k: v for k, v in base.items() if k not in ("$schema", "usages", "remarks", "scope", "area", "bbox")},
            "conversion": self.conversion(r[5]),
            "coordinate_system": {"subtype": "Cartesian", "axis": axes},
        }

    def geodetic_crs(self, code: str) -> dict:
        """A geographic 3D or geocentric CRS on an EPSG datum that EPSG
        has no such CRS for (OSGB36, Amersfoort, Belge 1972)."""
        r = self.row(self.geodetic, code)
        datum = self.row(self.datums, r[4])[7]
        return {"kind": r[3], "datum": int(datum)}


def projjson_of(pyproj, spec: dict) -> dict:
    """PROJJSON with EPSG's ids filled in by PROJ, from names."""
    crs = pyproj.CRS.from_json_dict(spec)
    return json.loads(crs.to_json())


def geodetic_projjson(pyproj, kind: str, datum: int) -> dict:
    """A geographic 3D or geocentric CRS on an EPSG datum, from its
    geographic 2D CRS."""
    base = None
    for info in pyproj.database.query_crs_info(auth_name="EPSG", pj_types="GEOGRAPHIC_2D_CRS"):
        crs = pyproj.CRS.from_epsg(info.code)
        d = crs.datum.to_json_dict()
        if d.get("id", {}).get("code") == datum:
            base = json.loads(crs.to_json())
            break
    if base is None:
        raise KeyError(datum)
    body = {k: v for k, v in base.items() if k in ("datum", "datum_ensemble")}
    if kind == "Geocentric":
        cs = {
            "subtype": "Cartesian",
            "axis": [
                {"name": f"Geocentric {a}", "abbreviation": a, "direction": f"geocentric{a}", "unit": "metre"}
                for a in "XYZ"
            ],
        }
        spec = {"type": "GeodeticCRS", "name": f"{base['name']} geocentric", **body, "coordinate_system": cs}
    else:
        axes = base["coordinate_system"]["axis"] + [
            {"name": "Ellipsoidal height", "abbreviation": "h", "direction": "up", "unit": "metre"}
        ]
        spec = {
            "type": "GeographicCRS",
            "name": f"{base['name']} 3D",
            **body,
            "coordinate_system": {"subtype": "ellipsoidal", "axis": axes},
        }
    spec["$schema"] = base.get("$schema")
    return projjson_of(pyproj, spec)


# --------------------------------------------------------------------------
# PROJ's results
# --------------------------------------------------------------------------


def crs_of(pyproj, spec: dict):
    if "epsg" in spec:
        return pyproj.CRS.from_epsg(spec["epsg"])
    if "compound" in spec:
        h, v = spec["compound"]
        return pyproj.CRS.from_user_input(f"EPSG:{h}+{v}")
    return pyproj.CRS.from_json_dict(spec["projjson"])


def proj_transformer(pyproj, src: dict, dst: dict, op: int | None):
    """PROJ's transformation, the EPSG operation `op` where given."""
    from pyproj.transformer import TransformerGroup

    s, d = crs_of(pyproj, src), crs_of(pyproj, dst)
    if op is None:
        return pyproj.Transformer.from_crs(s, d, always_xy=False)
    wanted = pyproj.crs.CoordinateOperation.from_epsg(op).name
    group = TransformerGroup(s, d, always_xy=False)
    chosen = [t for t in group.transformers if wanted in t.description]
    return chosen[0] if chosen else None


def proj_result(pyproj, src: dict, dst: dict, op: int | None, values: list[float], round_trip: bool = False):
    """PROJ's result, and with `round_trip` where it is after 1000 times
    back and forth (GIGS's round trip)."""
    try:
        t = proj_transformer(pyproj, src, dst, op)
        if t is None:
            return None, None
        n = len(values) if len(values) > 2 else 2
        out = list(t.transform(*values, errcheck=True))[:n]
        if not all(v == v and abs(v) != float("inf") for v in out):
            return None, None
        if not round_trip:
            return out, None
        p = out
        for _ in range(1000):
            back = t.transform(*p, direction="INVERSE", errcheck=True)
            p = list(t.transform(*back, errcheck=True))[:n]
        return out, p
    except Exception:  # noqa: BLE001 - PROJ has no result for this point
        return None, None


# --------------------------------------------------------------------------
# Records
# --------------------------------------------------------------------------


def group_fields(fields: list[str]) -> list[tuple[dict, list[int]]]:
    """Consecutive fields of one CRS, with their indices."""
    groups: list[tuple[dict, list[int]]] = []
    for i, f in enumerate(fields):
        d = describe(f)
        if d is None:
            continue
        key = (d["gigs"], d["name"])
        if groups and (groups[-1][0]["gigs"], groups[-1][0]["name"]) == key:
            groups[-1][1].append(i)
        else:
            groups.append((d, [i]))
    return groups


AXIS_WORDS = {
    "Latitude": ("north", "south"),
    "Longitude": ("east", "west"),
    "Easting": ("east",),
    "Northing": ("north",),
    "Westing": ("west",),
    "Southing": ("south",),
    "Ellipsoidal height": ("up",),
    "Geocentric X": ("geocentricX",),
    "Geocentric Y": ("geocentricY",),
    "Geocentric Z": ("geocentricZ",),
}


def axis_permutation(pyproj, spec: dict, quantities: list[str]) -> list[int]:
    """For each of the CRS's axes in order, the index of the file's field
    that holds it."""
    crs = crs_of(pyproj, spec)
    directions = [a.direction.lower() for a in crs.axis_info]
    order = []
    for direction in directions:
        for i, q in enumerate(quantities):
            if i not in order and direction.lower() in [w.lower() for w in AXIS_WORDS.get(q, ())]:
                order.append(i)
                break
        else:
            raise ValueError(f"{crs.name}: no field for axis {direction} among {quantities}")
    return order


def crs_spec(pyproj, d: dict, defs: Definitions, geographic: dict | None) -> dict:
    """The CRS of one group of fields."""
    if d["epsg"]:
        return {"epsg": int(d["epsg"])}
    gigs = d["gigs"]
    if gigs and gigs.startswith("62"):
        assert geographic is not None and "epsg" in geographic, d
        base = json.loads(pyproj.CRS.from_epsg(geographic["epsg"]).to_json())
        return {"projjson": projjson_of(pyproj, defs.projected_crs(gigs, base))}
    if gigs:
        g = defs.geodetic_crs(gigs)
        return {"projjson": geodetic_projjson(pyproj, g["kind"], g["datum"])}
    if d["name"] == "GIGS geocenCRS B":  # 5211: OSGB36 geocentric, no GIGS code given
        return {"projjson": geodetic_projjson(pyproj, "Geocentric", 6277)}
    raise ValueError(d)


def tolerances(header: dict[str, str], quantities: list[str], unit_of: list[str]) -> tuple[list[float], list[float]]:
    single, trip = [], []
    for q, unit in zip(quantities, unit_of):
        angular = q in ("Latitude", "Longitude")
        single.append(tolerance(header, "Geographic Tolerance" if angular else "Cartesian Tolerance"))
        trip.append(tolerance(header, "Round Trip Geographic Tolerance" if angular else "Round Trip Cartesian Tolerance"))
    return single, trip


def records_for(pyproj, root: Path, folder: str, output: str, op: int | None, defs: Definitions) -> list[dict]:
    header, fields, rows = read_table(root / folder / output)
    groups = group_fields(fields)
    assert len(groups) == 2, (output, groups)
    (first, fi), (second, si) = groups
    a = crs_spec(pyproj, first, defs, None)
    b = crs_spec(pyproj, second, defs, a if "epsg" in a else None)
    qa = [describe(fields[i])["quantity"] for i in fi]
    qb = [describe(fields[i])["quantity"] for i in si]
    pa, pb = axis_permutation(pyproj, a, qa), axis_permutation(pyproj, b, qb)
    ua = [describe(fields[i])["unit"] for i in fi]
    ub = [describe(fields[i])["unit"] for i in si]
    direction_col = next(i for i, f in enumerate(fields) if "Direction" in f)
    remarks_col = next((i for i, f in enumerate(fields) if "Remarks" in f), None)
    out = []
    for r in rows:
        va = [number(r[fi[i]]) for i in pa]
        vb = [number(r[si[i]]) for i in pb]
        if None in va or None in vb:
            continue
        forward = r[direction_col].strip().upper() == "FORWARD"
        src, dst, vin, vout = (a, b, va, vb) if forward else (b, a, vb, va)
        quantities = [qb[i] for i in pb] if forward else [qa[i] for i in pa]
        units = [ub[i] for i in pb] if forward else [ua[i] for i in pa]
        single, trip = tolerances(header, quantities, units)
        round_trip = remarks_col is not None and "Round Trip" in (r[remarks_col] if remarks_col < len(r) else "")
        proj, proj_trip = proj_result(pyproj, src, dst, op, vin, round_trip)
        # The operation as EPSG defines it, from its source to its target;
        # a REVERSE point runs it backwards.
        out.append(
            {
                "test": output.split("_")[2],
                "file": output,
                "point": r[0],
                "src": src,
                "dst": dst,
                "op": op,
                "in": vin,
                "out": vout,
                "tolerance": single,
                "round_trip": round_trip,
                "round_trip_tolerance": trip if round_trip else None,
                "proj": proj,
                "proj_round_trip": proj_trip,
            }
        )
    return out


def vertical_records(pyproj, root: Path) -> list[dict]:
    """5210: each given height or depth to each of the other three."""
    header, fields, rows = read_table(root / TRANSFORMATIONS / VERTICAL_TEST)
    columns = [(i, describe(f)) for i, f in enumerate(fields) if describe(f) and describe(f)["quantity"] in ("Height", "Depth")]
    cartesian = tolerance(header, "Cartesian Tolerance")
    trip = tolerance(header, "Round Trip Cartesian Tolerance")
    remarks_col = next(i for i, f in enumerate(fields) if "Remarks" in f)
    out = []
    for r in rows:
        lat, lon = number(r[1]), number(r[2])
        values = {int(d["epsg"]): number(r[i]) for i, d in columns}
        round_trip = "Round Trip" in (r[remarks_col] if remarks_col < len(r) else "")
        for s, sv in values.items():
            for t, tv in values.items():
                if s == t:
                    continue
                src, dst = {"compound": [VERTICAL_HORIZONTAL, s]}, {"compound": [VERTICAL_HORIZONTAL, t]}
                vin = [lat, lon, sv]
                proj, proj_trip = proj_result(pyproj, src, dst, None, vin, round_trip)
                out.append(
                    {
                        "test": "5210",
                        "file": VERTICAL_TEST,
                        "point": r[0],
                        "src": src,
                        "dst": dst,
                        "op": None,
                        "in": vin,
                        "out": [lat, lon, tv],
                        "tolerance": [1e-9, 1e-9, cartesian],
                        "round_trip": round_trip,
                        "round_trip_tolerance": [1e-9, 1e-9, trip] if round_trip else None,
                        "proj": proj,
                        "proj_round_trip": proj_trip,
                    }
                )
    return out


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--gigs", type=Path, help="a local GIGSTestDatasetFiles directory")
    args = ap.parse_args()
    root = args.gigs or DEST
    if not (root / CONVERSIONS).is_dir():
        if args.gigs:
            sys.exit(f"{root}: no {CONVERSIONS}")
        fetch(root)

    warnings.filterwarnings("ignore")
    import pyproj

    if not (ORACLE / "proj.db").exists():
        sys.exit(f"{ORACLE}/proj.db missing: run scripts/build_proj_oracle.py")
    pyproj.datadir.set_data_dir(str(ORACLE))
    if GRIDS.is_dir():
        pyproj.datadir.append_data_dir(str(GRIDS))
    pyproj.network.set_network_enabled(False)

    defs = Definitions(root)
    records = []
    for path in sorted((root / CONVERSIONS).glob("GIGS_conv_*_output*.txt")):
        if not path.name.endswith("_USGS.txt"):
            records += records_for(pyproj, root, CONVERSIONS, path.name, None, defs)
    for name, op in TRANSFORMATION_TESTS.items():
        records += records_for(pyproj, root, TRANSFORMATIONS, name, op, defs)
    records += vertical_records(pyproj, root)

    OUT.mkdir(parents=True, exist_ok=True)
    with open(OUT / "gigs.jsonl", "w") as f:
        for r in records:
            f.write(json.dumps(r) + "\n")
    tests = sorted({r["test"] for r in records})
    print(f"{len(records)} points from {len(tests)} test procedures ({', '.join(tests)}) to {OUT / 'gigs.jsonl'}")
    print(f"{sum(r['proj'] is None for r in records)} without a PROJ result")
    (OUT / "skipped.json").write_text(json.dumps({"commit": COMMIT, "skipped": SKIPPED}, indent=1) + "\n")
    for name, why in SKIPPED.items():
        print(f"  skipped {name}: {why}")


if __name__ == "__main__":
    main()
