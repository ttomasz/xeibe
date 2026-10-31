//! International Map of the World Polyconic, on an ellipsoid.
//!
//! Ported from PROJ 9.8.1 `src/projections/imw_p.cpp` (MIT, see
//! PROJ-NOTICE.md). Its sheets' meridians at ±`lon_1` are true to scale;
//! PROJ's CRS method has no `lon_1`, so it is PROJ's default by latitude.

use super::mlfn::Mlfn;
use super::{BuildError, Params, Projection, adjlon, param, proj_codes};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

const TOL: f64 = 1e-10;
const EPS: f64 = 1e-10;
const DEG_TO_RAD: f64 = 0.017_453_292_519_943_295;

/// Which of the two latitudes, if either, is the equator.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Zero {
    Neither,
    Phi1,
    Phi2,
}

#[derive(Debug)]
pub(crate) struct ImwPolyconic {
    lon0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
    es: f64,
    en: Mlfn,
    p: f64,
    pp: f64,
    q: f64,
    qp: f64,
    r_1: f64,
    r_2: f64,
    sphi_1: f64,
    sphi_2: f64,
    c2: f64,
    phi_1: f64,
    phi_2: f64,
    zero: Zero,
}

impl ImwPolyconic {
    pub(crate) fn new(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        let mut phi_1 = params.get(proj_codes::LATITUDE_OF_1ST_POINT)?;
        let mut phi_2 = params.get(proj_codes::LATITUDE_OF_2ND_POINT)?;
        let del = 0.5 * (phi_2 - phi_1);
        let sig = 0.5 * (phi_2 + phi_1);
        if del.abs() < EPS || sig.abs() < EPS {
            return Err(BuildError::Invalid(
                "Illegal value for lat_1 and lat_2: |lat_1 - lat_2| and |lat_1 + lat_2| should be > 0"
                    .into(),
            ));
        }
        // phi_1 the more southerly.
        if phi_2 < phi_1 {
            std::mem::swap(&mut phi_1, &mut phi_2);
        }
        let sig = sig.abs().to_degrees();
        let lam_1 = DEG_TO_RAD
            * if sig <= 60.0 {
                2.0
            } else if sig <= 76.0 {
                4.0
            } else {
                8.0
            };
        let es = ellipsoid.es;
        let xy = |phi: f64| {
            let sp = phi.sin();
            let r = 1.0 / (phi.tan() * (1.0 - es * sp * sp).sqrt());
            let f = lam_1 * sp;
            (r * f.sin(), r * (1.0 - f.cos()), sp, r)
        };
        let mut zero = Zero::Neither;
        let (x1, y1, sphi_1, r_1) = if phi_1 != 0.0 {
            xy(phi_1)
        } else {
            zero = Zero::Phi1;
            (lam_1, 0.0, 0.0, 0.0)
        };
        let (x2, t2, sphi_2, r_2) = if phi_2 != 0.0 {
            xy(phi_2)
        } else {
            zero = Zero::Phi2;
            (lam_1, 0.0, 0.0, 0.0)
        };
        let en = Mlfn::new(ellipsoid.n);
        let m1 = en.mlfn(phi_1, sphi_1, phi_1.cos());
        let m2 = en.mlfn(phi_2, sphi_2, phi_2.cos());
        let t = m2 - m1;
        let s = x2 - x1;
        let y2 = (t * t - s * s).sqrt() + y1;
        let t = 1.0 / t;
        Ok(ImwPolyconic {
            lon0: params.get(param::LON_NATURAL_ORIGIN)?,
            fe: params.get(param::FALSE_EASTING)?,
            fn_: params.get(param::FALSE_NORTHING)?,
            a: ellipsoid.a,
            es,
            en,
            p: (m2 * y1 - m1 * y2) * t,
            q: (y2 - y1) * t,
            pp: (m2 * x1 - m1 * x2) * t,
            qp: (x2 - x1) * t,
            r_1,
            r_2,
            sphi_1,
            sphi_2,
            c2: y2 - t2,
            phi_1,
            phi_2,
            zero,
        })
    }

    /// x and y, and the y of the first parallel at that longitude
    /// (`loc_for`).
    fn loc_for(&self, lam: f64, phi: f64) -> (f64, f64, f64) {
        if phi == 0.0 {
            return (lam, 0.0, 0.0);
        }
        let sp = phi.sin();
        let m = self.en.mlfn(phi, sp, phi.cos());
        let xa = self.pp + self.qp * m;
        let ya = self.p + self.q * m;
        let r = 1.0 / (phi.tan() * (1.0 - self.es * sp * sp).sqrt());
        let mut c = (r * r - xa * xa).sqrt();
        if phi < 0.0 {
            c = -c;
        }
        c += ya - r;
        let (xb, yb) = if self.zero == Zero::Phi2 {
            (lam, self.c2)
        } else {
            let t = lam * self.sphi_2;
            (self.r_2 * t.sin(), self.c2 + self.r_2 * (1.0 - t.cos()))
        };
        let (xc, yc) = if self.zero == Zero::Phi1 {
            (lam, 0.0)
        } else {
            let t = lam * self.sphi_1;
            (self.r_1 * t.sin(), self.r_1 * (1.0 - t.cos()))
        };
        let d = (xb - xc) / (yb - yc);
        let b = xc + d * (c + r - yc);
        let mut x = d * (r * r * (1.0 + d * d) - b * b).sqrt();
        if phi > 0.0 {
            x = -x;
        }
        x = (b + x) / (1.0 + d * d);
        let mut y = (r * r - x * x).sqrt();
        if phi > 0.0 {
            y = -y;
        }
        (x, y + c + r, yc)
    }
}

impl Projection for ImwPolyconic {
    fn forward(&self, lon: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let (x, y, _) = self.loc_for(adjlon(lon - self.lon0), phi);
        Ok((self.a * x + self.fe, self.a * y + self.fn_))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let (x, y) = ((x - self.fe) / self.a, (y - self.fn_) / self.a);
        let mut phi = self.phi_2;
        let mut lam = x / phi.cos();
        for _ in 0..1000 {
            let (tx, ty, yc) = self.loc_for(lam, phi);
            let denom = ty - yc;
            if denom != 0.0 || (ty - y).abs() > TOL {
                if denom == 0.0 {
                    return Err(CoordinateError::OutsideDomain);
                }
                phi = (phi - self.phi_1) * (y - yc) / denom + self.phi_1;
            }
            if tx != 0.0 && (tx - x).abs() > TOL {
                lam = lam * x / tx;
            }
            if (tx - x).abs() <= TOL && (ty - y).abs() <= TOL {
                return Ok((lam + self.lon0, phi));
            }
        }
        Err(CoordinateError::OutsideDomain)
    }
}
