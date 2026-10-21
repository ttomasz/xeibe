use std::fs::File;
use std::path::Path;

use arrow_array::RecordBatchReader;
use arrow_schema::SchemaRef;
use xeibe_arrow::{LayerReader, Settings};
use xeibe_export::{IpcWriter, ParquetOptions, ParquetWriter};
use xeibe_schema::rules::meta;

use crate::args::{BboxColumn, InputArgs, OutputArgs, OutputFormat, ReadArgs, SchemaArgs};

pub fn run(
    input: InputArgs,
    layer: String,
    preserve_order: bool,
    output: OutputArgs,
    schema: SchemaArgs,
    read: ReadArgs,
) -> super::Result {
    let (settings, schema) = settings(&schema, &read, preserve_order, &layer)?;
    let sources = super::sources(&input)?;
    let mut reader = xeibe_arrow::read(sources, &layer, schema, &settings.options)?;
    let rows = write(
        &mut reader,
        &output,
        &layer,
        settings.options.geometry.primary.as_deref(),
    )?;
    super::print_report(&reader.report());
    eprintln!("{rows} rows written to {}", output.output.display());
    Ok(())
}

/// The settings of a conversion (shared with `xeibe wfs convert`), and the
/// layer's schema from the settings file, or `None` to sample it.
/// `preserve_order`: keep the input order, whatever the settings file says.
pub(crate) fn settings(
    schema: &SchemaArgs,
    read: &ReadArgs,
    preserve_order: bool,
    layer: &str,
) -> super::Result<(Settings, Option<SchemaRef>)> {
    let mut settings = super::settings(schema.settings.as_deref(), read)?;
    if let Some(features) = schema.sample_features {
        settings.options.sample.features_per_layer = features;
    }
    if preserve_order {
        settings.options.preserve_order = true;
    }
    let schema = match settings.schema(layer) {
        Ok(schema) => Some(schema),
        Err(xeibe_arrow::Error::Schema(xeibe_schema::Error::UnknownLayer(_))) => None,
        Err(error) => return Err(error.into()),
    };
    Ok((settings, schema))
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
        OutputFormat::Parquet => write_parquet(reader, output, layer, primary),
        OutputFormat::Ipc => write_ipc(reader, path),
    };
    if result.is_err() {
        let _ = std::fs::remove_file(path);
    }
    result
}

fn write_parquet(
    reader: &mut LayerReader,
    output: &OutputArgs,
    layer: &str,
    primary: Option<&str>,
) -> super::Result<u64> {
    let path = &output.output;
    let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let options = ParquetOptions {
        bbox_column: match output.bbox_column {
            BboxColumn::Auto => xeibe_export::BboxColumn::Auto,
            BboxColumn::Always => xeibe_export::BboxColumn::Always,
            BboxColumn::Never => xeibe_export::BboxColumn::Never,
        },
        row_group_size: output.row_group_size,
        primary: primary.map(str::to_string),
    };
    let mut writer = ParquetWriter::new(file, reader.schema(), options, metadata(reader)?);
    let geometry = |error: xeibe_export::Error| -> Box<dyn std::error::Error> {
        match error {
            xeibe_export::Error::Curve { .. } => format!(
                "layer {layer}, {error}. Add --linearize to convert arcs to line segments (lossy), \
                 or use --format ipc to keep the curves"
            )
            .into(),
            xeibe_export::Error::Unsupported { .. } => {
                format!("layer {layer}, {error}; use --format ipc").into()
            }
            xeibe_export::Error::MalformedWkb { .. } => format!("layer {layer}, {error}").into(),
            error => error.into(),
        }
    };
    for batch in reader.by_ref() {
        writer.write(&batch?).map_err(geometry)?;
    }
    let summary = writer.finish().map_err(geometry)?;
    for warning in &summary.warnings {
        eprintln!("warning: {warning}");
    }
    Ok(summary.rows)
}

/// Arrow IPC file: batches as read, GeoArrow types (curves included) as they are.
fn write_ipc(reader: &mut LayerReader, path: &Path) -> super::Result<u64> {
    let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut writer = IpcWriter::new(file, &reader.schema(), metadata(reader)?)?;
    for batch in reader.by_ref() {
        writer.write(&batch?)?;
    }
    Ok(writer.finish()?.rows)
}

/// The settings the read used, as a settings file's JSON: stored in the
/// output's metadata as `gml:settings` (`docs/architecture.md`, "Settings
/// file"), so the file records how it was made and can be read again the same
/// way.
fn metadata(reader: &LayerReader) -> super::Result<Vec<(String, String)>> {
    let settings = serde_json::to_string(reader.settings())?;
    Ok(vec![(meta::SETTINGS.to_string(), settings)])
}
