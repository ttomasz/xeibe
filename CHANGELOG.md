# Changelog

The workspace crates, the `xeibe` CLI and the `xeibe` Python package, released
together under one version by a `v*` tag ([docs/releasing.md](docs/releasing.md)).
`xeibe-crs` has its own changelog of EPSG releases
([crates/xeibe-crs/CHANGELOG.md](crates/xeibe-crs/CHANGELOG.md)), and
`xeibe-proj` its own ([crates/xeibe-proj-py/CHANGELOG.md](crates/xeibe-proj-py/CHANGELOG.md)).

## 0.2.0

- `xeibe-reproject`: `Crs::from_user_input` reads a CRS from text in any
  form pyproj's `CRS.from_user_input` takes: EPSG codes in their usual
  spellings (`2180`, `EPSG:2180`, OGC URNs and URLs, `EPSG:2180+9650`),
  OGC's CRS84, CRS83 and CRS27, EPSG names and aliases, Esri's old codes of
  EPSG CRSs (`ESRI:102100`), PROJ strings (with or without `+type=crs`),
  WKT and PROJJSON. `user_input::parse` says which one a text is without
  reading it.

## 0.1.0

First release. Experimental: the Rust API, the CLI, the Python module and the
settings file will change, in breaking ways.

- GML 2, 3.1, 3.2 and 3.3 files and WFS 1.0, 1.1 and 2.0 responses (with
  paging) read into Arrow record batches with GeoArrow geometry, streamed
  with bounded memory and parsed in parallel, with schemas inferred from the
  data.
- Writers for Parquet with GeoParquet metadata and for Arrow IPC; a
  DataFusion table provider and `read_gml()`.
- Pure-Rust coordinate conversion (`xeibe-reproject`) with EPSG Dataset
  13.106 embedded (`xeibe-crs` 0.4.2).
- The `xeibe` command: `cargo install xeibe-cli`, or the binaries attached to
  this release (Linux x86_64 and aarch64, macOS arm64, Windows x86_64).
- The `xeibe` Python package: `pip install xeibe`.
