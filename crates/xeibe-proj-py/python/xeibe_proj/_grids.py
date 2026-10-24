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
"""

from __future__ import annotations

import logging
import os
import shutil
import ssl
import tempfile
import urllib.error
import urllib.request
from pathlib import Path
from typing import Protocol

from xeibe_proj import datadir, network
from xeibe_proj.exceptions import ProjError

__all__ = ["CDN", "LOGGER", "download", "load", "local_files"]

LOGGER = logging.getLogger("xeibe_proj.grids")

CDN = "https://cdn.proj.org/"
TIMEOUT = 60.0


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
    request = urllib.request.Request(file_url, headers={"User-Agent": f"xeibe-proj/{_version()}"})
    context = ssl.create_default_context(cafile=network.ca_bundle_path())
    try:
        with urllib.request.urlopen(request, timeout=TIMEOUT, context=context) as response, part.open("wb") as f:
            shutil.copyfileobj(response, f)
        size = part.stat().st_size
        part.replace(target)
    except (urllib.error.URLError, OSError) as e:
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


def _version() -> str:
    from importlib.metadata import PackageNotFoundError, version

    try:
        return version("xeibe-proj")
    except PackageNotFoundError:
        return "unknown"
