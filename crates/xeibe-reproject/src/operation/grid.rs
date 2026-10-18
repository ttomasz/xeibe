//! Grid-based transformations between geographic CRSs (level 3): NTv2,
//! NTv1 and NADCON (PROJ's `hgridshift`), NADCON5 (`gridshift`), and IGN's
//! geocentric translations by grid (`xyzgridshift`).
//!
//! Wrapped the way PROJ 9.8.1 exports these EPSG methods
//! (`SingleOperation::exportToPROJString`, `src/iso19111/operation/
//! singleoperation.cpp`, MIT, see PROJ-NOTICE.md): the grid of PROJ-data's
//! catalog, applied inverted when PROJ-data stores it the other way round;
//! horizontal shifts on longitude and latitude, leaving the height alone;
//! the geocentric translations between `push`/`pop +v_3`, through
//! geocentric coordinates on each CRS's ellipsoid, with the grid read at
//! the position on the interpolation CRS's ellipsoid.

use std::sync::{Arc, OnceLock};

use super::{Method, MethodError};
use crate::crs::{Crs, Kind};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;
use crate::geocentric::{cartesian, geodetic};
use crate::grid::set::{GInfo, HInfo, Node, generic, horizontal};
use crate::grid::shift::{GenericShift, hgrid_apply, three_samples};
use crate::grid::store::{self, GridFile, GridState};
use crate::grid::tiff::{Image, NotLoaded};
use crate::registry::{Operation, registry};

/// EPSG method codes handled here.
pub(super) const METHODS: &[u32] = &[
    9613, // NADCON
    9614, // NTv1
    9615, // NTv2
    1074, // NADCON5 (2D)
    1075, // NADCON5 (3D)
    1087, // Geocentric translations (geog2D domain) by grid (IGN)
];

pub(crate) const NADCON5_3D: u32 = 1075;
pub(crate) const GEOCENTRIC_BY_GRID: u32 = 1087;
/// The methods PROJ writes with an `xyzgridshift` step.
pub(crate) const XYZGRIDSHIFT_METHODS: &[u32] =
    &[GEOCENTRIC_BY_GRID, super::velocity::GRID_AND_VELOCITY];
/// "Latitude and longitude difference file".
const LAT_LON_FILE: u32 = 8656;
/// "Latitude difference file".
const LAT_FILE: u32 = 8657;
/// "Geocentric translation file".
const GEOCENTRIC_FILE: u32 = 8727;
/// "EPSG code for Interpolation CRS".
const INTERPOLATION_CRS: u32 = 1048;

/// The parameter holding the grid PROJ reads, for the methods where the
/// operation has more than one file.
pub(crate) fn file_parameter(method: u32) -> Option<u32> {
    match method {
        9614 | 9615 => Some(LAT_LON_FILE),
        9613 | 1074 | 1075 => Some(LAT_FILE),
        GEOCENTRIC_BY_GRID => Some(GEOCENTRIC_FILE),
        _ => None,
    }
}

/// A grid error for one coordinate.
pub(super) fn state_error(s: GridState) -> CoordinateError {
    match s {
        GridState::NotLoaded => CoordinateError::GridNotLoaded,
        GridState::Invalid(_) => CoordinateError::InvalidGrid,
    }
}

pub(super) fn not_loaded(_: NotLoaded) -> CoordinateError {
    CoordinateError::GridNotLoaded
}

fn geographic(crs: &Crs) -> Result<(Ellipsoid, f64), MethodError> {
    match &crs.kind {
        Kind::Geographic { datum } => Ok((datum.ellipsoid, datum.prime_meridian)),
        _ => Err(MethodError::WrongCrs("a geographic CRS")),
    }
}

/// Whether `op` can run: the method is one of these and the operation is
/// one PROJ can write as a pipeline. Grid availability is separate.
pub(crate) fn is_exportable(op: &Operation) -> bool {
    if op.method == GEOCENTRIC_BY_GRID {
        // PROJ needs the interpolation CRS to be the source or the target.
        return interpolation_end(op).is_some();
    }
    true
}

/// Whether the interpolation CRS, the CRS the grid's positions are in, is
/// the operation's target (`Some(true)`) or source (`Some(false)`): the one
/// on the same datum. For every EPSG operation with one this agrees with
/// PROJ's tests, which compare whole CRSs (ignoring axis order for this
/// method, dimension for the velocity grids).
pub(crate) fn interpolation_end(op: &Operation) -> Option<bool> {
    let r = registry();
    let datum = |code: Option<u32>| Some(r.geodetic.get(&code?)?.datum);
    let interp = datum(op.param(INTERPOLATION_CRS).map(|c| c as u32))?;
    if datum(op.dst) == Some(interp) {
        Some(true)
    } else if datum(op.src) == Some(interp) {
        Some(false)
    } else {
        None
    }
}

pub(super) fn build(op: &Operation, src: &Crs, dst: &Crs) -> Result<Arc<dyn Method>, MethodError> {
    let name = op.grid_name().ok_or(MethodError::MissingParameter(
        file_parameter(op.method).unwrap_or(0),
    ))?;
    let entry = crate::grid::on_cdn(name, &op.name)
        .ok_or_else(|| MethodError::GridUnavailable(name.to_string()))?;
    let file = store::file(entry.file);
    let (src_ellipsoid, src_pm) = geographic(src)?;
    let (dst_ellipsoid, dst_pm) = geographic(dst)?;
    match op.method {
        9613..=9615 => Ok(Arc::new(HGridShift {
            file,
            set: OnceLock::new(),
            inverted: entry.inverse,
            pm: (src_pm, dst_pm),
        })),
        1074 | NADCON5_3D => Ok(Arc::new(GridShift {
            file,
            set: OnceLock::new(),
            inverted: entry.inverse,
            pm: (src_pm, dst_pm),
        })),
        GEOCENTRIC_BY_GRID => {
            if entry.inverse {
                return Err(MethodError::WrongCrs("the grid's own direction"));
            }
            let Some(interp_is_target) = interpolation_end(op) else {
                return Err(MethodError::WrongCrs("an interpolation CRS at either end"));
            };
            let ellipsoid = if interp_is_target {
                dst_ellipsoid
            } else {
                src_ellipsoid
            };
            Ok(Arc::new(XyzGridShift {
                shift: XyzShift::new(file, ellipsoid, !interp_is_target),
                src: (src_ellipsoid, src_pm),
                dst: (dst_ellipsoid, dst_pm),
            }))
        }
        _ => Err(MethodError::NotImplemented),
    }
}

/// The file's images and the hierarchy a method builds from them once.
pub(super) fn loaded<'a, P>(
    file: &'a GridFile,
    set: &'a OnceLock<Result<Vec<Node<P>>, String>>,
    build: impl FnOnce(&[Image]) -> Result<Vec<Node<P>>, String>,
) -> Result<(&'a [Image], &'a [Node<P>]), CoordinateError> {
    let images = file.images().map_err(state_error)?;
    match set.get_or_init(|| build(images)) {
        Ok(top) => Ok((images, top)),
        Err(_) => Err(CoordinateError::InvalidGrid),
    }
}

/// `hgridshift`.
#[derive(Debug)]
struct HGridShift {
    file: Arc<GridFile>,
    set: OnceLock<Result<Vec<Node<HInfo>>, String>>,
    /// PROJ-data's grid goes the other way: `+inv`.
    inverted: bool,
    /// Source and target prime meridians.
    pm: (f64, f64),
}

impl HGridShift {
    fn apply(&self, c: [f64; 3], forward: bool) -> Result<[f64; 3], CoordinateError> {
        let (images, top) = loaded(&self.file, &self.set, horizontal)?;
        let (from, to) = if forward {
            (self.pm.0, self.pm.1)
        } else {
            (self.pm.1, self.pm.0)
        };
        match hgrid_apply(top, images, (c[0] + from, c[1]), forward != self.inverted)
            .map_err(not_loaded)?
        {
            Some((lon, lat)) => Ok([lon - to, lat, c[2]]),
            None => Err(CoordinateError::OutsideGrid),
        }
    }
}

impl Method for HGridShift {
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

/// `gridshift` (NADCON5).
#[derive(Debug)]
struct GridShift {
    file: Arc<GridFile>,
    set: OnceLock<Result<GenericSet, String>>,
    inverted: bool,
    pm: (f64, f64),
}

/// The hierarchy and the grid types `checkGridTypes` finds in it.
type GenericSet = (Vec<Node<GInfo>>, Arc<GenericShift>);

impl GridShift {
    fn apply(&self, c: [f64; 3], forward: bool) -> Result<[f64; 3], CoordinateError> {
        let images = self.file.images().map_err(state_error)?;
        let set = self.set.get_or_init(|| {
            let top = generic(images)?;
            let shift = GenericShift::new(images, &top)?;
            Ok((top, Arc::new(shift)))
        });
        let Ok((top, shift)) = set else {
            return Err(CoordinateError::InvalidGrid);
        };
        let (from, to) = if forward {
            (self.pm.0, self.pm.1)
        } else {
            (self.pm.1, self.pm.0)
        };
        let xyz = [c[0] + from, c[1], c[2]];
        match shift
            .apply(images, top, xyz, forward != self.inverted)
            .map_err(not_loaded)?
        {
            Ok(Some(out)) => Ok([out[0] - to, out[1], out[2]]),
            Ok(None) => Err(CoordinateError::OutsideGrid),
            Err(_) => Err(CoordinateError::InvalidGrid),
        }
    }
}

impl Method for GridShift {
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

/// PROJ's `xyzgridshift` step on geocentric coordinates: translations read
/// at the input position (`grid_ref=input_crs`), or at the output one,
/// found by iteration.
#[derive(Debug)]
pub(crate) struct XyzShift {
    file: Arc<GridFile>,
    set: OnceLock<Result<Vec<Node<GInfo>>, String>>,
    /// The ellipsoid the grid is read on (the interpolation CRS's).
    ellipsoid: Ellipsoid,
    grid_ref_is_input: bool,
}

const TRANSLATIONS: [&str; 3] = ["x_translation", "y_translation", "z_translation"];

impl XyzShift {
    pub(crate) fn new(
        file: Arc<GridFile>,
        ellipsoid: Ellipsoid,
        grid_ref_is_input: bool,
    ) -> XyzShift {
        XyzShift {
            file,
            set: OnceLock::new(),
            ellipsoid,
            grid_ref_is_input,
        }
    }

    pub(crate) fn file(&self) -> Arc<GridFile> {
        self.file.clone()
    }

    fn translations(&self, xyz: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        let (images, top) = loaded(&self.file, &self.set, generic)?;
        let [lon, lat, _] = geodetic(xyz, &self.ellipsoid);
        match three_samples(images, top, (lon, lat), TRANSLATIONS, "metre").map_err(not_loaded)? {
            Ok(Some(d)) => Ok(d),
            Ok(None) => Err(CoordinateError::OutsideGrid),
            Err(_) => Err(CoordinateError::InvalidGrid),
        }
    }

    /// `direct_adjustment`.
    fn direct(&self, p: [f64; 3], factor: f64) -> Result<[f64; 3], CoordinateError> {
        let d = self.translations(p)?;
        Ok([
            p[0] + factor * d[0],
            p[1] + factor * d[1],
            p[2] + factor * d[2],
        ])
    }

    /// `iterative_adjustment`.
    fn iterative(&self, init: [f64; 3], factor: f64) -> Result<[f64; 3], CoordinateError> {
        let mut p = init;
        for _ in 0..10 {
            let d = self.translations(p)?;
            let d = [d[0] * factor, d[1] * factor, d[2] * factor];
            let err = ((p[0] - init[0]) - d[0]).powi(2)
                + ((p[1] - init[1]) - d[1]).powi(2)
                + ((p[2] - init[2]) - d[2]).powi(2);
            p = [init[0] + d[0], init[1] + d[1], init[2] + d[2]];
            if err < 1e-10 {
                break;
            }
        }
        Ok(p)
    }

    pub(crate) fn forward(&self, p: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        if self.grid_ref_is_input {
            self.direct(p, 1.0)
        } else {
            self.iterative(p, 1.0)
        }
    }

    pub(crate) fn inverse(&self, p: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        if self.grid_ref_is_input {
            self.iterative(p, -1.0)
        } else {
            self.direct(p, -1.0)
        }
    }
}

/// `xyzgridshift` between geographic CRSs, through geocentric coordinates.
#[derive(Debug)]
struct XyzGridShift {
    shift: XyzShift,
    /// Ellipsoid and prime meridian of the source and the target.
    src: (Ellipsoid, f64),
    dst: (Ellipsoid, f64),
}

impl XyzGridShift {
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
        let shifted = if forward {
            self.shift.forward(xyz)?
        } else {
            self.shift.inverse(xyz)?
        };
        let [lon, lat, h] = geodetic(shifted, &to.0);
        Ok([lon - to.1, lat, if keep_height { c[2] } else { h }])
    }
}

impl Method for XyzGridShift {
    fn forward(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        self.apply(c, true, true)
    }

    fn inverse(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        self.apply(c, false, true)
    }

    fn keeps_height(&self) -> bool {
        true
    }

    fn forward_3d(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        self.apply(c, true, false)
    }

    fn inverse_3d(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        self.apply(c, false, false)
    }

    fn grid_files(&self) -> Vec<Arc<GridFile>> {
        vec![self.shift.file()]
    }
}
