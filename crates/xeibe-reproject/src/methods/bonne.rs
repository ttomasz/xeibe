//! Bonne (EPSG:9827) and Bonne (South Orientated) (EPSG:9828).
//!
//! Ported from PROJ 9.8.1 `src/projections/bonne.cpp` (`bonne`; MIT, see
//! PROJ-NOTICE.md). The latitude of natural origin is PROJ's `lat_1`. South
//! Orientated differs only in its axes (southing, westing), which
//! `transform.rs` applies, so both methods share these east/north formulas.

use std::f64::consts::FRAC_PI_2;

use super::mlfn::Mlfn;
use super::{BuildError, Params, Projection, adjlon, param};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

const EPS10: f64 = 1e-10;

#[derive(Debug)]
pub(crate) struct Bonne {
    lon0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
    es: f64,
    phi1: f64,
    cphi1: f64,
    am1: f64,
    m1: f64,
    /// `None` for a sphere.
    en: Option<Mlfn>,
}

impl Bonne {
    pub(crate) fn new(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        let phi1 = params.get(param::LAT_NATURAL_ORIGIN)?;
        if phi1.abs() < EPS10 {
            return Err(BuildError::Invalid("|lat_1| should be > 0".to_owned()));
        }
        let (mut cphi1, mut am1, mut m1, mut en) = (0.0, 0.0, 0.0, None);
        if ellipsoid.is_sphere() {
            cphi1 = if phi1.abs() + EPS10 >= FRAC_PI_2 {
                0.0
            } else {
                1.0 / phi1.tan()
            };
        } else {
            let mlfn = Mlfn::new(ellipsoid.n);
            let (s, c) = phi1.sin_cos();
            m1 = mlfn.mlfn(phi1, s, c);
            am1 = c / ((1.0 - ellipsoid.es * s * s).sqrt() * s);
            en = Some(mlfn);
        }
        Ok(Bonne {
            lon0: params.get(param::LON_NATURAL_ORIGIN)?,
            fe: params.get(param::FALSE_EASTING)?,
            fn_: params.get(param::FALSE_NORTHING)?,
            a: ellipsoid.a,
            es: ellipsoid.es,
            phi1,
            cphi1,
            am1,
            m1,
            en,
        })
    }
}

impl Projection for Bonne {
    fn forward(&self, lon: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = adjlon(lon - self.lon0);
        let (x, y) = match &self.en {
            Some(en) => {
                let (e, c) = phi.sin_cos();
                let rh = self.am1 + self.m1 - en.mlfn(phi, e, c);
                if rh.abs() > EPS10 {
                    let e = c * lam / (rh * (1.0 - self.es * e * e).sqrt());
                    (rh * e.sin(), self.am1 - rh * e.cos())
                } else {
                    (0.0, 0.0)
                }
            }
            None => {
                let rh = self.cphi1 + self.phi1 - phi;
                if rh.abs() > EPS10 {
                    let e = lam * phi.cos() / rh;
                    (rh * e.sin(), self.cphi1 - rh * e.cos())
                } else {
                    (0.0, 0.0)
                }
            }
        };
        Ok((self.a * x + self.fe, self.a * y + self.fn_))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let x = (x - self.fe) / self.a;
        let y = (y - self.fn_) / self.a;
        let (lam, phi) = match &self.en {
            Some(en) => {
                let y = self.am1 - y;
                let rh = x.hypot(y).copysign(self.phi1);
                let phi = en.inv_mlfn(self.am1 + self.m1 - rh);
                let abs_phi = phi.abs();
                let lam = if abs_phi < FRAC_PI_2 {
                    let s = phi.sin();
                    let lm = rh * (1.0 - self.es * s * s).sqrt() / phi.cos();
                    if self.phi1 > 0.0 {
                        lm * x.atan2(y)
                    } else {
                        lm * (-x).atan2(-y)
                    }
                } else if abs_phi - FRAC_PI_2 <= EPS10 {
                    0.0
                } else {
                    return Err(CoordinateError::OutsideDomain);
                };
                (lam, phi)
            }
            None => {
                let y = self.cphi1 - y;
                let rh = x.hypot(y).copysign(self.phi1);
                let phi = self.cphi1 + self.phi1 - rh;
                let abs_phi = phi.abs();
                if abs_phi > FRAC_PI_2 {
                    return Err(CoordinateError::OutsideDomain);
                }
                let lam = if FRAC_PI_2 - abs_phi <= EPS10 {
                    0.0
                } else {
                    let lm = rh / phi.cos();
                    if self.phi1 > 0.0 {
                        lm * x.atan2(y)
                    } else {
                        lm * (-x).atan2(-y)
                    }
                };
                (lam, phi)
            }
        };
        Ok((lam + self.lon0, phi))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lisbon 1890 / Portugal Bonne New (EPSG:5017). PROJ has no mapping for
    /// the EPSG method, so the vectors can't cover it; this checks the
    /// projection against pyproj 3.8.0 (PROJ 9.8.1) given the PROJ string
    /// `+proj=bonne +lat_1=39.666666666666664 +lon_0=-8.13190611111111
    /// +ellps=bessel`, which gives (99403.55690663883, -184374.10545086762).
    #[test]
    fn matches_proj_on_bessel() {
        let mut p = Params::default();
        p.push(
            param::LAT_NATURAL_ORIGIN,
            39.666_666_666_666_664_f64.to_radians(),
        );
        p.push(
            param::LON_NATURAL_ORIGIN,
            (-8.131_906_111_111_11_f64).to_radians(),
        );
        p.push(param::FALSE_EASTING, 0.0);
        p.push(param::FALSE_NORTHING, 0.0);
        let bessel = Ellipsoid::from_inverse_flattening(6_377_397.155, 299.152_812_8);
        let b = Bonne::new(&p, &bessel).unwrap();
        let (x, y) = b
            .forward((-7.0_f64).to_radians(), 38.0_f64.to_radians())
            .unwrap();
        assert!((x - 99_403.556_906_638_83).abs() < 1e-4, "{x}");
        assert!((y + 184_374.105_450_873_3).abs() < 1e-4, "{y}");
        let (lon, lat) = b.inverse(x, y).unwrap();
        assert!((lon.to_degrees() + 7.0).abs() < 1e-10);
        assert!((lat.to_degrees() - 38.0).abs() < 1e-10);
    }
}
