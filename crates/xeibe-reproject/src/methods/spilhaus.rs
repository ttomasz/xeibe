//! Spilhaus: Adams World in a Square II of the sphere turned so that the
//! oceans make one piece, on the conformal sphere of an ellipsoid.
//!
//! Ported from PROJ 9.8.1 `src/projections/spilhaus.cpp` and
//! `src/latitudes.cpp` (MIT, see PROJ-NOTICE.md). PROJ's defaults are from
//! PROJ issue 1851.

use std::f64::consts::{PI, TAU};

use super::adams::Adams;
use super::common::sinhpsi2tanphi;
use super::conic::k0;
use super::sphere::{Spherical, Unit, aasin, aatan2};
use super::{BuildError, Params};
use crate::error::CoordinateError;

#[derive(Debug)]
pub(crate) struct Spilhaus {
    e: f64,
    k0: f64,
    cosalpha: f64,
    sinalpha: f64,
    beta: f64,
    lambda_0: f64,
    conformal_distortion: f64,
    cosrot: f64,
    sinrot: f64,
}

/// The conformal latitude (PROJ's `pj_conformal_lat`).
fn conformal(phi: f64, e: f64) -> f64 {
    if e == 0.0 {
        return phi;
    }
    let (sphi, cphi) = phi.sin_cos();
    ((sphi / cphi).asinh() - e * (e * sphi).atanh())
        .sinh()
        .atan()
}

/// The geodetic latitude of a conformal one (`pj_conformal_lat_inverse`).
fn conformal_inverse(chi: f64, e: f64) -> Result<f64, CoordinateError> {
    if e == 0.0 {
        return Ok(chi);
    }
    Ok(sinhpsi2tanphi(chi.tan(), e)?.atan())
}

impl Spilhaus {
    /// On the sphere of the semi-major axis, its eccentricity kept for
    /// the conformal latitude.
    pub(crate) fn build(
        params: &Params,
        a: f64,
        e: f64,
    ) -> Result<Spherical<Spilhaus>, BuildError> {
        let lam0 = params.key("lon_0").unwrap_or(66.94970198f64.to_radians());
        let phi0 = params
            .key("lat_0")
            .unwrap_or((-49.56371678f64).to_radians());
        let azimuth = params.key("azi").unwrap_or(40.17823482f64.to_radians());
        let rotation = params.key("rot").unwrap_or(45f64.to_radians());
        let chi_c = conformal(phi0, e);
        let sinalpha = -chi_c.cos() * azimuth.cos();
        let es = e * e;
        let unit = Spilhaus {
            e,
            k0: k0(params),
            cosalpha: (1.0 - sinalpha * sinalpha).sqrt(),
            sinalpha,
            beta: PI + (-azimuth.sin()).atan2(-chi_c.tan()),
            lambda_0: azimuth.tan().atan2(-chi_c.sin()),
            conformal_distortion: phi0.cos()
                / (1.0 - es * phi0.sin() * phi0.sin()).sqrt()
                / chi_c.cos(),
            cosrot: rotation.cos(),
            sinrot: rotation.sin(),
        };
        let mut s = Spherical::from_keys(unit, params, a);
        s.set_lon0(lam0);
        Ok(s)
    }
}

impl Unit for Spilhaus {
    fn fwd(&self, lam: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let (sinphi_c, cosphi_c) = conformal(phi, self.e).sin_cos();
        let (sinlam, coslam) = (lam - self.lambda_0).sin_cos();
        // Snyder, A working manual, formulas 5-7 and 5-8b.
        let phi = aasin(self.sinalpha * sinphi_c - self.cosalpha * cosphi_c * coslam)?;
        let mut lam = self.beta
            + (cosphi_c * sinlam)
                .atan2(self.sinalpha * cosphi_c * coslam + self.cosalpha * sinphi_c);
        while lam > PI {
            lam -= TAU;
        }
        while lam < -PI {
            lam += TAU;
        }
        let (x, y) = Adams::WorldInASquare2.fwd(lam, phi)?;
        let factor = self.conformal_distortion * self.k0;
        Ok((
            -(x * self.cosrot + y * self.sinrot) * factor,
            -(x * -self.sinrot + y * self.cosrot) * factor,
        ))
    }

    fn inv(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let factor = 1.0 / (self.conformal_distortion * self.k0);
        let (lam, phi) = Adams::WorldInASquare2.inv(
            -(x * self.cosrot + y * -self.sinrot) * factor,
            -(x * self.sinrot + y * self.cosrot) * factor,
        )?;
        let (sinphi_s, cosphi_s) = phi.sin_cos();
        let (sinlam_s, coslam_s) = (lam - self.beta).sin_cos();
        let chi = aasin(self.sinalpha * sinphi_s + self.cosalpha * cosphi_s * coslam_s)?;
        let lam = self.lambda_0
            + aatan2(
                cosphi_s * sinlam_s,
                self.sinalpha * cosphi_s * coslam_s - self.cosalpha * sinphi_s,
            );
        Ok((lam, conformal_inverse(chi, self.e)?))
    }
}
