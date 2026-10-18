//! The ISO WKB writer (`docs/geometry.md`, "Column encoding", "Curves"), and
//! the reader that takes its output back.
//!
//! The bytes are checked against the testkit's independent reader, plus a few
//! exact type codes: ISO codes are `dimension * 1000 + base`, and the curve
//! types (8…12) are what makes the output ISO rather than OGC WKB.

use xeibe_geom::model::Geometry;
use xeibe_geom::wkb::{Endianness, read_wkb, wkb_size, write_wkb};
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
    round_trip(
        "<gml:Point><gml:pos>1 2</gml:pos></gml:Point>",
        "POINT (1 2)",
        1,
    );
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
    let curve =
        |segments: &str| format!("<gml:Curve><gml:segments>{segments}</gml:segments></gml:Curve>");
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
    assert_wkt(
        &oracle::decode(&out).expect("the WKB decodes"),
        "POINT (1 2)",
    );
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

/// Every kind the writer has, from GML, read back and written again.
#[test]
fn the_reader_takes_back_what_the_writer_writes() {
    let curve =
        |segments: &str| format!("<gml:Curve><gml:segments>{segments}</gml:segments></gml:Curve>");
    let snippets = [
        "<gml:Point><gml:pos>1 2</gml:pos></gml:Point>".to_string(),
        "<gml:Point><gml:pos>1 2 3</gml:pos></gml:Point>".to_string(),
        "<gml:Point/>".to_string(),
        "<gml:LineString><gml:posList>1 2 3 4</gml:posList></gml:LineString>".to_string(),
        "<gml:LineString/>".to_string(),
        concat!(
            "<gml:Polygon><gml:exterior><gml:LinearRing>",
            "<gml:posList>0 0 4 0 4 4 0 0</gml:posList></gml:LinearRing></gml:exterior>",
            "<gml:interior><gml:LinearRing>",
            "<gml:posList>1 1 2 1 2 2 1 1</gml:posList></gml:LinearRing></gml:interior>",
            "</gml:Polygon>"
        )
        .to_string(),
        concat!(
            "<gml:MultiPoint><gml:pointMember><gml:Point><gml:pos>1 2</gml:pos>",
            "</gml:Point></gml:pointMember><gml:pointMember><gml:Point><gml:pos>3 4</gml:pos>",
            "</gml:Point></gml:pointMember></gml:MultiPoint>"
        )
        .to_string(),
        concat!(
            "<gml:MultiGeometry><gml:geometryMember><gml:Point><gml:pos>0 1</gml:pos>",
            "</gml:Point></gml:geometryMember><gml:geometryMember><gml:LineString>",
            "<gml:posList>0 0 1 1</gml:posList></gml:LineString></gml:geometryMember>",
            "</gml:MultiGeometry>"
        )
        .to_string(),
        curve("<gml:Arc><gml:posList>0 0 1 1 2 0</gml:posList></gml:Arc>"),
        curve(concat!(
            "<gml:Arc><gml:posList>0 0 1 1 2 0</gml:posList></gml:Arc>",
            "<gml:LineStringSegment><gml:posList>2 0 3 0</gml:posList></gml:LineStringSegment>"
        )),
        concat!(
            "<gml:Polygon><gml:exterior><gml:Ring><gml:curveMember><gml:Curve><gml:segments>",
            "<gml:Circle><gml:posList>0 0 0.5 0.5 1 0</gml:posList></gml:Circle>",
            "</gml:segments></gml:Curve></gml:curveMember></gml:Ring></gml:exterior></gml:Polygon>"
        )
        .to_string(),
        concat!(
            "<gml:MultiCurve><gml:curveMember><gml:Curve><gml:segments>",
            "<gml:Arc><gml:posList>0 0 1 1 2 0</gml:posList></gml:Arc>",
            "</gml:segments></gml:Curve></gml:curveMember><gml:curveMember><gml:LineString>",
            "<gml:posList>5 5 6 6</gml:posList></gml:LineString></gml:curveMember></gml:MultiCurve>"
        )
        .to_string(),
        concat!(
            "<gml:MultiSurface><gml:surfaceMember><gml:Polygon><gml:exterior><gml:Ring>",
            "<gml:curveMember><gml:Curve><gml:segments>",
            "<gml:Circle><gml:posList>0 0 0.5 0.5 1 0</gml:posList></gml:Circle>",
            "</gml:segments></gml:Curve></gml:curveMember></gml:Ring></gml:exterior>",
            "</gml:Polygon></gml:surfaceMember><gml:surfaceMember><gml:Polygon><gml:exterior>",
            "<gml:LinearRing><gml:posList>5 5 6 5 6 6 5 5</gml:posList></gml:LinearRing>",
            "</gml:exterior></gml:Polygon></gml:surfaceMember></gml:MultiSurface>"
        )
        .to_string(),
    ];
    for snippet in &snippets {
        let geometry = geometry(snippet);
        let written = bytes(&geometry);
        let read = read_wkb(&written).unwrap_or_else(|e| panic!("{snippet}: {e}"));
        assert_eq!(bytes(&read), written, "{snippet}");
        let mut big = Vec::new();
        write_wkb(&geometry, Endianness::Big, &mut big);
        let read = read_wkb(&big).unwrap_or_else(|e| panic!("{snippet}, big endian: {e}"));
        assert_eq!(bytes(&read), written, "{snippet}, big endian");
    }
}

#[test]
fn the_reader_takes_ewkb_and_drops_m() {
    // EWKB: Point with the Z, M and SRID flags, SRID 2180, then x y z m.
    let mut ewkb = vec![1u8];
    ewkb.extend((1u32 | 0x8000_0000 | 0x4000_0000 | 0x2000_0000).to_le_bytes());
    ewkb.extend(2180u32.to_le_bytes());
    for value in [1.0f64, 2.0, 3.0, 4.0] {
        ewkb.extend(value.to_le_bytes());
    }
    let expected = geometry("<gml:Point><gml:pos>1 2 3</gml:pos></gml:Point>");
    assert_eq!(read_wkb(&ewkb).unwrap(), expected);

    // ISO LineString M (2002): the M ordinates are dropped.
    let mut iso = vec![1u8];
    iso.extend(2002u32.to_le_bytes());
    iso.extend(2u32.to_le_bytes());
    for value in [1.0f64, 2.0, 9.0, 3.0, 4.0, 9.0] {
        iso.extend(value.to_le_bytes());
    }
    let expected = geometry("<gml:LineString><gml:posList>1 2 3 4</gml:posList></gml:LineString>");
    assert_eq!(read_wkb(&iso).unwrap(), expected);
}

#[test]
fn the_reader_rejects_broken_wkb() {
    let written = bytes(&geometry(
        "<gml:LineString><gml:posList>1 2 3 4</gml:posList></gml:LineString>",
    ));
    let error = read_wkb(&written[..3]).unwrap_err();
    assert!(error.contains("truncated"), "{error}");
    // Positions missing: the count says more than the bytes left.
    let error = read_wkb(&written[..written.len() - 1]).unwrap_err();
    assert!(error.contains("exceeds"), "{error}");
    let mut trailing = written.clone();
    trailing.push(0);
    assert!(
        read_wkb(&trailing)
            .unwrap_err()
            .contains("after the geometry")
    );
    // A count far beyond the data must not allocate.
    let mut huge = written[..5].to_vec();
    huge.extend(u32::MAX.to_le_bytes());
    assert!(read_wkb(&huge).unwrap_err().contains("exceeds"));
    let mut unknown = vec![1u8];
    unknown.extend(17u32.to_le_bytes());
    assert!(read_wkb(&unknown).unwrap_err().contains("geometry type 17"));
    assert!(read_wkb(&[7]).unwrap_err().contains("byte order"));
}
