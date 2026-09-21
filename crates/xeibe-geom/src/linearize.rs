//! Opt-in, lossy conversion of curves to line strings.
//!
//! Each arc `p0 p1 p2` is split at its middle control point, and each half is
//! divided into equal steps no larger than `max_angle_step_deg` (and, with
//! `max_gap`, no longer than that chord). Z is interpolated linearly along the
//! angle. Collinear control points are kept as a straight line.

use std::f64::consts::TAU;

use crate::arcs::Circle;
use crate::model::{
    CircularString, Coords, Curve, CurvePart, CurvePolygon, Geometry, LineString, MultiLineString,
    MultiPolygon, Polygon, Surface,
};
use crate::options::LinearizeOptions;

/// Stored control points are always kept as vertices.
pub fn linearize_circular(arc: &CircularString, options: &LinearizeOptions) -> LineString {
    let coords = &arc.coords;
    let n = coords.len();
    let mut out = Coords { dim: coords.dim, values: Vec::with_capacity(coords.values.len()) };
    if n == 0 {
        return LineString { coords: out };
    }
    out.push(coords.get(0));
    let mut i = 0;
    while i + 2 < n {
        linearize_arc(coords.get(i), coords.get(i + 1), coords.get(i + 2), options, &mut out);
        i += 2;
    }
    // Positions that complete no arc (invalid input) are kept as they are.
    for j in i + 1..n {
        out.push(coords.get(j));
    }
    LineString { coords: out }
}

pub fn linearize(geometry: Geometry, options: &LinearizeOptions) -> Geometry {
    match geometry {
        Geometry::CircularString(arc) => Geometry::LineString(linearize_circular(&arc, options)),
        Geometry::CompoundCurve(curve) => {
            Geometry::LineString(linearize_curve(Curve::Compound(curve), options))
        }
        Geometry::CurvePolygon(polygon) => Geometry::Polygon(linearize_polygon(polygon, options)),
        Geometry::MultiCurve(curves) => Geometry::MultiLineString(MultiLineString(
            curves.0.into_iter().map(|c| linearize_curve(c, options)).collect(),
        )),
        Geometry::MultiSurface(surfaces) => Geometry::MultiPolygon(MultiPolygon(
            surfaces
                .0
                .into_iter()
                .map(|surface| match surface {
                    Surface::Polygon(polygon) => polygon,
                    Surface::CurvePolygon(polygon) => linearize_polygon(polygon, options),
                })
                .collect(),
        )),
        Geometry::GeometryCollection(mut members) => {
            members.0 = members.0.into_iter().map(|g| linearize(g, options)).collect();
            Geometry::GeometryCollection(members)
        }
        simple => simple,
    }
}

/// Any curve as one line string; the parts of a compound curve are joined.
pub fn linearize_curve(curve: Curve, options: &LinearizeOptions) -> LineString {
    match curve {
        Curve::Linear(line) => line,
        Curve::Circular(arc) => linearize_circular(&arc, options),
        Curve::Compound(compound) => {
            let mut coords = Coords::default();
            for part in compound.parts {
                let line = match part {
                    CurvePart::Linear(line) => line,
                    CurvePart::Circular(arc) => linearize_circular(&arc, options),
                };
                coords.append_joined(&line.coords, 0.0);
            }
            LineString { coords }
        }
    }
}

fn linearize_polygon(polygon: CurvePolygon, options: &LinearizeOptions) -> Polygon {
    Polygon {
        exterior: polygon.exterior.map(|ring| linearize_curve(ring, options)),
        interiors: polygon.interiors.into_iter().map(|ring| linearize_curve(ring, options)).collect(),
    }
}

/// Append the vertices of arc `p0 p1 p2` after `p0` (already in `out`).
fn linearize_arc(p0: &[f64], p1: &[f64], p2: &[f64], options: &LinearizeOptions, out: &mut Coords) {
    let Some(circle) = Circle::through([p0[0], p0[1]], [p1[0], p1[1]], [p2[0], p2[1]]) else {
        out.push(p1);
        out.push(p2);
        return;
    };
    let max_step = max_step_radians(circle.radius, options);
    let a0 = circle.angle_of([p0[0], p0[1]]);
    let a1 = circle.angle_of([p1[0], p1[1]]);
    let a2 = circle.angle_of([p2[0], p2[1]]);
    let first = if p0 == p2 { TAU / 2.0 * sign(circle.ccw) } else { circle.sweep(a0, a1) };
    let second = if p0 == p2 { TAU / 2.0 * sign(circle.ccw) } else { circle.sweep(a1, a2) };
    subdivide(&circle, a0, first, p0, p1, max_step, out);
    subdivide(&circle, a1, second, p1, p2, max_step, out);
}

fn sign(ccw: bool) -> f64 {
    if ccw { 1.0 } else { -1.0 }
}

/// The largest angle one segment may span on a circle of this radius.
fn max_step_radians(radius: f64, options: &LinearizeOptions) -> f64 {
    let mut step = if options.max_angle_step_deg > 0.0 {
        options.max_angle_step_deg.to_radians()
    } else {
        LinearizeOptions::default().max_angle_step_deg.to_radians()
    };
    if let Some(gap) = options.max_gap.filter(|gap| *gap > 0.0) {
        // Chord length 2r·sin(θ/2) ≤ gap.
        if gap < 2.0 * radius {
            step = step.min(2.0 * (gap / (2.0 * radius)).asin());
        }
    }
    step
}

/// Append the intermediate vertices from `start` over `sweep`, then `to` itself
/// (stored, so exact).
fn subdivide(
    circle: &Circle,
    start: f64,
    sweep: f64,
    from: &[f64],
    to: &[f64],
    max_step: f64,
    out: &mut Coords,
) {
    let segments = (sweep.abs() / max_step).ceil().max(1.0) as usize;
    let z = |t: f64| match (from.get(2), to.get(2)) {
        (Some(z0), Some(z1)) => Some(z0 + (z1 - z0) * t),
        _ => None,
    };
    for k in 1..segments {
        let t = k as f64 / segments as f64;
        let [x, y] = circle.point_at(start + sweep * t);
        match z(t) {
            Some(z) => out.push(&[x, y, z]),
            None => out.push(&[x, y]),
        }
    }
    out.push(to);
}
