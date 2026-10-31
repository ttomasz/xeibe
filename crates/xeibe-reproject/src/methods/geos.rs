//! Geostationary Satellite, sweep axis X or Y: the view from a satellite
//! over the equator.
//!
//! Ported from PROJ 9.8.1 `src/projections/geos.cpp` (MIT, see
//! PROJ-NOTICE.md), in units of the semi-major axis.

use super::sphere::Unit;
use super::{BuildError, Params, proj_codes};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

#[derive(Debug)]
pub(crate) struct Geostationary {
    /// Sweep axis X.
    flip_axis: bool,
    radius_g: f64,
    radius_g_1: f64,
    c: f64,
    /// `None` on a sphere; else √(1 − e²), 1 − e² and 1 / (1 − e²).
    ellipsoidal: Option<(f64, f64, f64)>,
}

impl Geostationary {
    pub(crate) fn new(
        params: &Params,
        ellipsoid: &Ellipsoid,
        sweep_x: bool,
    ) -> Result<Self, BuildError> {
        let radius_g_1 = params.get(proj_codes::SATELLITE_HEIGHT)? / ellipsoid.a;
        if radius_g_1 <= 0.0 || radius_g_1 > 1e10 {
            return Err(BuildError::Invalid("Invalid value for h.".into()));
        }
        let radius_g = 1.0 + radius_g_1;
        let one_es = 1.0 - ellipsoid.es;
        Ok(Geostationary {
            flip_axis: sweep_x,
            radius_g,
            radius_g_1,
            c: radius_g * radius_g - 1.0,
            ellipsoidal: (ellipsoid.es != 0.0).then(|| (one_es.sqrt(), one_es, 1.0 / one_es)),
        })
    }

    /// The view angles of the vector from the satellite, as x and y.
    fn angles(&self, vx: f64, vy: f64, vz: f64) -> (f64, f64) {
        let tmp = self.radius_g - vx;
        if self.flip_axis {
            (
                self.radius_g_1 * (vy / vz.hypot(tmp)).atan(),
                self.radius_g_1 * (vz / tmp).atan(),
            )
        } else {
            (
                self.radius_g_1 * (vy / tmp).atan(),
                self.radius_g_1 * (vz / vy.hypot(tmp)).atan(),
            )
        }
    }
}

impl Unit for Geostationary {
    fn fwd(&self, lam: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let Some((radius_p, radius_p2, radius_p_inv2)) = self.ellipsoidal else {
            let tmp = phi.cos();
            return Ok(self.angles(lam.cos() * tmp, lam.sin() * tmp, phi.sin()));
        };
        // Geocentric latitude.
        let phi = (radius_p2 * phi.tan()).atan();
        let r = radius_p / (radius_p * phi.cos()).hypot(phi.sin());
        let vx = r * lam.cos() * phi.cos();
        let vy = r * lam.sin() * phi.cos();
        let vz = r * phi.sin();
        // Visible from the satellite?
        if (self.radius_g - vx) * vx - vy * vy - vz * vz * radius_p_inv2 < 0.0 {
            return Err(CoordinateError::OutsideDomain);
        }
        Ok(self.angles(vx, vy, vz))
    }

    fn inv(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let vx = -1.0;
        let (vy, vz) = if self.flip_axis {
            let vz = (y / self.radius_g_1).tan();
            ((x / self.radius_g_1).tan() * 1.0_f64.hypot(vz), vz)
        } else {
            let vy = (x / self.radius_g_1).tan();
            (vy, (y / self.radius_g_1).tan() * 1.0_f64.hypot(vy))
        };
        let a = match self.ellipsoidal {
            Some((radius_p, _, _)) => {
                let a = vz / radius_p;
                vy * vy + a * a + vx * vx
            }
            None => vy * vy + vz * vz + vx * vx,
        };
        let b = 2.0 * self.radius_g * vx;
        let det = b * b - 4.0 * a * self.c;
        if det < 0.0 {
            return Err(CoordinateError::OutsideDomain);
        }
        let k = (-b - det.sqrt()) / (2.0 * a);
        let vx = self.radius_g + k * vx;
        let (vy, vz) = (vy * k, vz * k);
        let lam = vy.atan2(vx);
        let phi = match self.ellipsoidal {
            Some((_, _, radius_p_inv2)) => (radius_p_inv2 * vz / vx.hypot(vy)).atan(),
            None => (vz / vx.hypot(vy)).atan(),
        };
        Ok((lam, phi))
    }
}
