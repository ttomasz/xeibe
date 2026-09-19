//! Namespace handling. Elements are matched by namespace URI, never by prefix.

use std::sync::Arc;

use crate::QName;

/// Well-known namespace URIs.
pub mod ns {
    /// GML 2.x and 3.0/3.1 share this namespace.
    pub const GML: &str = "http://www.opengis.net/gml";
    pub const GML_32: &str = "http://www.opengis.net/gml/3.2";
    pub const XLINK: &str = "http://www.w3.org/1999/xlink";
    pub const XSI: &str = "http://www.w3.org/2001/XMLSchema-instance";
    pub const WFS: &str = "http://www.opengis.net/wfs";
    pub const WFS_20: &str = "http://www.opengis.net/wfs/2.0";
    pub const OWS: &str = "http://www.opengis.net/ows";
    pub const OWS_11: &str = "http://www.opengis.net/ows/1.1";
    pub const OGC: &str = "http://www.opengis.net/ogc";
    pub const FES_20: &str = "http://www.opengis.net/fes/2.0";
    /// Presence on the root element marks FME-produced GML (axis-order quirk).
    pub const FME: &str = "http://www.safe.com/gml/fme";
}

/// Namespace declarations in scope at some point of the document.
///
/// The splitter records the declarations of the root element and of the
/// ancestors of the feature container once; every chunk carries an `Arc` to
/// it so it can be parsed on its own.
#[derive(Debug, Clone, Default)]
pub struct NamespaceContext {
    /// `(prefix, uri)`; `None` prefix is the default namespace.
    bindings: Vec<(Option<Arc<str>>, Arc<str>)>,
}

impl NamespaceContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn declare(&mut self, prefix: Option<&str>, uri: &str) {
        todo!()
    }

    pub fn resolve_prefix(&self, prefix: Option<&str>) -> Option<&str> {
        todo!()
    }

    /// Resolve a prefixed name such as `gml:Point` into a [`QName`].
    pub fn resolve(&self, prefixed: &str) -> Option<QName> {
        todo!()
    }

    /// Serialize as `xmlns` attributes, to prepend to a standalone chunk.
    pub fn to_xmlns_attributes(&self) -> String {
        todo!()
    }
}
