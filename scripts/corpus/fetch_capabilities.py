#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["lxml>=5"]
# ///
"""Fetch WFS GetCapabilities (2.0.0, 1.1.0, 1.0.0) for a list of endpoints.

    scripts/corpus/fetch_capabilities.py example_data/wfs/endpoints-geoportal-pl.txt [WORKERS]

Saves example_data/wfs/<host>/<path>/capabilities-<version>.xml and writes a
summary (feature types, output formats, paging constraints) to summary.json in
the same directory as the endpoint list.
"""
import json
import re
import sys
import time
from concurrent.futures import ThreadPoolExecutor
import urllib.parse
import urllib.request
from pathlib import Path

from lxml import etree

ROOT = Path(__file__).resolve().parents[2] / "example_data" / "wfs"
VERSIONS = ["2.0.0", "1.1.0", "1.0.0"]


def local(tag: str) -> str:
    return tag.rsplit("}", 1)[-1]


def summarize(xml: bytes) -> dict:
    root = etree.fromstring(xml)
    info = {"root": local(root.tag), "version": root.get("version")}
    if info["root"] in ("ExceptionReport", "ServiceExceptionReport"):
        info["exception"] = " ".join(t.strip() for t in root.itertext() if t.strip())[:300]
        return info
    info["feature_types"] = [
        (ft.findtext("{*}Name") or "").strip() for ft in root.iter("{*}FeatureType")
    ]
    info["output_formats"] = sorted({
        (v.text or "").strip()
        for p in root.iter("{*}Parameter") if (p.get("name") or "").lower() == "outputformat"
        for v in p.iter("{*}Value")
    } | {(f.text or "").strip() for f in root.iter("{*}Format")})
    constraints = {}
    for c in root.iter("{*}Constraint"):
        name = c.get("name")
        val = c.findtext("{*}DefaultValue")
        if name and val is not None:
            constraints[name] = val.strip()
    info["constraints"] = constraints
    info["default_crs"] = sorted({(e.text or "").strip() for e in root.iter()
                                  if isinstance(e.tag, str) and local(e.tag) in ("DefaultCRS", "DefaultSRS", "SRS")})[:10]
    return info


def fetch_endpoint(endpoint: str) -> tuple[str, dict]:
    u = urllib.parse.urlparse(endpoint)
    target = ROOT / u.netloc / re.sub(r"[^A-Za-z0-9._/-]", "_", (u.path + ("_" + u.query if u.query else "")).strip("/"))
    target.mkdir(parents=True, exist_ok=True)
    result = {}
    sep = "&" if "?" in endpoint else "?"
    for version in VERSIONS:
        url = endpoint + sep + urllib.parse.urlencode(
            {"SERVICE": "WFS", "REQUEST": "GetCapabilities", "VERSION": version})
        try:
            req = urllib.request.Request(url, headers={"User-Agent": "xeibe-corpus"})
            with urllib.request.urlopen(req, timeout=60) as r:
                body = r.read(20_000_000)
            (target / f"capabilities-{version}.xml").write_bytes(body)
            result[version] = summarize(body)
        except Exception as e:  # noqa: BLE001
            result[version] = {"error": str(e)[:200]}
        s = result[version]
        print(f"{endpoint} {version}: {s.get('error') or s.get('exception') or len(s.get('feature_types', []))}",
              file=sys.stderr, flush=True)
        time.sleep(0.5)
    return endpoint, result


def main() -> None:
    endpoints_file = Path(sys.argv[1])
    workers = int(sys.argv[2]) if len(sys.argv) > 2 else 1
    endpoints = [l.strip() for l in endpoints_file.read_text().splitlines() if l.strip()]
    with ThreadPoolExecutor(max_workers=workers) as pool:
        summary = dict(pool.map(fetch_endpoint, endpoints))
    out = endpoints_file.with_suffix(".summary.json")
    out.write_text(json.dumps(summary, indent=1, ensure_ascii=False))
    print(f"summary -> {out}", file=sys.stderr)


if __name__ == "__main__":
    main()
