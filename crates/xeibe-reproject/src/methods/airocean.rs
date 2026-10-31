//! Airocean, Buckminster Fuller's Dymaxion map: the sphere onto an
//! icosahedron, by gnomonic projection from the centre, and the faces laid
//! out as a net, vertical or horizontal.
//!
//! Ported from PROJ 9.8.1 `src/projections/airocean.cpp` (MIT, see
//! PROJ-NOTICE.md). On an ellipsoid the latitude goes to the geocentric
//! one first, as in S2.

// PROJ's tables, digit for digit.
#![allow(clippy::excessive_precision)]

mod tables;

use super::{BuildError, Params, Projection, adjlon};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

type Xyz = [f64; 3];
type Face = [Xyz; 3];
type Matrix = [[f64; 4]; 4];

#[derive(Debug)]
pub(crate) struct Airocean {
    lon0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
    b: f64,
    es: f64,
    net_faces: [Face; 23],
    ico_air: [Matrix; 23],
    air_ico: [Matrix; 23],
}

fn mat_mult(m1: &Matrix, m2: &Matrix) -> Matrix {
    let mut res = [[0.0; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            res[i][j] = (m1[i][0] * m2[0][j])
                + (m1[i][1] * m2[1][j])
                + (m1[i][2] * m2[2][j])
                + (m1[i][3] * m2[3][j]);
        }
    }
    res
}

/// `m` applied to the point (x, y, z, 1).
fn vec_mult(m: &Matrix, v: &Xyz) -> Xyz {
    [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2] + m[i][3])
}

fn det(u: &Xyz, v: &Xyz, w: &Xyz) -> f64 {
    u[0] * (v[1] * w[2] - v[2] * w[1]) - v[0] * (u[1] * w[2] - u[2] * w[1])
        + w[0] * (u[1] * v[2] - u[2] * v[1])
}

fn in_face(p: &Xyz, f: &Face) -> bool {
    det(p, &f[1], &f[2]) <= 0.0 && det(&f[0], p, &f[2]) <= 0.0 && det(&f[0], &f[1], p) <= 0.0
}

impl Airocean {
    pub(crate) fn new(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        let (mut net_faces, mut ico_air, mut air_ico) = (
            tables::AIROCEAN_FACES,
            tables::ICO_AIR_TRANS,
            tables::AIR_ICO_TRANS,
        );
        match params.flag("orient") {
            None | Some("vertical") => {}
            Some("horizontal") => {
                let (m, inv) = (
                    &tables::ORIENT_HORIZONTAL_TRANS,
                    &tables::ORIENT_HORIZONTAL_INV_TRANS,
                );
                for i in 0..23 {
                    ico_air[i] = mat_mult(m, &tables::ICO_AIR_TRANS[i]);
                    air_ico[i] = mat_mult(&tables::AIR_ICO_TRANS[i], inv);
                    net_faces[i] = tables::AIROCEAN_FACES[i].map(|p| vec_mult(m, &p));
                }
            }
            Some(_) => {
                return Err(BuildError::Invalid(
                    "Invalid value for orient: only vertical or horizontal are supported".into(),
                ));
            }
        }
        let key = |k: &str| params.key(k).unwrap_or(0.0);
        Ok(Airocean {
            lon0: key("lon_0"),
            fe: key("x_0"),
            fn_: key("y_0"),
            a: ellipsoid.a,
            b: ellipsoid.b,
            es: ellipsoid.es,
            net_faces,
            ico_air,
            air_ico,
        })
    }

    fn one_minus_f(&self) -> f64 {
        1.0 - (self.a - self.b) / self.a
    }
}

impl Projection for Airocean {
    fn forward(&self, lon: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = adjlon(lon - self.lon0);
        let lat = if self.es != 0.0 {
            let f1 = self.one_minus_f();
            (f1 * f1 * phi.tan()).atan()
        } else {
            phi
        };
        let (sinlat, coslat) = lat.sin_cos();
        let (sinlon, coslon) = lam.sin_cos();
        let p = [coslat * coslon, coslat * sinlon, sinlat];
        let Some(face) = tables::ICO_FACES.iter().position(|f| in_face(&p, f)) else {
            return Err(CoordinateError::OutsideDomain);
        };
        // Onto the face's plane, from the centre.
        let (c, n) = (&tables::ICO_CENTERS[face], &tables::ICO_NORMALS[face]);
        let k = 1.0
            - (c[0] * n[0] + c[1] * n[1] + c[2] * n[2]) / (p[0] * n[0] + p[1] * n[1] + p[2] * n[2]);
        let q = p.map(|v| v - k * v);
        let m = &self.ico_air[face];
        let x = m[0][0] * q[0] + m[0][1] * q[1] + m[0][2] * q[2] + m[0][3];
        let y = m[1][0] * q[0] + m[1][1] * q[1] + m[1][2] * q[2] + m[1][3];
        Ok((self.a * x + self.fe, self.a * y + self.fn_))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let ra = 1.0 / self.a;
        let (x, y) = ((x - self.fe) * ra, (y - self.fn_) * ra);
        let pp = [x, y, 1.0];
        let Some(face) = self.net_faces.iter().position(|f| in_face(&pp, f)) else {
            return Err(CoordinateError::OutsideDomain);
        };
        let m = &self.air_ico[face];
        let p = [0, 1, 2].map(|i| m[i][0] * x + m[i][1] * y + m[i][3]);
        let norm = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
        let (q, r, s) = (p[0] / norm, p[1] / norm, p[2] / norm);
        let mut phi = (-s).acos() - std::f64::consts::FRAC_PI_2;
        let lam = r.atan2(q);
        if self.es != 0.0 {
            // From the geocentric latitude back.
            let f1 = self.one_minus_f();
            let tanphi = phi.tan();
            let xa = self.b / (tanphi * tanphi + f1 * f1).sqrt();
            let p = ((self.a * self.a - xa * xa).sqrt() / (f1 * xa)).atan();
            phi = if phi < 0.0 { -p } else { p };
        }
        Ok((lam + self.lon0, phi))
    }
}
