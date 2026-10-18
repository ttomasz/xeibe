//! Vertical transformations (level 3): geoid models between a geographic 3D
//! CRS and gravity-related heights or depths, vertical offsets (constant,
//! with a slope, by grid, by triangulation, by velocity grid), all applied
//! to the height while the horizontal position stays.
//!
//! As PROJ 9.8.1 writes them (`SingleOperation::exportToPROJString`,
//! `src/iso19111/operation/singleoperation.cpp`; `vgridshift.cpp`,
//! `vertoffset.cpp`, `tinshift.cpp`, `deformation.cpp`; MIT, see
//! PROJ-NOTICE.md):
//!
//! * a geoid model is `vgridshift` run backwards with a multiplier of 1:
//!   H = h − N, N read at the source position; a constant geoid height
//!   (method 1136) is the same with N fixed;
//! * a vertical offset grid is `vgridshift` forwards, H₂ = H₁ ± offset, the
//!   offset read at the position in the operation's interpolation CRS
//!   (NRCan's grids hold the value to subtract);
//! * a constant offset (with or without a slope) is added along the axes.
//!
//! Where EPSG's formula is simpler than PROJ's pipeline, it is EPSG's: the
//! offset and slope takes its radii of curvature on the ellipsoid of the
//! operation's horizontal CRS (PROJ: always GRS 1980), and a vertical
//! velocity grid adds (t₂ − t₁)·v_U to the height (PROJ: through geocentric
//! coordinates and back). Offsets run only between CRSs whose axes point the
//! same way in the same unit, the case EPSG's formulas cover; EPSG's few
//! offsets from a height to a depth or between units are deprecated, and
//! PROJ's reading of them puts the depth on the wrong side.
//!
//! Heights are internal: metres, up. A depth CRS turns them around in its
//! axis, as PROJ's `axisswap` does in the pipeline.

use std::sync::{Arc, OnceLock};

use super::grid::{loaded, not_loaded, state_error};
use super::velocity::{VelocityGrid, anchor_epoch};
use super::{Method, MethodError, required};
use crate::crs::{Crs, Kind};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;
use crate::grid::set::{Node, VInfo, vertical};
use crate::grid::shift::vgrid_value;
use crate::grid::store::{self, GridFile};
use crate::registry::Operation;

/// "Geographic3D to GravityRelatedHeight" methods PROJ runs from a grid:
/// those named so, and the ones it lists by code
/// (`Transformation::isGeographic3DToGravityRelatedHeight`). "Geographic3D
/// to Depth" (gtx) and (txt), 1121 and 1116, are in neither.
pub(crate) const GEOID_GRID_METHODS: &[u32] = &[
    // By name.
    1025, 1030, 1045, 1047, 1048, 1050, 1059, 1060, 1072, 1073, 1081, 1082, 1099, 1106, 1117, 1123,
    1134, 9661, 9662, 9663, 9664, 9665, //
    // By code.
    1083, 1088, 1089, 1090, 1091, 1092, 1093, 1094, 1095, 1096, 1097, 1098, 1100, 1103, 1105, 1109,
    1110, 1115, 1118, 1122, 1124, 1127, 1128, 1135, 9635,
];
/// "Geographic3D to GravityRelatedHeight" and "Geog3D to
/// Geog2D+GravityRelatedHeight" with a constant geoid height.
const CONSTANT_GEOID_METHODS: &[u32] = &[1136, 1131];
/// Vertical offset grids (`isRegularVerticalGridMethod`) and VERTCON, with
/// the sign PROJ applies the grid's value with.
pub(crate) fn offset_grid_sign(method: u32) -> Option<f64> {
    match method {
        // NRCan's grids hold the value to subtract.
        1112 | 1126 => Some(-1.0),
        1071 | 1080 | 1084 | 1085 | 1129 | 1101 | VERTCON => Some(1.0),
        _ => None,
    }
}
const VERTCON: u32 = 9658;
const VERTICAL_OFFSET: u32 = 9616;
const OFFSET_AND_SLOPE: u32 = 1046;
const VERTICAL_TIN: u32 = 1137;
const VERTICAL_VELOCITY: u32 = 1113;

/// EPSG method codes handled here, besides the geoid grids.
const OTHER_METHODS: &[u32] = &[
    1136,
    1131,
    1112,
    1126,
    1071,
    1080,
    1084,
    1085,
    1129,
    1101,
    VERTCON,
    VERTICAL_OFFSET,
    OFFSET_AND_SLOPE,
    VERTICAL_TIN,
    VERTICAL_VELOCITY,
];

pub(super) fn is_method(method: u32) -> bool {
    GEOID_GRID_METHODS.contains(&method) || OTHER_METHODS.contains(&method)
}

/// "Geoid (height correction) model file".
const GEOID_FILE: u32 = 8666;
/// "Geoid height".
const GEOID_HEIGHT: u32 = 8604;
/// "Vertical offset file", "Geoid model difference file".
const OFFSET_FILE: u32 = 8732;
const GEOID_DIFFERENCE_FILE: u32 = 1063;
/// "Vertical Offset".
const OFFSET: u32 = 8603;
/// Offset and slope: the evaluation point, the inclinations.
const EVALUATION_LATITUDE: u32 = 8617;
const EVALUATION_LONGITUDE: u32 = 8618;
const INCLINATION_LATITUDE: u32 = 8730;
const INCLINATION_LONGITUDE: u32 = 8731;
/// "EPSG code for Interpolation CRS", "EPSG code for Horizontal CRS".
const INTERPOLATION_CRS: u32 = 1048;
const HORIZONTAL_CRS: u32 = 1037;
/// "TIN offset file".
const TIN_FILE: u32 = 1064;
/// "Point motion velocity grid file".
const VELOCITY_FILE: u32 = 1050;

/// Whether PROJ can write the operation as a pipeline (its grid's
/// availability aside).
pub(crate) fn is_exportable(op: &Operation) -> bool {
    match op.method {
        // PROJ needs both vertical datums' anchor epochs and a geographic
        // interpolation CRS.
        VERTICAL_VELOCITY => {
            let epochs = |c: Option<u32>| c.and_then(anchor_epoch).is_some();
            epochs(op.src) && epochs(op.dst) && interpolation_geographic(op).is_some()
        }
        _ => true,
    }
}

/// The interpolation CRS's ellipsoid and prime meridian, when it is a
/// geographic CRS.
fn interpolation_geographic(op: &Operation) -> Option<(Ellipsoid, f64)> {
    let code = op.param(INTERPOLATION_CRS)? as u32;
    let crs = Crs::from_epsg(code).ok()?;
    match &crs.kind {
        Kind::Geographic { datum } => Some((datum.ellipsoid, datum.prime_meridian)),
        _ => None,
    }
}

/// The prime meridian the horizontal position is counted from: the
/// interpolation CRS's, else Greenwich.
fn interpolation_meridian(op: &Operation) -> f64 {
    interpolation_geographic(op).map_or(0.0, |(_, pm)| pm)
}

/// The axis direction (+1 up, −1 down) two vertical CRSs share, where
/// they share their unit too.
fn same_axes(src: &Crs, dst: &Crs) -> Result<f64, MethodError> {
    if src.vertical_datum().is_none() || dst.vertical_datum().is_none() {
        return Err(MethodError::WrongCrs("vertical CRSs"));
    }
    let unit = |c: &Crs| c.axes.axes.first().map(|a| a.factor);
    if src.height_direction() != dst.height_direction() || unit(src) != unit(dst) {
        return Err(MethodError::WrongCrs(
            "vertical CRSs with the same axis direction and unit",
        ));
    }
    Ok(src.height_direction())
}

fn file(op: &Operation, parameter: u32) -> Result<Arc<GridFile>, MethodError> {
    let name = op
        .file(parameter)
        .ok_or(MethodError::MissingParameter(parameter))?;
    let entry = crate::grid::on_cdn(name, &op.name)
        .ok_or_else(|| MethodError::GridUnavailable(name.to_string()))?;
    if entry.inverse {
        // PROJ-data stores no vertical grid the other way round.
        return Err(MethodError::WrongCrs("the grid's own direction"));
    }
    Ok(store::file(entry.file))
}

pub(super) fn build(op: &Operation, src: &Crs, dst: &Crs) -> Result<Arc<dyn Method>, MethodError> {
    let m = op.method;
    if GEOID_GRID_METHODS.contains(&m) || CONSTANT_GEOID_METHODS.contains(&m) {
        let Kind::Geographic { datum } = &src.horizontal().kind else {
            return Err(MethodError::WrongCrs("a geographic source CRS"));
        };
        if src.dimension() != 3 || dst.vertical_datum().is_none() {
            return Err(MethodError::WrongCrs(
                "a geographic 3D source and a vertical target",
            ));
        }
        if CONSTANT_GEOID_METHODS.contains(&m) {
            return Ok(Arc::new(ConstantGeoid {
                n: required(op, GEOID_HEIGHT)?,
            }));
        }
        return Ok(Arc::new(GeoidGrid {
            file: file(op, GEOID_FILE)?,
            set: OnceLock::new(),
            pm: datum.prime_meridian,
        }));
    }
    let vertical_ends = || {
        if src.vertical_datum().is_none() || dst.vertical_datum().is_none() {
            return Err(MethodError::WrongCrs("vertical CRSs"));
        }
        Ok(())
    };
    if let Some(sign) = offset_grid_sign(m) {
        let dir = same_axes(src, dst)?;
        let parameter = if op.file(OFFSET_FILE).is_some() {
            OFFSET_FILE
        } else {
            GEOID_DIFFERENCE_FILE
        };
        return Ok(Arc::new(OffsetGrid {
            file: file(op, parameter)?,
            set: OnceLock::new(),
            sign,
            pm: interpolation_meridian(op),
            dir,
        }));
    }
    match m {
        VERTICAL_OFFSET => Ok(Arc::new(Offset {
            offset: required(op, OFFSET)?,
            dir: same_axes(src, dst)?,
        })),
        OFFSET_AND_SLOPE => {
            let dir = same_axes(src, dst)?;
            let horizontal = required(op, HORIZONTAL_CRS)? as u32;
            let e = match Crs::from_epsg(horizontal).map(|c| c.kind) {
                Ok(Kind::Geographic { datum }) => datum.ellipsoid,
                _ => return Err(MethodError::WrongCrs("a geographic horizontal CRS")),
            };
            let phi0 = required(op, EVALUATION_LATITUDE)?;
            let s = phi0.sin();
            let w = 1.0 - e.es * s * s;
            Ok(Arc::new(OffsetAndSlope {
                offset: required(op, OFFSET)?,
                slope_lat: required(op, INCLINATION_LATITUDE)?,
                slope_lon: required(op, INCLINATION_LONGITUDE)?,
                phi0,
                lam0: required(op, EVALUATION_LONGITUDE)?,
                rho0: e.a * (1.0 - e.es) / (w * w.sqrt()),
                nu0: e.a / w.sqrt(),
                dir,
            }))
        }
        VERTICAL_TIN => {
            vertical_ends()?;
            let name = op
                .file(TIN_FILE)
                .ok_or(MethodError::MissingParameter(TIN_FILE))?;
            let entry = crate::grid::on_cdn(name, &op.name)
                .ok_or_else(|| MethodError::GridUnavailable(name.to_string()))?;
            Ok(Arc::new(VerticalTin {
                file: store::file(entry.file),
            }))
        }
        VERTICAL_VELOCITY => {
            vertical_ends()?;
            let (Some(from), Some(to)) =
                (op.src.and_then(anchor_epoch), op.dst.and_then(anchor_epoch))
            else {
                return Err(MethodError::WrongCrs("vertical CRSs with anchor epochs"));
            };
            let (_, pm) = interpolation_geographic(op)
                .ok_or(MethodError::WrongCrs("a geographic interpolation CRS"))?;
            Ok(Arc::new(VerticalVelocity {
                grid: VelocityGrid::new(file(op, VELOCITY_FILE)?),
                dt: to - from,
                pm,
            }))
        }
        _ => Err(MethodError::NotImplemented),
    }
}

fn outside<T>(v: Option<T>) -> Result<T, CoordinateError> {
    v.ok_or(CoordinateError::OutsideGrid)
}

/// A geoid model: H = h − N.
#[derive(Debug)]
struct GeoidGrid {
    file: Arc<GridFile>,
    set: OnceLock<Result<Vec<Node<VInfo>>, String>>,
    /// The source CRS's prime meridian: the grid is in Greenwich longitudes.
    pm: f64,
}

impl GeoidGrid {
    fn n(&self, c: [f64; 3]) -> Result<f64, CoordinateError> {
        let (images, top) = loaded(&self.file, &self.set, vertical)?;
        outside(vgrid_value(top, images, c[0] + self.pm, c[1]).map_err(not_loaded)?)
    }
}

impl Method for GeoidGrid {
    fn forward(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        Ok([c[0], c[1], c[2] - self.n(c)?])
    }

    fn inverse(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        Ok([c[0], c[1], c[2] + self.n(c)?])
    }

    fn grid_files(&self) -> Vec<Arc<GridFile>> {
        vec![self.file.clone()]
    }
}

/// A geoid of constant height.
#[derive(Debug)]
struct ConstantGeoid {
    n: f64,
}

impl Method for ConstantGeoid {
    fn forward(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        Ok([c[0], c[1], c[2] - self.n])
    }

    fn inverse(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        Ok([c[0], c[1], c[2] + self.n])
    }
}

/// Adds `d` along the axes (`dir`: +1 up, −1 down) to a height, metres up.
fn along(c: [f64; 3], dir: f64, d: f64) -> [f64; 3] {
    [c[0], c[1], c[2] + d * dir]
}

/// The reverse of [`along`].
fn back(c: [f64; 3], dir: f64, d: f64) -> [f64; 3] {
    [c[0], c[1], c[2] - d * dir]
}

/// A vertical offset grid.
#[derive(Debug)]
struct OffsetGrid {
    file: Arc<GridFile>,
    set: OnceLock<Result<Vec<Node<VInfo>>, String>>,
    sign: f64,
    pm: f64,
    /// The axes' direction: +1 up, −1 down.
    dir: f64,
}

impl OffsetGrid {
    fn offset(&self, c: [f64; 3]) -> Result<f64, CoordinateError> {
        let (images, top) = loaded(&self.file, &self.set, vertical)?;
        let v = outside(vgrid_value(top, images, c[0] + self.pm, c[1]).map_err(not_loaded)?)?;
        Ok(self.sign * v)
    }
}

impl Method for OffsetGrid {
    fn forward(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        Ok(along(c, self.dir, self.offset(c)?))
    }

    fn inverse(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        Ok(back(c, self.dir, self.offset(c)?))
    }

    fn grid_files(&self) -> Vec<Arc<GridFile>> {
        vec![self.file.clone()]
    }
}

/// A constant vertical offset.
#[derive(Debug)]
struct Offset {
    offset: f64,
    /// The axes' direction: +1 up, −1 down.
    dir: f64,
}

impl Method for Offset {
    fn forward(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        Ok(along(c, self.dir, self.offset))
    }

    fn inverse(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        Ok(back(c, self.dir, self.offset))
    }
}

/// `vertoffset`: an offset and a slope in latitude and longitude about an
/// evaluation point, at the position in the interpolation CRS.
#[derive(Debug)]
struct OffsetAndSlope {
    offset: f64,
    slope_lat: f64,
    slope_lon: f64,
    phi0: f64,
    lam0: f64,
    rho0: f64,
    nu0: f64,
    /// The axes' direction: +1 up, −1 down.
    dir: f64,
}

impl OffsetAndSlope {
    fn at(&self, c: [f64; 3]) -> f64 {
        let (lam, phi) = (c[0] - self.lam0, c[1]);
        self.offset
            + self.slope_lat * self.rho0 * (phi - self.phi0)
            + self.slope_lon * self.nu0 * lam * phi.cos()
    }
}

impl Method for OffsetAndSlope {
    fn forward(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        Ok(along(c, self.dir, self.at(c)))
    }

    fn inverse(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        Ok(back(c, self.dir, self.at(c)))
    }
}

/// `tinshift` on the height, at the position in the interpolation CRS.
#[derive(Debug)]
struct VerticalTin {
    file: Arc<GridFile>,
}

impl VerticalTin {
    fn apply(&self, c: [f64; 3], forward: bool) -> Result<[f64; 3], CoordinateError> {
        let tin = self.file.tin().map_err(state_error)?;
        let out = if forward {
            tin.forward(c[0], c[1], c[2])
        } else {
            tin.inverse(c[0], c[1], c[2])
        };
        let [_, _, z] = outside(out)?;
        Ok([c[0], c[1], z])
    }
}

impl Method for VerticalTin {
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

/// A velocity grid over the span between two vertical datums' anchor
/// epochs, EPSG's H(t₂) = H(t₁) + (t₂ − t₁)·v_U; backwards, the span the
/// other way.
#[derive(Debug)]
struct VerticalVelocity {
    grid: VelocityGrid,
    /// t₂ − t₁, years.
    dt: f64,
    /// The interpolation CRS's prime meridian.
    pm: f64,
}

impl VerticalVelocity {
    fn apply(&self, c: [f64; 3], forward: bool) -> Result<[f64; 3], CoordinateError> {
        let [_, _, up] = self.grid.enu(c[0] + self.pm, c[1])?;
        let dt = if forward { self.dt } else { -self.dt };
        Ok([c[0], c[1], c[2] + dt * up])
    }
}

impl Method for VerticalVelocity {
    fn forward(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        self.apply(c, true)
    }

    fn inverse(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        self.apply(c, false)
    }

    fn grid_files(&self) -> Vec<Arc<GridFile>> {
        vec![self.grid.file()]
    }
}
