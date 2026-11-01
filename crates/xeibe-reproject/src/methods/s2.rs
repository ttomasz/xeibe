//! S2: Google's S2 geometry cube face, as s and t from 0 to 1, of the
//! face that holds the centre (`lat_0`, `lon_0`).
//!
//! Ported from PROJ 9.8.1 `src/projections/s2.cpp` (MIT, see
//! PROJ-NOTICE.md). The output is neither scaled by the radius nor turned
//! to the central meridian, which only picks the face; the false origin
//! still applies. On an ellipsoid the latitude goes to the geocentric one.
//! A point on the far side of the face's plane, which PROJ projects
//! through the centre onto the face as if it were its antipode, is refused
//! (divergence in `docs/pyproj-compat.md`).

use std::f64::consts::{FRAC_1_PI, FRAC_PI_2, FRAC_PI_4};

use super::{BuildError, Params, Projection, adjlon};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

#[derive(Debug, Clone, Copy, PartialEq)]
enum UvToSt {
    Linear,
    Quadratic,
    Tangent,
    None,
}

#[derive(Debug)]
pub(crate) struct S2 {
    face: u8,
    uv_to_st: UvToSt,
    fe: f64,
    fn_: f64,
    es: f64,
    b: f64,
    a_squared: f64,
    one_minus_f: f64,
    one_minus_f_squared: f64,
}

impl S2 {
    pub(crate) fn new(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        let uv_to_st = match params.flag("UVtoST") {
            None | Some("quadratic") => UvToSt::Quadratic,
            Some("linear") => UvToSt::Linear,
            Some("tangent") => UvToSt::Tangent,
            Some("none") => UvToSt::None,
            Some(_) => {
                return Err(BuildError::Invalid(
                    "Invalid value for s2 parameter: should be linear, quadratic, tangent, or none."
                        .into(),
                ));
            }
        };
        let key = |k: &str| params.key(k).unwrap_or(0.0);
        let (phi0, lam0) = (key("lat_0"), key("lon_0"));
        let face = if phi0 >= FRAC_PI_2 - FRAC_PI_4 / 2.0 {
            2
        } else if phi0 <= -(FRAC_PI_2 - FRAC_PI_4 / 2.0) {
            5
        } else if lam0.abs() <= FRAC_PI_4 {
            0
        } else if lam0.abs() <= FRAC_PI_2 + FRAC_PI_4 {
            if lam0 > 0.0 { 1 } else { 4 }
        } else {
            3
        };
        let (a, b) = (ellipsoid.a, ellipsoid.b);
        let one_minus_f = 1.0 - (a - b) / a;
        Ok(S2 {
            face,
            uv_to_st,
            fe: key("x_0"),
            fn_: key("y_0"),
            es: ellipsoid.es,
            b,
            a_squared: a * a,
            one_minus_f,
            one_minus_f_squared: one_minus_f * one_minus_f,
        })
    }

    fn st_to_uv(&self, s: f64) -> f64 {
        match self.uv_to_st {
            UvToSt::Linear => 2.0 * s - 1.0,
            UvToSt::Quadratic => {
                if s >= 0.5 {
                    (1.0 / 3.0) * (4.0 * s * s - 1.0)
                } else {
                    (1.0 / 3.0) * (1.0 - 4.0 * (1.0 - s) * (1.0 - s))
                }
            }
            UvToSt::Tangent => {
                let s = (FRAC_PI_2 * s - FRAC_PI_4).tan();
                s + (1.0 / (1u64 << 53) as f64) * s
            }
            UvToSt::None => s,
        }
    }

    fn uv_to_st(&self, u: f64) -> f64 {
        match self.uv_to_st {
            UvToSt::Linear => 0.5 * (u + 1.0),
            UvToSt::Quadratic => {
                if u >= 0.0 {
                    0.5 * (1.0 + 3.0 * u).sqrt()
                } else {
                    1.0 - 0.5 * (1.0 - 3.0 * u).sqrt()
                }
            }
            UvToSt::Tangent => (2.0 * FRAC_1_PI) * (u.atan() + FRAC_PI_4),
            UvToSt::None => u,
        }
    }
}

impl Projection for S2 {
    fn forward(&self, lon: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = adjlon(lon);
        let lat = if self.es != 0.0 {
            (self.one_minus_f_squared * phi.tan()).atan()
        } else {
            phi
        };
        let (sinlat, coslat) = lat.sin_cos();
        let (sinlon, coslon) = lam.sin_cos();
        let (x, y, z) = (coslat * coslon, coslat * sinlon, sinlat);
        // The face's own coordinate must point towards it (`FaceXYZtoUV`).
        let towards = match self.face {
            0 => x > 0.0,
            1 => y > 0.0,
            2 => z > 0.0,
            3 => x < 0.0,
            4 => y < 0.0,
            _ => z < 0.0,
        };
        if !towards {
            return Err(CoordinateError::OutsideDomain);
        }
        let (u, v) = match self.face {
            0 => (y / x, z / x),
            1 => (-x / y, z / y),
            2 => (-x / z, -y / z),
            3 => (z / x, y / x),
            4 => (z / y, -x / y),
            _ => (-y / z, -x / z),
        };
        Ok((self.uv_to_st(u) + self.fe, self.uv_to_st(v) + self.fn_))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let u = self.st_to_uv(x - self.fe);
        let v = self.st_to_uv(y - self.fn_);
        let major = 1.0 / (1.0 + u * u + v * v).sqrt();
        let (m1, m2) = (u * major, v * major);
        let (q, r, s) = match self.face {
            0 => (major, m1, m2),
            1 => (-m1, major, m2),
            2 => (-m1, -m2, major),
            3 => (-major, -m2, -m1),
            4 => (m2, -major, -m1),
            _ => (m2, m1, -major),
        };
        let mut phi = (-s).acos() - FRAC_PI_2;
        let lam = r.atan2(q);
        if self.es != 0.0 {
            // From the geocentric latitude back.
            let tanphi = phi.tan();
            let xa = self.b / (tanphi * tanphi + self.one_minus_f_squared).sqrt();
            let p = ((self.a_squared - xa * xa).sqrt() / (self.one_minus_f * xa)).atan();
            phi = if phi < 0.0 { -p } else { p };
        }
        Ok((lam, phi))
    }
}
