//! Transverse Mercator variants that PROJ computes with plain `tmerc`.
//!
//! Transverse Mercator (South Orientated) (EPSG:9808) has the same formulas
//! and parameters as Transverse Mercator; its westing/southing axes are
//! applied by the caller from the CRS's axis directions. Transverse Mercator
//! 3D (1111) is `tmerc` too: PROJ passes the height through. See PROJ 9.8.1
//! `src/iso19111/operation/parammappings.cpp`.

use super::tmerc::TransverseMercator;
use super::{BuildError, Params};
use crate::ellipsoid::Ellipsoid;

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
