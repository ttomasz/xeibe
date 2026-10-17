//! New Zealand Map Grid (EPSG:9811).
//!
//! Ported from PROJ 9.8.1 `src/projections/nzmg.cpp` (MIT, see
//! PROJ-NOTICE.md): Merrifield's complex polynomial, defined for the
//! International 1924 ellipsoid only. PROJ uses its semi-major axis whatever
//! the CRS says, so the ellipsoid's own is used here; the series' constants
//! are for that ellipsoid.

use super::{BuildError, Params, Projection, param};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

const EPSLN: f64 = 1e-10;
const SEC5_TO_RAD: f64 = 0.484_813_681_109_536;
const RAD_TO_SEC5: f64 = 2.062_648_062_470_963_8;

const BF: [(f64, f64); 6] = [
    (0.7557853228, 0.0),
    (0.249204646, 0.003371507),
    (-0.001541739, 0.041058560),
    (-0.10162907, 0.01727609),
    (-0.26623489, -0.36249218),
    (-0.6870983, -1.1651967),
];

const TPSI: [f64; 10] = [
    0.6399175073,
    -0.1358797613,
    0.063294409,
    -0.02526853,
    0.0117879,
    -0.0055161,
    0.0026906,
    -0.001333,
    0.00067,
    -0.00034,
];

const TPHI: [f64; 9] = [
    1.5627014243,
    0.5185406398,
    -0.03333098,
    -0.1052906,
    -0.0368594,
    0.007317,
    0.01220,
    0.00394,
    -0.0013,
];

type Complex = (f64, f64);

/// Complex polynomial `z * (C0 + z (C1 + ...))` (PROJ's `pj_zpoly1`).
fn zpoly1(z: Complex, c: &[Complex]) -> Complex {
    let n = c.len() - 1;
    let mut a = c[n];
    for k in (0..n).rev() {
        let t = a.0;
        a = (c[k].0 + z.0 * t - z.1 * a.1, c[k].1 + z.0 * a.1 + z.1 * t);
    }
    let t = a.0;
    (z.0 * t - z.1 * a.1, z.0 * a.1 + z.1 * t)
}

/// As `zpoly1`, with the derivative (PROJ's `pj_zpolyd1`).
fn zpolyd1(z: Complex, c: &[Complex]) -> (Complex, Complex) {
    let mut n = c.len() - 1;
    let mut a = c[n];
    let mut b = a;
    let mut first = true;
    while n > 0 {
        n -= 1;
        if first {
            first = false;
        } else {
            let t = b.0;
            b = (a.0 + z.0 * t - z.1 * b.1, a.1 + z.0 * b.1 + z.1 * t);
        }
        let t = a.0;
        a = (c[n].0 + z.0 * t - z.1 * a.1, c[n].1 + z.0 * a.1 + z.1 * t);
    }
    let t = b.0;
    b = (a.0 + z.0 * t - z.1 * b.1, a.1 + z.0 * b.1 + z.1 * t);
    let t = a.0;
    a = (z.0 * t - z.1 * a.1, z.0 * a.1 + z.1 * t);
    (a, b)
}

#[derive(Debug)]
pub(crate) struct NewZealandMapGrid {
    lam0: f64,
    phi0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
}

impl NewZealandMapGrid {
    pub(crate) fn new(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        Ok(NewZealandMapGrid {
            lam0: params.get(param::LON_NATURAL_ORIGIN)?,
            phi0: params.get(param::LAT_NATURAL_ORIGIN)?,
            fe: params.get(param::FALSE_EASTING)?,
            fn_: params.get(param::FALSE_NORTHING)?,
            a: ellipsoid.a,
        })
    }
}

impl Projection for NewZealandMapGrid {
    fn forward(&self, lon: f64, lat: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = super::adjlon(lon - self.lam0);
        let phi = (lat - self.phi0) * RAD_TO_SEC5;
        let mut r = TPSI[TPSI.len() - 1];
        for c in TPSI[..TPSI.len() - 1].iter().rev() {
            r = c + phi * r;
        }
        r *= phi;
        let p = zpoly1((r, lam), &BF);
        Ok((self.a * p.1 + self.fe, self.a * p.0 + self.fn_))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let x = (x - self.fe) / self.a;
        let y = (y - self.fn_) / self.a;
        let mut p = (y, x);
        let mut converged = false;
        for _ in 0..20 {
            let (mut f, fp) = zpolyd1(p, &BF);
            f.0 -= y;
            f.1 -= x;
            let den = fp.0 * fp.0 + fp.1 * fp.1;
            let dp = (
                -(f.0 * fp.0 + f.1 * fp.1) / den,
                -(f.1 * fp.0 - f.0 * fp.1) / den,
            );
            p.0 += dp.0;
            p.1 += dp.1;
            if dp.0.abs() + dp.1.abs() <= EPSLN {
                converged = true;
                break;
            }
        }
        if !converged {
            return Err(CoordinateError::NoConvergence);
        }
        let mut phi = TPHI[TPHI.len() - 1];
        for c in TPHI[..TPHI.len() - 1].iter().rev() {
            phi = c + p.0 * phi;
        }
        Ok((p.1 + self.lam0, self.phi0 + p.0 * phi * SEC5_TO_RAD))
    }
}
