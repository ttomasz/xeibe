//! The lightweight geometry scan used by scans and read samples
//! (`docs/schema-inference.md` §2.2, §2.4).
//!
//! It must not build geometries: it collects the element kinds, curve and
//! dimension facts, srsName/axisLabels and the **first position as written**,
//! which is what the axis-order range check needs.

use xeibe_core::Dialect;
use xeibe_core::NamespaceContext;
use xeibe_core::reader::{GmlReader, XmlEvent};
use xeibe_geom::model::GeomKind;
use xeibe_geom::sniff::{GeometrySniff, sniff_geometry};
use xeibe_testkit::gml;

fn sniff(snippet: &str, inherited_srs: Option<&str>) -> GeometrySniff {
    let document = gml::geometry_document(snippet);
    let namespaces = NamespaceContext::new();
    let mut reader = GmlReader::new(document.as_bytes(), &namespaces, 0);
    let mut seen_wrapper = false;
    loop {
        match reader.next_event().expect("well-formed XML") {
            XmlEvent::Start { .. } if !seen_wrapper => seen_wrapper = true,
            XmlEvent::Start { .. } => break,
            XmlEvent::Eof => panic!("no geometry element"),
            _ => {}
        }
    }
    sniff_geometry(&mut reader, inherited_srs).expect("the geometry is sniffed")
}

#[test]
fn records_the_kind_and_the_first_position_as_written() {
    let sniffed = sniff(
        concat!(
            r#"<gml:Polygon srsName="EPSG:2180" srsDimension="2"><gml:exterior><gml:LinearRing>"#,
            "<gml:posList>245281.43 522942.51 245284.55 522943.68 245300.37 522958.14 245281.43 522942.51",
            "</gml:posList></gml:LinearRing></gml:exterior></gml:Polygon>"
        ),
        None,
    );
    assert_eq!(sniffed.kinds, [GeomKind::Polygon]);
    assert_eq!(sniffed.srs_name.as_deref(), Some("EPSG:2180"));
    assert_eq!(sniffed.srs_dimension, Some(2));
    assert_eq!(sniffed.dialect, Some(Dialect::Gml3));
    assert!(!sniffed.has_curves);
    assert!(!sniffed.has_unsupported);
    assert!(!sniffed.empty);
    // No axis decision is applied here: the range check needs the raw order.
    assert_eq!(sniffed.first_position, Some(vec![245281.43, 522942.51]));
}

#[test]
fn notices_curves_without_building_them() {
    let sniffed = sniff(
        concat!(
            "<gml:Curve><gml:segments>",
            "<gml:LineStringSegment><gml:posList>0 0 1 0</gml:posList></gml:LineStringSegment>",
            "<gml:Arc><gml:posList>1 0 2 1 3 0</gml:posList></gml:Arc>",
            "</gml:segments></gml:Curve>"
        ),
        None,
    );
    assert!(sniffed.has_curves);
    assert!(sniffed.kinds.contains(&GeomKind::Curve));
    assert_eq!(sniffed.first_position, Some(vec![0.0, 0.0]));
}

#[test]
fn notices_unsupported_geometry() {
    let sniffed = sniff(
        concat!(
            "<gml:Solid><gml:exterior><gml:CompositeSurface/></gml:exterior></gml:Solid>"
        ),
        None,
    );
    assert!(sniffed.has_unsupported);
    assert!(sniffed.kinds.contains(&GeomKind::Unsupported));
}

#[test]
fn reads_the_gml_2_dialect_and_its_carriers() {
    let sniffed = sniff(
        r#"<gml:Point srsName="EPSG:4326"><gml:coordinates>1,2</gml:coordinates></gml:Point>"#,
        None,
    );
    assert_eq!(sniffed.kinds, [GeomKind::Point]);
    assert_eq!(sniffed.dialect, Some(Dialect::Gml2));
    assert_eq!(sniffed.first_position, Some(vec![1.0, 2.0]));
}

#[test]
fn inherits_the_srs_name_when_the_geometry_has_none() {
    let sniffed = sniff(
        "<gml:Point><gml:pos>1 2</gml:pos></gml:Point>",
        Some("EPSG:2180"),
    );
    assert_eq!(sniffed.srs_name.as_deref(), Some("EPSG:2180"));
    // The geometry's own srsName wins.
    let sniffed = sniff(
        r#"<gml:Point srsName="EPSG:4326"><gml:pos>1 2</gml:pos></gml:Point>"#,
        Some("EPSG:2180"),
    );
    assert_eq!(sniffed.srs_name.as_deref(), Some("EPSG:4326"));
}

#[test]
fn keeps_axis_labels_as_evidence() {
    let sniffed = sniff(
        concat!(
            r#"<gml:Point srsName="EPSG:4326" axisLabels="Lat Long">"#,
            "<gml:pos>52.2 21.0</gml:pos></gml:Point>"
        ),
        None,
    );
    assert_eq!(sniffed.axis_labels.as_deref(), Some("Lat Long"));
    assert_eq!(sniffed.first_position, Some(vec![52.2, 21.0]));
}

#[test]
fn reports_empty_and_referenced_geometry() {
    let empty = sniff("<gml:Point/>", None);
    assert!(empty.empty);
    assert_eq!(empty.first_position, None);

    let referenced = sniff(
        r##"<gml:Point xlink:href="#p1"/>"##,
        None,
    );
    assert!(referenced.by_reference);
}

#[test]
fn a_3d_geometry_is_reported_as_such() {
    let sniffed = sniff(
        concat!(
            r#"<gml:LineString srsName="http://www.opengis.net/def/crs/EPSG/0/3812" srsDimension="3">"#,
            "<gml:posList>701548.2375 711198.8765 50.8437 701594.7455 711470.236 51.1</gml:posList>",
            "</gml:LineString>"
        ),
        None,
    );
    assert_eq!(sniffed.srs_dimension, Some(3));
    assert_eq!(
        sniffed.first_position,
        Some(vec![701548.2375, 711198.8765, 50.8437])
    );
}
