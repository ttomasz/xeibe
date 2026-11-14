//! Tunisia Mining Grid (EPSG:9816), which PROJ has no mapping for: the
//! grid of Tunisia's mineral leases, linear in grads from the false origin
//! (EPSG Guidance Note 7-2). A kilometre of easting is 0.012185 grad of
//! longitude; a kilometre of northing 0.010015 grad of latitude north of the
//! false origin and 0.01002 grad south of it. The longitude is counted from
//! the base CRS's prime meridian (Paris, for Carthage (Paris)).

use std::f64::consts::PI;

use super::{BuildError, Params, Projection, adjlon, param};
use crate::error::CoordinateError;

/// Grads of longitude per kilometre of easting.
const LON_RATE: f64 = 0.012185;
/// Grads of latitude per kilometre of northing, north and south of the
/// false origin.
const LAT_RATE_NORTH: f64 = 0.010015;
const LAT_RATE_SOUTH: f64 = 0.01002;

const GRAD: f64 = PI / 200.0;

#[derive(Debug)]
pub(crate) struct TunisiaMiningGrid {
    lat0: f64,
    lon0: f64,
    fe: f64,
    fn_: f64,
}

impl TunisiaMiningGrid {
    pub(crate) fn new(params: &Params) -> Result<Self, BuildError> {
        Ok(TunisiaMiningGrid {
            lat0: params.get(param::LAT_FALSE_ORIGIN)?,
            lon0: params.get(param::LON_FALSE_ORIGIN)?,
            fe: params.get(param::EASTING_FALSE_ORIGIN)?,
            fn_: params.get(param::NORTHING_FALSE_ORIGIN)?,
        })
    }
}

impl Projection for TunisiaMiningGrid {
    fn forward(&self, lon: f64, lat: f64) -> Result<(f64, f64), CoordinateError> {
        let rate = if lat > self.lat0 {
            LAT_RATE_NORTH
        } else {
            LAT_RATE_SOUTH
        };
        Ok((
            self.fe + adjlon(lon - self.lon0) / GRAD / LON_RATE * 1000.0,
            self.fn_ + (lat - self.lat0) / GRAD / rate * 1000.0,
        ))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let rate = if y > self.fn_ {
            LAT_RATE_NORTH
        } else {
            LAT_RATE_SOUTH
        };
        Ok((
            self.lon0 + (x - self.fe) / 1000.0 * LON_RATE * GRAD,
            self.lat0 + (y - self.fn_) / 1000.0 * rate * GRAD,
        ))
    }
}
