"""WKT in and out, identification and Bound CRSs (``docs/crs-wkt.md``),
against pyproj where both agree.

pyproj 3.8.0 bundles EPSG 12.029 and xeibe-crs a newer release, so the
comparisons use CRSs whose definitions didn't change in between.
``test_oracle.py`` compares on PROJ built from the same release.
"""

from __future__ import annotations

import pickle
import re

import pytest

from xeibe_proj import CRS, Transformer
from xeibe_proj.crs import AuthorityMatchInfo
from xeibe_proj.enums import WktVersion
from xeibe_proj.exceptions import CRSError

pyproj = pytest.importorskip("pyproj")

STABLE = [4326, 4258, 3857, 32633, 4978, 5773, 27700, 3035, 27572, 9707]
VERSIONS = ["WKT2_2019", "WKT1_GDAL"]
ID = re.compile(r',(ID|AUTHORITY)\[[^\[\]]*\]')

BESSEL = 'SPHEROID["Bessel 1841",6377397.155,299.1528128]'
BOUND_GEOG = (
    f'GEOGCS["x",DATUM["d",{BESSEL},TOWGS84[570.8,85.7,462.8,4.998,1.587,5.261,3.56]],'
    'PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]]'
)
BOUND_PROJ = (
    f'PROJCS["p",{BOUND_GEOG},PROJECTION["Transverse_Mercator"],PARAMETER["latitude_of_origin",0],'
    'PARAMETER["central_meridian",15],PARAMETER["scale_factor",0.9996],'
    'PARAMETER["false_easting",500000],PARAMETER["false_northing",0],UNIT["metre",1]]'
)


def assert_identified_alike(ours: CRS, theirs: object, code: int) -> None:
    """PROJ's EPSG code, or the CRS's own where PROJ finds none: WKT1
    can't say northing, easting, and its unstated axes are compared by
    unit only (``docs/crs-wkt.md``, Identification)."""
    expected = theirs.to_epsg()  # type: ignore[attr-defined]
    assert ours.to_epsg() in ((expected,) if expected is not None else (None, code))


def without_ids(wkt: str) -> str:
    """WKT without ids, which PROJ looks up in its database for a part
    written on its own and the PROJJSON doesn't carry."""
    while (stripped := ID.sub("", wkt)) != wkt:
        wkt = stripped
    return wkt


@pytest.mark.parametrize("version", VERSIONS)
@pytest.mark.parametrize("code", STABLE)
def test_wkt_reads_as_pyproj_reads_it(code: int, version: str) -> None:
    theirs = pyproj.CRS(code).to_wkt(version)
    ours = CRS(theirs)
    # WKT1 can't say northing, easting: such a CRS reads back with other
    # axes, in PROJ too.
    assert (ours == CRS(code)) == (pyproj.CRS(theirs) == pyproj.CRS(code))
    assert ours.srs == theirs
    assert ours.name == pyproj.CRS(theirs).name
    assert [str(a) for a in ours.axis_info] == [str(a) for a in pyproj.CRS(theirs).axis_info]
    assert_identified_alike(ours, pyproj.CRS(theirs), code)


@pytest.mark.parametrize("version", VERSIONS)
@pytest.mark.parametrize("code", STABLE)
def test_wkt_writes_as_pyproj_writes_it(code: int, version: str) -> None:
    ours, theirs = CRS(code), pyproj.CRS(code)
    for rule in (None, True, False):
        written = ours.to_wkt(version, output_axis_rule=rule)
        assert ("AXIS" in written) == ("AXIS" in theirs.to_wkt(version, output_axis_rule=rule))
        # WKT2 without its CS is what PROJ can't read back.
        if version == "WKT2_2019" and rule is False:
            assert "CS[" not in written
            continue
        # Usages and remarks come from each side's EPSG release.
        assert pyproj.CRS(written) == pyproj.CRS(theirs.to_wkt(version, output_axis_rule=rule))
    assert "\n" in ours.to_wkt(version, pretty=True)
    assert "\n" not in ours.to_wkt(version)


def test_wkt_versions() -> None:
    crs = CRS(4326)
    assert crs.to_wkt() == crs.to_wkt(WktVersion.WKT2_2019) == crs.to_wkt("wkt2_2018")
    with pytest.raises(ValueError, match="Invalid value"):
        crs.to_wkt("WKT_INVALID")
    for version in ("WKT2_2015", "WKT2_2019_SIMPLIFIED", "WKT1_ESRI"):
        with pytest.raises(NotImplementedError):
            crs.to_wkt(version)
    with pytest.raises(CRSError, match="CRS cannot be converted to a WKT string"):
        CRS(4979).to_wkt("WKT1_GDAL")


def test_from_wkt() -> None:
    wkt = CRS(2180).to_wkt()
    assert CRS.from_wkt(wkt) == CRS(2180)
    assert CRS.from_user_input(wkt).to_epsg() == 2180
    with pytest.raises(CRSError, match="Invalid WKT string"):
        CRS.from_wkt("EPSG:4326")
    with pytest.raises(TypeError):
        CRS.from_wkt(4326)  # type: ignore[arg-type]
    with pytest.raises(CRSError, match="Invalid projection"):
        CRS('GEOGCRS["x",DATUM["d"')


def test_object_with_to_wkt() -> None:
    """rasterio's and fiona's CRS objects: what their to_wkt() returns."""

    class Foreign:
        def to_wkt(self) -> str:
            return pyproj.CRS(32633).to_wkt("WKT1_GDAL")

    crs = CRS(Foreign())
    assert crs == CRS(32633)
    assert crs.to_epsg() == 32633


def test_pickle_keeps_the_wkt() -> None:
    wkt = pyproj.CRS(3035).to_wkt("WKT1_GDAL")
    crs = pickle.loads(pickle.dumps(CRS(wkt)))
    assert crs.srs == wkt
    assert crs.to_epsg() == CRS(wkt).to_epsg() == 3035


@pytest.mark.parametrize("version", VERSIONS)
@pytest.mark.parametrize("code", [4326, 32633, 27700, 4258, 3035, 27572, 3857])
def test_identification_matches_pyproj(code: int, version: str) -> None:
    """A CRS without ids is identified with PROJ's confidence levels. The
    full lists differ with the EPSG release (pyproj's has fewer deprecated
    CRSs); the Rust tests compare them on the same release."""
    wkt = without_ids(pyproj.CRS(code).to_wkt(version))
    ours, theirs = CRS(wkt), pyproj.CRS(wkt)
    for min_confidence in (0, 70, 100):
        assert ours.to_epsg(min_confidence) == theirs.to_epsg(min_confidence)
    assert ours.list_authority(min_confidence=100) == theirs.list_authority(min_confidence=100)
    assert ours.to_authority() == theirs.to_authority()
    assert ours.to_string() == theirs.to_string()


def test_ogc_crs_is_identified() -> None:
    lon_lat = CRS(4326).to_json_dict()
    lon_lat["coordinate_system"]["axis"].reverse()
    del lon_lat["id"]
    matches = CRS(lon_lat).list_authority()
    assert AuthorityMatchInfo("OGC", "CRS84", 70) in matches
    assert CRS("OGC:CRS84").list_authority() == [AuthorityMatchInfo("OGC", "CRS84", 100)]
    assert CRS(4326).list_authority() == [AuthorityMatchInfo("EPSG", "4326", 100)]


@pytest.mark.parametrize("version", VERSIONS)
@pytest.mark.parametrize("code", [4326, 4269, 27572, 32633, 5773])
def test_parts_write_as_pyproj_writes_them(code: int, version: str) -> None:
    ours, theirs = CRS(code), pyproj.CRS(code)
    for part in ("datum", "ellipsoid", "prime_meridian", "coordinate_system", "coordinate_operation"):
        mine, their = getattr(ours, part), getattr(theirs, part)
        assert (mine is None) == (their is None), part
        if mine is None:
            continue
        assert without_ids(mine.to_wkt(version) or "") == without_ids(their.to_wkt(version) or ""), part
    if ours.datum is not None:
        assert repr(ours.datum) == ours.datum.to_wkt(pretty=True)


def test_ellipsoid_and_prime_meridian_wkt() -> None:
    # pyproj's own tests: ids aside, the same text.
    ellipsoid = CRS(4326).ellipsoid
    assert ellipsoid is not None
    assert ellipsoid.to_wkt() == pyproj.CRS(4326).ellipsoid.to_wkt()
    meridian = CRS(4807).prime_meridian
    assert meridian is not None
    assert meridian.to_wkt() == pyproj.CRS(4807).prime_meridian.to_wkt()
    assert meridian.to_wkt("WKT1_GDAL") == 'PRIMEM["Paris",2.33722917,AUTHORITY["EPSG","8903"]]'
    cs = CRS(4326).coordinate_system
    assert cs is not None and cs.to_wkt("WKT1_GDAL") == ""


@pytest.mark.parametrize("wkt", [BOUND_GEOG, BOUND_PROJ])
def test_bound_crs_matches_pyproj(wkt: str) -> None:
    ours, theirs = CRS(wkt), pyproj.CRS(wkt)
    assert ours.type_name == theirs.type_name == "Bound CRS"
    for prop in (
        "name",
        "is_bound",
        "is_geographic",
        "is_projected",
        "is_geocentric",
        "is_vertical",
        "is_derived",
        "utm_zone",
        "area_of_use",
        "coordinate_system",
    ):
        assert getattr(ours, prop) == getattr(theirs, prop), prop
    assert [str(a) for a in ours.axis_info] == [str(a) for a in theirs.axis_info]
    assert str(ours.datum) == str(theirs.datum)
    assert str(ours.ellipsoid) == str(theirs.ellipsoid)
    assert ours.source_crs is not None and ours.target_crs is not None
    assert ours.source_crs.name == theirs.source_crs.name
    assert ours.target_crs.name == theirs.target_crs.name
    assert ours.target_crs.to_epsg() == 4326
    op, their_op = ours.coordinate_operation, theirs.coordinate_operation
    assert op is not None
    for attr in ("name", "type_name", "method_name", "method_code"):
        assert getattr(op, attr) == getattr(their_op, attr), attr
    assert op.towgs84 == pytest.approx(their_op.towgs84)
    assert repr(op) == repr(their_op)
    assert ours.to_epsg() is None and theirs.to_epsg() is None
    assert ours.list_authority() == theirs.list_authority() == []
    assert ours != ours.source_crs
    assert repr(ours) == repr(theirs)
    # PROJ keeps EPSG's ids on the units its WKT1 reader made, which
    # PROJJSON doesn't carry.
    for version in VERSIONS:
        assert without_ids(ours.to_wkt(version)) == without_ids(theirs.to_wkt(version))
    assert CRS(ours.to_wkt()) == ours


def test_bound_crs_transforms_as_pyproj_does() -> None:
    ours = Transformer.from_crs(CRS(BOUND_GEOG), 4326, always_xy=True)
    theirs = pyproj.Transformer.from_crs(pyproj.CRS(BOUND_GEOG), 4326, always_xy=True)
    x, y = ours.transform(14.5, 49.5)
    tx, ty = theirs.transform(14.5, 49.5)
    assert x == pytest.approx(tx, abs=1e-9) and y == pytest.approx(ty, abs=1e-9)


def test_towgs84_of_coordinate_frame_turns_the_rotations() -> None:
    crs = CRS(
        'BOUNDCRS[SOURCECRS[GEOGCRS["x",DATUM["d",ELLIPSOID["Bessel 1841",6377397.155,299.1528128]],'
        'CS[ellipsoidal,2],AXIS["lat",north],AXIS["lon",east],ANGLEUNIT["degree",0.0174532925199433]]],'
        'TARGETCRS[GEOGCRS["WGS 84",DATUM["World Geodetic System 1984",ELLIPSOID["WGS 84",6378137,298.257223563]],'
        'CS[ellipsoidal,2],AXIS["lat",north],AXIS["lon",east],ANGLEUNIT["degree",0.0174532925199433],'
        'ID["EPSG",4326]]],ABRIDGEDTRANSFORMATION["t",'
        'METHOD["Coordinate Frame rotation (geog2D domain)",ID["EPSG",9607]],'
        'PARAMETER["X-axis translation",1,ID["EPSG",8605]],PARAMETER["Y-axis translation",2,ID["EPSG",8606]],'
        'PARAMETER["Z-axis translation",3,ID["EPSG",8607]],PARAMETER["X-axis rotation",4,ID["EPSG",8608]],'
        'PARAMETER["Y-axis rotation",5,ID["EPSG",8609]],PARAMETER["Z-axis rotation",-0.0,ID["EPSG",8610]],'
        'PARAMETER["Scale difference",1.000007,ID["EPSG",8611]]]]'
    )
    op = crs.coordinate_operation
    assert op is not None
    assert op.towgs84 == pytest.approx([1, 2, 3, -4, -5, 0, 7])
    assert str(op.towgs84[5]) == "0.0"
    assert CRS(4326).coordinate_operation is None
    assert CRS(3004).coordinate_operation is not None
    assert CRS(3004).coordinate_operation.towgs84 == []  # type: ignore[union-attr]


def test_esri_old_codes() -> None:
    crs = CRS("ESRI:102100")
    assert crs == CRS(3857) == pyproj.CRS("ESRI:102100")
    assert crs.srs == "ESRI:102100"
    # The EPSG CRS it now is, where PROJ keeps Esri's own definition.
    assert crs.to_epsg() == 3857
    assert CRS.from_authority("ESRI", 102100) == crs
