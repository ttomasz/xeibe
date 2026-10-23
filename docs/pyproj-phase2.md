# pyproj compatibility, phase 2: geodesics, `Proj` and the rest

> **Status: designed, 2026-10-23; step 1 of 8 implemented (the geodesic
> port in Rust, see [Order of work](#order-of-work)).** This is the rest
> of phase 2 in [pyproj-compat.md](pyproj-compat.md#phases), after the WKT
> part ([crs-wkt.md](crs-wkt.md)). Facts about PROJ and pyproj were checked
> against PROJ 9.8.1 through pyproj 3.8.0.

Phase 2 is "interchange": what WKT-based libraries and geodesic work need.
The WKT part is done. This document covers the rest of phase 2's rows in
the [compatibility table](pyproj-compat.md#compatibility-table):

- `Geod`, complete, and `CRS.get_geod`;
- the legacy `Proj`, for EPSG and WKT input;
- `CRS.to_2d`, `CRS.to_3d` and `CRS.get_non_deprecated`;
- `pyproj.database.get_units_map`;
- `Transformer.from_crs(area_of_interest=, accuracy=, only_best=)`, and
  the same on `TransformerGroup`;
- `Transformer.to_json`, `to_json_dict` and `to_wkt`;
- `TransformerGroup.download_grids`.

## Decisions

| # | Decision | Why |
|---|---|---|
| 1 | **The whole of `geodesic.c` is ported to Rust, as a public module `xeibe_reproject::geodesic`**, replacing the reduced port the Azimuthal Equidistant uses. Every output of the C library is kept: both azimuths, the reduced length, the geodesic scales, the area, geodesic lines and polygons. | pyproj's `Geod` is a thin layer over `geodesic.c`, so a faithful port gives pyproj's numbers, to the last bit where the platform's `libm` is the same. The reduced port is already a line-by-line translation, so finishing it costs less than a new dependency. `geographiclib-rs` ports the Python version of GeographicLib instead, which rounds differently. |
| 2 | **PROJ's ellipsoid and prime meridian tables** (`ellps.cpp`, `datums.cpp`, MIT) **go into `xeibe-reproject`**, next to the geodesics. | `Geod(ellps=…)` and `get_ellps_map` need the first, and `get_prime_meridians_map` the second. Phase 3's PROJ strings (`+ellps`, `+pm`) need both. |
| 3 | **`Geod` is pyproj's `geod.py` ported to Python** (MIT, as `_containers.py` already is), over native functions that take the same writable float64 buffers as `Transform.convert`. | pyproj's own Python layer decides the argument handling, the containers and the warnings. Copying it gives the same behaviour, and only the loops are native. |
| 4 | **`Geod`'s `inplace=` is treated as `Transformer.transform`'s is**: it isn't in the signatures, and passing it raises `NotImplementedError`. | One rule for the whole package ([pyproj-compat.md](pyproj-compat.md#transformer-and-transformergroup)). |
| 5 | **`Proj(crs)` is the CRS's projection on its own ellipsoid, with no datum change**: longitude from Greenwich and latitude in degrees, in and out, and the output in the CRS's own unit and axis directions, easting first. This is what pyproj gets from the CRS's PROJ string. | pyproj builds `Proj` from `crs.to_proj4()`. For EPSG and WKT input, that string only says the projection, the ellipsoid, the prime meridian and the unit. Checked against pyproj (see [`Proj`](#proj)). |
| 6 | **`Proj`'s parameters keep EPSG's full precision**, where pyproj's PROJ string rounds them to 15 significant digits. | This is more precise, and differs from pyproj by about 10⁻⁶ of the unit at most (EPSG:2249: 2·10⁻⁶ US feet). The tests allow for it. |
| 7 | **`Proj` from PROJ parameters (`Proj(proj="utm", zone=33)`), `definition_string` and `to_latlong_def` stay phase 3**, as PROJ strings. | They are PROJ strings in and out. |
| 8 | **`to_2d` and `to_3d` follow PROJ's `demoteTo2D` and `promoteTo3D`**, including their lookup of the EPSG CRS with the same datum and name (EPSG:4326 ↔ EPSG:4979). | pyproj users get EPSG codes back from these (`to_3d().to_epsg()` is 4979). |
| 9 | **`get_non_deprecated` and `get_units_map` come from new generated tables in `xeibe-crs`**: EPSG's replacements of deprecated CRSs, and EPSG's units with their conversion factors. | Neither is in the generated data yet. PROJ's `build_db.py` takes both straight from EPSG. |
| 10 | **`get_units_map` lists EPSG's units and PROJ's own five, not Esri's.** | As for CRSs, other authorities are out ([pyproj-compat.md](pyproj-compat.md#crs)). PROJ's five (`PROJ:DM`, `IN`, `IND_CH`, `US_IN`, `US_YD`) and PROJ's short names (`us-ft`) are needed for phase 3's `+units` anyway. |
| 11 | **`area_of_interest`, `accuracy` and `only_best` are applied in `xeibe-reproject`'s selection**, as PROJ applies them in `proj_create_crs_to_crs`. When nothing is left, construction fails as it does today; PROJ would fall back to a ballpark operation. | Selection is already PROJ's, in Rust ([reprojection.md](reprojection.md)). There is no ballpark ([pyproj-compat.md](pyproj-compat.md#transformer-and-transformergroup), `allow_ballpark`). |
| 12 | **`Transformer.to_json` and `to_wkt` describe the operation in PROJJSON and WKT2 2019, built from EPSG's data**, and return `None` when the transformer has several candidates. | PROJ returns `None` until `proj_trans` has chosen one ("unavailable until proj_trans is called"). Equivalent to PROJ's output, not identical, as for CRSs ([crs-wkt.md](crs-wkt.md#decisions), decision 7). |
| 13 | **`download_grids` downloads whole grid files from the CDN** into the given directory or the first data directory, skipping those already present. | That is what pyproj does. Range requests are for the default network mode ([pyproj-compat.md](pyproj-compat.md#grids)). |

## Geodesics

### The Rust port

`xeibe_reproject::geodesic` mirrors `geodesic.h`:

| C | Rust |
|---|---|
| `struct geod_geodesic`, `geod_init` | `Geodesic::new(a, f)` |
| `geod_direct`, `geod_gendirect` (`GEOD_ARCMODE`, `GEOD_LONG_UNROLL`) | `Geodesic::direct`, `Geodesic::gen_direct` |
| `geod_inverse`, `geod_geninverse` | `Geodesic::inverse`, `Geodesic::gen_inverse` |
| `struct geod_geodesicline`, `geod_lineinit`, `geod_directline`, `geod_inverseline`, `geod_position`, `geod_genposition`, `geod_setdistance` | `GeodesicLine` |
| `struct geod_polygon`, `geod_polygon_*`, `geod_polygonarea` | `Polygon`, `Geodesic::polygon_area` |

The `outmask` and `caps` bit sets become a `Mask` bit set of the same
values. The general functions return a struct of every output, and
quantities that weren't asked for are NaN, as in C.

The tests are:

- PROJ's own `src/tests/geodtest.c` and `geodsigntest.c` (MIT), ported
  case by case;
- vectors from pyproj's `Geod`, written by a new
  `scripts/gen_geodesic_vectors.py`, of `fwd`, `inv`, the intermediate
  points and polygon areas;
- the Azimuthal Equidistant's existing vectors, which must not move.

Each vector covers random lines on WGS 84, plus a set of hard cases:

- antipodal and nearly antipodal points;
- meridians, the equator and the poles;
- a sphere, and flattenings of ±1/150 and ±1/50 (`Geod(a=, f=)` takes
  any `f`).

The committed vectors are compared bit for bit on Linux with glibc, and
within 4 ulps elsewhere, where `sin`, `cos` and `atan2` come from another
libm. All 3,324 committed values match exactly. Of the 441,495 values from
`--all`, 21 are 1 or 2 ulps off. They come from 9 nearly antipodal
lines, where the starting guess (`Astroid`) takes a cube root: Rust's
`cbrt` is `compiler_builtins`' (musl's) rather than glibc's, and the two
round differently now and then. glibc's is LGPL, so it isn't ported; the
full set is checked within 4 ulps.

Two things came out of the comparison:

- **`serde_json`'s default float parser can be one ulp off.** At first
  741 of 3,324 values differed by up to 9 ulps, because the test read its
  inputs one ulp away from pyproj's. The workspace now turns on
  `float_roundtrip`, which parses exactly, as PROJ's `strtod` does. That
  also applies to every PROJJSON parameter the library reads.
- **The reduced port applied `AngRound` in `sincosdx`**, which C's
  doesn't, and its `remainder` was approximate for large arguments. The
  full port reduces exactly (`remquo` on Rust's exact `%`) and follows C
  in both.

### `Geod` in `xeibe_proj`

`geod.py` follows pyproj's module:

- **The class:** `Geod(initstring=None, **kwargs)` with `ellps`, `a`, `b`,
  `f`, `rf`, `e` and `es`, and the attributes `a`, `b`, `f`, `es`,
  `sphere` and `initstring`.
- **Methods:** `fwd`, `inv`, `npts`, `inv_intermediate`,
  `fwd_intermediate` (`GeodIntermediateFlag`, `GeodIntermediateReturn`),
  `line_length`, `line_lengths` and `polygon_area_perimeter`.
- **Geometries:** `geometry_length` and `geometry_area_perimeter` take
  anything with shapely's `xy`, `exterior`, `interiors` and `geoms`;
  shapely is never imported.
- **Equality and pickling:** `repr`, equality by `repr` and pickling by
  `initstring`.
- **Module level:** `reverse_azimuth`, `pj_ellps`, `get_ellps_map`,
  `get_prime_meridians_map` and `geodesic_version_str` (`"2.2.0"`).
- **Errors:** `GeodError` for arrays of different lengths, and for the
  intermediate functions' argument errors, with pyproj's messages.
- **`CRS.get_geod()`:** `Geod(a=, rf=, b=)` from the CRS's ellipsoid, or
  `None` without one, as in pyproj.

The native side has one function per loop (`fwd`, `inv`, `intermediate`,
`line_length`, `polygon_area`). Each runs without the GIL on the buffers
from `_containers.to_buffer`. The tests compare every method with pyproj's,
and add pyproj's `test_geod.py` to the suite's pass list.

## `Proj`

pyproj's `Proj(crs)` is the transformer for `crs.to_proj4()`. Measured
with pyproj 3.8.0, it behaves like this:

| CRS | `Proj(crs)(lon, lat)` |
|---|---|
| Projected (EPSG:2180, 32633, 3035, 5514, 32661) | Equal to `Transformer.from_crs(crs.geodetic_crs, crs, always_xy=True)`: easting first, whatever the CRS's axis order |
| Projected with westing, southing (EPSG:2053, `+axis=wsu`) | The same: the directions stay, only the order is normalised |
| Projected on a non-Greenwich meridian (EPSG:27572, NTF (Paris) in grads) | Longitude from **Greenwich** in degrees, where its base CRS takes grads from Paris |
| Projected in US feet (EPSG:2249) | Feet, 2·10⁻⁶ ft from ours because the PROJ string rounds `x_0` and `lat_1` |
| Geographic (EPSG:4326) | The input, unchanged |
| Geographic on a non-Greenwich meridian (EPSG:4807) | Longitude minus the meridian's, in degrees (not grads) |
| Geocentric (EPSG:4978) | X and Y with the height taken as 0 |
| Compound (EPSG:9707) | Its horizontal part |
| Vertical (EPSG:5773) | `ProjError` ("Invalid projection") |

So `Proj(crs)` is built in Rust as a `Transform` with these parts:

- **Source:** the CRS's geodetic CRS read as longitude from Greenwich and
  latitude, in degrees.
- **Target:** the horizontal part of the CRS, easting first.
- **Operation:** only the conversion, with no datum step.

For a geographic CRS it is the prime meridian's shift. `inverse=True`
runs it backwards. `Proj` subclasses `Transformer`, as in pyproj, so
`transform`, `itransform` and `transform_bounds` also work.

`Proj` has these attributes and methods:

- `crs` and `srs`. pyproj's `srs` is the PROJ string, so until phase 3
  ours is the CRS's `srs`.
- `to_latlong()`, which is `Proj(crs.geodetic_crs)`.
- Pickling.
- `get_factors` stays out. `definition_string` and `to_latlong_def` raise
  "phase 3".
- `Proj(crs, preserve_units=False)` gives metres for a CRS in feet, as
  pyproj does.

The tests run `Proj(code)` forwards and backwards on points inside each
EPSG CRS's area of use, against pyproj on the oracle database.

## `to_2d`, `to_3d`, `get_non_deprecated`

PROJ's rules, checked with pyproj:

| Input | `to_3d()` | `to_2d()` |
|---|---|---|
| Geographic 2D | Geographic 3D with an ellipsoidal height (`h`, up, metre): EPSG's own when one has the same datum and name (4326 → 4979, 4258 → 4937), else without an id (4807) | Itself |
| Geographic 3D | Itself | Geographic 2D, EPSG's own when there is one (4979 → 4326) |
| Projected | Projected 3D on the promoted base, with `h` added, without an id | Itself, or the 2D CRS for a projected 3D CRS |
| Compound | Itself | Its horizontal part, demoted (9707 → 4326) |
| Bound | The Bound CRS of the promoted or demoted source | |
| Geocentric, vertical, engineering | Itself | Itself |

`name=` names a new CRS only when there is no EPSG one. PROJ ignores it
otherwise: `CRS(4326).to_3d(name="X")` is named "WGS 84". The 2D–3D
lookup uses the `CrsRecord` table: same datum, same name, the other
dimension.

`get_non_deprecated()` lists the replacements EPSG gives for a deprecated
CRS (`epsg_deprecation`, as PROJ's `build_db.py` fills its `deprecation`
table), as `CRS` objects. It gives `[]` for a CRS that isn't deprecated or
has no replacement. `gen_crs_tables.py` writes the table into `xeibe-crs`.

Both are tested over every EPSG CRS against pyproj on the oracle
database. The comparison covers type, name, id and axes.

## `get_units_map`

`get_units_map(auth_name=None, category=None, allow_deprecated=False)`
returns `{name: Unit(auth_name, code, name, category, conv_factor,
proj_short_name, deprecated)}`. Its data comes from these places:

- **The units:** EPSG's `epsg_unitofmeasure`, with `conv_factor` as
  `factor_b / factor_c`, as `build_db.py` computes it. `gen_crs_tables.py`
  writes it into `xeibe-crs`.
- **`category`:** PROJ's mapping from the unit type and name
  (`linear`, `linear_per_time`, `angular`, `scale`, `time`, …;
  `AuthorityFactory::getUnitList`).
- **`proj_short_name`, and PROJ's own five units:** from PROJ's
  `data/sql` (MIT), as a table in `xeibe-reproject`.

Tested against `get_units_map` on the oracle database, without its ESRI
entries.

## Selection: `area_of_interest`, `accuracy`, `only_best`

PROJ applies these while it builds the candidates in
`proj_create_crs_to_crs`:

- **`area_of_interest`** keeps the candidates whose area of use
  intersects the given box. A box across the antimeridian has west > east.
- **`accuracy`** keeps the candidates whose accuracy is known and no worse
  than the value, in metres.
- **`only_best`** makes a coordinate that the best candidate can't convert,
  because a grid is missing, an error, instead of trying the next one.

`xeibe-reproject`'s selection gains an options struct with the three. As
everywhere in this package, a selection with nothing left fails where PROJ
would ballpark. `TransformerGroup(area_of_interest=, accuracy=)` filters
its list in the same way. The tests extend
`gen_transformation_vectors.py` with each option on a sample of CRS pairs.
They compare the candidate lists and the converted points with PROJ's.

## `Transformer.to_json`, `to_json_dict`, `to_wkt`

When the transformer has one operation, these describe it in PROJ's
shapes:

- a `Conversion` between a projected CRS and its base;
- a `Transformation` for one EPSG transformation;
- a `ConcatenatedOperation` with `steps` for a route
  (`Inverse of ETRF2000-PL to WGS 84 (1) + Poland CS92`);
- each with `source_crs`, `target_crs`, the method, the parameters,
  accuracy, area of use and EPSG id.

They come from `xeibe_crs::operations_json` and the CRSs' PROJJSON.
`to_wkt` writes the same through the WKT2 writer. That needs
`CONVERSION` and `CONCATENATEDOPERATION` beside the `COORDINATEOPERATION`
it already writes. With several candidates, all three return `None`, as
pyproj's do. The tests compare with pyproj without ids, as for Bound CRSs
in [crs-wkt.md](crs-wkt.md).

## `download_grids`

`TransformerGroup.download_grids(directory=None, open_license=True,
verbose=False)` behaves as in pyproj:

- It fetches every grid of every operation in the group, as a whole
  file from `cdn.proj.org`.
- The files go into `directory`, or into the first directory of
  `get_data_dir()` (the user data directory when none is set).
- Files already present are skipped.
- `open_license=True` skips the grids without an open licence. That
  needs PROJ's `open_license` column in the generated grid catalog.
- `verbose` prints each file.

Downloads are logged as in [pyproj-compat.md](pyproj-compat.md#grids).
The tests use the local HTTP server the grid tests already use.

## Order of work

1. **The geodesic port in Rust**: the rest of `geodesic.c`, the
   Azimuthal Equidistant moved onto it, PROJ's tests,
   `gen_geodesic_vectors.py`, and the ellipsoid and prime meridian
   tables. It is gated on PROJ's tests and pyproj's numbers. **Done:**
   `xeibe_reproject::geodesic` and `xeibe_reproject::proj_tables`.
2. **`Geod` in `xeibe_proj`**, with `get_geod`, `pj_ellps`,
   `get_ellps_map` and `get_prime_meridians_map`, gated on pyproj and its
   `test_geod.py`.
3. **`Proj`**, gated on pyproj over every EPSG CRS.
4. **`to_2d`, `to_3d`, `get_non_deprecated`**, gated on pyproj over every
   EPSG CRS.
5. **`get_units_map`**, gated on pyproj.
6. **`area_of_interest`, `accuracy`, `only_best`**, gated on PROJ's
   candidate lists.
7. **`Transformer.to_json`, `to_json_dict` and `to_wkt`**, gated on
   pyproj's output without ids.
8. **`download_grids`**, gated on the local grid server.
