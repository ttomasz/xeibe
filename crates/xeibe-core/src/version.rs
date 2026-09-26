use serde::{Deserialize, Serialize};

/// GML version of a document (see `docs/schema-inference.md` §2.8).
///
/// GML 2 and 3.1 share a namespace, so the version is derived from
/// `xsi:schemaLocation`, the WFS version/output format, and the elements seen.
/// GML 3.3 keeps the 3.2 namespace and adds its own ([`crate::ns::GML_33`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum GmlVersion {
    V2,
    V3_0,
    V3_1,
    V3_2,
    V3_3,
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
    /// A GML 3.3 namespace is declared or used.
    pub gml_33: bool,
    pub saw_gml2_elements: bool,
    pub saw_gml3_elements: bool,
    /// `posList@dimension` is GML 3.0 only.
    pub saw_poslist_dimension: bool,
}

impl VersionHints {
    /// The evidence in order of strength: a GML 3.3 namespace, the 3.2
    /// namespace (3.3 if the `xsi:schemaLocation` or the output format says
    /// so), the `xsi:schemaLocation` of the GML schema, the WFS output format
    /// and version, and finally the elements seen.
    pub fn detect(&self) -> Option<GmlVersion> {
        if self.gml_33 {
            return Some(GmlVersion::V3_3);
        }
        let stated = self
            .schema_location
            .as_deref()
            .and_then(from_schema_location)
            .or_else(|| self.output_format.as_deref().and_then(from_output_format));
        if self.gml_namespace.as_deref() == Some(crate::ns::GML_32) {
            return Some(match stated {
                Some(GmlVersion::V3_3) => GmlVersion::V3_3,
                _ => GmlVersion::V3_2,
            });
        }
        if let Some(version) = stated {
            return Some(version);
        }
        if let Some(version) = self.wfs_version.as_deref().and_then(from_wfs_version) {
            return Some(version);
        }
        if self.saw_gml3_elements {
            return Some(if self.saw_poslist_dimension {
                GmlVersion::V3_0
            } else {
                GmlVersion::V3_1
            });
        }
        if self.saw_gml2_elements {
            return Some(GmlVersion::V2);
        }
        None
    }
}

/// The highest version of the `…/gml/<version>/…` URLs in
/// `xsi:schemaLocation`: a GML 3.3 document may list the 3.2 schema too.
fn from_schema_location(location: &str) -> Option<GmlVersion> {
    location
        .split_ascii_whitespace()
        .filter_map(|url| url.split_once("/gml/").and_then(|(_, rest)| version_prefix(rest)))
        .max()
}

/// `text/xml; subtype=gml/3.1.1`, `application/gml+xml; version=3.2`, `GML2`.
fn from_output_format(format: &str) -> Option<GmlVersion> {
    let format = format.to_ascii_lowercase();
    for marker in ["gml/", "version=", "gml"] {
        if let Some(version) = format
            .match_indices(marker)
            .find_map(|(i, _)| version_prefix(&format[i + marker.len()..]))
        {
            return Some(version);
        }
    }
    None
}

/// WFS 1.0 defaults to GML 2, 1.1 to GML 3.1, 2.0 to GML 3.2.
fn from_wfs_version(version: &str) -> Option<GmlVersion> {
    let version = version.trim();
    if version.starts_with("1.0") {
        Some(GmlVersion::V2)
    } else if version.starts_with("1.1") {
        Some(GmlVersion::V3_1)
    } else if version.starts_with("2.") {
        Some(GmlVersion::V3_2)
    } else {
        None
    }
}

/// The GML version a string such as `3.1.1/base/gml.xsd` or `2` starts with.
fn version_prefix(text: &str) -> Option<GmlVersion> {
    let text = text.trim_start_matches(['"', '\'']);
    if text.starts_with("3.3") {
        Some(GmlVersion::V3_3)
    } else if text.starts_with("3.2") {
        Some(GmlVersion::V3_2)
    } else if text.starts_with("3.1") {
        Some(GmlVersion::V3_1)
    } else if text.starts_with("3.0") {
        Some(GmlVersion::V3_0)
    } else if text.starts_with('3') {
        Some(GmlVersion::V3_1)
    } else if text.starts_with('2') {
        Some(GmlVersion::V2)
    } else {
        None
    }
}
