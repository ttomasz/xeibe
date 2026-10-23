"""``CRS.to_3d`` and ``CRS.to_2d``: PROJ's ``promoteTo3D`` and
``demoteTo2D`` (``crs.cpp``), over PROJJSON (docs/pyproj-phase2.md).

A geographic CRS with an EPSG id becomes the EPSG CRS of the other
dimension with its name, when that one is on the same datum with the same
two horizontal axes, as EPSG names them (4326 and 4979). Otherwise:

- promoted, it gets an ellipsoidal height in metres, its areas of use (with
  the scope "unknown") and the remark "Promoted to 3D from EPSG:<code>",
  but no id;
- demoted, it keeps its name, datum and first two axes only.

A projected CRS takes its base along and keeps its conversion; a compound
CRS's 2D form is its first component; a Bound CRS's source changes, and its
hub with it when the transformation is a TOWGS84 one. Anything else stays.
"""

from __future__ import annotations

from typing import Any

from xeibe_proj import _epsg
from xeibe_proj._json import JsonDict, as_dict, get_dict, get_dicts, ident

__all__ = ["demote", "promote"]

HEIGHT: JsonDict = {"name": "Ellipsoidal height", "abbreviation": "h", "direction": "up", "unit": "metre"}
PRELUDE = "Promoted to 3D from "
_DEGREE = 0.0174532925199433
# The metadata keys a CRS carries besides its definition.
_EXTENT_KEYS = ("area", "bbox", "vertical_extent", "temporal_extent")


def _factor(unit: object) -> tuple[str, float]:
    """A unit as (kind, factor), from PROJJSON's names or objects."""
    if isinstance(unit, str):
        return {"degree": ("angle", _DEGREE), "metre": ("length", 1.0), "unity": ("scale", 1.0)}.get(
            unit, (unit, 1.0)
        )
    u = as_dict(unit) or {}
    return str(u.get("type", "")), float(u.get("conversion_factor", 1.0))


def _same_axis(a: JsonDict, b: JsonDict) -> bool:
    """PROJ's axis equivalence: the direction and the unit."""
    fa, fb = _factor(a.get("unit")), _factor(b.get("unit"))
    close = fa[1] == fb[1] or abs(fa[1] - fb[1]) <= 1e-10 * max(abs(fa[1]), abs(fb[1]))
    return str(a.get("direction", "")).lower() == str(b.get("direction", "")).lower() and close


def _axes(crs: JsonDict) -> list[JsonDict]:
    return get_dicts(get_dict(crs, "coordinate_system"), "axis")


def _epsg_code(crs: JsonDict) -> int | None:
    """The CRS's EPSG code, when it has exactly that one id (PROJ looks the
    twin up only then)."""
    if "ids" in crs:
        return None
    i = ident(crs)
    if i is None or i[0].upper() != "EPSG":
        return None
    try:
        return int(i[1])
    except ValueError:
        return None


def _twin(crs: JsonDict, kind: str) -> int | None:
    """The EPSG CRS of ``kind`` with the CRS's name, first of them, when it
    is on the same datum with the same two horizontal axes
    (``is2DPartOf3D``)."""
    code = _epsg_code(crs)
    if code is None:
        return None
    record = _epsg.record(code)
    candidates = _epsg.named(str(crs.get("name", "")), kind)
    if record is None or not candidates:
        return None
    first = candidates[0]
    twin = _epsg.projjson(first.code)
    if twin is None or first.datum is None or first.datum != record.datum:
        return None
    mine, theirs = _axes(crs), _axes(twin)
    if len(theirs) < 2 or not all(_same_axis(a, b) for a, b in zip(mine[:2], theirs[:2], strict=False)):
        return None
    if kind == "geographic_3d" and not (len(theirs) == 3 and _same_axis(theirs[2], HEIGHT)):
        return None
    return first.code


def _usages(crs: JsonDict) -> list[JsonDict]:
    """The CRS's usages, each as a dict."""
    if "usages" in crs:
        return get_dicts(crs, "usages")
    if any(k in crs for k in ("scope", *_EXTENT_KEYS)):
        return [crs]
    return []


def _promoted_properties(crs: JsonDict, name: str | None, remarks: str | None) -> JsonDict:
    """``createProperties``: the name, the areas without their scopes, and
    the remark saying where the CRS came from."""
    out: JsonDict = {"name": name or crs.get("name", "")}
    usages: list[JsonDict] = []
    for usage in _usages(crs):
        extent = {k: usage[k] for k in _EXTENT_KEYS if k in usage}
        if extent:
            usages.append({"scope": "unknown", **extent})
    if len(usages) == 1:
        out.update(usages[0])
    elif usages:
        out["usages"] = usages
    i = ident(crs) if "ids" not in crs else None
    if i is not None:
        text = f"{PRELUDE}{i[0]}:{i[1]}"
        out["remarks"] = f"{text}. {remarks}" if remarks else text
    elif remarks:
        out["remarks"] = remarks
    return out


def _as_base(crs: JsonDict) -> JsonDict:
    """A CRS as a projected CRS's base: its definition and id, without its
    usages and remarks."""
    return {k: v for k, v in crs.items() if k not in ("$schema", "usages", "scope", "remarks", *_EXTENT_KEYS)}


def promote(crs: JsonDict, name: str | None, remarks: Any) -> tuple[JsonDict | None, int | None]:
    """The 3D form: (PROJJSON, None), or (None, EPSG code) for an EPSG
    twin, or (None, None) when the CRS stays as it is."""
    kind = crs.get("type")
    axes = _axes(crs)
    if kind == "GeographicCRS" and len(axes) == 2:
        twin = _twin(crs, "geographic_3d")
        if twin is not None:
            return None, twin
        out: JsonDict = {"type": "GeographicCRS", **_promoted_properties(crs, name, remarks)}
        out = _definition_first(out, crs, axes + [HEIGHT])
        return out, None
    if kind == "ProjectedCRS" and len(axes) == 2:
        base = get_dict(crs, "base_crs")
        base3d, code = promote(base, None, None)
        if code is not None:
            twin_json = _epsg.projjson(code)
            base3d = _as_base(twin_json) if twin_json is not None else None
        elif base3d is not None:
            base3d = _as_base(base3d)
        out = {"type": "ProjectedCRS", **_promoted_properties(crs, name, remarks)}
        out["base_crs"] = base3d if base3d is not None else base
        out["conversion"] = crs.get("conversion")
        out["coordinate_system"] = {**get_dict(crs, "coordinate_system"), "axis": axes + [HEIGHT]}
        return _ordered(out), None
    if kind == "BoundCRS":
        source = get_dict(crs, "source_crs")
        source3d = _promoted_or_same(source, name)
        target = get_dict(crs, "target_crs")
        out = dict(crs)
        out["source_crs"] = source3d
        if _is_towgs84(crs):
            out["target_crs"] = _promoted_or_same(target, None)
        return out, None
    return None, None


def demote(crs: JsonDict, name: str | None) -> tuple[JsonDict | None, int | None]:
    """The 2D form, as :func:`promote` gives the 3D one."""
    kind = crs.get("type")
    axes = _axes(crs)
    if kind == "GeographicCRS" and len(axes) == 3:
        twin = _twin(crs, "geographic_2d")
        if twin is not None:
            return None, twin
        out: JsonDict = {"type": "GeographicCRS", "name": name or crs.get("name", "")}
        return _definition_first(out, crs, axes[:2]), None
    if kind == "ProjectedCRS" and len(axes) == 3:
        base = get_dict(crs, "base_crs")
        base2d = base
        if base.get("type") == "GeographicCRS":
            json2d, code = demote(base, None)
            twin_json = _epsg.projjson(code) if code is not None else None
            if twin_json is not None:
                base2d = _as_base(twin_json)
            elif json2d is not None:
                base2d = _as_base(json2d)
        out = {"type": "ProjectedCRS", "name": name or crs.get("name", ""), "base_crs": base2d}
        out["conversion"] = crs.get("conversion")
        out["coordinate_system"] = {**get_dict(crs, "coordinate_system"), "axis": axes[:2]}
        return out, None
    if kind == "BoundCRS":
        out = dict(crs)
        out["source_crs"] = _demoted_or_same(get_dict(crs, "source_crs"), name)
        if _is_towgs84(crs):
            out["target_crs"] = _demoted_or_same(get_dict(crs, "target_crs"), None)
        return out, None
    if kind == "CompoundCRS":
        components = get_dicts(crs, "components")
        if len(components) >= 2:
            # PROJ's component is the database's CRS, with its usages.
            code = _epsg_code(components[0])
            if code is not None and _epsg.projjson(code) is not None:
                return None, code
            return components[0], None
    return None, None


def _promoted_or_same(crs: JsonDict, name: str | None) -> JsonDict:
    json3d, code = promote(crs, name, None)
    if code is not None:
        return _epsg.projjson(code) or crs
    return json3d if json3d is not None else crs


def _demoted_or_same(crs: JsonDict, name: str | None) -> JsonDict:
    json2d, code = demote(crs, name)
    if code is not None:
        return _epsg.projjson(code) or crs
    return json2d if json2d is not None else crs


def _is_towgs84(bound: JsonDict) -> bool:
    """Whether the Bound CRS's transformation is one TOWGS84 can say: a
    Helmert transformation to WGS 84 (``getTOWGS84Parameters``)."""
    method = get_dict(get_dict(bound, "transformation"), "method")
    i = ident(method)
    # Geocentric translations, Position Vector and Coordinate Frame, in
    # their geog2D, geocentric and geog3D domains.
    return i is not None and i[0] == "EPSG" and i[1] in (
        "9603", "1031", "1035", "9606", "1033", "1037", "9607", "1032", "1038",
    )


def _definition_first(out: JsonDict, crs: JsonDict, axes: list[JsonDict]) -> JsonDict:
    """A geographic CRS: the definition, then the metadata already in
    ``out``, in PROJ's key order."""
    datum: JsonDict = {k: crs[k] for k in ("datum", "datum_ensemble") if k in crs}
    cs = {**get_dict(crs, "coordinate_system"), "axis": axes}
    head = {"type": out.pop("type"), "name": out.pop("name")}
    return {**head, **datum, "coordinate_system": cs, **out}


def _ordered(out: JsonDict) -> JsonDict:
    """A projected CRS's keys in PROJ's order."""
    order = ["type", "name", "base_crs", "conversion", "coordinate_system"]
    return {**{k: out[k] for k in order if k in out}, **{k: v for k, v in out.items() if k not in order}}
