//! Mercator (variant A, EPSG:9804; variant B, 9805), Popular Visualisation
//! Pseudo Mercator (1024) and Mercator (1SP) (Spherical) (9841).
//!
//! Ported from PROJ 9.8.1 `src/projections/merc.cpp` (MIT, see
//! PROJ-NOTICE.md), with EPSG's parameters mapped as PROJ maps them. Variant A
//! takes a scale factor, variant B a standard parallel (which gives the scale
//! factor), and the latitude of natural origin is ignored. Pseudo Mercator
//! applies the spherical formulas to the ellipsoid's semi-major axis. The
//! spherical variant uses the radius of the conformal sphere at the origin
//! latitude (`+R_C`).

use super::common::{msfn, sinhpsi2tanphi};
use super::{BuildError, Params, Projection, param};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

#[derive(Debug)]
pub(crate) struct Mercator {
    lon0: f64,
    k0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
    /// Zero for the spherical formulas.
    e: f64,
}

impl Mercator {
    /// Variant A (9804).
    pub(crate) fn variant_a(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        Ok(Self::with(
            ellipsoid.a,
            ellipsoid.e,
            params.get(param::LON_NATURAL_ORIGIN)?,
            params.get(param::SCALE_NATURAL_ORIGIN)?,
            params.get(param::FALSE_EASTING)?,
            params.get(param::FALSE_NORTHING)?,
        ))
    }

    /// Variant B (9805): the scale factor comes from the standard parallel.
    pub(crate) fn variant_b(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        let phits = params.get(param::LAT_1ST_PARALLEL)?.abs();
        if phits >= std::f64::consts::FRAC_PI_2 {
            return Err(BuildError::Invalid(
                "the standard parallel must be within ±90°".to_owned(),
            ));
        }
        let (sin, cos) = phits.sin_cos();
        let k0 = if ellipsoid.is_sphere() {
            cos
        } else {
            msfn(sin, cos, ellipsoid.es)
        };
        Ok(Self::with(
            ellipsoid.a,
            ellipsoid.e,
            params.get(param::LON_NATURAL_ORIGIN)?,
            k0,
            params.get(param::FALSE_EASTING)?,
            params.get(param::FALSE_NORTHING)?,
        ))
    }

    /// Popular Visualisation Pseudo Mercator (1024): spherical formulas on
    /// the semi-major axis, scale factor 1.
    pub(crate) fn pseudo(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        Ok(Self::with(
            ellipsoid.a,
            0.0,
            params.get(param::LON_NATURAL_ORIGIN)?,
            1.0,
            params.get(param::FALSE_EASTING)?,
            params.get(param::FALSE_NORTHING)?,
        ))
    }

    /// Mercator (1SP) (Spherical) (9841): a sphere of the conformal radius at
    /// the latitude of natural origin, scale factor 1.
    pub(crate) fn spherical(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        let phi0 = params.get(param::LAT_NATURAL_ORIGIN)?;
        let t = 1.0 - ellipsoid.es * phi0.sin().powi(2);
        if t == 0.0 {
            return Err(BuildError::Invalid("invalid eccentricity".to_owned()));
        }
        let a = ellipsoid.a * (1.0 - ellipsoid.es).sqrt() / t;
        Ok(Self::with(
            a,
            0.0,
            params.get(param::LON_NATURAL_ORIGIN)?,
            1.0,
            params.get(param::FALSE_EASTING)?,
            params.get(param::FALSE_NORTHING)?,
        ))
    }

    fn with(a: f64, e: f64, lon0: f64, k0: f64, fe: f64, fn_: f64) -> Self {
        Mercator {
            lon0,
            k0,
            fe,
            fn_,
            a,
            e,
        }
    }
}

impl Projection for Mercator {
    fn forward(&self, lon: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = super::adjlon(lon - self.lon0);
        let x = self.k0 * lam;
        let y = if self.e == 0.0 {
            self.k0 * phi.tan().asinh()
        } else {
            let (s, c) = phi.sin_cos();
            self.k0 * ((s / c).asinh() - self.e * (self.e * s).atanh())
        };
        if !y.is_finite() {
            return Err(CoordinateError::OutsideDomain);
        }
        Ok((self.a * x + self.fe, self.a * y + self.fn_))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let x = (x - self.fe) / self.a;
        let y = (y - self.fn_) / self.a;
        let sinh = (y / self.k0).sinh();
        let phi = if self.e == 0.0 {
            sinh.atan()
        } else {
            sinhpsi2tanphi(sinh, self.e)?.atan()
        };
        Ok((x / self.k0 + self.lon0, phi))
    }
}
