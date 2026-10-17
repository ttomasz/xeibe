//! Colombia Urban (EPSG:1052).
//!
//! Ported from PROJ 9.8.1 `src/projections/col_urban.cpp` (`col_urban`; MIT,
//! see PROJ-NOTICE.md). EPSG's "projection plane origin height" is PROJ's
//! `h_0`.

use super::{BuildError, Params, Projection, adjlon, param};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

#[derive(Debug)]
pub(crate) struct ColombiaUrban {
    lon0: f64,
    phi0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
    es: f64,
    /// Height of the projection plane origin over a.
    h0: f64,
    rho0: f64,
    ca: f64,
    b: f64,
    c: f64,
    d: f64,
}

impl ColombiaUrban {
    pub(crate) fn new(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        let phi0 = params.get(param::LAT_NATURAL_ORIGIN)?;
        let es = ellipsoid.es;
        let h0 = params.get(param::PROJECTION_PLANE_ORIGIN_HEIGHT)? / ellipsoid.a;
        let sinphi0 = phi0.sin();
        let nu0 = 1.0 / (1.0 - es * sinphi0 * sinphi0).sqrt();
        let rho0 = (1.0 - es) / (1.0 - es * sinphi0 * sinphi0).powf(1.5);
        Ok(ColombiaUrban {
            lon0: params.get(param::LON_NATURAL_ORIGIN)?,
            phi0,
            fe: params.get(param::FALSE_EASTING)?,
            fn_: params.get(param::FALSE_NORTHING)?,
            a: ellipsoid.a,
            es,
            h0,
            rho0,
            ca: 1.0 + h0 / nu0,
            b: phi0.tan() / (2.0 * rho0 * nu0),
            c: 1.0 + h0,
            d: rho0 * (1.0 + h0 / (1.0 - es)),
        })
    }
}

impl ColombiaUrban {
    /// The forward formulas in units of a, `lam` counted from the origin.
    fn fwd(&self, lam: f64, phi: f64) -> (f64, f64) {
        let (sinphi, cosphi) = phi.sin_cos();
        let nu = 1.0 / (1.0 - self.es * sinphi * sinphi).sqrt();
        let lam_nu_cosphi = lam * nu * cosphi;
        let x = self.ca * lam_nu_cosphi;
        let sinphi_m = (0.5 * (phi + self.phi0)).sin();
        let rho_m = (1.0 - self.es) / (1.0 - self.es * sinphi_m * sinphi_m).powf(1.5);
        let g = 1.0 + self.h0 / rho_m;
        let y = g * self.rho0 * ((phi - self.phi0) + self.b * lam_nu_cosphi * lam_nu_cosphi);
        (x, y)
    }

    /// EPSG's closed-form inverse, which is only approximate (PROJ's `inv`).
    fn inv_approx(&self, x: f64, y: f64) -> (f64, f64) {
        let phi = self.phi0 + y / self.d - self.b * (x / self.c) * (x / self.c);
        let sinphi = phi.sin();
        let nu = 1.0 / (1.0 - self.es * sinphi * sinphi).sqrt();
        (x / (self.c * nu * phi.cos()), phi)
    }
}

impl Projection for ColombiaUrban {
    fn forward(&self, lon: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = adjlon(lon - self.lon0);
        let (x, y) = self.fwd(lam, phi);
        Ok((self.a * x + self.fe, self.a * y + self.fn_))
    }

    /// EPSG's inverse formulas are an approximation: a point converted
    /// forward and back lands up to 0.5 mm from where it started (PROJ's
    /// `inv` has the same error). The result is refined here so that it is
    /// the inverse of `forward`, by iterating the approximate inverse on the
    /// residual.
    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let x = (x - self.fe) / self.a;
        let y = (y - self.fn_) / self.a;
        let (lam0, phi0) = self.inv_approx(x, y);
        let (mut lam, mut phi) = (lam0, phi0);
        for _ in 0..4 {
            let (fx, fy) = self.fwd(lam, phi);
            let (gl, gp) = self.inv_approx(fx, fy);
            lam += lam0 - gl;
            phi += phi0 - gp;
        }
        Ok((lam + self.lon0, phi))
    }
}
