//! Bipolar Oblique Conic Conformal of the western hemisphere, with or
//! without its skew (`ns`).
//!
//! Ported from PROJ 9.8.1 `src/projections/bipc.cpp` (MIT, see
//! PROJ-NOTICE.md), on the unit sphere, with its constants. Two of PROJ's
//! mistakes are fixed (divergence in `docs/pyproj-compat.md`): its forward
//! leaves out the tangent in Snyder's angle α where the two cones meet,
//! (tanⁿ(z/2) + tanⁿ((104° − z)/2)) / T, which its inverse has, and so
//! puts the band where they meet up to 240 km off (80°W 10°N), where its
//! inverse doesn't take it back; and where an argument of acos is a hair
//! over 1, it takes ±1 as the angle rather than 0 or π.

// PROJ's constants, digit for digit.
#![allow(clippy::excessive_precision)]

use std::f64::consts::{FRAC_PI_2, PI};

use super::sphere::Unit;
use crate::error::CoordinateError;

const EPS: f64 = 1e-10;
const ONEEPS: f64 = 1.000000001;
const LAM_B: f64 = -0.34894976726250681539;
const N: f64 = 0.63055844881274687180;
const F: f64 = 1.89724742567461030582;
const AZAB: f64 = 0.81650043674686363166;
const AZBA: f64 = 1.82261843856185925133;
const T: f64 = 1.27246578267089012270;
const RHOC: f64 = 1.20709121521568721927;
const C_AZC: f64 = 0.69691523038678375519;
const S_AZC: f64 = 0.71715351331143607555;
const C45: f64 = std::f64::consts::FRAC_1_SQRT_2;
const S45: f64 = std::f64::consts::FRAC_1_SQRT_2;
const C20: f64 = 0.93969262078590838411;
const S20: f64 = -0.34202014332566873287;
const R110: f64 = 1.91986217719376253360;
const R104: f64 = 1.81514242207410275904;

const OUTSIDE: CoordinateError = CoordinateError::OutsideDomain;

#[derive(Debug)]
pub(crate) struct Bipolar {
    pub(crate) noskew: bool,
}

/// acos, with an argument a hair over ±1 taken as ±1.
fn clamped_acos(z: f64) -> Result<f64, CoordinateError> {
    if z.abs() > ONEEPS {
        return Err(OUTSIDE);
    }
    Ok(z.clamp(-1.0, 1.0).acos())
}

impl Unit for Bipolar {
    fn fwd(&self, lam: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let (sphi, cphi) = phi.sin_cos();
        let (sdlam, cdlam) = (LAM_B - lam).sin_cos();
        let (mut az, tphi) = if (phi.abs() - FRAC_PI_2).abs() < EPS {
            (if phi < 0.0 { PI } else { 0.0 }, None)
        } else {
            let tphi = sphi / cphi;
            (sdlam.atan2(C45 * (tphi - cdlam)), Some(tphi))
        };
        let tag = az > AZBA;
        let (z, av, mut y);
        if tag {
            let (sdlam, cdlam) = (lam + R110).sin_cos();
            z = clamped_acos(S20 * sphi + C20 * cphi * cdlam)?;
            if let Some(tphi) = tphi {
                az = sdlam.atan2(C20 * tphi - S20 * cdlam);
            }
            av = AZAB;
            y = RHOC;
        } else {
            z = clamped_acos(S45 * (sphi + cphi * cdlam))?;
            av = AZBA;
            y = -RHOC;
        }
        if z < 0.0 {
            return Err(OUTSIDE);
        }
        let t = (0.5 * z).tan().powf(N);
        let mut r = F * t;
        let al = 0.5 * (R104 - z);
        if al < 0.0 {
            return Err(OUTSIDE);
        }
        // Snyder's α, with the tangent PROJ's forward leaves out.
        let al = clamped_acos((t + al.tan().powf(N)) / T)?;
        let t = N * (av - az);
        if t.abs() < al {
            r /= (al + if tag { t } else { -t }).cos();
        }
        let mut x = r * t.sin();
        y += if tag { -r } else { r } * t.cos();
        if self.noskew {
            let t = x;
            x = -x * C_AZC - y * S_AZC;
            y = -y * C_AZC + t * S_AZC;
        }
        Ok((x, y))
    }

    fn inv(&self, mut x: f64, mut y: f64) -> Result<(f64, f64), CoordinateError> {
        if self.noskew {
            let t = x;
            x = -x * C_AZC + y * S_AZC;
            y = -y * C_AZC - t * S_AZC;
        }
        let neg = x < 0.0;
        let (s, c, av) = if neg {
            y = RHOC - y;
            (S20, C20, AZAB)
        } else {
            y += RHOC;
            (S45, C45, AZBA)
        };
        let mut r = x.hypot(y);
        let rp = r;
        let mut rl = r;
        let az = x.atan2(y);
        let f_az = az.abs();
        let mut z = 0.0;
        let mut converged = false;
        // PROJ stops at 1e-10 (0.6 mm) after at most 10 steps; we go on to
        // 1e-13.
        for _ in 0..30 {
            z = 2.0 * (r / F).powf(1.0 / N).atan();
            let al = (((0.5 * z).tan().powf(N) + (0.5 * (R104 - z)).tan().powf(N)) / T).acos();
            if f_az < al {
                r = rp * (al + if neg { az } else { -az }).cos();
            }
            if (rl - r).abs() < 1e-13 {
                converged = true;
                break;
            }
            rl = r;
        }
        if !converged {
            return Err(OUTSIDE);
        }
        let az = av - az / N;
        let phi = (s * z.cos() + c * z.sin() * az.cos()).asin();
        let lam = az.sin().atan2(c / z.tan() - s * az.cos());
        Ok((if neg { lam - R110 } else { LAM_B - lam }, phi))
    }
}
