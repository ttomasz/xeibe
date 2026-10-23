//! Geodesics on an ellipsoid of revolution: the direct and inverse
//! problems, geodesic lines, and the area and perimeter of geodesic
//! polygons.
//!
//! Ported in full from `geodesic.c` in PROJ 9.8.1 (MIT, see PROJ-NOTICE.md),
//! which is C. F. F. Karney's C library of the algorithms in "Algorithms
//! for geodesics", J. Geodesy 87, 43–55 (2013), part of GeographicLib
//! (MIT, <https://geographiclib.sourceforge.io/>, version 2.2.0). It is
//! the library pyproj's `Geod` calls, and the port follows it operation for
//! operation, so that it gives the same numbers: bit for bit on Linux,
//! but for an ulp or two on a few nearly antipodal lines, where Rust's
//! `cbrt` (`compiler_builtins`', from musl) and glibc's round differently.
//! Angles are in degrees and lengths in the units of the semi-major axis,
//! as in the original.
//!
//! ```
//! use xeibe_reproject::geodesic::Geodesic;
//!
//! let wgs84 = Geodesic::new(6378137.0, 1.0 / 298.257223563);
//! // JFK to Singapore Changi; pyproj gives 15347512.94051294 m,
//! // 3.3057734780176125° and 177.48784020815515°.
//! let (s12, azi1, azi2) = wgs84.inverse(40.64, -73.78, 1.36, 103.99);
//! assert_eq!((s12, azi1, azi2), (15347512.94051294, 3.3057734780176125, 177.48784020815515));
//! let (lat2, lon2, _) = wgs84.direct(40.64, -73.78, azi1, s12);
//! assert!((lat2 - 1.36).abs() < 1e-12 && (lon2 - 103.99).abs() < 1e-12);
//! ```

use std::f64::consts::PI;
use std::ops::{BitAnd, BitOr};

const ORDER: usize = 6;
const NC1: usize = ORDER;
const NC1P: usize = ORDER;
const NC2: usize = ORDER;
const NA3: usize = ORDER;
const NC3: usize = ORDER;
const NC3X: usize = (NC3 * (NC3 - 1)) / 2;
const NC4: usize = ORDER;
const NC4X: usize = (NC4 * (NC4 + 1)) / 2;
const NC: usize = ORDER + 1;

const MAXIT1: u32 = 20;
const MAXIT2: u32 = MAXIT1 + f64::MANTISSA_DIGITS + 10;

const TOL0: f64 = f64::EPSILON;
const TOL1: f64 = 200.0 * TOL0;
const TOLB: f64 = TOL0;
const DEGREE: f64 = PI / 180.0;

/// Quarter, half and full turns in degrees.
const QD: f64 = 90.0;
const HD: f64 = 180.0;
const TD: f64 = 360.0;

fn tiny() -> f64 {
    f64::MIN_POSITIVE.sqrt()
}
fn tol2() -> f64 {
    TOL0.sqrt()
}
fn xthresh() -> f64 {
    1000.0 * tol2()
}

/// The quantities to compute, as `geodesic.h`'s `GEOD_*` mask values (the
/// low bits are the series each needs). They combine with `|`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mask(u32);

impl Mask {
    /// Nothing.
    pub const NONE: Mask = Mask(0);
    /// Latitude.
    pub const LATITUDE: Mask = Mask(1 << 7);
    /// Longitude.
    pub const LONGITUDE: Mask = Mask((1 << 8) | (1 << 3));
    /// Azimuth.
    pub const AZIMUTH: Mask = Mask(1 << 9);
    /// Distance.
    pub const DISTANCE: Mask = Mask((1 << 10) | 1);
    /// Distance as an input to a geodesic line.
    pub const DISTANCE_IN: Mask = Mask((1 << 11) | 1 | (1 << 1));
    /// Reduced length `m12`.
    pub const REDUCED_LENGTH: Mask = Mask((1 << 12) | 1 | (1 << 2));
    /// Geodesic scales `M12` and `M21`.
    pub const GEODESIC_SCALE: Mask = Mask((1 << 13) | 1 | (1 << 2));
    /// Area `S12`.
    pub const AREA: Mask = Mask((1 << 14) | (1 << 4));
    /// Everything.
    pub const ALL: Mask = Mask(0x7F80 | 0x1F);

    const CAP_C1: u32 = 1 << 0;
    const CAP_C1P: u32 = 1 << 1;
    const CAP_C2: u32 = 1 << 2;
    const CAP_C3: u32 = 1 << 3;
    const CAP_C4: u32 = 1 << 4;
    const OUT_ALL: u32 = 0x7F80;

    fn has(self, other: Mask) -> bool {
        self.0 & other.0 & Self::OUT_ALL != 0
    }
    fn cap(self, cap: u32) -> bool {
        self.0 & cap != 0
    }
}

impl BitOr for Mask {
    type Output = Mask;
    fn bitor(self, rhs: Mask) -> Mask {
        Mask(self.0 | rhs.0)
    }
}

impl BitAnd for Mask {
    type Output = Mask;
    fn bitand(self, rhs: Mask) -> Mask {
        Mask(self.0 & rhs.0)
    }
}

/// How the general direct problem reads its distance and longitude
/// (`GEOD_ARCMODE`, `GEOD_LONG_UNROLL`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Flags {
    /// The distance is an arc length on the auxiliary sphere, in degrees.
    pub arc_mode: bool,
    /// The longitude is unrolled: it changes continuously along the line
    /// instead of being reduced to [−180°, 180°].
    pub long_unroll: bool,
}

/// The results of the general direct problem and of a position on a line.
/// Quantities not asked for are NaN.
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(non_snake_case)]
pub struct Position {
    /// Arc length on the auxiliary sphere, in degrees.
    pub a12: f64,
    pub lat2: f64,
    pub lon2: f64,
    /// Forward azimuth at point 2.
    pub azi2: f64,
    pub s12: f64,
    /// Reduced length.
    pub m12: f64,
    /// Geodesic scale of point 2 relative to point 1.
    pub M12: f64,
    /// Geodesic scale of point 1 relative to point 2.
    pub M21: f64,
    /// Area between the geodesic and the equator.
    pub S12: f64,
}

/// The results of the general inverse problem. Quantities not asked for
/// are NaN.
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(non_snake_case)]
pub struct Inverse {
    /// Arc length on the auxiliary sphere, in degrees (in [0, 180]).
    pub a12: f64,
    pub s12: f64,
    /// Forward azimuths at points 1 and 2.
    pub azi1: f64,
    pub azi2: f64,
    pub m12: f64,
    pub M12: f64,
    pub M21: f64,
    pub S12: f64,
}

fn sq(x: f64) -> f64 {
    x * x
}

/// C's `remquo` for y > 0: r = x − n y with n the integer nearest x / y
/// (ties to even), exactly, and n modulo 4 with n's sign.
fn remquo(x: f64, y: f64) -> (f64, i64) {
    // `%` is C's `fmod`, which is exact; reducing by 4 y keeps n's low
    // bits.
    let a = (x % (4.0 * y)).abs();
    let mut q = (a / y).trunc();
    // a − q y is exact with a fused multiply-add; a / y may have rounded
    // up to the next integer.
    let mut r = (-q).mul_add(y, a);
    if r < 0.0 {
        q -= 1.0;
        r += y;
    }
    if r > y / 2.0 || (r == y / 2.0 && q % 2.0 != 0.0) {
        r -= y;
        q += 1.0;
    }
    if x.is_sign_negative() {
        r = -r;
        q = -q;
    }
    if r == 0.0 {
        // A zero remainder has the sign of x.
        r = 0.0f64.copysign(x);
    }
    (r, q as i64)
}

/// C's `remainder`.
fn remainder(x: f64, y: f64) -> f64 {
    remquo(x, y).0
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

fn ang_normalize(x: f64) -> f64 {
    let y = remainder(x, TD);
    if y.abs() == HD { HD.copysign(x) } else { y }
}

fn lat_fix(x: f64) -> f64 {
    if x.abs() > QD { f64::NAN } else { x }
}

/// y − x reduced to [−180, 180], with its rounding error.
fn ang_diff(x: f64, y: f64) -> (f64, f64) {
    let (d, t) = sumx(remainder(-x, TD), remainder(y, TD));
    let (mut d, t) = sumx(remainder(d, TD), t);
    if d == 0.0 || d.abs() == HD {
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

/// Sine and cosine of x degrees, from the angle reduced exactly to
/// [−45°, 45°].
fn sincosdx(x: f64) -> (f64, f64) {
    let (r, q) = remquo(x, QD);
    sincos_quadrant(x, r * DEGREE, q)
}

/// Sine and cosine of x + t degrees, t being a small correction.
fn sincosde(x: f64, t: f64) -> (f64, f64) {
    let (r, q) = remquo(x, QD);
    sincos_quadrant(x, ang_round(r + t) * DEGREE, q)
}

fn sincos_quadrant(x: f64, r: f64, q: i64) -> (f64, f64) {
    let (s, c) = (r.sin(), r.cos());
    let (mut sinx, mut cosx) = match q.rem_euclid(4) {
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
        1 => HD.copysign(y) - ang,
        2 => QD - ang,
        3 => -QD + ang,
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
    let m = NC1 / 2;
    let t = polyvalx(m, &COEFF, sq(eps)) / COEFF[m + 1];
    (t + eps) / (1.0 - eps)
}

fn a2m1f(eps: f64) -> f64 {
    const COEFF: [f64; 5] = [-11.0, -28.0, -192.0, 0.0, 256.0];
    let m = NC2 / 2;
    let t = polyvalx(m, &COEFF, sq(eps)) / COEFF[m + 1];
    (t - eps) / (1.0 + eps)
}

/// The Fourier coefficients c\[1..=6\] from a table of polynomials in ε².
fn fourier_c(eps: f64, coeff: &[f64]) -> [f64; NC] {
    let mut c = [0.0; NC];
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

fn c1f(eps: f64) -> [f64; NC] {
    const COEFF: [f64; 18] = [
        -1.0, 6.0, -16.0, 32.0, -9.0, 64.0, -128.0, 2048.0, 9.0, -16.0, 768.0, 3.0, -5.0, 512.0,
        -7.0, 1280.0, -7.0, 2048.0,
    ];
    fourier_c(eps, &COEFF)
}

fn c1pf(eps: f64) -> [f64; NC] {
    const COEFF: [f64; 18] = [
        205.0, -432.0, 768.0, 1536.0, 4005.0, -4736.0, 3840.0, 12288.0, -225.0, 116.0, 384.0,
        -7173.0, 2695.0, 7680.0, 3467.0, 7680.0, 38081.0, 61440.0,
    ];
    fourier_c(eps, &COEFF)
}

fn c2f(eps: f64) -> [f64; NC] {
    const COEFF: [f64; 18] = [
        1.0, 2.0, 16.0, 32.0, 35.0, 64.0, 384.0, 2048.0, 15.0, 80.0, 768.0, 7.0, 35.0, 512.0, 63.0,
        1280.0, 77.0, 2048.0,
    ];
    fourier_c(eps, &COEFF)
}

/// What [`Geodesic::lengths`] computes: distance and reduced length over
/// b, the reduced length's secular coefficient, and the geodesic scales.
#[allow(non_snake_case)]
struct Lengths {
    s12b: f64,
    m12b: f64,
    m0: f64,
    M12: f64,
    M21: f64,
}

/// What [`Geodesic::lambda12`] computes for a trial azimuth.
struct Lambda {
    lam12: f64,
    salp2: f64,
    calp2: f64,
    sig12: f64,
    ssig1: f64,
    csig1: f64,
    ssig2: f64,
    csig2: f64,
    eps: f64,
    domg12: f64,
    dlam12: f64,
}

/// An ellipsoid to solve geodesic problems on (`struct geod_geodesic`).
#[derive(Debug, Clone)]
pub struct Geodesic {
    a: f64,
    f: f64,
    f1: f64,
    e2: f64,
    ep2: f64,
    n: f64,
    b: f64,
    c2: f64,
    etol2: f64,
    a3x: [f64; NA3],
    c3x: [f64; NC3X],
    c4x: [f64; NC4X],
}

impl Geodesic {
    /// For semi-major axis `a` and flattening `f` (0 for a sphere, negative
    /// for a prolate ellipsoid) (`geod_init`).
    pub fn new(a: f64, f: f64) -> Self {
        let f1 = 1.0 - f;
        let e2 = f * (2.0 - f);
        let b = a * f1;
        let c2 = (sq(a)
            + sq(b)
                * (if e2 == 0.0 {
                    1.0
                } else {
                    (if e2 > 0.0 {
                        e2.sqrt().atanh()
                    } else {
                        (-e2).sqrt().atan()
                    }) / e2.abs().sqrt()
                }))
            / 2.0;
        let etol2 = 0.1 * tol2() / (f.abs().max(0.001) * (1.0f64).min(1.0 - f / 2.0) / 2.0).sqrt();
        let mut g = Geodesic {
            a,
            f,
            f1,
            e2,
            ep2: e2 / sq(f1),
            n: f / (2.0 - f),
            b,
            c2,
            etol2,
            a3x: [0.0; NA3],
            c3x: [0.0; NC3X],
            c4x: [0.0; NC4X],
        };
        g.a3_coeff();
        g.c3_coeff();
        g.c4_coeff();
        g
    }

    /// The semi-major axis.
    pub fn a(&self) -> f64 {
        self.a
    }

    /// The flattening.
    pub fn f(&self) -> f64 {
        self.f
    }

    /// The area of the whole ellipsoid (4π c²).
    pub fn ellipsoid_area(&self) -> f64 {
        4.0 * PI * self.c2
    }

    fn a3_coeff(&mut self) {
        const COEFF: [f64; 18] = [
            -3.0, 128.0, -2.0, -3.0, 64.0, -1.0, -3.0, -1.0, 16.0, 3.0, -1.0, -2.0, 8.0, 1.0, -1.0,
            2.0, 1.0, 1.0,
        ];
        let (mut o, mut k) = (0, 0);
        for j in (0..NA3).rev() {
            let m = (NA3 - j - 1).min(j);
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
        for l in 1..NC3 {
            for j in (l..NC3).rev() {
                let m = (NC3 - j - 1).min(j);
                self.c3x[k] = polyvalx(m, &COEFF[o..], self.n) / COEFF[o + m + 1];
                k += 1;
                o += m + 2;
            }
        }
    }

    fn c4_coeff(&mut self) {
        const COEFF: [f64; 77] = [
            97.0, 15015.0, 1088.0, 156.0, 45045.0, -224.0, -4784.0, 1573.0, 45045.0, -10656.0,
            14144.0, -4576.0, -858.0, 45045.0, 64.0, 624.0, -4576.0, 6864.0, -3003.0, 15015.0,
            100.0, 208.0, 572.0, 3432.0, -12012.0, 30030.0, 45045.0, 1.0, 9009.0, -2944.0, 468.0,
            135135.0, 5792.0, 1040.0, -1287.0, 135135.0, 5952.0, -11648.0, 9152.0, -2574.0,
            135135.0, -64.0, -624.0, 4576.0, -6864.0, 3003.0, 135135.0, 8.0, 10725.0, 1856.0,
            -936.0, 225225.0, -8448.0, 4992.0, -1144.0, 225225.0, -1440.0, 4160.0, -4576.0, 1716.0,
            225225.0, -136.0, 63063.0, 1024.0, -208.0, 105105.0, 3584.0, -3328.0, 1144.0, 315315.0,
            -128.0, 135135.0, -2560.0, 832.0, 405405.0, 128.0, 99099.0,
        ];
        let (mut o, mut k) = (0, 0);
        for l in 0..NC4 {
            for j in (l..NC4).rev() {
                let m = NC4 - j - 1;
                self.c4x[k] = polyvalx(m, &COEFF[o..], self.n) / COEFF[o + m + 1];
                k += 1;
                o += m + 2;
            }
        }
    }

    fn a3f(&self, eps: f64) -> f64 {
        polyvalx(NA3 - 1, &self.a3x, eps)
    }

    /// C3\[1..=5\].
    fn c3f(&self, eps: f64) -> [f64; NC] {
        let mut c = [0.0; NC];
        let mut mult = 1.0;
        let mut o = 0;
        for (l, slot) in c.iter_mut().enumerate().take(NC3).skip(1) {
            let m = NC3 - l - 1;
            mult *= eps;
            *slot = mult * polyvalx(m, &self.c3x[o..], eps);
            o += m + 1;
        }
        c
    }

    /// C4\[0..=5\].
    fn c4f(&self, eps: f64) -> [f64; NC] {
        let mut c = [0.0; NC];
        let mut mult = 1.0;
        let mut o = 0;
        for (l, slot) in c.iter_mut().enumerate().take(NC4) {
            let m = NC4 - l - 1;
            *slot = mult * polyvalx(m, &self.c4x[o..], eps);
            o += m + 1;
            mult *= eps;
        }
        c
    }

    /// Distance over b, reduced length over b, m0 and the geodesic scales,
    /// as asked (`Lengths`). The points are (sin σ, cos σ, dn).
    #[allow(clippy::too_many_arguments)]
    fn lengths(
        &self,
        eps: f64,
        sig12: f64,
        (ssig1, csig1, dn1): (f64, f64, f64),
        (ssig2, csig2, dn2): (f64, f64, f64),
        cbet1: f64,
        cbet2: f64,
        want_s: bool,
        want_reduced: bool,
        want_scale: bool,
    ) -> Lengths {
        let (mut m0, mut j12, mut a1, mut a2) = (0.0, 0.0, 0.0, 0.0);
        let mut ca = [0.0; NC];
        let mut cb = [0.0; NC];
        let redlp = want_reduced || want_scale;
        if want_s || redlp {
            a1 = a1m1f(eps);
            ca = c1f(eps);
            if redlp {
                a2 = a2m1f(eps);
                cb = c2f(eps);
                m0 = a1 - a2;
                a2 += 1.0;
            }
            a1 += 1.0;
        }
        let mut out = Lengths {
            s12b: f64::NAN,
            m12b: f64::NAN,
            m0,
            M12: f64::NAN,
            M21: f64::NAN,
        };
        if want_s {
            let b1 = sin_cos_series(true, ssig2, csig2, &ca, NC1)
                - sin_cos_series(true, ssig1, csig1, &ca, NC1);
            out.s12b = a1 * (sig12 + b1);
            if redlp {
                let b2 = sin_cos_series(true, ssig2, csig2, &cb, NC2)
                    - sin_cos_series(true, ssig1, csig1, &cb, NC2);
                j12 = m0 * sig12 + (a1 * b1 - a2 * b2);
            }
        } else if redlp {
            for l in 1..=NC2 {
                cb[l] = a1 * ca[l] - a2 * cb[l];
            }
            j12 = m0 * sig12
                + (sin_cos_series(true, ssig2, csig2, &cb, NC2)
                    - sin_cos_series(true, ssig1, csig1, &cb, NC2));
        }
        if redlp {
            out.m12b = dn2 * (csig1 * ssig2) - dn1 * (ssig1 * csig2) - csig1 * csig2 * j12;
        }
        if want_scale {
            let csig12 = csig1 * csig2 + ssig1 * ssig2;
            let t = self.ep2 * (cbet1 - cbet2) * (cbet1 + cbet2) / (dn1 + dn2);
            out.M12 = csig12 + (t * ssig2 - csig2 * j12) * ssig1 / dn1;
            out.M21 = csig12 - (t * ssig1 - csig1 * j12) * ssig2 / dn2;
        }
        out
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
    /// lines (`InverseStart`). Returns (sig12, salp1, calp1, salp2, calp2,
    /// dnm); sig12 is negative unless the line was short.
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
            somg12 = omg12.sin();
            comg12 = omg12.cos();
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
                let l = self.lengths(
                    self.n,
                    PI + bet12a,
                    (sbet1, -cbet1, dn1),
                    (sbet2, cbet2, dn2),
                    cbet1,
                    cbet2,
                    false,
                    true,
                    false,
                );
                x = -1.0 + l.m12b / (cbet1 * cbet2 * l.m0 * PI);
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

    /// λ12 for a trial azimuth, and its derivative when `diffp`
    /// (`Lambda12`). The points are given as (sin β, cos β, dn).
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
        let b312 = sin_cos_series(true, ssig2, csig2, &ca, NC3 - 1)
            - sin_cos_series(true, ssig1, csig1, &ca, NC3 - 1);
        let domg12 = -self.f * self.a3f(eps) * salp0 * (sig12 + b312);
        let lam12 = eta + domg12;
        let mut dlam12 = 0.0;
        if diffp {
            if calp2 == 0.0 {
                dlam12 = -2.0 * self.f1 * dn1 / sbet1;
            } else {
                let l = self.lengths(
                    eps,
                    sig12,
                    (ssig1, csig1, dn1),
                    (ssig2, csig2, dn2),
                    cbet1,
                    cbet2,
                    false,
                    true,
                    false,
                );
                dlam12 = l.m12b * (self.f1 / (calp2 * cbet2));
            }
        }
        Lambda {
            lam12,
            salp2,
            calp2,
            sig12,
            ssig1,
            csig1,
            ssig2,
            csig2,
            eps,
            domg12,
            dlam12,
        }
    }

    /// The direct problem: the point at distance `s12` along azimuth
    /// `azi1` from (`lat1`, `lon1`), as (lat2, lon2, azi2) with lon2 in
    /// [−180, 180] (`geod_direct`).
    pub fn direct(&self, lat1: f64, lon1: f64, azi1: f64, s12: f64) -> (f64, f64, f64) {
        let p = self.gen_direct(
            lat1,
            lon1,
            azi1,
            Flags::default(),
            s12,
            Mask::LATITUDE | Mask::LONGITUDE | Mask::AZIMUTH,
        );
        (p.lat2, p.lon2, p.azi2)
    }

    /// The general direct problem (`geod_gendirect`): `s12_a12` is a
    /// distance, or with `flags.arc_mode` an arc length in degrees.
    pub fn gen_direct(
        &self,
        lat1: f64,
        lon1: f64,
        azi1: f64,
        flags: Flags,
        s12_a12: f64,
        outmask: Mask,
    ) -> Position {
        let caps = if flags.arc_mode {
            outmask
        } else {
            outmask | Mask::DISTANCE_IN
        };
        self.line(lat1, lon1, azi1, caps)
            .gen_position(flags, s12_a12, outmask)
    }

    /// The inverse problem: distance and forward azimuths at both ends from
    /// (`lat1`, `lon1`) to (`lat2`, `lon2`), as (s12, azi1, azi2)
    /// (`geod_inverse`).
    pub fn inverse(&self, lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> (f64, f64, f64) {
        let r = self.gen_inverse(lat1, lon1, lat2, lon2, Mask::DISTANCE);
        (r.s12, r.azi1, r.azi2)
    }

    /// The general inverse problem (`geod_geninverse`); the azimuths are
    /// always computed.
    pub fn gen_inverse(
        &self,
        lat1: f64,
        lon1: f64,
        lat2: f64,
        lon2: f64,
        outmask: Mask,
    ) -> Inverse {
        let (r, salp1, calp1, salp2, calp2) = self.gen_inverse_int(lat1, lon1, lat2, lon2, outmask);
        Inverse {
            azi1: atan2dx(salp1, calp1),
            azi2: atan2dx(salp2, calp2),
            ..r
        }
    }

    /// `geod_geninverse_int`: the results, and the azimuths at both ends as
    /// sines and cosines.
    #[allow(non_snake_case)]
    fn gen_inverse_int(
        &self,
        lat1: f64,
        lon1: f64,
        lat2: f64,
        lon2: f64,
        outmask: Mask,
    ) -> (Inverse, f64, f64, f64, f64) {
        let want_scale = outmask.has(Mask::GEODESIC_SCALE);
        let want_area = outmask.has(Mask::AREA);
        let (mut M12, mut M21) = (0.0, 0.0);
        let (mut lon12, mut lon12s) = ang_diff(lon1, lon2);
        let mut lonsign: f64 = if lon12.is_sign_negative() { -1.0 } else { 1.0 };
        lon12 *= lonsign;
        lon12s *= lonsign;
        let lam12 = lon12 * DEGREE;
        let (slam12, clam12) = sincosde(lon12, lon12s);
        lon12s = (HD - lon12) - lon12s;

        let mut lat1 = ang_round(lat_fix(lat1));
        let mut lat2 = ang_round(lat_fix(lat2));
        let swapp: f64 = if lat1.abs() < lat2.abs() || lat2.is_nan() {
            -1.0
        } else {
            1.0
        };
        if swapp < 0.0 {
            lonsign *= -1.0;
            std::mem::swap(&mut lat1, &mut lat2);
        }
        let latsign: f64 = if lat1.is_sign_negative() { 1.0 } else { -1.0 };
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

        let (mut s12x, mut m12x, mut a12) = (0.0, 0.0, 0.0);
        let (mut salp1, mut calp1, mut salp2, mut calp2) = (0.0, 0.0, 0.0, 0.0);
        let mut sig12;
        // somg12 == 2 marks that it still has to be computed.
        let (mut omg12, mut somg12, mut comg12) = (0.0, 2.0, 0.0);
        let mut meridian = lat1 == -QD || slam12 == 0.0;

        if meridian {
            // Both points on one full meridian: head to the target longitude
            // and arrive heading north.
            calp1 = clam12;
            salp1 = slam12;
            calp2 = 1.0;
            salp2 = 0.0;
            let (ssig1, csig1) = (sbet1, calp1 * cbet1);
            let (ssig2, csig2) = (sbet2, calp2 * cbet2);
            sig12 = ((csig1 * ssig2 - ssig1 * csig2).max(0.0) + 0.0)
                .atan2(csig1 * csig2 + ssig1 * ssig2);
            let l = self.lengths(
                self.n,
                sig12,
                (ssig1, csig1, dn1),
                (ssig2, csig2, dn2),
                cbet1,
                cbet2,
                true,
                true,
                want_scale,
            );
            s12x = l.s12b;
            m12x = l.m12b;
            if want_scale {
                M12 = l.M12;
                M21 = l.M21;
            }
            if sig12 < tol2() || m12x >= 0.0 {
                if sig12 < 3.0 * tiny() || (sig12 < TOL0 && (s12x < 0.0 || m12x < 0.0)) {
                    sig12 = 0.0;
                    m12x = 0.0;
                    s12x = 0.0;
                }
                m12x *= self.b;
                s12x *= self.b;
                a12 = sig12 / DEGREE;
            } else {
                // Prolate and too close to antipodal.
                meridian = false;
            }
        }

        if !meridian && sbet1 == 0.0 && (self.f <= 0.0 || lon12s >= self.f * HD) {
            // Along the equator.
            calp1 = 0.0;
            calp2 = 0.0;
            salp1 = 1.0;
            salp2 = 1.0;
            s12x = self.a * lam12;
            sig12 = lam12 / self.f1;
            omg12 = sig12;
            m12x = self.b * sig12.sin();
            if want_scale {
                M12 = sig12.cos();
                M21 = M12;
            }
            a12 = lon12 / self.f1;
        } else if !meridian {
            let (sig, sa1, ca1, sa2, ca2, dnm) =
                self.inverse_start(sbet1, cbet1, dn1, sbet2, cbet2, dn2, lam12, slam12, clam12);
            sig12 = sig;
            salp1 = sa1;
            calp1 = ca1;
            if sig12 >= 0.0 {
                // Short line.
                salp2 = sa2;
                calp2 = ca2;
                s12x = sig12 * self.b * dnm;
                m12x = sq(dnm) * self.b * (sig12 / dnm).sin();
                if want_scale {
                    M12 = (sig12 / dnm).cos();
                    M21 = M12;
                }
                a12 = sig12 / DEGREE;
                omg12 = lam12 / (self.f1 * dnm);
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
                            let (sdalp1, cdalp1) = (dalp1.sin(), dalp1.cos());
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
                sig12 = last.sig12;
                let l = self.lengths(
                    last.eps,
                    sig12,
                    (last.ssig1, last.csig1, dn1),
                    (last.ssig2, last.csig2, dn2),
                    cbet1,
                    cbet2,
                    true,
                    true,
                    want_scale,
                );
                s12x = l.s12b * self.b;
                m12x = l.m12b * self.b;
                if want_scale {
                    M12 = l.M12;
                    M21 = l.M21;
                }
                a12 = sig12 / DEGREE;
                if want_area {
                    let (sdomg12, cdomg12) = (last.domg12.sin(), last.domg12.cos());
                    somg12 = slam12 * cdomg12 - clam12 * sdomg12;
                    comg12 = clam12 * cdomg12 + slam12 * sdomg12;
                }
            }
        }

        let mut out = Inverse {
            a12,
            s12: f64::NAN,
            azi1: f64::NAN,
            azi2: f64::NAN,
            m12: f64::NAN,
            M12: f64::NAN,
            M21: f64::NAN,
            S12: f64::NAN,
        };
        if outmask.has(Mask::DISTANCE) {
            out.s12 = 0.0 + s12x;
        }
        if outmask.has(Mask::REDUCED_LENGTH) {
            out.m12 = 0.0 + m12x;
        }

        if want_area {
            let salp0 = salp1 * cbet1;
            let calp0 = calp1.hypot(salp1 * sbet1);
            let mut s12_area = if calp0 != 0.0 && salp0 != 0.0 {
                let (mut ssig1, mut csig1) = (sbet1, calp1 * cbet1);
                let (mut ssig2, mut csig2) = (sbet2, calp2 * cbet2);
                let k2 = sq(calp0) * self.ep2;
                let eps = k2 / (2.0 * (1.0 + (1.0 + k2).sqrt()) + k2);
                let a4 = sq(self.a) * calp0 * salp0 * self.e2;
                norm2(&mut ssig1, &mut csig1);
                norm2(&mut ssig2, &mut csig2);
                let ca = self.c4f(eps);
                let b41 = sin_cos_series(false, ssig1, csig1, &ca, NC4);
                let b42 = sin_cos_series(false, ssig2, csig2, &ca, NC4);
                a4 * (b42 - b41)
            } else {
                // Avoid problems with indeterminate σ1, σ2 on the equator.
                0.0
            };
            if !meridian && somg12 == 2.0 {
                somg12 = omg12.sin();
                comg12 = omg12.cos();
            }
            // C's literal: ω12 < 3π/4 roughly, the longitude difference
            // not too big, nor the latitude difference.
            #[allow(clippy::approx_constant)]
            let alp12 = if !meridian && comg12 > -0.7071 && sbet2 - sbet1 < 1.75 {
                let domg12 = 1.0 + comg12;
                let dbet1 = 1.0 + cbet1;
                let dbet2 = 1.0 + cbet2;
                2.0 * (somg12 * (sbet1 * dbet2 + sbet2 * dbet1))
                    .atan2(domg12 * (sbet1 * sbet2 + dbet1 * dbet2))
            } else {
                let mut salp12 = salp2 * calp1 - calp2 * salp1;
                let mut calp12 = calp2 * calp1 + salp2 * salp1;
                if salp12 == 0.0 && calp12 < 0.0 {
                    salp12 = tiny() * calp1;
                    calp12 = -1.0;
                }
                salp12.atan2(calp12)
            };
            s12_area += self.c2 * alp12;
            s12_area *= swapp * lonsign * latsign;
            out.S12 = s12_area + 0.0;
        }

        if swapp < 0.0 {
            std::mem::swap(&mut salp1, &mut salp2);
            std::mem::swap(&mut calp1, &mut calp2);
            if want_scale {
                std::mem::swap(&mut M12, &mut M21);
            }
        }
        salp1 *= swapp * lonsign;
        calp1 *= swapp * latsign;
        salp2 *= swapp * lonsign;
        calp2 *= swapp * latsign;
        if want_scale {
            out.M12 = M12;
            out.M21 = M21;
        }
        (out, salp1, calp1, salp2, calp2)
    }

    /// The geodesic line from (`lat1`, `lon1`) along `azi1`, able to give
    /// what `caps` asks for; `Mask::NONE` means distance in and longitude
    /// (`geod_lineinit`). Latitude and azimuth are always available.
    pub fn line(&self, lat1: f64, lon1: f64, azi1: f64, caps: Mask) -> GeodesicLine {
        let azi1 = ang_normalize(azi1);
        // Guard against underflow in salp0.
        let (salp1, calp1) = sincosdx(ang_round(azi1));
        GeodesicLine::new(self, lat1, lon1, azi1, salp1, calp1, caps)
    }

    /// [`Geodesic::line`] with its distance to point 3 set to `s13`
    /// (`geod_directline`).
    pub fn direct_line(
        &self,
        lat1: f64,
        lon1: f64,
        azi1: f64,
        s13: f64,
        caps: Mask,
    ) -> GeodesicLine {
        let mut line = self.line(lat1, lon1, azi1, caps);
        line.set_distance(s13);
        line
    }

    /// The geodesic line from (`lat1`, `lon1`) to (`lat2`, `lon2`), with
    /// its distance set to that between them (`geod_inverseline`).
    pub fn inverse_line(
        &self,
        lat1: f64,
        lon1: f64,
        lat2: f64,
        lon2: f64,
        caps: Mask,
    ) -> GeodesicLine {
        let (r, salp1, calp1, _, _) = self.gen_inverse_int(lat1, lon1, lat2, lon2, Mask::NONE);
        let azi1 = atan2dx(salp1, calp1);
        let mut caps = if caps == Mask::NONE {
            Mask::DISTANCE_IN | Mask::LONGITUDE
        } else {
            caps
        };
        // Make sure a12 can be turned into a distance.
        if caps.has(Mask::DISTANCE_IN) {
            caps = caps | Mask::DISTANCE;
        }
        let mut line = GeodesicLine::new(self, lat1, lon1, azi1, salp1, calp1, caps);
        line.set_arc(r.a12);
        line
    }

    /// The area (counter-clockwise positive) and perimeter of the polygon
    /// with these vertices, which needn't repeat the first one
    /// (`geod_polygonarea`).
    pub fn polygon_area(&self, lats: &[f64], lons: &[f64]) -> (f64, f64) {
        let mut p = Polygon::new(false);
        for (&lat, &lon) in lats.iter().zip(lons) {
            p.add_point(self, lat, lon);
        }
        let (_, area, perimeter) = p.compute(self, false, true);
        (area, perimeter)
    }
}

/// A geodesic line from a point along an azimuth (`struct
/// geod_geodesicline`), on which positions can be computed by distance or
/// arc length.
#[derive(Debug, Clone)]
pub struct GeodesicLine {
    lat1: f64,
    lon1: f64,
    azi1: f64,
    a: f64,
    f: f64,
    b: f64,
    c2: f64,
    f1: f64,
    salp0: f64,
    calp0: f64,
    k2: f64,
    salp1: f64,
    calp1: f64,
    ssig1: f64,
    csig1: f64,
    dn1: f64,
    stau1: f64,
    ctau1: f64,
    somg1: f64,
    comg1: f64,
    a1m1: f64,
    a2m1: f64,
    a3c: f64,
    b11: f64,
    b21: f64,
    b31: f64,
    a4: f64,
    b41: f64,
    s13: f64,
    a13: f64,
    caps: Mask,
    c1a: [f64; NC],
    c1pa: [f64; NC],
    c2a: [f64; NC],
    c3a: [f64; NC],
    c4a: [f64; NC],
}

impl GeodesicLine {
    /// `geod_lineinit_int`.
    fn new(
        g: &Geodesic,
        lat1: f64,
        lon1: f64,
        azi1: f64,
        salp1: f64,
        calp1: f64,
        caps: Mask,
    ) -> Self {
        let caps = (if caps == Mask::NONE {
            Mask::DISTANCE_IN | Mask::LONGITUDE
        } else {
            caps
        }) | Mask::LATITUDE
            | Mask::AZIMUTH;
        let lat1 = lat_fix(lat1);
        let (mut sbet1, mut cbet1) = sincosdx(ang_round(lat1));
        sbet1 *= g.f1;
        norm2(&mut sbet1, &mut cbet1);
        cbet1 = cbet1.max(tiny());
        let dn1 = (1.0 + g.ep2 * sq(sbet1)).sqrt();
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
        let k2 = sq(calp0) * g.ep2;
        let eps = k2 / (2.0 * (1.0 + (1.0 + k2).sqrt()) + k2);

        let mut l = GeodesicLine {
            lat1,
            lon1,
            azi1,
            a: g.a,
            f: g.f,
            b: g.b,
            c2: g.c2,
            f1: g.f1,
            salp0,
            calp0,
            k2,
            salp1,
            calp1,
            ssig1,
            csig1,
            dn1,
            stau1: 0.0,
            ctau1: 0.0,
            somg1,
            comg1,
            a1m1: 0.0,
            a2m1: 0.0,
            a3c: 0.0,
            b11: 0.0,
            b21: 0.0,
            b31: 0.0,
            a4: 0.0,
            b41: 0.0,
            s13: f64::NAN,
            a13: f64::NAN,
            caps,
            c1a: [0.0; NC],
            c1pa: [0.0; NC],
            c2a: [0.0; NC],
            c3a: [0.0; NC],
            c4a: [0.0; NC],
        };
        if caps.cap(Mask::CAP_C1) {
            l.a1m1 = a1m1f(eps);
            l.c1a = c1f(eps);
            l.b11 = sin_cos_series(true, ssig1, csig1, &l.c1a, NC1);
            let (s, c) = (l.b11.sin(), l.b11.cos());
            l.stau1 = ssig1 * c + csig1 * s;
            l.ctau1 = csig1 * c - ssig1 * s;
        }
        if caps.cap(Mask::CAP_C1P) {
            l.c1pa = c1pf(eps);
        }
        if caps.cap(Mask::CAP_C2) {
            l.a2m1 = a2m1f(eps);
            l.c2a = c2f(eps);
            l.b21 = sin_cos_series(true, ssig1, csig1, &l.c2a, NC2);
        }
        if caps.cap(Mask::CAP_C3) {
            l.c3a = g.c3f(eps);
            l.a3c = -l.f * salp0 * g.a3f(eps);
            l.b31 = sin_cos_series(true, ssig1, csig1, &l.c3a, NC3 - 1);
        }
        if caps.cap(Mask::CAP_C4) {
            l.c4a = g.c4f(eps);
            l.a4 = sq(l.a) * calp0 * salp0 * g.e2;
            l.b41 = sin_cos_series(false, ssig1, csig1, &l.c4a, NC4);
        }
        l
    }

    /// The point at distance `s12` from the start, as (lat2, lon2, azi2)
    /// (`geod_position`).
    pub fn position(&self, s12: f64) -> (f64, f64, f64) {
        let p = self.gen_position(
            Flags::default(),
            s12,
            Mask::LATITUDE | Mask::LONGITUDE | Mask::AZIMUTH,
        );
        (p.lat2, p.lon2, p.azi2)
    }

    /// The general position (`geod_genposition`): `s12_a12` is a distance,
    /// or with `flags.arc_mode` an arc length in degrees. Quantities the
    /// line can't give (see [`Geodesic::line`]'s caps) are NaN, and so is
    /// everything when a distance is given to a line without
    /// `Mask::DISTANCE_IN`.
    #[allow(non_snake_case)]
    pub fn gen_position(&self, flags: Flags, s12_a12: f64, outmask: Mask) -> Position {
        let outmask = Mask(outmask.0 & self.caps.0 & Mask::OUT_ALL);
        let mut out = Position {
            a12: f64::NAN,
            lat2: f64::NAN,
            lon2: f64::NAN,
            azi2: f64::NAN,
            s12: f64::NAN,
            m12: f64::NAN,
            M12: f64::NAN,
            M21: f64::NAN,
            S12: f64::NAN,
        };
        if !(flags.arc_mode || self.caps.has(Mask::DISTANCE_IN)) {
            // A distance this line can't take.
            return out;
        }
        let long_unroll = flags.long_unroll;
        let (mut sig12, mut ssig12, mut csig12);
        let (mut b12, mut ab1) = (0.0, 0.0);
        if flags.arc_mode {
            sig12 = s12_a12 * DEGREE;
            (ssig12, csig12) = sincosdx(s12_a12);
        } else {
            let tau12 = s12_a12 / (self.b * (1.0 + self.a1m1));
            let (s, c) = (tau12.sin(), tau12.cos());
            b12 = -sin_cos_series(
                true,
                self.stau1 * c + self.ctau1 * s,
                self.ctau1 * c - self.stau1 * s,
                &self.c1pa,
                NC1P,
            );
            sig12 = tau12 - (b12 - self.b11);
            ssig12 = sig12.sin();
            csig12 = sig12.cos();
            if self.f.abs() > 0.01 {
                // The reverted series is poor for large flattening: one
                // Newton step.
                let ssig2 = self.ssig1 * csig12 + self.csig1 * ssig12;
                let csig2 = self.csig1 * csig12 - self.ssig1 * ssig12;
                b12 = sin_cos_series(true, ssig2, csig2, &self.c1a, NC1);
                let serr = (1.0 + self.a1m1) * (sig12 + (b12 - self.b11)) - s12_a12 / self.b;
                sig12 -= serr / (1.0 + self.k2 * sq(ssig2)).sqrt();
                ssig12 = sig12.sin();
                csig12 = sig12.cos();
            }
        }

        let ssig2 = self.ssig1 * csig12 + self.csig1 * ssig12;
        let mut csig2 = self.csig1 * csig12 - self.ssig1 * ssig12;
        let dn2 = (1.0 + self.k2 * sq(ssig2)).sqrt();
        if outmask.has(Mask(
            Mask::DISTANCE.0 | Mask::REDUCED_LENGTH.0 | Mask::GEODESIC_SCALE.0,
        )) {
            if flags.arc_mode || self.f.abs() > 0.01 {
                b12 = sin_cos_series(true, ssig2, csig2, &self.c1a, NC1);
            }
            ab1 = (1.0 + self.a1m1) * (b12 - self.b11);
        }
        let sbet2 = self.calp0 * ssig2;
        let mut cbet2 = self.salp0.hypot(self.calp0 * csig2);
        if cbet2 == 0.0 {
            // salp0 = 0 and csig2 = 0: break the degeneracy.
            cbet2 = tiny();
            csig2 = tiny();
        }
        let salp2 = self.salp0;
        let calp2 = self.calp0 * csig2;

        if outmask.has(Mask::DISTANCE) {
            out.s12 = if flags.arc_mode {
                self.b * ((1.0 + self.a1m1) * sig12 + ab1)
            } else {
                s12_a12
            };
        }
        if outmask.has(Mask::LONGITUDE) {
            let e = 1.0f64.copysign(self.salp0);
            let somg2 = self.salp0 * ssig2;
            let comg2 = csig2;
            let omg12 = if long_unroll {
                e * (sig12 - (ssig2.atan2(csig2) - self.ssig1.atan2(self.csig1))
                    + ((e * somg2).atan2(comg2) - (e * self.somg1).atan2(self.comg1)))
            } else {
                (somg2 * self.comg1 - comg2 * self.somg1)
                    .atan2(comg2 * self.comg1 + somg2 * self.somg1)
            };
            let lam12 = omg12
                + self.a3c
                    * (sig12 + (sin_cos_series(true, ssig2, csig2, &self.c3a, NC3 - 1) - self.b31));
            let lon12 = lam12 / DEGREE;
            out.lon2 = if long_unroll {
                self.lon1 + lon12
            } else {
                ang_normalize(ang_normalize(self.lon1) + ang_normalize(lon12))
            };
        }
        if outmask.has(Mask::LATITUDE) {
            out.lat2 = atan2dx(sbet2, self.f1 * cbet2);
        }
        if outmask.has(Mask::AZIMUTH) {
            out.azi2 = atan2dx(salp2, calp2);
        }
        if outmask.has(Mask(Mask::REDUCED_LENGTH.0 | Mask::GEODESIC_SCALE.0)) {
            let b22 = sin_cos_series(true, ssig2, csig2, &self.c2a, NC2);
            let ab2 = (1.0 + self.a2m1) * (b22 - self.b21);
            let j12 = (self.a1m1 - self.a2m1) * sig12 + (ab1 - ab2);
            if outmask.has(Mask::REDUCED_LENGTH) {
                out.m12 = self.b
                    * ((dn2 * (self.csig1 * ssig2) - self.dn1 * (self.ssig1 * csig2))
                        - self.csig1 * csig2 * j12);
            }
            if outmask.has(Mask::GEODESIC_SCALE) {
                let t = self.k2 * (ssig2 - self.ssig1) * (ssig2 + self.ssig1) / (self.dn1 + dn2);
                out.M12 = csig12 + (t * ssig2 - csig2 * j12) * self.ssig1 / self.dn1;
                out.M21 = csig12 - (t * self.ssig1 - self.csig1 * j12) * ssig2 / dn2;
            }
        }
        if outmask.has(Mask::AREA) {
            let b42 = sin_cos_series(false, ssig2, csig2, &self.c4a, NC4);
            let (salp12, calp12) = if self.calp0 == 0.0 || self.salp0 == 0.0 {
                (
                    salp2 * self.calp1 - calp2 * self.salp1,
                    calp2 * self.calp1 + salp2 * self.salp1,
                )
            } else {
                (
                    self.calp0
                        * self.salp0
                        * (if csig12 <= 0.0 {
                            self.csig1 * (1.0 - csig12) + ssig12 * self.ssig1
                        } else {
                            ssig12 * (self.csig1 * ssig12 / (1.0 + csig12) + self.ssig1)
                        }),
                    sq(self.salp0) + sq(self.calp0) * self.csig1 * csig2,
                )
            };
            out.S12 = self.c2 * salp12.atan2(calp12) + self.a4 * (b42 - self.b41);
        }
        out.a12 = if flags.arc_mode {
            s12_a12
        } else {
            sig12 / DEGREE
        };
        out
    }

    /// Set the distance from the start to a reference point 3
    /// (`geod_setdistance`).
    pub fn set_distance(&mut self, s13: f64) {
        self.s13 = s13;
        self.a13 = self.gen_position(Flags::default(), s13, Mask::NONE).a12;
    }

    /// Set the arc length from the start to point 3 (`geod_setarc`).
    pub fn set_arc(&mut self, a13: f64) {
        self.a13 = a13;
        let flags = Flags {
            arc_mode: true,
            long_unroll: false,
        };
        self.s13 = self.gen_position(flags, a13, Mask::DISTANCE).s12;
    }

    /// The distance to point 3 (`s13`), NaN when it isn't set.
    pub fn distance(&self) -> f64 {
        self.s13
    }

    /// The arc length to point 3 (`a13`), NaN when it isn't set.
    pub fn arc(&self) -> f64 {
        self.a13
    }

    /// The start: latitude, longitude and azimuth.
    pub fn start(&self) -> (f64, f64, f64) {
        (self.lat1, self.lon1, self.azi1)
    }
}

/// An accumulator of a sum with twice the precision of an f64.
#[derive(Debug, Clone, Copy, Default)]
struct Accumulator(f64, f64);

impl Accumulator {
    fn add(&mut self, y: f64) {
        let (z, u) = sumx(y, self.1);
        let (s, t) = sumx(z, self.0);
        self.0 = s;
        self.1 = t;
        if self.0 == 0.0 {
            self.0 = u;
        } else {
            self.1 += u;
        }
    }

    fn sum(self, y: f64) -> f64 {
        let mut t = self;
        t.add(y);
        t.0
    }

    fn negate(&mut self) {
        self.0 = -self.0;
        self.1 = -self.1;
    }

    /// Reduce to [−y/2, y/2].
    fn remainder(&mut self, y: f64) {
        self.0 = remainder(self.0, y);
        self.add(0.0);
    }
}

/// 1 or −1 when going from `lon1` to `lon2` crosses the prime meridian
/// eastwards or westwards, else 0.
fn transit(lon1: f64, lon2: f64) -> i32 {
    let (lon12, _) = ang_diff(lon1, lon2);
    let lon1 = ang_normalize(lon1);
    let lon2 = ang_normalize(lon2);
    if lon12 > 0.0 && ((lon1 < 0.0 && lon2 >= 0.0) || (lon1 > 0.0 && lon2 == 0.0)) {
        1
    } else if lon12 < 0.0 && lon1 >= 0.0 && lon2 < 0.0 {
        -1
    } else {
        0
    }
}

/// The parity of ⌊lon2 / 360⌋ − ⌊lon1 / 360⌋, exactly, for unrolled
/// longitudes.
fn transit_direct(lon1: f64, lon2: f64) -> i32 {
    let lon1 = remainder(lon1, 2.0 * TD);
    let lon2 = remainder(lon2, 2.0 * TD);
    (if (0.0..TD).contains(&lon2) { 0 } else { 1 })
        - (if (0.0..TD).contains(&lon1) { 0 } else { 1 })
}

fn area_reduce_a(
    mut area: Accumulator,
    area0: f64,
    crossings: i32,
    reverse: bool,
    sign: bool,
) -> f64 {
    area.remainder(area0);
    if crossings & 1 != 0 {
        area.add((if area.0 < 0.0 { 1.0 } else { -1.0 }) * area0 / 2.0);
    }
    // Clockwise so far.
    if !reverse {
        area.negate();
    }
    if sign {
        if area.0 > area0 / 2.0 {
            area.add(-area0);
        } else if area.0 <= -area0 / 2.0 {
            area.add(area0);
        }
    } else if area.0 >= area0 {
        area.add(-area0);
    } else if area.0 < 0.0 {
        area.add(area0);
    }
    0.0 + area.0
}

fn area_reduce_b(area: f64, area0: f64, crossings: i32, reverse: bool, sign: bool) -> f64 {
    let mut area = remainder(area, area0);
    if crossings & 1 != 0 {
        area += (if area < 0.0 { 1.0 } else { -1.0 }) * area0 / 2.0;
    }
    if !reverse {
        area *= -1.0;
    }
    if sign {
        if area > area0 / 2.0 {
            area -= area0;
        } else if area <= -area0 / 2.0 {
            area += area0;
        }
    } else if area >= area0 {
        area -= area0;
    } else if area < 0.0 {
        area += area0;
    }
    0.0 + area
}

/// A geodesic polygon or polyline built point by point or edge by edge
/// (`struct geod_polygon`).
#[derive(Debug, Clone)]
pub struct Polygon {
    lat: f64,
    lon: f64,
    lat0: f64,
    lon0: f64,
    perimeter: Accumulator,
    area: Accumulator,
    polyline: bool,
    crossings: i32,
    num: u32,
}

impl Polygon {
    /// An empty polygon, or with `polyline` a polyline, which has a length
    /// but no area (`geod_polygon_init`).
    pub fn new(polyline: bool) -> Self {
        Polygon {
            lat: f64::NAN,
            lon: f64::NAN,
            lat0: f64::NAN,
            lon0: f64::NAN,
            perimeter: Accumulator::default(),
            area: Accumulator::default(),
            polyline,
            crossings: 0,
            num: 0,
        }
    }

    /// Remove every point (`geod_polygon_clear`).
    pub fn clear(&mut self) {
        *self = Polygon::new(self.polyline);
    }

    /// The number of points.
    pub fn len(&self) -> u32 {
        self.num
    }

    /// Whether it has no points.
    pub fn is_empty(&self) -> bool {
        self.num == 0
    }

    /// Add a vertex (`geod_polygon_addpoint`).
    pub fn add_point(&mut self, g: &Geodesic, lat: f64, lon: f64) {
        if self.num == 0 {
            self.lat0 = lat;
            self.lat = lat;
            self.lon0 = lon;
            self.lon = lon;
        } else {
            let mask = if self.polyline {
                Mask::DISTANCE
            } else {
                Mask::DISTANCE | Mask::AREA
            };
            let r = g.gen_inverse(self.lat, self.lon, lat, lon, mask);
            self.perimeter.add(r.s12);
            if !self.polyline {
                self.area.add(r.S12);
                self.crossings += transit(self.lon, lon);
            }
            self.lat = lat;
            self.lon = lon;
        }
        self.num += 1;
    }

    /// Add an edge of length `s` along azimuth `azi` from the last vertex;
    /// nothing happens without one (`geod_polygon_addedge`).
    pub fn add_edge(&mut self, g: &Geodesic, azi: f64, s: f64) {
        if self.num == 0 {
            return;
        }
        let mask = Mask::LATITUDE
            | Mask::LONGITUDE
            | if self.polyline {
                Mask::NONE
            } else {
                Mask::AREA
            };
        let p = g.gen_direct(self.lat, self.lon, azi, unrolled(), s, mask);
        self.perimeter.add(s);
        if !self.polyline {
            self.area.add(p.S12);
            self.crossings += transit_direct(self.lon, p.lon2);
        }
        self.lat = p.lat2;
        self.lon = p.lon2;
        self.num += 1;
    }

    /// The number of points, the area and the perimeter
    /// (`geod_polygon_compute`). `reverse` makes clockwise positive;
    /// `sign` allows a negative area, else the area is in [0, ellipsoid
    /// area). A polyline's area is NaN.
    pub fn compute(&self, g: &Geodesic, reverse: bool, sign: bool) -> (u32, f64, f64) {
        if self.num < 2 {
            return (self.num, if self.polyline { f64::NAN } else { 0.0 }, 0.0);
        }
        if self.polyline {
            return (self.num, f64::NAN, self.perimeter.0);
        }
        let r = g.gen_inverse(
            self.lat,
            self.lon,
            self.lat0,
            self.lon0,
            Mask::DISTANCE | Mask::AREA,
        );
        let perimeter = self.perimeter.sum(r.s12);
        let mut t = self.area;
        t.add(r.S12);
        let area = area_reduce_a(
            t,
            g.ellipsoid_area(),
            self.crossings + transit(self.lon, self.lon0),
            reverse,
            sign,
        );
        (self.num, area, perimeter)
    }

    /// As [`Polygon::compute`] with one more vertex, without adding it
    /// (`geod_polygon_testpoint`).
    pub fn test_point(
        &self,
        g: &Geodesic,
        lat: f64,
        lon: f64,
        reverse: bool,
        sign: bool,
    ) -> (u32, f64, f64) {
        let num = self.num + 1;
        if num == 1 {
            return (num, if self.polyline { f64::NAN } else { 0.0 }, 0.0);
        }
        let mut perimeter = self.perimeter.0;
        let mut tempsum = if self.polyline { 0.0 } else { self.area.0 };
        let mut crossings = self.crossings;
        for i in 0..(if self.polyline { 1 } else { 2 }) {
            let (la1, lo1, la2, lo2) = if i == 0 {
                (self.lat, self.lon, lat, lon)
            } else {
                (lat, lon, self.lat0, self.lon0)
            };
            let mask = if self.polyline {
                Mask::DISTANCE
            } else {
                Mask::DISTANCE | Mask::AREA
            };
            let r = g.gen_inverse(la1, lo1, la2, lo2, mask);
            perimeter += r.s12;
            if !self.polyline {
                tempsum += r.S12;
                crossings += transit(lo1, lo2);
            }
        }
        if self.polyline {
            return (num, f64::NAN, perimeter);
        }
        let area = area_reduce_b(tempsum, g.ellipsoid_area(), crossings, reverse, sign);
        (num, area, perimeter)
    }

    /// As [`Polygon::compute`] with one more edge, without adding it
    /// (`geod_polygon_testedge`). Without a vertex, the number is 0 and
    /// the rest NaN.
    pub fn test_edge(
        &self,
        g: &Geodesic,
        azi: f64,
        s: f64,
        reverse: bool,
        sign: bool,
    ) -> (u32, f64, f64) {
        let num = self.num + 1;
        if num == 1 {
            return (0, f64::NAN, f64::NAN);
        }
        let mut perimeter = self.perimeter.0 + s;
        if self.polyline {
            return (num, f64::NAN, perimeter);
        }
        let mut tempsum = self.area.0;
        let mut crossings = self.crossings;
        let p = g.gen_direct(
            self.lat,
            self.lon,
            azi,
            unrolled(),
            s,
            Mask::LATITUDE | Mask::LONGITUDE | Mask::AREA,
        );
        tempsum += p.S12;
        crossings += transit_direct(self.lon, p.lon2);
        let r = g.gen_inverse(
            p.lat2,
            p.lon2,
            self.lat0,
            self.lon0,
            Mask::DISTANCE | Mask::AREA,
        );
        perimeter += r.s12;
        tempsum += r.S12;
        crossings += transit(p.lon2, self.lon0);
        let area = area_reduce_b(tempsum, g.ellipsoid_area(), crossings, reverse, sign);
        (num, area, perimeter)
    }
}

fn unrolled() -> Flags {
    Flags {
        arc_mode: false,
        long_unroll: true,
    }
}

/// PROJ's `src/tests/geodsigntest.c` (MIT): the handling of ±0 and ±180
/// in the angle helpers and at the edges of the inverse and direct
/// problems.
#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f64 = f64::EPSILON;

    fn wgs84() -> Geodesic {
        Geodesic::new(6378137.0, 1.0 / 298.257223563)
    }

    /// Equal, with the same sign of zero, or both NaN.
    fn equiv(x: f64, y: f64) -> bool {
        (x.is_nan() && y.is_nan()) || (x == y && x.is_sign_negative() == y.is_sign_negative())
    }

    #[track_caller]
    fn check(x: f64, y: f64) {
        assert!(equiv(x, y), "{x:e} != {y:e}");
    }

    #[test]
    fn ang_round_signs() {
        let cases = [
            (-EPS / 32.0, -EPS / 32.0),
            (-EPS / 64.0, -0.0),
            (-0.0, -0.0),
            (0.0, 0.0),
            (EPS / 64.0, 0.0),
            (EPS / 32.0, EPS / 32.0),
            ((1.0 - 2.0 * EPS) / 64.0, (1.0 - 2.0 * EPS) / 64.0),
            ((1.0 - EPS) / 64.0, 1.0 / 64.0),
            ((1.0 - EPS / 2.0) / 64.0, 1.0 / 64.0),
            ((1.0 - EPS / 4.0) / 64.0, 1.0 / 64.0),
            (1.0 / 64.0, 1.0 / 64.0),
            ((1.0 + EPS / 2.0) / 64.0, 1.0 / 64.0),
            ((1.0 + EPS) / 64.0, 1.0 / 64.0),
            ((1.0 + 2.0 * EPS) / 64.0, (1.0 + 2.0 * EPS) / 64.0),
            ((1.0 - EPS) / 32.0, (1.0 - EPS) / 32.0),
            ((1.0 - EPS / 2.0) / 32.0, 1.0 / 32.0),
            ((1.0 - EPS / 4.0) / 32.0, 1.0 / 32.0),
            (1.0 / 32.0, 1.0 / 32.0),
            ((1.0 + EPS / 2.0) / 32.0, 1.0 / 32.0),
            ((1.0 + EPS) / 32.0, (1.0 + EPS) / 32.0),
            ((1.0 - EPS) / 16.0, (1.0 - EPS) / 16.0),
            ((1.0 - EPS / 2.0) / 16.0, (1.0 - EPS / 2.0) / 16.0),
            ((1.0 - EPS / 4.0) / 16.0, 1.0 / 16.0),
            (1.0 / 16.0, 1.0 / 16.0),
            ((1.0 + EPS / 4.0) / 16.0, 1.0 / 16.0),
            ((1.0 + EPS / 2.0) / 16.0, 1.0 / 16.0),
            ((1.0 + EPS) / 16.0, (1.0 + EPS) / 16.0),
            ((1.0 - EPS) / 8.0, (1.0 - EPS) / 8.0),
            ((1.0 - EPS / 2.0) / 8.0, (1.0 - EPS / 2.0) / 8.0),
            ((1.0 - EPS / 4.0) / 8.0, 1.0 / 8.0),
            ((1.0 + EPS / 2.0) / 8.0, 1.0 / 8.0),
            ((1.0 + EPS) / 8.0, (1.0 + EPS) / 8.0),
            (1.0 - EPS, 1.0 - EPS),
            (1.0 - EPS / 2.0, 1.0 - EPS / 2.0),
            (1.0 - EPS / 4.0, 1.0),
            (1.0, 1.0),
            (1.0 + EPS / 4.0, 1.0),
            (1.0 + EPS / 2.0, 1.0),
            (1.0 + EPS, 1.0 + EPS),
            (90.0 - 64.0 * EPS, 90.0 - 64.0 * EPS),
            (90.0 - 32.0 * EPS, 90.0),
            (90.0, 90.0),
        ];
        for (x, y) in cases {
            check(ang_round(x), y);
        }
    }

    #[test]
    fn sincosd_signs() {
        let inf = f64::INFINITY;
        let nan = f64::NAN;
        let cases = [
            (-inf, nan, nan),
            (-810.0, -1.0, 0.0),
            (-720.0, -0.0, 1.0),
            (-630.0, 1.0, 0.0),
            (-540.0, -0.0, -1.0),
            (-450.0, -1.0, 0.0),
            (-360.0, -0.0, 1.0),
            (-270.0, 1.0, 0.0),
            (-180.0, -0.0, -1.0),
            (-90.0, -1.0, 0.0),
            (-0.0, -0.0, 1.0),
            (0.0, 0.0, 1.0),
            (90.0, 1.0, 0.0),
            (180.0, 0.0, -1.0),
            (270.0, -1.0, 0.0),
            (360.0, 0.0, 1.0),
            (450.0, 1.0, 0.0),
            (540.0, 0.0, -1.0),
            (630.0, -1.0, 0.0),
            (720.0, 0.0, 1.0),
            (810.0, 1.0, 0.0),
            (inf, nan, nan),
            (nan, nan, nan),
        ];
        for (x, s, c) in cases {
            let (sx, cx) = sincosdx(x);
            assert!(equiv(sx, s) && equiv(cx, c), "sincosd({x}) = ({sx}, {cx})");
        }
        let (s1, c1) = sincosdx(9.0);
        let (s2, c2) = sincosdx(81.0);
        let (s3, c3) = sincosdx(-123456789.0);
        assert!(equiv(s1, c2) && equiv(s1, s3) && equiv(c1, s2) && equiv(c1, -c3));
    }

    #[test]
    fn atan2d_signs() {
        let inf = f64::INFINITY;
        let nan = f64::NAN;
        let cases = [
            (0.0, -0.0, 180.0),
            (-0.0, -0.0, -180.0),
            (0.0, 0.0, 0.0),
            (-0.0, 0.0, -0.0),
            (0.0, -1.0, 180.0),
            (-0.0, -1.0, -180.0),
            (0.0, 1.0, 0.0),
            (-0.0, 1.0, -0.0),
            (-1.0, 0.0, -90.0),
            (-1.0, -0.0, -90.0),
            (1.0, 0.0, 90.0),
            (1.0, -0.0, 90.0),
            (1.0, -inf, 180.0),
            (-1.0, -inf, -180.0),
            (1.0, inf, 0.0),
            (-1.0, inf, -0.0),
            (inf, 1.0, 90.0),
            (inf, -1.0, 90.0),
            (-inf, 1.0, -90.0),
            (-inf, -1.0, -90.0),
            (inf, -inf, 135.0),
            (-inf, -inf, -135.0),
            (inf, inf, 45.0),
            (-inf, inf, -45.0),
            (nan, 1.0, nan),
            (1.0, nan, nan),
        ];
        for (y, x, a) in cases {
            check(atan2dx(y, x), a);
        }
        let s = 7e-16;
        check(atan2dx(s, -1.0), 180.0 - atan2dx(s, 1.0));
    }

    #[test]
    fn sum_and_angle_signs() {
        for (u, v, s) in [
            (9.0, -9.0, 0.0),
            (-9.0, 9.0, 0.0),
            (-0.0, 0.0, 0.0),
            (0.0, -0.0, 0.0),
            (-0.0, -0.0, -0.0),
            (0.0, 0.0, 0.0),
        ] {
            check(sumx(u, v).0, s);
        }
        for (x, y) in [
            (-900.0, -180.0),
            (-720.0, -0.0),
            (-540.0, -180.0),
            (-360.0, -0.0),
            (-180.0, -180.0),
            (-0.0, -0.0),
            (0.0, 0.0),
            (180.0, 180.0),
            (360.0, 0.0),
            (540.0, 180.0),
            (720.0, 0.0),
            (900.0, 180.0),
        ] {
            check(ang_normalize(x), y);
        }
        for (x, y, d) in [
            (0.0, 0.0, 0.0),
            (0.0, -0.0, -0.0),
            (-0.0, 0.0, 0.0),
            (-0.0, -0.0, 0.0),
            (5.0, 365.0, 0.0),
            (365.0, 5.0, -0.0),
            (5.0, 185.0, 180.0),
            (185.0, 5.0, -180.0),
            (EPS, 180.0, 180.0),
            (-EPS, 180.0, -180.0),
            (EPS, -180.0, 180.0),
            (-EPS, -180.0, -180.0),
        ] {
            check(ang_diff(x, y).0, d);
        }
        check(ang_diff(138.0 + 128.0 * EPS, -164.0).0, 58.0 - 128.0 * EPS);
    }

    #[test]
    fn equatorial_signs() {
        let g = wgs84();
        // Coincident points on the equator: the signs of the latitudes
        // decide the azimuth.
        for (lat1, lat2, azi) in [(0.0, -0.0, 180.0), (-0.0, 0.0, 0.0)] {
            let (_, azi1, azi2) = g.inverse(lat1, 0.0, lat2, 0.0);
            check(azi1, azi);
            check(azi2, azi);
        }
        // Nearly antipodal: north or south.
        for (lat, a1, a2) in [(0.0, 56.0, 124.0), (-0.0, 124.0, 56.0)] {
            let (_, azi1, azi2) = g.inverse(lat, 0.0, lat, 179.5);
            assert!((azi1 - a1).abs() <= 1.0 && (azi2 - a2).abs() <= 1.0);
        }
        // Antipodal.
        for (lat, lon2, a1, a2) in [
            (0.0, 180.0, 0.0, 180.0),
            (-0.0, 180.0, 180.0, 0.0),
            (0.0, -180.0, -0.0, -180.0),
            (-0.0, -180.0, -180.0, -0.0),
        ] {
            let (_, azi1, azi2) = g.inverse(lat, 0.0, lat, lon2);
            check(azi1, a1);
            check(azi2, a2);
        }
        let prolate = Geodesic::new(6.4e6, -1.0 / 300.0);
        for (lon2, azi) in [(180.0, 90.0), (-180.0, -90.0)] {
            let (_, azi1, azi2) = prolate.inverse(0.0, 0.0, 0.0, lon2);
            check(azi1, azi);
            check(azi2, azi);
        }
        // Azimuths of ±0 and ±180 in the direct problem.
        for (azi1, lon, azi) in [
            (0.0, 180.0, 180.0),
            (-0.0, -180.0, -180.0),
            (180.0, 180.0, 0.0),
            (-180.0, -180.0, -0.0),
        ] {
            let p = g.gen_direct(
                0.0,
                0.0,
                azi1,
                unrolled(),
                15e6,
                Mask::LONGITUDE | Mask::AZIMUTH,
            );
            check(p.lon2, lon);
            check(p.azi2, azi);
        }
    }

    #[test]
    fn remquo_ties_to_even() {
        assert_eq!(remquo(45.0, 90.0), (45.0, 0));
        assert_eq!(remquo(135.0, 90.0), (-45.0, 2));
        assert_eq!(remquo(225.0, 90.0), (45.0, 2));
        assert_eq!(remquo(-810.0, 90.0).1.rem_euclid(4), 3);
        assert_eq!(remquo(-45.0, 90.0), (-45.0, 0));
        assert_eq!(remquo(-135.0, 90.0), (45.0, -2));
        check(remainder(-720.0, 360.0), -0.0);
        check(remainder(1e300, 360.0), 0.0);
        check(remainder(1e17 + 200.0, 360.0), 112.0);
    }
}
