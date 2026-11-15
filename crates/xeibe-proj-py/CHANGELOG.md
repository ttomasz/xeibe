# Changelog

`xeibe-proj`, released by `xeibe-proj-v*` tags (docs/releasing.md in the
xeibe repository). Each entry names the EPSG Dataset version it embeds
(docs/pyproj-compat.md, Versioning).

## 0.3.0

- EPSG Dataset 13.106, as in 0.2.0.
- `Transformer.transform` and `itransform` use `tt` as each coordinate's
  epoch (a decimal year), as pyproj does: a time-dependent transformation
  converts the coordinate at it. Before, `tt` passed through unused and
  every coordinate was converted at the transformation's reference epoch.
  `tt` must now have as many values as the other arrays.
- With `tt`, coordinates go through the operations that need an epoch:
  New Zealand's deformation model (ITRF96 to NZGD2000) and the NKG
  transformations from ITRF2014 and ITRF2020 to the Nordic and Baltic
  ETRS89 realizations. Without one, a coordinate with no other operation
  to go through raises `ProjError`, where pyproj writes `inf`.
- Every live EPSG CRS converts: the 21 on the five projection methods PROJ
  has no mapping for (Lambert Conic Conformal (West Orientated), Polar
  Stereographic (variant C), Lambert Conic Near-Conformal, Tunisia Mining
  Grid, Transverse Mercator Zoned Grid System) run from EPSG's formulas.
  pyproj can't convert them.
- Results that differ from 0.2.0, and from pyproj (docs/reprojection.md,
  "Beyond PROJ"):
  - BD72 / Belge Lambert 72 (EPSG:31300) moves by 745 m, onto EPSG's
    worked example: Lambert Conic Conformal (2SP Belgium) now turns the
    grid by 29.2985″, as EPSG defines it and PROJ doesn't.
  - TM75 to ETRS89 and WGS 84, Viti Levu and Vanua Levu to the Fiji Map
    Grid, and ED50 to ED87 go through EPSG's polynomials, which PROJ
    doesn't import.
  - EVRF2019 zero-tide to mean-tide heights follow EPSG's formula (−0.053 m
    at 42°N to +0.072 m at 69°N) where PROJ applies −0.054 m everywhere.
  - ETRS89-SVK [SKTRF09] + Baltic 1957 height to ETRS89 goes through
    Slovakia's own geoid model, not the Czech one.
  - A coordinate whose `tt` is the epoch of a time-specific Helmert, inside
    its area, goes through it when nothing else there is more accurate
    ("WGS 84 to ETRS89 (2)" at 2014.81 in the German and Dutch North Sea).
    These routes are not in `Transformer.operations` or the other lists.
    Between two CRSs with nothing else (ITRF2000 to SIRGAS 2000, at
    2000.4), the route converts a coordinate at its epoch wherever it is,
    and any other coordinate raises `ProjError`.
- Pairs that had no transformation, and have none in pyproj: Madrid 1870
  (Madrid) to ED50; ETRS89 + LAT-NLD or MSL-NLD depth to ETRS89
  ("Geographic3D to Depth"); ETRS89-SVK [SKTRF09] + EVRF2007 height to
  ETRS89.

## 0.2.0

- EPSG Dataset 13.106, as in 0.1.0.
- CRS text (`CRS(...)`, `from_user_input`, `from_string`,
  `from_authority`) is read by `xeibe-reproject`'s `Crs::from_user_input`,
  in the same forms as before.
- `EPSG:<horizontal>+<vertical>` is the compound CRS PROJ builds: named
  "horizontal + vertical", without a code, even when EPSG registers the
  pair. `CRS("EPSG:4326+5773")` was EPSG:9707 itself; `to_epsg()` still
  gives 9707.

## 0.1.0

- First release, with EPSG Dataset 13.106.
- `xeibe_proj.arrow`: GeoArrow arrays (native encodings, WKB and boxes)
  converted through the Arrow PyCapsule interface, without depending on
  any Arrow library.
- A wheel for Pyodide 314 (`pyemscripten_2026_0_wasm32`). Grids are
  fetched there with the browser's `XMLHttpRequest`.
