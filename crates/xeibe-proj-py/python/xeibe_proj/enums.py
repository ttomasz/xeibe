"""Enumerations, with pyproj's names and values.

Every option that takes one of these also takes its value as a string, in
any case (``"inverse"`` for ``TransformDirection.INVERSE``).
"""

from __future__ import annotations

from enum import Enum, IntFlag
from typing import Self

__all__ = [
    "CRSExtentUse",
    "GeodIntermediateFlag",
    "GridAvailabilityUse",
    "IntermediateCRSUse",
    "PJType",
    "ProjVersion",
    "TransformDirection",
    "WktVersion",
]


class BaseEnum(Enum):
    """An enumeration whose members can be given as their values, ignoring case."""

    @classmethod
    def create(cls, item: object) -> Self:
        """The member ``item`` is, or whose value it is."""
        if isinstance(item, cls):
            return item
        for member in cls:
            if member.value == item:
                return member
        if isinstance(item, str):
            upper = item.upper()
            for member in cls:
                if member.value == upper:
                    return member
        values = tuple(member.value for member in cls)
        raise ValueError(f"Invalid value supplied '{item}'. Only {values} are supported.")


class WktVersion(BaseEnum):
    """WKT flavours (writing WKT comes in phase 2)."""

    WKT2_2015 = "WKT2_2015"
    WKT2_2015_SIMPLIFIED = "WKT2_2015_SIMPLIFIED"
    WKT2_2018 = "WKT2_2018"
    WKT2_2018_SIMPLIFIED = "WKT2_2018_SIMPLIFIED"
    WKT2_2019 = "WKT2_2019"
    WKT2_2019_SIMPLIFIED = "WKT2_2019_SIMPLIFIED"
    WKT1_GDAL = "WKT1_GDAL"
    WKT1_ESRI = "WKT1_ESRI"


class ProjVersion(BaseEnum):
    """PROJ string versions (PROJ strings come in phase 3)."""

    PROJ_4 = 4
    PROJ_5 = 5


class TransformDirection(BaseEnum):
    """Which way a transformer converts."""

    FORWARD = "FORWARD"
    INVERSE = "INVERSE"
    IDENT = "IDENT"


class PJType(BaseEnum):
    """Object types, for ``database.get_codes`` and ``query_crs_info``."""

    UNKNOWN = "UNKNOWN"
    ELLIPSOID = "ELLIPSOID"
    PRIME_MERIDIAN = "PRIME_MERIDIAN"
    GEODETIC_REFERENCE_FRAME = "GEODETIC_REFERENCE_FRAME"
    DYNAMIC_GEODETIC_REFERENCE_FRAME = "DYNAMIC_GEODETIC_REFERENCE_FRAME"
    VERTICAL_REFERENCE_FRAME = "VERTICAL_REFERENCE_FRAME"
    DYNAMIC_VERTICAL_REFERENCE_FRAME = "DYNAMIC_VERTICAL_REFERENCE_FRAME"
    DATUM_ENSEMBLE = "DATUM_ENSEMBLE"
    CRS = "CRS"
    GEODETIC_CRS = "GEODETIC_CRS"
    GEOCENTRIC_CRS = "GEOCENTRIC_CRS"
    GEOGRAPHIC_CRS = "GEOGRAPHIC_CRS"
    GEOGRAPHIC_2D_CRS = "GEOGRAPHIC_2D_CRS"
    GEOGRAPHIC_3D_CRS = "GEOGRAPHIC_3D_CRS"
    VERTICAL_CRS = "VERTICAL_CRS"
    PROJECTED_CRS = "PROJECTED_CRS"
    DERIVED_PROJECTED_CRS = "DERIVED_PROJECTED_CRS"
    COMPOUND_CRS = "COMPOUND_CRS"
    TEMPORAL_CRS = "TEMPORAL_CRS"
    ENGINEERING_CRS = "ENGINEERING_CRS"
    BOUND_CRS = "BOUND_CRS"
    OTHER_CRS = "OTHER_CRS"
    CONVERSION = "CONVERSION"
    TRANSFORMATION = "TRANSFORMATION"
    CONCATENATED_OPERATION = "CONCATENATED_OPERATION"
    OTHER_COORDINATE_OPERATION = "OTHER_COORDINATE_OPERATION"


class GeodIntermediateFlag(IntFlag):
    """Flags for ``Geod.inv_intermediate`` and ``fwd_intermediate`` (phase 2)."""

    DEFAULT = 0x0
    NPTS_ROUND = 0x0
    NPTS_CEIL = 0x1
    NPTS_TRUNC = 0x2
    DEL_S_RECALC = 0x00
    DEL_S_NO_RECALC = 0x10
    AZIS_DISCARD = 0x000
    AZIS_KEEP = 0x100


class CRSExtentUse(BaseEnum):
    """How CRS extents limit a ``TransformerGroup``'s candidates."""

    NONE = "NONE"
    BOTH = "BOTH"
    INTERSECTION = "INTERSECTION"
    SMALLEST = "SMALLEST"


class IntermediateCRSUse(BaseEnum):
    """Whether a ``TransformerGroup`` goes through pivot CRSs."""

    ALWAYS = "ALWAYS"
    IF_NO_DIRECT_TRANSFORMATION = "IF_NO_DIRECT_TRANSFORMATION"
    NEVER = "NEVER"


class GridAvailabilityUse(BaseEnum):
    """How grid availability affects a ``TransformerGroup``'s candidates."""

    SORT = "SORT"
    DISCARD_MISSING = "DISCARD_MISSING"
    NONE = "NONE"
    KNOWN_AVAILABLE = "KNOWN_AVAILABLE"
