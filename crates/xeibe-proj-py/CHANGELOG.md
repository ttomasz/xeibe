# Changelog

`xeibe-proj`, released by `xeibe-proj-v*` tags (docs/releasing.md in the
xeibe repository). Each entry names the EPSG Dataset version it embeds
(docs/pyproj-compat.md, Versioning).

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
