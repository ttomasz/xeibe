# Testing

The test suites are written against the design documents, not against the
current code. Every crate is implemented and **every suite is expected to pass**.
A failing test is a regression, or a documented behaviour the code doesn't
follow yet; either the code or the document is wrong.

```sh
cargo test --workspace            # or: cargo nextest run --workspace
cargo test -p xeibe-geom          # one crate
cargo test -p xeibe-geom parse_curves::   # one module
```

`xeibe-testkit`'s own tests check the test oracles themselves against the
files in `tests/data`.

Benchmarks are separate: `cargo bench`, on synthetic data from
`xeibe_testkit::synth` ([benchmarks.md](benchmarks.md)).

## Layout

| Where | Contents |
|---|---|
| `crates/xeibe-testkit` | Test support: the sample manifest, GDAL reference output, independent WKT and WKB readers, builders for synthetic GML documents, synthetic datasets of any size for the benchmarks. Depends on no other crate of the project, so it stays an independent oracle |
| `crates/<crate>/tests/it/` | One integration-test binary per crate, with a module per topic and a `support` module. Everything goes through the public API |
| `crates/<crate>/tests/it/samples.rs` | The same crate tested against the real samples in `tests/data` |
| `crates/xeibe-py/tests/*.py` | The Python package, run against a built wheel: `smoke.py` (`scan`, `read`, settings, options) and `sedona.py` (`xeibe.sedona` in SedonaDB). Not part of `cargo test`; see below |
| `crates/xeibe-proj-py/tests/` | The pyproj-compatible package, with pytest against a built wheel: CRSs and transformations compared with pyproj (also on the oracle `proj.db`), the container rules, grids from a local HTTP server and through a fake `XMLHttpRequest`, a bare import without numpy. `tests/pyodide_browser/` checks the Pyodide wheel's grid fetch in Chromium. `tests/typing/` holds `assert_type` checks for mypy and pyright, `tests/pyproj_suite/` runs pyproj's own tests, `tests/geopandas_suite/` GeoPandas' through the shim. Not part of `cargo test`; see below |
| `crates/xeibe-wasm/tests/it/` | The web page's session run natively: scan, read, the map's buffers, grids loaded from `tests/data/reprojection/grids`, the table's rows. The JavaScript in `web/` has no automated tests yet: `npm run check` type-checks it |

The Python tests are PEP 723 scripts. Build the wheel first, then run each
with it:

```sh
uvx maturin build --release -m crates/xeibe-py/Cargo.toml -o target/py-wheels
uv run --with target/py-wheels/xeibe-*.whl crates/xeibe-py/tests/smoke.py
uv run --with target/py-wheels/xeibe-*.whl crates/xeibe-py/tests/sedona.py
```

`xeibe-proj-py`'s tests run with pytest. pyproj 3.8.0, which they compare
with, needs Python 3.12 or newer. The type checks run from the crate's
directory, where `pyproject.toml` configures them:

```sh
uvx maturin build --release -m crates/xeibe-proj-py/Cargo.toml -o target/proj-py-wheels
uv run --no-project --isolated --python 3.12 --with target/proj-py-wheels/xeibe_proj-*.whl \
  --with pytest --with numpy --with pyproj==3.8.0 pytest crates/xeibe-proj-py/tests
cd crates/xeibe-proj-py
uv run --no-project --isolated --python 3.12 --with ../../target/proj-py-wheels/xeibe_proj-*.whl --with mypy --with numpy mypy
uv run --no-project --isolated --python 3.12 --with ../../target/proj-py-wheels/xeibe_proj-*.whl --with pyright --with numpy pyright
uv run --no-project --isolated --python 3.12 --with ../../target/proj-py-wheels/xeibe_proj-*.whl --with mypy python -m mypy.stubtest xeibe_proj._native
```

`test_oracle.py` (marked `oracle`, about a minute; `-m 'not oracle'` skips
it) compares with pyproj on `target/proj-oracle/proj.db` and is skipped
without it. pyproj's own test suite runs against a built wheel with
`crates/xeibe-proj-py/tests/pyproj_suite/run.py` (`--update` to record new
passes in its `pass-list.txt`); see [pyproj-compat.md](pyproj-compat.md#testing).
GeoPandas' test suite runs with pyproj and through `install_as_pyproj()`
with `crates/xeibe-proj-py/tests/geopandas_suite/run.py`, which takes the
same `--wheel` and `--update` and runs pytest in the environment
`pytest_env.py` declares, which uv keeps in its cache
([pyproj-phase3.md](pyproj-phase3.md#geopandas)). Nothing it needs between
runs is kept under `target/`: CI's rust-cache deletes the files of
whatever there isn't Cargo's.
`.github/workflows/proj-py.yml` runs all of this on changes to the package,
`xeibe-crs` or `xeibe-reproject`. It builds and tests the wheel on Linux,
macOS and Windows, and checks a bare install on Python 3.11 with no numpy.
The type checks, pyproj's suite and GeoPandas' run on Linux only. The
oracle tests skip there.

Its `pyodide` job builds the Pyodide wheel and runs the same tests in a
Pyodide venv in Node ([pyproj-phase3.md](pyproj-phase3.md#pyodide)).
Locally, with Node, Rust's `wasm32-unknown-emscripten` target and
pyodide-build (its host Python is 3.14):

```sh
uv tool install --python 3.14 pyodide-cli==0.5.0 --with pyodide-build==0.39.3
pyodide xbuildenv install 314.0.7 && pyodide xbuildenv install-emscripten
(cd crates/xeibe-proj-py && source "$(pyodide config get emsdk_dir)/emsdk_env.sh" \
  && pyodide build -o ../../target/pyodide-wheels)
pyodide venv target/pyodide-venv
target/pyodide-venv/bin/pip install target/pyodide-wheels/xeibe_proj-*.whl pytest numpy pyproj
target/pyodide-venv/bin/python -m pytest crates/xeibe-proj-py/tests -p no:cacheprovider
crates/xeibe-proj-py/tests/pyodide_browser/run.py \
  --wheel target/pyodide-wheels/xeibe_proj-*.whl --pyodide "$(pyodide config get dist_dir)"
```

The last command checks the grid fetch through the browser's
`XMLHttpRequest` in Playwright's Chromium, which Node can't.

Each test module names the section of the design docs it comes from. A test
that deviates from a document deliberately says why in a comment (see the GDAL
cases below).

## Two kinds of test

**Synthetic tests** build a GML fragment in the test itself
(`xeibe_testkit::gml`) and assert on the result. They are the specification in
executable form: one behaviour per test, with the documented rule in the name.

**Sample-based tests** run against `tests/data/samples/**`, 18 files excerpted
from real data (see `tests/data/README.md`). Their expected values come from
the generated manifest `samples.json`:

- **feature counts and layer names** come from GDAL's `ogrinfo -al` output
  (`*.gdal.txt`), which is reliable for those;
- **the axis order** comes from `axis.expected_first_xy`, which
  `scripts/corpus/axis_evidence.py` verified against the CRS area of use and
  regions the features are known to lie in. **GDAL reads 4 of the 21 samples in
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

Anything else that differs from GDAL fails the test, listing every case. GDAL's
GML 3.3 snippets use the `gmlce` prefix without declaring it, so the harness wraps
them in a document that declares it.

`crates/xeibe-geom/tests/it/gml_madness.rs` covers the 25 ways of writing one
square listed in Even Rouault's
["GML madness"](https://erouault.blogspot.com/2014/04/gml-madness.html). The
snippets are our own encodings of each structure, with one test per variant,
numbered as in the post. `v24` is a GML 3.3 `SimpleRectangle`.

`crates/xeibe-geom/tests/it/gml33.rs` covers the rest of GML 3.3. Most of its tests
compare a compact encoding with the GML 3.2 geometry the spec says it abbreviates.

## Conventions

- Test names are sentences: `an_unclosed_ring_is_closed_with_a_warning`.
- Prefer one behaviour per test; loop over the samples only where the list
  comes from the manifest, and name the sample in the failure message.
- Reads in tests ask for `GeomEncoding::Wkb` when they compare geometry, so
  native and WKB columns don't need different assertions.
- Files a test writes go to `env!("CARGO_TARGET_TMPDIR")`, never to `/tmp`.
