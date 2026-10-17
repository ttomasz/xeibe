//! Geodesics on an ellipsoid of revolution: the inverse problem (distance and
//! initial azimuth between two points) and the direct problem (the point at a
//! given distance and azimuth), for the exact Azimuthal Equidistant.
//!
//! Ported from `geodesic.c` in PROJ 9.8.1 (MIT, see PROJ-NOTICE.md), which is
//! C. F. F. Karney's C library of the algorithms in "Algorithms for geodesics",
//! J. Geodesy 87, 43–55 (2013), part of GeographicLib (MIT,
//! <https://geographiclib.sourceforge.io/>). Only what the projection needs is
//! kept: no areas, reduced lengths or geodesic scales, no geodesic lines.
//! Angles are in degrees at this interface, as in the original.

use std::f64::consts::PI;

const ORDER: usize = 6;
const MAXIT1: u32 = 20;
const MAXIT2: u32 = MAXIT1 + 53 + 10;

const TOL0: f64 = f64::EPSILON;
const TOL1: f64 = 200.0 * TOL0;
const TOLB: f64 = TOL0;
const DEGREE: f64 = PI / 180.0;

fn tiny() -> f64 {
    f64::MIN_POSITIVE.sqrt()
}
fn tol2() -> f64 {
    TOL0.sqrt()
}
fn xthresh() -> f64 {
    1000.0 * tol2()
}

fn sq(x: f64) -> f64 {
    x * x
}

/// Σ p\[i\] xⁱ with p\[0\] the highest power (the C library's `polyvalx`);
/// uses `n + 1` coefficients.
fn polyvalx(n: usize, p: &[f64], x: f64) -> f64 {
    p[..=n].iter().fold(0.0, |y, &c| y * x + c)
}

fn norm2(sinx: &mut f64, cosx: &mut f64) {
    let r = sinx.hypot(*cosx);
    *sinx /= r;
    *cosx /= r;
}

/// IEEE remainder: x − n y with n the integer nearest x / y (ties to even).
fn remainder(x: f64, y: f64) -> f64 {
    let n = (x / y).round_ties_even();
    (-n).mul_add(y, x)
}

fn ang_normalize(x: f64) -> f64 {
    let y = remainder(x, 360.0);
    if y.abs() == 180.0 {
        180f64.copysign(x)
    } else {
        y
    }
}

fn lat_fix(x: f64) -> f64 {
    if x.abs() > 90.0 { f64::NAN } else { x }
}

/// Error-free sum: (s, t) with u + v = s + t exactly.
fn sumx(u: f64, v: f64) -> (f64, f64) {
    let s = u + v;
    let mut up = s - v;
    let mut vpp = s - up;
    up -= u;
    vpp -= v;
    let t = if s != 0.0 { 0.0 - (up + vpp) } else { s };
    (s, t)
}

/// y − x reduced to [−180, 180] with its rounding error.
fn ang_diff(x: f64, y: f64) -> (f64, f64) {
    let (mut d, mut t) = sumx(remainder(-x, 360.0), remainder(y, 360.0));
    (d, t) = sumx(remainder(d, 360.0), t);
    if d == 0.0 || d.abs() == 180.0 {
        d = d.copysign(if t == 0.0 { y - x } else { -t });
    }
    (d, t)
}

fn ang_round(x: f64) -> f64 {
    const Z: f64 = 1.0 / 16.0;
    let mut y = x.abs();
    let w = Z - y;
    y = if w > 0.0 { Z - w } else { y };
    y.copysign(x)
}

/// Sine and cosine of x degrees (plus the correction t), exact at multiples
/// of 90°.
fn sincosde(x: f64, t: f64) -> (f64, f64) {
    let n = (x / 90.0).round_ties_even();
    let r = (-n).mul_add(90.0, x);
    let q = (n as i64).rem_euclid(4);
    let r = ang_round(r + t) * DEGREE;
    let (s, c) = r.sin_cos();
    let (mut sinx, mut cosx) = match q {
        0 => (s, c),
        1 => (c, -s),
        2 => (-s, -c),
        _ => (-c, s),
    };
    cosx += 0.0;
    if sinx == 0.0 {
        sinx = sinx.copysign(x);
    }
    (sinx, cosx)
}

fn sincosdx(x: f64) -> (f64, f64) {
    sincosde(x, 0.0)
}

fn atan2dx(y: f64, x: f64) -> f64 {
    let (mut y, mut x) = (y, x);
    let mut q = 0;
    if y.abs() > x.abs() {
        std::mem::swap(&mut x, &mut y);
        q = 2;
    }
    if x.is_sign_negative() {
        x = -x;
        q += 1;
    }
    let ang = y.atan2(x) / DEGREE;
    match q {
        1 => 180f64.copysign(y) - ang,
        2 => 90.0 - ang,
        3 => -90.0 + ang,
        _ => ang,
    }
}

/// y = Σ c\[i\] sin(2 i x) (sinp, i = 1..n) or Σ c\[i\] cos((2 i + 1) x)
/// (i = 0..n−1) by Clenshaw summation.
fn sin_cos_series(sinp: bool, sinx: f64, cosx: f64, c: &[f64], n: usize) -> f64 {
    let mut idx = n + sinp as usize;
    let ar = 2.0 * (cosx - sinx) * (cosx + sinx);
    let mut y0 = if n & 1 == 1 {
        idx -= 1;
        c[idx]
    } else {
        0.0
    };
    let mut y1 = 0.0;
    for _ in 0..n / 2 {
        idx -= 1;
        y1 = ar * y0 - y1 + c[idx];
        idx -= 1;
        y0 = ar * y1 - y0 + c[idx];
    }
    if sinp {
        2.0 * sinx * cosx * y0
    } else {
        cosx * (y0 - y1)
    }
}

fn a1m1f(eps: f64) -> f64 {
    const COEFF: [f64; 5] = [1.0, 4.0, 64.0, 0.0, 256.0];
    let m = ORDER / 2;
    let t = polyvalx(m, &COEFF, sq(eps)) / COEFF[m + 1];
    (t + eps) / (1.0 - eps)
}

fn a2m1f(eps: f64) -> f64 {
    const COEFF: [f64; 5] = [-11.0, -28.0, -192.0, 0.0, 256.0];
    let m = ORDER / 2;
    let t = polyvalx(m, &COEFF, sq(eps)) / COEFF[m + 1];
    (t - eps) / (1.0 + eps)
}

/// The Fourier coefficients c\[1..=6\] from a table of polynomials in ε².
fn fourier_c(eps: f64, coeff: &[f64]) -> [f64; ORDER + 1] {
    let mut c = [0.0; ORDER + 1];
    let eps2 = sq(eps);
    let mut d = eps;
    let mut o = 0;
    for (l, slot) in c.iter_mut().enumerate().skip(1) {
        let m = (ORDER - l) / 2;
        *slot = d * polyvalx(m, &coeff[o..], eps2) / coeff[o + m + 1];
        o += m + 2;
        d *= eps;
    }
    c
}

fn c1f(eps: f64) -> [f64; ORDER + 1] {
    const COEFF: [f64; 18] = [
        -1.0, 6.0, -16.0, 32.0, -9.0, 64.0, -128.0, 2048.0, 9.0, -16.0, 768.0, 3.0, -5.0, 512.0,
        -7.0, 1280.0, -7.0, 2048.0,
    ];
    fourier_c(eps, &COEFF)
}

fn c1pf(eps: f64) -> [f64; ORDER + 1] {
    const COEFF: [f64; 18] = [
        205.0, -432.0, 768.0, 1536.0, 4005.0, -4736.0, 3840.0, 12288.0, -225.0, 116.0, 384.0,
        -7173.0, 2695.0, 7680.0, 3467.0, 7680.0, 38081.0, 61440.0,
    ];
    fourier_c(eps, &COEFF)
}

fn c2f(eps: f64) -> [f64; ORDER + 1] {
    const COEFF: [f64; 18] = [
        1.0, 2.0, 16.0, 32.0, 35.0, 64.0, 384.0, 2048.0, 15.0, 80.0, 768.0, 7.0, 35.0, 512.0, 63.0,
        1280.0, 77.0, 2048.0,
    ];
    fourier_c(eps, &COEFF)
}

/// A geodesic problem on one ellipsoid.
#[derive(Debug, Clone)]
pub(crate) struct Geodesic {
    a: f64,
    f: f64,
    f1: f64,
    ep2: f64,
    n: f64,
    b: f64,
    etol2: f64,
    a3x: [f64; ORDER],
    c3x: [f64; (ORDER * (ORDER - 1)) / 2],
}

impl Geodesic {
    /// For semi-major axis `a` and flattening `f` (0 for a sphere).
    pub(crate) fn new(a: f64, f: f64) -> Self {
        let f1 = 1.0 - f;
        let e2 = f * (2.0 - f);
        let n = f / (2.0 - f);
        let etol2 = 0.1 * tol2() / (f.abs().max(0.001) * (1.0f64).min(1.0 - f / 2.0) / 2.0).sqrt();
        let mut g = Geodesic {
            a,
            f,
            f1,
            ep2: e2 / sq(f1),
            n,
            b: a * f1,
            etol2,
            a3x: [0.0; ORDER],
            c3x: [0.0; (ORDER * (ORDER - 1)) / 2],
        };
        g.a3_coeff();
        g.c3_coeff();
        g
    }

    fn a3_coeff(&mut self) {
        const COEFF: [f64; 18] = [
            -3.0, 128.0, -2.0, -3.0, 64.0, -1.0, -3.0, -1.0, 16.0, 3.0, -1.0, -2.0, 8.0, 1.0, -1.0,
            2.0, 1.0, 1.0,
        ];
        let (mut o, mut k) = (0, 0);
        for j in (0..ORDER).rev() {
            let m = (ORDER - j - 1).min(j);
            self.a3x[k] = polyvalx(m, &COEFF[o..], self.n) / COEFF[o + m + 1];
            k += 1;
            o += m + 2;
        }
    }

    fn c3_coeff(&mut self) {
        const COEFF: [f64; 45] = [
            3.0, 128.0, 2.0, 5.0, 128.0, -1.0, 3.0, 3.0, 64.0, -1.0, 0.0, 1.0, 8.0, -1.0, 1.0, 4.0,
            5.0, 256.0, 1.0, 3.0, 128.0, -3.0, -2.0, 3.0, 64.0, 1.0, -3.0, 2.0, 32.0, 7.0, 512.0,
            -10.0, 9.0, 384.0, 5.0, -9.0, 5.0, 192.0, 7.0, 512.0, -14.0, 7.0, 512.0, 21.0, 2560.0,
        ];
        let (mut o, mut k) = (0, 0);
        for l in 1..ORDER {
            for j in (l..ORDER).rev() {
                let m = (ORDER - j - 1).min(j);
                self.c3x[k] = polyvalx(m, &COEFF[o..], self.n) / COEFF[o + m + 1];
                k += 1;
                o += m + 2;
            }
        }
    }

    fn a3f(&self, eps: f64) -> f64 {
        polyvalx(ORDER - 1, &self.a3x, eps)
    }

    /// C3\[1..=5\].
    fn c3f(&self, eps: f64) -> [f64; ORDER] {
        let mut c = [0.0; ORDER];
        let mut mult = 1.0;
        let mut o = 0;
        for (l, slot) in c.iter_mut().enumerate().skip(1) {
            let m = ORDER - l - 1;
            mult *= eps;
            *slot = mult * polyvalx(m, &self.c3x[o..], eps);
            o += m + 1;
        }
        c
    }

    /// Distance over b, reduced length over b and m0, as asked.
    #[allow(clippy::too_many_arguments)]
    fn lengths(
        &self,
        eps: f64,
        sig12: f64,
        (ssig1, csig1, dn1): (f64, f64, f64),
        (ssig2, csig2, dn2): (f64, f64, f64),
        want_s: bool,
        want_m: bool,
    ) -> (f64, f64, f64) {
        let (mut m0, mut j12, mut a1, mut a2) = (0.0, 0.0, 0.0, 0.0);
        let mut ca = [0.0; ORDER + 1];
        let mut cb = [0.0; ORDER + 1];
        if want_s || want_m {
            a1 = a1m1f(eps);
            ca = c1f(eps);
            if want_m {
                a2 = a2m1f(eps);
                cb = c2f(eps);
                m0 = a1 - a2;
                a2 += 1.0;
            }
            a1 += 1.0;
        }
        let mut s12b = 0.0;
        if want_s {
            let b1 = sin_cos_series(true, ssig2, csig2, &ca, ORDER)
                - sin_cos_series(true, ssig1, csig1, &ca, ORDER);
            s12b = a1 * (sig12 + b1);
            if want_m {
                let b2 = sin_cos_series(true, ssig2, csig2, &cb, ORDER)
                    - sin_cos_series(true, ssig1, csig1, &cb, ORDER);
                j12 = m0 * sig12 + (a1 * b1 - a2 * b2);
            }
        } else if want_m {
            for l in 1..=ORDER {
                cb[l] = a1 * ca[l] - a2 * cb[l];
            }
            j12 = m0 * sig12
                + (sin_cos_series(true, ssig2, csig2, &cb, ORDER)
                    - sin_cos_series(true, ssig1, csig1, &cb, ORDER));
        }
        let m12b = dn2 * (csig1 * ssig2) - dn1 * (ssig1 * csig2) - csig1 * csig2 * j12;
        (s12b, m12b, m0)
    }

    /// Positive root of k⁴ + 2k³ − (x² + y² − 1)k² − 2y²k − y² = 0.
    fn astroid(x: f64, y: f64) -> f64 {
        let p = sq(x);
        let q = sq(y);
        let r = (p + q - 1.0) / 6.0;
        if q == 0.0 && r <= 0.0 {
            return 0.0;
        }
        let s = p * q / 4.0;
        let r2 = sq(r);
        let r3 = r * r2;
        let disc = s * (s + 2.0 * r3);
        let mut u = r;
        if disc >= 0.0 {
            let mut t3 = s + r3;
            t3 += if t3 < 0.0 { -disc.sqrt() } else { disc.sqrt() };
            let t = t3.cbrt();
            u += t + if t != 0.0 { r2 / t } else { 0.0 };
        } else {
            let ang = (-disc).sqrt().atan2(-(s + r3));
            u += 2.0 * r * (ang / 3.0).cos();
        }
        let v = (sq(u) + q).sqrt();
        let uv = if u < 0.0 { q / (v - u) } else { u + v };
        let w = (uv - q) / (2.0 * v);
        uv / ((uv + sq(w)).sqrt() + w)
    }

    /// Starting azimuth for Newton's method, or the whole solution for short
    /// lines. Returns (sig12, salp1, calp1, salp2, calp2, dnm).
    #[allow(clippy::too_many_arguments)]
    fn inverse_start(
        &self,
        sbet1: f64,
        cbet1: f64,
        dn1: f64,
        sbet2: f64,
        cbet2: f64,
        dn2: f64,
        lam12: f64,
        slam12: f64,
        clam12: f64,
    ) -> (f64, f64, f64, f64, f64, f64) {
        let mut sig12 = -1.0;
        let (mut salp2, mut calp2, mut dnm) = (0.0, 0.0, 0.0);
        let sbet12 = sbet2 * cbet1 - cbet2 * sbet1;
        let cbet12 = cbet2 * cbet1 + sbet2 * sbet1;
        let sbet12a = sbet2 * cbet1 + cbet2 * sbet1;
        let shortline = cbet12 >= 0.0 && sbet12 < 0.5 && cbet2 * lam12 < 0.5;
        let (mut somg12, mut comg12);
        if shortline {
            let mut sbetm2 = sq(sbet1 + sbet2);
            sbetm2 /= sbetm2 + sq(cbet1 + cbet2);
            dnm = (1.0 + self.ep2 * sbetm2).sqrt();
            let omg12 = lam12 / (self.f1 * dnm);
            (somg12, comg12) = omg12.sin_cos();
        } else {
            somg12 = slam12;
            comg12 = clam12;
        }
        let mut salp1 = cbet2 * somg12;
        let mut calp1 = if comg12 >= 0.0 {
            sbet12 + cbet2 * sbet1 * sq(somg12) / (1.0 + comg12)
        } else {
            sbet12a - cbet2 * sbet1 * sq(somg12) / (1.0 - comg12)
        };
        let ssig12 = salp1.hypot(calp1);
        let csig12 = sbet1 * sbet2 + cbet1 * cbet2 * comg12;

        if shortline && ssig12 < self.etol2 {
            salp2 = cbet1 * somg12;
            calp2 = sbet12
                - cbet1
                    * sbet2
                    * (if comg12 >= 0.0 {
                        sq(somg12) / (1.0 + comg12)
                    } else {
                        1.0 - comg12
                    });
            norm2(&mut salp2, &mut calp2);
            sig12 = ssig12.atan2(csig12);
        } else if self.n.abs() > 0.1
            || csig12 >= 0.0
            || ssig12 >= 6.0 * self.n.abs() * PI * sq(cbet1)
        {
            // The zeroth order spherical approximation is good enough.
        } else {
            let (x, y, lamscale, betscale);
            let lam12x = (-slam12).atan2(-clam12);
            if self.f >= 0.0 {
                let k2 = sq(sbet1) * self.ep2;
                let eps = k2 / (2.0 * (1.0 + (1.0 + k2).sqrt()) + k2);
                lamscale = self.f * cbet1 * self.a3f(eps) * PI;
                betscale = lamscale * cbet1;
                x = lam12x / lamscale;
                y = sbet12a / betscale;
            } else {
                let cbet12a = cbet2 * cbet1 - sbet2 * sbet1;
                let bet12a = sbet12a.atan2(cbet12a);
                let (_, m12b, m0) = self.lengths(
                    self.n,
                    PI + bet12a,
                    (sbet1, -cbet1, dn1),
                    (sbet2, cbet2, dn2),
                    false,
                    true,
                );
                x = -1.0 + m12b / (cbet1 * cbet2 * m0 * PI);
                betscale = if x < -0.01 {
                    sbet12a / x
                } else {
                    -self.f * sq(cbet1) * PI
                };
                lamscale = betscale / cbet1;
                y = lam12x / lamscale;
            }
            if y > -TOL1 && x > -1.0 - xthresh() {
                if self.f >= 0.0 {
                    salp1 = (1.0f64).min(-x);
                    calp1 = -(1.0 - sq(salp1)).sqrt();
                } else {
                    calp1 = (if x > -TOL1 { 0.0f64 } else { -1.0 }).max(x);
                    salp1 = (1.0 - sq(calp1)).sqrt();
                }
            } else {
                let k = Self::astroid(x, y);
                let omg12a = lamscale
                    * if self.f >= 0.0 {
                        -x * k / (1.0 + k)
                    } else {
                        -y * (1.0 + k) / k
                    };
                somg12 = omg12a.sin();
                comg12 = -omg12a.cos();
                salp1 = cbet2 * somg12;
                calp1 = sbet12a - cbet2 * sbet1 * sq(somg12) / (1.0 - comg12);
            }
        }
        // Backwards check lets NaN through.
        #[allow(clippy::neg_cmp_op_on_partial_ord)]
        if !(salp1 <= 0.0) {
            norm2(&mut salp1, &mut calp1);
        } else {
            salp1 = 1.0;
            calp1 = 0.0;
        }
        (sig12, salp1, calp1, salp2, calp2, dnm)
    }

    /// λ12 for a trial azimuth, and its derivative when `diffp`. The
    /// points are given as (sin β, cos β, dn).
    #[allow(clippy::too_many_arguments)]
    fn lambda12(
        &self,
        (sbet1, cbet1, dn1): (f64, f64, f64),
        (sbet2, cbet2, dn2): (f64, f64, f64),
        salp1: f64,
        calp1: f64,
        slam120: f64,
        clam120: f64,
        diffp: bool,
    ) -> Lambda {
        let mut calp1 = calp1;
        if sbet1 == 0.0 && calp1 == 0.0 {
            calp1 = -tiny();
        }
        let salp0 = salp1 * cbet1;
        let calp0 = calp1.hypot(salp1 * sbet1);
        let mut ssig1 = sbet1;
        let somg1 = salp0 * sbet1;
        let mut csig1 = calp1 * cbet1;
        let comg1 = csig1;
        norm2(&mut ssig1, &mut csig1);
        let salp2 = if cbet2 != cbet1 { salp0 / cbet2 } else { salp1 };
        let calp2 = if cbet2 != cbet1 || sbet2.abs() != -sbet1 {
            (sq(calp1 * cbet1)
                + if cbet1 < -sbet1 {
                    (cbet2 - cbet1) * (cbet1 + cbet2)
                } else {
                    (sbet1 - sbet2) * (sbet1 + sbet2)
                })
            .sqrt()
                / cbet2
        } else {
            calp1.abs()
        };
        let mut ssig2 = sbet2;
        let somg2 = salp0 * sbet2;
        let mut csig2 = calp2 * cbet2;
        let comg2 = csig2;
        norm2(&mut ssig2, &mut csig2);
        let sig12 =
            ((csig1 * ssig2 - ssig1 * csig2).max(0.0) + 0.0).atan2(csig1 * csig2 + ssig1 * ssig2);
        let somg12 = (comg1 * somg2 - somg1 * comg2).max(0.0) + 0.0;
        let comg12 = comg1 * comg2 + somg1 * somg2;
        let eta = (somg12 * clam120 - comg12 * slam120).atan2(comg12 * clam120 + somg12 * slam120);
        let k2 = sq(calp0) * self.ep2;
        let eps = k2 / (2.0 * (1.0 + (1.0 + k2).sqrt()) + k2);
        let ca = self.c3f(eps);
        let b312 = sin_cos_series(true, ssig2, csig2, &ca, ORDER - 1)
            - sin_cos_series(true, ssig1, csig1, &ca, ORDER - 1);
        let domg12 = -self.f * self.a3f(eps) * salp0 * (sig12 + b312);
        let lam12 = eta + domg12;
        let mut dlam12 = 0.0;
        if diffp {
            if calp2 == 0.0 {
                dlam12 = -2.0 * self.f1 * dn1 / sbet1;
            } else {
                let (_, m12b, _) = self.lengths(
                    eps,
                    sig12,
                    (ssig1, csig1, dn1),
                    (ssig2, csig2, dn2),
                    false,
                    true,
                );
                dlam12 = m12b * (self.f1 / (calp2 * cbet2));
            }
        }
        Lambda {
            lam12,
            dlam12,
            salp2,
            calp2,
            sig12,
            ssig1,
            csig1,
            ssig2,
            csig2,
            eps,
        }
    }

    /// Distance in metres and azimuth at point 1 in degrees from (lat1, lon1)
    /// to (lat2, lon2), all in degrees (PROJ's `geod_inverse`).
    pub(crate) fn inverse(&self, lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> (f64, f64) {
        let (mut lon12, mut lon12s) = ang_diff(lon1, lon2);
        let mut lonsign = if lon12.is_sign_negative() { -1.0 } else { 1.0 };
        lon12 *= lonsign;
        lon12s *= lonsign;
        let lam12 = lon12 * DEGREE;
        let (slam12, clam12) = sincosde(lon12, lon12s);
        lon12s = (180.0 - lon12) - lon12s;

        let mut lat1 = ang_round(lat_fix(lat1));
        let mut lat2 = ang_round(lat_fix(lat2));
        let swapp = if lat1.abs() < lat2.abs() || lat2.is_nan() {
            -1.0
        } else {
            1.0
        };
        if swapp < 0.0 {
            lonsign *= -1.0;
            std::mem::swap(&mut lat1, &mut lat2);
        }
        let latsign = if lat1.is_sign_negative() { 1.0 } else { -1.0 };
        lat1 *= latsign;
        lat2 *= latsign;

        let (mut sbet1, mut cbet1) = sincosdx(lat1);
        sbet1 *= self.f1;
        norm2(&mut sbet1, &mut cbet1);
        cbet1 = cbet1.max(tiny());
        let (mut sbet2, mut cbet2) = sincosdx(lat2);
        sbet2 *= self.f1;
        norm2(&mut sbet2, &mut cbet2);
        cbet2 = cbet2.max(tiny());

        if cbet1 < -sbet1 {
            if cbet2 == cbet1 {
                sbet2 = sbet1.copysign(sbet2);
            }
        } else if sbet2.abs() == -sbet1 {
            cbet2 = cbet1;
        }

        let dn1 = (1.0 + self.ep2 * sq(sbet1)).sqrt();
        let dn2 = (1.0 + self.ep2 * sq(sbet2)).sqrt();

        let mut s12x = 0.0;
        let (mut salp1, mut calp1, mut salp2, mut calp2) = (0.0, 0.0, 0.0, 0.0);
        let mut meridian = lat1 == -90.0 || slam12 == 0.0;

        if meridian {
            // Both points on one full meridian: head to the target longitude
            // and arrive heading north.
            calp1 = clam12;
            salp1 = slam12;
            calp2 = 1.0;
            salp2 = 0.0;
            let (ssig1, csig1) = (sbet1, calp1 * cbet1);
            let (ssig2, csig2) = (sbet2, calp2 * cbet2);
            let sig12 = ((csig1 * ssig2 - ssig1 * csig2).max(0.0) + 0.0)
                .atan2(csig1 * csig2 + ssig1 * ssig2);
            let (s, m12x, _) = self.lengths(
                self.n,
                sig12,
                (ssig1, csig1, dn1),
                (ssig2, csig2, dn2),
                true,
                true,
            );
            s12x = s;
            if sig12 < tol2() || m12x >= 0.0 {
                if sig12 < 3.0 * tiny() || (sig12 < TOL0 && (s12x < 0.0 || m12x < 0.0)) {
                    s12x = 0.0;
                }
                s12x *= self.b;
            } else {
                // Prolate and too close to antipodal.
                meridian = false;
            }
        }

        if !meridian && sbet1 == 0.0 && (self.f <= 0.0 || lon12s >= self.f * 180.0) {
            // Along the equator.
            calp1 = 0.0;
            calp2 = 0.0;
            salp1 = 1.0;
            salp2 = 1.0;
            s12x = self.a * lam12;
        } else if !meridian {
            let (sig, sa1, ca1, sa2, ca2, dnm) =
                self.inverse_start(sbet1, cbet1, dn1, sbet2, cbet2, dn2, lam12, slam12, clam12);
            salp1 = sa1;
            calp1 = ca1;
            if sig >= 0.0 {
                // Short line.
                salp2 = sa2;
                calp2 = ca2;
                s12x = sig * self.b * dnm;
            } else {
                // Newton's method on λ12(α1) − λ12 = 0, keeping a bracket
                // of the root and bisecting when a step leaves it.
                let mut numit = 0u32;
                let (mut salp1a, mut calp1a, mut salp1b, mut calp1b) = (tiny(), 1.0, tiny(), -1.0);
                let mut tripn = false;
                let mut tripb = false;
                let last = loop {
                    let l = self.lambda12(
                        (sbet1, cbet1, dn1),
                        (sbet2, cbet2, dn2),
                        salp1,
                        calp1,
                        slam12,
                        clam12,
                        numit < MAXIT1,
                    );
                    let v = l.lam12;
                    // The reversed test lets NaN escape.
                    #[allow(clippy::neg_cmp_op_on_partial_ord)]
                    let done = tripb
                        || !(v.abs() >= (if tripn { 8.0 } else { 1.0 }) * TOL0)
                        || numit == MAXIT2;
                    if done {
                        break l;
                    }
                    if v > 0.0 && (numit > MAXIT1 || calp1 / salp1 > calp1b / salp1b) {
                        salp1b = salp1;
                        calp1b = calp1;
                    } else if v < 0.0 && (numit > MAXIT1 || calp1 / salp1 < calp1a / salp1a) {
                        salp1a = salp1;
                        calp1a = calp1;
                    }
                    if numit < MAXIT1 && l.dlam12 > 0.0 {
                        let dalp1 = -v / l.dlam12;
                        if dalp1.abs() < PI {
                            let (sdalp1, cdalp1) = dalp1.sin_cos();
                            let nsalp1 = salp1 * cdalp1 + calp1 * sdalp1;
                            if nsalp1 > 0.0 {
                                calp1 = calp1 * cdalp1 - salp1 * sdalp1;
                                salp1 = nsalp1;
                                norm2(&mut salp1, &mut calp1);
                                tripn = v.abs() <= 16.0 * TOL0;
                                numit += 1;
                                continue;
                            }
                        }
                    }
                    salp1 = (salp1a + salp1b) / 2.0;
                    calp1 = (calp1a + calp1b) / 2.0;
                    norm2(&mut salp1, &mut calp1);
                    tripn = false;
                    tripb = (salp1a - salp1).abs() + (calp1a - calp1) < TOLB
                        || (salp1 - salp1b).abs() + (calp1 - calp1b) < TOLB;
                    numit += 1;
                };
                salp2 = last.salp2;
                calp2 = last.calp2;
                let (s, _, _) = self.lengths(
                    last.eps,
                    last.sig12,
                    (last.ssig1, last.csig1, dn1),
                    (last.ssig2, last.csig2, dn2),
                    true,
                    true,
                );
                s12x = s * self.b;
            }
        }

        // Back to the original ordering of the points.
        if swapp < 0.0 {
            std::mem::swap(&mut salp1, &mut salp2);
            std::mem::swap(&mut calp1, &mut calp2);
        }
        salp1 *= swapp * lonsign;
        calp1 *= swapp * latsign;
        (0.0 + s12x, atan2dx(salp1, calp1))
    }

    /// The point at distance `s12` metres along azimuth `azi1` degrees from
    /// (`lat1`, 0), as (latitude, longitude) in degrees (PROJ's
    /// `geod_direct`, longitude normalised).
    pub(crate) fn direct(&self, lat1: f64, azi1: f64, s12: f64) -> (f64, f64) {
        let azi1 = ang_normalize(azi1);
        let (salp1, calp1) = sincosdx(ang_round(azi1));
        let lat1 = lat_fix(lat1);
        let (mut sbet1, mut cbet1) = sincosdx(ang_round(lat1));
        sbet1 *= self.f1;
        norm2(&mut sbet1, &mut cbet1);
        cbet1 = cbet1.max(tiny());

        let salp0 = salp1 * cbet1;
        let calp0 = calp1.hypot(salp1 * sbet1);
        let mut ssig1 = sbet1;
        let somg1 = salp0 * sbet1;
        let mut csig1 = if sbet1 != 0.0 || calp1 != 0.0 {
            cbet1 * calp1
        } else {
            1.0
        };
        let comg1 = csig1;
        norm2(&mut ssig1, &mut csig1);

        let k2 = sq(calp0) * self.ep2;
        let eps = k2 / (2.0 * (1.0 + (1.0 + k2).sqrt()) + k2);
        let a1m1 = a1m1f(eps);
        let c1a = c1f(eps);
        let b11 = sin_cos_series(true, ssig1, csig1, &c1a, ORDER);
        let (s, c) = b11.sin_cos();
        let stau1 = ssig1 * c + csig1 * s;
        let ctau1 = csig1 * c - ssig1 * s;
        let c1pa = c1pf(eps);
        let c3a = self.c3f(eps);
        let a3c = -self.f * salp0 * self.a3f(eps);
        let b31 = sin_cos_series(true, ssig1, csig1, &c3a, ORDER - 1);

        // σ12 from the distance by the reverted series.
        let tau12 = s12 / (self.b * (1.0 + a1m1));
        let (s, c) = tau12.sin_cos();
        let b12 = -sin_cos_series(
            true,
            stau1 * c + ctau1 * s,
            ctau1 * c - stau1 * s,
            &c1pa,
            ORDER,
        );
        let mut sig12 = tau12 - (b12 - b11);
        let (mut ssig12, mut csig12) = sig12.sin_cos();
        if self.f.abs() > 0.01 {
            // The reverted series is poor for large flattening: one Newton
            // step.
            let ssig2 = ssig1 * csig12 + csig1 * ssig12;
            let csig2 = csig1 * csig12 - ssig1 * ssig12;
            let b12 = sin_cos_series(true, ssig2, csig2, &c1a, ORDER);
            let serr = (1.0 + a1m1) * (sig12 + (b12 - b11)) - s12 / self.b;
            sig12 -= serr / (1.0 + k2 * sq(ssig2)).sqrt();
            (ssig12, csig12) = sig12.sin_cos();
        }

        let ssig2 = ssig1 * csig12 + csig1 * ssig12;
        let mut csig2 = csig1 * csig12 - ssig1 * ssig12;
        let sbet2 = calp0 * ssig2;
        let mut cbet2 = salp0.hypot(calp0 * csig2);
        if cbet2 == 0.0 {
            // salp0 = 0 and csig2 = 0: break the degeneracy.
            cbet2 = tiny();
            csig2 = tiny();
        }

        let somg2 = salp0 * ssig2;
        let comg2 = csig2;
        let omg12 = (somg2 * comg1 - comg2 * somg1).atan2(comg2 * comg1 + somg2 * somg1);
        let lam12 =
            omg12 + a3c * (sig12 + (sin_cos_series(true, ssig2, csig2, &c3a, ORDER - 1) - b31));
        let lon2 = ang_normalize(lam12 / DEGREE);
        let lat2 = atan2dx(sbet2, self.f1 * cbet2);
        (lat2, lon2)
    }
}

struct Lambda {
    lam12: f64,
    dlam12: f64,
    salp2: f64,
    calp2: f64,
    sig12: f64,
    ssig1: f64,
    csig1: f64,
    ssig2: f64,
    csig2: f64,
    eps: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wgs84() -> Geodesic {
        Geodesic::new(6378137.0, 1.0 / 298.257223563)
    }

    /// GeographicLib's documented example: JFK to Heathrow-ish.
    #[test]
    fn inverse_known_line() {
        let (s12, azi1) = wgs84().inverse(40.6, -73.8, 51.6, -0.5);
        assert!((s12 - 5_551_759.400).abs() < 1e-3, "{s12}");
        assert!((azi1 - 51.198_882_846).abs() < 1e-8, "{azi1}");
    }

    #[test]
    fn direct_undoes_inverse() {
        let g = wgs84();
        let cases = [
            (10.0, 0.0, 20.0, 30.0),
            (-35.0, 0.0, 60.0, 170.0),
            (80.0, 0.0, -80.0, 120.0),
            (0.0, 0.0, 0.0, 90.0),
            (0.0, 0.0, 0.5, 179.6),
            (89.0, 0.0, 89.5, -179.0),
            (45.0, 0.0, 45.0, 0.001),
            (-90.0, 0.0, 30.0, 0.0),
        ];
        for (lat1, lon1, lat2, lon2) in cases {
            let (s12, azi1) = g.inverse(lat1, lon1, lat2, lon2);
            let (lat, lon) = g.direct(lat1, azi1, s12);
            let dlon = (lon - lon2 + 180.0).rem_euclid(360.0) - 180.0;
            assert!((lat - lat2).abs() < 1e-9, "{lat1} {lat2}: {lat} vs {lat2}");
            assert!(
                dlon.abs() < 1e-9 || (lat2.abs() - 90.0).abs() < 1e-9,
                "{lat1} {lon2}: {lon} vs {lon2}"
            );
        }
    }

    #[test]
    fn sphere_matches_great_circle() {
        let g = Geodesic::new(1.0, 0.0);
        let (s12, azi1) = g.inverse(0.0, 0.0, 0.0, 90.0);
        assert!((s12 - std::f64::consts::FRAC_PI_2).abs() < 1e-14);
        assert!((azi1 - 90.0).abs() < 1e-12);
    }
}
