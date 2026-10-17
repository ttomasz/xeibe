//! Cassini-Soldner (EPSG:9806) and Hyperbolic Cassini-Soldner (EPSG:9833).
//!
//! Ported from PROJ 9.8.1 `src/projections/cass.cpp` (`cass`, with
//! `+hyperbolic` for the second; MIT, see PROJ-NOTICE.md) and
//! `src/generic_inverse.cpp`, which PROJ's ellipsoidal inverse ends with.

use std::f64::consts::{FRAC_PI_2, PI};

use super::mlfn::Mlfn;
use super::{BuildError, Params, Projection, adjlon, param};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

const C1: f64 = 1.0 / 6.0;
const C2: f64 = 1.0 / 120.0;
const C3: f64 = 1.0 / 24.0;
const C4: f64 = 1.0 / 3.0;
const C5: f64 = 1.0 / 15.0;

#[derive(Debug)]
pub(crate) struct Cassini {
    lon0: f64,
    phi0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
    es: f64,
    /// Meridian arc and its value at the origin, ellipsoidal case only.
    arc: Option<(Mlfn, f64)>,
    hyperbolic: bool,
}

impl Cassini {
    pub(crate) fn new(
        params: &Params,
        ellipsoid: &Ellipsoid,
        hyperbolic: bool,
    ) -> Result<Self, BuildError> {
        let phi0 = params.get(param::LAT_NATURAL_ORIGIN)?;
        let arc = if ellipsoid.is_sphere() {
            None
        } else {
            let en = Mlfn::new(ellipsoid.n);
            let m0 = en.mlfn(phi0, phi0.sin(), phi0.cos());
            Some((en, m0))
        };
        Ok(Cassini {
            lon0: params.get(param::LON_NATURAL_ORIGIN)?,
            phi0,
            fe: params.get(param::FALSE_EASTING)?,
            fn_: params.get(param::FALSE_NORTHING)?,
            a: ellipsoid.a,
            es: ellipsoid.es,
            arc,
            hyperbolic,
        })
    }

    /// The projection in units of a, `lam` counted from the origin.
    fn fwd(&self, lam: f64, phi: f64) -> (f64, f64) {
        let Some((en, m0)) = &self.arc else {
            return (
                (phi.cos() * lam.sin()).asin(),
                phi.tan().atan2(lam.cos()) - self.phi0,
            );
        };
        let es = self.es;
        let (sinphi, cosphi) = phi.sin_cos();
        let m = en.mlfn(phi, sinphi, cosphi);
        let nu_square = 1.0 / (1.0 - es * sinphi * sinphi);
        let nu = nu_square.sqrt();
        let tanphi = phi.tan();
        let t = tanphi * tanphi;
        let a = lam * cosphi;
        let c = es * (cosphi * cosphi) / (1.0 - es);
        let a2 = a * a;
        let x = nu * a * (1.0 - a2 * t * (C1 + (8.0 - t + 8.0 * c) * a2 * C2));
        let mut y = m - m0 + nu * tanphi * a2 * (0.5 + (5.0 - t + 6.0 * c) * a2 * C3);
        if self.hyperbolic {
            let rho = nu_square * (1.0 - es) * nu;
            y -= y * y * y / (6.0 * rho * nu);
        }
        (x, y)
    }
}

impl Projection for Cassini {
    fn forward(&self, lon: f64, lat: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = adjlon(lon - self.lon0);
        let (x, y) = self.fwd(lam, lat);
        Ok((self.a * x + self.fe, self.a * y + self.fn_))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let x = (x - self.fe) / self.a;
        let y = (y - self.fn_) / self.a;
        let Some((en, m0)) = &self.arc else {
            let dd = y + self.phi0;
            let phi = (dd.sin() * x.cos()).asin();
            let lam = x.tan().atan2(dd.cos());
            return Ok((lam + self.lon0, phi));
        };
        let es = self.es;
        let phi1 = en.inv_mlfn(m0 + y);
        let tanphi1 = phi1.tan();
        let t1 = tanphi1 * tanphi1;
        let sinphi1 = phi1.sin();
        let nu1_square = 1.0 / (1.0 - es * sinphi1 * sinphi1);
        let nu1 = nu1_square.sqrt();
        let rho1 = nu1_square * (1.0 - es) * nu1;
        let d = x / nu1;
        let d2 = d * d;
        let phi = phi1 - (nu1 * tanphi1 / rho1) * d2 * (0.5 - (1.0 + 3.0 * t1) * d2 * C3);
        let lam = d * (1.0 + t1 * d2 * (-C4 + (1.0 + 3.0 * t1) * d2 * C5)) / phi1.cos();
        // The series above only start the generic inversion, as in PROJ.
        let (lam, phi) = self.generic_inverse(x, y, lam, phi, 1e-12)?;
        Ok((lam + self.lon0, phi))
    }
}

impl Cassini {
    /// Newton's method in two variables with numerical derivatives (PROJ's
    /// `pj_generic_inverse_2d`).
    fn generic_inverse(
        &self,
        x: f64,
        y: f64,
        lam0: f64,
        phi0: f64,
        tolerance: f64,
    ) -> Result<(f64, f64), CoordinateError> {
        let (mut lam, mut phi) = (lam0, phi0);
        let (mut deriv_lam_x, mut deriv_lam_y, mut deriv_phi_x, mut deriv_phi_y) =
            (0.0, 0.0, 0.0, 0.0);
        for i in 0..15 {
            let (xa, ya) = self.fwd(lam, phi);
            let dx = xa - x;
            let dy = ya - y;
            if dx.abs() < tolerance && dy.abs() < tolerance {
                return Ok((lam, phi));
            }
            if i == 0 || dx.abs() > 1e-6 || dy.abs() > 1e-6 {
                let d_lam = if lam > 0.0 { -1e-6 } else { 1e-6 };
                let (x2, y2) = self.fwd(lam + d_lam, phi);
                let deriv_x_lam = (x2 - xa) / d_lam;
                let deriv_y_lam = (y2 - ya) / d_lam;

                let d_phi = if phi > 0.0 { -1e-6 } else { 1e-6 };
                let (x2, y2) = self.fwd(lam, phi + d_phi);
                let deriv_x_phi = (x2 - xa) / d_phi;
                let deriv_y_phi = (y2 - ya) / d_phi;

                let det = deriv_x_lam * deriv_y_phi - deriv_x_phi * deriv_y_lam;
                if det != 0.0 {
                    deriv_lam_x = deriv_y_phi / det;
                    deriv_lam_y = -deriv_x_phi / det;
                    deriv_phi_x = -deriv_y_lam / det;
                    deriv_phi_y = deriv_x_lam / det;
                }
            }
            // Limit the correction to avoid overshoots from a bad start.
            let delta_lam = (dx * deriv_lam_x + dy * deriv_lam_y).clamp(-0.3, 0.3);
            lam = (lam - delta_lam).clamp(-PI, PI);
            let delta_phi = (dx * deriv_phi_x + dy * deriv_phi_y).clamp(-0.3, 0.3);
            phi = (phi - delta_phi).clamp(-FRAC_PI_2, FRAC_PI_2);
        }
        Err(CoordinateError::OutsideDomain)
    }
}
