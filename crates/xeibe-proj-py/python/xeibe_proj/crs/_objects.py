"""The parts of a CRS (axes, datum, ellipsoid, …), read from PROJJSON with
pyproj's attribute names. As in pyproj, ``to_wkt()`` writes one on its own
and its ``repr`` is that WKT, pretty."""

from __future__ import annotations

import json
import math
import re
from collections.abc import Callable
from dataclasses import dataclass
from functools import cached_property
from typing import Any, ClassVar, Self, cast

from xeibe_proj import _native, _wkt
from xeibe_proj._json import JsonDict, as_dict, as_list, dumps, get_dict, get_dicts, ident
from xeibe_proj.aoi import AreaOfUse
from xeibe_proj.exceptions import CRSError

__all__ = [
    "AxisInfo",
    "Grid",
    "CoordinateOperation",
    "CoordinateSystem",
    "Datum",
    "Ellipsoid",
    "Param",
    "PrimeMeridian",
]



# PROJJSON's three unit names, with the EPSG code and factor pyproj reports
# for them on an axis.
_AXIS_UNITS: dict[str, tuple[str, float]] = {
    "metre": ("9001", 1.0),
    "degree": ("9122", math.pi / 180),
    "unity": ("9201", 1.0),
}
# On a parameter pyproj reports EPSG's plain degree.
_PARAM_UNITS: dict[str, tuple[str, float]] = {**_AXIS_UNITS, "degree": ("9102", math.pi / 180)}


def _unit(unit: object, table: dict[str, tuple[str, float]]) -> tuple[str, float, str, str]:
    """``(name, factor, authority, code)`` of a PROJJSON unit."""
    if isinstance(unit, str):
        code, factor = table.get(unit, ("", 1.0))
        return unit, factor, "EPSG" if code else "", code
    u = as_dict(unit)
    if u is not None:
        auth, code = ident(u) or ("", "")
        return str(u.get("name", "")), float(u.get("conversion_factor", 1.0)), auth, code
    return "unknown", 1.0, "", ""


def _value(value: object, factor: float = 1.0) -> float:
    """A PROJJSON number or ``{value, unit}``, times ``factor``."""
    v = as_dict(value)
    if v is not None:
        _, f, _, _ = _unit(v.get("unit", "metre"), _AXIS_UNITS)
        return float(v["value"]) * f * factor
    return float(value) * factor  # type: ignore[arg-type]


# What equivalence leaves out: metadata, not definition.
_METADATA = frozenset({"$schema", "id", "ids", "usages", "scope", "area", "bbox", "remarks"})
# `urn:ogc:def:<type>:<authority>:<version>:<code>`, lowercase as PROJ reads it.
_URN = re.compile(r"^urn:ogc:def:([A-Za-z]+):([^:]+):[^:]*:(.+)$")
_WKT = re.compile(r"^\s*[A-Za-z_]+\s*[\[(]")


def _definition(value: object) -> object:
    """PROJJSON without its metadata, for equivalence."""
    d = as_dict(value)
    if d is not None:
        return {k: _definition(v) for k, v in d.items() if k not in _METADATA}
    items = as_list(value)
    if items is not None:
        return [_definition(v) for v in items]
    return value


class _JsonObject:
    """An object kept as its PROJJSON."""

    _type = ""
    # The PROJJSON types this class holds, and its name in error messages.
    _types: ClassVar[frozenset[str]] = frozenset()
    _what = "object"
    # The object types of `urn:ogc:def:` this class reads, by the check
    # each makes on the PROJJSON found.
    _urns: ClassVar[dict[str, Callable[[JsonDict], bool]]] = {}

    def __init__(self, projjson: JsonDict) -> None:
        self._json = projjson

    @classmethod
    def from_json_dict(cls, projjson_dict: JsonDict) -> Self:
        """From a PROJJSON dict of this kind of object."""
        d = as_dict(projjson_dict)
        if d is None or str(d.get("type", "")) not in cls._types:
            raise CRSError(f"Invalid {cls._what} string: {json.dumps(projjson_dict, default=str)}")
        return cls(json.loads(json.dumps(d)))

    @classmethod
    def from_json(cls, projjson_str: str) -> Self:
        """From PROJJSON text."""
        try:
            d = json.loads(projjson_str)
        except ValueError as e:
            raise CRSError(f"Invalid JSON: {e}") from None
        return cls.from_json_dict(d)

    @classmethod
    def _by_code(cls, auth_name: str, code: int | str, urn_type: str | None = None) -> JsonDict | None:
        """An object of this class by authority and code; ``urn_type``
        is the type a URN named it by."""
        return None

    @classmethod
    def _from_string(cls, text: str) -> Self:
        """From a URN, WKT or PROJJSON, as PROJ's ``proj_create`` reads one."""
        invalid = CRSError(f"Invalid {cls._what} string: {text}")
        if text.lstrip().startswith("{"):
            try:
                return cls.from_json(text)
            except CRSError:
                raise invalid from None
        m = _URN.match(text)
        if m is not None:
            urn_type, auth, code = m.groups()
            check = cls._urns.get(urn_type)
            doc = None if check is None else cls._by_code(auth, code, urn_type)
            if doc is None or check is None or not check(doc):
                raise invalid
            return cls(doc)
        if _WKT.match(text):
            try:
                doc = json.loads(_native.wkt_object_to_projjson(text))
            except CRSError:
                raise invalid from None
            if str(doc.get("type", "")) not in cls._types:
                raise invalid
            return cls(doc)
        raise invalid

    @classmethod
    def from_string(cls, text: str) -> Self:
        """From a URN (``urn:ogc:def:datum:EPSG::6326``), WKT or PROJJSON."""
        return cls._from_string(text)

    @classmethod
    def from_user_input(cls, user_input: Any) -> Self:
        """From what pyproj takes: a string (``from_string``), a PROJJSON
        dict, an EPSG code, an ``(authority, code)`` pair, or an object
        with ``to_json``."""
        if isinstance(user_input, cls):
            return user_input
        if isinstance(user_input, str):
            return cls.from_string(user_input)
        if isinstance(user_input, dict):
            return cls.from_json_dict(user_input)  # pyright: ignore[reportUnknownArgumentType]
        if isinstance(user_input, int) and hasattr(cls, "from_epsg"):
            return cls.from_epsg(user_input)  # type: ignore[attr-defined, no-any-return]
        if isinstance(user_input, (list, tuple)) and len(user_input) == 2 and hasattr(cls, "from_authority"):  # pyright: ignore[reportUnknownArgumentType]
            return cls.from_authority(*user_input)  # type: ignore[attr-defined, no-any-return]
        to_json = getattr(cast(object, user_input), "to_json", None)
        if callable(to_json):
            return cls.from_json(str(to_json()))
        raise CRSError(f"Invalid {cls.__name__} input: {user_input!r}")

    @property
    def name(self) -> str:
        return str(self._json.get("name", "undefined"))

    @property
    def remarks(self) -> str | None:
        remarks = self._json.get("remarks")
        return None if remarks is None else str(remarks)

    @property
    def scope(self) -> str | None:
        scope = self._json.get("scope")
        return None if scope is None else str(scope)

    def to_json_dict(self) -> JsonDict:
        """The PROJJSON as a dict (a copy)."""
        out: JsonDict = json.loads(json.dumps(self._json))
        if self._type and "type" not in out:
            out = {"type": self._type, **out}
        return out

    def to_json(self, pretty: bool = False, indentation: int = 2) -> str:
        """The PROJJSON as text."""
        return dumps(self.to_json_dict(), pretty, indentation)

    def to_wkt(self, version: Any = "WKT2_2019", pretty: bool = False, output_axis_rule: bool | None = None) -> str | None:
        """The WKT of this part on its own, as PROJ writes it, or ``None``
        where PROJ writes nothing (a transformation in WKT1_GDAL).
        WKT2_2019 and WKT1_GDAL are written; other versions raise
        ``NotImplementedError``."""
        # Kept by the arguments, as CRS.to_wkt does.
        key = (version, pretty, output_axis_rule)
        try:
            return self._wkts[key]
        except (KeyError, TypeError):
            pass
        wkt = self._write(version, pretty, output_axis_rule)
        try:
            self._wkts[key] = wkt
        except TypeError:
            pass
        return wkt

    @cached_property
    def _wkts(self) -> dict[tuple[Any, Any, Any], str | None]:
        return {}

    def _write(self, version: Any, pretty: bool, output_axis_rule: bool | None) -> str | None:
        try:
            return _wkt.write(self.to_json_dict(), version, pretty, output_axis_rule)
        except CRSError:
            return None

    def is_exact_same(self, other: object) -> bool:
        """Whether ``other`` has the same PROJJSON."""
        return isinstance(other, type(self)) and self.to_json_dict() == other.to_json_dict()

    def __eq__(self, other: object) -> bool:
        """Equivalence, as pyproj's: ``other`` as ``from_user_input``
        takes it, with the same definition whatever its ids, usages and
        remarks."""
        try:
            other = self.from_user_input(other)
        except CRSError:
            return False
        return _definition(self.to_json_dict()) == _definition(other.to_json_dict())

    def __hash__(self) -> int:
        # Of what equivalence compares, so that equal objects hash equal.
        return hash(json.dumps(_definition(self.to_json_dict()), sort_keys=True))

    def __str__(self) -> str:
        return self.name

    def __repr__(self) -> str:
        return self.to_wkt(pretty=True) or f"<{type(self).__name__}: {self.name}>"


class _EpsgObject(_JsonObject):
    """An object EPSG registers: an ellipsoid, a prime meridian, a datum or
    a coordinate operation, which can be looked up by code and by name."""

    # Its kind in `xeibe-crs`'s objects table.
    _kind = ""

    @classmethod
    def _by_code(cls, auth_name: str, code: int | str, urn_type: str | None = None) -> JsonDict | None:
        if auth_name != "EPSG" or not str(code).strip().isdigit():
            return None
        found = _native.object_json(cls._kind, int(code))
        return None if found is None else json.loads(found)

    @classmethod
    def from_authority(cls, auth_name: str, code: int | str) -> Self:
        """By authority and code; EPSG is the only authority."""
        doc = cls._by_code(auth_name, code)
        if doc is None:
            raise CRSError(f"Invalid authority or code ({auth_name}, {code})")
        return cls(doc)

    @classmethod
    def from_epsg(cls, code: int | str) -> Self:
        """By EPSG code."""
        return cls.from_authority("EPSG", code)

    @classmethod
    def _named(cls, name: str, auth_name: str | None) -> list[JsonDict]:
        """The objects with this name or alias, in PROJ's order."""
        if auth_name not in (None, "EPSG"):
            return []
        return [json.loads(doc) for _, doc in _native.objects_by_name(cls._kind, name)]

    @classmethod
    def from_name(cls, name: str, auth_name: str | None = None) -> Self:
        """By name or alias, as PROJ's name search finds it: exactly,
        ignoring the case of ASCII letters."""
        found = cls._named(name, auth_name)
        if not found:
            raise CRSError(f"Invalid {cls._what} name: {name}")
        return cls(found[0])

    @classmethod
    def from_string(cls, text: str) -> Self:
        """From a URN, WKT, PROJJSON or a name."""
        try:
            return cls._from_string(text)
        except CRSError as err:
            try:
                return cls.from_name(text)
            except CRSError:
                raise err from None


def _always(doc: JsonDict) -> bool:
    return True


@dataclass(frozen=True)
class AxisInfo:
    """One axis of a coordinate system."""

    name: str
    abbrev: str
    direction: str
    unit_conversion_factor: float
    unit_name: str
    unit_auth_code: str
    unit_code: str

    def __str__(self) -> str:
        return f"{self.abbrev}[{self.direction}]: {self.name} ({self.unit_name})"

    def __repr__(self) -> str:
        return (
            f"Axis(name={self.name}, abbrev={self.abbrev}, direction={self.direction}, "
            f"unit_auth_code={self.unit_auth_code}, unit_code={self.unit_code}, "
            f"unit_name={self.unit_name})"
        )


def axis_from_json(axis: JsonDict) -> AxisInfo:
    """An axis of a PROJJSON coordinate system."""
    name, factor, auth, code = _unit(axis.get("unit", "unknown"), _AXIS_UNITS)
    return AxisInfo(
        name=str(axis.get("name", "")),
        abbrev=str(axis.get("abbreviation", "")),
        direction=str(axis.get("direction", "")),
        unit_conversion_factor=factor,
        unit_name=name,
        unit_auth_code=auth,
        unit_code=code,
    )


class CoordinateSystem(_JsonObject):
    """A coordinate system: its type (``ellipsoidal``, ``cartesian``, …) and axes."""

    _type = "CoordinateSystem"
    _types = frozenset({"CoordinateSystem"})
    _what = "coordinate system"

    @property
    def name(self) -> str:
        return str(self._json.get("subtype", "undefined")).lower()

    @property
    def axis_list(self) -> list[AxisInfo]:
        return [axis_from_json(a) for a in get_dicts(self._json, "axis")]

    def _write(self, version: Any, pretty: bool, output_axis_rule: bool | None) -> str | None:
        """The ``CS`` and ``AXIS`` nodes; empty in WKT1_GDAL, which has no
        coordinate system on its own, as in PROJ."""
        if _wkt.version_name(version) == "WKT1_GDAL":
            return ""
        return super()._write(version, pretty, output_axis_rule)


class Ellipsoid(_EpsgObject):
    """A reference ellipsoid (or sphere)."""

    _type = "Ellipsoid"
    _types = frozenset({"Ellipsoid"})
    _what = "ellipsoid"
    _kind = "ellipsoid"
    _urns = {"ellipsoid": _always}

    @classmethod
    def from_name(cls, name: str, auth_name: str | None = None) -> Self:
        """By name or alias, else, as pyproj does, by one of PROJ's
        ``+ellps`` names or their descriptions (``intl``)."""
        found = cls._named(name, auth_name)
        if found:
            return cls(found[0])
        if auth_name not in (None, "PROJ"):
            raise CRSError(f"Invalid ellipsoid name: {name}")
        from xeibe_proj.list import get_ellps_map

        ellps = get_ellps_map()
        key = name if name in ellps else next((k for k, v in ellps.items() if v.get("description") == name), None)
        if key is None:
            raise CRSError(f"Invalid ellipsoid name: {name}")
        params = ellps[key]
        doc: JsonDict = {"type": "Ellipsoid", "name": params["description"], "semi_major_axis": params["a"]}
        if "rf" in params:
            doc["inverse_flattening"] = params["rf"]
        else:
            doc["semi_minor_axis"] = params["b"]
        return cls(doc)

    @property
    def semi_major_metre(self) -> float:
        if "radius" in self._json:
            return _value(self._json["radius"])
        return _value(self._json["semi_major_axis"])

    @property
    def semi_minor_metre(self) -> float:
        if "semi_minor_axis" in self._json:
            return _value(self._json["semi_minor_axis"])
        rf = self.inverse_flattening
        a = self.semi_major_metre
        return a if rf == 0 else a * (1 - 1 / rf)

    @property
    def inverse_flattening(self) -> float:
        if "inverse_flattening" in self._json:
            return _value(self._json["inverse_flattening"])
        if "semi_minor_axis" in self._json:
            a, b = self.semi_major_metre, _value(self._json["semi_minor_axis"])
            return 0.0 if a == b else a / (a - b)
        return 0.0

    @property
    def is_semi_minor_computed(self) -> bool:
        return "semi_minor_axis" not in self._json


class PrimeMeridian(_EpsgObject):
    """A prime meridian."""

    _type = "PrimeMeridian"
    _types = frozenset({"PrimeMeridian"})
    _what = "prime meridian"
    _kind = "prime_meridian"
    _urns = {"meridian": _always}

    def _longitude(self) -> tuple[float, str, float]:
        lon = self._json.get("longitude", 0.0)
        l = as_dict(lon)
        if l is not None:
            name, factor, _, _ = _unit(l.get("unit", "degree"), _AXIS_UNITS)
            return float(l["value"]), name, factor
        return float(lon), "degree", math.pi / 180

    @property
    def longitude(self) -> float:
        return self._longitude()[0]

    @property
    def unit_name(self) -> str:
        return self._longitude()[1]

    @property
    def unit_conversion_factor(self) -> float:
        return self._longitude()[2]


_GREENWICH: JsonDict = {"name": "Greenwich", "longitude": 0}


# Each of pyproj's `DatumType`s, by the PROJJSON types PROJ's name search
# finds under it (`createObjectsFromName`'s tables). Without a type, pyproj
# tries them in this order.
_DATUM_TYPES: dict[str, Callable[[JsonDict], bool]] = {
    "DATUM_ENSEMBLE": lambda d: d.get("type") == "DatumEnsemble",
    "GEODETIC_REFERENCE_FRAME": lambda d: (
        d.get("type") in ("GeodeticReferenceFrame", "DynamicGeodeticReferenceFrame")
        or (d.get("type") == "DatumEnsemble" and "ellipsoid" in d)
    ),
    "DYNAMIC_GEODETIC_REFERENCE_FRAME": lambda d: d.get("type") == "DynamicGeodeticReferenceFrame",
    "VERTICAL_REFERENCE_FRAME": lambda d: (
        d.get("type") in ("VerticalReferenceFrame", "DynamicVerticalReferenceFrame")
        or (d.get("type") == "DatumEnsemble" and "ellipsoid" not in d)
    ),
    "DYNAMIC_VERTICAL_REFERENCE_FRAME": lambda d: d.get("type") == "DynamicVerticalReferenceFrame",
}


def _enum_name(value: object) -> str:
    """An enum member's name, or a string as pyproj's ``BaseEnum.create``
    takes it."""
    name = getattr(value, "name", value)
    return str(name).upper().replace(" ", "_")


class Datum(_EpsgObject):
    """A datum or datum ensemble."""

    _types = frozenset(
        {
            "GeodeticReferenceFrame",
            "DynamicGeodeticReferenceFrame",
            "VerticalReferenceFrame",
            "DynamicVerticalReferenceFrame",
            "EngineeringDatum",
            "ParametricDatum",
            "TemporalDatum",
            "DatumEnsemble",
        }
    )
    _what = "datum"
    _kind = "datum"
    _urns = {"datum": _always, "ensemble": lambda d: d.get("type") == "DatumEnsemble"}

    @classmethod
    def from_name(cls, name: str, auth_name: str | None = None, datum_type: Any = None) -> Self:
        """By name or alias, of ``datum_type`` (a ``DatumType`` or its
        name), or else the first of PROJ's types that has one: an ensemble,
        a geodetic datum, then a vertical one."""
        found = cls._named(name, auth_name)
        types = list(_DATUM_TYPES) if datum_type is None else [_enum_name(datum_type)]
        for t in types:
            check = _DATUM_TYPES.get(t)
            if check is None:
                raise CRSError(f"Invalid datum type: {datum_type}")
            for doc in found:
                if check(doc):
                    return cls(doc)
        raise CRSError(f"Invalid datum name: {name}")

    @property
    def type_name(self) -> str:
        if "members" in self._json:
            return "Datum Ensemble"
        return {
            "GeodeticReferenceFrame": "Geodetic Reference Frame",
            "DynamicGeodeticReferenceFrame": "Dynamic Geodetic Reference Frame",
            "VerticalReferenceFrame": "Vertical Reference Frame",
            "DynamicVerticalReferenceFrame": "Dynamic Vertical Reference Frame",
            "EngineeringDatum": "Engineering Datum",
            "ParametricDatum": "Parametric Datum",
            "TemporalDatum": "Temporal Datum",
        }.get(str(self._json.get("type", "")), "Unknown Datum")

    @property
    def ellipsoid(self) -> Ellipsoid | None:
        ellipsoid = as_dict(self._json.get("ellipsoid"))
        return None if ellipsoid is None else Ellipsoid(ellipsoid)

    @property
    def prime_meridian(self) -> PrimeMeridian | None:
        if "ellipsoid" not in self._json:
            return None
        return PrimeMeridian(as_dict(self._json.get("prime_meridian")) or _GREENWICH)

    def to_json_dict(self) -> JsonDict:
        out = super().to_json_dict()
        if "members" in out and "type" not in out:
            out = {"type": "DatumEnsemble", **out}
        return out


@dataclass(frozen=True)
class Param:
    """A parameter of a coordinate operation."""

    name: str
    auth_name: str
    code: str
    # A number, or a file name for a grid.
    value: float | str
    unit_conversion_factor: float
    unit_name: str
    unit_auth_name: str
    unit_code: str
    # PROJ's kind of unit: "linear", "angular", "scale", "time", ….
    unit_category: str = "unknown"

    def __str__(self) -> str:
        return f"{self.auth_name}:{self.code}: {self.name}"

    def __repr__(self) -> str:
        return (
            f"Param(name={self.name}, auth_name={self.auth_name}, code={self.code}, "
            f"value={self.value}, unit_name={self.unit_name}, "
            f"unit_auth_name={self.unit_auth_name}, unit_code={self.unit_code}, "
            f"unit_category={self.unit_category})"
        )


# PROJ's unit categories, by PROJJSON's shorthand units and unit types.
_UNIT_CATEGORIES = {
    "metre": "linear",
    "degree": "angular",
    "unity": "scale",
    "LinearUnit": "linear",
    "AngularUnit": "angular",
    "ScaleUnit": "scale",
    "TimeUnit": "time",
    "ParametricUnit": "parametric",
}


def _unit_category(unit: object) -> str:
    u = as_dict(unit)
    key = str(u.get("type", "")) if u is not None else str(unit)
    return _UNIT_CATEGORIES.get(key, "unknown")


def _param(param: JsonDict) -> Param:
    auth, code = ident(param) or ("", "")
    unit = param.get("unit", "unity")
    unit_name, factor, unit_auth, unit_code = _unit(unit, _PARAM_UNITS)
    value = param.get("value", 0.0)
    # A grid's file name is a parameter's value too.
    is_file = isinstance(value, str)
    return Param(
        name=str(param.get("name", "")),
        auth_name=auth,
        code=code,
        value=value if is_file else float(value),
        unit_conversion_factor=factor,
        unit_name=unit_name,
        unit_auth_name=unit_auth,
        unit_code=unit_code,
        unit_category="unknown" if is_file else _unit_category(unit),
    )


@dataclass(frozen=True)
class Grid:
    """A grid file an operation reads, from PROJ-data."""

    short_name: str
    full_name: str
    package_name: str
    url: str
    direct_download: bool
    open_license: bool
    available: bool

    def __repr__(self) -> str:
        return (
            f"Grid(short_name={self.short_name}, full_name={self.full_name}, "
            f"package_name={self.package_name}, url={self.url}, "
            f"direct_download={self.direct_download}, "
            f"open_license={self.open_license}, available={self.available})"
        )


# EPSG's Helmert methods, as PROJ's getTOWGS84Parameters groups them.
_COORDINATE_FRAME = {"1032", "1038", "9607", "1132", "1133", "1140", "1149"}
_POSITION_VECTOR = {"1033", "1037", "9606"}
_TRANSLATIONS = {"1031", "1035", "9603"}
_ARC_SECOND = math.pi / 180 / 3600
# WKT1's units of the TOWGS84 values, by EPSG parameter: name and SI factor.
_TOWGS84_UNITS = {
    "8608": ("arc-second", _ARC_SECOND),
    "8609": ("arc-second", _ARC_SECOND),
    "8610": ("arc-second", _ARC_SECOND),
    "8611": ("parts per million", 1e-6),
}


# pyproj's `CoordinateOperationType`s, by the tables of EPSG's operations
# PROJ's name search reads for each.
_OPERATION_TABLES: dict[str, tuple[str, ...]] = {
    "CONVERSION": ("conversion",),
    "TRANSFORMATION": ("helmert_transformation", "grid_transformation", "other_transformation"),
    "CONCATENATED_OPERATION": ("concatenated_operation",),
}


class CoordinateOperation(_EpsgObject):
    """A coordinate operation: the conversion of a projected CRS, or one a
    transformer may use."""

    _type = "Conversion"
    _types = frozenset({"Conversion", "Transformation", "ConcatenatedOperation", "CoordinateOperation"})
    _what = "coordinate operation"
    _kind = "conversion"
    _urns = {"coordinateOperation": _always}

    @classmethod
    def _by_code(cls, auth_name: str, code: int | str, urn_type: str | None = None) -> JsonDict | None:
        """A conversion, or else one of EPSG's transformations and
        concatenated operations."""
        found = super()._by_code(auth_name, code, urn_type)
        if found is not None or auth_name != "EPSG" or not str(code).strip().isdigit():
            return found
        from xeibe_proj import _operation_json

        return _operation_json.epsg_operation(int(code))

    @classmethod
    def from_authority(cls, auth_name: str, code: int | str, use_proj_alternative_grid_names: bool = False) -> Self:
        """By authority and code: a conversion, transformation or
        concatenated operation. With ``use_proj_alternative_grid_names``
        its grids are PROJ-data's files, else the names EPSG gives."""
        doc = cls._by_code(auth_name, code)
        if doc is None:
            raise CRSError(f"Invalid authority or code ({auth_name}, {code})")
        return cls._with_usage(doc, use_proj_alternative_grid_names)

    @classmethod
    def from_epsg(cls, code: int | str, use_proj_alternative_grid_names: bool = False) -> Self:
        """By EPSG code."""
        return cls.from_authority("EPSG", code, use_proj_alternative_grid_names)

    @classmethod
    def from_name(
        cls, name: str, auth_name: str | None = None, coordinate_operation_type: Any = "CONVERSION"
    ) -> Self:
        """By name, among the operations of ``coordinate_operation_type``
        (a ``CoordinateOperationType`` or its name; conversions unless
        given)."""
        kind = _enum_name(coordinate_operation_type)
        tables = _OPERATION_TABLES.get(kind)
        if tables is None:
            raise CRSError(f"Invalid coordinate operation name: {name}")
        if kind == "CONVERSION":
            found = cls._named(name, auth_name)
            if found:
                return cls._with_usage(found[0], False)
        elif auth_name in (None, "EPSG"):
            from xeibe_proj import _operation_json

            for code in _operation_json.epsg_operations_named(name, tables):
                doc = _operation_json.epsg_operation(code)
                if doc is not None:
                    return cls._with_usage(doc, False)
        raise CRSError(f"Invalid coordinate operation name: {name}")

    @classmethod
    def _from_string(cls, text: str) -> Self:
        made = super()._from_string(text)
        return cls._with_usage(made._json, False)

    @classmethod
    def _with_usage(cls, doc: JsonDict, proj_names: bool) -> Self:
        """An operation with its area of use, scope and grids, as pyproj
        describes one. A conversion's usage, which PROJ keeps but doesn't
        write, leaves its PROJJSON."""
        usage = (get_dicts(doc, "usages") or [doc])[0]
        bbox = as_dict(usage.get("bbox"))
        area = None
        if bbox is not None:
            name = usage.get("area")
            area = AreaOfUse(
                float(bbox["west_longitude"]),
                float(bbox["south_latitude"]),
                float(bbox["east_longitude"]),
                float(bbox["north_latitude"]),
                None if name is None else str(name),
            )
        scope = usage.get("scope")
        if doc.get("type") == "Conversion":
            doc = {k: v for k, v in doc.items() if k not in ("scope", "area", "bbox", "usages")}
        return cls(doc, area, _grids_of(doc, proj_names), None if scope is None else str(scope))

    @classmethod
    def _of_crs(cls, conversion: JsonDict, crs_code: int | None) -> CoordinateOperation:
        """A CRS's conversion, with the area of use and scope of EPSG's
        conversion it names, or that EPSG gives CRS ``crs_code``."""
        auth, code = ident(conversion) or ("", "")
        if not auth and crs_code is not None:
            from xeibe_proj import _operation_json

            auth, code = "EPSG", str(_operation_json.crs_conversion(crs_code) or "")
        found = cls._by_code(auth, code) or {}
        if found.get("type") != "Conversion":
            found = {}
        usage = {k: v for k, v in found.items() if k in ("scope", "area", "bbox", "usages")}
        return cls._with_usage({"type": "Conversion", **conversion, **usage}, False)

    def __init__(
        self,
        projjson: JsonDict,
        area_of_use: AreaOfUse | None = None,
        grids: list[Grid] | None = None,
        scope: str | None = None,
    ) -> None:
        super().__init__(projjson)
        self._area = area_of_use
        self._grids = grids or []
        self._scope = scope

    @property
    def scope(self) -> str | None:
        return self._scope if self._scope is not None else super().scope

    @property
    def type_name(self) -> str:
        return {
            "Conversion": "Conversion",
            "Transformation": "Transformation",
            "ConcatenatedOperation": "Concatenated Operation",
        }.get(str(self._json.get("type", "Conversion")), "Other Coordinate Operation")

    @property
    def method_name(self) -> str:
        return str(get_dict(self._json, "method").get("name", "unknown"))

    @property
    def method_auth_name(self) -> str:
        return (ident(get_dict(self._json, "method")) or ("", ""))[0]

    @property
    def method_code(self) -> str:
        return (ident(get_dict(self._json, "method")) or ("", ""))[1]

    @property
    def params(self) -> list[Param]:
        return [_param(p) for p in get_dicts(self._json, "parameters")]

    @property
    def accuracy(self) -> float:
        accuracy = self._json.get("accuracy")
        return -1.0 if accuracy is None else float(accuracy)

    @property
    def area_of_use(self) -> AreaOfUse | None:
        return self._area

    @property
    def is_instantiable(self) -> bool:
        return True

    @property
    def has_ballpark_transformation(self) -> bool:
        return False

    @property
    def towgs84(self) -> list[float]:
        """A Helmert transformation's values as WKT1's ``TOWGS84`` takes
        them (Position Vector, metres, arc-seconds, parts per million), as
        PROJ's ``getTOWGS84Parameters`` gives them: the three translations
        of a geocentric translation, all seven of the others; empty for any
        other operation."""
        method = get_dict(self._json, "method")
        code = (ident(method) or ("", ""))[1]
        name = str(method.get("name", "")).lower()
        params = get_dicts(self._json, "parameters")
        if code in _COORDINATE_FRAME or (len(params) == 7 and "coordinate frame" in name):
            seven, sign = True, -1.0
        elif code in _POSITION_VECTOR or (len(params) == 7 and "position vector" in name):
            seven, sign = True, 1.0
        elif code in _TRANSLATIONS or (len(params) == 3 and "geocentric translations" in name):
            seven, sign = False, 1.0
        else:
            return []
        # Each value in WKT1's unit: as given when it is in that unit, so
        # that 1.884 arc-seconds stay 1.884, else through the SI unit.
        values: dict[str, float] = {}
        for p in params:
            param = _param(p)
            if isinstance(param.value, str):
                return []
            wkt1_unit, wkt1_factor = _TOWGS84_UNITS.get(param.code, ("metre", 1.0))
            if param.unit_name == wkt1_unit:
                values[param.code] = param.value
            else:
                values[param.code] = param.value * param.unit_conversion_factor / wkt1_factor
        translations = [values.get(c) for c in ("8605", "8606", "8607")]
        if None in translations:
            return []
        if not seven:
            return [t for t in translations if t is not None]
        if not all(c in values for c in ("8608", "8609", "8610", "8611")):
            return []
        # The sign of a Position Vector rotation, and never -0.0.
        rotations = [sign * values[c] + 0.0 for c in ("8608", "8609", "8610")]
        return [*(t for t in translations if t is not None), *rotations, values["8611"]]

    def __repr__(self) -> str:
        return (
            f"<Coordinate Operation: {self.type_name}>\n"
            f"Name: {self.name}\n"
            f"Method: {self.method_name}\n"
            f"Area of Use:\n{self.area_of_use or '- undefined'}"
        )

    @property
    def grids(self) -> list[Grid]:
        return list(self._grids)

    @property
    def operations(self) -> tuple[CoordinateOperation, ...]:
        return tuple(CoordinateOperation(s) for s in get_dicts(self._json, "steps"))


def _grids_of(doc: JsonDict, proj_names: bool) -> list[Grid]:
    """The grid files an operation's parameters name, its steps' included:
    as EPSG names them, or, with ``proj_names``, as PROJ-data's files."""
    from xeibe_proj import _grids, network

    out: list[Grid] = []
    for op in [doc, *get_dicts(doc, "steps")]:
        for p in get_dicts(op, "parameters"):
            value = p.get("value")
            if not isinstance(value, str):
                continue
            found = _native.proj_data_file(value, str(op.get("name", ""))) if proj_names else None
            if found is None:
                out.append(Grid(value, "", "", "", False, False, False))
                continue
            file, cdn = found
            local = _grids._local(file)  # pyright: ignore[reportPrivateUsage]
            available = local is not None or (cdn and network.is_network_enabled())
            out.append(Grid(file, str(local or ""), "", _grids.CDN + file if cdn else "", cdn, cdn, available))
    return out
