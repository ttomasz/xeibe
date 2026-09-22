//! A scan partition = one source of one layer, read as a stream.

use std::sync::Arc;

use datafusion::arrow::array::{RecordBatch, RecordBatchOptions};
use datafusion::arrow::datatypes::{Schema, SchemaRef};
use datafusion::error::{DataFusionError, Result};
use datafusion::execution::{SendableRecordBatchStream, TaskContext};
use datafusion::physical_plan::stream::RecordBatchReceiverStream;
use datafusion::physical_plan::streaming::PartitionStream;
use xeibe_arrow::ReadOptions;
use xeibe_core::Source;

use crate::sources::external;

/// Batches buffered between the reader and DataFusion (the reader has queues
/// of its own).
const CHANNEL_CAPACITY: usize = 2;

#[derive(Debug)]
pub struct SourcePartition {
    source: Source,
    layer: String,
    /// The table's schema, given to the reader.
    table_schema: SchemaRef,
    /// The output: the table's columns named in `options.projection`, in that order.
    schema: SchemaRef,
    options: ReadOptions,
}

impl SourcePartition {
    /// `schema` is the table's schema; `options.projection` (all columns when
    /// `None`) picks and orders the partition's output columns.
    pub fn new(source: Source, layer: &str, schema: SchemaRef, options: ReadOptions) -> Self {
        let output = match &options.projection {
            Some(names) => {
                let fields = names
                    .iter()
                    .filter_map(|name| schema.field_with_name(name).ok().cloned())
                    .collect::<Vec<_>>();
                Arc::new(Schema::new_with_metadata(fields, schema.metadata().clone()))
            }
            None => schema.clone(),
        };
        SourcePartition { source, layer: layer.to_string(), table_schema: schema, schema: output, options }
    }
}

impl PartitionStream for SourcePartition {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    /// Runs `xeibe_arrow::read` in `spawn_blocking` and forwards its batches.
    /// Dropping the stream drops the reader, which stops the read.
    fn execute(&self, _ctx: Arc<TaskContext>) -> SendableRecordBatchStream {
        let mut builder = RecordBatchReceiverStream::builder(self.schema.clone(), CHANNEL_CAPACITY);
        let sender = builder.tx();
        let source = self.source.clone();
        let layer = self.layer.clone();
        let table_schema = self.table_schema.clone();
        let schema = self.schema.clone();
        let mut options = self.options.clone();
        options.projection = Some(schema.fields().iter().map(|field| field.name().clone()).collect());
        builder.spawn_blocking(move || {
            let reader = xeibe_arrow::read(source, &layer, Some(table_schema), &options).map_err(external)?;
            for batch in reader {
                let batch = batch.map_err(DataFusionError::from).and_then(|batch| conform(batch, &schema));
                let failed = batch.is_err();
                if sender.blocking_send(batch).is_err() || failed {
                    // The consumer is gone, or the error has been handed on.
                    break;
                }
            }
            Ok(())
        });
        builder.build()
    }
}

/// The reader's columns (in table order) as the partition's schema: by name,
/// in projection order, with the table's field metadata.
fn conform(batch: RecordBatch, schema: &SchemaRef) -> Result<RecordBatch> {
    let columns = schema
        .fields()
        .iter()
        .map(|field| {
            batch.column_by_name(field.name()).cloned().ok_or_else(|| {
                DataFusionError::Execution(format!("the GML reader returned no column {:?}", field.name()))
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let options = RecordBatchOptions::new().with_row_count(Some(batch.num_rows()));
    Ok(RecordBatch::try_new_with_options(schema.clone(), columns, &options)?)
}
