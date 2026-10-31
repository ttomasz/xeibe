//! Gauss Schreiber Transverse Mercator (Gauss-Laborde Réunion).
//!
//! Ported from PROJ 9.8.1 `src/projections/gstmerc.cpp` (MIT, see
//! PROJ-NOTICE.md).

use super::common::{phi2, tsfn};
use super::{BuildError, Params, Projection, adjlon, param};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

#[derive(Debug)]
pub(crate) struct GaussSchreiber {
    lon0: f64,
    fe: f64,
    fn_: f64,
    e: f64,
    c: f64,
    n1: f64,
    /// In metres.
    n2: f64,
    ys: f64,
}

impl GaussSchreiber {
    pub(crate) fn new(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        let phi0 = params.get(param::LAT_NATURAL_ORIGIN)?;
        let k0 = params.get(param::SCALE_NATURAL_ORIGIN)?;
        let (es, e) = (ellipsoid.es, ellipsoid.e);
        let n1 = (1.0 + es * phi0.cos().powi(4) / (1.0 - es)).sqrt();
        let phic = (phi0.sin() / n1).asin();
        let c = tsfn(-phic, -phi0.sin() / n1, 0.0).ln() - n1 * tsfn(-phi0, -phi0.sin(), e).ln();
        let n2 = k0 * ellipsoid.a * (1.0 - es).sqrt() / (1.0 - es * phi0.sin() * phi0.sin());
        Ok(GaussSchreiber {
            lon0: params.get(param::LON_NATURAL_ORIGIN)?,
            fe: params.get(param::FALSE_EASTING)?,
            fn_: params.get(param::FALSE_NORTHING)?,
            e,
            c,
            n1,
            n2,
            ys: -n2 * phic,
        })
    }
}

impl Projection for GaussSchreiber {
    fn forward(&self, lon: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = adjlon(lon - self.lon0);
        let l = self.n1 * lam;
        let ls = self.c + self.n1 * tsfn(-phi, -phi.sin(), self.e).ln();
        let sin_ls1 = l.sin() / ls.cosh();
        let ls1 = tsfn(-sin_ls1.asin(), -sin_ls1, 0.0).ln();
        Ok((
            self.n2 * ls1 + self.fe,
            self.ys + self.n2 * (ls.sinh() / l.cos()).atan() + self.fn_,
        ))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let (x, y) = (x - self.fe, y - self.fn_ - self.ys);
        let l = ((x / self.n2).sinh() / (y / self.n2).cos()).atan();
        let sin_c = (y / self.n2).sin() / (x / self.n2).cosh();
        let lc = tsfn(-sin_c.asin(), -sin_c, 0.0).ln();
        let phi = -phi2(((lc - self.c) / self.n1).exp(), self.e)?;
        Ok((l / self.n1 + self.lon0, phi))
    }
}
