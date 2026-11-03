"""The enums of ``pyproj.crs.enums``, for the sub-objects' constructors
and the builders. Names and values are pyproj 3.8.0's (MIT, see
NOTICE.md)."""

from __future__ import annotations

from xeibe_proj.enums import BaseEnum

__all__ = [
    "Cartesian2DCSAxis",
    "CoordinateOperationType",
    "DatumType",
    "Ellipsoidal2DCSAxis",
    "Ellipsoidal3DCSAxis",
    "VerticalCSAxis",
]


class DatumType(BaseEnum):
    """Datum types for :meth:`xeibe_proj.crs.Datum.from_name`."""

    GEODETIC_REFERENCE_FRAME = "GEODETIC_REFERENCE_FRAME"
    DYNAMIC_GEODETIC_REFERENCE_FRAME = "DYNAMIC_GEODETIC_REFERENCE_FRAME"
    VERTICAL_REFERENCE_FRAME = "VERTICAL_REFERENCE_FRAME"
    DYNAMIC_VERTICAL_REFERENCE_FRAME = "DYNAMIC_VERTICAL_REFERENCE_FRAME"
    DATUM_ENSEMBLE = "DATUM_ENSEMBLE"


class CoordinateOperationType(BaseEnum):
    """Operation types for :meth:`xeibe_proj.crs.CoordinateOperation.from_name`."""

    CONVERSION = "CONVERSION"
    TRANSFORMATION = "TRANSFORMATION"
    CONCATENATED_OPERATION = "CONCATENATED_OPERATION"
    OTHER_COORDINATE_OPERATION = "OTHER_COORDINATE_OPERATION"


class Cartesian2DCSAxis(BaseEnum):
    """The axes of :class:`xeibe_proj.crs.coordinate_system.Cartesian2DCS`."""

    EASTING_NORTHING = "EASTING_NORTHING"
    NORTHING_EASTING = "NORTHING_EASTING"
    EASTING_NORTHING_FT = "EASTING_NORTHING_FT"
    NORTHING_EASTING_FT = "NORTHING_EASTING_FT"
    EASTING_NORTHING_US_FT = "EASTING_NORTHING_US_FT"
    NORTHING_EASTING_US_FT = "NORTHING_EASTING_US_FT"
    NORTH_POLE_EASTING_SOUTH_NORTHING_SOUTH = "NORTH_POLE_EASTING_SOUTH_NORTHING_SOUTH"
    SOUTH_POLE_EASTING_NORTH_NORTHING_NORTH = "SOUTH_POLE_EASTING_NORTH_NORTHING_NORTH"
    WESTING_SOUTHING = "WESTING_SOUTHING"


class Ellipsoidal2DCSAxis(BaseEnum):
    """The axes of :class:`xeibe_proj.crs.coordinate_system.Ellipsoidal2DCS`."""

    LONGITUDE_LATITUDE = "LONGITUDE_LATITUDE"
    LATITUDE_LONGITUDE = "LATITUDE_LONGITUDE"


class Ellipsoidal3DCSAxis(BaseEnum):
    """The axes of :class:`xeibe_proj.crs.coordinate_system.Ellipsoidal3DCS`."""

    LONGITUDE_LATITUDE_HEIGHT = "LONGITUDE_LATITUDE_HEIGHT"
    LATITUDE_LONGITUDE_HEIGHT = "LATITUDE_LONGITUDE_HEIGHT"


class VerticalCSAxis(BaseEnum):
    """The axis of :class:`xeibe_proj.crs.coordinate_system.VerticalCS`."""

    GRAVITY_HEIGHT = "GRAVITY_HEIGHT"
    GRAVITY_HEIGHT_FT = "GRAVITY_HEIGHT_FT"
    GRAVITY_HEIGHT_US_FT = "GRAVITY_HEIGHT_US_FT"
    DEPTH = "DEPTH"
    DEPTH_FT = "DEPTH_FT"
    DEPTH_US_FT = "DEPTH_US_FT"
    UP = "UP"
    UP_FT = "UP_FT"
    UP_US_FT = "UP_US_FT"
