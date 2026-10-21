use std::path::Path;

use arrow_array::{RecordBatch, RecordBatchReader};
use arrow_schema::SchemaRef;
use xeibe_arrow::ReadOptions;
use xeibe_core::{Source, Sources};

/// PRG: point and polygon layers in EPSG:2180, two features each.
pub const PRG: &str = "pl/prg-address-points.gml";
/// RCN: buildings whose outlines have arcs (CurvePolygon).
pub const RCN_ARCS: &str = "pl/rcn-price-register-arcs.gml";

/// One layer of a sample: its schema, batches and the read's settings as JSON.
pub fn read(
    sample: &str,
    layer: &str,
    options: &ReadOptions,
) -> (SchemaRef, Vec<RecordBatch>, String) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/data/samples")
        .join(sample);
    let sources = Sources::from(Source::file(path).expect("the sample"));
    read_sources(sources, layer, options)
}

/// One layer of a GML document in memory.
pub fn read_document(document: &str, layer: &str) -> (SchemaRef, Vec<RecordBatch>, String) {
    let bytes = document.as_bytes().to_vec();
    let sources = Sources::from(Source::reader(
        "test.gml",
        Box::new(std::io::Cursor::new(bytes)),
    ));
    read_sources(sources, layer, &ReadOptions::default())
}

fn read_sources(
    sources: Sources,
    layer: &str,
    options: &ReadOptions,
) -> (SchemaRef, Vec<RecordBatch>, String) {
    let mut reader = xeibe_arrow::read(sources, layer, None, options).expect("a reader");
    let batches: Vec<RecordBatch> = reader
        .by_ref()
        .map(|batch| batch.expect("a batch"))
        .collect();
    let settings = serde_json::to_string(reader.settings()).unwrap();
    (reader.schema(), batches, settings)
}

pub fn settings_metadata(settings: String) -> Vec<(String, String)> {
    vec![("gml:settings".to_string(), settings)]
}
