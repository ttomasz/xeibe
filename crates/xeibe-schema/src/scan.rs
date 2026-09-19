//! Scan: stream chunks, build one path tree per chunk, merge.

use xeibe_core::{FeatureChunk, Sources};

use crate::DatasetObservation;
use crate::options::Limits;

/// How much of the input a scan reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScanExtent {
    /// Everything: the complete layer list and full-data evidence.
    #[default]
    Full,
    /// The first N features of the input, whatever their layers. Layers that
    /// start later are missing.
    Sample { max_features: u64 },
}

#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub extent: ScanExtent,
    pub limits: Limits,
    /// Only these layers (the sample of a read without a schema); `None` = all.
    pub layers: Option<Vec<xeibe_core::QName>>,
    pub splitter: xeibe_core::SplitterOptions,
    pub threads: usize,
}

impl Default for ScanOptions {
    fn default() -> Self {
        todo!()
    }
}

/// Builds the observation for a set of sources.
pub struct Scanner {
    options: ScanOptions,
}

impl Scanner {
    pub fn new(options: ScanOptions) -> Self {
        todo!()
    }

    pub fn run(&self, sources: Sources) -> crate::Result<DatasetObservation> {
        todo!()
    }

    /// Scan one chunk into a partial observation (called from worker threads, and
    /// by a read that samples its layer from buffered chunks).
    pub fn scan_chunk(&self, chunk: &FeatureChunk) -> crate::Result<DatasetObservation> {
        todo!()
    }
}

/// Per-chunk tree builder: walks features, keeps per-parent child counters on
/// a stack, and folds them into `max_occurs` when the parent closes.
pub(crate) struct TreeBuilder {
    observation: DatasetObservation,
    stack: Vec<Frame>,
}

struct Frame {
    child_counts: Vec<(xeibe_core::QName, u32)>,
    had_text: bool,
    had_children: bool,
}

impl TreeBuilder {
    pub fn new() -> Self {
        todo!()
    }

    pub fn finish(self) -> DatasetObservation {
        todo!()
    }
}
