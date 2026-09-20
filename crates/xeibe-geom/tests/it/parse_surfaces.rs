//! `Polygon`, `Surface` and its patches, `OrientableSurface`,
//! `CompositeSurface`, and the `unsupported_geometry` policy
//! (`docs/geometry.md`, "Mapping table", "Unsupported geometry").

use xeibe_geom::model::GeomKind;
use xeibe_geom::{Error, GeometryOptions, options::UnsupportedGeometry};
use xeibe_testkit::wkt::assert_wkt;

use crate::support::{FixedAxis, assert_geometry, g31, parse, parse_with, to_g};

const RING: &str = "<gml:LinearRing><gml:posList>0 0 4 0 4 4 0 4 0 0</gml:posList></gml:LinearRing>";
const HOLE: &str = "<gml:LinearRing><gml:posList>1 1 2 1 2 2 1 2 1 1</gml:posList></gml:LinearRing>";

#[test]
fn polygon_with_exterior_and_interior_rings() {
    let snippet = format!(
        "<gml:Polygon><gml:exterior>{RING}</gml:exterior><gml:interior>{HOLE}</gml:interior></gml:Polygon>"
    );
    assert_geometry(
        &snippet,
        "POLYGON ((0 0,4 0,4 4,0 4,0 0),(1 1,2 1,2 2,1 2,1 1))",
    );
}

#[test]
fn the_gml_2_boundary_spellings_are_accepted() {
    // `outerBoundaryIs`/`innerBoundaryIs` (GML 2, deprecated in 3.x).
    let snippet = concat!(
        "<gml:Polygon><gml:outerBoundaryIs><gml:LinearRing>",
        "<gml:coordinates>0,0 4,0 4,4 0,4 0,0</gml:coordinates>",
        "</gml:LinearRing></gml:outerBoundaryIs><gml:innerBoundaryIs><gml:LinearRing>",
        "<gml:coordinates>1,1 2,1 2,2 1,2 1,1</gml:coordinates>",
        "</gml:LinearRing></gml:innerBoundaryIs></gml:Polygon>"
    );
    assert_wkt(
        &g31(snippet),
        "POLYGON ((0 0,4 0,4 4,0 4,0 0),(1 1,2 1,2 2,1 2,1 1))",
    );
}

#[test]
fn a_surface_with_one_patch_is_a_polygon() {
    let snippet = format!(
        "<gml:Surface><gml:patches><gml:PolygonPatch><gml:exterior>{RING}</gml:exterior>\
         </gml:PolygonPatch></gml:patches></gml:Surface>"
    );
    assert_geometry(&snippet, "POLYGON ((0 0,4 0,4 4,0 4,0 0))");
    assert_eq!(parse(&snippet).unwrap().source_kind, GeomKind::Surface);
}

#[test]
fn a_surface_with_several_patches_keeps_them_as_parts() {
    // The patches are connected but are not dissolved (§10.5.10).
    let second = "<gml:LinearRing><gml:posList>4 0 8 0 8 4 4 4 4 0</gml:posList></gml:LinearRing>";
    let snippet = format!(
        "<gml:Surface><gml:patches>\
         <gml:PolygonPatch><gml:exterior>{RING}</gml:exterior></gml:PolygonPatch>\
         <gml:PolygonPatch><gml:exterior>{second}</gml:exterior></gml:PolygonPatch>\
         </gml:patches></gml:Surface>"
    );
    assert_geometry(
        &snippet,
        "MULTIPOLYGON (((0 0,4 0,4 4,0 4,0 0)),((4 0,8 0,8 4,4 4,4 0)))",
    );
}

#[test]
fn triangle_and_rectangle_patches_are_polygons() {
    let triangle = concat!(
        "<gml:Surface><gml:patches><gml:Triangle><gml:exterior>",
        "<gml:LinearRing><gml:posList>0 0 0 1 1 1 0 0</gml:posList></gml:LinearRing>",
        "</gml:exterior></gml:Triangle></gml:patches></gml:Surface>"
    );
    assert_geometry(triangle, "POLYGON ((0 0,0 1,1 1,0 0))");

    let rectangle = concat!(
        "<gml:Surface><gml:patches><gml:Rectangle><gml:exterior>",
        "<gml:LinearRing><gml:posList>0 0 0 1 1 1 1 0 0 0</gml:posList></gml:LinearRing>",
        "</gml:exterior></gml:Rectangle></gml:patches></gml:Surface>"
    );
    assert_geometry(rectangle, "POLYGON ((0 0,0 1,1 1,1 0,0 0))");
}

#[test]
fn a_patch_with_a_curved_ring_is_a_curve_polygon() {
    let snippet = concat!(
        "<gml:Surface><gml:patches><gml:PolygonPatch><gml:exterior><gml:Ring>",
        "<gml:curveMember><gml:Curve><gml:segments>",
        "<gml:Arc><gml:posList>0 0 1 1 2 0</gml:posList></gml:Arc>",
        "<gml:LineStringSegment><gml:posList>2 0 0 0</gml:posList></gml:LineStringSegment>",
        "</gml:segments></gml:Curve></gml:curveMember>",
        "</gml:Ring></gml:exterior></gml:PolygonPatch></gml:patches></gml:Surface>"
    );
    assert_geometry(
        snippet,
        "CURVEPOLYGON (COMPOUNDCURVE (CIRCULARSTRING (0 0,1 1,2 0),(2 0,0 0)))",
    );
}

#[test]
fn orientable_surface_reverses_its_rings() {
    let base = format!(
        "<gml:baseSurface><gml:Polygon><gml:exterior>{RING}</gml:exterior></gml:Polygon></gml:baseSurface>"
    );
    assert_geometry(
        &format!(r#"<gml:OrientableSurface orientation="+">{base}</gml:OrientableSurface>"#),
        "POLYGON ((0 0,4 0,4 4,0 4,0 0))",
    );
    assert_geometry(
        &format!(r#"<gml:OrientableSurface orientation="-">{base}</gml:OrientableSurface>"#),
        "POLYGON ((0 0,0 4,4 4,4 0,0 0))",
    );
}

#[test]
fn an_orientable_surface_without_a_base_surface_is_an_error() {
    assert!(parse("<gml:OrientableSurface/>").is_err());
    assert!(parse("<gml:OrientableSurface><gml:baseSurface/></gml:OrientableSurface>").is_err());
}

#[test]
fn composite_surface_keeps_its_members() {
    let second = "<gml:LinearRing><gml:posList>4 0 8 0 8 4 4 4 4 0</gml:posList></gml:LinearRing>";
    let snippet = format!(
        "<gml:CompositeSurface>\
         <gml:surfaceMember><gml:Polygon><gml:exterior>{RING}</gml:exterior></gml:Polygon></gml:surfaceMember>\
         <gml:surfaceMember><gml:Polygon><gml:exterior>{second}</gml:exterior></gml:Polygon></gml:surfaceMember>\
         </gml:CompositeSurface>"
    );
    assert_geometry(
        &snippet,
        "MULTIPOLYGON (((0 0,4 0,4 4,0 4,0 0)),((4 0,8 0,8 4,4 4,4 0)))",
    );
}

#[test]
fn an_empty_polygon_is_an_empty_geometry() {
    assert_geometry("<gml:Polygon/>", "POLYGON EMPTY");
    assert_geometry("<gml:Surface><gml:patches/></gml:Surface>", "POLYGON EMPTY");
}

#[test]
fn a_polygon_with_only_interior_rings_follows_the_unsupported_policy() {
    // Allowed by the standard (§10.5.5), not representable in WKB.
    let snippet = format!("<gml:Polygon><gml:interior>{HOLE}</gml:interior></gml:Polygon>");
    let error = parse(&snippet).expect_err("the default policy is Error");
    assert!(matches!(error, Error::Unsupported { .. }), "got {error:?}");

    let null = GeometryOptions {
        unsupported_geometry: UnsupportedGeometry::Null,
        ..GeometryOptions::default()
    };
    let parsed = parse_with(&snippet, &null, &FixedAxis::no_swap()).expect("null geometry");
    assert!(parsed.geometry.is_none());
}

#[test]
fn solids_and_triangulated_surfaces_are_unsupported() {
    // Out of scope (support matrix §5.1): no CityGML, no 3D solids.
    for snippet in [
        concat!(
            "<gml:Solid><gml:exterior><gml:CompositeSurface><gml:surfaceMember>",
            "<gml:Polygon><gml:exterior><gml:LinearRing>",
            "<gml:posList srsDimension=\"3\">1 2 0 3 4 0 5 6 0 1 2 0</gml:posList>",
            "</gml:LinearRing></gml:exterior></gml:Polygon></gml:surfaceMember>",
            "</gml:CompositeSurface></gml:exterior></gml:Solid>"
        ),
        concat!(
            "<gml:TriangulatedSurface><gml:patches><gml:Triangle><gml:exterior>",
            "<gml:LinearRing><gml:posList srsDimension=\"3\">0 0 0 0 0 1 0 1 0 0 0 0</gml:posList>",
            "</gml:LinearRing></gml:exterior></gml:Triangle></gml:patches></gml:TriangulatedSurface>"
        ),
        concat!(
            "<gml:PolyhedralSurface><gml:polygonPatches><gml:PolygonPatch><gml:exterior>",
            "<gml:LinearRing><gml:posList srsDimension=\"3\">1 2 3 4 5 6 7 8 9 1 2 3</gml:posList>",
            "</gml:LinearRing></gml:exterior></gml:PolygonPatch></gml:polygonPatches></gml:PolyhedralSurface>"
        ),
    ] {
        let error = parse(snippet).expect_err("unsupported");
        assert!(matches!(error, Error::Unsupported { .. }), "got {error:?}");
    }
}

#[test]
fn the_raw_xml_policy_keeps_the_source_of_unsupported_geometry() {
    let snippet = concat!(
        "<gml:Tin><gml:patches><gml:Triangle><gml:exterior><gml:LinearRing>",
        "<gml:posList srsDimension=\"3\">0 0 1 0 1 1 1 1 1 0 0 1</gml:posList>",
        "</gml:LinearRing></gml:exterior></gml:Triangle></gml:patches></gml:Tin>"
    );
    let options = GeometryOptions {
        unsupported_geometry: UnsupportedGeometry::RawXml,
        ..GeometryOptions::default()
    };
    let parsed = parse_with(snippet, &options, &FixedAxis::no_swap()).expect("raw XML");
    assert!(parsed.geometry.is_none());
    assert_eq!(parsed.source_kind, GeomKind::Unsupported);
    let raw = parsed.raw_xml.expect("the source XML is kept");
    assert!(raw.contains("Triangle"), "{raw:?}");
}

#[test]
fn a_3d_polygon_keeps_its_z_ordinates() {
    let snippet = concat!(
        "<gml:Polygon><gml:exterior><gml:LinearRing>",
        "<gml:posList srsDimension=\"3\">0 0 8 1 0 8 1 1 8 0 0 8</gml:posList>",
        "</gml:LinearRing></gml:exterior></gml:Polygon>"
    );
    assert_wkt(
        &to_g(&crate::support::geometry(snippet)),
        "POLYGON Z ((0 0 8,1 0 8,1 1 8,0 0 8))",
    );
}
