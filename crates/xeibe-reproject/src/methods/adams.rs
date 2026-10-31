//! Peirce Quincuncial, Guyou and Adams's: the world or a hemisphere in a
//! square, through the elliptic integral of the first kind.
//!
//! Ported from PROJ 9.8.1 `src/projections/adams.cpp` (MIT, see
//! PROJ-NOTICE.md). PROJ has no closed inverse: for Peirce Quincuncial's
//! square and diamond and for Adams World in a Square II it starts from a
//! guess and solves by Newton's method; the others have none.

use std::f64::consts::{FRAC_2_PI, FRAC_PI_2, FRAC_PI_4, PI};

use super::sphere::{Unit, aacos, aasin, generic_inverse};
use crate::error::CoordinateError;

const RSQRT2: f64 = std::f64::consts::FRAC_1_SQRT_2;
/// The complete elliptic integral of the first kind with m = 0.5, twice.
const SHD: f64 = 1.854_074_677_301_371_9 * 2.0;

/// Peirce Quincuncial's shapes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Shape {
    Square,
    Diamond,
    NHemisphere,
    SHemisphere,
    /// With its `scrollx`.
    Horizontal(f64),
    /// With its `scrolly`.
    Vertical(f64),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Adams {
    Guyou,
    Peirce(Shape),
    Hemisphere,
    WorldInASquare1,
    WorldInASquare2,
}

impl Adams {
    /// The projection of a PROJ name with its `shape`, `scrollx` and
    /// `scrolly`, `None` for another name.
    pub(crate) fn of(
        proj: &str,
        shape: Option<&str>,
        scroll: impl Fn(&str) -> Option<f64>,
    ) -> Result<Option<Adams>, String> {
        let scroll = |k: &str| match scroll(k) {
            Some(s) if !(-1.0..=1.0).contains(&s) => Err(format!(
                "Invalid value for {k}: |{k}| should between -1 and 1"
            )),
            s => Ok(s.unwrap_or(0.0)),
        };
        Ok(Some(match proj {
            "guyou" => Adams::Guyou,
            "adams_hemi" => Adams::Hemisphere,
            "adams_ws1" => Adams::WorldInASquare1,
            "adams_ws2" => Adams::WorldInASquare2,
            "peirce_q" => Adams::Peirce(match shape.unwrap_or("diamond") {
                "square" => Shape::Square,
                "diamond" => Shape::Diamond,
                "nhemisphere" => Shape::NHemisphere,
                "shemisphere" => Shape::SHemisphere,
                "horizontal" => Shape::Horizontal(scroll("scrollx")?),
                "vertical" => Shape::Vertical(scroll("scrolly")?),
                _ => return Err("peirce_q: invalid value for 'shape' parameter".into()),
            }),
            _ => return Ok(None),
        }))
    }
}

/// Turns x and y by 45°.
fn rotate(x: f64, y: f64) -> (f64, f64) {
    (RSQRT2 * (x - y), RSQRT2 * (x + y))
}

/// The elliptic integral of the first kind with k² = 0.5, by an even
/// Chebyshev series, to better than 1e-7 (`ell_int_5`).
// PROJ's coefficients, digit for digit.
#[allow(clippy::excessive_precision)]
fn ell_int_5(phi: f64) -> f64 {
    const C0: f64 = 2.19174570831038;
    const C: [f64; 7] = [
        -8.58691003636495e-07,
        2.02692115653689e-07,
        3.12960480765314e-05,
        5.30394739921063e-05,
        -0.0012804644680613,
        -0.00575574836830288,
        0.0914203033408211,
    ];
    let y = phi * FRAC_2_PI;
    let y = 2.0 * y * y - 1.0;
    let y2 = 2.0 * y;
    let (mut d1, mut d2) = (0.0, 0.0);
    for c in C {
        let temp = d1;
        d1 = y2 * d1 - d2 + c;
        d2 = temp;
    }
    phi * (y * d1 - d2 + 0.5 * C0)
}

const TOL: f64 = 1e-9;

impl Unit for Adams {
    fn fwd(&self, lam: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let (a, b, sm, sn) = match *self {
            Adams::Guyou => {
                if lam.abs() - TOL > FRAC_PI_2 {
                    return Err(CoordinateError::OutsideDomain);
                }
                if (phi.abs() - FRAC_PI_2).abs() < TOL {
                    return Ok((0.0, if phi < 0.0 { -1.85407 } else { 1.85407 }));
                }
                let sl = lam.sin();
                let (sp, cp) = phi.sin_cos();
                (
                    aacos((cp * sl - sp) * RSQRT2)?,
                    aacos((cp * sl + sp) * RSQRT2)?,
                    lam < 0.0,
                    phi < 0.0,
                )
            }
            Adams::Peirce(shape) => {
                if shape == Shape::NHemisphere && phi < -TOL {
                    return Err(CoordinateError::OutsideDomain);
                }
                if shape == Shape::SHemisphere && phi > -TOL {
                    return Err(CoordinateError::OutsideDomain);
                }
                let (sl, cl) = lam.sin_cos();
                let cp = phi.cos();
                (
                    aacos(cp * (sl + cl) * RSQRT2)?,
                    aacos(cp * (sl - cl) * RSQRT2)?,
                    sl < 0.0,
                    cl > 0.0,
                )
            }
            Adams::Hemisphere => {
                let sp = phi.sin();
                if lam.abs() - TOL > FRAC_PI_2 {
                    return Err(CoordinateError::OutsideDomain);
                }
                let a = phi.cos() * lam.sin();
                (aacos(a)?, FRAC_PI_2 - phi, sp + a < 0.0, sp - a < 0.0)
            }
            Adams::WorldInASquare1 => {
                let sp = (0.5 * phi).tan();
                let b = aasin(sp)?.cos() * (0.5 * lam).sin();
                (
                    aacos((b - sp) * RSQRT2)?,
                    aacos((b + sp) * RSQRT2)?,
                    lam < 0.0,
                    phi < 0.0,
                )
            }
            Adams::WorldInASquare2 => {
                let spp = (0.5 * phi).tan();
                let a = aasin(spp)?.cos() * (0.5 * lam).sin();
                (aacos(a)?, aacos(spp)?, spp + a < 0.0, spp - a < 0.0)
            }
        };
        let mut m = aasin((1.0 + (a + b).cos().min(0.0)).sqrt())?;
        if sm {
            m = -m;
        }
        let mut n = aasin((1.0 - (a - b).cos().max(0.0)).abs().sqrt())?;
        if sn {
            n = -n;
        }
        let (mut x, mut y) = (ell_int_5(m), ell_int_5(n));
        let shape = match *self {
            Adams::Peirce(shape) => shape,
            Adams::Hemisphere | Adams::WorldInASquare2 => return Ok(rotate(x, y)),
            _ => return Ok((x, y)),
        };
        // The southern hemisphere spun out to the quincunx's triangles.
        if matches!(shape, Shape::Square | Shape::Diamond) && phi < 0.0 {
            if lam < -0.75 * PI {
                y = SHD - y;
            }
            if (-0.75 * PI..-0.25 * PI).contains(&lam) {
                x = -SHD - x;
            }
            if (-0.25 * PI..0.25 * PI).contains(&lam) {
                y = -SHD - y;
            }
            if (0.25 * PI..0.75 * PI).contains(&lam) {
                x = SHD - x;
            }
            if lam >= 0.75 * PI {
                y = SHD - y;
            }
        }
        // The square is the diamond turned by 45°.
        if shape == Shape::Square {
            (x, y) = rotate(x, y);
        }
        // The rectangles spin the southern hemisphere out east or north,
        // their origin between the two, then scroll.
        if let Shape::Horizontal(scroll) = shape {
            if phi < 0.0 {
                x = SHD - x;
            }
            x -= SHD / 2.0;
            if scroll != 0.0 {
                x += scroll * (SHD / 2.0 * 2.0 * 2.0);
                if x >= SHD / 2.0 * 2.0 {
                    x -= SHD * 2.0;
                } else if x < -(SHD / 2.0 * 2.0) {
                    x += SHD * 2.0;
                }
            }
        }
        if let Shape::Vertical(scroll) = shape {
            if phi < 0.0 {
                y = SHD - y;
            }
            y -= SHD / 2.0;
            if scroll != 0.0 {
                y += scroll * (SHD / 2.0 * 2.0 * 2.0);
                if y >= SHD / 2.0 * 2.0 {
                    y -= SHD * 2.0;
                } else if y < -(SHD / 2.0 * 2.0) {
                    y += SHD * 2.0;
                }
            }
        }
        Ok((x, y))
    }

    fn inv(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let square = match *self {
            Adams::Peirce(Shape::Square) => true,
            Adams::Peirce(Shape::Diamond) => false,
            Adams::WorldInASquare2 => {
                let phi = (y / 2.62181347).clamp(-1.0, 1.0) * FRAC_PI_2;
                let lam = if phi.abs() >= FRAC_PI_2 {
                    0.0
                } else {
                    (x / 2.62205760 / phi.cos()).clamp(-1.0, 1.0) * PI
                };
                return generic_inverse(|l, p| self.fwd(l, p), x, y, (lam, phi), 1e-10);
            }
            // PROJ has no inverse.
            _ => return Err(CoordinateError::OutsideDomain),
        };
        // PROJ's starting points, found by trial.
        let mut phi = 0.0;
        let lam;
        if square {
            if x == 0.0 && y < 0.0 {
                lam = -FRAC_PI_4;
                if y.abs() < 2.622057580396 {
                    phi = FRAC_PI_4;
                }
            } else if x > 0.0 && y.abs() < 1e-7 {
                lam = FRAC_PI_4;
            } else if x < 0.0 && y.abs() < 1e-7 {
                lam = -3.0 * PI / 4.0;
                phi = FRAC_PI_2 / 2.622057574224 * x + FRAC_PI_2;
            } else if x.abs() < 1e-7 && y > 0.0 {
                lam = 3.0 * PI / 4.0;
            } else if x >= 0.0 && y <= 0.0 {
                if x == 0.0 && y == 0.0 {
                    return Ok((0.0, FRAC_PI_2));
                }
                lam = 0.0;
            } else if x >= 0.0 && y >= 0.0 {
                lam = FRAC_PI_2;
            } else if x <= 0.0 && y >= 0.0 {
                lam = if x.abs() < y.abs() {
                    PI * 0.9
                } else {
                    -PI * 0.9
                };
            } else {
                lam = -FRAC_PI_2;
            }
        } else {
            if x >= 0.0 && y <= 0.0 {
                if x > 0.0 && y == 0.0 {
                    lam = FRAC_PI_2;
                } else if x == 0.0 && y == 0.0 {
                    return Ok((0.0, FRAC_PI_2));
                } else if x == 0.0 && y < 0.0 {
                    lam = 0.0;
                    phi = FRAC_PI_4;
                } else {
                    lam = FRAC_PI_4;
                }
            } else if x >= 0.0 && y >= 0.0 {
                lam = 3.0 * PI / 4.0;
            } else if x <= 0.0 && y >= 0.0 {
                lam = -3.0 * PI / 4.0;
            } else {
                lam = -FRAC_PI_4;
            }
            if x.abs() > SHD / 2.0 + 1e-3 || y.abs() > SHD / 2.0 + 1e-3 {
                phi = -FRAC_PI_4;
            }
        }
        generic_inverse(|l, p| self.fwd(l, p), x, y, (lam, phi), 1e-10)
    }
}
