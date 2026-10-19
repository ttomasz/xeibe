use std::sync::Arc;

use bytes::Bytes;

use crate::{NamespaceContext, SourceId};

/// A self-contained run of whole features, produced by the splitter.
#[derive(Debug, Clone)]
pub struct FeatureChunk {
    pub source: SourceId,
    /// Sequence number within the read; used to restore order.
    pub seq: u64,
    /// Offset in the decoded stream, for error locations.
    pub byte_offset: u64,
    pub bytes: Bytes,
    /// Namespace declarations in scope at the feature members.
    pub namespaces: Arc<NamespaceContext>,
    /// Sequence number of the first feature in this chunk.
    pub first_feature_seq: u64,
    /// Number of features in this chunk.
    pub features: u64,
    /// Where each feature starts, in `bytes` and in the decoded stream: a
    /// chunk is its features one after another, without what was between
    /// them in the stream. Empty: `bytes` is the stream from `byte_offset`.
    pub feature_offsets: Arc<[FeatureOffset]>,
    /// The `boundedBy` of the innermost collection around these features
    /// that has one, whose srsName and srsDimension the features inherit
    /// (`docs/geometry.md`, "srsName inheritance"). All features of a chunk
    /// share it.
    pub collection_bounded_by: Option<Arc<RawElement>>,
}

impl FeatureChunk {
    /// The decoded stream's offset of a position in `bytes`.
    pub fn stream_offset(&self, position: usize) -> u64 {
        stream_offset(&self.feature_offsets, self.byte_offset, position)
    }
}

/// Where a feature of a [`FeatureChunk`] starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeatureOffset {
    /// In the chunk's bytes.
    pub in_chunk: usize,
    /// In the decoded stream.
    pub in_stream: u64,
}

/// A position in a chunk's bytes as an offset in the decoded stream: from
/// the feature it falls in, or from `base` without `offsets`.
pub(crate) fn stream_offset(offsets: &[FeatureOffset], base: u64, position: usize) -> u64 {
    match offsets.partition_point(|offset| offset.in_chunk <= position) {
        0 => base + position as u64,
        i => {
            let feature = offsets[i - 1];
            feature.in_stream + (position - feature.in_chunk) as u64
        }
    }
}

/// One element as written: its bytes, from its start tag to its end tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawElement {
    pub source: SourceId,
    /// Offset in the decoded stream, for error locations.
    pub byte_offset: u64,
    pub bytes: Bytes,
    /// Namespace declarations in scope at the element (its own are in its bytes).
    pub namespaces: Arc<NamespaceContext>,
}
