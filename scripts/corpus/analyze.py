#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["lxml>=5"]
# ///
"""Analyze the corpus: one record per GML/XML document (also inside zip archives).

    scripts/corpus/analyze.py [--workers 8] [--max-mb 0]   # 0 = read whole documents
    scripts/corpus/analyze.py --report

Streams each document with lxml.iterparse and records: root element, GML
namespace/version hints, feature member containers, feature types with counts,
GML geometry/segment elements (arcs!), srsName values, coordinate carriers,
srsDimension values, and any xlink:href use. Results are cached in
example_data/_inventory/corpus_analysis.jsonl keyed by (path, member, size, mtime).
"""
import argparse
import collections
import json
import sys
import zipfile
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

from lxml import etree

PROJECT = Path(__file__).resolve().parents[2]
ROOTS = [PROJECT / "example_data"]
OUT = PROJECT / "example_data" / "_inventory" / "corpus_analysis.jsonl"
GML_NS = {"http://www.opengis.net/gml", "http://www.opengis.net/gml/3.2"}
XLINK_HREF = "{http://www.w3.org/1999/xlink}href"
MEMBERS = {"featureMember", "featureMembers", "member"}
CARRIERS = {"pos", "posList", "coordinates", "coord", "pointProperty", "pointRep"}
INTERESTING = {
    "Point", "LineString", "LinearRing", "Polygon", "Curve", "OrientableCurve", "CompositeCurve",
    "Ring", "Surface", "OrientableSurface", "CompositeSurface", "PolygonPatch", "Triangle", "Rectangle",
    "MultiPoint", "MultiLineString", "MultiCurve", "MultiPolygon", "MultiSurface", "MultiGeometry",
    "MultiSolid", "Solid", "Shell", "CompositeSolid", "PolyhedralSurface", "TriangulatedSurface", "Tin",
    "LineStringSegment", "Arc", "ArcString", "Circle", "ArcByBulge", "ArcStringByBulge",
    "ArcByCenterPoint", "CircleByCenterPoint", "CubicSpline", "BSpline", "Bezier", "Clothoid",
    "OffsetCurve", "Geodesic", "GeodesicString", "Envelope", "Box", "Null", "null",
    "outerBoundaryIs", "innerBoundaryIs", "exterior", "interior", "boundedBy",
    "RectifiedGridCoverage", "GridCoverage", "TimeInstant", "TimePeriod",
}


def local(tag) -> str:
    return tag.rsplit("}", 1)[-1] if isinstance(tag, str) else ""


def ns(tag) -> str:
    return tag[1:].split("}", 1)[0] if isinstance(tag, str) and tag.startswith("{") else ""


def analyze_stream(fh, max_bytes: int) -> dict:
    rec = {"root": None, "root_ns": None, "gml_ns": set(), "schema_location": None, "members": collections.Counter(),
           "feature_types": collections.Counter(), "geom": collections.Counter(), "carriers": collections.Counter(),
           "srs": collections.Counter(), "srs_dim": collections.Counter(), "xlink_href": 0, "depth_max": 0,
           "error": None, "truncated": False, "fme": False}
    depth, member_depth = 0, None
    try:
        ctx = etree.iterparse(fh, events=("start", "end"), huge_tree=True, resolve_entities=False,
                              no_network=True, load_dtd=False, recover=False)
        for event, el in ctx:
            if event == "start":
                depth += 1
                rec["depth_max"] = max(rec["depth_max"], depth)
                tag = el.tag
                if rec["root"] is None:
                    rec["root"], rec["root_ns"] = local(tag), ns(tag)
                    rec["schema_location"] = el.get("{http://www.w3.org/2001/XMLSchema-instance}schemaLocation")
                    rec["fme"] = "http://www.safe.com/gml/fme" in (el.nsmap or {}).values()
                n, name = ns(tag), local(tag)
                if n in GML_NS or n.startswith("http://www.opengis.net/wfs"):
                    if name in MEMBERS:
                        rec["members"][f"{n.rsplit('/', 1)[-1]}:{name}"] += 1
                        member_depth = depth
                if member_depth is not None and depth == member_depth + 1:
                    rec["feature_types"][f"{{{n}}}{name}"] += 1
                if n in GML_NS:
                    rec["gml_ns"].add(n)
                    if name in INTERESTING:
                        rec["geom"][name] += 1
                    elif name in CARRIERS:
                        rec["carriers"][name] += 1
                    s = el.get("srsName")
                    if s:
                        rec["srs"][s] += 1
                    d = el.get("srsDimension") or el.get("dimension")
                    if d:
                        rec["srs_dim"][d] += 1
                if el.get(XLINK_HREF) is not None:
                    rec["xlink_href"] += 1
            else:
                depth -= 1
                if member_depth is not None and depth < member_depth:
                    member_depth = None
                if depth <= 2:
                    el.clear(keep_tail=False)
                    parent = el.getparent()
                    while parent is not None and el.getprevious() is not None:
                        del parent[0]
            if max_bytes and fh.tell() > max_bytes:
                rec["truncated"] = True
                break
    except Exception as e:  # noqa: BLE001
        rec["error"] = str(e)[:300]
    rec["gml_ns"] = sorted(rec["gml_ns"])
    for k in ("members", "feature_types", "geom", "carriers", "srs", "srs_dim"):
        rec[k] = dict(rec[k].most_common(40))
    return rec


def analyze(job: tuple) -> list[dict]:
    path, max_bytes = job
    p = Path(path)
    st = p.stat()
    base = {"path": str(p.relative_to(PROJECT)), "size": st.st_size, "mtime": int(st.st_mtime)}
    out = []
    try:
        if zipfile.is_zipfile(p):
            with zipfile.ZipFile(p) as z:
                for info in z.infolist():
                    if info.filename.lower().endswith((".gml", ".xml")) and not info.is_dir():
                        with z.open(info) as fh:
                            r = analyze_stream(fh, max_bytes)
                        out.append({**base, "member": info.filename, "member_size": info.file_size, **r})
        else:
            with p.open("rb") as fh:
                out.append({**base, "member": None, **analyze_stream(fh, max_bytes)})
    except Exception as e:  # noqa: BLE001
        out.append({**base, "member": None, "error": str(e)[:300]})
    return out


def files() -> list[Path]:
    result = []
    for root in ROOTS:
        for p in root.rglob("*"):
            if p.is_file() and p.suffix.lower() in (".gml", ".xml", ".zip") and "_inventory" not in p.parts:
                if p.name.startswith("capabilities-"):
                    continue
                result.append(p)
    return result


def report() -> None:
    recs = [json.loads(l) for l in OUT.open()]
    docs = [r for r in recs if r.get("root")]
    print(f"documents: {len(docs)} (errors {sum(1 for r in recs if r.get('error'))})")
    agg = collections.Counter()
    for r in docs:
        for k, v in r["geom"].items():
            agg[k] += 1
    print("documents containing element:", ", ".join(f"{k}={v}" for k, v in agg.most_common()))
    arcs = [r for r in docs if any(k in r["geom"] for k in
                                    ("Arc", "ArcString", "Circle", "ArcByCenterPoint", "CircleByCenterPoint",
                                     "ArcByBulge", "ArcStringByBulge"))]
    print(f"\ndocuments with arcs: {len(arcs)}")
    for r in arcs[:25]:
        print("  ", r["path"], r.get("member") or "", {k: v for k, v in r["geom"].items() if "Arc" in k or "Circle" in k})
    ver = collections.Counter(tuple(r["gml_ns"]) for r in docs)
    print("\nGML namespaces:", ver.most_common())
    roots = collections.Counter(f"{r['root_ns'].rsplit('/', 2)[-2:] if r['root_ns'] else ''}:{r['root']}" for r in docs)
    print("roots:", roots.most_common(12))


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--workers", type=int, default=8)
    ap.add_argument("--max-mb", type=float, default=0)
    ap.add_argument("--report", action="store_true")
    args = ap.parse_args()
    if args.report:
        report()
        return
    cache = {}
    if OUT.exists():
        for l in OUT.open():
            r = json.loads(l)
            cache.setdefault((r["path"], r["size"], r["mtime"]), []).append(r)
    todo, keep = [], []
    for p in files():
        st = p.stat()
        key = (str(p.relative_to(PROJECT)), st.st_size, int(st.st_mtime))
        if key in cache:
            keep.extend(cache[key])
        else:
            todo.append((str(p), int(args.max_mb * 1e6)))
    todo.sort(key=lambda j: Path(j[0]).stat().st_size)
    print(f"{len(keep)} cached records, analyzing {len(todo)} files", file=sys.stderr)
    with ProcessPoolExecutor(max_workers=args.workers) as pool:
        for i, recs in enumerate(pool.map(analyze, todo, chunksize=1), 1):
            keep.extend(recs)
            if i % 100 == 0 or i == len(todo):
                print(f"  {i}/{len(todo)}", file=sys.stderr, flush=True)
    with OUT.open("w") as f:
        for r in keep:
            f.write(json.dumps(r, ensure_ascii=False) + "\n")
    print(f"-> {OUT}", file=sys.stderr)


if __name__ == "__main__":
    main()
