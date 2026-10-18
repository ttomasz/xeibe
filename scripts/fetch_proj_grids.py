#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Copy PROJ-data's grids into `target/proj-grids/` for level 3's oracle.

The `scripts/gdal` image (`ubuntu-full`) bundles all of PROJ-data, the grids
`cdn.proj.org` serves, in `/usr/local/gdal-internal/share/proj`. With them in
one directory, PROJ (pyproj, `scripts/gen_transformation_vectors.py --grids`)
counts every published grid as available, as it does with network access,
and the `--ignored` tests of `crates/xeibe-reproject` read them the same way.

    scripts/fetch_proj_grids.py            # about 770 MB, a few seconds

Files already there are kept.
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "target" / "proj-grids"
SOURCE = "/usr/local/gdal-internal/share/proj"


def main() -> int:
    OUT.mkdir(parents=True, exist_ok=True)
    # The container mounts the project read-write at the same path.
    command = f"cp --update=none {SOURCE}/*.tif {SOURCE}/*.json {OUT}/"
    result = subprocess.run([str(ROOT / "scripts" / "gdal"), "sh", "-c", command], check=False)
    if result.returncode != 0:
        return result.returncode
    files = sorted(OUT.iterdir())
    size = sum(f.stat().st_size for f in files)
    print(f"{len(files)} files, {size / 1e6:.0f} MB in {OUT.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
