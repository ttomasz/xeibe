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
- `xeibe wfs count` and `xeibe wfs convert` take `--filter` (an OGC filter
  as XML, or `@FILE`). A request whose URL would be longer than 1,800 bytes
  is sent as an XML `POST` (WFS 1.1 and 2.0; `WfsOptions::method`,
  `--http-method auto|get|post`). Some servers, ArcGIS Server among them,
  fail on URLs from about 2,000 bytes. Vendor parameters (`--param`) and a
  bbox are only sent by `GET`, so a request that must be a `POST` with them
  fails with a message. `next` links too long to send, which GeoServer
  writes after a `POST`, give way to `startIndex` paging. A `POST` that
  the server redirects, and that then fails or isn't answered with features,
  is an error that names the URL to use as the service URL.
- A WFS page whose `numberReturned` is below the features it holds (deegree
  writes `0`), or whose `numberMatched` is below the features received so
  far, no longer ends a `startIndex` read after the first page. The counted
  features are used instead, and such a total is treated as unknown.
- Parquet files with repeated text properties now open in pandas and
  geopandas. View types are written as plain `Utf8`/`Binary`; Arrow IPC
  output keeps the view types.
- WFS 2.0 responses with several queries, and the features a `resolve`
  adds under `wfs:additionalObjects`, are now tested end to end.
- `xeibe-reproject`: geocentric coordinates that are already in an
  operation's source CRS go into it as they are. They used to be converted
  to latitude, longitude and height and back first, which moved them by
  about a nanometre and took most of the time. Conversions from, to or
  through geocentric CRSs, such as ITRF2014 to GDA2020, are up to four
  times faster, and a geocentric CRS to itself returns the input exactly.
- `xeibe-reproject`: `Transform::trace_point` returns every step of one
  point's conversion. Each step is a projection, an operation (named, and
  marked when run inverted) or a leg through a Bound CRS's hub, and comes
  with the CRS the point is in after it, its coordinates and the kind of
  height. An operation between geocentric CRSs also gives its change at
  the point as metres east, north and up, since a change of Z is not a
  change of height. A point that fails keeps the steps up to the failure.
  Conversions are no slower for it. The reproject page shows the steps in
  each point's details.
- `xeibe-reproject`: time-dependent transformations (ITRF2014 to GDA2020,
  say) now run at the epoch of the coordinates. Give it for the whole
  transform with `Transform::with_epochs`, or per coordinate with
  `Transform::convert_at`, `convert_xyt` and `convert_xyzt`. Without an
  epoch they run at their reference epoch, as before. As in PROJ, only a
  dynamic CRS takes an epoch (`Crs::is_dynamic`), and the transform's
  epoch replaces a coordinate's own (`Transform::overrides_epoch` tells).
  Conversions are no slower for it.
- `xeibe-reproject`: two different epochs on a transform
  (`with_epochs(Some(2010.0), Some(2002.0))`, PROJ's `EPSG:8254@2010` to
  `EPSG:8254@2002`) move the coordinates in time with EPSG's point motion
  operations, by velocity grid: within and between the realizations
  NAD83(CSRS)v6 and v7, and ETRF2014 in the Nordic and Baltic countries
  (NKG_RF17vel), also with a transformation before or after. The grid
  loads like any other. PROJ's routes, names and results; unlike PROJ,
  also between CRSs that one EPSG operation joins and from projected
  CRSs. Where EPSG has no point motion operation for either CRS, or none
  xeibe can run (Canada's v8 grid, Indonesia, New Caledonia), two
  different epochs are an error (`Error::Epoch`).
- `xeibe-reproject`: New Zealand's deformation model (`ITRF96 to NZGD2000
  (1)` to `(7)`, EPSG method 1079), a port of PROJ's `defmodel`. It needs
  the epoch of each coordinate: without one the coordinate fails with the
  new `CoordinateError::MissingEpoch`, as in PROJ, and outside the model's
  time with `CoordinateError::EpochOutOfRange`. `OperationInfo::needs_epoch`
  tells in advance. The model's master file names its grids, so they are
  asked for (`Transform::grid_requests`) once it is loaded.
- `xeibe-reproject`: the NKG transformations from ITRF2014 and ITRF2020 to
  the Nordic and Baltic ETRS89 realizations, which start with the
  velocities from the coordinate's epoch to 2000.0 ("ETRF2014 to NKG_ETRF14
  (1)", EPSG method 1144). They need each coordinate's epoch
  (`OperationInfo::needs_epoch`); a coordinate without one goes through the
  next operation, as in PROJ.
- `xeibe-reproject`: every live EPSG CRS now converts. The five projection
  methods PROJ has no mapping for run from EPSG's formulas: Lambert Conic
  Conformal (West Orientated), Polar Stereographic (variant C), Lambert
  Conic Near-Conformal, Tunisia Mining Grid and Transverse Mercator Zoned
  Grid System. 21 CRSs didn't convert before, and the 26 CRSs on these
  methods are now identified from WKT.
- `xeibe-reproject`: Lambert Conic Conformal (2SP Belgium) now turns the
  grid by 29.2985″, as EPSG defines the method and PROJ doesn't.
  Coordinates in BD72 / Belge Lambert 72 (EPSG:31300), the one CRS on it,
  move by 745 m from 0.2.0 and from PROJ, onto EPSG's worked example.
  Belgian Lambert 72 (EPSG:31370) is unchanged. PROJ strings still write
  the CRS as PROJ does, without the turn.
- `xeibe-reproject`: hydroid models to chart datum depths ("Geographic3D to
  Depth", EPSG methods 1116 and 1121), which PROJ can't run. ETRS89 +
  LAT-NLD or MSL-NLD depth now converts to ETRS89, also from UTM and RD
  with those depths.
- `xeibe-reproject`: heights on a national realization of ETRS89 reach the
  ensemble through the realization's own geoid model. ETRS89-SVK [SKTRF09]
  + EVRF2007 height to ETRS89 had no transformation, and SKTRF09 + Baltic
  1957 height took the Czech geoid model: its results change.
- `xeibe-reproject`: EPSG's concatenated operations with two vertical
  conversions in a row ("KOC CD height to KOC WD depth (ft) (1)": an
  offset, height to depth, metres to feet) were left out. They now convert
  as in PROJ.
- `xeibe-reproject`: EVRF2019 zero-tide to mean-tide heights (EPSG method
  1107), from EPSG's formula, which PROJ doesn't import. It is in the
  choice, so 4 pairs change: 0.2.0, like PROJ, applied a constant −0.054 m
  everywhere, and the formula goes from −0.053 m at 42°N to +0.072 m at
  69°N.
- `xeibe-reproject`: EPSG's polynomial transformations (methods 9645, 9648,
  9651 to 9654 and 9617), which PROJ doesn't import. Those that do at
  least as well as what PROJ chooses are in the choice, so their pairs
  change from 0.2.0 and from PROJ: TM75 to ETRS89 (1) and to WGS 84 (1)
  (0.4 m, over the 1 m Helmert), Fiji's (over a route through a 5 m
  Helmert) and ED50 to ED87 (1) (1 m, like the Helmert). Madrid 1870
  (Madrid), which had no transformation, converts to ED50. The others
  (NAM's RD polynomials, ED50 to WGS 84 (35), Belge Lambert 72 to ED50 /
  UTM zone 31N) run only with `Transform::via`.
- `xeibe-reproject`: time-specific Helmert transformations (EPSG methods
  1065 and 1066), which hold at one epoch and which PROJ can't run. They
  run with `Transform::via`, and in the choice only for a coordinate at
  the operation's epoch (to the thousandth of a year), inside its area,
  when nothing else there is more accurate: "WGS 84 to ETRS89 (2)" at
  2014.81 in the German and Dutch North Sea, say. Every other coordinate,
  and every one without an epoch, chooses as before. The routes are listed
  last, with `OperationInfo::only_at_epoch`. A pair with only such routes
  now sets up, and its coordinates fail without an epoch
  (`CoordinateError::MissingEpoch`) or at another one (`EpochOutOfRange`).
  At the epoch such a pair's route converts them whatever its area, as any
  only operation does ("ITRF2000 to SIRGAS 2000 (1)" at 2000.4).
  `Transform::explain_point` says of such a route that the coordinate isn't
  at its epoch (`Outcome::NotAtEpoch`), and of an operation it displaces
  why (`PassReason::AtEpoch` when the two are as accurate). The reproject
  page marks these routes, and doesn't count them among the candidates.
- The reproject page takes a coordinate epoch, optionally: none by
  default, one decimal year for every point, or each point's own as a
  fourth value on its line (`x y z t`, as `cs2cs` reads it), chosen in the
  operation options. With it the time-dependent operations run at that
  epoch, a deformation model's points convert, and a point at a
  time-specific Helmert's epoch goes through it. Where an operation
  depends on the epoch and none is set, its card says so and offers to
  set one (the operation's own, for a time-specific Helmert), and lines
  of four numbers come with the offer to read the fourth as the epoch.
  The PROJ commands carry the epoch as the fourth column; the output is
  the coordinates alone, and swapping source and target puts each
  point's epoch back.
  `xeibe-reproject`: `Transform::explain_point_at`,
  `trace_point_at`, `operation_for_at` and `operation_index_for_at`, for a
  coordinate with an epoch of its own, as `convert_at` converts it.
- `xeibe-crs` 0.4.3: the operations table gains the transformations of the
  methods above that PROJ doesn't import (EVRF2019's and the polynomials).
- Rust API changes that break callers, besides those above:
  `MemberRule::element` gave way to `MemberRule::matches`;
  `xeibe_geom::arcs::arc_by_bulge_midpoint` takes a `BulgeNormal`;
  `xeibe_wfs::paging::NextPage`, `CoordinateError`, `Outcome`, `PassReason`
  and the `Error` enums of `xeibe-reproject` and `xeibe-wfs` have new
  variants; `OperationInfo`,
  `Capabilities`, `Constraints`, `GeometrySniff`, `GeometryStats`,
  `InferenceOptions`, `GmlOptions` and `WfsOptions` have new fields; and
  `xeibe_io::Error::HttpStatus` names the URL the response came from after
  redirects.

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
