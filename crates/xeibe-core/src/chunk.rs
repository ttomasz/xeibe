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
}
