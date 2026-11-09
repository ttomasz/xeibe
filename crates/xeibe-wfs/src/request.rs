//! Request building: KVP for `GET` and XML for `POST` (see `docs/wfs.md`,
//! "Request encoding (KVP)" and "Request encoding (XML POST)").

use quick_xml::escape::escape;
use quick_xml::events::Event;
use url::Url;

use crate::{Error, WfsVersion};

/// Longest `GetFeature` URL [`crate::HttpMethod::Auto`] sends as a `GET`;
/// a longer request is an XML `POST`. ArcGIS Server fails on URLs from about
/// 2,000 bytes with a misleading 500 ("Xml Parsing failed") rather than a 414,
/// so the limit stays under that, with room for `COUNT` and `STARTINDEX`.
/// A `next` link longer than this isn't followed during a `POST` read.
pub const MAX_GET_URL_LENGTH: usize = 1800;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultType {
    Results,
    Hits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    Asc,
    Desc,
}

/// Sort keys as `SORTBY` takes them: `property [ASC|DESC]`, comma-separated,
/// e.g. `name DESC,id`. The order defaults to ascending; 1.1's `A` and `D`
/// are read too, in any case. The property is passed on as written (`app:id`,
/// `app:address/app:street`).
pub fn parse_sort_by(text: &str) -> Result<Vec<(String, SortOrder)>, String> {
    text.split(',')
        .map(|key| {
            let mut words = key.split_whitespace();
            let property = words
                .next()
                .ok_or_else(|| format!("{text:?}: an empty sort key"))?;
            let order = match words.next().map(str::to_ascii_uppercase).as_deref() {
                None | Some("ASC" | "A") => SortOrder::Asc,
                Some("DESC" | "D") => SortOrder::Desc,
                Some(other) => {
                    return Err(format!(
                        "{key:?}: the order is ASC or DESC, not {other:?}",
                        key = key.trim()
                    ));
                }
            };
            if let Some(extra) = words.next() {
                return Err(format!(
                    "{key:?}: expected `property [ASC|DESC]`, found {extra:?} after the order",
                    key = key.trim()
                ));
            }
            Ok((property.to_string(), order))
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct GetFeature {
    pub version: WfsVersion,
    pub type_name: String,
    /// `(prefix, uri)` for `NAMESPACES` (2.0) / `NAMESPACE` (1.1).
    pub namespace: Option<(String, String)>,
    pub srs_name: Option<String>,
    pub output_format: Option<String>,
    /// In the axis order assumed for `srs_name`; optional CRS as 5th value.
    pub bbox: Option<([f64; 4], Option<String>)>,
    /// FES/OGC filter XML, passed through.
    pub filter: Option<String>,
    pub property_names: Vec<String>,
    pub sort_by: Vec<(String, SortOrder)>,
    pub result_type: ResultType,
    /// `COUNT` (2.0) / `MAXFEATURES` (1.x).
    pub count: Option<u64>,
    /// `STARTINDEX` (2.0, or vendor on 1.x).
    pub start_index: Option<u64>,
    /// Vendor parameters, e.g. `CQL_FILTER`.
    pub vendor: Vec<(String, String)>,
}

impl GetFeature {
    /// `base` with the request's KVP parameters. Parameters already in `base`
    /// (`map=…`) are kept unless the request sets them itself.
    pub fn to_url(&self, base: &Url) -> crate::Result<Url> {
        let v2 = self.version.is_2();
        let v1_0 = self.version == WfsVersion::V1_0_0;
        let mut params: Vec<(&str, String)> = vec![
            ("SERVICE", "WFS".into()),
            ("VERSION", self.version.as_str().into()),
            ("REQUEST", "GetFeature".into()),
            (
                if v2 { "TYPENAMES" } else { "TYPENAME" },
                self.type_name.clone(),
            ),
        ];
        if let Some((prefix, uri)) = &self.namespace {
            if v2 {
                params.push(("NAMESPACES", format!("xmlns({prefix},{uri})")));
            } else if !v1_0 {
                params.push(("NAMESPACE", format!("xmlns({prefix}={uri})")));
            }
        }
        if let Some(srs_name) = &self.srs_name {
            params.push(("SRSNAME", srs_name.clone()));
        }
        if let Some(format) = &self.output_format {
            params.push(("OUTPUTFORMAT", format.clone()));
        }
        if let Some((bbox, crs)) = &self.bbox {
            let mut value = bbox.map(|v| v.to_string()).join(",");
            if let Some(crs) = crs {
                value.push(',');
                value.push_str(crs);
            }
            params.push(("BBOX", value));
        }
        if let Some(filter) = &self.filter {
            params.push(("FILTER", filter.clone()));
        }
        if !self.property_names.is_empty() {
            params.push(("PROPERTYNAME", self.property_names.join(",")));
        }
        // 1.0 has no SORTBY and no RESULTTYPE.
        if !self.sort_by.is_empty() && !v1_0 {
            let value = self
                .sort_by
                .iter()
                .map(|(property, order)| {
                    let order = match (order, v2) {
                        (SortOrder::Asc, true) => "ASC",
                        (SortOrder::Desc, true) => "DESC",
                        (SortOrder::Asc, false) => "A",
                        (SortOrder::Desc, false) => "D",
                    };
                    format!("{property} {order}")
                })
                .collect::<Vec<_>>()
                .join(",");
            params.push(("SORTBY", value));
        }
        if self.result_type == ResultType::Hits && !v1_0 {
            params.push(("RESULTTYPE", "hits".into()));
        }
        if let Some(count) = self.count {
            params.push((if v2 { "COUNT" } else { "MAXFEATURES" }, count.to_string()));
        }
        if let Some(start_index) = self.start_index {
            params.push(("STARTINDEX", start_index.to_string()));
        }
        for (key, value) in &self.vendor {
            params.push((key, value.clone()));
        }
        Ok(with_params(base, &params))
    }

    /// The request as an XML document, the body of a `POST` to [`post_url`]
    /// with `Content-Type: text/xml` (WFS 2.0.2 Table D.1, 04-094 Table 2b).
    ///
    /// WFS 2.0 and 1.1 only. A bbox and vendor parameters can't be sent: XML
    /// has no `BBOX` parameter, and servers ignore query parameters next to
    /// an XML body (GeoServer does). The filter is put into the query as
    /// written, without its XML declaration.
    pub fn to_xml(&self) -> crate::Result<String> {
        if self.version == WfsVersion::V1_0_0 {
            return Err(Error::Unsupported(
                "WFS 1.0 requests are only sent by GET; use WFS 1.1 or 2.0 to send them by POST"
                    .into(),
            ));
        }
        if self.bbox.is_some() {
            return Err(Error::Unsupported(
                "a bbox is only sent by GET; use GET, or a filter with a BBOX operator".into(),
            ));
        }
        if !self.vendor.is_empty() {
            let names: Vec<&str> = self.vendor.iter().map(|(key, _)| key.as_str()).collect();
            return Err(Error::Unsupported(format!(
                "vendor parameters ({}) are only sent by GET: servers ignore query parameters next to an XML request",
                names.join(", ")
            )));
        }
        let v2 = self.version.is_2();
        let (wfs, filter_prefix, filter_ns) = if v2 {
            (
                "http://www.opengis.net/wfs/2.0",
                "fes",
                "http://www.opengis.net/fes/2.0",
            )
        } else {
            (
                "http://www.opengis.net/wfs",
                "ogc",
                "http://www.opengis.net/ogc",
            )
        };
        let mut xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><wfs:GetFeature xmlns:wfs="{wfs}" xmlns:{filter_prefix}="{filter_ns}" service="WFS" version="{}""#,
            self.version.as_str()
        );
        if let Some(count) = self.count {
            let name = if v2 { "count" } else { "maxFeatures" };
            xml.push_str(&format!(r#" {name}="{count}""#));
        }
        // A vendor attribute on 1.1 (GeoServer, ArcGIS); 2.0's own.
        if let Some(start_index) = self.start_index {
            xml.push_str(&format!(r#" startIndex="{start_index}""#));
        }
        if self.result_type == ResultType::Hits {
            xml.push_str(r#" resultType="hits""#);
        }
        if let Some(format) = &self.output_format {
            xml.push_str(&format!(r#" outputFormat="{}""#, escape(format)));
        }
        xml.push_str(&format!(
            r#"><wfs:Query {}="{}""#,
            if v2 { "typeNames" } else { "typeName" },
            escape(&self.type_name)
        ));
        if let Some(srs_name) = &self.srs_name {
            xml.push_str(&format!(r#" srsName="{}""#, escape(srs_name)));
        }
        if let Some((prefix, uri)) = &self.namespace {
            xml.push_str(&format!(r#" xmlns:{prefix}="{}""#, escape(uri)));
        }
        xml.push('>');
        // `wfs:PropertyName` in 1.1 too (04-094 `QueryType`), not `ogc:`.
        for property in &self.property_names {
            xml.push_str(&format!(
                "<wfs:PropertyName>{}</wfs:PropertyName>",
                escape(property)
            ));
        }
        if let Some(filter) = &self.filter {
            xml.push_str(filter_element(filter)?);
        }
        if !self.sort_by.is_empty() {
            let p = filter_prefix;
            let reference = if v2 { "ValueReference" } else { "PropertyName" };
            xml.push_str(&format!("<{p}:SortBy>"));
            for (property, order) in &self.sort_by {
                // `ASC`/`DESC` in both versions: 1.1's `A`/`D` are KVP only.
                let order = match order {
                    SortOrder::Asc => "ASC",
                    SortOrder::Desc => "DESC",
                };
                xml.push_str(&format!(
                    "<{p}:SortProperty><{p}:{reference}>{}</{p}:{reference}><{p}:SortOrder>{order}</{p}:SortOrder></{p}:SortProperty>",
                    escape(property)
                ));
            }
            xml.push_str(&format!("</{p}:SortBy>"));
        }
        xml.push_str("</wfs:Query></wfs:GetFeature>");
        Ok(xml)
    }
}

/// The filter's root element as written, to put into an XML request: an XML
/// declaration, comments and whitespace around it are left out. A filter that
/// is not one well-formed element, or that has a DTD, is an error.
fn filter_element(filter: &str) -> crate::Result<&str> {
    let filter = filter.trim_start_matches('\u{feff}');
    let invalid = |message: String| Error::Filter(message);
    let mut reader = quick_xml::Reader::from_str(filter);
    let mut depth = 0usize;
    let mut element: Option<(usize, Option<usize>)> = None;
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader
            .read_event()
            .map_err(|e| invalid(format!("not well-formed XML: {e}")))?;
        let after = reader.buffer_position() as usize;
        let outside = depth == 0;
        match event {
            Event::Start(_) | Event::Empty(_) if outside && element.is_some() => {
                return Err(invalid(
                    "more than one element; a filter is one Filter element".into(),
                ));
            }
            Event::Start(_) => {
                if outside {
                    element = Some((before, None));
                }
                depth += 1;
            }
            Event::Empty(_) if outside => element = Some((before, Some(after))),
            Event::End(_) => {
                depth -= 1;
                if depth == 0
                    && let Some((_, end)) = &mut element
                {
                    *end = Some(after);
                }
            }
            Event::DocType(_) => {
                return Err(invalid("a DTD (<!DOCTYPE …>) is not allowed".into()));
            }
            Event::Text(text) if outside && !text.trim_ascii().is_empty() => {
                return Err(invalid("text outside the Filter element".into()));
            }
            Event::CData(_) | Event::GeneralRef(_) if outside => {
                return Err(invalid("text outside the Filter element".into()));
            }
            Event::Eof => break,
            _ => {}
        }
    }
    match element {
        Some((start, Some(end))) => Ok(&filter[start..end]),
        Some((_, None)) => Err(invalid(
            "not well-formed XML: the Filter element is not closed".into(),
        )),
        None => Err(invalid(
            "no XML element; a filter is one Filter element".into(),
        )),
    }
}

/// Where an XML `POST` goes: `base` without the parameters of WFS requests
/// (`SERVICE`, `REQUEST`, `TYPENAMES`, `FILTER`, …), whatever their case. A
/// server that finds them in the query string reads the request from there
/// and ignores the body (MapServer does). Other parameters, such as
/// MapServer's `map=`, are kept.
pub fn post_url(base: &Url) -> Url {
    const WFS_PARAMETERS: &[&str] = &[
        "SERVICE",
        "VERSION",
        "REQUEST",
        "ACCEPTVERSIONS",
        "SECTIONS",
        "TYPENAME",
        "TYPENAMES",
        "NAMESPACE",
        "NAMESPACES",
        "COUNT",
        "MAXFEATURES",
        "STARTINDEX",
        "RESULTTYPE",
        "SRSNAME",
        "OUTPUTFORMAT",
        "BBOX",
        "FILTER",
        "PROPERTYNAME",
        "SORTBY",
        "RESOURCEID",
        "FEATUREID",
    ];
    let kept: Vec<(String, String)> = base
        .query_pairs()
        .filter(|(key, _)| !WFS_PARAMETERS.iter().any(|k| k.eq_ignore_ascii_case(key)))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    let mut url = base.clone();
    url.set_query(None);
    if !kept.is_empty() {
        url.query_pairs_mut().extend_pairs(&kept);
    }
    url
}

/// `DescribeFeatureType` for one type, in the server's default schema
/// language (an XSD). `namespace` is `(prefix, uri)`, as for `GetFeature`.
pub fn describe_feature_type_url(
    base: &Url,
    version: WfsVersion,
    type_name: &str,
    namespace: Option<&(String, String)>,
) -> crate::Result<Url> {
    let v2 = version.is_2();
    let mut params: Vec<(&str, String)> = vec![
        ("SERVICE", "WFS".into()),
        ("VERSION", version.as_str().into()),
        ("REQUEST", "DescribeFeatureType".into()),
        (if v2 { "TYPENAMES" } else { "TYPENAME" }, type_name.into()),
    ];
    if let Some((prefix, uri)) = namespace {
        if v2 {
            params.push(("NAMESPACES", format!("xmlns({prefix},{uri})")));
        } else if version != WfsVersion::V1_0_0 {
            params.push(("NAMESPACE", format!("xmlns({prefix}={uri})")));
        }
    }
    Ok(with_params(base, &params))
}

pub fn get_capabilities_url(base: &Url, version: Option<WfsVersion>) -> crate::Result<Url> {
    let mut params = vec![("SERVICE", "WFS".to_string())];
    // Without a version the server answers with its highest one.
    if let Some(version) = version {
        params.push(("VERSION", version.as_str().to_string()));
    }
    params.push(("REQUEST", "GetCapabilities".to_string()));
    Ok(with_params(base, &params))
}

/// `base` with `params` appended. A parameter of `base` with the same name
/// (in any case: KVP names are case-insensitive) is dropped, and so are
/// the parameters that belong to another request.
fn with_params(base: &Url, params: &[(&str, String)]) -> Url {
    const REQUEST_SPECIFIC: &[&str] = &[
        "ACCEPTVERSIONS",
        "SECTIONS",
        "TYPENAME",
        "TYPENAMES",
        "NAMESPACE",
        "NAMESPACES",
        "COUNT",
        "MAXFEATURES",
        "STARTINDEX",
        "RESULTTYPE",
    ];
    let kept: Vec<(String, String)> = base
        .query_pairs()
        .filter(|(key, _)| {
            !params.iter().any(|(k, _)| k.eq_ignore_ascii_case(key))
                && !REQUEST_SPECIFIC.iter().any(|k| k.eq_ignore_ascii_case(key))
        })
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    let mut url = base.clone();
    url.set_query(None);
    {
        let mut query = url.query_pairs_mut();
        for (key, value) in &kept {
            query.append_pair(key, value);
        }
        for (key, value) in params {
            query.append_pair(key, value);
        }
    }
    url
}
