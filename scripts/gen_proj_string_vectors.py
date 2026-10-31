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
* `methods.jsonl`: `METHOD_STRINGS`, a few strings for each projection
  method EPSG has no CRS for and each one only PROJ defines, with PROJ's
  PROJJSON, its WKT (`WKT2_2019`, `WKT1_GDAL`, `WKT1_ESRI`) and PROJ's
  reading of that (as the changes to the string's PROJJSON), and points over the whole globe converted forwards
  (rounded to 1 µm) and those results back (to 1e-11°). The same file
  is written with and without `--all`.
* `custom.jsonl`: the same for `CUSTOM_STRINGS`, the projections PROJ
  has no CRS method for (`hammer`, `ob_tran`, …), which it names
  "PROJ hammer" with its PROJ string's keys as parameters.

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
    # PROJ-only projections (more in METHOD_STRINGS)
    "+proj=robin",
    "+proj=moll +lon_0=0 +datum=WGS84",
    "+proj=wintri",
    "+proj=natearth",
    "+proj=eck4",
    # DMS angles
    "+proj=tmerc +lat_0=49d30'N +lon_0=2d20'14.025\"E +k=0.9996 +datum=WGS84",
]


# The methods EPSG has no CRS for and the projections only PROJ defines,
# each in a few variants: on a sphere and on an ellipsoid (where PROJ's
# projection is spherical, it uses the semi-major axis as the radius), and
# with its own parameters.
WORLD = [
    "eck1", "eck2", "eck3", "eck4", "eck5", "eck6", "mbtfpq", "gall", "goode", "igh",
    "igh_o", "moll", "natearth", "natearth2", "patterson", "robin", "sinu", "times",
    "wag1", "wag2", "wag3", "wag4", "wag5", "wag6", "wag7", "crast", "qua_aut",
    "aitoff", "wink1", "wink2", "wintri", "loxim", "mill", "comill", "vandg",
]  # fmt: skip
METHOD_STRINGS = [
    *(f"+proj={p} +R=6371000" for p in WORLD),
    *(f"+proj={p} +lon_0=10 +x_0=1000 +y_0=-2000 +ellps=WGS84" for p in WORLD),
    "+proj=wag3 +lat_ts=30 +R=6371000",
    "+proj=wink1 +lat_ts=40 +ellps=WGS84",
    "+proj=wink2 +lat_1=40 +R=6371000",
    "+proj=wintri +lat_1=40 +ellps=WGS84",
    "+proj=wintri +lat_1=0 +R=6371000",
    "+proj=loxim +lat_1=40 +lon_0=-20 +R=6371000",
    "+proj=mill +R_A +ellps=WGS84",
    "+proj=vandg +R_A +ellps=GRS80",
    "+proj=ortho +lat_0=40 +lon_0=-100 +ellps=WGS84",
    "+proj=ortho +lat_0=90 +ellps=WGS84",
    "+proj=ortho +lat_0=-90 +lon_0=30 +ellps=WGS84",
    "+proj=ortho +lat_0=0 +lon_0=10 +x_0=1000 +y_0=2000 +ellps=GRS80",
    "+proj=ortho +lat_0=40 +lon_0=-100 +R=6371000",
    "+proj=ortho +lat_0=90 +R=6371000",
    "+proj=ortho +lat_0=0 +R=6371000",
    "+proj=ortho +lat_0=30 +lon_0=20 +ellps=WGS84 +f=0",
    "+proj=eqdc +lat_0=40 +lon_0=-96 +lat_1=20 +lat_2=60 +x_0=100 +y_0=200 +ellps=GRS80",
    "+proj=eqdc +lat_1=-30 +lat_2=-60 +R=6371000",
    "+proj=eqdc +lat_1=45 +lat_2=45 +ellps=WGS84",
    "+proj=nsper +lat_0=40 +lon_0=10 +h=3000000 +ellps=WGS84",
    "+proj=nsper +lat_0=90 +h=35786000 +R=6371000",
    "+proj=nsper +lat_0=0 +lon_0=-75 +h=1000000 +R=6371000",
    "+proj=gnom +lat_0=90 +R=6371000",
    "+proj=gnom +lat_0=30 +lon_0=40 +ellps=WGS84",
    "+proj=gnom +lat_0=0 +lon_0=40 +R=6371000",
    "+proj=gnom +lat_0=-90 +ellps=WGS84",
    "+proj=stere +lat_0=40 +lon_0=10 +k=0.9 +ellps=WGS84",
    "+proj=stere +lat_0=40 +lon_0=10 +R=6371000",
    "+proj=stere +lat_0=0 +lon_0=10 +ellps=WGS84",
    "+proj=stere +lat_0=0 +R=6371000",
    "+proj=tcea +lon_0=10 +R=6371000",
    "+proj=tcea +lon_0=10 +k=0.9 +x_0=100 +ellps=WGS84",
    "+proj=gstmerc +lat_0=-21.1166666667 +lon_0=55.5333333333 +k_0=1 +x_0=160000 +y_0=50000 +ellps=intl",
    "+proj=gstmerc +lat_0=45 +lon_0=10 +k_0=0.9996 +ellps=WGS84",
    "+proj=imw_p +lat_1=30 +lat_2=-40 +lon_0=10 +ellps=WGS84",
    "+proj=imw_p +lat_1=44 +lat_2=40 +lon_0=3 +ellps=clrk66",
    "+proj=tpeqd +lat_1=30 +lon_1=-30 +lat_2=40 +lon_2=50 +ellps=WGS84",
    "+proj=tpeqd +lat_1=0 +lon_1=0 +lat_2=60 +lon_2=90 +R=6371000",
    "+proj=omerc +lat_0=40 +lat_1=30 +lon_1=-100 +lat_2=50 +lon_2=-80 +k=1 +ellps=WGS84",
    "+proj=omerc +lat_0=-20 +lat_1=-10 +lon_1=20 +lat_2=-30 +lon_2=40 +k=0.9996 +x_0=500 +R=6371000",
    "+proj=geos +h=35785831 +lon_0=0 +sweep=x +ellps=WGS84",
    "+proj=geos +h=35785831 +lon_0=-75 +ellps=GRS80",
    "+proj=geos +h=35785831 +lon_0=140.7 +R=6371000",
    "+proj=geos +h=35785831 +sweep=x +R=6371000",
    "+proj=qsc +ellps=WGS84",
    "+proj=qsc +lat_0=90 +ellps=WGS84",
    "+proj=qsc +lat_0=-90 +R=6371000",
    "+proj=qsc +lon_0=90 +R=6371000",
    "+proj=qsc +lon_0=180 +ellps=WGS84",
    "+proj=qsc +lon_0=-90 +ellps=WGS84",
    "+proj=sch +plat_0=30 +plon_0=40 +phdg_0=50 +ellps=WGS84",
    "+proj=sch +plat_0=-10 +plon_0=-60 +phdg_0=-30 +h_0=100 +R=6371000",
    "+proj=peirce_q +lat_0=90 +shape=square +R=6371000",
    "+proj=peirce_q +lat_0=90 +lon_0=20 +shape=diamond +R=6371000",
    "+proj=peirce_q +lat_0=90 +shape=square +ellps=WGS84",
    # Other celestial bodies: Mars (IAU 2015), the Moon and a toy ellipsoid.
    "+proj=longlat +a=3396190 +b=3376200",
    "+proj=eqc +a=3396190 +b=3376200",
    "+proj=robin +R=1737400",
    "+proj=tmerc +lon_0=10 +a=2 +b=1.5",
]
# The projections PROJ has no CRS method for, which it names "PROJ hammer"
# with its keys as parameters: each without parameters on a sphere and an
# ellipsoid, then with its own.
CUSTOM_PLAIN = [
    "adams_hemi", "adams_ws1", "adams_ws2", "guyou", "airocean", "airy", "apian",
    "august", "bacon", "bertin1953", "boggs", "collg", "denoy", "fahey", "fouc",
    "gins8", "gs48", "gs50", "alsk", "hatano", "healpix", "rhealpix", "imoll",
    "imoll_o", "isea", "kav5", "kav7", "larr", "lask", "lee_os", "mbt_fps", "mbt_s",
    "mbtfpp", "mil_os", "nell", "nell_h", "nicol", "ortel", "putp1", "putp2", "putp3",
    "putp3p", "putp4p", "putp5", "putp5p", "putp6", "putp6p", "rouss", "tcc",
    "tobmerc", "vandg2", "vandg3", "vandg4", "weren", "calcofi", "s2", "spilhaus",
    "ups", "hammer", "urmfps", "lcca", "bipc", "ccon",
]  # fmt: skip
CUSTOM_STRINGS = [
    *(f"+proj={p} +R=6371000" for p in CUSTOM_PLAIN),
    *(f"+proj={p} +ellps=WGS84" for p in CUSTOM_PLAIN),
    "+proj=adams_ws2 +lon_0=30 +R=6371000",
    "+proj=airocean +orient=horizontal +R=6371000",
    "+proj=airy +no_cut +lat_b=30 +R=6371000",
    "+proj=bipc +ns +R=6371000",
    "+proj=ccon +lat_1=52 +lon_0=19 +x_0=1000 +ellps=WGS84",
    "+proj=chamb +lat_1=10 +lon_1=30 +lon_2=40 +R=6371000",
    "+proj=euler +lat_1=20 +lat_2=50 +lon_0=10 +R=6371000",
    "+proj=fouc_s +n=0.6 +R=6371000",
    "+proj=gn_sinu +m=0.5 +n=1.785 +R=6371000",
    "+proj=hammer +W=0.5 +M=2 +lon_0=10 +R=6371000",
    "+proj=hammer +W=0.25 +R=6371000",
    "+proj=healpix +rot_xy=10 +ellps=WGS84",
    "+proj=rhealpix +north_square=1 +south_square=2 +R=6371000",
    "+proj=isea +mode=plane +R=6371007.18091875",
    "+proj=lagrng +W=2 +lat_1=10 +R=6371000",
    "+proj=lcca +lat_0=40 +lon_0=-100 +k_0=0.9 +x_0=1000 +y_0=-500 +ellps=WGS84",
    "+proj=leac +lat_1=20 +R=6371000",
    "+proj=leac +lat_1=-30 +south +ellps=WGS84",
    "+proj=lsat +lsat=5 +path=10 +ellps=WGS84",
    "+proj=misrsom +path=10 +ellps=WGS84",
    "+proj=som +inc_angle=98.2 +ps_rev=0.06868 +asc_lon=64.2 +ellps=WGS84",
    "+proj=murd1 +lat_1=20 +lat_2=60 +R=6371000",
    "+proj=murd2 +lat_1=20 +lat_2=60 +R=6371000",
    "+proj=murd3 +lat_1=20 +lat_2=60 +R=6371000",
    "+proj=ob_tran +o_proj=moll +o_lat_p=45 +o_lon_p=-90 +lon_0=-90 +R=6371000",
    "+proj=ob_tran +o_proj=merc +o_lat_p=30 +o_lon_p=20 +lon_0=10 +ellps=WGS84",
    "+proj=ob_tran +o_proj=longlat +o_lat_p=40 +o_lon_p=10 +lon_0=-170 +R=6371000",
    "+proj=ob_tran +o_proj=latlon +o_lat_p=39.25 +o_lon_p=0 +lon_0=18 +ellps=WGS84",
    "+proj=ocea +alpha=30 +lonc=20 +R=6371000",
    "+proj=ocea +lat_1=10 +lon_1=20 +lat_2=40 +lon_2=50 +ellps=WGS84",
    "+proj=oea +n=2 +m=3 +theta=10 +R=6371000",
    "+proj=pconic +lat_1=30 +lat_2=60 +lon_0=-90 +R=6371000",
    "+proj=rpoly +lat_ts=30 +R=6371000",
    "+proj=tissot +lat_1=20 +lat_2=60 +R=6371000",
    "+proj=tpers +lat_0=40 +lon_0=10 +h=3000000 +tilt=10 +azi=20 +R=6371000",
    "+proj=tpers +lat_0=90 +h=1000000 +tilt=-20 +R=6371000",
    "+proj=urm5 +n=0.8 +q=1.5 +alpha=20 +R=6371000",
    "+proj=urmfps +n=0.5 +R=6371000",
    "+proj=vitk1 +lat_1=20 +lat_2=60 +R=6371000",
    "+proj=ups +south +ellps=WGS84",
    "+proj=s2 +UVtoST=quadratic +ellps=WGS84",
]
WKT_VERSIONS = ["WKT2_2019", "WKT1_GDAL", "WKT1_ESRI"]


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


def geodetic_of(crs: pyproj.CRS) -> pyproj.CRS | None:
    """The geodetic CRS a CRS's points are converted from: a derived
    geographic CRS's base (pyproj's `geodetic_crs` of one is itself)."""
    j = crs.to_json_dict()
    if j["type"] == "DerivedGeographicCRS":
        return pyproj.CRS.from_json_dict(j["base_crs"])
    return crs.geodetic_crs


def converted(crs: pyproj.CRS, pts: list[list[float]]) -> list[list[float] | None] | None:
    base = crs.source_crs if crs.is_bound else crs
    if base is None or geodetic_of(base) is None or not (base.is_projected or base.is_geographic):
        return None
    try:
        t = pyproj.Transformer.from_crs(geodetic_of(base), base, always_xy=True)
        forward = lambda lon, lat: t.transform(lon, lat, errcheck=True)  # noqa: E731
    except pyproj.exceptions.ProjError:
        # A projection without an inverse (`airy`): PROJ builds no
        # transformation, but its projection runs forwards.
        if not base.is_projected:
            return None
        try:
            p = pyproj.Proj(base)
        except pyproj.exceptions.CRSError:
            return None
        forward = lambda lon, lat: p(lon, lat, errcheck=True)  # noqa: E731
    out: list[list[float] | None] = []
    for lon, lat in pts:
        try:
            x, y = forward(lon, lat)
        except (pyproj.exceptions.ProjError, pyproj.exceptions.CRSError):
            out.append(None)
            continue
        out.append([x, y] if math.isfinite(x) and math.isfinite(y) else None)
    return out


def method_points(srs: str) -> list[list[float]]:
    """A grid over the whole globe, around the projection's own meridian,
    and a few points near its origin."""
    lon0 = angle(srs, "lon_0") or angle(srs, "plon_0")
    lat0 = angle(srs, "lat_0") or angle(srs, "plat_0")
    out = [[lon0 + dx, lat] for dx in (-170, -100, -40, 0, 40, 100, 170) for lat in (-85, -60, -30, 0, 30, 60, 85)]
    for dx, dy in [(1, 1), (5, -5), (-30, 20), (60, 10)]:
        out.append([lon0 + dx, max(-89.0, min(89.0, lat0 + dy))])
    return [[round(lon, 6), lat] for lon, lat in out]


def changes(a: object, b: object, path: tuple = ()) -> list[list]:
    """What turns `a` into `b`: `[path, value]` sets the value at the path
    (keys and list indices), `[path]` removes it."""
    if isinstance(a, dict) and isinstance(b, dict):
        out = []
        for k in a:
            out += [[[*path, k]]] if k not in b else changes(a[k], b[k], (*path, k))
        out += [[[*path, k], b[k]] for k in b if k not in a]
        return out
    if isinstance(a, list) and isinstance(b, list) and len(a) == len(b):
        return [c for i, (x, y) in enumerate(zip(a, b, strict=True)) for c in changes(x, y, (*path, i))]
    return [] if a == b and type(a) is type(b) else [[list(path), b]]


def method_vector(srs: str) -> dict:
    """A projection's string as pyproj prepares it, PROJ's reading, its WKT
    and PROJ's reading of that, and points converted forwards and back."""
    srs = prepare(srs)
    record: dict = {"srs": srs, **reading(srs)}
    if record["projjson"] is None:
        return record
    crs = pyproj.CRS(srs)
    record["wkt"] = {}
    for version in WKT_VERSIONS:
        try:
            text = crs.to_wkt(version)
        except pyproj.exceptions.CRSError:
            text = None
        entry: dict = {"text": text}
        if text is not None:
            try:
                entry["changes"] = changes(record["projjson"], pyproj.CRS(text).to_json_dict())
            except pyproj.exceptions.CRSError as e:
                entry["error"] = error_text(e)
        record["wkt"][version] = entry
    pts = method_points(srs)
    out = converted(crs, pts)
    if out is None:
        return record
    record["points"] = pts
    record["converted"] = [None if p is None else [round(p[0], 6), round(p[1], 6)] for p in out]
    try:
        t = pyproj.Transformer.from_crs(crs, geodetic_of(crs), always_xy=True)
    except pyproj.exceptions.ProjError:
        record["inverse"] = None  # PROJ has no inverse (wag7)
        return record
    back: list[list[float] | None] = []
    for p in record["converted"]:
        try:
            if p is None:
                raise pyproj.exceptions.ProjError("no point")
            lon, lat = t.transform(p[0], p[1], errcheck=True)
        except pyproj.exceptions.ProjError:
            back.append(None)
            continue
        ok = math.isfinite(lon) and math.isfinite(lat)
        back.append([round(lon, 11), round(lat, 11)] if ok else None)
    record["inverse"] = back
    return record


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
    with ProcessPoolExecutor(max_workers=args.jobs, initializer=init_worker) as pool:
        methods = list(pool.map(method_vector, METHOD_STRINGS, chunksize=2))
    write(
        out / "methods.jsonl",
        methods,
        "PROJ strings of PROJ's projections, read, written as WKT and converted by PROJ",
    )
    with ProcessPoolExecutor(max_workers=args.jobs, initializer=init_worker) as pool:
        custom = list(pool.map(method_vector, CUSTOM_STRINGS, chunksize=2))
    write(
        out / "custom.jsonl",
        custom,
        "PROJ strings of PROJ's projections without a CRS method, read, written and converted by PROJ",
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
