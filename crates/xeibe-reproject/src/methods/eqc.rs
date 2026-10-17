//! Equidistant Cylindrical (EPSG:1028, and 9842 as EPSG once numbered it) and
//! Equidistant Cylindrical (Spherical) (EPSG:1029, 9823).
//!
//! Ported from PROJ 9.8.1 `src/projections/eqc.cpp` (`eqc`; MIT, see
//! PROJ-NOTICE.md). PROJ maps all four to the same `eqc` and chooses the
//! formulas by the ellipsoid of the base CRS, not by the method: with an
//! ellipsoid the northing is the meridian arc, whatever the method is called.
//! The latitude of the standard parallel (8823) and the one of the natural
//! origin (8801) are both optional and default to 0, as in PROJ; the
//! longitude of origin is 8802, or 8822 as the old 9842 spells it.

use super::mlfn::Mlfn;
use super::{BuildError, Params, Projection, adjlon, param};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

#[derive(Debug)]
pub(crate) struct EquidistantCylindrical {
    lon0: f64,
    phi0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
    /// Spherical: cos φ₁. Ellipsoidal: ν₁ cos φ₁ (in units of a).
    rc: f64,
    /// Meridian arc and its value at the origin; `None` for a sphere.
    arc: Option<(Mlfn, f64)>,
}

impl EquidistantCylindrical {
    pub(crate) fn new(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        let phi1 = params.get(param::LAT_1ST_PARALLEL).unwrap_or(0.0);
        let phi0 = params.get(param::LAT_NATURAL_ORIGIN).unwrap_or(0.0);
        let lon0 = match params.get(param::LON_NATURAL_ORIGIN) {
            Ok(v) => v,
            Err(_) => params.get(param::LON_FALSE_ORIGIN)?,
        };
        let cos_phi1 = phi1.cos();
        if cos_phi1 <= 0.0 {
            return Err(BuildError::Invalid("|lat_ts| should be <= 90°".to_owned()));
        }
        let (rc, arc) = if ellipsoid.is_sphere() {
            (cos_phi1, None)
        } else {
            let sin_phi1 = phi1.sin();
            let nu1 = 1.0 / (1.0 - ellipsoid.es * sin_phi1 * sin_phi1).sqrt();
            let en = Mlfn::new(ellipsoid.n);
            let m0 = en.mlfn(phi0, phi0.sin(), phi0.cos());
            (nu1 * cos_phi1, Some((en, m0)))
        };
        Ok(EquidistantCylindrical {
            lon0,
            phi0,
            fe: params.get(param::FALSE_EASTING)?,
            fn_: params.get(param::FALSE_NORTHING)?,
            a: ellipsoid.a,
            rc,
            arc,
        })
    }
}

impl Projection for EquidistantCylindrical {
    fn forward(&self, lon: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = adjlon(lon - self.lon0);
        let x = self.rc * lam;
        let y = match &self.arc {
            Some((en, m0)) => en.mlfn(phi, phi.sin(), phi.cos()) - m0,
            None => phi - self.phi0,
        };
        Ok((self.a * x + self.fe, self.a * y + self.fn_))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let x = (x - self.fe) / self.a;
        let y = (y - self.fn_) / self.a;
        let phi = match &self.arc {
            Some((en, m0)) => en.inv_mlfn(y + m0),
            None => y + self.phi0,
        };
        Ok((x / self.rc + self.lon0, phi))
    }
}
