//! Helpers shared by the conformal projections.
//!
//! Ported from PROJ 9.8.1 `src/tsfn.cpp`, `src/msfn.cpp` and `src/phi2.cpp`
//! (MIT, see PROJ-NOTICE.md).

use crate::error::CoordinateError;

/// cos φ / √(1 − e² sin² φ): the parallel's radius over a (PROJ's `pj_msfn`).
pub(crate) fn msfn(sinphi: f64, cosphi: f64, es: f64) -> f64 {
    cosphi / (1.0 - es * sinphi * sinphi).sqrt()
}

/// exp(−ψ), ψ the isometric latitude (PROJ's `pj_tsfn`; Snyder's t).
pub(crate) fn tsfn(phi: f64, sinphi: f64, e: f64) -> f64 {
    let cosphi = phi.cos();
    // exp(-asinh(tan φ)) = 1 / (tan φ + sec φ), written two ways for accuracy.
    (e * (e * sinphi).atanh()).exp()
        * if sinphi > 0.0 {
            cosphi / (1.0 + sinphi)
        } else {
            (1.0 - sinphi) / cosphi
        }
}

/// tan χ = sinh ψ to tan φ by Newton's method (PROJ's `pj_sinhpsi2tanphi`,
/// from GeographicLib's `Math::tauf`).
// The negated comparisons are how NaN ends the iteration, as in PROJ.
#[allow(clippy::neg_cmp_op_on_partial_ord)]
pub(crate) fn sinhpsi2tanphi(taup: f64, e: f64) -> Result<f64, CoordinateError> {
    const NUMIT: usize = 5;
    let rooteps = f64::EPSILON.sqrt();
    let tol = rooteps / 10.0;
    let tmax = 2.0 / rooteps;
    let e2m = 1.0 - e * e;
    let stol = tol * taup.abs().max(1.0);
    // 70 corresponds to χ = 89.18°.
    let mut tau = if taup.abs() > 70.0 {
        taup * (e * e.atanh()).exp()
    } else {
        taup / e2m
    };
    // Handles ±inf and NaN.
    if !(tau.abs() < tmax) {
        return Ok(tau);
    }
    for _ in 0..NUMIT {
        let tau1 = (1.0 + tau * tau).sqrt();
        let sig = (e * (e * tau / tau1).atanh()).sinh();
        let taupa = (1.0 + sig * sig).sqrt() * tau - sig * tau1;
        let dtau = (taup - taupa) * (1.0 + e2m * (tau * tau))
            / (e2m * tau1 * (1.0 + taupa * taupa).sqrt());
        tau += dtau;
        // Written backwards so that NaN ends the loop.
        if !(dtau.abs() >= stol) {
            return Ok(tau);
        }
    }
    Err(CoordinateError::NoConvergence)
}

/// Latitude from t = exp(−ψ) (PROJ's `pj_phi2`).
pub(crate) fn phi2(ts: f64, e: f64) -> Result<f64, CoordinateError> {
    Ok(sinhpsi2tanphi((1.0 / ts - ts) / 2.0, e)?.atan())
}
