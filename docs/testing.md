# Testing

The test suites are written against the design documents, not against the
current code: the crates are still skeletons whose bodies are `todo!()`, so
**almost every test fails today**. A failing test is a feature that is not
implemented yet; the count that passes is the progress bar.

```sh
cargo test --workspace            # or: cargo nextest run --workspace
cargo test -p xeibe-geom          # one crate
cargo test -p xeibe-geom parse_curves::   # one module
```

Only `xeibe-testkit`'s own tests must pass at all times: they check the test
oracles themselves against the files in `tests/data`.

## Layout

| Where | Contents |
|---|---|
| `crates/xeibe-testkit` | Test support: the sample manifest, GDAL reference output, independent WKT and WKB readers, builders for synthetic GML documents. Depends on no other crate of the project, so it keeps working while they are skeletons |
| `crates/<crate>/tests/it/` | One integration-test binary per crate, with a module per topic and a `support` module. Everything goes through the public API |
| `crates/<crate>/tests/it/samples.rs` | The same crate tested against the real samples in `tests/data` |

Each test module names the section of the design docs it comes from. A test
that deviates from a document deliberately says why in a comment (see the GDAL
cases below).

## Two kinds of test

**Synthetic tests** build a GML fragment in the test itself
(`xeibe_testkit::gml`) and assert on the result. They are the specification in
executable form: one behaviour per test, with the documented rule in the name.

**Sample-based tests** run against `tests/data/samples/**`, 17 files excerpted
from real data (see `tests/data/README.md`). Their expected values come from
the generated manifest `samples.json`:

- **feature counts and layer names** come from GDAL's `ogrinfo -al` output
  (`*.gdal.txt`), which is reliable for those;
- **the axis order** comes from `axis.expected_first_xy`, which
  `scripts/corpus/axis_evidence.py` verified against the CRS area of use and
  regions the features are known to lie in. **GDAL reads 4 of the 17 samples in
  the wrong order**, so tests never take the axis order from GDAL
  (`geometry.md`, "Observed in real services").

## Comparing geometry

`xeibe_testkit::wkt::G` is a small geometry value with a WKT reader, so
expected geometries are written as WKT strings. `xeibe_testkit::wkb` decodes
ISO WKB independently of the library, which is how geometry columns and the WKB
writer are checked.

Two comparisons exist:

- `assert_wkt` compares the structure exactly, which pins down *which* type the
  parser produces (a `CircularString`, not a one-part `CompoundCurve`);
- `assert_wkt_canonical` compares canonical forms, which merge adjacent parts
  of a compound curve and simplify curve-free curve types. It is used against
  GDAL, where those choices differ from ours by design.

## The GDAL geometry cases

`crates/xeibe-geom/tests/it/gdal_cases.rs` runs the 292 GML snippets of GDAL's
own test suite (`tests/data/gdal/gml_geometry_cases.jsonl`, MIT) and compares
with GDAL 3.13.3's result, because the docs adopt GDAL's behaviour wherever the
specs are silent. Deliberate differences are listed in the `OVERRIDES` table in
that file with their reason, for example:

- an empty element (`<gml:Point/>`) is an empty geometry, not a null one;
- a gap between curve members is a warning, not an error;
- solids, polyhedral and triangulated surfaces are out of scope, so they are
  geometry errors (`OnFeatureError`).

Anything else that differs from GDAL fails the test, listing every case.

## Conventions

- Test names are sentences: `an_unclosed_ring_is_a_warning_and_is_kept_as_written`.
- Prefer one behaviour per test; loop over the samples only where the list
  comes from the manifest, and name the sample in the failure message.
- Reads in tests ask for `GeomEncoding::Wkb` when they compare geometry, so
  native and WKB columns don't need different assertions.
- Files a test writes go to `env!("CARGO_TARGET_TMPDIR")`, never to `/tmp`.
