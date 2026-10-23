//! Lambert Conic Conformal, 1SP (EPSG:9801) and 2SP (EPSG:9802).
//!
//! Ported from PROJ 9.8.1 `src/projections/lcc.cpp` (MIT, see PROJ-NOTICE.md),
//! with EPSG's parameters mapped as PROJ maps them: 1SP is a tangent cone at
//! the natural origin with a scale factor, 2SP a secant cone through the two
//! standard parallels with its origin at the false origin.

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4};

use super::common::{msfn, phi2, tsfn};
use super::{BuildError, Params, Projection, adjlon, param};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

const EPS10: f64 = 1e-10;

/// A Lambert Conic Conformal (2SP)'s parameters as those of the 1SP that
/// projects alike, PROJ's `Conversion::convertToOtherMethod`: two 2SP
/// definitions whose parameters differ (rounded false origins, swapped
/// standard parallels) are equivalent in PROJ when their 1SP forms are.
pub(crate) fn two_sp_as_one_sp(params: &Params, ellipsoid: &Ellipsoid) -> Option<Params> {
    let phi_f = params.get(param::LAT_FALSE_ORIGIN).ok()?;
    let phi1 = params.get(param::LAT_1ST_PARALLEL).ok()?;
    let phi2_ = params.get(param::LAT_2ND_PARALLEL).ok()?;
    if [phi_f, phi1, phi2_].iter().any(|p| p.abs() >= FRAC_PI_2) {
        return None;
    }
    let (es, e) = (ellipsoid.es, ellipsoid.e);
    let m = |phi: f64| msfn(phi.sin(), phi.cos(), es);
    let t = |phi: f64| tsfn(phi, phi.sin(), e);
    let (m1, t1) = (m(phi1), t(phi1));
    let n_denom = t1.ln() - t(phi2_).ln();
    let n = if n_denom.abs() < EPS10 {
        phi1.sin()
    } else {
        (m1.ln() - m(phi2_).ln()) / n_denom
    };
    if n.abs() < EPS10 {
        return None;
    }
    let f = m1 / (n * t1.powf(n));
    let phi0 = n.asin();
    let k0 = f / (m(phi0) / (n * t(phi0).powf(n)));
    let correction = ellipsoid.a * f * (t(phi_f).powf(n) - t(phi0).powf(n));
    // PROJ rounds the latitude to a thousandth of a degree when that close.
    let mut phi0_deg = phi0.to_degrees();
    let thousandths = (phi0_deg * 1000.0 + 0.5).floor();
    if (phi0_deg * 1000.0 - thousandths).abs() < 1e-8 {
        phi0_deg = thousandths / 1000.0;
    }
    let mut out = Params::default();
    out.push(param::LAT_NATURAL_ORIGIN, phi0_deg.to_radians());
    out.push(
        param::LON_NATURAL_ORIGIN,
        params.get(param::LON_FALSE_ORIGIN).ok()?,
    );
    out.push(param::SCALE_NATURAL_ORIGIN, k0);
    out.push(
        param::FALSE_EASTING,
        params.get(param::EASTING_FALSE_ORIGIN).ok()?,
    );
    let northing = params.get(param::NORTHING_FALSE_ORIGIN).ok()?;
    out.push(
        param::FALSE_NORTHING,
        northing
            + if correction.abs() > 1e-8 {
                correction
            } else {
                0.0
            },
    );
    Some(out)
}

#[derive(Debug)]
pub(crate) struct LambertConic {
    lon0: f64,
    k0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
    e: f64,
    spherical: bool,
    n: f64,
    c: f64,
    rho0: f64,
}

impl LambertConic {
    pub(crate) fn one_sp(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        let phi0 = params.get(param::LAT_NATURAL_ORIGIN)?;
        Self::new(
            ellipsoid,
            phi0,
            phi0,
            phi0,
            params.get(param::LON_NATURAL_ORIGIN)?,
            params.get(param::SCALE_NATURAL_ORIGIN)?,
            params.get(param::FALSE_EASTING)?,
            params.get(param::FALSE_NORTHING)?,
        )
    }

    pub(crate) fn two_sp(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        Self::new(
            ellipsoid,
            params.get(param::LAT_FALSE_ORIGIN)?,
            params.get(param::LAT_1ST_PARALLEL)?,
            params.get(param::LAT_2ND_PARALLEL)?,
            params.get(param::LON_FALSE_ORIGIN)?,
            1.0,
            params.get(param::EASTING_FALSE_ORIGIN)?,
            params.get(param::NORTHING_FALSE_ORIGIN)?,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        ellipsoid: &Ellipsoid,
        phi0: f64,
        phi1: f64,
        phi2_: f64,
        lon0: f64,
        k0: f64,
        fe: f64,
        fn_: f64,
    ) -> Result<Self, BuildError> {
        let invalid = |m: &str| BuildError::Invalid(m.to_owned());
        if (phi1 + phi2_).abs() < EPS10 {
            return Err(invalid(
                "the standard parallels must not be symmetric about the equator",
            ));
        }
        let (mut sinphi, cosphi) = phi1.sin_cos();
        let mut n = sinphi;
        if cosphi.abs() < EPS10 || phi1.abs() >= FRAC_PI_2 {
            return Err(invalid("the first standard parallel must be within ±90°"));
        }
        if phi2_.cos().abs() < EPS10 || phi2_.abs() >= FRAC_PI_2 {
            return Err(invalid("the second standard parallel must be within ±90°"));
        }
        let secant = (phi1 - phi2_).abs() >= EPS10;
        let (es, e) = (ellipsoid.es, ellipsoid.e);
        let (c, rho0);
        if es != 0.0 {
            let m1 = msfn(sinphi, cosphi, es);
            let ml1 = tsfn(phi1, sinphi, e);
            if secant {
                sinphi = phi2_.sin();
                n = (m1 / msfn(sinphi, phi2_.cos(), es)).ln();
                let ml2 = tsfn(phi2_, sinphi, e);
                let denom = (ml1 / ml2).ln();
                if n == 0.0 || denom == 0.0 {
                    return Err(invalid("invalid eccentricity"));
                }
                n /= denom;
            }
            c = m1 * ml1.powf(-n) / n;
            rho0 = if ((phi0.abs()) - FRAC_PI_2).abs() < EPS10 {
                0.0
            } else {
                c * tsfn(phi0, phi0.sin(), e).powf(n)
            };
        } else {
            if secant {
                n = (cosphi / phi2_.cos()).ln()
                    / ((FRAC_PI_4 + 0.5 * phi2_).tan() / (FRAC_PI_4 + 0.5 * phi1).tan()).ln();
            }
            if n == 0.0 {
                return Err(invalid(
                    "the standard parallels must not be symmetric about the equator",
                ));
            }
            c = cosphi * (FRAC_PI_4 + 0.5 * phi1).tan().powf(n) / n;
            rho0 = if (phi0.abs() - FRAC_PI_2).abs() < EPS10 {
                0.0
            } else {
                c * (FRAC_PI_4 + 0.5 * phi0).tan().powf(-n)
            };
        }
        Ok(LambertConic {
            lon0,
            k0,
            fe,
            fn_,
            a: ellipsoid.a,
            e,
            spherical: es == 0.0,
            n,
            c,
            rho0,
        })
    }
}

impl Projection for LambertConic {
    fn forward(&self, lon: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = adjlon(lon - self.lon0);
        let rho = if (phi.abs() - FRAC_PI_2).abs() < EPS10 {
            if phi * self.n <= 0.0 {
                return Err(CoordinateError::OutsideDomain);
            }
            0.0
        } else if self.spherical {
            self.c * (FRAC_PI_4 + 0.5 * phi).tan().powf(-self.n)
        } else {
            self.c * tsfn(phi, phi.sin(), self.e).powf(self.n)
        };
        let lam = lam * self.n;
        let x = self.k0 * (rho * lam.sin());
        let y = self.k0 * (self.rho0 - rho * lam.cos());
        Ok((self.a * x + self.fe, self.a * y + self.fn_))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let mut x = (x - self.fe) / self.a / self.k0;
        let mut y = self.rho0 - (y - self.fn_) / self.a / self.k0;
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
        let phi = if self.spherical {
            2.0 * (self.c / rho).powf(1.0 / self.n).atan() - FRAC_PI_2
        } else {
            phi2((rho / self.c).powf(1.0 / self.n), self.e)?
        };
        let lam = x.atan2(y) / self.n;
        Ok((lam + self.lon0, phi))
    }
}
