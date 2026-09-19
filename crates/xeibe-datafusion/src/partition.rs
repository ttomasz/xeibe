//! A scan partition = one source of one layer, read as a stream.

use std::sync::Arc;

use datafusion::arrow::datatypes::SchemaRef;
use datafusion::execution::{SendableRecordBatchStream, TaskContext};
use datafusion::physical_plan::streaming::PartitionStream;
use xeibe_arrow::ReadOptions;
use xeibe_core::Source;

#[derive(Debug)]
pub struct SourcePartition {
    source: Source,
    layer: String,
    schema: SchemaRef,
    options: ReadOptions,
}

impl SourcePartition {
    pub fn new(source: Source, layer: &str, schema: SchemaRef, options: ReadOptions) -> Self {
        todo!()
    }
}

impl PartitionStream for SourcePartition {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    /// Runs `xeibe_arrow::read` in `spawn_blocking` and forwards its batches.
    fn execute(&self, ctx: Arc<TaskContext>) -> SendableRecordBatchStream {
        todo!()
    }
}
