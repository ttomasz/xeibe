# xeibe: GML → GeoArrow

A set of Rust crates that read **GML** (Geography Markup Language) documents and
**WFS** responses and convert them into **Apache Arrow** record batches with
**GeoArrow** geometry columns. The resulting batches can be written to Parquet or
GeoParquet, queried with DataFusion or SedonaDB, or passed to Python.

> **Status: design phase.** Nothing is implemented yet. These documents describe
> the intended design. [`support-matrix.md`](support-matrix.md) tracks progress.

## Goals

- **Bulk conversion** of large GML files (multi-GB) and complete WFS datasets,
  following pagination until every feature has been fetched.
- **GML 2, 3.1 and 3.2.** The target is the simple-feature part of GML plus curves,
  not all of GML.
- **Streaming.** Memory stays bounded no matter how large the input is.
- **Lossless where possible.** Curves are kept as curves. Identifiers such as `0012`
  stay strings. Values that don't fit the schema go into an overflow column instead
  of being dropped.
- **Schema inference first.** Real datasets rarely come with a usable XSD.
  The schema is inferred from the data using explicit, configurable rules.
- **Rich Arrow types.** `Utf8View`, `Date32`, `Timestamp`, `List`,
  `Struct`, `Map` and GeoArrow extension types.
- **Pure Rust.** No GDAL or other C dependencies. Can run on WebAssembly.

## Non-goals

- CityGML and other 3D, solid-heavy profiles.
- Resolving `xlink:href` references. They are kept as string columns, which can be
  used as foreign keys.
- XSD-driven schema mapping in the style of GDAL's GMLAS driver.
- **Writing GML, ever.** The project converts in one direction only, GML → GeoArrow.
  There will be no GML encoder, no round-tripping, and no WFS-T (transactions).
- Coverages, topology, and GML dictionaries or CRS definitions.

## Documents

| Document | Contents |
|---|---|
| [architecture.md](architecture.md) | `scan` / `read` API, settings file, crate layout, data flow, sources, parallelism, integrations |
| [schema-inference.md](schema-inference.md) | Scan, path tree, value statistics, `InferenceOptions`, rule engine, sampled schemas, `--explain` |
| [type-mapping.md](type-mapping.md) | How XML values and structures map to Arrow types; field metadata |
| [geometry.md](geometry.md) | GML geometry → GeoArrow/WKB, curves, CRS and axis order |
| [wfs.md](wfs.md) | WFS bulk-read client: paging, pages streamed into a read |
| [support-matrix.md](support-matrix.md) | Supported, planned and out-of-scope GML/WFS features (progress tracker) |

## Key decisions

| Decision | Rationale |
|---|---|
| Separate crates: core / geom / schema / arrow / wfs / integrations | Users who only need geometry don't pull in Arrow. Each part is testable on its own. |
| Standalone project with a thin DataFusion adapter, instead of developing inside SedonaDB | Faster iteration while the API is unstable. Also usable outside SedonaDB. Upstream an adapter later. |
| `geo-traits` as the geometry interface | One GML geometry model can feed GeoArrow builders, WKB writers and `geo`. |
| Schema inference = **observation** (path tree) + **policy** (`InferenceOptions`) | Different rules can be applied to one scan. Decisions can be explained. |
| XML attributes always prefixed with `@` | Predictable, never collides with element names. |
| No `Decimal128` | Poor support in downstream tools. Uses a "value-lossless" float rule instead. See type-mapping. |
| Two operations: **scan** (layers + schemas, full or sampled) and **read** (one layer, given or sampled schema) | Same model as DataFusion/Spark/Polars: pass a schema or infer it from a sample. Scan once, keep the schema, read many times. |
| Read parameters and schemas are kept apart; both fit in one JSON **settings file**. Schemas are `column → Arrow type string` maps | Editable by hand, reusable across inputs of the same kind. CRS and axis order are parameters, not schema. No custom type syntax beyond `Geometry(…)`. |
| **Stateless: every source is one sequential stream.** No download cache, no chunk index, no range requests, no saved WFS pages | Important servers such as geoportal.gov.pl don't support range requests. The splitter skips other layers cheaply, so an index buys little. Caching is left to the embedding framework. Much less to build and to keep consistent. |
| Zip kept minimal: the `zip` crate as it is, **local files only**, GML members found by content, one input per archive, no zips inside zips | DataFusion, Spark and Polars don't read zip at all. Only GDAL does it thoroughly. Remote zips are downloaded by the user. |
| A read without a schema samples **the requested layer**, with conservative types | Layers stored one after another (PRG) make a sample of the file's start useless. A frozen schema must accept data it hasn't seen. |
| One layer per read; the CLI stays simple (`scan`, `convert` one layer, `wfs`) | A multi-output read needs unbounded buffering when outputs drain at different speeds. |
| `ByteSource` in `xeibe-core` is synchronous. Async I/O lives in `xeibe-io` | Parsing is CPU-bound, and the core stays free of runtimes. DataFusion's `object_store` is adapted to it. |
| Parquet output: WKB + the native Parquet `GEOMETRY` type + GeoParquet 1.1 metadata | Row-group bbox statistics work for new readers, and the `geo` metadata keeps older ones working. Columns with curves are an error unless linearized: GeoParquet 1.1 forbids curve types. |
| xlinks kept as strings, never resolved | Keeps reading streaming. Works as foreign keys for SQL joins. |
| Out-of-schema data goes to an `_overflow` map column | Lossless with any schema: sampled, old or hand-written. The Arrow schema never changes mid-stream. |
| Where the specs are silent (e.g. `ArcByCenterPoint` angle convention, `EPSG:XXXX` axis order), follow GDAL | Most existing GML has been checked against GDAL. Marked **[GDAL]** in the docs |

## Test corpus

Real data in `example_data/` (not committed):

- **PRG address points** (Polish national address register): GML 3.2, 0.6–3.3 GB per
  file. Three feature types stored one after another (`AD_Miejscowosc`,
  `AD_UlicaPlac`, `AD_PunktAdresowy`). About 1.1M `xlink:href="#…"` references.
  `srsName="EPSG:2180"` in short form with coordinates in x/y order. Contains empty
  elements, INSPIRE-style type-wrapper elements, and datetimes both with and without
  a time zone.
- **BDOT10k** (Polish national topographic database): a multi-file package with XSDs.
- **PRG administrative boundaries** (`A00_Granice_panstwa.gml`, national border;
  `A01_Granice_wojewodztw.gml`, voivodeship borders): `gml:Surface` geometries.

Still needed: a GML 2 file using `<gml:coordinates>`, a GML 3.1.1 file, a WFS 1.1
response and a WFS 2.0 response, and data containing arcs (`Arc`, `ArcString`,
`CircleByCenterPoint`).

Reference material:
- `ogc_schemas/`: a local copy of schemas.opengis.net, plus the spec PDFs (GML 2.1.2,
  3.1.1, 3.2.1; GML SF profile 2.0; WFS 1.0/1.1/2.0.0/2.0.2; FES 2.0.2; OWS Common 2.0;
  CRS naming 07-092r3, 09-048r7, 11-135r2). The docs cite sections as `07-036 §10.4.7`
  and similar.
- `stuff.md`: links, including GDAL's GML autotests.
- `scripts/schema_coverage.py` checks that every schema element appears in the
  support matrix.
- `scripts/gdal <cmd>` runs GDAL **3.13.3** (Docker image `ghcr.io/osgeo/gdal:ubuntu-full-3.13.3`,
  with the GML, GMLAS, WFS, Parquet and Arrow drivers) as the reference implementation.
  The system GDAL is 3.8.4 and has no Parquet driver. The script keeps a
  background container running, so calls take ~50 ms instead of ~5 s.
  `example_data/`, `ogc_schemas/` and `tests/data/` are mounted read-only, so GDAL
  can't write `.gfs` files there. Write outputs to `target/gdal/`.
  `scripts/gdal --stop` removes the container.
- `scripts/validate-gml FILE…` validates samples offline with `xmllint --stream`
  against the schemas named in their `xsi:schemaLocation`. `schemas-extra/catalog.xml`
  maps `schemas.opengis.net` to `ogc_schemas/`, and `schemas-extra/` also holds the
  W3C `xlink.xsd`/`xml.xsd` and the extracted OGC xlink 1.0.0 schemas (both folders
  are git-ignored and built by `scripts/fetch_ogc_schemas.py`). Use `--schema ns=path` for
  local application schemas and `--net` for remote ones.
- Rust test tooling: `cargo nextest run --workspace`, `cargo insta review` (snapshots).

## Open questions

- How srsNames resolve to CRSs, and where PROJJSON for GeoParquet comes from: see
  [geometry.md](geometry.md#open-questions-srsname--crs).
- Default nesting for the CLI: `Struct` (lossless) or `FlattenSingleOnly`
  (friendlier for QGIS and shapefile users). The current plan is `Struct` in the
  library and `flat` in the CLI.
- Whether to use `DescribeFeatureType` as a *hint* for WFS schema inference, for
  example for columns that are always null.
- Whether to add a WebDAV-free directory listing for HTTP (e.g. parsing Apache or
  nginx index pages), or to require explicit file URLs.
- The file extension for settings files (`*.gml.json`?), and whether reads embed
  the settings in the Parquet metadata of the output by default.
