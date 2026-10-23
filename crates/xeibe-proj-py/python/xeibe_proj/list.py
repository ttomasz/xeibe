"""PROJ's lists, as ``pyproj.list``: the named ellipsoids and prime
meridians (PROJ's ``ellps.cpp`` and ``datums.cpp``). PROJ's operations
aren't listed: there is no PROJ."""

from __future__ import annotations

from xeibe_proj import _native
from xeibe_proj._unsupported import out

__all__ = ["get_ellps_map", "get_prime_meridians_map", "get_proj_operations_map"]


def get_ellps_map() -> dict[str, dict[str, float | str]]:
    """PROJ's named ellipsoids: ``{id: {"a": …, "rf" or "b": …,
    "description": …}}``."""
    return {
        id_: {"a": a, key: value, "description": description}
        for id_, a, key, value, description in _native.ellipsoids()
    }


def get_prime_meridians_map() -> dict[str, str]:
    """PROJ's named prime meridians: ``{id: longitude from Greenwich}``, as
    PROJ writes it (``"2d20'14.025\\"E"``)."""
    return dict(_native.prime_meridians())


def get_proj_operations_map() -> dict[str, str]:
    """Not supported: it lists PROJ's operations."""
    raise out("get_proj_operations_map", "it lists PROJ's operations")
