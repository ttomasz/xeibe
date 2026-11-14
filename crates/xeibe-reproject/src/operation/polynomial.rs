//! EPSG's polynomial transformations (EPSG Guidance Note 7-2), which PROJ
//! doesn't import: the general polynomials of degree 2 and 6 (9645, 9648),
//! the reversible ones of degree 4 and 13 (9651, 9654), the complex ones of
//! degree 3 and 4 (9652, 9653) and Madrid to ED50 (9617). Those in
//! [`COMPETING`] are in the choice; the others run only when forced
//! (`operation::only_forced`).
//!
//! A polynomial works on the ordinates as the CRS gives them, in its axis
//! order and unit (degrees, metres, links), reduced to an evaluation point
//! and scaled:
//!
//! * general: U = mS (XS − XS0), V = mS (YS − YS0), mT dX = Σ Aij U^i V^j,
//!   mT dY = Σ Bij U^i V^j, XT = XS − XS0 + XT0 + dX;
//! * reversible: the same with one evaluation point and one scaling factor
//!   m, U and V scaled by it and the differences not; the reverse is the
//!   same polynomial with the coefficients' signs reversed. Degree 13's one
//!   operation, "ED50 to WGS 84 (35)", is NIMA's TR8350.2 multiple
//!   regression equations, whose differences are in arc-seconds although
//!   EPSG gives them in degrees: as degrees they would move a point 2.6°,
//!   as arc-seconds they are what EPSG says "ED50 to WGS 84 (36)" (code 1998)
//!   approximates to 0.5 m, and they agree with it to 0.4 m;
//! * complex: mT (dX + i dY) = Σ (A(2k−1) + i A(2k)) (U + i V)^k;
//! * Madrid to ED50: dφ″ = A0 + A1 φ + A2 λ + A3 H and dλ″ = B00 + B0 +
//!   B1 φ + B2 λ + B3 H, in degrees from the Madrid meridian.
//!
//! U and V are the source ordinates in the source CRS's order, A gives the
//! target's first ordinate and B its second, and each target ordinate adds
//! the source ordinate of its own direction: where one CRS lists its
//! northing first and the other its easting ("Viti Levu 1912 / Viti Levu
//! Grid to Fiji 1986 / Fiji Map Grid (1)"), the easting goes with the
//! easting. That is the only reading under which EPSG's remark holds (its
//! Au0v1 and Bu1v0 are DLAS's "modified by subtracting (1/ms)", which
//! cancels the source easting added to the target's) and which agrees with
//! the route through the CRSs' transformations to WGS 84.
//!
//! The reverse of the others is by iteration, as EPSG says to do ("Use
//! iteration for reverse transformation ETRS89 to TM75").

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use super::{Method, MethodError};
use crate::crs::{Axes, Crs};
use crate::error::CoordinateError;
use crate::registry::Operation;

/// The polynomials in the choice, each better than what PROJ chooses for
/// the pair, or alone:
///
/// * TM75 to ETRS89 (1) and to WGS 84 (1): "Recommended by OSi and OSNI for
///   all horizontal CTs" (0.4 m); PROJ takes "TM75 to ETRS89 (2)" (1 m),
///   since the NTv2 grid that emulates the polynomial (3) isn't on its CDN;
/// * Viti Levu and Vanua Levu to the Fiji Map Grid (1): DLAS's, 1 m, where
///   PROJ goes through "Viti Levu 1912 to WGS 84 (2)" (5 m);
/// * Madrid 1870 (Madrid) to ED50 (2) and (3): the only live operations for
///   Madrid 1870, which PROJ can't transform at all;
/// * ED50 to ED87 (1), as accurate as the Helmert (2), the two being EPSG's
///   only operations between the datums.
///
/// Not the others: NAM's RD to ED50 / UTM and TM 5 NE polynomials, which
/// EPSG says NAM replaced in 2006 (and whose first variants it keeps for
/// legacy data), ED50 to WGS 84 (35) and Madrid to ED50 (1), which EPSG
/// supersedes, and BD72 / Belge Lambert 72 to ED50 / UTM zone 31N and back,
/// whose reverse (6305) doesn't reproduce the forward from EPSG's data.
pub(crate) const COMPETING: &[u32] = &[1041, 1042, 15950, 15951, 1027, 1028, 15753];

/// EPSG method codes handled here.
pub(super) const METHODS: &[u32] = &[
    GENERAL_2,
    GENERAL_6,
    REVERSIBLE_4,
    REVERSIBLE_13,
    COMPLEX_3,
    COMPLEX_4,
    MADRID,
];
const GENERAL_2: u32 = 9645;
const GENERAL_6: u32 = 9648;
const REVERSIBLE_4: u32 = 9651;
const REVERSIBLE_13: u32 = 9654;
const COMPLEX_3: u32 = 9652;
const COMPLEX_4: u32 = 9653;
const MADRID: u32 = 9617;

/// "Ordinate 1/2 of evaluation point" (reversible), and "… in source CRS",
/// "… in target CRS" (the others).
const EVALUATION: [u32; 2] = [8617, 8618];
const EVALUATION_SOURCE: [u32; 2] = [8619, 8620];
const EVALUATION_TARGET: [u32; 2] = [8621, 8622];
/// "Scaling factor for source/target CRS coord differences", and the one
/// of a reversible polynomial.
const SCALE_SOURCE: u32 = 8694;
const SCALE_TARGET: u32 = 8695;
const SCALE: u32 = 8696;
/// The complex coefficients A1 to A8.
const COMPLEX: [u32; 8] = [8624, 8625, 8626, 8627, 8628, 8629, 8630, 8631];
/// Madrid to ED50: A0 to A3, B00, B0 to B3.
const MADRID_A: [u32; 4] = [8623, 8624, 8625, 8626];
const MADRID_B00: u32 = 8638;
const MADRID_B: [u32; 4] = [8639, 8640, 8641, 8642];

const MAX_ITERATIONS: usize = 20;
const ARC_SECOND: f64 = std::f64::consts::PI / 648_000.0;

/// The general polynomials' coefficients by parameter code: (B, i, j) for
/// "Au{i}v{j}" (false) and "Bu{i}v{j}" (true), A0 and B0 as u0v0.
fn powers() -> &'static HashMap<u32, (bool, u32, u32)> {
    static POWERS: OnceLock<HashMap<u32, (bool, u32, u32)>> = OnceLock::new();
    POWERS.get_or_init(|| {
        let mut out = HashMap::new();
        for (b, letter) in [(false, 'A'), (true, 'B')] {
            if let Some(code) = xeibe_crs::parameter_code(&format!("{letter}0")) {
                out.insert(code, (b, 0, 0));
            }
            for i in 0..=13 {
                for j in 0..=13 {
                    if let Some(code) = xeibe_crs::parameter_code(&format!("{letter}u{i}v{j}")) {
                        out.insert(code, (b, i, j));
                    }
                }
            }
        }
        out
    })
}

#[derive(Debug)]
enum Kind {
    /// Σ c U^i V^j, for dX and dY.
    General {
        a: Vec<(i32, i32, f64)>,
        b: Vec<(i32, i32, f64)>,
    },
    /// The complex coefficients, A(2k−1) + i A(2k) for the k-th power.
    Complex(Vec<(f64, f64)>),
    /// A0 to A3; B00 in arc-seconds; B0 to B3.
    Madrid { a: [f64; 4], b00: f64, b: [f64; 4] },
}

#[derive(Debug)]
struct Polynomial {
    kind: Kind,
    /// Evaluation points in the source and target CRS, in their units.
    src0: [f64; 2],
    dst0: [f64; 2],
    ms: f64,
    mt: f64,
    /// Reversible: the reverse is the same polynomial with the signs of
    /// its coefficients reversed.
    reversible: bool,
    /// One CRS lists its northing first and the other its easting: each
    /// target ordinate adds the source's other one.
    crossed: bool,
    axes: (Axes, Axes),
}

fn number(op: &Operation, code: u32) -> Result<f64, MethodError> {
    op.param(code).ok_or(MethodError::MissingParameter(code))
}

/// An evaluation point stored in SI units, in the CRS's own.
fn point(op: &Operation, codes: [u32; 2], axes: &Axes) -> Result<[f64; 2], MethodError> {
    Ok([
        number(op, codes[0])? / axes.axes[0].factor,
        number(op, codes[1])? / axes.axes[1].factor,
    ])
}

pub(super) fn build(op: &Operation, src: &Crs, dst: &Crs) -> Result<Arc<dyn Method>, MethodError> {
    if src.axes.axes.len() < 2 || dst.axes.axes.len() < 2 {
        return Err(MethodError::WrongCrs("CRSs with two horizontal axes"));
    }
    let axes = (src.axes.clone(), dst.axes.clone());
    let reversible = matches!(op.method, REVERSIBLE_4 | REVERSIBLE_13);
    let (src0, dst0, ms, mt) = match op.method {
        MADRID => ([0.0; 2], [0.0; 2], 1.0, 1.0),
        REVERSIBLE_4 | REVERSIBLE_13 => {
            let p = point(op, EVALUATION, &axes.0)?;
            // mT divides the differences: 3600 makes arc-seconds degrees.
            let mt = if op.method == REVERSIBLE_13 {
                3600.0
            } else {
                1.0
            };
            (p, p, op.param(SCALE).unwrap_or(1.0), mt)
        }
        _ => (
            point(op, EVALUATION_SOURCE, &axes.0)?,
            point(op, EVALUATION_TARGET, &axes.1)?,
            number(op, SCALE_SOURCE)?,
            number(op, SCALE_TARGET)?,
        ),
    };
    let kind = match op.method {
        GENERAL_2 | GENERAL_6 | REVERSIBLE_4 | REVERSIBLE_13 => {
            let (mut a, mut b) = (Vec::new(), Vec::new());
            for (code, value) in &op.params {
                let (Some(&(is_b, i, j)), crate::registry::ParamValue::Number(c)) =
                    (powers().get(code), value)
                else {
                    continue;
                };
                let term = (i as i32, j as i32, *c);
                if is_b { b.push(term) } else { a.push(term) }
            }
            Kind::General { a, b }
        }
        COMPLEX_3 | COMPLEX_4 => {
            let degree = if op.method == COMPLEX_3 { 3 } else { 4 };
            let mut c = Vec::new();
            for k in 0..degree {
                c.push((number(op, COMPLEX[2 * k])?, number(op, COMPLEX[2 * k + 1])?));
            }
            Kind::Complex(c)
        }
        MADRID => Kind::Madrid {
            a: [
                number(op, MADRID_A[0])?,
                number(op, MADRID_A[1])?,
                number(op, MADRID_A[2])?,
                number(op, MADRID_A[3])?,
            ],
            b00: number(op, MADRID_B00)? / ARC_SECOND,
            b: [
                number(op, MADRID_B[0])?,
                number(op, MADRID_B[1])?,
                number(op, MADRID_B[2])?,
                number(op, MADRID_B[3])?,
            ],
        },
        _ => return Err(MethodError::NotImplemented),
    };
    let crossed = axes.0.first_is_northing() != axes.1.first_is_northing();
    Ok(Arc::new(Polynomial {
        kind,
        src0,
        dst0,
        ms,
        mt,
        reversible,
        crossed,
        axes,
    }))
}

fn sum(terms: &[(i32, i32, f64)], u: f64, v: f64, sign: f64) -> f64 {
    terms
        .iter()
        .map(|&(i, j, c)| sign * c * u.powi(i) * v.powi(j))
        .sum()
}

impl Polynomial {
    /// The target ordinates of source ordinates `x` (in the CRSs' units and
    /// axis order), the height being the source's; `reverse` applies a
    /// reversible polynomial's reverse.
    fn evaluate(&self, x: [f64; 3], reverse: bool) -> [f64; 3] {
        let (from0, to0) = if reverse {
            (self.dst0, self.src0)
        } else {
            (self.src0, self.dst0)
        };
        let (dx, dy) = match &self.kind {
            Kind::General { a, b } => {
                let (u, v) = (self.ms * (x[0] - from0[0]), self.ms * (x[1] - from0[1]));
                let sign = if reverse { -1.0 } else { 1.0 };
                (sum(a, u, v, sign) / self.mt, sum(b, u, v, sign) / self.mt)
            }
            Kind::Complex(c) => {
                let (u, v) = (self.ms * (x[0] - from0[0]), self.ms * (x[1] - from0[1]));
                let (mut re, mut im) = (0.0, 0.0);
                let (mut pr, mut pi) = (1.0, 0.0);
                for &(cr, ci) in c {
                    // (pr + i pi) · (u + i v)
                    (pr, pi) = (pr * u - pi * v, pr * v + pi * u);
                    re += cr * pr - ci * pi;
                    im += cr * pi + ci * pr;
                }
                (re / self.mt, im / self.mt)
            }
            Kind::Madrid { a, b00, b } => {
                let (lat, lon, h) = (x[0], x[1], x[2]);
                let dlat = a[0] + a[1] * lat + a[2] * lon + a[3] * h;
                let dlon = b00 + b[0] + b[1] * lat + b[2] * lon + b[3] * h;
                (dlat / 3600.0, dlon / 3600.0)
            }
        };
        let (a, b) = (x[0] - from0[0], x[1] - from0[1]);
        let (a, b) = if self.crossed { (b, a) } else { (a, b) };
        [a + to0[0] + dx, b + to0[1] + dy, x[2]]
    }

    /// The source ordinates the forward takes to `y`, by Newton's method
    /// on the two horizontal ordinates, from the reverse of the offset at
    /// `y`.
    fn solve(&self, y: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        let at = |x: [f64; 3]| self.evaluate(x, false);
        let first = at(y);
        let mut x = [2.0 * y[0] - first[0], 2.0 * y[1] - first[1], y[2]];
        for _ in 0..MAX_ITERATIONS {
            let f = at(x);
            let r = [f[0] - y[0], f[1] - y[1]];
            let scale = x[0].abs().max(x[1].abs()).max(1.0);
            if r[0].abs().max(r[1].abs()) <= 1e-12 * scale {
                return Ok(x);
            }
            let h = 1e-6 * scale;
            let fx = at([x[0] + h, x[1], x[2]]);
            let fy = at([x[0], x[1] + h, x[2]]);
            let j = [
                [(fx[0] - f[0]) / h, (fy[0] - f[0]) / h],
                [(fx[1] - f[1]) / h, (fy[1] - f[1]) / h],
            ];
            let det = j[0][0] * j[1][1] - j[0][1] * j[1][0];
            if det == 0.0 || !det.is_finite() {
                return Err(CoordinateError::NoConvergence);
            }
            x[0] -= (j[1][1] * r[0] - j[0][1] * r[1]) / det;
            x[1] -= (j[0][0] * r[1] - j[1][0] * r[0]) / det;
        }
        Err(CoordinateError::NoConvergence)
    }
}

impl Method for Polynomial {
    fn forward(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        let (from, to) = &self.axes;
        Ok(to.to_internal(self.evaluate(from.to_crs(c), false)))
    }

    fn inverse(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        let (from, to) = &self.axes;
        let y = to.to_crs(c);
        let x = if self.reversible {
            self.evaluate(y, true)
        } else {
            self.solve(y)?
        };
        Ok(from.to_internal(x))
    }
}
