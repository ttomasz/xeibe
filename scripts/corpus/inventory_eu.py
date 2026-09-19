#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Inventory data.europa.eu datasets with GML or WFS distributions.

Writes one JSON line per (dataset, distribution) to
example_data/_inventory/data_europa_eu.jsonl. Only metadata is fetched.
"""
import json
import sys
import time
import urllib.parse
import urllib.request
from pathlib import Path

API = "https://data.europa.eu/api/hub/search/search"
OUT = Path(__file__).resolve().parents[2] / "example_data" / "_inventory" / "data_europa_eu.jsonl"
FORMATS = ["GML", "WFS_SRVC"]
PAGE = 100


def text(v):
    if isinstance(v, dict):
        return v.get("en") or next(iter(v.values()), None)
    return v


def fetch(fmt: str, page: int) -> dict:
    params = {
        "filter": "dataset",
        "facets": json.dumps({"format": [fmt]}),
        "limit": PAGE,
        "page": page,
    }
    url = API + "?" + urllib.parse.urlencode(params)
    req = urllib.request.Request(url, headers={"User-Agent": "xeibe-corpus-inventory"})
    for attempt in range(5):
        try:
            with urllib.request.urlopen(req, timeout=120) as r:
                return json.load(r)["result"]
        except Exception as e:  # noqa: BLE001
            print(f"retry {fmt} p{page}: {e}", file=sys.stderr)
            time.sleep(5 * (attempt + 1))
    raise RuntimeError(f"failed {fmt} page {page}")


def main() -> None:
    seen = set()
    n = 0
    with OUT.open("w") as out:
        for fmt in FORMATS:
            page = 0
            while True:
                result = fetch(fmt, page)
                items = result.get("results", [])
                if not items:
                    break
                for ds in items:
                    for dist in ds.get("distributions", []):
                        dfmt = (dist.get("format") or {}).get("id")
                        if dfmt not in FORMATS:
                            continue
                        key = (ds["id"], dist.get("id"))
                        if key in seen:
                            continue
                        seen.add(key)
                        lic = dist.get("license") or {}
                        rec = {
                            "dataset": ds["id"],
                            "title": text(ds.get("title")),
                            "country": (ds.get("country") or {}).get("id"),
                            "catalog": (ds.get("catalog") or {}).get("id"),
                            "publisher": text((ds.get("publisher") or {}).get("name")),
                            "format": dfmt,
                            "download_url": (dist.get("download_url") or [None])[0],
                            "access_url": (dist.get("access_url") or [None])[0],
                            "license": lic.get("id") or lic.get("resource"),
                            "license_label": text(lic.get("label")),
                            "byte_size": dist.get("byte_size"),
                            "media_type": dist.get("media_type"),
                            "landing_page": [lp.get("resource") if isinstance(lp, dict) else lp
                                             for lp in (ds.get("landing_page") or [])][:1],
                        }
                        out.write(json.dumps(rec, ensure_ascii=False) + "\n")
                        n += 1
                print(f"{fmt} page {page}: total {n}", file=sys.stderr, flush=True)
                if (page + 1) * PAGE >= result.get("count", 0):
                    break
                page += 1
                time.sleep(1)
    print(f"done: {n} distributions -> {OUT}", file=sys.stderr)


if __name__ == "__main__":
    main()
