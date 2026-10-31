//! Equidistant Conic (EPSG:1119, and PROJ's own entry for it with a natural
//! origin), on a sphere or an ellipsoid.
//!
//! Ported from PROJ 9.8.1 `src/projections/eqdc.cpp` (MIT, see
//! PROJ-NOTICE.md).

use std::f64::consts::FRAC_PI_2;

use super::common::msfn;
use super::mlfn::Mlfn;
use super::{BuildError, Params, Projection, adjlon, param};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

const EPS10: f64 = 1e-10;

#[derive(Debug)]
pub(crate) struct EquidistantConic {
    lon0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
    n: f64,
    c: f64,
    rho0: f64,
    /// `None` on a sphere.
    en: Option<Mlfn>,
}

impl EquidistantConic {
    /// With EPSG's false origin (EPSG:1119), or PROJ's natural origin.
    pub(crate) fn new(
        params: &Params,
        ellipsoid: &Ellipsoid,
        false_origin: bool,
    ) -> Result<Self, BuildError> {
        let (lat0, lon0, fe, fn_) = if false_origin {
            (
                param::LAT_FALSE_ORIGIN,
                param::LON_FALSE_ORIGIN,
                param::EASTING_FALSE_ORIGIN,
                param::NORTHING_FALSE_ORIGIN,
            )
        } else {
            (
                param::LAT_NATURAL_ORIGIN,
                param::LON_NATURAL_ORIGIN,
                param::FALSE_EASTING,
                param::FALSE_NORTHING,
            )
        };
        let phi0 = params.get(lat0)?;
        let phi1 = params.get(param::LAT_1ST_PARALLEL)?;
        let phi2 = params.get(param::LAT_2ND_PARALLEL)?;
        let invalid = |m: &str| Err(BuildError::Invalid(m.into()));
        if phi1.abs() > FRAC_PI_2 {
            return invalid("Invalid value for lat_1: |lat_1| should be <= 90°");
        }
        if phi2.abs() > FRAC_PI_2 {
            return invalid("Invalid value for lat_2: |lat_2| should be <= 90°");
        }
        if (phi1 + phi2).abs() < EPS10 {
            return invalid("Invalid value for lat_1 and lat_2: |lat_1 + lat_2| should be > 0");
        }
        let (mut sinphi, mut cosphi) = phi1.sin_cos();
        let mut n = sinphi;
        let secant = (phi1 - phi2).abs() >= EPS10;
        let es = ellipsoid.es;
        let (c, rho0, en) = if es > 0.0 {
            let en = Mlfn::new(ellipsoid.n);
            let m1 = msfn(sinphi, cosphi, es);
            let ml1 = en.mlfn(phi1, sinphi, cosphi);
            if secant {
                (sinphi, cosphi) = phi2.sin_cos();
                let ml2 = en.mlfn(phi2, sinphi, cosphi);
                if ml1 == ml2 {
                    return invalid("Eccentricity too close to 1");
                }
                n = (m1 - msfn(sinphi, cosphi, es)) / (ml2 - ml1);
                if n == 0.0 {
                    return invalid("Invalid value for eccentricity");
                }
            }
            let c = ml1 + m1 / n;
            let rho0 = c - en.mlfn(phi0, phi0.sin(), phi0.cos());
            (c, rho0, Some(en))
        } else {
            if secant {
                n = (cosphi - phi2.cos()) / (phi2 - phi1);
            }
            if n == 0.0 {
                return invalid("Invalid value for lat_1 and lat_2: lat_1 + lat_2 should be > 0");
            }
            let c = phi1 + phi1.cos() / n;
            (c, c - phi0, None)
        };
        Ok(EquidistantConic {
            lon0: params.get(lon0)?,
            fe: params.get(fe)?,
            fn_: params.get(fn_)?,
            a: ellipsoid.a,
            n,
            c,
            rho0,
            en,
        })
    }
}

impl Projection for EquidistantConic {
    fn forward(&self, lon: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = adjlon(lon - self.lon0);
        let rho = self.c
            - match &self.en {
                Some(en) => en.mlfn(phi, phi.sin(), phi.cos()),
                None => phi,
            };
        let lam_n = lam * self.n;
        let x = rho * lam_n.sin();
        let y = self.rho0 - rho * lam_n.cos();
        Ok((self.a * x + self.fe, self.a * y + self.fn_))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let mut x = (x - self.fe) / self.a;
        let mut y = self.rho0 - (y - self.fn_) / self.a;
        let mut rho = x.hypot(y);
        if rho == 0.0 {
            return Ok((self.lon0, if self.n > 0.0 { FRAC_PI_2 } else { -FRAC_PI_2 }));
        }
        if self.n < 0.0 {
            rho = -rho;
            x = -x;
            y = -y;
        }
        let mut phi = self.c - rho;
        if let Some(en) = &self.en {
            phi = en.inv_mlfn(phi);
        }
        Ok((x.atan2(y) / self.n + self.lon0, phi))
    }
}
