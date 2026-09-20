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
