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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NamespaceContext {
    /// `(prefix, uri)`; `None` prefix is the default namespace. Each prefix
    /// appears once.
    bindings: Vec<(Option<Arc<str>>, Arc<str>)>,
}

impl NamespaceContext {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind `prefix` (`None`: the default namespace) to `uri`, replacing an
    /// earlier binding of the same prefix.
    pub fn declare(&mut self, prefix: Option<&str>, uri: &str) {
        match self
            .bindings
            .iter_mut()
            .find(|(p, _)| p.as_deref() == prefix)
        {
            Some(binding) => binding.1 = Arc::from(uri),
            None => self.bindings.push((prefix.map(Arc::from), Arc::from(uri))),
        }
    }

    pub fn resolve_prefix(&self, prefix: Option<&str>) -> Option<&str> {
        self.bindings
            .iter()
            .find(|(p, _)| p.as_deref() == prefix)
            .map(|(_, uri)| &**uri)
            // `xmlns=""` undeclares the default namespace.
            .filter(|uri| !uri.is_empty() || prefix.is_some())
    }

    /// Resolve a prefixed name such as `gml:Point` into a [`QName`].
    ///
    /// An unprefixed name takes the default namespace, or none if there is no
    /// default. An undeclared prefix gives `None`.
    pub fn resolve(&self, prefixed: &str) -> Option<QName> {
        match prefixed.split_once(':') {
            Some((prefix, local)) => {
                let uri = self.resolve_prefix(Some(prefix))?;
                Some(QName::new(Some(uri), local))
            }
            None => Some(QName::new(self.resolve_prefix(None), prefixed)),
        }
    }

    /// The bindings as `(prefix, uri)`, in declaration order.
    pub fn iter(&self) -> impl Iterator<Item = (Option<&str>, &str)> + '_ {
        self.bindings.iter().map(|(p, uri)| (p.as_deref(), &**uri))
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    /// `true` if some prefix (or the default namespace) is bound to `uri`.
    pub fn declares_uri(&self, uri: &str) -> bool {
        self.bindings.iter().any(|(_, u)| &**u == uri)
    }

    /// Serialize as `xmlns` attributes, to prepend to a standalone chunk.
    ///
    /// Each attribute is preceded by a space (`' xmlns:gml="…"'`), so the
    /// result can be inserted right after an element name. Empty if nothing is
    /// declared.
    pub fn to_xmlns_attributes(&self) -> String {
        let mut out = String::new();
        for (prefix, uri) in &self.bindings {
            match prefix {
                Some(prefix) => {
                    out.push_str(" xmlns:");
                    out.push_str(prefix);
                }
                None => out.push_str(" xmlns"),
            }
            out.push_str("=\"");
            for c in uri.chars() {
                match c {
                    '&' => out.push_str("&amp;"),
                    '<' => out.push_str("&lt;"),
                    '"' => out.push_str("&quot;"),
                    c => out.push(c),
                }
            }
            out.push('"');
        }
        out
    }
}
