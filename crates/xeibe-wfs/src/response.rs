//! Response-level facts read from the collection root (without full parsing).

use xeibe_core::reader::{GmlReader, XmlEvent};
use xeibe_core::{NamespaceContext, QName, ns};

use crate::exception::ExceptionReport;

#[derive(Debug, Clone, Default)]
pub struct ResponseInfo {
    /// `numberMatched` (2.0); `None` for "unknown".
    pub number_matched: Option<u64>,
    /// `numberReturned` (2.0) / `numberOfFeatures` (1.1).
    pub number_returned: Option<u64>,
    pub next: Option<String>,
    pub previous: Option<String>,
    pub time_stamp: Option<String>,
    /// Inner `wfs:FeatureCollection`s (multi-query responses).
    pub nested_collections: u32,
    /// `wfs:member xlink:href` (referenced members).
    pub referenced_members: u64,
    pub truncated: bool,
    /// Closing collection element present (complete page).
    pub complete: bool,
    /// Features counted in the members, nested collections included (1.0
    /// responses carry no count of their own).
    pub features: u64,
    /// `gml:id` (or GML 2 `fid`) of each counted feature that has one, in
    /// document order, for the duplicate check across pages.
    pub feature_ids: Vec<String>,
}

impl ResponseInfo {
    /// Features in this response: the server's count, else the counted members.
    pub fn returned(&self) -> u64 {
        self.number_returned.unwrap_or(self.features)
    }
}

pub enum Response {
    Features(ResponseInfo),
    Exception(ExceptionReport),
}

/// Inspect a fetched page (start and end of the document).
///
/// The root's attributes give the counts and links. The members are walked
/// without parsing the features in them (each feature is skipped as a
/// subtree) to count features, nested collections and references, and to see
/// whether the document ends. A page that breaks off is `complete: false`
/// rather than an error; a document that ends but is malformed is an error.
pub fn inspect(page: &[u8]) -> crate::Result<Response> {
    if let Some(report) = ExceptionReport::parse(page) {
        return Ok(Response::Exception(report));
    }
    let page = crate::xml::utf8(page)?;
    let mut reader = GmlReader::new(&page, &NamespaceContext::new(), 0);
    let mut info = ResponseInfo::default();
    // The root, with its attributes.
    let root = loop {
        match reader.next_event()? {
            XmlEvent::Start { name, attrs } => {
                for (attr, value) in attrs.iter() {
                    if attr.ns.is_some() {
                        continue;
                    }
                    let value = value.trim();
                    match &*attr.local {
                        "numberMatched" => info.number_matched = value.parse().ok(),
                        "numberReturned" | "numberOfFeatures" => {
                            info.number_returned = value.parse().ok();
                        }
                        "next" if !value.is_empty() => info.next = Some(value.to_string()),
                        "previous" if !value.is_empty() => info.previous = Some(value.to_string()),
                        "timeStamp" => info.time_stamp = Some(value.to_string()),
                        _ => {}
                    }
                }
                break name;
            }
            XmlEvent::Eof => {
                return Err(xeibe_core::Error::NoFeatures("an empty WFS response".into()).into());
            }
            _ => {}
        }
    };
    let walked = if is_collection(&root) {
        walk_collection(&mut reader, &mut info)
    } else {
        // A bare feature as the document (`GetFeatureById`).
        info.features = 1;
        reader.skip_element()
    };
    match walked {
        Ok(()) => info.complete = true,
        Err(error) if ends_with_end_tag(&page, &root.local) => return Err(error.into()),
        Err(_) => info.complete = false,
    }
    Ok(Response::Features(info))
}

/// The children of a collection, up to and including its end tag.
fn walk_collection(reader: &mut GmlReader<'_>, info: &mut ResponseInfo) -> xeibe_core::Result<()> {
    loop {
        match reader.next_event()? {
            XmlEvent::Start { name, attrs } => {
                if is_wfs(&name) && &*name.local == "truncatedResponse" {
                    info.truncated = true;
                    reader.skip_element()?;
                } else if is_member(&name) {
                    let href = attrs.get(&QName::new(Some(ns::XLINK), "href"));
                    if href.is_some() {
                        info.referenced_members += 1;
                    }
                    walk_member(reader, info)?;
                } else {
                    // `boundedBy`, `additionalObjects`, …
                    reader.skip_element()?;
                }
            }
            XmlEvent::End { .. } => return Ok(()),
            XmlEvent::Text(_) => {}
            XmlEvent::Eof => return Err(eof(reader)),
        }
    }
}

/// The content of a member element: features, or nested collections.
fn walk_member(reader: &mut GmlReader<'_>, info: &mut ResponseInfo) -> xeibe_core::Result<()> {
    loop {
        match reader.next_event()? {
            XmlEvent::Start { name, attrs } => {
                if is_collection(&name) {
                    info.nested_collections += 1;
                    walk_collection(reader, info)?;
                } else {
                    let id = attrs
                        .iter()
                        .find(|(attr, _)| {
                            (attr.is_gml() && &*attr.local == "id")
                                || (attr.ns.is_none() && &*attr.local == "fid")
                        })
                        .map(|(_, value)| value.into_owned());
                    info.features += 1;
                    info.feature_ids.extend(id);
                    reader.skip_element()?;
                }
            }
            XmlEvent::End { .. } => return Ok(()),
            XmlEvent::Text(_) => {}
            XmlEvent::Eof => return Err(eof(reader)),
        }
    }
}

fn eof(reader: &GmlReader<'_>) -> xeibe_core::Error {
    xeibe_core::Error::Xml {
        location: reader.location(),
        message: "unexpected end of input".into(),
    }
}

fn is_wfs(name: &QName) -> bool {
    matches!(name.ns.as_deref(), Some(ns::WFS) | Some(ns::WFS_20))
}

/// `wfs:FeatureCollection` (1.x, 2.0) or `gml:FeatureCollection`.
fn is_collection(name: &QName) -> bool {
    &*name.local == "FeatureCollection" && (is_wfs(name) || name.is_gml())
}

/// `wfs:member` (2.0), `gml:featureMember` / `gml:featureMembers` (1.x).
fn is_member(name: &QName) -> bool {
    (is_wfs(name) && &*name.local == "member")
        || name.is_gml_named("featureMember")
        || name.is_gml_named("featureMembers")
}

/// `true` if the document's last tag closes an element named `local` (any
/// prefix): the document ends, so an error in it is not a cut-off.
fn ends_with_end_tag(page: &[u8], local: &str) -> bool {
    let page = page.trim_ascii_end();
    let Some(start) = page.windows(2).rposition(|w| w == b"</") else {
        return false;
    };
    let Some(tag) = page[start + 2..].strip_suffix(b">") else {
        return false;
    };
    let tag = tag.trim_ascii_end();
    let name = tag.rsplit(|&b| b == b':').next().unwrap_or(tag);
    name == local.as_bytes()
}
