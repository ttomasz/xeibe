#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Inventory dane.gov.pl resources that are (or likely contain) GML, or are WFS.

Writes example_data/_inventory/dane_gov_pl.jsonl. Only metadata is fetched.
"""
import json
import sys
import time
import urllib.parse
import urllib.request
from pathlib import Path

API = "https://api.dane.gov.pl/1.4/resources"
OUT = Path(__file__).resolve().parents[2] / "example_data" / "_inventory" / "dane_gov_pl.jsonl"
QUERIES = [
    {"format[terms]": "gml"},
    {"format[terms]": "wfs"},
    {"format[terms]": "zip", "q": "gml"},
    {"format[terms]": "xml", "q": "gml"},
]


def fetch(params: dict, page: int) -> dict:
    query = dict(params, page=page, per_page=100)
    url = API + "?" + urllib.parse.urlencode(query)
    req = urllib.request.Request(url, headers={"Accept": "application/json", "User-Agent": "xeibe-corpus-inventory"})
    for attempt in range(5):
        try:
            with urllib.request.urlopen(req, timeout=120) as r:
                return json.load(r)
        except Exception as e:  # noqa: BLE001
            print(f"retry {query}: {e}", file=sys.stderr)
            time.sleep(5 * (attempt + 1))
    raise RuntimeError(f"failed {query}")


def main() -> None:
    seen = set()
    with OUT.open("w") as out:
        for params in QUERIES:
            page = 1
            while True:
                d = fetch(params, page)
                items = d.get("data", [])
                for r in items:
                    if r["id"] in seen:
                        continue
                    seen.add(r["id"])
                    a = r["attributes"]
                    ds = ((r.get("relationships") or {}).get("dataset") or {}).get("data") or {}
                    out.write(json.dumps({
                        "resource": r["id"],
                        "dataset": ds.get("id"),
                        "title": a.get("title"),
                        "format": a.get("format"),
                        "query": params,
                        "link": a.get("link"),
                        "download_url": a.get("download_url"),
                        "file_size": a.get("file_size"),
                        "media_type": a.get("media_type"),
                        "modified": a.get("modified"),
                    }, ensure_ascii=False) + "\n")
                count = d.get("meta", {}).get("count", 0)
                print(f"{params} page {page}: {len(seen)} total (count {count})", file=sys.stderr, flush=True)
                if page * 100 >= count or not items:
                    break
                page += 1
                time.sleep(1)
    print(f"done: {len(seen)} resources -> {OUT}", file=sys.stderr)


if __name__ == "__main__":
    main()
