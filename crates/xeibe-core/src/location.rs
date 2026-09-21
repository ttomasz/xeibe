use serde::{Deserialize, Serialize};

use crate::SourceId;

/// A position in the input, used in errors, warnings and `--explain` evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub source: SourceId,
    /// Byte offset in the decoded (UTF-8) stream.
    pub byte_offset: u64,
    /// Sequence number of the feature within the source, if known.
    pub feature_seq: Option<u64>,
    /// `gml:id` of the feature, if known.
    pub gml_id: Option<String>,
}

impl std::fmt::Display for Location {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}, byte {}", self.source, self.byte_offset)?;
        if let Some(seq) = self.feature_seq {
            write!(f, ", feature {seq}")?;
        }
        if let Some(id) = &self.gml_id {
            write!(f, " (gml:id {id})")?;
        }
        Ok(())
    }
}
