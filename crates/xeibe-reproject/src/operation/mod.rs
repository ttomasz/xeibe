//! Coordinate operations between datums: the methods behind EPSG's
//! transformations (`docs/reprojection.md`, levels 2 and 3).
//!
//! A method works on the internal coordinates of its operation's own source
//! and target CRSs (see `crs.rs`): longitude (from the datum's prime
//! meridian), latitude and height in radians and metres for a geographic CRS,
//! X, Y, Z for a geocentric one, easting, northing and height for a projected
//! one. `pipeline.rs` gets coordinates into the operation's source CRS and out
//! of its target CRS.

use std::sync::Arc;

use crate::crs::Crs;
use crate::error::CoordinateError;
use crate::grid::store::GridFile;
use crate::registry::Operation;

pub(crate) mod grid;
pub(crate) mod helmert;
mod offsets;
mod plane;
mod tin;
mod velocity;
mod vertical;

/// One operation's math, set up with its parameters and CRSs.
pub(crate) trait Method: Send + Sync + std::fmt::Debug {
    /// From the operation's source CRS to its target CRS.
    fn forward(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError>;
    /// From the target CRS back to the source CRS.
    fn inverse(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError>;

    /// A geog2D-domain operation: PROJ wraps it in `push`/`pop +v_3`, so it
    /// hands back the input height. Between two such operations PROJ's
    /// pipeline drops the `pop`/`push` pair and the transformed height flows
    /// on; `forward_3d`/`inverse_3d` give that height.
    fn keeps_height(&self) -> bool {
        false
    }

    fn forward_3d(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        self.forward(c)
    }

    fn inverse_3d(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        self.inverse(c)
    }

    /// The grid files the method reads.
    fn grid_files(&self) -> Vec<Arc<GridFile>> {
        Vec::new()
    }
}

/// Why an operation can't be set up.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum MethodError {
    /// xeibe doesn't implement the method (grids, time-dependent, vertical…).
    NotImplemented,
    /// A parameter the method needs is missing.
    MissingParameter(u32),
    /// The operation's CRSs aren't of the kind the method works between.
    WrongCrs(&'static str),
    /// The grid (as EPSG names it) isn't one PROJ-data publishes.
    GridUnavailable(String),
}

/// EPSG parameter codes used by the methods.
pub(crate) mod param {
    pub const LATITUDE_OFFSET: u32 = 8601;
    pub const LONGITUDE_OFFSET: u32 = 8602;
    pub const VERTICAL_OFFSET: u32 = 8603;
    pub const GEOID_HEIGHT: u32 = 8604;
    pub const ORDINATE_1_EVAL_POINT_TARGET: u32 = 8621;
    pub const ORDINATE_2_EVAL_POINT_TARGET: u32 = 8622;
    pub const SCALE_FACTOR_SOURCE_AXES: u32 = 1061;
    pub const ROTATION_SOURCE_AXES: u32 = 8614;
    pub const A0: u32 = 8623;
    pub const A1: u32 = 8624;
    pub const A2: u32 = 8625;
    pub const B0: u32 = 8639;
    pub const B1: u32 = 8640;
    pub const B2: u32 = 8641;
    pub const EASTING_OFFSET: u32 = 8728;
    pub const NORTHING_OFFSET: u32 = 8729;
    pub const X_TRANSLATION: u32 = 8605;
    pub const Y_TRANSLATION: u32 = 8606;
    pub const Z_TRANSLATION: u32 = 8607;
    pub const X_ROTATION: u32 = 8608;
    pub const Y_ROTATION: u32 = 8609;
    pub const Z_ROTATION: u32 = 8610;
    pub const SCALE_DIFFERENCE: u32 = 8611;
    pub const ORDINATE_1_EVALUATION_POINT: u32 = 8617;
    pub const ORDINATE_2_EVALUATION_POINT: u32 = 8618;
    pub const ORDINATE_3_EVALUATION_POINT: u32 = 8667;
}

/// Whether xeibe implements the method with this EPSG code. Selection uses
/// it as PROJ uses "can be exported to a PROJ pipeline".
pub(crate) fn is_implemented(method: u32) -> bool {
    helmert::METHODS.contains(&method)
        || offsets::METHODS.contains(&method)
        || plane::METHODS.contains(&method)
        || grid::METHODS.contains(&method)
        || velocity::METHODS.contains(&method)
        || tin::METHODS.contains(&method)
        || vertical::is_method(method)
}

/// Whether xeibe can run this operation: its method is implemented and, for
/// the few methods with conditions on the operation, PROJ could write it as
/// a pipeline. Whether its grid is available is a separate question.
pub(crate) fn is_exportable(op: &Operation) -> bool {
    if grid::METHODS.contains(&op.method) {
        return grid::is_exportable(op);
    }
    if velocity::METHODS.contains(&op.method) {
        return velocity::is_exportable(op);
    }
    if vertical::is_method(op.method) {
        return vertical::is_exportable(op);
    }
    is_implemented(op.method)
}

/// A geoid model read from a grid (`vgridshift` backwards in PROJ).
/// A datum shift that models the ellipsoidal height too (the Helmert
/// family, NADCON5 3D): it changes an ellipsoidal height, and leaves any
/// other height alone.
pub(crate) fn shifts_ellipsoidal_height(method: u32) -> bool {
    helmert::is_helmert(method) || method == grid::NADCON5_3D
}

pub(crate) fn is_geoid_grid(method: u32) -> bool {
    vertical::GEOID_GRID_METHODS.contains(&method)
}

/// The sign a vertical offset grid applies its value with (`vgridshift`
/// forwards), for the methods that are one.
pub(crate) fn offset_grid_sign(method: u32) -> Option<f64> {
    vertical::offset_grid_sign(method)
}

/// The math for one registry operation between its two CRSs.
pub(crate) fn build(op: &Operation, src: &Crs, dst: &Crs) -> Result<Arc<dyn Method>, MethodError> {
    if helmert::METHODS.contains(&op.method) {
        return helmert::build(op, src, dst);
    }
    if offsets::METHODS.contains(&op.method) {
        return offsets::build(op, src, dst);
    }
    if plane::METHODS.contains(&op.method) {
        return plane::build(op, src, dst);
    }
    if grid::METHODS.contains(&op.method) {
        return grid::build(op, src, dst);
    }
    if velocity::METHODS.contains(&op.method) {
        return velocity::build(op, src, dst);
    }
    if tin::METHODS.contains(&op.method) {
        return tin::build(op, src, dst);
    }
    if vertical::is_method(op.method) {
        return vertical::build(op, src, dst);
    }
    Err(MethodError::NotImplemented)
}

/// A parameter the method can't do without.
pub(crate) fn required(op: &Operation, code: u32) -> Result<f64, MethodError> {
    op.param(code).ok_or(MethodError::MissingParameter(code))
}
