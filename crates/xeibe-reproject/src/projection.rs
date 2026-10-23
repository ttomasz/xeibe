//! A CRS's own projection, as pyproj's legacy `Proj` applies it
//! (`docs/pyproj-phase2.md`, `Proj`): the map projection on the CRS's
//! ellipsoid, with no datum change.
//!
//! pyproj builds `Proj(crs)` from the CRS's PROJ string, which says only the
//! projection, the ellipsoid, the prime meridian and the units. So the
//! input is longitude from Greenwich and latitude in degrees, whatever the
//! CRS's base uses, and the output is the projection's easting and northing
//! in the CRS's unit, whatever its axes say. PROJ's string carries an axis
//! order only for three methods, matching their CRSs' axes: Transverse
//! Mercator (South Orientated) (`+axis=wsu`), Krovak and Krovak Modified
//! (`+axis=swu` or `+czech`); for them the output is in the CRS's axes.
//! A geographic CRS's projection is the shift to its prime meridian, in
//! degrees; a geocentric one's gives X and Y (and Z from a height).

use std::f64::consts::PI;

use crate::crs::{Axes, Crs, Kind};
use crate::error::{CoordinateError, Error};
use crate::methods::adjlon;
use crate::transform::{
    AxisOrder, coordinate_error, from_geographic, from_internal, to_geographic, to_internal,
};

/// Degrees to radians and back, by multiplying, as pyproj converts.
const DG2RAD: f64 = PI / 180.0;
const RAD2DG: f64 = 180.0 / PI;

/// The projection of a CRS, forwards (degrees to the CRS) or backwards.
#[derive(Debug, Clone)]
pub struct Projection {
    crs: Crs,
    /// The output axes: the CRS's, or with `in_metres` the same in metres.
    axes: Axes,
    /// The output is in the CRS's axes (directions and order), not the
    /// projection's easting and northing.
    oriented: bool,
    inverse: bool,
}

/// The methods whose PROJ string has an axis order: Transverse Mercator
/// (South Orientated), Krovak, Krovak Modified.
const ORIENTED: [u32; 3] = [9808, 9819, 1042];

impl Projection {
    /// The projection of `crs`, or of its horizontal part for a compound
    /// CRS. A vertical or engineering CRS has none.
    pub fn new(crs: &Crs) -> Result<Projection, Error> {
        let crs = crs.horizontal().without_bound();
        match crs.kind {
            Kind::Vertical { .. } => Err(Error::Unsupported {
                crs: crs.label(),
                what: "a projection of a vertical CRS".into(),
            }),
            Kind::Engineering { .. } => Err(Error::Unsupported {
                crs: crs.label(),
                what: "a projection of an engineering CRS".into(),
            }),
            _ => Ok(Projection {
                axes: crs.axes.clone(),
                oriented: match &crs.kind {
                    Kind::Projected { method_code, .. } => {
                        method_code.is_some_and(|m| ORIENTED.contains(&m))
                    }
                    _ => true,
                },
                crs,
                inverse: false,
            }),
        }
    }

    /// The same with lengths in metres instead of the CRS's unit (pyproj's
    /// `preserve_units=False`), in the same directions.
    pub fn in_metres(&self) -> Projection {
        let mut p = self.clone();
        if !matches!(p.crs.kind, Kind::Geographic { .. }) {
            p.axes = p.axes.in_metres();
        }
        p
    }

    /// The projection the other way.
    pub fn inverse(&self) -> Projection {
        Projection {
            inverse: !self.inverse,
            ..self.clone()
        }
    }

    /// Whether this runs backwards, from the CRS to degrees.
    pub fn is_inverse(&self) -> bool {
        self.inverse
    }

    /// The CRS whose projection this is.
    pub fn crs(&self) -> &Crs {
        &self.crs
    }

    /// Whether the CRS's side is angular (a geographic CRS), so its values
    /// are degrees too.
    pub fn crs_is_angular(&self) -> bool {
        matches!(self.crs.kind, Kind::Geographic { .. })
    }

    fn meridian(&self) -> f64 {
        self.crs.datum().map_or(0.0, |d| d.prime_meridian)
    }

    /// One point, with an optional third value (a height in, Z out, for a
    /// geocentric CRS; passed through otherwise).
    pub fn convert(
        &self,
        x: f64,
        y: f64,
        z: Option<f64>,
    ) -> Result<(f64, f64, Option<f64>), CoordinateError> {
        if !(x.is_finite() && y.is_finite()) {
            return Err(CoordinateError::NotFinite);
        }
        let h = z.unwrap_or(0.0);
        if self.inverse {
            let (lon, lat, h) = self.backward(x, y, h)?;
            Ok((lon, lat, z.map(|_| h)))
        } else {
            let (x, y, h) = self.forward(x, y, h)?;
            Ok((x, y, z.map(|_| h)))
        }
    }

    /// Longitude from Greenwich and latitude, in degrees, to the CRS, in
    /// PROJ's order (`fwd_prepare`): the input checked, the longitude
    /// reduced, then counted from the prime meridian.
    fn forward(&self, lon: f64, lat: f64, h: f64) -> Result<(f64, f64, f64), CoordinateError> {
        let (lam, phi) = (lon * DG2RAD, lat * DG2RAD);
        if phi.abs() - PI / 2.0 > 1e-12 || lam.abs() > 10.0 {
            return Err(CoordinateError::InvalidGeographic);
        }
        let phi = phi.clamp(-PI / 2.0, PI / 2.0);
        let lam = adjlon(lam) - self.meridian();
        if let Kind::Geographic { .. } = self.crs.kind {
            return Ok((adjlon(lam) * RAD2DG, phi * RAD2DG, h));
        }
        let internal = from_geographic(&self.crs, [lam, phi, h])?;
        if self.oriented {
            let [x, y, z] = from_internal(&self.axes, AxisOrder::Authority, internal);
            return Ok((x, y, z));
        }
        let unit = self.axes.linear_factor();
        Ok((internal[0] / unit, internal[1] / unit, internal[2]))
    }

    /// The CRS to longitude from Greenwich and latitude, in degrees.
    fn backward(&self, x: f64, y: f64, h: f64) -> Result<(f64, f64, f64), CoordinateError> {
        let [lam, phi, h] = match self.crs.kind {
            Kind::Geographic { .. } => [x * DG2RAD, y * DG2RAD, h],
            _ if self.oriented => {
                let internal = to_internal(&self.axes, AxisOrder::Authority, [x, y, h])?;
                to_geographic(&self.crs, internal)?
            }
            _ => {
                let unit = self.axes.linear_factor();
                to_geographic(&self.crs, [x * unit, y * unit, h])?
            }
        };
        Ok((adjlon(lam + self.meridian()) * RAD2DG, phi * RAD2DG, h))
    }

    /// Convert in place; stops at the first point that fails, naming it.
    pub fn convert_xy(&self, x: &mut [f64], y: &mut [f64]) -> Result<(), Error> {
        for (i, (x, y)) in x.iter_mut().zip(y.iter_mut()).enumerate() {
            let (cx, cy, _) = self
                .convert(*x, *y, None)
                .map_err(|reason| coordinate_error(i, *x, *y, reason))?;
            (*x, *y) = (cx, cy);
        }
        Ok(())
    }

    /// As [`Projection::convert_xy`], with third values.
    pub fn convert_xyz(&self, x: &mut [f64], y: &mut [f64], z: &mut [f64]) -> Result<(), Error> {
        for (i, ((x, y), z)) in x.iter_mut().zip(y.iter_mut()).zip(z.iter_mut()).enumerate() {
            let (cx, cy, cz) = self
                .convert(*x, *y, Some(*z))
                .map_err(|reason| coordinate_error(i, *x, *y, reason))?;
            (*x, *y, *z) = (cx, cy, cz.unwrap_or(*z));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utm_and_back() {
        let p = Projection::new(&Crs::from_epsg(32633).unwrap()).unwrap();
        let (x, y, _) = p.convert(15.0, 52.0, None).unwrap();
        assert!((x - 500_000.0).abs() < 1e-6, "{x}");
        let (lon, lat, _) = p.inverse().convert(x, y, None).unwrap();
        assert!((lon - 15.0).abs() < 1e-12 && (lat - 52.0).abs() < 1e-12);
    }

    #[test]
    fn paris_meridian() {
        // NTF (Paris) in grads: Proj takes and gives degrees from Greenwich.
        // EPSG's Paris is 2.5969213 grad = 2.33722917°; pyproj, through
        // PROJ's "+pm=paris" (2°20'14.025"), gives 0.16277083333333325.
        let p = Projection::new(&Crs::from_epsg(4807).unwrap()).unwrap();
        let (lon, lat, _) = p.convert(2.5, 46.5, None).unwrap();
        assert!((lon - (2.5 - 2.33722917)).abs() < 1e-12, "{lon}");
        assert!((lat - 46.5).abs() < 1e-12, "{lat}");
    }

    #[test]
    fn polar_and_south_orientated_axes() {
        // UPS North (N,E): pyproj's Proj gives easting, northing.
        let p = Projection::new(&Crs::from_epsg(32661).unwrap()).unwrap();
        let (x, y, _) = p.convert(10.0, 80.0, None).unwrap();
        assert!((x - 2193261.9367645574).abs() < 1e-6 && (y - 903957.0916030575).abs() < 1e-6);
        // Lo29: westing, southing, as +axis=wsu.
        let p = Projection::new(&Crs::from_epsg(2053).unwrap()).unwrap();
        let (x, y, _) = p.convert(29.0, -26.0, None).unwrap();
        assert!(
            x.abs() < 1e-6 && (y - 2876834.572603928).abs() < 1e-6,
            "{x} {y}"
        );
    }

    #[test]
    fn vertical_has_none() {
        assert!(Projection::new(&Crs::from_epsg(5773).unwrap()).is_err());
    }
}
