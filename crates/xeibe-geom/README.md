# xeibe-geom

GML geometry parsing for [xeibe](https://github.com/ttomasz/xeibe). No Arrow
dependency.

> **Warning: experimental, not for production use.** This crate is
> AI-generated and has not been thoroughly tested yet. Its interfaces are not
> stable and will change in the near future, in breaking ways. Do not use it in
> production.

## What it does

- Parses GML 2, 3.1, 3.2 and 3.3 geometry into an internal model that keeps
  curves as curves: arcs, circles, compound curves and curve polygons. Of GML
  3.3's additions, that is the compact encodings (`gmlce:SimplePolygon`,
  `SimpleArc`, `SimpleMultiPoint`, ...) and `gmltin:SimpleTrianglePatch`.
- Exposes that model through [`geo-traits`](https://crates.io/crates/geo-traits)
  and writes ISO WKB, curve types included. Curves can also be linearized.
- Resolves `srsName` to an EPSG CRS and decides the axis order, using the EPSG
  facts in [`xeibe-crs`](https://crates.io/crates/xeibe-crs) and evidence from
  the coordinates themselves.

Most users want [`xeibe-arrow`](https://crates.io/crates/xeibe-arrow) instead,
which reads whole GML documents into Arrow record batches.

## Documentation

See the [geometry design document](https://github.com/ttomasz/xeibe/blob/main/docs/geometry.md).

## License

CC0-1.0 (public domain dedication).
