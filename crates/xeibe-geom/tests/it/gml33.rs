//! GML 3.3 (OGC 10-129r1) geometry (`docs/geometry.md`, "GML 3.3").
//!
//! The compact encodings (§7) are read as the GML 3.2 geometry each one
//! abbreviates: the spec defines every one as logically equivalent to a 3.2
//! `Surface` or `Curve`, so most tests compare the two. Triangulated surfaces
//! (§8) and referenceable grids (§10) are unsupported, like their GML 3.2
//! counterparts.

use xeibe_core::Dialect;
use xeibe_geom::Error;
use xeibe_geom::model::GeomKind;
use xeibe_testkit::wkt::{Tol, assert_wkt, assert_wkt_tol};

use crate::support::{FixedAxis, RecordingAxis, g, g33, parse_gml33, parse_gml33_with};

fn curve(segment: &str) -> String {
    format!("<gml:Curve><gml:segments>{segment}</gml:segments></gml:Curve>")
}

#[track_caller]
fn assert_warning_free(snippet: &str) {
    let parsed = parse_gml33(snippet).unwrap_or_else(|e| panic!("parsing {snippet}: {e}"));
    assert_eq!(parsed.warnings, Vec::<String>::new(), "{snippet}");
}

// ------------------------------------------------------ compact surfaces

#[test]
fn a_simple_polygon_is_a_polygon_whose_ring_is_closed_for_it() {
    // §7.3: "The last coordinate does not have to repeat the first", so
    // closing the ring is not a repair and gives no warning.
    let snippet = "<gmlce:SimplePolygon><gml:posList>0 0 1 0 1 1 0 1</gml:posList></gmlce:SimplePolygon>";
    assert_wkt(&g33(snippet), "POLYGON ((0 0,1 0,1 1,0 1,0 0))");
    assert_warning_free(snippet);
}

#[test]
fn a_simple_polygon_that_repeats_its_first_corner_is_closed_once() {
    let snippet = "<gmlce:SimplePolygon><gml:posList>0 0 1 0 1 1 0 0</gml:posList></gmlce:SimplePolygon>";
    assert_wkt(&g33(snippet), "POLYGON ((0 0,1 0,1 1,0 0))");
}

#[test]
fn a_simple_polygon_is_the_surface_of_one_polygon_patch() {
    // §7.3: equivalent to a `Surface` with one `PolygonPatch` whose exterior
    // is a `LinearRing`, and no interior.
    let compact = "<gmlce:SimplePolygon><gml:posList>0 0 4 0 4 3 0 3</gml:posList></gmlce:SimplePolygon>";
    let surface = concat!(
        "<gml:Surface><gml:patches><gml:PolygonPatch><gml:exterior><gml:LinearRing>",
        "<gml:posList>0 0 4 0 4 3 0 3 0 0</gml:posList>",
        "</gml:LinearRing></gml:exterior></gml:PolygonPatch></gml:patches></gml:Surface>"
    );
    assert_eq!(g33(compact), g(surface));
}

#[test]
fn simple_rectangles_and_simple_triangles_are_polygons() {
    // §7.4–7.5: four and three corners. Read as polygons, like the
    // `Rectangle` and `Triangle` patches (GDAL makes a TRIANGLE).
    assert_wkt(
        &g33("<gmlce:SimpleRectangle><gml:posList>0 0 1 0 1 1 0 1</gml:posList></gmlce:SimpleRectangle>"),
        "POLYGON ((0 0,1 0,1 1,0 1,0 0))",
    );
    assert_wkt(
        &g33("<gmlce:SimpleTriangle><gml:posList>0 0 1 0 1 1</gml:posList></gmlce:SimpleTriangle>"),
        "POLYGON ((0 0,1 0,1 1,0 0))",
    );
}

#[test]
fn corners_may_be_pos_and_point_property_elements() {
    let snippet = concat!(
        "<gmlce:SimpleTriangle><gml:pos>0 0</gml:pos><gml:pos>1 0</gml:pos>",
        "<gml:pointProperty><gml:Point><gml:pos>1 1</gml:pos></gml:Point></gml:pointProperty>",
        "</gmlce:SimpleTriangle>"
    );
    assert_wkt(&g33(snippet), "POLYGON ((0 0,1 0,1 1,0 0))");
}

#[test]
fn a_simple_polygon_keeps_its_srs_name_and_dimension() {
    let parsed = parse_gml33(concat!(
        r#"<gmlce:SimplePolygon gml:id="g1" srsName="http://www.opengis.net/def/crs/EPSG/0/4258" srsDimension="3">"#,
        "<gml:posList>50 6 1 50 7 2 51 7 3</gml:posList></gmlce:SimplePolygon>"
    ))
    .expect("a SimplePolygon");
    assert_eq!(parsed.srs_name.as_deref(), Some("http://www.opengis.net/def/crs/EPSG/0/4258"));
    assert_eq!(parsed.source_kind, GeomKind::Polygon);
    assert_wkt(
        &crate::support::to_g(&parsed.geometry.expect("a geometry")),
        "POLYGON Z ((50 6 1,50 7 2,51 7 3,50 6 1))",
    );
}

#[test]
fn a_simple_polygon_with_fewer_than_three_corners_is_an_error() {
    let error = parse_gml33("<gmlce:SimplePolygon><gml:posList>0 0 1 1</gml:posList></gmlce:SimplePolygon>")
        .expect_err("two corners");
    assert!(
        matches!(error, Error::PositionCount { element: "SimplePolygon", found: 2, .. }),
        "got {error:?}"
    );
}

#[test]
fn an_empty_compact_encoding_is_an_empty_geometry() {
    // As `<gml:Point/>` ("Empty, invalid and degenerate geometry"); GDAL
    // gives a null geometry for some of these.
    assert_wkt(&g33("<gmlce:SimplePolygon/>"), "POLYGON EMPTY");
    assert_wkt(&g33("<gmlce:SimplePolygon><gml:posList/></gmlce:SimplePolygon>"), "POLYGON EMPTY");
    assert_wkt(&g33("<gmlce:SimpleMultiPoint/>"), "MULTIPOINT EMPTY");
    assert_wkt(&g33("<gmlce:SimpleArc/>"), "LINESTRING EMPTY");
    assert_wkt(&g33("<gmlce:SimpleArcByCenterPoint/>"), "LINESTRING EMPTY");
    assert_wkt(&g33("<gmlce:SimpleArcByBulge/>"), "LINESTRING EMPTY");
}

#[test]
fn a_simple_triangle_patch_is_a_triangle_of_a_surface() {
    // §8.4: a `Triangle` patch given by its three corners, allowed wherever a
    // patch is. A `gml:Surface` of them is read like one of `Triangle`s.
    let snippet = concat!(
        "<gml:Surface><gml:patches>",
        "<gmltin:SimpleTrianglePatch><gml:posList>0 0 0 1 1 1</gml:posList></gmltin:SimpleTrianglePatch>",
        "<gmltin:SimpleTrianglePatch><gml:posList>0 0 1 1 1 0</gml:posList></gmltin:SimpleTrianglePatch>",
        "</gml:patches></gml:Surface>"
    );
    assert_wkt(&g33(snippet), "MULTIPOLYGON (((0 0,0 1,1 1,0 0)),((0 0,1 1,1 0,0 0)))");
}

// ---------------------------------------------------- compact multipoint

#[test]
fn a_simple_multi_point_has_one_point_per_position() {
    // §7.13; GDAL case ogr_gml_geom:1940.
    assert_wkt(
        &g33("<gmlce:SimpleMultiPoint><gml:posList>0 1 2 3</gml:posList></gmlce:SimpleMultiPoint>"),
        "MULTIPOINT ((0 1),(2 3))",
    );
    assert_wkt(
        &g33(r#"<gmlce:SimpleMultiPoint srsDimension="3"><gml:posList>0 1 2 3 4 5</gml:posList></gmlce:SimpleMultiPoint>"#),
        "MULTIPOINT Z ((0 1 2),(3 4 5))",
    );
}

// -------------------------------------------------------- compact curves

/// Each compact curve and the GML 3.2 segment it abbreviates (§7.6–7.12),
/// with the same content.
fn compact_curves() -> Vec<(String, String)> {
    let pair = |compact: &str, segment: &str, content: &str, compact_content: &str| {
        (
            format!("<gmlce:{compact}>{compact_content}</gmlce:{compact}>"),
            curve(&format!("<gml:{segment}>{content}</gml:{segment}>")),
        )
    };
    let by_center = |prefix: &str, start: u32, end: u32| {
        format!(
            r#"<gml:pos>1 2</gml:pos><{prefix}:radius uom="m">2</{prefix}:radius><{prefix}:startAngle>{start}</{prefix}:startAngle><{prefix}:endAngle>{end}</{prefix}:endAngle>"#
        )
    };
    let by_bulge = |prefix: &str, positions: &str, arcs: usize| {
        let mut content = format!("<gml:posList>{positions}</gml:posList>");
        content.push_str(&format!("<{prefix}:bulge>2</{prefix}:bulge>").repeat(arcs));
        content.push_str(&format!("<{prefix}:normal>-1</{prefix}:normal>").repeat(arcs));
        content
    };
    let points = |positions: &str| format!("<gml:posList>{positions}</gml:posList>");
    vec![
        pair("SimpleArc", "Arc", &points("0 0 1 1 2 0"), &points("0 0 1 1 2 0")),
        pair("SimpleArcString", "ArcString", &points("0 0 1 1 2 0 3 -1 4 0"), &points("0 0 1 1 2 0 3 -1 4 0")),
        pair("SimpleCircle", "Circle", &points("0 0 1 1 2 0"), &points("0 0 1 1 2 0")),
        pair("SimpleArcByCenterPoint", "ArcByCenterPoint", &by_center("gml", 0, 90), &by_center("gmlce", 0, 90)),
        // §7.12: the angles are required and "should differ by 360 degrees".
        pair("SimpleCircleByCenterPoint", "CircleByCenterPoint", &by_center("gml", 0, 0), &by_center("gmlce", 0, 360)),
        pair("SimpleArcByBulge", "ArcByBulge", &by_bulge("gml", "2 0 -2 0", 1), &by_bulge("gmlce", "2 0 -2 0", 1)),
        pair(
            "SimpleArcStringByBulge",
            "ArcStringByBulge",
            &by_bulge("gml", "2 0 -2 0 -6 0", 2),
            &by_bulge("gmlce", "2 0 -2 0 -6 0", 2),
        ),
    ]
}

#[test]
fn each_compact_curve_is_the_curve_of_the_segment_it_abbreviates() {
    for (compact, curve) in compact_curves() {
        assert_eq!(g33(&compact), g(&curve), "{compact}");
        assert_warning_free(&compact);
    }
}

#[test]
fn compact_curves_are_circular_strings() {
    assert_wkt(
        &g33("<gmlce:SimpleArc><gml:posList>0 0 1 1 2 0</gml:posList></gmlce:SimpleArc>"),
        "CIRCULARSTRING (0 0,1 1,2 0)",
    );
    assert_wkt_tol(
        &g33(concat!(
            "<gmlce:SimpleArcByBulge><gml:posList>2 0 -2 0</gml:posList>",
            "<gmlce:bulge>2</gmlce:bulge><gmlce:normal>-1</gmlce:normal></gmlce:SimpleArcByBulge>"
        )),
        "CIRCULARSTRING (2 0,0 2,-2 0)",
        Tol::abs(1e-9),
    );
}

#[test]
fn a_compact_curve_follows_the_rules_of_its_segment_and_is_named_in_errors() {
    // An `Arc` needs an odd number of positions ("Arcs given by points").
    let error = parse_gml33("<gmlce:SimpleArc><gml:posList>0 0 1 1 2 0 3 3</gml:posList></gmlce:SimpleArc>")
        .expect_err("four positions");
    assert!(matches!(error, Error::PositionCount { element: "SimpleArc", found: 4, .. }), "got {error:?}");
    // A `Circle` needs exactly three.
    assert!(parse_gml33("<gmlce:SimpleCircle><gml:posList>0 0 1 1 2 0 3 1 4 0</gml:posList></gmlce:SimpleCircle>").is_err());
    // A center-point arc needs its radius.
    assert!(
        parse_gml33(concat!(
            "<gmlce:SimpleArcByCenterPoint><gml:pos>0 0</gml:pos><gmlce:startAngle>0</gmlce:startAngle>",
            "<gmlce:endAngle>90</gmlce:endAngle></gmlce:SimpleArcByCenterPoint>"
        ))
        .is_err()
    );
}

#[test]
fn a_num_arc_that_does_not_match_warns_as_for_the_segment() {
    let parsed = parse_gml33(
        r#"<gmlce:SimpleArcString numArc="3"><gml:posList>0 0 1 1 2 0</gml:posList></gmlce:SimpleArcString>"#,
    )
    .expect("the positions are used");
    assert_eq!(parsed.warnings.len(), 1, "{:?}", parsed.warnings);
}

// ------------------------------------------------- where compact ones fit

#[test]
fn compact_surfaces_are_members_of_surface_aggregates() {
    let snippet = concat!(
        "<gml:MultiSurface>",
        "<gml:surfaceMember><gmlce:SimpleRectangle><gml:posList>0 0 1 0 1 1 0 1</gml:posList></gmlce:SimpleRectangle></gml:surfaceMember>",
        "<gml:surfaceMember><gml:Polygon><gml:exterior><gml:LinearRing>",
        "<gml:posList>5 5 6 5 6 6 5 5</gml:posList></gml:LinearRing></gml:exterior></gml:Polygon></gml:surfaceMember>",
        "</gml:MultiSurface>"
    );
    assert_wkt(&g33(snippet), "MULTIPOLYGON (((0 0,1 0,1 1,0 1,0 0)),((5 5,6 5,6 6,5 5)))");
}

#[test]
fn compact_curves_are_members_of_curve_aggregates_and_rings() {
    let multi = concat!(
        "<gml:MultiCurve>",
        "<gml:curveMember><gmlce:SimpleArc><gml:posList>0 0 1 1 2 0</gml:posList></gmlce:SimpleArc></gml:curveMember>",
        "<gml:curveMember><gml:LineString><gml:posList>5 5 6 6</gml:posList></gml:LineString></gml:curveMember>",
        "</gml:MultiCurve>"
    );
    assert_wkt(&g33(multi), "MULTICURVE (CIRCULARSTRING (0 0,1 1,2 0),(5 5,6 6))");

    // Two half circles make the ring of a curve polygon.
    let ring = concat!(
        "<gml:Polygon><gml:exterior><gml:Ring>",
        "<gml:curveMember><gmlce:SimpleArc><gml:posList>0 0 1 1 2 0</gml:posList></gmlce:SimpleArc></gml:curveMember>",
        "<gml:curveMember><gmlce:SimpleArc><gml:posList>2 0 1 -1 0 0</gml:posList></gmlce:SimpleArc></gml:curveMember>",
        "</gml:Ring></gml:exterior></gml:Polygon>"
    );
    assert_wkt(&g33(ring), "CURVEPOLYGON (CIRCULARSTRING (0 0,1 1,2 0,1 -1,0 0))");
}

#[test]
fn a_compact_encoding_of_the_wrong_kind_is_invalid() {
    let snippet = concat!(
        "<gml:MultiCurve><gml:curveMember>",
        "<gmlce:SimplePolygon><gml:posList>0 0 1 0 1 1</gml:posList></gmlce:SimplePolygon>",
        "</gml:curveMember></gml:MultiCurve>"
    );
    let error = parse_gml33(snippet).expect_err("a polygon where a curve belongs");
    assert!(matches!(error, Error::InvalidGeometry { .. }), "got {error:?}");
}

#[test]
fn compact_encodings_are_gml_3_for_the_axis_decision() {
    // GML 3.3 builds on GML 3.2, so the `GmlVersion` mode treats it as GML 3.
    let axis = RecordingAxis::default();
    let parsed = parse_gml33_with(
        r#"<gmlce:SimpleMultiPoint srsName="EPSG:4326"><gml:posList>50 20</gml:posList></gmlce:SimpleMultiPoint>"#,
        &axis,
    )
    .expect("a SimpleMultiPoint");
    assert_eq!(parsed.dialect, Dialect::Gml3);
    assert_eq!(axis.calls(), vec![(Some("EPSG:4326".to_string()), Dialect::Gml3)]);

    let swapped = parse_gml33_with(
        "<gmlce:SimpleTriangle><gml:posList>50 20 51 20 51 21</gml:posList></gmlce:SimpleTriangle>",
        &FixedAxis::swap(),
    )
    .expect("a SimpleTriangle");
    assert_wkt(
        &crate::support::to_g(&swapped.geometry.expect("a geometry")),
        "POLYGON ((20 50,20 51,21 51,20 50))",
    );
}

// ------------------------------------------------------------ out of scope

#[test]
fn triangulated_surfaces_and_referenceable_grids_are_unsupported() {
    // Like `gml:TriangulatedSurface`, `gml:Tin` and `gml:Grid` (support
    // matrix §5.1): geometry errors, handled by `OnFeatureError`.
    let triangle = "<gmltin:SimpleTrianglePatch><gml:posList>0 0 0 1 1 1</gml:posList></gmltin:SimpleTrianglePatch>";
    for snippet in [
        format!("<gmltin:TriangulatedSurface><gml:patches>{triangle}</gml:patches></gmltin:TriangulatedSurface>"),
        format!("<gmltin:TIN><gml:patches>{triangle}</gml:patches></gmltin:TIN>"),
        "<gml:MultiGeometry><gml:geometryMember><gmltin:TIN/></gml:geometryMember></gml:MultiGeometry>".to_string(),
        concat!(
            r#"<gmlrgrid:ReferenceableGridByVectors xmlns:gmlrgrid="http://www.opengis.net/gml/3.3/rgrid" dimension="2">"#,
            "</gmlrgrid:ReferenceableGridByVectors>"
        )
        .to_string(),
    ] {
        let error = parse_gml33(&snippet).expect_err("unsupported");
        assert!(matches!(error, Error::Unsupported { .. }), "{snippet}: got {error:?}");
    }
}
