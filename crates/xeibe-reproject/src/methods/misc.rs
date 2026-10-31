//! Small spherical projections PROJ has no CRS method for, many without an
//! inverse: Apian, Ortelius, Bacon, August, Boggs, Denoyer, Ginsburg VIII,
//! Larrivée, Laskowski, Nicolosi, Transverse Central Cylindrical, Van der
//! Grinten II to IV, Airy, Chamberlin Trimetric, Rectangular Polyconic and
//! Urmaev V; and Hammer, Lagrange, Tobler-Mercator, Oblated Equal Area and
//! Oblique Cylindrical Equal Area, which have one.
//!
//! Ported from PROJ 9.8.1 `src/projections/` (MIT, see PROJ-NOTICE.md):
//! `bacon.cpp`, `august.cpp`, `boggs.cpp`, `denoy.cpp`, `gins8.cpp`,
//! `larr.cpp`, `lask.cpp`, `nicol.cpp`, `tcc.cpp`, `vandg2.cpp`,
//! `vandg4.cpp`, `airy.cpp`, `chamb.cpp`, `rpoly.cpp`, `urm5.cpp`,
//! `hammer.cpp`, `lagrng.cpp`, `tobmerc.cpp`, `oea.cpp` and `ocea.cpp`, on
//! the unit sphere.

// PROJ's constants, digit for digit.
#![allow(clippy::excessive_precision)]

use std::f64::consts::{FRAC_2_PI, FRAC_PI_2, PI, SQRT_2};

use super::adjlon;
use super::sphere::{Unit, aacos, aasin, aatan2};
use crate::error::CoordinateError;

const OUTSIDE: CoordinateError = CoordinateError::OutsideDomain;

#[derive(Debug)]
pub(crate) enum Misc {
    /// Apian Globular I, Ortelius Oval and Bacon Globular (`bacon.cpp`).
    Bacon {
        bacn: bool,
        ortl: bool,
    },
    August,
    Boggs,
    Denoyer,
    Ginsburg8,
    Larrivee,
    Laskowski,
    Nicolosi,
    TransverseCentralCylindrical,
    /// Van der Grinten II, or III.
    VanDerGrinten2 {
        vdg3: bool,
    },
    VanDerGrinten4,
    Hammer {
        w: f64,
        m: f64,
        rm: f64,
    },
    Lagrange {
        a1: f64,
        a2: f64,
        hrw: f64,
        hw: f64,
        rw: f64,
        w: f64,
    },
    Airy(Box<Airy>),
    Chamberlin(Box<Chamberlin>),
    RectangularPolyconic {
        phi0: f64,
        mode: Option<(f64, f64)>,
    },
    Urmaev5 {
        m: f64,
        rmn: f64,
        q3: f64,
        n: f64,
    },
    ToblerMercator {
        k0: f64,
    },
    OblatedEqualArea(Box<Oblated>),
    /// With PROJ's central meridian for it (`ocea` sets its own).
    ObliqueCylindricalEqualArea {
        rok: f64,
        rtk: f64,
        sinphi: f64,
        cosphi: f64,
    },
}

#[derive(Debug)]
pub(crate) struct Airy {
    p_halfpi: f64,
    sinph0: f64,
    cosph0: f64,
    cb: f64,
    /// North pole, south pole, equatorial, oblique.
    mode: u8,
    no_cut: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct Vect {
    r: f64,
    az: f64,
}

#[derive(Debug, Clone, Copy, Default)]
struct Control {
    phi: f64,
    lam: f64,
    cosphi: f64,
    sinphi: f64,
    v: Vect,
    p: (f64, f64),
}

#[derive(Debug)]
pub(crate) struct Chamberlin {
    c: [Control; 3],
    p: (f64, f64),
    beta_1: f64,
    beta_2: f64,
}

#[derive(Debug)]
pub(crate) struct Oblated {
    theta: f64,
    m: f64,
    n: f64,
    two_r_m: f64,
    two_r_n: f64,
    rm: f64,
    rn: f64,
    hm: f64,
    hn: f64,
    cp0: f64,
    sp0: f64,
}

/// Distance and azimuth between two points (`chamb.cpp`'s `vect`).
fn vect(dphi: f64, c1: f64, s1: f64, c2: f64, s2: f64, dlam: f64) -> Result<Vect, CoordinateError> {
    let cdl = dlam.cos();
    let r = if dphi.abs() > 1.0 || dlam.abs() > 1.0 {
        aacos(s1 * s2 + c1 * c2 * cdl)?
    } else {
        // More accurate for small distances.
        let dp = (0.5 * dphi).sin();
        let dl = (0.5 * dlam).sin();
        2.0 * aasin((dp * dp + c1 * c2 * dl * dl).sqrt())?
    };
    Ok(if r.abs() > 1e-9 {
        Vect {
            r,
            az: (c2 * dlam.sin()).atan2(c1 * s2 - s1 * c2 * cdl),
        }
    } else {
        Vect { r: 0.0, az: 0.0 }
    })
}

/// The angle opposite `a` in a triangle of sides `b`, `c`, `a` (`lc`).
fn lc(b: f64, c: f64, a: f64) -> Result<f64, CoordinateError> {
    aacos(0.5 * (b * b + c * c - a * a) / (b * c))
}

impl Misc {
    /// The projection of a PROJ name with its keys (`W`, `M`, `lat_1`, in
    /// SI units), `None` for another.
    pub(crate) fn of(
        proj: &str,
        key: impl Fn(&str) -> Option<f64>,
        flag: impl Fn(&str) -> bool,
    ) -> Result<Option<Misc>, String> {
        let k = |name: &str| key(name).unwrap_or(0.0);
        let phi0 = k("lat_0");
        Ok(Some(match proj {
            "airy" => {
                const EPS: f64 = 1e-10;
                let beta = 0.5 * (FRAC_PI_2 - k("lat_b"));
                let cb = if beta.abs() < EPS {
                    -0.5
                } else {
                    let cb = 1.0 / beta.tan();
                    cb * cb * beta.cos().ln()
                };
                let (mode, p_halfpi) = if (phi0.abs() - FRAC_PI_2).abs() < EPS {
                    if phi0 < 0.0 {
                        (1, -FRAC_PI_2)
                    } else {
                        (0, FRAC_PI_2)
                    }
                } else if phi0.abs() < EPS {
                    (2, 0.0)
                } else {
                    (3, 0.0)
                };
                Misc::Airy(Box::new(Airy {
                    p_halfpi,
                    sinph0: phi0.sin(),
                    cosph0: phi0.cos(),
                    cb,
                    mode,
                    no_cut: flag("no_cut"),
                }))
            }
            "chamb" => {
                let lam0 = k("lon_0");
                let mut c = [Control::default(); 3];
                for (i, ci) in c.iter_mut().enumerate() {
                    ci.phi = k(&format!("lat_{}", i + 1));
                    ci.lam = adjlon(k(&format!("lon_{}", i + 1)) - lam0);
                    ci.cosphi = ci.phi.cos();
                    ci.sinphi = ci.phi.sin();
                }
                let fail = |_| "Invalid value for control points".to_owned();
                for i in 0..3 {
                    let j = if i == 2 { 0 } else { i + 1 };
                    c[i].v = vect(
                        c[j].phi - c[i].phi,
                        c[i].cosphi,
                        c[i].sinphi,
                        c[j].cosphi,
                        c[j].sinphi,
                        c[j].lam - c[i].lam,
                    )
                    .map_err(fail)?;
                    if c[i].v.r == 0.0 {
                        return Err(
                            "Invalid value for control points: they should be distinct".into()
                        );
                    }
                }
                let beta_0 = lc(c[0].v.r, c[2].v.r, c[1].v.r).map_err(fail)?;
                let beta_1 = lc(c[0].v.r, c[1].v.r, c[2].v.r).map_err(fail)?;
                c[0].p.1 = c[2].v.r * beta_0.sin();
                c[1].p.1 = c[0].p.1;
                c[2].p.1 = 0.0;
                c[1].p.0 = 0.5 * c[0].v.r;
                c[0].p.0 = -c[1].p.0;
                c[2].p.0 = c[0].p.0 + c[2].v.r * beta_0.cos();
                Misc::Chamberlin(Box::new(Chamberlin {
                    p: (c[2].p.0, 2.0 * c[0].p.1),
                    c,
                    beta_1,
                    beta_2: PI - beta_0,
                }))
            }
            "rpoly" => {
                let phi1 = k("lat_ts").abs();
                Misc::RectangularPolyconic {
                    phi0,
                    mode: (phi1 > 1e-9).then(|| {
                        let fxb = 0.5 * phi1.sin();
                        (0.5 / fxb, fxb)
                    }),
                }
            }
            "urm5" => {
                let n = key("n").ok_or("Missing parameter n.")?;
                if n <= 0.0 || n > 1.0 {
                    return Err("Invalid value for n: it should be in ]0,1] range.".into());
                }
                let alpha = k("alpha");
                let t = n * alpha.sin();
                let denom = (1.0 - t * t).sqrt();
                if denom == 0.0 {
                    return Err(
                        "Invalid value for n / alpha: n * sin(|alpha|) should be < 1.".into(),
                    );
                }
                let m = alpha.cos() / denom;
                Misc::Urmaev5 {
                    m,
                    rmn: 1.0 / (m * n),
                    q3: k("q") / 3.0,
                    n,
                }
            }
            "tobmerc" => Misc::ToblerMercator {
                k0: key("k_0").or_else(|| key("k")).unwrap_or(1.0),
            },
            "oea" => {
                let n = k("n");
                if n <= 0.0 {
                    return Err("Invalid value for n: it should be > 0".into());
                }
                let m = k("m");
                if m <= 0.0 {
                    return Err("Invalid value for m: it should be > 0".into());
                }
                Misc::OblatedEqualArea(Box::new(Oblated {
                    theta: k("theta"),
                    m,
                    n,
                    two_r_m: 2.0 / m,
                    two_r_n: 2.0 / n,
                    rm: 1.0 / m,
                    rn: 1.0 / n,
                    hm: 0.5 * m,
                    hn: 0.5 * n,
                    cp0: phi0.cos(),
                    sp0: phi0.sin(),
                }))
            }
            "apian" => Misc::Bacon {
                bacn: false,
                ortl: false,
            },
            "ortel" => Misc::Bacon {
                bacn: false,
                ortl: true,
            },
            "bacon" => Misc::Bacon {
                bacn: true,
                ortl: false,
            },
            "august" => Misc::August,
            "boggs" => Misc::Boggs,
            "denoy" => Misc::Denoyer,
            "gins8" => Misc::Ginsburg8,
            "larr" => Misc::Larrivee,
            "lask" => Misc::Laskowski,
            "nicol" => Misc::Nicolosi,
            "tcc" => Misc::TransverseCentralCylindrical,
            "vandg2" => Misc::VanDerGrinten2 { vdg3: false },
            "vandg3" => Misc::VanDerGrinten2 { vdg3: true },
            "vandg4" => Misc::VanDerGrinten4,
            "hammer" => {
                let w = match key("W") {
                    Some(w) if w.abs() <= 0.0 => {
                        return Err("Invalid value for W: it should be > 0".into());
                    }
                    Some(w) => w.abs(),
                    None => 0.5,
                };
                let m = match key("M") {
                    Some(m) if m.abs() <= 0.0 => {
                        return Err("Invalid value for M: it should be > 0".into());
                    }
                    Some(m) => m.abs(),
                    None => 1.0,
                };
                Misc::Hammer {
                    w,
                    m: m / w,
                    rm: 1.0 / m,
                }
            }
            "lagrng" => {
                let w = key("W").unwrap_or(2.0);
                if w <= 0.0 {
                    return Err("Invalid value for W: it should be > 0".into());
                }
                let sin_phi1 = key("lat_1").unwrap_or(0.0).sin();
                if (sin_phi1.abs() - 1.0).abs() < 1e-10 {
                    return Err("Invalid value for lat_1: |lat_1| should be < 90°".into());
                }
                let rw = 1.0 / w;
                let hrw = 0.5 * rw;
                let a1 = ((1.0 - sin_phi1) / (1.0 + sin_phi1)).powf(hrw);
                Misc::Lagrange {
                    a1,
                    a2: a1 * a1,
                    hrw,
                    hw: 0.5 * w,
                    rw,
                    w,
                }
            }
            _ => return Ok(None),
        }))
    }
}

impl Unit for Misc {
    fn fwd(&self, mut lam: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        Ok(match *self {
            Misc::Airy(ref q) => airy(q, lam, phi)?,
            Misc::Chamberlin(ref q) => chamberlin(q, lam, phi)?,
            Misc::RectangularPolyconic { phi0, mode } => {
                let fa = match mode {
                    Some((fxa, fxb)) => (lam * fxb).tan() * fxa,
                    None => 0.5 * lam,
                };
                if phi.abs() < 1e-9 {
                    (fa + fa, -phi0)
                } else {
                    let y = 1.0 / phi.tan();
                    let fa = 2.0 * (fa * phi.sin()).atan();
                    (fa.sin() * y, phi - phi0 + (1.0 - fa.cos()) * y)
                }
            }
            Misc::Urmaev5 { m, rmn, q3, n } => {
                let phi = aasin(n * phi.sin())?;
                let t = phi * phi;
                (m * lam * phi.cos(), phi * (1.0 + t * q3) * rmn)
            }
            Misc::ToblerMercator { k0 } => {
                if phi.abs() >= FRAC_PI_2 {
                    return Err(OUTSIDE);
                }
                let cosphi = phi.cos();
                (k0 * lam * cosphi * cosphi, k0 * phi.tan().asinh())
            }
            Misc::OblatedEqualArea(ref q) => {
                let (sp, cp) = phi.sin_cos();
                let cl = lam.cos();
                let az = aatan2(cp * lam.sin(), q.cp0 * sp - q.sp0 * cp * cl) + q.theta;
                let shz = (0.5 * aacos(q.sp0 * sp + q.cp0 * cp * cl)?).sin();
                let m = aasin(shz * az.sin())?;
                let n = aasin(shz * az.cos() * m.cos() / (m * q.two_r_m).cos())?;
                (
                    q.m * (m * q.two_r_m).sin() * n.cos() / (n * q.two_r_n).cos(),
                    q.n * (n * q.two_r_n).sin(),
                )
            }
            Misc::ObliqueCylindricalEqualArea {
                rok,
                rtk,
                sinphi,
                cosphi,
            } => {
                let y = lam.sin();
                let t = lam.cos();
                let mut x = ((phi.tan() * cosphi + sinphi * y) / t).atan();
                if t < 0.0 {
                    x += PI;
                }
                (x * rtk, rok * (sinphi * phi.sin() - cosphi * phi.cos() * y))
            }
            Misc::Bacon { bacn, ortl } => {
                const HLFPI2: f64 = 2.46740110027233965467;
                const EPS: f64 = 1e-10;
                let y = if bacn { FRAC_PI_2 * phi.sin() } else { phi };
                let ax = lam.abs();
                let x = if ax >= EPS {
                    let x = if ortl && ax >= FRAC_PI_2 {
                        (HLFPI2 - phi * phi + EPS).sqrt() + ax - FRAC_PI_2
                    } else {
                        let f = 0.5 * (HLFPI2 / ax + ax);
                        ax - f + (f * f - y * y).sqrt()
                    };
                    if lam < 0.0 { -x } else { x }
                } else {
                    0.0
                };
                (x, y)
            }
            Misc::August => {
                const M: f64 = 1.333333333333333;
                let t = (0.5 * phi).tan();
                let c1 = (1.0 - t * t).sqrt();
                lam *= 0.5;
                let c = 1.0 + c1 * lam.cos();
                let x1 = lam.sin() * c1 / c;
                let y1 = t / c;
                let (x12, y12) = (x1 * x1, y1 * y1);
                (
                    M * x1 * (3.0 + x12 - 3.0 * y12),
                    M * y1 * (3.0 + 3.0 * x12 - y12),
                )
            }
            Misc::Boggs => {
                const EPS: f64 = 1e-7;
                const FXC: f64 = 2.00276;
                const FXC2: f64 = 1.11072;
                const FYC: f64 = 0.49931;
                let mut theta = phi;
                let x = if (phi.abs() - FRAC_PI_2).abs() < EPS {
                    0.0
                } else {
                    let c = theta.sin() * PI;
                    for _ in 0..20 {
                        let th1 = (theta + theta.sin() - c) / (1.0 + theta.cos());
                        theta -= th1;
                        if th1.abs() < EPS {
                            break;
                        }
                    }
                    theta *= 0.5;
                    FXC * lam / (1.0 / phi.cos() + FXC2 / theta.cos())
                };
                (x, FYC * (phi + SQRT_2 * theta.sin()))
            }
            Misc::Denoyer => {
                const C0: f64 = 0.95;
                const C1: f64 = -0.08333333333333333333;
                const C3: f64 = 0.00166666666666666666;
                const D1: f64 = 0.9;
                const D5: f64 = 0.03;
                let l = lam.abs();
                (
                    lam * ((C0 + l * (C1 + l * l * C3))
                        * (phi * (D1 + D5 * phi * phi * phi * phi)))
                        .cos(),
                    phi,
                )
            }
            Misc::Ginsburg8 => {
                const CL: f64 = 0.000952426;
                const CP: f64 = 0.162388;
                const C12: f64 = 0.08333333333333333;
                let t = phi * phi;
                let y = phi * (1.0 + t * C12);
                let x = lam * (1.0 - CP * t);
                let t = lam * lam;
                (x * (0.87 - CL * t * t), y)
            }
            Misc::Larrivee => {
                const SIXTH: f64 = 0.16666666666666666;
                (
                    0.5 * lam * (1.0 + phi.cos().sqrt()),
                    phi / ((0.5 * phi).cos() * (SIXTH * lam).cos()),
                )
            }
            Misc::Laskowski => {
                let (l2, p2) = (lam * lam, phi * phi);
                (
                    lam * (0.975534 + p2 * (-0.119161 + l2 * -0.0143059 + p2 * -0.0547009)),
                    phi * (1.00384
                        + l2 * (0.0802894 + p2 * -0.0285500 + l2 * 0.000199025)
                        + p2 * (0.0998909 + p2 * -0.0491032)),
                )
            }
            Misc::Nicolosi => nicolosi(lam, phi),
            Misc::TransverseCentralCylindrical => {
                let b = phi.cos() * lam.sin();
                let bt = 1.0 - b * b;
                if bt < 1e-10 {
                    return Err(OUTSIDE);
                }
                (b / bt.sqrt(), phi.tan().atan2(lam.cos()))
            }
            Misc::VanDerGrinten2 { vdg3 } => {
                const TOL: f64 = 1e-10;
                let bt = (FRAC_2_PI * phi).abs();
                let ct = 1.0 - bt * bt;
                let ct = if ct < 0.0 { 0.0 } else { ct.sqrt() };
                if lam.abs() < TOL {
                    (0.0, PI * if phi < 0.0 { -bt } else { bt } / (1.0 + ct))
                } else {
                    let at = 0.5 * (PI / lam - lam / PI).abs();
                    let (mut x, mut y) = if vdg3 {
                        let x1 = bt / (1.0 + ct);
                        (PI * ((at * at + 1.0 - x1 * x1).sqrt() - at), PI * x1)
                    } else {
                        let x1 = (ct * (1.0 + at * at).sqrt() - at * ct * ct)
                            / (1.0 + at * at * bt * bt);
                        (PI * x1, PI * (1.0 - x1 * (x1 + 2.0 * at) + TOL).sqrt())
                    };
                    if lam < 0.0 {
                        x = -x;
                    }
                    if phi < 0.0 {
                        y = -y;
                    }
                    (x, y)
                }
            }
            Misc::VanDerGrinten4 => vandg4(lam, phi),
            Misc::Hammer { w, m, rm } => {
                let cosphi = phi.cos();
                lam *= w;
                let denom = 1.0 + cosphi * lam.cos();
                if denom == 0.0 {
                    return Err(OUTSIDE);
                }
                let d = (2.0 / denom).sqrt();
                (m * d * cosphi * lam.sin(), rm * d * phi.sin())
            }
            Misc::Lagrange { a1, hrw, rw, .. } => {
                let sin_phi = phi.sin();
                if (sin_phi.abs() - 1.0).abs() < 1e-10 {
                    (0.0, if phi < 0.0 { -2.0 } else { 2.0 })
                } else {
                    let v = a1 * ((1.0 + sin_phi) / (1.0 - sin_phi)).powf(hrw);
                    lam *= rw;
                    let c = 0.5 * (v + 1.0 / v) + lam.cos();
                    if c < 1e-10 {
                        return Err(OUTSIDE);
                    }
                    (2.0 * lam.sin() / c, (v - 1.0 / v) / c)
                }
            }
        })
    }

    fn inv(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        match *self {
            Misc::ToblerMercator { k0 } => {
                let phi = (y / k0).sinh().atan();
                let cosphi = phi.cos();
                Ok((x / k0 / (cosphi * cosphi), phi))
            }
            Misc::OblatedEqualArea(ref q) => {
                let n = q.hn * aasin(y * q.rn)?;
                let m = q.hm * aasin(x * q.rm * (n * q.two_r_n).cos() / n.cos())?;
                let xp = 2.0 * m.sin();
                let yp = 2.0 * n.sin() * (m * q.two_r_m).cos() / m.cos();
                let az = aatan2(xp, yp) - q.theta;
                let c_az = az.cos();
                let z = 2.0 * aasin(0.5 * xp.hypot(yp))?;
                let (sz, cz) = z.sin_cos();
                Ok((
                    aatan2(sz * az.sin(), q.cp0 * cz - q.sp0 * sz * c_az),
                    aasin(q.sp0 * cz + q.cp0 * sz * c_az)?,
                ))
            }
            Misc::ObliqueCylindricalEqualArea {
                rok,
                rtk,
                sinphi,
                cosphi,
            } => {
                let y = y / rok;
                let x = x / rtk;
                let t = (1.0 - y * y).sqrt();
                let s = x.sin();
                Ok((
                    (t * sinphi * s - y * cosphi).atan2(t * x.cos()),
                    (y * sinphi + t * cosphi * s).asin(),
                ))
            }
            Misc::Hammer { w, .. } => {
                let z = (1.0 - 0.25 * w * w * x * x - 0.25 * y * y).sqrt();
                if (2.0 * z * z - 1.0).abs() < 1e-10 {
                    return Err(OUTSIDE);
                }
                Ok((aatan2(w * x * z, 2.0 * z * z - 1.0) / w, aasin(z * y)?))
            }
            Misc::Lagrange { a2, hw, w, .. } => {
                if (y.abs() - 2.0).abs() < 1e-10 {
                    return Ok((0.0, if y < 0.0 { -FRAC_PI_2 } else { FRAC_PI_2 }));
                }
                let x2 = x * x;
                let y2p = 2.0 + y;
                let y2m = 2.0 - y;
                let c = y2p * y2m - x2;
                if c.abs() < 1e-10 {
                    return Err(OUTSIDE);
                }
                let phi =
                    2.0 * ((y2p * y2p + x2) / (a2 * (y2m * y2m + x2))).powf(hw).atan() - FRAC_PI_2;
                Ok((w * (4.0 * x).atan2(c), phi))
            }
            // PROJ has no inverse.
            _ => Err(OUTSIDE),
        }
    }
}

fn airy(q: &Airy, lam: f64, mut phi: f64) -> Result<(f64, f64), CoordinateError> {
    const EPS: f64 = 1e-10;
    let (sinlam, coslam) = lam.sin_cos();
    if q.mode >= 2 {
        let (sinphi, cosphi) = phi.sin_cos();
        let mut cosz = cosphi * coslam;
        if q.mode == 3 {
            cosz = q.sinph0 * sinphi + q.cosph0 * cosz;
        }
        if !q.no_cut && cosz < -EPS {
            return Err(OUTSIDE);
        }
        let s = 1.0 - cosz;
        let krho = if s.abs() > EPS {
            let t = 0.5 * (1.0 + cosz);
            if t == 0.0 {
                return Err(OUTSIDE);
            }
            -t.ln() / s - q.cb / t
        } else {
            0.5 - q.cb
        };
        let y = if q.mode == 3 {
            krho * (q.cosph0 * sinphi - q.sinph0 * cosphi * coslam)
        } else {
            krho * sinphi
        };
        return Ok((krho * cosphi * sinlam, y));
    }
    phi = (q.p_halfpi - phi).abs();
    if !q.no_cut && phi - EPS > FRAC_PI_2 {
        return Err(OUTSIDE);
    }
    phi *= 0.5;
    if phi > EPS {
        let t = phi.tan();
        let krho = -2.0 * (phi.cos().ln() / t + t * q.cb);
        let y = krho * coslam;
        Ok((krho * sinlam, if q.mode == 0 { -y } else { y }))
    } else {
        Ok((0.0, 0.0))
    }
}

// PROJ's loops over the three control points, which index two arrays.
#[allow(clippy::needless_range_loop)]
fn chamberlin(q: &Chamberlin, lam: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
    const THIRD: f64 = 0.333333333333333333;
    let (sinphi, cosphi) = phi.sin_cos();
    let mut v = [Vect::default(); 3];
    for i in 0..3 {
        let c = &q.c[i];
        v[i] = vect(phi - c.phi, c.cosphi, c.sinphi, cosphi, sinphi, lam - c.lam)?;
        // At a control point.
        if v[i].r == 0.0 {
            return Ok(c.p);
        }
        v[i].az = adjlon(v[i].az - c.v.az);
    }
    // The mean of the arcs' intercepts.
    let (mut x, mut y) = q.p;
    for i in 0..3 {
        let j = if i == 2 { 0 } else { i + 1 };
        let mut a = lc(q.c[i].v.r, v[i].r, v[j].r)?;
        if v[i].az < 0.0 {
            a = -a;
        }
        match i {
            0 => {
                x += v[i].r * a.cos();
                y -= v[i].r * a.sin();
            }
            1 => {
                let a = q.beta_1 - a;
                x -= v[i].r * a.cos();
                y -= v[i].r * a.sin();
            }
            _ => {
                let a = q.beta_2 - a;
                x += v[i].r * a.cos();
                y += v[i].r * a.sin();
            }
        }
    }
    Ok((x * THIRD, y * THIRD))
}

fn nicolosi(lam: f64, phi: f64) -> (f64, f64) {
    const EPS: f64 = 1e-10;
    if lam.abs() < EPS {
        return (0.0, phi);
    }
    if phi.abs() < EPS {
        return (lam, 0.0);
    }
    if (lam.abs() - FRAC_PI_2).abs() < EPS {
        return (lam * phi.cos(), FRAC_PI_2 * phi.sin());
    }
    if (phi.abs() - FRAC_PI_2).abs() < EPS {
        return (0.0, phi);
    }
    let tb = FRAC_PI_2 / lam - lam / FRAC_PI_2;
    let c = phi / FRAC_PI_2;
    let sp = phi.sin();
    let d = (1.0 - c * c) / (sp - c);
    let r2 = (tb / d) * (tb / d);
    let m = (tb * sp / d - 0.5 * tb) / (1.0 + r2);
    let n = (sp / r2 + 0.5 * d) / (1.0 + 1.0 / r2);
    let x = phi.cos();
    let x = (m * m + x * x / (1.0 + r2)).sqrt();
    let x = FRAC_PI_2 * (m + if lam < 0.0 { -x } else { x });
    let y = (n * n - (sp * sp / r2 + d * sp - 1.0) / (1.0 + 1.0 / r2)).sqrt();
    let y = FRAC_PI_2 * (n + if phi < 0.0 { y } else { -y });
    (x, y)
}

fn vandg4(lam: f64, phi: f64) -> (f64, f64) {
    const TOL: f64 = 1e-10;
    if phi.abs() < TOL {
        return (lam, 0.0);
    }
    if lam.abs() < TOL || (phi.abs() - FRAC_PI_2).abs() < TOL {
        return (0.0, phi);
    }
    let bt = (FRAC_2_PI * phi).abs();
    let bt2 = bt * bt;
    let ct = 0.5 * (bt * (8.0 - bt * (2.0 + bt2)) - 5.0) / (bt2 * (bt - 1.0));
    let ct2 = ct * ct;
    let mut dt = FRAC_2_PI * lam;
    dt += 1.0 / dt;
    dt = (dt * dt - 4.0).sqrt();
    if lam.abs() - FRAC_PI_2 < 0.0 {
        dt = -dt;
    }
    let dt2 = dt * dt;
    let mut x1 = bt + ct;
    x1 *= x1;
    let t = bt + 3.0 * ct;
    let ft = x1 * (bt2 + ct2 * dt2 - 1.0)
        + (1.0 - bt2) * (bt2 * (t * t + 4.0 * ct2) + ct2 * (12.0 * bt * ct + 4.0 * ct2));
    let x1 = (dt * (x1 + ct2 - 1.0) + 2.0 * ft.sqrt()) / (4.0 * x1 + dt2);
    let mut x = FRAC_PI_2 * x1;
    let mut y = FRAC_PI_2 * (1.0 + dt * x1.abs() - x1 * x1).sqrt();
    if lam < 0.0 {
        x = -x;
    }
    if phi < 0.0 {
        y = -y;
    }
    (x, y)
}
