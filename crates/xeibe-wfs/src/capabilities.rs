//! `GetCapabilities` parsing (WFS 1.0/1.1/2.0), see `docs/wfs.md` "Capabilities".

use serde::{Deserialize, Serialize};

use crate::xml::Element;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum WfsVersion {
    V1_0_0,
    V1_1_0,
    V2_0_0,
    V2_0_2,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    pub version: WfsVersion,
    /// `ServiceIdentification` / `Service` (producer fingerprinting).
    pub service_title: Option<String>,
    pub feature_types: Vec<FeatureTypeInfo>,
    /// The advertised `GetFeature` GET URL; empty if none is advertised.
    pub get_feature_url: String,
    pub output_formats: Vec<String>,
    pub constraints: Constraints,
}

/// Service (Table 13) and operation (Table 14) constraints.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Constraints {
    pub kvp_encoding: Option<bool>,
    pub xml_encoding: Option<bool>,
    pub implements_result_paging: Option<bool>,
    pub count_default: Option<u64>,
    pub paging_is_transaction_safe: Option<bool>,
    pub response_cache_timeout_s: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureTypeInfo {
    /// Prefixed name as advertised, plus its namespace URI.
    pub name: String,
    pub namespace: Option<String>,
    pub title: Option<String>,
    /// `DefaultCRS` (2.0) / `DefaultSRS` (1.1) / `SRS` (1.0).
    pub default_crs: Option<String>,
    pub other_crs: Vec<String>,
    pub output_formats: Vec<String>,
    /// `WGS84BoundingBox` / `LatLongBoundingBox`, lon/lat.
    pub wgs84_bbox: Option<[f64; 4]>,
}

impl WfsVersion {
    /// `"2.0.0"`, `"1.1.0"`, …, as sent in `VERSION`.
    pub fn as_str(self) -> &'static str {
        match self {
            WfsVersion::V1_0_0 => "1.0.0",
            WfsVersion::V1_1_0 => "1.1.0",
            WfsVersion::V2_0_0 => "2.0.0",
            WfsVersion::V2_0_2 => "2.0.2",
        }
    }

    pub fn parse(version: &str) -> Option<Self> {
        match version.trim() {
            "1.0.0" | "1.0" => Some(WfsVersion::V1_0_0),
            "1.1.0" | "1.1" => Some(WfsVersion::V1_1_0),
            "2.0.0" | "2.0" => Some(WfsVersion::V2_0_0),
            "2.0.2" => Some(WfsVersion::V2_0_2),
            _ => None,
        }
    }

    pub fn is_2(self) -> bool {
        self >= WfsVersion::V2_0_0
    }
}

impl std::fmt::Display for WfsVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Capabilities {
    /// An exception report is [`crate::Error::Exception`]; any other document
    /// that is not WFS capabilities is an error too.
    pub fn parse(xml: &[u8]) -> crate::Result<Self> {
        let xml = crate::xml::utf8(xml)?;
        if let Some(report) = crate::exception::ExceptionReport::parse(&xml) {
            return Err(crate::Error::Exception(report));
        }
        let root = crate::xml::parse(&xml)?;
        if !root.is("WFS_Capabilities") {
            return Err(crate::Error::Unsupported(format!(
                "not a WFS capabilities document (root element <{}>)",
                root.local
            )));
        }
        let version = root
            .attr("version")
            .and_then(WfsVersion::parse)
            .or((root.ns.as_deref() == Some(xeibe_core::ns::WFS_20)).then_some(WfsVersion::V2_0_0))
            .ok_or_else(|| {
                crate::Error::Unsupported(format!(
                    "unsupported WFS version {:?}",
                    root.attr("version").unwrap_or("")
                ))
            })?;
        let mut capabilities = if version == WfsVersion::V1_0_0 {
            parse_10(&root)
        } else {
            parse_ows(&root)
        };
        capabilities.version = version;
        capabilities.feature_types = root
            .child("FeatureTypeList")
            .map(|list| {
                list.children_named("FeatureType")
                    .filter_map(feature_type)
                    .collect()
            })
            .unwrap_or_default();
        Ok(capabilities)
    }

    /// By the advertised name; an unprefixed `name` also matches a prefixed type
    /// with that local name.
    pub fn feature_type(&self, name: &str) -> Option<&FeatureTypeInfo> {
        self.feature_types
            .iter()
            .find(|t| t.name == name)
            .or_else(|| {
                if name.contains(':') {
                    return None;
                }
                self.feature_types
                    .iter()
                    .find(|t| t.name.rsplit(':').next() == Some(name))
            })
    }

    /// Best GML output format: GML 3.2 → 3.1.1 → 2.
    ///
    /// The type's own `OutputFormats` if it lists any, otherwise the
    /// `GetFeature` formats. `None` when nothing GML is advertised: the request
    /// then leaves the format to the server's default.
    pub fn preferred_output_format(&self, feature_type: &FeatureTypeInfo) -> Option<String> {
        let formats = if feature_type.output_formats.is_empty() {
            &self.output_formats
        } else {
            &feature_type.output_formats
        };
        formats
            .iter()
            .filter_map(|format| gml_rank(format).map(|rank| (rank, format)))
            .min_by_key(|(rank, _)| *rank)
            .map(|(_, format)| format.clone())
    }
}

/// 0 for GML 3.2, 1 for GML 3.1.1, 2 for GML 2; `None` for anything else.
fn gml_rank(format: &str) -> Option<u8> {
    let format = format
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    // `application/gml+xml` without a version is GML 3.2 in WFS 2.0.
    if format.contains("gml/3.2")
        || format.contains("version=3.2")
        || format.contains("gml32")
        || format == "application/gml+xml"
    {
        Some(0)
    } else if format.contains("gml/3.1")
        || format.contains("version=3.1")
        || format.contains("gml3")
    {
        Some(1)
    } else if format.contains("gml/2") || format.contains("gml2") {
        Some(2)
    } else {
        None
    }
}

/// WFS 1.1 and 2.0: `ows:ServiceIdentification`, `ows:OperationsMetadata`.
fn parse_ows(root: &Element) -> Capabilities {
    let metadata = root.child("OperationsMetadata");
    let get_feature = metadata.and_then(|m| {
        m.children_named("Operation")
            .find(|o| o.attr("name") == Some("GetFeature"))
    });
    let get_feature_url = get_feature
        .and_then(|op| {
            op.children_named("DCP")
                .filter_map(|dcp| dcp.path(&["HTTP", "Get"]))
                .find_map(|get| get.attr("href"))
        })
        .unwrap_or_default()
        .to_string();
    // `AllowedValues/Value` in OWS 1.1 (2.0), bare `Value`s in OWS 1.0 (1.1).
    let output_formats = get_feature
        .and_then(|op| {
            op.children_named("Parameter").find(|p| {
                p.attr("name")
                    .is_some_and(|n| n.eq_ignore_ascii_case("outputFormat"))
            })
        })
        .map(|parameter| {
            let mut values = Vec::new();
            parameter.descendants("Value", &mut values);
            values.iter().map(|v| v.text.clone()).collect()
        })
        .unwrap_or_default();
    // An operation constraint wins over a service constraint of the same name;
    // servers put `CountDefault` in either place.
    let constraint = |name: &str| {
        let find = |parent: Option<&Element>| {
            parent?
                .children_named("Constraint")
                .find(|c| c.attr("name").is_some_and(|n| n.eq_ignore_ascii_case(name)))
                .and_then(constraint_value)
        };
        find(get_feature).or_else(|| find(metadata))
    };
    let flag = |name: &str| constraint(name).and_then(|v| parse_bool(&v));
    let number = |name: &str| constraint(name).and_then(|v| v.trim().parse().ok());
    Capabilities {
        version: WfsVersion::V2_0_0,
        service_title: root
            .child("ServiceIdentification")
            .and_then(|s| s.child_text("Title")),
        feature_types: Vec::new(),
        get_feature_url,
        output_formats,
        constraints: Constraints {
            kvp_encoding: flag("KVPEncoding"),
            xml_encoding: flag("XMLEncoding"),
            implements_result_paging: flag("ImplementsResultPaging"),
            count_default: number("CountDefault"),
            paging_is_transaction_safe: flag("PagingIsTransactionSafe"),
            response_cache_timeout_s: number("ResponseCacheTimeout"),
        },
    }
}

/// WFS 1.0: `Service`, `Capability/Request/GetFeature`.
fn parse_10(root: &Element) -> Capabilities {
    let get_feature = root.path(&["Capability", "Request", "GetFeature"]);
    let get_feature_url = get_feature
        .and_then(|op| {
            op.children_named("DCPType")
                .filter_map(|dcp| dcp.path(&["HTTP", "Get"]))
                .find_map(|get| get.attr("onlineResource"))
        })
        .unwrap_or_default()
        .to_string();
    // Formats are empty elements: `<ResultFormat><GML2/></ResultFormat>`.
    let output_formats = get_feature
        .and_then(|op| op.child("ResultFormat"))
        .map(|formats| formats.children.iter().map(|f| f.local.clone()).collect())
        .unwrap_or_default();
    Capabilities {
        version: WfsVersion::V1_0_0,
        service_title: root.child("Service").and_then(|s| s.child_text("Title")),
        feature_types: Vec::new(),
        get_feature_url,
        output_formats,
        constraints: Constraints::default(),
    }
}

/// `DefaultValue` (OWS 1.1), else the first `Value`.
fn constraint_value(constraint: &Element) -> Option<String> {
    constraint.child_text("DefaultValue").or_else(|| {
        let mut values = Vec::new();
        constraint.descendants("Value", &mut values);
        values.first().map(|v| v.text.clone())
    })
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

fn feature_type(element: &Element) -> Option<FeatureTypeInfo> {
    let name = element.child("Name").filter(|n| !n.text.is_empty())?;
    let texts = |locals: &[&str]| -> Vec<String> {
        element
            .children
            .iter()
            .filter(|c| locals.contains(&c.local.as_str()) && !c.text.is_empty())
            .map(|c| c.text.clone())
            .collect()
    };
    Some(FeatureTypeInfo {
        name: name.text.clone(),
        namespace: name.text_ns.clone(),
        title: element.child_text("Title"),
        default_crs: texts(&["DefaultCRS", "DefaultSRS", "SRS"])
            .into_iter()
            .next(),
        other_crs: texts(&["OtherCRS", "OtherSRS"]),
        output_formats: element
            .child("OutputFormats")
            .map(|formats| {
                formats
                    .children_named("Format")
                    .map(|f| f.text.clone())
                    .filter(|f| !f.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        wgs84_bbox: wgs84_bbox(element),
    })
}

/// `ows:WGS84BoundingBox` (1.1/2.0, "lon lat" corners) or `LatLongBoundingBox`
/// (1.0, attributes); the first one if a type lists several.
fn wgs84_bbox(element: &Element) -> Option<[f64; 4]> {
    if let Some(bbox) = element.child("WGS84BoundingBox") {
        let corner = |local: &str| -> Option<[f64; 2]> {
            let text = bbox.child_text(local)?;
            let mut values = text.split_whitespace().map(|v| v.parse::<f64>().ok());
            let corner = [values.next()??, values.next()??];
            values.next().is_none().then_some(corner)
        };
        let (lower, upper) = (corner("LowerCorner")?, corner("UpperCorner")?);
        return Some([lower[0], lower[1], upper[0], upper[1]]);
    }
    let bbox = element.child("LatLongBoundingBox")?;
    let value = |name: &str| bbox.attr(name)?.trim().parse::<f64>().ok();
    Some([
        value("minx")?,
        value("miny")?,
        value("maxx")?,
        value("maxy")?,
    ])
}
