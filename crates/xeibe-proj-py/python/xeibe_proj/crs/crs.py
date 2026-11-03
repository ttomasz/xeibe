"""``CRS``: a coordinate reference system, kept as PROJJSON.

Input is an EPSG code in any of its usual spellings, one of OGC's three
longitude-first CRSs, an old Esri code that EPSG has since defined, WKT
(WKT1, ESRI's included, and WKT2), PROJJSON, or a PROJ string, dict or
keyword arguments for the CRSs PROJ reads into an EPSG method
(``docs/pyproj-phase3.md``, "PROJ strings"). WKT is written in WKT2_2019
and WKT1_GDAL, PROJ strings as PROJ writes them, and a CRS without an EPSG
id is identified by its definition, with PROJ's confidence levels
(``docs/crs-wkt.md``).
"""

from __future__ import annotations

import json
import math
import numbers
import re
import warnings
from functools import cached_property
from collections.abc import Sequence
from typing import Any, NamedTuple, Self, cast

from xeibe_proj import _epsg, _native, _wkt
from xeibe_proj._json import JsonDict, as_dict, as_list, dumps, get_dict, get_dicts, ident
from xeibe_proj._operation_json import SCHEMA
from xeibe_proj._unsupported import out
from xeibe_proj.aoi import AreaOfUse
from xeibe_proj.crs import _cf1x8, _dimension
from xeibe_proj.crs._objects import (
    AxisInfo,
    CoordinateOperation,
    CoordinateSystem,
    Datum,
    Ellipsoid,
    PrimeMeridian,
    nested_json,
    axis_from_json,
)
from xeibe_proj.crs.coordinate_system import Cartesian2DCS, Ellipsoidal2DCS, VerticalCS
from xeibe_proj.exceptions import CRSError
from xeibe_proj.geod import Geod

__all__ = [
    "CRS",
    "AuthorityMatchInfo",
    "BoundCRS",
    "CompoundCRS",
    "CustomConstructorCRS",
    "DerivedGeographicCRS",
    "GeocentricCRS",
    "GeographicCRS",
    "ProjectedCRS",
    "VerticalCRS",
]



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
# The CRS keywords of WKT1 (GDAL's and ESRI's) and WKT2: a string that
# starts with one is read as WKT, and the reader names what it doesn't read.
_WKT = re.compile(
    r"\s*(GEOGCS|PROJCS|GEOCCS|VERT_CS|VERTCS|COMPD_CS|LOCAL_CS|FITTED_CS|GEOGCRS|GEODCRS|"
    r"PROJCRS|VERTCRS|COMPOUNDCRS|ENGCRS|BOUNDCRS|GEODETICCRS|GEOGRAPHICCRS|PROJECTEDCRS|"
    r"VERTICALCRS|ENGINEERINGCRS|DERIVEDPROJCRS|DERIVEDPROJECTEDCRS|BASEGEOGCRS|TIMECRS|"
    r"TEMPORALCRS|PARAMETRICCRS)\s*\[",
    re.IGNORECASE,
)

# A PROJ string's `+key` or `+key=value`, as pyproj's `to_dict` reads it.
_PROJ_PARAM = re.compile(r"\+(?P<param>\w+)=?(?P<value>\S+)?\s*?")

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
    if auth == "ESRI" and code.isdigit():
        # Esri's old codes of CRSs EPSG has since defined: ESRI:102100 is
        # EPSG:3857, and reads as it.
        moved = _native.esri_code(int(code))
        if moved is not None:
            return _epsg_json(moved)
    if auth in _OTHER_AUTHORITIES or auth == "OGC":
        raise out(
            f"{auth}:{code}",
            "only EPSG codes, OGC:CRS84, CRS83 and CRS27, and Esri's old codes of EPSG CRSs are known",
        )
    raise CRSError(f"unknown authority {auth!r} in {auth}:{code}")


def _epsg_code(text: str) -> int:
    try:
        return int(text.strip())
    except ValueError:
        raise CRSError(f"not an EPSG code: {text!r}") from None


def _from_wkt(text: str) -> JsonDict:
    try:
        return _check_projjson(json.loads(_native.wkt_to_projjson(text)))
    except CRSError as e:
        raise CRSError(f"Invalid projection: {text}: {e}") from None


def _from_string(text: str) -> JsonDict:
    s = text.strip()
    if s.startswith("{"):
        try:
            projjson = json.loads(s)
        except json.JSONDecodeError as e:
            raise CRSError(f"invalid PROJJSON: {e}") from None
        return _check_projjson(projjson)
    if _is_proj_string(s):
        return _from_proj_string(_prepare_from_proj_string(s))
    if _WKT.match(s):
        return _from_wkt(text)
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
    # A bare number is an EPSG code, as in pyproj.
    if s.isdigit():
        return _epsg_json(int(s))
    code = _epsg.code_for_name(s)
    if code is not None:
        return _epsg_json(code)
    raise CRSError(f"Invalid projection: {text}: not an EPSG code, OGC CRS, WKT, PROJJSON or EPSG name")


# PROJ strings. `_prepare_from_dict` and `_prepare_from_proj_string` are
# pyproj 3.8.0's (MIT, see NOTICE.md): what a CRS from a dict or a PROJ
# string hands PROJ, and so the `srs`.


def _is_proj_string(text: str) -> bool:
    """pyproj's ``is_proj``: not WKT, with a ``=``; URLs and PROJJSON,
    which may have one too, aren't."""
    s = text.strip()
    return "=" in s and not _WKT.match(s) and not s.startswith(("{", "http://", "https://"))


def _prepare_from_dict(projparams: dict[str, Any], allow_json: bool = True) -> str:
    if "proj" not in projparams and "init" not in projparams and allow_json:
        return json.dumps(projparams)
    pjargs: list[str] = []
    for key, value in projparams.items():
        if isinstance(value, (list, tuple)):
            value = ",".join(str(v) for v in cast(Sequence[object], value))
        if value is None or str(value) == "True":
            pjargs.append(f"+{key}")
        elif str(value) == "False":
            pass
        else:
            pjargs.append(f"+{key}={value}")
    return _prepare_from_proj_string(" ".join(pjargs))


def _prepare_from_proj_string(in_crs_string: str) -> str:
    in_crs_string = re.sub(r"[\s+]?=[\s+]?", "=", in_crs_string.lstrip())
    starting_params = ("+init", "+proj", "init", "proj")
    if not in_crs_string.startswith(starting_params):
        kvpairs: list[str] = []
        first_item_inserted = False
        for kvpair in in_crs_string.split():
            if not first_item_inserted and kvpair.startswith(starting_params):
                kvpairs.insert(0, kvpair)
                first_item_inserted = True
            else:
                kvpairs.append(kvpair)
        in_crs_string = " ".join(kvpairs)
    if "type=crs" not in in_crs_string:
        in_crs_string += " +type=crs" if "+" in in_crs_string else " type=crs"
    in_crs_string = in_crs_string.replace("+init=EPSG", "+init=epsg").strip()
    if in_crs_string.startswith(("+init", "init")):
        warnings.warn(
            "'+init=<authority>:<code>' syntax is deprecated. "
            "'<authority>:<code>' is the preferred initialization method. "
            "When making the change, be mindful of axis order changes: "
            "https://pyproj4.github.io/pyproj/stable/gotchas.html"
            "#axis-order-changes-in-proj-6",
            FutureWarning,
            stacklevel=4,
        )
    return in_crs_string


def _from_proj_string(srs: str) -> JsonDict:
    try:
        return _check_projjson(json.loads(_native.proj_string_to_projjson(srs)))
    except CRSError as e:
        # PROJ's words for a pipeline, an operation rather than a CRS.
        what = "Input is not a CRS" if re.search(r"(^|\s)\+proj=pipeline\b", srs) else "Invalid projection"
        raise CRSError(f"{what}: {srs}: ({e})") from None


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

    ``CRS(x)`` takes what ``CRS.from_user_input`` takes, and PROJ
    parameters as keyword arguments (``CRS(proj="utm", zone=33)``), added
    to a PROJ string or dict given as ``x``, as in pyproj.
    """

    _projjson: JsonDict
    # The text the CRS was made from, or None for a dict (``srs``).
    _srs: str | None

    def __init__(self, projparams: Any = None, **kwargs: Any) -> None:
        if kwargs:
            base = "" if not projparams else _prepared(projparams)
            srs = " ".join((base, _prepare_from_dict(kwargs, allow_json=False)))
            srs = _prepare_from_proj_string(srs)
            self._projjson, self._srs = _from_proj_string(srs), srs
            return
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
        crs._srs = srs
        return crs

    @classmethod
    def _restore(cls, srs: str | None, projjson: str) -> Self:
        """Unpickle a CRS."""
        return cls._from_projjson(json.loads(projjson), srs)

    # Constructors ----------------------------------------------------------

    @classmethod
    def from_user_input(cls, value: Any, **kwargs: Any) -> Self:
        """A CRS from a ``CRS``, an EPSG code (``4326``, ``"EPSG:4326"``,
        ``"epsg:4326"``, ``"EPSG:4326+5773"``, an OGC URN or URL), an
        ``(authority, code)`` pair, ``"OGC:CRS84"`` (or CRS83, CRS27), an
        old Esri code of an EPSG CRS (``"ESRI:102100"``, read as
        EPSG:3857), an EPSG name or alias, WKT, PROJJSON as text or a dict,
        or another library's CRS object (its ``to_json_dict()`` or
        ``to_wkt()``)."""
        if isinstance(value, cls) and not kwargs:
            return value
        return cls(value, **kwargs)

    @classmethod
    def from_string(cls, in_crs_string: str) -> Self:
        """A CRS from text: see ``from_user_input``."""
        if not isinstance(in_crs_string, str):  # pyright: ignore[reportUnnecessaryIsInstance]
            raise CRSError("CRS input is not a string")
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
        """A CRS from WKT: WKT1 as GDAL and Esri write it, or WKT2 (2015 or
        2019)."""
        if not isinstance(in_wkt_string, str):  # pyright: ignore[reportUnnecessaryIsInstance]
            raise TypeError(f"WKT must be a string, not {type(in_wkt_string).__name__}")
        if not _WKT.match(in_wkt_string):
            raise CRSError(f"Invalid WKT string: {in_wkt_string}")
        return cls._from_projjson(_from_wkt(in_wkt_string), in_wkt_string)

    @classmethod
    def from_proj4(cls, in_proj_string: str) -> Self:
        """A CRS from a PROJ string (``"+proj=utm +zone=33 +datum=WGS84"``),
        for the CRSs PROJ reads into an EPSG method; ``+init=epsg:CODE``
        with pyproj's ``FutureWarning``."""
        if not _is_proj_string(in_proj_string):
            raise CRSError(f"Invalid PROJ string: {in_proj_string}")
        return cls.from_user_input(_prepare_from_proj_string(in_proj_string))

    @classmethod
    def from_dict(cls, proj_dict: dict[str, Any]) -> Self:
        """A CRS from a dict of PROJ parameters (``{"proj": "utm", "zone":
        33}``), or of PROJJSON."""
        return cls.from_user_input(_prepare_from_dict(proj_dict))

    @staticmethod
    def from_cf(
        in_cf: dict[str, Any], ellipsoidal_cs: Any = None, cartesian_cs: Any = None, vertical_cs: Any = None
    ) -> CRS:
        """A CRS from CF's grid mapping attributes (CF 1.8): its
        ``crs_wkt`` (or ``spatial_ref``), else ``grid_mapping_name`` and the
        parameters, datum and names, with these coordinate systems for the
        geographic, projected and vertical parts (``Ellipsoidal2DCS()``,
        ``Cartesian2DCS()`` and ``VerticalCS()`` by default). A height
        (``geopotential_datum_name``) and ``towgs84`` apply to a CRS of any
        grid mapping, where pyproj's leaves them off a geographic one."""
        from xeibe_proj.crs.coordinate_operation import ToWGS84Transformation

        for key in ("crs_wkt", "spatial_ref"):
            if key in in_cf:
                return CRS(in_cf[key])
        grid_mapping = in_cf.get("grid_mapping_name")
        if grid_mapping is None:
            raise CRSError("CF projection parameters missing 'grid_mapping_name'")
        datum = _cf1x8.horizontal_datum(in_cf)
        geographic_name = in_cf.get("geographic_crs_name")
        geographic: CRS
        if datum is not None:
            geographic = GeographicCRS(name=geographic_name or "undefined", datum=datum, ellipsoidal_cs=ellipsoidal_cs)
        elif geographic_name and geographic_name not in ("undefined", "unknown"):
            geographic = CRS(geographic_name)
            if ellipsoidal_cs is not None:
                doc = geographic.to_json_dict()
                doc["coordinate_system"] = CoordinateSystem.from_user_input(ellipsoidal_cs).to_json_dict()
                geographic = CRS(doc)
        else:
            geographic = GeographicCRS(ellipsoidal_cs=ellipsoidal_cs)
        horizontal: CRS
        if grid_mapping == "latitude_longitude":
            horizontal = geographic
        elif grid_mapping == "rotated_latitude_longitude":
            horizontal = DerivedGeographicCRS(
                base_crs=geographic, conversion=_cf1x8.pole_rotation(in_cf), ellipsoidal_cs=ellipsoidal_cs
            )
        else:
            horizontal = ProjectedCRS(
                name=in_cf.get("projected_crs_name", "undefined"),
                conversion=_cf1x8.conversion_from_cf(in_cf, grid_mapping),
                geodetic_crs=geographic,
                cartesian_cs=cartesian_cs,
            )
        if "towgs84" in in_cf:
            horizontal = BoundCRS(
                source_crs=horizontal,
                target_crs="WGS 84",
                transformation=ToWGS84Transformation(
                    horizontal.geodetic_crs, *_cf1x8.try_list_if_string(in_cf["towgs84"])
                ),
            )
        if "geopotential_datum_name" not in in_cf:
            return horizontal
        vertical = VerticalCRS(
            name="undefined",
            datum=in_cf["geopotential_datum_name"],
            geoid_model=in_cf.get("geoid_name"),
            vertical_cs=vertical_cs,
        )
        return CompoundCRS(name="undefined", components=[horizontal, vertical])

    # Identity ---------------------------------------------------------------

    @property
    def srs(self) -> str:
        """The input the CRS was made from: its text, or the PROJJSON of a
        dict as text."""
        return json.dumps(self._schema_first()) if self._srs is None else self._srs

    @property
    def name(self) -> str:
        """The CRS's name; a Bound CRS's is its source CRS's."""
        if self.is_bound:
            source = self.source_crs
            return "undefined" if source is None else source.name
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
    def _matches(self) -> list[AuthorityMatchInfo]:
        """The authority codes the CRS matches, best first, with PROJ's
        confidence (``Crs::identify`` in ``xeibe-reproject``). A CRS with
        an id that is its definition matches that id only. Without ids, the
        OGC CRS it is equivalent to is a match too: 100 with its name, else
        70, as in PROJ. A Bound CRS matches nothing."""
        ident = self._id()
        if ident is not None and ident[0] != "EPSG":
            return [AuthorityMatchInfo(ident[0], ident[1], 100)]
        parsed = self._parsed
        if parsed is None:
            return [] if ident is None else [AuthorityMatchInfo(ident[0], ident[1], 100)]
        found = [AuthorityMatchInfo("EPSG", str(code), confidence) for code, confidence in parsed.identify()]
        if ident is None and not self.is_bound:
            for code, (_, name) in _epsg.OGC_CRS.items():
                ogc = _epsg.ogc_projjson(code)
                if ogc is not None and parsed.equivalent(_native.Crs(ogc)):
                    found.append(AuthorityMatchInfo("OGC", code, 100 if self.name == name else 70))
        # Stable: EPSG first among equals.
        return sorted(found, key=lambda m: -m.confidence)

    def to_epsg(self, min_confidence: int = 70) -> int | None:
        """The best EPSG match's code at ``min_confidence`` or more, or
        ``None``."""
        ident = self.to_authority("EPSG", min_confidence)
        return None if ident is None else int(ident[1])

    def to_authority(self, auth_name: str | None = None, min_confidence: int = 70) -> tuple[str, str] | None:
        """The best match's ``(authority, code)`` at ``min_confidence`` or
        more, or ``None``."""
        matches = self.list_authority(auth_name, min_confidence)
        return (matches[0].auth_name, matches[0].code) if matches else None

    def list_authority(self, auth_name: str | None = None, min_confidence: int = 70) -> list[AuthorityMatchInfo]:
        """The authority codes the CRS matches at ``min_confidence`` or
        more, best first: EPSG's (and OGC's three CRSs), with PROJ's
        confidence levels (``docs/crs-wkt.md``, Identification)."""
        wanted = None if auth_name is None else auth_name.upper()
        return [
            m
            for m in self._matches
            if m.confidence >= min_confidence and (wanted is None or m.auth_name == wanted)
        ]

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

    def _source(self) -> CRS | None:
        """A Bound CRS's source CRS, which pyproj answers most questions
        about a Bound CRS with."""
        return self.source_crs if self.is_bound else None

    @property
    def is_geographic(self) -> bool:
        if (source := self._source()) is not None:
            return source.is_geographic
        if self.is_compound:
            return any(c.is_geographic for c in self._components()[:1])
        return self._kind() in ("GeographicCRS", "GeodeticCRS", "DerivedGeographicCRS") and self._cs_subtype() == "ellipsoidal"

    @property
    def is_projected(self) -> bool:
        if (source := self._source()) is not None:
            return source.is_projected
        if self.is_compound:
            return any(c.is_projected for c in self._components()[:1])
        return self._kind() in ("ProjectedCRS", "DerivedProjectedCRS")

    @property
    def is_vertical(self) -> bool:
        if (source := self._source()) is not None:
            return source.is_vertical
        if self.is_compound:
            return any(c.is_vertical for c in self._components())
        return self._kind() in ("VerticalCRS", "DerivedVerticalCRS")

    @property
    def is_geocentric(self) -> bool:
        if (source := self._source()) is not None:
            return source.is_geocentric
        return self._kind() in ("GeodeticCRS", "GeographicCRS") and self._cs_subtype() == "cartesian"

    @property
    def is_compound(self) -> bool:
        return self._kind() == "CompoundCRS"

    @property
    def is_engineering(self) -> bool:
        if (source := self._source()) is not None:
            return source.is_engineering
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
        if (source := self._source()) is not None:
            return source.utm_zone
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
        if (source := self._source()) is not None:
            return source._geodetic_json()
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
        if (source := self._source()) is not None:
            return source._datum_json()
        kind = self._kind()
        if kind in ("VerticalCRS", "EngineeringCRS", "TemporalCRS"):
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
        """A projected CRS's conversion, or a Bound CRS's transformation to
        its target CRS (with both CRSs, as pyproj gives it)."""
        # PROJ models EPSG's derived vertical CRSs as plain vertical ones.
        if self._kind() == "DerivedVerticalCRS":
            return None
        transformation = as_dict(self._projjson.get("transformation"))
        if self.is_bound and transformation is not None:
            return CoordinateOperation(
                {
                    "type": "Transformation",
                    "name": transformation.get("name", "unknown"),
                    "source_crs": self._projjson.get("source_crs"),
                    "target_crs": self._projjson.get("target_crs"),
                    **{k: v for k, v in transformation.items() if k not in ("type", "name")},
                }
            )
        conversion = as_dict(self._projjson.get("conversion"))
        if conversion is None:
            return None
        return CoordinateOperation._of_crs(conversion, self._epsg_code())  # pyright: ignore[reportPrivateUsage]

    @property
    def axis_info(self) -> list[AxisInfo]:
        # A new list each time, as pyproj gives: changing it leaves the CRS
        # alone. The axes are frozen, so they are shared.
        return list(self._axes)

    @cached_property
    def _axes(self) -> tuple[AxisInfo, ...]:
        """``axis_info``, made once: ``__hash__`` asks for it too."""
        if (source := self._source()) is not None:
            return source._axes
        if self.is_compound:
            return tuple(a for c in self._components() for a in c._axes)
        return tuple(axis_from_json(a) for a in self._cs_axes())

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
        # A derived geographic CRS (a rotated pole) is geodetic itself, as
        # PROJ has it; its datum is its base's (`_geodetic_json`).
        if self._kind() == "DerivedGeographicCRS":
            return CRS._from_projjson(self._projjson)
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
        """The PROJJSON as a dict (a copy), with ``$schema``."""
        out_: JsonDict = json.loads(json.dumps(self._schema_first()))
        return out_

    def to_json(self, pretty: bool = False, indentation: int = 2) -> str:
        """The PROJJSON as text, as PROJ writes it (``_json.dumps``)."""
        return dumps(self._schema_first(), pretty, indentation)

    def _schema_first(self) -> JsonDict:
        """The PROJJSON with ``$schema`` first, as PROJ writes it; a CRS
        made from a dict may have none."""
        if "$schema" in self._projjson:
            return self._projjson
        return {"$schema": SCHEMA, **self._projjson}

    def to_wkt(self, version: Any = "WKT2_2019", pretty: bool = False, output_axis_rule: bool | None = None) -> str:
        """The CRS as WKT, written as PROJ writes it: ``WKT2_2019`` (or
        ``WKT2_2018``, the same) or ``WKT1_GDAL``; other versions raise
        ``NotImplementedError``. ``pretty`` indents by 4 spaces per level.
        ``output_axis_rule`` writes every axis (``True``), none
        (``False``), or PROJ's choice for the version (``None``). A
        ``CRSError`` when the version can't express the CRS (a geographic
        3D CRS in WKT1_GDAL)."""
        key = (version, pretty, output_axis_rule)
        try:
            return self._wkts[key]
        except (KeyError, TypeError):
            pass
        name = _wkt.version_name(version)
        code = self._shared_code
        try:
            if code is None:
                wkt = _wkt.write(self._projjson, name, pretty, output_axis_rule)
            else:
                wkt = _epsg.wkt(code, name, pretty, output_axis_rule)
        except CRSError as e:
            raise CRSError(
                f"CRS cannot be converted to a WKT string of a '{version}' version. "
                f"Select a different version of a WKT string or edit your CRS. ({e})"
            ) from None
        try:
            self._wkts[key] = wkt
        except TypeError:
            pass
        return wkt

    @cached_property
    def _wkts(self) -> dict[tuple[Any, Any, Any], str]:
        """``to_wkt``'s results by its arguments: a CRS doesn't change, and
        the cache goes with it. Not by the CRS, whose ``__eq__`` and
        ``__hash__`` match CRSs whose WKT differs, as ``lru_cache`` would."""
        return {}

    def to_proj4(self, version: Any = 5) -> str:
        """The CRS as a PROJ string, as PROJ writes it, with pyproj's
        ``UserWarning`` that a PROJ string loses information. PROJ writes
        the same string for a CRS in either ``version``."""
        from xeibe_proj.enums import ProjVersion

        warnings.warn(
            "You will likely lose important projection information when "
            "converting to a PROJ string from another format. See: "
            "https://proj.org/faq.html#what-is-the-best-format-for-describing-"
            "coordinate-reference-systems",
            stacklevel=2,
        )
        ProjVersion.create(version)
        try:
            return _native.to_proj4(self._projjson)
        except CRSError:
            raise CRSError("CRS cannot be converted to a PROJ string.") from None

    def to_dict(self) -> dict[str, Any]:
        """The PROJ string's parameters, parsed as pyproj parses them."""

        def parse(value: str) -> Any:
            if value.lower() == "true":
                return True
            if value.lower() == "false":
                return False
            try:
                return int(value)
            except ValueError:
                pass
            try:
                return float(value)
            except ValueError:
                pass
            parts = value.split(",")
            return [float(p.strip()) for p in parts] if len(parts) > 1 else value

        out_: dict[str, Any] = {}
        for m in _PROJ_PARAM.finditer(self.to_proj4()):
            key, value = m.groups()
            parsed = None if value is None else parse(value)
            if parsed is not False:
                out_[key] = parsed
        return out_

    def to_cf(self, wkt_version: Any = "WKT2_2019", errcheck: bool = False) -> dict[str, Any]:
        """CF's grid mapping attributes (CF 1.8): ``crs_wkt``, the
        ellipsoid, prime meridian and names, and the grid mapping with its
        parameters where CF has one for the CRS's projection. With
        ``errcheck``, a CRS CF has no grid mapping for warns. Angles are in
        degrees, as CF has them (pyproj writes NTF's in grads), and a
        Lambert Conic Conformal (1SP) with a scale under 1 is the secant
        cone through two parallels it is (pyproj leaves the scale cf)."""
        cf: dict[str, Any] = {"crs_wkt": self.to_wkt(wkt_version)}
        operation = self.coordinate_operation
        source = self.source_crs
        if self.is_bound and operation is not None and operation.towgs84 and source is not None:
            sub = source.to_cf(wkt_version, errcheck)
            sub.pop("crs_wkt")
            return {**cf, **sub, "towgs84": operation.towgs84}
        if self.is_compound:
            for component in self.sub_crs_list:
                sub = component.to_cf(wkt_version, errcheck)
                sub.pop("crs_wkt")
                cf.update(sub)
            return cf
        if self.is_vertical:
            geoid = as_dict(self._projjson.get("geoid_model"))
            if geoid is not None:
                cf["geoid_name"] = geoid.get("name")
            if self.datum is not None:
                cf["geopotential_datum_name"] = self.datum.name
            if "geoid_name" in cf and "geopotential_datum_name" in cf:
                # Against CF 1.8, which allows one of the two, as pyproj
                # writes them: from_cf keeps both.
                warnings.warn(
                    "geoid_name and geopotential_datum_name are both written, which CF 1.8 doesn't allow, "
                    "so that from_cf keeps the geoid model and the datum",
                    stacklevel=2,
                )
            return cf
        ellipsoid = self.ellipsoid
        if ellipsoid is not None:
            cf["semi_major_axis"] = ellipsoid.semi_major_metre
            cf["semi_minor_axis"] = ellipsoid.semi_minor_metre
            cf["inverse_flattening"] = ellipsoid.inverse_flattening
            cf["reference_ellipsoid_name"] = ellipsoid.name
        meridian = self.prime_meridian
        if meridian is not None:
            # In degrees, as CF has it (Paris's is 2.5969213 grads).
            cf["longitude_of_prime_meridian"] = math.degrees(meridian.longitude * meridian.unit_conversion_factor)
            cf["prime_meridian_name"] = meridian.name
        geodetic = self.geodetic_crs
        if geodetic is not None:
            cf["geographic_crs_name"] = geodetic.name
            if geodetic.datum is not None:
                cf["horizontal_datum_name"] = geodetic.datum.name
        mapped: dict[str, Any] | None = None
        if self.is_geographic:
            if operation is None:
                return {**cf, "grid_mapping_name": "latitude_longitude"}
            mapped = _cf1x8.rotated_pole_to_cf(operation)
        elif self.is_projected and not self.is_bound and operation is not None:
            cf["projected_crs_name"] = self.name
            mapped = _cf1x8.conversion_to_cf(operation, ellipsoid)
        if mapped is None:
            if errcheck:
                what = f"Unsupported coordinate operation: {operation.method_name}" if operation else "Coordinate operation not found."
                warnings.warn(what, stacklevel=2)
            return {"crs_wkt": cf["crs_wkt"]}
        return {**cf, **mapped}

    def cs_to_cf(self) -> list[dict[str, Any]]:
        """CF's attributes of each of the CRS's axes, a compound CRS's
        components' in turn."""
        axes: list[dict[str, Any]] = []
        datum = self.datum
        if self.type_name == "Temporal CRS" and datum is not None:
            doc = datum.to_json_dict()
            origin = str(doc.get("time_origin", "1875-05-20")).strip().rstrip("zZ")
            if len(origin) == 4:
                origin = f"{origin}-01-01"
            temporal: dict[str, Any] = {
                "standard_name": "time",
                "long_name": "time",
                "calendar": str(doc.get("calendar", "proleptic_gregorian")).lower().replace(" ", "_"),
                "axis": "T",
            }
            unit = self.axis_info[0].unit_name.lower().replace("calendar", "").strip()
            if unit:
                temporal["units"] = f"{unit} since {origin}"
            axes.append(temporal)

        def rotated(crs: CRS) -> bool:
            operation = crs.coordinate_operation
            return operation is not None and _cf1x8.rotated_pole_to_cf(operation) is not None

        cs, source = self.coordinate_system, self.source_crs
        if cs is not None:
            axes.extend(cs.to_cf(rotated_pole=rotated(self)))
        elif self.is_bound and source is not None and source.coordinate_system is not None:
            axes.extend(source.coordinate_system.to_cf(rotated_pole=rotated(source)))
        else:
            for component in self.sub_crs_list:
                axes.extend(component.cs_to_cf())
        return axes

    def to_2d(self, name: str | None = None) -> CRS:
        """The CRS without a height, as PROJ's ``demoteTo2D``: EPSG's 2D
        twin of a geographic 3D CRS (4979 gives 4326), a projected CRS on
        its demoted base, a compound CRS's horizontal part. ``name`` names a
        new CRS, not an EPSG one."""
        json2d, code = _dimension.demote(self._projjson, name)
        if code is not None:
            return CRS.from_epsg(code)
        return CRS._from_projjson(self._projjson if json2d is None else json2d)

    def to_3d(self, name: str | None = None) -> CRS:
        """The CRS with an ellipsoidal height, as PROJ's ``promoteTo3D``:
        EPSG's 3D twin of a geographic 2D CRS (4326 gives 4979), else the CRS
        with a height axis in metres, without an id. ``name`` names a new
        CRS, not an EPSG one."""
        json3d, code = _dimension.promote(self._projjson, name, self.remarks)
        if code is not None:
            return CRS.from_epsg(code)
        return CRS._from_projjson(self._projjson if json3d is None else json3d)

    def get_geod(self) -> Geod | None:
        """The ``Geod`` of the CRS's ellipsoid, or ``None`` without one."""
        if self.ellipsoid is None:
            return None
        return Geod(
            a=self.ellipsoid.semi_major_metre,
            rf=self.ellipsoid.inverse_flattening,
            b=self.ellipsoid.semi_minor_metre,
        )

    def get_non_deprecated(self) -> list[CRS]:
        """The CRSs EPSG gives as replacements of this one, when it is a
        deprecated EPSG CRS; else none."""
        code = self._epsg_code()
        if code is None:
            return []
        return [CRS.from_epsg(c) for c in _native.replacements(code)]

    # Comparison ---------------------------------------------------------------

    @cached_property
    def _parsed(self) -> _native.Crs | None:
        """The CRS as ``xeibe-reproject`` reads it, or ``None`` when it
        can't: from ``_native_input``, or for an EPSG CRS the one every
        ``CRS`` of its code shares (``_epsg.parsed``)."""
        code = self._shared_code
        try:
            return _native.Crs(self._native_input()) if code is None else _epsg.parsed(code)
        except CRSError:
            return None

    def _native_input(self) -> str | JsonDict:
        """What ``xeibe-reproject`` reads the CRS from: its WKT or PROJ
        string when it was made from one (their axes are implicit, which
        identification compares leniently, as in PROJ), else its PROJJSON
        dict, which Rust reads as it is."""
        srs = self._srs
        if srs is not None and (_WKT.match(srs) or _is_proj_string(srs)):
            return srs
        return self._projjson

    @cached_property
    def _shared_code(self) -> int | None:
        """The code when the PROJJSON is an EPSG CRS's shared one
        (``_epsg.shared_code``), whose results are kept per code."""
        return _epsg.shared_code(self._projjson)

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
        return hash((self.type_name, len(self._axes)))

    def is_exact_same(self, other: Any) -> bool:
        """Whether both have the same PROJJSON, identifiers and names included."""
        try:
            other_crs = CRS.from_user_input(other)
        except (CRSError, NotImplementedError, TypeError, ValueError):
            return False
        return self._schema_first() == other_crs._schema_first()

    def __reduce__(self) -> tuple[Any, tuple[str | None, str]]:
        return CRS._restore, (self._srs, self.to_json())

    def __str__(self) -> str:
        return self.srs

    def __repr__(self) -> str:
        axes = "".join(f"- {a}\n" for a in self.axis_info) or "- undefined\n"
        cs = self.coordinate_system
        source = self._source()
        sub_crs = ""
        if cs is not None and cs.axis_list:
            cs_name = cs.name
        elif source is not None:
            cs_name = str(source.coordinate_system)
            sub_crs = f"Source CRS: {source.name}\n"
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


def _prepared(value: Any) -> str:
    """The text pyproj makes of ``CRS()``'s first argument before adding
    keyword arguments to it, without reading it."""
    if isinstance(value, str):
        return _prepare_from_proj_string(value) if _is_proj_string(value) else value
    d = as_dict(value)
    if d is not None and "type" not in d:
        return _prepare_from_dict(d)
    return _parse(value)[1]


def _parse(value: Any) -> tuple[JsonDict, str]:
    """The PROJJSON and ``srs`` of anything ``CRS()`` takes."""
    if isinstance(value, bool):
        raise CRSError(f"Invalid CRS input: {value!r}")
    if isinstance(value, numbers.Integral):
        # An int, or numpy's (an EPSG code read from an array).
        return _epsg_json(int(value)), f"EPSG:{int(value)}"
    if isinstance(value, str):
        if _is_proj_string(value):
            srs = _prepare_from_proj_string(value)
            return _from_proj_string(srs), srs
        return _from_string(value), value
    d = as_dict(value)
    if d is not None:
        if "type" in d:
            return _check_projjson(d), json.dumps(d)
        srs = _prepare_from_dict(d)
        if srs.startswith("{"):
            raise CRSError(f"Invalid CRS input: {value!r}")
        return _from_proj_string(srs), srs
    # Another library's CRS object (pyproj's, rasterio's).
    to_json_dict = getattr(value, "to_json_dict", None)
    if callable(to_json_dict):
        return _check_projjson(to_json_dict()), str(value)
    to_wkt = getattr(value, "to_wkt", None)
    if callable(to_wkt):
        text = str(to_wkt())
        return _from_wkt(text), text
    if isinstance(value, (tuple, list)):
        pair = cast(Sequence[object], value)
        if len(pair) == 2:
            auth, code = str(pair[0]), str(pair[1])
            return _from_authority(auth, code), f"{auth}:{code}"
    raise CRSError(f"Invalid CRS input: {value!r}")


# Builders: pyproj 3.8.0's classes and arguments (MIT, see NOTICE.md),
# each making PROJJSON for CRS.__init__.


def _nested_crs(value: Any) -> JsonDict:
    return nested_json(CRS.from_user_input(value))


class CustomConstructorCRS(CRS):
    """A CRS class whose constructor takes other arguments than ``CRS``'s.
    Its ``from_*`` methods make an instance of the class from what
    ``CRS``'s take, and raise ``CRSError`` for a CRS of another type."""

    # The `type_name`s of the CRSs the class holds: each subclass's own.
    _expected_types: tuple[str, ...] | None = None

    def _check_type(self) -> None:
        if self._expected_types is None:
            raise NotImplementedError(f"{type(self).__name__} gives no _expected_types")
        if self.type_name not in self._expected_types:
            raise CRSError(f"Invalid type {self.type_name}. Expected {self._expected_types}.")

    @classmethod
    def _from_projjson(cls, projjson: JsonDict, srs: str | None = None) -> Self:
        crs = super()._from_projjson(projjson, srs)
        crs._check_type()
        return crs

    @classmethod
    def from_user_input(cls, value: Any, **kwargs: Any) -> Self:
        if isinstance(value, cls):
            return value
        crs = cls.__new__(cls)
        CRS.__init__(crs, value, **kwargs)
        crs._check_type()
        return crs

    @classmethod
    def from_string(cls, in_crs_string: str) -> Self:
        return cls.from_user_input(in_crs_string)


class GeographicCRS(CustomConstructorCRS):
    """A geographic CRS of a datum and an ellipsoidal coordinate system."""

    _expected_types = ("Geographic CRS", "Geographic 2D CRS", "Geographic 3D CRS")

    def __init__(
        self,
        name: str = "undefined",
        datum: Any = "urn:ogc:def:ensemble:EPSG::6326",
        ellipsoidal_cs: Any | None = None,
    ) -> None:
        """
        Parameters
        ----------
        name: str, default="undefined"
            The CRS's name.
        datum: Any, default="urn:ogc:def:ensemble:EPSG::6326"
            What :meth:`Datum.from_user_input` takes, or a ``CustomDatum``.
        ellipsoidal_cs: Any, optional
            What :meth:`CoordinateSystem.from_user_input` takes;
            ``Ellipsoidal2DCS()`` by default.
        """
        d = Datum.from_user_input(datum)
        super().__init__(
            {
                "type": "GeographicCRS",
                "name": name,
                "datum_ensemble" if d.type_name == "Datum Ensemble" else "datum": nested_json(d),
                "coordinate_system": nested_json(CoordinateSystem.from_user_input(ellipsoidal_cs or Ellipsoidal2DCS())),
            }
        )


class DerivedGeographicCRS(CustomConstructorCRS):
    """A geographic CRS derived from another by a conversion (a rotated
    pole)."""

    _expected_types = ("Derived Geographic CRS", "Derived Geographic 2D CRS", "Derived Geographic 3D CRS")

    def __init__(self, base_crs: Any, conversion: Any, ellipsoidal_cs: Any | None = None, name: str = "undefined") -> None:
        """
        Parameters
        ----------
        base_crs: Any
            The geographic CRS it is derived from: what
            :meth:`CRS.from_user_input` takes.
        conversion: Any
            What :meth:`CoordinateOperation.from_user_input` takes.
        ellipsoidal_cs: Any, optional
            ``Ellipsoidal2DCS()`` by default.
        name: str, default="undefined"
            The CRS's name.
        """
        super().__init__(
            {
                "type": "DerivedGeographicCRS",
                "name": name,
                "base_crs": _nested_crs(base_crs),
                "conversion": nested_json(CoordinateOperation.from_user_input(conversion)),
                "coordinate_system": nested_json(CoordinateSystem.from_user_input(ellipsoidal_cs or Ellipsoidal2DCS())),
            }
        )


class GeocentricCRS(CustomConstructorCRS):
    """A geocentric CRS of a datum, in metres."""

    _expected_types = ("Geocentric CRS",)

    def __init__(self, name: str = "undefined", datum: Any = "urn:ogc:def:datum:EPSG::6326") -> None:
        """
        Parameters
        ----------
        name: str, default="undefined"
            The CRS's name.
        datum: Any, default="urn:ogc:def:datum:EPSG::6326"
            What :meth:`Datum.from_user_input` takes, or a ``CustomDatum``.
        """
        axes = [
            {"name": f"Geocentric {c}", "abbreviation": c, "direction": f"geocentric{c}", "unit": "metre"}
            for c in "XYZ"
        ]
        super().__init__(
            {
                "type": "GeodeticCRS",
                "name": name,
                "datum": nested_json(Datum.from_user_input(datum)),
                "coordinate_system": {"subtype": "Cartesian", "axis": axes},
            }
        )


class ProjectedCRS(CustomConstructorCRS):
    """A projected CRS: a geodetic CRS and a conversion."""

    _expected_types = ("Projected CRS", "Derived Projected CRS")

    def __init__(
        self,
        conversion: Any,
        name: str = "undefined",
        cartesian_cs: Any | None = None,
        geodetic_crs: Any | None = None,
    ) -> None:
        """
        Parameters
        ----------
        conversion: Any
            What :meth:`CoordinateOperation.from_user_input` takes, or a
            conversion of ``xeibe_proj.crs.coordinate_operation``.
        name: str, default="undefined"
            The CRS's name.
        cartesian_cs: Any, optional
            ``Cartesian2DCS()`` by default.
        geodetic_crs: Any, optional
            The base CRS; ``GeographicCRS()`` (WGS 84) by default.
        """
        super().__init__(
            {
                "type": "ProjectedCRS",
                "name": name,
                "base_crs": _nested_crs(geodetic_crs or GeographicCRS()),
                "conversion": nested_json(CoordinateOperation.from_user_input(conversion)),
                "coordinate_system": nested_json(CoordinateSystem.from_user_input(cartesian_cs or Cartesian2DCS())),
            }
        )


class VerticalCRS(CustomConstructorCRS):
    """A vertical CRS of a vertical datum, optionally with a geoid model."""

    _expected_types = ("Vertical CRS",)

    def __init__(self, name: str, datum: Any, vertical_cs: Any | None = None, geoid_model: str | None = None) -> None:
        """
        Parameters
        ----------
        name: str
            The CRS's name.
        datum: Any
            What :meth:`Datum.from_user_input` takes.
        vertical_cs: Any, optional
            ``VerticalCS()`` by default.
        geoid_model: str, optional
            The geoid model's name.
        """
        doc: JsonDict = {
            "type": "VerticalCRS",
            "name": name,
            "datum": nested_json(Datum.from_user_input(datum)),
            "coordinate_system": nested_json(CoordinateSystem.from_user_input(vertical_cs or VerticalCS())),
        }
        if geoid_model is not None:
            doc["geoid_model"] = {"name": geoid_model}
        super().__init__(doc)


class CompoundCRS(CustomConstructorCRS):
    """A compound CRS of horizontal and vertical CRSs."""

    _expected_types = ("Compound CRS",)

    def __init__(self, name: str, components: list[Any]) -> None:
        """
        Parameters
        ----------
        name: str
            The CRS's name.
        components: list[Any]
            The CRSs, each what :meth:`CRS.from_user_input` takes.
        """
        super().__init__({"type": "CompoundCRS", "name": name, "components": [_nested_crs(c) for c in components]})


class BoundCRS(CustomConstructorCRS):
    """A CRS with a transformation to another (``+towgs84``)."""

    _expected_types = ("Bound CRS",)

    def __init__(self, source_crs: Any, target_crs: Any, transformation: Any) -> None:
        """
        Parameters
        ----------
        source_crs: Any
            The CRS: what :meth:`CRS.from_user_input` takes.
        target_crs: Any
            The CRS its transformation goes to.
        transformation: Any
            What :meth:`CoordinateOperation.from_user_input` takes, or a
            ``ToWGS84Transformation``.
        """
        # PROJJSON's abridged transformation: the Bound CRS's are its CRSs.
        operation = nested_json(CoordinateOperation.from_user_input(transformation))
        abridged = {k: v for k, v in operation.items() if k not in ("source_crs", "target_crs")}
        super().__init__(
            {
                "type": "BoundCRS",
                "source_crs": _nested_crs(source_crs),
                "target_crs": _nested_crs(target_crs),
                "transformation": abridged,
            }
        )
