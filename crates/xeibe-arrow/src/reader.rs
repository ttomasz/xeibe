use std::sync::{Arc, Mutex};

use arrow_array::{RecordBatch, RecordBatchReader};
use arrow_schema::{ArrowError, SchemaRef};

use crate::ReadReport;
use crate::pipeline::{ReadPlan, lock};
use crate::report::Warning;

/// Streams one layer's batches. Implements `RecordBatchReader`.
///
/// Dropping the reader stops the read: the pipeline's threads end once they
/// notice that nobody takes their output.
pub struct LayerReader {
    schema: SchemaRef,
    receiver: crossbeam_channel::Receiver<crate::Result<RecordBatch>>,
    report: Arc<Mutex<ReadReport>>,
    plan: Arc<ReadPlan>,
    /// Warnings of the source stream (skipped sources).
    source_warnings: Arc<Mutex<Vec<Warning>>>,
    /// An error was returned; the reader is exhausted.
    failed: bool,
}

impl LayerReader {
    pub(crate) fn new(
        receiver: crossbeam_channel::Receiver<crate::Result<RecordBatch>>,
        report: Arc<Mutex<ReadReport>>,
        plan: Arc<ReadPlan>,
        source_warnings: Arc<Mutex<Vec<Warning>>>,
    ) -> Self {
        LayerReader { schema: plan.schema.clone(), receiver, report, plan, source_warnings, failed: false }
    }

    /// Report so far (complete once the reader is exhausted).
    pub fn report(&self) -> ReadReport {
        let mut report = lock(&self.report).clone();
        let (decisions, axis_warnings) = self.plan.axis_report();
        report.axis_decisions = decisions;
        let mut warnings: Vec<Warning> = lock(&self.source_warnings).clone();
        warnings.extend(axis_warnings);
        warnings.append(&mut report.warnings);
        for warning in warnings {
            report.warn(warning);
        }
        report
    }

    /// Like `next`, with the crate's error type.
    pub fn next_batch(&mut self) -> Option<crate::Result<RecordBatch>> {
        if self.failed {
            return None;
        }
        let next = self.receiver.recv().ok()?;
        self.failed = next.is_err();
        Some(next)
    }
}

impl Iterator for LayerReader {
    type Item = Result<RecordBatch, ArrowError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_batch().map(|batch| {
            batch.map_err(|error| match error {
                crate::Error::Arrow(error) => error,
                error => ArrowError::ExternalError(Box::new(error)),
            })
        })
    }
}

impl RecordBatchReader for LayerReader {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }
}
