#!/usr/bin/env python3
# Runs inside the GDAL container (scripts/gdal python3 ...), which provides osgeo;
# `uv run` on the host fails on `from osgeo import ...`.
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Verify the recorded axis order of test samples with independent evidence.

    scripts/gdal python3 scripts/corpus/axis_evidence.py [NAME ...]    # runs inside the GDAL container

For every [[sample]] in tests/data/samples.toml with `axis_order`, all positions in
the sample (pos, posList, coordinates, coord, lower/upperCorner) are read in both
orders, as written and swapped, and each reading is tested against the checks below
(envelope corners are only used by crs_area and same_as, as a box may reach outside a region):

  crs_area   the area of use of the CRS named by srsName (PROJ database, 0.5° margin)
  place      each entry of `axis_places`: a region the features are known to lie in,
             taken from something other than the geometry (TERYT code in the
             attributes, the file name, the publisher's country), looked up in
             example_data/reference/axis_reference.gpkg (scripts/corpus/reference_data.py)
  same_as    `axis_same_as`: another sample with the same features (e.g. the same WFS
             in another version); its positions, read in its recorded order, must equal ours

`axis_scope = "source"` evaluates the whole original document instead of the sample,
for samples whose few features cannot tell the readings apart (same document, same order).

The recorded order is `confirmed` when it passes every check (>= 95 % of positions inside)
and at least one check rejects the swapped reading (> 10 % of positions outside). Otherwise it is `contradicted` (a check fails for the
recorded order) or `ambiguous` (nothing tells the two readings apart). The coordinates are
tokenized here directly, so the result does not depend on any GML reader's axis logic.

Prints one JSON object {name: result} to stdout. Exit status 1 unless all are confirmed.
"""
import json
import math
import re
import sys
import tomllib
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path

from osgeo import ogr, osr

osr.UseExceptions()
ogr.UseExceptions()

PROJECT = Path(__file__).resolve().parents[2]
DATA = PROJECT / "tests" / "data"
REFERENCE = PROJECT / "example_data" / "reference" / "axis_reference.gpkg"
GML_NS = ("http://www.opengis.net/gml", "http://www.opengis.net/gml/3.2")
MAX_POSITIONS = 500
# Natural Earth 1:10m coastlines are generalized by up to a few km (e.g. small islands, harbours)
DEFAULT_TOLERANCE_M = {"countries": 5000}
# Share of positions inside: the recorded order must have >= PASS inside; the swapped reading is
# rejected when more than 1 - REJECT of its positions fall outside (a genuine reading of a
# dataset known to lie in the region cannot put a tenth of it elsewhere).
PASS, REJECT = 0.95, 0.90
# CRS areas of use are soft limits: data often reaches slightly past them (e.g. Polish CS2000 zones)
AREA_MARGIN_DEG = 0.5

ADV_CRS = {"ETRS89_UTM32": 25832, "ETRS89_UTM33": 25833, "DE_DHDN_3GK2": 31466, "DE_DHDN_3GK3": 31467,
           "DE_DHDN_3GK4": 31468, "ETRS89_LAT-LON": 4258}


def epsg_code(srs_name: str) -> int:
    """EPSG code of an srsName in any of the common forms (see docs/geometry.md)."""
    s = srs_name.strip()
    m = re.match(r"urn:adv:crs:([^*]+)", s)
    if m:
        return ADV_CRS[m.group(1)]
    m = re.search(r"(?:EPSG[:/]+(?:[\d.]*[:/])?|epsg\.xml#)(\d+)$", s, re.I)
    if not m:
        raise ValueError(f"unsupported srsName {srs_name!r}")
    return int(m.group(1))


def srs(code: int) -> osr.SpatialReference:
    sr = osr.SpatialReference()
    sr.ImportFromEPSG(code)
    sr.SetAxisMappingStrategy(osr.OAMS_TRADITIONAL_GIS_ORDER)  # we always pass (east/lon, north/lat)
    return sr


def local(tag: str) -> str:
    return tag.rsplit("}", 1)[-1]


ENVELOPES = {"Envelope", "Box", "boundedBy"}


def positions(fh) -> list[tuple[str, tuple[float, float], bool]]:
    """(srsName, (first, second) value as written, inside an envelope) for every position.

    Streams the document; srsName and srsDimension are inherited from ancestors.
    """
    out = []
    stack = []  # (srsName, dimension, in_envelope) per open element
    for event, el in ET.iterparse(fh, events=("start", "end")):
        parent = stack[-1] if stack else (None, None, False)
        if event == "start":
            dim = el.get("srsDimension") or el.get("dimension")
            stack.append((el.get("srsName", parent[0]), int(dim) if dim else parent[1],
                          parent[2] or local(el.tag) in ENVELOPES))
            continue
        srs_name, dim, env = stack.pop()
        ns = el.tag[1:].split("}", 1)[0] if el.tag.startswith("{") else ""
        name = local(el.tag)
        if ns in GML_NS and srs_name:
            if name in ("pos", "lowerCorner", "upperCorner", "posList"):
                values = [float(v) for v in (el.text or "").split()]
                d = dim or 2
                for i in range(0, len(values) - d + 1, d):
                    out.append((srs_name, (values[i], values[i + 1]), env))
            elif name == "coordinates":
                cs, ts, dec = el.get("cs", ","), el.get("ts", " "), el.get("decimal", ".")
                for t in (el.text or "").strip().split(ts):
                    if t.strip():
                        v = [float(x.replace(dec, ".")) for x in t.strip().split(cs)]
                        out.append((srs_name, (v[0], v[1]), env))
            elif name == "coord":
                v = {local(c.tag): float(c.text) for c in el}
                out.append((srs_name, (v["X"], v["Y"]), env))
        if not stack or name in ("featureMember", "member", "featureMembers"):
            el.clear()
    return out


def open_positions(sample: dict, scope: str) -> list[tuple[str, tuple[float, float], bool]]:
    if scope == "sample":
        with (DATA / sample["file"]).open("rb") as fh:
            return positions(fh)
    src = PROJECT / sample["source"]
    if sample.get("member"):
        with zipfile.ZipFile(src) as z, z.open(sample["member"]) as fh:
            return positions(fh)
    with src.open("rb") as fh:
        return positions(fh)


def subsample(items: list, n: int) -> list:
    if len(items) <= n:
        return items
    step = len(items) / n
    return [items[int(i * step)] for i in range(n)]


class Region:
    def __init__(self, layer: str, code: str):
        ds = ogr.Open(str(REFERENCE))
        lyr = ds.GetLayerByName(layer)
        lyr.SetAttributeFilter(f"code = '{code}'")
        feats = list(lyr)
        if len(feats) != 1:
            raise ValueError(f"{layer} code {code}: {len(feats)} matches in {REFERENCE.name}")
        self.name = feats[0].GetField("name")
        geom = feats[0].GetGeometryRef().Clone()
        geom.AssignSpatialReference(lyr.GetSpatialRef())
        geom.GetSpatialReference().SetAxisMappingStrategy(osr.OAMS_TRADITIONAL_GIS_ORDER)
        # metric working CRS: azimuthal equidistant around the region, so distances are in metres
        wgs = geom.Clone()
        wgs.TransformTo(srs(4326))
        c = wgs.GetEnvelope()  # (minx, maxx, miny, maxy) = lon/lat
        aeqd = osr.SpatialReference()
        aeqd.ImportFromProj4(f"+proj=aeqd +lat_0={(c[2] + c[3]) / 2} +lon_0={(c[0] + c[1]) / 2} +units=m")
        aeqd.SetAxisMappingStrategy(osr.OAMS_TRADITIONAL_GIS_ORDER)
        geom.TransformTo(aeqd)
        self.geom, self.aeqd = geom, aeqd

    def distance_m(self, pt: ogr.Geometry) -> float:
        p = pt.Clone()
        p.TransformTo(self.aeqd)
        return 0.0 if self.geom.Contains(p) else self.geom.Distance(p)


def to_point(xy, sr) -> ogr.Geometry | None:
    if not all(math.isfinite(v) for v in xy):
        return None
    p = ogr.Geometry(ogr.wkbPoint)
    p.AddPoint_2D(*xy)
    p.AssignSpatialReference(sr)
    return p


def lonlat(xy, code: int) -> tuple[float, float] | None:
    try:
        t = osr.CoordinateTransformation(srs(code), srs(4326))
        lon, lat, _ = t.TransformPoint(*xy)
    except RuntimeError:
        return None
    return (lon, lat) if math.isfinite(lon) and math.isfinite(lat) else None


def readings(pos, order: str):
    """Candidate (x, y) = (east/lon, north/lat) for each position under one reading."""
    return [(s, xy if order == "as written" else (xy[1], xy[0])) for s, xy, _ in pos]


def share(flags: list[bool]) -> float:
    return round(sum(flags) / len(flags), 3) if flags else 0.0


def check_crs_area(pos, order):
    flags = []
    for s, xy in readings(pos, order):
        code = epsg_code(s)
        a = srs(code).GetAreaOfUse()
        ll = lonlat(xy, code)
        if ll is None or a is None:
            flags.append(False)
            continue
        lon, lat = ll
        m = AREA_MARGIN_DEG
        w, e = a.west_lon_degree - m, a.east_lon_degree + m
        in_lon = w <= lon <= e if w <= e else (lon >= w or lon <= e)
        flags.append(in_lon and a.south_lat_degree - m <= lat <= a.north_lat_degree + m)
    return share(flags)


def check_place(pos, order, region: Region, tolerance_m: float):
    flags, first_distance = [], None
    for s, xy in readings(pos, order):
        sr = srs(epsg_code(s))
        pt = to_point(xy, sr)
        try:
            d = region.distance_m(pt) if pt else math.inf
        except RuntimeError:
            d = math.inf
        if first_distance is None:
            first_distance = d
        flags.append(d <= tolerance_m)
    return share(flags), (round(first_distance / 1000, 1) if math.isfinite(first_distance) else None)


def evaluate(sample: dict, by_name: dict) -> dict:
    recorded = sample["axis_order"]
    scope = sample.get("axis_scope", "sample")
    pos_all = open_positions(sample, "sample")
    pos = subsample(open_positions(sample, scope) if scope != "sample" else pos_all, MAX_POSITIONS)
    # envelope corners are not feature locations: a bounding box may reach outside the region
    pos_features = [p for p in pos if not p[2]]
    true_reading = "as written" if recorded == "x/y" else "swapped"
    other = "swapped" if true_reading == "as written" else "as written"
    checks = []

    def add(name, detail, f_true, f_other, extra=None):
        checks.append({"check": name, "detail": detail, "recorded_order_inside": f_true,
                       "swapped_order_inside": f_other, "passes": f_true >= PASS,
                       "rejects_swapped": f_other < REJECT, **(extra or {})})

    srs_names = sorted({s for s, _, _ in pos})
    add("crs_area", f"area of use of {', '.join(f'EPSG:{epsg_code(s)}' for s in srs_names)}",
        check_crs_area(pos, true_reading), check_crs_area(pos, other))
    for place in sample.get("axis_places", []):
        region = Region(place["layer"], place["code"])
        tol = float(place.get("tolerance_m", DEFAULT_TOLERANCE_M.get(place["layer"], 0)))
        f_true, d_true = check_place(pos_features, true_reading, region, tol)
        f_other, d_other = check_place(pos_features, other, region, tol)
        add("place", f"{region.name} ({place['layer']} {place['code']}"
            + (f", tolerance {tol:g} m" if tol else "") + f"); expected because: {place['why']}",
            f_true, f_other, {"first_position_km_outside": {"recorded": d_true, "swapped": d_other}})
    if sample.get("axis_same_as"):
        ref = by_name[sample["axis_same_as"]]
        ref_pos = readings(open_positions(ref, "sample"), "as written" if ref["axis_order"] == "x/y" else "swapped")
        ours = readings(pos_all, true_reading)
        mine = [xy for _, xy in ours]
        theirs = [xy for _, xy in ref_pos]
        same = len(mine) == len(theirs) and all(abs(a - b) < 1e-6 for p, q in zip(mine, theirs) for a, b in zip(p, q))
        swapped_mine = [(y, x) for x, y in mine]
        swapped_same = len(mine) == len(theirs) and all(
            abs(a - b) < 1e-6 for p, q in zip(swapped_mine, theirs) for a, b in zip(p, q))
        add("same_as", f"same positions as `{ref['name']}` read in its recorded order ({ref['axis_order']})",
            1.0 if same else 0.0, 1.0 if swapped_same else 0.0)
    if any(not c["passes"] for c in checks):
        status = "contradicted"
    elif any(c["rejects_swapped"] for c in checks):
        status = "confirmed"
    else:
        status = "ambiguous"
    return {"axis_order": recorded, "status": status, "scope": scope, "positions_checked": len(pos),
            "checks": checks}


def main() -> int:
    spec = tomllib.loads((DATA / "samples.toml").read_text())
    by_name = {s["name"]: s for s in spec["sample"]}
    wanted = set(sys.argv[1:])
    results = {}
    for s in spec["sample"]:
        if "axis_order" in s and (not wanted or s["name"] in wanted):
            try:
                results[s["name"]] = evaluate(s, by_name)
            except Exception as e:  # noqa: BLE001 - report per sample
                results[s["name"]] = {"axis_order": s["axis_order"], "status": "error", "error": f"{type(e).__name__}: {e}"}
    json.dump(results, sys.stdout, indent=1, ensure_ascii=False)
    print()
    return 0 if all(r["status"] == "confirmed" for r in results.values()) else 1


if __name__ == "__main__":
    sys.exit(main())
