"""Grids: fetched by range from a local stand-in for cdn.proj.org, cached,
read from data directories, and every use logged."""

from __future__ import annotations

import functools
import http.server
import json
import logging
import os
import threading
from collections.abc import Iterator
from pathlib import Path

import pytest

from xeibe_proj import Transformer, _grids, _native, datadir, network
from xeibe_proj.exceptions import ProjError

from .conftest import GRIDS, ROOT

# St. George Island to NAD83 (2) (EPSG:8546, NADCON5), the operation chosen
# with every grid available: PROJ's results.
VECTORS = ROOT / "tests/data/reprojection/operations-grids.jsonl"


def _vectors() -> tuple[list[float], list[float], list[list[float] | None]]:
    for line in VECTORS.read_text().splitlines()[1:]:
        record = json.loads(line)
        if record["op"] == 8546:
            return (
                [p[0] for p in record["in"]],
                [p[1] for p in record["in"]],
                record["out"],
            )
    raise AssertionError("no vectors for EPSG:8546")


class _Handler(http.server.SimpleHTTPRequestHandler):
    """Serves the committed grids, by range or whole, and counts the
    requests."""

    requests: list[str] = []

    def do_GET(self) -> None:
        self.requests.append(self.path)
        path = Path(self.translate_path(self.path))
        if not path.is_file():
            self.send_error(404)
            return
        data = path.read_bytes()
        if self.headers["Range"] is None:
            self.send_response(200)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return
        start, _, end = self.headers["Range"].removeprefix("bytes=").partition("-")
        body = data[int(start) : int(end) + 1 if end else len(data)]
        self.send_response(206)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:
        pass


@pytest.fixture
def cdn(monkeypatch: pytest.MonkeyPatch) -> Iterator[list[str]]:
    handler = functools.partial(_Handler, directory=str(GRIDS))
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    monkeypatch.setattr(_grids, "CDN", f"http://127.0.0.1:{server.server_address[1]}/")
    _Handler.requests = []
    _native.clear_grids()
    network.set_network_enabled(True)
    yield _Handler.requests
    server.shutdown()
    _native.clear_grids()


def _check(t: Transformer) -> None:
    xs, ys, want = _vectors()
    pairs = [(x, y, w) for x, y, w in zip(xs, ys, want, strict=True) if w is not None]
    ox, oy = t.transform([p[0] for p in pairs], [p[1] for p in pairs])
    for (_, _, w), x, y in zip(pairs, ox, oy, strict=True):
        assert (x, y) == pytest.approx(w, abs=1e-9)


def test_fetched_cached_and_logged(cdn: list[str], caplog: pytest.LogCaptureFixture) -> None:
    caplog.set_level(logging.DEBUG, logger="xeibe_proj.grids")
    t = Transformer.from_crs(4138, 4269)
    assert t.is_network_enabled
    _check(t)
    assert cdn, "nothing was fetched"
    infos = [r for r in caplog.records if r.levelno == logging.INFO]
    assert any("may use grids" in r.getMessage() for r in infos)
    downloads = [r for r in infos if r.getMessage().startswith("downloaded")]
    assert downloads and all(getattr(r, "grid", "").endswith(".tif") for r in downloads)

    # A second process would read the cache, not the network.
    fetched = len(cdn)
    _native.clear_grids()
    caplog.clear()
    _check(Transformer.from_crs(4138, 4269))
    assert len(cdn) == fetched
    assert any("from the cache" in r.getMessage() for r in caplog.records if r.levelno == logging.DEBUG)


def test_logged_once_per_transformer(cdn: list[str], caplog: pytest.LogCaptureFixture) -> None:
    caplog.set_level(logging.INFO, logger="xeibe_proj.grids")
    t = Transformer.from_crs(4138, 4269)
    _check(t)
    _check(t)
    assert sum("may use grids" in r.getMessage() for r in caplog.records) == 1


def test_local_grids(cdn: list[str], caplog: pytest.LogCaptureFixture, monkeypatch: pytest.MonkeyPatch) -> None:
    caplog.set_level(logging.DEBUG, logger="xeibe_proj.grids")
    monkeypatch.setattr(datadir, "_user_dirs", None)
    datadir.set_data_dir(GRIDS)
    _check(Transformer.from_crs(4138, 4269))
    assert not cdn
    assert any("reading grid" in r.getMessage() and f"from {GRIDS}" in r.getMessage() for r in caplog.records)


def test_download_failure(cdn: list[str], monkeypatch: pytest.MonkeyPatch, caplog: pytest.LogCaptureFixture) -> None:
    monkeypatch.setattr(_grids, "CDN", "http://127.0.0.1:9/")
    with pytest.raises(ProjError, match="downloading"):
        _check(Transformer.from_crs(4138, 4269))
    assert any(r.levelno == logging.ERROR for r in caplog.records)


def test_network_off_uses_no_grids(caplog: pytest.LogCaptureFixture) -> None:
    # Every operation EPSG has between these needs a grid, and there is no
    # ballpark fallback.
    caplog.set_level(logging.DEBUG, logger="xeibe_proj.grids")
    with pytest.raises(ProjError, match="no usable transformation"):
        Transformer.from_crs(4138, 4269)
    assert not caplog.records


def test_network_setting(monkeypatch: pytest.MonkeyPatch) -> None:
    network.set_network_enabled(None)
    monkeypatch.delenv("PROJ_NETWORK", raising=False)
    assert network.is_network_enabled()
    monkeypatch.setenv("PROJ_NETWORK", "OFF")
    assert not network.is_network_enabled()
    network.set_network_enabled(True)
    assert network.is_network_enabled()


def test_data_dirs(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    monkeypatch.setattr(datadir, "_user_dirs", None)
    assert datadir.get_data_dir() == ""
    monkeypatch.setenv("PROJ_DATA", str(tmp_path))
    assert datadir.get_data_dir() == str(tmp_path)
    other = tmp_path / "other"
    other.mkdir()
    datadir.append_data_dir(other)
    assert datadir.get_data_dir().split(os.pathsep) == [str(tmp_path), str(other)]
    with pytest.raises(Exception, match="not a directory"):
        datadir.set_data_dir(tmp_path / "missing")


def test_local_grids_with_the_network_off(monkeypatch: pytest.MonkeyPatch, caplog: pytest.LogCaptureFixture) -> None:
    """With the network off, an operation whose grid is in a data directory
    is chosen, as PROJ does with PROJ_DATA."""
    caplog.set_level(logging.DEBUG, logger="xeibe_proj.grids")
    _native.clear_grids()
    monkeypatch.setattr(datadir, "_user_dirs", None)
    datadir.set_data_dir(GRIDS)
    t = Transformer.from_crs(4138, 4269)
    assert not t.is_network_enabled
    _check(t)
    assert any("reading grid" in r.getMessage() for r in caplog.records)
    from xeibe_proj.transformer import TransformerGroup

    group = TransformerGroup(4138, 4269)
    assert group.best_available and not group.unavailable_operations
    _native.clear_grids()



ST_GEORGE = ["us_noaa_nadcon5_sg1952_nad83_1986_stgeorge.tif", "us_noaa_stgeorge.tif"]


def test_download_grids(
    cdn: list[str], monkeypatch: pytest.MonkeyPatch, tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    """The missing grids of a group's unavailable operations come whole,
    once each, into the user data directory, where they are found next
    time."""
    from xeibe_proj import transformer
    from xeibe_proj.transformer import TransformerGroup

    network.set_network_enabled(False)
    group = TransformerGroup(4138, 4269)
    assert not group.best_available
    assert [g.short_name for op in group.unavailable_operations for g in op.grids] == ST_GEORGE
    calls: list[bool] = []
    user_dir = transformer.get_user_data_dir
    monkeypatch.setattr(transformer, "get_user_data_dir", lambda create=False: calls.append(create) or user_dir(create))
    group.download_grids(verbose=True)
    assert calls == [True]
    user = Path(datadir.get_user_data_dir())
    assert sorted(p.name for p in user.iterdir() if p.is_file()) == ST_GEORGE
    for name in ST_GEORGE:
        assert (user / name).read_bytes() == (GRIDS / name).read_bytes()
    assert capsys.readouterr().out == "".join(f"Downloading: {_grids.CDN}{n}\n" for n in ST_GEORGE)
    assert cdn == [f"/{n}" for n in ST_GEORGE]
    # Found in the user data directory: available now, nothing to fetch.
    _native.clear_grids()
    again = TransformerGroup(4138, 4269)
    assert again.best_available and not again.unavailable_operations
    _check(again.transformers[0])
    again.download_grids()
    assert cdn == [f"/{n}" for n in ST_GEORGE]


def test_download_grids_into_a_directory(cdn: list[str], tmp_path: Path, caplog: pytest.LogCaptureFixture) -> None:
    from xeibe_proj.transformer import TransformerGroup

    caplog.set_level(logging.DEBUG, logger="xeibe_proj.grids")
    network.set_network_enabled(False)
    group = TransformerGroup(4138, 4269)
    target = tmp_path / "grids"
    # A file already there is kept.
    target.mkdir()
    (target / ST_GEORGE[1]).write_bytes(b"mine")
    group.download_grids(directory=target, open_license=False)
    assert cdn == [f"/{ST_GEORGE[0]}"]
    assert (target / ST_GEORGE[1]).read_bytes() == b"mine"
    assert sorted(p.name for p in target.iterdir()) == ST_GEORGE
    assert any("downloaded grid" in r.getMessage() and r.levelno == logging.INFO for r in caplog.records)
    assert any("already in" in r.getMessage() for r in caplog.records)


def test_download_grids_failure(cdn: list[str], tmp_path: Path, caplog: pytest.LogCaptureFixture) -> None:
    with pytest.raises(ProjError, match="downloading"):
        _grids.download(f"{_grids.CDN}missing.tif", "missing.tif", tmp_path)
    assert list(tmp_path.iterdir()) == []
    assert any(r.levelno == logging.ERROR for r in caplog.records)
