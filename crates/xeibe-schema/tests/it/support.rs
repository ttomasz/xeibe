//! Helpers shared by the `xeibe-schema` tests.

#![allow(dead_code)]

use arrow_schema::{DataType, Field, Schema};
use xeibe_core::{QName, Source, Sources};
use xeibe_schema::{
    DatasetObservation, InferenceOptions, LayerSchema, SampleOptions, ScanExtent, ScanOptions,
    Scanner, infer_schema,
};
use xeibe_testkit::gml::APP;

/// Scan a document held in memory.
pub fn scan(document: &str) -> DatasetObservation {
    scan_with(document, ScanOptions::default())
}

pub fn scan_with(document: &str, options: ScanOptions) -> DatasetObservation {
    let source = Source::reader(
        "test.gml",
        Box::new(std::io::Cursor::new(document.as_bytes().to_vec())),
    );
    Scanner::new(options)
        .run(Sources::from(source))
        .expect("the document scans")
}

/// Scan a file below `tests/data`.
pub fn scan_file(relative: &str) -> DatasetObservation {
    let source = Source::file(xeibe_testkit::data_dir().join(relative)).expect("a file source");
    Scanner::new(ScanOptions::default())
        .run(Sources::from(source))
        .expect("the sample scans")
}

pub fn sample_scan(max_features: u64, document: &str) -> DatasetObservation {
    scan_with(
        document,
        ScanOptions {
            extent: ScanExtent::Sample { max_features },
            ..ScanOptions::default()
        },
    )
}

/// A layer name in the synthetic documents' application namespace.
pub fn layer(local: &str) -> QName {
    QName::new(Some(APP), local)
}

/// Infer the schema of one layer with the given options.
pub fn layer_schema(
    observation: &DatasetObservation,
    local: &str,
    options: &InferenceOptions,
) -> LayerSchema {
    infer_schema(observation, &layer(local), options, None)
        .unwrap_or_else(|e| panic!("inferring {local}: {e}"))
}

/// The Arrow schema of one layer with default options.
pub fn schema(document: &str, local: &str) -> Schema {
    layer_schema(&scan(document), local, &InferenceOptions::default()).schema
}

/// The Arrow schema of a sampled scan, with the conservative rules applied.
pub fn sampled_schema(document: &str, local: &str, sample: &SampleOptions) -> Schema {
    let observation = scan(document);
    infer_schema(
        &observation,
        &layer(local),
        &InferenceOptions::default(),
        Some(sample),
    )
    .expect("a sampled schema")
    .schema
}

/// A field by name, or a helpful panic listing what there is.
#[track_caller]
pub fn field<'a>(schema: &'a Schema, name: &str) -> &'a Field {
    schema.fields().iter().find(|f| f.name() == name).map(|f| f.as_ref()).unwrap_or_else(|| {
        panic!(
            "no column {name:?}; the schema has {:?}",
            column_names(schema)
        )
    })
}

#[track_caller]
pub fn data_type(schema: &Schema, name: &str) -> DataType {
    field(schema, name).data_type().clone()
}

pub fn column_names(schema: &Schema) -> Vec<String> {
    schema.fields().iter().map(|f| f.name().clone()).collect()
}

/// A nested field of a struct column, by dotted path (`idIIP.lokalnyId`).
#[track_caller]
pub fn nested(schema: &Schema, path: &str) -> Field {
    let mut parts = path.split('.');
    let first = parts.next().expect("a path");
    let mut current = field(schema, first).clone();
    for part in parts {
        let children = match current.data_type() {
            DataType::Struct(fields) => fields.clone(),
            DataType::List(inner) | DataType::LargeList(inner) => match inner.data_type() {
                DataType::Struct(fields) => fields.clone(),
                other => panic!("{path}: list items are {other}"),
            },
            other => panic!("{path}: {} is {other}, not a struct", current.name()),
        };
        current = children
            .iter()
            .find(|f| f.name() == part)
            .unwrap_or_else(|| {
                panic!(
                    "{path}: no child {part:?} in {:?}",
                    children.iter().map(|f| f.name()).collect::<Vec<_>>()
                )
            })
            .as_ref()
            .clone();
    }
    current
}

/// The GeoArrow extension type of a geometry column, from its field metadata.
pub fn extension_name(field: &Field) -> Option<&str> {
    field
        .metadata()
        .get("ARROW:extension:name")
        .map(String::as_str)
}
