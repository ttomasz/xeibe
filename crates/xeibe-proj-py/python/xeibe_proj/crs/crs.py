"""``CRS``: a coordinate reference system, kept as PROJJSON.

Input is an EPSG code in any of its usual spellings, one of OGC's three
longitude-first CRSs, or PROJJSON. WKT comes in phase 2 and PROJ strings in
phase 3 (``docs/pyproj-compat.md``).
"""

from __future__ import annotations

import json
import re
from functools import cached_property
from collections.abc import Sequence
from typing import Any, NamedTuple, Self, cast

from xeibe_proj import _epsg, _native
from xeibe_proj._json import JsonDict, as_dict, as_list, get_dict, get_dicts, ident
from xeibe_proj._unsupported import not_yet, out
from xeibe_proj.aoi import AreaOfUse
from xeibe_proj.crs._objects import (
    AxisInfo,
    CoordinateOperation,
    CoordinateSystem,
    Datum,
    Ellipsoid,
    PrimeMeridian,
    axis_from_json,
)
from xeibe_proj.exceptions import CRSError

__all__ = ["CRS", "AuthorityMatchInfo"]



class AuthorityMatchInfo(NamedTuple):
    """An authority code a CRS matches, with the confidence (0 to 100)."""

    auth_name: str
    code: str
    confidence: int


# Authorities whose codes aren't in EPSG.
_OTHER_AUTHORITIES = {"ESRI", "IGNF", "IAU_2015", "NKG", "PROJ", "OGC", "NRCAN"}

_AUTH_CODE = re.compile(r"(?P<auth>[A-Za-z_0-9]+)\s*:\s*(?P<code>[^:]+)")
_URN = re.compile(r"urn:ogc:def:crs:(?P<auth>[^:]+):[^:]*:(?P<code>[^:]+)", re.IGNORECASE)
_URN_COMPOUND = re.compile(r"urn:ogc:def:crs,(?P<parts>.+)", re.IGNORECASE)
_URN_PART = re.compile(r"crs:(?P<auth>[^:]+):[^:]*:(?P<code>[^:,]+)", re.IGNORECASE)
_URL = re.compile(
    r"https?://www\.opengis\.net/def/crs/(?P<auth>[^/]+)/[^/]*/(?P<code>[^/?#]+)/?", re.IGNORECASE
)
_URL_COMPOUND = re.compile(r"https?://www\.opengis\.net/def/crs-compound\?(?P<query>.+)", re.IGNORECASE)
_GML_SRS = re.compile(r"https?://www\.opengis\.net/gml/srs/epsg\.xml#(?P<code>\d+)", re.IGNORECASE)
_WKT = re.compile(
    r"\s*(GEOGCS|PROJCS|GEOCCS|VERT_CS|COMPD_CS|LOCAL_CS|GEOGCRS|GEODCRS|PROJCRS|VERTCRS|"
    r"COMPOUNDCRS|ENGCRS|BOUNDCRS|GEODETICCRS|GEOGRAPHICCRS|PROJECTEDCRS|VERTICALCRS|"
    r"ENGINEERINGCRS|DERIVEDPROJCRS|BASEGEOGCRS)\s*\[",
    re.IGNORECASE,
)

# What equality ignores: identifiers and descriptions.
_METADATA = {"$schema", "id", "ids", "remarks", "scope", "area", "bbox", "usages"}


def _epsg_json(code: int) -> JsonDict:
    projjson = _epsg.projjson(code)
    if projjson is None:
        raise CRSError(f"EPSG:{code} is not in the EPSG tables (EPSG {_epsg_version()})")
    return projjson


def _epsg_version() -> str:
    from xeibe_proj import _native

    return _native.epsg_version()


def _compound(codes: list[int]) -> JsonDict:
    """``EPSG:<horizontal>+<vertical>``: EPSG's compound CRS of the two if
    there is one, else one built from them as PROJ builds it."""
    if len(codes) != 2:
        raise CRSError(f"a compound CRS needs two components, not {len(codes)}")
    code = _epsg.compounds().get((codes[0], codes[1]))
    if code is not None:
        return _epsg_json(code)
    h, v = _epsg_json(codes[0]), _epsg_json(codes[1])
    return {
        "type": "CompoundCRS",
        "name": f"{h['name']} + {v['name']}",
        "components": [h, v],
    }


def _from_authority(auth: str, code: str) -> JsonDict:
    auth = auth.strip().upper()
    code = code.strip()
    if auth == "EPSG":
        if "+" in code:
            return _compound([_epsg_code(c) for c in code.split("+")])
        return _epsg_json(_epsg_code(code))
    if auth == "OGC":
        projjson = _epsg.ogc_projjson(code)
        if projjson is not None:
            return projjson
    if auth in _OTHER_AUTHORITIES or auth == "OGC":
        raise out(f"{auth}:{code}", "only EPSG codes and OGC:CRS84, CRS83 and CRS27 are known")
    raise CRSError(f"unknown authority {auth!r} in {auth}:{code}")


def _epsg_code(text: str) -> int:
    try:
        return int(text.strip())
    except ValueError:
        raise CRSError(f"not an EPSG code: {text!r}") from None


def _from_string(text: str) -> JsonDict:
    s = text.strip()
    if s.startswith("{"):
        try:
            projjson = json.loads(s)
        except json.JSONDecodeError as e:
            raise CRSError(f"invalid PROJJSON: {e}") from None
        return _check_projjson(projjson)
    if s.startswith("+") or s.lower().startswith("proj="):
        raise not_yet("CRS from a PROJ string", 3)
    if _WKT.match(s):
        raise not_yet("CRS from WKT", 2)
    if m := _GML_SRS.fullmatch(s):
        return _epsg_json(int(m["code"]))
    if m := _URN_COMPOUND.fullmatch(s):
        parts = [_URN_PART.fullmatch(p.strip()) for p in re.split(r",(?=crs:)", m["parts"])]
        if any(p is None or p["auth"].upper() != "EPSG" for p in parts):
            raise CRSError(f"unsupported compound URN: {text!r}")
        return _compound([_epsg_code(p["code"]) for p in parts if p is not None])
    if m := _URN.fullmatch(s):
        return _from_authority(m["auth"], m["code"])
    if m := _URL_COMPOUND.fullmatch(s):
        urls = [q.split("=", 1)[-1] for q in m["query"].split("&")]
        parts = [_URL.fullmatch(u) for u in urls]
        if any(p is None or p["auth"].upper() != "EPSG" for p in parts):
            raise CRSError(f"unsupported compound URL: {text!r}")
        return _compound([_epsg_code(p["code"]) for p in parts if p is not None])
    if m := _URL.fullmatch(s):
        return _from_authority(m["auth"], m["code"])
    if m := _AUTH_CODE.fullmatch(s):
        return _from_authority(m["auth"], m["code"])
    code = _epsg.code_for_name(s)
    if code is not None:
        return _epsg_json(code)
    raise CRSError(f"Invalid projection: {text}: not an EPSG code, OGC CRS, PROJJSON or EPSG name")


def _check_projjson(projjson: object) -> JsonDict:
    p = as_dict(projjson)
    if p is None or "type" not in p:
        raise CRSError("invalid PROJJSON: not an object with a type")
    if not str(p["type"]).endswith("CRS"):
        raise CRSError(f"PROJJSON of a {p['type']}, not of a CRS")
    return p


def _definition(value: object, ignore_axis_order: bool) -> object:
    """What equality compares: the PROJJSON without identifiers,
    descriptions and the names of CRSs and conversions."""
    d = as_dict(value)
    if d is not None:
        kind = str(d.get("type", ""))
        out_: JsonDict = {}
        for k, v in d.items():
            if k in _METADATA:
                continue
            if k == "name" and (kind.endswith("CRS") or kind == "Conversion"):
                continue
            out_[k] = _definition(v, ignore_axis_order)
        conversion = as_dict(out_.get("conversion"))
        if conversion is not None:
            conversion.pop("name", None)
        if ignore_axis_order and isinstance(out_.get("axis"), list):
            out_["axis"] = sorted(out_["axis"], key=lambda a: json.dumps(a, sort_keys=True))
        return out_
    items = as_list(value)
    if items is not None:
        return [_definition(v, ignore_axis_order) for v in items]
    return value


class CRS:
    """A coordinate reference system.

    ``CRS(x)`` takes what ``CRS.from_user_input`` takes. PROJ parameters as
    keyword arguments (``CRS(proj="utm", zone=33)``) come in phase 3.
    """

    _projjson: JsonDict
    _srs: str

    def __init__(self, projparams: Any = None, **kwargs: Any) -> None:
        if kwargs:
            raise not_yet("CRS from PROJ parameters", 3)
        if projparams is None:
            raise CRSError("Invalid CRS input: None")
        if isinstance(projparams, CRS):
            self._projjson, self._srs = projparams._projjson, projparams._srs
            return
        self._projjson, self._srs = _parse(projparams)

    @classmethod
    def _from_projjson(cls, projjson: JsonDict, srs: str | None = None) -> Self:
        crs = cls.__new__(cls)
        crs._projjson = projjson
        crs._srs = json.dumps(projjson) if srs is None else srs
        return crs

    @classmethod
    def _restore(cls, srs: str, projjson: str) -> Self:
        """Unpickle a CRS."""
        return cls._from_projjson(json.loads(projjson), srs)

    # Constructors ----------------------------------------------------------

    @classmethod
    def from_user_input(cls, value: Any, **kwargs: Any) -> Self:
        """A CRS from a ``CRS``, an EPSG code (``4326``, ``"EPSG:4326"``,
        ``"epsg:4326"``, ``"EPSG:4326+5773"``, an OGC URN or URL), an
        ``(authority, code)`` pair, ``"OGC:CRS84"`` (or CRS83, CRS27), an
        EPSG name or alias, or PROJJSON as text or a dict."""
        if isinstance(value, cls) and not kwargs:
            return value
        return cls(value, **kwargs)

    @classmethod
    def from_string(cls, in_crs_string: str) -> Self:
        """A CRS from text: see ``from_user_input``."""
        return cls(in_crs_string)

    @classmethod
    def from_epsg(cls, code: int | str) -> Self:
        """The EPSG CRS with this code."""
        return cls._from_projjson(_epsg_json(_epsg_code(str(code))), f"EPSG:{code}")

    @classmethod
    def from_authority(cls, auth_name: str, code: str | int) -> Self:
        """The CRS with this authority and code (EPSG, or OGC's three)."""
        return cls._from_projjson(_from_authority(auth_name, str(code)), f"{auth_name}:{code}")

    @classmethod
    def from_json(cls, crs_json: str) -> Self:
        """A CRS from PROJJSON text."""
        try:
            projjson = json.loads(crs_json)
        except json.JSONDecodeError as e:
            raise CRSError(f"invalid PROJJSON: {e}") from None
        return cls._from_projjson(_check_projjson(projjson), crs_json)

    @classmethod
    def from_json_dict(cls, crs_dict: dict[str, Any]) -> Self:
        """A CRS from a PROJJSON dict."""
        return cls._from_projjson(_check_projjson(crs_dict))

    @classmethod
    def from_wkt(cls, in_wkt_string: str) -> Self:
        raise not_yet("CRS.from_wkt", 2)

    @classmethod
    def from_proj4(cls, in_proj_string: str) -> Self:
        raise not_yet("CRS.from_proj4", 3)

    @classmethod
    def from_dict(cls, proj_dict: dict[str, Any]) -> Self:
        raise not_yet("CRS.from_dict", 3)

    @classmethod
    def from_cf(cls, in_cf: dict[str, Any], ellipsoidal_cs: Any = None, cartesian_cs: Any = None, vertical_cs: Any = None) -> Self:
        raise not_yet("CRS.from_cf", 3)

    # Identity ---------------------------------------------------------------

    @property
    def srs(self) -> str:
        """The input the CRS was made from."""
        return self._srs

    @property
    def name(self) -> str:
        return str(self._projjson.get("name", "undefined"))

    @property
    def type_name(self) -> str:
        """pyproj's name for the type, e.g. ``"Projected CRS"``."""
        kind = str(self._projjson.get("type", ""))
        if kind in ("GeographicCRS", "GeodeticCRS", "DerivedGeographicCRS"):
            if self._cs_subtype() == "cartesian":
                return "Geocentric CRS"
            prefix = "Derived " if kind == "DerivedGeographicCRS" else ""
            return f"{prefix}Geographic {len(self._cs_axes())}D CRS"
        return {
            "ProjectedCRS": "Projected CRS",
            "DerivedProjectedCRS": "Derived Projected CRS",
            # PROJ models EPSG's derived vertical CRSs (a change of unit or
            # direction) as plain vertical CRSs.
            "VerticalCRS": "Vertical CRS",
            "DerivedVerticalCRS": "Vertical CRS",
            "CompoundCRS": "Compound CRS",
            "EngineeringCRS": "Engineering CRS",
            "BoundCRS": "Bound CRS",
            "TemporalCRS": "Temporal CRS",
        }.get(kind, "Unknown CRS")

    def _id(self) -> tuple[str, str] | None:
        return ident(self._projjson)

    def _epsg_code(self) -> int | None:
        """The EPSG code in the PROJJSON (an OGC CRS gives its EPSG twin's)."""
        ident = self._id()
        if ident is None:
            return None
        if ident[0] == "EPSG":
            try:
                return int(ident[1])
            except ValueError:
                return None
        if ident[0] == "OGC" and ident[1] in _epsg.OGC_CRS:
            return _epsg.OGC_CRS[ident[1]][0]
        return None

    @cached_property
    def _identified(self) -> tuple[str, str] | None:
        """The authority and code: from the PROJJSON's id, else the EPSG CRS
        of the same name and definition (until identification by
        definition comes with phase 2)."""
        ident = self._id()
        if ident is not None:
            return ident
        candidate: int | None = None
        if self.is_compound:
            codes = [CRS._from_projjson(c)._identified for c in get_dicts(self._projjson, "components")]
            if len(codes) == 2 and all(c is not None and c[0] == "EPSG" for c in codes):
                candidate = _epsg.compounds().get((int(codes[0][1]), int(codes[1][1])))  # type: ignore[index]
        if candidate is None:
            candidate = _epsg.code_for_name(self.name)
        if candidate is None:
            return None
        other = _epsg.projjson(candidate)
        if other is None or not self.equals(CRS._from_projjson(other)):
            return None
        return "EPSG", str(candidate)

    def to_epsg(self, min_confidence: int = 70) -> int | None:
        """The EPSG code, or ``None``."""
        ident = self._identified
        if ident is None or ident[0] != "EPSG":
            return None
        return int(ident[1])

    def to_authority(self, auth_name: str | None = None, min_confidence: int = 70) -> tuple[str, str] | None:
        """``(authority, code)``, or ``None``."""
        ident = self._identified
        if ident is None or (auth_name is not None and ident[0] != auth_name.upper()):
            return None
        return ident

    def list_authority(self, auth_name: str | None = None, min_confidence: int = 70) -> list[AuthorityMatchInfo]:
        """The authority codes the CRS matches."""
        ident = self.to_authority(auth_name, min_confidence)
        return [] if ident is None else [AuthorityMatchInfo(ident[0], ident[1], 100)]

    def to_string(self) -> str:
        """``"EPSG:<code>"`` (or ``"OGC:CRS84"``) when the CRS has one, else the input."""
        ident = self.to_authority(min_confidence=100)
        return ":".join(ident) if ident else self.srs

    @property
    def is_deprecated(self) -> bool:
        ident = self._id()
        if ident is None or ident[0] != "EPSG":
            return False
        record = _epsg.record(int(ident[1]))
        return record is not None and record.deprecated

    # Kind ---------------------------------------------------------------------

    def _kind(self) -> str:
        return str(self._projjson.get("type", ""))

    def _cs(self) -> JsonDict:
        return get_dict(self._projjson, "coordinate_system")

    def _cs_subtype(self) -> str:
        return str(self._cs().get("subtype", "")).lower()

    def _cs_axes(self) -> list[JsonDict]:
        return get_dicts(self._cs(), "axis")

    def _components(self) -> list[CRS]:
        return [CRS._from_projjson(c) for c in get_dicts(self._projjson, "components")]

    @property
    def is_geographic(self) -> bool:
        if self.is_compound:
            return any(c.is_geographic for c in self._components()[:1])
        return self._kind() in ("GeographicCRS", "GeodeticCRS", "DerivedGeographicCRS") and self._cs_subtype() == "ellipsoidal"

    @property
    def is_projected(self) -> bool:
        if self.is_compound:
            return any(c.is_projected for c in self._components()[:1])
        return self._kind() in ("ProjectedCRS", "DerivedProjectedCRS")

    @property
    def is_vertical(self) -> bool:
        if self.is_compound:
            return any(c.is_vertical for c in self._components())
        return self._kind() in ("VerticalCRS", "DerivedVerticalCRS")

    @property
    def is_geocentric(self) -> bool:
        return self._kind() in ("GeodeticCRS", "GeographicCRS") and self._cs_subtype() == "cartesian"

    @property
    def is_compound(self) -> bool:
        return self._kind() == "CompoundCRS"

    @property
    def is_engineering(self) -> bool:
        return self._kind() == "EngineeringCRS"

    @property
    def is_derived(self) -> bool:
        return self._kind() in ("ProjectedCRS", "DerivedProjectedCRS", "DerivedGeographicCRS")

    @property
    def is_bound(self) -> bool:
        return self._kind() == "BoundCRS"

    @property
    def utm_zone(self) -> str | None:
        """The UTM zone, e.g. ``"33N"``, of a UTM projected CRS."""
        for crs in self._components():
            zone = crs.utm_zone
            if zone:
                return zone
        operation = self.coordinate_operation
        if operation is not None and "UTM ZONE" in operation.name.upper():
            return operation.name.upper().split("UTM ZONE ")[-1]
        return None

    # Parts --------------------------------------------------------------------

    def _geodetic_json(self) -> JsonDict | None:
        """The geographic or geocentric CRS underneath."""
        kind = self._kind()
        if kind in ("GeographicCRS", "GeodeticCRS"):
            return self._projjson
        if kind in ("ProjectedCRS", "DerivedProjectedCRS", "DerivedGeographicCRS"):
            base = as_dict(self._projjson.get("base_crs"))
            return None if base is None else CRS._from_projjson(base)._geodetic_json()
        if kind == "CompoundCRS":
            components = self._components()
            return components[0]._geodetic_json() if components else None
        return None

    def _datum_json(self) -> JsonDict | None:
        kind = self._kind()
        if kind in ("VerticalCRS", "EngineeringCRS"):
            return as_dict(self._projjson.get("datum") or self._projjson.get("datum_ensemble"))
        if kind == "DerivedVerticalCRS":
            base = as_dict(self._projjson.get("base_crs"))
            return None if base is None else CRS._from_projjson(base)._datum_json()
        geodetic = self._geodetic_json()
        if geodetic is None:
            return None
        return as_dict(geodetic.get("datum") or geodetic.get("datum_ensemble"))

    @property
    def datum(self) -> Datum | None:
        datum = self._datum_json()
        return None if datum is None else Datum(datum)

    @property
    def ellipsoid(self) -> Ellipsoid | None:
        datum = self.datum
        return None if datum is None else datum.ellipsoid

    @property
    def prime_meridian(self) -> PrimeMeridian | None:
        datum = self.datum
        return None if datum is None else datum.prime_meridian

    @property
    def coordinate_system(self) -> CoordinateSystem | None:
        cs = as_dict(self._projjson.get("coordinate_system"))
        return None if cs is None else CoordinateSystem(cs)

    @property
    def coordinate_operation(self) -> CoordinateOperation | None:
        # PROJ models EPSG's derived vertical CRSs as plain vertical ones.
        if self._kind() == "DerivedVerticalCRS":
            return None
        conversion = as_dict(self._projjson.get("conversion"))
        if conversion is None:
            return None
        return CoordinateOperation({"type": "Conversion", **conversion})

    @property
    def axis_info(self) -> list[AxisInfo]:
        if self.is_compound:
            return [a for c in self._components() for a in c.axis_info]
        return [axis_from_json(a) for a in self._cs_axes()]

    def _usage(self) -> JsonDict | None:
        """The first of the PROJJSON's usages (scope, area, bbox), as PROJ
        reports a CRS's area of use and scope."""
        usages = get_dicts(self._projjson, "usages")
        if usages:
            return usages[0]
        if any(k in self._projjson for k in ("scope", "area", "bbox")):
            return self._projjson
        return None

    @property
    def area_of_use(self) -> AreaOfUse | None:
        """EPSG's area of use: its first usage's box in degrees and
        description."""
        usage = self._usage()
        bbox = None if usage is None else as_dict(usage.get("bbox"))
        if usage is not None and bbox is not None:
            name = usage.get("area")
            return AreaOfUse(
                float(bbox["west_longitude"]),
                float(bbox["south_latitude"]),
                float(bbox["east_longitude"]),
                float(bbox["north_latitude"]),
                None if name is None else str(name),
            )
        # Without a box, None where PROJ says -1000 on every side; the
        # description is still in the PROJJSON ("area").
        code = self._epsg_code()
        if code is None and self.is_compound:
            components = self._components()
            return components[0].area_of_use if components else None
        record = None if code is None else _epsg.record(code)
        if record is None or record.area is None:
            return None
        return AreaOfUse(*record.area)

    @property
    def geodetic_crs(self) -> CRS | None:
        geodetic = self._geodetic_json()
        return None if geodetic is None else CRS._from_projjson(geodetic)

    @property
    def source_crs(self) -> CRS | None:
        if self._kind() == "DerivedVerticalCRS":
            return None
        base = as_dict(self._projjson.get("base_crs") or self._projjson.get("source_crs"))
        return None if base is None else CRS._from_projjson(base)

    @property
    def target_crs(self) -> CRS | None:
        target = as_dict(self._projjson.get("target_crs"))
        return None if target is None else CRS._from_projjson(target)

    @property
    def sub_crs_list(self) -> list[CRS]:
        return self._components()

    @property
    def remarks(self) -> str | None:
        """EPSG's remarks (read on first use), or the PROJJSON's."""
        remarks = self._projjson.get("remarks")
        if remarks is not None:
            return str(remarks)
        ident = self._id()
        if ident is not None and ident[0] == "EPSG" and ident[1].isdigit():
            return _native.crs_remarks(int(ident[1]))
        return None

    @property
    def scope(self) -> str | None:
        """The first usage's scope."""
        usage = self._usage()
        scope = None if usage is None else usage.get("scope")
        return None if scope is None else str(scope)

    # Output -------------------------------------------------------------------

    def to_json_dict(self) -> dict[str, Any]:
        """The PROJJSON as a dict (a copy)."""
        out_: JsonDict = json.loads(json.dumps(self._projjson))
        return out_

    def to_json(self, pretty: bool = False, indentation: int = 2) -> str:
        """The PROJJSON as text."""
        return json.dumps(self._projjson, indent=indentation if pretty else None)

    def to_wkt(self, version: Any = "WKT2_2019", pretty: bool = False, output_axis_rule: bool | None = None) -> str:
        raise not_yet("CRS.to_wkt", 2)

    def to_proj4(self, version: Any = 5) -> str:
        raise not_yet("CRS.to_proj4", 3)

    def to_dict(self) -> dict[str, Any]:
        raise not_yet("CRS.to_dict", 3)

    def to_cf(self, wkt_version: Any = None, errcheck: bool = False) -> dict[str, Any]:
        raise not_yet("CRS.to_cf", 3)

    def cs_to_cf(self) -> list[dict[str, Any]]:
        raise not_yet("CRS.cs_to_cf", 3)

    def to_2d(self, name: str | None = None) -> CRS:
        raise not_yet("CRS.to_2d", 2)

    def to_3d(self, name: str | None = None) -> CRS:
        raise not_yet("CRS.to_3d", 2)

    def get_geod(self) -> Any:
        raise not_yet("CRS.get_geod", 2)

    def get_non_deprecated(self) -> list[CRS]:
        raise not_yet("CRS.get_non_deprecated", 2)

    # Comparison ---------------------------------------------------------------

    @cached_property
    def _parsed(self) -> _native.Crs | None:
        """The CRS as ``xeibe-reproject`` reads it, or ``None`` when it can't."""
        try:
            return _native.Crs(json.dumps(self._projjson))
        except CRSError:
            return None

    def equals(self, other: Any, ignore_axis_order: bool = False) -> bool:
        """Whether both CRSs define the same coordinates, as PROJ's
        ``proj_is_equivalent_to`` judges them: the same datum, ellipsoid,
        projection and parameters and the same axes, whatever their names
        and identifiers (``Crs::equivalent`` in ``xeibe-reproject``). With
        ``ignore_axis_order``, CRSs that differ only in axis order are equal
        too (``EPSG:4326`` and ``OGC:CRS84``). A CRS ``xeibe-reproject``
        can't read is compared by its PROJJSON without identifiers,
        descriptions and CRS names."""
        try:
            other_crs = CRS.from_user_input(other)
        except (CRSError, NotImplementedError, TypeError, ValueError):
            return False
        a, b = self._parsed, other_crs._parsed
        if a is not None and b is not None:
            return a.equivalent(b, ignore_axis_order)
        return _definition(self._projjson, ignore_axis_order) == _definition(other_crs._projjson, ignore_axis_order)

    def __eq__(self, other: object) -> bool:
        return self.equals(other)

    def __hash__(self) -> int:
        # Equal CRSs may differ in their numbers by PROJ's tolerance, so the
        # hash only takes what equal CRSs always share.
        return hash((self.type_name, len(self.axis_info)))

    def is_exact_same(self, other: Any) -> bool:
        """Whether both have the same PROJJSON, identifiers and names included."""
        try:
            other_crs = CRS.from_user_input(other)
        except (CRSError, NotImplementedError, TypeError, ValueError):
            return False
        return self._projjson == other_crs._projjson

    def __reduce__(self) -> tuple[Any, tuple[str, str]]:
        return CRS._restore, (self._srs, self.to_json())

    def __str__(self) -> str:
        return self.srs

    def __repr__(self) -> str:
        axes = "".join(f"- {a}\n" for a in self.axis_info) or "- undefined\n"
        cs = self.coordinate_system
        sub_crs = ""
        if cs is not None and cs.axis_list:
            cs_name = cs.name
        else:
            components = self._components()
            cs_name = "|".join(c.coordinate_system.name if c.coordinate_system else "" for c in components)
            sub_crs = "Sub CRS:\n" + "".join(f"- {c.name}\n" for c in components)
        operation = self.coordinate_operation
        operation_repr = (
            f"Coordinate Operation:\n- name: {operation.name}\n- method: {operation.method_name}\n"
            if operation is not None
            else ""
        )
        srs = self.to_string()
        srs = srs if len(srs) <= 50 else f"{srs[:50]} ..."
        return (
            f"<{self.type_name}: {srs}>\n"
            f"Name: {self.name}\n"
            f"Axis Info [{cs_name or 'undefined'}]:\n"
            f"{axes}"
            "Area of Use:\n"
            f"{self.area_of_use or '- undefined'}\n"
            f"{operation_repr}"
            f"Datum: {self.datum}\n"
            f"- Ellipsoid: {self.ellipsoid or 'undefined'}\n"
            f"- Prime Meridian: {self.prime_meridian or 'undefined'}\n"
            f"{sub_crs}"
        )


def _parse(value: Any) -> tuple[JsonDict, str]:
    """The PROJJSON and ``srs`` of anything ``CRS()`` takes."""
    if isinstance(value, bool):
        raise CRSError(f"Invalid CRS input: {value!r}")
    if isinstance(value, int):
        return _epsg_json(value), f"EPSG:{value}"
    if isinstance(value, str):
        return _from_string(value), value
    d = as_dict(value)
    if d is not None:
        if "type" in d:
            return _check_projjson(d), json.dumps(d)
        raise not_yet("CRS from a dict of PROJ parameters", 3)
    # Another library's CRS object (pyproj's, rasterio's).
    to_json_dict = getattr(value, "to_json_dict", None)
    if callable(to_json_dict):
        return _check_projjson(to_json_dict()), str(value)
    if hasattr(value, "to_wkt"):
        raise not_yet("CRS from an object with to_wkt()", 2)
    if isinstance(value, (tuple, list)):
        pair = cast(Sequence[object], value)
        if len(pair) == 2:
            auth, code = str(pair[0]), str(pair[1])
            return _from_authority(auth, code), f"{auth}:{code}"
    raise CRSError(f"Invalid CRS input: {value!r}")
