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
        Self {
            ns: ns.map(Arc::from),
            local: Arc::from(local),
        }
    }

    /// `true` if this element is in one of the GML namespaces (2/3.1 or 3.2).
    pub fn is_gml(&self) -> bool {
        matches!(
            self.ns.as_deref(),
            Some(crate::ns::GML) | Some(crate::ns::GML_32)
        )
    }

    /// `true` if this is `local` in one of the GML namespaces.
    pub fn is_gml_named(&self, local: &str) -> bool {
        self.is_gml() && &*self.local == local
    }

    /// Clark notation: `{uri}local`.
    pub fn to_clark(&self) -> String {
        self.to_string()
    }
}

/// Clark notation, like [`QName::to_clark`].
impl std::fmt::Display for QName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.ns {
            Some(ns) => write!(f, "{{{ns}}}{}", self.local),
            None => f.write_str(&self.local),
        }
    }
}
