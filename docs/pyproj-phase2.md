# pyproj compatibility, phase 2: geodesics, `Proj` and the rest

> **Status: designed, 2026-10-23; implemented, 2026-10-24 (the geodesic
> port in Rust, `Geod`, `Proj`, `to_2d`/`to_3d`/`get_non_deprecated`,
> `get_units_map`, the selection's options, `Transformer.to_json` and
> `to_wkt`, and `download_grids`, see [Order of work](#order-of-work)).**
> This is the rest
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
| 5 | **`Proj(crs)` is the CRS's projection on its own ellipsoid, with no datum change**: longitude from Greenwich and latitude in degrees in, and out the projection's easting and northing in the CRS's own unit (the CRS's axes for the three methods whose PROJ string has an axis order). This is what pyproj gets from the CRS's PROJ string. | pyproj builds `Proj` from `crs.to_proj4()`. For EPSG and WKT input, that string only says the projection, the ellipsoid, the prime meridian and the unit. Checked against pyproj (see [`Proj`](#proj)). |
| 6 | **`Proj` follows EPSG, not pyproj's PROJ string**: its full-precision parameters, prime meridians and ellipsoids, and the CRS's own datum. | It is what PROJ's own conversion of the CRS gives. pyproj's string rounds parameters, names PROJ's prime meridians and ellipsoids, and with `+datum=NAD83` reads WGS 84 in: up to 0.35 mm, and kilometres for EASE-Grid (see [Validation](#validation)). |
| 7 | **`Proj` from PROJ parameters (`Proj(proj="utm", zone=33)`), `definition_string` and `to_latlong_def` stay phase 3**, as PROJ strings. | They are PROJ strings in and out. |
| 8 | **`to_2d` and `to_3d` follow PROJ's `demoteTo2D` and `promoteTo3D`**, including their lookup of the EPSG CRS with the same datum and name (EPSG:4326 ↔ EPSG:4979). | pyproj users get EPSG codes back from these (`to_3d().to_epsg()` is 4979). |
| 9 | **`get_non_deprecated` and `get_units_map` come from new generated tables in `xeibe-crs`**: EPSG's replacements of deprecated CRSs, and EPSG's units with their conversion factors. | Neither is in the generated data yet. PROJ's `build_db.py` takes both straight from EPSG. |
| 10 | **`get_units_map` lists EPSG's units and PROJ's own five, not Esri's.** | As for CRSs, other authorities are out ([pyproj-compat.md](pyproj-compat.md#crs)). PROJ's five (`PROJ:DM`, `IN`, `IND_CH`, `US_IN`, `US_YD`) and PROJ's short names (`us-ft`) are needed for phase 3's `+units` anyway. |
| 11 | **`area_of_interest`, `accuracy` and `only_best` are applied in `xeibe-reproject`'s selection**, as PROJ applies them in `proj_create_crs_to_crs`. When nothing is left, construction fails as it does today; PROJ would fall back to a ballpark operation. | Selection is already PROJ's, in Rust ([reprojection.md](reprojection.md)). There is no ballpark ([pyproj-compat.md](pyproj-compat.md#transformer-and-transformergroup), `allow_ballpark`). |
| 12 | **`Transformer.to_json` and `to_wkt` describe the operation in PROJJSON and WKT2 2019, built from EPSG's data**, and return `None` when the transformer has several candidates. | PROJ returns `None` until `proj_trans` has chosen one ("unavailable until proj_trans is called"). Equivalent to PROJ's output, not identical, as for CRSs ([crs-wkt.md](crs-wkt.md#decisions), decision 7). |
| 13 | **`download_grids` downloads whole grid files from the CDN** into the given directory or the user data directory, skipping those already present. | That is what pyproj does. Range requests are for the default network mode ([pyproj-compat.md](pyproj-compat.md#grids)). |

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

The native side is a `Geodesic` class with one method per loop (`fwd`,
`inv`, `line_length`, `polygon_area_perimeter`) and geodesic lines whose
`fill` writes the intermediate points. Each runs without the GIL on the
buffers from `_containers.to_buffer`. The point count and spacing of
`inv_intermediate` are worked out in Python, as pyproj does, with C's
rounding and division. The tests compare every method with pyproj's,
and add pyproj's `test_geod.py` to the suite's pass list.

## `Proj`

pyproj's `Proj(crs)` is the transformer for `crs.to_proj4()`. Measured
with pyproj 3.8.0, it behaves like this:

| CRS | `Proj(crs)(lon, lat)` |
|---|---|
| Projected (EPSG:2180, 32633, 3035, 5514) | The projection's easting and northing in the CRS's unit, whatever the CRS's axis order: equal to `Transformer.from_crs(crs.geodetic_crs, crs, always_xy=True)` |
| Projected with polar axes (EPSG:32661, UPS North (N,E): "south along 180°E", "south along 90°E") | Easting, northing too: the PROJ string has no axis order, so the axes are ignored |
| Transverse Mercator (South Orientated), Krovak, Krovak Modified (EPSG:2053, `+axis=wsu`) | The CRS's axes: PROJ's mapping of these three methods carries an axis order (`+axis=wsu`, `+axis=swu`, `+czech`) that matches their CRSs |
| Projected on a non-Greenwich meridian (EPSG:27572, NTF (Paris) in grads) | Longitude from **Greenwich** in degrees, where its base CRS takes grads from Paris |
| Projected in US feet (EPSG:2249) | Feet |
| Geographic (EPSG:4326) | The input, its longitude reduced to [−180°, 180°] (190° gives −170°) |
| Geographic on a non-Greenwich meridian (EPSG:4807) | Longitude minus the meridian's, in degrees (not grads) |
| Geocentric (EPSG:4978) | X and Y with the height taken as 0 |
| Compound (EPSG:9707) | Its horizontal part |
| Vertical (EPSG:5773) | `ProjError` ("Invalid projection") |

`radians=True` follows PROJ's angular input and output: the longitude and
latitude side, and a geographic CRS's side, are in radians. Input is
checked and reduced as PROJ's `fwd_prepare` does it: a latitude beyond the
poles or a longitude beyond ±10 radians is an error, the longitude is
reduced, then counted from the prime meridian.

### Implementation

- **Rust:** `xeibe_reproject::Projection` is the CRS's projection, with
  no datum step. It goes forwards from degrees to the CRS, or with
  `inverse()` backwards, and `in_metres()` gives pyproj's
  `preserve_units=False`. Bounding boxes go through the same port of
  PROJ's `proj_trans_bounds` as `Transform::convert_bounds`, now generic
  over both.
- **Native:** a `Projection` class with the methods `Transformer` calls
  on a native transform: no operations and no grids.
- **Python:** `Proj` subclasses `Transformer`, as in pyproj, so
  `transform`, `itransform` and `transform_bounds` also work.
  `Transformer._angular` decides which side radians apply to, and `Proj`
  overrides it.

`Proj` has these attributes and methods:

- `crs` and `srs`. pyproj's `srs` is the PROJ string, so until phase 3
  ours is the CRS's `srs`.
- `source_crs` and `target_crs`, which are `None`, as for pyproj's
  pipeline. `name` is EPSG's method name (`Transverse Mercator`), where
  pyproj gives PROJ's (`tmerc`).
- `to_latlong()`, which is `Proj(crs.geodetic_crs)`.
- Equality, hashing and pickling, by CRS and `preserve_units`.
- `get_factors` stays out. `definition_string` and `to_latlong_def` raise
  "phase 3".
- `Proj(crs, preserve_units=False)` gives metres for a CRS in feet, as
  pyproj does. The CRS stays the EPSG one; pyproj's becomes a PROJ
  string's.

### Validation

`test_oracle.py` projects the middle of each EPSG CRS's area of use with
`Proj(code)` and back from pyproj's result, against pyproj on the oracle
database. All 8,000 current CRSs that pyproj's `Proj` handles match within
1 mm in the CRS's unit and 10⁻⁸°, but two. In 297 more, pyproj's `Proj`
fails, mostly because PROJ can't write the CRS as a PROJ string, and ours
works.

The differences all come from pyproj's PROJ strings, and ours follow EPSG,
as PROJ's own conversion of the CRS does:

- **NAD83, 472 CRSs, 1.05·10⁻⁴ m.** `+datum=NAD83` implies
  `+towgs84=0,0,0`, so PROJ reads the input as WGS 84 and moves it
  onto GRS 1980 through geocentric coordinates. The two ellipsoids'
  semi-minor axes differ by 0.1 mm.
- **Paris meridian, 29 CRSs, up to 3.5·10⁻⁴ m.** `+pm=paris` is PROJ's
  2°20′14.025″, 3.3·10⁻⁹° from EPSG's 2.5969213 grad.
- **Named ellipsoids, 9 CRSs, up to 1.2·10⁻⁴ m.** PROJ writes
  Schwarzeck's Bessel Namibia (GLM) as `+ellps=bess_nam`, whose
  semi-major axis is rounded to the millimetre.
- **Rounded parameters, below 10⁻⁶ of the unit.** The PROJ string has
  15 significant digits.
- **EASE-Grid North and South (EPSG:3973, 3974), kilometres.** The PROJ
  string has `+R_A`, the authalic sphere, for EPSG's spherical LAEA;
  PROJ's own conversion of these CRSs doesn't. These two are listed in the
  test.

## `to_2d`, `to_3d`, `get_non_deprecated`

PROJ's rules (`promoteTo3D`, `demoteTo2D` in `crs.cpp`), checked with
pyproj:

| Input | `to_3d()` | `to_2d()` |
|---|---|---|
| Geographic 2D | Geographic 3D with an ellipsoidal height (`h`, up, metre): EPSG's own when the first EPSG geographic 3D CRS of the same name is on the same datum with the same two axes (4326 → 4979, 4258 → 4937), else a new one (4807) | Itself |
| Geographic 3D | Itself | Geographic 2D, EPSG's own on the same rule (4979 → 4326), else a new one |
| Projected | Projected 3D on the promoted base (2180's on 4937), with `h` added, without an id | Itself, or for a projected 3D CRS the 2D CRS on the demoted base |
| Compound | Itself | Its first component, as EPSG defines it (9707 → 4326) |
| Bound | The Bound CRS of the promoted source; with a TOWGS84 transformation, of the promoted hub too | The same, demoted |
| Geocentric, vertical, engineering | Itself | Itself |

The twin is looked up only for a CRS with one EPSG id. A new 3D CRS keeps
the CRS's areas of use, each with the scope "unknown", and gets the remark
"Promoted to 3D from EPSG:<code>", followed by EPSG's remarks; it has no
id. A new 2D CRS keeps only its name, datum and axes. `name=` names a new
CRS only: `CRS(4326).to_3d(name="X")` is EPSG:4979, named "WGS 84".

`get_non_deprecated()` lists the replacements EPSG gives for a deprecated
CRS, as `CRS` objects. They come from `epsg_deprecation`, as PROJ's
`build_db.py` fills its `deprecation` table, through
`xeibe_crs::replacements`. It gives `[]` for a CRS that isn't deprecated or
has no replacement. The table also has EPSG's 295 old 8-digit codes, which
PROJ doesn't import (61206405 → 4120).

### Validation

`test_oracle.py` compares `to_2d()` and `to_3d()` of every current EPSG CRS
with pyproj's on the oracle database. The comparison covers type, name, id,
axes, datum, scope, area and geodetic CRS. It also compares
`get_non_deprecated()` of every deprecated one. Everything matches but
these pyproj artifacts:

- **Remarks.** PROJ's database has few CRS remarks, so pyproj's promoted
  remark stops at "Promoted to 3D from EPSG:…". Ours goes on with EPSG's,
  as for the CRSs themselves.
- **WKT round trips.** pyproj rebuilds the result from its WKT, which
  drops the ids of a compound CRS's components. EPSG:3857's promoted
  base stays the 2D EPSG:4326 in pyproj.
- **Deprecated CRSs.** pyproj's results for them can carry the
  replacement's definition under the old id (EPSG:4855's `to_2d` is named
  like EPSG:5105), so only their replacements are compared.

## `get_units_map`

`get_units_map(auth_name=None, category=None, allow_deprecated=False)`
returns `{name: Unit(auth_name, code, name, category, conv_factor,
proj_short_name, deprecated)}`, as PROJ's `getUnitList` reads its
database. Its data comes from these places:

- **The units:** EPSG's `epsg_unitofmeasure`, with `conv_factor` as
  `factor_b / factor_c`, as `build_db.py` computes it, and 0 where EPSG
  gives none. They are in `xeibe_crs::UNITS_OF_MEASURE`.
- **`category`:** PROJ's mapping from the unit's type and name: `linear`,
  `angular`, `scale` and `time`, and `linear_per_time`, `angular_per_time`
  or `scale_per_time` for a name with "per" in it.
- **`proj_short_name`, and PROJ's own five units:** from PROJ's
  `data/sql/customizations.sql` (MIT), in
  `xeibe_reproject::proj_tables` (`UNIT_SHORT_NAMES`, `PROJ_UNITS`).

The units come in PROJ's order, by authority and code. A name used twice
is the later unit's, as in pyproj's dict. An empty `auth_name` means every
authority. `test_database.py` compares every filter with pyproj, value by
value and in order: equal, without Esri's units.

## Selection: `area_of_interest`, `accuracy`, `only_best`

PROJ applies these while it builds the candidates in
`proj_create_crs_to_crs` (`crs_to_crs.cpp`), through its operation factory
context (`FilterResults` in `coordinateoperationfactory.cpp`):

- **`area_of_interest`** replaces the smaller of the two CRSs' areas as the
  area of interest. Candidates must intersect it, are ranked by how much of
  it they cover, and `hasTooSmallAreas` measures against it. A box across
  the antimeridian has west > east. PROJ still keeps several candidates and
  chooses one per coordinate.
- **`accuracy`** keeps the candidates whose accuracy is known and no worse
  than the value, in metres. That applies also to the fallback that takes
  back every candidate when none has an area matching; 0 sets no limit.
- **`only_best`** keeps the candidates whose grids are missing (PROJ's
  `KNOWN_AVAILABLE` instead of discarding them with the network off). A
  coordinate whose best candidate can't run is then an error instead of
  falling back to the next one.

### Implementation

- **Rust:** `xeibe_reproject::Selection` (`area_of_interest`,
  `accuracy`) goes to `Transform::with_selection`. The search's `Areas`
  takes the user's area and the accuracy (`select/filter.rs`,
  `select/search.rs`), and the transform keeps the selection for its
  inverse and for the routes through a Bound CRS's hub.
- **`only_best`** needs no Rust change. `Grids::ProjData` is PROJ's
  `KNOWN_AVAILABLE`, and a grid that isn't loaded already stops a
  coordinate instead of trying the next candidate. With the network off,
  `from_crs(only_best=True)` builds the candidates that way and loads the
  grids it has, skipping the missing ones. A coordinate whose best
  operation needs one of those fails when it is converted. PROJ fails at
  construction instead when there is a single candidate.
- **Python:** `from_crs` and `TransformerGroup` pass the options on. A
  group with no candidate left is empty, as pyproj's, instead of an error.
  Transformers keep their options through pickling and
  `get_last_used_operation`.

### Validation

`gen_transformation_vectors.py --options` writes
`selection-options.jsonl`: for a sample of the selection pairs, PROJ's
candidates and per-point results with an area of interest inside the
pair's areas, a small one, an accuracy, and both. The Rust test compares
them as it compares `selection.jsonl`, and a Python test runs the same
records through `from_crs`. Both match PROJ.

One difference in `TransformerGroup` is older than this step and
remains. pyproj's group searches with grid availability `IGNORED`, while
ours lists what `from_crs` would choose from (`KNOWN_AVAILABLE`). Where
an operation needs a grid, pyproj's group can therefore have no route
through a pivot that ours lists, and the reverse.

## `Transformer.to_json`, `to_json_dict`, `to_wkt`

When the transformer has one operation, these describe it as PROJ does.
With several candidates, all three return `None`, as pyproj's do ("unavailable
until proj_trans is called"); a `Proj` gives its CRS's projection.

Each step of the operation becomes PROJ's object for it:

- a map projection is a `Conversion` with the conversion's EPSG id, its
  inverse "Inverse of …" with the method "Inverse of …" and the authority
  `INVERSE(EPSG)`;
- an EPSG transformation is a `Transformation` with its source and target
  CRSs, method, parameters, accuracy, usage, id and remarks. Inverted, it
  follows PROJ's `inverseAsTransformation`: the methods whose inverse is
  the same method with negated parameters (geocentric translations,
  Molodensky, the offsets, longitude rotation) are written that way, the
  others with "Inverse of" the method;
- one of PROJ's null operations from a datum ensemble's CRS to a member's
  carries PROJ's id (`PROJ:ETRS89_TO_ETRS89-FIN[EUREF-FIN]`), remarks and
  scope, as its database has them;
- between CRSs on one datum, PROJ's own: a null geographic offset (2D ↔ 3D),
  a geographic/geocentric conversion, a null geocentric translation.

Several steps make a `ConcatenatedOperation` with `steps`, whose steps carry
no usage, and whose accuracy is PROJ's sum written with 15 significant
digits. As PROJ does, its first and last steps start and end in the
transformer's CRSs, and an EPSG operation run between other CRSs than its
own (their 3D forms) gets them, with a `DERIVED_FROM(EPSG)` id.

### Implementation

- **Rust:** `OperationInfo::chain` lists every step of a candidate: its
  name, its kind (`StepKind`: an EPSG operation, PROJ's ensemble null
  operation, a projection, a change on one datum…) and the CRSs it runs
  between (`StepCrs`, with the promoted and code-less compound CRSs of the
  search). `steps` still lists only the EPSG transformations.
- **`xeibe-crs`:** `operations_json` adds EPSG's method and parameter
  names, each parameter with the kind of unit its values have, and each
  projected CRS's conversion code (PROJJSON leaves that id out inside a
  CRS but gives it to the conversion as a step).
- **Python:** `_operation_json.py` builds the PROJJSON from the chain;
  `to_wkt` writes it through the WKT2 writer, which gained
  `CONCATENATEDOPERATION`, usages and remarks on an outermost operation,
  the CRSs of an operation with their ids only, and PROJ's
  `SCOPE["unknown"]` for a usage without a scope.

### Validation

`test_operation_json_matches_pyproj` (oracle) compares every operation of
`TransformerGroup` between 300 random pairs of geographic, geocentric,
projected and compound EPSG CRSs with pyproj's operation of the same name:
type, name, accuracy, id, area and keys of the whole, and of each step its
method, parameters in SI units, CRS codes, id, accuracy, remarks and area.
All match, with what PROJ does differently left out:

- PROJ marks an operation `DERIVED_FROM` where its search took a pivot
  datum and merged a null step into it, which depends on its search's
  order; ids are compared without the wrapper.
- In compound and 3D routes PROJ may run a step between the 3D form of a
  2D CRS, without an id, where ours names the 2D CRS.
- PROJ writes a horizontal and a vertical operation run together as one
  `Transformation` whose method is a PROJ pipeline string, which
  [pyproj-compat.md](pyproj-compat.md) leaves out; ours is the
  concatenation of the two.
- A projection's parameters are the CRS's, compared by the CRS tests
  (PROJ drops Modified Krovak's polynomial coefficients there).

Parameters are in SI units where PROJ keeps EPSG's (arc-seconds, parts
per million), and the WKT has no `VERSION` (PROJJSON has no key for it).
PROJ's WKT writes an inverted Helmert transformation as its approximate
inverse with negated parameters ("… (approx. inversion)"); ours keeps the
inverse method, as both write in PROJJSON.

## `download_grids`

`TransformerGroup.download_grids(directory=None, open_license=True,
verbose=False)` behaves as in pyproj:

- It fetches the missing grids of the group's `unavailable_operations`,
  each once, as a whole file from `cdn.proj.org`, through a `.part` file
  renamed when complete.
- The files go into `directory`, or into the user data directory
  (`get_user_data_dir(True)`), as pyproj's.
- Files already in the directory are kept, where pyproj downloads them
  again.
- `open_license=True` skips the grids without an open licence. Every grid
  an operation here can use is on the CDN, and PROJ's catalog has every
  CDN grid openly licensed (`gen_grid_catalog.py` checks it); the three
  grids it has elsewhere (RDNAP 2008, not open) can't be loaded, so no
  candidate uses them. The flag therefore changes nothing yet.
- `verbose` prints `Downloading: <url>` for each file and warns of each
  grid skipped.

For the files to be found next time, grids are looked for as PROJ looks
for resource files: in the directories `set_data_dir` set, alone, or
else in the user data directory and then `PROJ_DATA`. The transformer
module has pyproj's names `get_user_data_dir` and
`_download_resource_file`, which pyproj's tests patch.

Downloads are logged as in [pyproj-compat.md](pyproj-compat.md#grids)
(`INFO` for each file with its size, `DEBUG` for one kept, `ERROR` before
the `ProjError` of a failure). The tests run against the local HTTP server
the grid tests use, which now also serves whole files.

## Order of work

1. **The geodesic port in Rust**: the rest of `geodesic.c`, the
   Azimuthal Equidistant moved onto it, PROJ's tests,
   `gen_geodesic_vectors.py`, and the ellipsoid and prime meridian
   tables. It is gated on PROJ's tests and pyproj's numbers. **Done:**
   `xeibe_reproject::geodesic` and `xeibe_reproject::proj_tables`.
2. **`Geod` in `xeibe_proj`**, with `get_geod`, `pj_ellps`,
   `get_ellps_map` and `get_prime_meridians_map`, gated on pyproj and its
   `test_geod.py`. **Done:** `xeibe_proj.geod` and `xeibe_proj.list`. All
   of pyproj's `test_geod.py` passes but `inplace=` (decision 4), and the
   suite went from 149 to 318 of 960.
3. **`Proj`**, gated on pyproj over every EPSG CRS. **Done:**
   `xeibe_reproject::Projection` and `xeibe_proj.proj`, matching pyproj but
   for its PROJ strings' own differences ([Validation](#validation)).
   pyproj's `test_proj.py` mostly needs PROJ strings (phase 3); the suite
   went from 318 to 324 of 960.
4. **`to_2d`, `to_3d`, `get_non_deprecated`**, gated on pyproj over every
   EPSG CRS. **Done:** `xeibe_proj.crs._dimension` and
   `xeibe_crs::replacements` (with `xeibe_crs::UNITS_OF_MEASURE` for step 5);
   the suite went from 324 to 335 of 963.
5. **`get_units_map`**, gated on pyproj. **Done**, equal to pyproj's
   without Esri's units; the suite went from 335 to 348 of 963.
6. **`area_of_interest`, `accuracy`, `only_best`**, gated on PROJ's
   candidate lists. **Done:** `xeibe_reproject::Selection` and
   `selection-options.jsonl`. On the full set (`--all --options`, 71,796
   points) every point matches PROJ but for one tie of EPSG variants,
   broken by EPSG's data as elsewhere, and one route PROJ's pipeline
   optimiser turns into a no-op across GRS 1980 and WGS 84 (0.3 mm). The
   suite went from 348 to 351 of 963.
7. **`Transformer.to_json`, `to_json_dict` and `to_wkt`**, gated on
   pyproj's output. **Done:** `OperationInfo::chain` and
   `xeibe_proj._operation_json`, matching pyproj's operations but for
   PROJ's own artifacts ([Validation](#validation-3)). Routes through a
   national realization that no coordinate could take are no longer
   candidates, so WGS 84 to Poland CS92 has PROJ's one operation. The suite
   went from 351 to 356 of 963.
8. **`download_grids`**, gated on the local grid server. **Done:**
   `xeibe_proj._grids.download`, with grids looked for in the user data
   directory too.
