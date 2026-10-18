//! Offsets between geographic CRSs: Longitude rotation (EPSG:9601),
//! Geographic2D offsets (9619), Geographic3D offsets (9660) and Geographic2D
//! with Height Offsets (9618).
//!
//! Follows PROJ 9.8.1 `src/iso19111/operation/transformation.cpp` and
//! `src/transformations/geogoffset.cpp` (MIT, see PROJ-NOTICE.md): the
//! target coordinates are the source coordinates plus the offsets, and the
//! inverse subtracts them. Longitude rotation is the same step with a
//! longitude offset only; PROJ writes it as a change of prime meridian, which
//! amounts to the same sum because the internal longitudes are counted from
//! each datum's own meridian. The height is carried through unchanged unless
//! the method has a height offset.

use std::sync::Arc;

use super::{Method, MethodError, param, required};
use crate::crs::{Crs, Kind};
use crate::error::CoordinateError;
use crate::registry::Operation;

/// EPSG method codes handled here.
pub(super) const METHODS: &[u32] = &[9601, 9619, 9660, 9618];

/// Adds fixed offsets to longitude, latitude and height.
#[derive(Debug)]
struct Offsets {
    /// Longitude, latitude (radians) and height (metres) offsets.
    d: [f64; 3],
}

pub(super) fn build(op: &Operation, src: &Crs, dst: &Crs) -> Result<Arc<dyn Method>, MethodError> {
    // Geographic2D with Height Offsets starts from a compound CRS: its
    // horizontal part is geographic.
    for crs in [src, dst] {
        if !matches!(crs.horizontal().kind, Kind::Geographic { .. }) {
            return Err(MethodError::WrongCrs("a geographic CRS"));
        }
    }
    let d = match op.method {
        9601 => [required(op, param::LONGITUDE_OFFSET)?, 0.0, 0.0],
        9619 => [
            required(op, param::LONGITUDE_OFFSET)?,
            required(op, param::LATITUDE_OFFSET)?,
            0.0,
        ],
        9660 => [
            required(op, param::LONGITUDE_OFFSET)?,
            required(op, param::LATITUDE_OFFSET)?,
            required(op, param::VERTICAL_OFFSET)?,
        ],
        9618 => [
            required(op, param::LONGITUDE_OFFSET)?,
            required(op, param::LATITUDE_OFFSET)?,
            required(op, param::GEOID_HEIGHT)?,
        ],
        _ => return Err(MethodError::NotImplemented),
    };
    Ok(Arc::new(Offsets { d }))
}

impl Method for Offsets {
    fn forward(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        Ok([c[0] + self.d[0], c[1] + self.d[1], c[2] + self.d[2]])
    }

    fn inverse(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        Ok([c[0] - self.d[0], c[1] - self.d[1], c[2] - self.d[2]])
    }
}
