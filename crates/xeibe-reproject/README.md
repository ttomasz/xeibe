# xeibe-reproject

Coordinate conversion between EPSG coordinate reference systems, in pure Rust,
part of [xeibe](https://github.com/ttomasz/xeibe). Driven by the PROJJSON
definitions and coordinate operations in
[`xeibe-crs`](https://crates.io/crates/xeibe-crs); no PROJ or other C library.

> **Warning: experimental, not for production use.** This crate is
> AI-generated and has not been thoroughly tested yet. Its interfaces are not
> stable and will change in the near future, in breaking ways. Do not use it in
> production.

## What it does

- Conversions within one datum: map projections, changes of axis order and
  units, geographic to geocentric.
- Transformations between datums, chosen from EPSG's operations the way PROJ
  chooses them, or forced.
- Grid-based transformations, with PROJ-data's grids, which the caller loads.
- Vertical and compound CRSs: geoid models, vertical offsets and offset grids.
- Engineering CRSs, on their own datum and through EPSG's operations.

When no EPSG transformation applies, setting up the transform fails; a
ballpark approximation is never used. Operations that need a coordinate
epoch (point motion, deformation models) are not supported.

```rust
use xeibe_reproject::{AxisOrder, Transform};

// ETRS89 geographic to ETRS89 / PL-1992, longitude first.
let t = Transform::from_epsg(4258, 2180, AxisOrder::EastFirst).unwrap();
let (x, y, _) = t.convert(21.0, 52.2, None).unwrap();
```

PROJ serves as the test oracle.

## Documentation

See the [reprojection design document](https://github.com/ttomasz/xeibe/blob/main/docs/reprojection.md).

## License

CC0-1.0 AND MIT: some of the projection code is ported from PROJ, which is MIT;
see [`PROJ-NOTICE.md`](https://github.com/ttomasz/xeibe/blob/main/crates/xeibe-reproject/PROJ-NOTICE.md).
Everything else is CC0-1.0.
