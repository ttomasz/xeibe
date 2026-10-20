"""Areas of interest and of use, as in pyproj."""

from __future__ import annotations

import math
from dataclasses import dataclass
from typing import NamedTuple

__all__ = ["AreaOfInterest", "AreaOfUse", "BBox"]


def _is_null(value: object) -> bool:
    return value is None or (isinstance(value, float) and math.isnan(value))


@dataclass(frozen=True)
class AreaOfInterest:
    """The area to choose a transformation for, in degrees."""

    west_lon_degree: float
    south_lat_degree: float
    east_lon_degree: float
    north_lat_degree: float

    def __post_init__(self) -> None:
        values = (
            self.west_lon_degree,
            self.south_lat_degree,
            self.east_lon_degree,
            self.north_lat_degree,
        )
        if any(_is_null(v) for v in values):
            raise ValueError("NaN or None values are not allowed.")


class AreaOfUse(NamedTuple):
    """The area where a CRS or an operation is valid, in degrees."""

    west: float
    south: float
    east: float
    north: float
    name: str | None = None

    @property
    def bounds(self) -> tuple[float, float, float, float]:
        """``(west, south, east, north)``."""
        return self.west, self.south, self.east, self.north

    def __str__(self) -> str:
        return f"- name: {self.name}\n- bounds: {self.bounds}"


@dataclass
class BBox:
    """A box that can be tested against other boxes."""

    west: float
    south: float
    east: float
    north: float

    def __post_init__(self) -> None:
        if any(_is_null(v) for v in (self.west, self.south, self.east, self.north)):
            raise ValueError("NaN or None values are not allowed.")

    def intersects(self, other: BBox | AreaOfUse) -> bool:
        """Whether the two boxes overlap."""
        return (
            self.west < other.east
            and other.west < self.east
            and self.south < other.north
            and other.south < self.north
        )

    def contains(self, other: BBox | AreaOfUse) -> bool:
        """Whether ``other`` lies within this box."""
        return (
            other.west >= self.west
            and other.east <= self.east
            and other.south >= self.south
            and other.north <= self.north
        )
