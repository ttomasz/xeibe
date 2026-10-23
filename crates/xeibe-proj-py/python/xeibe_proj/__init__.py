"""A pyproj-compatible API over xeibe's EPSG tables and coordinate conversion.

``from xeibe_proj import CRS, Transformer`` replaces the same import from
pyproj for code that converts coordinates between EPSG CRSs. What differs,
and what is planned, is in ``docs/pyproj-compat.md`` in the xeibe
repository. Imports only the standard library and the native module.
"""

from __future__ import annotations

import platform
import sys
from importlib.metadata import PackageNotFoundError, version

from xeibe_proj import network
from xeibe_proj._native import epsg_version as _epsg_version
from xeibe_proj.crs import CRS
from xeibe_proj.database import get_authorities, get_codes, get_units_map
from xeibe_proj.exceptions import DataDirError, ProjError
from xeibe_proj.geod import Geod, geodesic_version_str, pj_ellps
from xeibe_proj.list import get_ellps_map, get_prime_meridians_map, get_proj_operations_map
from xeibe_proj.proj import Proj
from xeibe_proj.transformer import Transformer, itransform, transform

__all__ = [
    "CRS",
    "EPSG_VERSION",
    "DataDirError",
    "Geod",
    "Proj",
    "ProjError",
    "Transformer",
    "__version__",
    "geodesic_version_str",
    "get_authorities",
    "get_codes",
    "get_ellps_map",
    "get_prime_meridians_map",
    "get_proj_operations_map",
    "get_units_map",
    "itransform",
    "network",
    "pj_ellps",
    "set_use_global_context",
    "show_versions",
    "transform",
]

try:
    __version__ = version("xeibe-proj")
except PackageNotFoundError:  # pragma: no cover - running from a source tree
    __version__ = "unknown"

#: The EPSG Dataset release of the tables, e.g. ``"13.106"``.
EPSG_VERSION: str = _epsg_version()

# Parts of pyproj's top level that aren't here: name -> why.
_MISSING = {
    "proj_version_str": "there is no PROJ to report a version of",
    "PROJ_VERSION": "there is no PROJ to report a version of",
    "PROJ_VERSION_STR": "there is no PROJ to report a version of",
    "PROJ_COMPILED_VERSION": "there is no PROJ to report a version of",
    "PROJ_COMPILED_VERSION_STR": "there is no PROJ to report a version of",
    "pj_list": "not supported: it lists PROJ's projections",
}


def __getattr__(name: str) -> object:
    reason = _MISSING.get(name)
    if reason is not None:
        raise AttributeError(f"xeibe_proj.{name}: {reason} (see docs/pyproj-compat.md)")
    raise AttributeError(f"module 'xeibe_proj' has no attribute {name!r}")


def set_use_global_context(active: bool | None = None) -> None:
    """A no-op: there is no PROJ context."""


def show_versions() -> None:
    """Print the versions of this package, its EPSG data and Python."""
    print("xeibe-proj:")
    print(f"  version: {__version__}")
    print(f"     EPSG: {EPSG_VERSION}")
    print(f"  network: {network.is_network_enabled()}")
    print("\nSystem:")
    print(f"   python: {sys.version}")
    print(f"  machine: {platform.platform()}")
