#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""pyproj's own test suite against xeibe_proj, measured by a pass list.

Fetches pyproj's sources at the pinned tag into target/pyproj-src, builds
the xeibe-proj wheel (unless --wheel is given), and runs pyproj's tests with
`xeibe_pyproj_shim` aliasing pyproj to xeibe_proj. The IDs of the tests
that pass are compared with `pass-list.txt` next to this script:

- a listed test that fails is a regression, and the run exits 1;
- a test that passes but isn't listed is progress: `--update` adds it.

The list only grows (docs/pyproj-compat.md, Testing). Tests marked
`network` or `cli` are not run: they need a network connection or pyproj's
command line. The network is off (PROJ_NETWORK=OFF), as in pyproj's tests.

    crates/xeibe-proj-py/tests/pyproj_suite/run.py            # check
    crates/xeibe-proj-py/tests/pyproj_suite/run.py --update   # rewrite the list
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

PYPROJ_TAG = "3.8.0"
HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]
SRC = ROOT / "target" / "pyproj-src"
WHEELS = ROOT / "target" / "proj-py-wheels"
REPORT = ROOT / "target" / "pyproj-suite.xml"
PASS_LIST = HERE / "pass-list.txt"
# pyproj's test dependencies (its pyproject.toml, group "test").
# certifi is a dependency of pyproj itself. pytest is pinned because the
# shim subclasses its Module collector.
TEST_DEPS = ["pytest==9.1.1", "numpy", "packaging", "pandas", "shapely", "xarray", "certifi"]


def fetch() -> None:
    if (SRC / "test").is_dir():
        tag = subprocess.run(
            ["git", "-C", str(SRC), "describe", "--tags"], capture_output=True, text=True
        ).stdout.strip()
        if tag == PYPROJ_TAG:
            return
        sys.exit(f"{SRC} holds pyproj {tag or '?'}, not {PYPROJ_TAG}: remove it")
    subprocess.run(
        ["git", "clone", "--quiet", "--depth", "1", "--branch", PYPROJ_TAG,
         "https://github.com/pyproj4/pyproj", str(SRC)],
        check=True,
    )


def build() -> Path:
    subprocess.run(
        ["uvx", "maturin", "build", "--quiet", "--release",
         "-m", str(ROOT / "crates/xeibe-proj-py/Cargo.toml"), "-o", str(WHEELS)],
        check=True,
    )
    wheels = sorted(WHEELS.glob("xeibe_proj-*.whl"), key=lambda p: p.stat().st_mtime)
    if not wheels:
        sys.exit(f"no wheel in {WHEELS}")
    return wheels[-1]


def run(wheel: Path, extra: list[str]) -> None:
    command = ["uv", "run", "--quiet", "--no-project", "--isolated", "--python", "3.12",
               "--with", str(wheel)]
    for dep in TEST_DEPS:
        command += ["--with", dep]
    command += [
        "python", "-m", "pytest", "test",
        "-p", "xeibe_pyproj_shim",
        "-m", "not network and not cli",
        # Runs pyproj's docstrings, which the shim's modules don't have.
        "--deselect", "test/test_doctest_wrapper.py::test_doctests",
        "-q", "--tb=no", "-p", "no:cacheprovider", "--continue-on-collection-errors",
        f"--junitxml={REPORT}",
        *extra,
    ]
    env = {**os.environ, "PROJ_NETWORK": "OFF", "PYTHONPATH": str(HERE)}
    REPORT.unlink(missing_ok=True)
    # pytest exits 1 when tests fail, which most do: the list decides.
    subprocess.run(command, cwd=SRC, env=env, check=False)
    if not REPORT.exists():
        sys.exit("pytest wrote no report: see its output above")


def passed() -> set[str]:
    out: set[str] = set()
    for case in ET.parse(REPORT).iter("testcase"):
        if any(child.tag in ("failure", "error", "skipped") for child in case):
            continue
        out.add(f"{case.get('classname')}::{case.get('name')}")
    return out


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--update", action="store_true", help="add newly passing tests to the list")
    parser.add_argument("--wheel", type=Path, help="a built xeibe-proj wheel (default: build one)")
    parser.add_argument("pytest_args", nargs="*", help="passed to pytest, e.g. -k crs")
    args = parser.parse_args()

    fetch()
    run(args.wheel.resolve() if args.wheel else build(), args.pytest_args)
    now = passed()
    listed = set(PASS_LIST.read_text().splitlines()) if PASS_LIST.exists() else set()
    regressed = sorted(listed - now)
    new = sorted(now - listed)
    total = sum(1 for _ in ET.parse(REPORT).iter("testcase"))
    print(f"\n{len(now)} of {total} pyproj tests pass ({len(listed)} listed, {len(new)} new)")
    if args.update:
        PASS_LIST.write_text("".join(f"{t}\n" for t in sorted(listed | now)))
        print(f"wrote {PASS_LIST.relative_to(ROOT)}")
    elif new:
        print("passing but not listed (run with --update):")
        print("".join(f"  {t}\n" for t in new), end="")
    if regressed and not args.pytest_args:
        print("REGRESSED (listed, now failing):")
        print("".join(f"  {t}\n" for t in regressed), end="")
        sys.exit(1)


if __name__ == "__main__":
    main()
