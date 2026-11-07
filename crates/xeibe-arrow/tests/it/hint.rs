//! A schema hint (a WFS `DescribeFeatureType`'s declared properties) in an
//! inferred read (`docs/wfs.md`, "Schema hint from `DescribeFeatureType`"):
//! it types the properties that never had a value in the sample and adds the
//! ones the sample never saw, and never overrides what the sample shows.

use std::sync::Arc;

use arrow_schema::DataType;
use xeibe_arrow::ReadOptions;
use xeibe_core::QName;
use xeibe_schema::SchemaHint;
use xeibe_schema::hint::{DeclaredProperty, XSD};
use xeibe_schema::options::FieldOverride;
use xeibe_schema::{PathPattern, TypeSet};
use xeibe_testkit::gml;
use xeibe_testkit::wkt::assert_wkt;

use crate::support::{Read, extension_name, read_with};

fn xs(local: &str) -> Option<QName> {
    Some(QName::new(Some(XSD), local))
}

fn property(name: &str, type_name: Option<QName>, repeated: bool) -> DeclaredProperty {
    DeclaredProperty {
        name: QName::new(Some(gml::APP), name),
        type_name,
        repeated,
    }
}

/// `app:Parcel` as its schema declares it: the sampled `code` (as an int),
/// the never-valued `area` and `survey`, and `owner`, `built`, `tags`,
/// `checked`, `site` and `note`, which the sample never sees.
fn hint() -> SchemaHint {
    SchemaHint {
        layer: QName::new(Some(gml::APP), "Parcel"),
        source: "the DescribeFeatureType schema".to_string(),
        properties: vec![
            property("code", xs("int"), false),
            property("area", xs("double"), false),
            property("survey", xs("date"), true),
            property("owner", xs("string"), false),
            property("built", xs("dateTime"), false),
            property("tags", xs("string"), true),
            property("checked", xs("boolean"), false),
            property(
                "site",
                Some(QName::new(Some(gml::GML_32), "PointPropertyType")),
                false,
            ),
            // A complex type: ignored.
            property(
                "address",
                Some(QName::new(Some(gml::APP), "AddressType")),
                false,
            ),
            property("note", None, false),
        ],
    }
}

fn sampled(id: &str) -> String {
    gml::feature(
        "Parcel",
        id,
        r#"<app:code>A1</app:code><app:area xsi:nil="true"/><app:survey/>"#,
    )
}

/// Two sampled features, then one beyond the sample with every property.
fn document() -> String {
    let late = gml::feature(
        "Parcel",
        "p3",
        concat!(
            "<app:code>B2</app:code><app:area>12.5</app:area>",
            "<app:survey>2020-01-02</app:survey><app:survey>2021-03-04</app:survey>",
            "<app:owner>Kowalski</app:owner><app:built>2001-02-03T04:05:06Z</app:built>",
            "<app:tags>a</app:tags><app:tags>b</app:tags><app:checked>true</app:checked>",
            "<app:site><gml:Point srsName=\"EPSG:3857\"><gml:pos>1 2</gml:pos></gml:Point></app:site>",
        ),
    );
    gml::gml32_collection(&[&sampled("p1"), &sampled("p2"), &late])
}

fn options(hint: SchemaHint) -> ReadOptions {
    let mut options = ReadOptions::default();
    options.sample.features_per_layer = 2;
    options.preserve_order = true;
    options.inference.hint = Some(Arc::new(hint));
    options
}

fn read(options: &ReadOptions) -> Read {
    read_with(&document(), "Parcel", None, options)
}

#[test]
fn declared_types_fill_only_what_the_sample_cannot_show() {
    let read = read(&options(hint()));
    // The sampled columns first, then the declared ones the sample never
    // saw, in the order of the declarations; `address` and `note` have no
    // type a hint fills in.
    assert_eq!(
        read.column_names(),
        [
            "@id", "code", "area", "survey", "owner", "built", "tags", "checked", "site"
        ]
    );
    // The sample shows text: the declared int doesn't override it.
    assert!(
        matches!(read.data_type("code"), DataType::Utf8View),
        "{:?}",
        read.data_type("code")
    );
    assert_eq!(read.strings("code")[2].as_deref(), Some("B2"));
    // Never a value in the sample: the declared types.
    assert_eq!(read.data_type("area"), DataType::Float64);
    assert_eq!(read.f64s("area"), [None, None, Some(12.5)]);
    let DataType::List(item) = read.data_type("survey") else {
        panic!(
            "survey is declared to repeat: {:?}",
            read.data_type("survey")
        );
    };
    assert_eq!(item.data_type(), &DataType::Date32);
    // Never in the sample: columns of the declared types.
    assert_eq!(
        read.strings("owner"),
        [None, None, Some("Kowalski".to_string())]
    );
    assert_eq!(read.data_type("checked"), DataType::Boolean);
    assert_eq!(read.bools("checked"), [None, None, Some(true)]);
    assert_eq!(
        read.string_lists("tags"),
        [
            None,
            None,
            Some(vec![Some("a".to_string()), Some("b".to_string())])
        ]
    );
    // A dateTime may or may not carry a time zone: text.
    assert!(matches!(read.data_type("built"), DataType::Utf8View));
    assert_eq!(
        read.strings("built")[2].as_deref(),
        Some("2001-02-03T04:05:06Z")
    );
    // A geometry property type: a WKB geometry column.
    assert_eq!(
        extension_name(&read.field("site")),
        Some("geoarrow.wkb"),
        "{:?}",
        read.field("site")
    );
    let sites = read.geometries("site");
    assert!(sites[0].is_none() && sites[1].is_none());
    assert_wkt(sites[2].as_ref().expect("a point"), "POINT(1 2)");
}

#[test]
fn the_reasons_name_the_declaration() {
    let read = read(&options(hint()));
    // The settings a read can save keep the columns, not the hint.
    let inferred = read.report.inferred.expect("an inferred schema");
    let columns = inferred.layers.values().next().expect("one layer");
    assert!(columns.contains_key("owner"), "{:?}", columns.keys());
    // A scan of the sampled features alone.
    let explain = xeibe_arrow::scan(
        crate::support::sources(&gml::gml32_collection(&[&sampled("p1"), &sampled("p2")])),
        xeibe_schema::ScanExtent::Full,
        &options(hint()),
    )
    .expect("a scan")
    .explain("Parcel")
    .expect("an explanation");
    for expected in [
        "not in the sample: a column because the DescribeFeatureType schema declares it",
        "the DescribeFeatureType schema declares it xs:boolean",
        "no value in the sample, so its type comes from the declaration",
        "the DescribeFeatureType schema declares it xs:dateTime: text",
    ] {
        assert!(explain.contains(expected), "{expected:?} in\n{explain}");
    }
}

#[test]
fn a_hint_for_another_feature_type_is_ignored() {
    let mut other = hint();
    other.layer = QName::new(Some(gml::APP), "Road");
    // Only the sampled features: without the hint, the third one's
    // repeated `survey` doesn't fit the single-value column.
    let document = gml::gml32_collection(&[&sampled("p1"), &sampled("p2")]);
    let read = read_with(&document, "Parcel", None, &options(other));
    assert_eq!(read.column_names(), ["@id", "code", "area", "survey"]);
    assert!(matches!(read.data_type("area"), DataType::Utf8View));
}

#[test]
fn settings_and_type_options_still_apply_to_declared_columns() {
    let mut options = options(hint());
    // A dropped declared column stays out; a typed one takes the settings'
    // type.
    options.inference.overrides = vec![
        (
            PathPattern::parse("Parcel/owner").expect("a pattern"),
            FieldOverride::Drop,
        ),
        (
            PathPattern::parse("Parcel/area").expect("a pattern"),
            FieldOverride::Type(DataType::Utf8),
        ),
    ];
    // Only text: every declared column is text.
    options.inference.types.enabled = TypeSet::STRING;
    let read = read(&options);
    assert!(!read.column_names().contains(&"owner".to_string()));
    assert_eq!(read.data_type("area"), DataType::Utf8);
    assert!(matches!(read.data_type("checked"), DataType::Utf8View));
    let DataType::List(item) = read.data_type("survey") else {
        panic!("{:?}", read.data_type("survey"));
    };
    assert!(matches!(item.data_type(), DataType::Utf8View));
}
