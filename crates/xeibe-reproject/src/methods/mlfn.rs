//! Meridian arc length and its inverse, as a series in the third flattening.
//!
//! Ported from PROJ 9.8.1 `src/mlfn.cpp` (`pj_enfn`, `pj_mlfn`, `pj_inv_mlfn`;
//! MIT, see PROJ-NOTICE.md). The sixth-order series is accurate to full double
//! precision for |f| <= 1/150.

use super::auxlat::{self, AuxLat, ORDER};

/// The coefficients PROJ keeps in `en`: the rectifying radius over a, and the
/// series between geographic and rectifying latitude in both directions.
#[derive(Debug, Clone)]
pub(crate) struct Mlfn {
    radius: f64,
    to_rectifying: [f64; ORDER],
    to_geographic: [f64; ORDER],
}

impl Mlfn {
    /// From the third flattening n of the ellipsoid.
    pub(crate) fn new(n: f64) -> Self {
        Mlfn {
            radius: auxlat::rectifying_radius(n),
            to_rectifying: auxlat::coeffs(n, AuxLat::Geographic, AuxLat::Rectifying),
            to_geographic: auxlat::coeffs(n, AuxLat::Rectifying, AuxLat::Geographic),
        }
    }

    /// Distance along the meridian from the equator to latitude φ, in units
    /// of a.
    pub(crate) fn mlfn(&self, phi: f64, sphi: f64, cphi: f64) -> f64 {
        self.radius * auxlat::convert_sc(phi, sphi, cphi, &self.to_rectifying)
    }

    /// Latitude whose meridian distance from the equator is `mu`, in units
    /// of a.
    pub(crate) fn inv_mlfn(&self, mu: f64) -> f64 {
        auxlat::convert(mu / self.radius, &self.to_geographic)
    }
}
