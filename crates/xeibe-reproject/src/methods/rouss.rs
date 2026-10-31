//! Roussilhe Stereographic, on an ellipsoid, and the meridian distance
//! series it uses.
//!
//! Ported from PROJ 9.8.1 `src/projections/rouss.cpp` and
//! `src/proj_mdist.cpp` (MIT, see PROJ-NOTICE.md).

use super::conic::k0;
use super::{BuildError, Params, Projection, adjlon};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

/// Meridian distance as a series in e² (PROJ's `proj_mdist_ini`).
#[derive(Debug)]
struct Mdist {
    es: f64,
    e: f64,
    b: Vec<f64>,
}

impl Mdist {
    fn new(es: f64) -> Self {
        const MAX_ITER: usize = 20;
        let mut e = [0.0; MAX_ITER];
        e[0] = 1.0;
        let (mut ens, mut numf, mut twon1, mut denfi, mut denf, mut twon) =
            (es, 1.0, 1.0, 1.0, 1.0, 4.0);
        let (mut el, mut big_es) = (1.0, 1.0);
        let mut i = 1;
        while i < MAX_ITER {
            numf *= twon1 * twon1;
            let den = twon * denf * denf * twon1;
            e[i] = numf / den * ens;
            big_es -= e[i];
            ens *= es;
            twon *= 4.0;
            denfi += 1.0;
            denf *= denfi;
            twon1 += 2.0;
            // No change: stop.
            if big_es == el {
                break;
            }
            el = big_es;
            i += 1;
        }
        let total = big_es;
        let mut b = Vec::with_capacity(i);
        let mut s = 1.0 - big_es;
        b.push(s);
        let (mut numf, mut denf, mut numfi, mut denfi) = (1.0, 1.0, 2.0, 3.0);
        for ej in e.iter().take(i).skip(1) {
            s -= ej;
            numf *= numfi;
            denf *= denfi;
            b.push(s * numf / denf);
            numfi += 2.0;
            denfi += 2.0;
        }
        Mdist { es, e: total, b }
    }

    fn dist(&self, phi: f64, sphi: f64, cphi: f64) -> f64 {
        let sc = sphi * cphi;
        let sphi2 = sphi * sphi;
        let d = phi * self.e - self.es * sc / (1.0 - self.es * sphi2).sqrt();
        let sum = self.b.iter().rev().fold(0.0, |sum, &b| b + sphi2 * sum);
        d + sc * sum
    }

    fn inv(&self, dist: f64) -> Result<f64, CoordinateError> {
        let k = 1.0 / (1.0 - self.es);
        let mut phi = dist;
        for _ in 0..20 {
            let s = phi.sin();
            let t = 1.0 - self.es * s * s;
            let t = (self.dist(phi, s, phi.cos()) - dist) * (t * t.sqrt()) * k;
            phi -= t;
            if t.abs() < 1e-14 {
                return Ok(phi);
            }
        }
        Err(CoordinateError::OutsideDomain)
    }
}

#[derive(Debug)]
pub(crate) struct Roussilhe {
    lon0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
    es: f64,
    k0: f64,
    en: Mdist,
    s0: f64,
    a_: [f64; 6],
    b_: [f64; 8],
    c_: [f64; 8],
    d_: [f64; 11],
}

impl Roussilhe {
    pub(crate) fn new(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        let key = |k: &str| params.key(k).unwrap_or(0.0);
        let phi0 = key("lat_0");
        let es = ellipsoid.es;
        let en = Mdist::new(es);
        let sp0 = phi0.sin();
        let s0 = en.dist(phi0, sp0, phi0.cos());
        let es2 = es * sp0 * sp0;
        let t = 1.0 - es2;
        let n0 = 1.0 / t.sqrt();
        let r2 = t * t / (1.0 - es);
        let r4 = r2 * r2;
        let t = phi0.tan();
        let t2 = t * t;
        let a1 = r2 / 4.0;
        let a2 = r2 * (2.0 * t2 - 1.0 - 2.0 * es2) / 12.0;
        Ok(Roussilhe {
            lon0: key("lon_0"),
            fe: key("x_0"),
            fn_: key("y_0"),
            a: ellipsoid.a,
            es,
            k0: k0(params),
            en,
            s0,
            a_: [
                a1,
                a2,
                r2 * t * (1.0 + 4.0 * t2) / (12.0 * n0),
                r4 / 24.0,
                r4 * (-1.0 + t2 * (11.0 + 12.0 * t2)) / 24.0,
                r4 * (-2.0 + t2 * (11.0 - 2.0 * t2)) / 240.0,
            ],
            b_: [
                t / (2.0 * n0),
                r2 / 12.0,
                r2 * (1.0 + 2.0 * t2 - 2.0 * es2) / 4.0,
                r2 * t * (2.0 - t2) / (24.0 * n0),
                r2 * t * (5.0 + 4.0 * t2) / (8.0 * n0),
                r4 * (-2.0 + t2 * (-5.0 + 6.0 * t2)) / 48.0,
                r4 * (5.0 + t2 * (19.0 + 12.0 * t2)) / 24.0,
                r4 / 120.0,
            ],
            c_: [
                a1,
                a2,
                r2 * t * (1.0 + t2) / (3.0 * n0),
                r4 * (-3.0 + t2 * (34.0 + 22.0 * t2)) / 240.0,
                r4 * (4.0 + t2 * (13.0 + 12.0 * t2)) / 24.0,
                r4 / 16.0,
                r4 * t * (11.0 + t2 * (33.0 + t2 * 16.0)) / (48.0 * n0),
                r4 * t * (1.0 + t2 * 4.0) / (36.0 * n0),
            ],
            d_: [
                t / (2.0 * n0),
                r2 / 12.0,
                r2 * (2.0 * t2 + 1.0 - 2.0 * es2) / 4.0,
                r2 * t * (1.0 + t2) / (8.0 * n0),
                r2 * t * (1.0 + t2 * 2.0) / (4.0 * n0),
                r4 * (1.0 + t2 * (6.0 + t2 * 6.0)) / 16.0,
                r4 * t2 * (3.0 + t2 * 4.0) / 8.0,
                r4 / 80.0,
                r4 * t * (-21.0 + t2 * (178.0 - t2 * 26.0)) / 720.0,
                r4 * t * (29.0 + t2 * (86.0 + t2 * 48.0)) / (96.0 * n0),
                r4 * t * (37.0 + t2 * 44.0) / (96.0 * n0),
            ],
        })
    }
}

impl Projection for Roussilhe {
    fn forward(&self, lon: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = adjlon(lon - self.lon0);
        let [a1, a2, a3, a4, a5, a6] = self.a_;
        let [b1, b2, b3, b4, b5, b6, b7, b8] = self.b_;
        let (sp, cp) = phi.sin_cos();
        let s = self.en.dist(phi, sp, cp) - self.s0;
        let s2 = s * s;
        let al = lam * cp / (1.0 - self.es * sp * sp).sqrt();
        let al2 = al * al;
        let x =
            self.k0 * al * (1.0 + s2 * (a1 + s2 * a4) - al2 * (a2 + s * a3 + s2 * a5 + al2 * a6));
        let y = self.k0
            * (al2 * (b1 + al2 * b4)
                + s * (1.0
                    + al2 * (b3 - al2 * b6)
                    + s2 * (b2 + s2 * b8)
                    + s * al2 * (b5 + s * b7)));
        Ok((self.a * x + self.fe, self.a * y + self.fn_))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let [c1, c2, c3, c4, c5, c6, c7, c8] = self.c_;
        let [d1, d2, d3, d4, d5, d6, d7, d8, d9, d10, d11] = self.d_;
        let x = (x - self.fe) / self.a / self.k0;
        let y = (y - self.fn_) / self.a / self.k0;
        let (x2, y2) = (x * x, y * y);
        let al = x
            * (1.0 - c1 * y2
                + x2 * (c2 + c3 * y - c4 * x2 + c5 * y2 - c7 * x2 * y)
                + y2 * (c6 * y2 - c8 * x2 * y));
        let s = self.s0
            + y * (1.0 + y2 * (-d2 + d8 * y2))
            + x2 * (-d1
                + y * (-d3 + y * (-d5 + y * (-d7 + y * d11)))
                + x2 * (d4 + y * (d6 + y * d10) - x2 * d9));
        let phi = self.en.inv(s)?;
        let sp = phi.sin();
        Ok((
            al * (1.0 - self.es * sp * sp).sqrt() / phi.cos() + self.lon0,
            phi,
        ))
    }
}
