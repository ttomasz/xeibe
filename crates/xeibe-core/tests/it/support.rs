//! Helpers shared by the `xeibe-core` tests.

#![allow(dead_code)]

use std::path::PathBuf;

use xeibe_core::reader::{GmlReader, XmlEvent};
use xeibe_core::{
    FeatureChunk, FeatureSplitter, NamespaceContext, QName, SourceId, SplitterOptions, ns,
};

/// Split a document held in memory.
pub fn split(document: &str, options: SplitterOptions) -> Vec<FeatureChunk> {
    try_split(document, options).expect("the document splits")
}

pub fn try_split(
    document: &str,
    options: SplitterOptions,
) -> xeibe_core::Result<Vec<FeatureChunk>> {
    FeatureSplitter::new(document.as_bytes(), SourceId(0), options).collect()
}

/// One chunk per feature, which makes chunk boundaries easy to assert on.
pub fn one_feature_per_chunk() -> SplitterOptions {
    SplitterOptions {
        target_chunk_bytes: 1,
        ..SplitterOptions::default()
    }
}

/// The feature elements of one chunk, parsed from the chunk alone: every chunk
/// must be parseable with only the namespace context it carries.
pub fn features_in_chunk(chunk: &FeatureChunk) -> Vec<QName> {
    let mut reader = GmlReader::new(&chunk.bytes, &chunk.namespaces, chunk.byte_offset);
    let mut stack: Vec<QName> = Vec::new();
    let mut features = Vec::new();
    loop {
        match reader.next_event().expect("a chunk parses on its own") {
            XmlEvent::Start { name, .. } => {
                let is_feature = match stack.last() {
                    None => !is_member_wrapper(&name),
                    Some(parent) => stack.len() == 1 && is_member_wrapper(parent),
                };
                if is_feature {
                    features.push(name.clone());
                }
                stack.push(name);
            }
            XmlEvent::End { .. } => {
                stack.pop();
            }
            XmlEvent::Text(_) => {}
            XmlEvent::Eof => break,
        }
    }
    features
}

/// Feature elements of every chunk, in order.
pub fn features(chunks: &[FeatureChunk]) -> Vec<QName> {
    chunks.iter().flat_map(features_in_chunk).collect()
}

/// Local names of the feature elements, which is what most tests assert on.
pub fn feature_names(chunks: &[FeatureChunk]) -> Vec<String> {
    features(chunks)
        .iter()
        .map(|name| name.local.to_string())
        .collect()
}

fn is_member_wrapper(name: &QName) -> bool {
    let in_gml = matches!(name.ns.as_deref(), Some(ns::GML) | Some(ns::GML_32));
    let in_wfs = matches!(name.ns.as_deref(), Some(ns::WFS) | Some(ns::WFS_20));
    (in_gml && matches!(&*name.local, "featureMember" | "featureMembers"))
        || (in_wfs && matches!(&*name.local, "member" | "featureMember" | "additionalObjects"))
}

/// A namespace context with the prefixes the synthetic documents use.
pub fn app_context(gml_ns: &str) -> NamespaceContext {
    let mut context = NamespaceContext::new();
    context.declare(Some("gml"), gml_ns);
    context.declare(Some("app"), xeibe_testkit::gml::APP);
    context.declare(Some("xlink"), ns::XLINK);
    context.declare(Some("xsi"), ns::XSI);
    context
}

/// A reader over a fragment, with the synthetic documents' prefixes in scope.
pub fn reader<'a>(fragment: &'a [u8], context: &NamespaceContext) -> GmlReader<'a> {
    GmlReader::new(fragment, context, 0)
}

/// Events of a fragment, with whitespace-only text dropped.
pub fn events(fragment: &str, context: &NamespaceContext) -> Vec<Event> {
    let mut reader = GmlReader::new(fragment.as_bytes(), context, 0);
    let mut events = Vec::new();
    loop {
        match reader.next_event().expect("well-formed XML") {
            XmlEvent::Start { name, .. } => events.push(Event::Start(name)),
            XmlEvent::End { name } => events.push(Event::End(name)),
            XmlEvent::Text(text) => {
                if !text.trim().is_empty() {
                    events.push(Event::Text(text.into_owned()));
                }
            }
            XmlEvent::Eof => break,
        }
    }
    events
}

/// An owned copy of [`XmlEvent`], for comparing event sequences.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Start(QName),
    End(QName),
    Text(String),
}

impl Event {
    pub fn start(local: &str) -> Event {
        Event::Start(QName::new(Some(xeibe_testkit::gml::APP), local))
    }

    pub fn end(local: &str) -> Event {
        Event::End(QName::new(Some(xeibe_testkit::gml::APP), local))
    }

    pub fn text(text: &str) -> Event {
        Event::Text(text.to_string())
    }
}

/// A directory for files a test creates (`target/tmp/<name>`).
pub fn temp_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("creating the temporary directory");
    dir
}

/// Read a source to the end, as the splitter would.
pub fn read_to_string(source: &xeibe_core::Source) -> xeibe_core::Result<String> {
    use std::io::Read;
    let mut text = String::new();
    source.open()?.read_to_string(&mut text)?;
    Ok(text)
}
