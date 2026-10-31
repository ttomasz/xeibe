//! HEALPix and rHEALPix: the sphere, or the authalic sphere of an
//! ellipsoid, equal-area in twelve squares, with rHEALPix's polar
//! triangles gathered into one square each.
//!
//! Ported from PROJ 9.8.1 `src/projections/healpix.cpp` (MIT, see
//! PROJ-NOTICE.md). On an ellipsoid PROJ takes the authalic radius as the
//! semi-major axis.

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};

use super::authalic_q::Authalic;
use super::{BuildError, Params, Projection, adjlon};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

const EPS: f64 = 1e-15;

/// The rotations of the polar triangles, by `rotate_index`.
const ROT: [[[f64; 2]; 2]; 7] = [
    [[1.0, 0.0], [0.0, 1.0]],
    [[0.0, -1.0], [1.0, 0.0]],
    [[-1.0, 0.0], [0.0, -1.0]],
    [[0.0, 1.0], [-1.0, 0.0]],
    [[0.0, 1.0], [-1.0, 0.0]],
    [[-1.0, 0.0], [0.0, -1.0]],
    [[0.0, -1.0], [1.0, 0.0]],
];

fn rotate_index(i: i32) -> usize {
    match i {
        0..=3 => i as usize,
        -1 => 4,
        -2 => 5,
        -3 => 6,
        _ => 0,
    }
}

fn sign(v: f64) -> f64 {
    if v > 0.0 {
        1.0
    } else if v < 0.0 {
        -1.0
    } else {
        0.0
    }
}

fn rotate((x, y): (f64, f64), angle: f64) -> (f64, f64) {
    let (s, c) = angle.sin_cos();
    (x * c - y * s, y * c + x * s)
}

#[derive(Debug, Clone, Copy)]
enum Kind {
    Healpix {
        rot_xy: f64,
    },
    Rhealpix {
        north_square: i32,
        south_square: i32,
    },
}

#[derive(Debug)]
pub(crate) struct Healpix {
    kind: Kind,
    lon0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
    authalic: Option<Authalic>,
}

impl Healpix {
    /// `healpix` or `rhealpix`, `None` for another.
    pub(crate) fn of(
        proj: &str,
        params: &Params,
        ellipsoid: &Ellipsoid,
    ) -> Result<Option<Self>, BuildError> {
        let kind = match proj {
            "healpix" => Kind::Healpix {
                rot_xy: params.key("rot_xy").unwrap_or(0.0),
            },
            "rhealpix" => {
                // pj_param's integers.
                let square = |k: &str| params.key(k).unwrap_or(0.0) as i32;
                let (north_square, south_square) = (square("north_square"), square("south_square"));
                for (k, v) in [
                    ("north_square", north_square),
                    ("south_square", south_square),
                ] {
                    if !(0..=3).contains(&v) {
                        return Err(BuildError::Invalid(format!(
                            "Invalid value for {k}: it should be in [0,3] range."
                        )));
                    }
                }
                Kind::Rhealpix {
                    north_square,
                    south_square,
                }
            }
            _ => return Ok(None),
        };
        let (a, authalic) = if ellipsoid.es != 0.0 {
            let au = Authalic::new(ellipsoid);
            (ellipsoid.a * (0.5 * au.qp).sqrt(), Some(au))
        } else {
            (ellipsoid.a, None)
        };
        let key = |k: &str| params.key(k).unwrap_or(0.0);
        Ok(Some(Healpix {
            kind,
            lon0: key("lon_0"),
            fe: key("x_0"),
            fn_: key("y_0"),
            a,
            authalic,
        }))
    }
}

/// HEALPix on the unit sphere.
fn sphere(lam: f64, phi: f64) -> (f64, f64) {
    let phi0 = (2.0f64 / 3.0).asin();
    if phi.abs() <= phi0 {
        (lam, 3.0 * PI / 8.0 * phi.sin())
    } else {
        let sigma = (3.0 * (1.0 - phi.sin().abs())).sqrt();
        let cn = (2.0 * lam / PI + 2.0).floor().min(3.0);
        let lamc = -3.0 * FRAC_PI_4 + FRAC_PI_2 * cn;
        (
            lamc + (lam - lamc) * sigma,
            sign(phi) * FRAC_PI_4 * (2.0 - sigma),
        )
    }
}

fn sphere_inverse(x: f64, y: f64) -> (f64, f64) {
    if y.abs() <= FRAC_PI_4 {
        (x, (8.0 * y / (3.0 * PI)).asin())
    } else if y.abs() < FRAC_PI_2 {
        let cn = (2.0 * x / PI + 2.0).floor().min(3.0);
        let xc = -3.0 * FRAC_PI_4 + FRAC_PI_2 * cn;
        let tau = 2.0 - 4.0 * y.abs() / PI;
        (
            xc + (x - xc) / tau,
            sign(y) * (1.0 - tau.powi(2) / 3.0).asin(),
        )
    } else {
        (-PI, sign(y) * FRAC_PI_2)
    }
}

/// Whether a point is in the polygon, its vertices included (`pnpoly`).
fn pnpoly(vert: &[[f64; 2]], tx: f64, ty: f64) -> bool {
    if vert.iter().any(|v| tx == v[0] && ty == v[1]) {
        return true;
    }
    let mut counter = 0;
    let mut p1 = vert[0];
    for &p2 in &vert[1..] {
        if ty > p1[1].min(p2[1])
            && ty <= p1[1].max(p2[1])
            && tx <= p1[0].max(p2[0])
            && p1[1] != p2[1]
        {
            let xinters = (ty - p1[1]) * (p2[0] - p1[0]) / (p2[1] - p1[1]) + p1[0];
            if p1[0] == p2[0] || tx <= xinters {
                counter += 1;
            }
        }
        p1 = p2;
    }
    counter % 2 == 1
}

fn in_image(x: f64, y: f64, kind: Kind) -> bool {
    const Q: f64 = FRAC_PI_4;
    const H: f64 = FRAC_PI_2;
    match kind {
        Kind::Healpix { .. } => pnpoly(
            &[
                [-PI - EPS, Q],
                [-3.0 * Q, H + EPS],
                [-H, Q + EPS],
                [-Q, H + EPS],
                [0.0, Q + EPS],
                [Q, H + EPS],
                [H, Q + EPS],
                [3.0 * Q, H + EPS],
                [PI + EPS, Q],
                [PI + EPS, -Q],
                [3.0 * Q, -H - EPS],
                [H, -Q - EPS],
                [Q, -H - EPS],
                [0.0, -Q - EPS],
                [-Q, -H - EPS],
                [-H, -Q - EPS],
                [-3.0 * Q, -H - EPS],
                [-PI - EPS, -Q],
                [-PI - EPS, Q],
            ],
            x,
            y,
        ),
        Kind::Rhealpix {
            north_square,
            south_square,
        } => {
            let (n, s) = (f64::from(north_square), f64::from(south_square));
            pnpoly(
                &[
                    [-PI - EPS, Q + EPS],
                    [-PI + n * H - EPS, Q + EPS],
                    [-PI + n * H - EPS, 3.0 * Q + EPS],
                    [-PI + (n + 1.0) * H + EPS, 3.0 * Q + EPS],
                    [-PI + (n + 1.0) * H + EPS, Q + EPS],
                    [PI + EPS, Q + EPS],
                    [PI + EPS, -Q - EPS],
                    [-PI + (s + 1.0) * H + EPS, -Q - EPS],
                    [-PI + (s + 1.0) * H + EPS, -3.0 * Q - EPS],
                    [-PI + s * H - EPS, -3.0 * Q - EPS],
                    [-PI + s * H - EPS, -Q - EPS],
                    [-PI - EPS, -Q - EPS],
                ],
                x,
                y,
            )
        }
    }
}

#[derive(Debug, PartialEq)]
enum Region {
    North,
    South,
    Equatorial,
}

/// The polar cap of a point and its pole point (`get_cap`).
fn cap(
    x: f64,
    y: f64,
    north_square: i32,
    south_square: i32,
    inverse: bool,
) -> (Region, i32, f64, f64) {
    if !inverse {
        let (region, c) = if y > FRAC_PI_4 {
            (Region::North, FRAC_PI_2)
        } else if y < -FRAC_PI_4 {
            (Region::South, -FRAC_PI_2)
        } else {
            return (Region::Equatorial, 0, x, y);
        };
        let (cn, cx) = if x < -FRAC_PI_2 {
            (0, -3.0 * FRAC_PI_4)
        } else if x < 0.0 {
            (1, -FRAC_PI_4)
        } else if x < FRAC_PI_2 {
            (2, FRAC_PI_4)
        } else {
            (3, 3.0 * FRAC_PI_4)
        };
        return (region, cn, cx, c);
    }
    let (region, cx, cy, x, square) = if y > FRAC_PI_4 {
        let sq = f64::from(north_square);
        (
            Region::North,
            -3.0 * FRAC_PI_4 + sq * FRAC_PI_2,
            FRAC_PI_2,
            x - sq * FRAC_PI_2,
            north_square,
        )
    } else if y < -FRAC_PI_4 {
        let sq = f64::from(south_square);
        (
            Region::South,
            -3.0 * FRAC_PI_4 + sq * FRAC_PI_2,
            -FRAC_PI_2,
            x - sq * FRAC_PI_2,
            south_square,
        )
    } else {
        return (Region::Equatorial, 0, x, y);
    };
    // Where x, y goes when the polar square comes apart.
    let q = FRAC_PI_4;
    let cn = if region == Region::North {
        if y >= -x - q - EPS && y < x + 5.0 * q - EPS {
            (square + 1) % 4
        } else if y > -x - q + EPS && y >= x + 5.0 * q - EPS {
            (square + 2) % 4
        } else if y <= -x - q + EPS && y > x + 5.0 * q + EPS {
            (square + 3) % 4
        } else {
            square
        }
    } else if y <= x + q + EPS && y > -x - 5.0 * q + EPS {
        (square + 1) % 4
    } else if y < x + q - EPS && y <= -x - 5.0 * q + EPS {
        (square + 2) % 4
    } else if y >= x + q - EPS && y < -x - 5.0 * q - EPS {
        (square + 3) % 4
    } else {
        square
    };
    (region, cn, cx, cy)
}

/// The polar triangles into a square, or back (`combine_caps`).
fn combine_caps(x: f64, y: f64, north_square: i32, south_square: i32, inverse: bool) -> (f64, f64) {
    let (region, cn, cx, cy) = cap(x, y, north_square, south_square, inverse);
    if region == Region::Equatorial {
        return (cx, cy);
    }
    let north = region == Region::North;
    let pole = if north { north_square } else { south_square };
    let index = if north != inverse {
        cn - pole
    } else {
        -(cn - pole)
    };
    let r = ROT[rotate_index(index)];
    let (vx, vy) = (x - cx, y - cy);
    let base = if inverse { cn } else { pole };
    (
        r[0][0] * vx + r[0][1] * vy - 3.0 * FRAC_PI_4 + f64::from(base) * FRAC_PI_2,
        r[1][0] * vx + r[1][1] * vy + if north { FRAC_PI_2 } else { -FRAC_PI_2 },
    )
}

impl Projection for Healpix {
    fn forward(&self, lon: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = adjlon(lon - self.lon0);
        let phi = self.authalic.as_ref().map_or(phi, |au| au.forward(phi));
        let xy = sphere(lam, phi);
        let (x, y) = match self.kind {
            Kind::Healpix { rot_xy } => rotate(xy, -rot_xy),
            Kind::Rhealpix {
                north_square,
                south_square,
            } => combine_caps(xy.0, xy.1, north_square, south_square, false),
        };
        Ok((self.a * x + self.fe, self.a * y + self.fn_))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let ra = 1.0 / self.a;
        let xy = ((x - self.fe) * ra, (y - self.fn_) * ra);
        let (x, y) = match self.kind {
            Kind::Healpix { rot_xy } => rotate(xy, rot_xy),
            Kind::Rhealpix { .. } => xy,
        };
        if !in_image(x, y, self.kind) {
            return Err(CoordinateError::OutsideDomain);
        }
        let (x, y) = match self.kind {
            Kind::Healpix { .. } => (x, y),
            Kind::Rhealpix {
                north_square,
                south_square,
            } => combine_caps(x, y, north_square, south_square, true),
        };
        let (lam, phi) = sphere_inverse(x, y);
        let phi = self.authalic.as_ref().map_or(phi, |au| au.inverse(phi));
        Ok((lam + self.lon0, phi))
    }
}
