# xeibe-proj (Python)

Coordinate conversion between **EPSG** coordinate reference systems with a
**pyproj-compatible API**, in pure Rust: no PROJ, no `proj.db`, no C
dependencies, and no required Python dependencies. Part of
[xeibe](https://github.com/ttomasz/xeibe).

> **Warning: experimental, not for production use.** This package is
> AI-generated and has not been thoroughly tested yet. Its interfaces are not
> stable and will change, in breaking ways. Only part of pyproj's API is
> implemented; the plan, and what will never be supported, is in
> [`docs/pyproj-compat.md`](https://github.com/ttomasz/xeibe/blob/main/docs/pyproj-compat.md).

## Usage

```python
from xeibe_proj import CRS, Transformer

t = Transformer.from_crs("EPSG:4326", "EPSG:2180", always_xy=True)
x, y = t.transform(21.0, 52.0)
xs, ys = t.transform([21.0, 19.9], [52.0, 50.1])   # lists in, lists out
left, bottom, right, top = t.transform_bounds(14.1, 49.0, 24.2, 55.9)

crs = CRS("urn:ogc:def:crs:EPSG::2180")
crs.name, crs.axis_info, crs.area_of_use, crs.to_epsg()
crs == CRS(2180)                                    # True

prj = CRS(open("parcels.prj").read())               # WKT1 (GDAL's or Esri's) or WKT2
prj.to_epsg(), prj.list_authority()                 # identified by its definition
prj.to_wkt("WKT1_GDAL", pretty=True)                # or "WKT2_2019"

from xeibe_proj import Geod

g = Geod(ellps="WGS84")                             # or crs.get_geod()
az12, az21, dist = g.inv(-73.78, 40.64, 103.99, 1.36)
area, perimeter = g.polygon_area_perimeter([0, 1, 1], [0, 0, 1])

from xeibe_proj import Proj

p = Proj(2180)                                      # the CRS's projection, no datum shift
x, y = p(19.0, 52.0)                                # degrees from Greenwich in
lon, lat = p(x, y, inverse=True)
```

Coordinates come back in the type they went in: a number, a list, a tuple,
an `array.array` or a numpy array (pandas and xarray objects give numpy
arrays). numpy is not a dependency and is never imported: its arrays are
read through the buffer protocol. `pip install xeibe-proj[numpy]` only
declares it.

CRSs can be given as EPSG codes in any common spelling (`4326`,
`"EPSG:4326"`, `"EPSG:4326+5773"`, OGC URNs and URLs), as `"OGC:CRS84"`, as
EPSG names (`"WGS 84"`), as WKT (WKT1 as GDAL and Esri write it, with
`TOWGS84`, and WKT2), or as PROJJSON. Esri's old codes of CRSs EPSG has
since defined (`"ESRI:102100"`) give the EPSG CRS. PROJ strings are not read
yet.

## Differences from pyproj

- **Failed coordinates raise** `ProjError`, which names the coordinate.
  There is no `inf` result: `errcheck` and `inplace` don't exist (on
  `Geod.fwd` and `inv` neither), and passing either raises
  `NotImplementedError`. `direction` and `return_back_azimuth` are
  keyword-only.
- **No ballpark transformations.** Without a usable EPSG operation,
  `Transformer.from_crs` raises `ProjError`.
- **Grids are fetched by default**, from `cdn.proj.org`, by byte range, and
  cached under the user cache directory. `xeibe_proj.network.set_network_enabled(False)`
  or `PROJ_NETWORK=OFF` turns this off, and then only operations without
  grids are used, as in pyproj's default install. Grid files in `PROJ_DATA`
  (or `xeibe_proj.datadir.set_data_dir`) are read instead of fetched.
- **A newer EPSG release** than pyproj's bundled `proj.db`:
  `xeibe_proj.EPSG_VERSION`. Names and operations can differ.
- **No PROJ version**: `proj_version_str` and `PROJ_VERSION` don't exist.
- **WKT is written in two versions**, `WKT2_2019` and `WKT1_GDAL`; the
  others raise `NotImplementedError`. The output is PROJ's, but for ids PROJ
  looks up in its database.
- **`Proj` follows EPSG**, where pyproj's follows the CRS's PROJ string:
  full-precision parameters, EPSG's prime meridians and the CRS's own
  datum. Results differ by up to 0.35 mm (kilometres for EASE-Grid). `srs`
  is the CRS's, not a PROJ string.
- **Other authorities** than EPSG and OGC (`ESRI:`, `IGNF:`, …) are not
  known, apart from Esri's old codes of EPSG CRSs.

## Logging

Every use of a grid is logged to the `xeibe_proj.grids` logger: `INFO` when
a transformer first converts with grids and for each download, `DEBUG` for
the cache and data directories, `ERROR` for a failed download. The package
adds no handlers, so the `INFO` records are dropped unless the application
configures logging:

```python
import logging
logging.basicConfig(level=logging.INFO)
```

## Types

The package is fully typed (`py.typed`) and checked with mypy and pyright in
strict mode. `Transformer.transform` has overloads that follow the
container rules: `float` in gives `float` out, `list[float]` gives
`list[float]`, a numpy array gives a numpy array.

## Licence

The code is CC0-1.0. The embedded EPSG data is IOGP's, used under the EPSG
Dataset Terms of Use, some projection code is ported from PROJ (MIT), and
Esri's names of EPSG objects, for reading Esri's WKT, are Apache-2.0: see
`NOTICE.md`.
