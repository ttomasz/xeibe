//! One square, many encodings: the variants of Even Rouault's "GML madness"
//! (<https://erouault.blogspot.com/2014/04/gml-madness.html>), which lists
//! about 25 schema-valid GML 3.2 ways to write the same unit square. The
//! snippets here are our own encodings of each structure, numbered as in the
//! post.
//!
//! Every variant that stays within the design gives the same polygon, or the
//! same polygon split into parts where the source splits it (patches and
//! composite members are kept, not dissolved; `docs/geometry.md`, "Mapping
//! table"). A bare surface patch, not valid GML, is read as GDAL reads it.
//! `xlink:href` points (no resolution) and DTD entities (support matrix §1)
//! are errors by design.

use xeibe_core::Dialect;
use xeibe_geom::Error;
use xeibe_geom::GeometryOptions;
use xeibe_geom::model::GeomKind;
use xeibe_geom::parse::{MAX_DEPTH, ParseContext};

use crate::support::{FixedAxis, assert_geometry, parse, parse_in, warnings};

/// The square, drawn clockwise from the origin as in the post.
const SQUARE: &str = "POLYGON ((0 0,0 1,1 1,1 0,0 0))";
const MULTI_SQUARE: &str = "MULTIPOLYGON (((0 0,0 1,1 1,1 0,0 0)))";

/// `Polygon` with the given exterior boundary.
fn polygon(exterior: &str) -> String {
    format!("<gml:Polygon><gml:exterior>{exterior}</gml:exterior></gml:Polygon>")
}

/// `Polygon` whose exterior is a `Ring` of the given curve members.
fn ring_polygon(members: &[&str]) -> String {
    let members: String = members
        .iter()
        .map(|member| format!("<gml:curveMember>{member}</gml:curveMember>"))
        .collect();
    polygon(&format!("<gml:Ring>{members}</gml:Ring>"))
}

fn line(pos_list: &str) -> String {
    format!("<gml:LineString><gml:posList>{pos_list}</gml:posList></gml:LineString>")
}

const RING: &str =
    "<gml:LinearRing><gml:posList>0 0 0 1 1 1 1 0 0 0</gml:posList></gml:LinearRing>";

/// A variant must be a clean parse: the square, and no warnings (no gaps
/// bridged, no ring closed).
#[track_caller]
fn assert_square(snippet: &str, expected: &str) {
    assert_geometry(snippet, expected);
    assert_eq!(warnings(snippet), Vec::<String>::new(), "{snippet}");
}

#[track_caller]
fn assert_by_reference(snippet: &str) {
    let error = parse(snippet).expect_err("a point given by reference");
    assert!(
        matches!(error, Error::ByReference { .. } | Error::Unsupported { .. }),
        "got {error:?}"
    );
}

// ------------------------------------------- 1–5: LinearRing and its carriers

#[test]
fn v01_linear_ring_with_a_pos_list() {
    assert_square(&polygon(RING), SQUARE);
}

#[test]
fn v02_linear_ring_with_one_pos_per_position() {
    let ring = concat!(
        "<gml:LinearRing><gml:pos>0 0</gml:pos><gml:pos>0 1</gml:pos>",
        "<gml:pos>1 1</gml:pos><gml:pos>1 0</gml:pos><gml:pos>0 0</gml:pos></gml:LinearRing>"
    );
    assert_square(&polygon(ring), SQUARE);
}

#[test]
fn v03_linear_ring_of_inline_point_properties() {
    let points: String = ["0 0", "0 1", "1 1", "1 0", "0 0"]
        .iter()
        .map(|pos| {
            format!("<gml:pointProperty><gml:Point><gml:pos>{pos}</gml:pos></gml:Point></gml:pointProperty>")
        })
        .collect();
    assert_square(&polygon(&format!("<gml:LinearRing>{points}</gml:LinearRing>")), SQUARE);
}

#[test]
fn v04_the_closing_point_given_by_reference_is_not_resolved() {
    // The ring closes on the first point by `xlink:href`. Points by reference
    // are not planned (support matrix §5.3), even back to the same geometry.
    let ring = concat!(
        r#"<gml:LinearRing><gml:pointProperty><gml:Point gml:id="p0"><gml:pos>0 0</gml:pos></gml:Point></gml:pointProperty>"#,
        "<gml:pos>0 1</gml:pos><gml:pos>1 1</gml:pos><gml:pos>1 0</gml:pos>",
        r##"<gml:pointProperty xlink:href="#p0"/></gml:LinearRing>"##
    );
    assert_by_reference(&polygon(ring));
}

#[test]
fn v05_linear_ring_with_gml_2_coordinates() {
    // `coordinates` is deprecated in 3.2 but still read in every version.
    let ring = "<gml:LinearRing><gml:coordinates>0,0 0,1 1,1 1,0 0,0</gml:coordinates></gml:LinearRing>";
    assert_square(&polygon(ring), SQUARE);
    // A deprecated carrier inside `exterior` doesn't make the geometry GML 2
    // (`docs/geometry.md`, "Axis order": the dialect table).
    let parsed = parse(&polygon(ring)).unwrap();
    assert_eq!(parsed.dialect, Dialect::Gml3);
}

// ------------------------------------------------ 6: a bare Rectangle patch

#[test]
fn v06_a_rectangle_patch_on_its_own_is_read_as_a_polygon() {
    // `Rectangle` is a surface patch, not a geometry (its substitution group is
    // `AbstractSurfacePatch`), so it can't be a property value in schema-valid
    // GML. GDAL reads a bare patch as a polygon, and so do we.
    let snippet = format!("<gml:Rectangle><gml:exterior>{RING}</gml:exterior></gml:Rectangle>");
    assert_square(&snippet, SQUARE);
    assert_eq!(parse(&snippet).unwrap().source_kind, GeomKind::Patch);

    // The other patches too. GDAL makes the triangle a `TRIANGLE`; we have no
    // such type, and a `Triangle` inside a `Surface` is a polygon already.
    let patch = format!("<gml:PolygonPatch><gml:exterior>{RING}</gml:exterior></gml:PolygonPatch>");
    assert_square(&patch, SQUARE);
    let triangle = concat!(
        "<gml:Triangle><gml:exterior><gml:LinearRing><gml:posList>0 0 0 1 1 1 0 0</gml:posList>",
        "</gml:LinearRing></gml:exterior></gml:Triangle>"
    );
    assert_square(triangle, "POLYGON ((0 0,0 1,1 1,0 0))");
}

// ---------------------------------------- 7–16: Ring made of curve members

#[test]
fn v07_ring_of_one_line_string() {
    assert_square(&ring_polygon(&[&line("0 0 0 1 1 1 1 0 0 0")]), SQUARE);
}

#[test]
fn v08_ring_of_one_line_string_with_pos() {
    let line = concat!(
        "<gml:LineString><gml:pos>0 0</gml:pos><gml:pos>0 1</gml:pos><gml:pos>1 1</gml:pos>",
        "<gml:pos>1 0</gml:pos><gml:pos>0 0</gml:pos></gml:LineString>"
    );
    assert_square(&ring_polygon(&[line]), SQUARE);
}

#[test]
fn v09_ring_of_one_line_string_per_edge() {
    let edges = [line("0 0 0 1"), line("0 1 1 1"), line("1 1 1 0"), line("1 0 0 0")];
    let edges: Vec<&str> = edges.iter().map(String::as_str).collect();
    assert_square(&ring_polygon(&edges), SQUARE);
}

#[test]
fn v10_ring_of_a_curve_with_one_line_string_segment() {
    let curve = concat!(
        "<gml:Curve><gml:segments><gml:LineStringSegment>",
        "<gml:posList>0 0 0 1 1 1 1 0 0 0</gml:posList>",
        "</gml:LineStringSegment></gml:segments></gml:Curve>"
    );
    assert_square(&ring_polygon(&[curve]), SQUARE);
}

#[test]
fn v11_ring_of_a_curve_with_one_segment_per_edge() {
    let segments: String = ["0 0 0 1", "0 1 1 1", "1 1 1 0", "1 0 0 0"]
        .iter()
        .map(|edge| {
            format!("<gml:LineStringSegment><gml:posList>{edge}</gml:posList></gml:LineStringSegment>")
        })
        .collect();
    let curve = format!("<gml:Curve><gml:segments>{segments}</gml:segments></gml:Curve>");
    assert_square(&ring_polygon(&[&curve]), SQUARE);
}

#[test]
fn v12_segments_sharing_points_by_reference_are_not_resolved() {
    // Each segment starts at the previous one's end point, by `xlink:href`.
    let curve = concat!(
        "<gml:Curve><gml:segments>",
        "<gml:LineStringSegment>",
        r#"<gml:pointProperty><gml:Point gml:id="c0"><gml:pos>0 0</gml:pos></gml:Point></gml:pointProperty>"#,
        r#"<gml:pointProperty><gml:Point gml:id="c1"><gml:pos>0 1</gml:pos></gml:Point></gml:pointProperty>"#,
        "</gml:LineStringSegment>",
        "<gml:LineStringSegment>",
        r##"<gml:pointProperty xlink:href="#c1"/>"##,
        r#"<gml:pointProperty><gml:Point gml:id="c2"><gml:pos>1 1</gml:pos></gml:Point></gml:pointProperty>"#,
        "</gml:LineStringSegment>",
        "<gml:LineStringSegment>",
        r##"<gml:pointProperty xlink:href="#c2"/>"##,
        r#"<gml:pointProperty><gml:Point gml:id="c3"><gml:pos>1 0</gml:pos></gml:Point></gml:pointProperty>"#,
        "</gml:LineStringSegment>",
        "<gml:LineStringSegment>",
        r##"<gml:pointProperty xlink:href="#c3"/><gml:pointProperty xlink:href="#c0"/>"##,
        "</gml:LineStringSegment>",
        "</gml:segments></gml:Curve>"
    );
    assert_by_reference(&ring_polygon(&[curve]));
}

#[test]
fn v13_ring_of_a_composite_curve() {
    let composite = format!(
        "<gml:CompositeCurve><gml:curveMember>{}</gml:curveMember></gml:CompositeCurve>",
        line("0 0 0 1 1 1 1 0 0 0")
    );
    assert_square(&ring_polygon(&[&composite]), SQUARE);
}

#[test]
fn v14_ring_of_nested_composite_curves() {
    // Composites of composites, 32 levels deep (well within `MAX_DEPTH`), and
    // a second member, itself nested, that closes the square.
    let mut composite = line("0 0 0 1 1 1");
    for _ in 0..32 {
        composite = format!(
            "<gml:CompositeCurve><gml:curveMember>{composite}</gml:curveMember></gml:CompositeCurve>"
        );
    }
    let closing = format!(
        "<gml:CompositeCurve><gml:curveMember>{}</gml:curveMember>\
         <gml:curveMember><gml:CompositeCurve><gml:curveMember>{}</gml:curveMember>\
         </gml:CompositeCurve></gml:curveMember></gml:CompositeCurve>",
        line("1 1 1 0"),
        line("1 0 0 0"),
    );
    assert_square(&ring_polygon(&[&composite, &closing]), SQUARE);
}

#[test]
fn nesting_deeper_than_the_limit_is_a_geometry_error_not_a_crash() {
    // The parser recurses into members, so unbounded nesting would overflow
    // the stack and abort the process. Past `MAX_DEPTH` open elements the
    // geometry is invalid, and the reader ends up after it as for any error.
    let mut composite = line("0 0 0 1 1 1 1 0 0 0");
    for _ in 0..10_000 {
        composite = format!(
            "<gml:CompositeCurve><gml:curveMember>{composite}</gml:curveMember></gml:CompositeCurve>"
        );
    }
    let error = parse(&composite).expect_err("too deep");
    assert!(matches!(error, Error::InvalidGeometry { .. }), "got {error:?}");

    // Just within the limit still parses: 2 elements per level, plus the
    // line string and its `posList`.
    let mut composite = line("0 0 0 1 1 1 1 0 0 0");
    for _ in 0..(MAX_DEPTH - 2) / 2 {
        composite = format!(
            "<gml:CompositeCurve><gml:curveMember>{composite}</gml:curveMember></gml:CompositeCurve>"
        );
    }
    assert_geometry(&composite, "LINESTRING (0 0,0 1,1 1,1 0,0 0)");
}

#[test]
fn v15_ring_of_an_orientable_curve_with_the_default_orientation() {
    // `orientation` defaults to "+".
    let orientable = format!(
        "<gml:OrientableCurve><gml:baseCurve>{}</gml:baseCurve></gml:OrientableCurve>",
        line("0 0 0 1 1 1 1 0 0 0")
    );
    assert_square(&ring_polygon(&[&orientable]), SQUARE);
}

#[test]
fn v16_ring_closed_by_a_reversed_orientable_curve() {
    // The second half is written from the origin and reversed, so it continues
    // from (1 1) back to the origin.
    let reversed = format!(
        r#"<gml:OrientableCurve orientation="-"><gml:baseCurve>{}</gml:baseCurve></gml:OrientableCurve>"#,
        line("0 0 1 0 1 1")
    );
    assert_square(&ring_polygon(&[&line("0 0 0 1 1 1"), &reversed]), SQUARE);
}

// ------------------------------------------------ 17–18: Surface and patches

#[test]
fn v17_surface_with_a_polygon_patch() {
    let snippet = format!(
        "<gml:Surface><gml:patches><gml:PolygonPatch><gml:exterior>{RING}</gml:exterior>\
         </gml:PolygonPatch></gml:patches></gml:Surface>"
    );
    assert_square(&snippet, SQUARE);
    assert_eq!(parse(&snippet).unwrap().source_kind, GeomKind::Surface);
}

#[test]
fn v18_surface_with_a_rectangle_patch() {
    let snippet = format!(
        "<gml:Surface><gml:patches><gml:Rectangle><gml:exterior>{RING}</gml:exterior>\
         </gml:Rectangle></gml:patches></gml:Surface>"
    );
    assert_square(&snippet, SQUARE);
}

// ------------------------------------- 19–23: composite and multi surfaces

fn patch_surface(pos_list: &str) -> String {
    format!(
        "<gml:Surface><gml:patches><gml:PolygonPatch><gml:exterior><gml:LinearRing>\
         <gml:posList>{pos_list}</gml:posList></gml:LinearRing></gml:exterior>\
         </gml:PolygonPatch></gml:patches></gml:Surface>"
    )
}

#[test]
fn v19_composite_surface_of_one_surface() {
    // A composite is always a Multi geometry, also with one member.
    let snippet = format!(
        "<gml:CompositeSurface><gml:surfaceMember>{}</gml:surfaceMember></gml:CompositeSurface>",
        patch_surface("0 0 0 1 1 1 1 0 0 0")
    );
    assert_square(&snippet, MULTI_SQUARE);
}

#[test]
fn v20_composite_surface_of_two_triangles() {
    // Split along the diagonal. The members are kept, not dissolved.
    let snippet = format!(
        "<gml:CompositeSurface><gml:surfaceMember>{}</gml:surfaceMember>\
         <gml:surfaceMember>{}</gml:surfaceMember></gml:CompositeSurface>",
        patch_surface("0 0 0 1 1 1 0 0"),
        patch_surface("0 0 1 1 1 0 0 0"),
    );
    assert_square(
        &snippet,
        "MULTIPOLYGON (((0 0,0 1,1 1,0 0)),((0 0,1 1,1 0,0 0)))",
    );
}

#[test]
fn v21_multi_surface_with_surface_member() {
    let snippet = format!(
        "<gml:MultiSurface><gml:surfaceMember>{}</gml:surfaceMember></gml:MultiSurface>",
        patch_surface("0 0 0 1 1 1 1 0 0 0")
    );
    assert_square(&snippet, MULTI_SQUARE);
}

#[test]
fn v22_multi_surface_with_surface_members() {
    let snippet = format!(
        "<gml:MultiSurface><gml:surfaceMembers>{}</gml:surfaceMembers></gml:MultiSurface>",
        patch_surface("0 0 0 1 1 1 1 0 0 0")
    );
    assert_square(&snippet, MULTI_SQUARE);
}

#[test]
fn v23_nested_composite_surfaces_are_flattened() {
    // A MultiPolygon can't hold a MultiPolygon, so the inner composite's
    // members become members of the outer one.
    let inner = format!(
        "<gml:CompositeSurface><gml:surfaceMember>{}</gml:surfaceMember></gml:CompositeSurface>",
        patch_surface("0 0 0 1 1 1 1 0 0 0")
    );
    let snippet = format!(
        "<gml:CompositeSurface><gml:surfaceMember>{inner}</gml:surfaceMember></gml:CompositeSurface>"
    );
    assert_square(&snippet, MULTI_SQUARE);

    // With a sibling, both levels' members end up side by side.
    let split = format!(
        "<gml:CompositeSurface><gml:surfaceMember>{}</gml:surfaceMember></gml:CompositeSurface>",
        patch_surface("0 0 0 1 1 1 0 0")
    );
    let snippet = format!(
        "<gml:CompositeSurface><gml:surfaceMember>{split}</gml:surfaceMember>\
         <gml:surfaceMember>{}</gml:surfaceMember></gml:CompositeSurface>",
        patch_surface("0 0 1 1 1 0 0 0")
    );
    assert_square(
        &snippet,
        "MULTIPOLYGON (((0 0,0 1,1 1,0 0)),((0 0,1 1,1 0,0 0)))",
    );
}

// --------------------------------------- 24: GML 3.3 compact encoding

#[test]
fn v24_gml_33_simple_rectangle() {
    // GML 3.3 compact encoding (OGC 10-129r1 §10.3): four corners, the ring's
    // closing position implied. GDAL reads it as the closed polygon.
    let snippet = concat!(
        r#"<gmlce:SimpleRectangle xmlns:gmlce="http://www.opengis.net/gml/3.3/ce">"#,
        "<gml:posList>0 0 0 1 1 1 1 0</gml:posList></gmlce:SimpleRectangle>"
    );
    assert_square(snippet, SQUARE);
}

// ------------------------------------------ 25: coordinates from entities

#[test]
fn v25_coordinates_from_dtd_entities_are_rejected() {
    // Documents with a DTD are rejected outright (XXE, billion laughs; support
    // matrix §1), so internal entities never expand either.
    let document = concat!(
        "<!DOCTYPE g [<!ENTITY pt0 \"0 0\"><!ENTITY pt1 \"0 1\">",
        "<!ENTITY pt2 \"1 1\"><!ENTITY pt3 \"1 0\">]>",
        r#"<g xmlns:gml="http://www.opengis.net/gml/3.2">"#,
        "<gml:Polygon><gml:exterior><gml:LinearRing>",
        "<gml:posList>&pt0; &pt1; &pt2; &pt3; &pt0;</gml:posList>",
        "</gml:LinearRing></gml:exterior></gml:Polygon></g>"
    );
    let error = parse_in(
        document,
        &GeometryOptions::default(),
        &FixedAxis::no_swap(),
        &ParseContext::default(),
    )
    .expect_err("a DTD");
    assert!(
        matches!(error, Error::Core(xeibe_core::Error::DtdNotSupported(_))),
        "got {error:?}"
    );
}
