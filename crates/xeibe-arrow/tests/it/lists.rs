//! Lists and their alignment (`docs/schema-inference.md`, "Lists and
//! alignment").
//!
//! Every column under one anchor has one entry per occurrence of the anchor,
//! with null where an occurrence lacks the value, so entry *i* of every such
//! column comes from the anchor's *i*-th occurrence.

use std::collections::HashMap;
use std::sync::Arc;

use arrow_schema::{DataType, Field};
use xeibe_arrow::{OnFeatureError, ReadOptions, read};
use xeibe_schema::rules::meta;
use xeibe_testkit::gml;
use xeibe_testkit::wkt::assert_wkt;

use crate::support::{read_document, read_with, schema_of, sources};

fn parcel(id: &str, body: &str) -> String {
    gml::feature("Parcel", id, body)
}

fn list_column(name: &str, item: DataType, path: &str) -> Field {
    Field::new(name, DataType::List(Arc::new(Field::new("item", item, true))), true)
        .with_metadata(HashMap::from([(meta::PATH.to_string(), path.to_string())]))
}

fn s(text: &str) -> Option<String> {
    Some(text.to_string())
}

#[test]
fn columns_under_one_anchor_stay_aligned() {
    let document = gml::gml32_collection(&[&parcel(
        "p1",
        concat!(
            "<app:adres><app:ulica>Polna</app:ulica><app:numer>1</app:numer></app:adres>",
            "<app:adres><app:numer>2</app:numer></app:adres>"
        ),
    )]);
    let read = read_document(&document, "Parcel");
    assert_eq!(read.string_lists("ulica"), [Some(vec![s("Polna"), None])]);
    // A sample that holds the whole layer types like a full scan: `Int64[]`
    // (`rules::every_column_below_a_repeated_element_is_a_list_anchored_on_it`).
    assert_eq!(read.i64_lists("numer"), [Some(vec![Some(1), Some(2)])]);
}

#[test]
fn a_value_missing_from_the_first_occurrences_gets_leading_nulls() {
    let schema = schema_of(vec![
        list_column("ulica", DataType::Utf8View, "adres[]/ulica"),
        list_column("numer", DataType::Utf8View, "adres[]/numer"),
    ]);
    let document = gml::gml32_collection(&[&parcel(
        "p1",
        concat!(
            "<app:adres><app:numer>1</app:numer></app:adres>",
            "<app:adres><app:numer>2</app:numer></app:adres>",
            "<app:adres><app:ulica>Polna</app:ulica><app:numer>3</app:numer></app:adres>"
        ),
    )]);
    let read = read_with(&document, "Parcel", Some(schema), &ReadOptions::default());
    assert_eq!(read.string_lists("ulica"), [Some(vec![None, None, s("Polna")])]);
    assert_eq!(read.string_lists("numer"), [Some(vec![s("1"), s("2"), s("3")])]);
}

#[test]
fn a_value_missing_from_the_last_occurrences_gets_trailing_nulls() {
    let schema = schema_of(vec![
        list_column("ulica", DataType::Utf8View, "adres[]/ulica"),
        list_column("numer", DataType::Utf8View, "adres[]/numer"),
    ]);
    let document = gml::gml32_collection(&[&parcel(
        "p1",
        concat!(
            "<app:adres><app:ulica>Polna</app:ulica><app:numer>1</app:numer></app:adres>",
            "<app:adres><app:numer>2</app:numer></app:adres>",
            "<app:adres/>"
        ),
    )]);
    let read = read_with(&document, "Parcel", Some(schema), &ReadOptions::default());
    assert_eq!(read.string_lists("ulica"), [Some(vec![s("Polna"), None, None])]);
    assert_eq!(
        read.string_lists("numer"),
        [Some(vec![s("1"), s("2"), None])],
        "an empty occurrence gives null in every column"
    );
}

#[test]
fn an_attribute_of_the_anchor_is_aligned_with_its_text() {
    // `name` and `name/@codeSpace`: the counter goes up before the attributes
    // are read.
    let schema = schema_of(vec![
        list_column("name", DataType::Utf8View, "name[]"),
        list_column("@codeSpace", DataType::Utf8View, "name[]/@codeSpace"),
    ]);
    let document = gml::gml32_collection(&[&parcel(
        "p1",
        r#"<app:name>Warsaw</app:name><app:name codeSpace="PRNG">Warszawa</app:name>"#,
    )]);
    let read = read_with(&document, "Parcel", Some(schema), &ReadOptions::default());
    assert_eq!(read.string_lists("name"), [Some(vec![s("Warsaw"), s("Warszawa")])]);
    assert_eq!(read.string_lists("@codeSpace"), [Some(vec![None, s("PRNG")])]);
}

#[test]
fn a_feature_without_the_anchor_gets_null_not_an_empty_list() {
    let schema = schema_of(vec![list_column("numer", DataType::Utf8View, "adres[]/numer")]);
    let document = gml::gml32_collection(&[
        &parcel("p1", "<app:adres><app:numer>1</app:numer></app:adres>"),
        &parcel("p2", "<app:other>x</app:other>"),
    ]);
    let read = read_with(&document, "Parcel", Some(schema), &ReadOptions::default());
    assert_eq!(read.string_lists("numer"), [Some(vec![s("1")]), None]);
}

#[test]
fn a_second_value_within_one_occurrence_is_a_feature_error() {
    let schema = schema_of(vec![list_column("numer", DataType::Utf8View, "adres[]/numer")]);
    let document = gml::gml32_collection(&[
        &parcel("p1", "<app:adres><app:numer>1</app:numer><app:numer>2</app:numer></app:adres>"),
        &parcel("p2", "<app:adres><app:numer>3</app:numer></app:adres>"),
    ]);
    let failed = match read(sources(&document), "Parcel", Some(schema.clone()), &ReadOptions::default()) {
        Err(_) => true,
        Ok(reader) => reader.into_iter().any(|batch| batch.is_err()),
    };
    assert!(failed, "it would shift every later entry");

    let skipping = ReadOptions {
        on_feature_error: OnFeatureError::Skip,
        ..ReadOptions::default()
    };
    let read = read_with(&document, "Parcel", Some(schema), &skipping);
    assert_eq!(read.string_lists("numer"), [Some(vec![s("3")])]);
}

#[test]
fn a_hand_written_list_without_a_marker_is_anchored_on_its_first_step() {
    let schema = schema_of(vec![
        list_column("ulica", DataType::Utf8View, "adres/ulica"),
        list_column("numer", DataType::Utf8View, "adres/numer"),
    ]);
    let document = gml::gml32_collection(&[&parcel(
        "p1",
        concat!(
            "<app:adres><app:numer>1</app:numer></app:adres>",
            "<app:adres><app:ulica>Polna</app:ulica><app:numer>2</app:numer></app:adres>"
        ),
    )]);
    let read = read_with(&document, "Parcel", Some(schema), &ReadOptions::default());
    assert_eq!(read.string_lists("ulica"), [Some(vec![None, s("Polna")])]);
}

#[test]
fn an_inner_anchor_counts_across_the_outer_occurrences() {
    // Repetition at two levels: `text` is anchored on `spelling`, so it keeps
    // every spelling of every name, aligned with the other spelling columns.
    let document = gml::gml32_collection(&[&parcel(
        "p1",
        concat!(
            "<app:name><app:language>pol</app:language>",
            "<app:spelling><app:text>Łódź</app:text><app:script>Latn</app:script></app:spelling>",
            "<app:spelling><app:text>Lodz</app:text></app:spelling></app:name>",
            "<app:name><app:language>deu</app:language>",
            "<app:spelling><app:text>Lodsch</app:text></app:spelling></app:name>"
        ),
    )]);
    let read = read_document(&document, "Parcel");
    assert_eq!(read.string_lists("language"), [Some(vec![s("pol"), s("deu")])]);
    assert_eq!(
        read.string_lists("text"),
        [Some(vec![s("Łódź"), s("Lodz"), s("Lodsch")])]
    );
    assert_eq!(read.string_lists("script"), [Some(vec![s("Latn"), None, None])]);
}

#[test]
fn a_geometry_below_a_repeated_element_is_a_list_of_wkb() {
    let object = |id: &str, x: &str, foo: &str| {
        format!(
            concat!(
                "<app:content><app:Object gml:id=\"{}\"><app:geometry>",
                "<gml:Point srsName=\"EPSG:2180\"><gml:pos>{} 2</gml:pos></gml:Point>",
                "</app:geometry><app:foo>{}</app:foo></app:Object></app:content>"
            ),
            id, x, foo
        )
    };
    let document = gml::gml32_collection(&[&parcel(
        "p1",
        &format!("{}{}", object("o1", "10", "bar"), object("o2", "20", "baz")),
    )]);
    let read = read_document(&document, "Parcel");
    assert_eq!(read.string_lists("foo"), [Some(vec![s("bar"), s("baz")])]);
    let geometries = read.geometry_lists("geometry");
    let row = geometries[0].as_ref().expect("a list");
    assert_eq!(row.len(), 2, "one geometry per `content`, aligned with `foo`");
    assert_wkt(row[0].as_ref().expect("a geometry"), "POINT (10 2)");
    assert_wkt(row[1].as_ref().expect("a geometry"), "POINT (20 2)");
}
