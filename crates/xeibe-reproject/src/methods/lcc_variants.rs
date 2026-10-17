//! Lambert Conic Conformal variants that PROJ computes with `lcc`: 2SP
//! Belgium (EPSG:9803, PROJ ignores the Belgium-specific correction), 2SP
//! Michigan (1051) and 1SP variant B (1102).
//!
//! Parameter mapping from PROJ 9.8.1
//! `src/iso19111/operation/parammappings.cpp` (MIT, see PROJ-NOTICE.md).

use std::f64::consts::FRAC_PI_2;

use super::lcc::LambertConic;
use super::{BuildError, Params, param};
use crate::ellipsoid::Ellipsoid;

/// 2SP Belgium: the regular 2SP formulas.
pub(crate) fn belgium(params: &Params, ellipsoid: &Ellipsoid) -> Result<LambertConic, BuildError> {
    LambertConic::two_sp(params, ellipsoid)
}

/// 2SP Michigan: 2SP with an ellipsoid scale factor (parameter 1038) as k0.
pub(crate) fn michigan(params: &Params, ellipsoid: &Ellipsoid) -> Result<LambertConic, BuildError> {
    LambertConic::new(
        ellipsoid,
        params.get(param::LAT_FALSE_ORIGIN)?,
        params.get(param::LAT_1ST_PARALLEL)?,
        params.get(param::LAT_2ND_PARALLEL)?,
        params.get(param::LON_FALSE_ORIGIN)?,
        params.get(param::ELLIPSOID_SCALE_FACTOR)?,
        params.get(param::EASTING_FALSE_ORIGIN)?,
        params.get(param::NORTHING_FALSE_ORIGIN)?,
    )
}

/// 1SP variant B: the latitude of natural origin is the single standard
/// parallel; the origin of the grid is the false origin.
pub(crate) fn one_sp_variant_b(
    params: &Params,
    ellipsoid: &Ellipsoid,
) -> Result<LambertConic, BuildError> {
    let phi1 = params.get(param::LAT_NATURAL_ORIGIN)?;
    if phi1.abs() >= FRAC_PI_2 {
        return Err(BuildError::Invalid(
            "the standard parallel must be within ±90°".to_owned(),
        ));
    }
    LambertConic::new(
        ellipsoid,
        params.get(param::LAT_FALSE_ORIGIN)?,
        phi1,
        phi1,
        params.get(param::LON_FALSE_ORIGIN)?,
        params.get(param::SCALE_NATURAL_ORIGIN)?,
        params.get(param::EASTING_FALSE_ORIGIN)?,
        params.get(param::NORTHING_FALSE_ORIGIN)?,
    )
}
