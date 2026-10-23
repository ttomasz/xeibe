"""``to_2d``, ``to_3d`` and ``get_non_deprecated`` against pyproj
(docs/pyproj-phase2.md). Every EPSG CRS is compared on the oracle database
in test_oracle.py; these are the inputs without EPSG ids."""

from __future__ import annotations

from typing import Any

import pytest

from xeibe_proj import CRS

pyproj = pytest.importorskip("pyproj")

BESSEL_TOWGS84 = (
    'GEOGCS["X",DATUM["D",SPHEROID["Bessel 1841",6377397.155,299.1528128],'
    "TOWGS84[570.8,85.7,462.8,4.998,1.587,5.261,3.56]],"
    'PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]]'
)
GEOGRAPHIC_NO_ID = (
    'GEOGCRS["My WGS",DATUM["World Geodetic System 1984",ELLIPSOID["WGS 84",6378137,298.257223563]],'
    'CS[ellipsoidal,2],AXIS["latitude",north,ANGLEUNIT["degree",0.0174532925199433]],'
    'AXIS["longitude",east,ANGLEUNIT["degree",0.0174532925199433]]]'
)


def facts(crs: Any) -> tuple[object, ...]:
    # pyproj rebuilds the result from its WKT, which names WKT1's
    # "Latitude" axis "Geodetic latitude".
    return (
        crs.type_name,
        crs.name,
        crs.to_json_dict().get("id"),
        [(a.name.lower().removeprefix("geodetic "), a.abbrev.lower(), a.direction, a.unit_name) for a in crs.axis_info],
        crs.datum and crs.datum.name,
        crs.remarks,
    )


@pytest.mark.parametrize("text", [GEOGRAPHIC_NO_ID, BESSEL_TOWGS84, "EPSG:32633", "EPSG:4979", "EPSG:9707"])
@pytest.mark.parametrize("name", [None, "Renamed"])
def test_matches_pyproj(text: str, name: str | None) -> None:
    ours, theirs = CRS(text), pyproj.CRS(text)
    for method in ("to_3d", "to_2d"):
        a, b = getattr(ours, method)(name=name), getattr(theirs, method)(name=name)
        assert facts(a) == facts(b), method
        if b.is_bound:
            assert facts(a.source_crs) == facts(b.source_crs)
            assert facts(a.target_crs) == facts(b.target_crs)


def test_round_trips() -> None:
    assert CRS(2180).to_3d().to_2d() == CRS(2180)
    assert CRS(2180).to_3d().geodetic_crs.to_epsg() == 4937
    assert CRS(4326).to_3d().to_epsg() == 4979
    assert CRS(4979).to_2d().to_epsg() == 4326
    assert CRS(4326).to_3d(name="X").name == "WGS 84"
    assert CRS(4978).to_2d() == CRS(4978) and CRS(5773).to_3d() == CRS(5773)


def test_non_deprecated() -> None:
    assert [c.to_epsg() for c in CRS(4140).get_non_deprecated()] == [4617]
    assert CRS(4326).get_non_deprecated() == []
    assert CRS(GEOGRAPHIC_NO_ID).get_non_deprecated() == []
