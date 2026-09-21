//! Thin layer over `quick-xml` with namespace resolution and DTD rejection.

use std::borrow::Cow;
use std::collections::HashMap;

use quick_xml::events::{BytesRef, Event};
use quick_xml::name::{Namespace, NamespaceResolver, PrefixDeclaration, ResolveResult};

use crate::{Location, NamespaceContext, QName, SourceId};

/// Events delivered to GML consumers (scan, geometry parser, feature builder).
///
/// Empty elements (`<a/>`) produce a `Start` and an `End`. Comments,
/// processing instructions and the XML declaration are skipped.
#[derive(Debug)]
pub enum XmlEvent<'a> {
    Start {
        name: QName,
        attrs: Attributes<'a>,
    },
    End {
        name: QName,
    },
    /// Unescaped text, including CDATA content. Adjacent text, CDATA sections
    /// and entity references are merged into one event.
    Text(Cow<'a, str>),
    Eof,
}

/// Attributes of a start tag, resolved to [`QName`]s lazily.
///
/// Namespace declarations (`xmlns`, `xmlns:*`) are not attributes. An
/// unprefixed attribute is in no namespace.
#[derive(Debug)]
pub struct Attributes<'a> {
    raw: quick_xml::events::attributes::Attributes<'a>,
    resolver: &'a NamespaceResolver,
}

impl<'a> Attributes<'a> {
    pub fn get(&self, name: &QName) -> Option<Cow<'a, str>> {
        self.iter_raw()
            .find(|(ns, local, _)| *local == &*name.local && *ns == name.ns.as_deref())
            .map(|(_, _, value)| value)
    }

    pub fn iter(&self) -> impl Iterator<Item = (QName, Cow<'a, str>)> + '_ {
        self.iter_raw()
            .map(|(ns, local, value)| (QName::new(ns, local), value))
    }

    /// `(namespace, local name, unescaped value)` without allocating names.
    /// Malformed attributes and attributes with an undeclared prefix are
    /// skipped; values that fail to unescape are returned raw.
    pub fn iter_raw(&self) -> impl Iterator<Item = (Option<&'a str>, &'a str, Cow<'a, str>)> + '_ {
        let mut raw = self.raw.clone();
        raw.with_checks(false);
        raw.filter_map(move |attr| {
            let attr = attr.ok()?;
            if attr.key.as_namespace_binding().is_some() {
                return None;
            }
            let (resolved, local) = self.resolver.resolve_attribute(attr.key);
            let ns = match resolved {
                ResolveResult::Unbound => None,
                ResolveResult::Bound(Namespace(ns)) => Some(ns),
                ResolveResult::Unknown(_) => return None,
            };
            let value = match quick_xml::escape::unescape(&attr.value) {
                Ok(Cow::Borrowed(_)) | Err(_) => attr.value,
                Ok(Cow::Owned(unescaped)) => Cow::Owned(unescaped),
            };
            Some((ns, local.into_inner(), value))
        })
    }
}

/// Namespace-aware pull reader over a UTF-8 buffer (one chunk or one document).
///
/// DTDs are rejected ([`crate::Error::DtdNotSupported`]) and only the
/// predefined entities and character references are expanded.
pub struct GmlReader<'a> {
    inner: quick_xml::NsReader<&'a [u8]>,
    base_offset: u64,
    buf: &'a [u8],
    source: SourceId,
    /// An event read ahead while merging text, with its start position.
    pending: Option<(Event<'a>, u64)>,
    /// Buffer position of the last `Start` returned, for [`Self::capture_element`].
    last_start: u64,
    /// Name, tag text and raw name length of the last `Start` returned, for
    /// [`Self::current_start`].
    last_start_tag: Option<(QName, &'a str, usize)>,
    /// Interned names: `"{ns}\0local"` → name, so repeated elements share `Arc`s.
    names: HashMap<String, QName>,
    scratch: String,
}

/// Bound on the name cache, against documents with unbounded distinct names.
const NAME_CACHE_LIMIT: usize = 4096;

impl<'a> GmlReader<'a> {
    /// `context` holds namespace declarations inherited from outside the buffer.
    pub fn new(buf: &'a [u8], context: &NamespaceContext, base_offset: u64) -> Self {
        let mut inner = quick_xml::NsReader::from_reader(buf);
        let config = inner.config_mut();
        config.expand_empty_elements = true;
        config.check_end_names = true;
        config.trim_text_start = false;
        config.trim_text_end = false;
        let resolver = inner.resolver_mut();
        resolver.set_max_namespace_bindings(usize::MAX);
        for (prefix, uri) in context.iter() {
            let prefix = match prefix {
                Some(prefix) => PrefixDeclaration::Named(prefix),
                None => PrefixDeclaration::Default,
            };
            // Only `xml`/`xmlns` misuse fails here; such a binding is ignored.
            let _ = resolver.add(prefix, Namespace(uri));
        }
        Self {
            inner,
            base_offset,
            buf,
            source: SourceId(0),
            pending: None,
            last_start: 0,
            last_start_tag: None,
            names: HashMap::new(),
            scratch: String::new(),
        }
    }

    /// Set the source reported in [`Self::location`] (default `SourceId(0)`).
    pub fn with_source(mut self, source: SourceId) -> Self {
        self.source = source;
        self
    }

    pub fn next_event(&mut self) -> crate::Result<XmlEvent<'_>> {
        loop {
            let (event, start) = self.read_raw()?;
            match event {
                Event::Start(start_tag) => {
                    self.last_start = start;
                    let name = self.resolve_element(start_tag.name())?;
                    // `raw` must borrow the input, not the event, so the
                    // attributes are read from the buffer.
                    let tag = self.tag_text(start);
                    let name_len = start_tag.name().as_ref().len();
                    self.last_start_tag = Some((name.clone(), tag, name_len));
                    return Ok(XmlEvent::Start {
                        name,
                        attrs: Attributes {
                            raw: quick_xml::events::attributes::Attributes::new(tag, name_len),
                            resolver: self.inner.resolver(),
                        },
                    });
                }
                Event::End(end) => {
                    let name = self.resolve_element(end.name())?;
                    return Ok(XmlEvent::End { name });
                }
                Event::Text(text) => {
                    let first = text.xml10_content();
                    return self.merge_text(first).map(XmlEvent::Text);
                }
                Event::CData(cdata) => {
                    let first = cdata.xml10_content();
                    return self.merge_text(first).map(XmlEvent::Text);
                }
                Event::GeneralRef(reference) => {
                    let first = Cow::Owned(self.expand_reference(&reference, start)?);
                    return self.merge_text(first).map(XmlEvent::Text);
                }
                Event::DocType(_) => {
                    return Err(crate::Error::DtdNotSupported(self.location_at(start)));
                }
                Event::Empty(_) => unreachable!("empty elements are expanded"),
                Event::Comment(_) | Event::PI(_) | Event::Decl(_) => continue,
                Event::Eof => return Ok(XmlEvent::Eof),
            }
        }
    }

    /// Skip the current element and its subtree without resolving names.
    ///
    /// Call it right after the element's `Start`; the next event is whatever
    /// follows the element's end tag.
    pub fn skip_element(&mut self) -> crate::Result<()> {
        let mut depth = 0usize;
        loop {
            let (event, start) = self.read_raw()?;
            match event {
                Event::Start(_) => depth += 1,
                Event::End(_) => {
                    if depth == 0 {
                        return Ok(());
                    }
                    depth -= 1;
                }
                Event::DocType(_) => {
                    return Err(crate::Error::DtdNotSupported(self.location_at(start)));
                }
                Event::Eof => {
                    return Err(self.xml_error(start, "unexpected end of input inside an element"));
                }
                _ => {}
            }
        }
    }

    /// Return the raw XML of the current element's subtree (for `RawXml` columns).
    ///
    /// Call it right after the element's `Start`. The result runs from the
    /// start tag to the end tag, as written; namespace declarations inherited
    /// from ancestors are not added.
    pub fn capture_element(&mut self) -> crate::Result<String> {
        let start = self.last_start_position();
        self.skip_element()?;
        Ok(self.raw_since(start))
    }

    /// Name and attributes of the last `Start` returned by [`Self::next_event`].
    ///
    /// For consumers handed the reader right after a start tag whose event
    /// the caller has already taken (the geometry parser, for example). Only
    /// meaningful until the next event is read: the namespace scope is the
    /// element's own only until then.
    pub fn current_start(&self) -> Option<(QName, Attributes<'_>)> {
        let (name, tag, name_len) = self.last_start_tag.as_ref()?;
        let attrs = Attributes {
            raw: quick_xml::events::attributes::Attributes::new(tag, *name_len),
            resolver: self.inner.resolver(),
        };
        Some((name.clone(), attrs))
    }

    /// Buffer position where the last `Start` returned begins; pass it to
    /// [`Self::raw_since`] later to get the element's raw XML.
    pub fn last_start_position(&self) -> usize {
        self.last_start as usize
    }

    /// The raw XML from `position` (see [`Self::last_start_position`]) to the
    /// current position, as written.
    pub fn raw_since(&self, position: usize) -> String {
        let end = self.inner.buffer_position() as usize;
        let raw = &self.buf[position.min(end)..end];
        String::from_utf8_lossy(raw).into_owned()
    }

    pub fn location(&self) -> Location {
        self.location_at(self.inner.buffer_position())
    }

    fn location_at(&self, position: u64) -> Location {
        Location {
            source: self.source,
            byte_offset: self.base_offset + position,
            feature_seq: None,
            gml_id: None,
        }
    }

    fn xml_error(&self, position: u64, message: impl Into<String>) -> crate::Error {
        crate::Error::Xml {
            location: self.location_at(position),
            message: message.into(),
        }
    }

    /// The next raw event (the read-ahead one first) and its start position.
    fn read_raw(&mut self) -> crate::Result<(Event<'a>, u64)> {
        if let Some(pending) = self.pending.take() {
            return Ok(pending);
        }
        let start = self.inner.buffer_position();
        match self.inner.read_event() {
            Ok(event) => Ok((event, start)),
            Err(error) => Err(self.xml_error(self.inner.error_position(), error.to_string())),
        }
    }

    /// Append the text, CDATA and references that follow `first`.
    fn merge_text(&mut self, first: Cow<'a, str>) -> crate::Result<Cow<'a, str>> {
        let mut text = first;
        loop {
            let (event, start) = self.read_raw()?;
            match event {
                Event::Text(more) => text.to_mut().push_str(&more.xml10_content()),
                Event::CData(more) => text.to_mut().push_str(&more.xml10_content()),
                Event::GeneralRef(reference) => {
                    let expanded = self.expand_reference(&reference, start)?;
                    text.to_mut().push_str(&expanded);
                }
                Event::Comment(_) | Event::PI(_) => {}
                other => {
                    self.pending = Some((other, start));
                    return Ok(text);
                }
            }
        }
    }

    /// Character references and the five predefined entities; anything else
    /// is an error (no DTD, so no other entity can be defined).
    fn expand_reference(&self, reference: &BytesRef<'_>, start: u64) -> crate::Result<String> {
        match reference.resolve_char_ref() {
            Ok(Some(c)) => return Ok(c.to_string()),
            Ok(None) => {}
            Err(error) => return Err(self.xml_error(start, error.to_string())),
        }
        let name: &str = reference;
        let expanded = match name {
            "amp" => "&",
            "lt" => "<",
            "gt" => ">",
            "quot" => "\"",
            "apos" => "'",
            _ => return Err(self.xml_error(start, format!("undefined entity &{name};"))),
        };
        Ok(expanded.to_string())
    }

    /// The text between `<` and `>` of the tag starting at `start`.
    fn tag_text(&self, start: u64) -> &'a str {
        let buf: &'a [u8] = self.buf;
        let end = self.inner.buffer_position() as usize;
        let start = start as usize + 1;
        let mut tag = &buf[start.min(end)..end];
        tag = tag.strip_suffix(b">").unwrap_or(tag);
        tag = tag.strip_suffix(b"/").unwrap_or(tag);
        // quick-xml already checked that the tag is UTF-8.
        std::str::from_utf8(tag).unwrap_or_default()
    }

    fn resolve_element(&mut self, name: quick_xml::name::QName<'_>) -> crate::Result<QName> {
        let (resolved, local) = self.inner.resolver().resolve_element(name);
        let ns = match resolved {
            ResolveResult::Unbound => None,
            ResolveResult::Bound(Namespace(ns)) => Some(ns),
            ResolveResult::Unknown(prefix) => {
                let message = format!(
                    "undeclared namespace prefix `{prefix}` in <{}>",
                    name.as_ref()
                );
                return Err(self.xml_error(self.inner.buffer_position(), message));
            }
        };
        let local = local.into_inner();
        self.scratch.clear();
        if let Some(ns) = ns {
            self.scratch.push_str(ns);
        }
        self.scratch.push('\0');
        self.scratch.push_str(local);
        if let Some(name) = self.names.get(self.scratch.as_str()) {
            return Ok(name.clone());
        }
        let qname = QName::new(ns, local);
        if self.names.len() >= NAME_CACHE_LIMIT {
            self.names.clear();
        }
        self.names.insert(self.scratch.clone(), qname.clone());
        Ok(qname)
    }
}
