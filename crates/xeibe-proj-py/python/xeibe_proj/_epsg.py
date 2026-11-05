"""The EPSG tables from ``xeibe-crs``, read once and cached."""

from __future__ import annotations

import json
from functools import cache, lru_cache
from typing import Any, NamedTuple

from xeibe_proj import _native
from xeibe_proj._json import ident

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


# The PROJJSON of each EPSG CRS read so far.
_shared: dict[int, JsonDict] = {}


def projjson(code: int) -> JsonDict | None:
    """The PROJJSON of an EPSG CRS. Shared: never modify it."""
    out = _shared.get(code)
    if out is None:
        text = _native.projjson(code)
        if text is None:
            return None
        # The first one read stays, should two threads read it at once.
        out = _shared.setdefault(code, json.loads(text))
    return out


def shared_code(projjson: JsonDict) -> int | None:
    """The code of the EPSG CRS whose shared PROJJSON this is: that very
    dict, not an equal one, so that Rust can read the CRS from its own
    text, which the dict was read from and still holds. ``None`` for any
    other dict, even an unchanged copy."""
    i = ident(projjson)
    if i is None or i[0] != "EPSG" or not i[1].isdigit():
        return None
    code = int(i[1])
    return code if _shared.get(code) is projjson else None


# A few KB each, kept for the codes used most recently.
@lru_cache(maxsize=256)
def wkt(code: int, version: str, pretty: bool, axes: bool | None) -> str:
    """``_native.to_wkt`` of an EPSG CRS: the same for every ``CRS`` of
    the code, which ``CRS.to_wkt`` keeps per instance."""
    return _native.to_wkt(_shared[code], version, pretty, axes)


@lru_cache(maxsize=256)
def parsed(code: int) -> _native.Crs:
    """The EPSG CRS as ``xeibe-reproject`` reads it, shared by every
    ``CRS`` of the code (``_native.Crs`` is immutable)."""
    return _native.Crs(_shared[code])


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

