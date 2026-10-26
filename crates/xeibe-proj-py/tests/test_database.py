"""pyproj.database's queries over the EPSG tables."""

from __future__ import annotations

import math

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


@pytest.mark.parametrize(
    "kwargs",
    [
        {},
        {"auth_name": "EPSG"},
        {"auth_name": "PROJ"},
        {"auth_name": ""},
        {"allow_deprecated": True},
        {"category": "linear"},
        {"category": "angular"},
        {"category": "scale"},
        {"category": "linear_per_time"},
        {"category": "angular_per_time"},
        {"category": "scale_per_time"},
        {"category": "time"},
    ],
    ids=repr,
)
def test_units_map_matches_pyproj(kwargs: dict[str, object]) -> None:
    """EPSG's units and PROJ's own, in PROJ's order, as pyproj lists them,
    without Esri's."""
    theirs = {k: v for k, v in pyproj.database.get_units_map(**kwargs).items() if v.auth_name != "ESRI"}
    ours = database.get_units_map(**kwargs)  # type: ignore[arg-type]
    # The factors within an ulp: macOS wheels' proj.db rounds some of them
    # the other way.
    assert [tuple(u._replace(conv_factor=0.0)) for u in ours.values()] == [
        tuple(u._replace(conv_factor=0.0)) for u in theirs.values()
    ]
    assert all(
        math.isclose(o.conv_factor, t.conv_factor, rel_tol=1e-15)
        for o, t in zip(ours.values(), theirs.values(), strict=True)
    )
    assert list(ours) == list(theirs)
    assert database.get_units_map(auth_name="ESRI") == {}
