//! Transverse Mercator (EPSG:9807), the Poder/Engsager series that PROJ uses by
//! default, accurate to a few nanometres within 3,900 km of the central
//! meridian. On a sphere the closed formulas are used instead.
//!
//! Ported from PROJ 9.8.1 `src/projections/tmerc.cpp` (MIT, see
//! PROJ-NOTICE.md): `exact_e_fwd`, `exact_e_inv`, `setup_exact` and the
//! spherical `tmerc_spherical_fwd`/`_inv`.

use std::f64::consts::{FRAC_PI_2, PI};

use super::auxlat::{self, AuxLat, ORDER};
use super::{BuildError, Params, Projection, adjlon, param};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

/// |Ce| beyond this, about 150° from the central meridian, is outside the
/// domain.
const CE_LIMIT: f64 = 2.623395162778;

#[derive(Debug)]
pub(crate) struct TransverseMercator {
    lon0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
    kind: Kind,
}

#[derive(Debug)]
enum Kind {
    Ellipsoidal {
        /// Normalised meridian quadrant times k0.
        qn: f64,
        /// Northing of the origin latitude, negated, in units of a.
        zb: f64,
        cgb: [f64; ORDER],
        cbg: [f64; ORDER],
        utg: [f64; ORDER],
        gtu: [f64; ORDER],
    },
    Spherical {
        k0: f64,
        phi0: f64,
    },
}

impl TransverseMercator {
    pub(crate) fn new(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        let phi0 = params.get(param::LAT_NATURAL_ORIGIN)?;
        let lon0 = params.get(param::LON_NATURAL_ORIGIN)?;
        let k0 = params.get(param::SCALE_NATURAL_ORIGIN)?;
        let fe = params.get(param::FALSE_EASTING)?;
        let fn_ = params.get(param::FALSE_NORTHING)?;
        Ok(Self::with(ellipsoid, phi0, lon0, k0, fe, fn_))
    }

    pub(crate) fn with(
        ellipsoid: &Ellipsoid,
        phi0: f64,
        lon0: f64,
        k0: f64,
        fe: f64,
        fn_: f64,
    ) -> Self {
        let kind = if ellipsoid.is_sphere() {
            Kind::Spherical { k0, phi0 }
        } else {
            let n = ellipsoid.n;
            let cgb = auxlat::coeffs(n, AuxLat::Conformal, AuxLat::Geographic);
            let cbg = auxlat::coeffs(n, AuxLat::Geographic, AuxLat::Conformal);
            let qn = k0 * auxlat::rectifying_radius(n);
            let utg = auxlat::coeffs(n, AuxLat::Rectifying, AuxLat::Conformal);
            let gtu = auxlat::coeffs(n, AuxLat::Conformal, AuxLat::Rectifying);
            // Gaussian latitude of the origin, then its northing.
            let z = auxlat::convert(phi0, &cbg);
            let zb = -qn * auxlat::convert(z, &gtu);
            Kind::Ellipsoidal {
                qn,
                zb,
                cgb,
                cbg,
                utg,
                gtu,
            }
        };
        TransverseMercator {
            lon0,
            fe,
            fn_,
            a: ellipsoid.a,
            kind,
        }
    }
}

impl Projection for TransverseMercator {
    fn forward(&self, lon: f64, lat: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = adjlon(lon - self.lon0);
        let (x, y) = match &self.kind {
            Kind::Ellipsoidal {
                qn, zb, cbg, gtu, ..
            } => exact_fwd(lam, lat, *qn, *zb, cbg, gtu)?,
            Kind::Spherical { k0, phi0 } => sphere_fwd(lam, lat, *k0, *phi0)?,
        };
        Ok((self.a * x + self.fe, self.a * y + self.fn_))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let x = (x - self.fe) / self.a;
        let y = (y - self.fn_) / self.a;
        let (lam, phi) = match &self.kind {
            Kind::Ellipsoidal {
                qn, zb, cgb, utg, ..
            } => exact_inv(x, y, *qn, *zb, cgb, utg)?,
            Kind::Spherical { k0, phi0 } => sphere_inv(x, y, *k0, *phi0)?,
        };
        Ok((lam + self.lon0, phi))
    }
}

/// Complex Clenshaw summation (PROJ's `clenS`). Returns (real, imaginary).
fn clens(a: &[f64], sin_r: f64, cos_r: f64, sinh_i: f64, cosh_i: f64) -> (f64, f64) {
    let r = 2.0 * cos_r * cosh_i;
    let i = -2.0 * sin_r * sinh_i;
    let mut it = a.iter().rev();
    let mut hr = *it.next().expect("non-empty series");
    let (mut hi, mut hr1, mut hi1) = (0.0, 0.0, 0.0);
    for &c in it {
        let hr2 = hr1;
        let hi2 = hi1;
        hr1 = hr;
        hi1 = hi;
        hr = -hr2 + r * hr1 - i * hi1 + c;
        hi = -hi2 + i * hr1 + r * hi1;
    }
    let r = sin_r * cosh_i;
    let i = cos_r * sinh_i;
    (r * hr - i * hi, r * hi + i * hr)
}

fn exact_fwd(
    lam: f64,
    phi: f64,
    qn: f64,
    zb: f64,
    cbg: &[f64; ORDER],
    gtu: &[f64; ORDER],
) -> Result<(f64, f64), CoordinateError> {
    // Ellipsoidal latitude, longitude -> Gaussian latitude, longitude.
    let mut cn = auxlat::convert(phi, cbg);
    // Gaussian -> complementary spherical latitude.
    let (sin_cn, cos_cn) = cn.sin_cos();
    let (sin_ce, cos_ce) = lam.sin_cos();
    let cos_cn_cos_ce = cos_cn * cos_ce;
    cn = sin_cn.atan2(cos_cn_cos_ce);
    let inv_denom_tan_ce = 1.0 / sin_cn.hypot(cos_cn_cos_ce);
    let tan_ce = sin_ce * cos_cn * inv_denom_tan_ce;
    // Complementary spherical N, E -> ellipsoidal normalised N, E.
    let mut ce = tan_ce.asinh();
    let two_inv_denom_tan_ce = 2.0 * inv_denom_tan_ce;
    let two_inv_denom_tan_ce_square = two_inv_denom_tan_ce * inv_denom_tan_ce;
    let tmp_r = cos_cn_cos_ce * two_inv_denom_tan_ce_square;
    let sin_arg_r = sin_cn * tmp_r;
    let cos_arg_r = cos_cn_cos_ce * tmp_r - 1.0;
    let sinh_arg_i = tan_ce * two_inv_denom_tan_ce;
    let cosh_arg_i = two_inv_denom_tan_ce_square - 1.0;
    let (dcn, dce) = clens(gtu, sin_arg_r, cos_arg_r, sinh_arg_i, cosh_arg_i);
    cn += dcn;
    ce += dce;
    if ce.abs() <= CE_LIMIT {
        Ok((qn * ce, qn * cn + zb))
    } else {
        Err(CoordinateError::OutsideDomain)
    }
}

fn exact_inv(
    x: f64,
    y: f64,
    qn: f64,
    zb: f64,
    cgb: &[f64; ORDER],
    utg: &[f64; ORDER],
) -> Result<(f64, f64), CoordinateError> {
    // Normalise N, E.
    let mut cn = (y - zb) / qn;
    let mut ce = x / qn;
    if ce.abs() > CE_LIMIT {
        return Err(CoordinateError::OutsideDomain);
    }
    // Normalised N, E -> complementary spherical latitude, longitude.
    let (sin_arg_r, cos_arg_r) = (2.0 * cn).sin_cos();
    let exp_2_ce = (2.0 * ce).exp();
    let half_inv_exp_2_ce = 0.5 / exp_2_ce;
    let sinh_arg_i = 0.5 * exp_2_ce - half_inv_exp_2_ce;
    let cosh_arg_i = 0.5 * exp_2_ce + half_inv_exp_2_ce;
    let (dcn, dce) = clens(utg, sin_arg_r, cos_arg_r, sinh_arg_i, cosh_arg_i);
    cn += dcn;
    ce += dce;
    // Complementary spherical latitude -> Gaussian latitude, longitude.
    let (sin_cn, cos_cn) = cn.sin_cos();
    let sinh_ce = ce.sinh();
    let lam = sinh_ce.atan2(cos_cn);
    let modulus_ce = sinh_ce.hypot(cos_cn);
    let rr = sin_cn.hypot(modulus_ce);
    let cn = sin_cn.atan2(modulus_ce);
    // Gaussian latitude -> ellipsoidal latitude.
    let phi = auxlat::convert_sc(cn, sin_cn / rr, modulus_ce / rr, cgb);
    Ok((lam, phi))
}

/// PROJ's `tmerc_spherical_fwd`, in units of the radius.
fn sphere_fwd(lam: f64, phi: f64, k0: f64, phi0: f64) -> Result<(f64, f64), CoordinateError> {
    const EPS10: f64 = 1e-10;
    let cosphi = phi.cos();
    let b = cosphi * lam.sin();
    if (b.abs() - 1.0).abs() <= EPS10 {
        return Err(CoordinateError::OutsideDomain);
    }
    let x = 0.5 * k0 * ((1.0 + b) / (1.0 - b)).ln();
    let mut y;
    if cosphi == 1.0 {
        // Lets |longitudes| > 90° round-trip at the equator.
        y = if !(-FRAC_PI_2..=FRAC_PI_2).contains(&lam) {
            PI
        } else {
            0.0
        };
    } else {
        y = cosphi * lam.cos() / (1.0 - b * b).sqrt();
        let b = y.abs();
        if b >= 1.0 {
            if b - 1.0 > EPS10 {
                return Err(CoordinateError::OutsideDomain);
            }
            y = 0.0;
        } else {
            y = y.acos();
        }
    }
    if phi < 0.0 {
        y = -y;
    }
    Ok((x, k0 * (y - phi0)))
}

/// PROJ's `tmerc_spherical_inv`, in units of the radius.
fn sphere_inv(x: f64, y: f64, k0: f64, phi0: f64) -> Result<(f64, f64), CoordinateError> {
    let h = (x / k0).exp();
    if h == 0.0 {
        return Err(CoordinateError::OutsideDomain);
    }
    let g = 0.5 * (h - 1.0 / h);
    // D, as in equation 8-8 of USGS "Map Projections - A Working Manual".
    let d = phi0 + y / k0;
    let h = d.cos();
    let phi = ((1.0 - h * h) / (1.0 + g * g)).sqrt().asin().copysign(d);
    let lam = if g != 0.0 || h != 0.0 {
        g.atan2(h)
    } else {
        0.0
    };
    Ok((lam, phi))
}
