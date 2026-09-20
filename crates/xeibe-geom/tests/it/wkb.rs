//! The ISO WKB writer (`docs/geometry.md`, "Column encoding", "Curves").
//!
//! The bytes are checked against the testkit's independent reader, plus a few
//! exact type codes: ISO codes are `dimension * 1000 + base`, and the curve
//! types (8…12) are what makes the output ISO rather than OGC WKB.

use xeibe_geom::model::Geometry;
use xeibe_geom::wkb::{Endianness, wkb_size, write_wkb};
use xeibe_testkit::wkb as oracle;
use xeibe_testkit::wkt::{Tol, assert_wkt, assert_wkt_tol};

use crate::support::geometry;

fn bytes(geometry: &Geometry) -> Vec<u8> {
    let mut out = Vec::new();
    write_wkb(geometry, Endianness::Little, &mut out);
    assert_eq!(
        wkb_size(geometry),
        out.len(),
        "wkb_size must match what is written"
    );
    out
}

#[track_caller]
fn round_trip(snippet: &str, expected_wkt: &str, expected_code: u32) {
    let geometry = geometry(snippet);
    let encoded = bytes(&geometry);
    assert_eq!(
        oracle::type_code(&encoded).expect("a type code"),
        expected_code,
        "ISO type code of {expected_wkt}"
    );
    assert_wkt_tol(
        &oracle::decode(&encoded).expect("the WKB decodes"),
        expected_wkt,
        Tol::abs(1e-9),
    );
}

#[test]
fn writes_the_simple_feature_types() {
    round_trip("<gml:Point><gml:pos>1 2</gml:pos></gml:Point>", "POINT (1 2)", 1);
    round_trip(
        "<gml:LineString><gml:posList>1 2 3 4</gml:posList></gml:LineString>",
        "LINESTRING (1 2,3 4)",
        2,
    );
    round_trip(
        concat!(
            "<gml:Polygon><gml:exterior><gml:LinearRing>",
            "<gml:posList>0 0 1 0 1 1 0 0</gml:posList>",
            "</gml:LinearRing></gml:exterior></gml:Polygon>"
        ),
        "POLYGON ((0 0,1 0,1 1,0 0))",
        3,
    );
    round_trip(
        concat!(
            "<gml:MultiPoint><gml:pointMember><gml:Point><gml:pos>1 2</gml:pos>",
            "</gml:Point></gml:pointMember></gml:MultiPoint>"
        ),
        "MULTIPOINT ((1 2))",
        4,
    );
    round_trip(
        concat!(
            "<gml:MultiGeometry><gml:geometryMember><gml:Point><gml:pos>0 1</gml:pos>",
            "</gml:Point></gml:geometryMember></gml:MultiGeometry>"
        ),
        "GEOMETRYCOLLECTION (POINT (0 1))",
        7,
    );
}

#[test]
fn writes_the_iso_curve_types() {
    let curve = |segments: &str| format!("<gml:Curve><gml:segments>{segments}</gml:segments></gml:Curve>");
    round_trip(
        &curve("<gml:Arc><gml:posList>0 0 1 1 2 0</gml:posList></gml:Arc>"),
        "CIRCULARSTRING (0 0,1 1,2 0)",
        8,
    );
    round_trip(
        &curve(concat!(
            "<gml:Arc><gml:posList>0 0 1 1 2 0</gml:posList></gml:Arc>",
            "<gml:LineStringSegment><gml:posList>2 0 3 0</gml:posList></gml:LineStringSegment>"
        )),
        "COMPOUNDCURVE (CIRCULARSTRING (0 0,1 1,2 0),(2 0,3 0))",
        9,
    );
    round_trip(
        concat!(
            "<gml:Polygon><gml:exterior><gml:Ring><gml:curveMember><gml:Curve><gml:segments>",
            "<gml:Circle><gml:posList>0 0 0.5 0.5 1 0</gml:posList></gml:Circle>",
            "</gml:segments></gml:Curve></gml:curveMember></gml:Ring></gml:exterior></gml:Polygon>"
        ),
        "CURVEPOLYGON (CIRCULARSTRING (0 0,0.5 0.5,1 0,0.5 -0.5,0 0))",
        10,
    );
    round_trip(
        concat!(
            "<gml:MultiCurve><gml:curveMember><gml:Curve><gml:segments>",
            "<gml:Arc><gml:posList>0 0 1 1 2 0</gml:posList></gml:Arc>",
            "</gml:segments></gml:Curve></gml:curveMember></gml:MultiCurve>"
        ),
        "MULTICURVE (CIRCULARSTRING (0 0,1 1,2 0))",
        11,
    );
}

#[test]
fn three_dimensional_geometry_uses_the_iso_z_codes() {
    round_trip(
        "<gml:Point><gml:pos>1 2 3</gml:pos></gml:Point>",
        "POINT Z (1 2 3)",
        1001,
    );
    round_trip(
        concat!(
            r#"<gml:LineString srsDimension="3">"#,
            "<gml:posList>1 2 3 4 5 6</gml:posList></gml:LineString>"
        ),
        "LINESTRING Z (1 2 3,4 5 6)",
        1002,
    );
}

#[test]
fn point_geometry_is_written_little_endian_byte_for_byte() {
    let geometry = geometry("<gml:Point><gml:pos>1 2</gml:pos></gml:Point>");
    let mut expected = vec![1u8];
    expected.extend(1u32.to_le_bytes());
    expected.extend(1.0f64.to_le_bytes());
    expected.extend(2.0f64.to_le_bytes());
    assert_eq!(bytes(&geometry), expected);
}

#[test]
fn big_endian_output_is_byte_swapped() {
    let geometry = geometry("<gml:Point><gml:pos>1 2</gml:pos></gml:Point>");
    let mut out = Vec::new();
    write_wkb(&geometry, Endianness::Big, &mut out);
    assert_eq!(out[0], 0, "0 marks big endian");
    assert_eq!(oracle::type_code(&out).unwrap(), 1);
    assert_wkt(&oracle::decode(&out).expect("the WKB decodes"), "POINT (1 2)");
}

#[test]
fn empty_geometries_are_written_too() {
    let empty_point = geometry("<gml:Point/>");
    assert_wkt(
        &oracle::decode(&bytes(&empty_point)).expect("the WKB decodes"),
        "POINT EMPTY",
    );
    let empty_line = geometry("<gml:LineString/>");
    assert_wkt(
        &oracle::decode(&bytes(&empty_line)).expect("the WKB decodes"),
        "LINESTRING EMPTY",
    );
}
