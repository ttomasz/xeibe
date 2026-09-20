#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["zstandard>=0.23"]
# ///
"""Generate `crates/xeibe-crs` from an official EPSG Dataset release.

Input is the EPSG "PostgreSQL scripts" archive (`EPSG-v<MAJOR>_<MINOR>-PostgreSQL.zip`)
downloaded from https://epsg.org/download-dataset.html, plus, optionally, the
"WKT File" archive used as a differential test oracle.

Nothing here needs PROJ, GDAL or pyproj: the EPSG relational tables are read
directly.  The one thing they are *not* is normalised, so see `angle()` below.

    scripts/gen_crs_tables.py                      # uses example_data/EPSG-*-PostgreSQL.zip
    scripts/gen_crs_tables.py --sql PATH --wkt PATH
    scripts/gen_crs_tables.py --check             # regenerate into a temp dir and diff

Outputs (all committed):

    crates/xeibe-crs/src/table.rs      CRS facts, one `CrsRecord` per EPSG code
    crates/xeibe-crs/data/projjson.bin zstd-sharded PROJJSON, addressed by code
    crates/xeibe-crs/EPSG-NOTICE.md    provenance and EPSG terms (regenerated)
"""

from __future__ import annotations

import argparse
import json
import re
import sqlite3
import struct
import sys
import tempfile
import zipfile
from collections import Counter, defaultdict
from decimal import Decimal
from pathlib import Path

import zstandard as zstd

REPO = Path(__file__).resolve().parent.parent
CRATE = REPO / "crates" / "xeibe-crs"

# EPSG publishes worked examples as if they were CRSs; PROJ omits them and so do
# we.  They are not real coordinate reference systems.
EXAMPLE_NAME = re.compile(r"^(EPSG (seismic bin grid|local engineering grid) example|EPSG example|enter here name of)")

# --------------------------------------------------------------------------
# Loading
# --------------------------------------------------------------------------


def load_epsg(zip_path: Path, db_path: Path) -> sqlite3.Connection:
    """Ingest the EPSG PostgreSQL scripts into SQLite.

    The scripts are plain DDL plus INSERT statements and need no translation;
    only the UTF-8 BOM has to go.  The foreign-key script is skipped, it adds
    nothing for a read-only extract.
    """
    with zipfile.ZipFile(zip_path) as z:
        names = {Path(n).name: n for n in z.namelist()}
        for required in ("PostgreSQL_Table_Script.sql", "PostgreSQL_Data_Script.sql"):
            if required not in names:
                sys.exit(f"{zip_path}: missing {required}")
        table_sql = z.read(names["PostgreSQL_Table_Script.sql"]).decode("utf-8-sig")
        data_sql = z.read(names["PostgreSQL_Data_Script.sql"]).decode("utf-8-sig")

    db = sqlite3.connect(db_path)
    db.executescript("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF;")
    db.executescript(table_sql)
    db.executescript(data_sql)
    db.commit()
    db.row_factory = sqlite3.Row
    return db


def epsg_version(db: sqlite3.Connection) -> tuple[str, str]:
    row = db.execute(
        "select version_number, version_date from epsg_versionhistory order by version_date desc limit 1"
    ).fetchone()
    if row is None:
        return ("unknown", "unknown")
    return (str(row["version_number"]), str(row["version_date"])[:10])


# --------------------------------------------------------------------------
# Units
# --------------------------------------------------------------------------

# "degree (supplier to define representation)" is plain degree.
UOM_ALIAS = {9122: 9102}

# Eleven angular units in the EPSG dataset have no conversion factor at all.
# They split into two groups that must be treated quite differently.
#
# Numeric packings: the *value* encodes degrees, minutes and seconds in its
# digits, so it has to be decoded before use.  Every such value must go through
# `angle()` before it reaches `unit()`, which asserts on them -- passing one
# through verbatim is a silent corruption, not an error.
SEXAGESIMAL_DMS = 9110  # DDD.MMSSsss
SEXAGESIMAL_DM = 9111  # DDD.MMm
SEXAGESIMAL_DMS_S = 9121  # DDD.MMSSsss with more decimals; unused in v13.103
SEXAGESIMAL = {SEXAGESIMAL_DMS, SEXAGESIMAL_DM, SEXAGESIMAL_DMS_S}

# Display formats: these say how a degree value is *written* ("45°30'15\"N"),
# not what it measures.  The quantity is degrees, so they take the degree
# factor while keeping EPSG's name, which is what PROJ emits too.  In v13.103
# they appear only as axis units, on deprecated CRSs.
DEGREE_FORMAT = {9107, 9108, 9115, 9116, 9117, 9118, 9119, 9120}
DEGREE = 9102

UNIT_TYPE = {"length": "LinearUnit", "angle": "AngularUnit", "scale": "ScaleUnit", "time": "TimeUnit"}
# PROJJSON writes these three as a bare string rather than an object.
UNIT_SHORTHAND = {"metre", "degree", "unity"}


def from_sexagesimal(value: float, uom: int) -> float:
    """Decode EPSG's packed angles to decimal degrees.

    The digits after the decimal point are fixed-width fields, not a fraction,
    and they are padded on the *right*: `-58.3` is -58°30', not -58.3° and not
    -58°03'.  Fields are never range-checked -- EPSG:10788 genuinely stores
    `44.75` meaning 44°75' = 45.25°, and PROJ reads it the same way.
    """
    packed = Decimal(str(value))
    sign = -1 if packed < 0 else 1
    packed = abs(packed)
    degrees = int(packed)
    frac = (f"{packed}".split(".") + [""])[1]
    if uom == SEXAGESIMAL_DM:  # DDD.MMm -- the tail is decimal minutes
        minutes = Decimal(frac[:2].ljust(2, "0") + "." + (frac[2:] or "0"))
        return float(sign * (Decimal(degrees) + minutes / 60))
    minutes = Decimal(frac[:2].ljust(2, "0"))  # DDD.MMSSsss -- the tail is decimal seconds
    seconds = Decimal(frac[2:4].ljust(2, "0") + "." + (frac[4:] or "0"))
    return float(sign * (Decimal(degrees) + minutes / 60 + seconds / 3600))


def angle(value, uom: int | None):
    """Normalise one angular value, returning `(value, uom)` fit for `unit()`."""
    if uom in SEXAGESIMAL:
        return from_sexagesimal(value, uom), DEGREE
    return value, uom


class Epsg:
    """Read-side helpers over the EPSG tables."""

    def __init__(self, db: sqlite3.Connection):
        self.db = db
        self._unit_cache: dict[int, object] = {}

    def one(self, sql: str, *args):
        return self.db.execute(sql, args).fetchone()

    def all(self, sql: str, *args):
        return self.db.execute(sql, args).fetchall()

    # -- units ------------------------------------------------------------

    def unit(self, uom: int | None):
        if uom is None:
            return None
        uom = UOM_ALIAS.get(uom, uom)
        assert uom not in SEXAGESIMAL, (
            f"uom {uom} is a sexagesimal packing and must be decoded with angle() "
            f"before rendering; emitting it verbatim would silently corrupt the value"
        )
        if uom in self._unit_cache:
            return self._unit_cache[uom]
        row = self.one("select * from epsg_unitofmeasure where uom_code=?", uom)
        if row is None:
            return None
        name = row["unit_of_meas_name"]
        if name in UNIT_SHORTHAND:
            out = name
        else:
            factor = self.unit_factor(uom)
            assert factor is not None, f"uom {uom} ({name}) has no conversion factor"
            out = {
                "type": UNIT_TYPE.get(row["unit_of_meas_type"], "Unit"),
                "name": name,
                "conversion_factor": factor,
            }
        self._unit_cache[uom] = out
        return out

    def unit_factor(self, uom: int | None) -> float | None:
        """Conversion factor to the unit system's base (metre / radian / unity)."""
        if uom is None:
            return None
        uom = UOM_ALIAS.get(uom, uom)
        if uom in SEXAGESIMAL:
            return None
        if uom in DEGREE_FORMAT:
            uom = DEGREE  # a way of writing degrees, so: the degree factor
        row = self.one("select * from epsg_unitofmeasure where uom_code=?", uom)
        if row is None or row["factor_b"] is None or not row["factor_c"]:
            return None
        return row["factor_b"] / row["factor_c"]

    def unit_type(self, uom: int | None) -> str | None:
        if uom is None:
            return None
        row = self.one("select * from epsg_unitofmeasure where uom_code=?", UOM_ALIAS.get(uom, uom))
        return row["unit_of_meas_type"] if row else None

    # -- building blocks --------------------------------------------------

    def ellipsoid(self, code: int) -> dict:
        e = self.one("select * from epsg_ellipsoid where ellipsoid_code=?", code)
        unit = self.unit(e["uom_code"])
        sma = e["semi_major_axis"]

        def measure(v):
            return v if unit == "metre" else {"value": v, "unit": unit}

        # ellipsoid_shape 0 marks a sphere; PROJJSON spells those as a radius.
        if not e["ellipsoid_shape"]:
            out = {"name": e["ellipsoid_name"], "radius": measure(sma)}
        else:
            out = {"name": e["ellipsoid_name"], "semi_major_axis": measure(sma)}
            if e["inv_flattening"]:
                out["inverse_flattening"] = e["inv_flattening"]
            else:
                out["semi_minor_axis"] = measure(e["semi_minor_axis"])
        out["id"] = {"authority": "EPSG", "code": code}
        return out

    def prime_meridian(self, code: int) -> dict | None:
        # 8901 is Greenwich; PROJJSON leaves it implicit.
        if code is None or code == 8901:
            return None
        pm = self.one("select * from epsg_primemeridian where prime_meridian_code=?", code)
        value, uom = angle(pm["greenwich_longitude"], pm["uom_code"])
        unit = self.unit(uom)
        return {
            "name": pm["prime_meridian_name"],
            "longitude": value if unit == "degree" else {"value": value, "unit": unit},
            "id": {"authority": "EPSG", "code": code},
        }

    def datum(self, code: int) -> tuple[str | None, dict | None]:
        """Returns the PROJJSON key ("datum" or "datum_ensemble") and its object."""
        d = self.one("select * from epsg_datum where datum_code=?", code)
        if d is not None and d["datum_type"] == "ensemble":
            return self.datum_ensemble(code, d["datum_name"])
        if d is None:
            return self.datum_ensemble(code, None)

        kind = {
            "geodetic": "GeodeticReferenceFrame",
            "dynamic geodetic": "DynamicGeodeticReferenceFrame",
            "vertical": "VerticalReferenceFrame",
            "dynamic vertical": "DynamicVerticalReferenceFrame",
            "engineering": "EngineeringDatum",
        }.get(d["datum_type"], "GeodeticReferenceFrame")
        out = {"type": kind, "name": d["datum_name"]}
        if kind.startswith("Dynamic") and d["frame_reference_epoch"] is not None:
            out["frame_reference_epoch"] = d["frame_reference_epoch"]
        if d["ellipsoid_code"]:
            out["ellipsoid"] = self.ellipsoid(d["ellipsoid_code"])
        pm = self.prime_meridian(d["prime_meridian_code"])
        if pm:
            out["prime_meridian"] = pm
        out["id"] = {"authority": "EPSG", "code": code}
        return "datum", out

    def datum_ensemble(self, code: int, name: str | None) -> tuple[str | None, dict | None]:
        e = self.one("select * from epsg_datumensemble where datum_ensemble_code=?", code)
        if e is None:
            return None, None
        members = self.all(
            "select * from epsg_datumensemblemember where datum_ensemble_code=? order by datum_sequence", code
        )
        out = {
            "name": name or e["datum_ensemble_name"],
            "members": [
                {
                    "name": self.one("select datum_name from epsg_datum where datum_code=?", m["datum_code"])[0],
                    "id": {"authority": "EPSG", "code": m["datum_code"]},
                }
                for m in members
            ],
            "accuracy": str(e["ensemble_accuracy"]),
        }
        # The ellipsoid is a property of the members; they share one.
        if members:
            first = self.one("select * from epsg_datum where datum_code=?", members[0]["datum_code"])
            if first and first["ellipsoid_code"]:
                out["ellipsoid"] = self.ellipsoid(first["ellipsoid_code"])
        out["id"] = {"authority": "EPSG", "code": code}
        return "datum_ensemble", out

    def axes(self, cs_code: int) -> list[dict]:
        out = []
        for a in self.all(
            "select * from epsg_coordinateaxis where coord_sys_code=? order by coord_axis_order", cs_code
        ):
            named = self.one(
                "select * from epsg_coordinateaxisname where coord_axis_name_code=?", a["coord_axis_name_code"]
            )
            axis = {"name": named["coord_axis_name"], "abbreviation": a["coord_axis_abbreviation"]}
            direction, meridian = split_orientation(a["coord_axis_orientation"])
            axis["direction"] = direction
            if meridian is not None:
                axis["meridian"] = {"longitude": meridian}
            unit = self.unit(a["uom_code"])
            if unit:
                axis["unit"] = unit
            out.append(axis)
        return out

    def coordinate_system(self, cs_code: int) -> dict:
        cs = self.one("select * from epsg_coordinatesystem where coord_sys_code=?", cs_code)
        return {"subtype": cs["coord_sys_type"], "axis": self.axes(cs_code)}

    def conversion(self, code: int) -> dict:
        op = self.one("select * from epsg_coordoperation where coord_op_code=?", code)
        method = self.one(
            "select * from epsg_coordoperationmethod where coord_op_method_code=?", op["coord_op_method_code"]
        )
        out = {
            "name": op["coord_op_name"],
            "method": {
                "name": method["coord_op_method_name"],
                "id": {"authority": "EPSG", "code": op["coord_op_method_code"]},
            },
            "parameters": [],
        }
        rows = self.all(
            """select v.parameter_value, v.uom_code, v.parameter_code, p.parameter_name
                 from epsg_coordoperationparamvalue v
                 join epsg_coordoperationparam p on p.parameter_code = v.parameter_code
                 join epsg_coordoperationparamusage u
                      on u.parameter_code = v.parameter_code
                     and u.coord_op_method_code = v.coord_op_method_code
                where v.coord_op_code = ?
                order by u.sort_order""",
            code,
        )
        for p in rows:
            value, uom = angle(p["parameter_value"], p["uom_code"])
            out["parameters"].append(
                {
                    "name": p["parameter_name"],
                    "value": value,
                    "unit": self.unit(uom),
                    "id": {"authority": "EPSG", "code": p["parameter_code"]},
                }
            )
        return out


ORIENTATION = re.compile(r"^(North|South) along (\d+(?:\.\d+)?)°([EW])$")


def split_orientation(orientation: str) -> tuple[str, float | None]:
    """`"North along 90°E"` is a direction plus a meridian in PROJJSON."""
    m = ORIENTATION.match(orientation)
    if not m:
        return orientation, None
    longitude = float(m.group(2)) * (1 if m.group(3) == "E" else -1)
    return m.group(1).lower(), longitude


# --------------------------------------------------------------------------
# PROJJSON
# --------------------------------------------------------------------------

PROJJSON_SCHEMA = "https://proj.org/schemas/v0.7/projjson.schema.json"

# EPSG's `coord_ref_sys_kind` -> PROJJSON `type`.  "derived" and datum-less
# verticals are resolved in `render_crs`.
CRS_TYPE = {
    "projected": "ProjectedCRS",
    "geographic 2D": "GeographicCRS",
    "geographic 3D": "GeographicCRS",
    "geocentric": "GeodeticCRS",
    "vertical": "VerticalCRS",
    "compound": "CompoundCRS",
    "engineering": "EngineeringCRS",
}

# Only these take their definition from a base CRS plus a conversion.  For
# everything else EPSG still populates `base_crs_code` (a geographic 2D CRS
# points at its 3D parent) and `projection_conv_code`, which must be ignored:
# a GeographicCRS carrying a `conversion` is not valid PROJJSON.
DERIVED_KINDS = {"projected", "derived"}


def is_derived_vertical(e: Epsg, code: int) -> bool:
    row = e.one("select * from epsg_coordinatereferencesystem where coord_ref_sys_code=?", code)
    return bool(
        row
        and row["coord_ref_sys_kind"] == "vertical"
        and row["base_crs_code"]
        and row["projection_conv_code"]
    )


def root_vertical_datum(e: Epsg, code: int) -> int:
    """Walk a vertical derivation chain back to the CRS that owns the datum."""
    seen = set()
    while code not in seen:
        seen.add(code)
        row = e.one("select * from epsg_coordinatereferencesystem where coord_ref_sys_code=?", code)
        if row is None:
            break
        if not is_derived_vertical(e, code) and row["datum_code"]:
            return row["datum_code"]
        if not row["base_crs_code"]:
            break
        code = row["base_crs_code"]
    raise ValueError(f"EPSG:{code} is a vertical chain with no datum at its root")


def render_crs(e: Epsg, code: int, *, root: bool = True) -> dict:
    c = e.one("select * from epsg_coordinatereferencesystem where coord_ref_sys_code=?", code)
    kind = c["coord_ref_sys_kind"]

    out: dict = {}
    if root:
        out["$schema"] = PROJJSON_SCHEMA
    out["type"] = CRS_TYPE.get(kind, "ProjectedCRS")
    out["name"] = c["coord_ref_sys_name"]

    if kind == "compound":
        out["components"] = [
            render_crs(e, c["cmpd_horizcrs_code"], root=False),
            render_crs(e, c["cmpd_vertcrs_code"], root=False),
        ]
        out["id"] = {"authority": "EPSG", "code": code}
        return out

    derived = kind in DERIVED_KINDS
    if kind == "vertical" and c["base_crs_code"] and c["projection_conv_code"]:
        # "Height Depth Reversal" / "Change of Vertical Unit": a vertical CRS
        # defined from another one rather than straight from a datum.  EPSG also
        # denormalises the base's datum onto these rows, so the presence of
        # datum_code says nothing -- the conversion is what makes it derived.
        if is_derived_vertical(e, c["base_crs_code"]):
            # ...unless the base is *itself* derived, e.g. EPSG:8051
            # "MSL depth (ft)" <- 5715 "MSL depth" <- 5714 "MSL height".
            # PROJJSON's derived_vertical_crs requires a plain VerticalCRS as
            # its base, so a two-step chain cannot be expressed; EPSG's own WKT
            # export gives up on these four CRSs for the same reason ("WKT is
            # not supported for vertical CRS defined by...").  Collapse to the
            # root datum, as PROJ does.  Nothing is lost: the coordinate system
            # already carries what distinguishes this CRS, its axis direction
            # and unit.
            out["type"] = "VerticalCRS"
            key, value = e.datum(root_vertical_datum(e, code))
            if key:
                out[key] = value
            if c["coord_sys_code"]:
                out["coordinate_system"] = e.coordinate_system(c["coord_sys_code"])
            out["id"] = {"authority": "EPSG", "code": code}
            return out
        out["type"] = "DerivedVerticalCRS"
        derived = True

    if derived:
        # The datum belongs to the base CRS; a derived CRS must not repeat it.
        if c["base_crs_code"]:
            out["base_crs"] = render_crs(e, c["base_crs_code"], root=False)
        if c["projection_conv_code"]:
            out["conversion"] = e.conversion(c["projection_conv_code"])
    elif c["datum_code"]:
        key, value = e.datum(c["datum_code"])
        if key:
            out[key] = value
    if c["coord_sys_code"]:
        out["coordinate_system"] = e.coordinate_system(c["coord_sys_code"])
    out["id"] = {"authority": "EPSG", "code": code}
    return out


# --------------------------------------------------------------------------
# CRS facts table
# --------------------------------------------------------------------------

# Matches `FirstAxis` in xeibe-geom.
AXIS_EAST, AXIS_NORTH, AXIS_OTHER = 0, 1, 2

KIND_ORDINAL = {
    "projected": "Projected",
    "geographic 2D": "Geographic2d",
    "geographic 3D": "Geographic3d",
    "geocentric": "Geocentric",
    "vertical": "Vertical",
    "compound": "Compound",
    "engineering": "Engineering",
    "derived": "Derived",
}


def first_axis(direction: str) -> int:
    if direction == "east":
        return AXIS_EAST
    if direction == "north":
        return AXIS_NORTH
    return AXIS_OTHER


def crs_record(e: Epsg, row: sqlite3.Row) -> dict:
    """The facts the axis-order decision and the range check need."""
    code = row["coord_ref_sys_code"]
    kind = row["coord_ref_sys_kind"]

    axes: list[dict] = []
    extra_dimensions = 0
    if row["coord_sys_code"]:
        axes = e.axes(row["coord_sys_code"])
    elif kind == "compound":
        # The horizontal part decides the axis order; the dimension is the sum
        # (`docs/geometry.md`, "Edge cases": only the first two are ever swapped).
        for part, is_horizontal in ((row["cmpd_horizcrs_code"], True), (row["cmpd_vertcrs_code"], False)):
            if not part:
                continue
            component = e.one(
                "select * from epsg_coordinatereferencesystem where coord_ref_sys_code=?", part
            )
            if not (component and component["coord_sys_code"]):
                continue
            part_axes = e.axes(component["coord_sys_code"])
            if is_horizontal:
                axes = part_axes
            else:
                extra_dimensions += len(part_axes)

    direction = first_axis(axes[0]["direction"]) if axes else AXIS_OTHER

    # The linear unit matters for ArcByCenterPoint radii; the angular one tells
    # a geographic CRS in gradians from one in degrees.
    linear = angular = None
    if row["coord_sys_code"] and axes:
        uom = e.one(
            "select uom_code from epsg_coordinateaxis where coord_sys_code=? order by coord_axis_order limit 1",
            row["coord_sys_code"],
        )
        if uom:
            factor, utype = e.unit_factor(uom["uom_code"]), e.unit_type(uom["uom_code"])
            if utype == "length":
                linear = factor
            elif utype == "angle":
                angular = factor

    # EPSG's area of use, verbatim, in WGS84 degrees.  Deliberately not
    # reprojected into the CRS's own units: that would need a projection engine
    # and the result would be ours, not EPSG's.
    extent = e.one(
        """select e.bbox_south_bound_lat, e.bbox_west_bound_lon,
                  e.bbox_north_bound_lat, e.bbox_east_bound_lon
             from epsg_usage u join epsg_extent e on e.extent_code = u.extent_code
            where u.object_table_name = 'epsg_coordinatereferencesystem' and u.object_code = ?
            order by u.usage_code limit 1""",
        code,
    )
    area = None
    if extent and extent[0] is not None:
        area = [float(v) for v in extent]

    return {
        "code": code,
        "name": row["coord_ref_sys_name"],
        "kind": KIND_ORDINAL.get(kind, "Engineering"),
        "first_axis": direction,
        "dimension": max(len(axes) + extra_dimensions, 1),
        "linear_unit_m": linear,
        "angular_unit_rad": angular,
        "area_wgs84": area,
        "deprecated": bool(row["deprecated"]),
    }


# --------------------------------------------------------------------------
# Emitting
# --------------------------------------------------------------------------

SHARD = 64  # CRSs per zstd frame: ~115 KiB decompressed, ~700 KiB total
PROJJSON_MAGIC = b"XPJ1"


def pack_projjson(docs: dict[int, dict]) -> bytes:
    """A flat container: a code index, then zstd frames of newline-joined docs.

    Looking one CRS up decompresses a single frame, not the whole 14 MB.
    """
    codes = sorted(docs)
    compressor = zstd.ZstdCompressor(level=19)
    shards, index, blob = [], [], bytearray()
    for start in range(0, len(codes), SHARD):
        group = codes[start : start + SHARD]
        raw = b"\n".join(json.dumps(docs[c], separators=(",", ":"), ensure_ascii=False).encode() for c in group)
        frame = compressor.compress(raw)
        shards.append((len(blob), len(frame), len(raw)))
        for line, c in enumerate(group):
            index.append((c, start // SHARD, line))
        blob += frame

    out = bytearray()
    out += PROJJSON_MAGIC
    out += struct.pack("<II", len(shards), len(index))
    for offset, comp_len, raw_len in shards:
        out += struct.pack("<III", offset, comp_len, raw_len)
    for code, shard, line in index:
        out += struct.pack("<IHH", code, shard, line)
    out += blob
    return bytes(out)


def rust_string(s: str) -> str:
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def read_previous_table(table_rs: Path) -> tuple[str | None, set[int]]:
    """The EPSG version and CRS codes already committed, before we overwrite."""
    if not table_rs.exists():
        return None, set()
    text = table_rs.read_text()
    version = re.search(r'pub const EPSG_VERSION: &str = "([^"]+)"', text)
    codes = {int(c) for c in re.findall(r"CrsRecord \{ code: (\d+)", text)}
    return (version.group(1) if version else None), codes


CHANGELOG_HEADER = """\
# Changelog

Releases of the EPSG Geodetic Parameter Dataset embedded in this crate, newest
first. Generated by `scripts/gen_crs_tables.py`; the release notes and change
requests are EPSG's own words, from the dataset's `epsg_versionhistory` and
`epsg_change` tables.
"""

CHANGE_REQUEST = re.compile(r"\b(\d{4}\.\d{3})\b")


def changelog_entry(e: Epsg, version: str, date: str, remark: str, measured: str | None) -> str:
    """One release: EPSG's note, the change requests behind it, our diff."""
    lines = [f"## EPSG v{version} — {date}", ""]
    if remark:
        lines += [remark.strip(), ""]

    # The remark names its change requests ("Includes change requests 2026.075
    # and 2026.138"); epsg_change says what each actually was.
    requests = []
    for change_id in dict.fromkeys(CHANGE_REQUEST.findall(remark or "")):
        row = e.one("select * from epsg_change where change_id = ?", float(change_id))
        if row is None:
            requests.append(f"- `{change_id}`")
            continue
        text = (row["request"] or "").strip().rstrip(".")
        reporter = (row["reporter"] or "").strip()
        requests.append(f"- `{change_id}` {text}" + (f" _({reporter})_" if reporter else ""))
    if requests:
        lines += ["### Change requests", ""] + requests + [""]

    if measured:
        lines += ["### Effect on this crate", "", measured, ""]
    return "\n".join(lines)


def update_changelog(
    path: Path, e: Epsg, version: str, date: str, previous: str | None, measured: str | None
) -> int:
    """Prepend any EPSG releases not yet documented. Idempotent.

    Every release between the one previously committed and this one gets an
    entry, so skipping a few releases still produces a complete history.
    """
    existing = path.read_text() if path.exists() else ""
    documented = set(re.findall(r"^## EPSG v(\S+)", existing, re.M))
    if version in documented:
        return 0

    releases = e.all(
        "select version_number, version_date, version_remarks from epsg_versionhistory order by version_date"
    )
    # Everything after the previously committed release, up to this one.
    after_previous = previous is None
    pending = []
    for row in releases:
        number = str(row["version_number"])
        if number == previous:
            after_previous = True
            continue
        if not after_previous or number in documented:
            continue
        pending.append(row)
        if number == version:
            break

    if not pending:
        pending = [r for r in releases if str(r["version_number"]) == version]
    if not pending:
        return 0

    entries = [
        changelog_entry(
            e,
            str(row["version_number"]),
            str(row["version_date"])[:10],
            row["version_remarks"] or "",
            measured if str(row["version_number"]) == version else None,
        )
        for row in reversed(pending)
    ]

    body = existing[len(CHANGELOG_HEADER) :].lstrip("\n") if existing.startswith(CHANGELOG_HEADER) else existing
    path.write_text(CHANGELOG_HEADER + "\n" + "\n".join(entries) + ("\n" + body if body.strip() else ""))
    return len(pending)


def update_crate_version(cargo_toml: Path, epsg: str) -> str:
    """Record the EPSG version as semver *build metadata*: `0.1.0+epsg-13.103`.

    The semver core stays hand-maintained and keeps meaning what semver says it
    means -- API compatibility -- while the build metadata, which Cargo shows
    but ignores when resolving, carries the data provenance.  `curl-sys` and
    `libgit2-sys` version their bundled sources the same way.

    Mapping EPSG's own numbers onto major.minor does not work: `13.005` is not
    a legal semver minor (Cargo: "invalid leading zero in minor version
    number"), `12.059a` and `12.059b` are real EPSG versions with letters in
    them, and v13 runs two parallel streams (13.001-13.005 alongside
    13.101-13.103, with 13.005 and 13.101 released the same day), so the minor
    can move backwards -- which Cargo will not let you publish.
    """
    text = cargo_toml.read_text()
    metadata = "+epsg-" + re.sub(r"[^0-9A-Za-z.]", "-", epsg)

    if re.search(r"(?m)^version\.workspace\s*=\s*true\s*$", text):
        # First run: this crate stops inheriting the workspace version, because
        # its build metadata has to vary independently.
        new = f'version = "0.1.0{metadata}"'
        text = re.sub(r"(?m)^version\.workspace\s*=\s*true\s*$", new, text, count=1)
    else:
        text = re.sub(
            r'(?m)^version\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+)(?:\+[^"]*)?"',
            lambda m: f'version = "{m.group(1)}{metadata}"',
            text,
            count=1,
        )
    cargo_toml.write_text(text)
    return re.search(r'(?m)^version\s*=\s*"([^"]+)"', text).group(1)


def opt_f32(v) -> str:
    return "None" if v is None else f"Some({v!r}f32)"


def emit_table(records: list[dict], version: str, date: str, counts: dict) -> str:
    lines = [
        "// @generated by scripts/gen_crs_tables.py -- do not edit.",
        "//",
        f"// Source: EPSG Geodetic Parameter Dataset v{version} ({date}).",
        "// The EPSG Dataset is owned by IOGP and used under the EPSG Dataset Terms",
        "// of Use; see EPSG-NOTICE.md next to this crate's Cargo.toml.",
        "",
        "use crate::{CrsKind, CrsRecord, FirstAxis};",
        "",
        f"/// EPSG Dataset version this table was generated from.",
        f"pub const EPSG_VERSION: &str = {rust_string(version)};",
        f"/// Publication date of that EPSG Dataset version.",
        f"pub const EPSG_DATE: &str = {rust_string(date)};",
        "",
        "/// Every CRS in the EPSG Dataset, sorted by code.",
        "pub static CRS: &[CrsRecord] = &[",
    ]
    for r in records:
        area = (
            "None"
            if r["area_wgs84"] is None
            else "Some([{}f32, {}f32, {}f32, {}f32])".format(*(repr(v) for v in r["area_wgs84"]))
        )
        lines.append(
            "    CrsRecord {{ code: {code}, name: {name}, kind: CrsKind::{kind}, "
            "first_axis: FirstAxis::{axis}, dimension: {dim}, linear_unit_m: {lin}, "
            "angular_unit_rad: {ang}, area_wgs84: {area}, deprecated: {dep} }},".format(
                code=r["code"],
                name=rust_string(r["name"]),
                kind=r["kind"],
                axis=("EastOrLon", "NorthOrLat", "Other")[r["first_axis"]],
                dim=r["dimension"],
                lin=opt_f32(r["linear_unit_m"]),
                ang=opt_f32(r["angular_unit_rad"]),
                area=area,
                dep="true" if r["deprecated"] else "false",
            )
        )
    lines += [
        "];",
        "",
        f"/// Number of CRSs that also carry a PROJJSON definition.",
        f"pub const PROJJSON_COUNT: usize = {counts['projjson']};",
        "",
    ]
    return "\n".join(lines)


NOTICE = """\
# EPSG Dataset notice

`xeibe-crs` contains data extracted from the **EPSG Geodetic Parameter
Dataset** v{version} ({date}), published by the International Association of
Oil and Gas Producers (IOGP) at <https://epsg.org/>.

**Ownership of the EPSG Dataset by IOGP is hereby acknowledged.**

The EPSG Dataset is used here under the EPSG Dataset Terms of Use,
<https://epsg.org/terms-of-use.html>. Those terms permit extracting subsets of
the data and redistributing them, and they require that:

- IOGP's ownership is acknowledged in any onward publication or transmission,
  including of permitted modifications;
- anyone to whom this data is passed on is informed of the Terms of Use --
  which is what this file is for;
- the data is not distributed for profit, and any commercial packaging derives
  its value from what the provider adds rather than from the Dataset itself;
- data modified other than as the Terms permit is not attributed to the EPSG
  Dataset.

## What was extracted, and how

`scripts/gen_crs_tables.py` reads the official EPSG "PostgreSQL scripts"
release and emits:

- `src/table.rs` -- for each CRS: code, name, kind, first axis direction,
  dimension, axis units and EPSG's published area of use.
- `data/projjson.bin` -- a PROJJSON encoding of each CRS definition.

Values are reproduced as EPSG publishes them, with two representation changes
that the Terms permit because they preserve numeric equivalence:

1. Angles stored in EPSG's sexagesimal packings (units of measure 9110
   `DDD.MMSSsss` and 9111 `DDD.MMm`) are converted to decimal degrees, and
   relabelled as degrees. For example EPSG:2009 stores a longitude of natural
   origin of `-58.3`, meaning -58°30', which is emitted as `-58.5` degrees.
2. Unit of measure 9122, "degree (supplier to define representation)", is
   emitted as unit of measure 9102, "degree".

No coordinate operation parameters, ellipsoid parameters or areas of use are
otherwise recalculated, re-fitted or reprojected. In particular the area of use
is EPSG's own WGS 84 bounding box, not a reprojection of it.

## Licensing of this crate

The xeibe source code in this crate is under the workspace licence. The EPSG
data it embeds is **not** placed under that licence and is not in the public
domain; it remains IOGP's, under the Terms of Use linked above. The crate's
`license` field records this as a combined expression.
"""


# --------------------------------------------------------------------------
# Differential check against EPSG's own WKT
# --------------------------------------------------------------------------
#
# The WKT release is EPSG rendering the same database themselves, and crucially
# it is already normalised: decimal degrees, no sexagesimal packing.  Diffing
# against it catches the class of mistake that yields plausible-but-wrong
# numbers rather than an error -- which is the only class that matters here.
# It covers non-deprecated CRSs only; EPSG does not publish WKT for the rest.

WKT_TOKEN = re.compile(
    r"\s*(?:([A-Za-z][A-Za-z0-9_]*)\s*\["  # keyword[
    r'|(")((?:[^"]|"")*)"'  # "quoted string"
    r"|([-+]?[0-9]*\.?[0-9]+(?:[eE][-+]?\d+)?)"  # number
    r"|(\])|(,)"
    r"|([A-Za-z][A-Za-z0-9_ °′″.+-]*))"  # bare word, e.g. CS[Cartesian,2]
)


def parse_wkt(text: str, pos: int = 0):
    """Just enough WKT2 to read EPSG's CRS files: `KEYWORD[arg, ...]`."""
    m = WKT_TOKEN.match(text, pos)
    if not m or m.group(1) is None:
        raise ValueError(f"expected a keyword at {pos}: {text[pos : pos + 40]!r}")
    node = {"kw": m.group(1), "args": []}
    pos = m.end()
    while True:
        m = WKT_TOKEN.match(text, pos)
        if not m:
            raise ValueError(f"unparsable at {pos}: {text[pos : pos + 40]!r}")
        keyword, quote, body, number, close, comma, bare = m.groups()
        if close:
            return node, m.end()
        if comma:
            pos = m.end()
        elif quote is not None:
            node["args"].append(body.replace('""', '"'))
            pos = m.end()
        elif number is not None:
            node["args"].append(float(number))
            pos = m.end()
        elif bare is not None:
            node["args"].append({"bare": bare.strip()})
            pos = m.end()
        else:
            child, pos = parse_wkt(text, pos)
            node["args"].append(child)


def wkt_facts(node: dict) -> dict:
    """The numbers worth comparing: projection parameters and axis directions."""

    def kids(n, kw):
        return [a for a in n["args"] if isinstance(a, dict) and a.get("kw") == kw]

    facts: dict = {}
    # A derived CRS spells its conversion DERIVINGCONVERSION; both carry the
    # same METHOD and PARAMETER nodes.
    conv = kids(node, "CONVERSION") or kids(node, "DERIVINGCONVERSION")
    if conv:
        facts["params"] = sorted(
            (p["args"][0], round(float(p["args"][1]), 9)) for p in kids(conv[0], "PARAMETER")
        )
        facts["method"] = kids(conv[0], "METHOD")[0]["args"][0]
    axes = kids(node, "AXIS")
    if axes:
        facts["axes"] = [
            (a["args"][1]["bare"] if isinstance(a["args"][1], dict) else a["args"][1]).lower() for a in axes
        ]
    return facts


def json_facts(doc: dict) -> dict:
    facts: dict = {}
    conv = doc.get("conversion")
    if conv:
        facts["params"] = sorted((p["name"], round(float(p["value"]), 9)) for p in conv["parameters"])
        facts["method"] = conv["method"]["name"]
    cs = doc.get("coordinate_system")
    if cs:
        facts["axes"] = [a["direction"].lower() for a in cs["axis"]]
    return facts


def wkt_check(wkt_zip: Path, docs: dict[int, dict]) -> int:
    """Returns the number of mismatches, printing the first few."""
    with zipfile.ZipFile(wkt_zip) as z:
        names = {
            int(m.group(1)): n
            for n in z.namelist()
            if (m := re.match(r"EPSG-CRS-(\d+)\.wkt$", Path(n).name))
        }
        compared = mismatched = unparsed = 0
        shown = 0
        for code, name in sorted(names.items()):
            doc = docs.get(code)
            if doc is None:
                continue
            text = z.read(name).decode("utf-8").strip()
            try:
                tree, _ = parse_wkt(text)
            except ValueError:
                unparsed += 1  # EPSG ships prose where WKT is unsupported
                continue
            ours, theirs = json_facts(doc), wkt_facts(tree)
            differing = [k for k in set(ours) | set(theirs) if ours.get(k) != theirs.get(k)]
            compared += 1
            if differing:
                mismatched += 1
                if shown < 5:
                    shown += 1
                    key = differing[0]
                    print(f"  MISMATCH EPSG:{code} {key}")
                    print(f"    ours: {str(ours.get(key))[:150]}")
                    print(f"    EPSG: {str(theirs.get(key))[:150]}")
    print(f"  WKT cross-check: {compared - mismatched}/{compared} agree ({unparsed} files are not WKT)")
    return mismatched


# --------------------------------------------------------------------------
# Main
# --------------------------------------------------------------------------


def find_sql_zip() -> Path:
    candidates = sorted((REPO / "example_data").glob("EPSG-*PostgreSQL.zip"))
    if not candidates:
        sys.exit(
            "no EPSG PostgreSQL archive found in example_data/.\n"
            "Download one from https://epsg.org/download-dataset.html (free account required)."
        )
    return candidates[-1]


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--sql", type=Path, help="EPSG PostgreSQL scripts zip")
    ap.add_argument("--wkt", type=Path, help="EPSG WKT zip, used as a differential check")
    ap.add_argument("--out", type=Path, default=CRATE, help="crate directory to write")
    ap.add_argument(
        "--no-check", action="store_true", help="skip the WKT cross-check even if the WKT archive is present"
    )
    args = ap.parse_args()

    sql_zip = args.sql or find_sql_zip()
    print(f"reading {sql_zip.relative_to(REPO) if sql_zip.is_relative_to(REPO) else sql_zip}")

    with tempfile.TemporaryDirectory() as tmp:
        db = load_epsg(sql_zip, Path(tmp) / "epsg.db")
        version, date = epsg_version(db)
        print(f"EPSG Dataset v{version} ({date})")
        e = Epsg(db)

        rows = e.all("select * from epsg_coordinatereferencesystem order by coord_ref_sys_code")
        records, docs = [], {}
        skipped = Counter()
        failures = defaultdict(list)

        for row in rows:
            code = row["coord_ref_sys_code"]
            if EXAMPLE_NAME.match(row["coord_ref_sys_name"]):
                skipped["EPSG worked example, not a real CRS"] += 1
                continue
            records.append(crs_record(e, row))
            try:
                docs[code] = render_crs(e, code)
            except Exception as exc:  # noqa: BLE001 - reported, not swallowed
                failures[f"{type(exc).__name__}: {exc}"].append(code)

        print(f"CRS records: {len(records)}   PROJJSON documents: {len(docs)}")
        for reason, n in skipped.items():
            print(f"  skipped {n}: {reason}")
        for reason, codes in failures.items():
            print(f"  FAILED {len(codes)}: {reason} (e.g. {codes[:5]})")

        wkt_zip = args.wkt
        if wkt_zip is None and not args.no_check:
            found = sorted((REPO / "example_data").glob("EPSG-*WKT*.[Zz]ip"))
            wkt_zip = found[-1] if found else None
        if wkt_zip and not args.no_check:
            if wkt_check(wkt_zip, docs):
                sys.exit("EPSG's own WKT disagrees with the rendered PROJJSON; refusing to write.")
        elif not args.no_check:
            print("  (no WKT archive found; skipping the cross-check)")

        if failures:
            sys.exit("some CRSs failed to render; refusing to write.")

        out = args.out
        (out / "data").mkdir(parents=True, exist_ok=True)
        (out / "src").mkdir(parents=True, exist_ok=True)

        # Read what is already committed before overwriting it, so the
        # changelog can say what actually changed.
        previous_version, previous_codes = read_previous_table(out / "src" / "table.rs")
        current_codes = {r["code"] for r in records}
        added = sorted(current_codes - previous_codes)
        removed = sorted(previous_codes - current_codes)
        measured = None
        if previous_codes:
            measured = f"{len(records)} CRSs"
            if added or removed:
                measured += f" ({len(added)} added, {len(removed)} removed)"
                if removed:
                    shown = ", ".join(f"`EPSG:{c}`" for c in removed[:10])
                    measured += f"\n\nRemoved: {shown}" + (" …" if len(removed) > 10 else "")
            else:
                measured += ", unchanged"

        blob = pack_projjson(docs)
        (out / "data" / "projjson.bin").write_bytes(blob)
        table = emit_table(records, version, date, {"projjson": len(docs)})
        (out / "src" / "table.rs").write_text(table)
        (out / "EPSG-NOTICE.md").write_text(NOTICE.format(version=version, date=date))

        print(f"wrote {out / 'src' / 'table.rs'}  ({len(table) / 1024:.0f} KiB)")
        print(f"wrote {out / 'data' / 'projjson.bin'}  ({len(blob) / 1024:.0f} KiB)")
        print(f"wrote {out / 'EPSG-NOTICE.md'}")

        written = update_changelog(out / "CHANGELOG.md", e, version, date, previous_version, measured)
        if written:
            print(f"wrote {out / 'CHANGELOG.md'}  ({written} release(s) added)")

        cargo_toml = out / "Cargo.toml"
        if cargo_toml.exists():
            print(f"crate version: {update_crate_version(cargo_toml, version)}")


if __name__ == "__main__":
    main()
