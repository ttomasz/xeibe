//! The feature-boundary splitter (`docs/architecture.md`,
//! "Feature-boundary splitter").
//!
//! The splitter never parses features. It recognizes member containers, cuts
//! the stream into chunks of whole features at the first boundary after
//! `target_chunk_bytes`, and gives every chunk the namespace declarations it
//! needs to be parsed on its own.

use xeibe_core::{Error, MemberRule, QName, SplitterOptions, ns};
use xeibe_testkit::gml;

use crate::support::{feature_names, features, one_feature_per_chunk, split, try_split};

fn parcels(count: usize) -> Vec<String> {
    (0..count)
        .map(|i| gml::feature("Parcel", &format!("p{i}"), &format!("<app:area>{i}</app:area>")))
        .collect()
}

fn refs(features: &[String]) -> Vec<&str> {
    features.iter().map(String::as_str).collect()
}

#[test]
fn defaults_follow_the_documented_ranges() {
    let options = SplitterOptions::default();
    assert!(
        (16 << 20..=64 << 20).contains(&options.target_chunk_bytes),
        "the planned default chunk size is 16–64 MB, got {}",
        options.target_chunk_bytes
    );
    assert!(
        options.layers.is_none(),
        "a scan reads every layer unless a read sets a filter"
    );
    // The built-in rules cover the containers every GML/WFS version uses.
    let rules = &options.member_rules;
    let has = |rule: MemberRule| rules.contains(&rule);
    assert!(has(MemberRule::OnePerElement(QName::new(
        Some(ns::GML_32),
        "featureMember"
    ))));
    assert!(has(MemberRule::OnePerElement(QName::new(
        Some(ns::GML),
        "featureMember"
    ))));
    assert!(has(MemberRule::ManyPerElement(QName::new(
        Some(ns::GML_32),
        "featureMembers"
    ))));
    assert!(has(MemberRule::OnePerElement(QName::new(
        Some(ns::WFS_20),
        "member"
    ))));
}

#[test]
fn splits_feature_members() {
    let features = parcels(3);
    let document = gml::gml32_collection(&refs(&features));
    let chunks = split(&document, one_feature_per_chunk());
    assert_eq!(feature_names(&chunks), ["Parcel", "Parcel", "Parcel"]);
    assert_eq!(chunks.len(), 3, "one chunk per feature at this chunk size");
}

#[test]
fn splits_a_feature_members_container() {
    let features = parcels(3);
    let document = gml::gml32_collection_members(&refs(&features));
    let chunks = split(&document, one_feature_per_chunk());
    assert_eq!(feature_names(&chunks), ["Parcel", "Parcel", "Parcel"]);
}

#[test]
fn splits_wfs_20_members_and_wfs_11_feature_members() {
    let features = parcels(2);
    let wfs20 = gml::wfs20_collection(
        r#"numberMatched="unknown" numberReturned="2""#,
        &refs(&features),
    );
    assert_eq!(feature_names(&split(&wfs20, one_feature_per_chunk())), ["Parcel"; 2]);

    let wfs11 = gml::wfs11_collection(r#"numberOfFeatures="2""#, &refs(&features));
    assert_eq!(feature_names(&split(&wfs11, one_feature_per_chunk())), ["Parcel"; 2]);
}

#[test]
fn reads_the_document_header_before_the_first_feature() {
    let features = parcels(1);
    let document = gml::collection(
        ns::GML_32,
        "gml:featureMember",
        &refs(&features),
        r#" xsi:schemaLocation="http://example.com/app app.xsd""#,
        "",
    );
    let mut splitter =
        xeibe_core::FeatureSplitter::new(document.as_bytes(), xeibe_core::SourceId(0), SplitterOptions::default());
    let header = splitter.header().expect("a header");
    assert_eq!(header.root, QName::new(Some(ns::GML_32), "FeatureCollection"));
    assert_eq!(
        header.schema_location.as_deref(),
        Some("http://example.com/app app.xsd")
    );
    assert!(!header.fme_produced);
    assert_eq!(
        header.namespaces.resolve_prefix(Some("app")),
        Some(gml::APP)
    );
}

#[test]
fn header_keeps_the_wfs_response_attributes() {
    // Paging reads these (`docs/wfs.md`).
    let features = parcels(2);
    let document = gml::wfs20_collection(
        r#"numberMatched="17" numberReturned="2" next="http://example.com/next""#,
        &refs(&features),
    );
    let mut splitter =
        xeibe_core::FeatureSplitter::new(document.as_bytes(), xeibe_core::SourceId(0), SplitterOptions::default());
    let header = splitter.header().expect("a header");
    let attribute = |local: &str| {
        header
            .wfs_attributes
            .iter()
            .find(|(name, _)| &*name.local == local)
            .map(|(_, value)| value.as_str())
    };
    assert_eq!(attribute("numberMatched"), Some("17"));
    assert_eq!(attribute("numberReturned"), Some("2"));
    assert_eq!(attribute("next"), Some("http://example.com/next"));
}

#[test]
fn header_flags_fme_produced_documents() {
    // The FME namespace on the root changes the axis-order decision.
    let features = parcels(1);
    let document = gml::fme_collection(&refs(&features));
    let mut splitter =
        xeibe_core::FeatureSplitter::new(document.as_bytes(), xeibe_core::SourceId(0), SplitterOptions::default());
    assert!(splitter.header().expect("a header").fme_produced);
}

#[test]
fn chunks_carry_sequence_numbers_and_offsets() {
    let features = parcels(4);
    let document = gml::gml32_collection(&refs(&features));
    let chunks = split(&document, one_feature_per_chunk());
    let seqs: Vec<u64> = chunks.iter().map(|c| c.seq).collect();
    assert_eq!(seqs, [0, 1, 2, 3], "chunks are numbered in order");
    let first_features: Vec<u64> = chunks.iter().map(|c| c.first_feature_seq).collect();
    assert_eq!(first_features, [0, 1, 2, 3]);
    assert!(
        chunks.windows(2).all(|w| w[0].byte_offset < w[1].byte_offset),
        "byte offsets grow: {:?}",
        chunks.iter().map(|c| c.byte_offset).collect::<Vec<_>>()
    );
    assert!(chunks.iter().all(|c| c.source == xeibe_core::SourceId(0)));
}

#[test]
fn a_large_target_puts_every_feature_in_one_chunk() {
    let features = parcels(5);
    let document = gml::gml32_collection(&refs(&features));
    let chunks = split(&document, SplitterOptions::default());
    assert_eq!(chunks.len(), 1, "a 16+ MB target holds this document");
    assert_eq!(feature_names(&chunks).len(), 5);
    assert_eq!(chunks[0].first_feature_seq, 0);
}

#[test]
fn a_single_feature_larger_than_the_target_becomes_its_own_chunk() {
    let big = "x".repeat(4096);
    let features: Vec<String> = (0..3)
        .map(|i| gml::feature("Parcel", &format!("p{i}"), &format!("<app:note>{big}</app:note>")))
        .collect();
    let document = gml::gml32_collection(&refs(&features));
    let chunks = split(
        &document,
        SplitterOptions {
            target_chunk_bytes: 512,
            ..SplitterOptions::default()
        },
    );
    assert_eq!(chunks.len(), 3);
    assert_eq!(feature_names(&chunks), ["Parcel"; 3]);
}

#[test]
fn a_layer_filter_skips_other_feature_types() {
    // Every read sets a filter; features of other layers never enter a chunk
    // (`docs/architecture.md`, "Layers").
    let members = [
        gml::feature("Road", "r1", "<app:name>A</app:name>"),
        gml::feature("Parcel", "p1", "<app:area>1</app:area>"),
        gml::feature("Road", "r2", "<app:name>B</app:name>"),
        gml::feature("Parcel", "p2", "<app:area>2</app:area>"),
    ];
    let document = gml::gml32_collection(&refs(&members));
    let options = SplitterOptions {
        layers: Some(vec![QName::new(Some(gml::APP), "Parcel")]),
        ..one_feature_per_chunk()
    };
    let chunks = split(&document, options);
    assert_eq!(feature_names(&chunks), ["Parcel", "Parcel"]);
    // Feature sequence numbers count the features of the read, so the second
    // Parcel is the second feature this read sees.
    assert_eq!(
        chunks.iter().map(|c| c.first_feature_seq).collect::<Vec<_>>(),
        [0, 1]
    );
}

#[test]
fn nested_features_are_not_members_of_the_collection() {
    // A feature inside a property is part of its parent feature, not a layer of
    // its own: only members of the collection are features here.
    let inner = gml::feature("Building", "b1", "<app:height>3</app:height>");
    let outer = gml::feature("Parcel", "p1", &format!("<app:has>{inner}</app:has>"));
    let document = gml::gml32_collection(&[&outer]);
    let chunks = split(&document, one_feature_per_chunk());
    assert_eq!(feature_names(&chunks), ["Parcel"]);
}

#[test]
fn comments_cdata_and_attributes_do_not_confuse_the_splitter() {
    let member = gml::feature(
        "Parcel",
        "p1",
        r#"<app:note title="a &gt; b"><![CDATA[<gml:featureMember>]]></app:note>"#,
    );
    let document = gml::collection(
        ns::GML_32,
        "gml:featureMember",
        &[&member],
        "",
        "<!-- <gml:featureMember><app:Parcel/></gml:featureMember> -->\n",
    );
    let chunks = split(&document, one_feature_per_chunk());
    assert_eq!(feature_names(&chunks), ["Parcel"]);
}

#[test]
fn chunks_inherit_namespaces_declared_on_ancestors_of_the_members() {
    // The prefix is declared on the container, not on the features.
    let document = concat!(
        r#"<?xml version="1.0"?>"#,
        r#"<gml:FeatureCollection xmlns:gml="http://www.opengis.net/gml/3.2">"#,
        r#"<gml:featureMembers xmlns:app="http://example.com/app">"#,
        r#"<app:Parcel gml:id="p1"/><app:Parcel gml:id="p2"/>"#,
        r#"</gml:featureMembers></gml:FeatureCollection>"#
    );
    let chunks = split(document, one_feature_per_chunk());
    assert_eq!(
        features(&chunks),
        vec![
            QName::new(Some(gml::APP), "Parcel"),
            QName::new(Some(gml::APP), "Parcel")
        ]
    );
}

#[test]
fn a_feature_as_the_document_root_is_a_one_feature_dataset() {
    let document = gml::single_feature_document("Parcel", "p1", "<app:area>1</app:area>");
    let options = SplitterOptions {
        allow_single_feature_root: true,
        ..one_feature_per_chunk()
    };
    assert_eq!(feature_names(&split(&document, options)), ["Parcel"]);
}

#[test]
fn a_document_without_features_is_an_error() {
    // e.g. ISO metadata in a zip member (`docs/architecture.md`, "Zip archives").
    let document = concat!(
        r#"<gmd:MD_Metadata xmlns:gmd="http://www.isotc211.org/2005/gmd">"#,
        r#"<gmd:fileIdentifier/></gmd:MD_Metadata>"#
    );
    let options = SplitterOptions {
        allow_single_feature_root: false,
        ..SplitterOptions::default()
    };
    let error = try_split(document, options).expect_err("no features");
    assert!(matches!(error, Error::NoFeatures(_)), "got {error:?}");
}

#[test]
fn an_empty_collection_is_not_an_error() {
    // `resultType=hits` responses have a collection but no members.
    let document = gml::wfs20_collection(r#"numberMatched="42" numberReturned="0""#, &[]);
    let chunks = split(&document, SplitterOptions::default());
    assert!(feature_names(&chunks).is_empty());
}

#[test]
fn a_custom_member_rule_covers_application_collections() {
    let member = gml::feature("Parcel", "p1", "");
    let document = format!(
        r#"<app:Dataset xmlns:app="{}" xmlns:gml="{}"><app:contains>{member}</app:contains></app:Dataset>"#,
        gml::APP,
        ns::GML_32
    );
    let options = SplitterOptions {
        member_rules: vec![MemberRule::OnePerElement(QName::new(
            Some(gml::APP),
            "contains",
        ))],
        ..one_feature_per_chunk()
    };
    assert_eq!(feature_names(&split(&document, options)), ["Parcel"]);
}
