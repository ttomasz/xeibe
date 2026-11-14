//! Lambert Conic Near-Conformal (EPSG:9817), which PROJ has no mapping
//! for: the Lambert Conic with the meridional arc truncated to its cubic
//! term, as the Levant Zone grid was computed (EPSG Guidance Note 7-2).
//! The reverse solves EPSG's two equations exactly by iteration rather
//! than with its 3 mm shortcut.

use super::{BuildError, Params, Projection, adjlon, param};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

const MAX_ITERATIONS: usize = 20;

#[derive(Debug)]
pub(crate) struct NearConformal {
    lon0: f64,
    sin0: f64,
    k0: f64,
    fe: f64,
    fn_: f64,
    /// A = 1 / (6 ρ0 ν0).
    a3: f64,
    r0: f64,
    s0: f64,
    /// The meridional arc's series: A′ (per degree), B′, C′, D′, E′.
    arc: [f64; 5],
}

impl NearConformal {
    pub(crate) fn new(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        let phi0 = params.get(param::LAT_NATURAL_ORIGIN)?;
        let (sin0, cos0) = phi0.sin_cos();
        if sin0.abs() < 1e-10 || cos0.abs() < 1e-10 {
            return Err(BuildError::Invalid(
                "the latitude of natural origin must be off the equator and the poles".into(),
            ));
        }
        let k0 = params.get(param::SCALE_NATURAL_ORIGIN)?;
        let (a, f, es) = (ellipsoid.a, ellipsoid.f, ellipsoid.es);
        let n = f / (2.0 - f);
        let (n2, n3, n4, n5) = (n * n, n.powi(3), n.powi(4), n.powi(5));
        let arc = [
            a * (1.0 - n + 5.0 * (n2 - n3) / 4.0 + 81.0 * (n4 - n5) / 64.0) * std::f64::consts::PI
                / 180.0,
            3.0 * a * (n - n2 + 7.0 * (n3 - n4) / 8.0 + 55.0 * n5 / 64.0) / 2.0,
            15.0 * a * (n2 - n3 + 3.0 * (n4 - n5) / 4.0) / 16.0,
            35.0 * a * (n3 - n4 + 11.0 * n5 / 16.0) / 48.0,
            315.0 * a * (n4 - n5) / 512.0,
        ];
        let w = 1.0 - es * sin0 * sin0;
        let nu0 = a / w.sqrt();
        let rho0 = a * (1.0 - es) / (w * w.sqrt());
        let mut out = NearConformal {
            lon0: params.get(param::LON_NATURAL_ORIGIN)?,
            sin0,
            k0,
            fe: params.get(param::FALSE_EASTING)?,
            fn_: params.get(param::FALSE_NORTHING)?,
            a3: 1.0 / (6.0 * rho0 * nu0),
            r0: k0 * nu0 * cos0 / sin0,
            s0: 0.0,
            arc,
        };
        out.s0 = out.s(phi0);
        Ok(out)
    }

    /// The meridional arc to `phi` (radians), its first term in degrees.
    fn s(&self, phi: f64) -> f64 {
        let [a, b, c, d, e] = self.arc;
        a * phi.to_degrees() - b * (2.0 * phi).sin() + c * (4.0 * phi).sin() - d * (6.0 * phi).sin()
            + e * (8.0 * phi).sin()
    }

    /// ds/dφ, per radian.
    fn ds(&self, phi: f64) -> f64 {
        let [a, b, c, d, e] = self.arc;
        a.to_degrees() - 2.0 * b * (2.0 * phi).cos() + 4.0 * c * (4.0 * phi).cos()
            - 6.0 * d * (6.0 * phi).cos()
            + 8.0 * e * (8.0 * phi).cos()
    }
}

impl Projection for NearConformal {
    fn forward(&self, lon: f64, lat: f64) -> Result<(f64, f64), CoordinateError> {
        let m = self.s(lat) - self.s0;
        let big_m = self.k0 * (m + self.a3 * m * m * m);
        let r = self.r0 - big_m;
        let theta = adjlon(lon - self.lon0) * self.sin0;
        let sin = theta.sin();
        // FN + r0 − r cos θ, as EPSG writes it.
        Ok((
            self.fe + r * sin,
            self.fn_ + big_m + r * sin * (theta / 2.0).tan(),
        ))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let (de, dn) = (x - self.fe, self.r0 - (y - self.fn_));
        let theta = if self.sin0 >= 0.0 {
            de.atan2(dn)
        } else {
            (-de).atan2(-dn)
        };
        let r = de.hypot(dn).copysign(self.sin0);
        let big_m = self.r0 - r;
        // k0 m + k0 A m³ = M', by Newton from m = M'.
        let mut m = big_m;
        for _ in 0..MAX_ITERATIONS {
            let step = (big_m - self.k0 * m - self.k0 * self.a3 * m * m * m)
                / (-self.k0 - 3.0 * self.k0 * self.a3 * m * m);
            m -= step;
            if step.abs() < 1e-9 {
                break;
            }
        }
        // s(φ) = m + s0, by Newton from the origin's latitude.
        let mut phi = (self.s0 + m) / self.arc[0] * std::f64::consts::PI / 180.0;
        for i in 0.. {
            let step = (m + self.s0 - self.s(phi)) / self.ds(phi);
            phi += step;
            if step.abs() < 1e-14 {
                break;
            }
            if i == MAX_ITERATIONS {
                return Err(CoordinateError::NoConvergence);
            }
        }
        Ok((self.lon0 + theta / self.sin0, phi))
    }
}
