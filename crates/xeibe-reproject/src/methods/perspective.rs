//! Vertical Perspective (EPSG:9838) and Gnomonic, the perspective
//! azimuthal projections from a point above the surface and from its
//! centre.
//!
//! Ported from PROJ 9.8.1 `src/projections/nsper.cpp` and `gnom.cpp` (MIT,
//! see PROJ-NOTICE.md). PROJ maps Vertical Perspective to `nsper`, which is
//! spherical: the sphere of the semi-major axis, the ellipsoidal height of
//! the origin unused. Gnomonic is spherical on a sphere; on an ellipsoid
//! PROJ uses Karney's ellipsoidal gnomonic, through the geodesics of the
//! ellipsoid scaled to a semi-major axis of 1.

use std::f64::consts::FRAC_PI_2;

use super::sphere::Unit;
use super::{BuildError, Params, Projection, adjlon, param};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;
use crate::geodesic::{Flags, Geodesic, Mask};

const EPS10: f64 = 1e-10;
const OUTSIDE: CoordinateError = CoordinateError::OutsideDomain;
const DEG_TO_RAD: f64 = 0.017_453_292_519_943_295;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    NPole,
    SPole,
    Equatorial,
    Oblique,
}

fn mode(phi0: f64) -> Mode {
    if (phi0.abs() - FRAC_PI_2).abs() < EPS10 {
        if phi0 < 0.0 { Mode::SPole } else { Mode::NPole }
    } else if phi0.abs() < EPS10 {
        Mode::Equatorial
    } else {
        Mode::Oblique
    }
}

/// Vertical Perspective on the unit sphere (`nsper`, without tilt).
#[derive(Debug)]
pub(crate) struct NearSidedPerspective {
    phi0: f64,
    mode: Mode,
    sinph0: f64,
    cosph0: f64,
    /// The height of the viewpoint over the radius.
    pn1: f64,
    p: f64,
    rp: f64,
    pfact: f64,
}

impl NearSidedPerspective {
    /// The latitude of the origin and the viewpoint's height over a sphere
    /// of radius `a`.
    pub(crate) fn new(phi0: f64, height: f64, a: f64) -> Result<Self, BuildError> {
        let mode = mode(phi0);
        let (sinph0, cosph0) = if mode == Mode::Oblique {
            phi0.sin_cos()
        } else {
            (0.0, 0.0)
        };
        let pn1 = height / a;
        if pn1 <= 0.0 || pn1 > 1e10 {
            return Err(BuildError::Invalid("Invalid value for h".into()));
        }
        let p = 1.0 + pn1;
        let h = 1.0 / pn1;
        Ok(NearSidedPerspective {
            phi0,
            mode,
            sinph0,
            cosph0,
            pn1,
            p,
            rp: 1.0 / p,
            pfact: (p + 1.0) * h,
        })
    }
}

impl Unit for NearSidedPerspective {
    fn fwd(&self, lam: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let (sinphi, cosphi) = phi.sin_cos();
        let mut coslam = lam.cos();
        let y = match self.mode {
            Mode::Oblique => self.sinph0 * sinphi + self.cosph0 * cosphi * coslam,
            Mode::Equatorial => cosphi * coslam,
            Mode::SPole => -sinphi,
            Mode::NPole => sinphi,
        };
        if y < self.rp {
            return Err(OUTSIDE);
        }
        let mut y = self.pn1 / (self.p - y);
        let x = y * cosphi * lam.sin();
        match self.mode {
            Mode::Oblique => y *= self.cosph0 * sinphi - self.sinph0 * cosphi * coslam,
            Mode::Equatorial => y *= sinphi,
            Mode::NPole | Mode::SPole => {
                if self.mode == Mode::NPole {
                    coslam = -coslam;
                }
                y *= cosphi * coslam;
            }
        }
        Ok((x, y))
    }

    fn inv(&self, mut x: f64, mut y: f64) -> Result<(f64, f64), CoordinateError> {
        let rh = x.hypot(y);
        if rh.abs() <= EPS10 {
            return Ok((0.0, self.phi0));
        }
        let sinz = 1.0 - rh * rh * self.pfact;
        if sinz < 0.0 {
            return Err(OUTSIDE);
        }
        let sinz = (self.p - sinz.sqrt()) / (self.pn1 / rh + rh / self.pn1);
        let cosz = (1.0 - sinz * sinz).sqrt();
        let phi = match self.mode {
            Mode::Oblique => {
                let phi = (cosz * self.sinph0 + y * sinz * self.cosph0 / rh).asin();
                y = (cosz - self.sinph0 * phi.sin()) * rh;
                x *= sinz * self.cosph0;
                phi
            }
            Mode::Equatorial => {
                let phi = (y * sinz / rh).asin();
                y = cosz * rh;
                x *= sinz;
                phi
            }
            Mode::NPole => {
                y = -y;
                cosz.asin()
            }
            Mode::SPole => -cosz.asin(),
        };
        Ok((x.atan2(y), phi))
    }
}

/// Gnomonic, on a sphere or on an ellipsoid.
#[derive(Debug)]
pub(crate) struct Gnomonic {
    lon0: f64,
    phi0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
    mode: Mode,
    sinph0: f64,
    cosph0: f64,
    /// The ellipsoid's geodesics with a = 1, for the ellipsoidal form.
    geod: Option<Geodesic>,
}

impl Gnomonic {
    pub(crate) fn new(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        let phi0 = params.get(param::LAT_NATURAL_ORIGIN)?;
        let mode = mode(phi0);
        let (sinph0, cosph0) = if mode == Mode::Oblique {
            phi0.sin_cos()
        } else {
            (0.0, 0.0)
        };
        Ok(Gnomonic {
            lon0: params.get(param::LON_NATURAL_ORIGIN)?,
            phi0,
            fe: params.get(param::FALSE_EASTING)?,
            fn_: params.get(param::FALSE_NORTHING)?,
            a: ellipsoid.a,
            mode,
            sinph0,
            cosph0,
            geod: (ellipsoid.es != 0.0).then(|| Geodesic::new(1.0, ellipsoid.f)),
        })
    }

    fn forward_s(&self, lam: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let (sinphi, cosphi) = phi.sin_cos();
        let mut coslam = lam.cos();
        let y = match self.mode {
            Mode::Equatorial => cosphi * coslam,
            Mode::Oblique => self.sinph0 * sinphi + self.cosph0 * cosphi * coslam,
            Mode::SPole => -sinphi,
            Mode::NPole => sinphi,
        };
        if y <= EPS10 {
            return Err(OUTSIDE);
        }
        let mut y = 1.0 / y;
        let x = y * cosphi * lam.sin();
        match self.mode {
            Mode::Equatorial => y *= sinphi,
            Mode::Oblique => y *= self.cosph0 * sinphi - self.sinph0 * cosphi * coslam,
            Mode::NPole | Mode::SPole => {
                if self.mode == Mode::NPole {
                    coslam = -coslam;
                }
                y *= cosphi * coslam;
            }
        }
        Ok((x, y))
    }

    fn inverse_s(&self, mut x: f64, mut y: f64) -> (f64, f64) {
        let rh = x.hypot(y);
        let mut phi = rh.atan();
        let sinz = phi.sin();
        let cosz = (1.0 - sinz * sinz).sqrt();
        if rh.abs() <= EPS10 {
            return (0.0, self.phi0);
        }
        let clamp = |p: f64| {
            if p.abs() >= 1.0 {
                if p > 0.0 { FRAC_PI_2 } else { -FRAC_PI_2 }
            } else {
                p.asin()
            }
        };
        match self.mode {
            Mode::Oblique => {
                phi = clamp(cosz * self.sinph0 + y * sinz * self.cosph0 / rh);
                y = (cosz - self.sinph0 * phi.sin()) * rh;
                x *= sinz * self.cosph0;
            }
            Mode::Equatorial => {
                phi = clamp(y * sinz / rh);
                y = cosz * rh;
                x *= sinz;
            }
            Mode::SPole => phi -= FRAC_PI_2,
            Mode::NPole => {
                phi = FRAC_PI_2 - phi;
                y = -y;
            }
        }
        (x.atan2(y), phi)
    }

    fn forward_e(
        &self,
        geod: &Geodesic,
        lam: f64,
        phi: f64,
    ) -> Result<(f64, f64), CoordinateError> {
        let r = geod.gen_inverse(
            self.phi0 / DEG_TO_RAD,
            0.0,
            phi / DEG_TO_RAD,
            lam / DEG_TO_RAD,
            Mask::REDUCED_LENGTH | Mask::GEODESIC_SCALE,
        );
        if r.M12 <= 0.0 {
            return Err(OUTSIDE);
        }
        let rho = r.m12 / r.M12;
        let azi0 = r.azi1 * DEG_TO_RAD;
        Ok((rho * azi0.sin(), rho * azi0.cos()))
    }

    // The negated comparison is how NaN ends the iteration, as in PROJ.
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    fn inverse_e(&self, geod: &Geodesic, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let eps = 0.01 * f64::EPSILON.sqrt();
        // Clockwise from north.
        let azi0 = x.atan2(y) / DEG_TO_RAD;
        let mut rho = x.hypot(y);
        let mut s = rho.atan();
        let little = rho <= 1.0;
        if !little {
            rho = 1.0 / rho;
        }
        let line = geod.line(
            self.phi0 / DEG_TO_RAD,
            0.0,
            azi0,
            Mask::LATITUDE
                | Mask::LONGITUDE
                | Mask::DISTANCE_IN
                | Mask::REDUCED_LENGTH
                | Mask::GEODESIC_SCALE,
        );
        let mut trip = false;
        let (mut lat1, mut lon1) = (0.0, 0.0);
        for _ in 0..10 {
            let p = line.gen_position(
                Flags::default(),
                s,
                Mask::LATITUDE | Mask::LONGITUDE | Mask::REDUCED_LENGTH | Mask::GEODESIC_SCALE,
            );
            (lat1, lon1) = (p.lat2, p.lon2);
            if trip {
                break;
            }
            // Little: solve rho(s) = rho with drho/ds = 1/M², else
            // 1/rho(s) = 1/rho with d(1/rho)/ds = -1/m².
            let ds = if little {
                (p.m12 - rho * p.M12) * p.M12
            } else {
                (rho * p.m12 - p.M12) * p.m12
            };
            s -= ds;
            // Written backwards so that NaN ends the iteration.
            if !(ds.abs() >= eps) {
                trip = true;
            }
        }
        if !trip {
            return Err(OUTSIDE);
        }
        Ok((lon1 * DEG_TO_RAD, lat1 * DEG_TO_RAD))
    }
}

impl Projection for Gnomonic {
    fn forward(&self, lon: f64, lat: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = adjlon(lon - self.lon0);
        let (x, y) = match &self.geod {
            None => self.forward_s(lam, lat)?,
            Some(geod) => self.forward_e(geod, lam, lat)?,
        };
        Ok((self.a * x + self.fe, self.a * y + self.fn_))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let (x, y) = ((x - self.fe) / self.a, (y - self.fn_) / self.a);
        let (lam, phi) = match &self.geod {
            None => self.inverse_s(x, y),
            Some(geod) => self.inverse_e(geod, x, y)?,
        };
        Ok((lam + self.lon0, phi))
    }
}
