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

/// Parse a `boundedBy` snippet as a feature's, with `context` inherited.
fn bounded_by_inherited(snippet: &str, context: &ParseContext) -> (Option<Envelope>, ParseContext) {
    let document = gml::geometry_document(snippet);
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
    GeometryParser::new(&options)
        .parse_bounded_by_inherited(&mut reader, context)
        .expect("boundedBy parses")
}

#[test]
fn a_bounded_by_hands_down_its_srs_name_and_dimension() {
    // The nearest declaration wins (`docs/geometry.md`, "srsName inheritance").
    let outer = ParseContext { srs_name: Some("EPSG:2180".into()), srs_dimension: Some(3), axis: None };
    let (envelope, inherited) = bounded_by_inherited(
        concat!(
            r#"<gml:boundedBy><gml:Envelope srsName="EPSG:2176" srsDimension="2">"#,
            "<gml:lowerCorner>1 2</gml:lowerCorner><gml:upperCorner>3 4</gml:upperCorner>",
            "</gml:Envelope></gml:boundedBy>"
        ),
        &outer,
    );
    assert_eq!(envelope.expect("an envelope").srs_name.as_deref(), Some("EPSG:2176"));
    assert_eq!((inherited.srs_name.as_deref(), inherited.srs_dimension), (Some("EPSG:2176"), Some(2)));

    // Without declarations of its own it passes the outer ones on.
    let (envelope, inherited) = bounded_by_inherited(
        concat!(
            "<gml:boundedBy><gml:Envelope>",
            "<gml:lowerCorner>1 2 3</gml:lowerCorner><gml:upperCorner>4 5 6</gml:upperCorner>",
            "</gml:Envelope></gml:boundedBy>"
        ),
        &outer,
    );
    let envelope = envelope.expect("an envelope");
    assert_eq!(envelope.srs_name.as_deref(), Some("EPSG:2180"));
    assert_eq!(envelope.upper, [4.0, 5.0, 6.0]);
    assert_eq!((inherited.srs_name.as_deref(), inherited.srs_dimension), (Some("EPSG:2180"), Some(3)));

    let (envelope, inherited) =
        bounded_by_inherited("<gml:boundedBy><gml:Null>unknown</gml:Null></gml:boundedBy>", &outer);
    assert!(envelope.is_none());
    assert_eq!((inherited.srs_name.as_deref(), inherited.srs_dimension), (Some("EPSG:2180"), Some(3)));
}

/// What the collection `boundedBy` of a document hands down, read as the
/// splitter keeps it.
fn collection(bounded_by: &str) -> (Option<Envelope>, ParseContext) {
    let feature = gml::feature("Parcel", "p1", "");
    let document = gml::collection(gml::GML_32, "gml:featureMember", &[&feature], "", bounded_by);
    let chunk = xeibe_core::FeatureSplitter::new(
        document.as_bytes(),
        xeibe_core::SourceId(0),
        xeibe_core::SplitterOptions::default(),
    )
    .next()
    .expect("a chunk")
    .expect("the document splits");
    match chunk.collection_bounded_by.as_deref() {
        Some(raw) => xeibe_geom::parse::collection_bounded_by(raw).expect("the boundedBy is well-formed"),
        None => (None, ParseContext::default()),
    }
}

#[test]
fn a_collection_bounded_by_is_read_from_the_element_the_splitter_keeps() {
    let (envelope, inherited) = collection(concat!(
        r#"<gml:boundedBy><gml:Envelope srsName="EPSG:2180" srsDimension="3">"#,
        "<gml:lowerCorner>0 0 0</gml:lowerCorner><gml:upperCorner>10 20 30</gml:upperCorner>",
        "</gml:Envelope></gml:boundedBy>"
    ));
    let envelope = envelope.expect("an envelope");
    assert_eq!((envelope.lower, envelope.upper), (vec![0.0, 0.0, 0.0], vec![10.0, 20.0, 30.0]));
    assert_eq!((inherited.srs_name.as_deref(), inherited.srs_dimension), (Some("EPSG:2180"), Some(3)));

    // Corners that can't be read give no extent, but the srsName still counts.
    let (envelope, inherited) = collection(concat!(
        r#"<gml:boundedBy><gml:Envelope srsName="EPSG:2180">"#,
        "<gml:lowerCorner>a b</gml:lowerCorner><gml:upperCorner>1 1</gml:upperCorner>",
        "</gml:Envelope></gml:boundedBy>"
    ));
    assert!(envelope.is_none());
    assert_eq!(inherited.srs_name.as_deref(), Some("EPSG:2180"));

    let (envelope, inherited) = collection("<gml:boundedBy><gml:Null>unknown</gml:Null></gml:boundedBy>");
    assert!(envelope.is_none());
    assert_eq!(inherited.srs_name, None);
}
