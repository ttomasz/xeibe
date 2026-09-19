//! Arc computations for parameter-defined curves (see `docs/geometry.md`,
//! "Arcs given by parameters"). All results are in output axis order.

/// Midpoint of the arc from `p3` back to `p1` on the circle through
/// `p1, p2, p3` (5th control point of a `Circle`). `None` if collinear.
pub fn circle_closing_midpoint(p1: [f64; 2], p2: [f64; 2], p3: [f64; 2]) -> Option<[f64; 2]> {
    todo!()
}

/// `ArcByCenterPoint` in a projected CRS, GDAL convention: angles in degrees,
/// counter-clockwise from +x, arc from start to end through the mean angle.
/// Returns the three CircularString control points.
pub fn arc_by_center_point(
    center: [f64; 2],
    radius: f64,
    start_deg: f64,
    end_deg: f64,
) -> [[f64; 2]; 3] {
    todo!()
}

/// `CircleByCenterPoint` in a projected CRS: W, N, E, S, W (as GDAL).
pub fn circle_by_center_point(center: [f64; 2], radius: f64) -> [[f64; 2]; 5] {
    todo!()
}

/// `ArcByBulge` middle control point, GDAL formula (unverified against
/// ISO 19107 §6.4.17): `mid + n̂ · bulge · normal`.
pub fn arc_by_bulge_midpoint(p0: [f64; 2], p1: [f64; 2], bulge: f64, normal: f64) -> [f64; 2] {
    todo!()
}

/// Geodesic linearization for `ArcByCenterPoint`/`CircleByCenterPoint` in a
/// geographic CRS (AIXM bearings, clockwise from north). Lossy.
pub fn geodesic_arc(
    center_lon_lat: [f64; 2],
    radius_m: f64,
    start_bearing_deg: f64,
    end_bearing_deg: f64,
    step_deg: f64,
    semi_major_m: f64,
) -> Vec<[f64; 2]> {
    todo!()
}

/// Convert a length with a `uom` (e.g. `m`, `km`, `[nmi_i]`, `ft`) to metres.
pub fn uom_to_metres(uom: Option<&str>) -> Option<f64> {
    todo!()
}

/// Convert an angle with a `uom` (`deg` default, `rad`) to degrees.
pub fn angle_to_degrees(value: f64, uom: Option<&str>) -> Option<f64> {
    todo!()
}
