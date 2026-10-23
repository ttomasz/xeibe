"""Queries of the EPSG tables, as in ``pyproj.database``.

There is no ``proj.db``: the tables are ``xeibe-crs``'s, so the only
authorities are EPSG and OGC (for CRS84, CRS83 and CRS27), and EPSG's
descriptions of areas of use aren't there (``AreaOfUse.name`` is ``None``).
"""

from __future__ import annotations

from typing import NamedTuple

from xeibe_proj import _epsg, _native
from xeibe_proj.aoi import AreaOfInterest, AreaOfUse, BBox
from xeibe_proj.enums import PJType

__all__ = [
    "CRSInfo",
    "Unit",
    "get_authorities",
    "get_codes",
    "get_database_metadata",
    "get_units_map",
    "query_crs_info",
    "query_utm_crs_info",
]

_KIND_TYPES: dict[str, PJType] = {
    "geographic_2d": PJType.GEOGRAPHIC_2D_CRS,
    "geographic_3d": PJType.GEOGRAPHIC_3D_CRS,
    "projected": PJType.PROJECTED_CRS,
    "geocentric": PJType.GEOCENTRIC_CRS,
    "vertical": PJType.VERTICAL_CRS,
    "compound": PJType.COMPOUND_CRS,
    "engineering": PJType.ENGINEERING_CRS,
    "derived": PJType.DERIVED_PROJECTED_CRS,
}

# Types that stand for several.
_GROUPS: dict[PJType, set[PJType]] = {
    PJType.CRS: set(_KIND_TYPES.values()),
    PJType.GEODETIC_CRS: {PJType.GEOGRAPHIC_2D_CRS, PJType.GEOGRAPHIC_3D_CRS, PJType.GEOCENTRIC_CRS},
    PJType.GEOGRAPHIC_CRS: {PJType.GEOGRAPHIC_2D_CRS, PJType.GEOGRAPHIC_3D_CRS},
}


class CRSInfo(NamedTuple):
    """One CRS found by a query."""

    auth_name: str
    code: str
    name: str
    type: PJType
    deprecated: bool
    area_of_use: AreaOfUse | None
    projection_method_name: str | None


class Unit(NamedTuple):
    """A unit of measure, as ``get_units_map`` lists it."""

    auth_name: str
    code: str
    name: str
    category: str
    conv_factor: float
    proj_short_name: str | None
    deprecated: bool


def get_authorities() -> list[str]:
    """The authorities whose codes are known."""
    return ["EPSG", "OGC"]


def _types(pj_types: PJType | str | list[PJType] | list[str] | None) -> set[PJType]:
    if pj_types is None:
        return _GROUPS[PJType.CRS]
    items = pj_types if isinstance(pj_types, list) else [pj_types]
    out: set[PJType] = set()
    for item in items:
        t = PJType.create(item)
        out |= _GROUPS.get(t, {t})
    return out


def get_codes(auth_name: str, pj_type: PJType | str, allow_deprecated: bool = False) -> list[str]:
    """The codes of one authority's CRSs of a type."""
    auth = auth_name.upper()
    types = _types(pj_type)
    codes: list[str] = []
    if auth in ("EPSG", "ANY"):
        codes += [
            str(r.code)
            for r in _epsg.records().values()
            if _KIND_TYPES.get(r.kind) in types and (allow_deprecated or not r.deprecated)
        ]
    if auth in ("OGC", "ANY") and PJType.GEOGRAPHIC_2D_CRS in types:
        codes += list(_epsg.OGC_CRS)
    return codes


def _method(code: int) -> str | None:
    projjson = _epsg.projjson(code)
    if projjson is None:
        return None
    conversion = projjson.get("conversion")
    if not isinstance(conversion, dict):
        return None
    name = conversion.get("method", {}).get("name")  # pyright: ignore[reportUnknownMemberType, reportUnknownVariableType]
    return name if isinstance(name, str) else None


def _matches(area: tuple[float, float, float, float] | None, aoi: AreaOfInterest, contains: bool) -> bool:
    if area is None:
        return False
    west, south, east, north = area
    box = BBox(aoi.west_lon_degree, aoi.south_lat_degree, aoi.east_lon_degree, aoi.north_lat_degree)
    # An area across the antimeridian is two boxes.
    parts = [BBox(west, south, east, north)] if west <= east else [BBox(west, south, 180, north), BBox(-180, south, east, north)]
    if contains:
        return any(p.contains(box) for p in parts)
    return any(p.intersects(box) for p in parts)


def query_crs_info(
    auth_name: str | None = None,
    pj_types: PJType | str | list[PJType] | list[str] | None = None,
    area_of_interest: AreaOfInterest | None = None,
    contains: bool = False,
    allow_deprecated: bool = False,
) -> list[CRSInfo]:
    """The CRSs of these types (all by default) whose area of use
    intersects (or with ``contains``, holds) ``area_of_interest``."""
    if auth_name is not None and auth_name.upper() not in ("EPSG", "ANY"):
        return []
    types = _types(pj_types)
    out: list[CRSInfo] = []
    for r in _epsg.records().values():
        t = _KIND_TYPES.get(r.kind)
        if t not in types or (r.deprecated and not allow_deprecated):
            continue
        if area_of_interest is not None and not _matches(r.area, area_of_interest, contains):
            continue
        out.append(
            CRSInfo(
                auth_name="EPSG",
                code=str(r.code),
                name=r.name,
                type=t,
                deprecated=r.deprecated,
                area_of_use=None if r.area is None else AreaOfUse(*r.area),
                projection_method_name=_method(r.code) if t is PJType.PROJECTED_CRS else None,
            )
        )
    return out


def query_utm_crs_info(
    datum_name: str | None = None,
    area_of_interest: AreaOfInterest | None = None,
    contains: bool = False,
) -> list[CRSInfo]:
    """UTM projected CRSs, by name as pyproj finds them: ``"<datum_name> /
    UTM zone …"`` when ``datum_name`` is given."""
    prefix = None if datum_name is None else f"{datum_name.strip()} / UTM zone"
    return [
        info
        for info in query_crs_info(pj_types=PJType.PROJECTED_CRS, area_of_interest=area_of_interest, contains=contains)
        if "UTM zone" in info.name and (prefix is None or info.name.startswith(prefix))
    ]


def get_database_metadata(key: str) -> str | None:
    """``EPSG.VERSION`` (as ``v13.106``) and ``EPSG.DATE``; ``None`` otherwise."""
    if key == "EPSG.VERSION":
        return f"v{_native.epsg_version()}"
    if key == "EPSG.DATE":
        return _native.epsg_date()
    return None


def _unit_category(kind: str, name: str) -> str:
    """PROJ's category of a unit (``AuthorityFactory::getUnitList``)."""
    if kind == "length":
        return "linear_per_time" if " per " in name else "linear"
    if kind == "angle":
        return "angular_per_time" if " per " in name else "angular"
    if kind == "scale":
        return "scale_per_time" if " per year" in name or " per second" in name else "scale"
    return kind


def get_units_map(auth_name: str | None = None, category: str | None = None, allow_deprecated: bool = False) -> dict[str, Unit]:
    """Units of measure by name: EPSG's and PROJ's own (Esri's aren't
    known), as PROJ's database lists them. ``category`` is one of
    ``linear``, ``angular``, ``scale``, ``time`` and their ``_per_time``
    forms. A name used twice is the later unit's."""
    out: dict[str, Unit] = {}
    for auth, code, name, kind, factor, short_name, deprecated in _native.units():
        if auth_name and auth != auth_name:
            continue
        unit_category = _unit_category(kind, name)
        if category is not None and unit_category != category:
            continue
        if deprecated and not allow_deprecated:
            continue
        out[name] = Unit(auth, code, name, unit_category, factor or 0.0, short_name, deprecated)
    return out
