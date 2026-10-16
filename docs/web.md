# Web page (WebAssembly)

🤔 Considering (P2). A static page on GitHub Pages that runs xeibe compiled to
WebAssembly. A user drops a local GML file or a saved WFS response on it and sees
how xeibe reads it: layers, the inferred schema and why, axis-order decisions,
the read report, the data on a map and in a table. Nothing is uploaded: the file
never leaves the browser.

It serves two purposes at once:

- **Debugging.** "Why is this column `text`?", "why are my points in the sea?",
  "which features were skipped?" answered without installing anything.
- **Showcase.** The quickest way to see what xeibe does with real data.

It is a thin front end over `xeibe-arrow`. It adds no reading behaviour of its
own, so what the page shows is what the CLI and Python produce with the same
options.

## Non-goals

- A general GIS viewer or editor. One file (or zip), one layer at a time.
- Reprojection in the library. The page reprojects for display only (see
  [Map](#map)); exports keep the source CRS, as everywhere else.
- Multi-GB inputs. Browser memory limits apply (see [Limits](#limits)). The CLI is
  the tool for those.
- A hosted service. The page is static files; there is no server side.

## Layout

The screen is split in two halves. The divider can be dragged; on narrow screens
the map stacks above the tabs.

| Area | Contents |
|---|---|
| Top bar | File picker and drop zone, layer selector (BDOT10k-style files have dozens of layers), "Try a sample", progress |
| Left half | Map |
| Right half | Tabs, below |

| Tab | Shows | From |
|---|---|---|
| Overview | Feature count, extent, CRS, axis-order decisions with their reasons, read report (skipped features, warnings, skipped sources, whether the scan was complete) | `ScanResult::layers`, `extent`, `axis_decisions`, `is_complete`, `skipped_sources`; the read report |
| Schema | Columns with Arrow type and source path, and the inference reasoning | `ScanResult::schema`, `explain` |
| Data | Virtualized table of the read rows | the record batches |
| Options | Form for the read options (preset, overrides, axis order, CRS, linearization, …) and the equivalent `xeibe` command, Python snippet and settings file, ready to copy | `ReadOptions`, `ScanResult::to_settings` |
| Export | Download as GeoParquet or Arrow IPC | the record batches |

Selecting a row zooms the map to its feature; clicking a feature on the map
selects its row.

The Options tab is what makes the page more than a viewer: options are tried
interactively, then carried over to the CLI or Python unchanged.

## Map

**The map needs lon/lat; xeibe never reprojects** ([geometry.md](geometry.md),
support matrix: "Reprojection: not planned"). The page reprojects in JavaScript,
for display only:

- proj4js, given the CRS definition from `xeibe_crs::projjson(code)`. proj4js
  parses definitions with `wkt-parser`, which reads WKT1, WKT2 and PROJJSON.
  **Unverified:** check that proj4js accepts our PROJJSON for the common cases
  (EPSG:2180, the ETRS89 / UTM and Gauss-Krüger families, the AdV CRSs) before
  relying on it.
- A CRS that can't be reprojected (unknown, engineering, failed parse) falls back
  to a plain Cartesian view with no basemap.

Two things the map shows that a table doesn't:

- **Wrong axis order is visible at a glance.** Swapped coordinates land in the
  wrong country or the sea.
- **The CRS's area of use** (`CrsRecord::area_in_axis_order`) drawn as a
  rectangle. Data outside it explains itself.

Rendering: deck.gl with `@geoarrow/deck.gl-layers`, which draws GeoArrow arrays
straight from Arrow IPC, with no GeoJSON conversion. A basemap via MapLibre.
Reprojection then has to happen on the GeoArrow coordinate buffers before they
are handed to deck.gl.

## Architecture

```mermaid
flowchart LR
    file["File / drop"] --> worker
    subgraph worker["Web Worker"]
        bytes["bytes in memory"] --> wasm["xeibe-wasm<br/>scan, read (threads = 1)"]
    end
    worker -->|"scan result (JSON)<br/>batches (Arrow IPC)"| ui["UI thread"]
    ui --> map["map (deck.gl + MapLibre)"]
    ui --> tabs["tabs"]
```

- **`crates/xeibe-wasm`**: wasm-bindgen bindings. Not a default workspace member,
  like `xeibe-py`. Kept thin, so the unstable Rust API doesn't spread into
  JavaScript: a few functions taking bytes and an options object, returning JSON
  and Arrow IPC.
- **Web Worker.** Everything runs in a worker so a long read doesn't freeze the
  UI. Results cross to the UI thread as Arrow IPC buffers (transferable, no copy).
- **Single-threaded.** WebAssembly threads need `SharedArrayBuffer`, which needs
  COOP/COEP headers, which GitHub Pages can't set. The read runs with
  `threads = 1`, which must mean "everything on the calling thread" (see
  [Library changes](#library-changes)).
- **`web/`**: the front end (Vite, TypeScript or plain JS) in this repository.
- **`.github/workflows/pages.yml`**: builds `xeibe-wasm` and `web/`, deploys to
  GitHub Pages.

### Inputs

| Input | How |
|---|---|
| `.gml`, `.xml`, saved WFS responses | Read into memory, wrapped in an in-memory `ByteSource` |
| `.gz`, `.zst` | As above; `Source::open` decompresses |
| `.zip` | Needs an in-memory archive source (below) |
| Live WFS | Out of scope at first: most servers send no CORS headers, so the browser blocks the request. Maybe later as best effort for servers that do |

A streaming `ByteSource` over `Blob.slice()` with `FileReaderSync` (available in
workers) would avoid holding the whole file in memory. That is a later
improvement, not needed for the first version.

## Library changes

Checked on 2026-10-16 with `cargo check`/`cargo build --target
wasm32-unknown-unknown`:

| Crate | State |
|---|---|
| `xeibe-core`, `xeibe-crs`, `xeibe-geom`, `xeibe-schema`, `xeibe-arrow` | Compile. zstd's C code compiled with the system clang |
| `xeibe-io` (even without `http`) | Fails: `Url::to_file_path` in `resolve.rs` doesn't exist on wasm. The page doesn't need `xeibe-io`; a `cfg` fixes the build anyway |
| `xeibe-wfs` | Fails through `xeibe-io` |

Compiling isn't enough: on `wasm32-unknown-unknown`, spawning a thread panics at
runtime. Needed:

1. **A pipeline that can run without threads.** `Pipeline::start` always starts a
   splitter, at least one worker and a reorder thread, and `scan_sample` uses
   `std::thread::scope` even for one thread. `threads = 1` should run everything
   on the calling thread; this is planned as part of making `threads` an exact
   count of the threads xeibe uses ([architecture.md](architecture.md#parallel-pipeline)).
   The scanner already runs on the calling thread for `threads <= 1`, and the
   decompression read-ahead thread (`decode.rs`) is already skipped on wasm.
2. **The default thread count on wasm.** `available_parallelism()` fails there and
   the default falls back to 1, which is right once 1 means inline. A request for
   more threads on wasm should be an error, not a panic.
3. **In-memory zip archives.** `Source::ZipMember` holds a `PathBuf` and
   `archive.rs` opens it with `std::fs::File`. The `zip` crate itself only needs
   `Read + Seek`, so an archive held in memory (`Cursor<Bytes>`) is a small
   addition.
4. **zstd.** Builds today with clang. If linking or CI makes it a problem, the
   pure-Rust `ruzstd` decoder can replace it on wasm only.
5. **GeoParquet export** needs `parquet` in the wasm build; its `zstd` feature
   brings the same C dependency.

## Limits

- **Memory.** wasm32 addresses at most 4 GB; in practice browsers manage 1–2 GB.
  Files of a few hundred MB are fine. For bigger ones the page offers "first N
  features": it stops pulling batches after N rows, which with an inline read
  also stops the parsing. That bounds the Arrow output but not the input, which
  is in memory until reads stream from the `Blob` (see [Inputs](#inputs)). Beyond
  that, the page points to the CLI.
- **Speed.** One thread, so several times slower than the CLI on a multi-core
  machine. Fine for previewing.
- **Bundle size.** The EPSG tables plus arrow, geoarrow and parquet: a wasm file of
  a few MB, an estimated 1–2 MB gzipped. To be measured; loaded once, then cached.
- **CORS** rules out most live WFS endpoints (see [Inputs](#inputs)).

## Phases

1. **MVP.** Drop a file; layer selector; Overview, Schema and Data tabs (first
   100 rows); Parquet download. Requires library changes 1–2.
2. **Map and options.** Map with display reprojection and the area-of-use box;
   Options tab with the CLI, Python and settings export; zip inputs; samples from
   `tests/data`.
3. **Maybe.** Streaming reads of large files; WFS against servers that allow
   CORS.

## Open questions

- Does proj4js accept `xeibe_crs::projjson` output for the CRSs that matter? If
  not: WKT from the same tables, or proj4 strings.
- Which `tests/data` samples to offer under "Try a sample", and whether to bundle
  them or fetch them from the repository.
- Front-end tooling: plain JS keeps the toolchain smallest; TypeScript helps once
  the Options form mirrors `ReadOptions`. Could `ReadOptions` be described
  (JSON Schema) so the form is generated rather than kept in sync by hand?
