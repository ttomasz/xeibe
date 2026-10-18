# xeibe-cli

The `xeibe` command: convert **GML** files and **WFS** services to
(Geo)Parquet or Arrow IPC. Part of [xeibe](https://github.com/ttomasz/xeibe).

> **Warning: experimental, not for production use.** This tool is
> AI-generated and has not been thoroughly tested yet. Its commands, options
> and output are not stable and will change in the near future, in breaking
> ways. Do not use it in production.

## Install

```sh
cargo install xeibe-cli
```

The `object-store` feature adds `s3://`, `gs://` and `az://` inputs.

## Usage

```sh
xeibe gml scan parcels.gml                          # layers and their inferred schemas
xeibe gml convert parcels.gml --layer Parcel -o parcels.parquet
xeibe gml convert 'data/*.gml' --layer Parcel -o parcels.parquet --settings parcels.json

xeibe wfs layers https://example.com/geoserver/wfs  # feature types a service offers
xeibe wfs count https://example.com/geoserver/wfs --type-name app:Parcel
xeibe wfs convert https://example.com/geoserver/wfs --type-name app:Parcel -o parcels.parquet
```

Inputs can be files (also `.gml.gz` and `.gml.zst`), directories, globs, zip
archives, `-` for stdin, and URLs. Every command explains its options, with
examples, in `--help`.

## Documentation

See the [architecture document](https://github.com/ttomasz/xeibe/blob/main/docs/architecture.md)
and the [support matrix](https://github.com/ttomasz/xeibe/blob/main/docs/support-matrix.md).

## License

CC0-1.0 (public domain dedication).
