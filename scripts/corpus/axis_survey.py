#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Survey real WFS axis-order behaviour from the samples in example_data/wfs/.

For each feature type sampled in several WFS versions, compare the first
coordinate of 1.1.0 / 2.0.0 against 1.0.0: same order, swapped, or a different
feature (not comparable). Grouped by producer, version, srsName form and CRS kind.

    scripts/corpus/axis_survey.py > example_data/wfs/axis_survey.txt
"""
import collections
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2] / "example_data" / "wfs"
GEOGRAPHIC = {"4326", "4258", "4269", "4617", "4674", "4167", "4283", "4619"}
FIRST = re.compile(r'<gml:(?:pos|posList)[^>]*>\s*([-\d.eE]+)\s+([-\d.eE]+)'
                   r'|<gml:coordinates[^>]*>\s*([-\d.eE]+),([-\d.eE]+)')


def producer(head: str, path: str) -> str:
    low = head.lower()
    if "arcgis" in path.lower() or "/mapserver/wfsserver" in path.lower() or "esri" in low:
        return "ArcGIS"
    if 'xmlns:ms="http://mapserver.gis.umn.edu/mapserver"' in low or "mapserver.gis.umn.edu" in low:
        return "MapServer"
    if "geoserver" in low or "xmlns:gs=" in low:
        return "GeoServer"
    if "deegree" in low:
        return "deegree"
    return "other"


def form(s: str) -> str:
    if s.startswith("urn:ogc"):
        return "urn"
    if s.lower().startswith("urn:x-ogc"):
        return "urn:x-ogc"
    if "def/crs" in s:
        return "def/crs"
    if "epsg.xml#" in s:
        return "epsg.xml#"
    if re.match(r"^EPSG:\d+$", s, re.I):
        return "short"
    return "other"


def main() -> None:
    data = collections.defaultdict(dict)
    for p in ROOT.rglob("getfeature-*.xml"):
        b = p.read_bytes()[:3_000_000].decode("utf-8", "replace")
        if "ExceptionReport" in b[:3000]:
            continue
        i = max(b.find("featureMember"), b.find(":member>"))
        body = b[i:] if i > 0 else b
        m, s = FIRST.search(body), re.search(r'srsName="([^"]+)"', body)
        if not m or not s:
            continue
        a, c = (m.group(1), m.group(2)) if m.group(1) else (m.group(3), m.group(4))
        data[p.parent][p.stem.split("-")[1]] = (producer(b[:20000], str(p)), s.group(1), float(a), float(c))
    res, ex = collections.Counter(), {}
    for d, vs in data.items():
        if "1.0.0" not in vs:
            continue
        ref = vs["1.0.0"]
        for v in ("1.1.0", "2.0.0"):
            if v not in vs:
                continue
            prod, srs, a, c = vs[v]
            r = "same" if (a, c) == ref[2:] else "SWAPPED" if (a, c) == (ref[3], ref[2]) else None
            if r is None:
                continue
            code = re.split(r"[:/#]", srs)[-1]
            key = (prod, v, form(srs), "geographic" if code in GEOGRAPHIC else "projected", r)
            res[key] += 1
            ex.setdefault(key, (d.relative_to(ROOT).parts[0], ref[1], ref[2], ref[3], srs, a, c))
    print("producer   WFS   srsName form  CRS kind    vs 1.0.0   n   example (host, 1.0.0 srsName: x y -> this version)")
    for key, n in sorted(res.items()):
        e = ex[key]
        print(f"{key[0]:10} {key[1]} {key[2]:12} {key[3]:10} {key[4]:8} {n:3}  "
              f"{e[0][:30]}  [{e[1][-26:]}] {e[2]:.2f} {e[3]:.2f} -> [{e[4][-26:]}] {e[5]:.2f} {e[6]:.2f}")


if __name__ == "__main__":
    main()
