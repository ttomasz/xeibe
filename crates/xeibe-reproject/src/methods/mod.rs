//! Projection methods, keyed by EPSG method code.
//!
//! Each method takes its parameters from the PROJJSON conversion, already in
//! SI units (radians, metres, plain factors), and works in metres on the
//! ellipsoid of the base CRS. Longitudes are counted from the base CRS's prime
//! meridian, as EPSG defines the parameters.

use std::f64::consts::{FRAC_PI_2, PI};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

mod adams;
mod aea;
mod aeqd;
mod airocean;
mod authalic_q;
pub(crate) mod auxlat;
mod bipc;
mod bonne;
mod calcofi;
mod cass;
mod cea;
mod col_urban;
mod common;
mod conic;
mod custom;
mod eqc;
mod eqdc;
mod eqearth;
mod geos;
mod goode;
mod gstmerc;
mod healpix;
mod imoll;
mod imw_p;
mod isea;
mod krovak;
mod labrd;
mod laea;
mod lcc;
mod lcc_variants;

pub(crate) use lcc::two_sp_as_one_sp;
pub(crate) use merc::variant_a_as_b;
mod merc;
mod mlfn;
mod nzmg;
mod ob_tran;
pub(crate) use ob_tran::{is_geographic as is_rotated_pole, pole_rotation_as_ob_tran};
mod misc;
mod mod_ster;
mod omerc;
mod ortho;
mod pcyl;
mod perspective;
mod poly;
pub(crate) mod proj_codes;
mod qsc;
mod rouss;
mod s2;
mod sch;
mod som;
mod sphere;
mod spilhaus;
mod stere;
mod sterea;
mod tmerc;
mod tmerc_variants;
mod tpeqd;
mod world;

/// Whether a method or parameter code is EPSG's, not this crate's for one
/// only PROJ defines (`proj_codes`).
pub(crate) fn is_epsg(code: u32) -> bool {
    code != 0 && code < proj_codes::FIRST_METHOD
}

/// The PROJJSON `id` of a method or parameter with this code: EPSG's, or
/// none for one only PROJ defines, which PROJ names without an id.
pub(crate) fn id(code: u32) -> Option<Value> {
    is_epsg(code).then(|| json!({"authority": "EPSG", "code": code}))
}

fn epsg_id(v: &Value) -> Option<u32> {
    (v["id"]["authority"] == "EPSG")
        .then(|| v["id"]["code"].as_u64())
        .flatten()
        .and_then(|c| u32::try_from(c).ok())
}

/// A PROJJSON method's code: its EPSG id, else its EPSG name (WKT may name
/// it without an id), else PROJ's name of one only PROJ defines.
pub(crate) fn method_code(method: &Value) -> Option<u32> {
    let name = method["name"].as_str();
    epsg_id(method)
        .or_else(|| name.and_then(xeibe_crs::method_code))
        .or_else(|| {
            let name = name?;
            proj_codes::METHODS
                .iter()
                .find(|(_, n)| crate::name::equivalent(n, name))
                .map(|&(c, _)| c)
        })
}

/// A PROJJSON parameter's code, as `method_code`.
pub(crate) fn parameter_code(param: &Value) -> Option<u32> {
    let name = param["name"].as_str();
    epsg_id(param)
        .or_else(|| name.and_then(xeibe_crs::parameter_code))
        .or_else(|| {
            let name = name?;
            proj_codes::PARAMS
                .iter()
                .find(|(_, n)| crate::name::equivalent(n, name))
                .map(|&(c, _)| c)
        })
}

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

/// Conversion parameters by EPSG parameter code, in SI units, or for a
/// projection PROJ has no CRS method for ("PROJ hammer"), by PROJ key, with
/// the flags of its method's name.
#[derive(Debug, Default, Clone)]
pub(crate) struct Params {
    values: Vec<(u32, f64)>,
    keys: Vec<(String, f64)>,
    flags: Vec<(String, String)>,
}

impl Params {
    pub(crate) fn push(&mut self, code: u32, value: f64) {
        self.values.push((code, value));
    }

    pub(crate) fn push_key(&mut self, key: &str, value: f64) {
        self.keys.push((key.to_owned(), value));
    }

    /// A flag (`south`, value empty) or a key with a word for its value
    /// (`o_proj=moll`), from a "PROJ" method's name.
    pub(crate) fn push_flag(&mut self, key: &str, value: &str) {
        self.flags.push((key.to_owned(), value.to_owned()));
    }

    /// A PROJ key's value, in SI units.
    pub(crate) fn key(&self, key: &str) -> Option<f64> {
        self.keys.iter().find(|(k, _)| k == key).map(|&(_, v)| v)
    }

    /// A flag's value: empty for a bare flag, `None` when absent.
    pub(crate) fn flag(&self, key: &str) -> Option<&str> {
        self.flags
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// The PROJ keys and their values.
    pub(crate) fn proj_key_values(&self) -> impl Iterator<Item = (&str, f64)> {
        self.keys.iter().map(|(k, v)| (k.as_str(), *v))
    }

    /// The flags and their values (empty for a bare flag).
    pub(crate) fn proj_flags(&self) -> impl Iterator<Item = (&str, &str)> {
        self.flags.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Every PROJ key and flag, to check them against what a projection
    /// takes.
    pub(crate) fn proj_keys(&self) -> impl Iterator<Item = &str> {
        self.keys
            .iter()
            .map(|(k, _)| k.as_str())
            .chain(self.flags.iter().map(|(k, _)| k.as_str()))
    }

    /// Every parameter, by EPSG code, in SI units.
    pub(crate) fn values(&self) -> &[(u32, f64)] {
        &self.values
    }

    /// The same parameters, in any order, with values within PROJ's
    /// relative tolerance for them (2e-10), as PROJ's `SingleOperation` compares
    /// them: a parameter only one side has must have its neutral value (1
    /// for a scale factor, else 0), and for Lambert Conic Conformal (2SP)
    /// the two standard parallels may be swapped.
    pub(crate) fn same(&self, other: &Params, method: Option<u32>) -> bool {
        let close = |a: f64, b: f64| a == b || (a - b).abs() <= 2e-10 * a.abs().max(b.abs());
        let keys = |a: &Params, b: &Params| {
            a.keys
                .iter()
                .all(|(k, v)| b.key(k).is_some_and(|w| close(*v, w)))
        };
        self.covered_by(other, method)
            && other.covered_by(self, method)
            && keys(self, other)
            && keys(other, self)
    }

    fn covered_by(&self, other: &Params, method: Option<u32>) -> bool {
        let close = |a: f64, b: f64| a == b || (a - b).abs() <= 2e-10 * a.abs().max(b.abs());
        let swapped = |code: u32| match code {
            param::LAT_1ST_PARALLEL => Some(param::LAT_2ND_PARALLEL),
            param::LAT_2ND_PARALLEL => Some(param::LAT_1ST_PARALLEL),
            _ => None,
        };
        // An azimuth or skew angle 360° from another is the same, as in
        // PROJ (ESRI writes −36.87° where EPSG has 323.13°).
        let same_turn = |code: u32, a: f64, b: f64| {
            matches!(code, 8813 | 8814) && {
                let (a, b) = (
                    (a.to_degrees() + 360.0) % 360.0,
                    (b.to_degrees() + 360.0) % 360.0,
                );
                (a - b).abs() <= 1e-10 * a.abs()
            }
        };
        self.values.iter().all(|&(code, value)| {
            match other.values.iter().find(|(c, _)| *c == code) {
                Some(&(_, v)) if close(value, v) || same_turn(code, value, v) => true,
                Some(_) => {
                    method == Some(9802)
                        && swapped(code).is_some_and(|twin| {
                            other.get(twin).is_ok_and(|v| close(value, v))
                                && self
                                    .get(twin)
                                    .is_ok_and(|mine| other.get(code).is_ok_and(|v| close(mine, v)))
                        })
                }
                None => value == if is_scale(code) { 1.0 } else { 0.0 },
            }
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

/// Scale factors, whose neutral value is 1.
fn is_scale(code: u32) -> bool {
    matches!(code, 8805 | 8815 | 8819 | 1038)
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
    pub const LAT_TOPOCENTRIC_ORIGIN: u32 = 8834;
    pub const LON_TOPOCENTRIC_ORIGIN: u32 = 8835;
    pub const VIEWPOINT_HEIGHT: u32 = 8840;
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
    /// Something the method can't be built with here, such as a PROJ key
    /// that changes more than the projection.
    Unsupported(String),
}

/// Parameters a method fixes, added where the definition leaves them out.
pub(crate) fn complete(code: Option<u32>, params: &mut Params) {
    krovak::complete(code, params);
}

/// The projection for an EPSG method code, or this crate's code of one only
/// PROJ defines, or by name a "PROJ" method.
pub(crate) fn build(
    code: Option<u32>,
    name: &str,
    params: &Params,
    ellipsoid: &Ellipsoid,
) -> Result<Arc<dyn Projection>, BuildError> {
    let Some(code) = code else {
        // A projection PROJ has no CRS method for.
        return match name.strip_prefix("PROJ ") {
            Some(proj) => custom::build(proj, params, ellipsoid),
            None => Err(BuildError::UnknownMethod),
        };
    };
    if code >= proj_codes::FIRST_METHOD {
        return proj_method(code, params, ellipsoid);
    }
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
        9841 | 1026 => Arc::new(merc::Mercator::spherical(params, ellipsoid)?),
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
        9840 => Arc::new(ortho::LocalOrthographic::orthographic(
            params, ellipsoid, false,
        )?),
        1119 => Arc::new(eqdc::EquidistantConic::new(params, ellipsoid, true)?),
        9838 => {
            // On the sphere of the semi-major axis; the origin's height unused.
            let unit = perspective::NearSidedPerspective::new(
                params.get(param::LAT_TOPOCENTRIC_ORIGIN)?,
                params.get(param::VIEWPOINT_HEIGHT)?,
                ellipsoid.a,
            )?;
            Arc::new(sphere::Spherical::with(
                unit,
                params.get(param::LON_TOPOCENTRIC_ORIGIN)?,
                params,
                ellipsoid.a,
            )?)
        }
        _ => return Err(BuildError::UnknownMethod),
    })
}

/// The projection of a method only PROJ defines.
fn proj_method(
    code: u32,
    params: &Params,
    ellipsoid: &Ellipsoid,
) -> Result<Arc<dyn Projection>, BuildError> {
    use proj_codes as c;
    if code == c::SINUSOIDAL && !ellipsoid.is_sphere() {
        return Ok(Arc::new(world::EllipsoidalSinusoidal::new(
            params, ellipsoid,
        )?));
    }
    match code {
        c::EQUIDISTANT_CONIC => {
            return Ok(Arc::new(eqdc::EquidistantConic::new(
                params, ellipsoid, false,
            )?));
        }
        c::GNOMONIC => return Ok(Arc::new(perspective::Gnomonic::new(params, ellipsoid)?)),
        c::HOTINE_OBLIQUE_MERCATOR_TWO_POINT_NATURAL_ORIGIN => {
            return omerc::ObliqueMercator::two_points(params, ellipsoid);
        }
        c::STEREOGRAPHIC => {
            // PROJ's `stere` with its latitude of true scale at the pole: as
            // variant A, at any latitude of origin.
            return Ok(Arc::new(stere::Stereographic::variant_a(
                params, ellipsoid,
            )?));
        }
        c::GAUSS_SCHREIBER_TRANSVERSE_MERCATOR => {
            return Ok(Arc::new(gstmerc::GaussSchreiber::new(params, ellipsoid)?));
        }
        c::INTERNATIONAL_MAP_OF_THE_WORLD_POLYCONIC => {
            return Ok(Arc::new(imw_p::ImwPolyconic::new(params, ellipsoid)?));
        }
        c::TWO_POINT_EQUIDISTANT => {
            let (unit, lon0) = tpeqd::TwoPointEquidistant::new(params)?;
            return Ok(Arc::new(sphere::Spherical::with(
                unit,
                lon0,
                params,
                ellipsoid.a,
            )?));
        }
        c::GEOSTATIONARY_SATELLITE_SWEEP_X | c::GEOSTATIONARY_SATELLITE_SWEEP_Y => {
            let sweep_x = code == c::GEOSTATIONARY_SATELLITE_SWEEP_X;
            return Ok(Arc::new(sphere::Spherical::new(
                geos::Geostationary::new(params, ellipsoid, sweep_x)?,
                params,
                ellipsoid,
            )?));
        }
        c::QUADRILATERALIZED_SPHERICAL_CUBE => {
            let unit = qsc::Qsc::new(
                params.get(param::LAT_NATURAL_ORIGIN)?,
                params.get(param::LON_NATURAL_ORIGIN)?,
                ellipsoid,
            );
            return Ok(Arc::new(sphere::Spherical::new(unit, params, ellipsoid)?));
        }
        c::SPHERICAL_CROSS_TRACK_HEIGHT => return Ok(Arc::new(sch::Sch::new(params, ellipsoid)?)),
        c::PEIRCE_QUINCUNCIAL_SQUARE | c::PEIRCE_QUINCUNCIAL_DIAMOND => {
            let square = code == c::PEIRCE_QUINCUNCIAL_SQUARE;
            let shape = if square {
                adams::Shape::Square
            } else {
                adams::Shape::Diamond
            };
            return Ok(Arc::new(sphere::Spherical::new(
                adams::Adams::Peirce(shape),
                params,
                ellipsoid,
            )?));
        }
        c::ORTHOGRAPHIC_SPHERICAL => {
            return Ok(Arc::new(ortho::LocalOrthographic::orthographic(
                params, ellipsoid, true,
            )?));
        }
        c::GOODE_HOMOLOSINE => {
            return Ok(Arc::new(sphere::Spherical::new(
                goode::Goode::new(),
                params,
                ellipsoid,
            )?));
        }
        c::INTERRUPTED_GOODE_HOMOLOSINE | c::INTERRUPTED_GOODE_HOMOLOSINE_OCEAN => {
            let ocean = code == c::INTERRUPTED_GOODE_HOMOLOSINE_OCEAN;
            return Ok(Arc::new(sphere::Spherical::new(
                goode::Interrupted::new(ocean),
                params,
                ellipsoid,
            )?));
        }
        _ => {}
    }
    if let Some(w) = world::World::of(code, params)? {
        // Miller and Van der Grinten are on the authalic sphere (`R_A`).
        return Ok(
            if matches!(code, c::MILLER_CYLINDRICAL | c::VAN_DER_GRINTEN) {
                Arc::new(sphere::Spherical::authalic(w, params, ellipsoid)?)
            } else {
                Arc::new(sphere::Spherical::new(w, params, ellipsoid)?)
            },
        );
    }
    Err(BuildError::UnknownMethod)
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
