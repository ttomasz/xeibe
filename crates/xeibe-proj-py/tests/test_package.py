"""The package's top level and what a bare install imports."""

from __future__ import annotations

import os
import subprocess
import sys

import pytest

import xeibe_proj


def test_no_numpy_on_bare_use() -> None:
    code = (
        "import sys, xeibe_proj\n"
        "from xeibe_proj import CRS, Transformer\n"
        "from array import array\n"
        "CRS('EPSG:2180').axis_info\n"
        "t = Transformer.from_crs(4326, 2180)\n"
        "t.transform([52.0], [21.0]); t.transform(array('d', [52.0]), array('d', [21.0]))\n"
        "assert 'numpy' not in sys.modules, 'numpy was imported'\n"
    )
    # The whole environment: Python on Windows doesn't start without SYSTEMROOT.
    env = {**os.environ, "PROJ_NETWORK": "OFF"}
    subprocess.run([sys.executable, "-c", code], check=True, env=env)


def test_versions() -> None:
    assert xeibe_proj.EPSG_VERSION.startswith("13.")
    assert xeibe_proj.__version__
    with pytest.raises(AttributeError, match="no PROJ"):
        xeibe_proj.proj_version_str  # noqa: B018
    with pytest.raises(AttributeError, match="phase 2"):
        xeibe_proj.Geod  # noqa: B018
    assert not hasattr(xeibe_proj, "PROJ_VERSION")


def test_deprecated_module_functions() -> None:
    with pytest.raises(NotImplementedError):
        xeibe_proj.transform(4326, 3857, 1, 2)
