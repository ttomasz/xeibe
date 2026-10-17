//! Albers Equal Area (EPSG:9822).
//!
//! Ported from PROJ 9.8.1 `src/projections/aea.cpp` (`aea`; MIT, see
//! PROJ-NOTICE.md). EPSG's latitude and longitude of false origin become
//! `lat_0` and `lon_0`, the two standard parallels `lat_1` and `lat_2`.

use std::f64::consts::FRAC_PI_2;

use super::authalic_q::Authalic;
use super::common::msfn;
use super::{BuildError, Params, Projection, adjlon, param};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

const EPS10: f64 = 1e-10;
const TOL7: f64 = 1e-7;

#[derive(Debug)]
pub(crate) struct AlbersEqualArea {
    lon0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
    /// `None` for a sphere.
    authalic: Option<Authalic>,
    ec: f64,
    n: f64,
    c: f64,
    dd: f64,
    n2: f64,
    rho0: f64,
}

impl AlbersEqualArea {
    pub(crate) fn new(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        let invalid = |m: &str| BuildError::Invalid(m.to_owned());
        let phi0 = params.get(param::LAT_FALSE_ORIGIN)?;
        let phi1 = params.get(param::LAT_1ST_PARALLEL)?;
        let phi2 = params.get(param::LAT_2ND_PARALLEL)?;
        if phi1.abs() > FRAC_PI_2 {
            return Err(invalid("|lat_1| should be <= 90°"));
        }
        if phi2.abs() > FRAC_PI_2 {
            return Err(invalid("|lat_2| should be <= 90°"));
        }
        if (phi1 + phi2).abs() < EPS10 {
            return Err(invalid("|lat_1 + lat_2| should be > 0"));
        }
        let (mut sinphi, mut cosphi) = phi1.sin_cos();
        let mut n = sinphi;
        let secant = (phi1 - phi2).abs() >= EPS10;
        let es = ellipsoid.es;
        let (ec, c, dd, n2, rho0);
        let authalic;
        if es > 0.0 {
            let au = Authalic::new(ellipsoid);
            let m1 = msfn(sinphi, cosphi, es);
            let ml1 = au.q(sinphi);
            if secant {
                sinphi = phi2.sin();
                cosphi = phi2.cos();
                let m2 = msfn(sinphi, cosphi, es);
                let ml2 = au.q(sinphi);
                if ml2 == ml1 {
                    return Err(invalid("invalid standard parallels"));
                }
                n = (m1 * m1 - m2 * m2) / (ml2 - ml1);
                if n == 0.0 {
                    return Err(invalid("invalid eccentricity"));
                }
            }
            ec = 1.0
                - 0.5 * (1.0 - es) * ((1.0 - ellipsoid.e) / (1.0 + ellipsoid.e)).ln() / ellipsoid.e;
            c = m1 * m1 + n * ml1;
            dd = 1.0 / n;
            rho0 = dd * (c - n * au.q(phi0.sin())).sqrt();
            n2 = 0.0;
            authalic = Some(au);
        } else {
            if secant {
                n = 0.5 * (n + phi2.sin());
            }
            n2 = n + n;
            c = cosphi * cosphi + n2 * sinphi;
            dd = 1.0 / n;
            rho0 = dd * (c - n2 * phi0.sin()).sqrt();
            ec = 0.0;
            authalic = None;
        }
        Ok(AlbersEqualArea {
            lon0: params.get(param::LON_FALSE_ORIGIN)?,
            fe: params.get(param::EASTING_FALSE_ORIGIN)?,
            fn_: params.get(param::NORTHING_FALSE_ORIGIN)?,
            a: ellipsoid.a,
            authalic,
            ec,
            n,
            c,
            dd,
            n2,
            rho0,
        })
    }
}

impl Projection for AlbersEqualArea {
    fn forward(&self, lon: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = adjlon(lon - self.lon0);
        let q = match &self.authalic {
            Some(au) => self.n * au.q(phi.sin()),
            None => self.n2 * phi.sin(),
        };
        let rho = self.c - q;
        if rho < 0.0 {
            return Err(CoordinateError::OutsideDomain);
        }
        let rho = self.dd * rho.sqrt();
        let lam = lam * self.n;
        let x = rho * lam.sin();
        let y = self.rho0 - rho * lam.cos();
        Ok((self.a * x + self.fe, self.a * y + self.fn_))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let mut x = (x - self.fe) / self.a;
        let mut y = self.rho0 - (y - self.fn_) / self.a;
        let mut rho = x.hypot(y);
        if rho == 0.0 {
            let phi = if self.n > 0.0 { FRAC_PI_2 } else { -FRAC_PI_2 };
            return Ok((self.lon0, phi));
        }
        if self.n < 0.0 {
            rho = -rho;
            x = -x;
            y = -y;
        }
        let phi = rho / self.dd;
        let phi = match &self.authalic {
            Some(au) => {
                let qs = (self.c - phi * phi) / self.n;
                if (self.ec - qs.abs()).abs() > TOL7 {
                    if qs.abs() > 2.0 {
                        return Err(CoordinateError::OutsideDomain);
                    }
                    let phi = au.inverse((qs / au.qp).asin());
                    if phi.is_nan() {
                        return Err(CoordinateError::OutsideDomain);
                    }
                    phi
                } else if qs < 0.0 {
                    -FRAC_PI_2
                } else {
                    FRAC_PI_2
                }
            }
            None => {
                let qs_div_2 = (self.c - phi * phi) / self.n2;
                if qs_div_2.abs() <= 1.0 {
                    qs_div_2.asin()
                } else if qs_div_2 < 0.0 {
                    -FRAC_PI_2
                } else {
                    FRAC_PI_2
                }
            }
        };
        let lam = x.atan2(y) / self.n;
        Ok((lam + self.lon0, phi))
    }
}
