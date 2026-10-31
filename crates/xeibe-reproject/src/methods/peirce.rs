//! Peirce Quincuncial, square and diamond: the world in a square, through
//! the elliptic integral of the first kind.
//!
//! Ported from PROJ 9.8.1 `src/projections/adams.cpp` (`peirce_q`; MIT, see
//! PROJ-NOTICE.md). PROJ has no closed inverse: it starts from a guess by
//! quadrant and solves by Newton's method.

use std::f64::consts::{FRAC_2_PI, FRAC_PI_2, FRAC_PI_4, PI};

use super::sphere::{Unit, aacos, aasin, generic_inverse};
use crate::error::CoordinateError;

const RSQRT2: f64 = std::f64::consts::FRAC_1_SQRT_2;
/// The complete elliptic integral of the first kind with m = 0.5, twice.
const SHD: f64 = 1.854_074_677_301_371_9 * 2.0;

#[derive(Debug)]
pub(crate) struct Peirce {
    square: bool,
}

impl Peirce {
    pub(crate) fn new(square: bool) -> Self {
        Peirce { square }
    }
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

impl Unit for Peirce {
    fn fwd(&self, lam: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let (sl, cl) = lam.sin_cos();
        let cp = phi.cos();
        let a = aacos(cp * (sl + cl) * RSQRT2)?;
        let b = aacos(cp * (sl - cl) * RSQRT2)?;
        let mut m = aasin((1.0 + (a + b).cos().min(0.0)).sqrt())?;
        if sl < 0.0 {
            m = -m;
        }
        let mut n = aasin((1.0 - (a - b).cos().max(0.0)).abs().sqrt())?;
        if cl > 0.0 {
            n = -n;
        }
        let (mut x, mut y) = (ell_int_5(m), ell_int_5(n));
        // The southern hemisphere spun out to the quincunx's triangles.
        if phi < 0.0 {
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
        if self.square {
            let temp = x;
            x = RSQRT2 * (x - y);
            y = RSQRT2 * (temp + y);
        }
        Ok((x, y))
    }

    fn inv(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        // PROJ's starting points, found by trial.
        let mut phi = 0.0;
        let lam;
        if self.square {
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
