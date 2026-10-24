#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Release helpers for .github/workflows/release.yml (docs/releasing.md).

    scripts/release.py plan --tag v0.2.0          # what the tag releases; exit 1 if it doesn't match
    scripts/release.py plan --series xeibe-proj   # the same for a dry run, from the checkout
    scripts/release.py notes --series workspace   # the changelog section of the current version
    scripts/release.py crates                     # publishable crates, in publishing order
    scripts/release.py publish-crates [--dry-run] # publish those whose version isn't on crates.io

There are two release series. `v*` tags release the workspace crates on
crates.io, the CLI binaries and the `xeibe` wheels; `xeibe-proj-v*` tags the
`xeibe-proj` wheels. `--github` appends the plan to $GITHUB_OUTPUT.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
INDEX = "https://index.crates.io"
# crates.io lets an account create only a few new crates in a row, then about
# one every 10 minutes; a first release of the whole workspace waits that out.
RATE_LIMIT_WAIT = 11 * 60
RATE_LIMIT_TRIES = 12


@dataclass(frozen=True)
class Series:
    name: str
    tag_prefix: str
    # The crate whose version the tag must carry.
    crate: str
    # The Python package: its crate's manifest and the wheel's name.
    manifest: str
    wheel: str
    changelog: str


SERIES = {
    s.name: s
    for s in [
        Series("workspace", "v", "xeibe-core", "crates/xeibe-py/Cargo.toml", "xeibe", "CHANGELOG.md"),
        Series(
            "xeibe-proj",
            "xeibe-proj-v",
            "xeibe-proj-py",
            "crates/xeibe-proj-py/Cargo.toml",
            "xeibe_proj",
            "crates/xeibe-proj-py/CHANGELOG.md",
        ),
    ]
}


def metadata() -> dict:
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        cwd=REPO,
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    return json.loads(out)


def series_for_tag(tag: str) -> Series:
    # Longest prefix first: `xeibe-proj-v1.0.0` must not read as a `v` tag.
    for s in sorted(SERIES.values(), key=lambda s: -len(s.tag_prefix)):
        if tag.startswith(s.tag_prefix):
            return s
    sys.exit(f"{tag}: not a release tag (v*, xeibe-proj-v*)")


def changelog_section(series: Series, version: str) -> str | None:
    """The text under `## <version>` in the series' changelog."""
    path = REPO / series.changelog
    if not path.exists():
        return None
    section: list[str] | None = None
    for line in path.read_text().splitlines():
        if line.startswith("## "):
            if section is not None:
                break
            if line[3:].strip() == version:
                section = []
        elif section is not None:
            section.append(line)
    return "\n".join(section).strip() if section is not None else None


def plan(args: argparse.Namespace) -> int:
    meta = metadata()
    versions = {p["name"]: p["version"] for p in meta["packages"]}
    series = series_for_tag(args.tag) if args.tag else SERIES[args.series]
    version = versions[series.crate]
    errors = []
    if args.tag and args.tag != series.tag_prefix + version:
        errors.append(f"tag {args.tag} doesn't match {series.crate}'s version {version}")
    if series.name == "workspace":
        # The crates share the workspace version, except those that have their
        # own (docs/releasing.md).
        own = {"xeibe-crs", "xeibe-esri", "xeibe-proj-py"}
        for name, v in sorted(versions.items()):
            if name not in own and v != version:
                errors.append(f"{name} is at {v}, not the workspace version {version}")
    if changelog_section(series, version) is None:
        errors.append(f"{series.changelog} has no `## {version}` section")
    for e in errors:
        print(f"error: {e}", file=sys.stderr)
    out = {
        "series": series.name,
        "version": version,
        "tag": series.tag_prefix + version,
        "manifest": series.manifest,
        "wheel": series.wheel,
    }
    for k, v in out.items():
        print(f"{k}={v}")
    if args.github and (path := os.environ.get("GITHUB_OUTPUT")):
        with open(path, "a") as f:
            f.writelines(f"{k}={v}\n" for k, v in out.items())
    # A dry run reports what's wrong, but goes on to build.
    return 1 if errors and args.tag else 0


def notes(args: argparse.Namespace) -> int:
    series = SERIES[args.series]
    version = {p["name"]: p["version"] for p in metadata()["packages"]}[series.crate]
    text = changelog_section(series, version)
    if text is None:
        sys.exit(f"{series.changelog} has no `## {version}` section")
    print(text)
    return 0


def publishable() -> list[dict]:
    """The crates that go to crates.io, each after the crates it depends on.

    Dev-dependencies count: crates.io needs every dependency with a version to
    exist, and cargo drops only those without one (xeibe-testkit).
    """
    packages = [p for p in metadata()["packages"] if p["publish"] != []]
    by_name = {p["name"]: p for p in packages}
    order: list[dict] = []
    seen: set[str] = set()

    def visit(p: dict, path: tuple[str, ...]) -> None:
        if p["name"] in seen:
            return
        if p["name"] in path:
            sys.exit(f"dependency cycle: {' -> '.join(path + (p['name'],))}")
        for d in p["dependencies"]:
            if d["name"] in by_name and d["name"] != p["name"]:
                visit(by_name[d["name"]], path + (p["name"],))
        seen.add(p["name"])
        order.append(p)

    for p in sorted(packages, key=lambda p: p["name"]):
        visit(p, ())
    return order


def index_path(name: str) -> str:
    # https://doc.rust-lang.org/cargo/reference/registry-index.html#index-files
    n = name.lower()
    if len(n) <= 2:
        return f"{len(n)}/{n}"
    if len(n) == 3:
        return f"3/{n[0]}/{n}"
    return f"{n[:2]}/{n[2:4]}/{n}"


def released(name: str) -> set[str]:
    """The versions of a crate on crates.io, without build metadata."""
    try:
        with urllib.request.urlopen(f"{INDEX}/{index_path(name)}", timeout=30) as r:
            lines = r.read().decode().splitlines()
    except urllib.error.HTTPError as e:
        if e.code == 404:
            return set()
        raise
    return {json.loads(line)["vers"].split("+")[0] for line in lines if line}


def crates(_: argparse.Namespace) -> int:
    for p in publishable():
        print(p["name"], p["version"])
    return 0


def publish_crates(args: argparse.Namespace) -> int:
    todo = [p for p in publishable() if p["version"].split("+")[0] not in released(p["name"])]
    if not todo:
        print("every crate's version is on crates.io already")
        return 0
    for p in todo:
        print(f"{'would publish' if args.dry_run else 'publishing'} {p['name']} {p['version']}", flush=True)
    if args.dry_run:
        return 0
    for p in todo:
        for attempt in range(1, RATE_LIMIT_TRIES + 1):
            # --no-verify: the workflow's test job has already built every
            # crate from its package (`cargo package`), at this commit.
            r = subprocess.run(
                ["cargo", "publish", "--no-verify", "-p", p["name"]],
                cwd=REPO,
                capture_output=True,
                text=True,
            )
            sys.stdout.write(r.stdout)
            sys.stderr.write(r.stderr)
            if r.returncode == 0:
                break
            if "429" not in r.stderr or attempt == RATE_LIMIT_TRIES:
                print(f"error: publishing {p['name']} failed", file=sys.stderr)
                return 1
            print(f"rate-limited; retrying {p['name']} in {RATE_LIMIT_WAIT // 60} min", flush=True)
            time.sleep(RATE_LIMIT_WAIT)
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="command", required=True)
    p = sub.add_parser("plan", help="what a tag (or a dry run of a series) releases")
    g = p.add_mutually_exclusive_group(required=True)
    g.add_argument("--tag", help="the pushed tag, e.g. v0.2.0 or xeibe-proj-v0.1.0")
    g.add_argument("--series", choices=SERIES, help="a dry run: the series at the checkout's versions")
    p.add_argument("--github", action="store_true", help="append key=value pairs to $GITHUB_OUTPUT")
    p.set_defaults(func=plan)
    p = sub.add_parser("notes", help="print the changelog section of the series' current version")
    p.add_argument("--series", choices=SERIES, required=True)
    p.set_defaults(func=notes)
    p = sub.add_parser("crates", help="list the publishable crates in publishing order")
    p.set_defaults(func=crates)
    p = sub.add_parser("publish-crates", help="publish the crates whose version isn't on crates.io")
    p.add_argument("--dry-run", action="store_true", help="only list what would be published")
    p.set_defaults(func=publish_crates)
    args = ap.parse_args()
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
