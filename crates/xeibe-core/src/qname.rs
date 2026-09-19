use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Namespace URI + local name. Never the prefix: prefixes differ between
/// files and WFS pages.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct QName {
    pub ns: Option<Arc<str>>,
    pub local: Arc<str>,
}

impl QName {
    pub fn new(ns: Option<&str>, local: &str) -> Self {
        todo!()
    }

    /// `true` if this element is in one of the GML namespaces (2/3.1 or 3.2).
    pub fn is_gml(&self) -> bool {
        todo!()
    }

    /// Clark notation: `{uri}local`.
    pub fn to_clark(&self) -> String {
        todo!()
    }
}

impl std::fmt::Display for QName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        todo!()
    }
}
