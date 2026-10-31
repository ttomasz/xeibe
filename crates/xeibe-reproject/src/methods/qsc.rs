//! Quadrilateralized Spherical Cube: one face of a cube around the Earth,
//! equal-area on the sphere, and on an ellipsoid through the geocentric
//! latitude.
//!
//! Ported from PROJ 9.8.1 `src/projections/qsc.cpp` (MIT, see
//! PROJ-NOTICE.md). The face is chosen from the origin's latitude and the
//! central meridian.

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};

use super::sphere::Unit;
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

const EPS10: f64 = 1e-10;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Face {
    Front,
    Right,
    Back,
    Left,
    Top,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Area {
    A0,
    A1,
    A2,
    A3,
}

#[derive(Debug)]
pub(crate) struct Qsc {
    face: Face,
    /// On an ellipsoid: a, b and 1 − f.
    ellipsoid: Option<(f64, f64, f64)>,
}

impl Qsc {
    pub(crate) fn new(phi0: f64, lam0: f64, ellipsoid: &Ellipsoid) -> Self {
        let face = if phi0 >= FRAC_PI_2 - FRAC_PI_4 / 2.0 {
            Face::Top
        } else if phi0 <= -(FRAC_PI_2 - FRAC_PI_4 / 2.0) {
            Face::Bottom
        } else if lam0.abs() <= FRAC_PI_4 {
            Face::Front
        } else if lam0.abs() <= FRAC_PI_2 + FRAC_PI_4 {
            if lam0 > 0.0 { Face::Right } else { Face::Left }
        } else {
            Face::Back
        };
        let ellipsoid = (ellipsoid.es != 0.0).then(|| {
            let a = ellipsoid.a;
            let b = a * (1.0 - ellipsoid.es).sqrt();
            (a, b, 1.0 - (a - b) / a)
        });
        Qsc { face, ellipsoid }
    }
}

fn equat_face_theta(phi: f64, y: f64, x: f64) -> (f64, Area) {
    if phi < EPS10 {
        return (0.0, Area::A0);
    }
    let theta = y.atan2(x);
    if theta.abs() <= FRAC_PI_4 {
        (theta, Area::A0)
    } else if theta > FRAC_PI_4 && theta <= FRAC_PI_2 + FRAC_PI_4 {
        (theta - FRAC_PI_2, Area::A1)
    } else if theta > FRAC_PI_2 + FRAC_PI_4 || theta <= -(FRAC_PI_2 + FRAC_PI_4) {
        (if theta >= 0.0 { theta - PI } else { theta + PI }, Area::A2)
    } else {
        (theta + FRAC_PI_2, Area::A3)
    }
}

fn shift_longitude_origin(longitude: f64, offset: f64) -> f64 {
    let slon = longitude + offset;
    if slon < -PI {
        slon + TAU
    } else if slon > PI {
        slon - TAU
    } else {
        slon
    }
}

impl Unit for Qsc {
    fn fwd(&self, lam: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        // The geocentric latitude: from the ellipsoid to the sphere.
        let lat = match self.ellipsoid {
            Some((_, _, one_minus_f)) => (one_minus_f * one_minus_f * phi.tan()).atan(),
            None => phi,
        };
        let mut longitude = lam;
        let (theta, phi, area);
        match self.face {
            Face::Top => {
                phi = FRAC_PI_2 - lat;
                (theta, area) = if (FRAC_PI_4..=FRAC_PI_2 + FRAC_PI_4).contains(&longitude) {
                    (longitude - FRAC_PI_2, Area::A0)
                } else if longitude > FRAC_PI_2 + FRAC_PI_4 || longitude <= -(FRAC_PI_2 + FRAC_PI_4)
                {
                    (
                        if longitude > 0.0 {
                            longitude - PI
                        } else {
                            longitude + PI
                        },
                        Area::A1,
                    )
                } else if longitude > -(FRAC_PI_2 + FRAC_PI_4) && longitude <= -FRAC_PI_4 {
                    (longitude + FRAC_PI_2, Area::A2)
                } else {
                    (longitude, Area::A3)
                };
            }
            Face::Bottom => {
                phi = FRAC_PI_2 + lat;
                (theta, area) = if (FRAC_PI_4..=FRAC_PI_2 + FRAC_PI_4).contains(&longitude) {
                    (-longitude + FRAC_PI_2, Area::A0)
                } else if (-FRAC_PI_4..FRAC_PI_4).contains(&longitude) {
                    (-longitude, Area::A1)
                } else if (-(FRAC_PI_2 + FRAC_PI_4)..-FRAC_PI_4).contains(&longitude) {
                    (-longitude - FRAC_PI_2, Area::A2)
                } else {
                    (
                        if longitude > 0.0 {
                            -longitude + PI
                        } else {
                            -longitude - PI
                        },
                        Area::A3,
                    )
                };
            }
            _ => {
                longitude = match self.face {
                    Face::Right => shift_longitude_origin(longitude, FRAC_PI_2),
                    Face::Back => shift_longitude_origin(longitude, PI),
                    Face::Left => shift_longitude_origin(longitude, -FRAC_PI_2),
                    _ => longitude,
                };
                let (sinlat, coslat) = lat.sin_cos();
                let (sinlon, coslon) = longitude.sin_cos();
                let q = coslat * coslon;
                let r = coslat * sinlon;
                let s = sinlat;
                let (p, y, x) = match self.face {
                    Face::Front => (q.acos(), s, r),
                    Face::Right => (r.acos(), s, -q),
                    Face::Back => ((-q).acos(), s, -r),
                    _ => ((-r).acos(), s, q),
                };
                phi = p;
                (theta, area) = equat_face_theta(phi, y, x);
            }
        }
        // μ and ν for the area of definition (Eqs. 3-21 and 3-38 of
        // O'Neill and Laubscher), then x and y with t = tan ν.
        let mut mu =
            ((12.0 / PI) * (theta + (theta.sin() * FRAC_PI_4.cos()).acos() - FRAC_PI_2)).atan();
        let t =
            ((1.0 - phi.cos()) / (mu.cos() * mu.cos()) / (1.0 - (1.0 / theta.cos()).atan().cos()))
                .sqrt();
        mu += match area {
            Area::A0 => 0.0,
            Area::A1 => FRAC_PI_2,
            Area::A2 => PI,
            Area::A3 => PI + FRAC_PI_2,
        };
        Ok((t * mu.cos(), t * mu.sin()))
    }

    fn inv(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let nu = (x * x + y * y).sqrt().atan();
        let mut mu = y.atan2(x);
        let area = if x >= 0.0 && x >= y.abs() {
            Area::A0
        } else if y >= 0.0 && y >= x.abs() {
            mu -= FRAC_PI_2;
            Area::A1
        } else if x < 0.0 && -x >= y.abs() {
            mu = if mu < 0.0 { mu + PI } else { mu - PI };
            Area::A2
        } else {
            mu += FRAC_PI_2;
            Area::A3
        };
        let t = (PI / 12.0) * mu.tan();
        let tantheta = t.sin() / (t.cos() - (1.0 / 2.0_f64.sqrt()));
        let theta = tantheta.atan();
        let cosmu = mu.cos();
        let tannu = nu.tan();
        let cosphi = (1.0
            - cosmu * cosmu * tannu * tannu * (1.0 - (1.0 / theta.cos()).atan().cos()))
        .clamp(-1.0, 1.0);
        let (mut lam, mut phi);
        match self.face {
            Face::Top => {
                phi = FRAC_PI_2 - cosphi.acos();
                lam = match area {
                    Area::A0 => theta + FRAC_PI_2,
                    Area::A1 => {
                        if theta < 0.0 {
                            theta + PI
                        } else {
                            theta - PI
                        }
                    }
                    Area::A2 => theta - FRAC_PI_2,
                    Area::A3 => theta,
                };
            }
            Face::Bottom => {
                phi = cosphi.acos() - FRAC_PI_2;
                lam = match area {
                    Area::A0 => -theta + FRAC_PI_2,
                    Area::A1 => -theta,
                    Area::A2 => -theta - FRAC_PI_2,
                    Area::A3 => {
                        if theta < 0.0 {
                            -theta - PI
                        } else {
                            -theta + PI
                        }
                    }
                };
            }
            _ => {
                // Through the unit sphere's cartesian coordinates.
                let mut q = cosphi;
                let mut t = q * q;
                let mut s = if t >= 1.0 {
                    0.0
                } else {
                    (1.0 - t).sqrt() * theta.sin()
                };
                t += s * s;
                let mut r = if t >= 1.0 { 0.0 } else { (1.0 - t).sqrt() };
                match area {
                    Area::A1 => (r, s) = (-s, r),
                    Area::A2 => (r, s) = (-r, -s),
                    Area::A3 => (r, s) = (s, -r),
                    Area::A0 => {}
                }
                match self.face {
                    Face::Right => (q, r) = (-r, q),
                    Face::Back => (q, r) = (-q, -r),
                    Face::Left => (q, r) = (r, -q),
                    _ => {}
                }
                phi = (-s).acos() - FRAC_PI_2;
                lam = r.atan2(q);
                lam = match self.face {
                    Face::Right => shift_longitude_origin(lam, -FRAC_PI_2),
                    Face::Back => shift_longitude_origin(lam, -PI),
                    Face::Left => shift_longitude_origin(lam, FRAC_PI_2),
                    _ => lam,
                };
            }
        }
        // From the sphere back to the ellipsoid.
        if let Some((a, b, one_minus_f)) = self.ellipsoid {
            let invert_sign = phi < 0.0;
            let tanphi = phi.tan();
            let xa = b / (tanphi * tanphi + one_minus_f * one_minus_f).sqrt();
            phi = ((a * a - xa * xa).sqrt() / (one_minus_f * xa)).atan();
            if invert_sign {
                phi = -phi;
            }
        }
        Ok((lam, phi))
    }
}
