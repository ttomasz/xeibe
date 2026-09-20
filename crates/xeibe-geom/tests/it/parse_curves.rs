//! `Curve` and its segments, `Ring`, `OrientableCurve`, `CompositeCurve`, and
//! the arcs (`docs/geometry.md`, "Mapping table", "Arcs given by points",
//! "Arcs given by parameters", "Joining segments and members").
//!
//! Expected values for the computed arcs follow GDAL, whose conventions the
//! design adopts; several come from GDAL's own test suite
//! (`tests/data/gdal/gml_geometry_cases.jsonl`).

use xeibe_testkit::wkt::{Tol, assert_wkt, assert_wkt_tol};

use crate::support::{assert_geometry, assert_geometry_tol, g, parse, warnings};

fn curve(segments: &str) -> String {
    format!("<gml:Curve><gml:segments>{segments}</gml:segments></gml:Curve>")
}

#[test]
fn a_curve_of_line_string_segments_is_a_line_string() {
    assert_geometry(
        &curve("<gml:LineStringSegment><gml:posList>0 0 1 1</gml:posList></gml:LineStringSegment>"),
        "LINESTRING (0 0,1 1)",
    );
}

#[test]
fn adjacent_segments_share_their_end_point_once() {
    // "The shared start point of each following segment is dropped."
    let segments = concat!(
        "<gml:LineStringSegment><gml:posList>0 0 1 1</gml:posList></gml:LineStringSegment>",
        "<gml:LineStringSegment><gml:posList>1 1 2 0</gml:posList></gml:LineStringSegment>"
    );
    assert_geometry(&curve(segments), "LINESTRING (0 0,1 1,2 0)");
}

#[test]
fn a_gap_between_segments_keeps_both_points_and_warns() {
    // GDAL rejects non-contiguous curves; we keep the data and warn.
    let segments = concat!(
        "<gml:LineStringSegment><gml:posList>0 0 1 1</gml:posList></gml:LineStringSegment>",
        "<gml:LineStringSegment><gml:posList>5 5 6 6</gml:posList></gml:LineStringSegment>"
    );
    assert_geometry(&curve(segments), "LINESTRING (0 0,1 1,5 5,6 6)");
    assert!(!warnings(&curve(segments)).is_empty(), "a gap warns");
}

#[test]
fn a_curve_with_one_arc_segment_is_a_circular_string() {
    assert_geometry(
        &curve("<gml:Arc><gml:posList>0 0 1 1 2 0</gml:posList></gml:Arc>"),
        "CIRCULARSTRING (0 0,1 1,2 0)",
    );
}

#[test]
fn mixed_segments_become_a_compound_curve() {
    let segments = concat!(
        "<gml:LineStringSegment><gml:posList>-1 0 0 0</gml:posList></gml:LineStringSegment>",
        "<gml:Arc><gml:posList>0 0 1 1 2 0</gml:posList></gml:Arc>",
        "<gml:LineStringSegment><gml:posList>2 0 3 0</gml:posList></gml:LineStringSegment>"
    );
    assert_geometry(
        &curve(segments),
        "COMPOUNDCURVE ((-1 0,0 0),CIRCULARSTRING (0 0,1 1,2 0),(2 0,3 0))",
    );
}

#[test]
fn consecutive_segments_of_one_kind_merge_into_one_part() {
    let segments = concat!(
        "<gml:LineStringSegment><gml:posList>-2 0 -1 0</gml:posList></gml:LineStringSegment>",
        "<gml:LineStringSegment><gml:posList>-1 0 0 0</gml:posList></gml:LineStringSegment>",
        "<gml:Arc><gml:posList>0 0 1 1 2 0</gml:posList></gml:Arc>",
        "<gml:Arc><gml:posList>2 0 3 1 4 0</gml:posList></gml:Arc>"
    );
    assert_geometry(
        &curve(segments),
        "COMPOUNDCURVE ((-2 0,-1 0,0 0),CIRCULARSTRING (0 0,1 1,2 0,3 1,4 0))",
    );
}

#[test]
fn arcs_keep_their_stored_points_exactly() {
    // Arcs given by points are lossless.
    assert_geometry(
        &curve("<gml:ArcString><gml:posList>0 0 1 1 2 0 3 -1 4 0</gml:posList></gml:ArcString>"),
        "CIRCULARSTRING (0 0,1 1,2 0,3 -1,4 0)",
    );
    // [GDAL] An `Arc` with more than 3 positions is accepted (odd count ≥ 3).
    assert_geometry(
        &curve("<gml:Arc><gml:posList>0 0 1 1 2 0 3 -1 4 0</gml:posList></gml:Arc>"),
        "CIRCULARSTRING (0 0,1 1,2 0,3 -1,4 0)",
    );
}

#[test]
fn an_even_number_of_arc_positions_is_an_error() {
    assert!(parse(&curve("<gml:Arc><gml:posList>0 0 0 1</gml:posList></gml:Arc>")).is_err());
    assert!(
        parse(&curve(
            "<gml:ArcString><gml:posList>0 0 0 1 1 0 2 0</gml:posList></gml:ArcString>"
        ))
        .is_err()
    );
}

#[test]
fn a_num_arc_that_does_not_match_only_warns() {
    let snippet = curve(
        r#"<gml:ArcString numArc="2"><gml:posList>0 0 1 1 2 0</gml:posList></gml:ArcString>"#,
    );
    assert_geometry(&snippet, "CIRCULARSTRING (0 0,1 1,2 0)");
    assert!(!warnings(&snippet).is_empty(), "numArc mismatch warns");
}

#[test]
fn a_circle_is_closed_with_one_computed_point() {
    // [GDAL] `p1 p2 p3 m p1`: only `m` is computed, the direction is kept.
    // Circle through (0,0), (1,1), (2,0): centre (1,0), radius 1, travelling
    // over the top, so the arc back to the start passes (1,-1).
    assert_geometry_tol(
        &curve("<gml:Circle><gml:posList>0 0 1 1 2 0</gml:posList></gml:Circle>"),
        "CIRCULARSTRING (0 0,1 1,2 0,1 -1,0 0)",
        Tol::abs(1e-9),
    );
    // The other direction of travel (GDAL case ogr_gml_geom:2633).
    assert_geometry_tol(
        &curve(concat!(
            "<gml:Circle><gml:posList>-1 0 0 1 -0.707106781186547 -0.707106781186548",
            "</gml:posList></gml:Circle>"
        )),
        concat!(
            "CIRCULARSTRING (-1 0,0 1,-0.707106781186547 -0.707106781186548,",
            "-0.923879532511287 -0.38268343236509,-1 0)"
        ),
        Tol::abs(1e-9),
    );
}

#[test]
fn a_circle_needs_three_distinct_non_collinear_points() {
    assert!(parse(&curve("<gml:Circle><gml:posList>0 0 0 1</gml:posList></gml:Circle>")).is_err());
    assert!(
        parse(&curve(
            "<gml:Circle><gml:posList>0 0 1 0 2 0</gml:posList></gml:Circle>"
        ))
        .is_err(),
        "collinear points define no circle"
    );
}

#[test]
fn arc_by_center_point_follows_the_gdal_convention() {
    // Angles in degrees, counter-clockwise from +x, through the mean angle
    // (GDAL case ogr_gml_geom:2304).
    let snippet = curve(concat!(
        "<gml:ArcByCenterPoint><gml:pos>1 2</gml:pos><gml:radius>2</gml:radius>",
        "<gml:startAngle>90</gml:startAngle><gml:endAngle>270</gml:endAngle>",
        "</gml:ArcByCenterPoint>"
    ));
    assert_geometry_tol(&snippet, "CIRCULARSTRING (1 4,-1 2,1 0)", Tol::abs(1e-9));
}

#[test]
fn arc_by_center_point_converts_the_radius_unit() {
    // `uom="km"` with a CRS in metres (GDAL case ogr_gml_geom:2311).
    let snippet = curve(concat!(
        r#"<gml:ArcByCenterPoint><gml:pos>1 2</gml:pos><gml:radius uom="km">0.002</gml:radius>"#,
        "<gml:startAngle>90</gml:startAngle><gml:endAngle>270</gml:endAngle>",
        "</gml:ArcByCenterPoint>"
    ));
    assert_geometry_tol(&snippet, "CIRCULARSTRING (1 4,-1 2,1 0)", Tol::abs(1e-6));
}

#[test]
fn angles_accept_the_uom_spellings_seen_in_real_data() {
    // The Geneva sample writes `uom="degree"`, not `deg`.
    let degree = curve(concat!(
        r#"<gml:ArcByCenterPoint numArc="1"><gml:pos>0 0</gml:pos>"#,
        r#"<gml:radius uom="m">1</gml:radius>"#,
        r#"<gml:startAngle uom="degree">0</gml:startAngle>"#,
        r#"<gml:endAngle uom="degree">90</gml:endAngle></gml:ArcByCenterPoint>"#
    ));
    assert_geometry_tol(
        &degree,
        "CIRCULARSTRING (1 0,0.7071067811865476 0.7071067811865475,0 1)",
        Tol::abs(1e-9),
    );
}

#[test]
fn circle_by_center_point_starts_in_the_west() {
    // [GDAL] W, N, E, S, W (GDAL case ogr_gml_geom:2410).
    let snippet = curve(concat!(
        "<gml:CircleByCenterPoint><gml:pos>1 2</gml:pos><gml:radius>2</gml:radius>",
        "</gml:CircleByCenterPoint>"
    ));
    assert_geometry_tol(
        &snippet,
        "CIRCULARSTRING (-1 2,1 4,3 2,1 0,-1 2)",
        Tol::abs(1e-9),
    );
}

#[test]
fn a_parameter_arc_without_its_parameters_is_an_error() {
    for segment in [
        "<gml:ArcByCenterPoint><gml:pos>1 2</gml:pos><gml:startAngle>90</gml:startAngle><gml:endAngle>270</gml:endAngle></gml:ArcByCenterPoint>",
        "<gml:ArcByCenterPoint><gml:pos>1 2</gml:pos><gml:radius>2</gml:radius><gml:startAngle>90</gml:startAngle></gml:ArcByCenterPoint>",
        "<gml:CircleByCenterPoint><gml:pos>1 2</gml:pos></gml:CircleByCenterPoint>",
    ] {
        assert!(parse(&curve(segment)).is_err(), "{segment}");
    }
}

#[test]
fn arc_by_bulge_uses_the_gdal_formula() {
    // GDAL case ogr_gml_geom:2290: chord (2,0) → (-2,0), bulge 2, normal -1.
    let snippet = curve(concat!(
        "<gml:ArcByBulge><gml:posList>2 0 -2 0</gml:posList><gml:bulge>2</gml:bulge>",
        "<gml:normal>-1</gml:normal></gml:ArcByBulge>"
    ));
    assert_geometry_tol(&snippet, "CIRCULARSTRING (2 0,0 2,-2 0)", Tol::abs(1e-9));

    for bad in [
        "<gml:ArcByBulge><gml:posList>2 0</gml:posList><gml:bulge>2</gml:bulge><gml:normal>-1</gml:normal></gml:ArcByBulge>",
        "<gml:ArcByBulge><gml:posList>2 0 -2 0</gml:posList><gml:normal>-1</gml:normal></gml:ArcByBulge>",
    ] {
        assert!(parse(&curve(bad)).is_err(), "{bad}");
    }
}

#[test]
fn stored_coordinates_win_over_computed_ones() {
    // Where a computed arc meets a stored position, the stored one is used, so
    // rounding never moves source data (`docs/geometry.md`, "Joining segments").
    let snippet = curve(concat!(
        r#"<gml:ArcByCenterPoint><gml:pos>0 0</gml:pos><gml:radius uom="m">1</gml:radius>"#,
        "<gml:startAngle>0</gml:startAngle><gml:endAngle>90</gml:endAngle></gml:ArcByCenterPoint>",
        "<gml:LineStringSegment><gml:posList>0.0000000001 1.0000000001 5 5</gml:posList>",
        "</gml:LineStringSegment>"
    ));
    let geometry = g(&snippet);
    let vertices = geometry.vertices();
    let joint = &vertices[vertices.len() - 2];
    assert_eq!(
        joint,
        &vec![0.0000000001, 1.0000000001],
        "the stored position is kept exactly: {geometry}"
    );
}

#[test]
fn orientable_curve_reverses_with_a_minus_orientation() {
    let base = concat!(
        "<gml:baseCurve><gml:LineString><gml:posList>0 1 2 3</gml:posList></gml:LineString>",
        "</gml:baseCurve>"
    );
    assert_geometry(
        &format!(r#"<gml:OrientableCurve orientation="+">{base}</gml:OrientableCurve>"#),
        "LINESTRING (0 1,2 3)",
    );
    assert_geometry(
        &format!(r#"<gml:OrientableCurve orientation="-">{base}</gml:OrientableCurve>"#),
        "LINESTRING (2 3,0 1)",
    );
    // They may nest (§10.4.6): two reversals cancel.
    let nested = format!(
        r#"<gml:OrientableCurve orientation="-"><gml:baseCurve><gml:OrientableCurve orientation="-">{base}</gml:OrientableCurve></gml:baseCurve></gml:OrientableCurve>"#
    );
    assert_geometry(&nested, "LINESTRING (0 1,2 3)");
}

#[test]
fn an_orientable_curve_without_a_base_curve_is_an_error() {
    assert!(parse("<gml:OrientableCurve/>").is_err());
    assert!(parse("<gml:OrientableCurve><gml:baseCurve/></gml:OrientableCurve>").is_err());
    assert!(
        parse(concat!(
            "<gml:OrientableCurve><gml:baseCurve><gml:Point><gml:pos>0 0</gml:pos>",
            "</gml:Point></gml:baseCurve></gml:OrientableCurve>"
        ))
        .is_err(),
        "a point is not a curve"
    );
}

#[test]
fn composite_curve_joins_its_members() {
    let composite = concat!(
        "<gml:CompositeCurve>",
        "<gml:curveMember><gml:LineString><gml:posList>0 0 1 1</gml:posList></gml:LineString></gml:curveMember>",
        "<gml:curveMember><gml:LineString><gml:posList>1 1 2 2</gml:posList></gml:LineString></gml:curveMember>",
        "</gml:CompositeCurve>"
    );
    assert_geometry(composite, "LINESTRING (0 0,1 1,2 2)");

    let with_arc = concat!(
        "<gml:CompositeCurve>",
        "<gml:curveMember><gml:LineString><gml:posList>-1 0 0 0</gml:posList></gml:LineString></gml:curveMember>",
        "<gml:curveMember><gml:Curve><gml:segments>",
        "<gml:Arc><gml:posList>0 0 1 1 2 0</gml:posList></gml:Arc>",
        "</gml:segments></gml:Curve></gml:curveMember></gml:CompositeCurve>"
    );
    assert_geometry(
        with_arc,
        "COMPOUNDCURVE ((-1 0,0 0),CIRCULARSTRING (0 0,1 1,2 0))",
    );
}

#[test]
fn a_ring_of_curve_members_closes_the_cycle() {
    let ring = concat!(
        "<gml:Polygon><gml:exterior><gml:Ring>",
        "<gml:curveMember><gml:LineString><gml:posList>0 0 1 0</gml:posList></gml:LineString></gml:curveMember>",
        "<gml:curveMember><gml:LineString><gml:posList>1 0 1 1 0 0</gml:posList></gml:LineString></gml:curveMember>",
        "</gml:Ring></gml:exterior></gml:Polygon>"
    );
    assert_geometry(ring, "POLYGON ((0 0,1 0,1 1,0 0))");

    // A ring with an arc stays a curve polygon (GDAL case ogr_gml_geom:2789).
    let curved = concat!(
        "<gml:Polygon><gml:exterior><gml:Ring><gml:curveMember><gml:Curve><gml:segments>",
        "<gml:Circle><gml:posList>0 0 0.5 0.5 1 0</gml:posList></gml:Circle>",
        "</gml:segments></gml:Curve></gml:curveMember></gml:Ring></gml:exterior></gml:Polygon>"
    );
    assert_wkt_tol(
        &g(curved),
        "CURVEPOLYGON (CIRCULARSTRING (0 0,0.5 0.5,1 0,0.5 -0.5,0 0))",
        Tol::abs(1e-9),
    );
}

#[test]
fn a_curve_without_segments_is_an_error() {
    assert!(parse("<gml:Curve/>").is_err());
    assert!(parse("<gml:Curve><gml:segments/></gml:Curve>").is_err());
}

#[test]
fn the_interpolation_attribute_only_warns_when_it_disagrees() {
    // The element name decides how a segment is parsed (support matrix §5.4).
    let snippet = curve(concat!(
        r#"<gml:LineStringSegment interpolation="circularArc3Points">"#,
        "<gml:posList>0 0 1 1</gml:posList></gml:LineStringSegment>"
    ));
    assert_wkt(&g(&snippet), "LINESTRING (0 0,1 1)");
    assert!(!warnings(&snippet).is_empty(), "a mismatch warns");
}
