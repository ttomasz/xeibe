//! GML version detection (`docs/schema-inference.md` §2.8).
//!
//! GML 2 and 3.1 share a namespace, so the version comes from
//! `xsi:schemaLocation`, the WFS version and output format, and the elements
//! seen.

use xeibe_core::version::VersionHints;
use xeibe_core::{GmlVersion, ns};

fn hints() -> VersionHints {
    VersionHints {
        gml_namespace: Some(ns::GML.to_string()),
        ..VersionHints::default()
    }
}

#[test]
fn the_gml_32_namespace_decides_on_its_own() {
    let hints = VersionHints {
        gml_namespace: Some(ns::GML_32.to_string()),
        ..VersionHints::default()
    };
    assert_eq!(hints.detect(), Some(GmlVersion::V3_2));
}

#[test]
fn schema_location_distinguishes_gml_2_from_gml_3_1() {
    let gml2 = VersionHints {
        schema_location: Some(
            "http://www.opengis.net/gml http://schemas.opengis.net/gml/2.1.2/feature.xsd".into(),
        ),
        ..hints()
    };
    assert_eq!(gml2.detect(), Some(GmlVersion::V2));

    let gml31 = VersionHints {
        schema_location: Some(
            "http://www.opengis.net/gml http://schemas.opengis.net/gml/3.1.1/base/gml.xsd".into(),
        ),
        ..hints()
    };
    assert_eq!(gml31.detect(), Some(GmlVersion::V3_1));
}

#[test]
fn the_wfs_version_and_output_format_are_used_for_responses() {
    let wfs10 = VersionHints {
        wfs_version: Some("1.0.0".into()),
        ..hints()
    };
    assert_eq!(wfs10.detect(), Some(GmlVersion::V2));

    let wfs11 = VersionHints {
        wfs_version: Some("1.1.0".into()),
        output_format: Some("text/xml; subtype=gml/3.1.1".into()),
        ..hints()
    };
    assert_eq!(wfs11.detect(), Some(GmlVersion::V3_1));

    let wfs20 = VersionHints {
        wfs_version: Some("2.0.0".into()),
        output_format: Some("application/gml+xml; version=3.2".into()),
        gml_namespace: Some(ns::GML_32.to_string()),
        ..VersionHints::default()
    };
    assert_eq!(wfs20.detect(), Some(GmlVersion::V3_2));
}

#[test]
fn the_elements_seen_decide_when_nothing_else_does() {
    let gml2 = VersionHints {
        saw_gml2_elements: true,
        ..hints()
    };
    assert_eq!(gml2.detect(), Some(GmlVersion::V2));

    let gml3 = VersionHints {
        saw_gml3_elements: true,
        ..hints()
    };
    assert_eq!(gml3.detect(), Some(GmlVersion::V3_1));

    // `posList@dimension` exists only in GML 3.0.
    let gml30 = VersionHints {
        saw_gml3_elements: true,
        saw_poslist_dimension: true,
        ..hints()
    };
    assert_eq!(gml30.detect(), Some(GmlVersion::V3_0));
}

#[test]
fn the_schema_location_wins_over_the_elements_seen() {
    // Real files mix encodings: a GML 2 document may contain `posList`.
    let mixed = VersionHints {
        schema_location: Some("http://schemas.opengis.net/gml/2.1.2/feature.xsd".into()),
        saw_gml3_elements: true,
        ..hints()
    };
    assert_eq!(mixed.detect(), Some(GmlVersion::V2));
}

#[test]
fn without_evidence_the_version_is_unknown() {
    assert_eq!(VersionHints::default().detect(), None);
}
