"""EPSG's ellipsoids, prime meridians, datums and coordinate operations on
their own, with pyproj's constructors, and ``query_geodetic_crs_from_datum``
(``docs/pyproj-phase3.md``, "The objects table"), against PROJ's results in
``tests/data/crs-objects`` (``scripts/gen_object_vectors.py``)."""

from __future__ import annotations

import json
import math
import re
from pathlib import Path
from typing import Any

import pytest

from xeibe_proj import CRS
from xeibe_proj.crs import CoordinateOperation, CoordinateSystem, Datum, Ellipsoid, PrimeMeridian
from xeibe_proj.database import query_geodetic_crs_from_datum
from xeibe_proj.enums import PJType
from xeibe_proj.exceptions import CRSError

VECTORS = Path(__file__).resolve().parents[3] / "tests/data/crs-objects"
CLASSES: dict[str, Any] = {
    "ellipsoid": Ellipsoid,
    "prime_meridian": PrimeMeridian,
    "datum": Datum,
    "conversion": CoordinateOperation,
}
# Where ours differ from PROJ's, why given: (kind, code, WKT version or
# None for the PROJJSON too).
DEVIATIONS: set[tuple[str, int, str | None]] = {
    # RH2000: EPSG types it a static vertical datum, and its own WKT is a
    # VDATUM without an epoch, but the record has a frame reference epoch,
    # from which PROJ makes it dynamic.
    ("datum", 5208, None),
    # Lambert Azimuthal Equal Area (Spherical) on its own in WKT1: PROJ
    # writes the ellipsoidal method's name with the spherical method's
    # parameter names, which nothing reads; ours keeps the spherical
    # method's name. Deprecated conversions no live CRS uses.
    ("conversion", 19867, "WKT1_GDAL"),
    ("conversion", 19868, "WKT1_GDAL"),
    ("conversion", 19965, "WKT1_GDAL"),
}
# PROJ's database holds at most this many parameters of a conversion: it
# drops Krovak Modified's evaluation point and C1 to C10, which ours keep.
PROJ_MAX_PARAMETERS = 7
# An engineering datum's id, which PROJ alone of its datums leaves out of
# the WKT it writes on its own, and ours writes.
ENGINEERING_IDS = {"WKT2_2019": ',ID["EPSG",{code}]]', "WKT1_GDAL": ',AUTHORITY["EPSG","{code}"]]'}
TOKEN = re.compile(r'"(?:[^"]|"")*"|[-+]?\d+(?:\.\d*)?(?:[eE][-+]?\d+)?|[^\s\d"]+')


def load(name: str) -> list[dict[str, Any]]:
    with (VECTORS / name).open() as f:
        return [json.loads(line) for line in list(f)[1:]]


def without_nested_ids(value: Any, root: bool = True) -> Any:
    """PROJJSON without ``$schema`` and the ids of the objects inside it,
    which PROJ leaves out and ours keep (an ensemble's members keep theirs)."""
    if isinstance(value, dict):
        return {
            k: without_nested_ids(v, k == "members")
            for k, v in value.items()
            if k != "$schema" and (root or k != "id")
        }
    if isinstance(value, list):
        return [without_nested_ids(v, root) for v in value]
    return value


def same(a: Any, b: Any) -> bool:
    """Equal JSON, numbers to 1e-12 relative."""
    if isinstance(a, dict) and isinstance(b, dict):
        return a.keys() == b.keys() and all(same(a[k], b[k]) for k in a)
    if isinstance(a, list) and isinstance(b, list):
        return len(a) == len(b) and all(same(x, y) for x, y in zip(a, b))
    if isinstance(a, (int, float)) and isinstance(b, (int, float)) and not isinstance(a, bool):
        return math.isclose(a, b, rel_tol=1e-12, abs_tol=1e-12)
    return bool(a == b)


def same_wkt(a: str | None, b: str | None) -> bool:
    """The same WKT tokens, numbers within 1e-11 relative, as the CRS
    tests compare (``tests/it/wkt.rs``)."""
    if a is None or b is None:
        return a is b
    ta, tb = TOKEN.findall(a), TOKEN.findall(b)
    if len(ta) != len(tb):
        return False
    for x, y in zip(ta, tb):
        if x == y:
            continue
        try:
            if not math.isclose(float(x), float(y), rel_tol=1e-11):
                return False
        except ValueError:
            return False
    return True


OBJECTS = load("objects.jsonl")


@pytest.mark.parametrize("r", OBJECTS, ids=lambda r: f"{r['kind']}-{r['code']}")
def test_object_matches_proj(r: dict[str, Any]) -> None:
    kind, code = r["kind"], r["code"]
    cls = CLASSES[kind]
    if (kind, code, None) in DEVIATIONS:
        return
    if r["projjson"]["name"].startswith("EPSG example"):
        # EPSG's worked examples are left out, as their CRSs are.
        with pytest.raises(CRSError, match="Invalid authority or code"):
            cls.from_epsg(code)
        return
    ours = cls.from_epsg(code)
    doc = without_nested_ids(ours.to_json_dict())
    if kind == "conversion" and "parameters" in doc:
        doc["parameters"] = doc["parameters"][:PROJ_MAX_PARAMETERS]
    assert same(doc, without_nested_ids(r["projjson"])), (doc, r["projjson"])
    for version, theirs in r["wkt"].items():
        if (kind, code, version) in DEVIATIONS or "Krovak Modified" in str(theirs):
            continue
        if r["projjson"]["type"] == "EngineeringDatum":
            theirs = theirs[:-1] + ENGINEERING_IDS[version].format(code=code)
        assert same_wkt(ours.to_wkt(version), theirs), (version, ours.to_wkt(version), theirs)
    # And back: the same object from its URN, its URL and its PROJJSON,
    # and the same WKT from its WKT.
    urn = {"ellipsoid": "ellipsoid", "prime_meridian": "meridian", "datum": "datum", "conversion": "coordinateOperation"}
    assert cls.from_string(f"urn:ogc:def:{urn[kind]}:EPSG::{code}") == ours
    assert cls.from_string(f"http://www.opengis.net/def/{urn[kind]}/EPSG/0/{code}") == ours
    wkt = ours.to_wkt()
    assert cls.from_string(wkt).to_wkt() == wkt
    assert cls.from_json(ours.to_json()) == ours
    assert cls.from_user_input(code) == ours


@pytest.mark.parametrize("r", load("names.jsonl"), ids=lambda r: f"{r['kind']}-{r['name']}")
def test_name_finds_what_proj_finds(r: dict[str, Any]) -> None:
    want = r["found"]
    # Esri's names of EPSG objects, and other authorities' objects: they
    # stay out, as other authorities' CRSs do.
    if r["source"].startswith("ESRI") or (isinstance(want, list) and want[0] != "EPSG"):
        return
    if isinstance(want, list) and want[1] == r["of"] and r["of"] == 1205:
        return  # EPSG's worked example
    cls = CLASSES[r["kind"]]
    try:
        doc = cls.from_name(r["name"]).to_json_dict()
    except CRSError:
        assert want is None
        return
    if want is None:
        # Beyond pyproj: a name that differs other than in ASCII case
        # (`rÉSEAU`), which PROJ alone ignores, or an engineering datum,
        # which pyproj never looks for.
        assert not r["name"].isascii() or doc["type"] == "EngineeringDatum", doc
        return
    if isinstance(want, dict):
        # pyproj's own ellipsoid from PROJ's +ellps table.
        assert same(without_nested_ids(doc), without_nested_ids(want["projjson"]))
    else:
        assert want == ["EPSG", doc["id"]["code"]]


@pytest.mark.parametrize("r", load("query.jsonl"), ids=lambda r: f"{r['datum']}-{r['crs_auth_name']}-{r['pj_type']}")
def test_query_geodetic_crs_from_datum_matches_proj(r: dict[str, Any]) -> None:
    found = query_geodetic_crs_from_datum(r["crs_auth_name"], "EPSG", str(r["datum"]), r["pj_type"])
    ours = [list(c.to_authority() or ("", "")) for c in found]
    # ESRI's and IGNF's CRSs stay out.
    theirs = [[a, str(c)] for a, c in r["crs"] if a in ("EPSG", "OGC")]
    assert ours == theirs


def test_constructors_as_pyproj_documents_them() -> None:
    wgs84 = Datum.from_epsg("6326")
    assert wgs84.name == "World Geodetic System 1984 ensemble"
    assert wgs84.type_name == "Datum Ensemble"
    assert wgs84 == 6326 and wgs84 != "invalid" and wgs84 == ("EPSG", 6326)
    assert Datum.from_name("WGS84") == wgs84 == Datum.from_string("World Geodetic System 1984")
    assert Datum.from_string("urn:ogc:def:ensemble:EPSG::6326") == wgs84
    assert Ellipsoid.from_epsg(7030) == CRS(4326).ellipsoid
    assert hash(Ellipsoid.from_epsg(7030)) == hash(CRS(4326).ellipsoid)
    assert Ellipsoid.from_name("intl").name == "International 1924 (Hayford 1909, 1910)"
    assert PrimeMeridian.from_authority("EPSG", 8903).name == "Paris"
    utm = CoordinateOperation.from_epsg(16031)
    assert (utm.method_auth_name, utm.method_code) == ("EPSG", "9807")
    # A conversion's area of use and scope, which PROJ's PROJJSON leaves out;
    # a projected CRS's conversion has them too.
    assert utm.area_of_use is not None and utm.area_of_use.bounds == (0.0, 0.0, 6.0, 84.0)
    assert utm.scope is not None and "scope" not in utm.to_json_dict()
    zone15 = CRS(26915).coordinate_operation
    assert zone15 is not None and zone15.area_of_use is not None
    assert zone15.area_of_use.bounds == (-96.0, 0.0, -90.0, 84.0)
    assert CoordinateOperation.from_name("UTM zone 12N").name == "UTM zone 12N"
    assert CoordinateOperation.from_string("urn:ogc:def:coordinateOperation:EPSG::1671").name == (
        "ETRS89-FRA [RGF93 v1] to WGS 84 (1)"
    )
    assert Datum.from_string('ENGINEERINGDATUM["Engineering datum"]').type_name == "Engineering Datum"
    msl = Datum.from_string('PDATUM["Mean Sea Level",ANCHOR["1013.25 hPa at 15°C"]]')
    assert msl.type_name == "Parametric Datum"
    assert msl.to_wkt() == 'PDATUM["Mean Sea Level",ANCHOR["1013.25 hPa at 15°C"]]'


def test_strings_name_objects_as_crs_strings_do() -> None:
    # An authority and code in any case, as a URN, its URL or AUTH:CODE.
    grs80 = Ellipsoid.from_epsg(7019)
    for text in (
        "EPSG:7019",
        "epsg:7019",
        "URN:OGC:DEF:ELLIPSOID:EPSG::7019",
        "https://www.opengis.net/def/ellipsoid/EPSG/0/7019",
        " urn:ogc:def:ellipsoid:EPSG:10.106:7019 ",
    ):
        assert Ellipsoid.from_string(text) == grs80, text
    assert Datum.from_user_input("EPSG:6258").name == "European Terrestrial Reference System 1989 ensemble"
    assert CoordinateOperation.from_string("urn:ogc:def:coordinateoperation:EPSG::16031").name == "UTM zone 31N"
    # A URN or URL of another type of object isn't one of these.
    with pytest.raises(CRSError, match="Invalid ellipsoid string"):
        Ellipsoid.from_string("http://www.opengis.net/def/datum/EPSG/0/6326")
    with pytest.raises(CRSError, match="Invalid datum string"):
        Datum.from_string("urn:ogc:def:ensemble:EPSG::6171")


def test_names_compare_beyond_ascii_case() -> None:
    # Case in any script, accents, EPSG's stray spaces and typographic
    # apostrophes, which nobody types.
    assert Datum.from_name("PANAMÁ-COLÓN 1911") == Datum.from_epsg(1072) == Datum.from_name("panama-colon  1911")
    assert Datum.from_name("Nivellement General de Futuna et d'Alofi 2022") == Datum.from_epsg(1455)
    assert Datum.from_name("Chart Datum Portugal").name == "Chart Datum Portugal "
    # A name equal ignoring case comes first: EPSG gives the accented name
    # to the three RGF93 frames, the unaccented one to v1 only.
    assert Datum.from_name("Réseau Géodésique Français 1993") == Datum.from_epsg(1312)
    assert Datum.from_name("Reseau Geodesique Francais 1993") == Datum.from_epsg(6171)
    # Operation and CRS names too.
    assert CoordinateOperation.from_name("UTM ZONE 31N") == CoordinateOperation.from_epsg(16031)
    assert CRS("panama-colon  1911").to_epsg() == 5467
    # EPSG's alias has an en dash.
    assert CRS("MGI 1901 / Gauss-Kruger zone 5") == CRS("MGI 1901 / Gauss–Kruger zone 5")


def test_datum_names_find_any_type() -> None:
    # Of datums whose names tie, the ensemble, as pyproj looks for one
    # first: ETRS89 is the ensemble's alias and IRENET95's.
    assert Datum.from_name("ETRS89") == Datum.from_epsg(6258)
    assert Datum.from_name("ETRS89", datum_type="GEODETIC_REFERENCE_FRAME") == Datum.from_epsg(6258)
    # Engineering datums, which pyproj doesn't look for.
    dabajuro = Datum.from_name("Dabajuro")
    assert dabajuro.type_name == "Engineering Datum"
    assert Datum.from_name("Dabajuro", datum_type="ENGINEERING_DATUM") == dabajuro
    assert dabajuro.to_wkt() == 'EDATUM["Dabajuro",ID["EPSG",9307]]'
    with pytest.raises(CRSError, match="Invalid datum name"):
        Datum.from_name("Dabajuro", datum_type="GEODETIC_REFERENCE_FRAME")
    with pytest.raises(CRSError, match="Invalid datum type"):
        Datum.from_name("Dabajuro", datum_type="NOT_A_TYPE")


def test_transformations_and_their_grids() -> None:
    op = CoordinateOperation.from_epsg(1312)
    assert op.type_name == "Transformation"
    assert [g.short_name for g in op.grids] == ["NTv1_0.gsb"]
    assert op.area_of_use is not None
    proj = CoordinateOperation.from_epsg(1312, True).grids[0]
    assert (proj.short_name, proj.url) == ("ca_nrc_ntv1_can.tif", "https://cdn.proj.org/ca_nrc_ntv1_can.tif")
    concatenated = CoordinateOperation.from_epsg(3896)
    assert concatenated.type_name == "Concatenated Operation"
    assert [s.name for s in concatenated.operations] == ["MGI (Ferro) to MGI (1)", "MGI to WGS 84 (3)"]
    assert CoordinateOperation.from_name("NAD27 to NAD83 (3)", coordinate_operation_type="TRANSFORMATION") == op


@pytest.mark.parametrize(
    "call, message",
    [
        (lambda: Datum.from_epsg(1), "Invalid authority or code"),
        (lambda: Ellipsoid.from_authority("BOB", 1), "Invalid authority or code"),
        (lambda: Datum.from_name("3-598y5-98y"), "Invalid datum name"),
        (lambda: Datum.from_name("WGS84", datum_type="VERTICAL_REFERENCE_FRAME"), "Invalid datum name: WGS84"),
        (lambda: Ellipsoid.from_name("intl", auth_name="ESRI"), "Invalid ellipsoid name"),
        (lambda: Ellipsoid.from_string("urn:ogc:def:datum:EPSG::6326"), "Invalid ellipsoid string"),
        (lambda: PrimeMeridian.from_string("3-598y5-98y"), "Invalid prime meridian string"),
        (lambda: CoordinateOperation.from_user_input({}), "Invalid coordinate operation"),
        (lambda: CoordinateSystem.from_string("urn:ogc:def:cs:EPSG::6422"), "Invalid coordinate system string"),
    ],
)
def test_errors_as_pyproj_words_them(call: Any, message: str) -> None:
    with pytest.raises(CRSError, match=message):
        call()


def test_query_raises_as_pyproj_does() -> None:
    with pytest.raises(ValueError, match="GEOCENTRIC_CRS"):
        query_geodetic_crs_from_datum("EPSG", "EPSG", "1116", PJType.PROJECTED_CRS)
    with pytest.raises(TypeError):
        query_geodetic_crs_from_datum("EPSG", None, "1116")  # type: ignore[arg-type]
    assert query_geodetic_crs_from_datum("foo-bar", "EPSG", "6269") == []
    assert [c.to_authority() for c in query_geodetic_crs_from_datum(None, "EPSG", "6269")] == [
        ("EPSG", "4269"),
        ("OGC", "CRS83"),
    ]
