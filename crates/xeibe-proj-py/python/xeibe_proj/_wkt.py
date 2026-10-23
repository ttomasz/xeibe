"""Writing WKT, for ``CRS.to_wkt`` and the ``to_wkt`` of a CRS's parts.

WKT2_2019 (and ``WKT2_2018``, its old name in PROJ) and WKT1_GDAL are
written, in Rust (``docs/crs-wkt.md``, Writing); the other versions raise
``NotImplementedError``.
"""

from __future__ import annotations

import json
from typing import Any

from xeibe_proj import _native
from xeibe_proj._json import JsonDict
from xeibe_proj._unsupported import out
from xeibe_proj.enums import WktVersion

_WRITTEN = {
    WktVersion.WKT2_2019: "WKT2_2019",
    WktVersion.WKT2_2018: "WKT2_2019",
    WktVersion.WKT1_GDAL: "WKT1_GDAL",
}


def version_name(version: Any) -> str:
    """The version the writer takes for pyproj's ``version``: a
    ``ValueError`` for an unknown one, ``NotImplementedError`` for one that
    isn't written."""
    member = WktVersion.create(version)
    name = _WRITTEN.get(member)
    if name is None:
        raise out(f"to_wkt({member.value!r})", "only WKT2_2019 (or WKT2_2018) and WKT1_GDAL are written")
    return name


def write(projjson: JsonDict, version: Any, pretty: bool, output_axis_rule: bool | None) -> str:
    """The WKT of a CRS's or a part's PROJJSON; a ``CRSError`` when it
    can't be written in that version."""
    name = version_name(version)
    return _native.to_wkt(json.dumps(projjson), name, pretty, output_axis_rule)
