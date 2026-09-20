//! Aggregates: `MultiPoint`, `MultiLineString`/`MultiCurve`,
//! `MultiPolygon`/`MultiSurface`, `MultiGeometry` — in both the singular and
//! the plural member spellings (`docs/geometry.md`, support matrix §5.3).

use xeibe_geom::model::GeomKind;
use xeibe_testkit::wkt::assert_wkt;

use crate::support::{assert_geometry, g31, parse};

#[test]
fn multi_point_in_both_member_spellings() {
    assert_geometry(
        concat!(
            "<gml:MultiPoint>",
            "<gml:pointMember><gml:Point><gml:pos>1 2</gml:pos></gml:Point></gml:pointMember>",
            "<gml:pointMember><gml:Point><gml:pos>3 4</gml:pos></gml:Point></gml:pointMember>",
            "</gml:MultiPoint>"
        ),
        "MULTIPOINT ((1 2),(3 4))",
    );
    assert_geometry(
        concat!(
            "<gml:MultiPoint><gml:pointMembers>",
            "<gml:Point><gml:pos>1 2</gml:pos></gml:Point>",
            "<gml:Point><gml:pos>3 4</gml:pos></gml:Point>",
            "</gml:pointMembers></gml:MultiPoint>"
        ),
        "MULTIPOINT ((1 2),(3 4))",
    );
}

#[test]
fn multi_line_string_is_read_in_every_version() {
    // GML 2/3.1 `MultiLineString` is deprecated in 3.1 and gone in 3.2, but
    // real 3.2 files still use it (lenient reading).
    let snippet = concat!(
        "<gml:MultiLineString>",
        "<gml:lineStringMember><gml:LineString><gml:coordinates>0,0 1,1</gml:coordinates>",
        "</gml:LineString></gml:lineStringMember>",
        "<gml:lineStringMember><gml:LineString><gml:coordinates>2,2 3,3</gml:coordinates>",
        "</gml:LineString></gml:lineStringMember></gml:MultiLineString>"
    );
    assert_wkt(&g31(snippet), "MULTILINESTRING ((0 0,1 1),(2 2,3 3))");
    assert_eq!(
        crate::support::parse_gml31(snippet).unwrap().source_kind,
        GeomKind::MultiLineString
    );
}

#[test]
fn multi_curve_is_a_multi_line_string_while_it_has_no_curves() {
    assert_geometry(
        concat!(
            "<gml:MultiCurve><gml:curveMember>",
            "<gml:LineString><gml:posList>0 0 1 1</gml:posList></gml:LineString>",
            "</gml:curveMember></gml:MultiCurve>"
        ),
        "MULTILINESTRING ((0 0,1 1))",
    );
    // With an arc it stays a MultiCurve.
    assert_geometry(
        concat!(
            "<gml:MultiCurve><gml:curveMembers><gml:Curve><gml:segments>",
            "<gml:Arc><gml:posList>0 0 1 1 2 0</gml:posList></gml:Arc>",
            "</gml:segments></gml:Curve></gml:curveMembers></gml:MultiCurve>"
        ),
        "MULTICURVE (CIRCULARSTRING (0 0,1 1,2 0))",
    );
}

#[test]
fn multi_polygon_and_multi_surface() {
    let ring = "<gml:LinearRing><gml:posList>0 0 1 0 1 1 0 0</gml:posList></gml:LinearRing>";
    assert_geometry(
        &format!(
            "<gml:MultiSurface><gml:surfaceMember><gml:Polygon><gml:exterior>{ring}</gml:exterior>\
             </gml:Polygon></gml:surfaceMember></gml:MultiSurface>"
        ),
        "MULTIPOLYGON (((0 0,1 0,1 1,0 0)))",
    );
    assert_wkt(
        &g31(concat!(
            "<gml:MultiPolygon><gml:polygonMember><gml:Polygon><gml:outerBoundaryIs>",
            "<gml:LinearRing><gml:coordinates>0,0 1,0 1,1 0,0</gml:coordinates></gml:LinearRing>",
            "</gml:outerBoundaryIs></gml:Polygon></gml:polygonMember></gml:MultiPolygon>"
        )),
        "MULTIPOLYGON (((0 0,1 0,1 1,0 0)))",
    );
}

#[test]
fn a_multi_surface_with_a_curved_member_stays_a_multi_surface() {
    let snippet = concat!(
        "<gml:MultiSurface><gml:surfaceMember><gml:Polygon><gml:exterior><gml:Ring>",
        "<gml:curveMember><gml:Curve><gml:segments>",
        "<gml:Circle><gml:posList>0 0 1 1 1 -1</gml:posList></gml:Circle>",
        "</gml:segments></gml:Curve></gml:curveMember>",
        "</gml:Ring></gml:exterior></gml:Polygon></gml:surfaceMember></gml:MultiSurface>"
    );
    let geometry = crate::support::g(snippet);
    assert_eq!(geometry.tag(), "MULTISURFACE", "{geometry}");
}

#[test]
fn multi_geometry_is_a_geometry_collection() {
    assert_geometry(
        concat!(
            "<gml:MultiGeometry>",
            "<gml:geometryMember><gml:Point><gml:pos>0 1</gml:pos></gml:Point></gml:geometryMember>",
            "<gml:geometryMember><gml:LineString><gml:posList>2 3 4 5</gml:posList>",
            "</gml:LineString></gml:geometryMember></gml:MultiGeometry>"
        ),
        "GEOMETRYCOLLECTION (POINT (0 1),LINESTRING (2 3,4 5))",
    );
    assert_geometry(
        concat!(
            "<gml:MultiGeometry><gml:geometryMembers>",
            "<gml:LineString><gml:posList>0 0 1 1</gml:posList></gml:LineString>",
            "</gml:geometryMembers></gml:MultiGeometry>"
        ),
        "GEOMETRYCOLLECTION (LINESTRING (0 0,1 1))",
    );
}

#[test]
fn empty_aggregates_are_empty_geometries() {
    assert_geometry("<gml:MultiPoint/>", "MULTIPOINT EMPTY");
    assert_geometry("<gml:MultiSurface/>", "MULTIPOLYGON EMPTY");
    assert_geometry("<gml:MultiGeometry/>", "GEOMETRYCOLLECTION EMPTY");
}

#[test]
fn a_member_of_the_wrong_kind_is_an_error() {
    assert!(
        parse(concat!(
            "<gml:MultiPoint><gml:pointMember><gml:LineString>",
            "<gml:posList>0 1 2 3</gml:posList></gml:LineString></gml:pointMember></gml:MultiPoint>"
        ))
        .is_err()
    );
    assert!(
        parse(concat!(
            "<gml:MultiCurve><gml:curveMember><gml:Point><gml:pos>0 1</gml:pos>",
            "</gml:Point></gml:curveMember></gml:MultiCurve>"
        ))
        .is_err()
    );
}

#[test]
fn a_member_given_by_reference_is_not_resolved() {
    let error = parse(
        r##"<gml:MultiCurve><gml:curveMember xlink:href="#c1"/></gml:MultiCurve>"##,
    )
    .expect_err("a referenced member");
    assert!(
        matches!(
            error,
            xeibe_geom::Error::ByReference { .. } | xeibe_geom::Error::Unsupported { .. }
        ),
        "got {error:?}"
    );
}
