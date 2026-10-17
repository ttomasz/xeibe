//! Plane transformations between projected (or other Cartesian) CRSs:
//! Similarity transformation (EPSG:9621), Affine parametric transformation
//! (9624) and Cartesian Grid Offsets (9656).
//!
//! Follows PROJ 9.8.1 `src/iso19111/operation/singleoperation.cpp`
//! (`exportToPROJStringGeneric`, Cartesian grid offsets) and
//! `src/transformations/affine.cpp` (MIT, see PROJ-NOTICE.md). Similarity and
//! affine parametric apply to the ordinates exactly as the CRS gives them:
//! PROJ leaves the axis order and units alone ("Do not mess with axis unit and
//! order"), so "ordinate 1" is the first axis, a northing for a CRS that lists
//! northing first, and in the CRS's unit. Cartesian grid offsets work on
//! easting and northing in metres. The height is carried through.

use std::sync::Arc;

use super::{Method, MethodError, param, required};
use crate::crs::{Axes, Crs, Kind};
use crate::error::CoordinateError;
use crate::registry::Operation;

/// EPSG method codes handled here.
pub(super) const METHODS: &[u32] = &[9621, 9624, 9656];

/// `[x', y'] = [xoff, yoff] + m · [x, y]`.
#[derive(Debug)]
struct Plane {
    off: [f64; 2],
    m: [[f64; 2]; 2],
    inv: Option<[[f64; 2]; 2]>,
    /// Apply to the ordinates in the CRSs' own axes and units, not to
    /// easting and northing in metres.
    raw: Option<(Axes, Axes)>,
}

pub(super) fn build(op: &Operation, src: &Crs, dst: &Crs) -> Result<Arc<dyn Method>, MethodError> {
    for crs in [src, dst] {
        if matches!(crs.kind, Kind::Geographic { .. }) {
            return Err(MethodError::WrongCrs("a Cartesian or projected CRS"));
        }
    }
    let (off, m, raw) = match op.method {
        9656 => (
            [
                required(op, param::EASTING_OFFSET)?,
                required(op, param::NORTHING_OFFSET)?,
            ],
            [[1.0, 0.0], [0.0, 1.0]],
            false,
        ),
        9624 => (
            [required(op, param::A0)?, required(op, param::B0)?],
            [
                [required(op, param::A1)?, required(op, param::A2)?],
                [required(op, param::B1)?, required(op, param::B2)?],
            ],
            true,
        ),
        9621 => {
            let scale = required(op, param::SCALE_FACTOR_SOURCE_AXES)?;
            let (s, c) = required(op, param::ROTATION_SOURCE_AXES)?.sin_cos();
            (
                [
                    required(op, param::ORDINATE_1_EVAL_POINT_TARGET)?,
                    required(op, param::ORDINATE_2_EVAL_POINT_TARGET)?,
                ],
                [[scale * c, scale * s], [-scale * s, scale * c]],
                true,
            )
        }
        _ => return Err(MethodError::NotImplemented),
    };
    let det = m[0][0] * m[1][1] - m[0][1] * m[1][0];
    let inv = (det != 0.0).then(|| {
        [
            [m[1][1] / det, -m[0][1] / det],
            [-m[1][0] / det, m[0][0] / det],
        ]
    });
    let raw = raw.then(|| (src.axes.clone(), dst.axes.clone()));
    Ok(Arc::new(Plane { off, m, inv, raw }))
}

impl Plane {
    fn apply(&self, c: [f64; 3]) -> [f64; 3] {
        [
            self.off[0] + self.m[0][0] * c[0] + self.m[0][1] * c[1],
            self.off[1] + self.m[1][0] * c[0] + self.m[1][1] * c[1],
            c[2],
        ]
    }

    fn unapply(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        let i = self.inv.ok_or(CoordinateError::OutsideDomain)?;
        let (x, y) = (c[0] - self.off[0], c[1] - self.off[1]);
        Ok([i[0][0] * x + i[0][1] * y, i[1][0] * x + i[1][1] * y, c[2]])
    }
}

impl Method for Plane {
    fn forward(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        Ok(match &self.raw {
            Some((from, to)) => to.to_internal(self.apply(from.to_crs(c))),
            None => self.apply(c),
        })
    }

    fn inverse(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        Ok(match &self.raw {
            Some((from, to)) => from.to_internal(self.unapply(to.to_crs(c))?),
            None => self.unapply(c)?,
        })
    }
}
