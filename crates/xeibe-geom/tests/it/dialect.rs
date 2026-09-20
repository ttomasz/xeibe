//! Per-element GML dialect classification, which drives
//! `AxisOrderMode::GmlVersion` (`docs/geometry.md`, "Modes").
//!
//! GML 2 and GML 3.1 share a namespace and real files mix encodings, so the
//! dialect is decided per geometry element, not from the document header.

use xeibe_core::{Dialect, QName, ns};
use xeibe_geom::dialect::{DialectTracker, classify_element};

fn gml(local: &str) -> QName {
    QName::new(Some(ns::GML), local)
}

fn gml32(local: &str) -> QName {
    QName::new(Some(ns::GML_32), local)
}

#[test]
fn gml_2_carriers_and_structure() {
    for local in ["coordinates", "coord", "outerBoundaryIs", "innerBoundaryIs", "Box"] {
        assert_eq!(classify_element(&gml(local)), Some(Dialect::Gml2), "{local}");
    }
}

#[test]
fn gml_3_carriers_and_structure() {
    for local in [
        "pos",
        "posList",
        "pointProperty",
        "exterior",
        "interior",
        "Curve",
        "Surface",
        "Envelope",
    ] {
        assert_eq!(classify_element(&gml(local)), Some(Dialect::Gml3), "{local}");
    }
    // Anything in the GML 3.2 namespace is GML 3.
    assert_eq!(classify_element(&gml32("Point")), Some(Dialect::Gml3));
    assert_eq!(classify_element(&gml32("coordinates")), Some(Dialect::Gml3));
}

#[test]
fn version_neutral_elements_say_nothing() {
    for local in ["Point", "LineString", "LinearRing", "Polygon", "MultiPoint"] {
        assert_eq!(classify_element(&gml(local)), None, "{local}");
    }
    // Elements outside GML are not classified at all.
    assert_eq!(
        classify_element(&QName::new(Some("http://example.com/app"), "pos")),
        None
    );
}

#[test]
fn the_structure_decides_for_the_whole_geometry() {
    // `gml:coordinates` inside GML 3 structure stays GML 3: a deprecated
    // coordinate encoding does not make the geometry GML 2.
    let mut tracker = DialectTracker::default();
    tracker.observe(&gml("Polygon"));
    tracker.observe(&gml("exterior"));
    tracker.observe(&gml("LinearRing"));
    tracker.observe(&gml("coordinates"));
    assert_eq!(tracker.result(), Dialect::Gml3);

    let mut tracker = DialectTracker::default();
    tracker.observe(&gml("Polygon"));
    tracker.observe(&gml("outerBoundaryIs"));
    tracker.observe(&gml("LinearRing"));
    tracker.observe(&gml("coordinates"));
    assert_eq!(tracker.result(), Dialect::Gml2);
}

#[test]
fn a_carrier_alone_decides_when_there_is_no_structure() {
    let mut point2 = DialectTracker::default();
    point2.observe(&gml("Point"));
    point2.observe(&gml("coordinates"));
    assert_eq!(point2.result(), Dialect::Gml2);

    let mut point3 = DialectTracker::default();
    point3.observe(&gml("Point"));
    point3.observe(&gml("pos"));
    assert_eq!(point3.result(), Dialect::Gml3);
}
