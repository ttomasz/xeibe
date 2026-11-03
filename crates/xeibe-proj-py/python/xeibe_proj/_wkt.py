"""Writing WKT, for ``CRS.to_wkt`` and the ``to_wkt`` of a CRS's parts.

WKT2_2019 (and ``WKT2_2018``, its old name in PROJ), WKT2_2015, their
simplified forms and WKT1_GDAL are written, in Rust (``docs/crs-wkt.md``,
Writing); WKT1_ESRI raises ``NotImplementedError``.
"""

from __future__ import annotations

from typing import Any

from xeibe_proj import _native
from xeibe_proj._json import JsonDict
from xeibe_proj._unsupported import out
from xeibe_proj.enums import WktVersion

_WRITTEN = {
    WktVersion.WKT2_2019: "WKT2_2019",
    WktVersion.WKT2_2018: "WKT2_2019",
    WktVersion.WKT2_2015: "WKT2_2015",
    WktVersion.WKT2_2019_SIMPLIFIED: "WKT2_2019_SIMPLIFIED",
    WktVersion.WKT2_2018_SIMPLIFIED: "WKT2_2019_SIMPLIFIED",
    WktVersion.WKT2_2015_SIMPLIFIED: "WKT2_2015_SIMPLIFIED",
    WktVersion.WKT1_GDAL: "WKT1_GDAL",
}


def version_name(version: Any) -> str:
    """The version the writer takes for pyproj's ``version``: a
    ``ValueError`` for an unknown one, ``NotImplementedError`` for one that
    isn't written."""
    member = WktVersion.create(version)
    name = _WRITTEN.get(member)
    if name is None:
        raise out(f"to_wkt({member.value!r})", "Esri's WKT1 isn't written: only WKT2 and WKT1_GDAL")
    return name


def write(projjson: JsonDict, version: Any, pretty: bool, output_axis_rule: bool | None) -> str:
    """The WKT of a CRS's or a part's PROJJSON; a ``CRSError`` when it
    can't be written in that version."""
    name = version_name(version)
    return _native.to_wkt(projjson, name, pretty, output_axis_rule)
