# Changelog

The workspace crates, the `xeibe` CLI and the `xeibe` Python package, released
together under one version by a `v*` tag ([docs/releasing.md](docs/releasing.md)).
`xeibe-crs` has its own changelog of EPSG releases
([crates/xeibe-crs/CHANGELOG.md](crates/xeibe-crs/CHANGELOG.md)), and
`xeibe-proj` its own ([crates/xeibe-proj-py/CHANGELOG.md](crates/xeibe-proj-py/CHANGELOG.md)).

## Unreleased

- GML Simple Features profile files that use their own collection element
  (as in the profile's Annex D.5) now read with default options. Before,
  they failed with "no feature collection or feature member found".
- Geometry ids (`gml:id`, or `gid` in GML 2) can be kept as a text column
  with `geometry_id = "Column"`. They are still dropped by default.
  Attributes on a geometry property itself, such as `priorityLocation`'s
  `@priority`, are now read like any other attribute.
- `ArcByBulge`, `ArcStringByBulge` and GML 3.3's simple variants now follow
  the standard's definition. A two-number normal is used as a vector (only
  its first number was used before, so `0 1` gave a straight line). The
  midpoint is now computed before the axis swap, which fixes arcs in CRSs
  written y/x: they used to bulge to the wrong side.
- `xeibe wfs convert` without a layer schema in the settings reads the
  server's DescribeFeatureType schema. It gives types to columns that
  have no values in the sample, and adds declared properties the sample
  never saw. The sample's own types still win, and a failed request is
  only a warning.
- `--sort-by` (`WfsOptions::sort_by`) takes WFS sort keys such as
  `'name DESC,id'` and sends them with every request of a read. Before, it
  was only sent with offset pages, so reads that follow `next` links were
  never sorted. There are warnings when paging without a sort order could
  return pages out of order, and when the server can't sort.
- Parquet files with repeated text properties now open in pandas and
  geopandas. View types are written as plain `Utf8`/`Binary`; Arrow IPC
  output keeps the view types.
- WFS 2.0 responses with several queries, and the features a `resolve`
  adds under `wfs:additionalObjects`, are now tested end to end.

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
