# Benchmarks

Synthetic benchmarks, run with `cargo bench`. They need no corpus, no
network and no grids: every input is generated in memory from a fixed seed,
so two runs on one machine measure the same bytes, and a change in the
numbers is a change in the code.

```sh
cargo bench --bench '*'                      # every benchmark of the default members
cargo bench -p xeibe-arrow                   # one crate
cargo bench -p xeibe-arrow -- read/given     # benchmarks whose name contains this
cargo bench -p xeibe-geom -- --quick         # a rough number fast (Criterion's --quick)
cargo bench -p xeibe-core -- --list          # list them
```

The harness is [Criterion](https://docs.rs/criterion/0.8), without its
plots: the report is text, and the results are kept in
`target/criterion/`. Each run compares itself with the one before and says
whether it changed. To compare a branch with `main`, save a baseline on
`main` and compare against it:

```sh
git switch main && cargo bench -p xeibe-arrow -- --save-baseline main
git switch -   && cargo bench -p xeibe-arrow -- --baseline main
```

Criterion's own flags (`--quick`, `--list`, `--save-baseline`) work with
`-p <crate>` or `--bench '*'`, which only run the benchmark targets. A bare
`cargo bench -- --quick` also runs the libtest harness of crates without
benchmarks, which rejects them. The libraries with benchmarks set
`bench = false` on their `[lib]` for that reason.

They run in the `bench` profile, which is `release` with line tables, so a
profiler such as `perf` shows readable stacks
(`perf record -g target/release/deps/arrow-* --bench read/given/1`).
Shipped binaries use the `dist` profile, about 10% faster
([releasing.md](releasing.md)).

## The synthetic datasets

`xeibe_testkit::synth` writes the inputs. A `Synth` is a GML 3.2 feature
collection of `features` features of the layer `app:Building`, each with:

| Property | Example | Column |
|---|---|---|
| `@gml:id` | `b.17` | `Utf8View` |
| `name` | `Building 17`, every seventh `Smith &amp; Sons 14` | `Utf8View` (the entity is expanded) |
| `code` | `00412093` | `Utf8View` (leading zeros) |
| `floors` | `3` | `Int64` |
| `area` | `1873.21` | `Float64` |
| `built` | `1987-05-12` | `Date32` |
| `updated` | `2021-03-04T12:34:56` | `Timestamp(µs)` |
| `active` | `true` | `Boolean` |
| `tag`, 0–3 times | `t12` | `List<Utf8View>` |
| `attr0`… (`extra_properties`) | `v918` | `Utf8View` |
| `geometry` | see below | native GeoArrow, or WKB |

The geometry's `Shape` is a point, a line string of `vertices` positions, a
polygon of `vertices` positions with `holes` rings of 8, a multi-surface of
`parts` such polygons, or a circle in one `gml:ArcString` (a
`CurvePolygon`). Coordinates are EPSG:2180 metres with two decimals, as in
PRG and BDOT10k, and the srsName is `urn:ogc:def:crs:EPSG::2180`, so every
position is swapped from northing–easting. With `other_layer_every`, a short
`app:Road` follows every so many buildings, which a read of `Building`
skips in the splitter.

`crates/xeibe-arrow/tests/it/synth.rs` checks that these documents read
with the intended column types, the right axis order and every layer, so
the benchmarks measure a correct read.

## What is measured

| Crate | Group | Measures | Throughput |
|---|---|---|---|
| `xeibe-core` | `split` | The feature-boundary splitter over 46 MB of polygons, with every layer and with one (the other skipped) | bytes |
| | `decode_and_split` | The same, from gzip, zstd and ISO-8859-2 input through `decoded_reader` | decoded bytes |
| | `reader` | `GmlReader` events over one 2 MB chunk; skipping each subtree unparsed | bytes |
| `xeibe-geom` | `parse` | `GeometryParser` on about 1000 positions of each shape | positions |
| | `write_wkb` | ISO WKB of the same geometries | positions |
| | `linearize` | The circle at GDAL's 4° step and at 0.5° | |
| `xeibe-arrow` | `scan` | A full scan of 12.5 MB (10,000 polygons of 32 vertices, a road after every tenth) on one thread and on all | bytes |
| | `read` | Reads of the same: a sampled schema, a given one, WKB, only the geometry column (projection), converted to EPSG:4258 | bytes |
| | `read_points` | 25,000 points with ten extra properties, 19 MB: attribute-heavy | bytes |
| `xeibe-export` | `parquet` | `ParquetWriter` on 20,000 polygons, native and WKB, with and without the bbox column | bytes in memory |
| | `ipc` | `IpcWriter` on the same | bytes in memory |
| `xeibe-reproject` | `convert_xy` | 10,000 points: TM and its inverse, Web Mercator, LAEA, LCC, PL-1992 to UTM 34N across datums | points |
| | `transform_new` | Choosing the operation for each of those pairs | |
| | `crs` | `Crs` from an EPSG code, PROJJSON, WKT2, WKT1 and a PROJ string; WKT2 and WKT1 written | |
| | `geodesic` | Karney's inverse and direct problems on WGS 84 | problems |

Benchmark names hold no machine-dependent values: the thread counts are
`1` and `all` (`xeibe_core::parallel::default_threads`, one worker per
core), so names compare across machines while the numbers don't.

## Reference numbers

Medians from one run of `cargo bench --bench '*'` on a 4-core cloud
container. A shared machine is noisy; use these for scale only, and compare
runs on one machine.

| Benchmark | Time | Throughput |
|---|---|---|
| `split/one_layer` | 41 ms | 1.0 GiB/s |
| `decode_and_split/gzip` | 103 ms | 424 MiB/s |
| `decode_and_split/zstd` | 107 ms | 407 MiB/s |
| `decode_and_split/iso-8859-2` | 46 ms | 937 MiB/s |
| `reader/events` | 11.4 ms | 176 MiB/s |
| `parse/point` | 1.50 ms | 665 K points/s |
| `parse/polygon_256_holes_4` | 72 µs | 12 M positions/s |
| `write_wkb/linestring_1000` | 4.7 µs | 213 M positions/s |
| `scan/full/1` | 148 ms | 81 MiB/s |
| `scan/full/all` | 53 ms | 226 MiB/s |
| `read/given/1` | 277 ms | 43 MiB/s |
| `read/given/all` | 104 ms | 115 MiB/s |
| `read/to_epsg_4258/1` | 344 ms | 35 MiB/s |
| `read_points/sampled/1` | 595 ms | 31 MiB/s |
| `read_points/sampled/all` | 210 ms | 87 MiB/s |
| `parquet/wkb/bbox` | 96 ms | 157 MiB/s |
| `ipc/wkb` | 2.6 ms | 5.3 GiB/s |
| `convert_xy/4258_to_2180` | 2.0 ms | 5.0 M points/s |
| `convert_xy/2180_to_32634` | 4.1 ms | 2.5 M points/s |
| `transform_new/4258_to_2180` | 280 ns | |
| `transform_new/2180_to_32634` | 5.9 ms | |
| `crs/from_epsg/2180` | 50 µs | |
| `geodesic/inverse` | 13.9 ms | 717 K problems/s |

Choosing an operation is cheap on one datum and takes milliseconds across
datums, where the candidate operations are looked up and ranked; a read
does it once per CRS.

## Adding a benchmark

- Put it in the crate's `benches/<crate>.rs` (one target per crate,
  `harness = false`), and generate its input with `xeibe_testkit::synth`,
  or with a fixed seed like `geographic` in `benches/reproject.rs`. Never
  read `tests/data` or `example_data/`: the benchmarks must run in a fresh
  clone.
- Build the input outside `b.iter`. Where the measured code consumes its
  input, use `iter_batched_ref` so the copy isn't timed.
- Give the group a `Throughput` where there is a natural unit, and keep
  one iteration well under a second; set `sample_size(10)` for the slow
  ones.
- Assert once, outside the measurement, that the code did the work (the
  row count of a read): a benchmark of an error path is fast and useless.
- CI builds the benchmarks (`clippy --all-targets`) but doesn't run them.
