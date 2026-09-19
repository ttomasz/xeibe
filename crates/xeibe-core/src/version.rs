use serde::{Deserialize, Serialize};

/// GML version of a document (see `docs/schema-inference.md` §2.8).
///
/// GML 2 and 3.1 share a namespace, so the version is derived from
/// `xsi:schemaLocation`, the WFS version/output format, and the elements seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum GmlVersion {
    V2,
    V3_0,
    V3_1,
    V3_2,
}

/// Encoding style of one geometry element, decided per element from the tags
/// used (not from the document header). Drives `AxisOrderMode::GmlVersion`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Dialect {
    /// `coordinates`/`coord` carriers, `outerBoundaryIs`, `Box`, GML 2 properties.
    Gml2,
    /// `pos`, `posList`, `exterior`, `Curve`, `Surface`, `Envelope`, 3.2 namespace, …
    Gml3,
}

/// Evidence collected while reading, turned into a version guess.
#[derive(Debug, Clone, Default)]
pub struct VersionHints {
    pub gml_namespace: Option<String>,
    pub schema_location: Option<String>,
    pub wfs_version: Option<String>,
    pub output_format: Option<String>,
    pub saw_gml2_elements: bool,
    pub saw_gml3_elements: bool,
    /// `posList@dimension` is GML 3.0 only.
    pub saw_poslist_dimension: bool,
}

impl VersionHints {
    pub fn detect(&self) -> Option<GmlVersion> {
        todo!()
    }
}
