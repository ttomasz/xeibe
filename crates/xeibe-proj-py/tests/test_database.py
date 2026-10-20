"""pyproj.database's queries over the EPSG tables."""

from __future__ import annotations

import pytest

from xeibe_proj import database
from xeibe_proj.aoi import AreaOfInterest
from xeibe_proj.enums import PJType

pyproj = pytest.importorskip("pyproj")


def test_utm_query_matches_pyproj() -> None:
    aoi = AreaOfInterest(21.0, 52.0, 21.1, 52.1)
    ours = database.query_utm_crs_info(datum_name="WGS 84", area_of_interest=aoi)
    theirs = pyproj.database.query_utm_crs_info(datum_name="WGS 84", area_of_interest=aoi)
    assert [i.code for i in ours] == [i.code for i in theirs]
    assert ours[0].projection_method_name == "Transverse Mercator"
    assert ours[0].type is PJType.PROJECTED_CRS


def test_query_crs_info() -> None:
    infos = database.query_crs_info(auth_name="EPSG", pj_types="GEOGRAPHIC_2D_CRS")
    assert "4326" in {i.code for i in infos}
    assert all(i.type is PJType.GEOGRAPHIC_2D_CRS and not i.deprecated for i in infos)
    assert database.query_crs_info(auth_name="ESRI") == []
    geodetic = {i.type for i in database.query_crs_info(pj_types=PJType.GEODETIC_CRS)}
    assert geodetic == {PJType.GEOGRAPHIC_2D_CRS, PJType.GEOGRAPHIC_3D_CRS, PJType.GEOCENTRIC_CRS}


def test_codes_and_metadata() -> None:
    assert "3857" in database.get_codes("EPSG", "PROJECTED_CRS")
    assert "CRS84" in database.get_codes("OGC", PJType.GEOGRAPHIC_2D_CRS)
    assert database.get_authorities() == ["EPSG", "OGC"]
    version = database.get_database_metadata("EPSG.VERSION")
    assert version is not None and version.startswith("v13.")
    assert database.get_database_metadata("PROJ.VERSION") is None
