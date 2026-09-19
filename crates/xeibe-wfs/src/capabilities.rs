//! `GetCapabilities` parsing (WFS 1.0/1.1/2.0), see `docs/wfs.md` "Capabilities".

use serde::{Deserialize, Serialize};

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

impl Capabilities {
    pub fn parse(xml: &[u8]) -> crate::Result<Self> {
        todo!()
    }

    pub fn feature_type(&self, name: &str) -> Option<&FeatureTypeInfo> {
        todo!()
    }

    /// Best GML output format: GML 3.2 → 3.1.1 → 2.
    pub fn preferred_output_format(&self, feature_type: &FeatureTypeInfo) -> Option<String> {
        todo!()
    }
}
