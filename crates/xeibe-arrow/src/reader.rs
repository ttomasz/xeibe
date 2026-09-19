use arrow_array::{RecordBatch, RecordBatchReader};
use arrow_schema::{ArrowError, SchemaRef};

use crate::ReadReport;

/// Streams one layer's batches. Implements `RecordBatchReader`.
pub struct LayerReader {
    schema: SchemaRef,
    receiver: crossbeam_channel::Receiver<crate::Result<RecordBatch>>,
    report: std::sync::Arc<std::sync::Mutex<ReadReport>>,
}

impl LayerReader {
    /// Report so far (complete once the reader is exhausted).
    pub fn report(&self) -> ReadReport {
        todo!()
    }
}

impl Iterator for LayerReader {
    type Item = Result<RecordBatch, ArrowError>;

    fn next(&mut self) -> Option<Self::Item> {
        todo!()
    }
}

impl RecordBatchReader for LayerReader {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }
}
