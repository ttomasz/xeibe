//! Local Orthographic (EPSG:1130).
//!
//! Ported from PROJ 9.8.1 `src/projections/ortho.cpp` (MIT, see
//! PROJ-NOTICE.md). PROJ maps EPSG:1130 to `ortho` with the azimuth as
//! `alpha` and the scale factor as `k_0`; the ellipsoidal formulas are those
//! of EPSG guidance note 7.2 §3.3.5, inverted by Newton's method.

use std::f64::consts::{FRAC_PI_2, PI};

use super::{BuildError, Params, Projection, adjlon, param};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

const EPS10: f64 = 1e-10;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    NPole,
    SPole,
    Equatorial,
    Oblique,
}

#[derive(Debug)]
pub(crate) struct LocalOrthographic {
    lon0: f64,
    phi0: f64,
    k0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
    es: f64,
    one_es: f64,
    /// a / b
    a_over_b: f64,
    spherical: bool,
    mode: Mode,
    sinph0: f64,
    cosph0: f64,
    nu0: f64,
    y_shift: f64,
    y_scale: f64,
    sinalpha: f64,
    cosalpha: f64,
}

impl LocalOrthographic {
    pub(crate) fn new(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        let phi0 = params.get(param::LAT_PROJECTION_CENTRE)?;
        let lon0 = params.get(param::LON_PROJECTION_CENTRE)?;
        let alpha = params.get(param::AZIMUTH_PROJECTION_CENTRE)?;
        let k0 = params.get(param::SCALE_PROJECTION_CENTRE)?;
        let fe = params.get(param::EASTING_PROJECTION_CENTRE)?;
        let fn_ = params.get(param::NORTHING_PROJECTION_CENTRE)?;
        let (sinph0, cosph0) = phi0.sin_cos();
        let mode = if (phi0.abs() - FRAC_PI_2).abs() <= EPS10 {
            if phi0 < 0.0 { Mode::SPole } else { Mode::NPole }
        } else if phi0.abs() > EPS10 {
            Mode::Oblique
        } else {
            Mode::Equatorial
        };
        let es = ellipsoid.es;
        let spherical = es == 0.0;
        let (mut nu0, mut y_shift, mut y_scale) = (0.0, 0.0, 0.0);
        if !spherical {
            nu0 = 1.0 / (1.0 - es * sinph0 * sinph0).sqrt();
            y_shift = es * nu0 * sinph0 * cosph0;
            y_scale = 1.0 / (1.0 - es * cosph0 * cosph0).sqrt();
        }
        let (sinalpha, cosalpha) = alpha.sin_cos();
        Ok(LocalOrthographic {
            lon0,
            phi0,
            k0,
            fe,
            fn_,
            a: ellipsoid.a,
            es,
            one_es: 1.0 - es,
            a_over_b: ellipsoid.a / ellipsoid.b,
            spherical,
            mode,
            sinph0,
            cosph0,
            nu0,
            y_shift,
            y_scale,
            sinalpha,
            cosalpha,
        })
    }

    fn forward_s(&self, lam: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let cosphi = phi.cos();
        let coslam = lam.cos();
        let y = match self.mode {
            Mode::Equatorial => {
                if cosphi * coslam < -EPS10 {
                    return Err(CoordinateError::OutsideDomain);
                }
                phi.sin()
            }
            Mode::Oblique => {
                if self.sinph0 * phi.sin() + self.cosph0 * cosphi * coslam < -EPS10 {
                    return Err(CoordinateError::OutsideDomain);
                }
                self.cosph0 * phi.sin() - self.sinph0 * cosphi * coslam
            }
            Mode::NPole | Mode::SPole => {
                let coslam = if self.mode == Mode::NPole {
                    -coslam
                } else {
                    coslam
                };
                if (phi - self.phi0).abs() - EPS10 > FRAC_PI_2 {
                    return Err(CoordinateError::OutsideDomain);
                }
                cosphi * coslam
            }
        };
        Ok((cosphi * lam.sin(), y))
    }

    fn inverse_s(&self, xf: f64, yf: f64) -> Result<(f64, f64), CoordinateError> {
        let mut x = (self.cosalpha * xf + self.sinalpha * yf) / self.k0;
        let mut y = (-self.sinalpha * xf + self.cosalpha * yf) / self.k0;
        let rh = x.hypot(y);
        let mut sinc = rh;
        if sinc > 1.0 {
            if sinc - 1.0 > EPS10 {
                return Err(CoordinateError::OutsideDomain);
            }
            sinc = 1.0;
        }
        let cosc = (1.0 - sinc * sinc).sqrt();
        if rh.abs() <= EPS10 {
            return Ok((0.0, self.phi0));
        }
        let phi = match self.mode {
            Mode::NPole => {
                y = -y;
                sinc.acos()
            }
            Mode::SPole => -sinc.acos(),
            Mode::Equatorial | Mode::Oblique => {
                let p = if self.mode == Mode::Equatorial {
                    let p = y * sinc / rh;
                    x *= sinc;
                    y = cosc * rh;
                    p
                } else {
                    let p = cosc * self.sinph0 + y * sinc * self.cosph0 / rh;
                    y = (cosc - self.sinph0 * p) * rh;
                    x *= sinc * self.cosph0;
                    p
                };
                if p.abs() >= 1.0 {
                    if p < 0.0 { -FRAC_PI_2 } else { FRAC_PI_2 }
                } else {
                    p.asin()
                }
            }
        };
        let lam = if y == 0.0 && matches!(self.mode, Mode::Oblique | Mode::Equatorial) {
            if x == 0.0 {
                0.0
            } else if x < 0.0 {
                -FRAC_PI_2
            } else {
                FRAC_PI_2
            }
        } else {
            x.atan2(y)
        };
        Ok((lam, phi))
    }

    fn forward_e(&self, lam: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let (sinphi, cosphi) = phi.sin_cos();
        let (sinlam, coslam) = lam.sin_cos();
        if self.sinph0 * sinphi + self.cosph0 * cosphi * coslam < -EPS10 {
            return Err(CoordinateError::OutsideDomain);
        }
        let nu = 1.0 / (1.0 - self.es * sinphi * sinphi).sqrt();
        let xp = nu * cosphi * sinlam;
        let yp = nu * (sinphi * self.cosph0 - cosphi * self.sinph0 * coslam)
            + self.es * (self.nu0 * self.sinph0 - nu * sinphi) * self.cosph0;
        Ok((xp, yp))
    }

    fn inverse_e(&self, xf: f64, yf: f64) -> Result<(f64, f64), CoordinateError> {
        let x = (self.cosalpha * xf + self.sinalpha * yf) / self.k0;
        let y = (-self.sinalpha * xf + self.cosalpha * yf) / self.k0;
        let es = self.es;

        if matches!(self.mode, Mode::NPole | Mode::SPole) {
            let north = self.mode == Mode::NPole;
            let rh2 = x * x + y * y;
            let phi = if rh2 >= 1.0 - 1e-15 {
                if rh2 - 1.0 > EPS10 {
                    return Err(CoordinateError::OutsideDomain);
                }
                0.0
            } else {
                (rh2 * self.one_es / (1.0 - es * rh2)).sqrt().acos()
                    * if north { 1.0 } else { -1.0 }
            };
            return Ok((x.atan2(y * if north { -1.0 } else { 1.0 }), phi));
        }

        if self.mode == Mode::Equatorial {
            if x * x + (y * self.a_over_b).powi(2) > 1.0 + 1e-11 {
                return Err(CoordinateError::OutsideDomain);
            }
            let sinphi2 = if y == 0.0 {
                0.0
            } else {
                1.0 / (((1.0 - es) / y).powi(2) + es)
            };
            let sign = if y > 0.0 { 1.0 } else { -1.0 };
            if sinphi2 > 1.0 - 1e-11 {
                return Ok((0.0, FRAC_PI_2 * sign));
            }
            let phi = sinphi2.sqrt().asin() * sign;
            let sinlam = x * ((1.0 - es * sinphi2) / (1.0 - sinphi2)).sqrt();
            let lam = if sinlam.abs() - 1.0 > -1e-15 {
                FRAC_PI_2 * if x > 0.0 { 1.0 } else { -1.0 }
            } else {
                sinlam.asin()
            };
            return Ok((lam, phi));
        }

        let y_recentered = (y - self.y_shift) / self.y_scale;
        if x * x + y_recentered * y_recentered > 1.0 + 1e-11 {
            return Err(CoordinateError::OutsideDomain);
        }
        // EPSG suggests starting from the origin, which fails near the poles;
        // PROJ starts from the spherical solution (through the rotation
        // again, as it does).
        let (mut lam, mut phi) = self.inverse_s(x, y_recentered)?;
        for _ in 0..20 {
            let (sinphi, cosphi) = phi.sin_cos();
            let (sinlam, coslam) = lam.sin_cos();
            let one_minus_es_sinphi2 = 1.0 - es * sinphi * sinphi;
            let nu = 1.0 / one_minus_es_sinphi2.sqrt();
            let x_new = nu * cosphi * sinlam;
            let y_new = nu * (sinphi * self.cosph0 - cosphi * self.sinph0 * coslam)
                + es * (self.nu0 * self.sinph0 - nu * sinphi) * self.cosph0;
            let rho = (1.0 - es) * nu / one_minus_es_sinphi2;
            let j11 = -rho * sinphi * sinlam;
            let j12 = nu * cosphi * coslam;
            let j21 = rho * (cosphi * self.cosph0 + sinphi * self.sinph0 * coslam);
            let j22 = nu * self.sinph0 * cosphi * sinlam;
            let d = j11 * j22 - j12 * j21;
            let dx = x - x_new;
            let dy = y - y_new;
            let dphi = (j22 * dx - j12 * dy) / d;
            let dlam = (-j21 * dx + j11 * dy) / d;
            phi += dphi;
            if phi > FRAC_PI_2 {
                phi = FRAC_PI_2 - (phi - FRAC_PI_2);
                lam = adjlon(lam + PI);
            } else if phi < -FRAC_PI_2 {
                phi = -FRAC_PI_2 + (-FRAC_PI_2 - phi);
                lam = adjlon(lam + PI);
            }
            lam += dlam;
            if dphi.abs() < 1e-12 && dlam.abs() < 1e-12 {
                return Ok((lam, phi));
            }
        }
        Err(CoordinateError::OutsideDomain)
    }
}

impl Projection for LocalOrthographic {
    fn forward(&self, lon: f64, lat: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = adjlon(lon - self.lon0);
        let (xp, yp) = if self.spherical {
            self.forward_s(lam, lat)?
        } else {
            self.forward_e(lam, lat)?
        };
        let x = (xp * self.cosalpha - yp * self.sinalpha) * self.k0;
        let y = (xp * self.sinalpha + yp * self.cosalpha) * self.k0;
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
