#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["requests>=2.32"]
# ///
"""Check whether EPSG has published a Dataset release newer than the one in
`crates/xeibe-crs`.

Uses the public GeoRepository API at <https://apps.epsg.org/api/v1/>, which
needs no account. Note that only the *check* is open: the API has no bulk
download endpoint (just per-object `/export`), so fetching the archive itself
still goes through the registered-user download page. See `--help` output and
`docs/geometry.md`.

    scripts/check_epsg_release.py            # human-readable, exit 0/1
    scripts/check_epsg_release.py --json     # machine-readable
    scripts/check_epsg_release.py --github   # append to $GITHUB_OUTPUT

Exit status is 0 when up to date, 1 when a newer release exists, 2 on error,
so it doubles as a shell guard.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from pathlib import Path

import requests

API = "https://apps.epsg.org/api/v1/VersionHistory"
DOWNLOAD_PAGE = "https://epsg.org/download-dataset.html"
REPO = Path(__file__).resolve().parent.parent
TABLE = REPO / "crates" / "xeibe-crs" / "src" / "table.rs"


def committed() -> tuple[str, str]:
    """The EPSG version and date the generated table was built from."""
    if not TABLE.exists():
        sys.exit(f"{TABLE} does not exist; run scripts/gen_crs_tables.py first")
    text = TABLE.read_text()
    version = re.search(r'pub const EPSG_VERSION: &str = "([^"]+)"', text)
    date = re.search(r'pub const EPSG_DATE: &str = "([^"]+)"', text)
    if not version or not date:
        sys.exit(f"{TABLE} has no EPSG_VERSION/EPSG_DATE; regenerate it")
    return version.group(1), date.group(1)


def sort_key(version: str) -> tuple:
    """Order EPSG version strings.

    They are not plain numbers: `13.005` and `13.101` are both v13, there are
    letter suffixes (`12.059a`), and v13 runs two parallel streams. Compare
    field by field, splitting digits from letters, so `12.059` < `12.059a` and
    `13.005` < `13.101`.
    """
    key: list = []
    for field in version.split("."):
        digits = re.match(r"(\d*)(.*)", field)
        key.append((int(digits.group(1) or 0), digits.group(2)))
    return tuple(key)


def latest_releases(count: int = 10, timeout: float = 30.0) -> list[dict]:
    params = {
        "includeDeprecated": "false",
        "searchRemark": "false",
        "sortField": "-RevisionDate",
        "page": "0",
        "pageSize": str(count),
    }
    response = requests.get(API, params=params, timeout=timeout)
    response.raise_for_status()
    results = response.json().get("Results") or []
    return [
        {
            "version": r["Name"],
            "date": (r.get("RevisionDate") or "")[:10],
            "remarks": r.get("Remarks") or "",
            "url": next((l["href"] for l in r.get("Links") or [] if l.get("rel") == "self"), None),
        }
        for r in results
    ]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--json", action="store_true", help="write a JSON object to stdout")
    ap.add_argument("--github", action="store_true", help="append key=value pairs to $GITHUB_OUTPUT")
    ap.add_argument("--count", type=int, default=10, help="how many releases to fetch (default 10)")
    args = ap.parse_args()

    current, current_date = committed()
    try:
        releases = latest_releases(args.count)
    except requests.RequestException as exc:
        print(f"could not reach the EPSG API: {exc}", file=sys.stderr)
        return 2
    if not releases:
        print("the EPSG API returned no releases", file=sys.stderr)
        return 2

    # Compare on revision date first, version second.
    #
    # Version order alone is not enough: EPSG runs parallel streams, so v13 has
    # both 13.001-13.005 and 13.101-13.103, and a 13.006 published after 13.103
    # would sort *lower* and be missed. The revision date is monotonic in real
    # time, so it catches those; the version only breaks ties within a date
    # (13.005 and 13.101 were both released on 2026-08-05).
    def newer_than_committed(release: dict) -> bool:
        if release["date"] != current_date:
            return release["date"] > current_date
        return sort_key(release["version"]) > sort_key(current)

    newest = max(releases, key=lambda r: (r["date"], sort_key(r["version"])))
    newer = sorted(
        (r for r in releases if newer_than_committed(r)),
        key=lambda r: (r["date"], sort_key(r["version"])),
    )
    update = bool(newer)

    result = {
        "current": current,
        "latest": newest["version"],
        "latest_date": newest["date"],
        "latest_remarks": newest["remarks"],
        "latest_url": newest["url"],
        "update_available": update,
        "newer_versions": [r["version"] for r in newer],
        "download_page": DOWNLOAD_PAGE,
    }

    if args.json:
        print(json.dumps(result, indent=2))
    else:
        print(f"committed: EPSG v{current}")
        print(f"latest:    EPSG v{newest['version']} ({newest['date']})")
        if update:
            print(f"\n{len(newer)} newer release(s): {', '.join(result['newer_versions'])}")
            print(f"\n{newest['remarks']}")
            print(f"\nDownload the PostgreSQL and WKT archives from {DOWNLOAD_PAGE}")
            print("then run scripts/gen_crs_tables.py")
        else:
            print("\nup to date")

    if args.github and (out := os.environ.get("GITHUB_OUTPUT")):
        with open(out, "a") as fh:
            for key in ("current", "latest", "latest_date", "update_available"):
                value = result[key]
                fh.write(f"{key}={str(value).lower() if isinstance(value, bool) else value}\n")
            # Remarks can contain anything, so use a heredoc-style delimiter.
            fh.write(f"latest_remarks<<EOF\n{result['latest_remarks']}\nEOF\n")

    return 1 if update else 0


if __name__ == "__main__":
    sys.exit(main())
