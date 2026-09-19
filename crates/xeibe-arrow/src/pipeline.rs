//! Parallel read pipeline:
//! splitter → bounded chunk queue → workers (parse + build) → bounded batch
//! queue → optional reorder by sequence number. One layer per pipeline.

use std::sync::Arc;

use arrow_array::RecordBatch;
use xeibe_core::FeatureChunk;
use xeibe_schema::LayerSchema;

use crate::ReadOptions;

pub struct Pipeline {
    options: ReadOptions,
    layer: Arc<LayerSchema>,
}

/// A batch tagged for reordering.
pub struct SeqBatch {
    pub chunk_seq: u64,
    pub batch: RecordBatch,
}

impl Pipeline {
    pub fn new(options: ReadOptions, layer: Arc<LayerSchema>) -> Self {
        todo!()
    }

    /// Start the workers. `chunks` already contains only this layer's features
    /// (splitter layer filter), starting with any buffered sample chunks.
    pub fn start(
        self,
        chunks: Box<dyn Iterator<Item = xeibe_core::Result<FeatureChunk>> + Send>,
    ) -> crossbeam_channel::Receiver<crate::Result<RecordBatch>> {
        todo!()
    }

    fn process_chunk(&self, chunk: &FeatureChunk) -> crate::Result<Vec<SeqBatch>> {
        todo!()
    }
}
