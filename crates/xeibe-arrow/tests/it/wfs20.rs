//! WFS 2.0 response structures read end to end (OGC 09-025r2 §11.3.3):
//! the nested collections of a `GetFeature` with several queries
//! (§11.3.3.5), the members of one query that reference a feature of another,
//! and the objects a `resolve` adds under `wfs:additionalObjects` (§11.3.4).

use xeibe_arrow::ReadOptions;
use xeibe_arrow::report::WarningKind;
use xeibe_testkit::gml;
use xeibe_testkit::wkt::assert_wkt;

use crate::support::{Read, read_document, read_with, scan_document};

/// Rows in document order: each nested collection is a chunk of its own, so
/// without `preserve_order` the workers may hand them back in any order.
fn read_in_order(document: &str, layer: &str) -> Read {
    let options = ReadOptions {
        preserve_order: true,
        ..ReadOptions::default()
    };
    read_with(document, layer, None, &options)
}

const SRS: &str = "urn:ogc:def:crs:EPSG::3857";

fn envelope(tag: &str, srs: &str) -> String {
    format!(
        "<{tag}><gml:Envelope srsName=\"{srs}\"><gml:lowerCorner>0 0</gml:lowerCorner>\
         <gml:upperCorner>10 10</gml:upperCorner></gml:Envelope></{tag}>"
    )
}

fn parcel(id: &str, name: &str, x: u32) -> String {
    gml::feature(
        "Parcel",
        id,
        &format!(
            "<app:name>{name}</app:name>\
             <app:geom><gml:Point><gml:pos>{x} 1</gml:pos></gml:Point></app:geom>"
        ),
    )
}

/// One query's result: a `wfs:FeatureCollection` with its own envelope and
/// `wfs:member`s, which are features or, written as is, references.
fn query(bounded_by: &str, members: &[&str]) -> String {
    let members: String = members
        .iter()
        .map(|member| {
            if member.starts_with("<wfs:member") {
                member.to_string()
            } else {
                format!("<wfs:member>{member}</wfs:member>")
            }
        })
        .collect();
    format!(
        "<wfs:FeatureCollection numberMatched=\"unknown\" numberReturned=\"0\">\
         {bounded_by}{members}</wfs:FeatureCollection>"
    )
}

/// A `GetFeature` with two queries, `Parcel` and `Parcel,Road`. The second
/// one's result references `p1`, which the first already holds (§11.3.3.5).
/// The first collection's envelope gives its features their srsName; the
/// second has none, so its features take the outer collection's.
fn multi_query() -> String {
    let road = gml::feature(
        "Road",
        "r1",
        "<app:name>Main</app:name>\
         <app:centre><gml:LineString><gml:posList>0 0 5 5</gml:posList></gml:LineString></app:centre>",
    );
    let first = query(
        &envelope("wfs:boundedBy", SRS),
        &[&parcel("p1", "A", 1), &parcel("p2", "B", 2)],
    );
    let second = query(
        "",
        &[
            &parcel("p3", "C", 3),
            &road,
            r##"<wfs:member xlink:href="#p1"/>"##,
        ],
    );
    gml::wfs20_collection(
        r#"numberMatched="unknown" numberReturned="0""#,
        &[&first, &second],
    )
    .replacen(
        "<wfs:member>",
        &format!("{}<wfs:member>", envelope("wfs:boundedBy", SRS)),
        1,
    )
}

#[test]
fn a_multi_query_response_reads_every_querys_features_into_their_layers() {
    let document = multi_query();
    let scan = scan_document(&document);
    let layers: Vec<(String, u64, Vec<String>)> = scan
        .layers()
        .into_iter()
        .map(|layer| (layer.name.local.to_string(), layer.feature_count, layer.crs))
        .collect();
    assert_eq!(
        layers,
        [
            ("Parcel".to_string(), 3, vec![SRS.to_string()]),
            ("Road".to_string(), 1, vec![SRS.to_string()]),
        ]
    );

    // One layer spans both queries' collections.
    let parcels = read_in_order(&document, "Parcel");
    assert_eq!(parcels.column_names(), ["@id", "name", "geom"]);
    let ids = parcels.strings("@id");
    assert_eq!(
        ids,
        ["p1", "p2", "p3"].map(|id| Some(id.to_string())),
        "the referenced member is not read a second time"
    );
    let geometries = parcels.geometries("geom");
    for (geometry, x) in geometries.iter().zip(1..) {
        assert_wkt(
            geometry.as_ref().expect("a point"),
            &format!("POINT({x} 1)"),
        );
    }

    let roads = read_document(&document, "Road");
    assert_eq!(roads.strings("name"), [Some("Main".to_string())]);
    assert_wkt(
        roads.geometries("centre")[0].as_ref().expect("a line"),
        "LINESTRING(0 0,5 5)",
    );

    // The reference is counted and reported, not fetched.
    for read in [&parcels, &roads] {
        let warnings: Vec<&str> = read
            .report
            .warnings
            .iter()
            .filter(|warning| warning.kind == WarningKind::ReferencedMember)
            .map(|warning| warning.message.as_str())
            .collect();
        assert_eq!(
            warnings,
            ["1 feature members given by reference (not read)"]
        );
    }
}

#[test]
fn an_inner_collection_without_members_adds_no_layer() {
    // A query that matched nothing still returns its (empty) collection.
    let document = gml::wfs20_collection(
        r#"numberMatched="2" numberReturned="2""#,
        &[
            &query("", &[&parcel("p1", "A", 1), &parcel("p2", "B", 2)]),
            &query("", &[]),
        ],
    );
    let layers: Vec<(String, u64)> = scan_document(&document)
        .layers()
        .into_iter()
        .map(|layer| (layer.name.local.to_string(), layer.feature_count))
        .collect();
    assert_eq!(layers, [("Parcel".to_string(), 2)]);
    assert_eq!(read_document(&document, "Parcel").rows(), 2);
}

/// A response with `resolve=local`: the members' `owner` references an
/// `Owner` that is no member of the result, so the server adds it under
/// `wfs:additionalObjects`, in a `wfs:SimpleFeatureCollection` or
/// `wfs:ValueCollection` (the two choices of `wfs.xsd`).
fn with_additional_objects(container: &str) -> String {
    let owner = gml::feature("Owner", "o1", "<app:name>Kowalski</app:name>");
    let member = |id: &str| {
        gml::feature(
            "Parcel",
            id,
            r##"<app:area>5</app:area><app:owner xlink:href="#o1"/>"##,
        )
    };
    gml::wfs20_collection(
        r#"numberMatched="2" numberReturned="2""#,
        &[&member("p1"), &member("p2")],
    )
    .replacen(
        "\n</wfs:FeatureCollection>",
        &format!(
            "<wfs:additionalObjects><wfs:{container}>\
             <wfs:member>{owner}</wfs:member>\
             </wfs:{container}></wfs:additionalObjects>\n</wfs:FeatureCollection>"
        ),
        1,
    )
}

#[test]
fn additional_objects_are_features_of_their_own_layers() {
    for container in ["SimpleFeatureCollection", "ValueCollection"] {
        let document = with_additional_objects(container);
        let layers: Vec<(String, u64)> = scan_document(&document)
            .layers()
            .into_iter()
            .map(|layer| (layer.name.local.to_string(), layer.feature_count))
            .collect();
        assert_eq!(
            layers,
            [("Parcel".to_string(), 2), ("Owner".to_string(), 1)],
            "{container}"
        );

        let parcels = read_document(&document, "Parcel");
        assert_eq!(parcels.column_names(), ["@id", "area", "owner"]);
        assert_eq!(
            parcels.strings("owner"),
            vec![Some("o1".to_string()); 2],
            "a link within the file, without its #: the Owner's @id"
        );
        assert!(parcels.report.warnings.is_empty(), "{container}");

        let owners = read_document(&document, "Owner");
        assert_eq!(owners.column_names(), ["@id", "name"]);
        assert_eq!(owners.strings("@id"), [Some("o1".to_string())]);
        assert_eq!(owners.strings("name"), [Some("Kowalski".to_string())]);
    }
}
