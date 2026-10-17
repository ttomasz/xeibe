//! American Polyconic (EPSG:9818).
//!
//! Ported from PROJ 9.8.1 `src/projections/poly.cpp` (`poly`; MIT, see
//! PROJ-NOTICE.md).

use super::common::msfn;
use super::mlfn::Mlfn;
use super::{BuildError, Params, Projection, adjlon, param};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

const TOL: f64 = 1e-10;
const CONV: f64 = 1e-10;
const N_ITER: usize = 10;
const I_ITER: usize = 20;
const ITOL: f64 = 1e-12;

#[derive(Debug)]
pub(crate) struct Polyconic {
    lon0: f64,
    phi0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
    es: f64,
    one_es: f64,
    ml0: f64,
    /// `None` for a sphere.
    en: Option<Mlfn>,
}

impl Polyconic {
    pub(crate) fn new(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        let phi0 = params.get(param::LAT_NATURAL_ORIGIN)?;
        let (en, ml0) = if ellipsoid.is_sphere() {
            (None, -phi0)
        } else {
            let en = Mlfn::new(ellipsoid.n);
            let ml0 = en.mlfn(phi0, phi0.sin(), phi0.cos());
            (Some(en), ml0)
        };
        Ok(Polyconic {
            lon0: params.get(param::LON_NATURAL_ORIGIN)?,
            phi0,
            fe: params.get(param::FALSE_EASTING)?,
            fn_: params.get(param::FALSE_NORTHING)?,
            a: ellipsoid.a,
            es: ellipsoid.es,
            one_es: 1.0 - ellipsoid.es,
            ml0,
            en,
        })
    }

    fn inverse_ellipsoidal(
        &self,
        en: &Mlfn,
        x: f64,
        y: f64,
    ) -> Result<(f64, f64), CoordinateError> {
        let y = y + self.ml0;
        if y.abs() <= TOL {
            return Ok((x, 0.0));
        }
        let r = y * y + x * x;
        let mut phi = y;
        let mut converged = false;
        for _ in 0..I_ITER {
            let (sp, cp) = phi.sin_cos();
            let s2ph = sp * cp;
            if cp.abs() < ITOL {
                return Err(CoordinateError::OutsideDomain);
            }
            let mut mlp = (1.0 - self.es * sp * sp).sqrt();
            let c = sp * mlp / cp;
            let ml = en.mlfn(phi, sp, cp);
            let mlb = ml * ml + r;
            mlp = self.one_es / (mlp * mlp * mlp);
            let dphi = (ml + ml + c * mlb - 2.0 * y * (c * ml + 1.0))
                / (self.es * s2ph * (mlb - 2.0 * y * ml) / c
                    + 2.0 * (y - ml) * (c * mlp - 1.0 / s2ph)
                    - mlp
                    - mlp);
            phi += dphi;
            if dphi.abs() <= ITOL {
                converged = true;
                break;
            }
        }
        if !converged {
            return Err(CoordinateError::OutsideDomain);
        }
        let c = phi.sin();
        let lam = (x * phi.tan() * (1.0 - self.es * c * c).sqrt()).asin() / phi.sin();
        Ok((lam, phi))
    }

    // The negated comparison is how NaN ends the iteration, as in PROJ.
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    fn inverse_spherical(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let y = self.phi0 + y;
        if y.abs() <= TOL {
            return Ok((x, 0.0));
        }
        let mut phi = y;
        let b = x * x + y * y;
        let mut i = N_ITER;
        loop {
            let tp = phi.tan();
            let dphi =
                (y * (phi * tp + 1.0) - phi - 0.5 * (phi * phi + b) * tp) / ((phi - y) / tp - 1.0);
            phi -= dphi;
            // Written backwards so that NaN ends the loop.
            if !(dphi.abs() > CONV) {
                break;
            }
            i -= 1;
            if i == 0 {
                return Err(CoordinateError::OutsideDomain);
            }
        }
        let lam = (x * phi.tan()).asin() / phi.sin();
        Ok((lam, phi))
    }
}

impl Projection for Polyconic {
    fn forward(&self, lon: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = adjlon(lon - self.lon0);
        let (x, y) = match &self.en {
            Some(en) => {
                if phi.abs() <= TOL {
                    (lam, -self.ml0)
                } else {
                    let (sp, cp) = phi.sin_cos();
                    let ms = if cp.abs() > TOL {
                        msfn(sp, cp, self.es) / sp
                    } else {
                        0.0
                    };
                    let lam = lam * sp;
                    (
                        ms * lam.sin(),
                        (en.mlfn(phi, sp, cp) - self.ml0) + ms * (1.0 - lam.cos()),
                    )
                }
            }
            None => {
                if phi.abs() <= TOL {
                    (lam, self.ml0)
                } else {
                    let cot = 1.0 / phi.tan();
                    let e = lam * phi.sin();
                    (e.sin() * cot, phi - self.phi0 + cot * (1.0 - e.cos()))
                }
            }
        };
        Ok((self.a * x + self.fe, self.a * y + self.fn_))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let x = (x - self.fe) / self.a;
        let y = (y - self.fn_) / self.a;
        let (lam, phi) = match &self.en {
            Some(en) => self.inverse_ellipsoidal(en, x, y)?,
            None => self.inverse_spherical(x, y)?,
        };
        Ok((lam + self.lon0, phi))
    }
}
