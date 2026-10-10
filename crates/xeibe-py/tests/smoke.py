#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["pyarrow>=15"]  # xeibe returns arro3 objects; pyarrow checks they convert
# ///
"""Smoke test of the Python module `xeibe` on the PRG sample in tests/data.

Build the wheel, then run this script with it:

    uvx maturin build --release -m crates/xeibe-py/Cargo.toml -o target/py-wheels
    uv run --with target/py-wheels/xeibe_py-*.whl crates/xeibe-py/tests/smoke.py
"""

import json
import os
import sys
import tempfile
import warnings
from pathlib import Path

import arro3.core
import pyarrow as pa

try:
    import xeibe
except ImportError:
    sys.exit("xeibe is not installed: build the wheel and pass it with `uv run --with` (see the docstring)")

ROOT = Path(__file__).resolve().parents[3]
PRG = str(ROOT / "tests/data/samples/pl/prg-address-points.gml")
LAYER = "AD_PunktAdresowy"


def main() -> None:
    scan = xeibe.scan([PRG])
    assert scan.is_complete
    layers = {layer["name"]: layer for layer in scan.layers}
    assert set(layers) == {"AD_Miejscowosc", "AD_UlicaPlac", LAYER}, layers
    assert layers[LAYER]["feature_count"] == 2
    assert layers[LAYER]["geometry_columns"] == ["georeferencja"]
    schema = scan.schema(LAYER)
    assert isinstance(schema, arro3.core.Schema)
    assert pa.schema(schema).names == schema.names, "arro3 converts to pyarrow"
    assert "georeferencja" in scan.explain(LAYER)
    settings = os.path.join(tempfile.mkdtemp(), "prg.json")
    scan.save(settings)
    assert not xeibe.scan([PRG], sample=1).is_complete

    # Without a schema: sampled. The stream is one-shot.
    stream = xeibe.read([PRG], LAYER)
    assert isinstance(stream, arro3.core.RecordBatchReader)
    assert isinstance(stream.schema, arro3.core.Schema)
    table = pa.table(stream)
    assert table.num_rows == 2
    assert table.column("kodPocztowy").to_pylist() == ["68-213", "68-213"]
    geometry = table.schema.field("georeferencja")
    assert geometry.metadata[b"ARROW:extension:name"].startswith(b"geoarrow."), geometry.metadata
    try:
        pa.table(stream)
    except Exception as error:
        assert "closed" in str(error), error
    else:
        raise AssertionError("a consumed stream must not be read again")

    # An Arrow schema (arro3's or pyarrow's) as the schema, options as a dict.
    for given in (schema, pa.schema(schema)):
        table = pa.table(xeibe.read([PRG], LAYER, schema=given, options={"batch_size": 1}))
        assert table.num_rows == 2
        assert table.column_names == schema.names, "a given schema is used as it is"
    assert xeibe.read([PRG], LAYER, schema=schema).read_all().num_rows == 2, "arro3 alone reads it"

    # A settings file as the schema; RecordBatchReader interop.
    reader = pa.RecordBatchReader.from_stream(xeibe.read([PRG], LAYER, schema=settings))
    assert sum(batch.num_rows for batch in reader) == 2

    try:
        xeibe.read([PRG], "NoSuchLayer")
    except xeibe.UnknownLayerError as error:
        assert isinstance(error, xeibe.XeibeError)
        assert "NoSuchLayer" in str(error)
    else:
        raise AssertionError("an unknown layer is an error")

    # An input that isn't GML is an error.
    xsd = str(ROOT / "tests/data/samples/ch/AGR_SPB.xsd")
    for call in (lambda: xeibe.read([xsd], LAYER), lambda: xeibe.scan([xsd])):
        try:
            call()
        except xeibe.NoFeaturesError as error:
            assert isinstance(error, xeibe.XeibeError)
        else:
            raise AssertionError("an input without features is an error")
    # Among others, it is skipped with a warning; a warning filter can make
    # the warning an error.
    for given in (None, schema):
        with warnings.catch_warnings(record=True) as caught:
            warnings.simplefilter("always")
            assert pa.table(xeibe.read([PRG, xsd], LAYER, schema=given)).num_rows == 2
        assert [w.category for w in caught] == [xeibe.SkippedSourceWarning], caught
        assert "AGR_SPB.xsd" in str(caught[0].message)
    with warnings.catch_warnings():
        warnings.simplefilter("error", xeibe.SkippedSourceWarning)
        try:
            pa.table(xeibe.read([PRG, xsd], LAYER, schema=schema))
        except Exception as error:
            assert "AGR_SPB.xsd" in str(error), error
        else:
            raise AssertionError("an error filter makes a skipped source an error")

    # Settings without a file: a dict or JSON text, whole or one layer's
    # columns; options as JSON text.
    whole = Path(settings).read_text()
    columns = '{"code": {"type": "text", "path": "kodPocztowy"}, "geom": {"type": "geometry(Point)", "path": "georeferencja"}}'
    for given in (whole, json.loads(whole)):
        assert pa.table(xeibe.read([PRG], LAYER, schema=given)).column_names == schema.names
    table = pa.table(xeibe.read([PRG], LAYER, schema=columns))
    assert table.column_names == ["code", "geom"]
    assert table.column("code").to_pylist() == ["68-213", "68-213"]
    assert pa.table(xeibe.read([PRG], LAYER, schema=json.loads(columns))).num_rows == 2
    reader = xeibe.read([PRG], LAYER, schema=schema, options=json.dumps({"batch_size": 1}))
    assert [batch.num_rows for batch in pa.RecordBatchReader.from_stream(reader)] == [1, 1]

    # The public API is scan, read, Scan and the errors and warning.
    assert sorted(xeibe.__all__) == sorted(
        ["scan", "read", "Scan", "XeibeError", "UnknownLayerError", "NoFeaturesError", "SkippedSourceWarning"]
    ), xeibe.__all__

    # Internal helpers of xeibe.sedona (xeibe._xeibe, not public).
    from xeibe import _xeibe

    # read_or_empty: an input without features is an empty stream, with a warning.
    for sources, rows in (([xsd], 0), ([PRG, xsd], 2)):
        with warnings.catch_warnings(record=True) as caught:
            warnings.simplefilter("always")
            assert pa.table(_xeibe.read_or_empty(sources, LAYER, schema)).num_rows == rows
        assert [w.category for w in caught] == [xeibe.SkippedSourceWarning], caught

    # layer_schema: one layer's schema from settings.
    for given in (whole, json.loads(whole), settings):
        assert _xeibe.layer_schema(given, LAYER).names == schema.names
    assert _xeibe.layer_schema(whole, "NoSuchLayer") is None
    assert _xeibe.layer_schema(columns, LAYER).names == ["code", "geom"]

    # read_options: key=value pairs over settings or the defaults.
    options = _xeibe.read_options(None, {"preset": "strings", "batch_size": 1, "axis_order": "xy"})
    assert options["batch_size"] == 1 and options["geometry"]["axis"] == "XY", options
    assert _xeibe.read_options(whole)["geometry"] == json.loads(whole)["options"]["geometry"]
    table = pa.table(xeibe.read([PRG], LAYER, options=options))
    assert pa.types.is_string_view(table.schema.field("numerPorzadkowy").type), table.schema
    for pairs, message in (({"colour": "red"}, 'unknown option "colour"'), ({"threads": "many"}, "invalid threads")):
        try:
            _xeibe.read_options(None, pairs)
        except ValueError as error:
            assert message in str(error), error
        else:
            raise AssertionError(f"{pairs} must be rejected")

    # to_wkb: native geometry columns as geoarrow.wkb, CRS and path kept.
    native = _xeibe.layer_schema(columns, LAYER)
    assert native.field("geom").metadata[b"ARROW:extension:name"] == b"geoarrow.point"
    wkb = _xeibe.to_wkb(native)
    assert wkb.field("geom").metadata[b"ARROW:extension:name"] == b"geoarrow.wkb"
    assert wkb.field("geom").metadata[b"gml:path"] == b"georeferencja"
    assert wkb.field("code") == native.field("code")
    print("ok")


if __name__ == "__main__":
    main()
