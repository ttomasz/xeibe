//! Lambert Cylindrical Equal Area (EPSG:9835) and its spherical variant
//! (EPSG:9834).
//!
//! Ported from PROJ 9.8.1 `src/projections/cea.cpp` (`cea`; MIT, see
//! PROJ-NOTICE.md). PROJ maps the spherical variant to `cea +R_A`: the
//! ellipsoid is replaced by the sphere of equal area, using the series from
//! `src/ell_set.cpp`.

use std::f64::consts::FRAC_PI_2;

use super::authalic_q::Authalic;
use super::{BuildError, Params, Projection, adjlon, param};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

const EPS: f64 = 1e-10;

#[derive(Debug)]
pub(crate) struct CylindricalEqualArea {
    lon0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
    k0: f64,
    /// `None` for a sphere.
    authalic: Option<Authalic>,
}

impl CylindricalEqualArea {
    pub(crate) fn ellipsoidal(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        Self::new(params, ellipsoid)
    }

    /// With the ellipsoid replaced by the sphere of equal area (`+R_A`).
    pub(crate) fn authalic_sphere(
        params: &Params,
        ellipsoid: &Ellipsoid,
    ) -> Result<Self, BuildError> {
        const RA4: f64 = 17.0 / 360.0;
        const RA6: f64 = 67.0 / 3024.0;
        let es = ellipsoid.es;
        let radius = ellipsoid.a * (1.0 - es * (1.0 / 6.0 + es * (RA4 + es * RA6)));
        Self::new(params, &Ellipsoid::sphere(radius))
    }

    fn new(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        let lat_ts = params.get(param::LAT_1ST_PARALLEL)?;
        let mut k0 = lat_ts.cos();
        if k0 < 0.0 {
            return Err(BuildError::Invalid("|lat_ts| should be <= 90°".to_owned()));
        }
        let authalic = if ellipsoid.is_sphere() {
            None
        } else {
            let t = lat_ts.sin();
            k0 /= (1.0 - ellipsoid.es * t * t).sqrt();
            Some(Authalic::new(ellipsoid))
        };
        Ok(CylindricalEqualArea {
            lon0: params.get(param::LON_NATURAL_ORIGIN)?,
            fe: params.get(param::FALSE_EASTING)?,
            fn_: params.get(param::FALSE_NORTHING)?,
            a: ellipsoid.a,
            k0,
            authalic,
        })
    }
}

impl Projection for CylindricalEqualArea {
    fn forward(&self, lon: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = adjlon(lon - self.lon0);
        let x = self.k0 * lam;
        let y = match &self.authalic {
            Some(au) => 0.5 * au.q(phi.sin()) / self.k0,
            None => phi.sin() / self.k0,
        };
        Ok((self.a * x + self.fe, self.a * y + self.fn_))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let x = (x - self.fe) / self.a;
        let y = (y - self.fn_) / self.a;
        let phi = match &self.authalic {
            Some(au) => {
                let phi = au.inverse((2.0 * y * self.k0 / au.qp).asin());
                if phi.is_nan() {
                    return Err(CoordinateError::OutsideDomain);
                }
                phi
            }
            None => {
                let y = y * self.k0;
                let t = y.abs();
                if t - EPS > 1.0 {
                    return Err(CoordinateError::OutsideDomain);
                }
                if t >= 1.0 {
                    FRAC_PI_2.copysign(y)
                } else {
                    y.asin()
                }
            }
        };
        Ok((x / self.k0 + self.lon0, phi))
    }
}
