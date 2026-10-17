//! Equal Earth (EPSG:1078).
//!
//! Ported from PROJ 9.8.1 `src/projections/eqearth.cpp` (`eqearth`; MIT, see
//! PROJ-NOTICE.md). The polynomial is Šavrič, Patterson and Jenny's; the
//! ellipsoidal form goes through the authalic latitude.

use super::authalic_q::Authalic;
use super::{BuildError, Params, Projection, adjlon, param};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

const A1: f64 = 1.340264;
const A2: f64 = -0.081106;
const A3: f64 = 0.000893;
const A4: f64 = 0.003796;
/// √3 / 2.
const M: f64 = 0.866_025_403_784_438_6;
/// y at 90° latitude on a sphere with radius 1.
const MAX_Y: f64 = 1.3173627591574;
const EPS: f64 = 1e-11;
const MAX_ITER: usize = 12;

#[derive(Debug)]
pub(crate) struct EqualEarth {
    lon0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
    /// `None` for a sphere.
    authalic: Option<Authalic>,
    /// Authalic radius over a.
    rqda: f64,
}

impl EqualEarth {
    pub(crate) fn new(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        let (authalic, rqda) = if ellipsoid.is_sphere() {
            (None, 1.0)
        } else {
            let au = Authalic::new(ellipsoid);
            let rqda = (0.5 * au.qp).sqrt();
            (Some(au), rqda)
        };
        Ok(EqualEarth {
            lon0: params.get(param::LON_NATURAL_ORIGIN)?,
            fe: params.get(param::FALSE_EASTING)?,
            fn_: params.get(param::FALSE_NORTHING)?,
            a: ellipsoid.a,
            authalic,
            rqda,
        })
    }
}

impl Projection for EqualEarth {
    fn forward(&self, lon: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = adjlon(lon - self.lon0);
        let mut sbeta = phi.sin();
        if let Some(au) = &self.authalic {
            sbeta = au.q(sbeta) / au.qp;
            // Rounding error.
            if sbeta.abs() > 1.0 {
                sbeta = sbeta.signum();
            }
        }
        let psi = (M * sbeta).asin();
        let psi2 = psi * psi;
        let psi6 = psi2 * psi2 * psi2;
        let x =
            lam * psi.cos() / (M * (A1 + 3.0 * A2 * psi2 + psi6 * (7.0 * A3 + 9.0 * A4 * psi2)));
        let y = psi * (A1 + A2 * psi2 + psi6 * (A3 + A4 * psi2));
        Ok((
            self.a * (x * self.rqda) + self.fe,
            self.a * (y * self.rqda) + self.fn_,
        ))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let x = (x - self.fe) / self.a / self.rqda;
        let y = ((y - self.fn_) / self.a / self.rqda).clamp(-MAX_Y, MAX_Y);
        let mut yc = y;
        let mut converged = false;
        for _ in 0..MAX_ITER {
            let y2 = yc * yc;
            let y6 = y2 * y2 * y2;
            let f = yc * (A1 + A2 * y2 + y6 * (A3 + A4 * y2)) - y;
            let fder = A1 + 3.0 * A2 * y2 + y6 * (7.0 * A3 + 9.0 * A4 * y2);
            let tol = f / fder;
            yc -= tol;
            if tol.abs() < EPS {
                converged = true;
                break;
            }
        }
        if !converged {
            return Err(CoordinateError::OutsideDomain);
        }
        let y2 = yc * yc;
        let y6 = y2 * y2 * y2;
        let lam = M * x * (A1 + 3.0 * A2 * y2 + y6 * (7.0 * A3 + 9.0 * A4 * y2)) / yc.cos();
        let mut phi = (yc.sin() / M).asin();
        if let Some(au) = &self.authalic {
            phi = au.inverse(phi);
        }
        Ok((lam + self.lon0, phi))
    }
}
