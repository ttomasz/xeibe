#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["pyarrow>=15"]
# ///
"""Smoke test of the Python module `xeibe` on the PRG sample in tests/data.

Build the wheel, then run this script with it:

    uvx maturin build --release -m crates/xeibe-py/Cargo.toml -o target/py-wheels
    uv run --with target/py-wheels/xeibe_py-*.whl crates/xeibe-py/tests/smoke.py
"""

import os
import sys
import tempfile
from pathlib import Path

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
    assert isinstance(schema, pa.Schema)
    assert "georeferencja" in scan.explain(LAYER)
    settings = os.path.join(tempfile.mkdtemp(), "prg.gml.json")
    scan.save(settings)
    assert not xeibe.scan([PRG], sample=1).is_complete

    # Without a schema: sampled. The stream is one-shot.
    stream = xeibe.read([PRG], LAYER)
    assert isinstance(stream.schema, pa.Schema)
    table = pa.table(stream)
    assert table.num_rows == 2
    assert table.column("kodPocztowy").to_pylist() == ["68-213", "68-213"]
    geometry = table.schema.field("georeferencja")
    assert geometry.metadata[b"ARROW:extension:name"].startswith(b"geoarrow."), geometry.metadata
    try:
        pa.table(stream)
    except xeibe.XeibeError:
        pass
    else:
        raise AssertionError("a consumed stream must not be read again")

    # A pyarrow.Schema as the schema, options as a dict.
    table = pa.table(xeibe.read([PRG], LAYER, schema=schema, options={"batch_size": 1}))
    assert table.num_rows == 2
    assert table.column_names == schema.names, "a given schema is used as it is"

    # A settings file as the schema; RecordBatchReader interop.
    reader = pa.RecordBatchReader.from_stream(xeibe.read([PRG], LAYER, schema=settings))
    assert sum(batch.num_rows for batch in reader) == 2

    try:
        xeibe.read([PRG], "NoSuchLayer")
    except xeibe.XeibeError as error:
        assert "NoSuchLayer" in str(error)
    else:
        raise AssertionError("an unknown layer is an error")
    print("ok")


if __name__ == "__main__":
    main()
