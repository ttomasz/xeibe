//! Authalic latitude helpers for the equal-area projections.
//!
//! Ported from PROJ 9.8.1 `src/latitudes.cpp` (`pj_authalic_lat_q`,
//! `pj_authalic_lat_compute_coeffs`, `pj_authalic_lat_inverse`; MIT, see
//! PROJ-NOTICE.md).

use super::auxlat::{self, AuxLat, ORDER};
use crate::ellipsoid::Ellipsoid;

/// The series are used for |n| < 0.01; beyond that PROJ finishes with
/// Newton's method.
fn series_valid(n: f64) -> bool {
    n.abs() < 0.01
}

/// What the equal-area projections need of an ellipsoid.
#[derive(Debug, Clone)]
pub(crate) struct Authalic {
    e: f64,
    es: f64,
    one_es: f64,
    n: f64,
    /// q at the pole.
    pub(crate) qp: f64,
    xi_to_phi: [f64; ORDER],
}

impl Authalic {
    pub(crate) fn new(ellipsoid: &Ellipsoid) -> Self {
        let mut s = Authalic {
            e: ellipsoid.e,
            es: ellipsoid.es,
            one_es: 1.0 - ellipsoid.es,
            n: ellipsoid.n,
            qp: 0.0,
            xi_to_phi: auxlat::coeffs(ellipsoid.n, AuxLat::Authalic, AuxLat::Geographic),
        };
        s.qp = s.q(1.0);
        s
    }

    /// q such that the authalic latitude is asin(q / qp) (Snyder 3-12).
    pub(crate) fn q(&self, sinphi: f64) -> f64 {
        if self.e >= 1e-7 {
            let e_sinphi = self.e * sinphi;
            let d = 1.0 - e_sinphi * e_sinphi;
            if d == 0.0 {
                return f64::INFINITY;
            }
            self.one_es * (sinphi / d + e_sinphi.atanh() / self.e)
        } else {
            2.0 * sinphi
        }
    }

    /// Geographic latitude from the authalic latitude β.
    // The negated comparison is how NaN ends the iteration, as in PROJ.
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    pub(crate) fn inverse(&self, beta: f64) -> f64 {
        let mut phi = auxlat::convert(beta, &self.xi_to_phi);
        if series_valid(self.n) {
            return phi;
        }
        let q_div_one_minus_es = beta.sin() * self.qp / self.one_es;
        for _ in 0..10 {
            let (sinphi, cosphi) = phi.sin_cos();
            let d = 1.0 - self.es * (sinphi * sinphi);
            let dphi = (d * d) / (2.0 * cosphi)
                * (q_div_one_minus_es - sinphi / d - (self.e * sinphi).atanh() / self.e);
            // Written backwards so that NaN ends the loop.
            if !(dphi.abs() >= 1e-15) {
                break;
            }
            phi += dphi;
        }
        phi
    }
}
