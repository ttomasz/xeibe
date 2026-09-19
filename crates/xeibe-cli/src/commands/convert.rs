use crate::args::{InputArgs, OutputArgs, ReadArgs};

pub fn run(input: InputArgs, output: OutputArgs, read: ReadArgs) -> super::Result {
    todo!()
}

/// Write one layer's batches to `output` (shared with `xeibe wfs convert`).
pub(crate) fn write(reader: xeibe_arrow::LayerReader, output: &OutputArgs) -> super::Result {
    todo!()
}

/// GeoParquet `geo` metadata for the geometry columns of a schema.
fn geoparquet_metadata(schema: &arrow_schema::Schema) -> super::Result<String> {
    todo!()
}
