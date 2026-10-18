//! Transformations by triangulation (level 3): EPSG's "Cartesian Grid
//! Offsets by TIN Interpolation (JSON)" between projected CRSs (Finland's
//! KKJ to ETRS-TM35FIN) and "Geographic2D Offsets by TIN Interpolation
//! (JSON)" between geographic CRSs (Norway's NGO 1948 to EUREF89).
//!
//! As PROJ 9.8.1 writes them (`SingleOperation::exportToPROJString`, MIT,
//! see PROJ-NOTICE.md): `tinshift` on easting and northing in metres, or on
//! longitude and latitude in the CRSs' degrees.

use std::sync::Arc;

use super::grid::state_error;
use super::{Method, MethodError};
use crate::crs::{Crs, Kind};
use crate::error::CoordinateError;
use crate::grid::store::{self, GridFile};
use crate::registry::Operation;

/// EPSG method codes handled here.
pub(super) const METHODS: &[u32] = &[CARTESIAN_BY_TIN, GEOGRAPHIC_BY_TIN];

const CARTESIAN_BY_TIN: u32 = 1138;
const GEOGRAPHIC_BY_TIN: u32 = 1145;
/// "TIN offset file".
const TIN_FILE: u32 = 1064;
/// EPSG's degree (unit 9102), in radians.
const DEGREE: f64 = 0.017_453_292_519_943_3;

pub(super) fn build(op: &Operation, src: &Crs, dst: &Crs) -> Result<Arc<dyn Method>, MethodError> {
    let name = op
        .file(TIN_FILE)
        .ok_or(MethodError::MissingParameter(TIN_FILE))?;
    let entry = crate::grid::on_cdn(name, &op.name)
        .ok_or_else(|| MethodError::GridUnavailable(name.to_string()))?;
    let factor = match op.method {
        CARTESIAN_BY_TIN => {
            for crs in [src, dst] {
                if !matches!(crs.kind, Kind::Projected { .. }) {
                    return Err(MethodError::WrongCrs("a projected CRS"));
                }
            }
            1.0
        }
        GEOGRAPHIC_BY_TIN => {
            // PROJ requires degrees and hands the TIN the CRSs' own values.
            for crs in [src, dst] {
                let degrees = crs
                    .axes
                    .axes
                    .first()
                    .is_some_and(|a| (a.factor - DEGREE).abs() < 1e-15);
                if !matches!(crs.kind, Kind::Geographic { .. }) || !degrees {
                    return Err(MethodError::WrongCrs("a geographic CRS in degrees"));
                }
            }
            DEGREE
        }
        _ => return Err(MethodError::NotImplemented),
    };
    Ok(Arc::new(TinShift {
        file: store::file(entry.file),
        factor,
    }))
}

/// `tinshift` on the operation's horizontal coordinates.
#[derive(Debug)]
struct TinShift {
    file: Arc<GridFile>,
    /// From the TIN's unit (metre or degree) to internal radians or metres.
    factor: f64,
}

impl TinShift {
    fn apply(&self, c: [f64; 3], forward: bool) -> Result<[f64; 3], CoordinateError> {
        let tin = self.file.tin().map_err(state_error)?;
        let (x, y) = (c[0] / self.factor, c[1] / self.factor);
        let out = if forward {
            tin.forward(x, y, c[2])
        } else {
            tin.inverse(x, y, c[2])
        };
        match out {
            Some([x, y, z]) => Ok([x * self.factor, y * self.factor, z]),
            None => Err(CoordinateError::OutsideGrid),
        }
    }
}

impl Method for TinShift {
    fn forward(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        self.apply(c, true)
    }

    fn inverse(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        self.apply(c, false)
    }

    fn grid_files(&self) -> Vec<Arc<GridFile>> {
        vec![self.file.clone()]
    }
}
