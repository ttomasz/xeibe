//! Data that doesn't fit the schema (`docs/schema-inference.md` §6.3).
//!
//! Any schema a read uses — sampled, from an old settings file or written by
//! hand — can meet data it doesn't describe. `OnSchemaMismatch` decides.

use arrow_schema::{DataType, Field};
use xeibe_arrow::{ReadOptions, read};
use xeibe_schema::OnSchemaMismatch;
use xeibe_testkit::gml;

use crate::support::{read_with, schema_of, sources};

fn document() -> String {
    gml::gml32_collection(&[
        &gml::feature("Parcel", "p1", "<app:area>1</app:area><app:extra>x</app:extra>"),
        &gml::feature("Parcel", "p2", "<app:area>2</app:area>"),
    ])
}

fn given_schema() -> arrow_schema::SchemaRef {
    schema_of(vec![Field::new("area", DataType::Int64, true)])
}

fn options(on_mismatch: OnSchemaMismatch) -> ReadOptions {
    ReadOptions {
        on_mismatch,
        ..ReadOptions::default()
    }
}

#[test]
fn unknown_elements_go_to_the_overflow_column() {
    let read = read_with(
        &document(),
        "Parcel",
        Some(given_schema()),
        &options(OnSchemaMismatch::Overflow),
    );
    assert_eq!(read.column_names(), ["area", "_overflow"]);
    assert_eq!(read.data_type("area"), DataType::Int64);
    assert!(
        matches!(read.data_type("_overflow"), DataType::Map(_, _)),
        "a string → string map"
    );

    let overflow = read.overflow();
    assert_eq!(overflow[0], [("extra".to_string(), "x".to_string())]);
    assert!(overflow[1].is_empty(), "nothing unexpected in the second row");
    assert_eq!(read.report.overflow_per_path.get("extra"), Some(&1));
}

#[test]
fn a_value_that_does_not_parse_lands_in_the_overflow_too() {
    let document = gml::gml32_collection(&[&gml::feature("Parcel", "p1", "<app:area>huge</app:area>")]);
    let read = read_with(
        &document,
        "Parcel",
        Some(given_schema()),
        &options(OnSchemaMismatch::Overflow),
    );
    assert_eq!(read.i64s("area"), [None], "the column is null");
    assert_eq!(read.overflow()[0], [("area".to_string(), "huge".to_string())]);
}

#[test]
fn an_unexpected_repetition_keeps_the_first_value() {
    let document = gml::gml32_collection(&[&gml::feature(
        "Parcel",
        "p1",
        "<app:area>1</app:area><app:area>2</app:area>",
    )]);
    let read = read_with(
        &document,
        "Parcel",
        Some(given_schema()),
        &options(OnSchemaMismatch::Overflow),
    );
    assert_eq!(read.i64s("area"), [Some(1)]);
    assert_eq!(read.overflow()[0].len(), 1, "the rest goes to the overflow");
}

#[test]
fn dropping_keeps_the_schema_clean() {
    let read = read_with(
        &document(),
        "Parcel",
        Some(given_schema()),
        &options(OnSchemaMismatch::Drop),
    );
    assert_eq!(read.column_names(), ["area"], "no overflow column");
    assert_eq!(read.i64s("area"), [Some(1), Some(2)]);
}

#[test]
fn erroring_stops_the_read() {
    let reader = read(
        sources(&document()),
        "Parcel",
        Some(given_schema()),
        &options(OnSchemaMismatch::Error),
    );
    let failed = match reader {
        Err(_) => true,
        Ok(reader) => reader.into_iter().any(|batch| batch.is_err()),
    };
    assert!(failed, "unknown data is an error in this mode");
}

#[test]
fn an_unused_overflow_column_stays_null() {
    let document = gml::gml32_collection(&[&gml::feature("Parcel", "p1", "<app:area>1</app:area>")]);
    let read = read_with(
        &document,
        "Parcel",
        Some(given_schema()),
        &options(OnSchemaMismatch::Overflow),
    );
    assert!(read.overflow()[0].is_empty());
    assert!(read.report.overflow_per_path.is_empty());
}

#[test]
fn a_schema_that_already_has_an_overflow_column_does_not_get_a_second_one() {
    let schema = schema_of(vec![
        Field::new("area", DataType::Int64, true),
        Field::new(
            "_overflow",
            DataType::Map(
                std::sync::Arc::new(Field::new(
                    "entries",
                    DataType::Struct(
                        vec![
                            Field::new("key", DataType::Utf8View, false),
                            Field::new("value", DataType::Utf8View, true),
                        ]
                        .into(),
                    ),
                    false,
                )),
                false,
            ),
            true,
        ),
    ]);
    let read = read_with(
        &document(),
        "Parcel",
        Some(schema),
        &options(OnSchemaMismatch::Overflow),
    );
    assert_eq!(read.column_names(), ["area", "_overflow"]);
    assert_eq!(read.overflow()[0].len(), 1);
}
