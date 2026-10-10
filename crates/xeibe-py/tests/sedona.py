#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["pyarrow>=15", "sedonadb>=0.4.1"]
# ///
"""Test of `xeibe.sedona`, GML as a SedonaDB data source, on samples in tests/data.

Build the wheel, then run this script with it:

    uvx maturin build --release -m crates/xeibe-py/Cargo.toml -o target/py-wheels
    uv run --with target/py-wheels/xeibe_py-*.whl crates/xeibe-py/tests/sedona.py
"""

import shutil
import sys
import tempfile
import warnings
from pathlib import Path

import pyarrow as pa
import sedonadb

try:
    import xeibe
    from xeibe.sedona import GmlFormat, read_gml
except ImportError:
    sys.exit("xeibe is not installed: build the wheel and pass it with `uv run --with` (see the docstring)")

ROOT = Path(__file__).resolve().parents[3]
SAMPLES = ROOT / "tests/data/samples"
PRG = str(SAMPLES / "pl/prg-address-points.gml")
POINTS = "AD_PunktAdresowy"
ADDRESSES = [SAMPLES / f"wfs/pl-gugik-mapserver-addresses-{v}.xml" for v in ("wfs100", "wfs110", "wfs200")]


def rows(df) -> list[dict]:
    # Through the Arrow stream: `to_arrow_table()` wants geoarrow-pyarrow.
    return pa.table(df).to_pylist()


def fails(call, *messages: str) -> None:
    try:
        call()
    except Exception as error:
        for message in messages:
            assert message in str(error), f"{message!r} not in {error}"
    else:
        raise AssertionError(f"expected an error with {messages}")


def copies(directory: Path, *files: Path | str) -> str:
    directory.mkdir()
    for index, file in enumerate(files):
        file = Path(file)
        shutil.copy(file, directory / f"{index}-{file.name}")
    return str(directory)


def main() -> None:
    sd = sedonadb.connect()
    tmp = Path(tempfile.mkdtemp())

    # One layer, geometry as WKB with the CRS, usable by ST_ functions.
    df = read_gml(sd, PRG, POINTS)
    geometry = df.schema.field("georeferencja")
    assert "Wkb(epsg:2180)" in str(geometry), geometry
    df.to_view("points")
    got = rows(sd.sql('select "kodPocztowy", ST_SRID(georeferencja) srid, ST_X(georeferencja) x from points'))
    assert got == [
        {"kodPocztowy": "68-213", "srid": 2180, "x": 222442.2},
        {"kodPocztowy": "68-213", "srid": 2180, "x": 220155.39},
    ], got
    assert df.count() == 2, "count(*): a scan without columns"
    # Columns in table order: SedonaDB 0.4.1 fails to plan a DataFrame
    # `select()` that reorders the columns of any Python data source.
    assert rows(df.select("numerPorzadkowy", "georeferencja").limit(1))[0]["numerPorzadkowy"] is not None

    # The layer: required where the input has several.
    fails(lambda: read_gml(sd, PRG), "pass the layer option", "AD_UlicaPlac", POINTS)
    fails(lambda: sd.read(PRG, format=GmlFormat()).count(), "pass the layer option")
    single = str(SAMPLES / "be/wallonia-lpis-ecological-focus-area-gml33.gml")
    assert read_gml(sd, single).count() == 3, "one layer: no need to name it"
    assert sd.read(single, format=GmlFormat()).count() == 3
    two_layers = copies(tmp / "two-layers", single, SAMPLES / "ch/geneva-agr-spb-arcbycenterpoint.gml")
    fails(
        lambda: sd.read(two_layers, format=GmlFormat()).count(),
        "gml:layer",
        "EcologicalFocusArea",
        "AGR_SPB",
    )

    # Several files: per file, or one schema across them (read_gml).
    same = copies(tmp / "same", PRG, PRG)
    assert sd.read(same, format=GmlFormat(), options={"layer": POINTS}).count() == 4
    assert read_gml(sd, f"{same}/*.gml", POINTS).count() == 4
    # A file without the layer adds no rows and no columns.
    mixed = copies(tmp / "mixed", PRG, single)
    assert sd.read(mixed, format=GmlFormat(), options={"layer": POINTS}).count() == 2
    # WFS 1.0, 1.1 and 2.0: the same layer, `@id` from different paths. Sampled
    # per file, SedonaDB can't merge them; read_gml's one schema can.
    addresses = copies(tmp / "addresses", *ADDRESSES)
    fails(
        lambda: sd.read(addresses, format=GmlFormat(), options={"layer": "AD.Address"}),
        "gml:path",
    )
    assert read_gml(sd, addresses, "AD.Address").count() == 6
    assert read_gml(sd, f"{addresses}/*.xml", "AD.Address").count() == 6

    # Every file is read; those that aren't GML are skipped, as xeibe skips
    # them. `extension` restricts the files SedonaDB lists.
    metadata = tmp / "metadata.xml"
    metadata.write_text('<gmd:MD_Metadata xmlns:gmd="http://www.isotc211.org/2005/gmd"/>')
    notes = tmp / "notes.txt"
    notes.write_text("not GML")
    others = (SAMPLES / "ch/AGR_SPB.xsd", metadata, notes)
    with_others = copies(tmp / "with-others", PRG, SAMPLES / "wfs/pl-gugik-mapserver-addresses-wfs200.xml", *others)
    for layer, count in ((POINTS, 2), ("AD.Address", 2)):
        for df in (
            sd.read(with_others, format=GmlFormat(), options={"layer": layer}),
            read_gml(sd, with_others, layer),
        ):
            with warnings.catch_warnings(record=True) as caught:
                warnings.simplefilter("always")
                assert df.count() == count, layer
            skipped = sorted(str(w.message).split("/")[-1].split(":")[0] for w in caught)
            assert all(w.category is xeibe.SkippedSourceWarning for w in caught), caught
            assert skipped == ["2-AGR_SPB.xsd", "3-metadata.xml", "4-notes.txt"], skipped
    with warnings.catch_warnings():
        warnings.simplefilter("error", xeibe.SkippedSourceWarning)
        fails(lambda: sd.read(with_others, format=GmlFormat(), options={"layer": POINTS}).count(), "SkippedSourceWarning")
    assert sd.read(with_others, format=GmlFormat(extension="gml"), options={"layer": POINTS}).count() == 2
    fails(
        lambda: sd.read(with_others, format=GmlFormat(extension="gml"), options={"layer": "AD.Address"}).count(),
        "no file holds the layer 'AD.Address'",
    )
    fails(lambda: sd.read(PRG, format=GmlFormat(), options={"layer": "NoSuchLayer"}).count(), "NoSuchLayer")
    single_with_others = copies(tmp / "single-with-others", single, *others)
    assert sd.read(single_with_others, format=GmlFormat()).count() == 3, "no layer: the GML file decides"

    # The schema as a pyarrow.Schema, a Scan, columns as JSON text, settings.
    scan = xeibe.scan([PRG])
    columns = '{"code": {"type": "text", "path": "kodPocztowy"}, "geom": {"type": "geometry", "path": "georeferencja"}}'
    settings = str(tmp / "prg.json")
    scan.save(settings)
    for schema in (scan.schema(POINTS), scan, columns):
        df = sd.read(PRG, format=GmlFormat(), options={"layer": POINTS, "schema": schema})
        assert df.count() == 2
    df = sd.read(PRG, format=GmlFormat(), options={"layer": POINTS, "schema": columns})
    assert df.schema.names == ["code", "geom"], df.schema
    assert "Wkb" in str(df.schema.field("geom")), "a given native type becomes WKB"
    df = read_gml(sd, PRG, POINTS, settings=settings)
    assert "Wkb" in str(df.schema.field("georeferencja")), "geometry(Point) in the settings becomes WKB"

    # Read options as key=value pairs; unknown ones are rejected when given.
    df = read_gml(sd, PRG, POINTS, options={"preset": "strings", "batch_size": 1})
    assert "utf8" in str(df.schema.field("numerPorzadkowy")), df.schema
    fails(lambda: GmlFormat().with_options({"colour": "red"}), 'unknown option "colour"')
    fails(lambda: GmlFormat().with_options({"preset": "fancy"}), "invalid preset")

    # Registered: `.gml` paths are read as GML.
    sd.register(GmlFormat(extension="gml"))
    assert sd.read(PRG, options={"layer": "AD_UlicaPlac"}).count() == 2
    print("ok")


if __name__ == "__main__":
    main()
