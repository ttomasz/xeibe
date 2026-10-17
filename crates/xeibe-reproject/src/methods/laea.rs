//! Lambert Azimuthal Equal Area (EPSG:9820) and its spherical form
//! (EPSG:1027).
//!
//! Ported from PROJ 9.8.1 `src/projections/laea.cpp` and the authalic
//! latitude routines of `src/latitudes.cpp` (MIT, see PROJ-NOTICE.md). PROJ
//! maps EPSG:1027 to `laea` with `R_A`: the ellipsoid is replaced by the
//! sphere of equal surface area (`src/ell_set.cpp`).

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4};

use super::auxlat::{self, AuxLat, ORDER};
use super::{BuildError, Params, Projection, param};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

const EPS10: f64 = 1e-10;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    NPole,
    SPole,
    Equatorial,
    Oblique,
}

#[derive(Debug)]
pub(crate) struct LambertAzimuthal {
    lon0: f64,
    phi0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
    mode: Mode,
    sinb1: f64,
    cosb1: f64,
    kind: Kind,
}

#[derive(Debug)]
enum Kind {
    Spherical,
    Ellipsoidal {
        es: f64,
        e: f64,
        n: f64,
        qp: f64,
        xmf: f64,
        ymf: f64,
        dd: f64,
        rq: f64,
        /// Authalic to geographic, and geographic to authalic when the
        /// series are accurate (|n| < 0.01).
        to_geographic: [f64; ORDER],
        to_authalic: Option<[f64; ORDER]>,
    },
}

/// q such that the authalic latitude is asin(q / qp) (Snyder 3-11).
fn authalic_q(sinphi: f64, e: f64, es: f64) -> f64 {
    if e >= 1e-7 {
        let e_sinphi = e * sinphi;
        let denom = 1.0 - e_sinphi * e_sinphi;
        if denom == 0.0 {
            return f64::INFINITY;
        }
        (1.0 - es) * (sinphi / denom + e_sinphi.atanh() / e)
    } else {
        2.0 * sinphi
    }
}

impl LambertAzimuthal {
    pub(crate) fn new(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        Self::with(params, ellipsoid)
    }

    /// EPSG:1027: on the sphere with the surface area of the ellipsoid.
    pub(crate) fn spherical(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        const RA4: f64 = 17.0 / 360.0;
        const RA6: f64 = 67.0 / 3024.0;
        let es = ellipsoid.es;
        let radius = ellipsoid.a * (1.0 - es * (1.0 / 6.0 + es * (RA4 + es * RA6)));
        Self::with(params, &Ellipsoid::sphere(radius))
    }

    fn with(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        let phi0 = params.get(param::LAT_NATURAL_ORIGIN)?;
        let lon0 = params.get(param::LON_NATURAL_ORIGIN)?;
        let fe = params.get(param::FALSE_EASTING)?;
        let fn_ = params.get(param::FALSE_NORTHING)?;
        let t = phi0.abs();
        if t > FRAC_PI_2 + EPS10 {
            return Err(BuildError::Invalid(
                "the latitude of origin must be within ±90°".to_owned(),
            ));
        }
        let mode = if (t - FRAC_PI_2).abs() < EPS10 {
            if phi0 < 0.0 { Mode::SPole } else { Mode::NPole }
        } else if t.abs() < EPS10 {
            Mode::Equatorial
        } else {
            Mode::Oblique
        };
        let (mut sinb1, mut cosb1) = (0.0, 0.0);
        let kind = if ellipsoid.es != 0.0 {
            let (es, e, n) = (ellipsoid.es, ellipsoid.e, ellipsoid.n);
            let qp = authalic_q(1.0, e, es);
            let to_geographic = auxlat::coeffs(n, AuxLat::Authalic, AuxLat::Geographic);
            let to_authalic =
                (n.abs() < 0.01).then(|| auxlat::coeffs(n, AuxLat::Geographic, AuxLat::Authalic));
            let mut kind = Kind::Ellipsoidal {
                es,
                e,
                n,
                qp,
                xmf: 0.0,
                ymf: 0.0,
                dd: 1.0,
                rq: 0.0,
                to_geographic,
                to_authalic,
            };
            let Kind::Ellipsoidal {
                xmf,
                ymf,
                dd,
                rq,
                to_authalic,
                ..
            } = &mut kind
            else {
                unreachable!()
            };
            match mode {
                Mode::NPole | Mode::SPole => {}
                Mode::Equatorial => {
                    *rq = (0.5 * qp).sqrt();
                    *dd = 1.0 / *rq;
                    *xmf = 1.0;
                    *ymf = 0.5 * qp;
                }
                Mode::Oblique => {
                    *rq = (0.5 * qp).sqrt();
                    let (sinphi, cosphi) = phi0.sin_cos();
                    let b1 = authalic_lat(phi0, sinphi, cosphi, to_authalic.as_ref(), e, es, qp);
                    (sinb1, cosb1) = b1.sin_cos();
                    *dd = phi0.cos() / ((1.0 - es * sinphi * sinphi).sqrt() * *rq * cosb1);
                    *xmf = *rq;
                    *ymf = *xmf / *dd;
                    *xmf *= *dd;
                }
            }
            kind
        } else {
            if mode == Mode::Oblique {
                (sinb1, cosb1) = phi0.sin_cos();
            }
            Kind::Spherical
        };
        Ok(LambertAzimuthal {
            lon0,
            phi0,
            fe,
            fn_,
            a: ellipsoid.a,
            mode,
            sinb1,
            cosb1,
            kind,
        })
    }
}

/// Authalic latitude from the geographic one: by the series when it is
/// accurate, else from q.
fn authalic_lat(
    phi: f64,
    sinphi: f64,
    cosphi: f64,
    series: Option<&[f64; ORDER]>,
    e: f64,
    es: f64,
    qp: f64,
) -> f64 {
    match series {
        Some(c) => auxlat::convert_sc(phi, sinphi, cosphi, c),
        None => {
            let ratio = (authalic_q(sinphi, e, es) / qp).clamp(-1.0, 1.0);
            ratio.asin()
        }
    }
}

/// Geographic latitude from the authalic one `beta`.
#[allow(clippy::too_many_arguments)]
fn authalic_lat_inverse(
    beta: f64,
    series: &[f64; ORDER],
    accurate: bool,
    e: f64,
    es: f64,
    qp: f64,
) -> f64 {
    let mut phi = auxlat::convert(beta, series);
    if accurate {
        return phi;
    }
    // Large flattening: Newton's method on q(φ) = qp sin β.
    let q = beta.sin() * qp;
    let q_div_one_minus_es = q / (1.0 - es);
    for _ in 0..10 {
        let (sinphi, cosphi) = phi.sin_cos();
        let one_minus_es_sin2phi = 1.0 - es * (sinphi * sinphi);
        let dphi = (one_minus_es_sin2phi * one_minus_es_sin2phi) / (2.0 * cosphi)
            * (q_div_one_minus_es - sinphi / one_minus_es_sin2phi - (e * sinphi).atanh() / e);
        // Written backwards so that NaN ends the loop.
        #[allow(clippy::neg_cmp_op_on_partial_ord)]
        if !(dphi.abs() >= 1e-15) {
            break;
        }
        phi += dphi;
    }
    phi
}

impl Projection for LambertAzimuthal {
    fn forward(&self, lon: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = super::adjlon(lon - self.lon0);
        let (sinlam, coslam) = lam.sin_cos();
        let (sinphi, cosphi) = phi.sin_cos();
        let (x, y) = match &self.kind {
            Kind::Ellipsoidal {
                es,
                e,
                qp,
                xmf,
                ymf,
                to_authalic,
                ..
            } => {
                let xi = authalic_lat(phi, sinphi, cosphi, to_authalic.as_ref(), *e, *es, *qp);
                let mut q = xi.sin() * qp;
                let (mut sinb, mut cosb) = (0.0, 0.0);
                if matches!(self.mode, Mode::Oblique | Mode::Equatorial) {
                    (sinb, cosb) = xi.sin_cos();
                }
                let mut b = match self.mode {
                    Mode::Oblique => 1.0 + self.sinb1 * sinb + self.cosb1 * cosb * coslam,
                    Mode::Equatorial => 1.0 + cosb * coslam,
                    Mode::NPole => {
                        q = qp - q;
                        FRAC_PI_2 + phi
                    }
                    Mode::SPole => {
                        q += qp;
                        phi - FRAC_PI_2
                    }
                };
                if b.abs() < EPS10 {
                    return Err(CoordinateError::OutsideDomain);
                }
                match self.mode {
                    Mode::Oblique => {
                        b = (2.0 / b).sqrt();
                        (
                            xmf * b * cosb * sinlam,
                            ymf * b * (self.cosb1 * sinb - self.sinb1 * cosb * coslam),
                        )
                    }
                    Mode::Equatorial => {
                        b = (2.0 / (1.0 + cosb * coslam)).sqrt();
                        (xmf * b * cosb * sinlam, b * sinb * ymf)
                    }
                    Mode::NPole | Mode::SPole => {
                        if q >= 1e-15 {
                            b = q.sqrt();
                            (
                                b * sinlam,
                                coslam * if self.mode == Mode::SPole { b } else { -b },
                            )
                        } else {
                            (0.0, 0.0)
                        }
                    }
                }
            }
            Kind::Spherical => match self.mode {
                Mode::Equatorial | Mode::Oblique => {
                    let mut y = if self.mode == Mode::Equatorial {
                        1.0 + cosphi * coslam
                    } else {
                        1.0 + self.sinb1 * sinphi + self.cosb1 * cosphi * coslam
                    };
                    if y <= EPS10 {
                        return Err(CoordinateError::OutsideDomain);
                    }
                    y = (2.0 / y).sqrt();
                    let x = y * cosphi * sinlam;
                    y *= if self.mode == Mode::Equatorial {
                        sinphi
                    } else {
                        self.cosb1 * sinphi - self.sinb1 * cosphi * coslam
                    };
                    (x, y)
                }
                Mode::NPole | Mode::SPole => {
                    let coslam = if self.mode == Mode::NPole {
                        -coslam
                    } else {
                        coslam
                    };
                    if (phi + self.phi0).abs() < EPS10 {
                        return Err(CoordinateError::OutsideDomain);
                    }
                    let mut y = FRAC_PI_4 - phi * 0.5;
                    y = 2.0
                        * if self.mode == Mode::SPole {
                            y.cos()
                        } else {
                            y.sin()
                        };
                    (y * sinlam, y * coslam)
                }
            },
        };
        Ok((self.a * x + self.fe, self.a * y + self.fn_))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let (mut x, mut y) = ((x - self.fe) / self.a, (y - self.fn_) / self.a);
        match &self.kind {
            Kind::Ellipsoidal {
                es,
                e,
                n,
                qp,
                dd,
                rq,
                to_geographic,
                ..
            } => {
                let ab;
                match self.mode {
                    Mode::Equatorial | Mode::Oblique => {
                        x /= dd;
                        y *= dd;
                        let rho = x.hypot(y);
                        if rho < EPS10 {
                            return Ok((self.lon0, self.phi0));
                        }
                        let asin_argument = 0.5 * rho / rq;
                        if asin_argument > 1.0 {
                            return Err(CoordinateError::OutsideDomain);
                        }
                        let s_ce = 2.0 * asin_argument.asin();
                        let (s_ce, c_ce) = s_ce.sin_cos();
                        x *= s_ce;
                        if self.mode == Mode::Oblique {
                            ab = c_ce * self.sinb1 + y * s_ce * self.cosb1 / rho;
                            y = rho * self.cosb1 * c_ce - y * self.sinb1 * s_ce;
                        } else {
                            ab = y * s_ce / rho;
                            y = rho * c_ce;
                        }
                    }
                    Mode::NPole | Mode::SPole => {
                        if self.mode == Mode::NPole {
                            y = -y;
                        }
                        let q = x * x + y * y;
                        if q == 0.0 {
                            return Ok((self.lon0, self.phi0));
                        }
                        let a = 1.0 - q / qp;
                        ab = if self.mode == Mode::SPole { -a } else { a };
                    }
                }
                let lam = x.atan2(y);
                let phi =
                    authalic_lat_inverse(ab.asin(), to_geographic, n.abs() < 0.01, *e, *es, *qp);
                Ok((lam + self.lon0, phi))
            }
            Kind::Spherical => {
                let rh = x.hypot(y);
                let mut phi = rh * 0.5;
                if phi > 1.0 {
                    return Err(CoordinateError::OutsideDomain);
                }
                phi = 2.0 * phi.asin();
                let (mut sinz, mut cosz) = (0.0, 0.0);
                if matches!(self.mode, Mode::Oblique | Mode::Equatorial) {
                    (sinz, cosz) = phi.sin_cos();
                }
                match self.mode {
                    Mode::Equatorial => {
                        phi = if rh.abs() <= EPS10 {
                            0.0
                        } else {
                            (y * sinz / rh).asin()
                        };
                        x *= sinz;
                        y = cosz * rh;
                    }
                    Mode::Oblique => {
                        phi = if rh.abs() <= EPS10 {
                            self.phi0
                        } else {
                            (cosz * self.sinb1 + y * sinz * self.cosb1 / rh).asin()
                        };
                        x *= sinz * self.cosb1;
                        y = (cosz - phi.sin() * self.sinb1) * rh;
                    }
                    Mode::NPole => {
                        y = -y;
                        phi = FRAC_PI_2 - phi;
                    }
                    Mode::SPole => phi -= FRAC_PI_2,
                }
                let lam = if y == 0.0 && matches!(self.mode, Mode::Equatorial | Mode::Oblique) {
                    0.0
                } else {
                    x.atan2(y)
                };
                Ok((lam + self.lon0, phi))
            }
        }
    }
}
