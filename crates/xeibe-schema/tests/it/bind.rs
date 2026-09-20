//! Binding a given schema to XML paths (`docs/architecture.md`,
//! "How columns are matched to XML").
//!
//! Columns are matched by name, the way spark-xml applies a user schema:
//! naming options turn XML names into column names, type wrappers are looked
//! through, and a `gml:path` in the field metadata wins over the name.

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

/// The source path a column is bound to, as local names.
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
                .collect()
        })
        .unwrap_or_else(|| panic!("no route to {column:?}"))
}

#[test]
fn columns_are_matched_to_elements_by_name() {
    let bound = bind(Schema::new(vec![
        field("area", DataType::Float64),
        field("@id", DataType::Utf8View),
    ]))
    .expect("the schema binds");
    assert_eq!(route(&bound, "area"), ["area"]);
    assert_eq!(bound.schema.fields().len(), 2);
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
fn a_type_wrapper_is_looked_through() {
    // `idIIP/AD_IdentyfikatorIIP/lokalnyId` fills `idIIP.lokalnyId`.
    let inner = Field::new("lokalnyId", DataType::Utf8View, true);
    let bound = bind(Schema::new(vec![field(
        "idIIP",
        DataType::Struct(vec![inner].into()),
    )]))
    .expect("the schema binds");
    let path = route(&bound, "idIIP");
    assert_eq!(path, ["idIIP"], "the column is the property, got {path:?}");
}

#[test]
fn a_path_in_the_metadata_wins_over_the_name() {
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
fn an_unusable_type_is_an_error() {
    // Binding fails when a column cannot be filled from XML at all.
    let bad = Schema::new(vec![field(
        "area",
        DataType::FixedSizeBinary(16),
    )]);
    assert!(bind(bad).is_err());
}
