"""Coordinate systems for building a CRS, as ``pyproj.crs.coordinate_system``:
pyproj 3.8.0's classes and arguments (MIT, see NOTICE.md), and axes made
from the enums' members."""

from __future__ import annotations

from typing import Any, Self

from xeibe_proj._json import JsonDict
from xeibe_proj.crs._objects import CoordinateSystem, PartBuilder
from xeibe_proj.crs.enums import Cartesian2DCSAxis, Ellipsoidal2DCSAxis, Ellipsoidal3DCSAxis, VerticalCSAxis

__all__ = ["Cartesian2DCS", "Ellipsoidal2DCS", "Ellipsoidal3DCS", "VerticalCS"]

# PROJJSON's units, as pyproj names them.
UNIT_METRE = "metre"
UNIT_DEGREE = "degree"
UNIT_FT: JsonDict = {"type": "LinearUnit", "name": "foot", "conversion_factor": 0.3048}
UNIT_US_FT: JsonDict = {"type": "LinearUnit", "name": "US survey foot", "conversion_factor": 0.304800609601219}


def _axis(name: str, abbreviation: str, direction: str, unit: Any) -> JsonDict:
    return {"name": name, "abbreviation": abbreviation, "direction": direction, "unit": unit}


def _unit(member: str) -> Any:
    """The linear unit an axis enum's member names: ``…_US_FT``, ``…_FT``
    or metres."""
    if member.endswith("_US_FT"):
        return UNIT_US_FT
    return UNIT_FT if member.endswith("_FT") else UNIT_METRE


_LONGITUDE = _axis("Longitude", "lon", "east", UNIT_DEGREE)
_LATITUDE = _axis("Latitude", "lat", "north", UNIT_DEGREE)
_HEIGHT = _axis("Ellipsoidal height", "h", "up", UNIT_METRE)


def _cartesian_2d(member: Cartesian2DCSAxis) -> list[JsonDict]:
    name = member.name
    unit = _unit(name)
    # In feet the axes are X and Y, as in pyproj.
    e, n = ("X", "Y") if unit != UNIT_METRE else ("E", "N")
    if name == "WESTING_SOUTHING":
        return [_axis("Easting", "Y", "west", unit), _axis("Northing", "X", "south", unit)]
    if name.startswith(("NORTH_POLE", "SOUTH_POLE")):
        direction = "south" if name.startswith("NORTH_POLE") else "north"
        return [_axis("Easting", e, direction, unit), _axis("Northing", n, direction, unit)]
    axes = [_axis("Easting", e, "east", unit), _axis("Northing", n, "north", unit)]
    return axes[::-1] if name.startswith("NORTHING") else axes


def _vertical(member: VerticalCSAxis) -> JsonDict:
    unit = _unit(member.name)
    if member.name.startswith("DEPTH"):
        return _axis("Depth", "D", "down", unit)
    if member.name.startswith("UP"):
        return _axis("up", "H", "up", unit)
    return _axis("Gravity-related height", "H", "up", unit)


def _cs(subtype: str, axes: list[JsonDict]) -> JsonDict:
    return {"type": "CoordinateSystem", "subtype": subtype, "axis": axes}


class Ellipsoidal2DCS(PartBuilder, CoordinateSystem):
    """An ellipsoidal 2D coordinate system, in degrees."""

    def __new__(cls, axis: Ellipsoidal2DCSAxis | str = Ellipsoidal2DCSAxis.LONGITUDE_LATITUDE) -> Self:
        """
        Parameters
        ----------
        axis: :class:`xeibe_proj.crs.enums.Ellipsoidal2DCSAxis` or str, optional
            The axis order. Default is ``LONGITUDE_LATITUDE``.
        """
        lon_lat = Ellipsoidal2DCSAxis.create(axis) is Ellipsoidal2DCSAxis.LONGITUDE_LATITUDE
        return cls.from_json_dict(_cs("ellipsoidal", [_LONGITUDE, _LATITUDE] if lon_lat else [_LATITUDE, _LONGITUDE]))


class Ellipsoidal3DCS(PartBuilder, CoordinateSystem):
    """An ellipsoidal 3D coordinate system, in degrees and metres."""

    def __new__(cls, axis: Ellipsoidal3DCSAxis | str = Ellipsoidal3DCSAxis.LONGITUDE_LATITUDE_HEIGHT) -> Self:
        """
        Parameters
        ----------
        axis: :class:`xeibe_proj.crs.enums.Ellipsoidal3DCSAxis` or str, optional
            The axis order. Default is ``LONGITUDE_LATITUDE_HEIGHT``.
        """
        lon_lat = Ellipsoidal3DCSAxis.create(axis) is Ellipsoidal3DCSAxis.LONGITUDE_LATITUDE_HEIGHT
        horizontal = [_LONGITUDE, _LATITUDE] if lon_lat else [_LATITUDE, _LONGITUDE]
        return cls.from_json_dict(_cs("ellipsoidal", [*horizontal, _HEIGHT]))


class Cartesian2DCS(PartBuilder, CoordinateSystem):
    """A Cartesian 2D coordinate system, for a projected CRS."""

    def __new__(cls, axis: Cartesian2DCSAxis | str = Cartesian2DCSAxis.EASTING_NORTHING) -> Self:
        """
        Parameters
        ----------
        axis: :class:`xeibe_proj.crs.enums.Cartesian2DCSAxis` or str, optional
            The axes, their order and unit. Default is ``EASTING_NORTHING``.
        """
        return cls.from_json_dict(_cs("Cartesian", _cartesian_2d(Cartesian2DCSAxis.create(axis))))


class VerticalCS(PartBuilder, CoordinateSystem):
    """A vertical coordinate system."""

    def __new__(cls, axis: VerticalCSAxis | str = VerticalCSAxis.GRAVITY_HEIGHT) -> Self:
        """
        Parameters
        ----------
        axis: :class:`xeibe_proj.crs.enums.VerticalCSAxis` or str, optional
            The axis's direction and unit. Default is ``GRAVITY_HEIGHT``.
        """
        return cls.from_json_dict(_cs("vertical", [_vertical(VerticalCSAxis.create(axis))]))
