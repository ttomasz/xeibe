# Changelog

`xeibe-proj`, released by `xeibe-proj-v*` tags (docs/releasing.md in the
xeibe repository). Each entry names the EPSG Dataset version it embeds
(docs/pyproj-compat.md, Versioning).

## Unreleased

- First release, with EPSG Dataset 13.106.
- `xeibe_proj.arrow`: GeoArrow arrays (native encodings, WKB and boxes)
  converted through the Arrow PyCapsule interface, without depending on
  any Arrow library.
- A wheel for Pyodide 314 (`pyemscripten_2026_0_wasm32`). Grids are
  fetched there with the browser's `XMLHttpRequest`.
