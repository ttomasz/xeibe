#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Fetch small GetFeature responses (a few features) per endpoint, feature type and WFS version.

    scripts/corpus/fetch_wfs_samples.py example_data/wfs/endpoints-geoportal-pl.summary.json [--types 3] [--count 3]

Uses the summary written by fetch_capabilities.py. Saves
example_data/wfs/<host>/<path>/<type>/getfeature-<version>.xml plus request URLs
in requests.tsv. Each version is requested with its default output format
(2.0 → GML 3.2, 1.1 → GML 3.1.1, 1.0 → GML 2).
"""
import argparse
import json
import re
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor
import urllib.parse
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2] / "example_data" / "wfs"


def safe(s: str) -> str:
    return re.sub(r"[^A-Za-z0-9._-]", "_", s)


LOCK = threading.Lock()


def sample_endpoint(endpoint: str, versions: dict, args, log) -> None:
    u = urllib.parse.urlparse(endpoint)
    base = ROOT / u.netloc / re.sub(r"[^A-Za-z0-9._/-]", "_", (u.path + ("_" + u.query if u.query else "")).strip("/"))
    sep = "&" if "?" in endpoint else "?"
    types = (versions.get("2.0.0") or {}).get("feature_types") or \
            (versions.get("1.1.0") or {}).get("feature_types") or \
            (versions.get("1.0.0") or {}).get("feature_types") or []
    for ft in types[: args.types]:
        for version in ("2.0.0", "1.1.0", "1.0.0"):
            v = versions.get(version, {})
            if not v or "error" in v or "exception" in v:
                continue
            params = {"SERVICE": "WFS", "REQUEST": "GetFeature", "VERSION": version}
            if version == "2.0.0":
                params.update(TYPENAMES=ft, COUNT=args.count)
            else:
                params.update(TYPENAME=ft, MAXFEATURES=args.count)
            url = endpoint + sep + urllib.parse.urlencode(params)
            target = base / safe(ft) / f"getfeature-{version}.xml"
            if target.exists():
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            status = "ok"
            try:
                req = urllib.request.Request(url, headers={"User-Agent": "xeibe-corpus"})
                with urllib.request.urlopen(req, timeout=120) as r:
                    body = r.read(50_000_000)
                target.write_bytes(body)
                if b"ExceptionReport" in body[:2000]:
                    status = "exception"
            except Exception as e:  # noqa: BLE001
                status = f"error: {str(e)[:120]}"
            with LOCK:
                log.write(f"{time.strftime('%Y-%m-%dT%H:%M:%S')}\t{status}\t{target.relative_to(ROOT)}\t{url}\n")
                log.flush()
                print(f"{status:10} {version} {ft} ({u.netloc})", file=sys.stderr, flush=True)
            time.sleep(0.5)


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("summary", type=Path)
    p.add_argument("--types", type=int, default=3, help="max feature types per endpoint")
    p.add_argument("--count", type=int, default=3)
    p.add_argument("--workers", type=int, default=1)
    args = p.parse_args()
    summary = json.loads(args.summary.read_text())
    log = (ROOT / "requests.tsv").open("a")
    with ThreadPoolExecutor(max_workers=args.workers) as pool:
        list(pool.map(lambda kv: sample_endpoint(kv[0], kv[1], args, log), summary.items()))


if __name__ == "__main__":
    main()
