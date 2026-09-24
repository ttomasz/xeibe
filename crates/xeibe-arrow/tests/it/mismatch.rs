//! Data that doesn't fit the schema (`docs/schema-inference.md` §6.3).
//!
//! What the schema describes is read, everything else is not: content outside
//! the schema is skipped without a trace, and a value that doesn't fit a
//! column the schema does have is a feature error, handled by
//! `OnFeatureError`.

use std::collections::HashMap;

use arrow_schema::{DataType, Field};
use xeibe_arrow::{OnFeatureError, ReadOptions, read};
use xeibe_schema::rules::meta;
use xeibe_testkit::gml;

use crate::support::{read_with, schema_of, sources};

fn parcel(id: &str, body: &str) -> String {
    gml::feature("Parcel", id, body)
}

fn area_schema() -> arrow_schema::SchemaRef {
    schema_of(vec![Field::new("area", DataType::Int64, true)])
}

fn fails(document: &str, schema: arrow_schema::SchemaRef, options: &ReadOptions) -> bool {
    match read(sources(document), "Parcel", Some(schema), options) {
        Err(_) => true,
        Ok(reader) => reader.into_iter().any(|batch| batch.is_err()),
    }
}

fn skipping() -> ReadOptions {
    ReadOptions {
        on_feature_error: OnFeatureError::Skip,
        ..ReadOptions::default()
    }
}

#[test]
fn content_outside_the_schema_is_not_read() {
    let document = gml::gml32_collection(&[
        &parcel("p1", "<app:area>1</app:area><app:extra>x</app:extra>"),
        &parcel("p2", "<app:area>2</app:area><app:deep><app:x>1</app:x></app:deep>"),
    ]);
    let read = read_with(&document, "Parcel", Some(area_schema()), &ReadOptions::default());
    assert_eq!(read.column_names(), ["area"], "no overflow column");
    assert_eq!(read.i64s("area"), [Some(1), Some(2)]);
    assert!(read.report.skipped.is_empty(), "skipping content is not an error");
    assert!(
        read.report.warnings.is_empty(),
        "and it is not reported: {:?}",
        read.report.warnings
    );
}

#[test]
fn a_value_that_does_not_parse_is_a_feature_error() {
    let document = gml::gml32_collection(&[
        &parcel("p1", "<app:area>1</app:area>"),
        &parcel("p2", "<app:area>huge</app:area>"),
    ]);
    assert!(
        fails(&document, area_schema(), &ReadOptions::default()),
        "the default policy stops the read"
    );
    let read = read_with(&document, "Parcel", Some(area_schema()), &skipping());
    assert_eq!(read.i64s("area"), [Some(1)], "the feature is skipped, not nulled");
    assert_eq!(read.report.skipped.len(), 1);
}

#[test]
fn a_repeated_element_in_a_scalar_column_is_a_feature_error() {
    // The schema has the wrong type for the data: never keep one value and
    // silently drop the other.
    let document = gml::gml32_collection(&[&parcel(
        "p1",
        "<app:area>1</app:area><app:area>2</app:area>",
    )]);
    assert!(fails(&document, area_schema(), &ReadOptions::default()));
    let read = read_with(&document, "Parcel", Some(area_schema()), &skipping());
    assert_eq!(read.rows(), 0);
    assert_eq!(read.report.skipped.len(), 1);
}

#[test]
fn a_later_repetition_after_a_sampled_schema_is_a_feature_error() {
    // A sample saw `n` once per feature, so `n` is a scalar column.
    let document = gml::gml32_collection(&[
        &parcel("p1", "<app:n>a</app:n>"),
        &parcel("p2", "<app:n>b</app:n><app:n>c</app:n>"),
    ]);
    let options = ReadOptions {
        on_feature_error: OnFeatureError::Skip,
        sample: xeibe_schema::SampleOptions {
            features_per_layer: 1,
            ..xeibe_schema::SampleOptions::default()
        },
        ..ReadOptions::default()
    };
    let read = read_with(&document, "Parcel", None, &options);
    assert_eq!(read.strings("n"), [Some("a".to_string())]);
    assert_eq!(read.report.skipped.len(), 1);
}

#[test]
fn a_geometry_kind_the_column_cannot_hold_is_a_geometry_error() {
    let ring = "<gml:exterior><gml:LinearRing><gml:posList>0 0 1 0 1 1 0 0</gml:posList></gml:LinearRing></gml:exterior>";
    let document = gml::gml32_collection(&[&parcel(
        "p1",
        &format!(
            "<app:area>1</app:area><app:geom><gml:MultiSurface srsName=\"EPSG:2180\"><gml:surfaceMember><gml:Polygon>{ring}</gml:Polygon></gml:surfaceMember></gml:MultiSurface></app:geom>"
        ),
    )]);
    let settings = xeibe_arrow::Settings {
        format_version: xeibe_arrow::Settings::FORMAT_VERSION,
        options: ReadOptions::default(),
        layers: [(
            "Parcel".to_string(),
            [
                ("area".to_string(), xeibe_arrow::ColumnSpec::Type("bigint".into())),
                ("geom".to_string(), xeibe_arrow::ColumnSpec::Type("geometry(Polygon)".into())),
            ]
            .into_iter()
            .collect(),
        )]
        .into_iter()
        .collect(),
    };
    let schema = settings.schema("Parcel").expect("a schema");
    assert!(fails(&document, schema.clone(), &ReadOptions::default()));

    let null_geometry = ReadOptions {
        on_feature_error: OnFeatureError::NullGeometry,
        ..ReadOptions::default()
    };
    let read = read_with(&document, "Parcel", Some(schema), &null_geometry);
    assert_eq!(read.i64s("area"), [Some(1)], "the attributes are kept");
}

#[test]
fn an_unsupported_geometry_is_a_geometry_error() {
    // No separate policy: `OnFeatureError` decides (`docs/geometry.md`,
    // "Unsupported geometry").
    let document = gml::gml32_collection(&[&parcel(
        "p1",
        concat!(
            "<app:area>1</app:area><app:geom><gml:Tin><gml:patches><gml:Triangle><gml:exterior><gml:LinearRing>",
            "<gml:posList srsDimension=\"3\">0 0 1 0 1 1 1 1 1 0 0 1</gml:posList>",
            "</gml:LinearRing></gml:exterior></gml:Triangle></gml:patches></gml:Tin></app:geom>"
        ),
    )]);
    let schema = schema_of(vec![
        Field::new("area", DataType::Int64, true),
        Field::new("geom", DataType::Binary, true).with_metadata(HashMap::from([(
            "ARROW:extension:name".to_string(),
            "geoarrow.wkb".to_string(),
        )])),
    ]);
    assert!(fails(&document, schema.clone(), &ReadOptions::default()));

    let null_geometry = ReadOptions {
        on_feature_error: OnFeatureError::NullGeometry,
        ..ReadOptions::default()
    };
    let read = read_with(&document, "Parcel", Some(schema), &null_geometry);
    assert_eq!(read.i64s("area"), [Some(1)]);
    assert_eq!(read.geometries("geom"), [None]);
}

#[test]
fn a_text_column_at_an_element_with_children_gets_its_raw_xml() {
    // How to keep the source GML of a geometry (`docs/geometry.md`, "Options").
    let document = gml::gml32_collection(&[&parcel(
        "p1",
        "<app:geom><gml:Point srsName=\"EPSG:2180\"><gml:pos>1 2</gml:pos></gml:Point></app:geom>",
    )]);
    let schema = schema_of(vec![Field::new("geom_gml", DataType::Utf8View, true).with_metadata(
        HashMap::from([(meta::PATH.to_string(), "geom".to_string())]),
    )]);
    let read = read_with(&document, "Parcel", Some(schema), &ReadOptions::default());
    let xml = read.strings("geom_gml")[0].clone().expect("the raw XML");
    assert!(xml.contains("Point") && xml.contains("1 2"), "{xml}");
}
