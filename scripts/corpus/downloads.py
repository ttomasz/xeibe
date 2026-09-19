#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Select, probe and download GML files from the portal inventories.

    scripts/corpus/downloads.py select   [--per-host 4] [--max-file-gb 1.5]
    scripts/corpus/downloads.py download [--budget-gb 18] [--workers 6]
    scripts/corpus/downloads.py status

select:   picks a diverse subset of direct file downloads (a few per host, per
          file kind), probes each with HEAD / ranged GET for size and type,
          drops HTML landing pages and oversized files, and writes
          example_data/_inventory/selection.jsonl.
download: downloads the selection into example_data/downloads/<portal>/<host>/,
          at most one request per host at a time, stopping at the budget.
          Every result is appended to example_data/downloads/log.jsonl
          (url, path, bytes, sha256, licence, title). Existing files are skipped.
"""
import argparse
import hashlib
import json
import random
import re
import sys
import threading
import time
import urllib.parse
import urllib.request
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

PROJECT = Path(__file__).resolve().parents[2]
INV = PROJECT / "example_data" / "_inventory"
SELECTION = INV / "selection.jsonl"
DL = PROJECT / "example_data" / "downloads"
LOG = DL / "log.jsonl"
UA = {"User-Agent": "xeibe-corpus (test data collection)"}

OPEN_LICENCE = re.compile(
    r"creativecommons|cc[-_ ]?by|cc[-_]?zero|publicdomain|dl-(by|zero)-de|modellicentie-gratis|"
    r"terms_open|opendata|open-data|nlod|etalab|ogl|licence-ouverte", re.I)


def kind_of(url: str | None) -> str | None:
    if not url:
        return None
    low = url.lower()
    path = urllib.parse.urlparse(low).path
    if "getfeature" in low:
        return None  # handled as WFS endpoints, not file downloads
    for ext in (".gml.gz", ".gml", ".xml", ".zip", ".gz"):
        if path.endswith(ext) or path.endswith(ext.replace(".", "_")):
            return ext.lstrip(".")
    return None


def candidates() -> list[dict]:
    out = []
    for line in (INV / "data_europa_eu.jsonl").open():
        r = json.loads(line)
        if r["format"] != "GML":
            continue
        url = r["download_url"] or r["access_url"]
        k = kind_of(url)
        if k:
            out.append({"portal": "data.europa.eu", "url": url, "kind": k, "title": r["title"],
                        "licence": r["license"], "country": r["country"], "dataset": r["dataset"]})
    for line in (INV / "dane_gov_pl.jsonl").open():
        r = json.loads(line)
        url = r["download_url"] or r["link"]
        if not url or r["format"] == "wfs":
            continue
        out.append({"portal": "dane.gov.pl", "url": url, "kind": r["format"], "title": r["title"],
                    "licence": "dane.gov.pl (see dataset)", "country": "pl", "dataset": r["dataset"],
                    "declared_size": r["file_size"]})
    return out


def probe(url: str) -> dict:
    info = {"final_url": url, "size": None, "content_type": None, "filename": None, "error": None}
    for method, headers in (("HEAD", {}), ("GET", {"Range": "bytes=0-1023"})):
        try:
            req = urllib.request.Request(url, method=method, headers={**UA, **headers})
            with urllib.request.urlopen(req, timeout=60) as r:
                info["final_url"] = r.geturl()
                info["content_type"] = r.headers.get("Content-Type")
                cr = r.headers.get("Content-Range")
                cl = r.headers.get("Content-Length")
                if cr and "/" in cr and cr.rsplit("/", 1)[1].isdigit():
                    info["size"] = int(cr.rsplit("/", 1)[1])
                elif cl and method == "HEAD":
                    info["size"] = int(cl)
                cd = r.headers.get("Content-Disposition") or ""
                m = re.search(r'filename\*?=(?:UTF-8\'\')?"?([^";]+)', cd)
                if m:
                    info["filename"] = urllib.parse.unquote(m.group(1))
                if method == "GET":
                    head = r.read(1024)
                    info["sniff"] = head[:200].decode("utf-8", "replace")
                info["error"] = None
                if info["size"] is not None or method == "GET":
                    return info
        except Exception as e:  # noqa: BLE001
            info["error"] = f"{method}: {str(e)[:150]}"
    return info


def looks_like_page(info: dict) -> bool:
    ct = (info.get("content_type") or "").lower()
    sniff = (info.get("sniff") or "").lstrip().lower()
    return "text/html" in ct or sniff.startswith("<!doctype html") or sniff.startswith("<html")


def cmd_select(args) -> None:
    rng = random.Random(42)
    groups = defaultdict(list)
    for c in candidates():
        host = urllib.parse.urlparse(c["url"]).netloc
        c["host"] = host
        groups[(c["portal"], host, c["kind"])].append(c)
    chosen = []
    for key, items in sorted(groups.items()):
        rng.shuffle(items)
        # prefer open licences, then distinct datasets/titles
        items.sort(key=lambda c: not OPEN_LICENCE.search(c["licence"] or ""))
        seen_titles, picked = set(), []
        limit = args.per_host * (3 if key[0] == "dane.gov.pl" else 1)
        for c in items:
            t = re.sub(r"\d+", "#", (c["title"] or "")[:40])
            if t in seen_titles and len(items) > limit:
                continue
            seen_titles.add(t)
            picked.append(c)
            if len(picked) >= limit * 2:  # probe extra, some will be dropped
                break
        chosen.extend(picked)
    print(f"probing {len(chosen)} candidates from {len(groups)} (portal, host, kind) groups", file=sys.stderr)

    by_host = defaultdict(list)
    for c in chosen:
        by_host[c["host"]].append(c)

    def probe_host(items: list[dict]) -> list[dict]:
        kept, per_kind = [], defaultdict(int)
        limit = args.per_host * (3 if items[0]["portal"] == "dane.gov.pl" else 1)
        for c in items:
            if per_kind[c["kind"]] >= limit:
                continue
            c["probe"] = probe(c["url"])
            time.sleep(0.3)
            size = c["probe"]["size"] or c.get("declared_size")
            if c["probe"]["error"] and not size or looks_like_page(c["probe"]):
                continue
            if size and size > args.max_file_gb * 1e9:
                continue
            c["size"] = size
            kept.append(c)
            per_kind[c["kind"]] += 1
        return kept

    selection = []
    with ThreadPoolExecutor(max_workers=12) as pool:
        for kept in pool.map(probe_host, by_host.values()):
            selection.extend(kept)
    with SELECTION.open("w") as f:
        for c in selection:
            f.write(json.dumps(c, ensure_ascii=False) + "\n")
    known = sum(c["size"] or 0 for c in selection)
    unknown = sum(1 for c in selection if not c["size"])
    print(f"selected {len(selection)} files from {len(by_host)} hosts: {known/1e9:.2f} GB known, "
          f"{unknown} of unknown size -> {SELECTION}", file=sys.stderr)


def safe_name(c: dict) -> str:
    name = c["probe"].get("filename") or Path(urllib.parse.urlparse(c["probe"]["final_url"]).path).name
    name = re.sub(r"[^A-Za-z0-9._-]", "_", urllib.parse.unquote(name))[-120:] or "download"
    tag = hashlib.sha1(c["url"].encode()).hexdigest()[:8]
    return f"{tag}_{name}"


def cmd_download(args) -> None:
    selection = [json.loads(l) for l in SELECTION.open()]
    done = {json.loads(l)["url"] for l in LOG.open()} if LOG.exists() else set()
    budget = args.budget_gb * 1e9
    used = sum(p.stat().st_size for p in DL.rglob("*") if p.is_file() and p.name != "log.jsonl")
    lock = threading.Lock()
    state = {"used": used}
    by_host = defaultdict(list)
    for c in selection:
        if c["url"] not in done:
            by_host[c["host"]].append(c)
    # smallest first within each host so the budget covers as many hosts as possible
    for items in by_host.values():
        items.sort(key=lambda c: c["size"] or 5e8)

    def run_host(items: list[dict]) -> None:
        for c in items:
            with lock:
                if state["used"] + (c["size"] or 0) > budget:
                    continue
            target = DL / c["portal"] / c["host"] / safe_name(c)
            target.parent.mkdir(parents=True, exist_ok=True)
            rec = {k: c.get(k) for k in ("portal", "url", "title", "licence", "country", "dataset", "kind")}
            rec["path"] = str(target.relative_to(PROJECT))
            try:
                h = hashlib.sha256()
                n = 0
                req = urllib.request.Request(c["url"], headers=UA)
                with urllib.request.urlopen(req, timeout=300) as r, target.open("wb") as out:
                    while chunk := r.read(1 << 20):
                        out.write(chunk)
                        h.update(chunk)
                        n += len(chunk)
                        with lock:
                            state["used"] += len(chunk)
                            over = state["used"] > budget
                        if over:
                            raise RuntimeError("budget exceeded")
                rec.update(status="ok", bytes=n, sha256=h.hexdigest())
            except Exception as e:  # noqa: BLE001
                target.unlink(missing_ok=True)
                rec.update(status="error", error=str(e)[:200])
            rec["retrieved"] = time.strftime("%Y-%m-%dT%H:%M:%S")
            with lock:
                with LOG.open("a") as f:
                    f.write(json.dumps(rec, ensure_ascii=False) + "\n")
                print(f"{rec['status']:5} {state['used']/1e9:6.2f} GB  {rec.get('bytes', 0)/1e6:8.1f} MB  "
                      f"{c['host']}  {rec['path'].rsplit('/', 1)[-1][:60]}", file=sys.stderr, flush=True)
            time.sleep(0.5)

    DL.mkdir(parents=True, exist_ok=True)
    with ThreadPoolExecutor(max_workers=args.workers) as pool:
        list(pool.map(run_host, by_host.values()))
    print(f"done: {state['used']/1e9:.2f} GB in {DL}", file=sys.stderr)


def cmd_status(args) -> None:
    if not LOG.exists():
        print("no downloads yet")
        return
    recs = [json.loads(l) for l in LOG.open()]
    ok = [r for r in recs if r["status"] == "ok"]
    by = defaultdict(lambda: [0, 0])
    for r in ok:
        by[r["portal"]][0] += 1
        by[r["portal"]][1] += r["bytes"]
    for p, (n, b) in by.items():
        print(f"{p:16} {n:5} files {b/1e9:7.2f} GB")
    print(f"errors: {len(recs) - len(ok)}")


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("select")
    s.add_argument("--per-host", type=int, default=4)
    s.add_argument("--max-file-gb", type=float, default=1.5)
    d = sub.add_parser("download")
    d.add_argument("--budget-gb", type=float, default=18)
    d.add_argument("--workers", type=int, default=6)
    sub.add_parser("status")
    args = p.parse_args()
    {"select": cmd_select, "download": cmd_download, "status": cmd_status}[args.cmd](args)


if __name__ == "__main__":
    main()
