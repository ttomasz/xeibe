//! GDAL 3.13 reference output.
//!
//! GDAL is a reference, not ground truth: it reads 4 of the 17 samples with the
//! wrong axis order and misreads the 3D NGI line (see `tests/data/BOM.md` and
//! `docs/geometry.md`). Tests say where they deviate on purpose.

use serde::Deserialize;

use crate::{read_data, read_data_str};

// ------------------------------------------------- ogrinfo -al reference files

#[derive(Debug, Clone)]
pub struct GdalReport {
    pub layers: Vec<GdalLayer>,
}

#[derive(Debug, Clone, Default)]
pub struct GdalLayer {
    pub name: String,
    /// `Point`, `Curve Polygon`, `None`, …
    pub geometry_type: String,
    pub feature_count: u64,
    /// The geometry column's name, if the layer has one.
    pub geometry_column: Option<String>,
    /// `(name, type)` as `ogrinfo` prints the field definitions.
    pub fields: Vec<(String, String)>,
    pub features: Vec<GdalFeature>,
}

#[derive(Debug, Clone, Default)]
pub struct GdalFeature {
    pub fid: u64,
    /// `(name, type, value)`.
    pub attrs: Vec<(String, String, String)>,
    /// WKT, absent for a feature without geometry.
    pub wkt: Option<String>,
}

impl GdalReport {
    /// Parse the output of `ogrinfo -al`.
    pub fn parse(text: &str) -> GdalReport {
        let mut layers: Vec<GdalLayer> = Vec::new();
        // The `Layer SRS WKT:` block is skipped by following its brackets.
        let mut in_srs_wkt = false;
        let mut srs_depth: i32 = 0;
        for line in text.lines() {
            if let Some(name) = line.strip_prefix("Layer name: ") {
                layers.push(GdalLayer {
                    name: name.trim().to_string(),
                    ..Default::default()
                });
                in_srs_wkt = false;
                continue;
            }
            let Some(layer) = layers.last_mut() else {
                continue;
            };
            if line.starts_with("Layer SRS WKT:") {
                in_srs_wkt = true;
                srs_depth = 0;
                continue;
            }
            if in_srs_wkt {
                if line.trim() == "(unknown)" {
                    in_srs_wkt = false;
                } else {
                    srs_depth += line.matches('[').count() as i32;
                    srs_depth -= line.matches(']').count() as i32;
                    if srs_depth <= 0 {
                        in_srs_wkt = false;
                    }
                }
                continue;
            }
            if let Some(value) = line.strip_prefix("Geometry: ") {
                layer.geometry_type = value.trim().to_string();
            } else if let Some(value) = line.strip_prefix("Feature Count: ") {
                layer.feature_count = value.trim().parse().unwrap_or(0);
            } else if let Some(value) = line.strip_prefix("Geometry Column = ") {
                layer.geometry_column = Some(value.trim().to_string());
            } else if line.starts_with("OGRFeature(") {
                let fid = line
                    .rsplit(':')
                    .next()
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(0);
                layer.features.push(GdalFeature {
                    fid,
                    ..Default::default()
                });
            } else if let Some(rest) = line.strip_prefix("  ") {
                let Some(feature) = layer.features.last_mut() else {
                    continue;
                };
                if let Some((name, kind, value)) = parse_attribute(rest) {
                    feature.attrs.push((name, kind, value));
                } else if is_wkt(rest) {
                    feature.wkt = Some(rest.trim().to_string());
                }
            } else if let Some(field) = parse_field_definition(line) {
                layer.fields.push(field);
            }
        }
        GdalReport { layers }
    }

    pub fn layer(&self, name: &str) -> Option<&GdalLayer> {
        self.layers.iter().find(|l| l.name == name)
    }

    pub fn total_features(&self) -> u64 {
        self.layers.iter().map(|l| l.feature_count).sum()
    }
}

impl GdalLayer {
    /// The value of one attribute of one feature, as `ogrinfo` printed it.
    pub fn attr(&self, feature: usize, name: &str) -> Option<&str> {
        self.features.get(feature)?.attrs.iter().find_map(|(n, _, v)| {
            (n == name).then_some(v.as_str())
        })
    }
}

/// `  gml_id (String) = PL.ZIPIN…`
fn parse_attribute(line: &str) -> Option<(String, String, String)> {
    let (name, rest) = line.split_once(" (")?;
    let (kind, value) = rest.split_once(") = ")?;
    if name.is_empty() || name.contains(' ') || !kind.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    Some((name.to_string(), kind.to_string(), value.to_string()))
}

/// `gml_id: String (0.0) NOT NULL`
fn parse_field_definition(line: &str) -> Option<(String, String)> {
    if line.starts_with(' ') || line.starts_with('(') {
        return None;
    }
    let (name, rest) = line.split_once(": ")?;
    if name.is_empty() || name.contains(' ') || name.contains('[') {
        return None;
    }
    let kind = rest.split(" (").next().unwrap_or(rest).trim();
    if kind.is_empty() || !kind.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    Some((name.to_string(), kind.to_string()))
}

fn is_wkt(line: &str) -> bool {
    let line = line.trim();
    let tag: String = line.chars().take_while(|c| c.is_ascii_uppercase()).collect();
    !tag.is_empty() && line[tag.len()..].trim_start().starts_with(['(', 'E', 'Z', 'M'])
}

// ------------------------------------------- GML geometry snippets from GDAL

/// One snippet of `autotest/ogr/ogr_gml_geom.py` with GDAL 3.13.3's result.
#[derive(Debug, Clone, Deserialize)]
pub struct GeometryCase {
    /// e.g. `ogr_gml_geom:74`.
    pub id: String,
    /// Where the snippet comes from, with a line number.
    pub source: String,
    pub gml: String,
    pub gdal: GdalGeometryResult,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GdalGeometryResult {
    /// GDAL's WKT, or `None` when GDAL produced no geometry.
    pub wkt: Option<String>,
    /// GDAL's message, `"null geometry"` when it returned nothing.
    pub error: Option<String>,
    /// GDAL's geometry-type name, when it produced one.
    #[serde(default, rename = "type")]
    pub geometry_type: Option<String>,
}

/// The 292 GDAL geometry snippets (`tests/data/gdal/gml_geometry_cases.jsonl`).
pub fn geometry_cases() -> Vec<GeometryCase> {
    read_data_str("gdal/gml_geometry_cases.jsonl")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("valid case"))
        .collect()
}

/// Parse a `*.gdal.txt` file below `tests/data`.
pub fn report(relative: &str) -> GdalReport {
    GdalReport::parse(&String::from_utf8_lossy(&read_data(relative)))
}
