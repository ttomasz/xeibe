//! Two Point Equidistant, on the sphere: distances from two points are
//! true.
//!
//! Ported from PROJ 9.8.1 `src/projections/tpeqd.cpp` (MIT, see
//! PROJ-NOTICE.md). PROJ centres it halfway between the two points'
//! longitudes, whatever the CRS's central meridian.

use std::f64::consts::FRAC_PI_2;

use super::sphere::{Unit, aacos, aasin, asqrt};
use super::{BuildError, Params, adjlon, proj_codes};
use crate::error::CoordinateError;

#[derive(Debug)]
pub(crate) struct TwoPointEquidistant {
    cp1: f64,
    sp1: f64,
    cp2: f64,
    sp2: f64,
    ccs: f64,
    cs: f64,
    sc: f64,
    r2z0: f64,
    z02: f64,
    dlam2: f64,
    hz0: f64,
    thz0: f64,
    rhshz0: f64,
    ca: f64,
    sa: f64,
    lp: f64,
    lamc: f64,
}

impl TwoPointEquidistant {
    /// The projection, and the central meridian PROJ gives it.
    pub(crate) fn new(params: &Params) -> Result<(Self, f64), BuildError> {
        let phi_1 = params.get(proj_codes::LATITUDE_OF_1ST_POINT)?;
        let lam_1 = params.get(proj_codes::LONGITUDE_OF_1ST_POINT)?;
        let phi_2 = params.get(proj_codes::LATITUDE_OF_2ND_POINT)?;
        let lam_2 = params.get(proj_codes::LONGITUDE_OF_2ND_POINT)?;
        if phi_1 == phi_2 && lam_1 == lam_2 {
            return Err(BuildError::Invalid(
                "Invalid value for lat_1/lon_1/lat_2/lon_2: the 2 points should be distinct."
                    .into(),
            ));
        }
        let lam0 = adjlon(0.5 * (lam_1 + lam_2));
        let dlam2 = adjlon(lam_2 - lam_1);
        let (sp1, cp1) = phi_1.sin_cos();
        let (sp2, cp2) = phi_2.sin_cos();
        let cs = cp1 * sp2;
        let sc = sp1 * cp2;
        let ccs = cp1 * cp2 * dlam2.sin();
        // The central angle between the points, by Vincenty's formula on the
        // sphere.
        let cs_minus_sc_cos_dlam = cs - sc * dlam2.cos();
        let z02 = ((cp2 * dlam2.sin()).powi(2) + cs_minus_sc_cos_dlam.powi(2))
            .sqrt()
            .atan2(sp1 * sp2 + cp1 * cp2 * dlam2.cos());
        if z02 == 0.0 {
            return Err(BuildError::Invalid(
                "Invalid value for lat_1 and lat_2: their absolute value should be < 90°.".into(),
            ));
        }
        let hz0 = 0.5 * z02;
        let a12 = (cp2 * dlam2.sin()).atan2(cs_minus_sc_cos_dlam);
        let pp =
            aasin(cp1 * a12.sin()).map_err(|_| BuildError::Invalid("the two points".into()))?;
        let lp = adjlon((cp1 * a12.cos()).atan2(sp1) - hz0);
        let dlam2 = dlam2 * 0.5;
        Ok((
            TwoPointEquidistant {
                cp1,
                sp1,
                cp2,
                sp2,
                ccs,
                cs,
                sc,
                r2z0: 0.5 / z02,
                z02: z02 * z02,
                dlam2,
                hz0,
                thz0: hz0.tan(),
                rhshz0: 0.5 / hz0.sin(),
                ca: pp.cos(),
                sa: pp.sin(),
                lp,
                lamc: FRAC_PI_2 - (a12.sin() * sp1).atan2(a12.cos()) - dlam2,
            },
            lam0,
        ))
    }
}

impl Unit for TwoPointEquidistant {
    fn fwd(&self, lam: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let (sp, cp) = phi.sin_cos();
        let dl1 = lam + self.dlam2;
        let dl2 = lam - self.dlam2;
        let z1 = aacos(self.sp1 * sp + self.cp1 * cp * dl1.cos())?;
        let z2 = aacos(self.sp2 * sp + self.cp2 * cp * dl2.cos())?;
        let (z1, z2) = (z1 * z1, z2 * z2);
        let t = z1 - z2;
        let x = self.r2z0 * t;
        let t = self.z02 - t;
        let mut y = self.r2z0 * asqrt(4.0 * self.z02 * z2 - t * t);
        if self.ccs * sp - cp * (self.cs * dl1.sin() - self.sc * dl2.sin()) < 0.0 {
            y = -y;
        }
        Ok((x, y))
    }

    fn inv(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let cz1 = y.hypot(x + self.hz0).cos();
        let cz2 = y.hypot(x - self.hz0).cos();
        let s = cz1 + cz2;
        let d = cz1 - cz2;
        let mut lam = -d.atan2(s * self.thz0);
        let mut phi = aacos((self.thz0 * s).hypot(d) * self.rhshz0)?;
        if y < 0.0 {
            phi = -phi;
        }
        // Longitude and latitude are now relative to the equator through
        // the two points.
        let (sp, cp) = phi.sin_cos();
        lam -= self.lp;
        let s = lam.cos();
        let phi = aasin(self.sa * sp + self.ca * cp * s)?;
        let lam = (cp * lam.sin()).atan2(self.sa * cp * s - self.ca * sp) + self.lamc;
        Ok((lam, phi))
    }
}
