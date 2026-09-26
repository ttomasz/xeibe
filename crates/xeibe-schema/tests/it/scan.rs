//! Scanning: chunks in, one `DatasetObservation` out
//! (`docs/schema-inference.md` §2, §6).

use xeibe_core::{GmlVersion, Source, Sources};
use xeibe_schema::{Merge, ScanExtent, ScanOptions, Scanner};
use xeibe_testkit::gml;

use crate::support::{layer, scan, scan_with};

fn parcel(id: &str, body: &str) -> String {
    gml::feature("Parcel", id, body)
}

#[test]
fn every_feature_type_becomes_a_layer() {
    let document = gml::gml32_collection(&[
        &parcel("p1", "<app:area>1</app:area>"),
        &gml::feature("Road", "r1", "<app:name>A</app:name>"),
        &parcel("p2", "<app:area>2</app:area>"),
    ]);
    let observation = scan(&document);
    let names: Vec<String> = observation
        .layers
        .keys()
        .map(|name| name.local.to_string())
        .collect();
    assert_eq!(names, ["Parcel", "Road"], "layers in first-seen order");
    assert_eq!(observation.layers[&layer("Parcel")].feature_count, 2);
    assert_eq!(observation.layers[&layer("Road")].feature_count, 1);
    assert!(!observation.sampled, "a full scan is not sampled");
}

#[test]
fn the_gml_version_is_recorded() {
    let gml32 = scan(&gml::gml32_collection(&[&parcel("p1", "")]));
    assert!(gml32.gml_versions.contains(&GmlVersion::V3_2));

    let gml2 = scan(&gml::gml2_collection(&[&parcel("p1", "")]));
    assert!(gml2.gml_versions.contains(&GmlVersion::V2));
}

#[test]
fn geometry_statistics_are_collected_without_building_geometry() {
    let document = gml::gml32_collection(&[
        &parcel(
            "p1",
            concat!(
                r#"<app:geom><gml:Point srsName="EPSG:2180" srsDimension="2">"#,
                "<gml:pos>205249.1 530976.79</gml:pos></gml:Point></app:geom>"
            ),
        ),
        &parcel(
            "p2",
            concat!(
                r#"<app:geom><gml:Curve srsName="EPSG:2180"><gml:segments>"#,
                "<gml:Arc><gml:posList>0 0 1 1 2 0</gml:posList></gml:Arc>",
                "</gml:segments></gml:Curve></app:geom>"
            ),
        ),
    ]);
    let observation = scan(&document);
    let root = &observation.layers[&layer("Parcel")].root;
    let geom = root
        .children
        .values()
        .find_map(|child| child.geometry.as_ref())
        .expect("a geometry column");
    assert_eq!(geom.count, 2);
    assert!(geom.has_curves, "the second feature has an arc");
    assert_eq!(geom.srs.get("EPSG:2180"), Some(&2));
    assert!(geom.dims.contains(&2));
    // The first position of each geometry feeds the axis-order range check.
    let evidence = geom
        .axis_evidence
        .values()
        .next()
        .expect("axis evidence per key");
    assert!(evidence.samples >= 1);
    assert!(evidence.sampled_bbox.is_some());
}

#[test]
fn the_extent_of_a_layer_is_the_union_of_its_geometries() {
    let document = gml::gml32_collection(&[
        &parcel("p1", "<app:geom><gml:Point><gml:pos>0 0</gml:pos></gml:Point></app:geom>"),
        &parcel("p2", "<app:geom><gml:Point><gml:pos>10 20</gml:pos></gml:Point></app:geom>"),
    ]);
    let observation = scan(&document);
    assert_eq!(
        observation.layers[&layer("Parcel")].extent,
        Some([0.0, 0.0, 10.0, 20.0])
    );
}

#[test]
fn a_sampled_scan_stops_early_and_says_so() {
    let members: Vec<String> = (0..10)
        .map(|i| parcel(&format!("p{i}"), &format!("<app:area>{i}</app:area>")))
        .collect();
    let refs: Vec<&str> = members.iter().map(String::as_str).collect();
    let document = gml::gml32_collection(&refs);

    let sampled = scan_with(
        &document,
        ScanOptions {
            extent: ScanExtent::Sample { max_features: 3 },
            ..ScanOptions::default()
        },
    );
    assert!(sampled.sampled);
    assert_eq!(sampled.layers[&layer("Parcel")].feature_count, 3);
}

#[test]
fn a_sampled_scan_can_miss_layers_that_start_later() {
    // PRG stores its layers one after another, which is why a sampled scan is
    // only a quick look.
    let document = gml::gml32_collection(&[
        &parcel("p1", ""),
        &parcel("p2", ""),
        &gml::feature("Road", "r1", ""),
    ]);
    let sampled = scan_with(
        &document,
        ScanOptions {
            extent: ScanExtent::Sample { max_features: 2 },
            ..ScanOptions::default()
        },
    );
    assert!(sampled.layer("Road").is_err(), "Road starts after the sample");
}

#[test]
fn several_sources_are_merged_into_one_observation() {
    let first = gml::gml32_collection(&[&parcel("p1", "<app:area>1</app:area>")]);
    let second = gml::gml32_collection(&[&parcel("p2", "<app:area>2</app:area><app:extra>x</app:extra>")]);
    let sources = Sources::from(vec![
        Source::reader("a.gml", Box::new(std::io::Cursor::new(first.into_bytes()))),
        Source::reader("b.gml", Box::new(std::io::Cursor::new(second.into_bytes()))),
    ]);
    let observation = Scanner::new(ScanOptions::default())
        .run(sources)
        .expect("both sources scan");
    let parcel_layer = &observation.layers[&layer("Parcel")];
    assert_eq!(parcel_layer.feature_count, 2);
    assert_eq!(parcel_layer.root.children.len(), 2, "area and extra");
}

/// A source over a document held in memory.
fn reader(name: &str, document: &str) -> Source {
    Source::reader(name, Box::new(std::io::Cursor::new(document.as_bytes().to_vec())))
}

/// ISO metadata, as zips ship it next to the GML: a root, but no features.
const ISO_METADATA: &str = r#"<gmd:MD_Metadata xmlns:gmd="http://www.isotc211.org/2005/gmd"><gmd:fileIdentifier/></gmd:MD_Metadata>"#;

#[test]
fn sources_without_features_are_skipped_and_listed() {
    // The splitter finds out at the end of the input (metadata) or before
    // the root (an empty file); both are skipped, full scan or sampled.
    let document = gml::gml32_collection(&[&parcel("p1", "<app:area>1</app:area>")]);
    for extent in [ScanExtent::Full, ScanExtent::Sample { max_features: 10 }] {
        let sources = Sources::from(vec![
            reader("metadata.xml", ISO_METADATA),
            reader("parcels.gml", &document),
            reader("empty.xml", ""),
        ]);
        let observation = Scanner::new(ScanOptions { extent, ..ScanOptions::default() })
            .run(sources)
            .unwrap_or_else(|e| panic!("{extent:?}: {e}"));
        assert_eq!(observation.layers[&layer("Parcel")].feature_count, 1, "{extent:?}");
        assert_eq!(observation.skipped_sources, ["metadata.xml", "empty.xml"], "{extent:?}");
        assert_eq!(observation.source_context.len(), 3, "one context per source id");
    }
}

#[test]
fn a_scan_in_which_every_source_is_skipped_fails() {
    let error = Scanner::new(ScanOptions::default())
        .run(Sources::from(reader("metadata.xml", ISO_METADATA)))
        .expect_err("nothing to scan");
    assert_eq!(error.to_string(), "no feature collection or feature member found in metadata.xml");

    let sources = Sources::from(vec![reader("a.xml", ISO_METADATA), reader("b.xml", "")]);
    let error = Scanner::new(ScanOptions::default()).run(sources).expect_err("nothing to scan");
    assert!(error.to_string().ends_with("found in any of the 2 sources"), "{error}");
}

#[test]
fn observations_merge_the_same_way_whatever_the_order() {
    // This is what makes parallel scans and multi-page WFS reads possible.
    let first = scan(&gml::gml32_collection(&[&parcel("p1", "<app:area>1</app:area>")]));
    let second = scan(&gml::gml32_collection(&[&gml::feature("Road", "r1", "")]));

    let mut forwards = first.clone();
    forwards.merge(second.clone());
    let mut backwards = second;
    backwards.merge(first);

    assert_eq!(forwards.layers.len(), 2);
    assert_eq!(forwards.layers.len(), backwards.layers.len());
    assert_eq!(
        forwards.layers[&layer("Parcel")].feature_count,
        backwards.layers[&layer("Parcel")].feature_count
    );
}

#[test]
fn a_chunk_can_be_scanned_on_its_own() {
    // Workers scan one chunk each, and a read samples its layer from the
    // chunks it has buffered.
    let document = gml::gml32_collection(&[&parcel("p1", "<app:area>1</app:area>")]);
    let chunks: Vec<_> = xeibe_core::FeatureSplitter::new(
        document.as_bytes(),
        xeibe_core::SourceId(0),
        xeibe_core::SplitterOptions::default(),
    )
    .collect::<xeibe_core::Result<Vec<_>>>()
    .expect("the document splits");

    let scanner = Scanner::new(ScanOptions::default());
    let mut merged = xeibe_schema::DatasetObservation::default();
    for chunk in &chunks {
        merged.merge(scanner.scan_chunk(chunk).expect("the chunk scans"));
    }
    assert_eq!(merged.layers[&layer("Parcel")].feature_count, 1);
}

#[test]
fn the_scan_can_be_limited_to_some_layers() {
    let document = gml::gml32_collection(&[&parcel("p1", ""), &gml::feature("Road", "r1", "")]);
    let observation = scan_with(
        &document,
        ScanOptions {
            layers: Some(vec![layer("Parcel")]),
            ..ScanOptions::default()
        },
    );
    assert_eq!(observation.layers.len(), 1);
    assert!(observation.layer("Parcel").is_ok());
}

#[test]
fn layers_can_be_looked_up_by_prefixed_or_clark_name() {
    let document = gml::gml32_collection(&[&parcel("p1", "")]);
    let observation = scan(&document);
    assert!(observation.layer("Parcel").is_ok(), "local name");
    assert!(observation.layer("app:Parcel").is_ok(), "prefixed name");
    assert!(
        observation
            .layer(&format!("{{{}}}Parcel", xeibe_testkit::gml::APP))
            .is_ok(),
        "Clark notation"
    );
    assert!(observation.layer("Nope").is_err());
}

#[test]
fn source_context_is_kept_for_axis_decisions() {
    let document = gml::fme_collection(&[&parcel("p1", "")]);
    let observation = scan(&document);
    assert_eq!(observation.source_context.len(), 1);
    assert!(
        observation.source_context[0].fme_produced,
        "the FME namespace is axis-order evidence"
    );
}

#[test]
fn geometries_inherit_the_collection_bounded_by() {
    // Collection `boundedBy` → feature `boundedBy` → geometry: the nearest
    // srsName and srsDimension win (`docs/geometry.md`, "srsName inheritance").
    let line = |coordinates: &str| {
        format!("<app:geom><gml:LineString><gml:posList>{coordinates}</gml:posList></gml:LineString></app:geom>")
    };
    let document = gml::collection(
        gml::GML_32,
        "gml:featureMember",
        &[
            &parcel("p1", &line("0 0 1 1 1 2")),
            &parcel(
                "p2",
                &format!(
                    concat!(
                        r#"<gml:boundedBy><gml:Envelope srsName="EPSG:2176" srsDimension="2">"#,
                        "<gml:lowerCorner>0 0</gml:lowerCorner><gml:upperCorner>1 1</gml:upperCorner>",
                        "</gml:Envelope></gml:boundedBy>{}"
                    ),
                    line("0 0 1 1")
                ),
            ),
        ],
        "",
        concat!(
            r#"<gml:boundedBy><gml:Envelope srsName="EPSG:2180" srsDimension="3">"#,
            "<gml:lowerCorner>0 0 0</gml:lowerCorner><gml:upperCorner>10 20 30</gml:upperCorner>",
            "</gml:Envelope></gml:boundedBy>"
        ),
    );
    let observation = scan(&document);
    let root = &observation.layers[&layer("Parcel")].root;
    let geom = root
        .children
        .iter()
        .find(|(name, _)| &*name.local == "geom")
        .and_then(|(_, child)| child.geometry.as_ref())
        .expect("a geometry column");
    assert_eq!(geom.srs.get("EPSG:2180"), Some(&1), "{:?}", geom.srs);
    assert_eq!(geom.srs.get("EPSG:2176"), Some(&1), "{:?}", geom.srs);
    assert!(geom.dims.contains(&3), "the collection's srsDimension: {:?}", geom.dims);
    assert!(geom.dims.contains(&2), "the feature's srsDimension: {:?}", geom.dims);
    // The collection's envelope is the dataset's declared extent.
    assert_eq!(observation.extent, Some([0.0, 0.0, 10.0, 20.0]));
}
