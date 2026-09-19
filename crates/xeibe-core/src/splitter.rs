//! Feature-boundary splitter: finds feature members without full parsing and
//! cuts the stream into [`FeatureChunk`]s (see `docs/architecture.md`).

use std::io::Read;

use serde::{Deserialize, Serialize};

use crate::{FeatureChunk, NamespaceContext, QName, SourceId};

/// Which elements wrap features.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemberRule {
    /// Each child element is one feature: `gml:featureMember`, `wfs:member`.
    OnePerElement(QName),
    /// Each child element of this container is a feature: `gml:featureMembers`.
    ManyPerElement(QName),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SplitterOptions {
    /// Target chunk size; chunks are cut at the next feature boundary.
    pub target_chunk_bytes: usize,
    /// Built-in rules (`featureMember`, `featureMembers`, `wfs:member`) plus custom ones.
    pub member_rules: Vec<MemberRule>,
    /// Treat a document whose root is a feature as a single-feature dataset.
    pub allow_single_feature_root: bool,
    /// Only these feature types enter chunks; others are skipped unparsed.
    /// Set by every read (one layer); `None` for scans.
    #[serde(skip)]
    pub layers: Option<Vec<QName>>,
}

impl Default for SplitterOptions {
    fn default() -> Self {
        todo!()
    }
}

/// Information about the document root, collected before the first chunk.
#[derive(Debug, Clone)]
pub struct DocumentHeader {
    pub root: QName,
    pub namespaces: NamespaceContext,
    pub schema_location: Option<String>,
    /// WFS 2.0 `numberMatched` / `numberReturned` / `next`, WFS 1.1 `numberOfFeatures`.
    pub wfs_attributes: Vec<(QName, String)>,
    /// Root declares `xmlns:fme="http://www.safe.com/gml/fme"`.
    pub fme_produced: bool,
}

/// Iterator of chunks over one decoded source.
pub struct FeatureSplitter<R: Read> {
    reader: R,
    source: SourceId,
    options: SplitterOptions,
    header: Option<DocumentHeader>,
    next_seq: u64,
}

impl<R: Read> FeatureSplitter<R> {
    pub fn new(reader: R, source: SourceId, options: SplitterOptions) -> Self {
        todo!()
    }

    /// Read up to the first feature member and return the document header.
    pub fn header(&mut self) -> crate::Result<&DocumentHeader> {
        todo!()
    }
}

impl<R: Read> Iterator for FeatureSplitter<R> {
    type Item = crate::Result<FeatureChunk>;

    fn next(&mut self) -> Option<Self::Item> {
        todo!()
    }
}
