//! Geographic ↔ geocentric on one ellipsoid.
//!
//! Ported from PROJ 9.8.1 `src/conversions/cart.cpp` (MIT, see PROJ-NOTICE.md).

use crate::ellipsoid::Ellipsoid;

/// Geodetic to geocentric (PROJ's `cart` forward, `src/conversions/cart.cpp`).
pub(crate) fn cartesian([lam, phi, h]: [f64; 3], ell: &Ellipsoid) -> [f64; 3] {
    let (sinphi, cosphi) = phi.sin_cos();
    let n = normal_radius_of_curvature(ell, sinphi);
    [
        (n + h) * cosphi * lam.cos(),
        (n + h) * cosphi * lam.sin(),
        (n * (1.0 - ell.es) + h) * sinphi,
    ]
}

/// Geocentric to geodetic (PROJ's `cart` inverse: Bowring's formula with one
/// iteration, exact to well below a millimetre near the Earth's surface).
pub(crate) fn geodetic([x, y, z]: [f64; 3], ell: &Ellipsoid) -> [f64; 3] {
    let ra = 1.0 / ell.a;
    let (x_div_a, y_div_a, z_div_a) = (x * ra, y * ra, z * ra);
    // Distance from the Z axis.
    let p_div_a = (x_div_a * x_div_a + y_div_a * y_div_a).sqrt();
    let b_div_a = 1.0 - ell.f;
    let p_div_a_b_div_a = p_div_a * b_div_a;
    let norm = (z_div_a * z_div_a + p_div_a_b_div_a * p_div_a_b_div_a).sqrt();
    let (c, s) = if norm != 0.0 {
        (p_div_a_b_div_a / norm, z_div_a / norm)
    } else {
        (1.0, 0.0)
    };
    let y_phi = z_div_a + ell.e2s * b_div_a * s * s * s;
    let x_phi = p_div_a - ell.es * c * c * c;
    let norm_phi = (y_phi * y_phi + x_phi * x_phi).sqrt();
    let (mut cosphi, mut sinphi) = if norm_phi != 0.0 {
        (x_phi / norm_phi, y_phi / norm_phi)
    } else {
        (1.0, 0.0)
    };
    let phi = if x_phi <= 0.0 {
        // Very close to the centre there is no single answer: clamp to a pole.
        cosphi = 0.0;
        sinphi = if z >= 0.0 { 1.0 } else { -1.0 };
        std::f64::consts::FRAC_PI_2.copysign(sinphi)
    } else {
        (y_phi / x_phi).atan()
    };
    let lam = y_div_a.atan2(x_div_a);
    let h = if cosphi < 1e-6 {
        // Poleward of 89.99994°: height from Z and the geocentric radius.
        z.abs() - geocentric_radius(ell.a, b_div_a, cosphi, sinphi)
    } else {
        ell.a * p_div_a / cosphi - normal_radius_of_curvature(ell, sinphi)
    };
    [lam, phi, h]
}

fn normal_radius_of_curvature(ell: &Ellipsoid, sinphi: f64) -> f64 {
    if ell.es == 0.0 {
        ell.a
    } else {
        ell.a / (1.0 - ell.es * sinphi * sinphi).sqrt()
    }
}

fn geocentric_radius(a: f64, b_div_a: f64, cosphi: f64, sinphi: f64) -> f64 {
    let cosphi2 = cosphi * cosphi;
    let sinphi2 = sinphi * sinphi;
    let b_div_a2 = b_div_a * b_div_a;
    let t = b_div_a2 * sinphi2;
    a * ((cosphi2 + b_div_a2 * t) / (cosphi2 + t)).sqrt()
}
