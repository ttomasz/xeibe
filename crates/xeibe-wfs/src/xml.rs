//! Small XML helpers for capabilities and exception reports: UTF-8 decoding,
//! the root element's name, and a namespace-resolved element tree.
//!
//! Both documents are small, and reading them needs prefix resolution inside
//! text content (`<Name>ms:AD.Address</Name>`), so they are read into a tree
//! with `quick-xml` directly instead of through `xeibe_core::reader`.

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

/// An element with its resolved name, attributes, text and children.
#[derive(Debug, Default)]
pub(crate) struct Element {
    pub ns: Option<String>,
    pub local: String,
    /// `(namespace, local name, value)`; namespace declarations left out.
    pub attrs: Vec<(Option<String>, String, String)>,
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
            .find(|(_, l, _)| l == local)
            .map(|(_, _, v)| v.as_str())
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
                    let (ns, local) = reader.resolver().resolve_attribute(attr.key);
                    let value = quick_xml::escape::unescape(&attr.value)
                        .map(Cow::into_owned)
                        .unwrap_or_else(|_| attr.value.to_string());
                    element
                        .attrs
                        .push((namespace(ns), local.as_ref().to_string(), value));
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
