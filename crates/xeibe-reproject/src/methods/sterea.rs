//! Oblique Stereographic (EPSG:9809), through the conformal "Gauss sphere".
//!
//! Ported from PROJ 9.8.1 `src/projections/sterea.cpp` and `src/gauss.cpp`
//! (MIT, see PROJ-NOTICE.md).

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4};

use super::{BuildError, Params, Projection, adjlon, param};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

const MAX_ITER: usize = 20;
const DEL_TOL: f64 = 1e-14;

#[derive(Debug)]
pub(crate) struct ObliqueStereographic {
    lon0: f64,
    k0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
    // Gauss sphere.
    c: f64,
    k: f64,
    e: f64,
    ratexp: f64,
    phic0: f64,
    sinc0: f64,
    cosc0: f64,
    r2: f64,
}

fn srat(esinp: f64, ratexp: f64) -> f64 {
    ((1.0 - esinp) / (1.0 + esinp)).powf(ratexp)
}

impl ObliqueStereographic {
    pub(crate) fn new(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        let phi0 = params.get(param::LAT_NATURAL_ORIGIN)?;
        let lon0 = params.get(param::LON_NATURAL_ORIGIN)?;
        let k0 = params.get(param::SCALE_NATURAL_ORIGIN)?;
        let fe = params.get(param::FALSE_EASTING)?;
        let fn_ = params.get(param::FALSE_NORTHING)?;
        let invalid = || BuildError::Invalid("invalid Gauss sphere parameters".to_owned());

        // pj_gauss_ini
        let (e, es) = (ellipsoid.e, ellipsoid.es);
        let (sphi, cphi) = phi0.sin_cos();
        let cphi = cphi * cphi;
        let r = (1.0 - es).sqrt() / (1.0 - es * sphi * sphi);
        let c = (1.0 + es * cphi * cphi / (1.0 - es)).sqrt();
        if c == 0.0 {
            return Err(invalid());
        }
        let phic0 = (sphi / c).asin();
        let ratexp = 0.5 * c * e;
        let srat_val = srat(e * sphi, ratexp);
        if srat_val == 0.0 {
            return Err(invalid());
        }
        let k = if 0.5 * phi0 + FRAC_PI_4 < 1e-10 {
            1.0 / srat_val
        } else {
            (0.5 * phic0 + FRAC_PI_4).tan() / ((0.5 * phi0 + FRAC_PI_4).tan().powf(c) * srat_val)
        };
        let (sinc0, cosc0) = phic0.sin_cos();
        Ok(ObliqueStereographic {
            lon0,
            k0,
            fe,
            fn_,
            a: ellipsoid.a,
            c,
            k,
            e,
            ratexp,
            phic0,
            sinc0,
            cosc0,
            r2: 2.0 * r,
        })
    }
}

impl Projection for ObliqueStereographic {
    fn forward(&self, lon: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = adjlon(lon - self.lon0);
        // pj_gauss
        let gphi = 2.0
            * (self.k
                * (0.5 * phi + FRAC_PI_4).tan().powf(self.c)
                * srat(self.e * phi.sin(), self.ratexp))
            .atan()
            - FRAC_PI_2;
        let glam = self.c * lam;
        let (sinc, cosc) = gphi.sin_cos();
        let cosl = glam.cos();
        let denom = 1.0 + self.sinc0 * sinc + self.cosc0 * cosc * cosl;
        if denom == 0.0 {
            return Err(CoordinateError::OutsideDomain);
        }
        let k = self.k0 * self.r2 / denom;
        let x = k * cosc * glam.sin();
        let y = k * (self.cosc0 * sinc - self.sinc0 * cosc * cosl);
        Ok((self.a * x + self.fe, self.a * y + self.fn_))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let x = (x - self.fe) / self.a / self.k0;
        let y = (y - self.fn_) / self.a / self.k0;
        let rho = x.hypot(y);
        let (mut gphi, glam) = if rho != 0.0 {
            let c = 2.0 * rho.atan2(self.r2);
            let (sinc, cosc) = c.sin_cos();
            (
                (cosc * self.sinc0 + y * sinc * self.cosc0 / rho).asin(),
                (x * sinc).atan2(rho * self.cosc0 * cosc - y * self.sinc0 * sinc),
            )
        } else {
            (self.phic0, 0.0)
        };
        // pj_inv_gauss
        let lam = glam / self.c;
        let num = ((0.5 * gphi + FRAC_PI_4).tan() / self.k).powf(1.0 / self.c);
        let mut phi = 0.0;
        let mut converged = false;
        for _ in 0..MAX_ITER {
            phi = 2.0 * (num * srat(self.e * gphi.sin(), -0.5 * self.e)).atan() - FRAC_PI_2;
            if (phi - gphi).abs() < DEL_TOL {
                converged = true;
                break;
            }
            gphi = phi;
        }
        if !converged {
            return Err(CoordinateError::OutsideDomain);
        }
        Ok((lam + self.lon0, phi))
    }
}
