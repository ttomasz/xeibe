//! Thin layer over `quick-xml` with namespace resolution and DTD rejection.

use crate::{Location, NamespaceContext, QName};

/// Events delivered to GML consumers (scan, geometry parser, feature builder).
#[derive(Debug)]
pub enum XmlEvent<'a> {
    Start {
        name: QName,
        attrs: Attributes<'a>,
    },
    End {
        name: QName,
    },
    /// Unescaped text, including CDATA content.
    Text(std::borrow::Cow<'a, str>),
    Eof,
}

/// Attributes of a start tag, resolved to [`QName`]s lazily.
#[derive(Debug)]
pub struct Attributes<'a> {
    raw: quick_xml::events::attributes::Attributes<'a>,
}

impl<'a> Attributes<'a> {
    pub fn get(&self, name: &QName) -> Option<std::borrow::Cow<'a, str>> {
        todo!()
    }

    pub fn iter(&self) -> impl Iterator<Item = (QName, std::borrow::Cow<'a, str>)> + '_ {
        std::iter::from_fn(|| todo!())
    }
}

/// Namespace-aware pull reader over a UTF-8 buffer (one chunk or one document).
pub struct GmlReader<'a> {
    inner: quick_xml::NsReader<&'a [u8]>,
    base_offset: u64,
}

impl<'a> GmlReader<'a> {
    /// `context` holds namespace declarations inherited from outside the buffer.
    pub fn new(buf: &'a [u8], context: &NamespaceContext, base_offset: u64) -> Self {
        todo!()
    }

    pub fn next_event(&mut self) -> crate::Result<XmlEvent<'_>> {
        todo!()
    }

    /// Skip the current element and its subtree without resolving names.
    pub fn skip_element(&mut self) -> crate::Result<()> {
        todo!()
    }

    /// Return the raw XML of the current element's subtree (for `RawXml` columns).
    pub fn capture_element(&mut self) -> crate::Result<String> {
        todo!()
    }

    pub fn location(&self) -> Location {
        todo!()
    }
}
