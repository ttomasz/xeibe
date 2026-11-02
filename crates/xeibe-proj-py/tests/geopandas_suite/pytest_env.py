#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = [
#     "geopandas==1.2.0",
#     "numpy>=2",
#     "packaging",
#     "pandas>=2.2.0",
#     "shapely>=2.1.0",
#     "pyproj==3.8.0",
#     "pyarrow",
#     "pytest==9.1.1",
# ]
#
# [tool.uv]
# # GeoPandas requires pyogrio; without it its file tests skip, in both runs.
# exclude-dependencies = ["pyogrio"]
# ///
"""pytest in the environment GeoPandas' suite runs in, for `run.py`.

The metadata above is that environment: GeoPandas at the pinned version
with its required dependencies (its METADATA) but pyogrio, pyproj at
3.8.0, which needs Python 3.12, pyarrow for the Arrow and GeoParquet
tests, and pytest, pinned as in pyproj's runner.
`run.py` adds the xeibe-proj wheel and runs this with uv, which keeps the
environment in its own cache:

    uv run --isolated --python 3.12 --with xeibe_proj-*.whl --script pytest_env.py ARGS

The arguments are pytest's. The directory GeoPandas is installed in is
the rootdir, so the test IDs in the JUnit report start with `geopandas.`,
as `pass-list.txt` has them, wherever uv put the environment.
"""

import importlib.util
import sys
from pathlib import Path

import pytest

if __name__ == "__main__":
    # find_spec doesn't import GeoPandas, which would import pyproj before
    # the shim could take its place.
    spec = importlib.util.find_spec("geopandas")
    assert spec is not None and spec.origin is not None, "GeoPandas isn't installed"
    site_packages = Path(spec.origin).parent.parent
    sys.exit(pytest.main([f"--rootdir={site_packages}", *sys.argv[1:]]))
