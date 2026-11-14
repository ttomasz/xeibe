//! Velocity grids applied over a fixed time span (level 3): EPSG's
//! "Geographic3D Offset using NEU velocity grid (NTv2_Vel)" between
//! realizations of NAD83(CSRS), and "Position Vector (geocen) & Geocen
//! translations NEU velocities (gtg)" from NKG_ETRF14 to the Nordic and
//! Baltic ETRS89 realizations.
//!
//! Also "Geocen translations by grid (gtg) & Geocen translations NEU
//! velocities (gtg)" (NKG_ETRF14 to ETRS89-NOR).
//!
//! These need no coordinate epoch: the time span is fixed by the CRSs'
//! anchor epochs or the operation's own epochs, as PROJ 9.8.1 writes them
//! (`+proj=deformation +dt=…`; `SingleOperation::exportToPROJString` and
//! `src/transformations/deformation.cpp`, MIT, see PROJ-NOTICE.md).
//!
//! Point motion operations ([`PointMotion`]) are the same grids applied
//! within one CRS over the span between two coordinate epochs: EPSG's
//! "Point motion (geog3D domain) using NEU velocity grid (NTv2_Vel)"
//! (NAD83(CSRS)) and "Point motion (geocen domain) using NEU velocity grid
//! (Gravsoft)" (NKG_RF17vel in ETRF2014), as PROJ's
//! `PointMotionOperation::_exportToPROJString` writes them.

use std::sync::{Arc, OnceLock};

use super::grid::{XyzShift, interpolation_end, loaded, not_loaded};
use super::helmert::Helmert;
use super::{Method, MethodError, YEAR, param, required};
use crate::crs::{Crs, Kind};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;
use crate::geocentric::{cartesian, geodetic};
use crate::grid::set::{GInfo, Node, generic};
use crate::grid::shift::three_samples;
use crate::grid::store::{self, GridFile};
use crate::registry::{Operation, registry};

/// EPSG method codes handled here.
pub(super) const METHODS: &[u32] = &[
    GEOG3D_OFFSET_BY_VELOCITY,
    HELMERT_AND_VELOCITY,
    GRID_AND_VELOCITY,
];

const GEOG3D_OFFSET_BY_VELOCITY: u32 = 1114;
const HELMERT_AND_VELOCITY: u32 = 1143;
pub(crate) const GRID_AND_VELOCITY: u32 = 1142;
/// "Geocentric translation file".
const TRANSLATION_FILE: u32 = 8727;
/// "Point motion velocity grid file".
const VELOCITY_FILE: u32 = 1050;
/// "Point motion velocity north grid file": of the Gravsoft method's three
/// files, the one PROJ's catalog maps to the single GeoTIFF.
pub(super) const VELOCITY_NORTH_FILE: u32 = 1072;

/// EPSG's point motion methods run here: the geog3D-domain NTv2_Vel grid
/// and the geocentric-domain Gravsoft grids. PROJ runs both (and the
/// deprecated 1070, whose one operation is deprecated too) as
/// `deformation` on geocentric coordinates.
const POINT_MOTION_NTV2_VEL: u32 = 1141;
pub(super) const POINT_MOTION_GRAVSOFT: u32 = 1139;

/// The parameter naming a point motion operation's velocity grid, for the
/// methods handled.
fn point_motion_file(method: u32) -> Option<u32> {
    match method {
        POINT_MOTION_NTV2_VEL => Some(VELOCITY_FILE),
        POINT_MOTION_GRAVSOFT => Some(VELOCITY_NORTH_FILE),
        _ => None,
    }
}

/// Whether the operation is a point motion operation xeibe can run (its
/// grid's availability aside): from a CRS to itself, by a handled method.
pub(crate) fn is_point_motion(op: &Operation) -> bool {
    op.src.is_some() && op.src == op.dst && point_motion_file(op.method).is_some()
}

/// EPSG's live point motion operations for the CRSs on a datum, as PROJ's
/// `getPointMotionOperationsFor` finds them: from one of the datum's CRSs
/// to itself.
fn point_motions(datum: u32) -> impl Iterator<Item = &'static Operation> {
    let r = registry();
    r.crs_by_datum
        .get(&datum)
        .into_iter()
        .flatten()
        .filter_map(|crs| r.by_crs.get(crs))
        .flatten()
        .filter_map(|code| r.ops.get(code))
        .filter(|op| !op.deprecated && op.src.is_some() && op.src == op.dst)
}

/// The point motion operation for the CRSs on a datum, among those xeibe
/// can run. EPSG has at most one per datum; the lowest code if it ever has
/// more.
pub(crate) fn point_motion_for(datum: u32) -> Option<&'static Operation> {
    point_motions(datum)
        .filter(|op| is_point_motion(op))
        .min_by_key(|op| op.code)
}

/// A point motion operation EPSG has for the datum's CRSs that xeibe can't
/// run: Indonesia's and New Caledonia's, by grids of geocentric velocities
/// (methods 1086 and 1120) that PROJ-data doesn't have and PROJ doesn't
/// read. For the error that names it.
pub(crate) fn unsupported_point_motion_for(datum: u32) -> Option<&'static Operation> {
    point_motions(datum)
        .filter(|op| !is_point_motion(op))
        .min_by_key(|op| op.code)
}

/// The operation's source and target epochs (stored in seconds).
const SOURCE_EPOCH: u32 = 1068;
const TARGET_EPOCH: u32 = 1069;

/// Whether PROJ can write the operation as a pipeline (its grid's
/// availability aside).
pub(crate) fn is_exportable(op: &Operation) -> bool {
    match op.method {
        GEOG3D_OFFSET_BY_VELOCITY => epochs(op).is_some() && interpolation_end(op).is_some(),
        HELMERT_AND_VELOCITY | GRID_AND_VELOCITY => {
            op.param(SOURCE_EPOCH).is_some() && op.param(TARGET_EPOCH).is_some()
        }
        _ => false,
    }
}

/// The source and target datums' anchor epochs.
fn epochs(op: &Operation) -> Option<(f64, f64)> {
    Some((anchor_epoch(op.src?)?, anchor_epoch(op.dst?)?))
}

/// The anchor epoch of a geodetic or vertical CRS's datum.
pub(crate) fn anchor_epoch(crs: u32) -> Option<f64> {
    let r = registry();
    let datum = match (r.geodetic.get(&crs), r.vertical.get(&crs)) {
        (Some(g), _) => g.datum,
        (None, Some(v)) => v.datum,
        (None, None) => return None,
    };
    r.datums.get(&datum)?.anchor_epoch
}

fn velocity_file(op: &Operation) -> Result<Arc<GridFile>, MethodError> {
    grid_file(op, VELOCITY_FILE)
}

/// A point motion operation: the velocities of a grid over a time span,
/// applied to coordinates of one CRS (PROJ's `deformation +dt`, between
/// `cart` steps for a geographic CRS).
#[derive(Debug)]
pub(crate) struct PointMotion {
    deformation: Deformation,
    /// The prime meridian of a geographic CRS; `None` for a geocentric one,
    /// whose coordinates the velocities apply to as they are.
    geographic: Option<f64>,
}

impl PointMotion {
    /// The operation on coordinates of `crs`, the CRS it is defined on.
    pub(crate) fn new(op: &Operation, crs: &Crs) -> Result<PointMotion, MethodError> {
        let file = point_motion_file(op.method).ok_or(MethodError::NotImplemented)?;
        let (datum, geographic) = match &crs.kind {
            Kind::Geographic { datum } => (datum, Some(datum.prime_meridian)),
            Kind::Geocentric { datum } => (datum, None),
            _ => return Err(MethodError::WrongCrs("a geographic or geocentric CRS")),
        };
        Ok(PointMotion {
            deformation: Deformation::new(grid_file(op, file)?, 0.0, datum.ellipsoid),
            geographic,
        })
    }

    /// The coordinates `years` later (earlier, if negative), in the CRS's
    /// internal form. Always the velocity at the point given: going back
    /// in time is the operation with its epochs exchanged, as in PROJ, not
    /// the inverse of going forward.
    pub(crate) fn moved(&self, c: [f64; 3], years: f64) -> Result<[f64; 3], CoordinateError> {
        let ellipsoid = &self.deformation.ellipsoid;
        match self.geographic {
            Some(pm) => {
                let xyz = cartesian([c[0] + pm, c[1], c[2]], ellipsoid);
                let [lon, lat, h] = geodetic(self.deformation.moved(xyz, years)?, ellipsoid);
                Ok([lon - pm, lat, h])
            }
            None => self.deformation.moved(c, years),
        }
    }

    pub(crate) fn grid_files(&self) -> Vec<Arc<GridFile>> {
        vec![self.deformation.file()]
    }
}

fn grid_file(op: &Operation, parameter: u32) -> Result<Arc<GridFile>, MethodError> {
    let name = op
        .file(parameter)
        .ok_or(MethodError::MissingParameter(parameter))?;
    let entry = crate::grid::on_cdn(name, &op.name)
        .ok_or_else(|| MethodError::GridUnavailable(name.to_string()))?;
    Ok(store::file(entry.file))
}

pub(super) fn build(op: &Operation, src: &Crs, dst: &Crs) -> Result<Arc<dyn Method>, MethodError> {
    match op.method {
        GEOG3D_OFFSET_BY_VELOCITY => {
            let geographic = |c: &Crs| match &c.kind {
                Kind::Geographic { datum } => {
                    Ok((datum.ellipsoid, datum.prime_meridian, c.dimension()))
                }
                _ => Err(MethodError::WrongCrs("a geographic CRS")),
            };
            let (src_e, src_pm, src_dim) = geographic(src)?;
            let (dst_e, dst_pm, dst_dim) = geographic(dst)?;
            let (from, to) = epochs(op).ok_or(MethodError::WrongCrs("CRSs with anchor epochs"))?;
            let target = interpolation_end(op)
                .ok_or(MethodError::WrongCrs("an interpolation CRS at either end"))?;
            Ok(Arc::new(GeographicVelocity {
                deformation: Deformation::new(
                    velocity_file(op)?,
                    to - from,
                    if target { dst_e } else { src_e },
                ),
                src: (src_e, src_pm),
                dst: (dst_e, dst_pm),
                // PROJ's `push`/`pop +v_3` when either CRS is 2D.
                keeps_height: src_dim == 2 || dst_dim == 2,
            }))
        }
        HELMERT_AND_VELOCITY => {
            let Kind::Geocentric { datum } = &src.kind else {
                return Err(MethodError::WrongCrs("a geocentric CRS"));
            };
            if !matches!(dst.kind, Kind::Geocentric { .. }) {
                return Err(MethodError::WrongCrs("a geocentric CRS"));
            }
            let t = [
                required(op, param::X_TRANSLATION)?,
                required(op, param::Y_TRANSLATION)?,
                required(op, param::Z_TRANSLATION)?,
            ];
            let rot = [
                required(op, param::X_ROTATION)?,
                required(op, param::Y_ROTATION)?,
                required(op, param::Z_ROTATION)?,
            ];
            let s = required(op, param::SCALE_DIFFERENCE)?;
            let dt = (required(op, TARGET_EPOCH)? - required(op, SOURCE_EPOCH)?) / YEAR;
            Ok(Arc::new(HelmertVelocity {
                helmert: Helmert::new(t, rot, s, true, [0.0; 3], false),
                deformation: Deformation::new(velocity_file(op)?, dt, datum.ellipsoid),
            }))
        }
        GRID_AND_VELOCITY => {
            let Kind::Geocentric { datum } = &src.kind else {
                return Err(MethodError::WrongCrs("a geocentric CRS"));
            };
            if !matches!(dst.kind, Kind::Geocentric { .. }) {
                return Err(MethodError::WrongCrs("a geocentric CRS"));
            }
            let dt = (required(op, TARGET_EPOCH)? - required(op, SOURCE_EPOCH)?) / YEAR;
            // PROJ writes the `xyzgridshift` step without an ellipsoid,
            // which defaults to GRS 1980: NKG_ETRF14's own.
            Ok(Arc::new(GridVelocity {
                shift: XyzShift::new(grid_file(op, TRANSLATION_FILE)?, datum.ellipsoid, true),
                deformation: Deformation::new(velocity_file(op)?, dt, datum.ellipsoid),
            }))
        }
        _ => Err(MethodError::NotImplemented),
    }
}

/// Geocentric translations by grid, then the velocities over the time span.
#[derive(Debug)]
struct GridVelocity {
    shift: XyzShift,
    deformation: Deformation,
}

impl Method for GridVelocity {
    fn forward(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        self.deformation.forward(self.shift.forward(c)?)
    }

    fn inverse(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        self.shift.inverse(self.deformation.inverse(c)?)
    }

    fn grid_files(&self) -> Vec<Arc<GridFile>> {
        vec![self.shift.file(), self.deformation.file()]
    }
}

/// A velocity grid: east, north and up velocities.
#[derive(Debug)]
pub(crate) struct VelocityGrid {
    file: Arc<GridFile>,
    set: OnceLock<Result<Vec<Node<GInfo>>, String>>,
}

const VELOCITIES: [&str; 3] = ["east_velocity", "north_velocity", "up_velocity"];

impl VelocityGrid {
    pub(crate) fn new(file: Arc<GridFile>) -> VelocityGrid {
        VelocityGrid {
            file,
            set: OnceLock::new(),
        }
    }

    pub(crate) fn file(&self) -> Arc<GridFile> {
        self.file.clone()
    }

    /// The east, north and up velocities at a longitude and latitude
    /// (radians from Greenwich), in metres a year.
    pub(crate) fn enu(&self, lon: f64, lat: f64) -> Result<[f64; 3], CoordinateError> {
        let (images, top) = loaded(&self.file, &self.set, generic)?;
        match three_samples(images, top, (lon, lat), VELOCITIES, "millimetres per year")
            .map_err(not_loaded)?
        {
            Ok(Some(v)) => Ok(v.map(|v| v / 1000.0)),
            Ok(None) => Err(CoordinateError::OutsideGrid),
            Err(_) => Err(CoordinateError::InvalidGrid),
        }
    }
}

/// PROJ's `deformation` step with a fixed `dt`, on geocentric coordinates.
#[derive(Debug)]
struct Deformation {
    grid: VelocityGrid,
    /// Years.
    dt: f64,
    /// The ellipsoid the grid is read on.
    ellipsoid: Ellipsoid,
}

const TOL: f64 = 1e-8;
const MAX_ITERATIONS: usize = 10;

impl Deformation {
    fn new(file: Arc<GridFile>, dt: f64, ellipsoid: Ellipsoid) -> Deformation {
        Deformation {
            grid: VelocityGrid::new(file),
            dt,
            ellipsoid,
        }
    }

    /// `pj_deformation_get_grid_shift`: the velocity at a point, in metres
    /// a year, as X, Y, Z.
    fn velocity(&self, xyz: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        let [lon, lat, _] = geodetic(xyz, &self.ellipsoid);
        let [e, n, u] = self.grid.enu(lon, lat)?;
        let (sp, cp) = lat.sin_cos();
        let (sl, cl) = lon.sin_cos();
        Ok([
            -sp * cl * n - sl * e + cp * cl * u,
            -sp * sl * n + cl * e + cp * sl * u,
            cp * n + sp * u,
        ])
    }

    fn file(&self) -> Arc<GridFile> {
        self.grid.file()
    }

    fn forward(&self, p: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        self.moved(p, self.dt)
    }

    /// The point `years` on, by the velocity where it is now.
    fn moved(&self, p: [f64; 3], years: f64) -> Result<[f64; 3], CoordinateError> {
        let v = self.velocity(p)?;
        Ok([
            p[0] + years * v[0],
            p[1] + years * v[1],
            p[2] + years * v[2],
        ])
    }

    /// The point that `forward` moves to `input`: `input - dt·v(out)`, by
    /// iteration. (PROJ's `pj_deformation_reverse_shift` steps the wrong
    /// way after its first estimate and leaves Z at that estimate; with
    /// velocities this smooth both stay within a micrometre.) A step that
    /// leaves the grid keeps the estimate so far, as PROJ does.
    fn inverse(&self, input: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        let v = self.velocity(input)?;
        let mut out: [f64; 3] = std::array::from_fn(|k| input[k] - self.dt * v[k]);
        for _ in 0..MAX_ITERATIONS {
            let v = match self.velocity(out) {
                Ok(v) => v,
                Err(CoordinateError::OutsideGrid) => break,
                Err(e) => return Err(e),
            };
            let next: [f64; 3] = std::array::from_fn(|k| input[k] - self.dt * v[k]);
            let change = (0..3).map(|k| (next[k] - out[k]).powi(2)).sum::<f64>();
            out = next;
            if change <= TOL * TOL {
                break;
            }
        }
        Ok(out)
    }
}

/// Geographic3D offset by a velocity grid: through geocentric coordinates on
/// each CRS's ellipsoid.
#[derive(Debug)]
struct GeographicVelocity {
    deformation: Deformation,
    src: (Ellipsoid, f64),
    dst: (Ellipsoid, f64),
    keeps_height: bool,
}

impl GeographicVelocity {
    fn apply(
        &self,
        c: [f64; 3],
        forward: bool,
        keep_height: bool,
    ) -> Result<[f64; 3], CoordinateError> {
        let (from, to) = if forward {
            (self.src, self.dst)
        } else {
            (self.dst, self.src)
        };
        let xyz = cartesian([c[0] + from.1, c[1], c[2]], &from.0);
        let moved = if forward {
            self.deformation.forward(xyz)?
        } else {
            self.deformation.inverse(xyz)?
        };
        let [lon, lat, h] = geodetic(moved, &to.0);
        Ok([lon - to.1, lat, if keep_height { c[2] } else { h }])
    }
}

impl Method for GeographicVelocity {
    fn forward(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        self.apply(c, true, self.keeps_height)
    }

    fn inverse(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        self.apply(c, false, self.keeps_height)
    }

    fn keeps_height(&self) -> bool {
        self.keeps_height
    }

    fn forward_3d(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        self.apply(c, true, false)
    }

    fn inverse_3d(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        self.apply(c, false, false)
    }

    fn grid_files(&self) -> Vec<Arc<GridFile>> {
        vec![self.deformation.file()]
    }
}

/// A Position Vector Helmert, then the velocities over the time span, on
/// geocentric coordinates.
#[derive(Debug)]
struct HelmertVelocity {
    helmert: Helmert,
    deformation: Deformation,
}

impl Method for HelmertVelocity {
    fn forward(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        self.deformation.forward(self.helmert.forward(c))
    }

    fn inverse(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        Ok(self.helmert.inverse(self.deformation.inverse(c)?))
    }

    fn grid_files(&self) -> Vec<Arc<GridFile>> {
        vec![self.deformation.file()]
    }
}
