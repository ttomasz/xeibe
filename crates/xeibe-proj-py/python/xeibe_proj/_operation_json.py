"""``Transformer.to_json``: the operation a transformer runs, in PROJ's
PROJJSON shapes (docs/pyproj-phase2.md, Transformer.to_json).

Each step of the chosen operation (``OperationInfo.chain``) becomes what
PROJ writes for it:

- a map projection is a ``Conversion``, its inverse "Inverse of …" with
  the method "Inverse of …" and the authority ``INVERSE(EPSG)``;
- an EPSG transformation is a ``Transformation`` with its CRSs, method,
  parameters, accuracy, usage, id and remarks. PROJ inverts the methods
  whose inverse is the same method with negated parameters (geocentric
  translations, Molodensky, offsets) that way, and the others as
  "Inverse of" the method;
- PROJ's null operations from a datum ensemble's CRS to a member's (its
  database's ``PROJ`` authority) and the operations between CRSs on one
  datum (null geographic offsets, geographic/geocentric conversions) are
  PROJ's own.

Several steps make a ``ConcatenatedOperation``, whose steps carry no usage.
Parameters are in their kind's SI unit (metre, unity) where PROJ keeps
EPSG's (arc-seconds, parts per million): angles in degrees, times in
years. The output is equivalent to PROJ's, not identical.
"""

from __future__ import annotations

import json
import math
from functools import cache
from typing import Any

from xeibe_proj import _epsg, _native
from xeibe_proj._json import JsonDict, as_dict, get_dict, get_dicts

__all__ = ["SCHEMA", "projection", "transformer_json"]

SCHEMA = "https://proj.org/schemas/v0.7/projjson.schema.json"
# EPSG's year, in seconds: what the time parameters were converted with.
_YEAR = 31556925.445
_INVERSE_OF = "Inverse of "
# PROJ's `inverseAsTransformation`: methods inverted by negating their
# parameters (the reciprocal for a change of vertical unit).
_NEGATED = {1031, 9603, 1035, 9604, 9605, 9601, 9619, 9660, 9618, 9656, 9616, 1069}
_CHANGE_OF_VERTICAL_UNIT = 1069
_LONGITUDE_ROTATION = 9601
# EPSG's accuracy for "unknown", which PROJ leaves out.
_UNKNOWN_ACCURACY = 999.0
_USAGE_KEYS = ("scope", "area", "bbox")
_WORLD: JsonDict = {
    "area": "World",
    "bbox": {"south_latitude": -90, "west_longitude": -180, "north_latitude": 90, "east_longitude": 180},
}

# A CRS of a step of `OperationInfo.chain`, and the step.
StepCrs = tuple[str, int, int | None]
Step = tuple[str, str, int | None, bool, StepCrs, StepCrs, float | None]


class _Registry:
    def __init__(self) -> None:
        doc = json.loads(_native.operations_json())
        self.ops: dict[int, JsonDict] = {o["code"]: o for o in doc["operations"]}
        self.extents: dict[int, list[Any]] = {e[0]: e for e in doc["extents"]}
        self.methods: dict[int, str] = {c: n for c, n in doc["methods"]}
        self.params: dict[int, tuple[str, str | None]] = {c: (n, k) for c, n, k in doc["parameters"]}
        self.projections: dict[int, int] = {crs: conv for crs, conv in doc["projections"]}


@cache
def _registry() -> _Registry:
    return _Registry()


def _unit(kind: str | None, value: float) -> tuple[object, float]:
    """A parameter's unit for its kind, and its value in that unit, from
    the SI value."""
    if kind == "length":
        return "metre", value
    if kind == "angle":
        return "degree", math.degrees(value)
    if kind == "scale":
        return "unity", value
    if kind == "time":
        return {"type": "TimeUnit", "name": "year", "conversion_factor": _YEAR}, value / _YEAR
    if kind == "length_per_time":
        return {"type": "LinearUnit", "name": "metres per second", "conversion_factor": 1.0}, value
    if kind == "angle_per_time":
        return {"type": "AngularUnit", "name": "radians per second", "conversion_factor": 1.0}, value
    if kind == "scale_per_time":
        return {"type": "ScaleUnit", "name": "unity per second", "conversion_factor": 1.0}, value
    return None, value


def _inverse_authority(authority: object) -> str:
    """PROJ's `addModifiedIdentifier`: INVERSE(X) for X, and back."""
    text = str(authority)
    if text.startswith("INVERSE(") and text.endswith(")"):
        return text[len("INVERSE(") : -1]
    return f"INVERSE({text})"


def _without_schema(crs: JsonDict) -> JsonDict:
    return {k: v for k, v in crs.items() if k != "$schema"}


def _crs(ref: StepCrs) -> JsonDict | None:
    """The PROJJSON of a step's CRS."""
    kind, code, vertical = ref
    if kind == "compound" and vertical is not None:
        h, v = _epsg.projjson(code), _epsg.projjson(vertical)
        if h is None or v is None:
            return None
        return {
            "type": "CompoundCRS",
            "name": f"{h.get('name')} + {v.get('name')}",
            "components": [_without_schema(h), _without_schema(v)],
        }
    crs = _epsg.projjson(code)
    if crs is None:
        return None
    if kind == "promoted":
        from xeibe_proj.crs._dimension import promote

        promoted, twin = promote(crs, None, None)
        if twin is not None:
            crs = _epsg.projjson(twin) or crs
        elif promoted is not None:
            crs = promoted
    return _without_schema(crs)


def _usage(extent_codes: list[int], scope: str | None) -> JsonDict:
    """The first usage's scope and area, as PROJ reports an operation's."""
    out: JsonDict = {}
    if scope:
        out["scope"] = scope
    extent = _registry().extents.get(extent_codes[0]) if extent_codes else None
    if extent is not None:
        _, name, south, west, north, east = extent
        out["area"] = name
        out["bbox"] = {"south_latitude": south, "west_longitude": west, "north_latitude": north, "east_longitude": east}
    return out


def _crs_area(crs: JsonDict) -> JsonDict:
    """A CRS's first area of use, as `area` and `bbox`."""
    usages = get_dicts(crs, "usages") or [crs]
    return {k: usages[0][k] for k in ("area", "bbox") if k in usages[0]}


def _parameters(op: JsonDict) -> list[JsonDict]:
    reg = _registry()
    out: list[JsonDict] = []
    for pcode, value in op.get("params", []):
        name, kind = reg.params.get(pcode, (str(pcode), None))
        param: JsonDict = {"name": name}
        if isinstance(value, str) or kind == "file":
            param["value"] = value
        elif value is not None:
            unit, shown = _unit(kind, float(value))
            param["value"] = shown
            if unit is not None:
                param["unit"] = unit
        param["id"] = {"authority": "EPSG", "code": pcode}
        out.append(param)
    return out


# PROJJSON's order of an operation's keys.
_ORDER = (
    "$schema", "type", "name", "source_crs", "target_crs", "method", "parameters",
    "accuracy", "scope", "area", "bbox", "id", "remarks",
)  # fmt: skip


def _ordered(op: JsonDict) -> JsonDict:
    out = {k: op[k] for k in _ORDER if k in op}
    out.update((k, v) for k, v in op.items() if k not in out)
    return out


def _inverted(op: JsonDict, name: str) -> JsonDict:
    """PROJ's inverse of a single operation: the CRSs swapped, an INVERSE
    id, and either negated parameters or the method's inverse."""
    out = dict(op)
    out["name"] = name
    if "source_crs" in op or "target_crs" in op:
        out["source_crs"], out["target_crs"] = op.get("target_crs"), op.get("source_crs")
    method = get_dict(op, "method")
    method_id = as_dict(method.get("id")) or {}
    if op.get("type") == "Transformation" and method_id.get("code") in _NEGATED:
        params: list[JsonDict] = []
        for p in get_dicts(op, "parameters"):
            value = p.get("value")
            if isinstance(value, (int, float)):
                if method_id.get("code") == _CHANGE_OF_VERTICAL_UNIT:
                    value = 0.0 if value == 0 else 1.0 / value
                elif value != 0:
                    # PROJ's `negate`: no negative zero.
                    value = -value
                p = {**p, "value": value}
            params.append(p)
        out["parameters"] = params
        if method_id.get("code") == _LONGITUDE_ROTATION:
            # PROJ's `createLongitudeRotation` knows it exact.
            out["accuracy"] = "0"
    else:
        method_name = str(method.get("name", ""))
        inverse_method: JsonDict = {
            **method,
            "name": method_name.removeprefix(_INVERSE_OF)
            if method_name.startswith(_INVERSE_OF)
            else _INVERSE_OF + method_name,
        }
        if method_id:
            inverse_method["id"] = {**method_id, "authority": _inverse_authority(method_id.get("authority"))}
        out["method"] = inverse_method
    op_id = as_dict(op.get("id"))
    if op_id is not None:
        out["id"] = {**op_id, "authority": _inverse_authority(op_id.get("authority"))}
    return _ordered(out)


def _epsg_operation(code: int) -> JsonDict | None:
    """One of EPSG's transformations (or conversions between datums)."""
    reg = _registry()
    op = reg.ops.get(code)
    if op is None:
        return None
    conversion = op.get("table") == "conversion"
    out: JsonDict = {"type": "Conversion" if conversion else "Transformation", "name": op["name"]}
    if not conversion:
        for key, crs in (("source_crs", op.get("src")), ("target_crs", op.get("dst"))):
            j = _epsg.projjson(crs) if isinstance(crs, int) else None
            if j is not None:
                out[key] = _without_schema(j)
    method: int = op["method"]
    out["method"] = {"name": reg.methods.get(method, str(method)), "id": {"authority": "EPSG", "code": method}}
    parameters = _parameters(op)
    if parameters:
        out["parameters"] = parameters
    if op.get("accuracy") is not None and float(op["accuracy"]) != _UNKNOWN_ACCURACY:
        # The text PROJ's database holds.
        out["accuracy"] = str(float(op["accuracy"]))
    out.update(_usage(op.get("extents", []), op.get("scope")))
    out["id"] = {"authority": "EPSG", "code": code}
    remarks = _native.operation_remarks(code)
    if remarks:
        out["remarks"] = remarks
    return out


def _zero(code: int, name: str, unit: str) -> JsonDict:
    return {"name": name, "value": 0, "unit": unit, "id": {"authority": "EPSG", "code": code}}


def _translations() -> list[JsonDict]:
    return [
        _zero(8605, "X-axis translation", "metre"),
        _zero(8606, "Y-axis translation", "metre"),
        _zero(8607, "Z-axis translation", "metre"),
    ]


def _ensemble_identity(method: int, source: JsonDict, target: JsonDict, accuracy: float | None) -> JsonDict:
    """A null operation of PROJ's database from a datum ensemble's CRS to
    a member's (`build_db.py`), with its code, remarks and area."""
    a, b = str(source.get("name", "")), str(target.get("name", ""))
    code = f"{a}_TO_{b}"
    if method == 9616:
        code = code.replace(" ", "_").replace("St.", "St").replace("(", "_").replace(")", "").replace("__", "_")
        method_name, params = "Vertical Offset", [_zero(8603, "Vertical Offset", "metre")]
    else:
        code = code.replace(" ", "").replace("(", "_").replace(")", "")
        method_name, params = "Geocentric translations (geog2D domain)", _translations()
    out: JsonDict = {
        "type": "Transformation",
        "name": f"{a} to {b}",
        "source_crs": source,
        "target_crs": target,
        "method": {"name": method_name, "id": {"authority": "EPSG", "code": method}},
        "parameters": params,
    }
    if accuracy is not None:
        out["accuracy"] = str(float(accuracy))
    # EPSG's scope 1024.
    out["scope"] = "Not known."
    out.update(_crs_area(target))
    out["id"] = {"authority": "PROJ", "code": code.upper()}
    if accuracy is not None:
        out["remarks"] = f"Accuracy {float(accuracy)} m, from datum ensemble definition"
    return out


def _horizontal(crs: JsonDict) -> JsonDict:
    if crs.get("type") == "CompoundCRS":
        components = get_dicts(crs, "components")
        if components:
            return _horizontal(components[0])
    if crs.get("type") == "BoundCRS":
        return _horizontal(get_dict(crs, "source_crs"))
    return crs


def projection(crs: JsonDict, inverse: bool) -> JsonDict | None:
    """A projected CRS's conversion, forwards or backwards, with the EPSG
    code an EPSG CRS's has."""
    h = _horizontal(crs)
    if h.get("type") != "ProjectedCRS":
        return None
    conversion = dict(get_dict(h, "conversion"))
    crs_id = as_dict(h.get("id"))
    if "id" not in conversion and crs_id is not None and crs_id.get("authority") == "EPSG":
        code = _registry().projections.get(int(crs_id.get("code", 0)))
        if code is not None:
            conversion["id"] = {"authority": "EPSG", "code": code}
    out: JsonDict = {"type": "Conversion", **conversion}
    if inverse:
        out["name"] = f"{_INVERSE_OF}{conversion.get('name', '')}"
        method = dict(get_dict(conversion, "method"))
        method["name"] = f"{_INVERSE_OF}{method.get('name', '')}"
        if "id" in method:
            method["id"] = {**get_dict(method, "id"), "authority": "INVERSE(EPSG)"}
        out["method"] = method
        if "id" in conversion:
            out["id"] = {**get_dict(conversion, "id"), "authority": "INVERSE(EPSG)"}
    return out


def _geodetic(crs: JsonDict) -> JsonDict:
    h = _horizontal(crs)
    return get_dict(h, "base_crs") if h.get("type") == "ProjectedCRS" else h


def _dimension_name(crs: JsonDict) -> str:
    if crs.get("type") == "GeodeticCRS":
        return "geocentric"
    axes = get_dicts(get_dict(crs, "coordinate_system"), "axis")
    return "geog3D" if len(axes) == 3 else "geog2D"


def _same_datum(a: JsonDict, b: JsonDict, name: str | None = None) -> JsonDict:
    """PROJ's operation between two geodetic CRSs on one datum, with the
    area it gives one: the CRSs' when they share it, else the world."""
    na, nb = _dimension_name(a), _dimension_name(b)
    an, bn = a.get("name", ""), b.get("name", "")
    if "geocentric" in (na, nb) and na != nb:
        return {
            "type": "Conversion",
            "name": name or f"Conversion from {an} ({na}) to {bn} ({nb})",
            "method": {"name": "Geographic/geocentric conversions", "id": {"authority": "EPSG", "code": 9602}},
        }
    if na == "geocentric":
        default = f"Null geocentric translation from {an} to {bn}"
        method: JsonDict = {"name": "Geocentric translations (geocentric domain)", "code": 1031}
        params = _translations()
        area = _WORLD
    else:
        default = (
            f"Null geographic offset from {an} to {bn}"
            if na == nb
            else f"Null geographic offset from {an} ({na}) to {bn} ({nb})"
        )
        params = [_zero(8601, "Latitude offset", "degree"), _zero(8602, "Longitude offset", "degree")]
        if "geog3D" in (na, nb):
            method = {"name": "Geographic3D offsets", "code": 9660}
            params.append(_zero(8603, "Vertical Offset", "metre"))
        else:
            method = {"name": "Geographic2D offsets", "code": 9619}
        area_a = _crs_area(a)
        area = area_a if area_a and area_a == _crs_area(b) else _WORLD
    return {
        "type": "Transformation",
        "name": name or default,
        "source_crs": a,
        "target_crs": b,
        "method": {"name": method["name"], "id": {"authority": "EPSG", "code": method["code"]}},
        "parameters": params,
        "accuracy": "0",
        **area,
    }


def _vertical_conversion(name: str, a: JsonDict, b: JsonDict) -> JsonDict:
    """Between vertical CRSs on one datum: height to depth, or a unit."""

    def direction(crs: JsonDict) -> object:
        axes = get_dicts(get_dict(crs, "coordinate_system"), "axis")
        return axes[0].get("direction") if axes else None

    if direction(a) != direction(b):
        method: JsonDict = {"name": "Height Depth Reversal", "id": {"authority": "EPSG", "code": 1068}}
    else:
        method = {"name": "Change of Vertical Unit", "id": {"authority": "EPSG", "code": 1104}}
    return {"type": "Conversion", "name": name, "method": method}


def _derived(op: JsonDict, source_ref: StepCrs, target_ref: StepCrs) -> JsonDict:
    """An EPSG operation run between other CRSs than its own (their 3D
    forms, say), as PROJ writes one: with those, and its id "derived from"
    the operation's."""
    own = [as_dict(op.get(k)) or {} for k in ("source_crs", "target_crs")]
    codes = [(as_dict(c.get("id")) or {}).get("code") for c in own]
    if codes == [source_ref[1], target_ref[1]] and source_ref[0] == target_ref[0] == "epsg":
        return op
    source, target = _crs(source_ref), _crs(target_ref)
    if source is None or target is None:
        return op
    out = {**op, "source_crs": source, "target_crs": target}
    op_id = as_dict(op.get("id"))
    if op_id is not None:
        out["id"] = {**op_id, "authority": f"DERIVED_FROM({op_id.get('authority')})"}
    return out


def _step(step: Step) -> JsonDict | None:
    """One step of the chain as PROJJSON, with its usage."""
    name, kind, code, inverse, source_ref, target_ref, accuracy = step
    if kind == "epsg" and code is not None:
        op = _epsg_operation(code)
        if op is None:
            return None
        if op.get("type") != "Transformation":
            return _inverted(op, name) if inverse else op
        return _derived(_inverted(op, name) if inverse else op, source_ref, target_ref)
    if kind == "projection" and code is not None:
        crs = _epsg.projjson(code)
        return None if crs is None else projection(crs, inverse)
    source, target = _crs(source_ref), _crs(target_ref)
    if source is None or target is None:
        return None
    if kind == "ensemble_identity" and code is not None:
        if inverse:
            return _inverted(_ensemble_identity(code, target, source, accuracy), name)
        return _ensemble_identity(code, source, target, accuracy)
    if kind == "same_datum":
        return _same_datum(source, target, name)
    if kind == "vertical_conversion":
        return _vertical_conversion(name, source, target)
    # Ours, without a counterpart in PROJ: null operations by their names.
    if kind == "ensemble_member":
        out = _same_datum(source, target, name)
        if out.get("type") == "Transformation":
            out["accuracy"] = None if accuracy is None else f"{accuracy:.15g}"
        return {k: v for k, v in out.items() if v is not None}
    return {
        "type": "Transformation",
        "name": name,
        "source_crs": source,
        "target_crs": target,
        "method": {"name": "Vertical Offset", "id": {"authority": "EPSG", "code": 9616}},
        "parameters": [_zero(8603, "Vertical Offset", "metre")],
    }


def _one_datum_steps(source: JsonDict, target: JsonDict) -> list[JsonDict]:
    """The steps on one datum, where there are no candidates: the inverse
    of the source's projection, PROJ's step between the geodetic CRSs where
    their dimension or kind changes (or where no projection says it all),
    and the target's projection."""
    steps: list[JsonDict] = []
    first, last = projection(source, True), projection(target, False)
    if first is not None:
        steps.append(first)
    a, b = _geodetic(source), _geodetic(target)
    if _dimension_name(a) != _dimension_name(b) or (first is None and last is None):
        steps.append(_same_datum(_without_schema(a), _without_schema(b)))
    if last is not None:
        steps.append(last)
    return steps


def transformer_json(source: JsonDict, target: JsonDict, infos: list[Any]) -> JsonDict | None:
    """The operation from ``source`` to ``target`` through the candidates
    ``infos`` (``OperationInfo``): ``None`` when there are several, as
    PROJ says nothing until it has chosen one."""
    if len(infos) > 1:
        return None
    if infos:
        # Without the identities a horizontal and a vertical operation run
        # together keep for the part they leave alone.
        chain = [s for s in infos[0].chain if not (s[1] == "same_datum" and s[4] == s[5])]
        steps = [s for s in (_step(step) for step in chain) if s is not None]
    else:
        steps = _one_datum_steps(source, target)
    if not steps:
        return None
    # PROJ's concatenations start and end in the CRSs asked for, whatever
    # their first and last steps' own (`createOperationsWithDatumPivot`).
    for i, key, crs in ((0, "source_crs", source), (-1, "target_crs", target)):
        if infos and key in steps[i]:
            steps[i] = {**steps[i], key: _without_schema(crs)}
    if len(steps) == 1:
        return {"$schema": SCHEMA, **steps[0]}
    out: JsonDict = {
        "$schema": SCHEMA,
        "type": "ConcatenatedOperation",
        "name": infos[0].name if infos else " + ".join(str(s.get("name", "")) for s in steps),
        "source_crs": _without_schema(source),
        "target_crs": _without_schema(target),
        "steps": [{k: v for k, v in s.items() if k not in _USAGE_KEYS} for s in steps],
    }
    if infos and infos[0].accuracy is not None:
        # PROJ's `toString` of a computed accuracy: 15 significant digits.
        out["accuracy"] = f"{infos[0].accuracy:.15g}"
    if infos and infos[0].area is not None:
        west, south, east, north = infos[0].area
        if infos[0].area_name:
            out["area"] = infos[0].area_name
        out["bbox"] = {"south_latitude": south, "west_longitude": west, "north_latitude": north, "east_longitude": east}
    if infos and infos[0].code is not None:
        code, inverse = infos[0].code
        out["id"] = {"authority": "INVERSE(EPSG)" if inverse else "EPSG", "code": code}
    return out
