//! Arc computations for parameter-defined curves (see `docs/geometry.md`,
//! "Arcs given by parameters"). All results are in output axis order.

use std::f64::consts::{PI, TAU};

/// Midpoint of the arc from `p3` back to `p1` on the circle through
/// `p1, p2, p3` (5th control point of a `Circle`). `None` if collinear.
pub fn circle_closing_midpoint(p1: [f64; 2], p2: [f64; 2], p3: [f64; 2]) -> Option<[f64; 2]> {
    let circle = Circle::through(p1, p2, p3)?;
    let a1 = circle.angle_of(p1);
    let a3 = circle.angle_of(p3);
    // The arc keeps turning the way p1 → p2 → p3 turns until it is back at p1.
    let mid = if circle.ccw {
        a3 + (a1 - a3).rem_euclid(TAU) / 2.0
    } else {
        a3 - (a3 - a1).rem_euclid(TAU) / 2.0
    };
    Some(circle.point_at(mid))
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
    let mid_deg = (start_deg + end_deg) / 2.0;
    [
        point_at_deg(center, radius, start_deg),
        point_at_deg(center, radius, mid_deg),
        point_at_deg(center, radius, end_deg),
    ]
}

/// `CircleByCenterPoint` in a projected CRS: W, N, E, S, W (as GDAL).
pub fn circle_by_center_point(center: [f64; 2], radius: f64) -> [[f64; 2]; 5] {
    [180.0, 90.0, 0.0, 270.0, 180.0].map(|deg| point_at_deg(center, radius, deg))
}

/// `ArcByBulge` middle control point, GDAL formula (unverified against
/// ISO 19107 §6.4.17): `mid + n̂ · bulge · normal`.
pub fn arc_by_bulge_midpoint(p0: [f64; 2], p1: [f64; 2], bulge: f64, normal: f64) -> [f64; 2] {
    let mid = [(p0[0] + p1[0]) / 2.0, (p0[1] + p1[1]) / 2.0];
    let (dx, dy) = (p1[0] - p0[0], p1[1] - p0[1]);
    let length = dx.hypot(dy);
    if length == 0.0 {
        return mid;
    }
    // The chord direction rotated 90° counter-clockwise, as a unit vector.
    let (nx, ny) = (-dy / length, dx / length);
    [mid[0] + nx * bulge * normal, mid[1] + ny * bulge * normal]
}

/// Geodesic linearization for `ArcByCenterPoint`/`CircleByCenterPoint` in a
/// geographic CRS (AIXM bearings, clockwise from north). Lossy.
///
/// The arc runs clockwise from the start bearing to the end bearing; equal
/// bearings give the full circle. Distances are measured on a sphere of radius
/// `semi_major_m`, as GDAL does. Returns `[lon, lat]` vertices.
pub fn geodesic_arc(
    center_lon_lat: [f64; 2],
    radius_m: f64,
    start_bearing_deg: f64,
    end_bearing_deg: f64,
    step_deg: f64,
    semi_major_m: f64,
) -> Vec<[f64; 2]> {
    let mut sweep = (end_bearing_deg - start_bearing_deg).rem_euclid(360.0);
    if sweep == 0.0 {
        sweep = 360.0;
    }
    let step = if step_deg > 0.0 { step_deg } else { 4.0 };
    let segments = (sweep / step).ceil().max(1.0) as usize;
    let distance = radius_m / semi_major_m;
    let lat1 = center_lon_lat[1].to_radians();
    let lon1 = center_lon_lat[0].to_radians();
    (0..=segments)
        .map(|i| {
            let bearing = (start_bearing_deg + sweep * i as f64 / segments as f64).to_radians();
            // Spherical direct problem.
            let lat2 =
                (lat1.sin() * distance.cos() + lat1.cos() * distance.sin() * bearing.cos()).asin();
            let lon2 = lon1
                + (bearing.sin() * distance.sin() * lat1.cos())
                    .atan2(distance.cos() - lat1.sin() * lat2.sin());
            let lon = (lon2.to_degrees() + 180.0).rem_euclid(360.0) - 180.0;
            [lon, lat2.to_degrees()]
        })
        .collect()
}

/// Convert a length with a `uom` (e.g. `m`, `km`, `[nmi_i]`, `ft`) to metres.
///
/// Returns the size of one unit in metres, or `None` if the unit is missing or
/// not a known length unit (the value is then taken to be in CRS units).
pub fn uom_to_metres(uom: Option<&str>) -> Option<f64> {
    let uom = uom?.trim();
    let factor = match uom {
        "m" | "metre" | "meter" | "metres" | "meters" | "urn:ogc:def:uom:EPSG::9001" => 1.0,
        "km" | "kilometre" | "kilometer" | "urn:ogc:def:uom:EPSG::9036" => 1000.0,
        "[nmi_i]" | "nmi" | "NM" | "urn:ogc:def:uom:EPSG::9030" => 1852.0,
        "[mi_i]" | "mi" | "urn:ogc:def:uom:EPSG::9093" => 1609.344,
        "[ft_i]" | "ft" | "urn:ogc:def:uom:EPSG::9002" => 0.3048,
        "[ft_us]" | "urn:ogc:def:uom:EPSG::9003" => 1200.0 / 3937.0,
        _ => return None,
    };
    Some(factor)
}

/// Convert an angle with a `uom` (`deg` default, `rad`) to degrees.
pub fn angle_to_degrees(value: f64, uom: Option<&str>) -> Option<f64> {
    match uom.map(str::trim) {
        None
        | Some(
            "deg" | "degree" | "degrees" | "°" | "urn:ogc:def:uom:EPSG::9102"
            | "urn:ogc:def:uom:EPSG::9122",
        ) => Some(value),
        Some("rad" | "radian" | "radians" | "urn:ogc:def:uom:EPSG::9101") => {
            Some(value.to_degrees())
        }
        Some("grad" | "gon" | "urn:ogc:def:uom:EPSG::9105" | "urn:ogc:def:uom:EPSG::9106") => {
            Some(value * 0.9)
        }
        Some(_) => None,
    }
}

/// A point on the circle at `deg` degrees counter-clockwise from +x. Multiples
/// of 90° are exact, so axis-aligned points carry no rounding noise.
fn point_at_deg(center: [f64; 2], radius: f64, deg: f64) -> [f64; 2] {
    let (cos, sin) = cos_sin_deg(deg);
    [center[0] + radius * cos, center[1] + radius * sin]
}

fn cos_sin_deg(deg: f64) -> (f64, f64) {
    let normalized = deg.rem_euclid(360.0);
    if normalized == 0.0 {
        (1.0, 0.0)
    } else if normalized == 90.0 {
        (0.0, 1.0)
    } else if normalized == 180.0 {
        (-1.0, 0.0)
    } else if normalized == 270.0 {
        (0.0, -1.0)
    } else {
        let radians = normalized.to_radians();
        (radians.cos(), radians.sin())
    }
}

/// The circle through three control points of a circular arc, and the turning
/// direction of `p1 → p2 → p3`. Shared by the `Circle` rule, linearization and
/// arc bounding boxes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Circle {
    pub center: [f64; 2],
    pub radius: f64,
    /// `true` if the arc through the three points turns counter-clockwise.
    pub ccw: bool,
}

impl Circle {
    /// `None` if the points are collinear (or two of them coincide), except
    /// for `p1 == p3 != p2`, which is the full circle with `p1`–`p2` as its
    /// diameter (ISO WKB's closed three-point circle), taken as counter-clockwise.
    pub fn through(p1: [f64; 2], p2: [f64; 2], p3: [f64; 2]) -> Option<Circle> {
        if p1 == p3 {
            if p1 == p2 {
                return None;
            }
            let center = [(p1[0] + p2[0]) / 2.0, (p1[1] + p2[1]) / 2.0];
            let radius = (p1[0] - center[0]).hypot(p1[1] - center[1]);
            return Some(Circle { center, radius, ccw: true });
        }
        // Work relative to p1 for precision with large projected coordinates.
        let (bx, by) = (p2[0] - p1[0], p2[1] - p1[1]);
        let (cx, cy) = (p3[0] - p1[0], p3[1] - p1[1]);
        let d = 2.0 * (bx * cy - by * cx);
        let scale = (bx * bx + by * by).max(cx * cx + cy * cy);
        if d == 0.0 || d.abs() <= 1e-12 * scale || !d.is_finite() {
            return None;
        }
        let b2 = bx * bx + by * by;
        let c2 = cx * cx + cy * cy;
        let ux = (cy * b2 - by * c2) / d;
        let uy = (bx * c2 - cx * b2) / d;
        Some(Circle {
            center: [p1[0] + ux, p1[1] + uy],
            radius: ux.hypot(uy),
            ccw: d > 0.0,
        })
    }

    pub fn angle_of(&self, p: [f64; 2]) -> f64 {
        (p[1] - self.center[1]).atan2(p[0] - self.center[0])
    }

    pub fn point_at(&self, angle: f64) -> [f64; 2] {
        [
            self.center[0] + self.radius * angle.cos(),
            self.center[1] + self.radius * angle.sin(),
        ]
    }

    /// Signed sweep from angle `from` to angle `to` in this circle's turning
    /// direction: positive (0, 2π] counter-clockwise, negative clockwise.
    /// Equal angles give a full turn.
    pub fn sweep(&self, from: f64, to: f64) -> f64 {
        if self.ccw {
            let sweep = (to - from).rem_euclid(TAU);
            if sweep == 0.0 { TAU } else { sweep }
        } else {
            let sweep = (from - to).rem_euclid(TAU);
            -(if sweep == 0.0 { TAU } else { sweep })
        }
    }
}

/// Extend `bbox` (`[min_x, min_y, max_x, max_y]`) by the arc through `p1, p2,
/// p3`, including the points where it crosses the circle's axis extremes.
pub(crate) fn extend_bbox_with_arc(bbox: &mut [f64; 4], p1: [f64; 2], p2: [f64; 2], p3: [f64; 2]) {
    for p in [p1, p2, p3] {
        extend_bbox(bbox, p);
    }
    let Some(circle) = Circle::through(p1, p2, p3) else {
        return;
    };
    let start = circle.angle_of(p1);
    let total = if p1 == p3 {
        TAU
    } else {
        circle.sweep(start, circle.angle_of(p2)) + circle.sweep(circle.angle_of(p2), circle.angle_of(p3))
    };
    let [cx, cy] = circle.center;
    let r = circle.radius;
    // The axis extremes, written exactly rather than through cos/sin.
    let extremes = [(0.0, [cx + r, cy]), (PI / 2.0, [cx, cy + r]), (PI, [cx - r, cy]), (-PI / 2.0, [cx, cy - r])];
    for (angle, point) in extremes {
        let reached = if total > 0.0 {
            (angle - start).rem_euclid(TAU) <= total
        } else {
            (start - angle).rem_euclid(TAU) <= -total
        };
        if reached {
            extend_bbox(bbox, point);
        }
    }
}

pub(crate) fn extend_bbox(bbox: &mut [f64; 4], p: [f64; 2]) {
    if p[0].is_nan() || p[1].is_nan() {
        return;
    }
    bbox[0] = bbox[0].min(p[0]);
    bbox[1] = bbox[1].min(p[1]);
    bbox[2] = bbox[2].max(p[0]);
    bbox[3] = bbox[3].max(p[1]);
}
