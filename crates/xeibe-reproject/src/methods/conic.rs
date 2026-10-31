//! Conic projections PROJ has no CRS method for: Lambert Conformal Conic
//! Alternative, Central Conic, and the simple conics (Euler, Murdoch I to
//! III, Perspective Conic, Tissot, Vitkovsky I).
//!
//! Ported from PROJ 9.8.1 `src/projections/lcca.cpp`, `ccon.cpp` and
//! `sconics.cpp` (MIT, see PROJ-NOTICE.md).

use std::f64::consts::FRAC_PI_2;

use super::mlfn::Mlfn;
use super::sphere::Unit;
use super::{BuildError, Params, Projection, adjlon};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

const EPS10: f64 = 1e-10;

fn key(params: &Params, k: &str) -> f64 {
    params.key(k).unwrap_or(0.0)
}

/// PROJ's `k0`: `k_0`, else `k`, else 1.
pub(crate) fn k0(params: &Params) -> f64 {
    params.key("k_0").or_else(|| params.key("k")).unwrap_or(1.0)
}

/// Lambert Conformal Conic Alternative (`lcca`), on an ellipsoid.
#[derive(Debug)]
pub(crate) struct Lcca {
    lon0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
    k0: f64,
    en: Mlfn,
    r0: f64,
    l: f64,
    m0: f64,
    c: f64,
}

impl Lcca {
    pub(crate) fn new(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        let phi0 = key(params, "lat_0");
        if phi0 == 0.0 {
            return Err(BuildError::Invalid(
                "Invalid value for lat_0: it should be different from 0.".into(),
            ));
        }
        let en = Mlfn::new(ellipsoid.n);
        let l = phi0.sin();
        let m0 = en.mlfn(phi0, l, phi0.cos());
        let s2p0 = l * l;
        let r0 = 1.0 / (1.0 - ellipsoid.es * s2p0);
        let n0 = r0.sqrt();
        let r0 = r0 * (1.0 - ellipsoid.es) * n0;
        Ok(Lcca {
            lon0: key(params, "lon_0"),
            fe: key(params, "x_0"),
            fn_: key(params, "y_0"),
            a: ellipsoid.a,
            k0: k0(params),
            en,
            r0: n0 / phi0.tan(),
            l,
            m0,
            c: 1.0 / (6.0 * r0 * n0),
        })
    }

    fn fs(&self, s: f64) -> f64 {
        s * (1.0 + s * s * self.c)
    }

    fn fsp(&self, s: f64) -> f64 {
        1.0 + 3.0 * s * s * self.c
    }
}

impl Projection for Lcca {
    fn forward(&self, lon: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = adjlon(lon - self.lon0);
        let s = self.en.mlfn(phi, phi.sin(), phi.cos()) - self.m0;
        let r = self.r0 - self.fs(s);
        let lam_l = lam * self.l;
        let x = self.k0 * (r * lam_l.sin());
        let y = self.k0 * (self.r0 - r * lam_l.cos());
        Ok((self.a * x + self.fe, self.a * y + self.fn_))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let x = (x - self.fe) / self.a / self.k0;
        let y = (y - self.fn_) / self.a / self.k0;
        let theta = x.atan2(self.r0 - y);
        let dr = y - x * (0.5 * theta).tan();
        let lam = theta / self.l;
        let mut s = dr;
        for _ in 0..10 {
            let dif = (self.fs(s) - dr) / self.fsp(s);
            s -= dif;
            if dif.abs() < 1e-12 {
                return Ok((lam + self.lon0, self.en.inv_mlfn(s + self.m0)));
            }
        }
        Err(CoordinateError::OutsideDomain)
    }
}

/// Central Conic (`ccon`), on the sphere of the semi-major axis.
#[derive(Debug)]
pub(crate) struct CentralConic {
    phi1: f64,
    sinphi1: f64,
    ctgphi1: f64,
}

impl CentralConic {
    pub(crate) fn new(params: &Params) -> Result<Self, BuildError> {
        let phi1 = key(params, "lat_1");
        if phi1.abs() < EPS10 {
            return Err(BuildError::Invalid(
                "Invalid value for lat_1: |lat_1| should be > 0".into(),
            ));
        }
        let (sinphi1, cosphi1) = phi1.sin_cos();
        Ok(CentralConic {
            phi1,
            sinphi1,
            ctgphi1: cosphi1 / sinphi1,
        })
    }
}

impl Unit for CentralConic {
    fn fwd(&self, lam: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let r = self.ctgphi1 - (phi - self.phi1).tan();
        Ok((
            r * (lam * self.sinphi1).sin(),
            self.ctgphi1 - r * (lam * self.sinphi1).cos(),
        ))
    }

    fn inv(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let y = self.ctgphi1 - y;
        Ok((
            x.atan2(y) / self.sinphi1,
            self.phi1 - (x.hypot(y) - self.ctgphi1).atan(),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Euler,
    Murdoch1,
    Murdoch2,
    Murdoch3,
    Perspective,
    Tissot,
    Vitkovsky1,
}

/// The simple conics of `sconics.cpp`, on the unit sphere.
#[derive(Debug)]
pub(crate) struct Simple {
    kind: Kind,
    n: f64,
    rho_c: f64,
    rho_0: f64,
    sig: f64,
    c1: f64,
    c2: f64,
}

impl Simple {
    /// The projection of a PROJ name, `None` for another.
    pub(crate) fn of(proj: &str, params: &Params) -> Result<Option<Self>, BuildError> {
        let kind = match proj {
            "euler" => Kind::Euler,
            "murd1" => Kind::Murdoch1,
            "murd2" => Kind::Murdoch2,
            "murd3" => Kind::Murdoch3,
            "pconic" => Kind::Perspective,
            "tissot" => Kind::Tissot,
            "vitk1" => Kind::Vitkovsky1,
            _ => return Ok(None),
        };
        let invalid = |m: &str| Err(BuildError::Invalid(m.into()));
        let (Some(p1), Some(p2)) = (params.key("lat_1"), params.key("lat_2")) else {
            return invalid(if params.key("lat_1").is_none() {
                "Missing parameter: lat_1 should be specified"
            } else {
                "Missing parameter: lat_2 should be specified"
            });
        };
        let mut del = 0.5 * (p2 - p1);
        let sig = 0.5 * (p2 + p1);
        if del.abs() < 1e-10 || sig.abs() < 1e-10 {
            return invalid(
                "Illegal value for lat_1 and lat_2: |lat_1 - lat_2| and |lat_1 + lat_2| should be > 0",
            );
        }
        let phi0 = key(params, "lat_0");
        let n;
        let (mut rho_c, mut c1, mut c2) = (0.0, 0.0, 0.0);
        let rho_0 = match kind {
            Kind::Tissot => {
                n = sig.sin();
                let cs = del.cos();
                rho_c = n / cs + cs / n;
                ((rho_c - 2.0 * phi0.sin()) / n).sqrt()
            }
            Kind::Murdoch1 => {
                rho_c = del.sin() / (del * sig.tan()) + sig;
                n = sig.sin();
                rho_c - phi0
            }
            Kind::Murdoch2 => {
                let cs = del.cos().sqrt();
                rho_c = cs / sig.tan();
                n = sig.sin() * cs;
                rho_c + (sig - phi0).tan()
            }
            Kind::Murdoch3 => {
                rho_c = del / (sig.tan() * del.tan()) + sig;
                n = sig.sin() * del.sin() * del.tan() / (del * del);
                rho_c - phi0
            }
            Kind::Euler => {
                n = sig.sin() * del.sin() / del;
                del *= 0.5;
                rho_c = del / (del.tan() * sig.tan()) + sig;
                rho_c - phi0
            }
            Kind::Perspective => {
                n = sig.sin();
                c2 = del.cos();
                c1 = 1.0 / sig.tan();
                let d = phi0 - sig;
                if d.abs() - EPS10 >= FRAC_PI_2 {
                    return invalid(
                        "Invalid value for lat_0/lat_1/lat_2: |lat_0 - 0.5 * (lat_1 + lat_2)| should be < 90°",
                    );
                }
                c2 * (c1 - d.tan())
            }
            Kind::Vitkovsky1 => {
                let cs = del.tan();
                n = cs * sig.sin() / del;
                rho_c = del / (cs * sig.tan()) + sig;
                rho_c - phi0
            }
        };
        Ok(Some(Simple {
            kind,
            n,
            rho_c,
            rho_0,
            sig,
            c1,
            c2,
        }))
    }
}

impl Unit for Simple {
    fn fwd(&self, lam: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let rho = match self.kind {
            Kind::Murdoch2 => self.rho_c + (self.sig - phi).tan(),
            Kind::Perspective => self.c2 * (self.c1 - (phi - self.sig).tan()),
            _ => self.rho_c - phi,
        };
        let lam = lam * self.n;
        Ok((rho * lam.sin(), self.rho_0 - rho * lam.cos()))
    }

    fn inv(&self, mut x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let mut y = self.rho_0 - y;
        let mut rho = x.hypot(y);
        if self.n < 0.0 {
            rho = -rho;
            x = -x;
            y = -y;
        }
        let lam = x.atan2(y) / self.n;
        let phi = match self.kind {
            Kind::Perspective => (self.c1 - rho / self.c2).atan() + self.sig,
            Kind::Murdoch2 => self.sig - (rho - self.rho_c).atan(),
            _ => self.rho_c - rho,
        };
        Ok((lam, phi))
    }
}
