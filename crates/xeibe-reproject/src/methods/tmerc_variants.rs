//! Transverse Mercator variants that PROJ computes with plain `tmerc`.
//!
//! Transverse Mercator (South Orientated) (EPSG:9808) has the same formulas
//! and parameters as Transverse Mercator; its westing/southing axes are
//! applied by the caller from the CRS's axis directions. Transverse Mercator
//! 3D (1111) is `tmerc` too: PROJ passes the height through. See PROJ 9.8.1
//! `src/iso19111/operation/parammappings.cpp`.
//!
//! Transverse Mercator Zoned Grid System (9824), which PROJ doesn't map,
//! is a set of zones in one CRS ("WGS 84 / UTM grid system"): each point
//! takes the zone its longitude falls in, and the zone number, times a
//! million, leads its easting (EPSG Guidance Note 7-2).

use super::tmerc::TransverseMercator;
use super::{BuildError, Params, Projection, adjlon, param};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

pub(crate) fn south_orientated(
    params: &Params,
    ellipsoid: &Ellipsoid,
) -> Result<TransverseMercator, BuildError> {
    TransverseMercator::new(params, ellipsoid)
}

pub(crate) fn three_d(
    params: &Params,
    ellipsoid: &Ellipsoid,
) -> Result<TransverseMercator, BuildError> {
    TransverseMercator::new(params, ellipsoid)
}

/// Transverse Mercator Zoned Grid System: zone Z of width W from the
/// initial longitude λI, central meridian λI + Z·W − W/2, and
/// E = Z·10⁶ + FE + x.
#[derive(Debug)]
pub(crate) struct ZonedGrid {
    /// Transverse Mercator on a central meridian of 0 and no false easting.
    tm: TransverseMercator,
    initial: f64,
    width: f64,
    zones: f64,
    fe: f64,
}

impl ZonedGrid {
    pub(crate) fn new(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        let width = params.get(param::ZONE_WIDTH)?;
        let zones = (std::f64::consts::TAU / width).round();
        if width.is_nan() || width <= 0.0 || (zones * width - std::f64::consts::TAU).abs() > 1e-9 {
            return Err(BuildError::Invalid(
                "the zone width must divide 360°".to_owned(),
            ));
        }
        let mut tm = Params::default();
        tm.push(
            param::LAT_NATURAL_ORIGIN,
            params.get(param::LAT_NATURAL_ORIGIN)?,
        );
        tm.push(param::LON_NATURAL_ORIGIN, 0.0);
        tm.push(
            param::SCALE_NATURAL_ORIGIN,
            params.get(param::SCALE_NATURAL_ORIGIN)?,
        );
        tm.push(param::FALSE_EASTING, 0.0);
        tm.push(param::FALSE_NORTHING, params.get(param::FALSE_NORTHING)?);
        Ok(ZonedGrid {
            tm: TransverseMercator::new(&tm, ellipsoid)?,
            initial: params.get(param::INITIAL_LONGITUDE)?,
            width,
            zones,
            fe: params.get(param::FALSE_EASTING)?,
        })
    }

    /// Zone `z`'s central meridian.
    fn central_meridian(&self, z: f64) -> f64 {
        self.initial + z * self.width - self.width / 2.0
    }
}

impl Projection for ZonedGrid {
    fn forward(&self, lon: f64, lat: f64) -> Result<(f64, f64), CoordinateError> {
        let lon = adjlon(lon);
        // EPSG's int and mod, with mod's floor towards negative infinity. A
        // longitude on a zone's western limit is in it: in radians, 0° from
        // −180° in 6° zones comes to a hair under 30.
        let k = ((lon - self.initial) / self.width + 1e-9).floor();
        let z = k - (k / self.zones).floor() * self.zones + 1.0;
        let (x, y) = self
            .tm
            .forward(adjlon(lon - self.central_meridian(z)), lat)?;
        Ok((z * 1e6 + self.fe + x, y))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let z = ((x - self.fe) / 1e6).round();
        if !(1.0..=self.zones).contains(&z) {
            return Err(CoordinateError::OutsideDomain);
        }
        let (lon, lat) = self.tm.inverse(x - z * 1e6 - self.fe, y)?;
        Ok((lon + self.central_meridian(z), lat))
    }
}
