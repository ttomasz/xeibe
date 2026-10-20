"""Exceptions, with pyproj's names and hierarchy. The messages differ."""

from __future__ import annotations

__all__ = ["CRSError", "DataDirError", "GeodError", "ProjError"]


class ProjError(RuntimeError):
    """A transformation can't be set up, or a coordinate can't be converted."""


class CRSError(ProjError):
    """The input doesn't describe a CRS this package can read."""


class GeodError(RuntimeError):
    """A geodesic computation failed."""


class DataDirError(RuntimeError):
    """A data directory doesn't exist."""
