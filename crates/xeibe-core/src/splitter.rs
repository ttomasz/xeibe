//! Feature-boundary splitter: finds feature members without full parsing and
//! cuts the stream into [`FeatureChunk`]s (see `docs/architecture.md`).
//!
//! The splitter is a byte scanner, not an XML parser. It finds markup by its
//! `<`, skips comments, CDATA sections and processing instructions as a
//! whole, and reads start tags with quoted attribute values in mind. Names are
//! resolved only outside features: for the root, the containers and the
//! feature elements themselves. Inside a feature it only counts depth, and a
//! feature of a layer that was not asked for is passed over without being
//! copied. A collection's `boundedBy` is copied like a feature and travels
//! with the chunks of the features it bounds, which inherit its srsName.

use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::sync::Arc;

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::{FeatureChunk, Location, NamespaceContext, QName, RawElement, SourceId, ns};

/// Which elements wrap features.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemberRule {
    /// Each child element is one feature: `gml:featureMember`, `wfs:member`.
    OnePerElement(QName),
    /// Each child element of this container is a feature: `gml:featureMembers`.
    ManyPerElement(QName),
}

impl MemberRule {
    pub fn element(&self) -> &QName {
        match self {
            MemberRule::OnePerElement(name) | MemberRule::ManyPerElement(name) => name,
        }
    }

    /// `gml:featureMember` and `gml:featureMembers` in both GML namespaces,
    /// and WFS 2.0 `wfs:member`.
    pub fn builtin() -> Vec<MemberRule> {
        vec![
            MemberRule::OnePerElement(QName::new(Some(ns::GML_32), "featureMember")),
            MemberRule::OnePerElement(QName::new(Some(ns::GML), "featureMember")),
            MemberRule::ManyPerElement(QName::new(Some(ns::GML_32), "featureMembers")),
            MemberRule::ManyPerElement(QName::new(Some(ns::GML), "featureMembers")),
            MemberRule::OnePerElement(QName::new(Some(ns::WFS_20), "member")),
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SplitterOptions {
    /// Target chunk size; chunks are cut at the next feature boundary.
    pub target_chunk_bytes: usize,
    /// Built-in rules (`featureMember`, `featureMembers`, `wfs:member`) plus custom ones.
    pub member_rules: Vec<MemberRule>,
    /// Treat a document whose root is a feature as a single-feature dataset.
    ///
    /// The root counts as a feature when it is not a known collection and no
    /// member element turns up inside it. Until one does, the document is
    /// held in memory.
    pub allow_single_feature_root: bool,
    /// Only these feature types enter chunks; others are skipped unparsed.
    /// Set by every read (one layer); `None` for scans. An entry without a
    /// namespace matches that local name in any namespace (a read that names
    /// its layer by local name only).
    #[serde(skip)]
    pub layers: Option<Vec<QName>>,
}

impl Default for SplitterOptions {
    /// 2 MB chunks, the built-in member rules, no single-feature root, every layer.
    fn default() -> Self {
        Self {
            target_chunk_bytes: 2 << 20,
            member_rules: MemberRule::builtin(),
            allow_single_feature_root: false,
            layers: None,
        }
    }
}

/// The layer filter: exact names, or local names for entries without a namespace.
fn layer_wanted(layers: &[QName], name: &QName) -> bool {
    layers
        .iter()
        .any(|layer| layer == name || (layer.ns.is_none() && layer.local == name.local))
}

/// Information about the document root, collected before the first chunk.
#[derive(Debug, Clone)]
pub struct DocumentHeader {
    pub root: QName,
    pub namespaces: NamespaceContext,
    pub schema_location: Option<String>,
    /// WFS 2.0 `numberMatched` / `numberReturned` / `next`, WFS 1.1 `numberOfFeatures`.
    pub wfs_attributes: Vec<(QName, String)>,
    /// Root declares `xmlns:fme="http://www.safe.com/gml/fme"`.
    pub fme_produced: bool,
}

/// Unprefixed root attributes kept in [`DocumentHeader::wfs_attributes`].
const WFS_ATTRIBUTES: &[&str] = &[
    "numberMatched",
    "numberReturned",
    "numberOfFeatures",
    "next",
    "previous",
    "timeStamp",
];

/// Bytes requested from the reader at a time.
const READ_BLOCK: usize = 256 * 1024;

/// Iterator of chunks over one decoded source.
///
/// Features are copied into a chunk without the member elements around
/// them and without the text between them, so a chunk is a sequence of
/// sibling feature elements. Namespaces declared outside the features are in
/// [`FeatureChunk::namespaces`]; declarations on a feature element stay in
/// its bytes.
pub struct FeatureSplitter<R: Read> {
    reader: R,
    source: SourceId,
    options: SplitterOptions,
    header: Option<DocumentHeader>,
    next_seq: u64,

    /// Read-ahead buffer; `buf[pos..]` is unread, `buf[0]` is at stream offset `base`.
    buf: Vec<u8>,
    pos: usize,
    base: u64,
    eof: bool,
    /// Scratch space the reader fills, allocated on the first read.
    block: Box<[u8]>,

    mode: Mode,
    /// Open elements outside features.
    stack: Vec<Frame>,
    /// Namespace declarations of the open elements, innermost last.
    bindings: Vec<(Option<Arc<str>>, Arc<str>)>,
    /// Bumped whenever `bindings` changes.
    bindings_generation: u64,
    /// The context built for `context_generation`.
    context: Arc<NamespaceContext>,
    context_generation: u64,

    /// A member rule element or a known collection was seen.
    saw_container: bool,
    /// Start of the root while it may still turn out to be a single feature.
    root_candidate: Option<u64>,
    /// The root was emitted as a single feature.
    root_feature: bool,
    referenced_members: u64,

    chunk: Vec<u8>,
    chunk_context: Arc<NamespaceContext>,
    chunk_offset: u64,
    chunk_first_feature: u64,
    chunk_bounded_by: Option<Arc<RawElement>>,
    features_emitted: u64,
    ready: VecDeque<FeatureChunk>,
    finished: bool,
    /// Reused for the start tags that are parsed.
    tag: Vec<u8>,
    /// The name of the element in [`Mode::Feature`], as written.
    feature_name: Vec<u8>,
    /// Element names resolved under `names_generation` of `bindings`.
    names: HashMap<Box<str>, Option<QName>>,
    names_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Before the root element.
    Prolog,
    /// Inside the root, outside any feature.
    Outside,
    /// Inside a feature that started at stream offset `start`, or inside a
    /// collection's `boundedBy` (`bounded_by`), which is copied too. `depth`
    /// counts the open elements named like it (`feature_name`), itself included.
    Feature {
        depth: usize,
        emit: bool,
        start: u64,
        bounded_by: bool,
    },
    /// After the root element.
    Epilog,
}

/// An open element outside features.
#[derive(Debug)]
struct Frame {
    /// The name as written, to check the end tag.
    raw_name: Box<[u8]>,
    /// Children of this element are features (a member rule matched).
    members: bool,
    /// For member elements: a child element was seen.
    has_child: bool,
    /// Length of `bindings` before this element's declarations.
    bindings_before: usize,
    /// The element's `boundedBy`, for the features inside it.
    bounded_by: Option<Arc<RawElement>>,
}

#[derive(Debug, Clone, Copy)]
enum Kind {
    /// `name_end`: index where the name ends.
    Start {
        name_end: usize,
        empty: bool,
    },
    End,
    /// Comment, CDATA, processing instruction or XML declaration.
    Other,
}

/// One piece of markup; indices into `buf`, valid until the next read.
#[derive(Debug, Clone, Copy)]
struct Markup {
    kind: Kind,
    start: usize,
    /// Index just after the closing `>`.
    end: usize,
}

impl<R: Read> FeatureSplitter<R> {
    pub fn new(reader: R, source: SourceId, options: SplitterOptions) -> Self {
        let context = Arc::new(NamespaceContext::new());
        Self {
            reader,
            source,
            options,
            header: None,
            next_seq: 0,
            buf: Vec::with_capacity(READ_BLOCK),
            pos: 0,
            base: 0,
            eof: false,
            block: Box::default(),
            mode: Mode::Prolog,
            stack: Vec::new(),
            bindings: Vec::new(),
            bindings_generation: 0,
            chunk_context: context.clone(),
            context,
            context_generation: 0,
            saw_container: false,
            root_candidate: None,
            root_feature: false,
            referenced_members: 0,
            chunk: Vec::new(),
            chunk_offset: 0,
            chunk_first_feature: 0,
            chunk_bounded_by: None,
            features_emitted: 0,
            ready: VecDeque::new(),
            finished: false,
            tag: Vec::new(),
            feature_name: Vec::new(),
            names: HashMap::new(),
            names_generation: 0,
        }
    }

    /// Read up to the first feature member and return the document header.
    ///
    /// The header is complete once the root start tag is read; chunks found
    /// on the way are kept for the iterator.
    pub fn header(&mut self) -> crate::Result<&DocumentHeader> {
        while self.header.is_none() {
            if self.finished {
                return Err(crate::Error::NoFeatures(self.source.to_string()));
            }
            if let Err(error) = self.step() {
                self.finished = true;
                return Err(error);
            }
        }
        Ok(self.header.as_ref().expect("the header was just read"))
    }

    /// Member elements without a child element seen so far (members by
    /// reference, `xlink:href`), which are not features.
    pub fn referenced_members(&self) -> u64 {
        self.referenced_members
    }

    /// Features that entered chunks so far (after the layer filter).
    pub fn features_emitted(&self) -> u64 {
        self.features_emitted
    }

    /// Process one piece of markup, or the end of the input.
    fn step(&mut self) -> crate::Result<()> {
        let markup = match self.mode {
            Mode::Feature { .. } => self.next_feature_markup()?,
            _ => self.next_markup()?,
        };
        let Some(markup) = markup else {
            return self.finish();
        };
        match self.mode {
            Mode::Feature {
                depth,
                emit,
                start,
                bounded_by,
            } => match markup.kind {
                Kind::Start { empty: false, .. } => {
                    self.mode = Mode::Feature {
                        depth: depth + 1,
                        emit,
                        start,
                        bounded_by,
                    };
                }
                Kind::End if depth == 1 => {
                    self.mode = Mode::Outside;
                    if bounded_by {
                        self.end_bounded_by(start, markup.end);
                    } else {
                        self.end_feature(emit, start, markup.end);
                    }
                }
                Kind::End => {
                    self.mode = Mode::Feature {
                        depth: depth - 1,
                        emit,
                        start,
                        bounded_by,
                    }
                }
                _ => {}
            },
            Mode::Prolog | Mode::Outside => match markup.kind {
                Kind::Start { .. } => self.start_outside(markup)?,
                Kind::End => self.end_outside(markup)?,
                Kind::Other => {}
            },
            Mode::Epilog => {
                if !matches!(markup.kind, Kind::Other) {
                    return Err(self.error(markup.start, "content after the root element"));
                }
            }
        }
        Ok(())
    }

    /// End of input: check the document is complete and flush the last chunk.
    fn finish(&mut self) -> crate::Result<()> {
        self.finished = true;
        if self.mode != Mode::Epilog && self.mode != Mode::Prolog {
            return Err(self.error(self.buf.len(), "unexpected end of input"));
        }
        self.flush_chunk();
        if !self.saw_container && !self.root_feature {
            return Err(crate::Error::NoFeatures(self.source.to_string()));
        }
        Ok(())
    }

    /// A start tag outside features: the root, a container or a feature.
    fn start_outside(&mut self, markup: Markup) -> crate::Result<()> {
        let Kind::Start { name_end, empty } = markup.kind else {
            unreachable!()
        };
        let tag_end = if empty {
            markup.end - 2
        } else {
            markup.end - 1
        };
        let mut tag = std::mem::take(&mut self.tag);
        tag.clear();
        tag.extend_from_slice(&self.buf[markup.start + 1..tag_end]);
        let result = self.start_tag(markup, &tag, name_end - markup.start - 1, empty);
        self.tag = tag;
        result
    }

    fn start_tag(
        &mut self,
        markup: Markup,
        tag: &[u8],
        name_len: usize,
        empty: bool,
    ) -> crate::Result<()> {
        let Ok(tag) = std::str::from_utf8(tag) else {
            return Err(self.error(markup.start, "invalid UTF-8 in a start tag"));
        };
        let raw_name = &tag[..name_len];
        let attributes = parse_attributes(&tag[name_len..])
            .map_err(|message| self.error(markup.start, message))?;

        let bindings_before = self.bindings.len();
        for (key, value) in &attributes {
            let prefix = if *key == "xmlns" {
                None
            } else if let Some(prefix) = key.strip_prefix("xmlns:") {
                Some(Arc::from(prefix))
            } else {
                continue;
            };
            self.bindings
                .push((prefix, Arc::from(unescape(value).as_ref())));
        }
        let declared = self.bindings.len() > bindings_before;
        if declared {
            self.bindings_generation += 1;
        }
        let Some(name) = self.resolve_element(raw_name) else {
            return Err(self.error(
                markup.start,
                format!("undeclared namespace prefix in <{raw_name}>"),
            ));
        };
        let start = self.base + markup.start as u64;
        let is_root = self.mode == Mode::Prolog;
        if is_root {
            self.read_header(&name, &attributes, bindings_before);
            self.mode = Mode::Outside;
        }
        let collection = is_collection(&name);

        let parent_has_members = self.stack.last().is_some_and(|frame| frame.members);
        if parent_has_members && !collection {
            // A feature. Its own declarations travel in its bytes.
            if declared {
                self.bindings.truncate(bindings_before);
                self.bindings_generation += 1;
            }
            if let Some(parent) = self.stack.last_mut() {
                parent.has_child = true;
            }
            let emit = self
                .options
                .layers
                .as_ref()
                .is_none_or(|layers| layer_wanted(layers, &name));
            if emit {
                self.begin_feature(start);
            }
            if empty {
                self.end_feature(emit, start, markup.end);
            } else {
                self.enter_feature(raw_name);
                self.mode = Mode::Feature {
                    depth: 1,
                    emit,
                    start,
                    bounded_by: false,
                };
            }
            return Ok(());
        }

        if !is_root && is_bounded_by(&name) {
            // The envelope of the collection this element is in, copied for
            // the features it bounds. Its declarations travel in its bytes.
            if declared {
                self.bindings.truncate(bindings_before);
                self.bindings_generation += 1;
            }
            if !empty {
                self.enter_feature(raw_name);
                self.mode = Mode::Feature {
                    depth: 1,
                    emit: true,
                    start,
                    bounded_by: true,
                };
            }
            return Ok(());
        }

        let members = self
            .options
            .member_rules
            .iter()
            .any(|rule| rule.element() == &name);
        if members || collection {
            self.saw_container = true;
            self.root_candidate = None;
        } else if is_root && self.options.allow_single_feature_root {
            self.root_candidate = Some(start);
        }
        if let Some(parent) = self.stack.last_mut() {
            parent.has_child = true;
        }
        if empty {
            if declared {
                self.bindings.truncate(bindings_before);
                self.bindings_generation += 1;
            }
            if members {
                self.referenced_members += 1;
            }
            if is_root {
                self.end_root(markup.end);
            }
            return Ok(());
        }
        self.stack.push(Frame {
            raw_name: raw_name.as_bytes().into(),
            members,
            has_child: false,
            bindings_before,
            bounded_by: None,
        });
        Ok(())
    }

    /// An end tag outside features.
    fn end_outside(&mut self, markup: Markup) -> crate::Result<()> {
        let name = self.buf[markup.start + 2..markup.end - 1].trim_ascii();
        let Some(frame) = self.stack.pop() else {
            return Err(self.error(markup.start, "end tag without a start tag"));
        };
        if *frame.raw_name != *name {
            let message = format!(
                "end tag </{}> does not match <{}>",
                String::from_utf8_lossy(name),
                String::from_utf8_lossy(&frame.raw_name)
            );
            return Err(self.error(markup.start, message));
        }
        if frame.members && !frame.has_child {
            self.referenced_members += 1;
        }
        if self.bindings.len() > frame.bindings_before {
            self.bindings.truncate(frame.bindings_before);
            self.bindings_generation += 1;
        }
        if self.stack.is_empty() {
            self.end_root(markup.end);
        }
        Ok(())
    }

    fn enter_feature(&mut self, raw_name: &str) {
        self.feature_name.clear();
        self.feature_name.extend_from_slice(raw_name.as_bytes());
    }

    /// The root element ended at `end`; a root that is still a candidate is
    /// the one feature of the document.
    fn end_root(&mut self, end: usize) {
        self.mode = Mode::Epilog;
        let Some(start) = self.root_candidate.take() else {
            return;
        };
        self.root_feature = true;
        let root = self.header.as_ref().map(|header| &header.root);
        let emit = match (&self.options.layers, root) {
            (Some(layers), Some(root)) => layer_wanted(layers, root),
            _ => true,
        };
        if emit {
            self.begin_feature(start);
        }
        self.end_feature(emit, start, end);
    }

    fn read_header(&mut self, root: &QName, attributes: &[(&str, &str)], bindings_before: usize) {
        let mut namespaces = NamespaceContext::new();
        for (prefix, uri) in &self.bindings[bindings_before..] {
            namespaces.declare(prefix.as_deref(), uri);
        }
        let mut schema_location = None;
        let mut wfs_attributes = Vec::new();
        for (key, value) in attributes {
            if key.starts_with("xmlns") && (key.len() == 5 || key.as_bytes()[5] == b':') {
                continue;
            }
            match key.split_once(':') {
                Some(_) => {
                    let name = self.resolve(key, false);
                    if name.is_some_and(|n| {
                        n.ns.as_deref() == Some(ns::XSI) && &*n.local == "schemaLocation"
                    }) {
                        schema_location = Some(unescape(value).into_owned());
                    }
                }
                None if WFS_ATTRIBUTES.contains(key) => {
                    wfs_attributes.push((QName::new(None, key), unescape(value).into_owned()));
                }
                None => {}
            }
        }
        self.header = Some(DocumentHeader {
            root: root.clone(),
            fme_produced: namespaces.declares_uri(ns::FME),
            namespaces,
            schema_location,
            wfs_attributes,
        });
    }

    /// Resolve a prefixed name against the open elements' declarations.
    /// [`Self::resolve`] for an element name, remembered while the bindings
    /// stay the same: the same few names repeat for every feature.
    fn resolve_element(&mut self, raw: &str) -> Option<QName> {
        if self.names_generation != self.bindings_generation {
            self.names.clear();
            self.names_generation = self.bindings_generation;
        }
        if let Some(name) = self.names.get(raw) {
            return name.clone();
        }
        let name = self.resolve(raw, true);
        self.names.insert(raw.into(), name.clone());
        name
    }

    fn resolve(&self, raw: &str, element: bool) -> Option<QName> {
        let (prefix, local) = match raw.split_once(':') {
            Some((prefix, local)) => (Some(prefix), local),
            None => (None, raw),
        };
        if prefix == Some("xml") {
            return Some(QName::new(
                Some("http://www.w3.org/XML/1998/namespace"),
                local,
            ));
        }
        if prefix.is_none() && !element {
            return Some(QName::new(None, local));
        }
        match self
            .bindings
            .iter()
            .rev()
            .find(|(p, _)| p.as_deref() == prefix)
        {
            Some((_, uri)) if !uri.is_empty() => Some(QName::new(Some(uri), local)),
            // `xmlns=""` undeclares the default namespace; `xmlns:p=""` is invalid.
            Some(_) | None => prefix.is_none().then(|| QName::new(None, local)),
        }
    }

    /// The namespace context of the open elements, shared while it is unchanged.
    fn current_context(&mut self) -> Arc<NamespaceContext> {
        if self.context_generation != self.bindings_generation {
            let mut context = NamespaceContext::new();
            for (prefix, uri) in &self.bindings {
                context.declare(prefix.as_deref(), uri);
            }
            if *self.context != context {
                self.context = Arc::new(context);
            }
            self.context_generation = self.bindings_generation;
        }
        self.context.clone()
    }

    /// A feature that enters a chunk starts at stream offset `start`.
    fn begin_feature(&mut self, start: u64) {
        let context = self.current_context();
        let bounded_by = self.collection_bounded_by();
        let same_bounds = match (bounded_by, &self.chunk_bounded_by) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (a, b) => a.is_none() && b.is_none(),
        };
        if !self.chunk.is_empty() && (!Arc::ptr_eq(&context, &self.chunk_context) || !same_bounds) {
            self.flush_chunk();
        }
        if self.chunk.is_empty() {
            // A chunk grows to about the target: no reallocation on the way.
            self.chunk.reserve(self.options.target_chunk_bytes);
            self.chunk_bounded_by = self.collection_bounded_by().cloned();
            self.chunk_context = context;
            self.chunk_offset = start;
            self.chunk_first_feature = self.features_emitted;
        }
    }

    /// The `boundedBy` of the innermost open collection that has one.
    fn collection_bounded_by(&self) -> Option<&Arc<RawElement>> {
        self.stack.iter().rev().find_map(|frame| frame.bounded_by.as_ref())
    }

    /// A collection's `boundedBy` that started at stream offset `start` ended
    /// at `end` (an index into `buf`).
    fn end_bounded_by(&mut self, start: u64, end: usize) {
        let from = (start - self.base) as usize;
        let element = RawElement {
            source: self.source,
            byte_offset: start,
            bytes: Bytes::copy_from_slice(&self.buf[from..end]),
            namespaces: self.current_context(),
        };
        if let Some(collection) = self.stack.last_mut() {
            collection.bounded_by = Some(Arc::new(element));
        }
    }

    /// A feature ends at `end` (an index into `buf`).
    fn end_feature(&mut self, emit: bool, start: u64, end: usize) {
        if !emit {
            return;
        }
        let from = (start - self.base) as usize;
        self.chunk.extend_from_slice(&self.buf[from..end]);
        self.features_emitted += 1;
        if self.chunk.len() >= self.options.target_chunk_bytes {
            self.flush_chunk();
        }
    }

    fn flush_chunk(&mut self) {
        if self.chunk.is_empty() {
            return;
        }
        let bytes = Bytes::from(std::mem::take(&mut self.chunk));
        self.ready.push_back(FeatureChunk {
            source: self.source,
            seq: self.next_seq,
            byte_offset: self.chunk_offset,
            bytes,
            namespaces: self.chunk_context.clone(),
            first_feature_seq: self.chunk_first_feature,
            collection_bounded_by: self.chunk_bounded_by.clone(),
        });
        self.next_seq += 1;
    }

    fn error(&self, index: usize, message: impl Into<String>) -> crate::Error {
        crate::Error::Xml {
            location: Location {
                source: self.source,
                byte_offset: self.base + index as u64,
                feature_seq: None,
                gml_id: None,
            },
            message: message.into(),
        }
    }

    // --- Byte scanning -----------------------------------------------------

    /// The next piece of markup, skipping text. `None` at the end of input.
    fn next_markup(&mut self) -> crate::Result<Option<Markup>> {
        loop {
            if let Some(i) = memchr::memchr(b'<', &self.buf[self.pos..]) {
                self.pos += i;
                break;
            }
            self.pos = self.buf.len();
            if !self.fill()? {
                return Ok(None);
            }
        }
        // From here on `pos` stays at the `<`, so reads keep the markup.
        self.need(2)?;
        let (kind, len) = match self.buf[self.pos + 1] {
            b'/' => (Kind::End, self.find(2, b">")? + 1),
            b'?' => (Kind::Other, self.find(2, b"?>")? + 2),
            b'!' => {
                self.need(4)?;
                if &self.buf[self.pos + 2..self.pos + 4] == b"--" {
                    (Kind::Other, self.find(4, b"-->")? + 3)
                } else {
                    self.need(9)?;
                    let head = &self.buf[self.pos..self.pos + 9];
                    if head == b"<![CDATA[" {
                        (Kind::Other, self.find(9, b"]]>")? + 3)
                    } else if head == b"<!DOCTYPE" {
                        return Err(crate::Error::DtdNotSupported(self.location(self.pos)));
                    } else {
                        return Err(self.error(self.pos, "unsupported markup declaration"));
                    }
                }
            }
            _ => {
                let (close, name_end) = self.scan_start_tag()?;
                let empty = self.buf[self.pos + close - 1] == b'/';
                let name_end = self.pos + name_end;
                (Kind::Start { name_end, empty }, close + 1)
            }
        };
        let start = self.pos;
        self.pos += len;
        Ok(Some(Markup {
            kind,
            start,
            end: self.pos,
        }))
    }

    /// Inside a feature, the next markup that can end it or hide a `<`: a
    /// tag named like the feature, a comment, CDATA or a processing
    /// instruction. XML allows no raw `<` in text or attribute values, so
    /// any other tag is passed over at its `<`, without looking for its end.
    fn next_feature_markup(&mut self) -> crate::Result<Option<Markup>> {
        loop {
            let Some(i) = memchr::memchr(b'<', &self.buf[self.pos..]) else {
                self.pos = self.buf.len();
                if !self.fill()? {
                    return Ok(None);
                }
                continue;
            };
            self.pos += i;
            self.need(2)?;
            let name_at = match self.buf[self.pos + 1] {
                b'!' | b'?' => return self.next_markup(),
                b'/' => 2,
                _ => 1,
            };
            let name_len = self.feature_name.len();
            self.need(name_at + name_len + 1)?;
            let at = self.pos + name_at;
            // The byte after the name rules out most tags before comparing names.
            let after = self.buf[at + name_len];
            if (after.is_ascii_whitespace() || after == b'>' || (name_at == 1 && after == b'/'))
                && self.buf[at..at + name_len] == self.feature_name[..]
            {
                return self.next_markup();
            }
            self.pos += 1;
        }
    }

    fn location(&self, index: usize) -> Location {
        Location {
            source: self.source,
            byte_offset: self.base + index as u64,
            feature_seq: None,
            gml_id: None,
        }
    }

    /// Make sure `buf[pos..pos + n]` is available.
    fn need(&mut self, n: usize) -> crate::Result<()> {
        while self.buf.len() - self.pos < n {
            if !self.fill()? {
                return Err(self.error(self.pos, "unexpected end of input in markup"));
            }
        }
        Ok(())
    }

    /// Offset from `pos` of the first `pattern` at or after `pos + from`.
    fn find(&mut self, mut from: usize, pattern: &[u8]) -> crate::Result<usize> {
        loop {
            let haystack = &self.buf[self.pos + from..];
            let found = match pattern {
                [byte] => memchr::memchr(*byte, haystack),
                _ => memchr::memmem::find(haystack, pattern),
            };
            if let Some(i) = found {
                return Ok(from + i);
            }
            from = (self.buf.len() - self.pos)
                .saturating_sub(pattern.len() - 1)
                .max(from);
            if !self.fill()? {
                return Err(self.error(self.pos, "unexpected end of input in markup"));
            }
        }
    }

    /// Offsets from `pos` of the closing `>` of a start tag and of the end of
    /// its name. A `>` inside a quoted attribute value does not close the tag.
    fn scan_start_tag(&mut self) -> crate::Result<(usize, usize)> {
        let mut i = 1;
        let name_end = loop {
            let tag = &self.buf[self.pos..];
            if let Some(j) = tag[i..]
                .iter()
                .position(|&b| b.is_ascii_whitespace() || b == b'/' || b == b'>')
            {
                break i + j;
            }
            i = tag.len();
            if !self.fill()? {
                return Err(self.error(self.pos, "unexpected end of input in a start tag"));
            }
        };
        // Past the name, jump from quote to quote to the closing `>`.
        let mut i = name_end;
        let mut quote = None;
        loop {
            let tag = &self.buf[self.pos..];
            while i < tag.len() {
                let found = match quote {
                    Some(q) => memchr::memchr(q, &tag[i..]),
                    None => memchr::memchr3(b'>', b'"', b'\'', &tag[i..]),
                };
                let Some(j) = found else {
                    i = tag.len();
                    break;
                };
                i += j;
                match (quote, tag[i]) {
                    (None, b'>') => return Ok((i, name_end)),
                    (None, q) => quote = Some(q),
                    (Some(_), _) => quote = None,
                }
                i += 1;
            }
            if !self.fill()? {
                return Err(self.error(self.pos, "unexpected end of input in a start tag"));
            }
        }
    }

    /// Read more input, first dropping what is no longer needed. `false` at
    /// the end of input.
    fn fill(&mut self) -> crate::Result<bool> {
        if self.eof {
            return Ok(false);
        }
        self.compact();
        if self.block.is_empty() {
            self.block = vec![0; READ_BLOCK].into_boxed_slice();
        }
        let n = loop {
            match self.reader.read(&mut self.block) {
                Ok(n) => break n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        };
        self.buf.extend_from_slice(&self.block[..n]);
        if n == 0 {
            self.eof = true;
        }
        Ok(n > 0)
    }

    /// Drop consumed bytes, keeping the feature being copied and a root that
    /// may still be a single feature.
    fn compact(&mut self) {
        let mut keep = self.pos;
        if let Mode::Feature {
            emit: true, start, ..
        } = self.mode
        {
            keep = keep.min((start - self.base) as usize);
        }
        if let Some(start) = self.root_candidate {
            keep = keep.min((start - self.base) as usize);
        }
        if keep == 0 {
            return;
        }
        self.buf.drain(..keep);
        self.pos -= keep;
        self.base += keep as u64;
    }
}

impl<R: Read> Iterator for FeatureSplitter<R> {
    type Item = crate::Result<FeatureChunk>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(chunk) = self.ready.pop_front() {
                return Some(Ok(chunk));
            }
            if self.finished {
                return None;
            }
            if let Err(error) = self.step() {
                self.finished = true;
                self.ready.clear();
                return Some(Err(error));
            }
        }
    }
}

/// The `boundedBy` of a collection: GML's, or WFS's own (`wfs:boundedBy`,
/// 2.0).
fn is_bounded_by(name: &QName) -> bool {
    &*name.local == "boundedBy"
        && matches!(
            name.ns.as_deref(),
            Some(ns::GML | ns::GML_32 | ns::WFS | ns::WFS_20)
        )
}

/// Collection elements, which are containers even inside a member.
fn is_collection(name: &QName) -> bool {
    match name.ns.as_deref() {
        Some(ns::GML) | Some(ns::GML_32) => &*name.local == "FeatureCollection",
        Some(ns::WFS) | Some(ns::WFS_20) => matches!(
            &*name.local,
            "FeatureCollection" | "SimpleFeatureCollection" | "additionalObjects"
        ),
        _ => false,
    }
}

/// `name="value"` pairs of a start tag, after the element name. Values are
/// returned as written (escaped).
fn parse_attributes(text: &str) -> Result<Vec<(&str, &str)>, String> {
    let bytes = text.as_bytes();
    let mut attributes = Vec::new();
    let mut i = 0;
    let skip_space = |i: &mut usize| {
        while *i < bytes.len() && bytes[*i].is_ascii_whitespace() {
            *i += 1;
        }
    };
    loop {
        skip_space(&mut i);
        if i >= bytes.len() {
            return Ok(attributes);
        }
        let key_start = i;
        while i < bytes.len() && bytes[i] != b'=' && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let key = &text[key_start..i];
        skip_space(&mut i);
        if bytes.get(i) != Some(&b'=') {
            return Err(format!("attribute `{key}` has no value"));
        }
        i += 1;
        skip_space(&mut i);
        let quote = match bytes.get(i) {
            Some(&q @ (b'"' | b'\'')) => q,
            _ => return Err(format!("attribute `{key}` has an unquoted value")),
        };
        i += 1;
        let value_start = i;
        while i < bytes.len() && bytes[i] != quote {
            i += 1;
        }
        if i >= bytes.len() {
            return Err(format!("attribute `{key}` has an unterminated value"));
        }
        attributes.push((key, &text[value_start..i]));
        i += 1;
    }
}

/// Expand entity and character references in an attribute value; an
/// invalid reference leaves the value as written.
fn unescape(value: &str) -> std::borrow::Cow<'_, str> {
    quick_xml::escape::unescape(value).unwrap_or(std::borrow::Cow::Borrowed(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns at most `step` bytes per read, to put every boundary case of
    /// the buffer handling at every position.
    struct Trickle<'a> {
        data: &'a [u8],
        step: usize,
    }

    impl Read for Trickle<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.step.min(buf.len()).min(self.data.len());
            buf[..n].copy_from_slice(&self.data[..n]);
            self.data = &self.data[n..];
            Ok(n)
        }
    }

    fn split(reader: impl Read, options: SplitterOptions) -> Vec<(u64, u64, Vec<u8>)> {
        FeatureSplitter::new(reader, SourceId(0), options)
            .map(|chunk| {
                let chunk = chunk.expect("the document splits");
                (
                    chunk.byte_offset,
                    chunk.first_feature_seq,
                    chunk.bytes.to_vec(),
                )
            })
            .collect()
    }

    fn with_target(target_chunk_bytes: usize) -> SplitterOptions {
        SplitterOptions {
            target_chunk_bytes,
            ..SplitterOptions::default()
        }
    }

    #[test]
    fn chunks_do_not_depend_on_how_the_input_arrives() {
        let document = concat!(
            "<?xml version=\"1.0\"?>\n<!-- a <comment> -->\n",
            "<gml:FeatureCollection xmlns:gml=\"http://www.opengis.net/gml/3.2\" a='x>y'>",
            "<gml:featureMember xmlns:app=\"http://example.com/app\">",
            "<app:A gml:id=\"a1\" note=\"1 > 0\"><app:v><![CDATA[</app:A>]]></app:v></app:A>",
            "</gml:featureMember>\n<?pi x?>",
            "<gml:featureMember><app:B xmlns:app=\"http://example.com/app\"/></gml:featureMember>",
            "<gml:featureMember xlink:href=\"#a1\" xmlns:xlink=\"http://www.w3.org/1999/xlink\"/>",
            "<gml:featureMembers xmlns:app=\"http://example.com/app\">",
            "<app:A/><app:B><app:B/></app:B></gml:featureMembers>",
            "</gml:FeatureCollection>\n"
        );
        for target in [1, 64, 1 << 20] {
            let expected = split(document.as_bytes(), with_target(target));
            for step in [1, 2, 3, 7, 13] {
                let trickle = Trickle {
                    data: document.as_bytes(),
                    step,
                };
                assert_eq!(
                    split(trickle, with_target(target)),
                    expected,
                    "target {target}, step {step}"
                );
            }
        }
        let per_feature = split(document.as_bytes(), with_target(1));
        let firsts: Vec<u64> = per_feature.iter().map(|c| c.1).collect();
        assert_eq!(firsts, [0, 1, 2, 3]);
        assert!(per_feature[0].2.ends_with(b"]]></app:v></app:A>"));
    }

    #[test]
    fn member_references_are_counted_not_split() {
        let document = concat!(
            "<wfs:FeatureCollection xmlns:wfs=\"http://www.opengis.net/wfs/2.0\">",
            "<wfs:member xlink:href=\"#x\"/><wfs:member></wfs:member>",
            "</wfs:FeatureCollection>"
        );
        let mut splitter =
            FeatureSplitter::new(document.as_bytes(), SourceId(0), SplitterOptions::default());
        assert_eq!(splitter.by_ref().count(), 0);
        assert_eq!(splitter.referenced_members(), 2);
    }

    #[test]
    fn a_truncated_document_is_an_error() {
        let document = concat!(
            "<gml:FeatureCollection xmlns:gml=\"http://www.opengis.net/gml/3.2\">",
            "<gml:featureMember><a>"
        );
        let result: crate::Result<Vec<_>> =
            FeatureSplitter::new(document.as_bytes(), SourceId(0), SplitterOptions::default())
                .collect();
        assert!(
            matches!(result, Err(crate::Error::Xml { .. })),
            "{result:?}"
        );
    }

    #[test]
    fn a_mismatched_end_tag_outside_features_is_an_error() {
        let document = concat!(
            "<gml:FeatureCollection xmlns:gml=\"http://www.opengis.net/gml/3.2\">",
            "<gml:featureMember></gml:featureMembers></gml:FeatureCollection>"
        );
        let result: crate::Result<Vec<_>> =
            FeatureSplitter::new(document.as_bytes(), SourceId(0), SplitterOptions::default())
                .collect();
        assert!(
            matches!(result, Err(crate::Error::Xml { .. })),
            "{result:?}"
        );
    }
}
