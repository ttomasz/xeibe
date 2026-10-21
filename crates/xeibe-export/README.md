# xeibe-export

Write Arrow record batches with **GeoArrow** geometry columns, as
[`xeibe-arrow`](https://crates.io/crates/xeibe-arrow) reads them from GML, to
files. Part of [xeibe](https://github.com/ttomasz/xeibe).

> **Warning: experimental, not for production use.** This crate is
> AI-generated and has not been thoroughly tested yet. Its interfaces are not
> stable and will change in the near future, in breaking ways. Do not use it in
> production.

## What it does

- Feature `parquet`: Parquet, ZSTD-compressed. Every geometry column is
  written as WKB with the Parquet `GEOMETRY` logical type and GeoParquet 1.1
  `geo` metadata (geometry types, CRS as PROJJSON, bbox), optionally with a
  `bbox` covering column. Curves are an error: neither standard has them.
- Feature `ipc`: Arrow IPC files, with the batches as they are.
- Writers take a batch at a time and write to any `std::io::Write`.

No feature is on by default.

## Documentation

See [Parquet and GeoParquet output](https://github.com/ttomasz/xeibe/blob/main/docs/geometry.md#parquet-and-geoparquet-output).

## License

CC0-1.0 (public domain dedication).
