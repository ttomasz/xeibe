//! Krovak (EPSG:9819), Krovak North Orientated (EPSG:1041), Krovak Modified
//! (EPSG:1042) and Krovak Modified North Orientated (EPSG:1043).
//!
//! Ported from PROJ 9.8.1 `src/projections/krovak.cpp` (`krovak` and
//! `mod_krovak`; MIT, see PROJ-NOTICE.md). PROJ fixes the co-latitude of the
//! cone axis, the pseudo standard parallel and the Modified Krovak
//! coefficients. Here the latter two come from the EPSG parameters, which
//! carry the same values. The co-latitude does not: EPSG stores it as
//! 30.1717303 (sexagesimal), one digit short of the defining 30°17′17.30311″,
//! which moves points by up to 3 mm, so the defining value is kept, as in PROJ.
//!
//! The formulas give a southing and a westing. `forward` returns the
//! east/north pair PROJ returns (the negated westing and southing, false
//! origin included); the south/west axes of the non-north-orientated CRSs
//! negate it again.

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4};

use super::{BuildError, Params, Projection, adjlon, param};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

const EPS: f64 = 1e-15;
const MAX_ITER: usize = 100;
/// Co-latitude of the cone axis, 30°17′17.30311″ (90° minus PROJ's `UQ`,
/// 59°42′42.69689″).
const COLATITUDE: f64 = std::f64::consts::FRAC_PI_2 - 1.04216856380474;

/// The evaluation point and C1 to C10 of every Modified Krovak CRS EPSG has
/// (S-JTSK/05), which PROJ's `mod_krovak` fixes and PROJ's WKT leaves out.
const MODIFIED: [(u32, f64); 12] = [
    (param::EVALUATION_POINT_ORDINATE_1, 1_089_000.0),
    (param::EVALUATION_POINT_ORDINATE_2, 654_000.0),
    (param::KROVAK_C1, 2.946529277e-2),
    (param::KROVAK_C1 + 1, 2.515965696e-2),
    (param::KROVAK_C1 + 2, 1.193845912e-7),
    (param::KROVAK_C1 + 3, -4.668270147e-7),
    (param::KROVAK_C1 + 4, 9.233980362e-12),
    (param::KROVAK_C1 + 5, 1.523735715e-12),
    (param::KROVAK_C1 + 6, 1.696780024e-18),
    (param::KROVAK_C1 + 7, 4.408314235e-18),
    (param::KROVAK_C1 + 8, -8.331083518e-24),
    (param::KROVAK_C1 + 9, -3.689471323e-24),
];

/// For Modified Krovak given without its correction (as PROJ writes it),
/// EPSG's correction: the CRS is then the EPSG one, also to `equivalent`.
/// A correction given in part stays as given, and fails to build.
pub(crate) fn complete(code: Option<u32>, params: &mut Params) {
    if matches!(code, Some(1042 | 1043)) && MODIFIED.iter().all(|(c, _)| params.get(*c).is_err()) {
        for (c, v) in MODIFIED {
            params.push(c, v);
        }
    }
}

/// The Modified Krovak correction: an evaluation point and ten coefficients.
#[derive(Debug)]
struct Modified {
    x0: f64,
    y0: f64,
    c: [f64; 10],
}

impl Modified {
    fn new(params: &Params) -> Result<Self, BuildError> {
        let mut c = [0.0; 10];
        for (i, c) in c.iter_mut().enumerate() {
            *c = params.get(param::KROVAK_C1 + i as u32)?;
        }
        Ok(Modified {
            x0: params.get(param::EVALUATION_POINT_ORDINATE_1)?,
            y0: params.get(param::EVALUATION_POINT_ORDINATE_2)?,
            c,
        })
    }

    /// Correction (dX, dY), in metres, that turns Krovak into Modified
    /// Krovak, from the southing and westing in metres.
    fn correction(&self, south: f64, west: f64) -> (f64, f64) {
        let [c1, c2, c3, c4, c5, c6, c7, c8, c9, c10] = self.c;
        let (xr, yr) = (south - self.x0, west - self.y0);
        let (xr2, yr2) = (xr * xr, yr * yr);
        let (xr4, yr4) = (xr2 * xr2, yr2 * yr2);
        let dx = c1 + c3 * xr - c4 * yr - 2.0 * c6 * xr * yr
            + c5 * (xr2 - yr2)
            + c7 * xr * (xr2 - 3.0 * yr2)
            - c8 * yr * (3.0 * xr2 - yr2)
            + 4.0 * c9 * xr * yr * (xr2 - yr2)
            + c10 * (xr4 + yr4 - 6.0 * xr2 * yr2);
        let dy = c2
            + c3 * yr
            + c4 * xr
            + 2.0 * c5 * xr * yr
            + c6 * (xr2 - yr2)
            + c8 * xr * (xr2 - 3.0 * yr2)
            + c7 * yr * (3.0 * xr2 - yr2)
            - 4.0 * c10 * xr * yr * (xr2 - yr2)
            + c9 * (xr4 + yr4 - 6.0 * xr2 * yr2);
        (dx, dy)
    }
}

#[derive(Debug)]
pub(crate) struct Krovak {
    lam0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
    e: f64,
    modified: Option<Modified>,
    /// Latitude of the pseudo standard parallel.
    s0: f64,
    alpha: f64,
    k: f64,
    n: f64,
    rho0: f64,
    ad: f64,
}

impl Krovak {
    pub(crate) fn new(
        params: &Params,
        ellipsoid: &Ellipsoid,
        modified: bool,
    ) -> Result<Self, BuildError> {
        let phi0 = params.get(param::LAT_PROJECTION_CENTRE)?;
        let lam0 = params.get(param::LON_OF_ORIGIN)?;
        let k0 = params.get(param::SCALE_PSEUDO_STANDARD_PARALLEL)?;
        let s0 = params.get(param::LAT_PSEUDO_STANDARD_PARALLEL)?;
        let modified = if modified {
            Some(Modified::new(params)?)
        } else {
            None
        };
        let es = ellipsoid.es;
        let e = ellipsoid.e;

        let alpha = (1.0 + (es * phi0.cos().powi(4)) / (1.0 - es)).sqrt();
        let u0 = (phi0.sin() / alpha).asin();
        let g = ((1.0 + e * phi0.sin()) / (1.0 - e * phi0.sin())).powf(alpha * e / 2.0);
        let tan_half = (phi0 / 2.0 + FRAC_PI_4).tan();
        if tan_half == 0.0 {
            return Err(BuildError::Invalid(
                "the latitude of the projection centre must not be -45°".to_owned(),
            ));
        }
        let k = (u0 / 2.0 + FRAC_PI_4).tan() / tan_half.powf(alpha) * g;
        let n0 = (1.0 - es).sqrt() / (1.0 - es * phi0.sin().powi(2));
        Ok(Krovak {
            lam0,
            fe: params.get(param::FALSE_EASTING)?,
            fn_: params.get(param::FALSE_NORTHING)?,
            a: ellipsoid.a,
            e,
            modified,
            s0,
            alpha,
            k,
            n: s0.sin(),
            rho0: k0 * n0 / s0.tan(),
            ad: COLATITUDE,
        })
    }
}

impl Projection for Krovak {
    fn forward(&self, lon: f64, lat: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = adjlon(lon - self.lam0);
        let e = self.e;
        let gfi = ((1.0 + e * lat.sin()) / (1.0 - e * lat.sin())).powf(self.alpha * e / 2.0);
        let u = 2.0
            * ((self.k * (lat / 2.0 + FRAC_PI_4).tan().powf(self.alpha) / gfi).atan() - FRAC_PI_4);
        let deltav = -lam * self.alpha;
        let s = (self.ad.cos() * u.sin() + self.ad.sin() * u.cos() * deltav.cos()).asin();
        let cos_s = s.cos();
        let (mut south, mut west);
        if cos_s < 1e-12 {
            (south, west) = (0.0, 0.0);
        } else {
            let d = (u.cos() * deltav.sin() / cos_s).asin();
            let eps = self.n * d;
            let rho = self.rho0 * (self.s0 / 2.0 + FRAC_PI_4).tan().powf(self.n)
                / (s / 2.0 + FRAC_PI_4).tan().powf(self.n);
            south = rho * eps.cos();
            west = rho * eps.sin();
            if let Some(modified) = &self.modified {
                let (dx, dy) = modified.correction(south * self.a, west * self.a);
                south -= dx / self.a;
                west -= dy / self.a;
            }
        }
        Ok((-west * self.a - self.fe, -south * self.a - self.fn_))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let mut south = (-y - self.fn_) / self.a;
        let mut west = (-x - self.fe) / self.a;
        if let Some(modified) = &self.modified {
            let (dx, dy) = modified.correction(south * self.a, west * self.a);
            south += dx / self.a;
            west += dy / self.a;
        }
        let rho = south.hypot(west);
        let eps = west.atan2(south);
        let d = eps / self.s0.sin();
        let s = if rho == 0.0 {
            FRAC_PI_2
        } else {
            2.0 * (((self.rho0 / rho).powf(1.0 / self.n) * (self.s0 / 2.0 + FRAC_PI_4).tan())
                .atan()
                - FRAC_PI_4)
        };
        let u = (self.ad.cos() * s.sin() - self.ad.sin() * s.cos() * d.cos()).asin();
        let deltav = (s.cos() * d.sin() / u.cos()).asin();
        let lam = self.lam0 - deltav / self.alpha;

        let e = self.e;
        let mut fi1 = u;
        let mut phi = 0.0;
        let mut converged = false;
        for _ in 0..MAX_ITER {
            phi = 2.0
                * ((self.k.powf(-1.0 / self.alpha)
                    * (u / 2.0 + FRAC_PI_4).tan().powf(1.0 / self.alpha)
                    * ((1.0 + e * fi1.sin()) / (1.0 - e * fi1.sin())).powf(e / 2.0))
                .atan()
                    - FRAC_PI_4);
            if (fi1 - phi).abs() < EPS {
                converged = true;
                break;
            }
            fi1 = phi;
        }
        if !converged {
            return Err(CoordinateError::OutsideDomain);
        }
        Ok((lam, phi))
    }
}
