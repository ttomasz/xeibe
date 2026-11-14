//! Lambert Conic Conformal variants that PROJ computes with `lcc`: 2SP
//! Michigan (1051) and 1SP variant B (1102). 2SP Belgium (EPSG:9803) PROJ
//! maps to `lcc` too, leaving out EPSG's turn of the grid by α = 29.2985″
//! (θ − α for θ, EPSG Guidance Note 7-2), which moves BD72 / Belge Lambert
//! 72 by 745 m; here it is kept. And one PROJ has no mapping
//! for: West Orientated (9826), the 1SP formulas with W = FE − r sin θ
//! (EPSG Guidance Note 7-2).
//!
//! Parameter mapping from PROJ 9.8.1
//! `src/iso19111/operation/parammappings.cpp` (MIT, see PROJ-NOTICE.md).

use std::f64::consts::FRAC_PI_2;

use super::lcc::LambertConic;
use super::{BuildError, Params, param};
use crate::ellipsoid::Ellipsoid;

/// 2SP Belgium: the regular 2SP formulas with the grid turned by
/// α = 29.2985″ about the cone's apex.
pub(crate) fn belgium(params: &Params, ellipsoid: &Ellipsoid) -> Result<LambertConic, BuildError> {
    const ALPHA: f64 = 29.2985 / 3600.0 * std::f64::consts::PI / 180.0;
    Ok(LambertConic::two_sp(params, ellipsoid)?.turned(ALPHA))
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

/// West Orientated: Greenland's and Denmark's older grids, with axes
/// positive north and west. EPSG's westing is W = FE − r sin θ. The caller
/// turns a west axis round from the projection's easting, so this is the
/// 1SP easting r sin θ − FE: 1SP with the false easting negated.
pub(crate) fn west_orientated(
    params: &Params,
    ellipsoid: &Ellipsoid,
) -> Result<LambertConic, BuildError> {
    let phi0 = params.get(param::LAT_NATURAL_ORIGIN)?;
    LambertConic::new(
        ellipsoid,
        phi0,
        phi0,
        phi0,
        params.get(param::LON_NATURAL_ORIGIN)?,
        params.get(param::SCALE_NATURAL_ORIGIN)?,
        -params.get(param::FALSE_EASTING)?,
        params.get(param::FALSE_NORTHING)?,
    )
}
