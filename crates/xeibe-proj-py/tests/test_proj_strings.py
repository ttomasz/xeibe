"""PROJ strings in ``xeibe_proj`` (docs/pyproj-phase3.md, "PROJ strings"):
pyproj's preparation of strings, dicts and keyword arguments, its warnings,
and PROJ's readings and strings from the committed vectors
(``scripts/gen_proj_string_vectors.py``). Every EPSG CRS and the whole
corpus are compared in ``xeibe-reproject``'s tests."""

from __future__ import annotations

import json
import warnings

import pytest

from xeibe_proj import CRS, Proj, Transformer
from xeibe_proj.exceptions import CRSError, ProjError

from .conftest import ROOT

CORPUS = ROOT / "tests/data/proj-string/corpus.jsonl"


def _corpus() -> list[dict[str, object]]:
    return [json.loads(line) for line in CORPUS.read_text().splitlines()[1:]]


def _same_tokens(a: str, b: str) -> bool:
    """The same PROJ string, numbers to 1e-13 (PROJ writes 15 digits)."""
    ta, tb = a.split(), b.split()
    if len(ta) != len(tb):
        return False
    for x, y in zip(ta, tb, strict=True):
        if x == y:
            continue
        kx, _, vx = x.partition("=")
        ky, _, vy = y.partition("=")
        if kx != ky:
            return False
        try:
            if abs(float(vx) - float(vy)) > 1e-13 * max(abs(float(vx)), abs(float(vy))):
                return False
        except ValueError:
            return False
    return True


def test_corpus_reads_identifies_and_writes_as_pyproj() -> None:
    """Each string pyproj reads: read here too, or refused with the reason
    (decision 6); the same ``to_epsg``; and, where the PROJJSON holds all of
    it, the same ``to_proj4``."""
    read = refused = 0
    for r in _corpus():
        srs = r["srs"]
        if not isinstance(srs, str) or r["projjson"] is None:
            continue
        with warnings.catch_warnings():
            warnings.simplefilter("ignore")
            try:
                crs = CRS(srs)
            except CRSError as e:
                assert "is not supported" in str(e), (srs, e)
                refused += 1
                continue
            read += 1
            assert crs.srs == srs
            assert crs.to_epsg() == r["to_epsg"], srs
            ours = None
            try:
                ours = crs.to_proj4()
            except CRSError:
                pass
            if r["to_proj4"] is not None and "+approx" not in srs:
                assert ours is not None and _same_tokens(ours, str(r["to_proj4"])), (srs, ours, r["to_proj4"])
    assert read > 150 and refused > 30, (read, refused)


def test_strings_dicts_and_keywords() -> None:
    utm = CRS("+proj=utm +zone=33 +datum=WGS84")
    assert utm.srs == "+proj=utm +zone=33 +datum=WGS84 +type=crs"
    assert utm.to_epsg() == 32633
    assert utm.is_projected and utm.name == "unknown"
    # pyproj's preparation: "+proj" first, spaces around "=", "+type=crs".
    assert CRS("+zone=33 +proj=utm +datum = WGS84").srs == "+proj=utm +zone=33 +datum=WGS84 +type=crs"
    assert CRS(proj="utm", zone=18, datum="NAD83").to_epsg() == 26918
    assert CRS({"proj": "longlat", "ellps": "WGS84", "datum": "WGS84", "no_defs": True}).to_epsg() == 4326
    assert CRS.from_dict({"proj": "utm", "zone": 30}).to_epsg() == 32630
    assert CRS.from_proj4("+proj=utm +zone=30 +south").to_epsg() == 32730
    # Keyword arguments add to a string given too, as in pyproj.
    assert CRS("+proj=utm +datum=WGS84", zone=34).to_epsg() == 32634
    with pytest.raises(CRSError, match="Invalid PROJ string"):
        CRS.from_proj4("EPSG:4326")
    # PROJ-only projections, pipelines and PROJ4 extensions are refused.
    for s in ("+proj=robin", "+proj=pipeline +step +proj=utm +zone=33", "+proj=longlat +datum=WGS84 +over"):
        with pytest.raises(CRSError, match="not supported"):
            CRS(s)


def test_init_and_its_warning() -> None:
    with pytest.warns(FutureWarning, match="'\\+init=<authority>:<code>' syntax is deprecated"):
        crs = CRS("+init=epsg:4326")
    # PROJ's PROJ.4 rules: EPSG's CRS, longitude first, and no EPSG code
    # for it any more.
    assert crs.name == "WGS 84"
    assert [a.abbrev for a in crs.axis_info] == ["lon", "lat"]
    assert crs.to_epsg() is None
    with pytest.warns(FutureWarning):
        assert CRS({"init": "epsg:26918"}).name == "NAD83 / UTM zone 18N"


def test_writing() -> None:
    with pytest.warns(UserWarning, match="You will likely lose important projection information"):
        text = CRS(2180).to_proj4()
    assert text == (
        "+proj=tmerc +lat_0=0 +lon_0=19 +k=0.9993 +x_0=500000 +y_0=-5300000 "
        "+ellps=GRS80 +units=m +no_defs +type=crs"
    )
    with warnings.catch_warnings():
        warnings.simplefilter("ignore", UserWarning)
        assert CRS(3857).to_proj4(4) == CRS(3857).to_proj4(5)
        assert CRS(4326).to_dict() == {"proj": "longlat", "datum": "WGS84", "no_defs": None, "type": "crs"}
        assert CRS("+proj=longlat +ellps=GRS80 +towgs84=1,2,3").to_dict()["towgs84"] == [1, 2, 3, 0, 0, 0, 0]
        with pytest.raises(ValueError, match="Invalid value"):
            CRS(4326).to_proj4(6)
        with pytest.raises(CRSError, match="cannot be converted to a PROJ string"):
            CRS(5800).to_proj4()  # an engineering CRS


def test_proj() -> None:
    p = Proj(proj="utm", zone=33, ellps="WGS84")
    # pyproj 3.8.0's results.
    assert p.definition_string() == "proj=utm zone=33 ellps=WGS84 units=m no_defs"
    assert p(15, 52) == pytest.approx((500000.0000000011, 5761038.212590415), abs=1e-6)
    assert Proj("EPSG:4326").definition_string() == "proj=longlat datum=WGS84 no_defs ellps=WGS84 towgs84=0,0,0"
    with pytest.warns(UserWarning):
        assert Proj(2180).to_latlong_def() == "+proj=longlat +ellps=GRS80 +no_defs +type=crs"
    t = Transformer.from_proj("EPSG:4326", "+proj=utm +zone=33 +datum=WGS84", always_xy=True)
    assert t.transform(15, 52) == pytest.approx((500000.0, 5761038.212590415), abs=1e-6)


def test_datums() -> None:
    # +nadgrids=@null leaves longitude and latitude as they are.
    nad83 = CRS("+proj=utm +zone=18 +datum=NAD83 +units=m +no_defs +nadgrids=null")
    assert nad83.is_bound
    x, y = Transformer.from_crs(nad83, 4326, always_xy=True).transform(500000, 4500000)
    assert (x, y) == pytest.approx((-75.0, 40.65085651660554), abs=1e-12)
    # An unknown datum converts on its own datum, and not to another.
    grs80 = CRS("+proj=longlat +ellps=GRS80")
    x, _ = Transformer.from_crs(grs80, "+proj=utm +zone=33 +ellps=GRS80", always_xy=True).transform(15, 52)
    assert x == pytest.approx(500000.0)
    with pytest.raises(ProjError, match="is unknown, so it can't be converted to another datum"):
        Transformer.from_crs(grs80, 2180)
