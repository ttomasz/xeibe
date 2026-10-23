"""CRS input, properties and comparison, against pyproj where both agree.

pyproj 3.8.0 bundles EPSG 12.029 and xeibe-crs a newer release, so the
comparisons use CRSs whose definitions didn't change in between.
"""

from __future__ import annotations

import json
import pickle

import pytest

from xeibe_proj import CRS
from xeibe_proj.crs import AxisInfo
from xeibe_proj.exceptions import CRSError, ProjError

pyproj = pytest.importorskip("pyproj")

STABLE = [4326, 4258, 3857, 32633, 4978, 4979, 5773, 9707, 25832, 27700, 3035]


@pytest.mark.parametrize(
    ("value", "code"),
    [
        (4326, 4326),
        ("4326", 4326),
        ("EPSG:4326", 4326),
        ("epsg:3857", 3857),
        ("EPSG: 2180", 2180),
        (("EPSG", "2180"), 2180),
        ("urn:ogc:def:crs:EPSG::2180", 2180),
        ("urn:ogc:def:crs:EPSG:9.8.1:2180", 2180),
        ("http://www.opengis.net/def/crs/EPSG/0/2180", 2180),
        ("https://www.opengis.net/def/crs/EPSG/0/2180", 2180),
        ("http://www.opengis.net/gml/srs/epsg.xml#2180", 2180),
        ("EPSG:4326+5773", 9707),
        ("urn:ogc:def:crs,crs:EPSG::4258,crs:EPSG::7837", 9924),
        ("WGS 84", 4326),
        ("wgs 84 / pseudo-mercator", 3857),
    ],
)
def test_input_forms(value: object, code: int) -> None:
    assert CRS.from_user_input(value).to_epsg() == code


def test_ogc_crs() -> None:
    crs = CRS("OGC:CRS84")
    assert crs.to_epsg() is None
    assert crs.to_authority() == ("OGC", "CRS84")
    assert [a.abbrev for a in crs.axis_info] == ["Lon", "Lat"]
    assert crs != CRS(4326)
    assert crs.equals(CRS(4326), ignore_axis_order=True)
    assert CRS("urn:ogc:def:crs:OGC:1.3:CRS84") == crs
    assert CRS("http://www.opengis.net/def/crs/OGC/1.3/CRS84") == crs


def test_projjson_input() -> None:
    projjson = CRS(2180).to_json_dict()
    assert CRS(projjson) == CRS(2180)
    assert CRS(json.dumps(projjson)).to_epsg() == 2180
    assert CRS.from_json(json.dumps(projjson)).is_exact_same(CRS(2180))
    del projjson["id"]
    anonymous = CRS.from_json_dict(projjson)
    assert anonymous == CRS(2180)
    assert not anonymous.is_exact_same(CRS(2180))
    # Identified by its name and definition.
    assert anonymous.to_epsg() == 2180


@pytest.mark.parametrize(
    ("value", "error"),
    [
        ("EPSG:1", CRSError),
        ("0", CRSError),
        ("not a crs", CRSError),
        (None, CRSError),
        # Mollweide, which only Esri defines.
        ("ESRI:54009", NotImplementedError),
        ("+proj=longlat +datum=WGS84", NotImplementedError),
        ('GEOGCS["WGS 84",DATUM["WGS_1984"]]', CRSError),
        ({"proj": "longlat"}, NotImplementedError),
    ],
)
def test_bad_input(value: object, error: type[Exception]) -> None:
    with pytest.raises(error):
        CRS(value)


def test_crs_error_is_proj_error() -> None:
    assert issubclass(CRSError, ProjError)


@pytest.mark.parametrize("code", STABLE)
def test_properties_match_pyproj(code: int) -> None:
    ours, theirs = CRS(code), pyproj.CRS(code)
    assert ours.name == theirs.name
    assert ours.type_name == theirs.type_name
    for prop in (
        "is_geographic",
        "is_projected",
        "is_vertical",
        "is_geocentric",
        "is_compound",
        "is_derived",
        "is_bound",
        "is_engineering",
        "utm_zone",
    ):
        assert getattr(ours, prop) == getattr(theirs, prop), prop
    assert ours.to_epsg() == theirs.to_epsg()
    assert ours.to_authority() == theirs.to_authority()
    assert ours.to_string() == theirs.to_string()
    assert [str(a) for a in ours.axis_info] == [str(a) for a in theirs.axis_info]
    # PROJ leaves the unit code out on a compound CRS's vertical axis.
    if not theirs.is_compound:
        assert [repr(a) for a in ours.axis_info] == [repr(a) for a in theirs.axis_info]
    for a, b in zip(ours.axis_info, theirs.axis_info, strict=True):
        assert a.unit_conversion_factor == pytest.approx(b.unit_conversion_factor)
    assert ours.area_of_use is not None and theirs.area_of_use is not None
    # EPSG revises areas between releases by a few hundredths of a degree.
    assert ours.area_of_use.name == theirs.area_of_use.name
    assert ours.area_of_use.bounds == pytest.approx(theirs.area_of_use.bounds, abs=0.05)
    assert (ours.datum and ours.datum.name) == (theirs.datum and theirs.datum.name)
    assert (ours.datum and ours.datum.type_name) == (theirs.datum and theirs.datum.type_name)
    assert (ours.ellipsoid and ours.ellipsoid.name) == (theirs.ellipsoid and theirs.ellipsoid.name)
    assert str(ours.prime_meridian) == str(theirs.prime_meridian)
    assert (ours.geodetic_crs and ours.geodetic_crs.name) == (theirs.geodetic_crs and theirs.geodetic_crs.name)
    assert (ours.source_crs and ours.source_crs.name) == (theirs.source_crs and theirs.source_crs.name)
    assert [c.name for c in ours.sub_crs_list] == [c.name for c in theirs.sub_crs_list]
    if theirs.coordinate_system is not None:
        assert ours.coordinate_system is not None
        assert ours.coordinate_system.name == str(theirs.coordinate_system)


def test_ellipsoid_and_operation_match_pyproj() -> None:
    ours, theirs = CRS(32633), pyproj.CRS(32633)
    assert ours.ellipsoid is not None and theirs.ellipsoid is not None
    for attr in ("semi_major_metre", "semi_minor_metre", "inverse_flattening", "is_semi_minor_computed"):
        assert getattr(ours.ellipsoid, attr) == pytest.approx(getattr(theirs.ellipsoid, attr)), attr
    op, their_op = ours.coordinate_operation, theirs.coordinate_operation
    assert op is not None and their_op is not None
    for attr in ("name", "method_name", "method_auth_name", "method_code", "type_name"):
        assert getattr(op, attr) == getattr(their_op, attr), attr
    for p, q in zip(op.params, their_op.params, strict=True):
        assert (p.name, p.auth_name, p.code, p.unit_name, p.unit_code) == (
            q.name,
            q.auth_name,
            q.code,
            q.unit_name,
            q.unit_code,
        )
        assert p.value == pytest.approx(q.value)
        assert p.unit_conversion_factor == pytest.approx(q.unit_conversion_factor)


@pytest.mark.parametrize("code", [32633, 4326, 9707, 3035])
def test_repr_matches_pyproj(code: int) -> None:
    assert repr(CRS(code)) == repr(pyproj.CRS(code))


def test_equality_and_hash() -> None:
    assert CRS(4326) == CRS("EPSG:4326")
    assert CRS(4326) == "EPSG:4326"
    assert CRS(4326) == 4326
    assert CRS(4326) != CRS(4258)
    assert CRS(4326) != "garbage"
    assert hash(CRS(4326)) == hash(CRS("epsg:4326"))
    assert len({CRS(4326), CRS("EPSG:4326"), CRS(3857)}) == 2
    assert CRS("EPSG:4326+5773") == CRS(9707)


def test_pickle_keeps_srs() -> None:
    crs = CRS("urn:ogc:def:crs:EPSG::2180")
    copy = pickle.loads(pickle.dumps(crs))
    assert copy == crs and copy.srs == crs.srs and copy.is_exact_same(crs)


def test_srs_and_str() -> None:
    assert CRS(4326).srs == "EPSG:4326"
    assert str(CRS("epsg:4326")) == "epsg:4326"
    assert CRS(CRS(3857)).srs == "EPSG:3857"


def test_deprecated() -> None:
    assert not CRS(4326).is_deprecated
    assert CRS(2172).is_deprecated == pyproj.CRS(2172).is_deprecated


def test_not_yet() -> None:
    with pytest.raises(NotImplementedError, match="not supported"):
        CRS(4326).to_wkt("WKT1_ESRI")
    with pytest.raises(NotImplementedError, match="phase 3"):
        CRS(4326).to_proj4()


def test_axis_info_is_frozen() -> None:
    axis = CRS(4326).axis_info[0]
    assert isinstance(axis, AxisInfo)
    with pytest.raises(AttributeError):
        axis.name = "x"  # type: ignore[misc]


def test_every_crs_equals_itself_without_ids() -> None:
    """Equality compares definitions (Crs::equivalent), not identifiers."""
    from xeibe_proj import _epsg

    for code in _epsg.records():
        crs = CRS(code)
        bare = crs.to_json_dict()
        bare.pop("id", None)
        assert crs == CRS(bare), code
        assert hash(crs) == hash(CRS(bare)), code


def test_equality_tolerates_rounding() -> None:
    projjson = CRS(32633).to_json_dict()
    projjson["conversion"]["parameters"][3]["value"] *= 1 + 1e-12
    assert CRS(projjson) == CRS(32633)
    projjson["conversion"]["parameters"][3]["value"] *= 1 + 1e-6
    assert CRS(projjson) != CRS(32633)
