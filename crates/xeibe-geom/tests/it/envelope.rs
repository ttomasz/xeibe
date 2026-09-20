//! `Envelope` and GML 2 `Box` (`docs/geometry.md`, "Envelopes").

use xeibe_core::NamespaceContext;
use xeibe_core::reader::{GmlReader, XmlEvent};
use xeibe_geom::model::Envelope;
use xeibe_geom::parse::{GeometryParser, ParseContext, parse_envelope};
use xeibe_geom::GeometryOptions;
use xeibe_testkit::gml;
use xeibe_testkit::wkt::{G, assert_wkt};

use crate::support::{assert_geometry, coords_to_vec};

/// Parse an envelope snippet: the reader is positioned on the envelope element.
fn envelope(snippet: &str, swap: bool) -> Envelope {
    let document = gml::geometry_document(snippet);
    let namespaces = NamespaceContext::new();
    let mut reader = GmlReader::new(document.as_bytes(), &namespaces, 0);
    let mut seen_wrapper = false;
    loop {
        match reader.next_event().expect("well-formed XML") {
            XmlEvent::Start { .. } if !seen_wrapper => seen_wrapper = true,
            XmlEvent::Start { .. } => break,
            XmlEvent::Eof => panic!("no envelope element"),
            _ => {}
        }
    }
    parse_envelope(&mut reader, swap).expect("an envelope")
}

#[test]
fn envelope_from_lower_and_upper_corner() {
    let envelope = envelope(
        r#"<gml:Envelope srsName="EPSG:2180"><gml:lowerCorner>1 2</gml:lowerCorner><gml:upperCorner>3 4</gml:upperCorner></gml:Envelope>"#,
        false,
    );
    assert_eq!(envelope.lower, [1.0, 2.0]);
    assert_eq!(envelope.upper, [3.0, 4.0]);
    assert_eq!(envelope.srs_name.as_deref(), Some("EPSG:2180"));
}

#[test]
fn the_axis_decision_applies_to_the_corners() {
    let envelope = envelope(
        "<gml:Envelope><gml:lowerCorner>1 2</gml:lowerCorner><gml:upperCorner>3 4</gml:upperCorner></gml:Envelope>",
        true,
    );
    assert_eq!(envelope.lower, [2.0, 1.0]);
    assert_eq!(envelope.upper, [4.0, 3.0]);
}

#[test]
fn the_legacy_corner_spellings_are_accepted() {
    // Two `pos` elements, `coordinates`, and the GML 2 `Box` (§10.1.4.6).
    let with_pos = envelope(
        "<gml:Envelope><gml:pos>1 2</gml:pos><gml:pos>3 4</gml:pos></gml:Envelope>",
        false,
    );
    assert_eq!((with_pos.lower, with_pos.upper), (vec![1.0, 2.0], vec![3.0, 4.0]));

    let with_coordinates = envelope(
        "<gml:Envelope><gml:coordinates>1,2 3,4</gml:coordinates></gml:Envelope>",
        false,
    );
    assert_eq!(
        (with_coordinates.lower, with_coordinates.upper),
        (vec![1.0, 2.0], vec![3.0, 4.0])
    );

    let box_with_coord = envelope(
        concat!(
            "<gml:Box><gml:coord><gml:X>1</gml:X><gml:Y>2</gml:Y></gml:coord>",
            "<gml:coord><gml:X>3</gml:X><gml:Y>4</gml:Y></gml:coord></gml:Box>"
        ),
        false,
    );
    assert_eq!(
        (box_with_coord.lower, box_with_coord.upper),
        (vec![1.0, 2.0], vec![3.0, 4.0])
    );
}

#[test]
fn a_3d_envelope_keeps_its_z_range() {
    let envelope = envelope(
        concat!(
            r#"<gml:Envelope srsDimension="3"><gml:lowerCorner>1 2 3</gml:lowerCorner>"#,
            "<gml:upperCorner>4 5 6</gml:upperCorner></gml:Envelope>"
        ),
        false,
    );
    assert_eq!(envelope.lower, [1.0, 2.0, 3.0]);
    assert_eq!(envelope.upper, [4.0, 5.0, 6.0]);
}

#[test]
fn an_envelope_across_the_antimeridian_is_kept_as_written() {
    // lower.x > upper.x is allowed and is not "fixed".
    let envelope = envelope(
        concat!(
            r#"<gml:Envelope srsName="urn:ogc:def:crs:OGC:1.3:CRS84">"#,
            "<gml:lowerCorner>170 -10</gml:lowerCorner>",
            "<gml:upperCorner>-170 10</gml:upperCorner></gml:Envelope>"
        ),
        false,
    );
    assert_eq!(envelope.lower[0], 170.0);
    assert_eq!(envelope.upper[0], -170.0);
}

#[test]
fn an_envelope_used_as_a_property_value_becomes_a_polygon() {
    // As a geometry (not as `boundedBy`): a closed ring of 5 positions.
    assert_geometry(
        concat!(
            r#"<gml:Envelope srsName="foo"><gml:lowerCorner>1 2</gml:lowerCorner>"#,
            "<gml:upperCorner>3 4</gml:upperCorner></gml:Envelope>"
        ),
        "POLYGON ((1 2,3 2,3 4,1 4,1 2))",
    );
    assert_geometry(
        concat!(
            "<gml:Box><gml:coord><gml:X>1</gml:X><gml:Y>2</gml:Y></gml:coord>",
            "<gml:coord><gml:X>3</gml:X><gml:Y>4</gml:Y></gml:coord></gml:Box>"
        ),
        "POLYGON ((1 2,3 2,3 4,1 4,1 2))",
    );
}

#[test]
fn envelope_to_polygon_closes_the_ring() {
    let envelope = Envelope {
        lower: vec![1.0, 2.0],
        upper: vec![3.0, 4.0],
        srs_name: None,
    };
    let polygon = envelope.to_polygon();
    let ring = coords_to_vec(&polygon.exterior.expect("an exterior ring").coords);
    assert_eq!(ring.len(), 5, "a closed ring of 5 positions");
    assert_eq!(ring.first(), ring.last());
    assert_wkt(
        &G::Polygon(vec![ring]),
        "POLYGON ((1 2,3 2,3 4,1 4,1 2))",
    );
}

#[test]
fn bounded_by_gives_the_envelope_and_its_srs_name() {
    // `boundedBy` is where the collection-level srsName usually comes from.
    let document = gml::geometry_document(concat!(
        r#"<gml:boundedBy><gml:Envelope srsName="EPSG:2180">"#,
        "<gml:lowerCorner>1 2</gml:lowerCorner><gml:upperCorner>3 4</gml:upperCorner>",
        "</gml:Envelope></gml:boundedBy>"
    ));
    let namespaces = NamespaceContext::new();
    let mut reader = GmlReader::new(document.as_bytes(), &namespaces, 0);
    let mut seen_wrapper = false;
    loop {
        match reader.next_event().expect("well-formed XML") {
            XmlEvent::Start { .. } if !seen_wrapper => seen_wrapper = true,
            XmlEvent::Start { .. } => break,
            XmlEvent::Eof => panic!("no boundedBy element"),
            _ => {}
        }
    }
    let options = GeometryOptions::default();
    let parser = GeometryParser::new(&options);
    let envelope = parser
        .parse_bounded_by(&mut reader, &ParseContext::default())
        .expect("boundedBy parses")
        .expect("an envelope");
    assert_eq!(envelope.srs_name.as_deref(), Some("EPSG:2180"));
    assert_eq!(envelope.lower, [1.0, 2.0]);
}

#[test]
fn a_null_bounded_by_has_no_envelope() {
    let document = gml::geometry_document("<gml:boundedBy><gml:Null>unknown</gml:Null></gml:boundedBy>");
    let namespaces = NamespaceContext::new();
    let mut reader = GmlReader::new(document.as_bytes(), &namespaces, 0);
    let mut seen_wrapper = false;
    loop {
        match reader.next_event().expect("well-formed XML") {
            XmlEvent::Start { .. } if !seen_wrapper => seen_wrapper = true,
            XmlEvent::Start { .. } => break,
            XmlEvent::Eof => panic!("no boundedBy element"),
            _ => {}
        }
    }
    let options = GeometryOptions::default();
    let parser = GeometryParser::new(&options);
    assert!(
        parser
            .parse_bounded_by(&mut reader, &ParseContext::default())
            .expect("boundedBy parses")
            .is_none()
    );
}
