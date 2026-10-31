//! Space Oblique Mercator (`som`), and for the orbits of Landsat
//! (`lsat`) and MISR (`misrsom`), on an ellipsoid.
//!
//! Ported from PROJ 9.8.1 `src/projections/som.cpp` (MIT, see
//! PROJ-NOTICE.md). PROJ sets the central meridian from the orbit
//! (`asc_lon`, or the path) and reads `ps_rev` as a plain number though it
//! writes it in degrees.

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};

use super::sphere::aasin;
use super::{BuildError, Params, Projection, adjlon};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

const TOL: f64 = 1e-7;

#[derive(Debug)]
pub(crate) struct SpaceObliqueMercator {
    lon0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
    es: f64,
    one_es: f64,
    rone_es: f64,
    a2: f64,
    a4: f64,
    b: f64,
    c1: f64,
    c3: f64,
    q: f64,
    t: f64,
    u: f64,
    w: f64,
    p22: f64,
    sa: f64,
    ca: f64,
    xj: f64,
    rlm: f64,
    rlm2: f64,
}

impl SpaceObliqueMercator {
    /// `som`, `lsat` or `misrsom`; `None` for another.
    pub(crate) fn of(
        proj: &str,
        params: &Params,
        ellipsoid: &Ellipsoid,
    ) -> Result<Option<Self>, BuildError> {
        let invalid = |m: String| Err(BuildError::Invalid(m));
        let key = |k: &str| params.key(k).unwrap_or(0.0);
        // pj_param's integers.
        let int = |k: &str| key(k) as i32;
        let (lam0, alf, p22, rlm) = match proj {
            "som" => {
                let lam0 = key("asc_lon");
                if !(-TAU..=TAU).contains(&lam0) {
                    return invalid(
                        "Invalid value for ascending longitude: should be in [-2pi, 2pi] range"
                            .into(),
                    );
                }
                let alf = key("inc_angle");
                if !(0.0..=PI).contains(&alf) {
                    return invalid(
                        "Invalid value for inclination angle: should be in [0, pi] range".into(),
                    );
                }
                let p22 = key("ps_rev").to_degrees();
                if p22 < 0.0 {
                    return invalid("Number of days per rotation should be positive".into());
                }
                (lam0, alf, p22, 0.0)
            }
            "misrsom" => {
                let path = int("path");
                if path <= 0 || path > 233 {
                    return invalid(
                        "Invalid value for path: path should be in [1, 233] range".into(),
                    );
                }
                (
                    129.3056f64.to_radians() - TAU / 233.0 * f64::from(path),
                    98.30382f64.to_radians(),
                    98.88 / 1440.0,
                    0.0,
                )
            }
            "lsat" => {
                let land = int("lsat");
                if land <= 0 || land > 5 {
                    return invalid(
                        "Invalid value for lsat: lsat should be in [1, 5] range".into(),
                    );
                }
                let path = int("path");
                let max = if land <= 3 { 251 } else { 233 };
                if path <= 0 || path > max {
                    return invalid(format!(
                        "Invalid value for path: path should be in [1, {max}] range"
                    ));
                }
                let path = f64::from(path);
                let (lam0, p22, alf) = if land <= 3 {
                    (
                        128.87f64.to_radians() - TAU / 251.0 * path,
                        103.2669323,
                        99.092f64.to_radians(),
                    )
                } else {
                    (
                        129.3f64.to_radians() - TAU / 233.0 * path,
                        98.8841202,
                        98.2f64.to_radians(),
                    )
                };
                (
                    lam0,
                    alf,
                    p22 / 1440.0,
                    PI * (1.0 / 248.0 + 0.5161290322580645),
                )
            }
            _ => return Ok(None),
        };
        let es = ellipsoid.es;
        let one_es = 1.0 - es;
        let rone_es = 1.0 / one_es;
        let sa = alf.sin();
        let mut ca = alf.cos();
        if ca.abs() < 1e-9 {
            ca = 1e-9;
        }
        let esc = es * ca * ca;
        let ess = es * sa * sa;
        let w = (1.0 - esc) * rone_es;
        let mut p = SpaceObliqueMercator {
            lon0: lam0,
            fe: key("x_0"),
            fn_: key("y_0"),
            a: ellipsoid.a,
            es,
            one_es,
            rone_es,
            a2: 0.0,
            a4: 0.0,
            b: 0.0,
            c1: 0.0,
            c3: 0.0,
            q: ess * rone_es,
            t: ess * (2.0 - es) * rone_es * rone_es,
            u: esc * rone_es,
            w: w * w - 1.0,
            p22,
            sa,
            ca,
            xj: one_es * one_es * one_es,
            rlm,
            rlm2: rlm + TAU,
        };
        p.seraz0(0.0, 1.0);
        let mut lam = 9.0;
        while lam <= 81.0001 {
            p.seraz0(lam, 4.0);
            lam += 18.0;
        }
        let mut lam = 18.0;
        while lam <= 72.0001 {
            p.seraz0(lam, 2.0);
            lam += 18.0;
        }
        p.seraz0(90.0, 1.0);
        p.a2 /= 30.0;
        p.a4 /= 60.0;
        p.b /= 30.0;
        p.c1 /= 15.0;
        p.c3 /= 45.0;
        Ok(Some(p))
    }

    /// One term of the Fourier coefficients' Simpson sums.
    fn seraz0(&mut self, lam: f64, mult: f64) {
        let lam = lam.to_radians();
        let sd = lam.sin();
        let sdsq = sd * sd;
        let s = self.s(lam.cos(), sdsq);
        let d_1 = 1.0 + self.q * sdsq;
        let h = ((1.0 + self.q * sdsq) / (1.0 + self.w * sdsq)).sqrt()
            * ((1.0 + self.w * sdsq) / (d_1 * d_1) - self.p22 * self.ca);
        let sq = (self.xj * self.xj + s * s).sqrt();
        let fc = mult * (h * self.xj - s * s) / sq;
        self.b += fc;
        self.a2 += fc * (lam + lam).cos();
        self.a4 += fc * (lam * 4.0).cos();
        let fc = mult * s * (h + self.xj) / sq;
        self.c1 += fc * lam.cos();
        self.c3 += fc * (lam * 3.0).cos();
    }

    fn s(&self, cos_lam: f64, sdsq: f64) -> f64 {
        self.p22
            * self.sa
            * cos_lam
            * ((1.0 + self.t * sdsq) / ((1.0 + self.w * sdsq) * (1.0 + self.q * sdsq))).sqrt()
    }
}

impl Projection for SpaceObliqueMercator {
    fn forward(&self, lon: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = adjlon(lon - self.lon0);
        let phi = phi.clamp(-FRAC_PI_2, FRAC_PI_2);
        let mut lampp = if phi >= 0.0 {
            FRAC_PI_2
        } else {
            PI + FRAC_PI_2
        };
        let tanphi = phi.tan();
        let (mut lamt, mut lamdp) = (0.0, 0.0);
        let mut nn = 0;
        // PROJ's loop counter: -1 when the inner loop ran out, 0 when it
        // stopped on its last step.
        let mut l: i32;
        loop {
            let mut sav = lampp;
            let lamtp = lam + self.p22 * lampp;
            let fac = if lamtp.cos() < 0.0 {
                lampp + lampp.sin() * FRAC_PI_2
            } else {
                lampp - lampp.sin() * FRAC_PI_2
            };
            l = 50;
            while l >= 0 {
                lamt = lam + self.p22 * sav;
                let c = lamt.cos();
                if c.abs() < TOL {
                    lamt -= TOL;
                }
                let xlam = (self.one_es * tanphi * self.sa + lamt.sin() * self.ca) / c;
                lamdp = xlam.atan() + fac;
                if (sav.abs() - lamdp.abs()).abs() < TOL {
                    break;
                }
                sav = lamdp;
                l -= 1;
            }
            nn += 1;
            if l == 0 || nn >= 3 || (lamdp > self.rlm && lamdp < self.rlm2) {
                break;
            }
            if lamdp <= self.rlm {
                lampp = TAU + FRAC_PI_2;
            } else if lamdp >= self.rlm2 {
                lampp = FRAC_PI_2;
            }
        }
        if l == 0 {
            return Err(CoordinateError::OutsideDomain);
        }
        let sp = phi.sin();
        let phidp = aasin(
            (self.one_es * self.ca * sp - self.sa * phi.cos() * lamt.sin())
                / (1.0 - self.es * sp * sp).sqrt(),
        )?;
        let tanph = (FRAC_PI_4 + 0.5 * phidp).tan().ln();
        let sd = lamdp.sin();
        let s = self.s(lamdp.cos(), sd * sd);
        let d = (self.xj * self.xj + s * s).sqrt();
        let x = self.b * lamdp + self.a2 * (2.0 * lamdp).sin() + self.a4 * (lamdp * 4.0).sin()
            - tanph * s / d;
        let y = self.c1 * sd + self.c3 * (lamdp * 3.0).sin() + tanph * self.xj / d;
        Ok((self.a * x + self.fe, self.a * y + self.fn_))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let ra = 1.0 / self.a;
        let x = (x - self.fe) * ra;
        let y = (y - self.fn_) * ra;
        let mut lamdp = x / self.b;
        let mut s;
        let mut nn = 50;
        loop {
            let sav = lamdp;
            let sd = lamdp.sin();
            s = self.s(lamdp.cos(), sd * sd);
            lamdp = (x + y * s / self.xj
                - self.a2 * (2.0 * lamdp).sin()
                - self.a4 * (lamdp * 4.0).sin()
                - s / self.xj * (self.c1 * lamdp.sin() + self.c3 * (lamdp * 3.0).sin()))
                / self.b;
            nn -= 1;
            if (lamdp - sav).abs() < TOL || nn == 0 {
                break;
            }
        }
        let sl = lamdp.sin();
        let fac = ((1.0 + s * s / self.xj / self.xj).sqrt()
            * (y - self.c1 * sl - self.c3 * (lamdp * 3.0).sin()))
        .exp();
        let phidp = 2.0 * (fac.atan() - FRAC_PI_4);
        let dd = sl * sl;
        if lamdp.cos().abs() < TOL {
            lamdp -= TOL;
        }
        let spp = phidp.sin();
        let sppsq = spp * spp;
        let denom = 1.0 - sppsq * (1.0 + self.u);
        if denom == 0.0 {
            return Err(CoordinateError::OutsideDomain);
        }
        let mut lamt = (((1.0 - sppsq * self.rone_es) * lamdp.tan() * self.ca
            - spp * self.sa * ((1.0 + self.q * dd) * (1.0 - sppsq) - sppsq * self.u).sqrt()
                / lamdp.cos())
            / denom)
            .atan();
        let sl = if lamt >= 0.0 { 1.0 } else { -1.0 };
        let scl = if lamdp.cos() >= 0.0 { 1.0 } else { -1.0 };
        lamt -= FRAC_PI_2 * (1.0 - scl) * sl;
        let lam = lamt - self.p22 * lamdp;
        let phi = if self.sa.abs() < TOL {
            aasin(spp / (self.one_es * self.one_es + self.es * sppsq).sqrt())?
        } else {
            ((lamdp.tan() * lamt.cos() - self.ca * lamt.sin()) / (self.one_es * self.sa)).atan()
        };
        Ok((lam + self.lon0, phi))
    }
}
