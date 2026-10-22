from __future__ import annotations

from collections.abc import Iterator
from pathlib import Path

import pytest

from xeibe_proj import datadir, network

ROOT = Path(__file__).resolve().parents[3]
GRIDS = ROOT / "tests/data/reprojection/grids"


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
