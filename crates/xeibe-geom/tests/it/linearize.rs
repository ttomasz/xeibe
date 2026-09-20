//! Linearization: the opt-in, lossy conversion of curves to line strings
//! (`docs/geometry.md`, "Linearization"). The parameters and defaults are
//! GDAL's, so our output stays comparable with the GDAL reference output.

use xeibe_geom::linearize::{linearize, linearize_circular};
use xeibe_geom::model::{CircularString, Coords, Curve, CurvePolygon, Dim, Geometry, OutputKind};
use xeibe_geom::options::LinearizeOptions;

use crate::support::coords_to_vec;

fn quarter_circle() -> CircularString {
    // Unit circle at the origin, from (1,0) over (√½,√½) to (0,1).
    CircularString {
        coords: Coords {
            dim: Some(Dim::Xy),
            values: vec![1.0, 0.0, 0.5f64.sqrt(), 0.5f64.sqrt(), 0.0, 1.0],
        },
        computed: Vec::new(),
    }
}

#[test]
fn the_defaults_are_gdals() {
    let options = LinearizeOptions::default();
    assert_eq!(options.max_angle_step_deg, 4.0, "OGR_ARC_STEPSIZE");
    assert_eq!(options.max_gap, None, "OGR_ARC_MAX_GAP is off by default");
}

#[test]
fn an_arc_becomes_a_line_on_the_circle() {
    let line = linearize_circular(&quarter_circle(), &LinearizeOptions::default());
    let vertices = coords_to_vec(&line.coords);
    assert!(vertices.len() >= 23, "90° / 4° steps, got {}", vertices.len());
    for vertex in &vertices {
        let radius = (vertex[0] * vertex[0] + vertex[1] * vertex[1]).sqrt();
        assert!((radius - 1.0).abs() < 1e-9, "{vertex:?} is off the circle");
    }
    // The ends are the stored control points, exactly.
    assert_eq!(vertices.first().unwrap(), &vec![1.0, 0.0]);
    assert_eq!(vertices.last().unwrap(), &vec![0.0, 1.0]);
}

#[test]
fn stored_control_points_are_always_kept_as_vertices() {
    let line = linearize_circular(&quarter_circle(), &LinearizeOptions::default());
    let middle = vec![0.5f64.sqrt(), 0.5f64.sqrt()];
    assert!(
        coords_to_vec(&line.coords).contains(&middle),
        "the middle control point must stay a vertex"
    );
}

#[test]
fn the_angle_step_bounds_the_turn_between_vertices() {
    let coarse = LinearizeOptions {
        max_angle_step_deg: 30.0,
        max_gap: None,
    };
    let line = linearize_circular(&quarter_circle(), &coarse);
    let vertices = coords_to_vec(&line.coords);
    assert!(vertices.len() <= 6, "30° steps over 90°, got {}", vertices.len());
    for pair in vertices.windows(2) {
        let angle = (pair[0][0] * pair[1][0] + pair[0][1] * pair[1][1])
            .clamp(-1.0, 1.0)
            .acos()
            .to_degrees();
        assert!(angle <= 30.0 + 1e-9, "a {angle}° step is too large");
    }
}

#[test]
fn the_max_gap_bounds_the_distance_between_vertices() {
    let options = LinearizeOptions {
        max_angle_step_deg: 90.0,
        max_gap: Some(0.05),
    };
    let line = linearize_circular(&quarter_circle(), &options);
    for pair in coords_to_vec(&line.coords).windows(2) {
        let distance = ((pair[0][0] - pair[1][0]).powi(2) + (pair[0][1] - pair[1][1]).powi(2)).sqrt();
        assert!(distance <= 0.05 + 1e-9, "a gap of {distance} is too large");
    }
}

#[test]
fn linearizing_a_geometry_changes_its_type() {
    let options = LinearizeOptions::default();
    assert_eq!(
        linearize(Geometry::CircularString(quarter_circle()), &options).kind(),
        OutputKind::LineString
    );

    let curve_polygon = Geometry::CurvePolygon(CurvePolygon {
        exterior: Some(Curve::Circular(CircularString {
            coords: Coords {
                dim: Some(Dim::Xy),
                values: vec![0.0, 0.0, 1.0, 1.0, 2.0, 0.0, 1.0, -1.0, 0.0, 0.0],
            },
            computed: Vec::new(),
        })),
        interiors: Vec::new(),
    });
    assert_eq!(
        linearize(curve_polygon, &options).kind(),
        OutputKind::Polygon
    );
}

#[test]
fn geometry_without_curves_is_left_alone() {
    let point = Geometry::Point(xeibe_geom::model::Point {
        coord: Some(vec![1.0, 2.0]),
    });
    assert_eq!(
        linearize(point.clone(), &LinearizeOptions::default()),
        point
    );
}
