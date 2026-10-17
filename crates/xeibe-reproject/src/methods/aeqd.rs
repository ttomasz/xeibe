//! Azimuthal Equidistant (EPSG:1125), Modified Azimuthal Equidistant
//! (EPSG:9832) and the Guam Projection (EPSG:9831).
//!
//! Ported from PROJ 9.8.1 `src/projections/aeqd.cpp` (MIT, see
//! PROJ-NOTICE.md). PROJ maps EPSG:1125 and EPSG:9832 to the same `aeqd`: the
//! modified method (EPSG's series approximation) is computed exactly, as the
//! distance and azimuth of a geodesic (see [`super::geodesic`]), with series
//! for the polar cases. EPSG:9831 is `aeqd` with `guam`, the Guam
//! approximation.

use std::f64::consts::{FRAC_PI_2, PI};

use super::auxlat::{self, AuxLat, ORDER};
use super::geodesic::Geodesic;
use super::{BuildError, Params, Projection, adjlon, param};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

const EPS10: f64 = 1e-10;
const TOL: f64 = 1e-14;
const DEG_TO_RAD: f64 = 0.017453292519943295;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    NPole,
    SPole,
    Equatorial,
    Oblique,
}

#[derive(Debug)]
pub(crate) struct AzimuthalEquidistant {
    lon0: f64,
    phi0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
    e: f64,
    es: f64,
    mode: Mode,
    sinph0: f64,
    cosph0: f64,
    kind: Kind,
}

#[derive(Debug)]
enum Kind {
    Spherical {
        geod: Geodesic,
    },
    Exact {
        meridian: Meridian,
        geod: Geodesic,
        mp: f64,
    },
    Guam {
        meridian: Meridian,
        m1: f64,
    },
}

/// Meridian arc on the ellipsoid in units of a (PROJ's `pj_mlfn` family).
#[derive(Debug)]
struct Meridian {
    radius: f64,
    to_rectifying: [f64; ORDER],
    to_geographic: [f64; ORDER],
}

impl Meridian {
    fn new(n: f64) -> Self {
        Meridian {
            radius: auxlat::rectifying_radius(n),
            to_rectifying: auxlat::coeffs(n, AuxLat::Geographic, AuxLat::Rectifying),
            to_geographic: auxlat::coeffs(n, AuxLat::Rectifying, AuxLat::Geographic),
        }
    }

    fn mlfn(&self, phi: f64, sinphi: f64, cosphi: f64) -> f64 {
        self.radius * auxlat::convert_sc(phi, sinphi, cosphi, &self.to_rectifying)
    }

    fn inv_mlfn(&self, mu: f64) -> f64 {
        auxlat::convert(mu / self.radius, &self.to_geographic)
    }
}

impl AzimuthalEquidistant {
    pub(crate) fn exact(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        Self::new(params, ellipsoid, false)
    }

    pub(crate) fn guam(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        Self::new(params, ellipsoid, true)
    }

    fn new(params: &Params, ellipsoid: &Ellipsoid, guam: bool) -> Result<Self, BuildError> {
        let phi0 = params.get(param::LAT_NATURAL_ORIGIN)?;
        let lon0 = params.get(param::LON_NATURAL_ORIGIN)?;
        let fe = params.get(param::FALSE_EASTING)?;
        let fn_ = params.get(param::FALSE_NORTHING)?;
        let (mode, sinph0, cosph0) = if (phi0.abs() - FRAC_PI_2).abs() < EPS10 {
            (
                if phi0 < 0.0 { Mode::SPole } else { Mode::NPole },
                if phi0 < 0.0 { -1.0 } else { 1.0 },
                0.0,
            )
        } else if phi0.abs() < EPS10 {
            (Mode::Equatorial, 0.0, 1.0)
        } else {
            let (s, c) = phi0.sin_cos();
            (Mode::Oblique, s, c)
        };
        let geod = Geodesic::new(1.0, ellipsoid.f);
        let kind = if ellipsoid.es == 0.0 {
            Kind::Spherical { geod }
        } else {
            let meridian = Meridian::new(ellipsoid.n);
            if guam {
                Kind::Guam {
                    m1: meridian.mlfn(phi0, sinph0, cosph0),
                    meridian,
                }
            } else {
                let mp = match mode {
                    Mode::NPole => meridian.mlfn(FRAC_PI_2, 1.0, 0.0),
                    Mode::SPole => meridian.mlfn(-FRAC_PI_2, -1.0, 0.0),
                    _ => 0.0,
                };
                Kind::Exact { meridian, geod, mp }
            }
        };
        Ok(AzimuthalEquidistant {
            lon0,
            phi0,
            fe,
            fn_,
            a: ellipsoid.a,
            e: ellipsoid.e,
            es: ellipsoid.es,
            mode,
            sinph0,
            cosph0,
            kind,
        })
    }

    /// The geodesic from the origin: easting and northing in units of a.
    fn forward_geodesic(&self, geod: &Geodesic, lam: f64, phi: f64) -> (f64, f64) {
        if lam.abs() < EPS10 && (phi - self.phi0).abs() < EPS10 {
            return (0.0, 0.0);
        }
        let (s12, azi1) = geod.inverse(
            self.phi0 / DEG_TO_RAD,
            0.0,
            phi / DEG_TO_RAD,
            lam / DEG_TO_RAD,
        );
        let azi1 = azi1 * DEG_TO_RAD;
        (s12 * azi1.sin(), s12 * azi1.cos())
    }

    fn forward_sphere(
        &self,
        geod: &Geodesic,
        lam: f64,
        phi: f64,
    ) -> Result<(f64, f64), CoordinateError> {
        match self.mode {
            Mode::Equatorial | Mode::Oblique => {
                let (sinphi, cosphi) = phi.sin_cos();
                let (sinlam, coslam) = lam.sin_cos();
                let cosphi_x_coslam = cosphi * coslam;
                let c = if self.mode == Mode::Equatorial {
                    cosphi_x_coslam
                } else {
                    self.sinph0 * sinphi + self.cosph0 * cosphi_x_coslam
                };
                if (c.abs() - 1.0).abs() < TOL {
                    if c < 0.0 {
                        return Err(CoordinateError::OutsideDomain);
                    }
                    return Ok(self.forward_geodesic(geod, lam, phi));
                }
                let mut y = c.acos();
                y /= y.sin();
                let x = y * cosphi * sinlam;
                y *= if self.mode == Mode::Equatorial {
                    sinphi
                } else {
                    self.cosph0 * sinphi - self.sinph0 * cosphi_x_coslam
                };
                Ok((x, y))
            }
            Mode::NPole | Mode::SPole => {
                let (sinlam, mut coslam) = lam.sin_cos();
                let mut phi = phi;
                if self.mode == Mode::NPole {
                    phi = -phi;
                    coslam = -coslam;
                }
                if (phi - FRAC_PI_2).abs() < EPS10 {
                    return Err(CoordinateError::OutsideDomain);
                }
                let y = FRAC_PI_2 + phi;
                Ok((y * sinlam, y * coslam))
            }
        }
    }

    fn inverse_sphere(&self, x0: f64, y0: f64) -> Result<(f64, f64), CoordinateError> {
        let mut c_rh = x0.hypot(y0);
        if c_rh > PI {
            if c_rh - EPS10 > PI {
                return Err(CoordinateError::OutsideDomain);
            }
            c_rh = PI;
        } else if c_rh < EPS10 {
            return Ok((0.0, self.phi0));
        }
        match self.mode {
            Mode::Equatorial | Mode::Oblique => {
                let (sinc, cosc) = c_rh.sin_cos();
                let (phi, x, y);
                if self.mode == Mode::Equatorial {
                    phi = aasin(y0 * sinc / c_rh)?;
                    x = x0 * sinc;
                    y = cosc * c_rh;
                } else {
                    phi = aasin(cosc * self.sinph0 + y0 * sinc * self.cosph0 / c_rh)?;
                    y = (cosc - self.sinph0 * phi.sin()) * c_rh;
                    x = x0 * (sinc * self.cosph0);
                }
                let lam = if y == 0.0 { 0.0 } else { x.atan2(y) };
                Ok((lam, phi))
            }
            Mode::NPole => Ok((x0.atan2(-y0), FRAC_PI_2 - c_rh)),
            Mode::SPole => Ok((x0.atan2(y0), c_rh - FRAC_PI_2)),
        }
    }
}

/// PROJ's `aasin`: arcsine, clamped at ±1 within rounding error.
fn aasin(v: f64) -> Result<f64, CoordinateError> {
    if v.abs() >= 1.0 {
        if v.abs() > 1.000_000_000_000_01 {
            return Err(CoordinateError::OutsideDomain);
        }
        Ok(FRAC_PI_2.copysign(v))
    } else {
        Ok(v.asin())
    }
}

impl Projection for AzimuthalEquidistant {
    fn forward(&self, lon: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = adjlon(lon - self.lon0);
        let (x, y) = match &self.kind {
            Kind::Spherical { geod } => self.forward_sphere(geod, lam, phi)?,
            Kind::Guam { meridian, m1 } => {
                let (sinphi, cosphi) = phi.sin_cos();
                let t = 1.0 / (1.0 - self.es * sinphi * sinphi).sqrt();
                (
                    lam * cosphi * t,
                    meridian.mlfn(phi, sinphi, cosphi) - m1 + 0.5 * lam * lam * cosphi * sinphi * t,
                )
            }
            Kind::Exact { meridian, geod, mp } => match self.mode {
                Mode::NPole | Mode::SPole => {
                    let coslam = if self.mode == Mode::NPole {
                        -lam.cos()
                    } else {
                        lam.cos()
                    };
                    let (sinphi, cosphi) = phi.sin_cos();
                    let rho = (mp - meridian.mlfn(phi, sinphi, cosphi)).abs();
                    (rho * lam.sin(), rho * coslam)
                }
                Mode::Equatorial | Mode::Oblique => self.forward_geodesic(geod, lam, phi),
            },
        };
        Ok((self.a * x + self.fe, self.a * y + self.fn_))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let (x, y) = ((x - self.fe) / self.a, (y - self.fn_) / self.a);
        let (lam, phi) = match &self.kind {
            Kind::Spherical { .. } => self.inverse_sphere(x, y)?,
            Kind::Guam { meridian, m1 } => {
                let x2 = 0.5 * x * x;
                let mut phi = self.phi0;
                let mut t = 0.0;
                for _ in 0..3 {
                    t = self.e * phi.sin();
                    t = (1.0 - t * t).sqrt();
                    phi = meridian.inv_mlfn(m1 + y - x2 * phi.tan() * t);
                }
                (x * t / phi.cos(), phi)
            }
            Kind::Exact { meridian, geod, mp } => {
                let s12 = x.hypot(y);
                if s12 < EPS10 {
                    (0.0, self.phi0)
                } else if matches!(self.mode, Mode::Oblique | Mode::Equatorial) {
                    let azi1 = x.atan2(y) / DEG_TO_RAD;
                    let (lat2, lon2) = geod.direct(self.phi0 / DEG_TO_RAD, azi1, s12);
                    (lon2 * DEG_TO_RAD, lat2 * DEG_TO_RAD)
                } else {
                    let north = self.mode == Mode::NPole;
                    let phi = meridian.inv_mlfn(if north { mp - s12 } else { mp + s12 });
                    (x.atan2(if north { -y } else { y }), phi)
                }
            }
        };
        Ok((lam + self.lon0, phi))
    }
}
