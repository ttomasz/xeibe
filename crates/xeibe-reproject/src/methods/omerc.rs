//! Hotine Oblique Mercator, variant A (EPSG:9812) and variant B (EPSG:9815).
//!
//! Ported from PROJ 9.8.1 `src/projections/omerc.cpp` (MIT, see
//! PROJ-NOTICE.md), set up with `alpha`, `gamma` and `lonc` as PROJ maps the
//! EPSG parameters. Variant A is `omerc` with `no_uoff`: the false easting and
//! northing apply at the natural origin, where the initial line crosses the
//! equator of the aposphere. Variant B places them at the projection centre.
//!
//! When the azimuth and the angle to the skew grid are both 90° (within
//! 1e-4°), PROJ does not use `omerc` but `somerc`, the Swiss Oblique Mercator
//! (`src/projections/somerc.cpp`), which differs from it by up to centimetres
//! (Hungary's EOV). `SwissMercator` is that port.

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4};
use std::sync::Arc;

use super::common::{phi2, tsfn};
use super::{BuildError, Params, Projection, adjlon, param};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

const TOL: f64 = 1e-7;
const EPS: f64 = 1e-10;

#[derive(Debug)]
pub(crate) struct ObliqueMercator {
    lam0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
    e: f64,
    b: f64,
    e_: f64,
    ar_b: f64,
    br_a: f64,
    r_b: f64,
    singam: f64,
    cosgam: f64,
    sinrot: f64,
    cosrot: f64,
    v_pole_n: f64,
    v_pole_s: f64,
    u_0: f64,
}

impl ObliqueMercator {
    pub(crate) fn variant_a(
        params: &Params,
        ellipsoid: &Ellipsoid,
    ) -> Result<Arc<dyn Projection>, BuildError> {
        let fe = params.get(param::FALSE_EASTING)?;
        let fn_ = params.get(param::FALSE_NORTHING)?;
        if is_swiss(params)? {
            return Ok(Arc::new(SwissMercator::new(params, ellipsoid, fe, fn_)?));
        }
        Ok(Arc::new(Self::new(params, ellipsoid, true, fe, fn_)?))
    }

    pub(crate) fn variant_b(
        params: &Params,
        ellipsoid: &Ellipsoid,
    ) -> Result<Arc<dyn Projection>, BuildError> {
        let fe = params.get(param::EASTING_PROJECTION_CENTRE)?;
        let fn_ = params.get(param::NORTHING_PROJECTION_CENTRE)?;
        if is_swiss(params)? {
            return Ok(Arc::new(SwissMercator::new(params, ellipsoid, fe, fn_)?));
        }
        Ok(Arc::new(Self::new(params, ellipsoid, false, fe, fn_)?))
    }

    fn new(
        params: &Params,
        ellipsoid: &Ellipsoid,
        no_uoff: bool,
        fe: f64,
        fn_: f64,
    ) -> Result<Self, BuildError> {
        let invalid = |m: &str| BuildError::Invalid(m.to_owned());
        let phi0 = params.get(param::LAT_PROJECTION_CENTRE)?;
        let lamc = params.get(param::LON_PROJECTION_CENTRE)?;
        let alpha_c = params.get(param::AZIMUTH_PROJECTION_CENTRE)?;
        let gamma = params.get(param::ANGLE_TO_SKEW_GRID)?;
        let k0 = params.get(param::SCALE_PROJECTION_CENTRE)?;
        let es = ellipsoid.es;
        let e = ellipsoid.e;
        let one_es = 1.0 - es;

        if (phi0.abs() - FRAC_PI_2).abs() <= TOL {
            return Err(invalid("|latitude of projection centre| must be < 90°"));
        }
        let com = one_es.sqrt();
        let (b, a, e_, d, f);
        if phi0.abs() > EPS {
            let (sinph0, cosph0) = phi0.sin_cos();
            let con = 1.0 - es * sinph0 * sinph0;
            let bb = cosph0 * cosph0;
            b = (1.0 + es * bb * bb / one_es).sqrt();
            a = b * k0 * com / con;
            d = b * com / (cosph0 * con.sqrt());
            let mut ff = d * d - 1.0;
            if ff <= 0.0 {
                ff = 0.0;
            } else {
                ff = ff.sqrt();
                if phi0 < 0.0 {
                    ff = -ff;
                }
            }
            f = ff + d;
            e_ = f * tsfn(phi0, sinph0, e).powf(b);
        } else {
            b = 1.0 / com;
            a = k0;
            e_ = 1.0;
            d = 1.0;
            f = 1.0;
        }
        // PROJ's `aasin` flags only values clearly beyond ±1.
        let aasin = |v: f64| -> Result<f64, BuildError> {
            if v.abs() > 1.00000000000001 {
                return Err(invalid(
                    "azimuth or skew angle out of range at this latitude",
                ));
            }
            Ok(v.clamp(-1.0, 1.0).asin())
        };
        // EPSG always gives both azimuth and skew angle, as PROJ's `alpha`
        // and `gamma`: the azimuth fixes the aposphere's initial line.
        let gamma0 = aasin(alpha_c.sin() / d)?;
        let lam0 = lamc - aasin(0.5 * (f - 1.0 / f) * gamma0.tan())? / b;

        let singam = gamma0.sin();
        let cosgam = gamma0.cos();
        let (sinrot, cosrot) = gamma.sin_cos();
        let r_b = 1.0 / b;
        let ar_b = a * r_b;
        let br_a = 1.0 / ar_b;
        let u_0 = if no_uoff {
            0.0
        } else {
            let u = (ar_b * ((d * d - 1.0).sqrt() / alpha_c.cos()).atan()).abs();
            if phi0 < 0.0 { -u } else { u }
        };
        let half = 0.5 * gamma0;
        Ok(ObliqueMercator {
            lam0,
            fe,
            fn_,
            a: ellipsoid.a,
            e,
            b,
            e_,
            ar_b,
            br_a,
            r_b,
            singam,
            cosgam,
            sinrot,
            cosrot,
            v_pole_n: ar_b * (FRAC_PI_4 - half).tan().ln(),
            v_pole_s: ar_b * (FRAC_PI_4 + half).tan().ln(),
            u_0,
        })
    }
}

impl Projection for ObliqueMercator {
    fn forward(&self, lon: f64, lat: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = adjlon(lon - self.lam0);
        let (mut u, v);
        if (lat.abs() - FRAC_PI_2).abs() > EPS {
            let w = self.e_ / tsfn(lat, lat.sin(), self.e).powf(self.b);
            let one_div_w = 1.0 / w;
            let s = 0.5 * (w - one_div_w);
            let t = 0.5 * (w + one_div_w);
            let vv = (self.b * lam).sin();
            let uu = (s * self.singam - vv * self.cosgam) / t;
            if (uu.abs() - 1.0).abs() < EPS {
                return Err(CoordinateError::OutsideDomain);
            }
            v = 0.5 * self.ar_b * ((1.0 - uu) / (1.0 + uu)).ln();
            let temp = (self.b * lam).cos();
            u = if temp.abs() < TOL {
                self.ar_b * self.b * lam
            } else {
                self.ar_b * (s * self.cosgam + vv * self.singam).atan2(temp)
            };
        } else {
            v = if lat > 0.0 {
                self.v_pole_n
            } else {
                self.v_pole_s
            };
            u = self.ar_b * lat;
        }
        u -= self.u_0;
        let x = v * self.cosrot + u * self.sinrot;
        let y = u * self.cosrot - v * self.sinrot;
        Ok((self.a * x + self.fe, self.a * y + self.fn_))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let x = (x - self.fe) / self.a;
        let y = (y - self.fn_) / self.a;
        let v = x * self.cosrot - y * self.sinrot;
        let u = y * self.cosrot + x * self.sinrot + self.u_0;
        let qp = (-self.br_a * v).exp();
        if qp == 0.0 {
            return Err(CoordinateError::OutsideDomain);
        }
        let sp = 0.5 * (qp - 1.0 / qp);
        let tp = 0.5 * (qp + 1.0 / qp);
        let vp = (self.br_a * u).sin();
        let up = (vp * self.cosgam + sp * self.singam) / tp;
        let (lam, phi);
        if (up.abs() - 1.0).abs() < EPS {
            lam = 0.0;
            phi = if up < 0.0 { -FRAC_PI_2 } else { FRAC_PI_2 };
        } else {
            let t = self.e_ / ((1.0 + up) / (1.0 - up)).sqrt();
            phi = phi2(t.powf(1.0 / self.b), self.e)?;
            if !phi.is_finite() {
                return Err(CoordinateError::OutsideDomain);
            }
            lam = -self.r_b * (sp * self.cosgam - vp * self.singam).atan2((self.br_a * u).cos());
        }
        Ok((lam + self.lam0, phi))
    }
}

/// Both the azimuth and the angle to the skew grid are 90°, to 1e-4°.
fn is_swiss(params: &Params) -> Result<bool, BuildError> {
    let right = FRAC_PI_2;
    let tol = 1e-4_f64.to_radians();
    Ok(
        (params.get(param::AZIMUTH_PROJECTION_CENTRE)? - right).abs() < tol
            && (params.get(param::ANGLE_TO_SKEW_GRID)? - right).abs() < tol,
    )
}

const SOMERC_EPS: f64 = 1e-10;
const SOMERC_NITER: usize = 6;

/// PROJ's `aasin`: values clearly beyond ±1 are outside the domain.
fn aasin(v: f64) -> Result<f64, CoordinateError> {
    if v.abs() > 1.00000000000001 {
        return Err(CoordinateError::OutsideDomain);
    }
    Ok(v.clamp(-1.0, 1.0).asin())
}

#[derive(Debug)]
pub(crate) struct SwissMercator {
    lam0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
    e: f64,
    one_es: f64,
    k: f64,
    c: f64,
    hlf_e: f64,
    kr: f64,
    cosp0: f64,
    sinp0: f64,
}

impl SwissMercator {
    fn new(params: &Params, ellipsoid: &Ellipsoid, fe: f64, fn_: f64) -> Result<Self, BuildError> {
        let phi0 = params.get(param::LAT_PROJECTION_CENTRE)?;
        let lam0 = params.get(param::LON_PROJECTION_CENTRE)?;
        let k0 = params.get(param::SCALE_PROJECTION_CENTRE)?;
        let es = ellipsoid.es;
        let e = ellipsoid.e;
        let one_es = 1.0 - es;
        let hlf_e = 0.5 * e;
        let cp = phi0.cos();
        let cp = cp * cp;
        let c = (1.0 + es * cp * cp / one_es).sqrt();
        let mut sp = phi0.sin();
        let sinp0 = sp / c;
        let phip0 = sinp0.asin();
        let cosp0 = phip0.cos();
        sp *= e;
        let k = (FRAC_PI_4 + 0.5 * phip0).tan().ln()
            - c * ((FRAC_PI_4 + 0.5 * phi0).tan().ln() - hlf_e * ((1.0 + sp) / (1.0 - sp)).ln());
        let kr = k0 * one_es.sqrt() / (1.0 - sp * sp);
        Ok(SwissMercator {
            lam0,
            fe,
            fn_,
            a: ellipsoid.a,
            e,
            one_es,
            k,
            c,
            hlf_e,
            kr,
            cosp0,
            sinp0,
        })
    }
}

impl Projection for SwissMercator {
    fn forward(&self, lon: f64, lat: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = adjlon(lon - self.lam0);
        let sp = self.e * lat.sin();
        let phip = 2.0
            * (self.c
                * ((FRAC_PI_4 + 0.5 * lat).tan().ln()
                    - self.hlf_e * ((1.0 + sp) / (1.0 - sp)).ln())
                + self.k)
                .exp()
                .atan()
            - FRAC_PI_2;
        let lamp = self.c * lam;
        let cp = phip.cos();
        let phipp = aasin(self.cosp0 * phip.sin() - self.sinp0 * cp * lamp.cos())?;
        let lampp = aasin(cp * lamp.sin() / phipp.cos())?;
        let x = self.kr * lampp;
        let y = self.kr * (FRAC_PI_4 + 0.5 * phipp).tan().ln();
        Ok((self.a * x + self.fe, self.a * y + self.fn_))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let x = (x - self.fe) / self.a;
        let y = (y - self.fn_) / self.a;
        let phipp = 2.0 * ((y / self.kr).exp().atan() - FRAC_PI_4);
        let lampp = x / self.kr;
        let cp = phipp.cos();
        let phip = aasin(self.cosp0 * phipp.sin() + self.sinp0 * cp * lampp.cos())?;
        let lamp = aasin(cp * lampp.sin() / phip.cos())?;
        let con = (self.k - (FRAC_PI_4 + 0.5 * phip).tan().ln()) / self.c;
        let mut phip = phip;
        for _ in 0..SOMERC_NITER {
            let esp = self.e * phip.sin();
            let delp = (con + (FRAC_PI_4 + 0.5 * phip).tan().ln()
                - self.hlf_e * ((1.0 + esp) / (1.0 - esp)).ln())
                * (1.0 - esp * esp)
                * phip.cos()
                / self.one_es;
            phip -= delp;
            if delp.abs() < SOMERC_EPS {
                return Ok((lamp / self.c + self.lam0, phip));
            }
        }
        Err(CoordinateError::OutsideDomain)
    }
}
