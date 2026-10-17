//! Laborde Oblique Mercator (EPSG:9813), the Madagascar projection.
//!
//! Ported from PROJ 9.8.1 `src/projections/labrd.cpp` (MIT, see
//! PROJ-NOTICE.md).

use std::f64::consts::FRAC_PI_4;

use super::{BuildError, Params, Projection, adjlon, param};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

const EPS: f64 = 1e-10;

#[derive(Debug)]
pub(crate) struct Laborde {
    lam0: f64,
    phi0: f64,
    k0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
    e: f64,
    one_es: f64,
    kr_g: f64,
    p0s: f64,
    big_a: f64,
    c: f64,
    ca: f64,
    cb: f64,
    cc: f64,
    cd: f64,
}

impl Laborde {
    pub(crate) fn new(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        let phi0 = params.get(param::LAT_PROJECTION_CENTRE)?;
        let lam0 = params.get(param::LON_PROJECTION_CENTRE)?;
        let az = params.get(param::AZIMUTH_PROJECTION_CENTRE)?;
        let k0 = params.get(param::SCALE_PROJECTION_CENTRE)?;
        if phi0 == 0.0 {
            return Err(BuildError::Invalid(
                "the latitude of the projection centre must not be 0".to_owned(),
            ));
        }
        let es = ellipsoid.es;
        let e = ellipsoid.e;
        let one_es = 1.0 - es;
        let sinp = phi0.sin();
        let t = 1.0 - es * sinp * sinp;
        let n = 1.0 / t.sqrt();
        let r = one_es * n / t;
        let kr_g = k0 * (n * r).sqrt();
        let p0s = ((r / n).sqrt() * phi0.tan()).atan();
        let big_a = sinp / p0s.sin();
        let t = e * sinp;
        let c = 0.5 * e * big_a * ((1.0 + t) / (1.0 - t)).ln()
            + -big_a * (FRAC_PI_4 + 0.5 * phi0).tan().ln()
            + (FRAC_PI_4 + 0.5 * p0s).tan().ln();
        let t = az + az;
        let mut cb = 1.0 / (12.0 * kr_g * kr_g);
        let ca = (1.0 - t.cos()) * cb;
        cb *= t.sin();
        Ok(Laborde {
            lam0,
            phi0,
            k0,
            fe: params.get(param::FALSE_EASTING)?,
            fn_: params.get(param::FALSE_NORTHING)?,
            a: ellipsoid.a,
            e,
            one_es,
            kr_g,
            p0s,
            big_a,
            c,
            ca,
            cb,
            cc: 3.0 * (ca * ca - cb * cb),
            cd: 6.0 * ca * cb,
        })
    }
}

impl Laborde {
    /// PROJ's inverse is a truncated series, off by up to 1e-7° from the
    /// forward formulas' own inverse. Newton's method on `forward` removes
    /// that, so a round trip returns the starting point.
    fn refine(&self, x: f64, y: f64, lam: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let (mut lon, mut lat) = (lam + self.lam0, phi);
        let h = 1e-7;
        for _ in 0..4 {
            let (fx, fy) = self.forward(lon, lat)?;
            let (rx, ry) = (x - fx, y - fy);
            if rx.abs() + ry.abs() < 1e-6 {
                break;
            }
            let (ax, ay) = self.forward(lon + h, lat)?;
            let (bx, by) = self.forward(lon, lat + h)?;
            let (j11, j21) = ((ax - fx) / h, (ay - fy) / h);
            let (j12, j22) = ((bx - fx) / h, (by - fy) / h);
            let det = j11 * j22 - j12 * j21;
            lon += (rx * j22 - ry * j12) / det;
            lat += (ry * j11 - rx * j21) / det;
        }
        Ok((lon, lat))
    }
}

impl Projection for Laborde {
    fn forward(&self, lon: f64, lat: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = adjlon(lon - self.lam0);
        let v1 = self.big_a * (FRAC_PI_4 + 0.5 * lat).tan().ln();
        let t = self.e * lat.sin();
        let v2 = 0.5 * self.e * self.big_a * ((1.0 + t) / (1.0 - t)).ln();
        let ps = 2.0 * ((v1 - v2 + self.c).exp().atan() - FRAC_PI_4);
        let i1 = ps - self.p0s;
        let (sinps, cosps) = ps.sin_cos();
        let (cosps2, sinps2) = (cosps * cosps, sinps * sinps);
        let aa = self.big_a;
        let i4 = aa * cosps;
        let i2 = 0.5 * aa * i4 * sinps;
        let i3 = i2 * aa * aa * (5.0 * cosps2 - sinps2) / 12.0;
        let mut i6 = i4 * aa * aa;
        let i5 = i6 * (cosps2 - sinps2) / 6.0;
        i6 *= aa * aa * (5.0 * cosps2 * cosps2 + sinps2 * (sinps2 - 18.0 * cosps2)) / 120.0;
        let t = lam * lam;
        let mut x = self.kr_g * lam * (i4 + t * (i5 + t * i6));
        let mut y = self.kr_g * (i1 + t * (i2 + t * i3));
        let (x2, y2) = (x * x, y * y);
        let v1 = 3.0 * x * y2 - x * x2;
        let v2 = y * y2 - 3.0 * x2 * y;
        x += self.ca * v1 + self.cb * v2;
        y += self.ca * v2 - self.cb * v1;
        Ok((self.a * x + self.fe, self.a * y + self.fn_))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let (x_in, y_in) = (x, y);
        let mut x = (x - self.fe) / self.a;
        let mut y = (y - self.fn_) / self.a;
        let (x2, y2) = (x * x, y * y);
        let v1 = 3.0 * x * y2 - x * x2;
        let v2 = y * y2 - 3.0 * x2 * y;
        let v3 = x * (5.0 * y2 * y2 + x2 * (-10.0 * y2 + x2));
        let v4 = y * (5.0 * x2 * x2 + y2 * (-10.0 * x2 + y2));
        x += -self.ca * v1 - self.cb * v2 + self.cc * v3 + self.cd * v4;
        y += self.cb * v1 - self.ca * v2 - self.cd * v3 + self.cc * v4;
        let ps = self.p0s + y / self.kr_g;
        let mut pe = ps + self.phi0 - self.p0s;
        for _ in 0..20 {
            let v1 = self.big_a * (FRAC_PI_4 + 0.5 * pe).tan().ln();
            let tpe = self.e * pe.sin();
            let v2 = 0.5 * self.e * self.big_a * ((1.0 + tpe) / (1.0 - tpe)).ln();
            let t = ps - 2.0 * ((v1 - v2 + self.c).exp().atan() - FRAC_PI_4);
            pe += t;
            if t.abs() < EPS {
                break;
            }
        }
        let t = self.e * pe.sin();
        let t = 1.0 - t * t;
        let re = self.one_es / (t * t.sqrt());
        let t = ps.tan();
        let t2 = t * t;
        let s = self.kr_g * self.kr_g;
        let d = re * self.k0 * self.kr_g;
        let i7 = t / (2.0 * d);
        let i8 = t * (5.0 + 3.0 * t2) / (24.0 * d * s);
        let d = ps.cos() * self.kr_g * self.big_a;
        let i9 = 1.0 / d;
        let d = d * s;
        let i10 = (1.0 + 2.0 * t2) / (6.0 * d);
        let i11 = (5.0 + t2 * (28.0 + 24.0 * t2)) / (120.0 * d * s);
        let x2 = x * x;
        let phi = pe + x2 * (-i7 + i8 * x2);
        let lam = x * (i9 + x2 * (-i10 + x2 * i11));
        self.refine(x_in, y_in, lam, phi)
    }
}
