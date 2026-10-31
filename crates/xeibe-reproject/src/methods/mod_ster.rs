//! The modified stereographic projections: Miller and Lee Oblated, and
//! those of the 48 and 50 United States and of Alaska.
//!
//! Ported from PROJ 9.8.1 `src/projections/mod_ster.cpp` and
//! `src/zpoly1.cpp` (MIT, see PROJ-NOTICE.md). Each fixes its own origin;
//! GS48, GS50 and Alaska fix their sphere or ellipsoid too, of which PROJ's
//! inverse keeps the scale of the CRS's own (`Spherical::forward_radius`).

use std::f64::consts::FRAC_PI_2;

use super::sphere::{Unit, aasin};
use crate::error::CoordinateError;

const EPSLN: f64 = 1e-12;
const DEG: f64 = std::f64::consts::PI / 180.0;

#[derive(Debug)]
pub(crate) struct ModifiedStereographic {
    zcoeff: &'static [(f64, f64)],
    e: f64,
    phi0: f64,
    schio: f64,
    cchio: f64,
}

/// What each projection fixes: its origin, and its radius or ellipsoid.
pub(crate) struct Fixed {
    pub(crate) lam0: f64,
    /// The semi-major axis forwards, where it fixes one.
    pub(crate) a: Option<f64>,
}

const MIL_OS: [(f64, f64); 3] = [(0.924500, 0.0), (0.0, 0.0), (0.019430, 0.0)];
const LEE_OS: [(f64, f64); 3] = [(0.721316, 0.0), (0.0, 0.0), (-0.0088162, -0.00617325)];
const GS48: [(f64, f64); 5] = [
    (0.98879, 0.0),
    (0.0, 0.0),
    (-0.050909, 0.0),
    (0.0, 0.0),
    (0.075528, 0.0),
];
const ALSK_E: [(f64, f64); 6] = [
    (0.9945303, 0.0),
    (0.0052083, -0.0027404),
    (0.0072721, 0.0048181),
    (-0.0151089, -0.1932526),
    (0.0642675, -0.1381226),
    (0.3582802, -0.2884586),
];
const ALSK_S: [(f64, f64); 6] = [
    (0.9972523, 0.0),
    (0.0052513, -0.0041175),
    (0.0074606, 0.0048125),
    (-0.0153783, -0.1968253),
    (0.0636871, -0.1408027),
    (0.3660976, -0.2937382),
];
const GS50_E: [(f64, f64); 10] = [
    (0.9827497, 0.0),
    (0.0210669, 0.0053804),
    (-0.1031415, -0.0571664),
    (-0.0323337, -0.0322847),
    (0.0502303, 0.1211983),
    (0.0251805, 0.0895678),
    (-0.0012315, -0.1416121),
    (0.0072202, -0.1317091),
    (-0.0194029, 0.0759677),
    (-0.0210072, 0.0834037),
];
const GS50_S: [(f64, f64); 10] = [
    (0.9842990, 0.0),
    (0.0211642, 0.0037608),
    (-0.1036018, -0.0575102),
    (-0.0329095, -0.0320119),
    (0.0499471, 0.1223335),
    (0.0260460, 0.0899805),
    (0.0007388, -0.1435792),
    (0.0075848, -0.1334108),
    (-0.0216473, 0.0776645),
    (-0.0225161, 0.0853673),
];

/// A projection's coefficients, central meridian and latitude of origin
/// (degrees), fixed semi-major axis, the e its formulas use, and whether
/// PROJ's set-up takes the sphere's branch.
type Setup = (&'static [(f64, f64)], f64, f64, Option<f64>, f64, bool);

/// Clarke 1866's e², which Alaska and GS50 fix on an ellipsoid.
const CLARKE_ES: f64 = 0.00676866;

impl ModifiedStereographic {
    /// The projection of a PROJ name on an ellipsoid of eccentricity `e`,
    /// and what it fixes; `None` for another name.
    pub(crate) fn of(proj: &str, e: f64) -> Option<(Self, Fixed)> {
        let ellipsoidal = e != 0.0;
        // Miller, Lee and GS48 set PROJ's e² to 0 but keep its e, which the
        // formulas use while the set-up takes the sphere's branch; Alaska and
        // GS50 fix Clarke 1866's on an ellipsoid.
        let (zcoeff, lam0, phi0, a, e, spherical_setup): Setup = match proj {
            "mil_os" => (&MIL_OS[..], 20.0, 18.0, None, e, true),
            "lee_os" => (&LEE_OS[..], -165.0, -10.0, None, e, true),
            "gs48" => (&GS48[..], -96.0, 39.0, Some(6_370_997.0), e, true),
            "alsk" if ellipsoidal => (
                &ALSK_E,
                -152.0,
                64.0,
                Some(6_378_206.4),
                CLARKE_ES.sqrt(),
                false,
            ),
            "alsk" => (&ALSK_S[..], -152.0, 64.0, Some(6_370_997.0), 0.0, true),
            "gs50" if ellipsoidal => (
                &GS50_E,
                -120.0,
                45.0,
                Some(6_378_206.4),
                CLARKE_ES.sqrt(),
                false,
            ),
            "gs50" => (&GS50_S[..], -120.0, 45.0, Some(6_370_997.0), 0.0, true),
            _ => return None,
        };
        let phi0 = phi0 * DEG;
        let chio = if spherical_setup {
            phi0
        } else {
            let esphi = e * phi0.sin();
            2.0 * (((FRAC_PI_2 + phi0) * 0.5).tan() * ((1.0 - esphi) / (1.0 + esphi)).powf(e * 0.5))
                .atan()
                - FRAC_PI_2
        };
        Some((
            ModifiedStereographic {
                zcoeff,
                e,
                phi0,
                schio: chio.sin(),
                cchio: chio.cos(),
            },
            Fixed {
                lam0: lam0 * DEG,
                a,
            },
        ))
    }
}

/// The complex polynomial `z (C0 + z (C1 + …))` (`pj_zpoly1`).
fn zpoly1(z: (f64, f64), c: &[(f64, f64)]) -> (f64, f64) {
    let mut a = c[c.len() - 1];
    for &(cr, ci) in c[..c.len() - 1].iter().rev() {
        let t = a.0;
        a = (cr + z.0 * t - z.1 * a.1, ci + z.0 * a.1 + z.1 * t);
    }
    let t = a.0;
    (z.0 * t - z.1 * a.1, z.0 * a.1 + z.1 * t)
}

/// The same with its derivative (`pj_zpolyd1`).
fn zpolyd1(z: (f64, f64), c: &[(f64, f64)]) -> ((f64, f64), (f64, f64)) {
    let mut a = c[c.len() - 1];
    let mut b = a;
    let mut first = true;
    for &(cr, ci) in c[..c.len() - 1].iter().rev() {
        if first {
            first = false;
        } else {
            let t = b.0;
            b = (a.0 + z.0 * t - z.1 * b.1, a.1 + z.0 * b.1 + z.1 * t);
        }
        let t = a.0;
        a = (cr + z.0 * t - z.1 * a.1, ci + z.0 * a.1 + z.1 * t);
    }
    let t = b.0;
    b = (a.0 + z.0 * t - z.1 * b.1, a.1 + z.0 * b.1 + z.1 * t);
    let t = a.0;
    a = (z.0 * t - z.1 * a.1, z.0 * a.1 + z.1 * t);
    (a, b)
}

impl Unit for ModifiedStereographic {
    fn fwd(&self, lam: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let (sinlon, coslon) = lam.sin_cos();
        let esphi = self.e * phi.sin();
        let chi = 2.0
            * (((FRAC_PI_2 + phi) * 0.5).tan()
                * ((1.0 - esphi) / (1.0 + esphi)).powf(self.e * 0.5))
            .atan()
            - FRAC_PI_2;
        let (schi, cchi) = chi.sin_cos();
        let denom = 1.0 + self.schio * schi + self.cchio * cchi * coslon;
        if denom == 0.0 {
            return Err(CoordinateError::OutsideDomain);
        }
        let s = 2.0 / denom;
        let p = (
            s * cchi * sinlon,
            s * (self.cchio * schi - self.schio * cchi * coslon),
        );
        Ok(zpoly1(p, self.zcoeff))
    }

    fn inv(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let mut p = (x, y);
        let mut converged = false;
        for _ in 0..20 {
            let (mut fxy, fpxy) = zpolyd1(p, self.zcoeff);
            fxy.0 -= x;
            fxy.1 -= y;
            let den = fpxy.0 * fpxy.0 + fpxy.1 * fpxy.1;
            let dp = (
                -(fxy.0 * fpxy.0 + fxy.1 * fpxy.1) / den,
                -(fxy.1 * fpxy.0 - fxy.0 * fpxy.1) / den,
            );
            p.0 += dp.0;
            p.1 += dp.1;
            if dp.0.abs() + dp.1.abs() <= EPSLN {
                converged = true;
                break;
            }
        }
        if !converged {
            return Err(CoordinateError::OutsideDomain);
        }
        let rh = p.0.hypot(p.1);
        let z = 2.0 * (0.5 * rh).atan();
        let (sinz, cosz) = z.sin_cos();
        if rh.abs() <= EPSLN {
            return Ok((0.0, self.phi0));
        }
        let chi = aasin(cosz * self.schio + p.1 * sinz * self.cchio / rh)?;
        let mut phi = chi;
        let mut converged = false;
        for _ in 0..20 {
            let esphi = self.e * phi.sin();
            let dphi = 2.0
                * (((FRAC_PI_2 + chi) * 0.5).tan()
                    * ((1.0 + esphi) / (1.0 - esphi)).powf(self.e * 0.5))
                .atan()
                - FRAC_PI_2
                - phi;
            phi += dphi;
            if dphi.abs() <= EPSLN {
                converged = true;
                break;
            }
        }
        if !converged {
            return Err(CoordinateError::OutsideDomain);
        }
        Ok((
            (p.0 * sinz).atan2(rh * self.cchio * cosz - p.1 * self.schio * sinz),
            phi,
        ))
    }
}
