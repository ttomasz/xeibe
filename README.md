# xeibe

An AI-generated Rust library for reading **GML (Geography Markup Language)** files and **OGC WFS (Open Geospatial Consortium Web Feature Service)** responses as **Apache Arrow**
record batches with **GeoArrow** geometry. The batches can be written to Parquet or
GeoParquet, queried with DataFusion or SedonaDB, or passed to Python.

- GML 2, 3.1, 3.2 and 3.3, including curves; WFS 1.0, 1.1 and 2.0 with paging.
- Streaming with bounded memory, parsed in parallel.
- Schemas inferred from the data, with explicit, configurable rules.

> **Warning: experimental, not for production use.** xeibe is AI-generated
> and has not been thoroughly tested yet. Its interfaces (the Rust API, the
> CLI, the Python module and the settings file) are not stable and will change
> in the near future, in breaking ways. Do not use it in production.

Every crate is implemented and the test suites pass. The design lives in
[`docs/`](docs/README.md), and [`docs/support-matrix.md`](docs/support-matrix.md)
tracks what is supported.

## Crates

| Crate | Role |
|---|---|
| `xeibe-core` | Streaming XML, namespaces, feature splitting, byte sources, zip |
| `xeibe-crs` | EPSG CRS facts, PROJJSON and coordinate operations, generated from the EPSG dataset |
| `xeibe-reproject` | Pure-Rust coordinate conversion between CRSs |
| `xeibe-geom` | GML geometry, WKB, CRS and axis order |
| `xeibe-schema` | Scanning and schema inference |
| `xeibe-arrow` | `scan()` and `read()`: features to Arrow, settings file, parallel pipeline |
| `xeibe-export` | Writers: Parquet with GeoParquet metadata, Arrow IPC |
| `xeibe-io` | HTTP and object-store inputs |
| `xeibe-wfs` | WFS capabilities and paged reads |
| `xeibe-datafusion` | DataFusion table provider and `read_gml()` |
| `xeibe-cli` | The `xeibe` command: `gml scan`, `gml convert`, `wfs layers`/`count`/`convert` |
| `xeibe-py` | Python module `xeibe` |
| `xeibe-wasm` | Bindings for the web page in [`web/`](web/) (not published) |
| `xeibe-testkit` | Test support for the suites (not published) |

Setting up a development environment is described in [`CLAUDE.md`](CLAUDE.md).

## Licensing

**The code and documentation** are released under **CC0 1.0** (public domain
dedication, see [`LICENSE`](LICENSE)).

**The test data in [`tests/data/`](tests/data/) is not CC0.** It holds small
excerpts of real datasets, and each keeps its publisher's licence:

- Polish official material (GUGiK/PZGiK): public domain in effect;
- Creative Commons Attribution 4.0;
- **Creative Commons Attribution-ShareAlike 3.0 Estonia**: excerpts stay under
  this licence;
- Datenlizenz Deutschland – Namensnennung 2.0; GeoNutzV (German federal data);
- Licence Ouverte / Open Licence 2.0 (Etalab);
- SITG open-data terms (Geneva);
- GDAL test-suite snippets in `tests/data/gdal/`: MIT, see
  [`tests/data/LICENSE-GDAL.txt`](tests/data/LICENSE-GDAL.txt).

Every file, its source, its licence and the attribution it requires are listed
in [`tests/data/BOM.md`](tests/data/BOM.md). Keep that attribution when you
redistribute the files.

Some data is downloaded by scripts and never committed: the raw test corpus
(`example_data/`, sources and licences in
[`example_data/SOURCES.md`](example_data/SOURCES.md)) and the OGC schemas and
specifications (`ogc_schemas/`, under OGC's terms).
