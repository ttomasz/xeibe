//! What PROJ's spherical projections share: the radius, central meridian
//! and false origin around formulas on the unit sphere, and the guarded
//! inverse trigonometric functions they use.
//!
//! Ported from PROJ 9.8.1 `src/fwd.cpp`, `src/inv.cpp`, `src/aasincos.cpp`,
//! `src/ell_set.cpp` (`R_A`) and `src/generic_inverse.cpp` (MIT, see
//! PROJ-NOTICE.md). A projection PROJ defines only on a sphere sets the
//! eccentricity to zero and works on the sphere of the semi-major axis
//! (Robinson on WGS 84 has a radius of 6378137 m), unless its CRS method
//! asks for the authalic sphere (`R_A`: Miller, Van der Grinten).

use std::f64::consts::{FRAC_PI_2, PI};

use super::{BuildError, Params, Projection, adjlon, param};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

/// Formulas on the unit sphere, longitude counted from the central
/// meridian (PROJ's `P->fwd` and `P->inv` with `P->es` zero).
pub(crate) trait Unit: Send + Sync + std::fmt::Debug {
    /// Longitude (reduced to ±π) and latitude to x and y on the unit sphere.
    fn fwd(&self, lam: f64, phi: f64) -> Result<(f64, f64), CoordinateError>;

    /// x and y on the unit sphere to longitude and latitude.
    fn inv(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError>;
}

/// A spherical projection: `Unit` formulas scaled to a radius, around a
/// central meridian and a false origin, as PROJ's `fwd_prepare` and
/// `fwd_finalize` (and their inverses) do.
#[derive(Debug)]
pub(crate) struct Spherical<U> {
    unit: U,
    lon0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
    ra: f64,
}

impl<U: Unit> Spherical<U> {
    /// On the sphere of the semi-major axis, with the usual longitude of
    /// natural origin, false easting and false northing.
    pub(crate) fn new(unit: U, params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        Self::with(
            unit,
            params.get(param::LON_NATURAL_ORIGIN)?,
            params,
            ellipsoid.a,
        )
    }

    /// On the sphere with the ellipsoid's area (PROJ's `R_A`).
    pub(crate) fn authalic(
        unit: U,
        params: &Params,
        ellipsoid: &Ellipsoid,
    ) -> Result<Self, BuildError> {
        const SIXTH: f64 = 1.0 / 6.0;
        const RA4: f64 = 17.0 / 360.0;
        const RA6: f64 = 67.0 / 3024.0;
        let es = ellipsoid.es;
        let a = ellipsoid.a * (1.0 - es * (SIXTH + es * (RA4 + es * RA6)));
        Self::with(unit, params.get(param::LON_NATURAL_ORIGIN)?, params, a)
    }

    /// From PROJ keys (a projection PROJ has no CRS method for): `lon_0`,
    /// `x_0` and `y_0`, each 0 when absent, on a sphere of radius `a`.
    pub(crate) fn from_keys(unit: U, params: &Params, a: f64) -> Self {
        let key = |k: &str| params.key(k).unwrap_or(0.0);
        Spherical {
            unit,
            lon0: key("lon_0"),
            fe: key("x_0"),
            fn_: key("y_0"),
            a,
            ra: 1.0 / a,
        }
    }

    /// The radius forwards, where PROJ's set-up replaces the semi-major
    /// axis after it has taken 1/a for the inverse (`gs48`, `alsk`,
    /// `gs50`): the inverse keeps the CRS's own.
    pub(crate) fn forward_radius(&mut self, a: f64) {
        self.a = a;
    }

    /// The central meridian, where PROJ's set-up replaces `lon_0`.
    pub(crate) fn set_lon0(&mut self, lon0: f64) {
        self.lon0 = lon0;
    }

    /// With this central meridian and radius.
    pub(crate) fn with(unit: U, lon0: f64, params: &Params, a: f64) -> Result<Self, BuildError> {
        Ok(Spherical {
            unit,
            lon0,
            fe: params.get(param::FALSE_EASTING)?,
            fn_: params.get(param::FALSE_NORTHING)?,
            a,
            ra: 1.0 / a,
        })
    }
}

impl<U: Unit> Projection for Spherical<U> {
    fn forward(&self, lon: f64, lat: f64) -> Result<(f64, f64), CoordinateError> {
        let (x, y) = self.unit.fwd(adjlon(lon - self.lon0), lat)?;
        Ok((self.a * x + self.fe, self.a * y + self.fn_))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let (lam, phi) = self
            .unit
            .inv((x - self.fe) * self.ra, (y - self.fn_) * self.ra)?;
        Ok((lam + self.lon0, phi))
    }
}

const ONE_TOL: f64 = 1.000_000_000_000_01;

/// asin, with |v| slightly over 1 taken as 1 (PROJ's `aasin`).
pub(crate) fn aasin(v: f64) -> Result<f64, CoordinateError> {
    if v.abs() >= 1.0 {
        if v.abs() > ONE_TOL {
            return Err(CoordinateError::OutsideDomain);
        }
        return Ok(if v < 0.0 { -FRAC_PI_2 } else { FRAC_PI_2 });
    }
    Ok(v.asin())
}

/// acos, with |v| slightly over 1 taken as 1 (PROJ's `aacos`).
pub(crate) fn aacos(v: f64) -> Result<f64, CoordinateError> {
    if v.abs() >= 1.0 {
        if v.abs() > ONE_TOL {
            return Err(CoordinateError::OutsideDomain);
        }
        return Ok(if v < 0.0 { PI } else { 0.0 });
    }
    Ok(v.acos())
}

/// atan2, 0 at the origin (PROJ's `aatan2`).
pub(crate) fn aatan2(n: f64, d: f64) -> f64 {
    const ATOL: f64 = 1e-50;
    if n.abs() < ATOL && d.abs() < ATOL {
        0.0
    } else {
        n.atan2(d)
    }
}

/// √v, 0 for v ≤ 0 (PROJ's `asqrt`).
pub(crate) fn asqrt(v: f64) -> f64 {
    if v <= 0.0 { 0.0 } else { v.sqrt() }
}

/// The inverse of `fwd` by Newton's method with a numerical Jacobian,
/// from `(lam, phi)` (PROJ's `pj_generic_inverse_2d`).
pub(crate) fn generic_inverse(
    fwd: impl Fn(f64, f64) -> Result<(f64, f64), CoordinateError>,
    x: f64,
    y: f64,
    (mut lam, mut phi): (f64, f64),
    tolerance: f64,
) -> Result<(f64, f64), CoordinateError> {
    let (mut lam_x, mut lam_y, mut phi_x, mut phi_y) = (0.0, 0.0, 0.0, 0.0);
    for i in 0..15 {
        let (xa, ya) = fwd(lam, phi)?;
        let (dx, dy) = (xa - x, ya - y);
        if dx.abs() < tolerance && dy.abs() < tolerance {
            return Ok((lam, phi));
        }
        if i == 0 || dx.abs() > 1e-6 || dy.abs() > 1e-6 {
            // The Jacobian, only while far from the result.
            let d_lam = if lam > 0.0 { -1e-6 } else { 1e-6 };
            let (x2, y2) = fwd(lam + d_lam, phi)?;
            let (x_lam, y_lam) = ((x2 - xa) / d_lam, (y2 - ya) / d_lam);
            let d_phi = if phi > 0.0 { -1e-6 } else { 1e-6 };
            let (x2, y2) = fwd(lam, phi + d_phi)?;
            let (x_phi, y_phi) = ((x2 - xa) / d_phi, (y2 - ya) / d_phi);
            let det = x_lam * y_phi - x_phi * y_lam;
            if det != 0.0 {
                lam_x = y_phi / det;
                lam_y = -x_phi / det;
                phi_x = -y_lam / det;
                phi_y = x_lam / det;
            }
        }
        // Limit the corrections, against overshoots from a bad guess.
        lam -= (dx * lam_x + dy * lam_y).clamp(-0.3, 0.3);
        lam = lam.clamp(-PI, PI);
        phi -= (dx * phi_x + dy * phi_y).clamp(-0.3, 0.3);
        phi = phi.clamp(-FRAC_PI_2, FRAC_PI_2);
    }
    Err(CoordinateError::OutsideDomain)
}
