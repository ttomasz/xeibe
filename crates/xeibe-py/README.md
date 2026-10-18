# xeibe (Python)

Read **GML** (Geography Markup Language) files into **Apache Arrow** with
**GeoArrow** geometry. The Python bindings of
[xeibe](https://github.com/ttomasz/xeibe), a Rust library.

> **Warning: experimental, not for production use.** This package is
> AI-generated and has not been thoroughly tested yet. Its interfaces are not
> stable and will change in the near future, in breaking ways. Do not use it in
> production.

## Usage

```python
import pyarrow as pa
import xeibe

# Every layer of the input, with its inferred schema.
scan = xeibe.scan(["parcels.gml"])
for layer in scan.layers:
    print(layer["name"], layer["feature_count"], layer["crs"])
print(scan.explain("Parcel"))  # why each column got its type

# One layer as a stream of record batches. Without a schema, the columns and
# their types are inferred from the layer's first features.
table = pa.table(xeibe.read(["parcels.gml"], "Parcel"))
```

`scan()` reads the whole input; `scan(paths, sample=1000)` stops after the
first 1000 features. Inputs are always a list: paths, globs, zip archives,
HTTP(S) URLs and object-store URLs (`s3://`, `gs://`, `az://`).

### Giving the schema

`read()`'s `schema` takes an Arrow schema or xeibe settings. A given schema
is also the projection: only its columns are read.

```python
# An Arrow schema: from a scan, or any object with __arrow_c_schema__
# (pyarrow.Schema, arro3.core.Schema, ...).
schema = scan.schema("Parcel")
stream = xeibe.read(["data/*.gml"], "Parcel", schema=schema)

# A settings file, as written by Scan.save() or `xeibe gml scan -o`: the
# columns of every layer and the read options, so every run reads the same
# schema.
scan.save("parcels.json")
stream = xeibe.read(["data/*.gml"], "Parcel", schema="parcels.json")

# Or only this layer's columns, as a dict (or JSON text): column name to type,
# or to type and the element path it is read from.
stream = xeibe.read(
    ["data/*.gml"],
    "Parcel",
    schema={
        "id": {"type": "text", "path": "@id"},
        "area": "double",
        "geometry": "geometry(MultiPolygon)",
    },
    options={"batch_size": 10_000},
)
```

`options` takes the read options as a dict, JSON text or a settings file.
Options given there override those of a settings file given as `schema`.

### Arrow objects

`scan.schema()` returns an `arro3.core.Schema`, and `read()` returns an
`arro3.core.RecordBatchReader`, a one-shot stream (from
[arro3](https://github.com/kylebarron/arro3), the package's only dependency).
Both implement the Arrow PyCapsule interface, so PyArrow (`pa.table(stream)`),
GeoPandas (`GeoDataFrame.from_arrow`), DuckDB, Polars and SedonaDB take them
as they are, without a copy. `xeibe.sedona` (extra `sedona`) makes GML a
SedonaDB data source.

## Documentation

See the [architecture document](https://github.com/ttomasz/xeibe/blob/main/docs/architecture.md)
(settings file, read options) and the
[support matrix](https://github.com/ttomasz/xeibe/blob/main/docs/support-matrix.md).

## License

CC0-1.0 (public domain dedication).
