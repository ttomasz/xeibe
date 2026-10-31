//! More of PROJ's pseudocylindrical projections, which have no CRS method:
//! the Putnins, Foucaut Sinusoidal, Nell, Nell-Hammer, Hatano, McBryde-
//! Thomas Flat-Pole Sine (No. 2) and Flat-Polar Parabolic, Collignon,
//! Fahey and Werenskiold I.
//!
//! Ported from PROJ 9.8.1 `src/projections/` (MIT, see PROJ-NOTICE.md):
//! `putp2.cpp`, `putp3.cpp` (`putp3`, `putp3p`), `putp4p.cpp` (`putp4p`,
//! `weren`), `putp5.cpp` (`putp5`, `putp5p`), `putp6.cpp` (`putp6`,
//! `putp6p`), `fouc_s.cpp`, `nell.cpp`, `nell_h.cpp`, `hatano.cpp`,
//! `mbt_fps.cpp`, `mbtfpp.cpp`, `collg.cpp` and `fahey.cpp`, on the unit
//! sphere.

// PROJ's constants, digit for digit.
#![allow(clippy::excessive_precision)]

use std::f64::consts::FRAC_PI_2;

use super::sphere::{Unit, aasin, asqrt};
use crate::error::CoordinateError;

const OUTSIDE: CoordinateError = CoordinateError::OutsideDomain;

#[derive(Debug)]
pub(crate) enum Pcyl {
    Putnins2,
    /// Putnins P3 and P3′.
    Putnins3 {
        a: f64,
    },
    /// Putnins P4′ and Werenskiold I.
    Putnins4p {
        cx: f64,
        cy: f64,
    },
    /// Putnins P5 and P5′.
    Putnins5 {
        a: f64,
        b: f64,
    },
    /// Putnins P6 and P6′.
    Putnins6 {
        cx: f64,
        cy: f64,
        a: f64,
        b: f64,
        d: f64,
    },
    FoucautSinusoidal {
        n: f64,
        n1: f64,
    },
    Nell,
    NellHammer,
    Hatano,
    FlatPoleSine2,
    FlatPolarParabolic,
    Collignon,
    Fahey,
}

impl Pcyl {
    /// The projection of a PROJ name, `None` for another.
    pub(crate) fn of(proj: &str, n: Option<f64>) -> Result<Option<Pcyl>, String> {
        const RPISQ: f64 = 0.1013211836;
        Ok(Some(match proj {
            "putp2" => Pcyl::Putnins2,
            "putp3" => Pcyl::Putnins3 { a: 4.0 * RPISQ },
            "putp3p" => Pcyl::Putnins3 { a: 2.0 * RPISQ },
            "putp4p" => Pcyl::Putnins4p {
                cx: 0.874038744,
                cy: 3.883251825,
            },
            "weren" => Pcyl::Putnins4p {
                cx: 1.0,
                cy: 4.442882938,
            },
            "putp5" => Pcyl::Putnins5 { a: 2.0, b: 1.0 },
            "putp5p" => Pcyl::Putnins5 { a: 1.5, b: 0.5 },
            "putp6" => Pcyl::Putnins6 {
                cx: 1.01346,
                cy: 0.91910,
                a: 4.0,
                b: 2.1471437182129378784,
                d: 2.0,
            },
            "putp6p" => Pcyl::Putnins6 {
                cx: 0.44329,
                cy: 0.80404,
                a: 6.0,
                b: 5.61125,
                d: 3.0,
            },
            "fouc_s" => {
                let n = n.unwrap_or(0.0);
                if !(0.0..=1.0).contains(&n) {
                    return Err("Invalid value for n: it should be in [0,1] range.".into());
                }
                Pcyl::FoucautSinusoidal { n, n1: 1.0 - n }
            }
            "nell" => Pcyl::Nell,
            "nell_h" => Pcyl::NellHammer,
            "hatano" => Pcyl::Hatano,
            "mbt_fps" => Pcyl::FlatPoleSine2,
            "mbtfpp" => Pcyl::FlatPolarParabolic,
            "collg" => Pcyl::Collignon,
            "fahey" => Pcyl::Fahey,
            _ => return Ok(None),
        }))
    }
}

/// Hatano's constants.
mod hatano {
    pub const CN: f64 = 2.67595;
    pub const CS: f64 = 2.43763;
    pub const RCN: f64 = 0.37369906014686373063;
    pub const RCS: f64 = 0.41023453108141924738;
    pub const FYCN: f64 = 1.75859;
    pub const FYCS: f64 = 1.93052;
    pub const RYCN: f64 = 0.56863737426006061674;
    pub const RYCS: f64 = 0.51799515156538134803;
    pub const FXC: f64 = 0.85;
    pub const RXC: f64 = 1.17647058823529411764;
}

/// asin, with values within `tol` over 1 taken as ±π/2.
fn asin_tol(v: f64, tol: f64) -> Result<f64, CoordinateError> {
    if v.abs() >= 1.0 {
        if v.abs() > tol {
            return Err(OUTSIDE);
        }
        Ok(if v < 0.0 { -FRAC_PI_2 } else { FRAC_PI_2 })
    } else {
        Ok(v.asin())
    }
}

impl Unit for Pcyl {
    fn fwd(&self, lam: f64, mut phi: f64) -> Result<(f64, f64), CoordinateError> {
        Ok(match *self {
            Pcyl::Putnins2 => {
                const C_X: f64 = 1.89490;
                const C_Y: f64 = 1.71848;
                const C_P: f64 = 0.6141848493043784;
                const PI_DIV_3: f64 = 1.0471975511965977;
                let p = C_P * phi.sin();
                let phi2 = phi * phi;
                phi *= 0.615709 + phi2 * (0.00909953 + phi2 * 0.0046292);
                let mut converged = false;
                for _ in 0..10 {
                    let (s, c) = phi.sin_cos();
                    let v = (phi + s * (c - 1.0) - p) / (1.0 + c * (c - 1.0) - s * s);
                    phi -= v;
                    if v.abs() < 1e-10 {
                        converged = true;
                        break;
                    }
                }
                if !converged {
                    phi = if phi < 0.0 { -PI_DIV_3 } else { PI_DIV_3 };
                }
                (C_X * lam * (phi.cos() - 0.5), C_Y * phi.sin())
            }
            Pcyl::Putnins3 { a } => {
                const C: f64 = 0.79788456;
                (C * lam * (1.0 - a * phi * phi), C * phi)
            }
            Pcyl::Putnins4p { cx, cy } => {
                let phi = aasin(0.883883476 * phi.sin())?;
                let x = cx * lam * phi.cos();
                let phi = phi * 0.333333333333333;
                (x / phi.cos(), cy * phi.sin())
            }
            Pcyl::Putnins5 { a, b } => {
                const C: f64 = 1.01346;
                const D: f64 = 1.2158542;
                (C * lam * (a - b * (1.0 + D * phi * phi).sqrt()), C * phi)
            }
            Pcyl::Putnins6 { cx, cy, a, b, d } => {
                const CON_POLE: f64 = 1.732050807568877;
                let p = b * phi.sin();
                phi *= 1.10265779;
                let mut converged = false;
                for _ in 0..10 {
                    let r = (1.0 + phi * phi).sqrt();
                    let v = ((a - r) * phi - (phi + r).ln() - p) / (a - 2.0 * r);
                    phi -= v;
                    if v.abs() < 1e-10 {
                        converged = true;
                        break;
                    }
                }
                let sqrt_1_plus_phi2 = if converged {
                    (1.0 + phi * phi).sqrt()
                } else {
                    phi = if p < 0.0 { -CON_POLE } else { CON_POLE };
                    2.0
                };
                (cx * lam * (d - sqrt_1_plus_phi2), cy * phi)
            }
            Pcyl::FoucautSinusoidal { n, n1 } => {
                let t = phi.cos();
                (lam * t / (n + n1 * t), n * phi + n1 * phi.sin())
            }
            Pcyl::Nell => {
                let k = 2.0 * phi.sin();
                let phi2 = phi * phi;
                phi *= 1.00371 + phi2 * (-0.0935382 + phi2 * -0.011412);
                for _ in 0..10 {
                    let v = (phi + phi.sin() - k) / (1.0 + phi.cos());
                    phi -= v;
                    if v.abs() < 1e-7 {
                        break;
                    }
                }
                (0.5 * lam * (1.0 + phi.cos()), phi)
            }
            Pcyl::NellHammer => (
                0.5 * lam * (1.0 + phi.cos()),
                2.0 * (phi - (0.5 * phi).tan()),
            ),
            Pcyl::Hatano => {
                use hatano::*;
                let c = phi.sin() * if phi < 0.0 { CS } else { CN };
                for _ in 0..20 {
                    let th1 = (phi + phi.sin() - c) / (1.0 + phi.cos());
                    phi -= th1;
                    if th1.abs() < 1e-7 {
                        break;
                    }
                }
                phi *= 0.5;
                (
                    FXC * lam * phi.cos(),
                    phi.sin() * if phi < 0.0 { FYCS } else { FYCN },
                )
            }
            Pcyl::FlatPoleSine2 => {
                const C1: f64 = 0.45503;
                const C2: f64 = 1.36509;
                const C3: f64 = 1.41546;
                const C_X: f64 = 0.22248;
                const C_Y: f64 = 1.44492;
                const C1_2: f64 = 0.33333333333333333333333333;
                let k = C3 * phi.sin();
                for _ in 0..10 {
                    let t = phi / C2;
                    let v = (C1 * t.sin() + phi.sin() - k) / (C1_2 * t.cos() + phi.cos());
                    phi -= v;
                    if v.abs() < 1e-7 {
                        break;
                    }
                }
                let t = phi / C2;
                (C_X * lam * (1.0 + 3.0 * phi.cos() / t.cos()), C_Y * t.sin())
            }
            Pcyl::FlatPolarParabolic => {
                const CSY: f64 = 0.95257934441568037152;
                const FXC: f64 = 0.92582009977255146156;
                const FYC: f64 = 3.40168025708304504493;
                const C23: f64 = 0.66666666666666666666;
                const C13: f64 = 0.33333333333333333333;
                let phi = (CSY * phi.sin()).asin();
                (
                    FXC * lam * (2.0 * (C23 * phi).cos() - 1.0),
                    FYC * (C13 * phi).sin(),
                )
            }
            Pcyl::Collignon => {
                const FXC: f64 = std::f64::consts::FRAC_2_SQRT_PI;
                const FYC: f64 = 1.77245385090551602729;
                let y = 1.0 - phi.sin();
                let y = if y <= 0.0 { 0.0 } else { y.sqrt() };
                (FXC * lam * y, FYC * (1.0 - y))
            }
            Pcyl::Fahey => {
                let x = (0.5 * phi).tan();
                (0.819152 * lam * asqrt(1.0 - x * x), 1.819152 * x)
            }
        })
    }

    fn inv(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        Ok(match *self {
            Pcyl::Putnins2 => {
                const C_X: f64 = 1.89490;
                const C_Y: f64 = 1.71848;
                const C_P: f64 = 0.6141848493043784;
                let phi = aasin(y / C_Y)?;
                let c = phi.cos();
                let lam = x / (C_X * (c - 0.5));
                (lam, aasin((phi + phi.sin() * (c - 1.0)) / C_P)?)
            }
            Pcyl::Putnins3 { a } => {
                const C: f64 = 0.79788456;
                let phi = y / C;
                (x / (C * (1.0 - a * phi * phi)), phi)
            }
            Pcyl::Putnins4p { cx, cy } => {
                let phi = aasin(y / cy)?;
                let lam = x * phi.cos() / cx;
                let phi = phi * 3.0;
                (lam / phi.cos(), aasin(1.13137085 * phi.sin())?)
            }
            Pcyl::Putnins5 { a, b } => {
                const C: f64 = 1.01346;
                const D: f64 = 1.2158542;
                let phi = y / C;
                (x / (C * (a - b * (1.0 + D * phi * phi).sqrt())), phi)
            }
            Pcyl::Putnins6 { cx, cy, a, b, d } => {
                let phi = y / cy;
                let r = (1.0 + phi * phi).sqrt();
                (
                    x / (cx * (d - r)),
                    aasin(((a - r) * phi - (phi + r).ln()) / b)?,
                )
            }
            Pcyl::FoucautSinusoidal { n, n1 } => {
                let phi = if n != 0.0 {
                    let mut phi = y;
                    let mut converged = false;
                    for _ in 0..10 {
                        let v = (n * phi + n1 * phi.sin() - y) / (n + n1 * phi.cos());
                        phi -= v;
                        if v.abs() < 1e-7 {
                            converged = true;
                            break;
                        }
                    }
                    if converged {
                        phi
                    } else if y < 0.0 {
                        -FRAC_PI_2
                    } else {
                        FRAC_PI_2
                    }
                } else {
                    aasin(y)?
                };
                let v = phi.cos();
                (x * (n + n1 * v) / v, phi)
            }
            Pcyl::Nell => (2.0 * x / (1.0 + y.cos()), aasin(0.5 * (y + y.sin()))?),
            Pcyl::NellHammer => {
                let p = 0.5 * y;
                let mut phi: f64 = 0.0;
                let mut converged = false;
                for _ in 0..9 {
                    let c = (0.5 * phi).cos();
                    let v = (phi - (phi / 2.0).tan() - p) / (1.0 - 0.5 / (c * c));
                    phi -= v;
                    if v.abs() < 1e-7 {
                        converged = true;
                        break;
                    }
                }
                if converged {
                    (2.0 * x / (1.0 + phi.cos()), phi)
                } else {
                    (2.0 * x, if p < 0.0 { -FRAC_PI_2 } else { FRAC_PI_2 })
                }
            }
            Pcyl::Hatano => {
                use hatano::*;
                const ONETOL: f64 = 1.000001;
                let mut th = asin_tol(y * if y < 0.0 { RYCS } else { RYCN }, ONETOL)?;
                let lam = RXC * x / th.cos();
                th += th;
                let phi = asin_tol((th + th.sin()) * if y < 0.0 { RCS } else { RCN }, ONETOL)?;
                (lam, phi)
            }
            Pcyl::FlatPoleSine2 => {
                const C1: f64 = 0.45503;
                const C2: f64 = 1.36509;
                const C3: f64 = 1.41546;
                const C_X: f64 = 0.22248;
                const C_Y: f64 = 1.44492;
                let t = aasin(y / C_Y)?;
                let phi = C2 * t;
                let lam = x / (C_X * (1.0 + 3.0 * phi.cos() / t.cos()));
                (lam, aasin((C1 * t.sin() + phi.sin()) / C3)?)
            }
            Pcyl::FlatPolarParabolic => {
                const CSY: f64 = 0.95257934441568037152;
                const FXC: f64 = 0.92582009977255146156;
                const FYC: f64 = 3.40168025708304504493;
                const C23: f64 = 0.66666666666666666666;
                const ONEEPS: f64 = 1.0000001;
                let phi = asin_tol(y / FYC, ONEEPS)? * 3.0;
                let lam = x / (FXC * (2.0 * (C23 * phi).cos() - 1.0));
                (lam, asin_tol(phi.sin() / CSY, ONEEPS)?)
            }
            Pcyl::Collignon => {
                const FXC: f64 = std::f64::consts::FRAC_2_SQRT_PI;
                const FYC: f64 = 1.77245385090551602729;
                let p = y / FYC - 1.0;
                let p = 1.0 - p * p;
                let phi = if p.abs() < 1.0 {
                    p.asin()
                } else if p.abs() > 1.0000001 {
                    return Err(OUTSIDE);
                } else if p < 0.0 {
                    -FRAC_PI_2
                } else {
                    FRAC_PI_2
                };
                let l = 1.0 - phi.sin();
                (if l <= 0.0 { 0.0 } else { x / (FXC * l.sqrt()) }, phi)
            }
            Pcyl::Fahey => {
                let y = y / 1.819152;
                let phi = 2.0 * y.atan();
                let y = 1.0 - y * y;
                (
                    if y.abs() < 1e-6 {
                        0.0
                    } else {
                        x / (0.819152 * y.sqrt())
                    },
                    phi,
                )
            }
        })
    }
}
