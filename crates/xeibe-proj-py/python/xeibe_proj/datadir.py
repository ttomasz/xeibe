"""Directories with local copies of PROJ-data's grid files.

There is no ``proj.db``: a directory counts when it exists. Grids found in
one are read from disk instead of fetched. Until ``xeibe-reproject`` can
choose operations by which grids are local, a local grid is only used with
the network on (see ``docs/pyproj-compat.md``, Grids).
"""

from __future__ import annotations

import os
import sys
from pathlib import Path

from xeibe_proj.exceptions import DataDirError

__all__ = ["append_data_dir", "get_data_dir", "get_user_data_dir", "set_data_dir"]

_user_dirs: list[str] | None = None


def set_data_dir(proj_data_dir: str | Path) -> None:
    """Use these directories (separated by ``os.pathsep``) instead of ``PROJ_DATA``."""
    global _user_dirs
    dirs = [d for d in str(proj_data_dir).split(os.pathsep) if d]
    missing = [d for d in dirs if not Path(d).is_dir()]
    if missing:
        raise DataDirError(f"not a directory: {', '.join(missing)}")
    _user_dirs = dirs


def append_data_dir(proj_data_dir: str | Path) -> None:
    """Add a directory after the current ones."""
    set_data_dir(os.pathsep.join([*_data_dirs(), str(proj_data_dir)]))


def get_data_dir() -> str:
    """The data directories, separated by ``os.pathsep``: those set with
    ``set_data_dir``, else ``PROJ_DATA`` (or ``PROJ_LIB``). Empty when there
    are none, where pyproj would raise ``DataDirError``: no data directory is
    needed here."""
    return os.pathsep.join(_data_dirs())


def get_user_data_dir(create: bool = False) -> str:
    """Where fetched grid ranges are cached: ``$XDG_CACHE_HOME/xeibe-proj``,
    ``~/Library/Caches/xeibe-proj`` or ``%LOCALAPPDATA%\\xeibe-proj``."""
    path = _cache_dir()
    if create:
        path.mkdir(parents=True, exist_ok=True)
    return str(path)


def _data_dirs() -> list[str]:
    if _user_dirs is not None:
        return list(_user_dirs)
    env = os.environ.get("PROJ_DATA") or os.environ.get("PROJ_LIB") or ""
    return [d for d in env.split(os.pathsep) if d and Path(d).is_dir()]


def _cache_dir() -> Path:
    if sys.platform == "win32":
        base = os.environ.get("LOCALAPPDATA") or str(Path.home() / "AppData" / "Local")
    elif sys.platform == "darwin":
        base = str(Path.home() / "Library" / "Caches")
    else:
        base = os.environ.get("XDG_CACHE_HOME") or str(Path.home() / ".cache")
    return Path(base) / "xeibe-proj"
