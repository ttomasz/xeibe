#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Extract inline GML geometry snippets from GDAL's autotest scripts and record
GDAL's result for each (reference oracle).

Step 1 (host):  collect string literals that look like GML geometry.
Step 2 (container, via scripts/gdal): run ogr.CreateGeometryFromGML on each and
record ISO WKT or failure.

    scripts/corpus/extract_gdal_geom_cases.py

Output: tests/data/gdal/gml_geometry_cases.jsonl (MIT, see LICENSE-GDAL.txt).
"""
import ast
import json
import subprocess
import sys
from pathlib import Path

PROJECT = Path(__file__).resolve().parents[2]
SRC = PROJECT / "example_data" / "gdal-autotest-src"
SCRIPTS = ["autotest/ogr/ogr_gml_geom.py"]
OUT = PROJECT / "tests" / "data" / "gdal" / "gml_geometry_cases.jsonl"

ORACLE = r'''
import json, sys
from osgeo import gdal, ogr
gdal.UseExceptions()
cases = [json.loads(l) for l in sys.stdin]
for c in cases:
    res = {"wkt": None, "error": None}
    try:
        with gdal.quiet_errors():
            g = ogr.CreateGeometryFromGML(c["gml"])
        if g is None:
            res["error"] = gdal.GetLastErrorMsg() or "null geometry"
        else:
            res["wkt"] = g.ExportToIsoWkt()
            res["type"] = ogr.GeometryTypeToName(g.GetGeometryType())
    except Exception as e:
        res["error"] = str(e)
    c["gdal"] = res
    print(json.dumps(c, ensure_ascii=False))
'''


def looks_like_gml(s: str) -> bool:
    s = s.strip()
    return s.startswith("<") and s.endswith(">") and len(s) < 20000 and (
        "gml" in s or "coordinates" in s or "pos" in s)


def main() -> None:
    commit = subprocess.run(["git", "-C", str(SRC), "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip()
    cases, seen = [], set()
    for rel in SCRIPTS:
        tree = ast.parse((SRC / rel).read_text())
        for node in ast.walk(tree):
            if isinstance(node, ast.Constant) and isinstance(node.value, str) and looks_like_gml(node.value):
                gml = node.value.strip()
                if gml in seen:
                    continue
                seen.add(gml)
                cases.append({"id": f"{Path(rel).stem}:{node.lineno}", "source": f"OSGeo/gdal@{commit[:12]}/{rel}#L{node.lineno}", "gml": gml})
    stdin = "".join(json.dumps(c, ensure_ascii=False) + "\n" for c in cases)
    result = subprocess.run([str(PROJECT / "scripts" / "gdal"), "python3", "-c", ORACLE],
                            input=stdin, capture_output=True, text=True)
    if result.returncode != 0:
        sys.exit(result.stderr)
    lines = [l for l in result.stdout.splitlines() if l.startswith("{")]
    OUT.write_text("\n".join(lines) + "\n")
    ok = sum(1 for l in lines if json.loads(l)["gdal"]["wkt"])
    print(f"{len(lines)} cases ({ok} parsed by GDAL, {len(lines) - ok} rejected) -> {OUT}", file=sys.stderr)


if __name__ == "__main__":
    main()
