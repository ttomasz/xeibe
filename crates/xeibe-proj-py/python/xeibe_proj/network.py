"""Network access for grids, which is on by default (unlike pyproj).

With the network on, transformations that need a grid are chosen when they
are the best for a coordinate, and their grids are fetched from
``cdn.proj.org`` when first needed. Every use of a grid is logged to the
``xeibe_proj.grids`` logger (see ``docs/pyproj-compat.md``, Grids). With it
off, only transformations without grids are used, as in pyproj's default
install.
"""

from __future__ import annotations

import importlib
import os
from pathlib import Path

__all__ = ["is_network_enabled", "set_ca_bundle_path", "set_network_enabled"]

_enabled: bool | None = None
_ca_bundle: str | None = None

_OFF = {"0", "off", "no", "false", "n", "f"}


def set_network_enabled(active: bool | None = None) -> None:
    """Turn network access on or off. ``None`` goes back to ``PROJ_NETWORK``."""
    global _enabled
    _enabled = active


def is_network_enabled() -> bool:
    """Whether grids may be fetched: set by ``set_network_enabled``, else
    ``PROJ_NETWORK`` (``OFF`` turns it off), else on."""
    if _enabled is not None:
        return _enabled
    return os.environ.get("PROJ_NETWORK", "ON").strip().lower() not in _OFF


def set_ca_bundle_path(ca_bundle_path: Path | str | bool | None = None) -> None:
    """The certificates that grid downloads trust.

    A path uses that file. ``True`` uses ``certifi``'s bundle if certifi is
    installed. ``None``, ``False`` or ``""`` go back to the default: the
    first of ``PROJ_CURL_CA_BUNDLE``, ``CURL_CA_BUNDLE`` and
    ``SSL_CERT_FILE`` that is set, else the system's certificates.
    """
    global _ca_bundle
    if ca_bundle_path is True:
        # Imported by name: certifi is optional, and not a dependency.
        try:
            certifi = importlib.import_module("certifi")
        except ImportError:
            _ca_bundle = None
        else:
            _ca_bundle = str(certifi.where())
    elif ca_bundle_path is None or ca_bundle_path is False or ca_bundle_path == "":
        _ca_bundle = None
    else:
        _ca_bundle = str(ca_bundle_path)


def ca_bundle_path() -> str | None:
    """The CA bundle grid downloads use, or `None` for the system's."""
    if _ca_bundle is not None:
        return _ca_bundle
    for name in ("PROJ_CURL_CA_BUNDLE", "CURL_CA_BUNDLE", "SSL_CERT_FILE"):
        value = os.environ.get(name)
        if value:
            return value
    return None
