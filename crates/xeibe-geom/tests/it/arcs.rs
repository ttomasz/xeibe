//! The arc computations on their own (`docs/geometry.md`, "Arcs given by
//! parameters"). The conventions are GDAL's, as the docs state.

use xeibe_geom::arcs::{
    angle_to_degrees, arc_by_bulge_midpoint, arc_by_center_point, circle_by_center_point,
    circle_closing_midpoint, geodesic_arc, uom_to_metres,
};

#[track_caller]
fn close(actual: [f64; 2], expected: [f64; 2]) {
    let ok = (actual[0] - expected[0]).abs() < 1e-9 && (actual[1] - expected[1]).abs() < 1e-9;
    assert!(ok, "{actual:?} != {expected:?}");
}

#[test]
fn the_closing_midpoint_of_a_circle_keeps_the_direction_of_travel() {
    // (0,0) → (1,1) → (2,0) travels over the top, so the arc back to the start
    // passes below: (1,-1).
    close(
        circle_closing_midpoint([0.0, 0.0], [1.0, 1.0], [2.0, 0.0]).expect("a circle"),
        [1.0, -1.0],
    );
    // The other direction.
    close(
        circle_closing_midpoint([0.0, 0.0], [1.0, -1.0], [2.0, 0.0]).expect("a circle"),
        [1.0, 1.0],
    );
    // Collinear points define no circle.
    assert_eq!(
        circle_closing_midpoint([0.0, 0.0], [1.0, 0.0], [2.0, 0.0]),
        None
    );
}

#[test]
fn arc_by_center_point_turns_counter_clockwise_from_the_x_axis() {
    let [start, mid, end] = arc_by_center_point([0.0, 0.0], 1.0, 0.0, 90.0);
    close(start, [1.0, 0.0]);
    close(mid, [0.5f64.sqrt(), 0.5f64.sqrt()]);
    close(end, [0.0, 1.0]);

    // end < start turns clockwise.
    let [start, mid, end] = arc_by_center_point([0.0, 0.0], 1.0, 90.0, 0.0);
    close(start, [0.0, 1.0]);
    close(mid, [0.5f64.sqrt(), 0.5f64.sqrt()]);
    close(end, [1.0, 0.0]);

    // The arc passes through the *mean* angle, so 350° → 10° goes the long way
    // round, through 180° ([GDAL]).
    let [_, mid, _] = arc_by_center_point([0.0, 0.0], 1.0, 350.0, 10.0);
    close(mid, [-1.0, 0.0]);

    // GDAL case ogr_gml_geom:2304, with a centre away from the origin.
    let [start, mid, end] = arc_by_center_point([1.0, 2.0], 2.0, 90.0, 270.0);
    close(start, [1.0, 4.0]);
    close(mid, [-1.0, 2.0]);
    close(end, [1.0, 0.0]);
}

#[test]
fn circle_by_center_point_starts_in_the_west() {
    // W, N, E, S, W ([GDAL], case ogr_gml_geom:2410).
    let points = circle_by_center_point([1.0, 2.0], 2.0);
    close(points[0], [-1.0, 2.0]);
    close(points[1], [1.0, 4.0]);
    close(points[2], [3.0, 2.0]);
    close(points[3], [1.0, 0.0]);
    close(points[4], [-1.0, 2.0]);
}

#[test]
fn arc_by_bulge_uses_the_perpendicular_of_the_chord() {
    // GDAL case ogr_gml_geom:2290: (2,0) → (-2,0), bulge 2, normal -1 → (0,2).
    close(
        arc_by_bulge_midpoint([2.0, 0.0], [-2.0, 0.0], 2.0, -1.0),
        [0.0, 2.0],
    );
    // A positive normal mirrors the bulge.
    close(
        arc_by_bulge_midpoint([2.0, 0.0], [-2.0, 0.0], 2.0, 1.0),
        [0.0, -2.0],
    );
}

#[test]
fn units_of_measure_are_converted() {
    assert_eq!(uom_to_metres(Some("m")), Some(1.0));
    assert_eq!(uom_to_metres(Some("km")), Some(1000.0));
    assert_eq!(uom_to_metres(Some("[nmi_i]")), Some(1852.0));
    assert_eq!(uom_to_metres(Some("ft")), Some(0.3048));
    assert_eq!(uom_to_metres(Some("unhandled")), None);

    assert_eq!(angle_to_degrees(90.0, Some("deg")), Some(90.0));
    // The Geneva sample writes `degree`.
    assert_eq!(angle_to_degrees(90.0, Some("degree")), Some(90.0));
    // No uom means degrees.
    assert_eq!(angle_to_degrees(90.0, None), Some(90.0));
    let from_radians = angle_to_degrees(std::f64::consts::FRAC_PI_2, Some("rad")).expect("radians");
    assert!((from_radians - 90.0).abs() < 1e-9, "{from_radians}");
    assert_eq!(angle_to_degrees(90.0, Some("nonsense")), None);
}

#[test]
fn a_geodesic_arc_follows_compass_bearings() {
    // Geographic CRSs use AIXM bearings: clockwise from north. Lossy, so the
    // tolerance is loose (support matrix: 🤔 Considering, P2).
    let points = geodesic_arc([0.0, 0.0], 111_000.0, 0.0, 90.0, 4.0, 6_378_137.0);
    assert!(points.len() >= 23, "one vertex per 4°, got {}", points.len());
    let first = points.first().expect("a first point");
    let last = points.last().expect("a last point");
    assert!(
        first[0].abs() < 0.01 && (first[1] - 1.0).abs() < 0.02,
        "bearing 0 is north: {first:?}"
    );
    assert!(
        (last[0] - 1.0).abs() < 0.02 && last[1].abs() < 0.01,
        "bearing 90 is east: {last:?}"
    );
}
