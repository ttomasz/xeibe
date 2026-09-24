//! Binding a given schema to XML paths (`docs/architecture.md`,
//! "Settings file", "Paths").
//!
//! A read only matches paths. A column's path is its `gml:path` metadata;
//! without one, the name is the path. The read applies no naming rules and
//! looks through nothing: a type wrapper is `*` in the path.

use std::collections::HashMap;

use arrow_schema::{DataType, Field, Schema};
use xeibe_schema::rules::meta;
use xeibe_schema::{InferenceOptions, bind_schema};
use xeibe_testkit::gml::APP;

use crate::support::layer;

fn field(name: &str, data_type: DataType) -> Field {
    Field::new(name, data_type, true)
}

fn bind(schema: Schema) -> xeibe_schema::Result<xeibe_schema::LayerSchema> {
    bind_schema(&layer("Parcel"), &schema, &InferenceOptions::default())
}

fn with_path(name: &str, data_type: DataType, path: &str) -> Field {
    field(name, data_type).with_metadata(HashMap::from([(meta::PATH.to_string(), path.to_string())]))
}

/// The source path a column is bound to, as local names (`*` for a wildcard
/// step), with the attribute as a last `@name` step.
fn route(bound: &xeibe_schema::LayerSchema, column: &str) -> Vec<String> {
    let index = bound
        .schema
        .fields()
        .iter()
        .position(|f| f.name() == column)
        .unwrap_or_else(|| panic!("no column {column:?}"));
    bound
        .routes
        .iter()
        .find(|route| route.field_path.first() == Some(&index))
        .map(|route| {
            route
                .source_path
                .iter()
                .map(|name| name.local.to_string())
                .chain(route.attribute.iter().map(|a| format!("@{}", a.local)))
                .collect()
        })
        .unwrap_or_else(|| panic!("no route to {column:?}"))
}

#[test]
fn without_a_path_the_name_is_the_path() {
    let bound = bind(Schema::new(vec![
        field("area", DataType::Float64),
        field("@id", DataType::Utf8View),
        field("owner/name", DataType::Utf8View),
    ]))
    .expect("the schema binds");
    assert_eq!(route(&bound, "area"), ["area"]);
    assert_eq!(route(&bound, "@id"), ["@id"], "the feature's gml:id");
    assert_eq!(route(&bound, "owner/name"), ["owner", "name"]);
    assert_eq!(bound.schema.fields().len(), 3, "the schema is used as it is");
    assert_eq!(
        bound.layer.ns.as_deref(),
        Some(APP),
        "the layer keeps its namespace"
    );
    assert!(
        bound.decisions.is_empty(),
        "a given schema has no inference decisions to explain"
    );
}

#[test]
fn a_type_wrapper_is_a_wildcard_step() {
    let bound = bind(Schema::new(vec![with_path(
        "lokalnyId",
        DataType::Utf8View,
        "idIIP/*/lokalnyId",
    )]))
    .expect("the schema binds");
    assert_eq!(route(&bound, "lokalnyId"), ["idIIP", "*", "lokalnyId"]);
}

#[test]
fn a_dotted_name_is_not_a_path() {
    // Names are free; only `/` separates steps, and nothing is looked through.
    let bound = bind(Schema::new(vec![field("idIIP.lokalnyId", DataType::Utf8View)]))
        .expect("the schema binds");
    assert_eq!(route(&bound, "idIIP.lokalnyId"), ["idIIP.lokalnyId"]);
}

#[test]
fn an_attribute_is_the_last_step() {
    let bound = bind(Schema::new(vec![with_path(
        "miejscowosc",
        DataType::Utf8View,
        "miejscowosc/@href",
    )]))
    .expect("the schema binds");
    assert_eq!(route(&bound, "miejscowosc"), ["miejscowosc", "@href"]);
}

#[test]
fn the_anchor_marker_is_not_a_step() {
    let bound = bind(Schema::new(vec![with_path(
        "datum",
        DataType::List(std::sync::Arc::new(Field::new("item", DataType::Date32, true))),
        "externeReferenz[]/*/datum",
    )]))
    .expect("the schema binds");
    assert_eq!(route(&bound, "datum"), ["externeReferenz", "*", "datum"]);
}

#[test]
fn a_path_may_have_one_anchor_only() {
    let bad = Schema::new(vec![with_path(
        "text",
        DataType::List(std::sync::Arc::new(Field::new("item", DataType::Utf8View, true))),
        "name[]/spelling[]/text",
    )]);
    assert!(bind(bad).is_err(), "no lists of lists");
}

#[test]
fn a_prefixed_step_needs_a_declared_namespace() {
    let schema = Schema::new(vec![with_path("x:code", DataType::Utf8View, "x:code")]);
    assert!(bind(schema.clone()).is_err(), "`x` is not declared");

    let declared = schema.with_metadata(HashMap::from([(
        meta::NS.to_string(),
        r#"{"x": "http://example.com/x"}"#.to_string(),
    )]));
    let bound = bind(declared).expect("the prefix is declared in gml:ns");
    let index = 0;
    let step = &bound
        .routes
        .iter()
        .find(|r| r.field_path.first() == Some(&index))
        .expect("a route")
        .source_path[0];
    assert_eq!(step.ns.as_deref(), Some("http://example.com/x"));
}

#[test]
fn a_path_in_the_metadata_renames_a_column() {
    let renamed = field("postcode", DataType::Utf8View).with_metadata(HashMap::from([(
        meta::PATH.to_string(),
        "kodPocztowy".to_string(),
    )]));
    let bound = bind(Schema::new(vec![renamed])).expect("the schema binds");
    assert_eq!(route(&bound, "postcode"), ["kodPocztowy"]);
}

#[test]
fn columns_that_never_match_stay_null() {
    // A settings file may describe data this input does not have.
    let bound = bind(Schema::new(vec![field("nothing_like_this", DataType::Int64)]))
        .expect("an unmatched column is not an error");
    assert_eq!(bound.schema.fields().len(), 1);
}

#[test]
fn a_non_null_field_is_kept_as_given() {
    // Inferred schemas are all-nullable, but a schema from code may not be.
    let schema = Schema::new(vec![Field::new("area", DataType::Float64, false)]);
    let bound = bind(schema).expect("the schema binds");
    assert!(!bound.schema.field(0).is_nullable());
}

#[test]
fn a_geometry_column_keeps_its_extension_type() {
    let geometry = field("geom", DataType::Binary).with_metadata(HashMap::from([(
        "ARROW:extension:name".to_string(),
        "geoarrow.wkb".to_string(),
    )]));
    let bound = bind(Schema::new(vec![geometry])).expect("the schema binds");
    assert_eq!(
        bound.schema.field(0).metadata().get("ARROW:extension:name").map(String::as_str),
        Some("geoarrow.wkb")
    );
}

#[test]
fn a_struct_column_is_an_error() {
    // Schemas are flat; only GeoArrow types are structs inside.
    let nested = Schema::new(vec![field(
        "idIIP",
        DataType::Struct(vec![Field::new("lokalnyId", DataType::Utf8View, true)].into()),
    )]);
    assert!(bind(nested).is_err());
}

#[test]
fn an_unusable_type_is_an_error() {
    // Binding fails when a column cannot be filled from XML at all.
    let bad = Schema::new(vec![field(
        "area",
        DataType::FixedSizeBinary(16),
    )]);
    assert!(bind(bad).is_err());
}
