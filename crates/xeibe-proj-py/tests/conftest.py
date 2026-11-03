from __future__ import annotations

import os
import subprocess
import sys
from collections.abc import Iterator
from pathlib import Path

import pytest

from xeibe_proj import datadir, network

ROOT = Path(__file__).resolve().parents[3]
GRIDS = ROOT / "tests/data/reprojection/grids"

# Pyodide (docs/pyproj-phase3.md, Pyodide): no processes, threads or sockets.
EMSCRIPTEN = sys.platform == "emscripten"


def run_python(code: str) -> None:
    """Run ``code`` in a new interpreter with the network off.

    Pyodide can't start processes, so there it runs in this interpreter
    instead, with no pyproj or numpy module imported, and ``sys.modules``,
    ``sys.meta_path`` and the environment are put back afterwards. Should
    the code import numpy, numpy refuses to load twice, and the test fails
    as it would in a new interpreter."""
    if not EMSCRIPTEN:
        # The whole environment: Python on Windows doesn't start without SYSTEMROOT.
        env = {**os.environ, "PROJ_NETWORK": "OFF"}
        subprocess.run([sys.executable, "-c", code], check=True, env=env)
        return
    modules = dict(sys.modules)
    meta_path = list(sys.meta_path)
    environ = dict(os.environ)
    for name in modules:
        if name.partition(".")[0] in ("pyproj", "numpy"):
            del sys.modules[name]
    os.environ["PROJ_NETWORK"] = "OFF"
    try:
        exec(compile(code, "<run_python>", "exec"), {"__name__": "__main__"})
    finally:
        for name in set(sys.modules) - set(modules):
            del sys.modules[name]
        sys.modules.update(modules)
        sys.meta_path[:] = meta_path
        os.environ.clear()
        os.environ.update(environ)


@pytest.fixture(autouse=True)
def offline(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> Iterator[None]:
    """Network off and an empty cache, unless a test turns it on."""
    # XDG_CACHE_HOME only counts on Linux.
    monkeypatch.setattr(datadir, "_cache_dir", lambda: tmp_path / "cache")
    monkeypatch.delenv("PROJ_DATA", raising=False)
    monkeypatch.delenv("PROJ_LIB", raising=False)
    network.set_network_enabled(False)
    yield
    network.set_network_enabled(None)
