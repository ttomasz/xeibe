use std::fs::File;
use std::path::Path;

use arrow_array::RecordBatchReader;
use arrow_schema::SchemaRef;
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::metadata::KeyValue;
use parquet::file::properties::WriterProperties;
use xeibe_arrow::{LayerReader, Settings};

use super::geoparquet::GeoColumns;
use crate::args::{BboxColumn, InputArgs, OutputArgs, OutputFormat, ReadArgs};

pub fn run(input: InputArgs, output: OutputArgs, read: ReadArgs) -> super::Result {
    let layer = output.layer.clone().ok_or("--layer is required: run `xeibe scan` to list the layers")?;
    let settings = super::settings(&read)?;
    let schema = layer_schema(&settings, &layer)?;
    let sources = super::sources(&input)?;
    let mut reader = xeibe_arrow::read(sources, &layer, schema, &settings.options)?;
    let rows = write(&mut reader, &output, &layer, settings.options.geometry.primary.as_deref())?;
    super::print_report(&reader.report());
    eprintln!("{rows} rows written to {}", output.output.display());
    Ok(())
}

/// The layer's schema from the settings file, or `None` to sample it.
pub(crate) fn layer_schema(settings: &Settings, layer: &str) -> super::Result<Option<SchemaRef>> {
    match settings.schema(layer) {
        Ok(schema) => Ok(Some(schema)),
        Err(xeibe_arrow::Error::Schema(xeibe_schema::Error::UnknownLayer(_))) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Write one layer's batches to `output` (shared with `xeibe wfs convert`).
/// A failed write removes the partial file. Returns the rows written.
pub(crate) fn write(
    reader: &mut LayerReader,
    output: &OutputArgs,
    layer: &str,
    primary: Option<&str>,
) -> super::Result<u64> {
    let path = &output.output;
    let result = match output.format {
        OutputFormat::Parquet => write_parquet(reader, path, layer, primary, output.bbox_column),
        OutputFormat::Ipc => write_ipc(reader, path),
    };
    if result.is_err() {
        let _ = std::fs::remove_file(path);
    }
    result
}

fn write_parquet(
    reader: &mut LayerReader,
    path: &Path,
    layer: &str,
    primary: Option<&str>,
    bbox: BboxColumn,
) -> super::Result<u64> {
    // The first batch tells `--bbox-column auto` which WKB columns hold points.
    let first = reader.next().transpose()?;
    let mut geo = GeoColumns::new(layer, &reader.schema(), primary, bbox, first.as_ref())?;
    let properties = WriterProperties::builder()
        .set_compression(Compression::ZSTD(ZstdLevel::default()))
        .build();
    let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut writer = ArrowWriter::try_new(file, geo.schema(), Some(properties))?;
    let mut rows = 0;
    for batch in first.map(Ok).into_iter().chain(reader.by_ref()) {
        let batch = geo.convert(&batch?)?;
        rows += batch.num_rows() as u64;
        writer.write(&batch)?;
    }
    if let Some(metadata) = geo.metadata() {
        writer.append_key_value_metadata(KeyValue::new("geo".to_string(), metadata));
    }
    writer.close()?;
    Ok(rows)
}

/// Arrow IPC file: batches as read, GeoArrow types (curves included) as they are.
fn write_ipc(reader: &mut LayerReader, path: &Path) -> super::Result<u64> {
    let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut writer = arrow_ipc::writer::FileWriter::try_new(file, &reader.schema())?;
    let mut rows = 0;
    for batch in reader.by_ref() {
        let batch = batch?;
        rows += batch.num_rows() as u64;
        writer.write(&batch)?;
    }
    writer.finish()?;
    Ok(rows)
}
