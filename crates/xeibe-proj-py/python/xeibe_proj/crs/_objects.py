"""The parts of a CRS (axes, datum, ellipsoid, …), read from PROJJSON with
pyproj's attribute names. pyproj's ``repr`` of these is WKT, which comes in
phase 2; until then it names the object."""

from __future__ import annotations

import json
import math
from dataclasses import dataclass

from xeibe_proj._json import JsonDict, as_dict, get_dict, get_dicts, ident
from xeibe_proj.aoi import AreaOfUse

__all__ = [
    "AxisInfo",
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


class _JsonObject:
    """An object kept as its PROJJSON."""

    _type = ""

    def __init__(self, projjson: JsonDict) -> None:
        self._json = projjson

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
        return json.dumps(self.to_json_dict(), indent=indentation if pretty else None)

    def is_exact_same(self, other: object) -> bool:
        """Whether ``other`` has the same PROJJSON."""
        return isinstance(other, type(self)) and self.to_json_dict() == other.to_json_dict()

    def __eq__(self, other: object) -> bool:
        return self.is_exact_same(other)

    def __hash__(self) -> int:
        return hash(self.to_json())

    def __str__(self) -> str:
        return self.name

    def __repr__(self) -> str:
        return f"<{type(self).__name__}: {self.name}>"


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

    @property
    def name(self) -> str:
        return str(self._json.get("subtype", "undefined")).lower()

    @property
    def axis_list(self) -> list[AxisInfo]:
        return [axis_from_json(a) for a in get_dicts(self._json, "axis")]


class Ellipsoid(_JsonObject):
    """A reference ellipsoid (or sphere)."""

    _type = "Ellipsoid"

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


class PrimeMeridian(_JsonObject):
    """A prime meridian."""

    _type = "PrimeMeridian"

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


class Datum(_JsonObject):
    """A datum or datum ensemble."""

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
    value: float
    unit_conversion_factor: float
    unit_name: str
    unit_auth_name: str
    unit_code: str

    def __str__(self) -> str:
        return f"{self.auth_name}:{self.code}: {self.name}"


def _param(param: JsonDict) -> Param:
    auth, code = ident(param) or ("", "")
    unit_name, factor, unit_auth, unit_code = _unit(param.get("unit", "unity"), _PARAM_UNITS)
    return Param(
        name=str(param.get("name", "")),
        auth_name=auth,
        code=code,
        value=float(param.get("value", 0.0)),
        unit_conversion_factor=factor,
        unit_name=unit_name,
        unit_auth_name=unit_auth,
        unit_code=unit_code,
    )


class CoordinateOperation(_JsonObject):
    """A conversion (the map projection of a projected CRS)."""

    _type = "Conversion"

    def __init__(self, projjson: JsonDict, area_of_use: AreaOfUse | None = None) -> None:
        super().__init__(projjson)
        self._area = area_of_use

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
        return []

    @property
    def grids(self) -> list[object]:
        return []

    @property
    def operations(self) -> tuple[CoordinateOperation, ...]:
        return tuple(CoordinateOperation(s) for s in get_dicts(self._json, "steps"))
