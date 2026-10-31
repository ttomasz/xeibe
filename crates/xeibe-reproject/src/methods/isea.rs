//! Icosahedral Snyder Equal Area (`isea`): Snyder's equal-area projection
//! of the sphere onto an icosahedron, laid out flat, or as the
//! icosahedron's quads and hexagons of a discrete global grid (`mode=di`,
//! `dd`, `hex`).
//!
//! Ported from PROJ 9.8.1 `src/projections/isea.cpp` (MIT, see
//! PROJ-NOTICE.md, which also has geogrid's licence): the forward from
//! Nathan Wagner's public-domain code, the inverse from Franz-Benjamin
//! Mocnik's MIT geogrid by Jérôme Jacovella-St-Louis. As in PROJ, the
//! forward works on the sphere of the semi-major axis with the geodetic
//! latitude, while the inverse works on the authalic sphere and only
//! exists for the plane with PROJ's default grid, in the ISEA or polar
//! orientation. `lon_0` and `lat_0` set the icosahedron's orientation, and
//! `lon_0` is also the central meridian.

// PROJ's constants, digit for digit.
#![allow(clippy::excessive_precision)]

use std::f64::consts::{FRAC_PI_2, PI, TAU};

use super::{BuildError, Params, Projection, adjlon};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

const DEG: f64 = PI / 180.0;
const DEG120: f64 = 2.09439510239319549229;
/// √5/π.
const ISEA_SCALE: f64 = 0.8301572857837594396028083;
const E_RAD: f64 = 0.91843818701052843323;
const F_RAD: f64 = 0.18871053078356206978;
const SDC2VOS: f64 = 0.6523581397843681859886783;
/// tan(SDC2VOS).
const TANG: f64 = 0.76393202250021030358019673567;
const TAN30: f64 = 0.57735026918962576450914878;
const COT_THETA: f64 = 1.0 / TAN30;
const COS_G: f64 = 0.80901699437494742410229341718281905886;
const SIN_G: f64 = 0.587785252292473129168705954639072768597652;
const COS_SDC2VOS: f64 = 0.7946544722917661229596057297879189448539;
const SIN_G_COS_SDC2VOS: f64 = SIN_G * COS_SDC2VOS;
const SQRT3: f64 = 1.73205080756887729352744634150587236694280525381038;
const SIN60: f64 = SQRT3 / 2.0;
const COS30: f64 = SQRT3 / 2.0;
const TABLE_G: f64 = TANG * SIN60;
/// R' / R.
const RPRIME_OVER_R: f64 = 0.9103832815095032;
const TABLE_H: f64 = 0.25 * TANG;
const ISEA_STD_LAT: f64 = 1.01722196792335072101;
const ISEA_STD_LONG: f64 = 0.19634954084936207740;
const FACES: usize = 20;

const OUTSIDE: CoordinateError = CoordinateError::OutsideDomain;

/// The faces' centres, the dodecahedron's vertices: latitude and
/// longitude in degrees, the latitude as `E_RAD` or `F_RAD` in radians.
const CENTRES: [(f64, f64); FACES] = [
    (E_RAD, -144.0),
    (E_RAD, -72.0),
    (E_RAD, 0.0),
    (E_RAD, 72.0),
    (E_RAD, 144.0),
    (F_RAD, -144.0),
    (F_RAD, -72.0),
    (F_RAD, 0.0),
    (F_RAD, 72.0),
    (F_RAD, 144.0),
    (-F_RAD, -108.0),
    (-F_RAD, -36.0),
    (-F_RAD, 36.0),
    (-F_RAD, 108.0),
    (-F_RAD, 180.0),
    (-E_RAD, -108.0),
    (-E_RAD, -36.0),
    (-E_RAD, 36.0),
    (-E_RAD, 108.0),
    (-E_RAD, 180.0),
];

fn centre_lon(face: usize) -> f64 {
    DEG * CENTRES[face].1
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Output {
    Plane,
    Q2di,
    Q2dd,
    Hex,
}

/// What the inverse needs (`pj_isea_data::initialize` and
/// `ISEAPlanarProjection`).
#[derive(Debug)]
struct Planar {
    orientation: (f64, f64),
    cos_orientation_lat: f64,
    sin_orientation_lat: f64,
    r2: f64,
    rprime2x: f64,
    rprime_tang: f64,
    rprime2_tan2g: f64,
    center_to_base: f64,
    tri_width: f64,
    y_offsets: [f64; 4],
    xo: f64,
    yo: f64,
    sx: f64,
    sy: f64,
}

#[derive(Debug)]
pub(crate) struct Isea {
    lon0: f64,
    fe: f64,
    fn_: f64,
    a: f64,
    o_lat: f64,
    o_lon: f64,
    o_az: f64,
    aperture: i32,
    resolution: i32,
    output: Output,
    centre_sin_cos: [(f64, f64); FACES],
    planar: Option<Planar>,
}

fn safe_asin(t: f64) -> f64 {
    const E: f64 = 1e-15;
    if t.abs() < E {
        0.0
    } else if (t - 1.0).abs() < E {
        FRAC_PI_2
    } else if (t + 1.0).abs() < E {
        -FRAC_PI_2
    } else {
        t.asin()
    }
}

fn safe_acos(t: f64) -> f64 {
    const E: f64 = 1e-15;
    if t.abs() < E {
        FRAC_PI_2
    } else if (t + 1.0).abs() < E {
        PI
    } else if (t - 1.0).abs() < E {
        0.0
    } else {
        t.acos()
    }
}

/// A longitude reduced to ±π as PROJ's ISEA does: fmod, then by turns.
fn normalize(lon: f64) -> f64 {
    let mut lon = lon % TAU;
    while lon > PI {
        lon -= TAU;
    }
    while lon < -PI {
        lon += TAU;
    }
    lon
}

impl Isea {
    pub(crate) fn new(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        let invalid = |m: &str| Err(BuildError::Invalid(m.into()));
        let (mut o_lat, mut o_lon) = match params.flag("orient") {
            None | Some("isea") => (ISEA_STD_LAT, ISEA_STD_LONG),
            Some("pole") => (FRAC_PI_2, 0.0),
            Some(_) => return invalid("Invalid value for orient: only isea or pole are supported"),
        };
        let o_az = params.key("azi").unwrap_or(0.0);
        if let Some(lon) = params.key("lon_0") {
            o_lon = lon;
        }
        if let Some(lat) = params.key("lat_0") {
            o_lat = lat;
        }
        let output = match params.flag("mode") {
            None | Some("plane") => Output::Plane,
            Some("di") => Output::Q2di,
            Some("dd") => Output::Q2dd,
            Some("hex") => Output::Hex,
            Some(_) => {
                return invalid("Invalid value for mode: only plane, di, dd or hex are supported");
            }
        };
        // pj_param's integers.
        let resolution = params.key("resolution").map_or(4, |r| r as i32);
        let aperture = params.key("aperture").map_or(3, |a| a as i32);
        let orientation =
            if output == Output::Plane && o_az == 0.0 && aperture == 3 && resolution == 4 {
                if o_lat == ISEA_STD_LAT && o_lon == ISEA_STD_LONG {
                    Some(((E_RAD + F_RAD) / 2.0, DEG * -11.25))
                } else if o_lat == FRAC_PI_2 && o_lon == 0.0 {
                    Some((0.0, 0.0))
                } else {
                    None
                }
            } else {
                None
            };
        let planar = orientation.map(|orientation| {
            let e = ellipsoid.e;
            let r2 = if e > 0.0 {
                let (a2, c2) = (ellipsoid.a * ellipsoid.a, ellipsoid.b * ellipsoid.b);
                let s = PI * (2.0 * a2 + c2 / e * ((1.0 + e) / (1.0 - e)).ln());
                s / (4.0 * PI)
            } else {
                ellipsoid.a * ellipsoid.a
            };
            let rprime = RPRIME_OVER_R * r2.sqrt();
            let rprime_tang = rprime * TANG;
            let center_to_base = rprime_tang / 2.0;
            let tri_width = rprime_tang * SQRT3;
            Planar {
                orientation,
                cos_orientation_lat: orientation.0.cos(),
                sin_orientation_lat: orientation.0.sin(),
                r2,
                rprime2x: 2.0 * rprime,
                rprime_tang,
                rprime2_tan2g: rprime_tang * rprime_tang,
                center_to_base,
                tri_width,
                y_offsets: [-2.0, -4.0, -5.0, -7.0].map(|k| k * center_to_base),
                xo: 2.5 * tri_width,
                yo: -1.5 * center_to_base,
                sx: 1.0 / tri_width,
                sy: 1.0 / (3.0 * center_to_base),
            }
        });
        let key = |k: &str| params.key(k).unwrap_or(0.0);
        Ok(Isea {
            lon0: key("lon_0"),
            fe: key("x_0"),
            fn_: key("y_0"),
            a: ellipsoid.a,
            o_lat,
            o_lon,
            o_az,
            aperture,
            resolution,
            output,
            centre_sin_cos: CENTRES.map(|(lat, _)| lat.sin_cos()),
            planar,
        })
    }

    /// The face of a point turned to the icosahedron's orientation, and
    /// its x and y on the face (`isea_snyder_forward`).
    fn snyder_forward(&self, lat: f64, lon: f64) -> Result<(usize, f64, f64), CoordinateError> {
        let (sin_lat, cos_lat) = lat.sin_cos();
        for i in 0..FACES {
            let (sin_c, cos_c) = self.centre_sin_cos[i];
            let d_lon = lon - centre_lon(i);
            let cos_lat_cos_lon = cos_lat * d_lon.cos();
            let z = safe_acos(sin_c * sin_lat + cos_c * cos_lat_cos_lon);
            // Not on this triangle.
            if z > SDC2VOS + 0.000005 {
                continue;
            }
            // Snyder's eq. 14.
            let mut az = (cos_lat * d_lon.sin()).atan2(cos_c * sin_lat - sin_c * cos_lat_cos_lon);
            az -= if (5..=9).contains(&i) || i == 15 || i == 16 {
                PI
            } else if i >= 17 {
                -PI
            } else {
                0.0
            };
            if az < 0.0 {
                az += 2.0 * PI;
            }
            // Into 0 to 120°, counting the turns.
            let mut turns = 0;
            while az < 0.0 {
                az += DEG120;
                turns -= 1;
            }
            while az > DEG120 + f64::EPSILON {
                az -= DEG120;
                turns += 1;
            }
            let (sin_az, cos_az) = az.sin_cos();
            // Eq. 9.
            let q = TANG.atan2(cos_az + sin_az * COT_THETA);
            if z > q + 0.000005 {
                continue;
            }
            // Eqs. 5 to 8 and 10 to 12.
            let h = (sin_az * SIN_G_COS_SDC2VOS - cos_az * COS_G).acos();
            let ag = az + DEG * 36.0 + h - PI;
            let mut az_prime = (2.0 * ag)
                .atan2(RPRIME_OVER_R * RPRIME_OVER_R * TANG * TANG - 2.0 * ag * COT_THETA);
            let d_prime = RPRIME_OVER_R * TANG / (az_prime.cos() + az_prime.sin() * COT_THETA);
            let f = d_prime / (2.0 * RPRIME_OVER_R * (q / 2.0).sin());
            let rho = 2.0 * RPRIME_OVER_R * f * (z / 2.0).sin();
            az_prime += DEG120 * f64::from(turns);
            return Ok((i, rho * az_prime.sin(), rho * az_prime.cos()));
        }
        // PROJ exits: no triangle holds the point.
        Err(OUTSIDE)
    }

    fn isea_forward(&self, lat: f64, lon: f64) -> Result<(f64, f64), CoordinateError> {
        // The point with the icosahedron's orientation as its pole.
        let (lat, lon) = {
            let (alpha, beta) = (self.o_lat, self.o_lon + PI);
            let dlambda = lon - beta;
            let (sin_p, cos_p) = lat.sin_cos();
            let (sin_a, cos_a) = alpha.sin_cos();
            let (sin_dl, cos_dl) = dlambda.sin_cos();
            let sin_phip = sin_a * sin_p - cos_a * cos_p * cos_dl;
            let lp_b = (cos_p * sin_dl).atan2(sin_a * cos_p * cos_dl + cos_a * sin_p);
            let lon = normalize(lp_b + beta);
            (
                safe_asin(sin_phip),
                normalize(lon - (-self.o_az + self.o_lon)),
            )
        };
        let (tri, x, y) = self.snyder_forward(lat, lon)?;
        let down = (tri / 5) % 2 == 1;
        if self.output == Output::Plane {
            let (x, y) = if down { (-x, -y) } else { (x, y) };
            let (cx, cy) = triangle_xy(tri);
            return Ok((x + cx, y + cy));
        }
        // The ISEA standard triangle's size.
        let pt = (
            x * ISEA_SCALE + 0.5,
            y * ISEA_SCALE + 2.0 * 0.14433756729740644112,
        );
        let (quadz, pt) = ptdd(tri, pt);
        match self.output {
            Output::Q2dd => Ok(pt),
            Output::Q2di => {
                let (_, di) = self.dddi(quadz, pt)?;
                Ok(di)
            }
            _ => {
                let (quadz, (d, i)) = self.dddi(quadz, pt)?;
                let d = d as i32;
                if !((i32::MIN >> 4)..=(i32::MAX >> 4)).contains(&d) {
                    return Err(OUTSIDE);
                }
                Ok((f64::from(d * 16 + quadz), i))
            }
        }
    }

    /// The quad and its d and i of a point of quad `quadz` (`isea_dddi`).
    fn dddi(&self, quadz: i32, pt: (f64, f64)) -> Result<(i32, (f64, f64)), CoordinateError> {
        if self.aperture == 3 && self.resolution % 2 != 0 {
            return self.dddi_ap3odd(quadz, pt);
        }
        let sidelength = if self.aperture > 0 {
            let s = f64::from(self.aperture).powf(f64::from(self.resolution) / 2.0);
            if s.abs() > f64::from(i32::MAX) {
                return Err(OUTSIDE);
            }
            s.round() as i64
        } else {
            i64::from(self.resolution)
        };
        if sidelength == 0 {
            return Err(OUTSIDE);
        }
        let (vx, vy) = rotate(pt, -30.0);
        let (hx, hy) = hexbin2(1.0 / sidelength as f64, vx, vy)?;
        // Only x and z make d and i: PROJ's updates of y are left out.
        let (mut x, y, mut z) = hex_iso(hx, hy);
        let mut quadz = quadz;
        // The point may be on another quad.
        if quadz <= 5 {
            if x == 0 && z == -sidelength {
                // North pole.
                quadz = 0;
                (x, z) = (0, 0);
            } else if z == -sidelength {
                quadz += 1;
                if quadz == 6 {
                    quadz = 1;
                }
                z = x - sidelength;
                x = 0;
            } else if x == sidelength {
                quadz += 5;
                x = 0;
            }
        } else if z == 0 && x == sidelength {
            // South pole.
            quadz = 11;
            (x, z) = (0, 0);
        } else if x == sidelength {
            quadz += 1;
            if quadz == 11 {
                quadz = 6;
            }
            x = y + sidelength;
            z = -x;
        } else if y == -sidelength {
            quadz -= 4;
            z = -x;
        }
        Ok((quadz, (x as f64, -z as f64)))
    }

    fn dddi_ap3odd(
        &self,
        quadz: i32,
        (vx, vy): (f64, f64),
    ) -> Result<(i32, (f64, f64)), CoordinateError> {
        // Hexes from a triangle's apex to its base.
        let sidelength = (2f64.powi(self.resolution) + 1.0) / 2.0;
        // Apex to base is cos 30°.
        let hexwidth = (PI / 6.0).cos() / sidelength;
        let maxcoord = (sidelength * 2.0).round() as i64;
        let (hx, hy) = hexbin2(hexwidth, vx, vy)?;
        let (x, y, z) = hex_iso(hx, hy);
        let mut d = x - z;
        let mut i = x + y + y;
        let mut quadz = quadz;
        // The next quad in the same row first, for where both are at
        // their maximum.
        if quadz <= 5 {
            if d == 0 && i == maxcoord {
                // North pole.
                quadz = 0;
                d = 0;
                i = 0;
            } else if i == maxcoord {
                quadz += 1;
                if quadz == 6 {
                    quadz = 1;
                }
                i = maxcoord - d;
                d = 0;
            } else if d == maxcoord {
                quadz += 5;
                d = 0;
            }
        } else if i == 0 && d == maxcoord {
            // South pole.
            quadz = 11;
            d = 0;
            i = 0;
        } else if d == maxcoord {
            quadz += 1;
            if quadz == 11 {
                quadz = 6;
            }
            d = maxcoord - i;
            i = 0;
        } else if i == maxcoord {
            quadz = (quadz - 4) % 5;
            i = 0;
        }
        Ok((quadz, (d as f64, i as f64)))
    }

    /// The point of the plane back on the sphere, the face found from it
    /// (`ISEAPlanarProjection::cartesianToGeo`).
    fn cartesian_to_geo(&self, p: &Planar, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        const EPSILON: f64 = 1e-11;
        const SR: f64 = -SIN60;
        const CR: f64 = 0.5;
        let (mut px, py) = (x, y);
        // Wrap around.
        if px < 0.0 || (px < p.tri_width / 2.0 && py < 0.0 && py * CR < px * SR) {
            px += 5.0 * p.tri_width;
        }
        let yp = -(px * SR + py * CR);
        let mut x = (px * CR - py * SR + yp / SQRT3) * p.sx;
        let mut y = yp * p.sy;
        if x < 0.0 || (y > x && x < 5.0 - EPSILON) {
            x += EPSILON;
        } else if x > 5.0 || (y < x && x > EPSILON) {
            x -= EPSILON;
        }
        if y < 0.0 || (x > y && y < 6.0 - EPSILON) {
            y += EPSILON;
        } else if y > 6.0 || (x < y && y > EPSILON) {
            y -= EPSILON;
        }
        if !((0.0..=5.0).contains(&x) && (0.0..=6.0).contains(&y)) {
            return Err(OUTSIDE);
        }
        let ix = (x as i32).clamp(0, 4);
        let iy = (y as i32).clamp(0, 5);
        if iy != ix && iy != ix + 1 {
            return Err(OUTSIDE);
        }
        let rhombus = ix + iy;
        let top = x - f64::from(ix) > y - f64::from(iy);
        // Even rhombi are the northern rows' faces, odd the southern.
        let column = (rhombus / 2) as usize;
        let face = match (rhombus % 2 == 0, top) {
            (true, true) => column,
            (true, false) => column + 5,
            (false, true) => column + 10,
            (false, false) => column + 15,
        };
        let fy = face / 5;
        let fx = face - 5 * fy;
        let rx = px - (2 * fx + fy / 2 + 1) as f64 * p.tri_width / 2.0;
        let ry = py - (p.y_offsets[fy] + 3.0 * p.center_to_base);
        let (lat, mut lon) = self.icosahedron_to_sphere(p, face, rx, ry)?;
        if lon < -PI - EPSILON {
            lon += TAU;
        } else if lon > PI + EPSILON {
            lon -= TAU;
        }
        Ok((lat, lon))
    }

    fn icosahedron_to_sphere(
        &self,
        p: &Planar,
        face: usize,
        x: f64,
        y: f64,
    ) -> Result<(f64, f64), CoordinateError> {
        const PRECISION: f64 = DEG * 1e-11;
        const AZ_MAX: f64 = DEG * 120.0;
        const WEST_VERTEX_LON: f64 = DEG * -144.0;
        let mut az = x.atan2(y);
        let rho = (x * x + y * y).sqrt();
        let mut az_adjustment = if face <= 4 || (10..=14).contains(&face) {
            0.0
        } else {
            DEG * 180.0
        };
        az += az_adjustment;
        while az < 0.0 {
            az_adjustment += AZ_MAX;
            az += AZ_MAX;
        }
        while az > AZ_MAX {
            az_adjustment -= AZ_MAX;
            az -= AZ_MAX;
        }
        let (sin_az, cos_az) = az.sin_cos();
        let cot_az = cos_az / sin_az;
        // A_G, or A_ABD.
        let area = p.rprime2_tan2g / (2.0 * (cot_az + COT_THETA));
        let mut delta_az = 10.0 * PRECISION;
        let target = area / p.r2 - WEST_VERTEX_LON;
        let mut az_earth = az;
        // Newton's method; PROJ has no limit, NaN ends it.
        let mut iterations = 0;
        while delta_az.abs() > PRECISION {
            iterations += 1;
            if iterations > 100 {
                return Err(CoordinateError::NoConvergence);
            }
            let (s, c) = az_earth.sin_cos();
            let h = (s * SIN_G_COS_SDC2VOS - c * COS_G).acos();
            let f = target - h - az_earth;
            let f2 = (c * SIN_G_COS_SDC2VOS + s * COS_G) / h.sin() - 1.0;
            delta_az = -f / f2;
            az_earth += delta_az;
        }
        let (s, c) = az_earth.sin_cos();
        let q = TANG.atan2(c + s * COT_THETA);
        let d = p.rprime_tang / (cos_az + sin_az * COT_THETA);
        let f = d / (p.rprime2x * (q / 2.0).sin());
        let z = 2.0 * (rho / (p.rprime2x * f)).asin();
        az_earth -= az_adjustment;
        let (sin_lat0, cos_lat0) = self.centre_sin_cos[face];
        let (sin_z, cos_z) = z.sin_cos();
        let cos_lat0_sin_z = cos_lat0 * sin_z;
        let lat = safe_asin(sin_lat0 * cos_z + cos_lat0_sin_z * az_earth.cos());
        let lon = centre_lon(face)
            + (az_earth.sin() * cos_lat0_sin_z).atan2(cos_z - sin_lat0 * lat.sin());
        // Back from the orientation.
        let precision_per_definition = DEG * 1e-5;
        let lon = if lat < DEG * -90.0 + precision_per_definition
            || lat > DEG * 90.0 - precision_per_definition
        {
            0.0
        } else {
            lon
        };
        if p.orientation.0 != 0.0 || p.orientation.1 != 0.0 {
            let (sin_lat, cos_lat) = lat.sin_cos();
            let (sin_lon, cos_lon) = lon.sin_cos();
            let cos_lon_cos_lat = cos_lon * cos_lat;
            Ok((
                (sin_lat * p.cos_orientation_lat - cos_lon_cos_lat * p.sin_orientation_lat).asin(),
                (sin_lon * cos_lat).atan2(
                    cos_lon_cos_lat * p.cos_orientation_lat + sin_lat * p.sin_orientation_lat,
                ) - p.orientation.1,
            ))
        } else {
            Ok((lat, lon))
        }
    }
}

/// The centre of a triangle on the plane (`isea_triangle_xy`).
fn triangle_xy(tri: usize) -> (f64, f64) {
    let tri = tri % FACES;
    let mut x = TABLE_G * ((tri % 5) as f64 - 2.0) * 2.0;
    if tri > 9 {
        x += TABLE_G;
    }
    let y = match tri / 5 {
        0 => 5.0 * TABLE_H,
        1 => TABLE_H,
        2 => -TABLE_H,
        _ => -5.0 * TABLE_H,
    };
    (x * RPRIME_OVER_R, y * RPRIME_OVER_R)
}

/// Turns a point by `degrees` clockwise (`isea_rotate`).
fn rotate((x, y): (f64, f64), degrees: f64) -> (f64, f64) {
    let mut rad = -degrees * PI / 180.0;
    while rad >= 2.0 * PI {
        rad -= 2.0 * PI;
    }
    while rad <= -2.0 * PI {
        rad += 2.0 * PI;
    }
    let (s, c) = rad.sin_cos();
    (x * c + y * s, -x * s + y * c)
}

/// A triangle's point to its quad's, and the quad (`isea_ptdd`).
fn ptdd(tri: usize, pt: (f64, f64)) -> (i32, (f64, f64)) {
    let down = (tri / 5) % 2 == 1;
    let quadz = (tri % 5) as i32 + (tri / 10) as i32 * 5 + 1;
    let (x, y) = rotate(pt, if down { 240.0 } else { 60.0 });
    if down {
        (quadz, (x + 0.5, y + COS30))
    } else {
        (quadz, (x, y))
    }
}

/// Isometric hex coordinates of xy ones (`hex_iso`): x, y and z.
fn hex_iso(x: i64, y: i64) -> (i64, i64, i64) {
    // Integer division truncates, as in C.
    let y = if x >= 0 { -y - (x + 1) / 2 } else { -y - x / 2 };
    (x, y, -x - y)
}

/// The hexagon of width `width` that holds a point, as xy hex coordinates
/// (`hexbin2`).
fn hexbin2(width: f64, x: f64, y: f64) -> Result<(i64, i64), CoordinateError> {
    // Rotated x, and y adjusted for it.
    let x = x / (30.0 * PI / 180.0).cos();
    let y = y - x / 2.0;
    if width == 0.0 {
        return Err(OUTSIDE);
    }
    let (x, y) = (x / width, y / width);
    let z = -x - y;
    let (rx, ry, rz) = ((x + 0.5).floor(), (y + 0.5).floor(), (z + 0.5).floor());
    let (mut ix, mut iy, iz) = (rx.round() as i64, ry.round() as i64, rz.round() as i64);
    let max = f64::from(i32::MAX);
    if (ix as f64 + iy as f64).abs() > max || (ix as f64 + iy as f64 + iz as f64).abs() > max {
        return Err(OUTSIDE);
    }
    let s = ix + iy + iz;
    if s != 0 {
        let (dx, dy, dz) = ((rx - x).abs(), (ry - y).abs(), (rz - z).abs());
        if dx >= dy && dx >= dz {
            ix -= s;
        } else if dy >= dx && dy >= dz {
            iy -= s;
        }
        // Else PROJ corrects z, which hex_xy drops.
    }
    // hex_xy: from isometric back.
    let iy = if ix >= 0 {
        -iy - (ix + 1) / 2
    } else {
        -iy - ix / 2
    };
    Ok((ix, iy))
}

impl Projection for Isea {
    fn forward(&self, lon: f64, lat: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = adjlon(lon - self.lon0);
        let (x, y) = self.isea_forward(lat, lam)?;
        Ok((self.a * x + self.fe, self.a * y + self.fn_))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let Some(p) = &self.planar else {
            return Err(OUTSIDE);
        };
        // PROJ's 1/a and a cancel.
        let (x, y) = (x - self.fe + p.xo, y - self.fn_ + p.yo);
        let (lat, lon) = self.cartesian_to_geo(p, x, y)?;
        Ok((adjlon(lon + self.lon0), lat))
    }
}
