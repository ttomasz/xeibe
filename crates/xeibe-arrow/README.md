# xeibe-arrow

Read **GML** (Geography Markup Language) documents into **Apache Arrow** record
batches with **GeoArrow** geometry columns. This is the main library crate of
[xeibe](https://github.com/ttomasz/xeibe).

> **Warning: experimental, not for production use.** This crate is
> AI-generated and has not been thoroughly tested yet. Its interfaces are not
> stable and will change in the near future, in breaking ways. Do not use it in
> production.

## What it does

- `scan()` lists every layer (feature type) of the input with its feature
  count, geometry columns, CRSs, extent and inferred Arrow schema.
- `read()` streams one layer as Arrow `RecordBatch`es: an `Iterator` and an
  `arrow_array::RecordBatchReader`. The schema is inferred from a sample of the
  layer, or given.
- GML 2, 3.1, 3.2 and 3.3, curves included. Geometry columns are GeoArrow
  (WKB or native types), with CRS metadata from the EPSG dataset and axis
  order resolved.
- Memory stays bounded however large the input is, and the input is parsed on
  several threads.
- A settings file (JSON) pins columns, types and options so that every run
  reads the same schema.

```rust,no_run
use xeibe_arrow::{ReadOptions, read, scan};
use xeibe_core::Source;
use xeibe_schema::ScanExtent;

let options = ReadOptions::default();

let result = scan(Source::file("parcels.gml")?, ScanExtent::Full, &options)?;
for layer in result.layers() {
    println!("{}: {} features", layer.name, layer.feature_count);
}

for batch in read(Source::file("parcels.gml")?, "Parcel", None, &options)? {
    let batch = batch?;
    println!("{} rows", batch.num_rows());
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

Inputs are files (also gzip- or zstd-compressed), zip archives and readers.
HTTP(S) and object-store inputs come from
[`xeibe-io`](https://crates.io/crates/xeibe-io), WFS services from
[`xeibe-wfs`](https://crates.io/crates/xeibe-wfs).

## Documentation

The design documents are in the repository:
[architecture](https://github.com/ttomasz/xeibe/blob/main/docs/architecture.md)
(the `scan`/`read` API, the settings file, parallelism),
[schema inference](https://github.com/ttomasz/xeibe/blob/main/docs/schema-inference.md),
[type mapping](https://github.com/ttomasz/xeibe/blob/main/docs/type-mapping.md),
[geometry](https://github.com/ttomasz/xeibe/blob/main/docs/geometry.md) and
the [support matrix](https://github.com/ttomasz/xeibe/blob/main/docs/support-matrix.md).

## License

CC0-1.0 (public domain dedication).
