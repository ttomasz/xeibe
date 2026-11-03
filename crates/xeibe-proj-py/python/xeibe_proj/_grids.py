"""Loading grids: from a data directory, the cache, or ``cdn.proj.org``.

Every use is logged to the ``xeibe_proj.grids`` logger, which is public API
(``docs/pyproj-compat.md``, Grids). The package adds no handlers: without
logging configured, the ``INFO`` records are dropped and only ``ERROR``
reaches stderr through Python's last-resort handler.

| Event | Level |
|---|---|
| A transformer that may read grids first converts | INFO |
| Ranges downloaded | INFO |
| Ranges read from the cache or a data directory | DEBUG |
| A whole file downloaded (``TransformerGroup.download_grids``) | INFO |
| A whole file kept, already in the directory | DEBUG |
| A download fails | ERROR, then ``ProjError`` |

Which operation a coordinate went through, and so whether it needed a grid,
needs operation metadata from ``xeibe-reproject``; until then the first
message names the grids the transformer may read.

Under Pyodide (``sys.platform == "emscripten"``) there are no sockets, so
grids are fetched with the browser's synchronous ``XMLHttpRequest``, and the
cache is Pyodide's in-memory file system, which lasts until the page is
reloaded (``docs/pyproj-phase3.md``, Pyodide).
"""

from __future__ import annotations

import importlib
import logging
import os
import shutil
import sys
import tempfile
from collections.abc import Generator
from contextlib import contextmanager
from io import BytesIO
from pathlib import Path
from typing import Any, BinaryIO, Protocol

from xeibe_proj import _native, datadir, network
from xeibe_proj.exceptions import ProjError

__all__ = ["CDN", "LOGGER", "download", "load", "local_files"]

LOGGER = logging.getLogger("xeibe_proj.grids")

CDN = "https://cdn.proj.org/"
TIMEOUT = 60.0

# Pyodide, in a browser or in Node: fetched through XMLHttpRequest.
_EMSCRIPTEN = sys.platform == "emscripten"


class _Transform(Protocol):
    def grid_requests(
        self, bounds: tuple[float, float, float, float] | None = None
    ) -> list[tuple[str, int, int | None]]: ...
    def add_grid_data(self, file: str, offset: int, data: bytes) -> None: ...


def _dirs() -> list[str]:
    """Where grid files are looked for, as PROJ looks for resource files:
    the directories set with ``set_data_dir`` alone, else the user data
    directory and then ``PROJ_DATA``."""
    dirs = datadir._data_dirs()  # pyright: ignore[reportPrivateUsage]
    if datadir._user_dirs is not None:  # pyright: ignore[reportPrivateUsage]
        return dirs
    return [datadir.get_user_data_dir(), *dirs]


def local_files() -> list[str]:
    """The files in the data directories and the user data directory:
    PROJ-data's grids, by name, that the choice of operation may use with
    the network off."""
    files: set[str] = set()
    for d in _dirs():
        try:
            files.update(p.name for p in Path(d).iterdir() if p.is_file())
        except OSError:
            continue
    return sorted(files)


def _local(file: str) -> Path | None:
    for d in _dirs():
        path = Path(d) / file
        if path.is_file():
            return path
    return None


def _read_range(path: Path, offset: int, length: int | None) -> bytes:
    with path.open("rb") as f:
        f.seek(offset)
        return f.read() if length is None else f.read(length)


def _cache_path(file: str, offset: int, length: int | None) -> Path:
    name = f"{offset}-{'end' if length is None else length}"
    return datadir._cache_dir() / "grids" / file / name  # pyright: ignore[reportPrivateUsage]


def _write_cache(path: Path, data: bytes) -> None:
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        fd, tmp = tempfile.mkstemp(dir=path.parent, prefix=".part-")
        with os.fdopen(fd, "wb") as f:
            f.write(data)
        os.replace(tmp, path)
    except OSError as e:
        LOGGER.warning("could not cache grid data at %s: %s", path, e)


def _xhr_get(url: str, headers: dict[str, str]) -> tuple[int, bytes]:
    """GET with the browser's synchronous ``XMLHttpRequest``: the status and
    the body.

    Not ``pyodide.http.pyxhr``: it never asks for an ``ArrayBuffer`` (a
    synchronous request can't, on a page's main thread), and encodes the
    response text as UTF-8, which garbles binary data in every thread. The
    ``x-user-defined`` charset works in
    every thread: it maps byte ``b`` to ``b`` below 0x80 and to
    ``U+F700 + b`` above, so each byte is the low byte of its UTF-16 code
    unit. No ``User-Agent``: browsers set it, and ``cdn.proj.org`` allows
    only ``Range`` in CORS requests."""
    # Pyodide's module of JavaScript's globals, untyped: by name.
    try:
        xhr: Any = importlib.import_module("js").XMLHttpRequest.new()
    except (ImportError, AttributeError) as e:
        raise OSError("no XMLHttpRequest here (Node has none): put the grids in a data directory") from e
    xhr.open("GET", url, False)
    xhr.overrideMimeType("text/plain; charset=x-user-defined")
    for name, value in headers.items():
        xhr.setRequestHeader(name, value)
    try:
        xhr.send()
    except Exception as e:  # a JsException: a network or CORS error
        raise OSError(f"request failed: {e}") from e
    status = int(xhr.status)
    if status == 0:
        raise OSError("request failed (network or CORS error)")
    if status >= 400:
        raise OSError(f"HTTP Error {status}: {xhr.statusText}")
    text = str(xhr.responseText)
    return status, text.encode("utf-16-le")[::2]


@contextmanager
def _open(url: str, headers: dict[str, str]) -> Generator[tuple[int, BinaryIO]]:
    """GET ``url``: the status and the body to read. Raises ``OSError``
    (``urllib.error.URLError`` is one) when the request fails or the status
    is an error."""
    if _EMSCRIPTEN:
        status, data = _xhr_get(url, headers)
        yield status, BytesIO(data)
        return
    # Here, not at the top: urllib.request and ssl take about half of
    # `import xeibe_proj`, for what only fetching a grid needs.
    import ssl
    import urllib.request

    request = urllib.request.Request(url, headers={**headers, "User-Agent": f"xeibe-proj/{_native.version()}"})
    context = ssl.create_default_context(cafile=network.ca_bundle_path())
    with urllib.request.urlopen(request, timeout=TIMEOUT, context=context) as response:
        yield response.status, response


def _fetch(file: str, offset: int, length: int | None) -> bytes:
    url = CDN + file
    end = "" if length is None else str(offset + length - 1)
    try:
        with _open(url, {"Range": f"bytes={offset}-{end}"}) as (status, body):
            data = body.read()
        # A server that ignores the range sends the whole file.
        if status == 200 and offset:
            data = data[offset:] if length is None else data[offset : offset + length]
    except OSError as e:
        LOGGER.error("downloading grid %s failed: %s", url, e, extra={"grid": file, "url": url})
        raise ProjError(f"grid {file}: downloading {url} failed: {e}") from e
    LOGGER.info(
        "downloaded %d bytes of grid %s from %s (offset %d)",
        len(data),
        file,
        url,
        offset,
        extra={"grid": file, "url": url, "offset": offset, "bytes": len(data)},
    )
    return data


def download(file_url: str, short_name: str, directory: str | Path, verbose: bool = False) -> None:
    """Download a whole grid file into ``directory``, as pyproj's
    ``_download_resource_file``: through ``<name>.part``, renamed when
    complete. A file of that name already there is kept; ``verbose``
    prints the URL of each download."""
    target = Path(directory, short_name)
    if target.is_file():
        LOGGER.debug("grid %s is already in %s", short_name, directory, extra={"grid": short_name, "path": str(target)})
        return
    if verbose:
        print(f"Downloading: {file_url}")
    target.parent.mkdir(parents=True, exist_ok=True)
    part = Path(directory, f"{short_name}.part")
    try:
        with _open(file_url, {}) as (_, body), part.open("wb") as f:
            shutil.copyfileobj(body, f)
        size = part.stat().st_size
        part.replace(target)
    except OSError as e:
        LOGGER.error("downloading grid %s failed: %s", file_url, e, extra={"grid": short_name, "url": file_url})
        raise ProjError(f"grid {short_name}: downloading {file_url} failed: {e}") from e
    finally:
        part.unlink(missing_ok=True)
    LOGGER.info(
        "downloaded grid %s from %s to %s (%d bytes)",
        short_name,
        file_url,
        target,
        size,
        extra={"grid": short_name, "url": file_url, "path": str(target), "bytes": size},
    )


class _Missing(ProjError):
    """A grid that isn't local, nor cached, with the network off."""


def _read(file: str, offset: int, length: int | None) -> bytes:
    local = _local(file)
    if local is not None:
        LOGGER.debug("reading grid %s from %s", file, local, extra={"grid": file, "path": str(local)})
        return _read_range(local, offset, length)
    cached = _cache_path(file, offset, length)
    if cached.is_file():
        LOGGER.debug("reading grid %s from the cache (offset %d)", file, offset, extra={"grid": file, "offset": offset})
        return cached.read_bytes()
    if not network.is_network_enabled():
        raise _Missing(f"grid {file} is needed but not local, not cached, and the network is off")
    data = _fetch(file, offset, length)
    _write_cache(cached, data)
    return data


def load(transform: _Transform, bounds: tuple[float, float, float, float] | None, missing_ok: bool = False) -> None:
    """Load what ``transform`` needs for coordinates within ``bounds``:
    ask for requests and fill them until none are left. With
    ``missing_ok``, grids that can't be had are left out: a coordinate
    whose operation needs one then fails when it is converted."""
    seen: set[tuple[str, int, int | None]] = set()
    skipped: set[str] = set()
    while True:
        requests = [r for r in transform.grid_requests(bounds) if r[0] not in skipped]
        if not requests:
            return
        for request in requests:
            if request in seen:
                file, offset, _ = request
                raise ProjError(f"grid {file}: bytes from {offset} are missing")
            seen.add(request)
            file, offset, length = request
            try:
                data = _read(file, offset, length)
            except _Missing:
                if not missing_ok:
                    raise
                skipped.add(file)
                continue
            transform.add_grid_data(file, offset, data)

