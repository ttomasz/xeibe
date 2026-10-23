"""The EPSG tables from ``xeibe-crs``, read once and cached."""

from __future__ import annotations

import json
from functools import cache
from typing import Any, NamedTuple

from xeibe_proj import _native
from xeibe_proj._json import get_dicts, ident

JsonDict = dict[str, Any]


class Record(NamedTuple):
    """An EPSG CRS: what ``xeibe-crs`` knows without its PROJJSON."""

    code: int
    name: str
    kind: str
    dimension: int
    deprecated: bool
    area: tuple[float, float, float, float] | None
    # The datum or datum ensemble, through the base or horizontal CRS.
    datum: int | None


@cache
def records() -> dict[int, Record]:
    return {r[0]: Record(*r) for r in _native.crs_records()}


def record(code: int) -> Record | None:
    return records().get(code)


@cache
def _names() -> dict[str, int]:
    """Lowercased name to code: the first non-deprecated CRS of that name,
    else the first deprecated one (``"WGS 84"`` is EPSG:4326, not 4978)."""
    out: dict[str, int] = {}
    for r in sorted(records().values(), key=lambda r: (r.deprecated, r.code)):
        out.setdefault(r.name.lower(), r.code)
    return out


@cache
def _by_name_and_kind() -> dict[tuple[str, str], list[Record]]:
    """CRSs by lowercased name and kind, the current ones first."""
    out: dict[tuple[str, str], list[Record]] = {}
    for r in sorted(records().values(), key=lambda r: (r.deprecated, r.code)):
        out.setdefault((r.name.lower(), r.kind), []).append(r)
    return out


def named(name: str, kind: str) -> list[Record]:
    """The CRSs of this kind with this name, ignoring case, the current
    ones first."""
    return _by_name_and_kind().get((name.lower(), kind), [])


def code_for_name(name: str) -> int | None:
    """The code of the CRS with this name or EPSG alias, ignoring case."""
    key = name.strip().lower()
    code = _names().get(key)
    if code is None:
        code = _native.alias(key)
    return code


@cache
def _projjson(code: int) -> JsonDict | None:
    text = _native.projjson(code)
    return None if text is None else json.loads(text)


def projjson(code: int) -> JsonDict | None:
    """The PROJJSON of an EPSG CRS. Shared: never modify it."""
    return _projjson(code)


# OGC's longitude-first variants of three EPSG CRSs: code, EPSG twin, name.
OGC_CRS: dict[str, tuple[int, str]] = {
    "CRS84": (4326, "WGS 84 (CRS84)"),
    "CRS83": (4269, "NAD83 (CRS83)"),
    "CRS27": (4267, "NAD27 (CRS27)"),
}


@cache
def ogc_projjson(code: str) -> JsonDict | None:
    """The PROJJSON of ``OGC:<code>``: its EPSG twin with the axes swapped."""
    entry = OGC_CRS.get(code.upper())
    if entry is None:
        return None
    twin = projjson(entry[0])
    if twin is None:
        return None
    out: JsonDict = json.loads(json.dumps(twin))
    out["name"] = entry[1]
    out["coordinate_system"]["axis"].reverse()
    out["id"] = {"authority": "OGC", "code": code.upper()}
    return out


@cache
def compounds() -> dict[tuple[int, int], int]:
    """EPSG's compound CRSs by their two components' codes."""
    out: dict[tuple[int, int], int] = {}
    for r in records().values():
        if r.kind != "compound":
            continue
        j = projjson(r.code)
        if j is None:
            continue
        parts = [ident(c) for c in get_dicts(j, "components")]
        codes = [int(p[1]) for p in parts if p is not None and p[0] == "EPSG" and p[1].isdigit()]
        if len(parts) == 2 and len(codes) == 2:
            key = (codes[0], codes[1])
            # The first non-deprecated one, else the first.
            if key not in out or (records()[out[key]].deprecated and not r.deprecated):
                out[key] = r.code
    return out
