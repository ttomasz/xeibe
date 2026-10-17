//! Conversion between two CRSs on the same datum.

use crate::crs::{Axes, Crs, Kind};
use crate::ellipsoid::Ellipsoid;
use crate::error::{CoordinateError, Error};
use crate::methods::{LAT_LIMIT, adjlon};

/// The order coordinates are given and returned in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AxisOrder {
    /// Easting or longitude first, as xeibe writes GeoArrow and GeoParquet:
    /// the first two values are swapped for a CRS whose first axis points
    /// north. Other axes stay as the CRS defines them, so a CRS with westing
    /// and southing axes keeps them.
    #[default]
    EastFirst,
    /// The CRS's own axis order, e.g. latitude first for EPSG:4326.
    Authority,
}

/// A conversion from one CRS to another, set up once and applied to many
/// coordinates.
///
/// Both CRSs must be on the same datum: this is a conversion (projection,
/// change of units or axes, geographic ↔ geocentric), never a datum
/// transformation.
#[derive(Debug, Clone)]
pub struct Transform {
    src: Crs,
    dst: Crs,
    order: AxisOrder,
}

impl Transform {
    /// Set up the conversion from `src` to `dst`.
    pub fn new(src: &Crs, dst: &Crs, order: AxisOrder) -> Result<Self, Error> {
        let (a, b) = (src.datum(), dst.datum());
        if a.key != b.key || a.ellipsoid != b.ellipsoid || a.prime_meridian != b.prime_meridian {
            return Err(Error::DatumMismatch {
                src: src.label(),
                dst: dst.label(),
                src_datum: a.name.clone(),
                dst_datum: b.name.clone(),
            });
        }
        Ok(Transform {
            src: src.clone(),
            dst: dst.clone(),
            order,
        })
    }

    /// Set up the conversion between two EPSG codes.
    pub fn from_epsg(src: u32, dst: u32, order: AxisOrder) -> Result<Self, Error> {
        Self::new(&Crs::from_epsg(src)?, &Crs::from_epsg(dst)?, order)
    }

    pub fn source(&self) -> &Crs {
        &self.src
    }

    pub fn target(&self) -> &Crs {
        &self.dst
    }

    /// The axis order coordinates are given and returned in.
    pub fn order(&self) -> AxisOrder {
        self.order
    }

    /// The conversion the other way.
    pub fn inverse(&self) -> Transform {
        Transform {
            src: self.dst.clone(),
            dst: self.src.clone(),
            order: self.order,
        }
    }

    /// Convert one coordinate. `z` is the third value when the data has one:
    /// the height for a 3D CRS, carried through unchanged for a 2D one.
    ///
    /// A coordinate with a NaN in x or y (GeoArrow's empty point) comes back
    /// unchanged.
    pub fn convert(
        &self,
        x: f64,
        y: f64,
        z: Option<f64>,
    ) -> Result<(f64, f64, Option<f64>), CoordinateError> {
        if x.is_nan() || y.is_nan() {
            return Ok((x, y, z));
        }
        let internal = to_internal(&self.src.axes, self.order, [x, y, z.unwrap_or(0.0)])?;
        let geographic = match &self.src.kind {
            Kind::Geographic { .. } => check_geographic(internal)?,
            Kind::Projected { projection, .. } => {
                let (lon, lat) = projection.inverse(internal[0], internal[1])?;
                if !(lon.is_finite() && lat.is_finite()) {
                    return Err(CoordinateError::OutsideDomain);
                }
                [adjlon(lon), lat, internal[2]]
            }
            Kind::Geocentric { datum } => geodetic(internal, &datum.ellipsoid),
        };
        let out = match &self.dst.kind {
            Kind::Geographic { .. } => geographic,
            Kind::Projected { projection, .. } => {
                let [lon, lat, h] = check_geographic(geographic)?;
                let (e, n) = projection.forward(lon, lat)?;
                if !(e.is_finite() && n.is_finite()) {
                    return Err(CoordinateError::OutsideDomain);
                }
                [e, n, h]
            }
            Kind::Geocentric { datum } => {
                cartesian(check_geographic(geographic)?, &datum.ellipsoid)
            }
        };
        let [x, y, h] = from_internal(&self.dst.axes, self.order, out);
        // A 2D target has no height of its own: hand back what came in.
        let z = z.map(|z| if self.dst.axes.dimension() == 3 { h } else { z });
        Ok((x, y, z))
    }

    /// Convert coordinates held as separate x and y buffers, in place. Stops
    /// at the first coordinate that fails, leaving the rest unconverted.
    pub fn convert_xy(&self, x: &mut [f64], y: &mut [f64]) -> Result<(), Error> {
        assert_eq!(x.len(), y.len(), "x and y buffers differ in length");
        for (index, (x, y)) in x.iter_mut().zip(y.iter_mut()).enumerate() {
            let (cx, cy, _) = self
                .convert(*x, *y, None)
                .map_err(|reason| coordinate_error(index, *x, *y, reason))?;
            (*x, *y) = (cx, cy);
        }
        Ok(())
    }

    /// Convert coordinates held as separate x, y and z buffers, in place.
    pub fn convert_xyz(&self, x: &mut [f64], y: &mut [f64], z: &mut [f64]) -> Result<(), Error> {
        assert!(
            x.len() == y.len() && y.len() == z.len(),
            "x, y and z buffers differ in length"
        );
        for (index, ((x, y), z)) in x.iter_mut().zip(y.iter_mut()).zip(z.iter_mut()).enumerate() {
            let (cx, cy, cz) = self
                .convert(*x, *y, Some(*z))
                .map_err(|reason| coordinate_error(index, *x, *y, reason))?;
            (*x, *y, *z) = (cx, cy, cz.unwrap_or(*z));
        }
        Ok(())
    }

    /// Convert interleaved coordinates (`x0 y0 x1 y1 …`, or with z when
    /// `dimension` is 3), in place. A fourth ordinate (M) is left alone.
    pub fn convert_interleaved(&self, coords: &mut [f64], dimension: usize) -> Result<(), Error> {
        assert!((2..=4).contains(&dimension), "dimension must be 2, 3 or 4");
        assert_eq!(
            coords.len() % dimension,
            0,
            "buffer is not a whole number of coordinates"
        );
        for (index, c) in coords.chunks_exact_mut(dimension).enumerate() {
            let z = (dimension >= 3).then(|| c[2]);
            let (x, y, z) = self
                .convert(c[0], c[1], z)
                .map_err(|reason| coordinate_error(index, c[0], c[1], reason))?;
            c[0] = x;
            c[1] = y;
            if let Some(z) = z {
                c[2] = z;
            }
        }
        Ok(())
    }
}

fn coordinate_error(index: usize, x: f64, y: f64, reason: CoordinateError) -> Error {
    Error::Coordinate {
        index,
        x,
        y,
        reason,
    }
}

/// From the caller's order and units to the internal order in radians and metres.
fn to_internal(
    axes: &Axes,
    order: AxisOrder,
    mut v: [f64; 3],
) -> Result<[f64; 3], CoordinateError> {
    if !v.iter().all(|v| v.is_finite()) {
        return Err(CoordinateError::NotFinite);
    }
    if order == AxisOrder::EastFirst && axes.first_is_northing() {
        v.swap(0, 1);
    }
    Ok(axes.to_internal(v))
}

/// The reverse of [`to_internal`].
fn from_internal(axes: &Axes, order: AxisOrder, v: [f64; 3]) -> [f64; 3] {
    let mut out = axes.to_crs(v);
    if order == AxisOrder::EastFirst && axes.first_is_northing() {
        out.swap(0, 1);
    }
    out
}

/// Reject latitudes beyond the poles and longitudes beyond ±10 radians, as
/// PROJ does before projecting; clamp latitudes that overshoot by rounding.
fn check_geographic([lon, lat, h]: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
    if lat.abs() > LAT_LIMIT || lon.abs() > 10.0 {
        return Err(CoordinateError::InvalidGeographic);
    }
    let lat = lat.clamp(-std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_2);
    Ok([lon, lat, h])
}

/// Geodetic to geocentric (PROJ's `cart` forward, `src/conversions/cart.cpp`).
fn cartesian([lam, phi, h]: [f64; 3], ell: &Ellipsoid) -> [f64; 3] {
    let (sinphi, cosphi) = phi.sin_cos();
    let n = normal_radius_of_curvature(ell, sinphi);
    [
        (n + h) * cosphi * lam.cos(),
        (n + h) * cosphi * lam.sin(),
        (n * (1.0 - ell.es) + h) * sinphi,
    ]
}

/// Geocentric to geodetic (PROJ's `cart` inverse: Bowring's formula with one
/// iteration, exact to well below a millimetre near the Earth's surface).
fn geodetic([x, y, z]: [f64; 3], ell: &Ellipsoid) -> [f64; 3] {
    let ra = 1.0 / ell.a;
    let (x_div_a, y_div_a, z_div_a) = (x * ra, y * ra, z * ra);
    // Distance from the Z axis.
    let p_div_a = (x_div_a * x_div_a + y_div_a * y_div_a).sqrt();
    let b_div_a = 1.0 - ell.f;
    let p_div_a_b_div_a = p_div_a * b_div_a;
    let norm = (z_div_a * z_div_a + p_div_a_b_div_a * p_div_a_b_div_a).sqrt();
    let (c, s) = if norm != 0.0 {
        (p_div_a_b_div_a / norm, z_div_a / norm)
    } else {
        (1.0, 0.0)
    };
    let y_phi = z_div_a + ell.e2s * b_div_a * s * s * s;
    let x_phi = p_div_a - ell.es * c * c * c;
    let norm_phi = (y_phi * y_phi + x_phi * x_phi).sqrt();
    let (mut cosphi, mut sinphi) = if norm_phi != 0.0 {
        (x_phi / norm_phi, y_phi / norm_phi)
    } else {
        (1.0, 0.0)
    };
    let phi = if x_phi <= 0.0 {
        // Very close to the centre there is no single answer: clamp to a pole.
        cosphi = 0.0;
        sinphi = if z >= 0.0 { 1.0 } else { -1.0 };
        std::f64::consts::FRAC_PI_2.copysign(sinphi)
    } else {
        (y_phi / x_phi).atan()
    };
    let lam = y_div_a.atan2(x_div_a);
    let h = if cosphi < 1e-6 {
        // Poleward of 89.99994°: height from Z and the geocentric radius.
        z.abs() - geocentric_radius(ell.a, b_div_a, cosphi, sinphi)
    } else {
        ell.a * p_div_a / cosphi - normal_radius_of_curvature(ell, sinphi)
    };
    [lam, phi, h]
}

fn normal_radius_of_curvature(ell: &Ellipsoid, sinphi: f64) -> f64 {
    if ell.es == 0.0 {
        ell.a
    } else {
        ell.a / (1.0 - ell.es * sinphi * sinphi).sqrt()
    }
}

fn geocentric_radius(a: f64, b_div_a: f64, cosphi: f64, sinphi: f64) -> f64 {
    let cosphi2 = cosphi * cosphi;
    let sinphi2 = sinphi * sinphi;
    let b_div_a2 = b_div_a * b_div_a;
    let t = b_div_a2 * sinphi2;
    a * ((cosphi2 + b_div_a2 * t) / (cosphi2 + t)).sqrt()
}
