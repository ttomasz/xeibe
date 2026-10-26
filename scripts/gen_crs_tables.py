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
    crates/xeibe-crs/src/aliases.rs    EPSG's aliases that name one CRS, for srsName lookup
    crates/xeibe-crs/src/deprecations.rs  EPSG's replacements of deprecated CRSs
    crates/xeibe-crs/src/units.rs      EPSG's units of measure with their factors
    crates/xeibe-crs/data/projjson.bin zstd-sharded PROJJSON, addressed by code
    crates/xeibe-crs/data/operations.bin coordinate operations, geodetic CRSs, datums
                                       and areas of use, as PROJ imports them (one
                                       zstd frame of JSON)
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
        # The epoch at which a static frame was aligned to its parent; PROJ
        # writes it (WKT2 ANCHOREPOCH) where EPSG gives one.
        if d["anchor_epoch"] is not None and not kind.startswith("Engineering"):
            out["anchor_epoch"] = d["anchor_epoch"]
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


def identify(e: Epsg, out: dict, code: int, root: bool) -> dict:
    """Close a CRS's document with its id. Its areas of use and scopes are
    not written here but in `usages.bin` (`pack_usages`), and `xeibe-crs`
    adds them to the root CRS as PROJ writes them."""
    out["id"] = {"authority": "EPSG", "code": code}
    return out


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
        return identify(e, out, code, root)

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
            return identify(e, out, code, root)
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
    return identify(e, out, code, root)


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


def first_axis(axes: list[dict]) -> int:
    """Northing first when PROJ swaps the axes for visualization (pyproj's
    `always_xy`; `mustAxisOrderBeSwitchedForVisualization` in PROJ's
    `crs.cpp`): northing-easting, and of the polar axes along meridians only
    UPS North and South (N,E) and RSPS2000. The other polar CRSs, whose axes
    both point north (or south) along meridians, are as written: EPSG:3031's
    "E" points north along 90°E, its "N" north along 0°E."""
    if not axes:
        return AXIS_OTHER
    d0 = axes[0]["direction"]
    d1 = axes[1]["direction"] if len(axes) > 1 else None
    if d0 == "east":
        return AXIS_EAST
    if (d0, d1) == ("north", "east"):
        return AXIS_NORTH
    if d0 != d1 or d0 not in ("north", "south"):
        return AXIS_OTHER
    m0, m1 = (a.get("meridian", {}).get("longitude") for a in axes[:2])
    if m0 is None or m1 is None:
        return AXIS_OTHER
    swapped = {"north": [(0, 90), (180, -90)], "south": [(180, 90)]}[d0]
    if any(abs(m0 - a) < 1e-10 and abs(m1 - b) < 1e-10 for a, b in swapped):
        return AXIS_NORTH
    return AXIS_EAST


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

    direction = first_axis(axes)

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


def datum_and_method(doc: dict) -> tuple[int | None, int | None]:
    """The EPSG codes of a CRS's datum (or ensemble) and projection method,
    of its horizontal part for a compound CRS: what identifying a CRS from
    its definition narrows the candidates by (`docs/crs-wkt.md`)."""
    if doc.get("type") == "CompoundCRS":
        doc = doc["components"][0]
    method = doc.get("conversion", {}).get("method", {}).get("id", {}).get("code")
    while doc is not None:
        datum = doc.get("datum") or doc.get("datum_ensemble")
        if datum is not None:
            return datum.get("id", {}).get("code"), method
        doc = doc.get("base_crs")
    return None, method


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


def opt_f64(v) -> str:
    # repr() is the shortest text that reads back as the same double, so
    # EPSG's factors are kept exactly and no digits go to waste.
    return "None" if v is None else f"Some({v!r}f64)"


def opt_u32(value: int | None) -> str:
    return "None" if value is None else f"Some({value})"


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
        # One record per line is the format verify_crs_tables.py reads back, and
        # it keeps a release's diff to the records that changed.
        "#[rustfmt::skip]",
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
            "angular_unit_rad: {ang}, area_wgs84: {area}, datum: {datum}, method: {method}, "
            "deprecated: {dep} }},".format(
                code=r["code"],
                name=rust_string(r["name"]),
                kind=r["kind"],
                axis=("EastOrLon", "NorthOrLat", "Other")[r["first_axis"]],
                dim=r["dimension"],
                lin=opt_f64(r["linear_unit_m"]),
                ang=opt_f64(r["angular_unit_rad"]),
                area=area,
                datum=opt_u32(r.get("datum")),
                method=opt_u32(r.get("method")),
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


# --------------------------------------------------------------------------
# Coordinate operations
# --------------------------------------------------------------------------
#
# What `xeibe-reproject` needs to choose and run datum transformations
# (`docs/reprojection.md`, level 2): EPSG's transformations and concatenated
# operations, the geodetic CRSs and datums the choice pivots through, and the
# areas of use. The choice follows PROJ, so the set of operations follows
# PROJ's import of EPSG (`scripts/build_db.py` in PROJ 9.8.1): an operation PROJ
# doesn't import doesn't exist for PROJ's choice, and mustn't for ours.

# build_db.py `fill_helmert_transformation`.
HELMERT_METHODS = {
    1031, 1032, 1033, 1034, 1035, 1037, 1038, 1039, 1053, 1054, 1055, 1056, 1057, 1058,
    1061, 1062, 1063, 1065, 1066, 1132, 1133, 1140, 1149, 9603, 9606, 9607, 9636,
}  # fmt: skip
# build_db.py `fill_other_transformation`.
OTHER_METHODS = {9601, 9616, 9618, 9619, 9624, 9660, 1068, 1069, 1046, 1131, 1136, 9621, 9656, 1143}
# build_db.py `fill_grid_transformation`: methods selected by name.
GRID_METHOD_PREFIXES = ("Geographic3D to", "Geog3D to")
GRID_METHOD_LIKE = (
    re.compile(r"^Point motion .* grid"),
    re.compile(r"^Vertical Offset .*rid"),
    re.compile(r"^Geographic3D Offset .* velocity .*rid"),
)
GRID_METHOD_NAMES = {
    "NADCON", "NADCON5 (2D)", "NADCON5 (3D)", "NTv1", "NTv2", "VERTCON",
    "Geocentric translations (geog2D domain) by grid (IGN)",
    "Geocentric translations using NEU velocity grid (gtg)",
    "Geocen translations by grid (gtg) & Geocen translations NEU velocities (gtg)",
    "New Zealand Deformation Model", "Cartesian Grid Offsets by TIN Interpolation (JSON)",
    "Vertical Offset by TIN Interpolation (JSON)", "Geographic2D Offsets by TIN Interpolation (JSON)",
    "Vertical change by geoid grid difference (NRCan)",
}  # fmt: skip
GRID_SKIPPED = {10929}  # SOPAC deformation model for California v1, skipped by build_db.py
OPERATIONS_MAGIC = b"XOP1"


def ensemble_accuracy(e: Epsg, code: int) -> float:
    return float(e.one("select ensemble_accuracy from epsg_datumensemble where datum_ensemble_code=?", code)[0])


def operation_table(row: sqlite3.Row) -> str | None:
    """The proj.db table PROJ imports an operation into, or None if it doesn't."""
    method, name, kind = row["coord_op_method_code"], row["coord_op_method_name"] or "", row["coord_op_type"]
    if kind == "concatenated operation":
        return "concatenated_operation"
    if kind == "transformation" and method in HELMERT_METHODS:
        return "helmert_transformation"
    if kind in ("transformation", "point motion operation") and method not in (1131, 1136):
        if (
            name.startswith(GRID_METHOD_PREFIXES)
            or any(p.search(name) for p in GRID_METHOD_LIKE)
            or name in GRID_METHOD_NAMES
        ):
            return None if row["coord_op_code"] in GRID_SKIPPED else "grid_transformation"
    if method in OTHER_METHODS:
        if method in (1068, 1069) and row["source_crs_code"] is None and row["target_crs_code"] is None:
            return None
        return "other_transformation"
    return None


def publication_date(row: sqlite3.Row) -> str | None:
    """A datum's publication date the way PROJ derives it (build_db.py
    `compute_publication_date`): the frame reference epoch wins, then the
    publication date, padded to a full date."""
    epoch = row["frame_reference_epoch"]
    if epoch is not None:
        epoch = float(epoch)
        year, frac = int(epoch), epoch - int(epoch)
        if frac == 0:
            return f"{year:04d}-01-01"
        if abs(frac - 0.4) < 1e-6:
            return f"{year:04d}-05-01"
        if abs(frac - 0.5) < 1e-6:
            return f"{year:04d}-07-01"
        raise ValueError(f"datum {row['datum_code']}: unexpected epoch {epoch}")
    return epsg_publication_date(row)


def epsg_publication_date(row: sqlite3.Row) -> str | None:
    """EPSG's own publication date of a datum, padded to a full date: when
    a realization was published, which the frame reference epoch (1989.0
    for every ETRF) doesn't tell."""
    date = row["publication_date"] or ""
    if not date:
        return None
    if len(date) == 4:
        return date + "-01-01"
    if len(date) == 7:
        return date + "-01"
    if len(date) == 9:
        m = re.match(r"([0-9]{4})-([0-9]{4})", date)
        assert m, (row["datum_code"], date)
        return m.group(1)
    assert len(date) == 10, (row["datum_code"], date)
    return date


def operation_param(e: Epsg, row: sqlite3.Row) -> list:
    """One parameter as `[code, value]` in SI units (radians, metres, plain
    factors, seconds), or `[code, "file name"]` for a grid file."""
    code = row["parameter_code"]
    if row["param_value_file_ref"]:
        return [code, row["param_value_file_ref"]]
    value, uom = row["parameter_value"], row["uom_code"]
    if value is None:
        return [code, None]
    if uom in SEXAGESIMAL:
        value, uom = angle(value, uom)
    factor = e.unit_factor(uom) if uom is not None else 1.0
    if factor is None:
        raise ValueError(f"parameter {code}: unit {uom} has no conversion factor")
    return [code, value * factor]


def export_operations(e: Epsg, version: str) -> dict:
    """The coordinate-operation registry as one JSON document."""
    # Each object's areas of use, one per usage. xeibe-reproject uses the area
    # covering them all; PROJ takes only the first in its database's order.
    usages: dict[int, list[int]] = defaultdict(list)
    crs_usages: dict[int, list[int]] = defaultdict(list)
    # Each operation's scope: its first usage's, as PROJ reports it.
    scopes: dict[int, str] = {}
    for row in e.all(
        """select u.object_table_name, u.object_code, u.extent_code, s.scope
             from epsg_usage u left join epsg_scope s on s.scope_code = u.scope_code
            order by u.usage_code"""
    ):
        if row["object_table_name"] == "epsg_coordoperation":
            usages[row["object_code"]].append(row["extent_code"])
            if row["scope"]:
                scopes.setdefault(row["object_code"], row["scope"])
        elif row["object_table_name"] == "epsg_coordinatereferencesystem":
            crs_usages[row["object_code"]].append(row["extent_code"])
    # PROJ hard-codes WGS 84's domain as the world (factory.cpp).
    crs_usages[4326] = [1262]

    rows = e.all(
        "select o.*, m.coord_op_method_name from epsg_coordoperation o "
        "left join epsg_coordoperationmethod m using (coord_op_method_code) order by coord_op_code"
    )
    by_code = {r["coord_op_code"]: r for r in rows}
    tables = {code: operation_table(r) for code, r in by_code.items()}

    def params(code: int, method: int) -> list:
        return [
            operation_param(e, p)
            for p in e.all(
                "select v.*, u.sort_order from epsg_coordoperationparamvalue v "
                "join epsg_coordoperationparamusage u "
                "on u.coord_op_method_code = v.coord_op_method_code and u.parameter_code = v.parameter_code "
                "where v.coord_op_code=? and v.coord_op_method_code=? order by u.sort_order",
                code,
                method,
            )
        ]

    ops: dict[int, dict] = {}
    for code, row in by_code.items():
        table = tables[code]
        if table is None or table == "concatenated_operation":
            continue
        p = params(code, row["coord_op_method_code"])
        if table == "helmert_transformation" and row["deprecated"] and (not p or p[0][1] is None):
            continue  # build_db.py: "non sense deprecated transforms (like EPSG:1076)"
        ops[code] = operation_record(row, table, p, usages[code], scopes.get(code))

    # Concatenated operations exist for PROJ only when every step does;
    # conversions (e.g. 3D to 2D) count as steps too.
    conversions: dict[int, dict] = {}
    for code, row in by_code.items():
        if tables[code] != "concatenated_operation":
            continue
        steps = [
            r["single_operation_code"]
            for r in e.all(
                "select single_operation_code from epsg_coordoperationpath "
                "where concat_operation_code=? order by op_path_step",
                code,
            )
        ]
        if not steps:
            continue
        ok = True
        for step in steps:
            if step in ops:
                continue
            step_row = by_code.get(step)
            if step_row is not None and step_row["coord_op_type"] == "conversion":
                if step not in conversions:
                    conversions[step] = operation_record(
                        step_row,
                        "conversion",
                        params(step, step_row["coord_op_method_code"]),
                        usages[step],
                        scopes.get(step),
                    )
                continue
            ok = False
            break
        if ok:
            record = operation_record(row, "concatenated_operation", [], usages[code], scopes.get(code))
            record["steps"] = steps
            ops[code] = record
    ops.update(conversions)

    # Supersession as build_db.py `fill_supersession` records it.
    crs_names = {
        r["coord_ref_sys_code"]: r["coord_ref_sys_name"]
        for r in e.all("select coord_ref_sys_code, coord_ref_sys_name from epsg_coordinatereferencesystem")
    }
    for row in e.all(
        "select object_code, superseded_by from epsg_supersession "
        "where object_table_name='epsg_coordoperation' and object_code != superseded_by order by supersession_id"
    ):
        old, new = row["object_code"], row["superseded_by"]
        if old not in ops or new not in ops:
            continue
        a, b = by_code[old], by_code[new]
        # PROJ's build_db.py skips the three NAD27 to NAD83 supersessions
        # (NTv1 by NTv2 in Canada and Quebec) and raises the NTv1
        # operations' accuracy instead so that NTv2 wins; keeping EPSG's
        # supersession makes the same choice.
        same = (a["source_crs_code"], a["target_crs_code"]) == (b["source_crs_code"], b["target_crs_code"])
        if not same:
            name = lambda c: crs_names.get(c) or ""  # noqa: E731
            if a["source_crs_code"] == b["source_crs_code"] and name(a["target_crs_code"]).startswith(
                "ETRS89"
            ) and name(b["target_crs_code"]).startswith("ETRS89"):
                same = True
            elif a["target_crs_code"] == b["target_crs_code"] and name(a["source_crs_code"]).startswith(
                "ETRS89"
            ) and name(b["source_crs_code"]).startswith("ETRS89"):
                same = True
        ops[old].setdefault("superseded_by", []).append([new, same, ops[new]["table"]])

    crs_extent = {code: sorted(set(codes)) for code, codes in crs_usages.items() if codes}
    extents_used = sorted(
        {x for op in ops.values() for x in op["extents"]} | {x for codes in crs_extent.values() for x in codes}
    )
    extents = []
    for code in extents_used:
        x = e.one("select * from epsg_extent where extent_code=?", code)
        west, east = x["bbox_west_bound_lon"], x["bbox_east_bound_lon"]
        # build_db.py `fill_extent` wraps longitudes beyond ±180.
        if west is not None and west < -180:
            west += 360
        if east is not None and east > 180:
            east -= 360
        # The description is what PROJ reports as the area's name.
        extents.append([code, x["extent_description"], x["bbox_south_bound_lat"], west, x["bbox_north_bound_lat"], east])

    # Geodetic CRSs as build_db.py `fill_geodetic_crs` imports them.
    geodetic = [
        [r["coord_ref_sys_code"], r["coord_ref_sys_name"], r["coord_ref_sys_kind"], r["datum_code"], r["deprecated"]]
        for r in e.all(
            "select * from epsg_coordinatereferencesystem where coord_ref_sys_kind in "
            "('geographic 2D', 'geographic 3D', 'geocentric') and datum_code is not null "
            "and not (coord_ref_sys_code > 61000000 and deprecated) order by coord_ref_sys_code"
        )
    ]
    # Vertical CRSs as build_db.py `fill_vertical_crs` imports them: a CRS
    # derived from another (depth, another unit) takes its base's datum, or
    # its base's base's.
    vertical = []
    for r in e.all(
        "select * from epsg_coordinatereferencesystem where coord_ref_sys_kind='vertical' order by coord_ref_sys_code"
    ):
        datum, base = r["datum_code"], r["base_crs_code"]
        for _ in range(2):
            if datum is not None or base is None:
                break
            b = e.one("select datum_code, base_crs_code from epsg_coordinatereferencesystem where coord_ref_sys_code=?", base)
            datum, base = b["datum_code"], b["base_crs_code"]
        if datum is not None:
            vertical.append([r["coord_ref_sys_code"], r["coord_ref_sys_name"], datum, r["deprecated"]])
    datums = []
    for r in e.all(
        "select * from epsg_datum where datum_type in "
        "('geodetic', 'dynamic geodetic', 'vertical', 'dynamic vertical', 'ensemble') order by datum_code"
    ):
        members = None
        ellipsoid, pm = r["ellipsoid_code"], r["prime_meridian_code"]
        is_vertical = r["datum_type"] in ("vertical", "dynamic vertical")
        if r["datum_type"] == "ensemble":
            members = [
                m["datum_code"]
                for m in e.all(
                    "select datum_code from epsg_datumensemblemember where datum_ensemble_code=? order by datum_sequence",
                    r["datum_code"],
                )
            ]
            first = e.one("select * from epsg_datum where datum_code=?", members[0]) if members else None
            if first is None:
                continue
            is_vertical = first["datum_type"] in ("vertical", "dynamic vertical")
            ellipsoid, pm = first["ellipsoid_code"], first["prime_meridian_code"]
        datums.append(
            [
                r["datum_code"],
                r["datum_name"],
                None if members is not None else publication_date(r),
                ellipsoid,
                pm,
                members,
                r["deprecated"],
                # PROJ makes a datum with a frame reference epoch dynamic.
                r["frame_reference_epoch"] is not None,
                # An ensemble's accuracy: how far apart its members may be.
                ensemble_accuracy(e, r["datum_code"]) if members is not None else None,
                # The anchor epoch (decimal year): velocity-grid operations
                # move coordinates from the source's to the target's.
                float(r["anchor_epoch"]) if r["anchor_epoch"] is not None else None,
                # A vertical datum (or ensemble of them).
                is_vertical,
                # EPSG's publication date: the newer of two realizations.
                None if members is not None else epsg_publication_date(r),
            ]
        )
    # Names, for describing operations (`Transformer.to_json`): each method's,
    # and each parameter's with the kind of unit EPSG gives it (the values
    # above are in that kind's SI unit), "file" for a grid or file name.
    methods = [
        [r["coord_op_method_code"], r["coord_op_method_name"]]
        for r in e.all("select coord_op_method_code, coord_op_method_name from epsg_coordoperationmethod order by 1")
    ]
    kinds: dict[int, Counter] = defaultdict(Counter)
    for r in e.all(
        """select v.parameter_code, v.param_value_file_ref, u.unit_of_meas_type, u.unit_of_meas_name
             from epsg_coordoperationparamvalue v
             left join epsg_unitofmeasure u on u.uom_code = v.uom_code"""
    ):
        kind = "file" if r["param_value_file_ref"] else r["unit_of_meas_type"]
        name = r["unit_of_meas_name"] or ""
        # Rates, per second once in SI; PROJ's rule (`getUnitList`), which
        # keeps "parts per million" a scale.
        per_time = " per year" in name or " per second" in name
        if kind in ("length", "angle") and " per " in name or kind == "scale" and per_time:
            kind += "_per_time"
        if kind:
            kinds[r["parameter_code"]][kind] += 1
    parameters = [
        [code, name, kinds[code].most_common(1)[0][0] if kinds[code] else None]
        for code, name in (
            (r["parameter_code"], r["parameter_name"])
            for r in e.all("select parameter_code, parameter_name from epsg_coordoperationparam order by 1")
        )
    ]
    # Each projected CRS's conversion, whose code PROJJSON leaves out inside
    # the CRS but gives the conversion as a step of an operation.
    projections = [
        [r["coord_ref_sys_code"], r["projection_conv_code"]]
        for r in e.all(
            """select coord_ref_sys_code, projection_conv_code from epsg_coordinatereferencesystem
                where projection_conv_code is not null order by 1"""
        )
    ]
    return {
        "epsg_version": version,
        "geodetic_crs": geodetic,
        "vertical_crs": vertical,
        "datums": datums,
        "extents": extents,
        "crs_extent": [[code, crs_extent[code]] for code in sorted(crs_extent)],
        "operations": [ops[c] for c in sorted(ops)],
        "methods": methods,
        "parameters": parameters,
        "projections": projections,
    }


EMULATION = re.compile(r"\bEmulation\b[^.]*?\bcodes? (\d{4,5})", re.I)


def operation_record(
    row: sqlite3.Row, table: str, params: list, extents: list[int], scope: str | None = None
) -> dict:
    record = {
        "code": row["coord_op_code"],
        "name": row["coord_op_name"],
        "table": table,
        "method": row["coord_op_method_code"],
        "src": row["source_crs_code"],
        "dst": row["target_crs_code"],
        "accuracy": row["coord_op_accuracy"],
        "deprecated": bool(row["deprecated"]),
        "extents": extents,
    }
    if scope:
        record["scope"] = scope
    if row["coord_op_variant"]:
        # EPSG numbers the operations between two CRSs in order of addition.
        record["variant"] = row["coord_op_variant"]
    if params:
        record["params"] = params
    if "Replaced by" in (row["remarks"] or ""):
        record["replaced"] = True
    # "Emulation using NTv2 method of NTF to ETRS89-FRA [RGF93 v1] (1),
    # code 9327": the operation this one reproduces with another method.
    emulates = [int(c) for c in EMULATION.findall(row["remarks"] or "")]
    if emulates:
        record["emulates"] = emulates
    return record


USAGES_MAGIC = b"XUS1"


def plain_text(value: str | None) -> str:
    """EPSG's text with Unix line ends, as PROJ's database has it."""
    return (value or "").replace("\r\n", "\n")


def pack_usages(e: Epsg, crs_codes: set[int]) -> bytes:
    """Each CRS's areas of use and scopes, by reference: PROJJSON spells
    them out ("scope", "area", "bbox", or "usages"), and EPSG's long area
    descriptions repeated per CRS would cost three times as much.

    A magic, the raw length, and one zstd frame of: the extents (u32 count;
    each u32 code, four f64 LE south, west, north, east, NaN when missing,
    u16 length and UTF-8 description), the scopes (u32 count; each u32 code,
    u16 length, UTF-8 text), and the usages in EPSG's order (u32 count; each
    u32 CRS code, u16 extent index, u16 scope index, 0xFFFF for none).
    """
    rows = [
        dict(r)
        for r in e.all(
            """select object_code, extent_code, scope_code from epsg_usage
                where object_table_name = 'epsg_coordinatereferencesystem'
                order by object_code, usage_code"""
        )
        if r["object_code"] in crs_codes
    ]
    # PROJ hard-codes WGS 84's domain as the world (factory.cpp), as in
    # export_operations.
    for r in rows:
        if r["object_code"] == 4326:
            r["extent_code"] = 1262
    extent_codes = sorted({r["extent_code"] for r in rows if r["extent_code"] is not None})
    scope_codes = sorted({r["scope_code"] for r in rows if r["scope_code"] is not None})
    extent_index = {c: i for i, c in enumerate(extent_codes)}
    scope_index = {c: i for i, c in enumerate(scope_codes)}

    def text(value: str | None) -> bytes:
        data = plain_text(value).encode()
        return struct.pack("<H", len(data)) + data

    def number(value) -> float:
        return float("nan") if value is None else float(value)

    raw = bytearray(struct.pack("<I", len(extent_codes)))
    for code in extent_codes:
        x = e.one("select * from epsg_extent where extent_code = ?", code)
        # The description ("Poland - onshore and offshore."), which PROJ
        # writes, not the short name ("Poland").
        bbox = [x["bbox_south_bound_lat"], x["bbox_west_bound_lon"], x["bbox_north_bound_lat"], x["bbox_east_bound_lon"]]
        raw += struct.pack("<I4d", code, *(number(v) for v in bbox)) + text(x["extent_description"])
    raw += struct.pack("<I", len(scope_codes))
    for code in scope_codes:
        raw += struct.pack("<I", code) + text(e.one("select scope from epsg_scope where scope_code = ?", code)["scope"])
    raw += struct.pack("<I", len(rows))
    for r in rows:
        raw += struct.pack(
            "<IHH",
            r["object_code"],
            extent_index.get(r["extent_code"], 0xFFFF),
            scope_index.get(r["scope_code"], 0xFFFF),
        )
    frame = zstd.ZstdCompressor(level=19).compress(bytes(raw))
    return USAGES_MAGIC + struct.pack("<I", len(raw)) + frame


REMARKS_MAGIC = b"XRM1"
REMARK_KINDS = {"crs": b"c", "operation": b"o"}


def pack_remarks(e: Epsg, crs_codes: set[int], op_codes: set[int]) -> bytes:
    """EPSG's remarks on the CRSs and operations written, in their own file
    so that only code asking for one reads it: a magic, the raw length, and
    one zstd frame of records (kind byte, code and length as u32 LE, UTF-8)."""
    raw = bytearray()
    sources = [
        ("crs", "select coord_ref_sys_code, remarks from epsg_coordinatereferencesystem", crs_codes),
        ("operation", "select coord_op_code, remarks from epsg_coordoperation", op_codes),
    ]
    for kind, sql, wanted in sources:
        for code, text in sorted(tuple(r) for r in e.all(sql)):
            if code in wanted and text:
                data = plain_text(text).encode()
                raw += REMARK_KINDS[kind] + struct.pack("<II", code, len(data)) + data
    frame = zstd.ZstdCompressor(level=19).compress(bytes(raw))
    return REMARKS_MAGIC + struct.pack("<I", len(raw)) + frame


def pack_operations(doc: dict) -> bytes:
    """One zstd frame of the registry's JSON, after a magic and its raw length."""
    raw = json.dumps(doc, separators=(",", ":"), ensure_ascii=False).encode()
    frame = zstd.ZstdCompressor(level=19).compress(raw)
    return OPERATIONS_MAGIC + struct.pack("<I", len(raw)) + frame


# --------------------------------------------------------------------------
# Aliases
# --------------------------------------------------------------------------
#
# EPSG records other names for its CRSs, grouped by naming system: its own
# abbreviations and former names, INSPIRE and EuroGeographics identifiers, and
# national ones ("Poland alternative identifier": PL-1992 for EPSG:2180).  Real
# srsNames use some of them, so the srsName parser looks names up here.
#
# A name must identify one CRS.  An alias shared by several codes keeps the
# single one that isn't deprecated, if there is one; otherwise it is dropped,
# as are names that are another CRS's current name, and numbers (ISO Geodetic
# Register codes), which would read as EPSG codes.

NUMBER = re.compile(r"^[\d\s.]+$")


def crs_aliases(e: Epsg, records: list[dict]) -> tuple[list[tuple[str, int]], Counter]:
    """`(lowercased alias, code)`, sorted, and why the others were left out."""
    known = {r["code"]: r for r in records}
    names = {r["name"].lower(): r["code"] for r in records}
    candidates: dict[str, set[int]] = defaultdict(set)
    for row in e.all(
        "select alias, object_code from epsg_alias where object_table_name = 'epsg_coordinatereferencesystem'"
    ):
        alias, code = row["alias"].strip(), row["object_code"]
        if alias and code in known:
            candidates[alias.lower()].add(code)
    aliases, dropped = [], Counter()
    for alias, codes in candidates.items():
        if len(codes) > 1:
            codes = {code for code in codes if not known[code]["deprecated"]}
        if NUMBER.match(alias):
            dropped["a number"] += 1
        elif len(codes) != 1:
            dropped["names several CRSs"] += 1
        elif names.get(alias, next(iter(codes))) != next(iter(codes)):
            dropped["another CRS's name"] += 1
        else:
            aliases.append((alias, next(iter(codes))))
    return sorted(aliases), dropped


def emit_aliases(aliases: list[tuple[str, int]], version: str, date: str) -> str:
    lines = [
        "// @generated by scripts/gen_crs_tables.py -- do not edit.",
        "//",
        f"// Source: EPSG Geodetic Parameter Dataset v{version} ({date}), table epsg_alias.",
        "// The EPSG Dataset is owned by IOGP and used under the EPSG Dataset Terms",
        "// of Use; see EPSG-NOTICE.md next to this crate's Cargo.toml.",
        "",
        "/// EPSG's aliases for its CRSs, lowercased and sorted, each naming one code.",
        "#[rustfmt::skip]",
        "pub static ALIASES: &[(&str, u32)] = &[",
    ]
    lines += [f"    ({rust_string(alias)}, {code})," for alias, code in aliases]
    lines += ["];", ""]
    return "\n".join(lines)


# --------------------------------------------------------------------------
# Deprecations and units
# --------------------------------------------------------------------------
#
# pyproj's `CRS.get_non_deprecated` and `database.get_units_map`, as PROJ's
# `build_db.py` fills its `deprecation` and `unit_of_measure` tables.


def crs_replacements(e: Epsg, records: list[dict]) -> list[tuple[int, int]]:
    """`(deprecated CRS, its replacement)` in EPSG's order, both CRSs we have
    (`fill_deprecation`: rows whose replacement is the CRS itself are left
    out)."""
    known = {r["code"] for r in records}
    rows = e.all(
        "select object_code, replaced_by from epsg_deprecation "
        "where object_table_name = 'epsg_coordinatereferencesystem' and object_code != replaced_by "
        "order by deprecation_id"
    )
    out = [(r["object_code"], r["replaced_by"]) for r in rows if r["object_code"] in known and r["replaced_by"] in known]
    # Grouped by code, each code's replacements in EPSG's order.
    return sorted(out, key=lambda pair: pair[0])


def emit_deprecations(replacements: list[tuple[int, int]], version: str, date: str) -> str:
    lines = [
        "// @generated by scripts/gen_crs_tables.py -- do not edit.",
        "//",
        f"// Source: EPSG Geodetic Parameter Dataset v{version} ({date}), table epsg_deprecation.",
        "// The EPSG Dataset is owned by IOGP and used under the EPSG Dataset Terms",
        "// of Use; see EPSG-NOTICE.md next to this crate's Cargo.toml.",
        "",
        "/// EPSG's replacements of deprecated CRSs: (deprecated code, replacement),",
        "/// sorted by the first, each code's replacements in EPSG's order.",
        "#[rustfmt::skip]",
        "pub static REPLACEMENTS: &[(u32, u32)] = &[",
    ]
    lines += [f"    ({code}, {replacement})," for code, replacement in replacements]
    lines += ["];", ""]
    return "\n".join(lines)


UNIT_KINDS = {"length": "Length", "angle": "Angle", "scale": "Scale", "time": "Time"}


def units_of_measure(e: Epsg) -> list[dict]:
    """Every EPSG unit with its factor to the SI unit of its kind,
    `factor_b / factor_c` as `fill_unit_of_measure` computes it (none
    where EPSG gives no factors)."""
    out = []
    for r in e.all(
        "select uom_code, unit_of_meas_name, unit_of_meas_type, factor_b, factor_c, deprecated "
        "from epsg_unitofmeasure order by uom_code"
    ):
        b, c = r["factor_b"], r["factor_c"]
        out.append(
            {
                "code": r["uom_code"],
                "name": r["unit_of_meas_name"],
                "kind": UNIT_KINDS[r["unit_of_meas_type"]],
                "factor": None if b is None or c is None else b / c,
                "deprecated": bool(r["deprecated"]),
            }
        )
    return out


def emit_units(units: list[dict], version: str, date: str) -> str:
    lines = [
        "// @generated by scripts/gen_crs_tables.py -- do not edit.",
        "//",
        f"// Source: EPSG Geodetic Parameter Dataset v{version} ({date}), table epsg_unitofmeasure.",
        "// The EPSG Dataset is owned by IOGP and used under the EPSG Dataset Terms",
        "// of Use; see EPSG-NOTICE.md next to this crate's Cargo.toml.",
        "",
        "use crate::{UnitKind, UnitOfMeasure};",
        "",
        "/// EPSG's units of measure, by code.",
        "#[rustfmt::skip]",
        "pub static UNITS_OF_MEASURE: &[UnitOfMeasure] = &[",
    ]
    for u in units:
        factor = opt_f64(u["factor"])
        lines.append(
            f"    UnitOfMeasure {{ code: {u['code']}, name: {rust_string(u['name'])}, kind: UnitKind::{u['kind']}, "
            f"factor: {factor}, deprecated: {str(u['deprecated']).lower()} }},"
        )
    lines += ["];", ""]
    return "\n".join(lines)


# Methods and parameters by name, for definitions that give them without
# EPSG ids (WKT, docs/crs-wkt.md): EPSG's names and its aliases ("Central
# Meridian" for 8802), lowercased, each naming one code. A name shared by
# several keeps the single one that isn't deprecated, if there is one.


def operation_names(e: Epsg, table: str, code_col: str, name_col: str) -> list[tuple[str, int]]:
    deprecated = {}
    candidates: dict[str, set[int]] = defaultdict(set)
    for row in e.all(f"select {code_col} as code, {name_col} as name, deprecated from {table}"):
        deprecated[row["code"]] = bool(row["deprecated"])
        candidates[row["name"].strip().lower()].add(row["code"])
    for row in e.all("select alias, object_code from epsg_alias where object_table_name = ?", table):
        if row["object_code"] in deprecated and row["alias"].strip():
            candidates[row["alias"].strip().lower()].add(row["object_code"])
    names = []
    for name, codes in candidates.items():
        if len(codes) > 1:
            codes = {code for code in codes if not deprecated[code]}
        if len(codes) == 1:
            names.append((name, next(iter(codes))))
    return sorted(names)


def unit_names(e: Epsg) -> list[tuple[str, int]]:
    """Units of measure by their exact name, for writing WKT1's `UNIT`
    `AUTHORITY`; a name used twice keeps the unit that isn't deprecated."""
    by_name: dict[str, set[int]] = defaultdict(set)
    deprecated = {}
    for r in e.all("select uom_code, unit_of_meas_name, deprecated from epsg_unitofmeasure"):
        by_name[r["unit_of_meas_name"]].add(r["uom_code"])
        deprecated[r["uom_code"]] = bool(r["deprecated"])
    out = []
    for name, codes in by_name.items():
        if len(codes) > 1:
            codes = {c for c in codes if not deprecated[c]}
        if len(codes) == 1:
            out.append((name, next(iter(codes))))
    return sorted(out)


def emit_names(
    methods: list[tuple[str, int]], parameters: list[tuple[str, int]], units: list[tuple[str, int]], version: str, date: str
) -> str:
    lines = [
        "// @generated by scripts/gen_crs_tables.py -- do not edit.",
        "//",
        f"// Source: EPSG Geodetic Parameter Dataset v{version} ({date}), tables",
        "// epsg_coordoperationmethod, epsg_coordoperationparam, epsg_unitofmeasure",
        "// and epsg_alias.",
        "// The EPSG Dataset is owned by IOGP and used under the EPSG Dataset Terms",
        "// of Use; see EPSG-NOTICE.md next to this crate's Cargo.toml.",
        "",
    ]
    for static, doc, rows in (
        ("METHODS", "operation methods", methods),
        ("PARAMETERS", "operation parameters", parameters),
    ):
        lines += [
            f"/// EPSG's {doc} by name and alias, lowercased and sorted, each naming one code.",
            "#[rustfmt::skip]",
            f"pub static {static}: &[(&str, u32)] = &[",
        ]
        lines += [f"    ({rust_string(name)}, {code})," for name, code in rows]
        lines += ["];", ""]
    lines += [
        "/// EPSG's units of measure by their exact name, sorted, each naming one code.",
        "#[rustfmt::skip]",
        "pub static UNITS: &[(&str, u32)] = &[",
    ]
    lines += [f"    ({rust_string(name)}, {code})," for name, code in units]
    lines += ["];", ""]
    return "\n".join(lines)


# Datums by name, for definitions that name them without ids (WKT1, where
# GDAL writes "Ordnance_Survey_of_Great_Britain_1936"): each datum and
# ensemble with its kind and deprecation, then EPSG's aliases for them.
# xeibe-reproject matches names as PROJ does (`isEquivalentName`), so they
# are kept as EPSG writes them.


def datum_names(e: Epsg) -> tuple[list[tuple[int, str, str, bool]], list[tuple[int, str]]]:
    datums = []
    for r in e.all("select datum_code, datum_name, datum_type, deprecated from epsg_datum order by datum_code"):
        kind = r["datum_type"]
        if kind == "ensemble":
            first = e.one(
                """select d.datum_type from epsg_datumensemblemember m
                     join epsg_datum d on d.datum_code = m.datum_code
                    where m.datum_ensemble_code = ? order by m.datum_sequence""",
                r["datum_code"],
            )
            kind = first["datum_type"] if first else "geodetic"
        kind = {"dynamic geodetic": "Geodetic", "geodetic": "Geodetic", "vertical": "Vertical",
                "dynamic vertical": "Vertical", "engineering": "Engineering"}[kind]  # fmt: skip
        datums.append((r["datum_code"], r["datum_name"], kind, bool(r["deprecated"])))
    known = {d[0] for d in datums}
    aliases = sorted(
        {
            (r["object_code"], r["alias"].strip())
            for r in e.all("select object_code, alias from epsg_alias where object_table_name = 'epsg_datum'")
            if r["object_code"] in known and r["alias"].strip()
        }
    )
    return datums, aliases


def emit_datums(datums: list, aliases: list, version: str, date: str) -> str:
    lines = [
        "// @generated by scripts/gen_crs_tables.py -- do not edit.",
        "//",
        f"// Source: EPSG Geodetic Parameter Dataset v{version} ({date}), tables",
        "// epsg_datum, epsg_datumensemblemember and epsg_alias.",
        "// The EPSG Dataset is owned by IOGP and used under the EPSG Dataset Terms",
        "// of Use; see EPSG-NOTICE.md next to this crate's Cargo.toml.",
        "",
        "use crate::{DatumKind, DatumName};",
        "",
        "/// EPSG's datums and datum ensembles, by code.",
        "#[rustfmt::skip]",
        "pub static DATUMS: &[DatumName] = &[",
    ]
    lines += [
        f"    DatumName {{ code: {code}, name: {rust_string(name)}, kind: DatumKind::{kind}, deprecated: {str(dep).lower()} }},"
        for code, name, kind, dep in datums
    ]
    lines += [
        "];",
        "",
        "/// EPSG's other names for its datums (abbreviations, former names), by code.",
        "#[rustfmt::skip]",
        "pub static DATUM_ALIASES: &[(u32, &str)] = &[",
    ]
    lines += [f"    ({code}, {rust_string(alias)})," for code, alias in aliases]
    lines += ["];", ""]
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
- `data/usages.bin` -- each CRS's areas of use (description and bounding
  box) and scopes, which `projjson` adds to the PROJJSON as usages.
- `data/operations.bin` -- EPSG's coordinate operations: method, CRSs,
  parameters, accuracy, areas of use and the first usage's scope, plus the
  datums and extents they refer to.
- `data/remarks.bin` -- EPSG's remarks on those CRSs and operations,
  verbatim.
- `src/aliases.rs` -- EPSG's aliases for CRSs (table `epsg_alias`), lowercased,
  keeping only those that name a single CRS.
- `src/names.rs` -- the names and aliases of EPSG's operation methods and
  parameters, lowercased, and the names of its units of measure, keeping
  only those that name a single one.
- `src/datums.rs` -- EPSG's datums and datum ensembles (code, name, kind,
  deprecation) and EPSG's aliases for them.
- `src/deprecations.rs` -- EPSG's replacements of deprecated CRSs (table
  `epsg_deprecation`).
- `src/units.rs` -- EPSG's units of measure: code, name, kind, the factor
  to SI (`factor_b / factor_c`) and deprecation.

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
            record = crs_record(e, row)
            records.append(record)
            try:
                docs[code] = render_crs(e, code)
            except Exception as exc:  # noqa: BLE001 - reported, not swallowed
                failures[f"{type(exc).__name__}: {exc}"].append(code)
            else:
                record["datum"], record["method"] = datum_and_method(docs[code])

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
        operations = export_operations(e, version)
        ops_blob = pack_operations(operations)
        (out / "data" / "operations.bin").write_bytes(ops_blob)
        op_codes = {op["code"] for op in operations["operations"]}
        remarks_blob = pack_remarks(e, set(docs), op_codes)
        (out / "data" / "remarks.bin").write_bytes(remarks_blob)
        usages_blob = pack_usages(e, set(docs))
        (out / "data" / "usages.bin").write_bytes(usages_blob)
        table = emit_table(records, version, date, {"projjson": len(docs)})
        (out / "src" / "table.rs").write_text(table)
        aliases, dropped = crs_aliases(e, records)
        (out / "src" / "aliases.rs").write_text(emit_aliases(aliases, version, date))
        methods = operation_names(e, "epsg_coordoperationmethod", "coord_op_method_code", "coord_op_method_name")
        parameters = operation_names(e, "epsg_coordoperationparam", "parameter_code", "parameter_name")
        units = unit_names(e)
        (out / "src" / "names.rs").write_text(emit_names(methods, parameters, units, version, date))
        datums, datum_aliases = datum_names(e)
        (out / "src" / "datums.rs").write_text(emit_datums(datums, datum_aliases, version, date))
        replacements = crs_replacements(e, records)
        (out / "src" / "deprecations.rs").write_text(emit_deprecations(replacements, version, date))
        uoms = units_of_measure(e)
        (out / "src" / "units.rs").write_text(emit_units(uoms, version, date))
        (out / "EPSG-NOTICE.md").write_text(NOTICE.format(version=version, date=date))

        print(f"wrote {out / 'src' / 'table.rs'}  ({len(table) / 1024:.0f} KiB)")
        print(f"wrote {out / 'data' / 'projjson.bin'}  ({len(blob) / 1024:.0f} KiB)")
        print(
            f"wrote {out / 'data' / 'operations.bin'}  ({len(ops_blob) / 1024:.0f} KiB, "
            f"{len(operations['operations'])} operations)"
        )
        print(f"wrote {out / 'data' / 'remarks.bin'}  ({len(remarks_blob) / 1024:.0f} KiB)")
        print(f"wrote {out / 'data' / 'usages.bin'}  ({len(usages_blob) / 1024:.0f} KiB)")
        print(f"wrote {out / 'src' / 'aliases.rs'}  ({len(aliases)} aliases)")
        print(f"wrote {out / 'src' / 'names.rs'}  ({len(methods)} method and {len(parameters)} parameter names)")
        print(f"wrote {out / 'src' / 'datums.rs'}  ({len(datums)} datums, {len(datum_aliases)} aliases)")
        print(f"wrote {out / 'src' / 'deprecations.rs'}  ({len(replacements)} replacements)")
        print(f"wrote {out / 'src' / 'units.rs'}  ({len(uoms)} units)")
        for reason, n in sorted(dropped.items()):
            print(f"  left out {n}: {reason}")
        print(f"wrote {out / 'EPSG-NOTICE.md'}")

        written = update_changelog(out / "CHANGELOG.md", e, version, date, previous_version, measured)
        if written:
            print(f"wrote {out / 'CHANGELOG.md'}  ({written} release(s) added)")

        cargo_toml = out / "Cargo.toml"
        if cargo_toml.exists():
            print(f"crate version: {update_crate_version(cargo_toml, version)}")


if __name__ == "__main__":
    main()
