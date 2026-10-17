//! Coordinate conversion between CRSs, in pure Rust, from the PROJJSON that
//! `xeibe-crs` generates out of the EPSG dataset.
//!
//! This is level 1 of `docs/reprojection.md`: conversions within one datum.
//! Projections, changes of axis order and units, and geographic ↔
//! geocentric. Datum transformations (level 2) and grids (level 3) are not
//! implemented; a pair of CRSs on different datums is an error, never a
//! silent approximation.
//!
//! ```
//! use xeibe_reproject::{AxisOrder, Transform};
//!
//! // ETRS89 geographic to ETRS89 / PL-1992, longitude first.
//! let t = Transform::from_epsg(4258, 2180, AxisOrder::EastFirst).unwrap();
//! let (x, y, _) = t.convert(21.0, 52.2, None).unwrap();
//! // PROJ 9.8.1 gives 636640.467013, 483432.200828.
//! assert!((x - 636_640.467_013).abs() < 1e-6 && (y - 483_432.200_828).abs() < 1e-6);
//! ```
//!
//! The math follows PROJ, which serves as the test oracle; see `PROJ-NOTICE.md`
//! for the parts ported from it.

mod bounds;
mod crs;
mod ellipsoid;
mod error;
mod methods;
mod transform;

pub use bounds::DEFAULT_DENSIFY;
pub use crs::Crs;
pub use error::{CoordinateError, Error};
pub use transform::{AxisOrder, Transform};
