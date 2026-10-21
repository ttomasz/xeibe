//! Projection methods, keyed by EPSG method code.
//!
//! Each method takes its parameters from the PROJJSON conversion, already in
//! SI units (radians, metres, plain factors), and works in metres on the
//! ellipsoid of the base CRS. Longitudes are counted from the base CRS's prime
//! meridian, as EPSG defines the parameters.

use std::f64::consts::{FRAC_PI_2, PI};
use std::sync::Arc;

use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

mod aea;
mod aeqd;
mod authalic_q;
pub(crate) mod auxlat;
mod bonne;
mod cass;
mod cea;
mod col_urban;
mod common;
mod eqc;
mod eqearth;
mod geodesic;
mod krovak;
mod labrd;
mod laea;
mod lcc;
mod lcc_variants;
mod merc;
mod mlfn;
mod nzmg;
mod omerc;
mod ortho;
mod poly;
mod stere;
mod sterea;
mod tmerc;
mod tmerc_variants;

/// One projection, set up with its parameters.
pub(crate) trait Projection: Send + Sync + std::fmt::Debug {
    /// Longitude and latitude in radians to easting and northing in metres,
    /// false origin included. The longitude is any value within ±10 radians;
    /// the method reduces it against its own origin.
    fn forward(&self, lon: f64, lat: f64) -> Result<(f64, f64), CoordinateError>;

    /// Easting and northing in metres to longitude and latitude in radians.
    /// The longitude need not be reduced to ±π; the caller does that.
    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError>;
}

/// Conversion parameters by EPSG parameter code, in SI units.
#[derive(Debug, Default, Clone)]
pub(crate) struct Params {
    values: Vec<(u32, f64)>,
}

impl Params {
    pub(crate) fn push(&mut self, code: u32, value: f64) {
        self.values.push((code, value));
    }

    /// The same parameters, in any order, with values within PROJ's
    /// relative tolerance (1e-10).
    pub(crate) fn same(&self, other: &Params) -> bool {
        let close = |a: f64, b: f64| a == b || (a - b).abs() <= 1e-10 * a.abs().max(b.abs());
        self.values.len() == other.values.len()
            && self.values.iter().all(|(code, value)| {
                other
                    .values
                    .iter()
                    .any(|(c, v)| c == code && close(*value, *v))
            })
    }

    pub(crate) fn get(&self, code: u32) -> Result<f64, BuildError> {
        self.values
            .iter()
            .find(|(c, _)| *c == code)
            .map(|(_, v)| *v)
            .ok_or(BuildError::MissingParameter(code))
    }
}

/// EPSG parameter codes.
pub(crate) mod param {
    pub const LAT_NATURAL_ORIGIN: u32 = 8801;
    pub const LON_NATURAL_ORIGIN: u32 = 8802;
    pub const SCALE_NATURAL_ORIGIN: u32 = 8805;
    pub const FALSE_EASTING: u32 = 8806;
    pub const FALSE_NORTHING: u32 = 8807;
    pub const LAT_PROJECTION_CENTRE: u32 = 8811;
    pub const LON_PROJECTION_CENTRE: u32 = 8812;
    pub const AZIMUTH_PROJECTION_CENTRE: u32 = 8813;
    pub const ANGLE_TO_SKEW_GRID: u32 = 8814;
    pub const SCALE_PROJECTION_CENTRE: u32 = 8815;
    pub const EASTING_PROJECTION_CENTRE: u32 = 8816;
    pub const NORTHING_PROJECTION_CENTRE: u32 = 8817;
    pub const SCALE_PSEUDO_STANDARD_PARALLEL: u32 = 8819;
    pub const LAT_STD_PARALLEL: u32 = 8832;
    pub const LON_OF_ORIGIN: u32 = 8833;
    pub const LAT_PSEUDO_STANDARD_PARALLEL: u32 = 8818;
    pub const EVALUATION_POINT_ORDINATE_1: u32 = 8617;
    pub const EVALUATION_POINT_ORDINATE_2: u32 = 8618;
    /// C1 to C10 are 1026 to 1035.
    pub const KROVAK_C1: u32 = 1026;
    pub const PROJECTION_PLANE_ORIGIN_HEIGHT: u32 = 1039;
    pub const LAT_FALSE_ORIGIN: u32 = 8821;
    pub const LON_FALSE_ORIGIN: u32 = 8822;
    pub const LAT_1ST_PARALLEL: u32 = 8823;
    pub const LAT_2ND_PARALLEL: u32 = 8824;
    pub const ELLIPSOID_SCALE_FACTOR: u32 = 1038;
    pub const EASTING_FALSE_ORIGIN: u32 = 8826;
    pub const NORTHING_FALSE_ORIGIN: u32 = 8827;
}

#[derive(Debug)]
pub(crate) enum BuildError {
    UnknownMethod,
    MissingParameter(u32),
    Invalid(String),
}

/// The projection for an EPSG method code. The name is only for messages.
pub(crate) fn build(
    code: Option<u32>,
    _name: &str,
    params: &Params,
    ellipsoid: &Ellipsoid,
) -> Result<Arc<dyn Projection>, BuildError> {
    let Some(code) = code else {
        return Err(BuildError::UnknownMethod);
    };
    Ok(match code {
        9807 => Arc::new(tmerc::TransverseMercator::new(params, ellipsoid)?),
        9801 => Arc::new(lcc::LambertConic::one_sp(params, ellipsoid)?),
        9802 => Arc::new(lcc::LambertConic::two_sp(params, ellipsoid)?),
        9808 => Arc::new(tmerc_variants::south_orientated(params, ellipsoid)?),
        1111 => Arc::new(tmerc_variants::three_d(params, ellipsoid)?),
        9803 => Arc::new(lcc_variants::belgium(params, ellipsoid)?),
        1051 => Arc::new(lcc_variants::michigan(params, ellipsoid)?),
        1102 => Arc::new(lcc_variants::one_sp_variant_b(params, ellipsoid)?),
        9804 => Arc::new(merc::Mercator::variant_a(params, ellipsoid)?),
        9805 => Arc::new(merc::Mercator::variant_b(params, ellipsoid)?),
        1024 => Arc::new(merc::Mercator::pseudo(params, ellipsoid)?),
        9841 => Arc::new(merc::Mercator::spherical(params, ellipsoid)?),
        9812 => omerc::ObliqueMercator::variant_a(params, ellipsoid)?,
        9815 => omerc::ObliqueMercator::variant_b(params, ellipsoid)?,
        9813 => Arc::new(labrd::Laborde::new(params, ellipsoid)?),
        9811 => Arc::new(nzmg::NewZealandMapGrid::new(params, ellipsoid)?),
        9819 | 1042 => Arc::new(krovak::Krovak::new(params, ellipsoid, code == 1042)?),
        1041 | 1043 => Arc::new(krovak::Krovak::new(params, ellipsoid, code == 1043)?),
        9806 => Arc::new(cass::Cassini::new(params, ellipsoid, false)?),
        9833 => Arc::new(cass::Cassini::new(params, ellipsoid, true)?),
        9822 => Arc::new(aea::AlbersEqualArea::new(params, ellipsoid)?),
        9818 => Arc::new(poly::Polyconic::new(params, ellipsoid)?),
        1078 => Arc::new(eqearth::EqualEarth::new(params, ellipsoid)?),
        1028 | 9842 | 1029 | 9823 => Arc::new(eqc::EquidistantCylindrical::new(params, ellipsoid)?),
        9835 => Arc::new(cea::CylindricalEqualArea::ellipsoidal(params, ellipsoid)?),
        9834 => Arc::new(cea::CylindricalEqualArea::authalic_sphere(
            params, ellipsoid,
        )?),
        9827 | 9828 => Arc::new(bonne::Bonne::new(params, ellipsoid)?),
        1052 => Arc::new(col_urban::ColombiaUrban::new(params, ellipsoid)?),
        9809 => Arc::new(sterea::ObliqueStereographic::new(params, ellipsoid)?),
        9810 => Arc::new(stere::Stereographic::variant_a(params, ellipsoid)?),
        9829 => Arc::new(stere::Stereographic::variant_b(params, ellipsoid)?),
        9820 => Arc::new(laea::LambertAzimuthal::new(params, ellipsoid)?),
        1027 => Arc::new(laea::LambertAzimuthal::spherical(params, ellipsoid)?),
        1125 | 9832 => Arc::new(aeqd::AzimuthalEquidistant::exact(params, ellipsoid)?),
        9831 => Arc::new(aeqd::AzimuthalEquidistant::guam(params, ellipsoid)?),
        1130 => Arc::new(ortho::LocalOrthographic::new(params, ellipsoid)?),
        _ => return Err(BuildError::UnknownMethod),
    })
}

/// Reduce a longitude to [−π, π], as PROJ's `adjlon`.
pub(crate) fn adjlon(lon: f64) -> f64 {
    // Let lon slightly overshoot, to avoid spurious sign switching at the
    // date line.
    if lon.abs() < PI + 1e-12 {
        return lon;
    }
    let mut lon = lon + PI;
    lon -= 2.0 * PI * (lon / (2.0 * PI)).floor();
    lon - PI
}

/// Half π plus PROJ's tolerance for latitudes that overshoot the pole.
pub(crate) const LAT_LIMIT: f64 = FRAC_PI_2 + 1e-12;
