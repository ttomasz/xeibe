//! Small XML helpers for capabilities, exception reports and
//! `DescribeFeatureType` schemas: UTF-8 decoding, the root element's name, and
//! a namespace-resolved element tree.
//!
//! These documents are small, and reading them needs prefix resolution inside
//! text content (`<Name>ms:AD.Address</Name>`) and attribute values
//! (`type="xsd:int"`), so they are read into a tree with `quick-xml` directly
//! instead of through `xeibe_core::reader`.

use std::borrow::Cow;
use std::io::Read;

use quick_xml::events::Event;
use quick_xml::name::{Namespace, ResolveResult};
use xeibe_core::{Location, SourceId};

/// The document as UTF-8: decompressed and transcoded when needed.
pub(crate) fn utf8(bytes: &[u8]) -> crate::Result<Cow<'_, [u8]>> {
    let plain = xeibe_core::decode::detect_compression(bytes)
        == xeibe_core::decode::Compression::None
        && xeibe_core::decode::detect_encoding(bytes).name() == "UTF-8"
        && !bytes.starts_with(b"\xEF\xBB\xBF");
    if plain {
        return Ok(Cow::Borrowed(bytes));
    }
    let raw = Box::new(std::io::Cursor::new(bytes.to_vec()));
    let mut decoded = Vec::with_capacity(bytes.len());
    xeibe_core::decode::decoded_reader(raw)?.read_to_end(&mut decoded)?;
    Ok(Cow::Owned(decoded))
}

/// An attribute: its local name and value (lookups ignore its namespace).
#[derive(Debug, Default)]
pub(crate) struct Attr {
    pub local: String,
    pub value: String,
    /// The value read as a QName (`xsd:int`, or `int` in the default
    /// namespace): the namespace of its prefix, when it has the form of one
    /// and the prefix is declared.
    pub value_ns: Option<String>,
}

/// An element with its resolved name, attributes, text and children.
#[derive(Debug, Default)]
pub(crate) struct Element {
    pub ns: Option<String>,
    pub local: String,
    /// Namespace declarations left out.
    pub attrs: Vec<Attr>,
    /// Concatenated, trimmed text directly inside the element.
    pub text: String,
    /// Namespace of the prefix in `text`, when `text` looks like a prefixed name.
    pub text_ns: Option<String>,
    pub children: Vec<Element>,
}

impl Element {
    /// `true` for `local` in any namespace.
    pub fn is(&self, local: &str) -> bool {
        self.local == local
    }

    /// Children named `local`, in any namespace.
    pub fn children_named<'a>(&'a self, local: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |c| c.is(local))
    }

    pub fn child(&self, local: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.is(local))
    }

    /// Follow a path of local names; the first match at every step.
    pub fn path(&self, path: &[&str]) -> Option<&Element> {
        path.iter()
            .try_fold(self, |element, local| element.child(local))
    }

    /// Text of a child, `None` if missing or empty.
    pub fn child_text(&self, local: &str) -> Option<String> {
        self.child(local)
            .map(|c| c.text.clone())
            .filter(|t| !t.is_empty())
    }

    /// Attribute by local name, in any namespace.
    pub fn attr(&self, local: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|attr| attr.local == local)
            .map(|attr| attr.value.as_str())
    }

    /// An attribute whose value is a QName (`type`, `base`, `ref` of an XSD):
    /// `(namespace, local name)`.
    pub fn attr_qname(&self, local: &str) -> Option<(Option<&str>, &str)> {
        let attr = self.attrs.iter().find(|attr| attr.local == local)?;
        let name = attr
            .value
            .split_once(':')
            .map_or(attr.value.as_str(), |(_, local)| local);
        Some((attr.value_ns.as_deref(), name))
    }

    /// Every descendant named `local`, depth first.
    pub fn descendants<'a>(&'a self, local: &'a str, out: &mut Vec<&'a Element>) {
        for child in &self.children {
            if child.is(local) {
                out.push(child);
            }
            child.descendants(local, out);
        }
    }
}

/// The root element's `(namespace, local name)`, reading no further than its
/// start tag. `None` if there is no readable root (not XML, empty).
pub(crate) fn root_name(xml: &[u8]) -> Option<(Option<String>, String)> {
    let mut reader = quick_xml::NsReader::from_reader(xml);
    loop {
        match reader.read_resolved_event() {
            Ok((resolved, Event::Start(start) | Event::Empty(start))) => {
                let local = start.local_name().as_ref().to_string();
                return Some((namespace(resolved), local));
            }
            Ok((_, Event::Eof)) | Err(_) => return None,
            // Only markup may come before the root.
            Ok((_, Event::Text(text))) if !text.trim_ascii().is_empty() => return None,
            Ok(_) => {}
        }
    }
}

/// Read the whole document into a tree.
pub(crate) fn parse(xml: &[u8]) -> crate::Result<Element> {
    let mut reader = quick_xml::NsReader::from_reader(xml);
    reader.config_mut().expand_empty_elements = true;
    let mut stack: Vec<Element> = Vec::new();
    let error = |reader: &quick_xml::NsReader<&[u8]>, message: String| {
        crate::Error::Core(xeibe_core::Error::Xml {
            location: Location {
                source: SourceId(0),
                byte_offset: reader.buffer_position(),
                feature_seq: None,
                gml_id: None,
            },
            message,
        })
    };
    loop {
        let (resolved, event) = match reader.read_resolved_event() {
            Ok(event) => event,
            Err(e) => return Err(error(&reader, e.to_string())),
        };
        match event {
            Event::Start(start) => {
                let mut element = Element {
                    ns: namespace(resolved),
                    local: start.local_name().as_ref().to_string(),
                    ..Element::default()
                };
                for attr in start.attributes().with_checks(false).flatten() {
                    if attr.key.as_namespace_binding().is_some() {
                        continue;
                    }
                    let (_, local) = reader.resolver().resolve_attribute(attr.key);
                    let value = quick_xml::escape::unescape(&attr.value)
                        .map(Cow::into_owned)
                        .unwrap_or_else(|_| attr.value.to_string());
                    let value_ns = is_qname(&value)
                        .then(|| {
                            let name = quick_xml::name::QName(value.as_str());
                            namespace(reader.resolver().resolve_element(name).0)
                        })
                        .flatten();
                    element.attrs.push(Attr {
                        local: local.as_ref().to_string(),
                        value,
                        value_ns,
                    });
                }
                stack.push(element);
            }
            Event::End(_) => {
                let Some(mut element) = stack.pop() else {
                    return Err(error(&reader, "unexpected end tag".into()));
                };
                element.text = element.text.trim().to_string();
                // A prefixed name as text: resolve it while its scope is open.
                if let Some((prefix, _)) = element.text.split_once(':')
                    && !prefix.is_empty()
                    && !element.text.contains(char::is_whitespace)
                    && !element.text.contains('/')
                {
                    let name = quick_xml::name::QName(element.text.as_str());
                    element.text_ns = namespace(reader.resolver().resolve_element(name).0);
                }
                match stack.last_mut() {
                    Some(parent) => parent.children.push(element),
                    None => return Ok(element),
                }
            }
            Event::Text(text) => {
                if let Some(element) = stack.last_mut() {
                    element.text.push_str(&text.xml10_content());
                }
            }
            Event::CData(cdata) => {
                if let Some(element) = stack.last_mut() {
                    element.text.push_str(&cdata.xml10_content());
                }
            }
            Event::GeneralRef(reference) => {
                if let Some(element) = stack.last_mut() {
                    element.text.push_str(&expand_reference(&reference));
                }
            }
            Event::Eof => {
                let message = if stack.is_empty() {
                    "no root element"
                } else {
                    "unexpected end of input"
                };
                return Err(error(&reader, message.into()));
            }
            // A DTD is skipped rather than rejected: nothing in it is expanded, and
            // some old servers still put one in front of their capabilities.
            Event::Empty(_)
            | Event::Comment(_)
            | Event::PI(_)
            | Event::Decl(_)
            | Event::DocType(_) => {}
        }
    }
}

/// `true` for a value with the form of a QName: a name, or two joined by
/// one `:`. Nothing is resolved for other values (URLs, numbers, text).
fn is_qname(value: &str) -> bool {
    let name = |part: &str| {
        part.chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_')
            && part
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
    };
    match value.split_once(':') {
        Some((prefix, local)) => name(prefix) && name(local),
        None => name(value),
    }
}

/// Character references and the predefined entities; anything else is kept
/// as written.
fn expand_reference(reference: &quick_xml::events::BytesRef<'_>) -> String {
    if let Ok(Some(c)) = reference.resolve_char_ref() {
        return c.to_string();
    }
    let name: &str = reference;
    match name {
        "amp" => "&".into(),
        "lt" => "<".into(),
        "gt" => ">".into(),
        "quot" => "\"".into(),
        "apos" => "'".into(),
        other => format!("&{other};"),
    }
}

fn namespace(resolved: ResolveResult<'_>) -> Option<String> {
    match resolved {
        ResolveResult::Bound(Namespace(ns)) => Some(ns.to_string()),
        ResolveResult::Unbound | ResolveResult::Unknown(_) => None,
    }
}
