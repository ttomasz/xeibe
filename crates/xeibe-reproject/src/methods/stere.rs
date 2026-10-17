//! Polar Stereographic variants A (EPSG:9810) and B (EPSG:9829), and the
//! general oblique and equatorial Stereographic they belong to.
//!
//! Ported from PROJ 9.8.1 `src/projections/stere.cpp` (MIT, see
//! PROJ-NOTICE.md). PROJ maps variant A to `stere` with `lat_0` = ±90° and a
//! scale factor, and variant B to `stere` with `lat_ts` and `lat_0` = ±90°
//! chosen by the sign of the standard parallel. Variant C (EPSG:9830) has no
//! PROJ mapping and is not supported.

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4};

use super::common::tsfn;
use super::{BuildError, Params, Projection, adjlon, param};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

const EPS10: f64 = 1e-10;
const TOL: f64 = 1e-8;
const NITER: usize = 8;
const CONV: f64 = 1e-10;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    SPole,
    NPole,
    Oblique,
    Equatorial,
}

#[derive(Debug)]
pub(crate) struct Stereographic {
    lon0: f64,
    phi0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
    e: f64,
    spherical: bool,
    mode: Mode,
    akm1: f64,
    sin_x1: f64,
    cos_x1: f64,
}

fn ssfn(phit: f64, sinphi: f64, eccen: f64) -> f64 {
    let sinphi = sinphi * eccen;
    (0.5 * (FRAC_PI_2 + phit)).tan() * ((1.0 - sinphi) / (1.0 + sinphi)).powf(0.5 * eccen)
}

impl Stereographic {
    /// Variant A: latitude and longitude of natural origin, scale factor.
    pub(crate) fn variant_a(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        Self::setup(
            ellipsoid,
            params.get(param::LAT_NATURAL_ORIGIN)?,
            params.get(param::LON_NATURAL_ORIGIN)?,
            params.get(param::SCALE_NATURAL_ORIGIN)?,
            FRAC_PI_2,
            params.get(param::FALSE_EASTING)?,
            params.get(param::FALSE_NORTHING)?,
        )
    }

    /// Variant B: latitude of standard parallel (true scale), longitude of
    /// origin.
    pub(crate) fn variant_b(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        let lat_ts = params.get(param::LAT_STD_PARALLEL)?;
        Self::setup(
            ellipsoid,
            if lat_ts >= 0.0 { FRAC_PI_2 } else { -FRAC_PI_2 },
            params.get(param::LON_OF_ORIGIN)?,
            1.0,
            lat_ts,
            params.get(param::FALSE_EASTING)?,
            params.get(param::FALSE_NORTHING)?,
        )
    }

    fn setup(
        ellipsoid: &Ellipsoid,
        phi0: f64,
        lon0: f64,
        k0: f64,
        phits: f64,
        fe: f64,
        fn_: f64,
    ) -> Result<Self, BuildError> {
        let t = phi0.abs();
        let mode = if (t - FRAC_PI_2).abs() < EPS10 {
            if phi0 < 0.0 { Mode::SPole } else { Mode::NPole }
        } else if t > EPS10 {
            Mode::Oblique
        } else {
            Mode::Equatorial
        };
        let phits = phits.abs();
        let (e, es) = (ellipsoid.e, ellipsoid.es);
        let akm1;
        let (mut sin_x1, mut cos_x1) = (0.0, 0.0);
        if es != 0.0 {
            match mode {
                Mode::NPole | Mode::SPole => {
                    akm1 = if (phits - FRAC_PI_2).abs() < EPS10 {
                        2.0 * k0 / ((1.0 + e).powf(1.0 + e) * (1.0 - e).powf(1.0 - e)).sqrt()
                    } else {
                        let t = phits.sin();
                        let akm1 = phits.cos() / tsfn(phits, t, e);
                        let t = t * e;
                        akm1 / (1.0 - t * t).sqrt()
                    };
                }
                Mode::Equatorial | Mode::Oblique => {
                    let t = phi0.sin();
                    let x = 2.0 * ssfn(phi0, t, e).atan() - FRAC_PI_2;
                    let t = t * e;
                    akm1 = 2.0 * k0 * phi0.cos() / (1.0 - t * t).sqrt();
                    (sin_x1, cos_x1) = x.sin_cos();
                }
            }
        } else {
            match mode {
                Mode::Oblique => {
                    (sin_x1, cos_x1) = phi0.sin_cos();
                    akm1 = 2.0 * k0;
                }
                Mode::Equatorial => akm1 = 2.0 * k0,
                Mode::SPole | Mode::NPole => {
                    akm1 = if (phits - FRAC_PI_2).abs() >= EPS10 {
                        phits.cos() / (FRAC_PI_4 - 0.5 * phits).tan()
                    } else {
                        2.0 * k0
                    };
                }
            }
        }
        Ok(Stereographic {
            lon0,
            phi0,
            fe,
            fn_,
            a: ellipsoid.a,
            e,
            spherical: es == 0.0,
            mode,
            akm1,
            sin_x1,
            cos_x1,
        })
    }

    fn forward_e(&self, lam: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let (sinlam, mut coslam) = lam.sin_cos();
        let mut phi = phi;
        let mut sinphi = phi.sin();
        let (mut sin_x, mut cos_x) = (0.0, 0.0);
        if matches!(self.mode, Mode::Oblique | Mode::Equatorial) {
            let x = 2.0 * ssfn(phi, sinphi, self.e).atan() - FRAC_PI_2;
            (sin_x, cos_x) = x.sin_cos();
        }
        let (mut x, y);
        match self.mode {
            Mode::Oblique => {
                let denom =
                    self.cos_x1 * (1.0 + self.sin_x1 * sin_x + self.cos_x1 * cos_x * coslam);
                if denom == 0.0 {
                    return Err(CoordinateError::OutsideDomain);
                }
                let aa = self.akm1 / denom;
                y = aa * (self.cos_x1 * sin_x - self.sin_x1 * cos_x * coslam);
                x = aa * cos_x;
            }
            Mode::Equatorial => {
                // Avoid the zero division.
                let mut aa = 0.0;
                if 1.0 + cos_x * coslam == 0.0 {
                    y = f64::INFINITY;
                } else {
                    aa = self.akm1 / (1.0 + cos_x * coslam);
                    y = aa * sin_x;
                }
                x = aa * cos_x;
            }
            Mode::SPole | Mode::NPole => {
                if self.mode == Mode::SPole {
                    phi = -phi;
                    coslam = -coslam;
                    sinphi = -sinphi;
                }
                x = if (phi - FRAC_PI_2).abs() < 1e-15 {
                    0.0
                } else {
                    self.akm1 * tsfn(phi, sinphi, self.e)
                };
                y = -x * coslam;
            }
        }
        x *= sinlam;
        Ok((x, y))
    }

    fn forward_s(&self, lam: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let (sinphi, cosphi) = phi.sin_cos();
        let (sinlam, mut coslam) = lam.sin_cos();
        match self.mode {
            Mode::Equatorial | Mode::Oblique => {
                let mut y = if self.mode == Mode::Equatorial {
                    1.0 + cosphi * coslam
                } else {
                    1.0 + self.sin_x1 * sinphi + self.cos_x1 * cosphi * coslam
                };
                if y <= EPS10 {
                    return Err(CoordinateError::OutsideDomain);
                }
                y = self.akm1 / y;
                let x = y * cosphi * sinlam;
                y *= if self.mode == Mode::Equatorial {
                    sinphi
                } else {
                    self.cos_x1 * sinphi - self.sin_x1 * cosphi * coslam
                };
                Ok((x, y))
            }
            Mode::NPole | Mode::SPole => {
                let mut phi = phi;
                if self.mode == Mode::NPole {
                    coslam = -coslam;
                    phi = -phi;
                }
                if (phi - FRAC_PI_2).abs() < TOL {
                    return Err(CoordinateError::OutsideDomain);
                }
                let y = self.akm1 * (FRAC_PI_4 + 0.5 * phi).tan();
                Ok((sinlam * y, y * coslam))
            }
        }
    }

    fn inverse_e(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let rho = x.hypot(y);
        let (mut x, mut y) = (x, y);
        let (mut tp, mut phi_l, halfe, halfpi);
        match self.mode {
            Mode::Oblique | Mode::Equatorial => {
                tp = 2.0 * (rho * self.cos_x1).atan2(self.akm1);
                let (sinphi, cosphi) = tp.sin_cos();
                phi_l = if rho == 0.0 {
                    (cosphi * self.sin_x1).asin()
                } else {
                    (cosphi * self.sin_x1 + (y * sinphi * self.cos_x1 / rho)).asin()
                };
                tp = (0.5 * (FRAC_PI_2 + phi_l)).tan();
                x *= sinphi;
                y = rho * self.cos_x1 * cosphi - y * self.sin_x1 * sinphi;
                halfpi = FRAC_PI_2;
                halfe = 0.5 * self.e;
            }
            Mode::NPole | Mode::SPole => {
                if self.mode == Mode::NPole {
                    y = -y;
                }
                tp = -rho / self.akm1;
                phi_l = FRAC_PI_2 - 2.0 * tp.atan();
                halfpi = -FRAC_PI_2;
                halfe = -0.5 * self.e;
            }
        }
        for _ in 0..NITER {
            let sinphi = self.e * phi_l.sin();
            let phi = 2.0 * (tp * ((1.0 + sinphi) / (1.0 - sinphi)).powf(halfe)).atan() - halfpi;
            if (phi_l - phi).abs() < CONV {
                let phi = if self.mode == Mode::SPole { -phi } else { phi };
                let lam = if x == 0.0 && y == 0.0 {
                    0.0
                } else {
                    x.atan2(y)
                };
                return Ok((lam, phi));
            }
            phi_l = phi;
        }
        Err(CoordinateError::OutsideDomain)
    }

    fn inverse_s(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let rh = x.hypot(y);
        let mut c = 2.0 * (rh / self.akm1).atan();
        let (sinc, cosc) = c.sin_cos();
        let mut lam = 0.0;
        let mut y = y;
        let phi = match self.mode {
            Mode::Equatorial => {
                let p = if rh.abs() <= EPS10 {
                    0.0
                } else {
                    (y * sinc / rh).asin()
                };
                if cosc != 0.0 || x != 0.0 {
                    lam = (x * sinc).atan2(cosc * rh);
                }
                p
            }
            Mode::Oblique => {
                let p = if rh.abs() <= EPS10 {
                    self.phi0
                } else {
                    (cosc * self.sin_x1 + y * sinc * self.cos_x1 / rh).asin()
                };
                c = cosc - self.sin_x1 * p.sin();
                if c != 0.0 || x != 0.0 {
                    lam = (x * sinc * self.cos_x1).atan2(c * rh);
                }
                p
            }
            Mode::NPole | Mode::SPole => {
                if self.mode == Mode::NPole {
                    y = -y;
                }
                let p = if rh.abs() <= EPS10 {
                    self.phi0
                } else {
                    (if self.mode == Mode::SPole {
                        -cosc
                    } else {
                        cosc
                    })
                    .asin()
                };
                lam = if x == 0.0 && y == 0.0 {
                    0.0
                } else {
                    x.atan2(y)
                };
                p
            }
        };
        Ok((lam, phi))
    }
}

impl Projection for Stereographic {
    fn forward(&self, lon: f64, lat: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = adjlon(lon - self.lon0);
        let (x, y) = if self.spherical {
            self.forward_s(lam, lat)?
        } else {
            self.forward_e(lam, lat)?
        };
        Ok((self.a * x + self.fe, self.a * y + self.fn_))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let (x, y) = ((x - self.fe) / self.a, (y - self.fn_) / self.a);
        let (lam, phi) = if self.spherical {
            self.inverse_s(x, y)?
        } else {
            self.inverse_e(x, y)?
        };
        Ok((lam + self.lon0, phi))
    }
}
