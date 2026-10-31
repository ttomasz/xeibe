//! PROJ's world projections on the sphere: the pseudocylindrical and
//! other small-scale projections only PROJ defines (Robinson, Mollweide,
//! the Eckerts, Wagners and Winkels, Natural Earth, …), and Sinusoidal,
//! which also has an ellipsoidal form.
//!
//! Ported from PROJ 9.8.1 `src/projections/` (MIT, see PROJ-NOTICE.md):
//! `eck1.cpp`, `eck2.cpp`, `eck3.cpp` (`eck3`, `wag6`), `eck4.cpp`, `eck5.cpp`,
//! `mbtfpq.cpp`, `gall.cpp`, `wag2.cpp`, `wag3.cpp`, `wag7.cpp`,
//! `crast.cpp`, `times.cpp`, `moll.cpp` (`moll`, `wag4`, `wag5`),
//! `gn_sinu.cpp` (`sinu`, `eck6`), `urmfps.cpp` (`wag1`), `sts.cpp`
//! (`qua_aut`), `robin.cpp`, `natearth.cpp`, `natearth2.cpp`,
//! `patterson.cpp`, `wink1.cpp`, `wink2.cpp`, `loxim.cpp`, `mill.cpp`,
//! `comill.cpp`, `vandg.cpp`, `aitoff.cpp` (`aitoff`, `wintri`) and
//! `tcea.cpp`.

// PROJ's constants, digit for digit.
#![allow(clippy::excessive_precision)]

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};

use super::mlfn::Mlfn;
use super::sphere::{Unit, aasin, asqrt, generic_inverse};
use super::{BuildError, Params, Projection, adjlon, param, proj_codes};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

const OUTSIDE: CoordinateError = CoordinateError::OutsideDomain;

/// One of the spherical world projections, with its constants.
#[derive(Debug)]
pub(crate) enum World {
    Eckert1,
    Eckert2,
    /// Eckert III and Wagner VI (`eck3.cpp`).
    Eckert3 {
        cx: f64,
        cy: f64,
        a: f64,
        b: f64,
    },
    Eckert4,
    Eckert5,
    FlatPolarQuartic,
    Gall,
    Wagner2,
    Wagner3 {
        cx: f64,
    },
    Wagner7,
    Craster,
    Times,
    /// Mollweide, Wagner IV and Wagner V (`moll.cpp`).
    Mollweide {
        cx: f64,
        cy: f64,
        cp: f64,
    },
    /// Spherical Sinusoidal and Eckert VI (`gn_sinu.cpp`).
    Sinusoidal {
        m: f64,
        n: f64,
        cx: f64,
        cy: f64,
    },
    /// Wagner I (`urmfps.cpp`).
    Urmaev {
        n: f64,
        cy: f64,
    },
    /// Quartic Authalic (`sts.cpp`).
    Sts {
        cx: f64,
        cy: f64,
        cp: f64,
        tan_mode: bool,
    },
    Robinson,
    NaturalEarth,
    NaturalEarth2,
    Patterson,
    Winkel1 {
        cosphi1: f64,
    },
    Winkel2 {
        cosphi1: f64,
    },
    Loximuthal {
        phi1: f64,
        cosphi1: f64,
        tanphi1: f64,
    },
    Miller,
    CompactMiller,
    VanDerGrinten,
    /// Aitoff, and Winkel Tripel with `cosphi1` (`aitoff.cpp`).
    Aitoff {
        winkel: Option<f64>,
    },
    TransverseCylindricalEqualArea {
        k0: f64,
        phi0: f64,
    },
}

/// A parameter only PROJ defines, or its default.
fn proj_param(params: &Params, code: u32, default: f64) -> f64 {
    params.get(code).unwrap_or(default)
}

impl World {
    /// The projection of one of PROJ's methods, `None` for another method.
    pub(crate) fn of(code: u32, params: &Params) -> Result<Option<World>, BuildError> {
        use proj_codes as c;
        let lat_1 = || params.get(param::LAT_1ST_PARALLEL);
        Ok(Some(match code {
            c::ECKERT_I => World::Eckert1,
            c::ECKERT_II => World::Eckert2,
            c::ECKERT_III => World::Eckert3 {
                cx: 0.422_238_200_315_771_2,
                cy: 0.844_476_400_631_542_4,
                a: 1.0,
                b: 0.405_284_734_569_351_1,
            },
            c::WAGNER_VI => World::Eckert3 {
                cx: 1.0,
                cy: 1.0,
                a: 0.0,
                b: 0.303_963_550_927_013_3,
            },
            c::ECKERT_IV => World::Eckert4,
            c::ECKERT_V => World::Eckert5,
            c::FLAT_POLAR_QUARTIC => World::FlatPolarQuartic,
            c::GALL_STEREOGRAPHIC => World::Gall,
            c::WAGNER_II => World::Wagner2,
            c::WAGNER_III => {
                let ts = proj_param(params, c::LATITUDE_OF_TRUE_SCALE, 0.0);
                World::Wagner3 {
                    cx: ts.cos() / (2.0 * ts / 3.0).cos(),
                }
            }
            c::WAGNER_VII => World::Wagner7,
            c::CRASTER_PARABOLIC => World::Craster,
            c::TIMES => World::Times,
            c::MOLLWEIDE => World::mollweide(FRAC_PI_2),
            c::WAGNER_IV => World::mollweide(PI / 3.0),
            c::WAGNER_V => World::Mollweide {
                cx: 0.90977,
                cy: 1.65014,
                cp: 3.00896,
            },
            c::SINUSOIDAL => World::sinusoidal(0.0, 1.0),
            c::ECKERT_VI => World::sinusoidal(1.0, 2.570_796_326_794_896_6),
            c::WAGNER_I => {
                let n = 0.866_025_403_784_438_6;
                World::Urmaev {
                    n,
                    cy: 1.139_753_528_477 / n,
                }
            }
            c::QUARTIC_AUTHALIC => World::sts(2.0, 2.0, false),
            c::ROBINSON => World::Robinson,
            c::NATURAL_EARTH => World::NaturalEarth,
            c::NATURAL_EARTH_II => World::NaturalEarth2,
            c::PATTERSON => World::Patterson,
            c::WINKEL_I => World::Winkel1 {
                cosphi1: lat_1().unwrap_or(0.0).cos(),
            },
            c::WINKEL_II => World::Winkel2 {
                cosphi1: lat_1().unwrap_or(0.0).cos(),
            },
            c::LOXIMUTHAL => {
                let phi1 = params.get(param::LAT_NATURAL_ORIGIN).unwrap_or(0.0);
                let cosphi1 = phi1.cos();
                if cosphi1 < 1e-8 {
                    return Err(BuildError::Invalid(
                        "Invalid value for lat_1: |lat_1| should be < 90°".into(),
                    ));
                }
                World::Loximuthal {
                    phi1,
                    cosphi1,
                    tanphi1: (FRAC_PI_4 + 0.5 * phi1).tan(),
                }
            }
            c::MILLER_CYLINDRICAL => World::Miller,
            c::COMPACT_MILLER => World::CompactMiller,
            c::VAN_DER_GRINTEN => World::VanDerGrinten,
            c::AITOFF => World::Aitoff { winkel: None },
            c::TRANSVERSE_CYLINDRICAL_EQUAL_AREA => World::TransverseCylindricalEqualArea {
                k0: params.get(param::SCALE_NATURAL_ORIGIN)?,
                phi0: params.get(param::LAT_NATURAL_ORIGIN)?,
            },
            c::WINKEL_TRIPEL => {
                // 50°28′ (acos(2/π)) when no latitude is given.
                let cosphi1 = lat_1().map_or(0.636_619_772_367_581_3, f64::cos);
                if cosphi1 == 0.0 {
                    return Err(BuildError::Invalid(
                        "Invalid value for lat_1: |lat_1| should be < 90°".into(),
                    ));
                }
                World::Aitoff {
                    winkel: Some(cosphi1),
                }
            }
            _ => return Ok(None),
        }))
    }

    pub(crate) fn mollweide(p: f64) -> World {
        let p2 = p + p;
        let sp = p.sin();
        let r = (2.0 * PI * sp / (p2 + p2.sin())).sqrt();
        World::Mollweide {
            cx: 2.0 * r / PI,
            cy: r / sp,
            cp: p2 + p2.sin(),
        }
    }

    /// `sts.cpp`'s set-up (Quartic Authalic, Kavrayskiy V, Foucaut,
    /// McBryde-Thomas Flat-Polar Sine).
    pub(crate) fn sts(p: f64, q: f64, tan_mode: bool) -> World {
        World::Sts {
            cx: q / p,
            cy: p,
            cp: 1.0 / q,
            tan_mode,
        }
    }

    pub(crate) fn sinusoidal(m: f64, n: f64) -> World {
        let cy = ((m + 1.0) / n).sqrt();
        World::Sinusoidal {
            m,
            n,
            cx: cy / (m + 1.0),
            cy,
        }
    }
}

// Robinson's table, at 5° intervals (`robin.cpp`). PROJ keeps it in single
// precision.
#[rustfmt::skip]
const ROBIN_X: [[f32; 4]; 19] = [
    [1.0, 2.2199e-17, -7.15515e-05, 3.1103e-06],
    [0.9986, -0.000482243, -2.4897e-05, -1.3309e-06],
    [0.9954, -0.00083103, -4.48605e-05, -9.86701e-07],
    [0.99, -0.00135364, -5.9661e-05, 3.6777e-06],
    [0.9822, -0.00167442, -4.49547e-06, -5.72411e-06],
    [0.973, -0.00214868, -9.03571e-05, 1.8736e-08],
    [0.96, -0.00305085, -9.00761e-05, 1.64917e-06],
    [0.9427, -0.00382792, -6.53386e-05, -2.6154e-06],
    [0.9216, -0.00467746, -0.00010457, 4.81243e-06],
    [0.8962, -0.00536223, -3.23831e-05, -5.43432e-06],
    [0.8679, -0.00609363, -0.000113898, 3.32484e-06],
    [0.835, -0.00698325, -6.40253e-05, 9.34959e-07],
    [0.7986, -0.00755338, -5.00009e-05, 9.35324e-07],
    [0.7597, -0.00798324, -3.5971e-05, -2.27626e-06],
    [0.7186, -0.00851367, -7.01149e-05, -8.6303e-06],
    [0.6732, -0.00986209, -0.000199569, 1.91974e-05],
    [0.6213, -0.010418, 8.83923e-05, 6.24051e-06],
    [0.5722, -0.00906601, 0.000182, 6.24051e-06],
    [0.5322, -0.00677797, 0.000275608, 6.24051e-06],
];
#[rustfmt::skip]
const ROBIN_Y: [[f32; 4]; 19] = [
    [-5.20417e-18, 0.0124, 1.21431e-18, -8.45284e-11],
    [0.062, 0.0124, -1.26793e-09, 4.22642e-10],
    [0.124, 0.0124, 5.07171e-09, -1.60604e-09],
    [0.186, 0.0123999, -1.90189e-08, 6.00152e-09],
    [0.248, 0.0124002, 7.10039e-08, -2.24e-08],
    [0.31, 0.0123992, -2.64997e-07, 8.35986e-08],
    [0.372, 0.0124029, 9.88983e-07, -3.11994e-07],
    [0.434, 0.0123893, -3.69093e-06, -4.35621e-07],
    [0.4958, 0.0123198, -1.02252e-05, -3.45523e-07],
    [0.5571, 0.0121916, -1.54081e-05, -5.82288e-07],
    [0.6176, 0.0119938, -2.41424e-05, -5.25327e-07],
    [0.6769, 0.011713, -3.20223e-05, -5.16405e-07],
    [0.7346, 0.0113541, -3.97684e-05, -6.09052e-07],
    [0.7903, 0.0109107, -4.89042e-05, -1.04739e-06],
    [0.8435, 0.0103431, -6.4615e-05, -1.40374e-09],
    [0.8936, 0.00969686, -6.4636e-05, -8.547e-06],
    [0.9394, 0.00840947, -0.000192841, -4.2106e-06],
    [0.9761, 0.00616527, -0.000256, -4.2106e-06],
    [1.0, 0.00328947, -0.000319159, -4.2106e-06],
];

fn robin_v(c: &[f32; 4], z: f64) -> f64 {
    let [c0, c1, c2, c3] = c.map(f64::from);
    c0 + z * (c1 + z * (c2 + z * c3))
}

fn robin_dv(c: &[f32; 4], z: f64) -> f64 {
    let [_, c1, c2, c3] = c.map(f64::from);
    c1 + 2.0 * z * c2 + z * z * 3.0 * c3
}

// Eckert IV: C_x = 2 / √(4π + π²), C_y = 2 √(π / (4 + π)), C_p = 2 + π/2.
const ECK4_CX: f64 = 0.422_238_200_315_771_2;
const ECK4_CY: f64 = 1.326_500_428_177_002_3;
/// 1 / C_y, as PROJ rounds it.
const ECK4_RCY: f64 = 0.753_863_307_360_021_8;
const ECK4_CP: f64 = 3.570_796_326_794_896_6;
const ECK4_RCP: f64 = 0.280_049_576_755_778_7;

const RAD_TO_DEG: f64 = 57.295_779_513_082_32;
const DEG_TO_RAD: f64 = 0.017_453_292_519_943_295;

/// x and y of Aitoff, or Winkel Tripel with `cosphi1`.
fn aitoff_xy(lam: f64, phi: f64, winkel: Option<f64>) -> (f64, f64) {
    let c = 0.5 * lam;
    let d = (phi.cos() * c.cos()).acos();
    let (mut x, mut y) = if d != 0.0 {
        let y = 1.0 / d.sin();
        (2.0 * d * phi.cos() * c.sin() * y, y * d * phi.sin())
    } else {
        (0.0, 0.0)
    };
    if let Some(cosphi1) = winkel {
        x = (x + lam * cosphi1) * 0.5;
        y = (y + phi) * 0.5;
    }
    (x, y)
}

fn winkel2_xy(lam: f64, mut phi: f64, cosphi1: f64) -> (f64, f64) {
    let y = phi * std::f64::consts::FRAC_2_PI;
    let k = PI * phi.sin();
    phi *= 1.8;
    let mut converged = false;
    for _ in 0..10 {
        let v = (phi + phi.sin() - k) / (1.0 + phi.cos());
        phi -= v;
        if v.abs() < 1e-7 {
            converged = true;
            break;
        }
    }
    phi = if converged {
        phi * 0.5
    } else if phi < 0.0 {
        -FRAC_PI_2
    } else {
        FRAC_PI_2
    };
    (
        0.5 * lam * (phi.cos() + cosphi1),
        FRAC_PI_4 * (phi.sin() + y),
    )
}

impl Unit for World {
    fn fwd(&self, lam: f64, mut phi: f64) -> Result<(f64, f64), CoordinateError> {
        Ok(match *self {
            World::Eckert1 => {
                const FC: f64 = 0.921_317_731_923_561_3;
                const RP: f64 = std::f64::consts::FRAC_1_PI;
                (FC * lam * (1.0 - RP * phi.abs()), FC * phi)
            }
            World::Eckert2 => {
                const FXC: f64 = 0.460_658_865_961_780_6;
                const FYC: f64 = 1.447_202_509_116_535_3;
                let s = (4.0 - 3.0 * phi.abs().sin()).sqrt();
                let y = FYC * (2.0 - s);
                (FXC * lam * s, if phi < 0.0 { -y } else { y })
            }
            World::Eckert3 { cx, cy, a, b } => {
                (cx * lam * (a + asqrt(1.0 - b * phi * phi)), cy * phi)
            }
            World::Eckert4 => {
                // Newton's method for θ + sin θ cos θ + 2 sin θ = C_p sin φ.
                let p = ECK4_CP * phi.sin();
                let v = phi * phi;
                let mut theta = phi * (0.895168 + v * (0.0218849 + v * 0.00826809));
                let mut converged = false;
                for _ in 0..6 {
                    let (s, c) = theta.sin_cos();
                    let v = (theta + s * (c + 2.0) - p) / (1.0 + c * (c + 2.0) - s * s);
                    theta -= v;
                    if v.abs() < 1e-7 {
                        converged = true;
                        break;
                    }
                }
                if converged {
                    (ECK4_CX * lam * (1.0 + theta.cos()), ECK4_CY * theta.sin())
                } else {
                    (ECK4_CX * lam, if theta < 0.0 { -ECK4_CY } else { ECK4_CY })
                }
            }
            World::Eckert5 => {
                const XF: f64 = 0.441_012_771_724_551_5;
                const YF: f64 = 0.882_025_543_449_103;
                (XF * (1.0 + phi.cos()) * lam, YF * phi)
            }
            World::FlatPolarQuartic => {
                const C: f64 = 1.707_106_781_186_547_5;
                const FYC: f64 = 1.874_758_284_622_695;
                const FXC: f64 = 0.312_459_714_103_782_5;
                let c = C * phi.sin();
                for _ in 0..20 {
                    let th1 =
                        ((0.5 * phi).sin() + phi.sin() - c) / (0.5 * (0.5 * phi).cos() + phi.cos());
                    phi -= th1;
                    if th1.abs() < 1e-7 {
                        break;
                    }
                }
                (
                    FXC * lam * (1.0 + 2.0 * phi.cos() / (0.5 * phi).cos()),
                    FYC * (0.5 * phi).sin(),
                )
            }
            World::Gall => {
                const YF: f64 = 1.707_106_781_186_547_5;
                const XF: f64 = 0.707_106_781_186_547_5;
                (XF * lam, YF * (0.5 * phi).tan())
            }
            World::Wagner2 => {
                let phi = aasin(0.88022 * (0.88550 * phi).sin())?;
                (0.92483 * lam * phi.cos(), 1.38725 * phi)
            }
            World::Wagner3 { cx } => (cx * lam * (2.0 / 3.0 * phi).cos(), phi),
            World::Wagner7 => {
                let y = 0.906_307_787_036_649_96 * phi.sin();
                let theta = y.asin();
                let ct = theta.cos();
                let lam = lam / 3.0;
                let d = 1.0 / (0.5 * (1.0 + ct * lam.cos())).sqrt();
                (2.66723 * ct * lam.sin() * d, y * 1.24104 * d)
            }
            World::Craster => {
                const XM: f64 = 0.977_205_023_805_839_8;
                const YM: f64 = 3.069_980_123_839_465_5;
                let phi = phi * 0.333_333_333_333_333_333;
                (XM * lam * (2.0 * (phi + phi).cos() - 1.0), YM * phi.sin())
            }
            World::Times => {
                let t = (phi / 2.0).tan();
                let s = (FRAC_PI_4 * t).sin();
                (lam * (0.74482 - 0.34588 * s * s), 1.70711 * t)
            }
            World::Mollweide { cx, cy, cp } => {
                let k = cp * phi.sin();
                let mut converged = false;
                for _ in 0..30 {
                    let v = (phi + phi.sin() - k) / (1.0 + phi.cos());
                    phi -= v;
                    if v.abs() < 1e-7 {
                        converged = true;
                        break;
                    }
                }
                phi = if !converged {
                    if phi < 0.0 { -FRAC_PI_2 } else { FRAC_PI_2 }
                } else {
                    phi * 0.5
                };
                (cx * lam * phi.cos(), cy * phi.sin())
            }
            World::Sinusoidal { m, n, cx, cy } => {
                if m == 0.0 {
                    if n != 1.0 {
                        phi = aasin(n * phi.sin())?;
                    }
                } else {
                    let k = n * phi.sin();
                    let mut converged = false;
                    for _ in 0..8 {
                        let v = (m * phi + phi.sin() - k) / (m + phi.cos());
                        phi -= v;
                        if v.abs() < 1e-7 {
                            converged = true;
                            break;
                        }
                    }
                    if !converged {
                        return Err(OUTSIDE);
                    }
                }
                (cx * lam * (m + phi.cos()), cy * phi)
            }
            World::Urmaev { n, cy } => {
                let phi = aasin(n * phi.sin())?;
                (0.877_382_675_3 * lam * phi.cos(), cy * phi)
            }
            World::Sts {
                cx,
                cy,
                cp,
                tan_mode,
            } => {
                let mut x = cx * lam * phi.cos();
                let mut y = cy;
                let phi = phi * cp;
                let c = phi.cos();
                if tan_mode {
                    x *= c * c;
                    y *= phi.tan();
                } else {
                    x /= c;
                    y *= phi.sin();
                }
                (x, y)
            }
            World::Robinson => {
                const FXC: f64 = 0.8487;
                const FYC: f64 = 1.3523;
                const C1: f64 = 11.459_155_902_616_464;
                const RC1: f64 = 0.087_266_462_599_716_48;
                let dphi = phi.abs();
                if phi.is_nan() {
                    return Err(OUTSIDE);
                }
                let i = ((dphi * C1 + 1e-15).floor() as usize).min(18);
                let dphi = RAD_TO_DEG * (dphi - RC1 * i as f64);
                let x = robin_v(&ROBIN_X[i], dphi) * FXC * lam;
                let y = robin_v(&ROBIN_Y[i], dphi) * FYC;
                (x, if phi < 0.0 { -y } else { y })
            }
            World::NaturalEarth => {
                let phi2 = phi * phi;
                let phi4 = phi2 * phi2;
                (
                    lam * (0.8707
                        + phi2
                            * (-0.131979
                                + phi2
                                    * (-0.013791 + phi4 * phi2 * (0.003971 + phi2 * -0.001529)))),
                    phi * (1.007226
                        + phi2
                            * (0.015085 + phi4 * (-0.044475 + 0.028874 * phi2 + -0.005916 * phi4))),
                )
            }
            World::NaturalEarth2 => {
                let phi2 = phi * phi;
                let phi4 = phi2 * phi2;
                let phi6 = phi2 * phi4;
                (
                    lam * (0.84719
                        + -0.13063 * phi2
                        + phi6
                            * phi6
                            * (-0.04515 + 0.05494 * phi2 + -0.02326 * phi4 + 0.00331 * phi6)),
                    phi * (1.01183 + phi4 * phi4 * (-0.02625 + 0.01926 * phi2 + -0.00396 * phi4)),
                )
            }
            World::Patterson => {
                let phi2 = phi * phi;
                (
                    lam,
                    phi * (1.0148 + phi2 * phi2 * (0.23185 + phi2 * (-0.14499 + 0.02406 * phi2))),
                )
            }
            World::Winkel1 { cosphi1 } => (0.5 * lam * (cosphi1 + phi.cos()), phi),
            World::Winkel2 { cosphi1 } => winkel2_xy(lam, phi, cosphi1),
            World::Loximuthal {
                phi1,
                cosphi1,
                tanphi1,
            } => {
                const EPS: f64 = 1e-8;
                let y = phi - phi1;
                let x = if y.abs() < EPS {
                    lam * cosphi1
                } else {
                    let x = FRAC_PI_4 + 0.5 * phi;
                    if x.abs() < EPS || (x.abs() - FRAC_PI_2).abs() < EPS {
                        0.0
                    } else {
                        lam * y / (x.tan() / tanphi1).ln()
                    }
                };
                (x, y)
            }
            World::Miller => (lam, (FRAC_PI_4 + phi * 0.4).tan().ln() * 1.25),
            World::CompactMiller => {
                let lat_sq = phi * phi;
                (lam, phi * (0.9902 + lat_sq * (0.1604 + -0.03054 * lat_sq)))
            }
            World::VanDerGrinten => vandg_fwd(lam, phi)?,
            World::Aitoff { winkel } => aitoff_xy(lam, phi, winkel),
            World::TransverseCylindricalEqualArea { k0, phi0 } => (
                phi.cos() * lam.sin() / k0,
                k0 * (phi.tan().atan2(lam.cos()) - phi0),
            ),
        })
    }

    fn inv(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        Ok(match *self {
            World::Eckert1 => {
                const FC: f64 = 0.921_317_731_923_561_3;
                const RP: f64 = std::f64::consts::FRAC_1_PI;
                let phi = y / FC;
                (x / (FC * (1.0 - RP * phi.abs())), phi)
            }
            World::Eckert2 => {
                const FXC: f64 = 0.460_658_865_961_780_6;
                const FYC: f64 = 1.447_202_509_116_535_3;
                const C13: f64 = 0.333_333_333_333_333_33;
                let p = 2.0 - y.abs() / FYC;
                let lam = x / (FXC * p);
                let s = (4.0 - p * p) * C13;
                let mut phi = if s.abs() >= 1.0 {
                    if s.abs() > 1.000_000_1 {
                        return Err(OUTSIDE);
                    }
                    if s < 0.0 { -FRAC_PI_2 } else { FRAC_PI_2 }
                } else {
                    s.asin()
                };
                if y < 0.0 {
                    phi = -phi;
                }
                (lam, phi)
            }
            World::Eckert3 { cx, cy, a, b } => {
                let phi = y / cy;
                let denominator = cx * (a + asqrt(1.0 - b * phi * phi));
                let lam = if denominator == 0.0 {
                    f64::INFINITY
                } else {
                    x / denominator
                };
                (lam, phi)
            }
            World::Eckert4 => {
                let sin_theta = y * ECK4_RCY;
                let one_minus = 1.0 - sin_theta.abs();
                let (lam, phi) = if (0.0..=1e-12).contains(&one_minus) {
                    (
                        x / ECK4_CX,
                        if sin_theta > 0.0 {
                            FRAC_PI_2
                        } else {
                            -FRAC_PI_2
                        },
                    )
                } else {
                    let theta = aasin(sin_theta)?;
                    let cos_theta = theta.cos();
                    let sin_phi = (theta + sin_theta * (cos_theta + 2.0)) * ECK4_RCP;
                    (x / (ECK4_CX * (1.0 + cos_theta)), aasin(sin_phi)?)
                };
                let beyond = lam.abs() - PI;
                if beyond > 0.0 {
                    if beyond > 1e-10 {
                        return Err(OUTSIDE);
                    }
                    return Ok((if lam > 0.0 { PI } else { -PI }, phi));
                }
                (lam, phi)
            }
            World::Eckert5 => {
                const RXF: f64 = 2.267_508_027_238_226_4;
                const RYF: f64 = 1.133_754_013_619_113_2;
                let phi = RYF * y;
                (RXF * x / (1.0 + phi.cos()), phi)
            }
            World::FlatPolarQuartic => {
                const ONETOL: f64 = 1.000_001;
                const RC: f64 = 0.585_786_437_626_905;
                const RYC: f64 = 0.533_402_096_794_177;
                const RXC: f64 = 3.200_412_580_765_062;
                let mut phi = RYC * y;
                let t;
                if phi.abs() > 1.0 {
                    if phi.abs() > ONETOL {
                        return Err(OUTSIDE);
                    }
                    (t, phi) = if phi < 0.0 { (-1.0, -PI) } else { (1.0, PI) };
                } else {
                    t = phi;
                    phi = 2.0 * phi.asin();
                }
                let lam = RXC * x / (1.0 + 2.0 * phi.cos() / (0.5 * phi).cos());
                phi = RC * (t + phi.sin());
                if phi.abs() > 1.0 {
                    if phi.abs() > ONETOL {
                        return Err(OUTSIDE);
                    }
                    phi = if phi < 0.0 { -FRAC_PI_2 } else { FRAC_PI_2 };
                } else {
                    phi = phi.asin();
                }
                (lam, phi)
            }
            World::Gall => {
                const RYF: f64 = 0.585_786_437_626_905;
                const RXF: f64 = std::f64::consts::SQRT_2;
                (RXF * x, 2.0 * (y * RYF).atan())
            }
            World::Wagner2 => {
                let phi = y / 1.38725;
                let lam = x / (0.92483 * phi.cos());
                (lam, aasin(phi.sin() / 0.88022)? / 0.88550)
            }
            World::Wagner3 { cx } => (x / (cx * (2.0 / 3.0 * y).cos()), y),
            // PROJ has no inverse.
            World::Wagner7 => return Err(OUTSIDE),
            World::Craster => {
                const RXM: f64 = 1.023_326_707_946_488_5;
                const RYM: f64 = 0.325_735_007_935_279_95;
                let phi = 3.0 * (y * RYM).asin();
                (
                    x * RXM / (2.0 * ((phi + phi) * 0.333_333_333_333_333_333).cos() - 1.0),
                    phi,
                )
            }
            World::Times => {
                let t = y / 1.70711;
                let s = (FRAC_PI_4 * t).sin();
                (x / (0.74482 - 0.34588 * s * s), 2.0 * t.atan())
            }
            World::Mollweide { cx, cy, cp } => {
                let mut phi = aasin(y / cy)?;
                let lam = x / (cx * phi.cos());
                if lam.abs() >= PI {
                    return Err(OUTSIDE);
                }
                phi += phi;
                (lam, aasin((phi + phi.sin()) / cp)?)
            }
            World::Sinusoidal { m, n, cx, cy } => {
                let y = y / cy;
                let phi = if m != 0.0 {
                    aasin((m * y + y.sin()) / n)?
                } else if n != 1.0 {
                    aasin(y.sin() / n)?
                } else {
                    y
                };
                (x / (cx * (m + y.cos())), phi)
            }
            World::Urmaev { n, cy } => {
                let y = y / cy;
                let phi = aasin(y.sin() / n)?;
                (x / (0.877_382_675_3 * y.cos()), phi)
            }
            World::Sts {
                cx,
                cy,
                cp,
                tan_mode,
            } => {
                let y = y / cy;
                let phi = if tan_mode { y.atan() } else { aasin(y)? };
                let c = phi.cos();
                let phi = phi / cp;
                let mut lam = x / (cx * phi.cos());
                if tan_mode {
                    lam /= c * c;
                } else {
                    lam *= c;
                }
                (lam, phi)
            }
            World::Robinson => robin_inv(x, y)?,
            World::NaturalEarth => {
                const MAX_Y: f64 = 0.8707 * 0.52 * PI;
                let (b0, b1, b2, b3, b4) = (1.007226, 0.015085, -0.044475, 0.028874, -0.005916);
                let y = y.clamp(-MAX_Y, MAX_Y);
                let yc = newton(y, |yc| {
                    let y2 = yc * yc;
                    let y4 = y2 * y2;
                    (
                        yc * (b0 + y2 * (b1 + y4 * (b2 + b3 * y2 + b4 * y4))) - y,
                        b0 + y2 * (3.0 * b1 + y4 * (7.0 * b2 + 9.0 * b3 * y2 + 11.0 * b4 * y4)),
                    )
                })?;
                let y2 = yc * yc;
                (
                    x / (0.8707
                        + y2 * (-0.131979
                            + y2 * (-0.013791 + y2 * y2 * y2 * (0.003971 + y2 * -0.001529)))),
                    yc,
                )
            }
            World::NaturalEarth2 => {
                const MAX_Y: f64 = 0.84719 * 0.535_117_535_153_096 * PI;
                let (b0, b1, b2, b3) = (1.01183, -0.02625, 0.01926, -0.00396);
                let y = y.clamp(-MAX_Y, MAX_Y);
                let yc = newton(y, |yc| {
                    let y2 = yc * yc;
                    let y4 = y2 * y2;
                    (
                        yc * (b0 + y4 * y4 * (b1 + b2 * y2 + b3 * y4)) - y,
                        b0 + y4 * y4 * (9.0 * b1 + 11.0 * b2 * y2 + 13.0 * b3 * y4),
                    )
                })?;
                let y2 = yc * yc;
                let y4 = y2 * y2;
                let y6 = y2 * y4;
                (
                    x / (0.84719
                        + -0.13063 * y2
                        + y6 * y6 * (-0.04515 + 0.05494 * y2 + -0.02326 * y4 + 0.00331 * y6)),
                    yc,
                )
            }
            World::Patterson => {
                // PROJ clamps a copy of y and starts from the unclamped one.
                const MAX_Y: f64 = 1.790_857_183;
                let (k1, k2, k3, k4) = (1.0148, 0.23185, -0.14499, 0.02406);
                let target = y.clamp(-MAX_Y, MAX_Y);
                let yc = newton(y, |yc| {
                    let y2 = yc * yc;
                    (
                        yc * (k1 + y2 * y2 * (k2 + y2 * (k3 + k4 * y2))) - target,
                        k1 + y2 * y2 * (5.0 * k2 + y2 * (7.0 * k3 + 9.0 * k4 * y2)),
                    )
                })?;
                (x, yc)
            }
            World::Winkel1 { cosphi1 } => (2.0 * x / (cosphi1 + y.cos()), y),
            World::Winkel2 { cosphi1 } => generic_inverse(
                |lam, phi| Ok(winkel2_xy(lam, phi, cosphi1)),
                x,
                y,
                (x, y),
                1e-10,
            )?,
            World::Loximuthal {
                phi1,
                cosphi1,
                tanphi1,
            } => {
                const EPS: f64 = 1e-8;
                let phi = y + phi1;
                let lam = if y.abs() < EPS {
                    x / cosphi1
                } else {
                    let l = FRAC_PI_4 + 0.5 * phi;
                    if l.abs() < EPS || (l.abs() - FRAC_PI_2).abs() < EPS {
                        0.0
                    } else {
                        x * (l.tan() / tanphi1).ln() / y
                    }
                };
                (lam, phi)
            }
            World::Miller => (x, 2.5 * ((0.8 * y).exp().atan() - FRAC_PI_4)),
            World::CompactMiller => {
                const MAX_Y: f64 = 0.600_020_766_986_265_5 * PI;
                let y = y.clamp(-MAX_Y, MAX_Y);
                let yc = newton(y, |yc| {
                    let y2 = yc * yc;
                    (
                        yc * (0.9902 + y2 * (0.1604 + -0.03054 * y2)) - y,
                        0.9902 + y2 * (3.0 * 0.1604 + 5.0 * -0.03054 * y2),
                    )
                })?;
                (x, yc)
            }
            World::VanDerGrinten => vandg_inv(x, y)?,
            World::Aitoff { winkel } => aitoff_inv(x, y, winkel)?,
            World::TransverseCylindricalEqualArea { k0, phi0 } => {
                let y = y / k0 + phi0;
                let x = x * k0;
                let t = (1.0 - x * x).sqrt();
                (x.atan2(t * y.cos()), (t * y.sin()).asin())
            }
        })
    }
}

/// Newton–Raphson from `start` on `f(yc) = (value, derivative)`, to 1e-11,
/// at most 100 steps (`natearth.cpp` and the like).
fn newton(start: f64, f: impl Fn(f64) -> (f64, f64)) -> Result<f64, CoordinateError> {
    let mut yc = start;
    for _ in 0..100 {
        let (v, d) = f(yc);
        let tol = v / d;
        yc -= tol;
        if tol.abs() < 1e-11 {
            return Ok(yc);
        }
    }
    Err(OUTSIDE)
}

fn robin_inv(x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
    const FXC: f64 = 0.8487;
    const FYC: f64 = 1.3523;
    const NODES: usize = 18;
    let mut lam = x / FXC;
    let mut phi = (y / FYC).abs();
    if phi >= 1.0 {
        if phi > 1.000_001 {
            return Err(OUTSIDE);
        }
        phi = if y < 0.0 { -FRAC_PI_2 } else { FRAC_PI_2 };
        lam /= f64::from(ROBIN_X[NODES][0]);
        return Ok((lam, phi));
    }
    if phi.is_nan() {
        return Err(OUTSIDE);
    }
    let mut i = (phi * NODES as f64).floor() as usize;
    if i >= NODES {
        return Err(OUTSIDE);
    }
    loop {
        if f64::from(ROBIN_Y[i][0]) > phi {
            i -= 1;
        } else if f64::from(ROBIN_Y[i + 1][0]) <= phi {
            i += 1;
        } else {
            break;
        }
    }
    let t_coefs = &ROBIN_Y[i];
    let c0 = f64::from(t_coefs[0]);
    let mut t = 5.0 * (phi - c0) / (f64::from(ROBIN_Y[i + 1][0]) - c0);
    let mut converged = false;
    for _ in 0..100 {
        let t1 = (robin_v(t_coefs, t) - phi) / robin_dv(t_coefs, t);
        t -= t1;
        if t1.abs() < 1e-10 {
            converged = true;
            break;
        }
    }
    if !converged {
        return Err(OUTSIDE);
    }
    let mut phi = (5.0 * i as f64 + t) * DEG_TO_RAD;
    if y < 0.0 {
        phi = -phi;
    }
    lam /= robin_v(&ROBIN_X[i], t);
    if lam.abs() > PI {
        return Err(OUTSIDE);
    }
    Ok((lam, phi))
}

fn vandg_fwd(lam: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
    const TOL: f64 = 1e-10;
    let mut p2 = (phi / FRAC_PI_2).abs();
    if p2 - TOL > 1.0 {
        return Err(OUTSIDE);
    }
    if p2 > 1.0 {
        p2 = 1.0;
    }
    if phi.abs() <= TOL {
        return Ok((lam, 0.0));
    }
    if lam.abs() <= TOL || (p2 - 1.0).abs() < TOL {
        let y = PI * (0.5 * p2.asin()).tan();
        return Ok((0.0, if phi < 0.0 { -y } else { y }));
    }
    let al = 0.5 * (PI / lam - lam / PI).abs();
    let al2 = al * al;
    let mut g = (1.0 - p2 * p2).sqrt();
    g /= p2 + g - 1.0;
    let g2 = g * g;
    p2 = g * (2.0 / p2 - 1.0);
    p2 *= p2;
    let mut x = g - p2;
    let g = p2 + al2;
    x = PI * (al * x + (al2 * x * x - g * (g2 - p2)).sqrt()).abs() / g;
    if lam < 0.0 {
        x = -x;
    }
    let mut y = (x / PI).abs();
    y = 1.0 - y * (y + 2.0 * al);
    if y < -TOL {
        return Err(OUTSIDE);
    }
    let y = if y < 0.0 {
        0.0
    } else {
        y.sqrt() * if phi < 0.0 { -PI } else { PI }
    };
    Ok((x, y))
}

fn vandg_inv(x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
    const TOL: f64 = 1e-10;
    const THIRD: f64 = 0.333_333_333_333_333_33;
    const C2_27: f64 = 0.074_074_074_074_074_07;
    const PI4_3: f64 = 4.188_790_204_786_391;
    const PISQ: f64 = 9.869_604_401_089_358;
    const TPISQ: f64 = 19.739_208_802_178_716;
    const HPISQ: f64 = 4.934_802_200_544_679;
    let x2 = x * x;
    let ay = y.abs();
    if ay < TOL {
        let t = x2 * x2 + TPISQ * (x2 + HPISQ);
        let lam = if x.abs() <= TOL {
            0.0
        } else {
            0.5 * (x2 - PISQ + t.sqrt()) / x
        };
        return Ok((lam, 0.0));
    }
    let y2 = y * y;
    let r = x2 + y2;
    let r2 = r * r;
    let c1 = -PI * ay * (r + PISQ);
    let c3 = r2 + 2.0 * PI * (ay * r + PI * (y2 + PI * (ay + FRAC_PI_2)));
    let mut c2 = c1 + PISQ * (r - 3.0 * y2);
    let c0 = PI * ay;
    c2 /= c3;
    let al = c1 / c3 - THIRD * c2 * c2;
    let m = 2.0 * (-THIRD * al).sqrt();
    let mut d = C2_27 * c2 * c2 * c2 + (c0 * c0 - THIRD * c2 * c1) / c3;
    let al_mul_m = al * m;
    if al_mul_m.abs() < 1e-16 {
        return Err(OUTSIDE);
    }
    d = 3.0 * d / al_mul_m;
    let t = d.abs();
    if t - TOL > 1.0 {
        return Err(OUTSIDE);
    }
    d = if t > 1.0 {
        if d > 0.0 { 0.0 } else { PI }
    } else {
        d.acos()
    };
    if r > PISQ {
        d = 2.0 * PI - d;
    }
    let mut phi = PI * (m * (d * THIRD + PI4_3).cos() - THIRD * c2);
    if y < 0.0 {
        phi = -phi;
    }
    let t = r2 + TPISQ * (x2 - y2 + HPISQ);
    let lam = if x.abs() <= TOL {
        0.0
    } else {
        0.5 * (r - PISQ + if t <= 0.0 { 0.0 } else { t.sqrt() }) / x
    };
    Ok((lam, phi))
}

fn aitoff_inv(x: f64, y: f64, winkel: Option<f64>) -> Result<(f64, f64), CoordinateError> {
    const EPSILON: f64 = 1e-12;
    const MAXITER: usize = 10;
    const MAXROUND: usize = 20;
    if x.abs() < EPSILON && y.abs() < EPSILON {
        return Ok((0.0, 0.0));
    }
    let (mut lam, mut phi) = (x, y);
    let mut round = 0;
    let mut iter;
    loop {
        iter = 0;
        loop {
            let sl = (lam * 0.5).sin();
            let cl = (lam * 0.5).cos();
            let sp = phi.sin();
            let cp = phi.cos();
            let mut d = cp * cl;
            let c = 1.0 - d * d;
            let denom = c.powf(1.5);
            if denom == 0.0 {
                return Err(OUTSIDE);
            }
            d = d.acos() / denom;
            let mut f1 = 2.0 * d * c * cp * sl;
            let mut f2 = d * c * sp;
            let mut f1p = 2.0 * (sl * cl * sp * cp / c - d * sp * sl);
            let mut f1l = cp * cp * sl * sl / c + d * cp * cl * sp * sp;
            let mut f2p = sp * sp * cl / c + d * sl * sl * cp;
            let mut f2l = 0.5 * (sp * cp * sl / c - d * sp * cp * cp * sl * cl);
            if let Some(cosphi1) = winkel {
                f1 = 0.5 * (f1 + lam * cosphi1);
                f2 = 0.5 * (f2 + phi);
                f1p *= 0.5;
                f1l = 0.5 * (f1l + cosphi1);
                f2p = 0.5 * (f2p + 1.0);
                f2l *= 0.5;
            }
            f1 -= x;
            f2 -= y;
            let det = f1p * f2l - f2p * f1l;
            let dl = ((f2 * f1p - f1 * f2p) / det) % PI;
            let dp = (f1 * f2l - f2 * f1l) / det;
            phi -= dp;
            lam -= dl;
            let more = dp.abs() > EPSILON || dl.abs() > EPSILON;
            let go_on = more && iter < MAXITER;
            iter += 1;
            if !go_on {
                break;
            }
        }
        if phi > FRAC_PI_2 {
            phi -= 2.0 * (phi - FRAC_PI_2);
        }
        if phi < -FRAC_PI_2 {
            phi -= 2.0 * (phi + FRAC_PI_2);
        }
        if (phi.abs() - FRAC_PI_2).abs() < EPSILON && winkel.is_none() {
            lam = 0.0;
        }
        let (xc, yc) = aitoff_xy(lam, phi, winkel);
        let far = (x - xc).abs() > EPSILON || (y - yc).abs() > EPSILON;
        let go_on = far && round < MAXROUND;
        round += 1;
        if !go_on {
            break;
        }
    }
    // PROJ's `iter == MAXITER && round == MAXROUND`, after its post-increments.
    if iter == MAXITER + 1 && round == MAXROUND + 1 {
        return Err(OUTSIDE);
    }
    Ok((lam, phi))
}

/// Sinusoidal on an ellipsoid (`gn_sinu.cpp`'s ellipsoidal form).
#[derive(Debug)]
pub(crate) struct EllipsoidalSinusoidal {
    lon0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
    es: f64,
    en: Mlfn,
}

impl EllipsoidalSinusoidal {
    pub(crate) fn new(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        Ok(EllipsoidalSinusoidal {
            lon0: params.get(param::LON_NATURAL_ORIGIN)?,
            fe: params.get(param::FALSE_EASTING)?,
            fn_: params.get(param::FALSE_NORTHING)?,
            a: ellipsoid.a,
            es: ellipsoid.es,
            en: Mlfn::new(ellipsoid.n),
        })
    }
}

impl Projection for EllipsoidalSinusoidal {
    fn forward(&self, lon: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = adjlon(lon - self.lon0);
        let (s, c) = phi.sin_cos();
        let y = self.en.mlfn(phi, s, c);
        let x = lam * c / (1.0 - self.es * s * s).sqrt();
        Ok((self.a * x + self.fe, self.a * y + self.fn_))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let x = (x - self.fe) / self.a;
        let y = (y - self.fn_) / self.a;
        let phi = self.en.inv_mlfn(y);
        let s = phi.abs();
        let lam = if s < FRAC_PI_2 {
            let s = phi.sin();
            x * (1.0 - self.es * s * s).sqrt() / phi.cos()
        } else if s - 1e-10 < FRAC_PI_2 {
            0.0
        } else {
            return Err(OUTSIDE);
        };
        Ok((lam + self.lon0, phi))
    }
}
