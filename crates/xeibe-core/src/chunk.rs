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
    /// The `boundedBy` of the innermost collection around these features
    /// that has one, whose srsName and srsDimension the features inherit
    /// (`docs/geometry.md`, "srsName inheritance"). All features of a chunk
    /// share it.
    pub collection_bounded_by: Option<Arc<RawElement>>,
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
