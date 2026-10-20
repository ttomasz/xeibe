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
| A download fails | ERROR, then ``ProjError`` |

Which operation a coordinate went through, and so whether it needed a grid,
needs operation metadata from ``xeibe-reproject``; until then the first
message names the grids the transformer may read.
"""

from __future__ import annotations

import logging
import os
import ssl
import tempfile
import urllib.error
import urllib.request
from pathlib import Path
from typing import Protocol

from xeibe_proj import datadir, network
from xeibe_proj.exceptions import ProjError

__all__ = ["LOGGER", "load"]

LOGGER = logging.getLogger("xeibe_proj.grids")

CDN = "https://cdn.proj.org/"
TIMEOUT = 60.0


class _Transform(Protocol):
    def grid_requests(
        self, bounds: tuple[float, float, float, float] | None = None
    ) -> list[tuple[str, int, int | None]]: ...
    def add_grid_data(self, file: str, offset: int, data: bytes) -> None: ...


def _local(file: str) -> Path | None:
    for d in datadir._data_dirs():  # pyright: ignore[reportPrivateUsage]
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


def _fetch(file: str, offset: int, length: int | None) -> bytes:
    url = CDN + file
    end = "" if length is None else str(offset + length - 1)
    request = urllib.request.Request(
        url,
        headers={"Range": f"bytes={offset}-{end}", "User-Agent": f"xeibe-proj/{_version()}"},
    )
    cafile = network.ca_bundle_path()
    context = ssl.create_default_context(cafile=cafile)
    try:
        with urllib.request.urlopen(request, timeout=TIMEOUT, context=context) as response:
            data: bytes = response.read()
            # A server that ignores the range sends the whole file.
            if response.status == 200 and offset:
                data = data[offset:] if length is None else data[offset : offset + length]
    except (urllib.error.URLError, OSError) as e:
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
        raise ProjError(f"grid {file} is needed but not local, not cached, and the network is off")
    data = _fetch(file, offset, length)
    _write_cache(cached, data)
    return data


def load(transform: _Transform, bounds: tuple[float, float, float, float] | None) -> None:
    """Load what ``transform`` needs for coordinates within ``bounds``:
    ask for requests and fill them until none are left."""
    seen: set[tuple[str, int, int | None]] = set()
    while True:
        requests = transform.grid_requests(bounds)
        if not requests:
            return
        for request in requests:
            if request in seen:
                file, offset, _ = request
                raise ProjError(f"grid {file}: bytes from {offset} are missing")
            seen.add(request)
            file, offset, length = request
            transform.add_grid_data(file, offset, _read(file, offset, length))


def _version() -> str:
    from importlib.metadata import PackageNotFoundError, version

    try:
        return version("xeibe-proj")
    except PackageNotFoundError:
        return "unknown"
