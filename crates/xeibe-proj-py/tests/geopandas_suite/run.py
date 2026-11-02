#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""GeoPandas' own test suite on xeibe_proj, through install_as_pyproj().

GeoPandas' tests, which its wheel includes, run twice: with pyproj, and
with `xeibe_geopandas_shim` installing the shim. Both run in the
environment `pytest_env.py` declares (GeoPandas at the pinned version, its
required dependencies but pyogrio, pyarrow, pyproj 3.8.0), with the
xeibe-proj wheel added (built unless --wheel is given). uv builds the
environment and keeps it in its own cache, not under target/, whose files
CI's rust-cache deletes. The tests that pass
with pyproj are the measure (docs/pyproj-phase3.md, GeoPandas):

- each must be in `pass-list.txt` or in `known.txt`, under a comment that
  says why it fails with the shim; a test in neither fails the run;
- a listed test that fails with the shim is a regression, and the run
  exits 1;
- a test in `known.txt` that passes now is progress: `--update` moves it
  to the pass list, and adds other new passes.

pyogrio and matplotlib are left out, so the file and plotting tests skip in
both runs. The network is off (PROJ_NETWORK=OFF). The JUnit reports go to
target/geopandas-suite.

    crates/xeibe-proj-py/tests/geopandas_suite/run.py            # check
    crates/xeibe-proj-py/tests/geopandas_suite/run.py --update   # rewrite the lists
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]
WORK = ROOT / "target" / "geopandas-suite"
WHEELS = ROOT / "target" / "proj-py-wheels"
PASS_LIST = HERE / "pass-list.txt"
KNOWN = HERE / "known.txt"
# pytest, in the environment its metadata declares.
PYTEST_ENV = HERE / "pytest_env.py"


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


def run(wheel: Path, shim: bool, extra: list[str]) -> Path:
    report = WORK / ("shim.xml" if shim else "pyproj.xml")
    command = ["uv", "run", "--quiet", "--no-project", "--isolated", "--python", "3.12",
               "--with", str(wheel), "--script", str(PYTEST_ENV),
               "--pyargs", "geopandas",
               "-q", "--tb=no", "-p", "no:cacheprovider", f"--junitxml={report}"]
    if shim:
        command += ["-p", "xeibe_geopandas_shim"]
    command += extra
    env = {**os.environ, "PROJ_NETWORK": "OFF", "PYTHONPATH": str(HERE)}
    report.unlink(missing_ok=True)
    print(f"GeoPandas' tests with {'the shim' if shim else 'pyproj'}:", flush=True)
    # pytest exits 1 when tests fail, which some do with pyproj too: the
    # lists decide.
    subprocess.run(command, cwd=WORK, env=env, check=False)
    if not report.exists():
        sys.exit("pytest wrote no report: see its output above")
    return report


def passed(report: Path) -> set[str]:
    out: set[str] = set()
    for case in ET.parse(report).iter("testcase"):
        if any(child.tag in ("failure", "error", "skipped") for child in case):
            continue
        # The class name is relative to site-packages, pytest_env.py's
        # rootdir; a path through site-packages is cut off all the same.
        module = case.get("classname", "").rpartition("site-packages.")[2]
        out.add(f"{module}::{case.get('name')}")
    return out


def read_known() -> list[tuple[list[str], list[str]]]:
    """`known.txt`'s blocks, separated by blank lines: the comment lines that
    give the reason, and the test IDs under them."""
    blocks: list[tuple[list[str], list[str]]] = []
    text = KNOWN.read_text() if KNOWN.exists() else ""
    for block in text.strip().split("\n\n"):
        lines = [ln.strip() for ln in block.splitlines() if ln.strip()]
        if not lines:
            continue
        reason = [ln for ln in lines if ln.startswith("#")]
        tests = [ln for ln in lines if not ln.startswith("#")]
        if not reason:
            sys.exit(f"{KNOWN.name}: {tests[0]} has no reason above it")
        blocks.append((reason, tests))
    return blocks


def write_known(blocks: list[tuple[list[str], list[str]]]) -> None:
    text = "\n\n".join("\n".join(reason + tests) for reason, tests in blocks if tests)
    KNOWN.write_text(text + "\n")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--update", action="store_true", help="record new passes in the lists")
    parser.add_argument("--wheel", type=Path, help="a built xeibe-proj wheel (default: build one)")
    parser.add_argument("pytest_args", nargs="*", help="passed to pytest, e.g. -k crs")
    args = parser.parse_args()

    WORK.mkdir(parents=True, exist_ok=True)
    wheel = args.wheel.resolve() if args.wheel else build()
    baseline = passed(run(wheel, False, args.pytest_args))
    now = passed(run(wheel, True, args.pytest_args))
    listed = set(PASS_LIST.read_text().splitlines()) if PASS_LIST.exists() else set()
    blocks = read_known()
    known = {t for _, tests in blocks for t in tests}

    regressed = sorted(listed - now)
    fixed = sorted(t for t in known if t in now)
    new = sorted(now - listed - known)
    unexplained = sorted(baseline - now - listed - known)
    print(f"\n{len(now & baseline)} of the {len(baseline)} GeoPandas tests that pass with pyproj "
          f"pass with the shim ({len(listed)} listed, {len(known)} known to fail)")
    if args.update:
        PASS_LIST.write_text("".join(f"{t}\n" for t in sorted(listed | (now & baseline))))
        print(f"wrote {PASS_LIST.relative_to(ROOT)}")
        if fixed:
            write_known([(reason, [t for t in tests if t not in now]) for reason, tests in blocks])
            print(f"wrote {KNOWN.relative_to(ROOT)}")
    else:
        if new or fixed:
            print("passing but not listed (run with --update):")
            print("".join(f"  {t}\n" for t in sorted(set(new) | set(fixed))), end="")
    failed = False
    if args.pytest_args:
        return
    if unexplained:
        print(f"FAILING WITH THE SHIM, in neither list (add to {KNOWN.name} with a reason):")
        print("".join(f"  {t}\n" for t in unexplained), end="")
        failed = True
    if regressed:
        print("REGRESSED (listed, now failing):")
        print("".join(f"  {t}\n" for t in regressed), end="")
        failed = True
    if failed:
        sys.exit(1)


if __name__ == "__main__":
    main()
