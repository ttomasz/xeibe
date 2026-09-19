use serde::{Deserialize, Serialize};

/// Glob over local names, optionally namespace-qualified:
/// `AD_PunktAdresowy/idIIP`, `*/area`, `**/@uom`, `{uri}*/**`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathPattern {
    pub raw: String,
}

impl PathPattern {
    pub fn parse(raw: &str) -> crate::Result<Self> {
        todo!()
    }

    /// `path` is the element path from the feature root, e.g. `["idIIP", "lokalnyId"]`.
    pub fn matches(&self, layer: &str, path: &[&str]) -> bool {
        todo!()
    }

    /// Higher = more specific (used to pick the winning override).
    pub fn specificity(&self) -> u32 {
        todo!()
    }
}
