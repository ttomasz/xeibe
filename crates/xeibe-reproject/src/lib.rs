//! Coordinate conversion between CRSs, in pure Rust, from the PROJJSON that
//! `xeibe-crs` generates out of the EPSG dataset.
//!
//! <div class="warning">
//!
//! **Experimental, not for production use.** This crate is AI-generated and
//! has not been thoroughly tested yet. Its interfaces are not stable and will
//! change in the near future, in breaking ways.
//!
//! </div>
//!
//! Levels 1 to 3 of the [reprojection document](https://github.com/ttomasz/xeibe/blob/main/docs/reprojection.md):
//!
//! * within one datum, conversions: projections, changes of axis order and
//!   units, geographic ↔ geocentric;
//! * between datums, EPSG's transformations, chosen per coordinate the way
//!   PROJ chooses them ([`Transform::new`]), or forced ([`Transform::via`]);
//! * grid-based ones among them, with PROJ-data's grids, which the caller
//!   loads: [`Transform::grid_requests`] lists what to fetch from
//!   [`CDN`] or a local copy, and a coordinate whose grid isn't loaded is an
//!   error ([`CoordinateError::GridNotLoaded`]);
//! * vertical and compound CRSs ([`Crs::compound`]): geoid models, vertical
//!   offsets and offset grids;
//! * engineering CRSs, on their own datum and through EPSG's operations.
//!
//! When no EPSG transformation applies, setting up the transform fails; a
//! ballpark approximation is never used. Operations that need a coordinate
//! epoch (point motion, deformation models) are not supported.
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
mod datum;
mod ellipsoid;
mod error;
mod extent;
mod geocentric;
mod grid;
mod methods;
mod operation;
mod registry;
mod select;
mod transform;

pub use bounds::DEFAULT_DENSIFY;
pub use crs::Crs;
pub use error::{CoordinateError, Error};
pub use grid::{CDN, GridRequest, Grids, clear_grids};
pub use select::OperationInfo;
pub use transform::{AxisOrder, Transform};
