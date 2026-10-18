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
//! Operations that need an epoch of the coordinates (point motion) are not
//! handled.

use std::sync::{Arc, OnceLock};

use super::grid::{XyzShift, interpolation_end, loaded, not_loaded};
use super::helmert::Helmert;
use super::{Method, MethodError, param, required};
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
/// The operation's source and target epochs (stored in seconds).
const SOURCE_EPOCH: u32 = 1068;
const TARGET_EPOCH: u32 = 1069;
/// EPSG's year (unit 1029), in seconds.
const YEAR: f64 = 31_556_925.445;

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
        let v = self.velocity(p)?;
        Ok([
            p[0] + self.dt * v[0],
            p[1] + self.dt * v[1],
            p[2] + self.dt * v[2],
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
